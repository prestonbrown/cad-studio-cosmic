//! The session turn state machine: one [`Session::run_turn`] call spawns one
//! agent process, translates its stream-json stdout into [`EngineEvent`]s,
//! tees every raw line to a project log, persists the session id the moment it
//! is learned, and classifies the terminal outcome — transcribed from the
//! vendored AgentSession.swift against the probe's one-content-block event
//! shapes.
//!
//! Event-translation semantics (AgentSession.swift:104-145, 225-262):
//! - `TextStreaming` carries the CURRENT text block's accumulated text; the
//!   accumulation resets at every `content_block_start`, matching upstream's
//!   `streamingID = nil` on block start.
//! - `TextConfirmed` carries one assistant text block verbatim. Reconciling it
//!   with the unconfirmed streaming row (replace the FIRST unconfirmed row,
//!   FIFO — upstream `unconfirmedTextIDs.removeFirst()`) is chat state, so it
//!   belongs to the app. A synthetic API-error assistant block is a text block
//!   like any other and confirms the same way; whether to display it is M4's
//!   call, not the engine's.
//! - Tool rows are keyed by the `tool_use` id present at
//!   `content_block_start`; the assistant snapshot that repeats the block
//!   carries the full input, which upstream uses only to refine the row's
//!   title/preview — the engine has no event for that refinement, so it is a
//!   no-op here. Every `input_json_delta` is forwarded as [`EngineEvent::ToolInput`];
//!   throttling is the app's job (upstream does it at 0.15 s).
//! - Thinking surfaces through the `system/thinking_tokens` estimate, not
//!   `thinking_delta`.
//! - The session id is captured from `system/init` OR `result` and persisted
//!   immediately (upstream `project.save()` on both).
//!
//! Outcome classification (AgentSession.swift:264-303): `Success` only when
//! the result line says `is_error: false`; its text is the last confirmed
//! assistant text, or — only when no text was confirmed this turn — the
//! result text (upstream `finish()`'s `hasText` rule). Failures join the
//! result text, the FULL stderr, and the full stdout when the process exited
//! nonzero. Cancellation is requested-side, not exit-side: a turn whose
//! handle was cancelled ends [`TurnOutcome::Cancelled`] whatever exit code
//! the child reports (upstream `wasInterrupted` is `Task.isCancelled`).
//!
//! The busy/missing/auth retry dance around single turns is the next M2 task;
//! this module only exposes the case-insensitive classifiers it will need.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use time::macros::format_description;
use time::OffsetDateTime;
use tokio::sync::{mpsc, watch};

use crate::agent::claude::{
    PromptCtx, TurnRequest, build_args, build_prompt, environment_bullets, render_system_prompt,
};
use crate::agent::locator::{self, AgentKind, AgentPath, shell_env};
use crate::agent::stream_event::{
    ContentBlock, Delta, InnerEvent, ResultEvent, StreamEvent, SystemKind, ToolResultContent,
    parse_line,
};
use crate::agent::tool_summary::{live_title, summarize};
use crate::parsing::threemf::parse_3mf;
use crate::processes::{Line, RunningProcess, spawn_streaming};
use crate::project::manifest::Manifest;
use crate::project::outputs::MissingCtx;
use crate::project::store::{ProjectMetadata, load_metadata, save_metadata};
use crate::settings::Settings;

/// Names an executable to spawn instead of discovering one — the test and
/// embedder seam (same pattern as the registry override in `processes`). The
/// path is never `--version`-validated: a scripted fake reads its scenario
/// from the environment and would treat a probe invocation as a turn.
const TEST_CLAUDE_ENV: &str = "CAD_TEST_CLAUDE";

/// The python line's display text until M5 verifies the project venv.
const PYTHON_UNVERIFIED: &str = "python interpreter not verified yet";

/// The log-name timestamp (AgentSession.swift:325-333). Upstream formats local
/// time; the port pins UTC because a process-local offset is unavailable once
/// threads exist — the name only needs uniqueness and sortability. Appending
/// on a same-second name collision keeps both turns' lines, where upstream's
/// create-then-open clobbers from offset 0.
const LOG_TIMESTAMP: &[time::format_description::FormatItem] =
    format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]");

/// One event of a running turn, in arrival order. Rows are identified by ids
/// the app keeps; see the module docs for the translation semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// `system/init`: the turn's process is up and streaming.
    Started { session_id: Option<String> },
    /// `system/thinking_tokens`.
    Thinking { estimated_tokens: i64 },
    /// A text delta, accumulated over the current text block.
    TextStreaming { text: String },
    /// One authoritative assistant text block.
    TextConfirmed { text: String },
    /// A tool row opened at `content_block_start` (or by an assistant
    /// snapshot the stream never announced).
    ToolStarted { id: String, title: String },
    /// One `input_json_delta` for the row's accumulating input JSON.
    ToolInput { id: String, partial: String },
    /// A `user` tool result closed the row.
    ToolFinished { id: String, output: String, is_error: bool },
    /// The process ended; no further events follow.
    TurnFinished(TurnOutcome),
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    Success { session_id: Option<String>, text: String },
    Failed { error: String, auth: bool },
    /// Both attempts collided with an active writer; the caller decides on a
    /// fresh turn (classified by the lifecycle dance, not single turns).
    Busy,
    /// The stored session is gone; the caller re-runs fresh.
    MissingSession,
    Cancelled,
}

/// A running turn: its event stream plus the cancel switch. Dropping the
/// handle drops the event receiver; the turn itself runs to completion
/// either way (the child's pipes are drained by the spawner).
pub struct TurnHandle {
    pub events: mpsc::Receiver<EngineEvent>,
    cancel: watch::Sender<bool>,
    pid: Arc<Mutex<Option<u32>>>,
}

impl TurnHandle {
    /// Request interruption: SIGINT now, SIGTERM after 3 s. Idempotent; a
    /// cancel that races the spawn still lands once the pid is known.
    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }

    /// The spawned child's pid, once the process is up.
    pub fn pid(&self) -> Option<u32> {
        *self.pid.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One agent session: the settings and system-prompt template every turn of
/// a project runs with. The turn itself is stateless across calls — resume
/// state lives in the project's metadata, re-read per turn.
pub struct Session {
    settings: Settings,
    system_template: String,
}

impl Session {
    pub fn new(settings: Settings) -> Self {
        Self { settings, system_template: String::new() }
    }

    /// Sets the `CADSystemPrompt.md` template substituted into
    /// `--append-system-prompt`. Upstream loads it from its app bundle and
    /// passes an empty string when the resource is missing; an engine
    /// `Session` starts in that empty state until the app supplies one.
    pub fn with_system_template(mut self, template: impl Into<String>) -> Self {
        self.system_template = template.into();
        self
    }

    /// Runs one turn: spawn, translate, tee, persist, classify. Returns as
    /// soon as the turn is set up — must be called inside a tokio runtime,
    /// and the events arrive on the handle's channel.
    pub fn run_turn(&mut self, root: &Path, req: &TurnRequest) -> TurnHandle {
        let (events_tx, events) = mpsc::channel(256);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let pid = Arc::new(Mutex::new(None));
        tokio::spawn(drive(
            root.to_path_buf(),
            req.clone(),
            self.settings.clone(),
            self.system_template.clone(),
            events_tx,
            cancel_rx,
            Arc::clone(&pid),
        ));
        TurnHandle { events, cancel: cancel_tx, pid }
    }
}

/// Everything the spawned task needs for one turn.
struct TurnDriver {
    root: PathBuf,
    metadata: ProjectMetadata,
    state: TurnState,
    events: mpsc::Sender<EngineEvent>,
}

/// Per-turn translation state (the mutable half of AgentSession).
struct TurnState {
    session_id: Option<String>,
    /// The current text block's accumulated stream text.
    streaming_text: String,
    /// `content_block_start`'s index → the tool_use id announced there.
    tool_ids: HashMap<i64, String>,
    /// Tool ids a row exists for.
    seen_tools: HashSet<String>,
    has_confirmed_text: bool,
    last_confirmed_text: String,
    turn_result: Option<ResultEvent>,
    /// Every stdout line newline-joined, for failure details.
    stdout_text: String,
}

async fn drive(
    root: PathBuf,
    req: TurnRequest,
    settings: Settings,
    system_template: String,
    events: mpsc::Sender<EngineEvent>,
    mut cancel_rx: watch::Receiver<bool>,
    pid_cell: Arc<Mutex<Option<u32>>>,
) {
    let mut driver = TurnDriver {
        root: root.clone(),
        metadata: load_metadata(&root)
            .unwrap_or_else(|_| ProjectMetadata::new(env!("CARGO_PKG_VERSION"))),
        state: TurnState {
            session_id: None,
            streaming_text: String::new(),
            tool_ids: HashMap::new(),
            seen_tools: HashSet::new(),
            has_confirmed_text: false,
            last_confirmed_text: String::new(),
            turn_result: None,
            stdout_text: String::new(),
        },
        events,
    };

    let Some(executable) = resolve_executable(&settings) else {
        let _ = driver
            .events
            .send(EngineEvent::TurnFinished(TurnOutcome::Failed {
                error: "claude is not installed or was not found. Install it or set its path in settings."
                    .to_string(),
                auth: false,
            }))
            .await;
        return;
    };

    // the missing-output note needs the parts count from the parsed 3MF and
    // a manifest that only counts when it loads
    let output_dir = root.join("output");
    let parts_count = std::fs::read(output_dir.join("model.3mf"))
        .ok()
        .and_then(|bytes| parse_3mf(&bytes).ok())
        .map_or(0, |parts| parts.len());
    let manifest = Manifest::load(&output_dir.join("manifest.json")).ok();
    let missing = MissingCtx {
        project_root: &root,
        output_dir: &output_dir,
        parts_count,
        printer_model: settings.printer_model.as_deref(),
        manifest: manifest.as_ref(),
    };
    let prompt = build_prompt(&req, &root, &missing);
    let system_prompt = render_system_prompt(&system_template, &prompt_ctx(&root, &settings));
    let venv_python = root.join(".venv/bin/python");
    let mut argv = vec![executable.path.to_string_lossy().into_owned()];
    argv.extend(build_args(
        &executable,
        &prompt,
        &system_prompt,
        &settings,
        driver.metadata.session_id.as_deref(),
        &venv_python,
    ));
    // The login environment is snapshotted per turn: upstream caches it in
    // its long-lived actor, which M4's app can layer on top the same way.
    let env: Vec<(String, String)> = shell_env().into_iter().collect();

    let mut proc = match spawn_streaming(&argv, &root, &env) {
        Ok(proc) => proc,
        Err(error) => {
            let _ = driver
                .events
                .send(EngineEvent::TurnFinished(TurnOutcome::Failed {
                    error: format!("could not start claude: {error}"),
                    auth: false,
                }))
                .await;
            return;
        }
    };
    *pid_cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(proc.pid);
    // a cancel that arrived before the pid was published still lands
    if *cancel_rx.borrow() {
        proc.interrupt();
    }
    let mut log = open_log(&root);

    // One branch per source: a stdout line to translate, or a cancel signal.
    // The cancel arm only flags; proc is touched after the select's futures
    // are gone, so the borrows never overlap.
    enum Step {
        Line(Option<Line>),
        Cancel,
        CancelChannelClosed,
    }
    let mut cancel_alive = true;
    loop {
        let step = tokio::select! {
            line = proc.lines.recv() => Step::Line(line),
            changed = cancel_rx.changed(), if cancel_alive => match changed {
                Ok(()) => Step::Cancel,
                Err(_) => Step::CancelChannelClosed,
            },
        };
        match step {
            Step::Cancel => {
                if *cancel_rx.borrow_and_update() {
                    proc.interrupt();
                }
            }
            // the handle is gone: no further cancel can arrive, stop polling
            Step::CancelChannelClosed => cancel_alive = false,
            Step::Line(Some(Line::Stdout(line))) => {
                if let Some(log) = log.as_mut() {
                    let _ = writeln!(log, "{line}");
                }
                driver.state.stdout_text.push_str(&line);
                driver.state.stdout_text.push('\n');
                driver.on_stdout_line(&line).await;
            }
            Step::Line(Some(Line::Stderr(_))) => {}
            Step::Line(None) => break,
        }
    }

    let RunningProcess { stderr_text, exit_status, .. } = proc;
    let stderr = stderr_text.await.unwrap_or_default();
    let Ok(status) = exit_status.await else {
        // only reachable when the spawner's reader task itself died
        let _ = driver
            .events
            .send(EngineEvent::TurnFinished(TurnOutcome::Failed {
                error: "the agent process exited without reporting a status".to_string(),
                auth: false,
            }))
            .await;
        return;
    };
    let interrupted = *cancel_rx.borrow();
    let final_outcome = if interrupted {
        TurnOutcome::Cancelled
    } else {
        outcome(&driver.state, &stderr, status.success())
    };
    let _ = driver
        .events
        .send(EngineEvent::TurnFinished(final_outcome))
        .await;
}

/// The system-prompt substitutions the session can derive on its own. The
/// date is ISO numeric: upstream's locale-dependent numeric format has no
/// deterministic engine-side equivalent.
fn prompt_ctx(root: &Path, settings: &Settings) -> PromptCtx {
    const ISO_DATE: &[time::format_description::FormatItem] =
        format_description!("[year]-[month]-[day]");
    PromptCtx {
        project_name: root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".to_string()),
        python_display: PYTHON_UNVERIFIED.to_string(),
        venv_python: root.join(".venv/bin/python"),
        environment: environment_bullets(root, PYTHON_UNVERIFIED),
        units: settings.measurement_unit.prompt_name().to_string(),
        printer: settings
            .printer_model
            .clone()
            .unwrap_or_else(|| "not specified".to_string()),
        guidelines: settings.printer_type.prompt_guidelines().to_string(),
        date: OffsetDateTime::now_utc().format(&ISO_DATE).unwrap_or_default(),
    }
}

/// The executable this session spawns: the test/embedder override, else the
/// locator's discovery from the settings' custom path.
fn resolve_executable(settings: &Settings) -> Option<AgentPath> {
    if let Some(path) = std::env::var_os(TEST_CLAUDE_ENV) {
        return Some(AgentPath { path: path.into(), version: String::new() });
    }
    locator::find_agent(AgentKind::Claude, settings.claude_path.as_deref())
}

/// Create this turn's tee log under `.cadstudio/logs/`. A failure to open is
/// not a turn failure — upstream's `try?` simply drops the log too.
fn open_log(root: &Path) -> Option<std::fs::File> {
    let dir = root.join(".cadstudio").join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    let name = OffsetDateTime::now_utc().format(&LOG_TIMESTAMP).ok()?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{name}.jsonl")))
        .ok()
}

impl TurnDriver {
    /// Translate one raw stdout line (AgentSession.swift:104-145's `process`),
    /// emitting EngineEvents and persisting session ids as they are learned.
    async fn on_stdout_line(&mut self, line: &str) {
        match parse_line(line) {
            Some(StreamEvent::System(system)) => match system.kind {
                SystemKind::Init => {
                    if let Some(session_id) = &system.session_id {
                        if self.metadata.session_id.as_deref() != Some(session_id) {
                            self.metadata.session_id = Some(session_id.clone());
                        }
                        // upstream saves unconditionally on .started
                        let _ = save_metadata(&self.root, &self.metadata);
                        self.state.session_id = Some(session_id.clone());
                    }
                    let _ = self
                        .events
                        .send(EngineEvent::Started { session_id: system.session_id })
                        .await;
                }
                SystemKind::ThinkingTokens => {
                    if let Some(tokens) = system.estimated_tokens {
                        let _ = self
                            .events
                            .send(EngineEvent::Thinking { estimated_tokens: tokens })
                            .await;
                    }
                }
                _ => {}
            },
            Some(StreamEvent::Stream { event }) => match event.event {
                InnerEvent::ContentStart { index, block } => {
                    // any block start closes the streaming text row
                    self.state.streaming_text.clear();
                    if let ContentBlock::ToolUse { id, name, .. } = block {
                        self.state.tool_ids.insert(index, id.clone());
                        self.state.seen_tools.insert(id.clone());
                        let title = live_title(&name, "");
                        let _ = self
                            .events
                            .send(EngineEvent::ToolStarted { id, title })
                            .await;
                    }
                }
                InnerEvent::ContentDelta { index, delta } => match delta {
                    Delta::Text { text } => {
                        self.state.streaming_text.push_str(&text);
                        let text = self.state.streaming_text.clone();
                        let _ = self.events.send(EngineEvent::TextStreaming { text }).await;
                    }
                    Delta::InputJson { partial_json } => {
                        if let Some(id) = self.state.tool_ids.get(&index) {
                            let _ = self
                                .events
                                .send(EngineEvent::ToolInput {
                                    id: id.clone(),
                                    partial: partial_json,
                                })
                                .await;
                        }
                    }
                    _ => {}
                },
                _ => {}
            },
            Some(StreamEvent::Assistant(assistant)) => match assistant.block {
                ContentBlock::Text { text } => {
                    self.state.has_confirmed_text = true;
                    self.state.last_confirmed_text = text.clone();
                    let _ = self.events.send(EngineEvent::TextConfirmed { text }).await;
                }
                ContentBlock::ToolUse { id, name, input } => {
                    self.state.streaming_text.clear();
                    // a row the stream never announced opens here; one it did
                    // only gets a title/input refinement upstream, which no
                    // EngineEvent carries
                    if self.state.seen_tools.insert(id.clone()) {
                        let title = summarize(&name, &input);
                        let _ = self
                            .events
                            .send(EngineEvent::ToolStarted { id, title })
                            .await;
                    }
                }
                _ => {}
            },
            Some(StreamEvent::UserToolResult(result)) => {
                let output = match result.content {
                    ToolResultContent::Text(text) => text,
                    ToolResultContent::Blocks(blocks) => blocks
                        .iter()
                        .map(|block| block.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    ToolResultContent::Other => String::new(),
                };
                let is_error = result.is_error.unwrap_or(false);
                let _ = self
                    .events
                    .send(EngineEvent::ToolFinished {
                        id: result.tool_use_id,
                        output,
                        is_error,
                    })
                    .await;
            }
            Some(StreamEvent::Result(result)) => {
                if let Some(session_id) = &result.session_id {
                    self.metadata.session_id = Some(session_id.clone());
                    let _ = save_metadata(&self.root, &self.metadata);
                    self.state.session_id = Some(session_id.clone());
                }
                self.state.turn_result = Some(result);
            }
            // decoded enough to be skippable: rate limits, unknown shapes,
            // and process noise that is not a JSON object at all
            Some(StreamEvent::RateLimit) | Some(StreamEvent::Unknown(_)) | None => {}
        }
    }
}

/// Classify the finished process (AgentSession.swift:264-303).
fn outcome(state: &TurnState, stderr: &str, exit_ok: bool) -> TurnOutcome {
    match state.turn_result.as_ref() {
        Some(result) if !result.is_error => TurnOutcome::Success {
            session_id: result.session_id.clone().or_else(|| state.session_id.clone()),
            text: if state.has_confirmed_text {
                state.last_confirmed_text.clone()
            } else {
                result.result.clone()
            },
        },
        result => {
            let message = result
                .map(|result| result.result.clone())
                .unwrap_or_default();
            let stdout = if exit_ok { "" } else { state.stdout_text.as_str() };
            let mut details: Vec<String> = [message, stderr.to_string(), stdout.to_string()]
                .into_iter()
                .map(|piece| piece.trim().to_string())
                .filter(|piece| !piece.is_empty())
                .collect();
            // the engine surfaces the API error status alongside the text;
            // upstream leaves it implicit in the result line it never shows
            if let Some(status) = result.as_ref().and_then(|result| result.api_error_status) {
                details.push(format!("api error status {status}"));
            }
            let details = details.join("\n");
            TurnOutcome::Failed { auth: is_authentication_error(&details), error: details }
        }
    }
}

/// Upstream `isBusySession`'s needle over the combined turn text.
pub fn is_busy_session(text: &str) -> bool {
    contains_ci(text, "already has an active writer")
}

/// Upstream `isMissingSession`'s needles.
pub fn is_missing_session(text: &str) -> bool {
    ["no conversation found", "no rollout found", "session not found", "thread not found"]
        .iter()
        .any(|needle| contains_ci(text, needle))
}

/// Upstream `isAuthenticationError`'s needles.
pub fn is_authentication_error(text: &str) -> bool {
    [
        "invalid api key",
        "/login",
        "not logged in",
        "authentication",
        "oauth token",
        "unauthorized",
        "codex login",
        "401",
    ]
    .iter()
    .any(|needle| contains_ci(text, needle))
}

/// `localizedCaseInsensitiveContains` for ASCII needles.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}
