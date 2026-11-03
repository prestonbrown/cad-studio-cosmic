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
//!   no-op here. [`EngineEvent::ToolStarted`] carries the block's name
//!   alongside the live title; every `input_json_delta` is forwarded as
//!   [`EngineEvent::ToolInput`]; throttling is the app's job (upstream does
//!   it at 0.15 s).
//! - Thinking blocks open at their `content_block_start` and close at the
//!   matching `content_block_stop` (upstream `startBlock`/`stopBlock`),
//!   redacted thinking included; the token estimate still rides
//!   `system/thinking_tokens`, not `thinking_delta`.
//! - The session id is captured from `system/init` OR `result` and persisted
//!   immediately (upstream `project.save()` on both).
//!
//! Outcome classification (AgentSession.swift:264-303): `Success` only when
//! the result line says `is_error: false`; its text is the last confirmed
//! assistant text, or — only when no text was confirmed this turn — the
//! result text (upstream `finish()`'s `hasText` rule). Failures join the
//! result text, the FULL stderr, and the full stdout when the process exited
//! nonzero, and carry the child's exit code for the app's "(code N)" row.
//! Cancellation is requested-side, not exit-side: a turn whose
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

use time::OffsetDateTime;
use time::macros::format_description;
use tokio::sync::{mpsc, watch};

use crate::agent::claude::{
    PromptCtx, TurnRequest, build_args, build_prompt, environment_bullets, render_system_prompt,
};
use crate::agent::codex::{self, CodexTranslator};
use crate::agent::locator::{self, AgentKind, AgentPath, shell_env};
use crate::agent::stream_event::{
    ContentBlock, Delta, InnerEvent, ResultEvent, StreamEvent, SystemKind, ToolResultContent,
    parse_line,
};
use crate::agent::tool_summary::{ToolTitle, live_title, summarize};
use crate::parsing::threemf::parse_3mf;
use crate::processes::{Line, RunningProcess, spawn_streaming};
use crate::project::manifest::Manifest;
use crate::project::outputs::MissingCtx;
use crate::project::store::{ProjectMetadata, load_metadata, save_metadata};
use crate::settings::Settings;

/// Names an executable to spawn instead of discovering one — the test and
/// embedder seam (same pattern as the registry override in `processes`). The
/// path is never `--version`-validated: a scripted fake reads its scenario
/// from the environment and would treat a probe invocation as a turn. It is
/// still held to the locator's executable-file check, so an override whose
/// file is gone resolves to nothing, as a vanished install does.
const TEST_CLAUDE_ENV: &str = "CAD_TEST_CLAUDE";

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

/// Which system chat row the lifecycle dance emits between attempts
/// (upstream appends localized `role: .system` rows, AgentSession.swift:81
/// and :91). The engine has no catalog, so it names the row and the app
/// localizes at its boundary — the ids are the stable contract chat.json
/// rows are keyed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemRow {
    /// A busy collision escalated to a fresh context.
    BusyFreshContext,
    /// The stored session is gone; the turn restarted fresh.
    MissingSession,
}

/// One event of a running turn, in arrival order. Rows are identified by ids
/// the app keeps; see the module docs for the translation semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// `system/init`: the turn's process is up and streaming.
    Started { session_id: Option<String> },
    /// `system/thinking_tokens`.
    Thinking { estimated_tokens: i64 },
    /// A thinking (or redacted thinking) block opened at
    /// `content_block_start`. One block is open at a time, keyed by the
    /// block index like a tool row.
    ThinkingStarted,
    /// The open thinking block's `content_block_stop` arrived; the duration
    /// the app reports is its own clock over the open interval.
    ThinkingFinished,
    /// A text delta, accumulated over the current text block.
    TextStreaming { text: String },
    /// One authoritative assistant text block.
    TextConfirmed { text: String },
    /// A tool row opened at `content_block_start` (or by an assistant
    /// snapshot the stream never announced). `name` is the `tool_use`
    /// block's tool name; `title` is a [`ToolTitle`] id the app localizes.
    ToolStarted {
        id: String,
        name: String,
        title: ToolTitle,
    },
    /// One `input_json_delta` for the row's accumulating input JSON.
    ToolInput { id: String, partial: String },
    /// A `user` tool result closed the row.
    ToolFinished {
        id: String,
        output: String,
        is_error: bool,
    },
    /// A system chat row the lifecycle dance emits between attempts
    /// (upstream `role: .system`): the busy and missing-session fallback
    /// explanations, as stable [`SystemRow`] ids the app localizes on
    /// receipt. Nothing else in the engine produces one.
    System { row: SystemRow },
    /// The process ended; no further events follow.
    TurnFinished(TurnOutcome),
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    Success {
        session_id: Option<String>,
        text: String,
    },
    /// The child's exit code — `None` when no process was ever spawned or
    /// it died without reporting one (signal), `Some(0)` when it exited
    /// clean but the turn still failed (an error result line).
    Failed {
        error: String,
        auth: bool,
        exit_code: Option<i32>,
    },
    /// The agent executable did not resolve for this turn (upstream's
    /// `locator.resolve()` guard, AgentSession.swift:40-50); nothing was
    /// spawned. Re-checked every turn, so an executable that vanishes
    /// mid-session ends the next turn here. Upstream answers with a
    /// `.configureClaude` system row, presents the agent-missing sheet, and
    /// reports the `non-trovato` activity.
    AgentMissing,
    /// The executable resolved but could not be spawned (upstream's
    /// `catch`, AgentSession.swift:98-100: the `impossibile-avviare` row
    /// with `error` as its detail, the `errore-di` activity). `error` is
    /// the OS error text, verbatim.
    StartFailed {
        error: String,
    },
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
    /// The settings file each attempt re-reads when attached (upstream
    /// reads UserDefaults live); `None` = the startup snapshot decides, the
    /// embedder/test seam.
    settings_file: Option<PathBuf>,
    system_template: String,
    retry_delay: Duration,
}

impl Session {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            settings_file: None,
            system_template: String::new(),
            retry_delay: RETRY_DELAY,
        }
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

    /// Attaches the settings file each attempt re-reads its settings from
    /// (the production path — the bridge hands the engine settings path in).
    /// Without it the startup snapshot decides every turn, which is what
    /// embedders that own their settings want.
    pub fn with_settings_file(mut self, path: PathBuf) -> Self {
        self.settings_file = Some(path);
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
            settings_file: self.settings_file.clone(),
            system_template: self.system_template.clone(),
            retry_delay: self.retry_delay,
            events: events_tx,
            cancel: cancel_rx,
            pid: Arc::clone(&pid),
        };
        (
            TurnHandle {
                events,
                cancel: cancel_tx,
                pid,
            },
            ctx,
        )
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
    settings_file: Option<PathBuf>,
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
    /// The engine this attempt runs, decided per attempt from the settings
    /// (upstream resets `engine = AgentEngine.current` at the top of every
    /// `run()`, AgentSession.swift:31).
    engine: AgentKind,
    /// The codex line translator, reset per attempt like upstream's
    /// (`translator = CodexTranslator()`); unused on claude attempts.
    translator: CodexTranslator,
    session_id: Option<String>,
    /// The current text block's accumulated stream text.
    streaming_text: String,
    /// `content_block_start`'s index → the tool_use id announced there.
    tool_ids: HashMap<i64, String>,
    /// The open thinking block's index (upstream `thinkingIndex`).
    thinking_index: Option<i64>,
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
        Self::ended(TurnOutcome::Failed {
            error: error.into(),
            auth: false,
            exit_code: None,
        })
    }

    /// A turn that ended with `outcome` before producing a stream.
    fn ended(outcome: TurnOutcome) -> Self {
        Self {
            outcome,
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
    let _ = ctx
        .events
        .send(EngineEvent::TurnFinished(attempt.outcome))
        .await;
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
        let resume_sid = if allows_resume {
            metadata.session_id
        } else {
            None
        };
        let attempt = run_attempt(&mut ctx, resume_sid.as_deref()).await;

        // the guards (AgentSession.swift:72/87 + 306/312): only an attempt
        // that actually resumed, was not cancelled, and errored or produced
        // no result line is classified — a successful turn's stderr noise is
        // carried unheard
        if resume_sid.is_some()
            && !matches!(attempt.outcome, TurnOutcome::Cancelled)
            && attempt
                .turn_result
                .as_ref()
                .is_none_or(|result| result.is_error)
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
                    .send(EngineEvent::System {
                        row: SystemRow::BusyFreshContext,
                    })
                    .await;
                allows_resume = false;
                continue;
            }
            if is_missing_session(&combined) {
                clear_session_id(&ctx.root);
                let _ = ctx
                    .events
                    .send(EngineEvent::System {
                        row: SystemRow::MissingSession,
                    })
                    .await;
                allows_resume = false;
                continue;
            }
        }

        let _ = ctx
            .events
            .send(EngineEvent::TurnFinished(attempt.outcome))
            .await;
        return;
    }
}

/// One spawned attempt: setup, spawn, translate, tee, persist, classify —
/// everything a single upstream `run()` does short of emitting a terminal
/// event. `resume_sid` is the id the argv resumes (None = fresh); the
/// metadata the attempt persists into is re-read from disk here. The
/// attempt's settings — engine included — are re-read from the attached
/// settings file, falling back to the startup snapshot
/// ([`Session::with_settings_file`]).
async fn run_attempt(ctx: &mut TurnCtx, resume_sid: Option<&str>) -> Attempt {
    let settings = attempt_settings(ctx);
    let engine = settings.agent_kind();
    let mut driver = TurnDriver {
        root: ctx.root.clone(),
        metadata: stored_metadata(&ctx.root),
        state: TurnState {
            engine,
            translator: CodexTranslator::default(),
            session_id: None,
            streaming_text: String::new(),
            tool_ids: HashMap::new(),
            thinking_index: None,
            seen_tools: HashSet::new(),
            has_confirmed_text: false,
            last_confirmed_text: String::new(),
            turn_result: None,
            stdout_text: String::new(),
        },
        events: ctx.events.clone(),
    };

    let Some(executable) = resolve_executable(engine, &settings) else {
        return Attempt::ended(TurnOutcome::AgentMissing);
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
        printer_model: settings.printer_model.as_deref(),
        manifest: manifest.as_ref(),
    };
    let prompt = build_prompt(&ctx.req, &missing);
    let system_prompt =
        render_system_prompt(&ctx.system_template, &prompt_ctx(&ctx.root, &settings));
    let venv_python = app_venv_python();
    let mut argv = vec![executable.path.to_string_lossy().into_owned()];
    match engine {
        // the codex argv: same assembled prompt and rendered system prompt,
        // codex flags (CodexRunner.swift:19-40)
        AgentKind::Codex => argv.extend(codex::build_args(
            &settings,
            &prompt,
            &ctx.req.images,
            &ctx.root,
            &system_prompt,
            resume_sid,
        )),
        AgentKind::Claude => argv.extend(build_args(
            &executable,
            &prompt,
            &system_prompt,
            &settings,
            resume_sid,
            &venv_python,
        )),
    }
    // The login environment is snapshotted per turn: upstream caches it in
    // its long-lived actor, which M4's app can layer on top the same way.
    let mut env = shell_env();
    // ModelBuilder.toolEnvironment's additions ride the login env
    // (ClaudeRunner.swift:23; upstream-contract.md §1)
    env.insert("MPLBACKEND".to_string(), "Agg".to_string());
    env.insert("PYTHONUNBUFFERED".to_string(), "1".to_string());
    let env: Vec<(String, String)> = env.into_iter().collect();

    let mut proc = match spawn_streaming(&argv, &ctx.root, &env) {
        Ok(proc) => proc,
        Err(error) => {
            return Attempt::ended(TurnOutcome::StartFailed {
                error: error.to_string(),
            });
        }
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

    let RunningProcess {
        stderr_text,
        exit_status,
        ..
    } = proc;
    let stderr = stderr_text.await.unwrap_or_default();
    let Ok(status) = exit_status.await else {
        // only reachable when the spawner's reader task itself died: no
        // detail to show, the app's code-less failure row says it all
        return Attempt::failed("");
    };
    let interrupted = *ctx.cancel.borrow();
    let outcome = if interrupted {
        TurnOutcome::Cancelled
    } else {
        outcome(&driver.state, &stderr, status.code())
    };
    Attempt {
        outcome,
        turn_result: driver.state.turn_result,
        stdout: driver.state.stdout_text,
        stderr,
    }
}

/// The project's stored metadata, or a fresh document when none loads.
fn stored_metadata(root: &Path) -> ProjectMetadata {
    load_metadata(root).unwrap_or_else(|_| ProjectMetadata::new(env!("CARGO_PKG_VERSION")))
}

/// The settings this attempt runs with (Ruling 1, M8 plan): upstream's
/// `run()` re-reads every setting live at the top of each turn — flipping
/// the engine picker mid-conversation changes the engine of the NEXT turn
/// while the stored session id stays engine-blind (upstream's settings
/// FOOTER says existing projects keep their engine; the code disagrees, and
/// the code is what ports). When a settings file is attached
/// ([`Session::with_settings_file`]) each attempt re-reads it, falling back
/// to the startup snapshot when it cannot load.
fn attempt_settings(ctx: &TurnCtx) -> Settings {
    match &ctx.settings_file {
        Some(path) => Settings::load(path).unwrap_or_else(|_| ctx.settings.clone()),
        None => ctx.settings.clone(),
    }
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

/// The app-wide venv interpreter, routed through python::env's canonical
/// data-dir helper so the session can never name a different venv than the
/// one M5 installs: `~/.local/share/cad-studio/venv/bin/python` on Linux
/// (spec §2/§6). `{{PYTHON}}` and the restricted-bash entry name it from
/// day one; its existence is M5's to establish, so nothing here requires
/// it to be present.
fn app_venv_python() -> PathBuf {
    crate::python::env::interpreter_path(&crate::python::env::venv_dir())
}

/// The system-prompt substitutions the session can derive on its own. The
/// date is ISO numeric: upstream's locale-dependent numeric format has no
/// deterministic engine-side equivalent. The python line is the shared
/// versions cell's display: the verified versions line, or upstream's
/// "build123d, matplotlib, numpy" fallback while no verification has
/// succeeded (AgentSession.swift:206).
fn prompt_ctx(root: &Path, settings: &Settings) -> PromptCtx {
    const ISO_DATE: &[time::format_description::FormatItem] =
        format_description!("[year]-[month]-[day]");
    let python_display = crate::python::env::versions_display();
    PromptCtx {
        project_name: root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".to_string()),
        python_display: python_display.clone(),
        venv_python: app_venv_python(),
        environment: environment_bullets(root, &python_display),
        units: settings.measurement_unit.prompt_name().to_string(),
        printer: settings
            .printer_model
            .clone()
            .unwrap_or_else(|| "not specified".to_string()),
        guidelines: settings.printer_type.prompt_guidelines().to_string(),
        date: OffsetDateTime::now_utc()
            .format(&ISO_DATE)
            .unwrap_or_default(),
    }
}

/// The executable this session spawns: the test/embedder override, else the
/// locator's discovery for `engine` from that engine's custom path setting
/// (upstream `AgentLocator.current` picks the locator by engine).
fn resolve_executable(engine: AgentKind, settings: &Settings) -> Option<AgentPath> {
    if let Some(path) = std::env::var_os(TEST_CLAUDE_ENV) {
        let path = PathBuf::from(path);
        return locator::is_executable_file(&path).then_some(AgentPath {
            path,
            version: String::new(),
        });
    }
    locator::find_agent(engine, settings.agent_path(engine))
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
    /// Translate one raw stdout line (AgentSession.swift:104-109's
    /// `handle`): the codex engine routes the line through the translator
    /// (one line → zero or more events), the claude engine decodes it
    /// directly — then each event goes through the same `process` below.
    async fn on_stdout_line(&mut self, line: &str) {
        let events: Vec<StreamEvent> = match self.state.engine {
            AgentKind::Codex => self.state.translator.events(line),
            AgentKind::Claude => parse_line(line).into_iter().collect(),
        };
        for event in events {
            self.process(event).await;
        }
    }

    /// Fold one decoded stream event into chat rows and metadata
    /// (AgentSession.swift:110-145's `process`), emitting EngineEvents and
    /// persisting session ids as they are learned.
    async fn process(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::System(system) => match system.kind {
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
                        .send(EngineEvent::Started {
                            session_id: system.session_id,
                        })
                        .await;
                }
                SystemKind::ThinkingTokens => {
                    if let Some(tokens) = system.estimated_tokens {
                        let _ = self
                            .events
                            .send(EngineEvent::Thinking {
                                estimated_tokens: tokens,
                            })
                            .await;
                    }
                }
                _ => {}
            },
            StreamEvent::Stream { event } => match event.event {
                InnerEvent::ContentStart { index, block } => {
                    // any block start closes the streaming text row
                    self.state.streaming_text.clear();
                    match block {
                        ContentBlock::ToolUse { id, name, .. } => {
                            self.state.tool_ids.insert(index, id.clone());
                            self.state.seen_tools.insert(id.clone());
                            let title = live_title(&name, "");
                            let _ = self
                                .events
                                .send(EngineEvent::ToolStarted { id, name, title })
                                .await;
                        }
                        // thinking and redacted thinking open the same row
                        // shape (upstream BlockKind maps both to .thinking)
                        ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking => {
                            self.state.thinking_index = Some(index);
                            let _ = self.events.send(EngineEvent::ThinkingStarted).await;
                        }
                        ContentBlock::Text { .. } => {}
                    }
                }
                InnerEvent::ContentStop { index } => {
                    // only the open thinking block's stop closes the row
                    // (upstream stopBlock's `index == thinkingIndex` guard)
                    if self.state.thinking_index == Some(index) {
                        self.state.thinking_index = None;
                        let _ = self.events.send(EngineEvent::ThinkingFinished).await;
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
            StreamEvent::Assistant(assistant) => match assistant.block {
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
                        let title = summarize(&name, &input).into();
                        let _ = self
                            .events
                            .send(EngineEvent::ToolStarted { id, name, title })
                            .await;
                    }
                }
                _ => {}
            },
            StreamEvent::UserToolResult(result) => {
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
            StreamEvent::Result(result) => {
                if let Some(session_id) = &result.session_id {
                    self.metadata.session_id = Some(session_id.clone());
                    let _ = save_metadata(&self.root, &self.metadata);
                    self.state.session_id = Some(session_id.clone());
                }
                self.state.turn_result = Some(result);
            }
            // decoded enough to be skippable: rate limits, unknown shapes,
            // (the claude decode's None — process noise that is not a JSON
            // object at all — never reaches here; it becomes no events)
            StreamEvent::RateLimit | StreamEvent::Unknown(_) => {}
        }
    }
}

/// Classify the finished process (AgentSession.swift:264-303). `exit_code`
/// is the child's `ExitStatus::code()`.
fn outcome(state: &TurnState, stderr: &str, exit_code: Option<i32>) -> TurnOutcome {
    match state.turn_result.as_ref() {
        Some(result) if !result.is_error => TurnOutcome::Success {
            session_id: result
                .session_id
                .clone()
                .or_else(|| state.session_id.clone()),
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
            let stdout = if exit_code == Some(0) {
                ""
            } else {
                state.stdout_text.as_str()
            };
            let mut details: Vec<String> = [message, stderr.to_string(), stdout.to_string()]
                .into_iter()
                .map(|piece| piece.trim().to_string())
                .filter(|piece| !piece.is_empty())
                .collect();
            // the engine surfaces the API error status alongside the text,
            // as the language-neutral protocol code; upstream leaves it
            // implicit in the result line it never shows
            if let Some(status) = result.as_ref().and_then(|result| result.api_error_status) {
                details.push(format!("HTTP {status}"));
            }
            let details = details.join("\n");
            TurnOutcome::Failed {
                auth: is_authentication_error(&details),
                error: details,
                exit_code,
            }
        }
    }
}

/// Upstream `isBusySession`'s needle over the combined turn text.
pub fn is_busy_session(text: &str) -> bool {
    contains_ci(text, "already has an active writer")
}

/// Upstream `isMissingSession`'s needles.
pub fn is_missing_session(text: &str) -> bool {
    [
        "no conversation found",
        "no rollout found",
        "session not found",
        "thread not found",
    ]
    .iter()
    .any(|needle| contains_ci(text, needle))
}

/// Upstream `isAuthenticationError`'s needles, with one M4 amendment: the
/// bare `401` must stand as a whole word — "1401 tokens used" and "duration
/// 4.401s" are counts and latencies, not an auth status (upstream's plain
/// substring match mis-fires on both).
pub fn is_authentication_error(text: &str) -> bool {
    [
        "invalid api key",
        "/login",
        "not logged in",
        "authentication",
        "oauth token",
        "unauthorized",
        "codex login",
    ]
    .iter()
    .any(|needle| contains_ci(text, needle))
        || contains_word(text, "401")
}

/// `\b<needle>\b` for an ASCII needle: some occurrence whose neighboring
/// characters are both non-word (alphanumeric or `_`), or absent at the
/// string's edges.
fn contains_word(haystack: &str, needle: &str) -> bool {
    fn is_word(edge: Option<char>) -> bool {
        matches!(edge, Some(c) if c.is_ascii_alphanumeric() || c == '_')
    }
    haystack.match_indices(needle).any(|(at, _)| {
        !is_word(haystack[..at].chars().next_back())
            && !is_word(haystack[at + needle.len()..].chars().next())
    })
}

/// `localizedCaseInsensitiveContains` for ASCII needles.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}
