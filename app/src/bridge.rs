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
use engine::agent::session::{EngineEvent, Session, TEST_CLAUDE_ENV, TurnHandle, TurnOutcome};
use engine::project::folder;
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

use crate::activity::{self, Activity, Signal};
use crate::chat::ChatState;
use crate::fl;
use crate::settings_store::SettingsStore;

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
            missing_outputs: missing_files(&folder::output_dir(root)),
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
        self.activity = activity::next(Signal::TurnStarted, false, engine);
    }
}

/// The manifest's parameters in model.py order — the panel list the bridge
/// refreshes after every build and turn end (upstream `output.reload()` +
/// `orderedParameters`). Pure: a missing manifest or model.py reads as no
/// parameters.
pub fn refresh_parameters(root: &Path) -> Vec<ModelParameter> {
    match Manifest::load(&folder::output_dir(root).join(folder::MANIFEST)) {
        Ok(manifest) => {
            let script = std::fs::read_to_string(folder::model_script(root)).unwrap_or_default();
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
    /// engine the turn runs, as the engine decided it
    /// (`TurnHandle::engine`), so the window names the turn's rows with it.
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

impl QueuedSend {
    fn new(prompt: String, attachments: Vec<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            prompt,
            attachments,
        }
    }
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
            // upstream counts every attachment (ChatInputView.swift:218-219)
            images: send.attachments.len(),
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
    /// The driver's session configuration, consulted only for the working
    /// label a send shows before the driver starts it (the driver's
    /// [`Message::TurnStarted`] then names the engine the turn runs).
    session: Session,
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
        Bridge::launch_with(self.interpreter, settings, settings_path(), root)
    }
}

impl Bridge {
    /// The bridge plus the message stream to return from the app's `init` or
    /// `update`, its turns reading their settings fresh from `settings`.
    /// Must be called inside the tokio runtime cosmic runs on.
    pub fn new(
        settings: &SettingsStore,
        root: impl Into<PathBuf>,
    ) -> (Self, cosmic::Task<Message>) {
        let (bridge, messages) = Self::launch_with(
            default_interpreter(),
            settings.load(),
            settings.path().to_path_buf(),
            root,
        );
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
        Self::launch_with(default_interpreter(), settings, settings_path(), root)
    }

    /// `settings_file` is the file each turn re-reads its settings from;
    /// a scripted agent under test (the engine's override seam) ignores it,
    /// so the developer's own settings cannot decide a test turn.
    fn launch_with(
        interpreter: PathBuf,
        settings: Settings,
        settings_file: PathBuf,
        root: impl Into<PathBuf>,
    ) -> (Self, UnboundedReceiver<Message>) {
        let root = root.into();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (msg_tx, msg_rx) = unbounded();
        let pipeline = Arc::new(Mutex::new(Pipeline::load(&root)));
        let mut session = Session::new(settings).with_system_template(SYSTEM_TEMPLATE);
        // the per-turn settings re-read (the engine picker flips the runner
        // mid-conversation, Ruling 1)
        if std::env::var_os(TEST_CLAUDE_ENV).is_none() {
            session = session.with_settings_file(settings_file);
        }
        tokio::spawn(
            Drive::new(root.clone(), cmd_rx, msg_tx.clone(), Arc::clone(&pipeline))
                .run(session.clone()),
        );
        (
            Self {
                commands: cmd_tx,
                out: msg_tx,
                pipeline,
                root,
                interpreter,
                session,
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
                // turn-start path reaches and which names the engine the
                // turn actually runs)
                pipeline.begin_turn(self.session.turn_settings().agent_kind());
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
        if std::fs::read_to_string(folder::model_script(&self.root)).is_err() {
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
        if !folder::model_script(&self.root).exists() {
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
                let mut metadata = store::load_metadata_or_new(&self.root);
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

    /// The manifest changed on disk, whoever wrote it
    /// (ProjectOutput.swift:48-51): re-read the parameter rows.
    pub fn reload_parameters(&self) {
        self.pipeline().parameters = refresh_parameters(&self.root);
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

/// What a command asks of the phase loop ([`Drive::command`]).
enum Next {
    /// Keep the current phase.
    Stay,
    /// Start this send as a turn now.
    Start(QueuedSend),
    /// Start the queue's next send now, if the queue is not paused.
    SendNext,
    /// Drain this build now.
    Build {
        handle: BuildHandle,
        version_prompt: Option<String>,
    },
}

/// The driver's end: the bridge handle is gone, so any running turn or build
/// was already cancelled; the engine's process registry is reaped (upstream's
/// quit path reaps orphans).
fn shutdown() {
    engine::processes::reap_orphans();
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
    /// Every message the driver sent, with the activity the pipeline held
    /// at that instant (`None` when the pipeline was locked mid-send):
    /// ordering tests read state at send time, not when a consumer wakes.
    #[cfg(test)]
    sent: Mutex<Vec<(Message, Option<Activity>)>>,
}

impl Drive {
    fn new(
        root: PathBuf,
        cmds: CmdReceiver<Command>,
        out: UnboundedSender<Message>,
        pipeline: Arc<Mutex<Pipeline>>,
    ) -> Self {
        Self {
            root,
            queue: VecDeque::new(),
            paused: false,
            cmds,
            out,
            pipeline,
            #[cfg(test)]
            sent: Mutex::new(Vec::new()),
        }
    }

    fn pipeline(&self) -> MutexGuard<'_, Pipeline> {
        self.pipeline.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Forward one message to the app layer. A dropped receiver is the app
    /// going away: the send never blocks and its failure is ignored.
    fn emit(&self, message: Message) {
        #[cfg(test)]
        self.sent.lock().unwrap_or_else(|e| e.into_inner()).push((
            message.clone(),
            self.pipeline.try_lock().ok().map(|p| p.activity.clone()),
        ));
        let _ = self.out.unbounded_send(message);
    }

    async fn run(mut self, mut session: Session) {
        let mut phase = Phase::Idle;
        loop {
            phase = match phase {
                Phase::Idle => match self.cmds.recv().await {
                    Some(cmd) => {
                        let next = self.command(cmd, None);
                        self.settle(next, &mut session, Phase::Idle)
                    }
                    None => return shutdown(),
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
                        TurnEnd::Shutdown => return shutdown(),
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
                        BuildEnd::Shutdown => return shutdown(),
                        _ => self.finish_build(end, version_prompt),
                    }
                }
                // a direct send or a build arriving in the gap eats the
                // pending delay: its own end re-arms the queue's next step
                Phase::Delay { auto_at } => tokio::select! {
                    _ = tokio::time::sleep_until(auto_at) => self.send_next_queued(&mut session),
                    cmd = self.cmds.recv() => match cmd {
                        Some(cmd) => {
                            let next = self.command(cmd, None);
                            self.settle(next, &mut session, Phase::Delay { auto_at })
                        }
                        None => return shutdown(),
                    },
                },
            };
        }
    }

    /// One command, applied by the same rule in every phase. `running` is
    /// the interrupt switch of the turn or build being drained, `None` while
    /// nothing runs. It decides the three phase-dependent answers: a send
    /// starts now or joins the queue FIFO, a build starts now or is refused
    /// late (the handle's busy guard raced the running start), and a resume
    /// starts the next queued send now or at the running one's end
    /// (upstream `sendNextQueued` guards `isBusy`). The queue edits
    /// themselves are phase-blind: upstream `removeQueued` drops the entry
    /// and an emptied queue loses its pause (Project.swift:278-283).
    fn command(&mut self, cmd: Command, running: Option<&dyn Fn()>) -> Next {
        match cmd {
            Command::Enqueue {
                prompt,
                attachments,
            } => {
                let send = QueuedSend::new(prompt, attachments);
                if running.is_none() {
                    return Next::Start(send);
                }
                self.queue.push_back(send);
                self.notify_queue();
                Next::Stay
            }
            // with nothing running, upstream's stop is a no-op
            Command::Cancel => {
                if let Some(cancel) = running {
                    cancel();
                }
                Next::Stay
            }
            Command::StartBuild {
                handle,
                version_prompt,
            } => {
                if running.is_some() {
                    handle.cancel();
                    return Next::Stay;
                }
                Next::Build {
                    handle,
                    version_prompt,
                }
            }
            Command::ResumeQueue => {
                self.paused = false;
                self.notify_queue();
                if running.is_some() {
                    Next::Stay
                } else {
                    Next::SendNext
                }
            }
            Command::RemoveQueued(id) => {
                self.queue.retain(|send| send.id != id);
                if self.queue.is_empty() {
                    self.paused = false;
                }
                self.notify_queue();
                Next::Stay
            }
        }
    }

    /// The phase an idle-side command leads to; `stay` is the phase it was
    /// applied in.
    fn settle(&mut self, next: Next, session: &mut Session, stay: Phase) -> Phase {
        match next {
            Next::Stay => stay,
            Next::Start(send) => self.start_turn(session, send),
            Next::SendNext => self.send_next_queued(session),
            Next::Build {
                handle,
                version_prompt,
            } => Phase::Building {
                handle,
                version_prompt,
            },
        }
    }

    /// One command absorbed inside a drain loop: `cancel` is the running
    /// turn's or build's interrupt switch, split from the event receiver the
    /// loop owns. Nothing starts while something runs, so every command
    /// leaves the phase as it is.
    fn absorb(&mut self, cmd: Command, cancel: &impl Fn()) {
        let next = self.command(cmd, Some(cancel));
        debug_assert!(matches!(next, Next::Stay), "a running phase starts nothing");
    }

    /// Start a turn (upstream `send`, Project.swift:192-208): copy the
    /// attachments into `references/` FIRST — before the user row is
    /// appended, exactly upstream's order — then hand the request to the
    /// engine's lifecycle dance, append the row carrying the COPY names,
    /// save, and announce. Every turn-start path lands here (a direct send,
    /// the 1 s auto-run, a resume), so a QUEUED send's copies are made here,
    /// at its own start, never at enqueue (Project.swift:177-190). The bridge
    /// is the single writer of `references/`: the engine's session consumes
    /// the names and writes nothing. The turn's ENGINE is the one the engine
    /// decided for it ([`TurnHandle::engine`]); it rides the transcript and
    /// the announcement, so the turn's rows and activity name the engine
    /// that actually runs them, whatever the settings say by the time they
    /// land. The transcript reloads from disk — the last turn's save is this
    /// turn's base, exactly the crash-recovery shape `load_chat`'s
    /// `unwrap_or_default` gives a fresh project.
    fn start_turn(&mut self, session: &mut Session, send: QueuedSend) -> Phase {
        let sources: Vec<PathBuf> = send.attachments.iter().map(PathBuf::from).collect();
        let references = attachments::copy_attachments(&sources, &self.root);
        let images: Vec<PathBuf> = references.iter().map(|name| self.root.join(name)).collect();
        let request = TurnRequest {
            prompt: send.prompt.clone(),
            references: references.clone(),
            images,
        };
        let handle = session.send(&self.root, &request);
        let engine = handle.engine();
        self.pipeline().begin_turn(engine);
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
        self.emit(Message::TurnStarted {
            prompt: send.prompt,
            attachments: references,
            engine,
        });
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
        self.emit(Message::QueueChanged {
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
            if let EngineEvent::ToolStarted { title, .. } = &event {
                let title = crate::engine_text::tool_title(title);
                self.pipeline().activity =
                    activity::next(Signal::ToolOpened(&title), false, transcript.engine);
            }
            if let EngineEvent::TurnFinished(outcome) = &event {
                let succeeded = matches!(outcome, TurnOutcome::Success { .. });
                // the terminal saves BEFORE it forwards: a consumer that saw
                // the TurnFinished message can read chat.json and the turn's
                // activity, outputs and parameters immediately (upstream's
                // agentDidFinish saves before the UI hears)
                let _ = store::save_chat(&self.root, &transcript.messages);
                let record_prompt = if succeeded {
                    self.settle_turn_outputs(&transcript, outcome)
                } else {
                    self.pipeline().activity =
                        activity::next(Signal::TurnFinished(outcome), false, transcript.engine);
                    None
                };
                self.emit(Message::Event(event));
                if succeeded {
                    self.turn_success_tail(record_prompt);
                }
                return TurnEnd::Finished { succeeded };
            }
            self.emit(Message::Event(event));
        }
    }

    /// The channel-close turn end (plan Review Focus 4): running tools fail
    /// open, the partial transcript saves, the app layer hears
    /// [`Message::Closed`].
    fn close_open_turn(&mut self, transcript: &mut ChatState) {
        transcript.fail_running_tools();
        let _ = store::save_chat(&self.root, &transcript.messages);
        self.pipeline().activity = activity::next(Signal::Closed, false, transcript.engine);
        self.emit(Message::Closed);
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
            self.emit(Message::BuildEventBridge(event.clone()));
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
                    pipeline.missing_outputs = missing_files(&folder::output_dir(&self.root));
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
        self.emit(Message::BuildFinished {
            outcome,
            version_prompt,
        });
        if interrupted {
            return Phase::Idle;
        }
        self.continue_queue(succeeded)
    }

    /// A successful turn's observed state (upstream `agentDidFinish`,
    /// Project.swift:262-276): parameters and missing outputs re-read and
    /// the finished activity set. Returns the prompt to record a version
    /// under, present only when ALL expected outputs exist.
    fn settle_turn_outputs(
        &mut self,
        transcript: &ChatState,
        outcome: &TurnOutcome,
    ) -> Option<String> {
        let mut pipeline = self.pipeline();
        pipeline.parameters = refresh_parameters(&self.root);
        pipeline.missing_outputs = missing_files(&folder::output_dir(&self.root));
        let complete = pipeline.missing_outputs.is_empty();
        pipeline.activity =
            activity::next(Signal::TurnFinished(outcome), complete, transcript.engine);
        if complete {
            Some(
                transcript
                    .messages
                    .iter()
                    .rev()
                    .find(|row| row.role == Role::User)
                    .map(|row| row.text.clone())
                    .unwrap_or_default(),
            )
        } else {
            None
        }
    }

    /// The successful turn's tail, after TurnFinished is forwarded: record
    /// the version, then announce — the announcement lands after every
    /// write the tail made (upstream's unconditional metadata `save()` has
    /// no dirty field to persist here — `current_version` moves only
    /// inside [`Self::record_version`], which loads and saves the metadata
    /// fresh).
    fn turn_success_tail(&mut self, record_prompt: Option<String>) {
        if let Some(prompt) = record_prompt {
            self.record_version(&prompt);
        }
        self.emit(Message::TurnSuccessBookkeeping);
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

    /// The project's metadata as it is on disk now
    /// ([`store::load_metadata_or_new`]).
    fn load_metadata(&self) -> ProjectMetadata {
        store::load_metadata_or_new(&self.root)
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

    /// The activity the pipeline held when the driver sent its
    /// TurnFinished message.
    fn activity_at_turn_finished(drive: &Drive) -> Activity {
        drive
            .sent
            .lock()
            .unwrap()
            .iter()
            .find(|(m, _)| matches!(m, Message::Event(EngineEvent::TurnFinished(_))))
            .expect("TurnFinished was sent")
            .1
            .clone()
            .expect("the pipeline was free at the send")
    }

    /// Drain one turn whose only event is `outcome`'s TurnFinished.
    async fn drain_terminal(root: &Path, outcome: TurnOutcome) -> (Drive, TurnEnd) {
        let (mut drive, _cmd_tx, _messages) = harness(root);
        drive.pipeline().activity = Activity::Working(fl!(
            "sta-lavorando",
            agent = AgentKind::Claude.display_name()
        ));
        let (ev_tx, mut ev_rx) = mpsc::channel::<EngineEvent>(1);
        ev_tx
            .send(EngineEvent::TurnFinished(outcome))
            .await
            .unwrap();
        drop(ev_tx);
        let end = drive
            .drain_turn(&mut ev_rx, || {}, ChatState::new(Vec::new()))
            .await;
        (drive, end)
    }

    /// A successful turn's activity is settled at the instant TurnFinished
    /// is sent: a consumer reading the bridge the moment it hears the
    /// terminal sees the finished state, never the turn's Working line,
    /// and the later TurnSuccessBookkeeping does not change it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_successful_turn_settles_its_activity_before_turn_finished() {
        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-settle-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (drive, end) = drain_terminal(
            &root,
            TurnOutcome::Success {
                session_id: None,
                text: String::new(),
            },
        )
        .await;
        assert_eq!(end, TurnEnd::Finished { succeeded: true });
        let at_send = activity_at_turn_finished(&drive);
        assert_eq!(
            at_send,
            Activity::Succeeded(fl!("risposta-completata")),
            "the terminal's activity is the finished one"
        );
        assert_eq!(
            at_send,
            drive.pipeline().activity,
            "the tail changes nothing more"
        );
    }

    /// A failed turn's activity is set at the instant TurnFinished is sent.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_turn_sets_its_activity_before_turn_finished() {
        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-settle-fail-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (drive, end) = drain_terminal(
            &root,
            TurnOutcome::Failed {
                error: String::new(),
                auth: true,
                exit_code: Some(1),
            },
        )
        .await;
        assert_eq!(end, TurnEnd::Finished { succeeded: false });
        assert_eq!(
            activity_at_turn_finished(&drive),
            Activity::Failed(fl!(
                "accesso-a-richiesto",
                agent = AgentKind::Claude.display_name()
            )),
        );
    }

    /// A codex turn announces its engine on TurnStarted and names itself in
    /// the working and failure activities, never a fixed "Claude". The
    /// scripted-agent seam suspends the settings re-read, so the launch
    /// snapshot's engine is the turn's engine here.
    #[tokio::test(flavor = "multi_thread")]
    async fn codex_turn_announces_its_engine_and_names_its_activities() {
        // LOCK ORDER: pin_en BEFORE ENV_LOCK, uniformly with every other
        // dual-lock test: one test taking them in the other order deadlocks
        // the suite (ABBA), and every pin_en waiter hangs behind it
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
            attachments: vec!["shot.png".into(), "notes.txt".into()],
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
            Some(("first".to_string(), 2)),
            "queued items expose preview text and the attachment count, non-images included"
        );
        drop(cmd_tx);
    }

    /// A failed turn leaves the driver idle over a paused, non-empty queue.
    /// Removing one queued send there drops exactly that send and keeps the
    /// pause, as it does while a turn runs: the idle driver applies queue
    /// commands by the same rule as every other phase.
    #[tokio::test(flavor = "multi_thread")]
    async fn remove_queued_after_a_failed_turn_removes_and_keeps_the_pause() {
        let root = std::env::temp_dir().join(format!("cad-bridge-idle-rm-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut drive, cmd_tx, mut messages) = harness(&root);
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        for (id, prompt) in [(first, "first"), (second, "second")] {
            drive.queue.push_back(QueuedSend {
                id,
                prompt: prompt.into(),
                attachments: Vec::new(),
            });
        }
        assert!(
            matches!(drive.continue_queue(false), Phase::Idle),
            "a failed turn ends idle"
        );
        assert!(drive.paused, "a failed turn pauses the queue");

        let driver = tokio::spawn(drive.run(Session::new(Settings::default())));
        cmd_tx.send(Command::RemoveQueued(first)).unwrap();
        let mut after = None;
        run_until(&mut messages, |m| match m {
            Message::QueueChanged { queue, paused } if queue.len() != 2 || !paused => {
                after = Some((
                    queue.iter().map(|item| item.id).collect::<Vec<_>>(),
                    *paused,
                ));
                true
            }
            _ => false,
        })
        .await;
        driver.abort();
        assert_eq!(
            after,
            Some((vec![second], true)),
            "the addressed send is gone and the rest stays paused"
        );
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
            announced[0].starts_with("references/")
                && announced[0].ends_with(&format!("-1-{leaf}")),
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
        assert_eq!(
            chat[0].attachments,
            vec![format!("references/{}", names[0])]
        );
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
