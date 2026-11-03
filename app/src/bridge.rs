//! The engine bridge: turn driving, the send queue, the build pipeline, and
//! the drain loop that carries [`EngineEvent`]s to the app layer — the port
//! of upstream's Project.swift turn plumbing (sendDraft/send at 177-200, the
//! queue at 72-73 and 279-304) and build plumbing (setParameter/build/
//! buildDidFinish at 343-416) wrapped around the engine's
//! [`Session::send`] lifecycle dance and [`run_model`] runner.
//!
//! The bridge is split in two halves the message stream joins:
//! - a [`Bridge`] is the app layer's command handle (`enqueue`/`cancel`/
//!   `resume_queue`, plus the build pipeline's `set_parameter`/`start_build`)
//!   with the synchronous guards upstream checks on the main actor — busy,
//!   python readiness, model.py presence — and the observed state the
//!   parameter panel and status line read (parameters, versions,
//!   `build_error`, `missing_outputs`, `activity`);
//! - a driver task owns the [`Session`], the FIFO queue, the pause flag, and
//!   the build drain, and turns engine events into [`Message`]s the app
//!   applies to its [`crate::chat::ChatState`] — the same events also fold
//!   into a driver-side transcript that `save_chat` persists at every turn
//!   end (upstream `agentDidFinish`'s `chat.save()`, Project.swift:262-278),
//!   so chat.json survives with no window open.
//!
//! The halves share one [`Pipeline`] cell behind a mutex: the handle's
//! guards must read the same busy flag the driver moves (upstream's
//! `isBusy` is `activity.isWorking`, one main-actor field both sides touch;
//! here the activity in the cell plays that role — a send or build marks it
//! `Working` synchronously before the command leaves, so no window exists
//! where a `set_parameter` can slip past a turn that is already starting).
//!
//! The drain law (plan Review Focus 1): commands are absorbed between EVERY
//! pair of event translations and selected against while waiting for the next
//! event, and the forward channel is unbounded — a slow consumer (or the
//! engine's 256-deep backpressure) can never wedge a cancel.
//!
//! Queue semantics (Project.swift:290-304): a send while a turn runs is
//! queued FIFO; after a successful turn the next queued send auto-runs after
//! 1.0 s; ANY other end — failure, cancellation (upstream reports cancel as
//! `continueQueue(succeeded: false)`), or the event channel closing without
//! a [`EngineEvent::TurnFinished`] — pauses the queue; `resume_queue` is
//! upstream `sendQueuedNow`. A send arriving while NO turn runs starts
//! immediately even with items queued (upstream `send` guards `isBusy`
//! only); the 1.0 s delay such a send preempts is re-armed at that turn's
//! end instead of firing between the two — at most one second of drift in
//! that race, deliberate. A build likewise eats a pending delay (the queue
//! re-arms at the build's end); an INTERRUPTED build leaves the queue
//! untouched, upstream `buildDidFinish`'s interrupt branch calling no
//! `continueQueue` at all.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use cosmic::iced::futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use engine::agent::attachments;
use engine::agent::claude::TurnRequest;
use engine::agent::locator::AgentKind;
use engine::agent::session::{EngineEvent, Session, TurnHandle, TurnOutcome};
use engine::project::manifest::Manifest;
use engine::project::outputs::missing_files;
use engine::project::parameters::{
    ModelParameter, ParameterError, ordered_parameters, set_parameter as rewrite_parameter,
};
use engine::project::store::{self, Action, ChatMessage, ProjectMetadata, Role};
use engine::project::versions::{self, ProjectVersion};
use engine::python::env::{EnvState, interpreter_path, venv_dir};
use engine::python::model::{BuildEvent, BuildHandle, BuildOutcome, run_model};
use engine::settings::{Settings, settings_path};
use time::OffsetDateTime;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{self, UnboundedReceiver as CmdReceiver, UnboundedSender as CmdSender};
use uuid::Uuid;

use crate::chat::ChatState;
use crate::fl;

/// Where a turn's engine name comes from: the settings file re-read at turn
/// start (Ruling 1 — the engine picker's write is decisive for the NEXT
/// turn, not the next project), falling back to the launch snapshot when
/// the test seam suspends the re-read. Mirrors the engine's own
/// `Session::with_settings_file` rule: a scripted agent must not have the
/// developer's real settings file decide its turn's name.
#[derive(Debug, Clone)]
struct EngineSource {
    /// The settings file re-read per turn; `None` under the test seam.
    file: Option<PathBuf>,
    /// The launch snapshot's engine — the answer when `file` is `None`.
    fallback: AgentKind,
}

impl EngineSource {
    fn at_launch(settings: &Settings) -> Self {
        Self {
            file: (std::env::var_os("CAD_TEST_CLAUDE").is_none()).then(settings_path),
            fallback: settings.agent_kind(),
        }
    }

    /// The engine the next turn runs under. A missing settings file reads
    /// as a fresh install (claude, `Settings::load`'s rule — NOT the
    /// snapshot); the snapshot answers only when the re-read is suspended
    /// or the read actually errors.
    fn current(&self) -> AgentKind {
        self.file
            .as_deref()
            .and_then(|path| Settings::load(path).ok())
            .map_or(self.fallback, |settings| settings.agent_kind())
    }
}

/// Upstream `Activity` (Project.swift:19-26): the status line the app
/// renders, and the sole busy flag — `isBusy` is `activity.isWorking`.
#[derive(Debug, Clone, PartialEq)]
pub enum Activity {
    Idle,
    Working(String),
    Succeeded(String),
    Failed(String),
}

impl Activity {
    /// Upstream `Activity.isWorking`.
    pub fn is_working(&self) -> bool {
        matches!(self, Activity::Working(_))
    }
}

/// The bridge's activity classification, as the header's pure model sees
/// it — two enums of the same upstream shape (the pipeline's actor-side
/// field, [`crate::activity::Activity`] the ActivityViewer's model); the
/// window folds terminal/build states across through this.
impl From<Activity> for crate::activity::Activity {
    fn from(activity: Activity) -> Self {
        match activity {
            Activity::Idle => Self::Idle,
            Activity::Working(text) => Self::Working(text),
            Activity::Succeeded(text) => Self::Succeeded(text),
            Activity::Failed(text) => Self::Failed(text),
        }
    }
}

/// Why a pipeline command refused (upstream's guard branches in
/// `setParameter`/`build`/`send`, Project.swift:343-362 and 373-391).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildRefusal {
    /// A turn or build is running (upstream `isBusy`).
    Busy,
    /// The Python environment is not verified — the setup sheet follows
    /// (upstream `PythonEnvironment.shared.isSetupSheetPresented = true`).
    PythonNotReady,
    /// No model.py in the project root (upstream `hasModelScript`).
    NoModelScript,
}

/// The state the handle and the driver share (upstream keeps all of this on
/// the one main-actor `Project` both sides touch). The activity doubles as
/// the busy flag; `draft` holds a refused send's text (upstream
/// `chat.draft = text`) until the next send takes it.
struct Pipeline {
    python_state: EnvState,
    activity: Activity,
    versions: Vec<ProjectVersion>,
    parameters: Vec<ModelParameter>,
    build_error: Option<String>,
    missing_outputs: Vec<String>,
    sheet_requested: bool,
    draft: Option<String>,
    /// The compare picker's selection (`None` = "Nessuna"); the overlay's
    /// previous values below parse from that version's stored model.py.
    compare_version: Option<i64>,
    comparison_parameters: BTreeMap<String, f64>,
}

impl Pipeline {
    /// The observed state a fresh project opens with: the version history,
    /// the manifest's parameter list, and the outputs still missing. The
    /// activity opens Idle by construction: nothing persists in-flight turn
    /// state (a mid-turn crash leaves only the chat's trailing user row and
    /// the stored session id), and a previous process's agent subprocess
    /// cannot exist by the time this loads — the app reaps orphans at
    /// startup — so a cold start never believes a turn is in flight.
    /// [`crate::bridge::tests::cold_start_over_crashed_store_starts_turns`]
    /// pins the whole law end to end.
    fn load(root: &Path) -> Self {
        Self {
            python_state: EnvState::Unknown,
            activity: Activity::Idle,
            versions: versions::load_versions(root),
            parameters: refresh_parameters(root),
            build_error: None,
            missing_outputs: missing_files(&root.join("output")),
            sheet_requested: false,
            draft: None,
            compare_version: None,
            comparison_parameters: BTreeMap::new(),
        }
    }

    fn busy(&self) -> bool {
        self.activity.is_working()
    }

    fn python_ready(&self) -> bool {
        matches!(self.python_state, EnvState::Ready { .. })
    }

    /// Upstream `send()`'s pre-task state (Project.swift:201-203): the stale
    /// build error clears and the working activity opens, naming the engine
    /// the turn runs under. EVERY turn start runs this — a handle-issued
    /// send, the 1 s auto-run, and a resume all go through the same
    /// `send()` upstream, and the busy flag must not care who started the
    /// turn.
    fn begin_turn(&mut self, engine: AgentKind) {
        self.build_error = None;
        self.activity = Activity::Working(fl!("sta-lavorando", agent = engine.display_name()));
    }
}

/// The manifest's parameters in model.py order — the panel list the bridge
/// refreshes after every build and turn end (upstream `output.reload()` +
/// `orderedParameters`). Pure: a missing manifest or model.py reads as no
/// parameters.
pub fn refresh_parameters(root: &Path) -> Vec<ModelParameter> {
    match Manifest::load(&root.join("output").join("manifest.json")) {
        Ok(manifest) => {
            let script = std::fs::read_to_string(root.join("model.py")).unwrap_or_default();
            ordered_parameters(&manifest.parameters, &script)
        }
        Err(_) => Vec::new(),
    }
}

/// The CAD contract every agent turn runs under — upstream loads
/// `CADSystemPrompt.md` from its app bundle and passes an empty string when
/// the resource is missing; an empty template here would mean turns without
/// the contract, so [`Bridge`] is built with this verbatim copy of the
/// upstream fixture.
pub const SYSTEM_TEMPLATE: &str = include_str!("../resources/CADSystemPrompt.md");

/// The gap between a successful turn and the next queued send
/// (Project.swift:296-300's `Task.sleep(for: .seconds(1))`).
const QUEUE_DELAY: Duration = Duration::from_secs(1);

/// What the driver reports to the app layer. One variant per engine event,
/// plus the lifecycle edges the events cannot carry: a turn starting (the
/// user row to append — upstream appends it in `send`, before the agent
/// runs), the event channel closing without a terminal (fail open and leave
/// the running state), and queue/pause changes (upstream exposes
/// `queue`/`isQueuePaused` as observed state the queue list renders).
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A send actually started (never a queued one): append the user row.
    /// `attachments` carries the `references/<stamp>-<n>-<name>` COPY names
    /// the bridge just made (upstream's row stores the copies,
    /// Project.swift:201) — render them root-relative. `engine` is the
    /// engine the turn runs under — the per-turn re-read's answer at turn
    /// start, so the window names the turn's rows with it.
    TurnStarted {
        prompt: String,
        attachments: Vec<String>,
        engine: AgentKind,
    },
    /// One engine event, in arrival order — apply to the chat state.
    Event(EngineEvent),
    /// The event channel closed without a [`EngineEvent::TurnFinished`]:
    /// the running tools have been failed open and saved; leave the
    /// running state.
    Closed,
    /// The queue's contents and pause state after a change.
    QueueChanged {
        queue: Vec<QueuedItem>,
        paused: bool,
    },
    /// A slider commit rewrote model.py (before any build it may trigger).
    ParameterCommitted { name: String, value: f64 },
    /// One build event, in arrival order — the build's forward of each
    /// streamed output line and its finish, parallel to [`Message::Event`].
    BuildEventBridge(engine::python::model::BuildEvent),
    /// A build ended and its disk writes (failure row, version snapshot,
    /// metadata) have landed — the outcome is the runner's classification.
    BuildFinished {
        outcome: BuildOutcome,
        version_prompt: Option<String>,
    },
    /// A successful turn's tail (refresh, version recording, metadata save)
    /// has landed on disk (upstream `agentDidFinish`, Project.swift:262-278).
    TurnSuccessBookkeeping,
    /// A command refused on python readiness: present the setup sheet
    /// (the bridge never touches windows; the app maps this to T8's sheet).
    PythonSheetRequested,
}

/// One command from the app layer to the driver.
enum Command {
    Enqueue {
        prompt: String,
        attachments: Vec<String>,
    },
    Cancel,
    ResumeQueue,
    /// Drop one queued send by id (upstream `removeQueued`,
    /// Project.swift:278-283): the queue un-pauses when this empties it.
    RemoveQueued(Uuid),
    /// A build whose guards already passed: the handle the bridge spawned
    /// (upstream `task = Task { … }`), for the driver to drain.
    StartBuild {
        handle: BuildHandle,
        version_prompt: Option<String>,
    },
}

/// A queued send (upstream `QueuedMessage`): the FIFO entry the queue holds
/// while a turn runs, carrying the ORIGINAL request paths — the copies are
/// made when the turn starts, never at enqueue (Project.swift:177-190). The
/// id is the identity the queue list renders and [`Bridge::remove_queued`]
/// addresses (upstream's items are full `ChatMessage`s with UUIDs).
#[derive(Debug, Clone, PartialEq)]
struct QueuedSend {
    id: Uuid,
    prompt: String,
    attachments: Vec<String>,
}

/// A queued send as the app layer sees the queue (upstream exposes the full
/// `ChatMessage` items; the shell needs identity, preview text and the image
/// count).
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedItem {
    pub id: Uuid,
    pub text: String,
    pub images: usize,
}

impl From<&QueuedSend> for QueuedItem {
    fn from(send: &QueuedSend) -> Self {
        Self {
            id: send.id,
            text: send.prompt.clone(),
            images: crate::chat::image_attachments(&send.attachments),
        }
    }
}

/// The app layer's handle onto the driver: clone-free by design, so dropping
/// it is the shutdown signal (the driver cancels any running turn or build
/// and reaps the engine's process registry — upstream's quit path reaps
/// orphans).
pub struct Bridge {
    commands: CmdSender<Command>,
    out: UnboundedSender<Message>,
    pipeline: Arc<Mutex<Pipeline>>,
    root: PathBuf,
    interpreter: PathBuf,
    engine_source: EngineSource,
}

/// [`Bridge::with_interpreter`]'s pending launch — a named construction seam
/// so tests (and any embedder with its own interpreter) read as a sentence.
pub struct BridgeBuilder {
    interpreter: PathBuf,
}

impl BridgeBuilder {
    /// [`Bridge::launch`] with the builder's interpreter.
    pub fn launch(
        self,
        settings: Settings,
        root: impl Into<PathBuf>,
    ) -> (Bridge, UnboundedReceiver<Message>) {
        Bridge::launch_with(self.interpreter, settings, root)
    }
}

impl Bridge {
    /// The bridge plus the message stream to return from the app's `init` or
    /// `update`. Must be called inside the tokio runtime cosmic runs on.
    pub fn new(settings: Settings, root: impl Into<PathBuf>) -> (Self, cosmic::Task<Message>) {
        let (bridge, messages) = Self::launch(settings, root);
        (bridge, cosmic::task::stream(messages))
    }

    /// [`Bridge::new`] without the `cosmic::Task` wrapper: spawns the driver
    /// and returns its message stream raw. A `cosmic::Task`'s stream is
    /// private to the iced runtime, so headless drivers (tests, the Task 8
    /// slice) tap here — the same seam pattern as the engine's
    /// `CAD_TEST_CLAUDE` override.
    pub fn launch(
        settings: Settings,
        root: impl Into<PathBuf>,
    ) -> (Self, UnboundedReceiver<Message>) {
        Self::launch_with(default_interpreter(), settings, root)
    }

    fn launch_with(
        interpreter: PathBuf,
        settings: Settings,
        root: impl Into<PathBuf>,
    ) -> (Self, UnboundedReceiver<Message>) {
        let root = root.into();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (msg_tx, msg_rx) = unbounded();
        let pipeline = Arc::new(Mutex::new(Pipeline::load(&root)));
        let engine_source = EngineSource::at_launch(&settings);
        let mut session = Session::new(settings).with_system_template(SYSTEM_TEMPLATE);
        // The per-turn settings re-read (the engine picker flips the runner
        // mid-conversation, Ruling 1) — attached only when a real agent
        // runs: a scripted agent under test (the CAD_TEST_CLAUDE seam)
        // suspends it so the developer's own settings file cannot decide a
        // test turn. The app-side name source follows the same rule.
        if std::env::var_os("CAD_TEST_CLAUDE").is_none() {
            session = session.with_settings_file(settings_path());
        }
        tokio::spawn(
            Drive::new(
                root.clone(),
                cmd_rx,
                msg_tx.clone(),
                Arc::clone(&pipeline),
                engine_source.clone(),
            )
            .run(session),
        );
        (
            Self {
                commands: cmd_tx,
                out: msg_tx,
                pipeline,
                root,
                interpreter,
                engine_source,
            },
            msg_rx,
        )
    }

    /// Override the interpreter builds run (default: the shared venv's
    /// `bin/python`). Tests point this at the fake-python wrapper — the
    /// same seam pattern as the engine's `CAD_TEST_CLAUDE` override.
    pub fn with_interpreter(interpreter: impl Into<PathBuf>) -> BridgeBuilder {
        BridgeBuilder {
            interpreter: interpreter.into(),
        }
    }

    fn pipeline(&self) -> MutexGuard<'_, Pipeline> {
        self.pipeline.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Send a message: runs immediately when no turn is running, joins the
    /// queue FIFO otherwise (upstream `sendDraft`). Refused while python is
    /// not ready — the draft is kept, the setup sheet requested, and the
    /// refusal named in the activity (upstream `send`, Project.swift:192-196,
    /// plus the status line every other refusal here already gets). The
    /// prompt is the user's text verbatim — the engine builds the full turn
    /// prompt around it.
    pub fn enqueue(&self, prompt: String, attachments: Vec<String>) -> Result<(), BuildRefusal> {
        {
            let mut pipeline = self.pipeline();
            if pipeline.busy() {
                // upstream sendDraft's busy branch: onto the queue
            } else if !pipeline.python_ready() {
                pipeline.draft = Some(prompt);
                pipeline.sheet_requested = true;
                pipeline.activity = Activity::Failed(fl!("python-setup-required"));
                let _ = self.out.unbounded_send(Message::PythonSheetRequested);
                return Err(BuildRefusal::PythonNotReady);
            } else {
                // the handle-side half of send()'s pre-task state: marking
                // busy HERE keeps the flag ahead of the driver's turn start
                // (the driver-side half lives in start_turn, which every
                // turn-start path reaches)
                pipeline.begin_turn(self.engine_source.current());
                pipeline.draft = None;
            }
        }
        let _ = self.commands.send(Command::Enqueue {
            prompt,
            attachments,
        });
        Ok(())
    }

    /// Drop one queued send (upstream `removeQueued`, Project.swift:278-283):
    /// the running turn is untouched; the queue un-pauses when this empties
    /// it.
    pub fn remove_queued(&self, id: Uuid) {
        let _ = self.commands.send(Command::RemoveQueued(id));
    }

    /// Commit one parameter: rewrites model.py, then rebuilds (upstream
    /// `setParameter`, Project.swift:343-362 — busy is the ONLY refusal; the
    /// rewrite happens whatever python's state, and the build it triggers is
    /// the one that refuses on readiness, sheet included).
    pub fn set_parameter(&mut self, name: &str, value: f64) -> Result<(), BuildRefusal> {
        if self.pipeline().busy() {
            return Err(BuildRefusal::Busy);
        }
        // upstream's guard reads the script with `try?` (Project.swift:344):
        // ANY read failure — missing, unreadable, any kind — returns
        // silently before anything happens. The pre-read is what splits
        // read from write failures downstream; the second read inside the
        // engine helper is the price of not widening its error type.
        if std::fs::read_to_string(self.root.join("model.py")).is_err() {
            return Ok(());
        }
        match rewrite_parameter(&self.root, name, value) {
            Ok(rewrite) => {
                {
                    let mut pipeline = self.pipeline();
                    if let Some(row) = pipeline.parameters.iter_mut().find(|p| p.name == name) {
                        row.value = value;
                    }
                }
                let _ = self.out.unbounded_send(Message::ParameterCommitted {
                    name: name.to_owned(),
                    value,
                });
                // the build's own guards refuse (and request the sheet)
                // with the rewrite already on disk — upstream parity
                let _ = self.start_build(Some(format!("{name} = {}", rewrite.literal)));
                Ok(())
            }
            Err(ParameterError::NotFound { .. }) => {
                self.pipeline().activity =
                    Activity::Failed(fl!("parametro-non-trovato-in-model-py", name = name));
                Ok(())
            }
            // the read above succeeded, so an io error now is the WRITE
            // failing — upstream's catch sets the failed activity
            Err(ParameterError::Io(err)) => {
                self.pipeline().activity = Activity::Failed(err.to_string());
                Ok(())
            }
        }
    }

    /// Build model.py with the project interpreter (upstream `build`,
    /// Project.swift:373-391): busy, python readiness, and model.py
    /// presence guard, then the working activity and a clear `build_error`
    /// before the runner spawns.
    pub fn start_build(&mut self, version_prompt: Option<String>) -> Result<(), BuildRefusal> {
        {
            let mut pipeline = self.pipeline();
            if pipeline.busy() {
                return Err(BuildRefusal::Busy);
            }
            if !pipeline.python_ready() {
                pipeline.sheet_requested = true;
                // same surface as enqueue's refusal: without it a refused
                // build/parameter commit leaves no status-line trace (the
                // window folds this activity at both call sites)
                pipeline.activity = Activity::Failed(fl!("python-setup-required"));
                let _ = self.out.unbounded_send(Message::PythonSheetRequested);
                return Err(BuildRefusal::PythonNotReady);
            }
        }
        if !self.root.join("model.py").exists() {
            self.pipeline().activity = Activity::Failed(fl!("nessun-model-py-da-compilare"));
            return Err(BuildRefusal::NoModelScript);
        }
        match run_model(&self.interpreter, &self.root) {
            Ok(handle) => {
                {
                    let mut pipeline = self.pipeline();
                    pipeline.activity = Activity::Working(fl!("compilazione-di-model-py"));
                    pipeline.build_error = None;
                }
                let _ = self.commands.send(Command::StartBuild {
                    handle,
                    version_prompt,
                });
                Ok(())
            }
            // upstream build()'s catch: a spawn failure sets the build
            // error without a process ever running
            Err(err) => {
                let mut pipeline = self.pipeline();
                pipeline.build_error = Some(err.to_string());
                pipeline.activity = Activity::Failed(fl!("errore-nello-script"));
                Ok(())
            }
        }
    }

    /// The build-failure row's action (upstream `askClaudeToFix`,
    /// Project.swift:258-260): the traceback goes back as an ordinary turn.
    pub fn ask_agent_to_fix(&self, traceback: &str) -> Result<(), BuildRefusal> {
        self.enqueue(
            fl!(
                "l-esecuzione-di-model-py-non-riesce-con-questo-errore-correggi-lo-script",
                error = traceback
            ),
            Vec::new(),
        )
    }

    /// Swap a recorded version back in (upstream `restore`,
    /// Project.swift:308-321): refuses while busy; on success the metadata
    /// points at the restored version (its session id, its number) and the
    /// observed state reloads — parameters re-read from the restored
    /// manifest, `build_error` and `missing_outputs` cleared, the activity
    /// announcing the restore. An engine failure surfaces as the failed
    /// activity plus `build_error`, like upstream's catch branch.
    ///
    /// Parity note: upstream does NOT reset `compareVersion` on restore —
    /// the picker's selection (and the values parsed from that version)
    /// survive the swap, so the port keeps them too.
    pub fn restore(&mut self, number: i64) -> Result<(), BuildRefusal> {
        if self.pipeline().busy() {
            return Err(BuildRefusal::Busy);
        }
        // the menu only offers recorded numbers; a miss (a history that
        // changed under the app) takes the failure branch, not a panic
        let outcome = self
            .pipeline()
            .versions
            .iter()
            .find(|version| version.number == number)
            .map(|version| {
                versions::restore(&self.root, version)
                    .map(|()| version.session_id.clone())
                    .map_err(|err| err.to_string())
            })
            .unwrap_or_else(|| Err(fl!("version-not-in-history", version = number)));
        match outcome {
            Ok(session_id) => {
                let mut metadata = fresh_metadata(&self.root);
                metadata.session_id = session_id;
                metadata.current_version = Some(number);
                let _ = store::save_metadata(&self.root, &metadata);
                let mut pipeline = self.pipeline();
                pipeline.parameters = refresh_parameters(&self.root);
                pipeline.missing_outputs = Vec::new();
                pipeline.build_error = None;
                pipeline.activity =
                    Activity::Succeeded(fl!("versione-lld-ripristinata", version = number));
                Ok(())
            }
            Err(err) => {
                let mut pipeline = self.pipeline();
                pipeline.build_error = Some(err);
                pipeline.activity = Activity::Failed(fl!(
                    "impossibile-ripristinare-la-versione-lld",
                    version = number
                ));
                Ok(())
            }
        }
    }

    /// The compare picker's selection (upstream `compareVersion`): store
    /// the choice and parse the previous parameter values from that
    /// version's stored model.py copy — a slider rewrite never regenerates
    /// the manifest, so the model source is the only value that stays
    /// truthful. `None` ("Nessuna") empties them. The ghost MESH the
    /// orange values annotate is M6's, documented out of scope here.
    pub fn compare_with(&mut self, number: Option<i64>) {
        let mut pipeline = self.pipeline();
        pipeline.comparison_parameters = versions::comparison_parameters(&self.root, number);
        pipeline.compare_version = number;
    }

    /// Interrupt the running turn or build (SIGINT now, SIGTERM after 3 s) —
    /// upstream `stop()`'s one `task` owns both. The running one only: the
    /// queue is untouched and pauses when the cancelled turn ends.
    pub fn cancel(&self) {
        let _ = self.commands.send(Command::Cancel);
    }

    /// Un-pause the queue and start its next message now (upstream
    /// `sendQueuedNow`).
    pub fn resume_queue(&self) {
        let _ = self.commands.send(Command::ResumeQueue);
    }

    /// Whether the python environment is verified (upstream
    /// `PythonEnvironment.shared.isReady`).
    pub fn python_ready(&self) -> bool {
        self.pipeline().python_ready()
    }

    /// The parameter panel's rows, manifest order (upstream `parameters`).
    pub fn parameters(&self) -> Vec<ModelParameter> {
        self.pipeline().parameters.clone()
    }

    /// The last build's error line (upstream `buildError`).
    pub fn build_error(&self) -> Option<String> {
        self.pipeline().build_error.clone()
    }

    /// The expected outputs not on disk (upstream `missingOutputs`).
    pub fn missing_outputs(&self) -> Vec<String> {
        self.pipeline().missing_outputs.clone()
    }

    /// The recorded version history (upstream `history.versions`).
    pub fn versions(&self) -> Vec<ProjectVersion> {
        self.pipeline().versions.clone()
    }

    /// The compare picker's selection (`None` = "Nessuna").
    pub fn compare_version(&self) -> Option<i64> {
        self.pipeline().compare_version
    }

    /// The compared version's parameter values (owned — the pipeline lives
    /// behind a mutex, so a reference cannot outlive the read).
    pub fn comparison_parameters(&self) -> BTreeMap<String, f64> {
        self.pipeline().comparison_parameters.clone()
    }

    /// The status line (upstream `activity`).
    pub fn activity(&self) -> Activity {
        self.pipeline().activity.clone()
    }

    /// The app's python-state mirror (upstream's `PythonEnvironment.shared`
    /// singleton: one environment, every Project observes it). The setup
    /// sheet's reducer owns the state — the startup check's and installs'
    /// outcomes reach every open bridge through the app, keeping all the
    /// readiness guards in agreement.
    pub fn set_python_state(&self, state: EnvState) {
        self.pipeline().python_state = state;
    }

    /// Test seam for T8's startup check / sheet completion: mark the
    /// environment verified so guards pass without a real venv (production
    /// goes through [`Bridge::set_python_state`] with the sheet's state).
    pub fn mark_python_ready_for_test(&self) {
        self.pipeline().python_state = EnvState::Ready {
            build123d_version: String::new(),
        };
    }

    /// Test seam: whether a refused command requested the setup sheet.
    pub fn sheet_requested_for_test(&self) -> bool {
        self.pipeline().sheet_requested
    }

    /// Drop the draft a refused send parked here — the input bar re-staged
    /// the text (upstream's `chat.draft` IS the input bar's; the window
    /// owns the one live copy, so the parked one must not linger where a
    /// later refusal could resurrect stale text).
    pub fn clear_parked_draft(&mut self) {
        self.pipeline().draft = None;
    }

    /// Test seam: the draft a refused send kept (upstream `chat.draft`).
    pub fn draft_text_for_test(&self) -> Option<String> {
        self.pipeline().draft.clone()
    }
}

/// The shared venv's interpreter — the one path ambient (every other path in
/// the bridge is injected).
fn default_interpreter() -> PathBuf {
    interpreter_path(&venv_dir())
}

fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The project's metadata, loaded now — the load-fresh law EVERY writer of
/// `project.json` follows, the shell's save points included (see
/// [`Drive::record_version`] for why: the engine writes `session_id` into
/// the same file mid-turn, and a copy held from launch would clobber it).
pub fn fresh_metadata(root: &Path) -> ProjectMetadata {
    store::load_metadata(root).unwrap_or_else(|_| ProjectMetadata::new(app_version()))
}

/// How a drained turn ended, for the driver's queue decision.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TurnEnd {
    /// A [`EngineEvent::TurnFinished`] arrived; `succeeded` is upstream's
    /// `failure == nil` (false for every failure AND cancellation).
    Finished { succeeded: bool },
    /// The event channel closed with no terminal event.
    Closed,
    /// The bridge handle was dropped mid-turn; the driver exits.
    Shutdown,
}

/// How a drained build ended — the runner's terminal classification, or the
/// channel/engine-task deaths a turn's [`TurnEnd`] mirrors.
enum BuildEnd {
    /// A [`BuildEvent::Finished`] arrived with the outcome.
    Finished(BuildOutcome),
    /// The event channel closed with no terminal event.
    Closed,
    /// The bridge handle was dropped mid-build; the driver exits.
    Shutdown,
}

/// The driver's phase: idle, draining a turn, draining a build, or in the
/// 1.0 s gap between a successful turn and the next queued send. The gap is
/// a deadline, not a spawned sleep, so re-entering the phase (a cancel
/// arriving mid-gap) restarts the wait where it left off.
enum Phase {
    Idle,
    Running {
        handle: TurnHandle,
        transcript: ChatState,
    },
    Building {
        handle: BuildHandle,
        version_prompt: Option<String>,
    },
    Delay {
        auto_at: tokio::time::Instant,
    },
}

/// The driver: everything the message stream's far side owns — the session,
/// the queue, the pause flag, the build drains, and the transcript that
/// persists. One `run` call is the bridge's lifetime.
struct Drive {
    root: PathBuf,
    queue: VecDeque<QueuedSend>,
    paused: bool,
    cmds: CmdReceiver<Command>,
    out: UnboundedSender<Message>,
    pipeline: Arc<Mutex<Pipeline>>,
    engine_source: EngineSource,
}

impl Drive {
    fn new(
        root: PathBuf,
        cmds: CmdReceiver<Command>,
        out: UnboundedSender<Message>,
        pipeline: Arc<Mutex<Pipeline>>,
        engine_source: EngineSource,
    ) -> Self {
        Self {
            root,
            queue: VecDeque::new(),
            paused: false,
            cmds,
            out,
            pipeline,
            engine_source,
        }
    }

    fn pipeline(&self) -> MutexGuard<'_, Pipeline> {
        self.pipeline.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn run(mut self, mut session: Session) {
        let mut phase = Phase::Idle;
        loop {
            phase = match phase {
                Phase::Idle => match self.cmds.recv().await {
                    Some(Command::Enqueue {
                        prompt,
                        attachments,
                    }) => self.start_turn(
                        &mut session,
                        QueuedSend {
                            id: Uuid::new_v4(),
                            prompt,
                            attachments,
                        },
                    ),
                    // nothing is running: upstream's stop is a no-op
                    Some(Command::Cancel) => Phase::Idle,
                    // removing from an empty-idle queue can only un-pause
                    Some(Command::RemoveQueued(_)) => {
                        self.paused = false;
                        self.notify_queue();
                        Phase::Idle
                    }
                    Some(Command::StartBuild {
                        handle,
                        version_prompt,
                    }) => Phase::Building {
                        handle,
                        version_prompt,
                    },
                    Some(Command::ResumeQueue) => {
                        self.paused = false;
                        self.notify_queue();
                        self.send_next_queued(&mut session)
                    }
                    None => {
                        engine::processes::reap_orphans();
                        return;
                    }
                },
                Phase::Running {
                    mut handle,
                    transcript,
                } => {
                    // split the receiver from the interrupt switch once: the
                    // drain loop selects on the receiver while calling
                    // `cancel`, two borrows of one handle the borrow checker
                    // cannot express through `&TurnHandle`. The dummy
                    // receiver left in the handle is never polled.
                    let mut events = std::mem::replace(&mut handle.events, mpsc::channel(1).1);
                    let end = self
                        .drain_turn(&mut events, || handle.cancel(), transcript)
                        .await;
                    match end {
                        TurnEnd::Finished { succeeded } => self.continue_queue(succeeded),
                        TurnEnd::Closed => self.continue_queue(false),
                        TurnEnd::Shutdown => {
                            engine::processes::reap_orphans();
                            return;
                        }
                    }
                }
                Phase::Building {
                    mut handle,
                    version_prompt,
                } => {
                    // the same receiver/interrupt split a turn's drain needs
                    let mut events = std::mem::replace(&mut handle.events, mpsc::channel(1).1);
                    let end = self.drain_build(&mut events, || handle.cancel()).await;
                    match end {
                        BuildEnd::Shutdown => {
                            engine::processes::reap_orphans();
                            return;
                        }
                        _ => self.finish_build(end, version_prompt),
                    }
                }
                Phase::Delay { auto_at } => tokio::select! {
                    _ = tokio::time::sleep_until(auto_at) => self.send_next_queued(&mut session),
                    cmd = self.cmds.recv() => match cmd {
                        // no turn is running, so a direct send starts now —
                        // and eats the pending delay: its own turn end
                        // re-arms the queue's next step instead. A build
                        // eats it the same way; the build's end re-arms.
                        Some(Command::Enqueue { prompt, attachments }) => {
                            self.start_turn(
                                &mut session,
                                QueuedSend {
                                    id: Uuid::new_v4(),
                                    prompt,
                                    attachments,
                                },
                            )
                        }
                        Some(Command::StartBuild { handle, version_prompt }) => {
                            Phase::Building { handle, version_prompt }
                        }
                        // nothing to interrupt; the gap keeps running
                        Some(Command::Cancel) => Phase::Delay { auto_at },
                        // an emptied queue un-pauses and ends the gap
                        Some(Command::RemoveQueued(id)) => {
                            self.queue.retain(|send| send.id != id);
                            if self.queue.is_empty() {
                                self.paused = false;
                                self.notify_queue();
                                Phase::Idle
                            } else {
                                self.notify_queue();
                                Phase::Delay { auto_at }
                            }
                        }
                        Some(Command::ResumeQueue) => {
                            self.paused = false;
                            self.notify_queue();
                            self.send_next_queued(&mut session)
                        }
                        None => {
                            engine::processes::reap_orphans();
                            return;
                        }
                    },
                },
            };
        }
    }

    /// One command applied outside a drain loop's reach. `cancel` is the
    /// running turn's or build's interrupt switch, split from the event
    /// receiver the drain loop owns.
    fn absorb(&mut self, cmd: Command, cancel: &impl Fn()) {
        match cmd {
            Command::Cancel => cancel(),
            Command::Enqueue {
                prompt,
                attachments,
            } => {
                self.queue.push_back(QueuedSend {
                    id: Uuid::new_v4(),
                    prompt,
                    attachments,
                });
                self.notify_queue();
            }
            // the handle's busy guard refuses a start while anything runs;
            // a handle reaching a drain means the guard raced the turn's
            // start — refuse it late by cancelling
            Command::StartBuild { handle, .. } => handle.cancel(),
            // upstream `sendNextQueued` guards `isBusy`: the flag clears now,
            // the queue moves at this turn's end
            Command::ResumeQueue => {
                self.paused = false;
                self.notify_queue();
            }
            // upstream `removeQueued`: drop the entry, and an emptied queue
            // loses its pause (Project.swift:278-283)
            Command::RemoveQueued(id) => {
                self.queue.retain(|send| send.id != id);
                if self.queue.is_empty() {
                    self.paused = false;
                }
                self.notify_queue();
            }
        }
    }

    /// Start a turn (upstream `send`, Project.swift:192-208): copy the
    /// attachments into `references/` FIRST — before the user row is
    /// appended, exactly upstream's order — then append the row carrying
    /// the COPY names, save, announce, and hand the request to the engine's
    /// lifecycle dance. Every turn-start path lands here (a direct send, the
    /// 1 s auto-run, a resume), so a QUEUED send's copies are made here, at
    /// its own start, never at enqueue (Project.swift:177-190). The bridge
    /// is the single writer of `references/`: the engine's session consumes
    /// the names and writes nothing. The turn's ENGINE is resolved here too
    /// (the per-turn re-read, Ruling 1) and rides the transcript and the
    /// announcement, so the turn's rows and activity name the engine that
    /// actually runs them, whatever the settings say by the time they land.
    /// The transcript reloads from disk — the last turn's save is this
    /// turn's base, exactly the crash-recovery shape `load_chat`'s
    /// `unwrap_or_default` gives a fresh project.
    fn start_turn(&mut self, session: &mut Session, send: QueuedSend) -> Phase {
        let engine = self.engine_source.current();
        self.pipeline().begin_turn(engine);
        let sources: Vec<PathBuf> = send.attachments.iter().map(PathBuf::from).collect();
        let references = attachments::copy_attachments(&sources, &self.root);
        let images: Vec<PathBuf> = references.iter().map(|name| self.root.join(name)).collect();
        let mut transcript = ChatState::new(store::load_chat(&self.root).unwrap_or_default());
        transcript.engine = engine;
        transcript.append(ChatMessage {
            id: Uuid::new_v4(),
            role: Role::User,
            text: send.prompt.clone(),
            date: OffsetDateTime::now_utc(),
            tool: None,
            detail: None,
            action: None,
            // the references/ copies upstream's row stores (Project.swift:201)
            attachments: references.clone(),
        });
        let _ = store::save_chat(&self.root, &transcript.messages);
        let _ = self.out.unbounded_send(Message::TurnStarted {
            prompt: send.prompt.clone(),
            attachments: references.clone(),
            engine,
        });
        let request = TurnRequest {
            prompt: send.prompt,
            references,
            images,
        };
        let handle = session.send(&self.root, &request);
        Phase::Running { handle, transcript }
    }

    /// Pop the queue's front and start it — upstream `sendNextQueued`'s
    /// guards minus `isBusy` (only ever called from idle states).
    fn send_next_queued(&mut self, session: &mut Session) -> Phase {
        if self.paused {
            return Phase::Idle;
        }
        match self.queue.pop_front() {
            Some(send) => {
                self.notify_queue();
                self.start_turn(session, send)
            }
            None => Phase::Idle,
        }
    }

    /// Upstream `continueQueue(succeeded:)` (Project.swift:290-300): an empty
    /// queue needs nothing, a failure pauses, a success schedules the next
    /// send after the delay.
    fn continue_queue(&mut self, succeeded: bool) -> Phase {
        if self.queue.is_empty() {
            return Phase::Idle;
        }
        if !succeeded {
            self.paused = true;
            self.notify_queue();
            return Phase::Idle;
        }
        Phase::Delay {
            auto_at: tokio::time::Instant::now() + QUEUE_DELAY,
        }
    }

    /// Announce the queue's observable state (upstream exposes
    /// `queue`/`isQueuePaused` directly; the driver's copy is unreachable
    /// from the app, so changes travel as messages).
    fn notify_queue(&self) {
        let _ = self.out.unbounded_send(Message::QueueChanged {
            queue: self.queue.iter().map(QueuedItem::from).collect(),
            paused: self.paused,
        });
    }

    /// Drain one turn's events: forward each as a [`Message::Event`] while
    /// folding it into the transcript, saving at the terminal. Commands are
    /// absorbed between every translation and selected against while idle —
    /// the cancel law. The event receiver is passed split from the handle
    /// (whose `cancel` the loop must reach while the receiver is borrowed);
    /// the dummy receiver left in the handle is never polled.
    async fn drain_turn(
        &mut self,
        events: &mut mpsc::Receiver<EngineEvent>,
        cancel: impl Fn(),
        mut transcript: ChatState,
    ) -> TurnEnd {
        loop {
            // between every pair of translations: absorb whatever arrived
            while let Ok(cmd) = self.cmds.try_recv() {
                self.absorb(cmd, &cancel);
            }
            let event = match events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => tokio::select! {
                    event = events.recv() => match event {
                        Some(event) => event,
                        None => {
                            self.close_open_turn(&mut transcript);
                            return TurnEnd::Closed;
                        }
                    },
                    cmd = self.cmds.recv() => match cmd {
                        Some(cmd) => {
                            self.absorb(cmd, &cancel);
                            continue;
                        }
                        None => {
                            cancel();
                            self.close_open_turn(&mut transcript);
                            return TurnEnd::Shutdown;
                        }
                    },
                },
                Err(TryRecvError::Disconnected) => {
                    self.close_open_turn(&mut transcript);
                    return TurnEnd::Closed;
                }
            };
            // a dropped receiver is the app going away: keep translating so
            // the engine's channel keeps draining, but never block on it
            transcript.apply(&event);
            if let EngineEvent::TurnFinished(outcome) = &event {
                let succeeded = matches!(outcome, TurnOutcome::Success { .. });
                let failure =
                    (!succeeded).then(|| turn_failure_activity(outcome, transcript.engine));
                // the terminal saves BEFORE it forwards: a consumer that saw
                // the TurnFinished message can read chat.json and a failed
                // turn's activity immediately (upstream's agentDidFinish
                // saves before the UI hears)
                let _ = store::save_chat(&self.root, &transcript.messages);
                if let Some(text) = failure {
                    self.pipeline().activity = Activity::Failed(text);
                    let _ = self.out.unbounded_send(Message::Event(event));
                } else {
                    let _ = self.out.unbounded_send(Message::Event(event));
                    self.turn_success_tail(&transcript);
                }
                return TurnEnd::Finished { succeeded };
            }
            let _ = self.out.unbounded_send(Message::Event(event));
        }
    }

    /// The channel-close turn end (plan Review Focus 4): running tools fail
    /// open, the partial transcript saves, the app layer hears
    /// [`Message::Closed`].
    fn close_open_turn(&mut self, transcript: &mut ChatState) {
        transcript.fail_running_tools();
        let _ = store::save_chat(&self.root, &transcript.messages);
        self.pipeline().activity =
            Activity::Failed(fl!("errore-di", agent = transcript.engine.display_name()));
        let _ = self.out.unbounded_send(Message::Closed);
    }

    /// Drain one build's events, forwarding each as
    /// [`Message::BuildEventBridge`] — the same absorb-between-translations
    /// law [`Self::drain_turn`] keeps for turns (a cancel lands between
    /// output lines, never after the terminal).
    async fn drain_build(
        &mut self,
        events: &mut mpsc::Receiver<BuildEvent>,
        cancel: impl Fn(),
    ) -> BuildEnd {
        loop {
            while let Ok(cmd) = self.cmds.try_recv() {
                self.absorb(cmd, &cancel);
            }
            let event = match events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => tokio::select! {
                    event = events.recv() => match event {
                        Some(event) => event,
                        None => return BuildEnd::Closed,
                    },
                    cmd = self.cmds.recv() => match cmd {
                        Some(cmd) => {
                            self.absorb(cmd, &cancel);
                            continue;
                        }
                        None => {
                            cancel();
                            return BuildEnd::Shutdown;
                        }
                    },
                },
                Err(TryRecvError::Disconnected) => return BuildEnd::Closed,
            };
            let _ = self
                .out
                .unbounded_send(Message::BuildEventBridge(event.clone()));
            if let BuildEvent::Finished(outcome) = event {
                return BuildEnd::Finished(outcome);
            }
        }
    }

    /// A build's terminal bookkeeping (upstream `buildDidFinish`,
    /// Project.swift:393-415): every disk write lands BEFORE the
    /// [`Message::BuildFinished`] announcement, so a consumer that saw it can
    /// assert disk state. An interrupted build leaves the outputs, the
    /// history, and the queue untouched — upstream's interrupt branch calls
    /// no `continueQueue`.
    fn finish_build(&mut self, end: BuildEnd, version_prompt: Option<String>) -> Phase {
        // the runner task died before classifying: not a success by any
        // measure, with no output text to show
        let outcome = match end {
            BuildEnd::Finished(outcome) => outcome,
            BuildEnd::Closed => BuildOutcome::Failed {
                last_line: None,
                output: String::new(),
            },
            BuildEnd::Shutdown => unreachable!("the run loop handles shutdown"),
        };
        let succeeded = matches!(outcome, BuildOutcome::Success);
        let interrupted = matches!(outcome, BuildOutcome::Interrupted);
        {
            let mut pipeline = self.pipeline();
            match &outcome {
                BuildOutcome::Interrupted => {
                    pipeline.activity = Activity::Failed(fl!("compilazione-interrotta"));
                }
                BuildOutcome::Success => {
                    pipeline.parameters = refresh_parameters(&self.root);
                    pipeline.missing_outputs = missing_files(&self.root.join("output"));
                    pipeline.activity = Activity::Succeeded(fl!("modello-generato"));
                }
                BuildOutcome::Failed { last_line, output } => {
                    pipeline.build_error = Some(
                        last_line
                            .clone()
                            .unwrap_or_else(|| fl!("errore-nello-script")),
                    );
                    pipeline.activity = Activity::Failed(fl!("errore-nello-script"));
                    let mut chat = store::load_chat(&self.root).unwrap_or_default();
                    chat.push(ChatMessage {
                        id: Uuid::new_v4(),
                        role: Role::System,
                        text: fl!("la-compilazione-di-model-py-non-riuscita"),
                        date: OffsetDateTime::now_utc(),
                        tool: None,
                        detail: Some(output.clone()),
                        action: Some(Action::AskClaudeToFix),
                        attachments: Vec::new(),
                    });
                    let _ = store::save_chat(&self.root, &chat);
                }
            }
        }
        if succeeded {
            // the build path records on model.py changes alone — no outputs
            // gate (upstream :397-401 against :269-271)
            let prompt = version_prompt
                .clone()
                .unwrap_or_else(|| fl!("compilazione-manuale-di-model-py"));
            self.record_version(&prompt);
        }
        let _ = self.out.unbounded_send(Message::BuildFinished {
            outcome,
            version_prompt,
        });
        if interrupted {
            return Phase::Idle;
        }
        self.continue_queue(succeeded)
    }

    /// The successful turn's tail (upstream `agentDidFinish`,
    /// Project.swift:262-278): refresh the observed outputs, record a
    /// version only when ALL expected outputs exist, then announce — the
    /// announcement lands after every write the tail made (upstream's
    /// unconditional metadata `save()` has no dirty field to persist here —
    /// `current_version` moves only inside [`Self::record_version`], which
    /// loads and saves the metadata fresh).
    fn turn_success_tail(&mut self, transcript: &ChatState) {
        let record_prompt;
        {
            let mut pipeline = self.pipeline();
            pipeline.parameters = refresh_parameters(&self.root);
            pipeline.missing_outputs = missing_files(&self.root.join("output"));
            if pipeline.missing_outputs.is_empty() {
                pipeline.activity = Activity::Succeeded(fl!("modello-generato"));
                record_prompt = Some(
                    transcript
                        .messages
                        .iter()
                        .rev()
                        .find(|row| row.role == Role::User)
                        .map(|row| row.text.clone())
                        .unwrap_or_default(),
                );
            } else {
                pipeline.activity = Activity::Succeeded(fl!("risposta-completata"));
                record_prompt = None;
            }
        }
        if let Some(prompt) = record_prompt {
            self.record_version(&prompt);
        }
        let _ = self.out.unbounded_send(Message::TurnSuccessBookkeeping);
    }

    /// Snapshot the project as the next version (upstream `recordVersion`,
    /// Project.swift:327-340): `has_changes` is the one guard, the summary
    /// row ("Version N", detail the number) lands before the snapshot, and
    /// a snapshot failure leaves the row in and surfaces as `build_error`.
    /// Metadata is loaded fresh at each record — the engine's session writes
    /// `session_id` into the same file mid-turn, and a copy held from launch
    /// would clobber it.
    fn record_version(&mut self, prompt: &str) {
        let mut metadata = self.load_metadata();
        if !versions::has_changes(
            &self.root,
            &self.pipeline().versions,
            metadata.current_version,
        ) {
            return;
        }
        let number = versions::next_number(&self.pipeline().versions);
        let mut chat = store::load_chat(&self.root).unwrap_or_default();
        chat.push(ChatMessage {
            id: Uuid::new_v4(),
            role: Role::Summary,
            text: fl!("versione-lld", version = number),
            date: OffsetDateTime::now_utc(),
            tool: None,
            detail: Some(number.to_string()),
            action: None,
            attachments: Vec::new(),
        });
        let _ = store::save_chat(&self.root, &chat);
        // the snapshot copies the whole output/ tree — run it on a local
        // copy of the history so the shared cell's guard is not held across
        // the file copies (the driver is this method's only caller, and the
        // engine never touches the in-memory list, so nothing races the
        // write-back)
        let mut versions = self.pipeline().versions.clone();
        let recorded = versions::record(
            &self.root,
            &mut versions,
            prompt,
            metadata.session_id.as_deref(),
            OffsetDateTime::now_utc(),
        );
        self.pipeline().versions = versions;
        match recorded {
            Ok(version) => {
                metadata.current_version = Some(version.number);
                let _ = store::save_metadata(&self.root, &metadata);
            }
            Err(err) => {
                self.pipeline().build_error = Some(err.to_string());
            }
        }
    }

    /// The project's metadata, loaded now ([`fresh_metadata`]'s law).
    fn load_metadata(&self) -> ProjectMetadata {
        fresh_metadata(&self.root)
    }
}

/// The failure activity a turn's terminal reports (upstream
/// AgentSession.swift:48/100/269/294 — the turn's engine name in each).
fn turn_failure_activity(outcome: &TurnOutcome, engine: AgentKind) -> String {
    match outcome {
        TurnOutcome::Failed { auth, .. } if *auth => {
            fl!("accesso-a-richiesto", agent = engine.display_name())
        }
        TurnOutcome::Failed { .. } | TurnOutcome::StartFailed { .. } => {
            fl!("errore-di", agent = engine.display_name())
        }
        TurnOutcome::AgentMissing => fl!("non-trovato", agent = engine.display_name()),
        TurnOutcome::Cancelled => crate::chat::interrupted(),
        // the engine's dance absorbs Busy/MissingSession before the app
        // hears them; Success never reaches a failure activity
        TurnOutcome::Busy | TurnOutcome::MissingSession | TurnOutcome::Success { .. } => {
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::iced::futures::StreamExt;
    use engine::agent::tool_summary::live_title;
    use engine::project::store::{ToolActivity, ToolState, load_chat};
    use std::path::Path;

    /// The fakes read their env at spawn time and parallel tests share one
    /// process: every setter/clearer of a CAD_FAKE_*/CAD_TEST_CLAUDE var
    /// holds this lock for the whole test (house pattern — app/tests/
    /// build_pipeline.rs, engine/tests/session_lifecycle.rs).
    static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

    fn fake_agent() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake-agent/fake_claude.py")
    }

    /// The crash-recovery shape a mid-turn app crash leaves behind
    /// (/tmp/crash-repro-jar on thelio, 2026-10-28): metadata holding the
    /// dead turn's session id and version pointer, a chat whose LAST row is
    /// the dead turn's user message, a recorded version, and stale turn
    /// logs — no busy flag anywhere, which is what the test asserts against.
    fn crashed_project() -> PathBuf {
        let root = std::env::temp_dir().join(format!("cad-crash-{}", Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("model.py"), "wall = 2\n").unwrap();
        std::fs::create_dir_all(root.join("output")).unwrap();
        let mut history = Vec::new();
        engine::project::versions::record(
            &root,
            &mut history,
            "a 20 mm cylinder jar",
            Some("97bfe962-15dd-46a0-93ca-e38aeaaf3515"),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let mut metadata = store::load_metadata(&root).unwrap();
        metadata.session_id = Some("97bfe962-15dd-46a0-93ca-e38aeaaf3515".into());
        metadata.current_version = Some(1);
        store::save_metadata(&root, &metadata).unwrap();
        // the dead in-flight turn: ...assistant, summary, then the user row
        // that never got an answer
        let rows = [
            ChatMessage {
                id: Uuid::new_v4(),
                role: Role::Assistant,
                text: "I made a 20 mm x 100 mm container.".into(),
                date: OffsetDateTime::now_utc(),
                tool: None,
                detail: None,
                action: None,
                attachments: Vec::new(),
            },
            ChatMessage {
                id: Uuid::new_v4(),
                role: Role::Summary,
                text: "Version 1".into(),
                date: OffsetDateTime::now_utc(),
                tool: None,
                detail: None,
                action: None,
                attachments: Vec::new(),
            },
            ChatMessage {
                id: Uuid::new_v4(),
                role: Role::User,
                text: "Animate how this mechanism moves, with a physics simulation.".into(),
                date: OffsetDateTime::now_utc(),
                tool: None,
                detail: None,
                action: None,
                attachments: Vec::new(),
            },
        ];
        store::save_chat(&root, &rows).unwrap();
        root
    }

    /// Drains bridge messages until `pred`, bounded (5 s) — never a fixed
    /// sleep (app/tests/build_pipeline.rs's helper, same shape).
    async fn run_until(
        messages: &mut UnboundedReceiver<Message>,
        mut pred: impl FnMut(&Message) -> bool,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, messages.next()).await {
                Ok(Some(message)) if pred(&message) => return,
                Ok(Some(_)) => continue,
                Ok(None) => panic!("message stream ended before predicate"),
                Err(_) => panic!("timed out waiting for predicate"),
            }
        }
    }

    /// A cold start over the crashed store must not believe a turn is in
    /// flight: the first send starts a turn (spawns the agent, appends its
    /// user row), resumes the stored session, and a second send after the
    /// first lands starts another — nothing queues or wedges.
    #[tokio::test(flavor = "multi_thread")]
    async fn cold_start_over_crashed_store_starts_turns() {
        let _guard = ENV_LOCK.lock().await;
        let root = crashed_project();
        let argv_out = root.join("argv-out");
        // SAFETY: agent-fake env holds this suite's ENV_LOCK.
        unsafe {
            std::env::set_var("CAD_TEST_CLAUDE", fake_agent());
            std::env::set_var(
                "CAD_FAKE_SCENARIO",
                fake_agent().parent().unwrap().join("scenarios/pong.jsonl"),
            );
            std::env::set_var("CAD_FAKE_ARGV_OUT", &argv_out);
            std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        }

        let (bridge, mut messages) = Bridge::launch(Settings::default(), root.clone());
        assert!(
            !bridge.activity().is_working(),
            "a cold start opens idle over the crashed store"
        );
        bridge.mark_python_ready_for_test();
        bridge
            .enqueue("Animate how this mechanism moves.".into(), Vec::new())
            .expect("the first send after a cold start is not refused");
        run_until(&mut messages, |m| matches!(m, Message::TurnStarted { .. })).await;
        run_until(&mut messages, |m| {
            matches!(
                m,
                Message::Event(EngineEvent::TurnFinished(TurnOutcome::Success { .. }))
            )
        })
        .await;

        let argv_lines = std::fs::read_to_string(&argv_out).unwrap();
        let spawns: Vec<&str> = argv_lines.lines().collect();
        assert_eq!(spawns.len(), 1, "exactly one agent process: {argv_lines}");
        assert!(
            spawns[0].contains("97bfe962-15dd-46a0-93ca-e38aeaaf3515")
                && spawns[0].contains("--resume")
                && spawns[0].contains("--fork-session"),
            "the cold start resumes the stored session: {}",
            spawns[0]
        );
        let chat = load_chat(&root).unwrap();
        // 3 crashed-store rows + the new user row + the fake's thinking and
        // assistant rows
        assert_eq!(chat.len(), 6, "the new send's user row appended");
        assert_eq!(chat[3].role, Role::User);
        assert_eq!(
            chat[3].text, "Animate how this mechanism moves.",
            "the send's user row is the one the UI handed over"
        );
        assert_eq!(chat.last().unwrap().role, Role::Assistant);

        // the second send must land too — the queue never wedged
        bridge
            .enqueue("Now make it printable.".into(), Vec::new())
            .expect("the second send after a cold start is not refused");
        run_until(&mut messages, |m| matches!(m, Message::TurnStarted { .. })).await;
        run_until(&mut messages, |m| {
            matches!(
                m,
                Message::Event(EngineEvent::TurnFinished(TurnOutcome::Success { .. }))
            )
        })
        .await;
        assert_eq!(
            std::fs::read_to_string(&argv_out).unwrap().lines().count(),
            2,
            "the second turn spawned its own agent process"
        );

        unsafe {
            std::env::remove_var("CAD_TEST_CLAUDE");
            std::env::remove_var("CAD_FAKE_SCENARIO");
            std::env::remove_var("CAD_FAKE_ARGV_OUT");
            std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
        }
    }

    /// A python-readiness refusal names itself in the status line — the
    /// one surface a refused send otherwise lacks (no row opens, no
    /// process spawns, the draft stays in the bar) — and still parks the
    /// draft and requests the setup sheet.
    #[tokio::test(flavor = "multi_thread")]
    async fn python_refusal_names_itself_in_the_activity() {
        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-refuse-{}", Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        // python_state opens Unknown: the app-level check has not landed
        let (bridge, mut messages) = Bridge::launch(Settings::default(), root.clone());
        assert_eq!(
            bridge.enqueue("make a bracket".into(), Vec::new()),
            Err(BuildRefusal::PythonNotReady)
        );
        assert_eq!(
            bridge.activity(),
            Activity::Failed(fl!("python-setup-required")),
            "the refusal leaves its reason in the status line"
        );
        assert!(bridge.sheet_requested_for_test());
        assert_eq!(
            bridge.draft_text_for_test().as_deref(),
            Some("make a bracket"),
            "the refused text stays parked for the input bar"
        );
        let mut sheet_requested = false;
        while let Ok(message) = messages.try_recv() {
            if matches!(message, Message::PythonSheetRequested) {
                sheet_requested = true;
            }
        }
        assert!(sheet_requested, "the setup sheet request still travels");
    }

    /// The CAD contract resource is present and is the upstream fixture
    /// byte-for-byte: an empty or drifted template means agent turns run
    /// without the contract the engine substitutes into
    /// `--append-system-prompt`.
    #[test]
    fn session_carries_system_template() {
        assert!(
            !SYSTEM_TEMPLATE.trim().is_empty(),
            "the template must not be empty"
        );
        let upstream = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/upstream/CADSystemPrompt.md"),
        )
        .unwrap();
        assert_eq!(
            SYSTEM_TEMPLATE.as_bytes(),
            upstream.as_slice(),
            "the embedded resource is the upstream fixture verbatim"
        );
    }

    /// The turn's engine source re-reads the settings file every ask
    /// (Ruling 1: a picker flip written mid-conversation decides the NEXT
    /// turn), and the launch snapshot answers only when the file is
    /// suspended (the scripted-agent test seam) or unreadable.
    #[test]
    fn engine_source_rereads_the_file_per_turn() {
        let dir = std::env::temp_dir().join(format!("cad-src-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");

        let mut codex = Settings::default();
        codex.agent_engine = "codex".into();
        codex.save(&file).unwrap();

        let source = EngineSource {
            file: Some(file.clone()),
            fallback: AgentKind::Claude,
        };
        assert_eq!(source.current(), AgentKind::Codex, "the file decides");

        Settings::default().save(&file).unwrap();
        assert_eq!(
            source.current(),
            AgentKind::Claude,
            "a flip on disk redirects the next turn"
        );

        let suspended = EngineSource {
            file: None,
            fallback: AgentKind::Codex,
        };
        assert_eq!(
            suspended.current(),
            AgentKind::Codex,
            "the snapshot answers when the re-read is suspended"
        );

        let unreadable = EngineSource {
            file: Some(dir.join("absent-settings.json")),
            fallback: AgentKind::Codex,
        };
        assert_eq!(
            unreadable.current(),
            AgentKind::Claude,
            "a missing settings file is a fresh install, not the snapshot"
        );
    }

    /// A codex turn announces its engine on TurnStarted and names itself in
    /// the working and failure activities — the Claude-only constants the
    /// T3 follow-up retired. The scripted-agent seam suspends the settings
    /// re-read, so the launch snapshot's engine is the turn's engine here.
    #[tokio::test(flavor = "multi_thread")]
    async fn codex_turn_announces_its_engine_and_names_its_activities() {
        // LOCK ORDER: pin_en BEFORE ENV_LOCK, uniformly with every other
        // dual-lock test — the previous ENV→pin order here inverted against
        // the pin→ENV tests and deadlocked the suite intermittently (ABBA:
        // this test parked on pin_en while a pin→ENV test parked on
        // ENV_LOCK; every pin_en waiter — including pure sync tests — hung
        // behind the never-released guard)
        let _lang = crate::localize::pin_en();
        let _guard = ENV_LOCK.lock().await;
        let root = std::env::temp_dir().join(format!("cad-codex-name-{}", Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        unsafe {
            std::env::set_var("CAD_TEST_CLAUDE", fake_agent());
            std::env::set_var(
                "CAD_FAKE_SCENARIO",
                fake_agent()
                    .parent()
                    .unwrap()
                    .join("scenarios/auth-error.jsonl"),
            );
            std::env::set_var(
                "CAD_FAKE_STDERR",
                fake_agent()
                    .parent()
                    .unwrap()
                    .join("scenarios/auth-error.stderr"),
            );
            std::env::set_var("CAD_FAKE_ARGV_OUT", root.join("argv-out"));
            std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        }

        let mut settings = Settings::default();
        settings.agent_engine = "codex".into();
        let (bridge, mut messages) = Bridge::launch(settings, root.clone());
        bridge.mark_python_ready_for_test();
        assert_eq!(
            bridge.activity(),
            Activity::Idle,
            "the activity starts idle, not pre-named"
        );
        bridge
            .enqueue("make a bracket".into(), Vec::new())
            .expect("the send is not refused");
        run_until(&mut messages, |m| {
            matches!(
                m,
                Message::TurnStarted {
                    engine: AgentKind::Codex,
                    ..
                }
            )
        })
        .await;
        assert_eq!(
            bridge.activity(),
            Activity::Working(fl!("sta-lavorando", agent = "Codex")),
            "the working activity names the turn's engine"
        );
        run_until(&mut messages, |m| {
            matches!(
                m,
                Message::Event(EngineEvent::TurnFinished(TurnOutcome::Failed { .. }))
            )
        })
        .await;
        assert_eq!(
            bridge.activity(),
            Activity::Failed(fl!("accesso-a-richiesto", agent = "Codex")),
            "the auth failure names the turn's engine"
        );

        unsafe {
            std::env::remove_var("CAD_TEST_CLAUDE");
            std::env::remove_var("CAD_FAKE_SCENARIO");
            std::env::remove_var("CAD_FAKE_STDERR");
            std::env::remove_var("CAD_FAKE_ARGV_OUT");
            std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
        }
    }

    fn tool_row(id: &str) -> ChatMessage {
        ChatMessage {
            id: Uuid::new_v4(),
            role: Role::Tool,
            text: crate::engine_text::tool_title(&live_title("Write", "")),
            date: OffsetDateTime::now_utc(),
            tool: Some(ToolActivity {
                tool_use_id: id.into(),
                name: "Write".into(),
                input: String::new(),
                output: None,
                preview: None,
                state: ToolState::Running,
            }),
            detail: None,
            action: None,
            attachments: Vec::new(),
        }
    }

    /// A driver over synthetic channels, its command sender, and its
    /// message stream.
    fn harness(root: &Path) -> (Drive, CmdSender<Command>, UnboundedReceiver<Message>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (msg_tx, msg_rx) = unbounded();
        (
            Drive::new(
                root.to_path_buf(),
                cmd_rx,
                msg_tx,
                Arc::new(Mutex::new(Pipeline::load(root))),
                EngineSource {
                    file: None,
                    fallback: AgentKind::Claude,
                },
            ),
            cmd_tx,
            msg_rx,
        )
    }

    /// The event channel closing without a TurnFinished (the engine task
    /// dying) ends the turn cleanly: the open tool row fails open and
    /// persists, the app layer hears `Closed`, and the queue treats it as a
    /// failure (plan Review Focus 4).
    #[tokio::test(flavor = "multi_thread")]
    async fn channel_close_without_turn_finished() {
        let root = std::env::temp_dir().join(format!("cad-bridge-close-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut drive, _cmd_tx, mut messages) = harness(&root);
        let (ev_tx, mut ev_rx) = mpsc::channel::<EngineEvent>(8);
        ev_tx
            .send(EngineEvent::ToolStarted {
                id: "t1".into(),
                name: "Write".into(),
                title: live_title("Write", ""),
            })
            .await
            .unwrap();
        drop(ev_tx);

        let transcript = ChatState::new(vec![tool_row("t1")]);
        let end = drive.drain_turn(&mut ev_rx, || {}, transcript).await;
        assert_eq!(end, TurnEnd::Closed);
        drive.queue.push_back(QueuedSend {
            id: Uuid::new_v4(),
            prompt: "next".into(),
            attachments: Vec::new(),
        });
        drive.continue_queue(false);
        assert!(
            drive.paused,
            "a channel close pauses the queue like any failure"
        );

        let saved = load_chat(&root).unwrap();
        let tool = saved[0].tool.as_ref().unwrap();
        assert_eq!(
            tool.state,
            ToolState::Failed,
            "the open tool row failed open"
        );
        assert_eq!(tool.preview, None);
        assert_eq!(
            messages.next().await,
            Some(Message::Event(EngineEvent::ToolStarted {
                id: "t1".into(),
                name: "Write".into(),
                title: live_title("Write", ""),
            })),
            "events already in flight still forward before the close"
        );
        assert_eq!(messages.next().await, Some(Message::Closed));
    }

    /// A cancel command is absorbed between event translations — no engine,
    /// no spawn: the interrupt switch fires while events keep flowing.
    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_command_lands_between_translations() {
        let root = std::env::temp_dir().join(format!("cad-bridge-cancel-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut drive, cmd_tx, mut messages) = harness(&root);
        let (ev_tx, mut ev_rx) = mpsc::channel::<EngineEvent>(1);
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&fired);
        let cancel = move || flag.store(true, std::sync::atomic::Ordering::SeqCst);

        // one event, then the channel goes quiet: the cancel must land in
        // the idle wait, not only between ready events
        ev_tx.send(EngineEvent::ThinkingStarted).await.unwrap();
        let transcript = ChatState::new(Vec::new());
        let (end, ()) = tokio::join!(drive.drain_turn(&mut ev_rx, cancel, transcript), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cmd_tx.send(Command::Cancel).unwrap();
            cmd_tx
                .send(Command::Enqueue {
                    prompt: "queued".into(),
                    attachments: Vec::new(),
                })
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = ev_tx
                .send(EngineEvent::TurnFinished(TurnOutcome::Cancelled))
                .await;
        });
        assert!(
            fired.load(std::sync::atomic::Ordering::SeqCst),
            "cancel fired mid-drain"
        );
        assert_eq!(end, TurnEnd::Finished { succeeded: false });
        assert_eq!(drive.queue.len(), 1, "the enqueue landed while draining");
        assert_eq!(
            messages.next().await,
            Some(Message::Event(EngineEvent::ThinkingStarted))
        );
    }

    /// `remove_queued` drops the addressed entry mid-drain and un-pauses an
    /// emptied queue (upstream `removeQueued`, Project.swift:278-283); the
    /// queue's observable contents travel with the change.
    #[tokio::test(flavor = "multi_thread")]
    async fn remove_queued_drops_entry_and_unpauses_when_empty() {
        let root = std::env::temp_dir().join(format!("cad-bridge-remove-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut drive, cmd_tx, mut messages) = harness(&root);
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        drive.queue.push_back(QueuedSend {
            id: first,
            prompt: "first".into(),
            attachments: vec!["shot.png".into()],
        });
        drive.queue.push_back(QueuedSend {
            id: second,
            prompt: "second".into(),
            attachments: Vec::new(),
        });
        drive.paused = true;
        drive.notify_queue();

        drive.absorb(Command::RemoveQueued(first), &|| {});
        assert_eq!(drive.queue.len(), 1, "the addressed entry is gone");
        assert_eq!(drive.queue.front().unwrap().id, second);
        assert!(
            drive.paused,
            "a queue that still holds items keeps its pause"
        );

        drive.absorb(Command::RemoveQueued(second), &|| {});
        assert!(drive.queue.is_empty());
        assert!(!drive.paused, "emptying the queue clears the pause");

        let mut seen = Vec::new();
        while let Ok(m) = messages.try_recv() {
            if let Message::QueueChanged { queue, paused } = m {
                seen.push((queue, paused));
            }
        }
        assert_eq!(
            seen.last().map(|(queue, paused)| (queue.len(), *paused)),
            Some((0, false)),
            "the final announcement carries the emptied, un-paused queue"
        );
        // the item view carries the image count the queue list renders
        let before_removal = seen.first().and_then(|(queue, _)| queue.first().cloned());
        assert_eq!(
            before_removal.map(|item| (item.text, item.images)),
            Some(("first".to_string(), 1)),
            "queued items expose preview text and the image count"
        );
        drop(cmd_tx);
    }

    /// The send's attachments are copied into `references/` at TURN START,
    /// before anything else happens (upstream `copyReferences` inside
    /// `send`, Project.swift:199-201): the persisted user row, the
    /// [`Message::TurnStarted`] announcement, and the child's prompt all
    /// carry the `references/<stamp>-<n>-<name>` COPY names — never the
    /// original request paths.
    #[tokio::test(flavor = "multi_thread")]
    async fn turn_rows_carry_reference_copy_names() {
        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-bridge-refs-{}", Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let source = std::env::temp_dir().join(format!("cad-src-{}.png", Uuid::new_v4()));
        std::fs::write(&source, b"png").unwrap();
        let leaf = source.file_name().unwrap().to_string_lossy().into_owned();
        let argv_out = root.join("argv-out");

        let guard = ENV_LOCK.lock().await;
        // SAFETY: agent-fake env holds this suite's ENV_LOCK.
        unsafe {
            std::env::set_var("CAD_TEST_CLAUDE", fake_agent());
            std::env::set_var(
                "CAD_FAKE_SCENARIO",
                fake_agent().parent().unwrap().join("scenarios/pong.jsonl"),
            );
            std::env::set_var("CAD_FAKE_ARGV_OUT", &argv_out);
            std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        }
        let (bridge, mut messages) = Bridge::launch(Settings::default(), root.clone());
        bridge.mark_python_ready_for_test();
        bridge
            .enqueue(
                "model this".into(),
                vec![source.to_string_lossy().into_owned()],
            )
            .expect("the send starts");

        // the announcement carries copy names, not request paths
        let mut announced = Vec::new();
        run_until(&mut messages, |m| {
            if let Message::TurnStarted { attachments, .. } = m {
                announced = attachments.clone();
                true
            } else {
                false
            }
        })
        .await;
        assert_eq!(announced.len(), 1, "{announced:?}");
        assert!(
            announced[0].starts_with("references/") && announced[0].ends_with(&format!("-1-{leaf}")),
            "the copy name is upstream's <stamp>-1-<name>: {}",
            announced[0]
        );
        // the copy is on disk BEFORE the row is even read by the app
        assert!(root.join(&announced[0]).is_file(), "{}", announced[0]);

        run_until(&mut messages, |m| {
            matches!(
                m,
                Message::Event(EngineEvent::TurnFinished(TurnOutcome::Success { .. }))
            )
        })
        .await;
        drop(guard);
        unsafe {
            std::env::remove_var("CAD_TEST_CLAUDE");
            std::env::remove_var("CAD_FAKE_SCENARIO");
            std::env::remove_var("CAD_FAKE_ARGV_OUT");
            std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
        }

        // the persisted row carries the same names
        let chat = load_chat(&root).unwrap();
        let row = chat
            .iter()
            .find(|row| row.role == Role::User)
            .expect("the send's user row");
        assert_eq!(row.attachments, announced, "row and announcement agree");
        // and the prompt the child received names the same copy
        let argv = std::fs::read_to_string(&argv_out).unwrap();
        let prompt = argv.lines().next().unwrap();
        assert!(prompt.contains(&announced[0]), "prompt: {prompt}");
        assert!(
            !prompt.contains(source.to_string_lossy().as_ref()),
            "the original path never reaches the prompt"
        );
    }

    /// A QUEUED send copies when it STARTS, not when it is queued (Ruling 4:
    /// upstream's queue holds pre-copy urls — Project.swift:177-190 — and
    /// `sendNextQueued` runs the same copy-on-send path as a direct send).
    #[tokio::test(flavor = "multi_thread")]
    async fn queued_send_copies_when_it_starts_not_when_queued() {
        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-bridge-qrefs-{}", Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let source = std::env::temp_dir().join(format!("cad-qsrc-{}.png", Uuid::new_v4()));
        std::fs::write(&source, b"png").unwrap();
        let leaf = source.file_name().unwrap().to_string_lossy().into_owned();

        let guard = ENV_LOCK.lock().await;
        // SAFETY: agent-fake env holds this suite's ENV_LOCK.
        unsafe {
            std::env::set_var("CAD_TEST_CLAUDE", fake_agent());
            std::env::set_var(
                "CAD_FAKE_SCENARIO",
                fake_agent().parent().unwrap().join("scenarios/pong.jsonl"),
            );
            std::env::set_var("CAD_FAKE_ARGV_OUT", root.join("argv-out"));
            std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        }
        let (mut drive, _cmd_tx, mut messages) = harness(&root);

        // absorbed mid-drain, the enqueue only queues: nothing is copied
        drive.absorb(
            Command::Enqueue {
                prompt: "model this".into(),
                attachments: vec![source.to_string_lossy().into_owned()],
            },
            &|| {},
        );
        assert!(
            !root.join("references").exists(),
            "queueing copies nothing upstream-side"
        );

        // the turn START makes the copy and the row together
        let mut session = Session::new(Settings::default()).with_system_template(SYSTEM_TEMPLATE);
        let phase = drive.send_next_queued(&mut session);
        assert!(matches!(phase, Phase::Running { .. }));
        let names: Vec<String> = std::fs::read_dir(root.join("references"))
            .expect("references/ exists once the queued turn starts")
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        assert!(names[0].ends_with(&format!("-1-{leaf}")), "{names:?}");

        // the announcement carries the copy name (start_turn announces
        // synchronously; the queue's own announcements precede it)
        let mut announced = Vec::new();
        run_until(&mut messages, |m| {
            if let Message::TurnStarted { attachments, .. } = m {
                announced = attachments.clone();
                true
            } else {
                false
            }
        })
        .await;
        assert_eq!(
            announced,
            vec![format!("references/{}", names[0])],
            "the queued turn's row carries the copy made at its start"
        );
        let chat = load_chat(&root).unwrap();
        assert_eq!(
            chat[0].role,
            Role::User,
            "the queued turn's user row persisted"
        );
        assert_eq!(chat[0].attachments, vec![format!("references/{}", names[0])]);
        drop(guard);
        unsafe {
            std::env::remove_var("CAD_TEST_CLAUDE");
            std::env::remove_var("CAD_FAKE_SCENARIO");
            std::env::remove_var("CAD_FAKE_ARGV_OUT");
            std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
        }
    }

    /// A successful turn's tail records a version whose snapshot carries the
    /// session id the ENGINE wrote mid-turn, and the record's metadata save
    /// keeps it: project.json is loaded fresh at each record, never held
    /// from launch. The engine's write lands AFTER the driver exists (a
    /// held-from-launch copy would have captured `session_id: None` and the
    /// tail's save would revert it — that is the regression pinned here).
    #[tokio::test(flavor = "multi_thread")]
    async fn turn_tail_record_keeps_engine_session_id() {
        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-bridge-rec-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("output")).unwrap();
        // every expected output on disk, so the tail records
        for name in engine::project::outputs::expected_outputs() {
            std::fs::write(root.join("output").join(name), b"x").unwrap();
        }
        let (mut drive, _cmd_tx, mut messages) = harness(&root);
        // the "engine's" mid-turn write: a session id in project.json,
        // BETWEEN the driver's construction and the tail's save
        let mut metadata = ProjectMetadata::new("test");
        metadata.session_id = Some("engine-session".into());
        store::save_metadata(&root, &metadata).unwrap();

        let (ev_tx, mut ev_rx) = mpsc::channel::<EngineEvent>(4);
        ev_tx
            .send(EngineEvent::TurnFinished(TurnOutcome::Success {
                session_id: Some("engine-session".into()),
                text: String::new(),
            }))
            .await
            .unwrap();
        drop(ev_tx);
        let transcript = ChatState::new(vec![ChatMessage {
            id: Uuid::new_v4(),
            role: Role::User,
            text: "a 20 mm cube".into(),
            date: OffsetDateTime::now_utc(),
            tool: None,
            detail: None,
            action: None,
            attachments: Vec::new(),
        }]);
        let end = drive.drain_turn(&mut ev_rx, || {}, transcript).await;
        assert_eq!(end, TurnEnd::Finished { succeeded: true });

        let saved = store::load_metadata(&root).unwrap();
        assert_eq!(saved.session_id.as_deref(), Some("engine-session"));
        assert_eq!(saved.current_version, Some(1));
        let version = &drive.pipeline().versions[0];
        assert_eq!(
            version.session_id.as_deref(),
            Some("engine-session"),
            "the snapshot carries the engine's session id"
        );
        assert_eq!(version.prompt, "a 20 mm cube");
        let chat = load_chat(&root).unwrap();
        assert_eq!(chat.last().unwrap().role, Role::Summary);
        assert_eq!(chat.last().unwrap().text, "Version 1");
        assert_eq!(
            messages.next().await,
            Some(Message::Event(EngineEvent::TurnFinished(
                TurnOutcome::Success {
                    session_id: Some("engine-session".into()),
                    text: String::new(),
                }
            )))
        );
        assert_eq!(messages.next().await, Some(Message::TurnSuccessBookkeeping));
    }
}
