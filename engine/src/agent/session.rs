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
//! The lifecycle dance around single turns is [`Session::send`]
//! (AgentSession.swift:30-102): the argv resumes (with `--fork-session`)
//! only while a session id is stored, and only a RESUMED, uncancelled
//! attempt that errored or produced no result line is classified — over the
//! combined result text + stdout + stderr, case-insensitively. A busy
//! collision waits the retry delay and retries once with resume; a second
//! collision (or a missing session, immediately) clears the stored id,
//! explains itself through a [`EngineEvent::System`] row, and restarts
//! fresh. Auth failures and everything else end the turn where it stands
//! (no retry); a cancel during the retry wait ends the dance with no
//! further spawn.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// How long a busy collision is left to clear before the resume retry
/// (AgentSession.swift:75's 5 s; injectable for tests).
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// The system row announcing a busy collision escalated to a fresh context
/// (AgentSession.swift:81; English catalog value — the Swift literal is the
/// Italian source string).
const BUSY_FRESH_MESSAGE: &str = "The previous session is still in use by another process: the conversation context started over, but the existing model.py is still the basis for the work.";

/// The system row announcing the stored session is gone and the turn
/// restarted fresh (AgentSession.swift:91; English catalog value).
const MISSING_SESSION_MESSAGE: &str = "The previous session wasn’t found: the conversation context has been reset, but the existing model.py remains the basis of the work.";

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
    /// A system chat row the lifecycle dance emits between attempts
    /// (upstream `role: .system`): the busy and missing-session fallback
    /// explanations. Nothing else in the engine produces one.
    System { text: String },
    /// The process ended; no further events follow.
    TurnFinished(TurnOutcome),
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    Success { session_id: Option<String>, text: String },
    Failed { error: String, auth: bool },
    /// A resume collided with an active writer twice. [`Session::send`]'s
    /// dance absorbs this itself (retry, then a fresh run), so the engine
    /// never emits it; it names the classification for a caller that drives
    /// single [`Session::run_turn`]s and orchestrates its own retry.
    Busy,
    /// The stored session is gone. As [`TurnOutcome::Busy`]: the dance
    /// clears the id and restarts fresh itself, so the engine never emits
    /// it.
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
    retry_delay: Duration,
}

impl Session {
    pub fn new(settings: Settings) -> Self {
        Self { settings, system_template: String::new(), retry_delay: RETRY_DELAY }
    }

    /// Sets the `CADSystemPrompt.md` template substituted into
    /// `--append-system-prompt`. Upstream loads it from its app bundle and
    /// passes an empty string when the resource is missing; an engine
    /// `Session` starts in that empty state until the app supplies one.
    pub fn with_system_template(mut self, template: impl Into<String>) -> Self {
        self.system_template = template.into();
        self
    }

    /// Sets the busy-retry wait (production default 5 s) — the test seam for
    /// the one place the dance deliberately idles.
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// Wire one turn's plumbing (event channel, cancel switch, pid cell) and
    /// hand the driver task everything it owns.
    fn launch(&self, root: &Path, req: &TurnRequest) -> (TurnHandle, TurnCtx) {
        let (events_tx, events) = mpsc::channel(256);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let pid = Arc::new(Mutex::new(None));
        let ctx = TurnCtx {
            root: root.to_path_buf(),
            req: req.clone(),
            settings: self.settings.clone(),
            system_template: self.system_template.clone(),
            retry_delay: self.retry_delay,
            events: events_tx,
            cancel: cancel_rx,
            pid: Arc::clone(&pid),
        };
        (TurnHandle { events, cancel: cancel_tx, pid }, ctx)
    }

    /// Runs one turn: spawn, translate, tee, persist, classify. Returns as
    /// soon as the turn is set up — must be called inside a tokio runtime,
    /// and the events arrive on the handle's channel.
    pub fn run_turn(&mut self, root: &Path, req: &TurnRequest) -> TurnHandle {
        let (handle, ctx) = self.launch(root, req);
        tokio::spawn(single_turn(ctx));
        handle
    }

    /// Runs one turn through the full lifecycle dance
    /// (AgentSession.swift:30-102): resume while a session id is stored,
    /// classify a resumed-and-errored attempt over its combined
    /// result+stdout+stderr text, and on busy/missing fall back to a fresh
    /// run — with the same handle semantics as [`Session::run_turn`]. This
    /// is the API the app drives.
    pub fn send(&mut self, root: &Path, req: &TurnRequest) -> TurnHandle {
        let (handle, ctx) = self.launch(root, req);
        tokio::spawn(full_dance(ctx));
        handle
    }
}

/// Everything one driver task owns; each attempt of the dance reuses the
/// plumbing while re-reading the project state it needs.
struct TurnCtx {
    root: PathBuf,
    req: TurnRequest,
    settings: Settings,
    system_template: String,
    retry_delay: Duration,
    events: mpsc::Sender<EngineEvent>,
    cancel: watch::Receiver<bool>,
    pid: Arc<Mutex<Option<u32>>>,
}

/// The per-attempt translation state plus the metadata base it persists
/// session ids into.
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

/// One finished attempt: its classified outcome plus the raw text the
/// dance's busy/missing guards read (upstream classifies over the result
/// line's text joined with the process's whole output).
struct Attempt {
    outcome: TurnOutcome,
    turn_result: Option<ResultEvent>,
    stdout: String,
    stderr: String,
}

impl Attempt {
    /// A turn that never produced a stream to classify.
    fn failed(error: impl Into<String>) -> Self {
        Self {
            outcome: TurnOutcome::Failed { error: error.into(), auth: false },
            turn_result: None,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

/// [`Session::run_turn`]'s driver: one attempt, then its terminal event.
async fn single_turn(mut ctx: TurnCtx) {
    let metadata = stored_metadata(&ctx.root);
    let resume_sid = metadata.session_id.clone();
    let attempt = run_attempt(&mut ctx, resume_sid.as_deref()).await;
    let _ = ctx.events.send(EngineEvent::TurnFinished(attempt.outcome)).await;
}

/// [`Session::send`]'s driver — the full turn dance
/// (AgentSession.swift:30-102). Each iteration is one `run()` invocation:
/// resume while allowed AND stored, classify a resumed, uncancelled,
/// errored-or-barren attempt, and either continue the dance or emit the
/// terminal outcome exactly once.
async fn full_dance(mut ctx: TurnCtx) {
    let mut attempt_no = 0;
    let mut allows_resume = true;
    loop {
        let metadata = stored_metadata(&ctx.root);
        let resume_sid = if allows_resume { metadata.session_id } else { None };
        let attempt = run_attempt(&mut ctx, resume_sid.as_deref()).await;

        // the guards (AgentSession.swift:72/87 + 306/312): only an attempt
        // that actually resumed, was not cancelled, and errored or produced
        // no result line is classified — a successful turn's stderr noise is
        // carried unheard
        if resume_sid.is_some()
            && !matches!(attempt.outcome, TurnOutcome::Cancelled)
            && attempt.turn_result.as_ref().is_none_or(|result| result.is_error)
        {
            let combined = combined_text(&attempt);
            if is_busy_session(&combined) {
                if attempt_no == 0 {
                    // leave the collision to clear, then retry once — but a
                    // cancel during the wait ends the dance with no further
                    // spawn
                    let cancelled = tokio::select! {
                        changed = ctx.cancel.changed() => match changed {
                            Ok(()) => *ctx.cancel.borrow_and_update(),
                            // the handle is gone: no cancel can arrive, so
                            // the wait simply runs out
                            Err(_) => {
                                tokio::time::sleep(ctx.retry_delay).await;
                                false
                            }
                        },
                        _ = tokio::time::sleep(ctx.retry_delay) => false,
                    };
                    if cancelled {
                        let _ = ctx
                            .events
                            .send(EngineEvent::TurnFinished(TurnOutcome::Cancelled))
                            .await;
                        return;
                    }
                    attempt_no = 1;
                    continue;
                }
                clear_session_id(&ctx.root);
                let _ = ctx
                    .events
                    .send(EngineEvent::System { text: BUSY_FRESH_MESSAGE.to_string() })
                    .await;
                allows_resume = false;
                continue;
            }
            if is_missing_session(&combined) {
                clear_session_id(&ctx.root);
                let _ = ctx
                    .events
                    .send(EngineEvent::System { text: MISSING_SESSION_MESSAGE.to_string() })
                    .await;
                allows_resume = false;
                continue;
            }
        }

        let _ = ctx.events.send(EngineEvent::TurnFinished(attempt.outcome)).await;
        return;
    }
}

/// One spawned attempt: setup, spawn, translate, tee, persist, classify —
/// everything a single upstream `run()` does short of emitting a terminal
/// event. `resume_sid` is the id the argv resumes (None = fresh); the
/// metadata the attempt persists into is re-read from disk here.
async fn run_attempt(ctx: &mut TurnCtx, resume_sid: Option<&str>) -> Attempt {
    let mut driver = TurnDriver {
        root: ctx.root.clone(),
        metadata: stored_metadata(&ctx.root),
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
        events: ctx.events.clone(),
    };

    let Some(executable) = resolve_executable(&ctx.settings) else {
        return Attempt::failed(
            "claude is not installed or was not found. Install it or set its path in settings.",
        );
    };

    // the missing-output note needs the parts count from the parsed 3MF and
    // a manifest that only counts when it loads
    let output_dir = ctx.root.join("output");
    let parts_count = std::fs::read(output_dir.join("model.3mf"))
        .ok()
        .and_then(|bytes| parse_3mf(&bytes).ok())
        .map_or(0, |parts| parts.len());
    let manifest = Manifest::load(&output_dir.join("manifest.json")).ok();
    let missing = MissingCtx {
        project_root: &ctx.root,
        output_dir: &output_dir,
        parts_count,
        printer_model: ctx.settings.printer_model.as_deref(),
        manifest: manifest.as_ref(),
    };
    let prompt = build_prompt(&ctx.req, &ctx.root, &missing);
    let system_prompt =
        render_system_prompt(&ctx.system_template, &prompt_ctx(&ctx.root, &ctx.settings));
    let venv_python = ctx.root.join(".venv/bin/python");
    let mut argv = vec![executable.path.to_string_lossy().into_owned()];
    argv.extend(build_args(
        &executable,
        &prompt,
        &system_prompt,
        &ctx.settings,
        resume_sid,
        &venv_python,
    ));
    // The login environment is snapshotted per turn: upstream caches it in
    // its long-lived actor, which M4's app can layer on top the same way.
    let env: Vec<(String, String)> = shell_env().into_iter().collect();

    let mut proc = match spawn_streaming(&argv, &ctx.root, &env) {
        Ok(proc) => proc,
        Err(error) => return Attempt::failed(format!("could not start claude: {error}")),
    };
    *ctx.pid.lock().unwrap_or_else(|e| e.into_inner()) = Some(proc.pid);
    // a cancel that arrived before the pid was published still lands
    if *ctx.cancel.borrow() {
        proc.interrupt();
    }
    let mut log = open_log(&ctx.root);

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
            changed = ctx.cancel.changed(), if cancel_alive => match changed {
                Ok(()) => Step::Cancel,
                Err(_) => Step::CancelChannelClosed,
            },
        };
        match step {
            Step::Cancel => {
                if *ctx.cancel.borrow_and_update() {
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
        return Attempt::failed("the agent process exited without reporting a status");
    };
    let interrupted = *ctx.cancel.borrow();
    let outcome = if interrupted {
        TurnOutcome::Cancelled
    } else {
        outcome(&driver.state, &stderr, status.success())
    };
    Attempt { outcome, turn_result: driver.state.turn_result, stdout: driver.state.stdout_text, stderr }
}

/// The project's stored metadata, or a fresh document when none loads.
fn stored_metadata(root: &Path) -> ProjectMetadata {
    load_metadata(root).unwrap_or_else(|_| ProjectMetadata::new(env!("CARGO_PKG_VERSION")))
}

/// Upstream's classification text: the result line's text joined with the
/// process's whole stdout and stderr (AgentSession.swift:307).
fn combined_text(attempt: &Attempt) -> String {
    let mut text = attempt
        .turn_result
        .as_ref()
        .map(|result| result.result.clone())
        .unwrap_or_default();
    text.push_str(&attempt.stdout);
    text.push_str(&attempt.stderr);
    text
}

/// Clear the stored session id and persist it (upstream
/// `project.metadata.sessionID = nil` before a fresh re-run).
fn clear_session_id(root: &Path) {
    if let Ok(mut metadata) = load_metadata(root) {
        metadata.session_id = None;
        let _ = save_metadata(root, &metadata);
    }
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
