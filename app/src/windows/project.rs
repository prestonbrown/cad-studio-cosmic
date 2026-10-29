//! The project window (upstream ProjectWindow.swift + ProjectWorkspace +
//! ProjectToolbar): the header hosting the ActivityViewer port, the
//! resizable chat/3D/schematic pane grid, and the M4 commands subset.
//!
//! The window owns no agent logic — the [`crate::bridge`] drives turns and
//! the pure modules own behavior — it folds messages between them: pane
//! messages fan into pane state, bridge messages fold into the chat
//! reducer and the activity model, and the send flow takes the draft
//! BEFORE anything is forwarded.
//!
//! Commands (the M4 subset plus Compila ⌘B, M7's parameters ⌥⌘I, and M8's
//! export ⇧⌘E): show-chat ⌘0, focus ⌘1/2/3, parameters ⌥⌘I, layout
//! presets, viewer presets + fit, schematic zoom/fit, build, stop, export,
//! reveal, open-in-slicer. The keyboard bindings are one pure
//! table, [`project_shortcut`]; the header's menu renders the same
//! commands.
//!
//! User-visible strings read through `fl!` at render time (the key names
//! the en/main.ftl entry); the menu roots the port coined (File/View) use
//! the port-added `menu-file`/`menu-view` keys.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ::time::OffsetDateTime;
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::stream::BoxStream;
use cosmic::iced::keyboard::{Key, Modifiers};
use cosmic::iced::stream;
use cosmic::iced::time::every;
use cosmic::iced::{Color, Length, Size, Subscription, Task};
use cosmic::widget::menu::action::MenuAction;
use cosmic::widget::pane_grid::{self, DragEvent, ResizeEvent};
use cosmic::widget::text_editor;
use cosmic::widget::{Column, Row, Space, button, container, popover, text};
use cosmic::{Element, Task as CosmicTask};
use engine::agent::session::EngineEvent;
use engine::project::layout::{LayoutNode, PaneKind};
use engine::project::manifest::Manifest;
use engine::project::store::Action;
use engine::project::store::{self, ProjectMetadata};
use engine::settings::Settings;
use engine::watch::{ReloadKind, WatchHandle, watch_directory};
use viewer::render::camera::Preset;

use crate::activity::{self, Activity, IssueKind};
use crate::attachments::ClipboardContent;
use crate::bridge::{self, Bridge};
use crate::chat::ChatState;
use crate::chat_view::{self, ChatPane, QueueItem};
use crate::fl;
use crate::panes;
use crate::parameters_view;
use crate::schematic::SchematicPane;
use crate::version_menu;
use crate::viewer_pane::ViewerPane;

/// Upstream's default project window size (the WindowGroup
/// `.defaultSize(1320, 840)`) and minimum (ProjectWorkspace's
/// minWidth/minHeight).
pub const DEFAULT_SIZE: (f32, f32) = (1320.0, 840.0);
pub const MIN_SIZE: (f32, f32) = (960.0, 600.0);

/// The pane grid's divider thickness — the spacing half of
/// [`panes::clamp_resize`]'s contract.
const GRID_SPACING: f32 = 8.0;

/// The output watcher's debounce (upstream FileWatcher's 0.3 s — shared
/// with the viewer pane's own watch).
const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// The commands the M4 project window exposes (upstream AppCommands +
/// ProjectToolbar, M4 subset + M5's Compila + M7's parameters toggle +
/// M8's export).
/// Doubles as the menu action type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    ShowChat,
    FocusChat,
    FocusModel,
    FocusSchematic,
    /// "Mostra/Nascondi parametri" ⌥⌘I (Commands.swift:69-73): flip the
    /// parameters overlay's visibility — the app-wide binding (upstream
    /// hangs it off the project, disabled with none open).
    ToggleParameters,
    LayoutChatLeading,
    LayoutChatTrailing,
    LayoutChatBottom,
    LayoutThreeColumns,
    ViewerPreset(Preset),
    ViewerFit,
    SchematicZoomIn,
    SchematicZoomOut,
    SchematicFit,
    /// "Compila modello" ⌘B (Commands.swift:115-119): build model.py now.
    Build,
    Stop,
    /// "Esporta pacchetto…" ⇧⌘E (Commands.swift:130-132): zip model.py +
    /// the non-dot outputs to a chosen destination.
    Export,
    Reveal,
    OpenInSlicer,
}

impl MenuAction for Command {
    type Message = Message;

    fn message(&self) -> Message {
        Message::Command(*self)
    }
}

impl MenuAction for AppRequest {
    type Message = Message;

    fn message(&self) -> Message {
        Message::App(*self)
    }
}

/// The project window's messages.
#[derive(Debug, Clone)]
pub enum Message {
    Chat(chat_view::Message),
    Viewer(crate::viewer_pane::Message),
    Schematic(crate::schematic::Message),
    Bridge(bridge::Message),
    /// A bridge refusal asked for the Python setup sheet — the app layer
    /// owns the sheet's presentation (it overlays whichever window shows).
    PresentSetupSheet,
    /// The shell's output watcher saw a settled change.
    OutputReload(ReloadKind),
    /// The pane grid's laid-out size (the resize clamp's input).
    GridResized(Size),
    /// The 1 s tick while working (elapsed label, focus flash).
    Tick,
    Command(Command),
    /// A pane-grid divider drag (clamped through [`panes::clamp_resize`]).
    Resized(ResizeEvent),
    /// A pane drag-and-drop rearrangement.
    Dragged(DragEvent),
    /// The attach portal answered.
    AttachChosen(Result<Vec<PathBuf>, String>),
    /// The export save portal answered (`Err` = cancelled — upstream's
    /// `guard let destination … else { return }`); the drive harness's
    /// `saveas` injects the `Ok` arm directly, the portal needing a user.
    ExportChosen(Result<PathBuf, String>),
    /// The zip finished: `Some(destination)` on status 0 (revealed), `None`
    /// on anything else — zip missing included (the failed activity row).
    ExportDone(Option<PathBuf>),
    /// An issue button opened/closed its list (`None` closes).
    ToggleIssues(Option<IssueKind>),
    /// The header's close button (the app layer closes the window).
    CloseRequested,
    /// An app-level request bubbled from the File menu (the window cannot
    /// act on windows or portals itself).
    App(AppRequest),
    /// A version menu entry was picked (VersionMenu.swift:14): restore that
    /// snapshot.
    Restore(i64),
    /// The toolbar's clock button — open/close the version history card
    /// (a popover, NOT a menu root: cosmic's menu overlay corrupts widget
    /// state when the item set changes; version_menu's module doc has the
    /// panic chain).
    ToggleVersionCard,
}

/// App-level commands the project window's File menu bubbles up
/// (upstream's global CommandGroups; the recents list itself lives in the
/// welcome window, so "Open Recent" focuses it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRequest {
    NewProject,
    OpenProject,
    OpenRecent,
    Welcome,
}

/// One open project's window state.
pub struct ProjectWindow {
    pub root: PathBuf,
    pub name: String,
    /// The settings path every settings write goes through (the
    /// single-writer law; stored so layout persists can reach it).
    settings_path: PathBuf,
    pub metadata: ProjectMetadata,
    /// The full layout tree (upstream `layout`); the grid shows
    /// [`panes::displayed`].
    pub layout: LayoutNode,
    pub chat_visible: bool,
    pub grid: pane_grid::State<PaneKind>,
    /// The pane grid's laid-out pixel size, for resize clamping (reported
    /// from layout through the shell relay — the window size would ignore
    /// the header).
    pub size: (f32, f32),
    pub chat: ChatState,
    pub chat_pane: ChatPane,
    pub viewer: ViewerPane,
    pub schematic: SchematicPane,
    pub bridge: Bridge,
    /// The queue as the input bar renders it (bridge announcements).
    pub queue: Vec<QueueItem>,
    pub queue_paused: bool,
    pub activity: Activity,
    pub work_started_at: Option<Instant>,
    pub missing_outputs: Vec<String>,
    /// The issue button's open list, if any.
    pub shown_issues: Option<IssueKind>,
    /// The version history card's open state, carrying the FROZEN clock its
    /// relative labels read (captured at open — a live per-frame time-ago
    /// would rebuild every row's text every second; version_menu's
    /// view_versions doc).
    version_card: Option<time::OffsetDateTime>,
    /// The focus command's one-second pane highlight.
    focus_flash: Option<PaneKind>,
    /// The last reported grid size (the report dedup).
    last_size: Arc<Mutex<Size>>,
    /// Keeps the shell's output watch alive; dropped with the window.
    #[allow(dead_code)] // read by no one: the field's DROP stops the watch
    watch: Option<WatchHandle>,
    /// The relay's sending half, kept so the stream ends with the window.
    #[allow(dead_code)]
    events: std::sync::mpsc::Sender<ShellEvent>,
    relay: Arc<OutputRelay>,
}

/// What crosses the shell's event channel into the subscription: output
/// reloads and the pane grid's size report (the viewer pane's `PaneEvent`
/// shape).
#[derive(Debug, Clone, Copy)]
enum ShellEvent {
    Reload(ReloadKind),
    /// The grid's laid-out size — the extent the resize clamp must see
    /// (the WINDOW size would ignore the header and overestimate it,
    /// loosening the per-split minimums).
    GridResized(Size),
}

/// The output-watch relay (the take-once pattern the viewer and schematic
/// panes established).
struct OutputRelay {
    id: u64,
    events: Mutex<Option<Receiver<ShellEvent>>>,
}

impl OutputRelay {
    fn take_events(&self) -> Option<Receiver<ShellEvent>> {
        self.events.lock().unwrap().take()
    }
}

impl std::hash::Hash for OutputRelay {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// The drawing-page files the schematic pane opens with: `schematic.svg`
/// plus every `schematic-<n>.svg` in `output/` (engine
/// `ProjectOutput.discoverDrawingPages` ordering).
fn drawing_pages(output_dir: &Path) -> Vec<PathBuf> {
    let mut pages = Vec::new();
    if let Ok(entries) = std::fs::read_dir(output_dir) {
        let mut numbered: Vec<(i64, PathBuf)> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
            .filter_map(|path| {
                let stem = path.file_stem()?.to_str()?;
                let number = stem.strip_prefix("schematic-")?.parse::<i64>().ok()?;
                Some((number, path))
            })
            .collect();
        numbered.sort_by_key(|&(number, _)| number);
        pages = numbered.into_iter().map(|(_, path)| path).collect();
    }
    let first = output_dir.join("schematic.svg");
    if first.exists() {
        pages.insert(0, first);
    }
    pages
}

/// The export set (upstream `exportPackage`, Project.swift:437-441):
/// `model.py` first, then every non-dot entry of `output/`, each filtered
/// to what exists on disk — the exact file list `/usr/bin/zip -j`
/// receives. The output entries sort by name so the archive is
/// deterministic; upstream's directory order is unspecified and the SET
/// is what matters.
fn export_files(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![root.join("model.py")];
    if let Ok(entries) = std::fs::read_dir(root.join("output")) {
        let mut outputs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| !name.to_string_lossy().starts_with('.'))
            })
            .collect();
        outputs.sort();
        files.extend(outputs);
    }
    files.into_iter().filter(|path| path.exists()).collect()
}

impl ProjectWindow {
    /// Open a project (upstream `Project.init`, Project.swift:91-105):
    /// prepare the folder scaffolding, load metadata + the stored layout,
    /// load the chat (absent/corrupt opens empty), build the panes over
    /// `output/`, and launch the bridge. Returns the window plus the
    /// bridge's message-stream task — the app maps it into this window's
    /// messages. Must run inside the tokio runtime cosmic runs on.
    pub fn new(root: PathBuf, settings: &Settings) -> (Self, CosmicTask<bridge::Message>) {
        let (bridge, bridge_task) = Bridge::new(settings.clone(), root.clone());
        (Self::with_bridge(root, settings, bridge), bridge_task)
    }

    /// [`ProjectWindow::new`] over an already-launched bridge — the
    /// headless harness's seam: a cosmic Task's stream is private to the
    /// iced runtime, so tests launch through [`Bridge::launch`] (whose
    /// receiver they can tap) and fold the messages by hand exactly as the
    /// app maps them; the window then reads the same bridge's pipeline the
    /// live app reads.
    pub fn with_bridge(root: PathBuf, settings: &Settings, bridge: Bridge) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let _ = store::prepare(&root);
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Project".to_string());
        let metadata = store::load_metadata(&root)
            .unwrap_or_else(|_why| ProjectMetadata::new(env!("CARGO_PKG_VERSION")));
        let layout = metadata
            .layout
            .clone()
            .unwrap_or_else(|| panes::stored_layout(settings));
        let chat_visible = true;
        let grid = panes::from_layout(&panes::displayed(&layout, chat_visible));

        let output_dir = root.join("output");
        let manifest = Manifest::load(&output_dir.join("manifest.json")).ok();
        let files = drawing_pages(&output_dir);

        // The shell's own output watch: schematic reloads + missing-output
        // recomputes (the viewer pane's watch serves the 3D pane alone).
        let (events, receiver) = std::sync::mpsc::channel::<ShellEvent>();
        let watch = watch_directory(&output_dir, WATCH_DEBOUNCE)
            .ok()
            .map(|(handle, reloads)| {
                let events = events.clone();
                let _ = std::thread::Builder::new()
                    .name("project-output-watch".into())
                    .spawn(move || {
                        while let Ok(kind) = reloads.recv() {
                            if events.send(ShellEvent::Reload(kind)).is_err() {
                                return; // window gone
                            }
                        }
                    });
                handle
            });

        ProjectWindow {
            name,
            settings_path: engine::settings::settings_path(),
            metadata,
            layout,
            chat_visible,
            grid,
            size: DEFAULT_SIZE,
            chat: ChatState::new(store::load_chat(&root).unwrap_or_default()),
            chat_pane: ChatPane::new(),
            viewer: {
                let mut pane = ViewerPane::new(
                    &root,
                    settings.measurement_unit,
                    settings.printer_model.clone(),
                );
                // the overlay's collapse state survives launches
                // (Settings.parameters_collapsed — the pane's field docs)
                pane.set_parameters_collapsed(settings.parameters_collapsed);
                pane
            },
            schematic: SchematicPane::new(&files, manifest.as_ref()),
            bridge,
            queue: Vec::new(),
            queue_paused: false,
            activity: Activity::Idle,
            work_started_at: None,
            missing_outputs: activity::missing_output_files(&output_dir),
            shown_issues: None,
            version_card: None,
            focus_flash: None,
            last_size: Arc::new(Mutex::new(Size::default())),
            watch,
            events,
            relay: Arc::new(OutputRelay {
                id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                events: Mutex::new(Some(receiver)),
            }),
            root,
        }
    }

    /// Whether a turn is running (upstream `isBusy`).
    pub fn is_busy(&self) -> bool {
        self.activity.is_working()
    }

    /// Test seam: point the settings writer somewhere disposable — the
    /// default is the user's real settings file.
    pub fn set_settings_path_for_test(&mut self, path: PathBuf) {
        self.settings_path = path;
    }

    fn output_dir(&self) -> PathBuf {
        self.root.join("output")
    }

    /// An established save point (upstream `save()`, Project.swift:429-433):
    /// metadata with the current layout, timestamps bumped — written
    /// THROUGH the disk copy (the bridge's load-fresh law,
    /// [`bridge::fresh_metadata`]): the engine writes the session id into
    /// project.json mid-turn and `record_version` the version pointer, both
    /// behind the window's back, so those fields are re-read here, never
    /// held from open.
    fn save_metadata(&mut self) {
        let mut metadata = bridge::fresh_metadata(&self.root);
        metadata.layout = Some(self.layout.clone());
        metadata.updated_at = OffsetDateTime::now_utc();
        metadata.app_version = env!("CARGO_PKG_VERSION").to_string();
        self.metadata = metadata.clone();
        let _ = store::save_metadata(&self.root, &metadata);
    }

    /// Upstream `activity`'s didSet (Project.swift:74-83): entering work
    /// stamps the timer, leaving clears it.
    fn set_activity(&mut self, activity: Activity) {
        if activity.is_working() && !self.activity.is_working() {
            self.work_started_at = Some(Instant::now());
        } else if !activity.is_working() {
            self.work_started_at = None;
        }
        self.activity = activity;
    }

    /// Re-read the missing-output list from `output/`.
    fn refresh_missing_outputs(&mut self) {
        self.missing_outputs = activity::missing_output_files(&self.output_dir());
    }

    /// Reload the transcript fresh from chat.json, keeping the input
    /// bar's draft (the load-fresh law — the driver's bookkeeping saved
    /// rows behind the window's back; row ids reconcile to the disk
    /// truth restore and reopen load).
    fn reload_chat_rows(&mut self) {
        let rows = store::load_chat(&self.root).unwrap_or_default();
        self.chat.reload_rows(rows);
    }

    /// Apply `edit` to the engine settings file, load-fresh then save —
    /// the ONE-writer law's one sanctioned window path (the app itself
    /// saves the engine settings; the AgentMissingSheet custom-path
    /// precedent).
    fn persist_settings(&mut self, edit: impl FnOnce(&mut Settings)) {
        let mut settings = Settings::load(&self.settings_path).unwrap_or_default();
        edit(&mut settings);
        if let Err(why) = settings.save(&self.settings_path) {
            eprintln!("cad-studio: settings save failed: {why}");
        }
    }

    /// The engine display name the activity rows interpolate
    /// (activity::engine_name), resolved fresh from the settings file so a
    /// mid-conversation engine flip (Ruling 1: the setting is re-read per
    /// turn) renames the next transition too.
    fn engine_name(&self) -> &'static str {
        activity::engine_name(&Settings::load(&self.settings_path).unwrap_or_default())
    }

    /// Apply a structure change (preset switch, pane move): the grid
    /// rebuilds and both the project manifest and the global default
    /// layout persist eagerly (upstream `setLayout` → `persistLayout`).
    fn apply_layout_persist(&mut self, node: LayoutNode) {
        self.layout = node.clone();
        self.chat_visible = true;
        self.grid = panes::from_layout(&panes::displayed(&node, true));
        self.save_metadata();
        let _ = panes::persist_layout(&self.root, &self.settings_path, &node);
    }

    /// Rebuild the grid over the current layout (a visibility flip).
    fn rebuild_grid(&mut self) {
        self.grid = panes::from_layout(&panes::displayed(&self.layout, self.chat_visible));
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Chat(msg) => self.update_chat(msg),
            // the shell owns the compare state (parameters_view docs): the
            // picker's report reaches BOTH halves — the bridge stores the
            // selection (and parses the version's parameter values), and
            // the pane stages the version's MESH as the ghost from the
            // version-store directory the history writes (upstream
            // `compare(with:)`, Project.swift:363-368)
            Message::Viewer(crate::viewer_pane::Message::Parameters(
                parameters_view::Message::Compare(number),
            )) => {
                self.bridge.compare_with(number);
                let version = number.map(|n| {
                    (
                        n,
                        engine::project::versions::versions_root(&self.root).join(n.to_string()),
                    )
                });
                self.viewer.set_compare(version);
                Task::none()
            }
            // the slider/field commit the pane emits (the pane's local
            // half — value snap, draft clear — already ran): the bridge
            // rewrites model.py and starts the build, and the working
            // activity crosses to the window the moment it does
            Message::Viewer(crate::viewer_pane::Message::ParameterCommit(name, value)) => {
                let _ = self.bridge.set_parameter(&name, value);
                self.set_activity(self.bridge.activity().into());
                Task::none()
            }
            // the ask-the-agent "Crea animazione" button
            // (ModelPane.swift:138-146): one ordinary turn carrying the
            // localized prompt — the button is disabled while busy
            // (upstream `.disabled(project.isBusy)`); this guard is the
            // belt over the bridge's own busy refusal
            Message::Viewer(crate::viewer_pane::Message::RequestMotionStudy) => {
                if !self.is_busy() {
                    let _ = self.bridge.enqueue(
                        fl!("anima-il-movimento-di-questo-meccanismo-con-una-simulazione-fisica"),
                        Vec::new(),
                    );
                    self.set_activity(self.bridge.activity().into());
                }
                Task::none()
            }
            // the overlay's collapse toggle persists through the engine
            // settings (parameters_view's documented contract: the shell
            // intercepts, persists, then forwards — the ONE-writer law's
            // sanctioned path, the AgentMissingSheet custom-path precedent)
            Message::Viewer(crate::viewer_pane::Message::Parameters(
                crate::parameters_view::Message::Collapse(collapsed),
            )) => {
                self.persist_settings(|settings| settings.parameters_collapsed = collapsed);
                self.viewer
                    .update(crate::viewer_pane::Message::Parameters(
                        crate::parameters_view::Message::Collapse(collapsed),
                    ))
                    .map(Message::Viewer)
            }
            Message::Viewer(msg) => self.viewer.update(msg).map(Message::Viewer),
            Message::Schematic(msg) => self.schematic.update(msg).map(Message::Schematic),
            Message::Bridge(msg) => self.update_bridge(msg),
            Message::OutputReload(kind) => {
                match kind {
                    ReloadKind::Schematic => {
                        let files = drawing_pages(&self.output_dir());
                        let manifest =
                            Manifest::load(&self.output_dir().join("manifest.json")).ok();
                        self.schematic.reload(&files, manifest.as_ref());
                    }
                    // the 3D pane's own watch rebuilds the scene; the shell
                    // refreshes the activity model's warnings
                    ReloadKind::Mesh | ReloadKind::Plate | ReloadKind::Manifest => {
                        self.refresh_missing_outputs();
                    }
                    ReloadKind::Motion => {}
                }
                Task::none()
            }
            Message::GridResized(size) => {
                self.size = (size.width, size.height);
                Task::none()
            }
            Message::Tick => {
                self.focus_flash = None;
                Task::none()
            }
            Message::Command(command) => self.update_command(command),
            Message::Resized(ResizeEvent { split, ratio }) => {
                // EVERY drag goes through the clamp before State::resize —
                // iced enforces only a global min (the banked review law)
                let (width, height) = self.size;
                let clamped =
                    panes::clamp_resize(&self.grid, &split, ratio, width, height, GRID_SPACING);
                self.grid.resize(split, clamped);
                if self.chat_visible {
                    // upstream setFraction: memory only until an
                    // established save point
                    let node = panes::to_layout(&self.grid);
                    panes::set_fraction(&mut self.metadata, &node);
                    self.layout = node;
                }
                Task::none()
            }
            Message::Dragged(DragEvent::Picked { .. } | DragEvent::Canceled { .. }) => Task::none(),
            Message::Dragged(DragEvent::Dropped { pane, target }) => {
                let Some((target_kind, edge)) = drop_target(&self.grid, target) else {
                    return Task::none();
                };
                let Some(moving_kind) = self.grid.panes.get(&pane).copied() else {
                    return Task::none();
                };
                let node = panes::moving(&self.layout, moving_kind, edge, target_kind);
                self.apply_layout_persist(node);
                Task::none()
            }
            Message::AttachChosen(Ok(paths)) => {
                let staged: Vec<String> = paths
                    .into_iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                self.chat.attach(staged);
                Task::none()
            }
            Message::AttachChosen(Err(_cancelled)) => Task::none(),
            // upstream exportPackage's body, Project.swift:436-444: a
            // cancelled panel returns; the chosen destination zips
            // model.py + the non-dot outputs (the spawn is the app
            // layer's — argv only, destination removed first)
            Message::ExportChosen(Err(_cancelled)) => Task::none(),
            Message::ExportChosen(Ok(destination)) => {
                let files = export_files(&self.root);
                CosmicTask::future(async move {
                    Message::ExportDone(crate::app::zip_package(destination, files).await)
                })
            }
            // status 0 → the succeeded row + the reveal
            // (activateFileViewerSelecting); anything else → the failed
            // row — including zip not being installed
            Message::ExportDone(Some(destination)) => {
                self.set_activity(Activity::Succeeded(fl!("pacchetto-esportato")));
                CosmicTask::future(async move {
                    crate::app::reveal_path(destination).await;
                    Message::Tick
                })
            }
            Message::ExportDone(None) => {
                self.set_activity(Activity::Failed(fl!("esportazione-non-riuscita")));
                Task::none()
            }
            Message::ToggleIssues(kind) => {
                self.shown_issues = kind;
                // the window's popover is a single slot: the two cards are
                // mutually exclusive by construction
                if self.shown_issues.is_some() {
                    self.version_card = None;
                }
                Task::none()
            }
            Message::ToggleVersionCard => {
                self.version_card = if self.version_card.is_some() {
                    None
                } else {
                    Some(time::OffsetDateTime::now_utc())
                };
                if self.version_card.is_some() {
                    self.shown_issues = None;
                }
                Task::none()
            }
            // the app layer acts (PresentSetupSheet's one producer — the
            // bridge's PythonSheetRequested forward — already lands there;
            // re-emitting it here would loop the variant back into itself)
            Message::CloseRequested | Message::App(_) | Message::PresentSetupSheet => Task::none(),
            Message::Restore(number) => {
                // restoring dismisses the card it was picked from
                self.version_card = None;
                self.restore(number)
            }
        }
    }

    /// Restore a version snapshot (upstream `restore`, Project.swift:308-321,
    /// the view half around the bridge's state half): the bridge swaps the
    /// files and the metadata's version pointer; the window then reloads
    /// what it mirrors from disk — the metadata pointer the menu label and
    /// save points read, the chat transcript the snapshot saved — and
    /// clears the missing-output list (upstream `missingOutputs = []`, not
    /// a recompute; the output watch re-derives warnings from the restored
    /// outputs on its own debounce). The overlay's rows follow the
    /// parameters the bridge re-read from the restored manifest. A refusal
    /// (busy) or an engine failure changed nothing observable, so there is
    /// nothing to reload (upstream's catch skips the reloads too).
    fn restore(&mut self, number: i64) -> Task<Message> {
        if self.bridge.restore(number).is_err() || self.bridge.build_error().is_some() {
            return Task::none();
        }
        // the layout stays the window's: only the fields the bridge's save
        // owns cross over
        let fresh = bridge::fresh_metadata(&self.root);
        self.metadata.session_id = fresh.session_id;
        self.metadata.current_version = fresh.current_version;
        self.chat = ChatState::new(store::load_chat(&self.root).unwrap_or_default());
        self.missing_outputs.clear();
        let parameters = self.bridge.parameters();
        self.viewer.sync_parameters(&parameters);
        ChatPane::scroll_to_bottom().map(Message::Chat)
    }

    /// The chat pane: shell-owned messages act first (the send flow takes
    /// the draft BEFORE the pane or bridge hear anything), then the pane's
    /// widget state applies.
    fn update_chat(&mut self, msg: chat_view::Message) -> Task<Message> {
        match &msg {
            chat_view::Message::Send => {
                // take_draft FIRST; a None draft means nothing was sent and
                // the pane keeps its buffer (sent() is skipped)
                if let Some((text, attachments)) = self.chat.take_draft() {
                    match self.bridge.enqueue(text, attachments.clone()) {
                        Ok(()) => self.chat_pane.sent(),
                        // a refused send keeps the draft in the input bar
                        // (upstream chat.draft IS the bar's): the pane still
                        // holds the text — sent() is skipped and the tail
                        // sync re-mirrors it — the attachments go back, and
                        // the bridge's parked copy clears so the text has
                        // exactly one home
                        Err(_refused) => {
                            self.chat.draft_attachments = attachments;
                            self.bridge.clear_parked_draft();
                        }
                    }
                    // the pipeline's activity crosses now — the working
                    // status of a send that started, or the refusal's
                    // reason for one that did not (the Build arm's fold)
                    self.set_activity(self.bridge.activity().into());
                }
            }
            chat_view::Message::SendSuggestion(suggestion) => {
                let _ = self.bridge.enqueue(suggestion.clone(), Vec::new());
                self.set_activity(self.bridge.activity().into());
            }
            chat_view::Message::Stop => self.bridge.cancel(),
            chat_view::Message::RemoveQueued(id) => {
                let id = *id;
                self.bridge.remove_queued(id);
            }
            chat_view::Message::SendQueuedNow => self.bridge.resume_queue(),
            // the app layer runs the attach portal and feeds the result
            // back as AttachChosen
            chat_view::Message::Attach => {}
            // upstream's keyDown monitor (ChatInputView.swift:62-73): read
            // the clipboard's CONTENT; files or an image attach and the
            // press is swallowed, anything else falls through to the
            // editor's own text paste. The pin's read_data is real on
            // Wayland (data-control); where a backend answers nothing the
            // None arm IS the fall-through
            chat_view::Message::Paste => {
                return cosmic::iced::clipboard::read_data::<ClipboardContent>()
                    .then(|content| match content {
                        Some(content) => Task::done(Message::Chat(
                            chat_view::Message::AttachContent(content),
                        )),
                        None => cosmic::iced::clipboard::read().map(|text| {
                            Message::Chat(chat_view::Message::Draft(
                                text_editor::Action::Edit(text_editor::Edit::Paste(
                                    Arc::new(text.unwrap_or_default()),
                                )),
                            ))
                        }),
                    });
            }
            // pasted or dropped content becomes draft attachments — raw
            // image bytes via upstream's savedImage temp file
            // (Attachments.swift:21-32)
            chat_view::Message::AttachContent(content) => {
                let staged: Vec<String> = match content {
                    ClipboardContent::Files(paths) => paths
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                    ClipboardContent::Image(png) => crate::attachments::saved_image(&png)
                        .map(|path| vec![path.to_string_lossy().into_owned()])
                        .unwrap_or_default(),
                };
                if !staged.is_empty() {
                    self.chat.attach(staged);
                }
            }
            // the draft strip's remove button (ChatViewModel.swift:26-28)
            chat_view::Message::Detach(path) => self.chat.detach(path),
            // a system row's Open Terminal action (MessageView.swift:71-77)
            chat_view::Message::RowAction(_, Action::OpenTerminal) => {
                crate::app::launch_terminal();
            }
            // the build-failure row's action: the row's detail (the build
            // output carrying the traceback) goes back to the agent as an
            // ordinary turn (upstream askClaudeToFix, Project.swift:258-260)
            chat_view::Message::RowAction(id, Action::AskClaudeToFix) => {
                let traceback = self
                    .chat
                    .messages
                    .iter()
                    .find(|row| row.id == *id)
                    .and_then(|row| row.detail.clone())
                    .unwrap_or_default();
                let _ = self.bridge.ask_agent_to_fix(&traceback);
                self.set_activity(self.bridge.activity().into());
            }
            // ConfigureClaude is the M8 settings window
            chat_view::Message::RowAction(_, Action::ConfigureClaude) => {}
            // a markdown link: the system opener (upstream NSWorkspace.open)
            chat_view::Message::OpenUrl(url) => {
                let url = url.clone();
                crate::app::open_url(&url);
            }
            _ => {}
        }
        let task = self.chat_pane.update(msg).map(Message::Chat);
        if self.chat.draft != self.chat_pane.draft() {
            self.chat.draft = self.chat_pane.draft();
        }
        task
    }

    /// The app layer's attach entry point: runs the file portal and feeds
    /// the result back as [`Message::AttachChosen`].
    pub fn attach(&self) -> Task<Message> {
        let future = crate::app::choose_files("Attach reference images or files");
        CosmicTask::future(async move { Message::AttachChosen(future.await) })
    }

    fn update_bridge(&mut self, msg: bridge::Message) -> Task<Message> {
        match msg {
            bridge::Message::TurnStarted {
                prompt,
                attachments,
            } => {
                // the user row joins the visible transcript when the send
                // actually starts (upstream appends it in `send`, before
                // the agent runs — the bridge message's contract)
                self.chat.append(store::ChatMessage {
                    id: uuid::Uuid::new_v4(),
                    role: store::Role::User,
                    text: prompt,
                    date: OffsetDateTime::now_utc(),
                    tool: None,
                    detail: None,
                    action: None,
                    attachments,
                });
                self.set_activity(activity::next(
                    activity::Signal::TurnStarted,
                    false,
                    self.engine_name(),
                ));
                ChatPane::scroll_to_bottom().map(Message::Chat)
            }
            bridge::Message::Event(event) => {
                // the chat reducer owns the rows; the activity folds after
                self.chat.apply(&event);
                match &event {
                    EngineEvent::ToolStarted { title, .. } => {
                        self.set_activity(activity::next(
                            activity::Signal::ToolOpened(title.as_str()),
                            false,
                            self.engine_name(),
                        ));
                    }
                    EngineEvent::TurnFinished(outcome) => {
                        // upstream agentDidFinish: outputs reload before the
                        // classification reads them (Project.swift:262-276)
                        self.refresh_missing_outputs();
                        let empty = self.missing_outputs.is_empty();
                        self.set_activity(activity::next(
                            activity::Signal::TurnFinished(outcome),
                            empty,
                            self.engine_name(),
                        ));
                        // a turn end is an established layout save point
                        self.save_metadata();
                    }
                    _ => {}
                }
                ChatPane::scroll_to_bottom().map(Message::Chat)
            }
            bridge::Message::Closed => {
                self.set_activity(activity::next(activity::Signal::Closed, false, self.engine_name()));
                ChatPane::scroll_to_bottom().map(Message::Chat)
            }
            bridge::Message::QueueChanged { queue, paused } => {
                self.queue = queue
                    .into_iter()
                    .map(|item| QueueItem {
                        id: item.id,
                        text: item.text,
                        images: item.images,
                    })
                    .collect();
                self.queue_paused = paused;
                Task::none()
            }
            // The build pipeline's window surface: the driver's bookkeeping
            // (version summary rows, the build-failure row) lands on disk
            // behind the window's back, so both terminal arms reload the
            // transcript fresh — keeping the input bar's draft — and the
            // activity folds from the pipeline, the same source that
            // classified the outcome. Both also sync the overlay's
            // parameter rows from the pipeline's refreshed set (the M6
            // carry-forward landed; restore's sync): BuildFinished re-read
            // the manifest, ParameterCommitted moved a row's value.
            bridge::Message::ParameterCommitted { .. } => {
                let parameters = self.bridge.parameters();
                self.viewer.sync_parameters(&parameters);
                Task::none()
            }
            // upstream's build is buffered (ModelBuilder.build returns one
            // result, no per-line surface) — the events exist for the drain
            bridge::Message::BuildEventBridge(_) => Task::none(),
            bridge::Message::BuildFinished { .. } => {
                self.refresh_missing_outputs();
                self.reload_chat_rows();
                // the driver's record_version moved current_version on
                // disk; the version menu's checkmark and the root's label
                // read the window's copy — refresh from the fresh metadata
                // exactly as restore does (display currency only; the
                // load-fresh law already protects the file)
                let fresh = bridge::fresh_metadata(&self.root);
                self.metadata.session_id = fresh.session_id;
                self.metadata.current_version = fresh.current_version;
                let parameters = self.bridge.parameters();
                self.viewer.sync_parameters(&parameters);
                self.set_activity(self.bridge.activity().into());
                ChatPane::scroll_to_bottom().map(Message::Chat)
            }
            bridge::Message::TurnSuccessBookkeeping => {
                self.refresh_missing_outputs();
                self.save_metadata();
                self.reload_chat_rows();
                self.set_activity(self.bridge.activity().into());
                ChatPane::scroll_to_bottom().map(Message::Chat)
            }
            bridge::Message::PythonSheetRequested => {
                CosmicTask::future(async move { Message::PresentSetupSheet })
            }
        }
    }

    fn update_command(&mut self, command: Command) -> Task<Message> {
        match command {
            Command::ShowChat => {
                self.chat_visible = !self.chat_visible;
                self.rebuild_grid();
                Task::none()
            }
            Command::FocusChat => {
                // upstream focus(.chat) shows the pane and focuses input
                if !self.chat_visible {
                    self.chat_visible = true;
                    self.rebuild_grid();
                }
                self.focus_flash = Some(PaneKind::Chat);
                ChatPane::focus_input().map(Message::Chat)
            }
            Command::FocusModel => {
                self.focus_flash = Some(PaneKind::Model3D);
                Task::none()
            }
            Command::FocusSchematic => {
                self.focus_flash = Some(PaneKind::Schematic);
                Task::none()
            }
            Command::ToggleParameters => self
                .viewer
                .update(crate::viewer_pane::Message::ToggleParameters)
                .map(Message::Viewer),
            Command::LayoutChatLeading => {
                self.apply_layout_persist(LayoutNode::chat_leading());
                Task::none()
            }
            Command::LayoutChatTrailing => {
                self.apply_layout_persist(LayoutNode::chat_trailing());
                Task::none()
            }
            Command::LayoutChatBottom => {
                self.apply_layout_persist(LayoutNode::chat_bottom());
                Task::none()
            }
            Command::LayoutThreeColumns => {
                self.apply_layout_persist(LayoutNode::three_columns());
                Task::none()
            }
            Command::ViewerPreset(preset) => self
                .viewer
                .update(crate::viewer_pane::Message::Preset(preset))
                .map(Message::Viewer),
            Command::ViewerFit => self
                .viewer
                .update(crate::viewer_pane::Message::Fit)
                .map(Message::Viewer),
            Command::SchematicZoomIn => self
                .schematic
                .update(crate::schematic::Message::ZoomIn)
                .map(Message::Schematic),
            Command::SchematicZoomOut => self
                .schematic
                .update(crate::schematic::Message::ZoomOut)
                .map(Message::Schematic),
            Command::SchematicFit => self
                .schematic
                .update(crate::schematic::Message::Fit)
                .map(Message::Schematic),
            Command::Stop => {
                self.bridge.cancel();
                Task::none()
            }
            Command::Build => {
                // the menu entry is disabled while busy (upstream's guard);
                // the shortcut can still land, where the bridge's own busy
                // refusal is the guard. The working (or refused-guard
                // failed) activity crosses to the window immediately — the
                // stop button and the elapsed timer ride it.
                let _ = self.bridge.start_build(None);
                self.set_activity(self.bridge.activity().into());
                Task::none()
            }
            Command::Export => {
                // The menu item this shortcut rides is disabled without
                // model.3mf (Commands.swift:131's hasThreeMF gate) and NOT
                // while busy — upstream's menu/toolbar split, copied as-is.
                // The keybind is the menu item's, so the arm repeats only
                // its gate.
                if !self.output_dir().join("model.3mf").exists() {
                    return Task::none();
                }
                let name = self.name.clone();
                CosmicTask::future(async move {
                    Message::ExportChosen(crate::app::choose_save_file(&name).await)
                })
            }
            Command::Reveal => {
                let root = self.root.clone();
                CosmicTask::future(async move {
                    crate::app::reveal_path(root).await;
                    Message::Tick
                })
            }
            Command::OpenInSlicer => {
                // upstream openInSlicer: the plate when present, else the
                // model (Project.swift:457-459)
                let output = self.output_dir();
                let plate = output.join("plate.3mf");
                let target = if plate.exists() {
                    plate
                } else {
                    output.join("model.3mf")
                };
                crate::app::open_path(&target);
                Task::none()
            }
        }
    }

    /// The window's subscriptions: the panes', the output relay, and the
    /// 1 s tick while anything animates (elapsed label or focus flash).
    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            self.viewer.subscription().map(Message::Viewer),
            self.schematic.subscription().map(Message::Schematic),
            Subscription::run_with(self.relay.clone(), relay_stream),
        ];
        if self.is_busy() || self.focus_flash.is_some() {
            subs.push(every(Duration::from_secs(1)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }

    /// The whole window: header over the pane grid (upstream
    /// ProjectSplitView), with the open issue list as a popover.
    pub fn view(&self) -> Element<'_, Message> {
        // the grid reports its laid-out size through the shell relay: the
        // resize clamp needs the GRID extent, and the window size would
        // ignore the header (the panes' responsive report pattern)
        let events = self.events.clone();
        let last_size = self.last_size.clone();
        let this = self;
        let sized = cosmic::iced::widget::responsive(move |size: Size| {
            {
                let mut last = last_size.lock().unwrap();
                if *last != size {
                    *last = size;
                    let _ = events.send(ShellEvent::GridResized(size));
                }
            }
            this.view_grid()
        });
        let base: Element<'_, Message> = Column::with_capacity(2)
            .push(self.view_header())
            .push(sized)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();

        // the issue list and the version history card ride as non-modal
        // popovers over the content — one slot, mutually exclusive
        let mut pop = popover(base);
        if let Some(now) = self.version_card {
            pop = pop.popup(crate::version_menu::view_versions(
                &version_menu::menu_entries(&self.bridge.versions(), self.metadata.current_version),
                self.is_busy(),
                now,
                Message::Restore,
            ));
        } else if let Some(kind) = self.shown_issues {
            pop = pop.popup(self.view_issue_list(kind));
        }
        pop.into()
    }

    /// The resizable pane grid (rebuilt per layout — iced elements are
    /// single-use), filling the window below the header.
    fn view_grid(&self) -> Element<'_, Message> {
        let grid = pane_grid::PaneGrid::new(&self.grid, |_pane, kind: &PaneKind, _| {
            let body: Element<'_, Message> = match kind {
                // upstream drops onto the WHOLE chat view
                // (ChatView.swift:20-31); the Wayland-native destination
                // the pin exposes wraps it, over the same mimes paste
                // reads (winit has no Wayland file drops — the window-
                // level FileDropped fallback covers X11 and macOS)
                PaneKind::Chat => cosmic::widget::DndDestination::<Message>::for_data(
                    chat_view::view_chat(
                        &self.chat_pane,
                        &self.chat,
                        self.is_busy(),
                        &self.queue,
                        self.queue_paused,
                        &self.root,
                    )
                    .map(Message::Chat),
                    |content: Option<ClipboardContent>, _action| {
                        // a drop that interpreted as nothing attaches
                        // nothing, like upstream's providers that yield
                        // no file URL and no image data
                        let content = content.unwrap_or(ClipboardContent::Files(Vec::new()));
                        Message::Chat(chat_view::Message::AttachContent(content))
                    },
                )
                .into(),
                PaneKind::Model3D => {
                    // the compare wiring's shell half: the overlay's previous
                    // values, gate, and picker inputs are what only the
                    // bridge knows (the pane still gates visibility)
                    let previous = self.bridge.comparison_parameters();
                    let versions = self.bridge.versions();
                    let overlay = parameters_view::OverlayContext {
                        previous: &previous,
                        busy: self.is_busy(),
                        versions: &versions,
                        compare: self.bridge.compare_version(),
                    };
                    self.viewer
                        .view_model_3d(Some(overlay))
                        .map(Message::Viewer)
                }
                PaneKind::Schematic => self.schematic.view().map(Message::Schematic),
            };
            // the focus command's one-second highlight (upstream
            // PaneFocusRequest's flash)
            let flashing = self.focus_flash == Some(*kind);
            let framed: Element<'_, Message> = container(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .class(flash_border(flashing))
                .into();
            pane_grid::Content::new(framed)
        })
        .spacing(GRID_SPACING)
        .on_resize(6, Message::Resized)
        .on_drag(Message::Dragged);
        container(grid)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The toolbar (ProjectToolbar.swift, M4 subset): chat toggle + stop |
    /// ActivityViewer | commands menu + reveal + export + slicer.
    fn view_header(&self) -> Element<'_, Message> {
        let chat_toggle = button::icon(
            cosmic::widget::icon::from_name("view-column")
                .size(16)
                .handle(),
        )
        .on_press(Message::Command(Command::ShowChat))
        .name("show-chat");

        let stop = button::icon(
            cosmic::widget::icon::from_name("media-playback-stop")
                .size(16)
                .handle(),
        )
        .on_press_maybe(self.is_busy().then_some(Message::Command(Command::Stop)))
        .name("stop");

        let reveal = button::icon(cosmic::widget::icon::from_name("folder").size(16).handle())
            .on_press(Message::Command(Command::Reveal))
            .name("reveal");

        // upstream gates the slicer button on output.hasThreeMF
        let has_3mf = self.output_dir().join("model.3mf").exists();

        // "Esporta pacchetto" (ProjectWindow.swift:135-140) — upstream's
        // share glyph; disabled !hasThreeMF || isBusy (the toolbar's
        // STRICTER gate — the menu entry above stays clickable while busy,
        // upstream's split, copied as-is). The tooltip is upstream's
        // `.help` naming the archive's contents.
        let export = button::icon(
            cosmic::widget::icon::from_name("emblem-shared")
                .size(16)
                .handle(),
        )
        .on_press_maybe((has_3mf && !self.is_busy()).then_some(Message::Command(Command::Export)))
        .name(fl!("esporta-pacchetto"));
        let export = cosmic::widget::tooltip(
            export,
            cosmic::widget::text(fl!(
                "esporta-step-3mf-stl-tavola-pdf-e-model-py-in-un-archivio-zip-e"
            )),
            cosmic::widget::tooltip::Position::FollowCursor,
        );

        let slicer = button::icon(
            cosmic::widget::icon::from_name("document-export")
                .size(16)
                .handle(),
        )
        .on_press_maybe(has_3mf.then_some(Message::Command(Command::OpenInSlicer)))
        .name("open-in-slicer");

        cosmic::widget::header_bar()
            .title(self.name.clone())
            .on_close(Message::CloseRequested)
            .start(chat_toggle)
            .start(stop)
            .center(self.view_activity())
            .end(crate::version_menu::version_button(
                self.metadata.current_version,
                self.version_card.is_some(),
                Message::ToggleVersionCard,
            ))
            .end(self.view_menu())
            .end(reveal)
            .end(export)
            .end(slicer)
            .into()
    }

    /// The minimal ActivityViewer port (Shared/ActivityViewer.swift):
    /// project name, status icon + text, elapsed, and the error/warning
    /// issue buttons — the upstream home of output warnings, plus M7's
    /// motion error and collision warnings from the viewer pane. The full
    /// port's manifest warnings are M8's.
    fn view_activity(&self) -> Element<'_, Message> {
        let mut row = Row::new()
            .spacing(6)
            .push(text(self.name.clone()).size(12))
            .push(text("›".to_string()).size(12).class(dim_text()));
        row = match &self.activity {
            Activity::Idle => row.push(text(fl!("pronto")).size(12).class(dim_text())),
            Activity::Working(_) => row
                .push(cosmic::widget::progress_bar::indeterminate_circular())
                .push(
                    text(self.activity.text().to_string())
                        .size(12)
                        .class(dim_text()),
                ),
            Activity::Succeeded(_) => row
                .push(
                    cosmic::widget::icon::from_name("object-select")
                        .size(12)
                        .icon(),
                )
                .push(
                    text(self.activity.text().to_string())
                        .size(12)
                        .class(cosmic::theme::Text::Color(ok_color())),
                ),
            Activity::Failed(_) => row
                .push(
                    cosmic::widget::icon::from_name("dialog-error")
                        .size(12)
                        .icon(),
                )
                .push(
                    text(self.activity.text().to_string())
                        .size(12)
                        .class(cosmic::theme::Text::Color(error_color())),
                ),
        };
        if let Some(start) = self.work_started_at {
            row = row.push(
                text(activity::elapsed_text(start.elapsed().as_secs()))
                    .size(12)
                    .font(cosmic::font::mono())
                    .class(dim_text()),
            );
        }
        row = row.push(Space::new().width(Length::Fill));

        let issues = activity::issues(
            self.bridge.build_error().as_deref(),
            &self.missing_outputs,
            self.viewer.motion_error(),
            self.viewer.collision_frames(),
        );
        for kind in [IssueKind::Error, IssueKind::Warning] {
            let matching: Vec<_> = issues.iter().filter(|i| i.kind == kind).collect();
            if matching.is_empty() {
                continue;
            }
            let open = self.shown_issues == Some(kind);
            row = row.push(
                button::custom(
                    Row::new()
                        .spacing(3)
                        .push(
                            cosmic::widget::icon::from_name(match kind {
                                IssueKind::Error => "dialog-error",
                                IssueKind::Warning => "dialog-warning",
                            })
                            .size(12)
                            .icon(),
                        )
                        .push(text(format!("{}", matching.len())).size(12)),
                )
                .class(if open {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Standard
                })
                .on_press(Message::ToggleIssues((!open).then_some(kind)))
                .padding([2, 8]),
            );
        }
        container(row.padding([2, 10])).width(Length::Fill).into()
    }

    /// The issue list popover (upstream `IssueList`).
    fn view_issue_list(&self, kind: IssueKind) -> Element<'_, Message> {
        let issues = activity::issues(
            self.bridge.build_error().as_deref(),
            &self.missing_outputs,
            self.viewer.motion_error(),
            self.viewer.collision_frames(),
        );
        let mut list = Column::new()
            .spacing(8)
            .padding(12)
            .max_width(380)
            .width(Length::Fill);
        for issue in issues.iter().filter(|i| i.kind == kind) {
            list = list.push(
                Row::new()
                    .spacing(8)
                    .push(
                        cosmic::widget::icon::from_name(match kind {
                            IssueKind::Error => "dialog-error",
                            IssueKind::Warning => "dialog-warning",
                        })
                        .size(14)
                        .icon(),
                    )
                    .push(text(issue.message.clone()).size(12)),
            );
        }
        list = list.push(
            Row::new().push(Space::new().width(Length::Fill)).push(
                button::text(fl!("chiudi"))
                    .class(cosmic::theme::Button::Standard)
                    .on_press(Message::ToggleIssues(None)),
            ),
        );
        container(list)
            .width(Length::Fill)
            .class(cosmic::theme::Container::custom(|theme| {
                cosmic::iced::widget::container::Style {
                    background: Some(cosmic::iced::Background::Color(
                        theme.cosmic().background(false).base.into(),
                    )),
                    border: cosmic::iced::Border {
                        color: theme.cosmic().background(false).on.into(),
                        width: 1.0,
                        radius: theme.cosmic().corner_radii.radius_s.into(),
                    },
                    ..Default::default()
                }
            }))
            .into()
    }

    /// The commands menu bar (the M4 subset of upstream AppCommands plus M5's
    /// Model root and version menu): a "File" root bubbling the app-level
    /// commands, a "View" root over the pane/layout/viewer/schematic
    /// commands, a "Model" root (upstream "Modello": Compila + Stop +
    /// Esporta), and
    /// the version menu root (VersionMenu.swift) trailing before the reveal
    /// and slicer buttons.
    fn view_menu(&self) -> Element<'static, Message> {
        use cosmic::widget::menu::{Item, ItemHeight, ItemWidth, items, root};

        let file_entries = vec![
            Item::Button(fl!("crea-nuovo-progetto"), None, AppRequest::NewProject),
            Item::Button(fl!("apri-progetto"), None, AppRequest::OpenProject),
            Item::Button(fl!("apri-recenti"), None, AppRequest::OpenRecent),
            Item::Divider,
            Item::Button(fl!("finestra-di-benvenuto"), None, AppRequest::Welcome),
        ];
        let file_binds = std::collections::HashMap::new();
        let file_tree = cosmic::widget::menu::Tree::with_children(
            {
                let root_button: Element<'_, Message> = root(fl!("menu-file")).into();
                root_button
            },
            items(&file_binds, file_entries),
        );

        // `compila-modello` (Commands.swift:115-119): disabled while busy,
        // like upstream's `.disabled(project?.isBusy == true)`
        let build_entry = if self.is_busy() {
            Item::ButtonDisabled(fl!("compila-modello"), None, Command::Build)
        } else {
            Item::Button(fl!("compila-modello"), None, Command::Build)
        };
        // `esporta-pacchetto-2` ("Esporta pacchetto…", Commands.swift:130-132):
        // gated on hasThreeMF only — the menu does NOT gate on busy
        // (upstream's menu/toolbar split). Button↔ButtonDisabled keeps the
        // item SET fixed — the static-roots law's build_entry precedent.
        let export_entry = if self.output_dir().join("model.3mf").exists() {
            Item::Button(fl!("esporta-pacchetto-2"), None, Command::Export)
        } else {
            Item::ButtonDisabled(fl!("esporta-pacchetto-2"), None, Command::Export)
        };
        let model_binds = std::collections::HashMap::new();
        let model_tree = cosmic::widget::menu::Tree::with_children(
            {
                let root_button: Element<'_, Message> = root(fl!("modello")).into();
                root_button
            },
            items(
                &model_binds,
                vec![
                    build_entry,
                    Item::Divider,
                    Item::Button(fl!("stop"), None, Command::Stop),
                    Item::Divider,
                    export_entry,
                ],
            ),
        );

        let entries = vec![
            Item::Button(
                if self.chat_visible {
                    fl!("nascondi-chat")
                } else {
                    fl!("mostra-chat")
                },
                None,
                Command::ShowChat,
            ),
            Item::Divider,
            Item::Button(fl!("vai-alla-chat"), None, Command::FocusChat),
            Item::Button(fl!("vai-al-viewer-3d"), None, Command::FocusModel),
            Item::Button(fl!("vai-alla-schematica-2d"), None, Command::FocusSchematic),
            Item::Divider,
            Item::Folder(
                fl!("layout"),
                vec![
                    Item::Button(fl!("chat-a-sinistra"), None, Command::LayoutChatLeading),
                    Item::Button(fl!("chat-a-destra"), None, Command::LayoutChatTrailing),
                    Item::Button(fl!("chat-in-basso"), None, Command::LayoutChatBottom),
                    Item::Button(fl!("tre-colonne"), None, Command::LayoutThreeColumns),
                ],
            ),
            Item::Folder(
                fl!("viewer-3d"),
                vec![
                    Item::Button(
                        fl!("vista-isometrica"),
                        None,
                        Command::ViewerPreset(Preset::Iso),
                    ),
                    Item::Button(
                        fl!("vista-frontale"),
                        None,
                        Command::ViewerPreset(Preset::Front),
                    ),
                    Item::Button(
                        fl!("vista-dall-alto"),
                        None,
                        Command::ViewerPreset(Preset::Top),
                    ),
                    Item::Button(
                        fl!("vista-da-destra"),
                        None,
                        Command::ViewerPreset(Preset::Right),
                    ),
                    Item::Divider,
                    Item::Button(fl!("adatta-alla-vista"), None, Command::ViewerFit),
                ],
            ),
            Item::Folder(
                fl!("schematica-2d"),
                vec![
                    Item::Button(fl!("ingrandisci"), None, Command::SchematicZoomIn),
                    Item::Button(fl!("riduci"), None, Command::SchematicZoomOut),
                    Item::Divider,
                    Item::Button(fl!("adatta-alla-finestra"), None, Command::SchematicFit),
                ],
            ),
        ];
        let key_binds = std::collections::HashMap::new();
        let root_button: Element<'_, Message> = root(fl!("menu-view")).into();
        let tree =
            cosmic::widget::menu::Tree::with_children(root_button, items(&key_binds, entries));
        // STATIC roots only: cosmic's menu overlay corrupts widget state
        // when an item set changes between rebuilds (the version menu
        // panicked "Downcast widget state" the moment a version landed
        // mid-turn; version_menu's module doc carries the chain). Dynamic
        // dropdowns — the version history — live as popovers instead.
        cosmic::widget::menu::bar(vec![file_tree, tree, model_tree])
            .item_width(ItemWidth::Static(150))
            .item_height(ItemHeight::Dynamic(30))
            .into()
    }
}

fn dim_text() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| {
        let mut color = Color::from(theme.cosmic().background(false).on);
        color.a *= 0.65;
        cosmic::iced::widget::text::Style {
            color: Some(color),
            ..Default::default()
        }
    })
}

fn ok_color() -> Color {
    Color::from_rgb(0.18, 0.66, 0.34)
}

fn error_color() -> Color {
    Color::from_rgb(0.90, 0.32, 0.32)
}

/// The focus flash's pane border.
fn flash_border(flashing: bool) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let mut style = cosmic::iced::widget::container::Style::default();
        if flashing {
            style.border = cosmic::iced::Border {
                color: Color::from(theme.cosmic().accent_color()),
                width: 2.0,
                radius: theme.cosmic().corner_radii.radius_s.into(),
            };
        }
        style
    })
}

/// Map an iced pane-grid drop target onto the upstream
/// (target pane, edge) pair. Only a pane's edge regions map — upstream has
/// no center-drop and no whole-grid-edge drop.
fn drop_target(
    grid: &pane_grid::State<PaneKind>,
    target: pane_grid::Target,
) -> Option<(PaneKind, panes::DropEdge)> {
    let (pane, region) = match target {
        pane_grid::Target::Pane(pane, region) => (pane, region),
        pane_grid::Target::Edge(_) => return None,
    };
    let kind = grid.panes.get(&pane).copied()?;
    let edge = match region {
        pane_grid::Region::Edge(pane_grid::Edge::Top) => panes::DropEdge::Top,
        pane_grid::Region::Edge(pane_grid::Edge::Left) => panes::DropEdge::Leading,
        pane_grid::Region::Edge(pane_grid::Edge::Right) => panes::DropEdge::Trailing,
        pane_grid::Region::Edge(pane_grid::Edge::Bottom) => panes::DropEdge::Bottom,
        pane_grid::Region::Center => return None,
    };
    Some((kind, edge))
}

/// The output-watch relay stream (the panes' take-once pattern).
fn relay_stream(relay: &Arc<OutputRelay>) -> BoxStream<'static, Message> {
    stream::channel(64, {
        let relay = relay.clone();
        async move |output| {
            let Some(events) = relay.take_events() else {
                // a second subscription over the same relay: the first owns
                // the receiver, this stream idles forever
                std::future::pending::<()>().await;
                return;
            };
            let mut sender = output.clone();
            let _ = std::thread::Builder::new()
                .name("project-output-events".to_string())
                .spawn(move || {
                    while let Ok(event) = events.recv() {
                        let message = match event {
                            ShellEvent::Reload(kind) => Message::OutputReload(kind),
                            ShellEvent::GridResized(size) => Message::GridResized(size),
                        };
                        let mut backoff = Duration::from_millis(1);
                        let started = Instant::now();
                        loop {
                            match sender.try_send(message.clone()) {
                                Ok(()) => break,
                                Err(err) if err.is_disconnected() => return,
                                Err(err) => {
                                    // liveness over completeness (the panes'
                                    // retry budget)
                                    if started.elapsed() > Duration::from_secs(2) {
                                        break;
                                    }
                                    let _ = err.into_inner();
                                    std::thread::sleep(backoff);
                                    backoff = (backoff * 2).min(Duration::from_millis(64));
                                }
                            }
                        }
                    }
                });
            std::future::pending::<()>().await;
        }
    })
    .boxed()
}

/// The M4 shortcut table (upstream keyboardShortcut values, ⌘→Ctrl,
/// ⌥→Alt). Pure: the app routes a key press through this before the
/// focused window sees it.
pub fn project_shortcut(key: &Key, modifiers: &Modifiers) -> Option<Command> {
    use Command::*;
    let ctrl = modifiers.control();
    let alt = modifiers.alt();
    let shift = modifiers.shift();
    let character = match key {
        Key::Character(ch) => ch.to_lowercase(),
        _ => return None,
    };
    match (ctrl, alt, shift, character.as_str()) {
        (true, false, false, "0") => Some(ShowChat),
        (true, false, false, "1") => Some(FocusChat),
        (true, false, false, "2") => Some(FocusModel),
        (true, false, false, "3") => Some(FocusSchematic),
        (true, false, false, "l") => Some(FocusChat),
        (true, true, false, "1") => Some(ViewerPreset(Preset::Iso)),
        (true, true, false, "2") => Some(ViewerPreset(Preset::Front)),
        (true, true, false, "3") => Some(ViewerPreset(Preset::Top)),
        (true, true, false, "4") => Some(ViewerPreset(Preset::Right)),
        (true, true, false, "0") => Some(ViewerFit),
        // ⌥⌘I parameters (Commands.swift:69-73) — the app-wide arm (the
        // viewer pane's subscription no longer carries it)
        (true, true, false, "i") => Some(ToggleParameters),
        (true, false, false, "+") | (true, false, false, "=") => Some(SchematicZoomIn),
        (true, false, false, "-") => Some(SchematicZoomOut),
        // ⌘. stop — the period key arrives as a character
        (true, false, false, ".") => Some(Stop),
        // ⌘B build (Commands.swift:118-120)
        (true, false, false, "b") => Some(Build),
        // ⇧⌘E export package (Commands.swift:130-132)
        (true, false, true, "e") => Some(Export),
        (true, false, true, "r") => Some(Reveal),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shortcut in the table is a command the window implements, and
    /// the upstream bindings land on the expected commands
    /// (Commands.swift:48-137).
    #[test]
    fn shortcut_table_matches_upstream() {
        let ctrl = Modifiers::CTRL;
        let ctrl_alt = Modifiers::CTRL | Modifiers::ALT;
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;

        assert_eq!(
            project_shortcut(&Key::Character("0".into()), &ctrl),
            Some(Command::ShowChat),
            "⌘0 shows/hides chat"
        );
        assert_eq!(
            project_shortcut(&Key::Character("1".into()), &ctrl),
            Some(Command::FocusChat)
        );
        assert_eq!(
            project_shortcut(&Key::Character("2".into()), &ctrl),
            Some(Command::FocusModel)
        );
        assert_eq!(
            project_shortcut(&Key::Character("3".into()), &ctrl),
            Some(Command::FocusSchematic)
        );
        assert_eq!(
            project_shortcut(&Key::Character("l".into()), &ctrl),
            Some(Command::FocusChat),
            "⌘L is also focus-chat (upstream 'Scrivi un messaggio')"
        );
        assert_eq!(
            project_shortcut(&Key::Character("1".into()), &ctrl_alt),
            Some(Command::ViewerPreset(Preset::Iso))
        );
        assert_eq!(
            project_shortcut(&Key::Character("0".into()), &ctrl_alt),
            Some(Command::ViewerFit)
        );
        assert_eq!(
            project_shortcut(&Key::Character("i".into()), &ctrl_alt),
            Some(Command::ToggleParameters),
            "⌥⌘I toggles the parameters overlay (Commands.swift:69-73)"
        );
        assert_eq!(
            project_shortcut(&Key::Character("i".into()), &ctrl),
            None,
            "no parameters binding without ⌥"
        );
        assert_eq!(
            project_shortcut(&Key::Character("=".into()), &ctrl),
            Some(Command::SchematicZoomIn),
            "the unshifted + key shares the zoom-in binding"
        );
        assert_eq!(
            project_shortcut(&Key::Character("-".into()), &ctrl),
            Some(Command::SchematicZoomOut)
        );
        assert_eq!(
            project_shortcut(&Key::Character(".".into()), &ctrl),
            Some(Command::Stop)
        );
        assert_eq!(
            project_shortcut(&Key::Character("r".into()), &ctrl_shift),
            Some(Command::Reveal)
        );
        // ⌘B builds the model (Commands.swift:118-120)
        assert_eq!(
            project_shortcut(&Key::Character("b".into()), &ctrl),
            Some(Command::Build)
        );
        // ⇧⌘E exports the package (Commands.swift:130-132)
        assert_eq!(
            project_shortcut(&Key::Character("e".into()), &ctrl_shift),
            Some(Command::Export)
        );
        assert_eq!(
            project_shortcut(&Key::Character("e".into()), &ctrl),
            None,
            "export needs the shift"
        );
        assert_eq!(
            project_shortcut(&Key::Character("1".into()), &Modifiers::empty()),
            None,
            "no binding without Ctrl"
        );
    }

    /// The export set matches upstream's package (Project.swift:437-441):
    /// model.py leads, output's non-dot entries follow (sorted — the
    /// archive is deterministic; upstream's directory order is arbitrary),
    /// and only files that exist make the list.
    #[test]
    fn export_files_builds_the_upstream_package_set() {
        let root = std::env::temp_dir().join(format!("cad-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("output")).unwrap();
        std::fs::write(root.join("model.py"), "wall = 3\n").unwrap();
        for name in [
            "model.3mf",
            "model.stl",
            "drawing.pdf",
            ".DS_Store",
            ".hidden.tmp",
        ] {
            std::fs::write(root.join("output").join(name), b"x").unwrap();
        }
        let names: Vec<_> = export_files(&root)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                "model.py".to_string(),
                "drawing.pdf".to_string(),
                "model.3mf".to_string(),
                "model.stl".to_string(),
            ],
            "model.py first, non-dot outputs sorted after it, dotfiles excluded"
        );
    }

    /// A model.py that does not exist drops out of the set (upstream's
    /// existence filter); with no model and an empty output directory the
    /// export has nothing to zip.
    #[test]
    fn export_files_filters_missing_model_and_survives_empty_output() {
        let root = std::env::temp_dir().join(format!("cad-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("output")).unwrap();
        std::fs::write(root.join("output").join("model.3mf"), b"x").unwrap();
        let names: Vec<_> = export_files(&root)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["model.3mf".to_string()],
            "the absent model.py drops out; the surviving output stays"
        );

        // no model.py anywhere and nothing in output/: the empty package
        // (zip then fails "Nothing to do!" → the failed activity)
        let bare = std::env::temp_dir().join(format!("cad-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&bare).unwrap();
        assert!(
            export_files(&bare).is_empty(),
            "a project with neither model.py nor outputs exports nothing"
        );
    }

    /// The schematic pane's file discovery matches upstream page order:
    /// schematic.svg first, then schematic-<n>.svg by number.
    #[test]
    fn drawing_pages_order_matches_upstream() {
        let dir = std::env::temp_dir().join(format!("cad-pages-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "schematic-10.svg",
            "schematic-2.svg",
            "other.svg",
            "schematic.svg",
            "schematic.png",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let pages = drawing_pages(&dir);
        let names: Vec<_> = pages
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                "schematic.svg".to_string(),
                "schematic-2.svg".to_string(),
                "schematic-10.svg".to_string(),
            ],
            "schematic.svg leads, numbered pages sort numerically, non-pages excluded"
        );
    }

    /// A send the bridge refuses names itself in the window's status line
    /// and keeps the draft in the input bar — a refusal must not vanish
    /// (no row opens, no process spawns, and the status line is the one
    /// surface that says why).
    #[tokio::test]
    async fn refused_send_surfaces_in_the_status_line() {
        let root = std::env::temp_dir().join(format!("cad-refuse-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &Settings::default());
        // the bridge opens unverified: the app-level check has not landed
        assert!(!window.bridge.python_ready());
        assert!(!window.is_busy());

        let _ = window.update(Message::Chat(chat_view::Message::SetDraft(
            "animate the cap".into(),
        )));
        let _ = window.update(Message::Chat(chat_view::Message::Send));

        assert!(
            matches!(window.activity, Activity::Failed(_)),
            "the refusal's reason reaches the status line, got {:?}",
            window.activity
        );
        assert!(
            window.chat.messages.is_empty(),
            "a refused send opens no row"
        );
        assert_eq!(
            window.chat.draft, "animate the cap",
            "the refused text stays in the input bar"
        );
    }

    /// A send the bridge accepts flips the window busy in the SAME update
    /// call — no observable window where the stop button, streaming
    /// indicator or header spinner lag a started send (upstream `send()`
    /// sets .working synchronously, Project.swift:192-208). The rise
    /// crosses through the Send arm's fold of the bridge's own activity —
    /// `enqueue` opened the turn before returning — not through the
    /// driver's later TurnStarted, which this test never folds.
    #[tokio::test]
    async fn successful_send_flips_busy_in_the_same_update() {
        let root = std::env::temp_dir().join(format!("cad-busy-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root, &Settings::default());
        // the readiness the app's startup check owns in production —
        // without it the send refuses (the refusal sibling) and nothing
        // rises
        window.bridge.mark_python_ready_for_test();
        assert!(!window.is_busy());

        let _ = window.update(Message::Chat(chat_view::Message::SetDraft(
            "make the wall 4 mm".into(),
        )));
        let _ = window.update(Message::Chat(chat_view::Message::Send));

        assert!(
            window.is_busy(),
            "a started send is busy in its own update call, got {:?}",
            window.activity
        );
        assert_eq!(
            window.chat.draft, "",
            "the accepted text left the input bar (the Ok branch ran)"
        );
    }

    /// The export outcome lands on the activity model exactly as
    /// upstream's Task does (Project.swift:445-450): a zero zip status →
    /// the succeeded row (the reveal rides the returned task); anything
    /// else → the failed row. A cancelled panel answers
    /// [`Message::ExportChosen`] with `Err` and changes nothing.
    #[tokio::test]
    async fn export_outcome_drives_the_activity_row() {
        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-export-ui-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root, &Settings::default());

        // a cancelled save panel is upstream's `guard … else { return }`
        let _ = window.update(Message::ExportChosen(Err("cancelled".into())));
        assert_eq!(
            window.activity,
            Activity::Idle,
            "a cancel leaves the row idle"
        );

        let destination = std::env::temp_dir().join("cad-export-out.zip");
        let _ = window.update(Message::ExportDone(Some(destination)));
        assert_eq!(
            window.activity,
            Activity::Succeeded("Package exported".into()),
            "a zero zip status is the succeeded row"
        );

        let _ = window.update(Message::ExportDone(None));
        assert_eq!(
            window.activity,
            Activity::Failed("Export failed".into()),
            "any other status (or no zip at all) is the failed row"
        );
    }

    /// The metadata-clobber pin (T7 review F1): the window's save points
    /// (turn end, bookkeeping) must write project.json THROUGH the disk
    /// copy. The engine writes the session id into the same file mid-turn
    /// and `record_version` the version pointer — both BEHIND the window's
    /// back — so a save from the window's open-time copy reverts them. The
    /// restore-then-bookkeeping tail pins the same law after a restore
    /// moves both fields.
    /// The overlay-sync triggers the M6 review owed (carry-forwards M6):
    /// the two terminal bridge events drive `sync_parameters` —
    /// `BuildFinished` because the build refreshed the manifest's parameter
    /// set, `ParameterCommitted` because a commit moved a row's value — so
    /// the overlay's rows follow the bridge without a restore (the only
    /// other sync; open populates nothing).
    #[tokio::test]
    async fn parameter_commit_and_build_finish_sync_the_overlay_rows() {
        let root = std::env::temp_dir().join(format!("cad-parm-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("model.py"), "wall = 3\n").unwrap();
        std::fs::write(
            root.join("output/manifest.json"),
            r#"{"parameters": {"wall": 3}}"#,
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root, &Settings::default());
        assert!(
            window.viewer.parameters.sliders.is_empty(),
            "open syncs nothing; the two events own the overlay's population"
        );

        // the build's drain: the manifest's set is what the panel shows
        let _ = window.update(Message::Bridge(bridge::Message::BuildFinished {
            outcome: engine::python::model::BuildOutcome::Success,
            version_prompt: None,
        }));
        assert_eq!(window.viewer.parameters.sliders.len(), 1);
        assert_eq!(window.viewer.parameters.sliders[0].0, "wall");
        assert_eq!(window.viewer.parameters.sliders[0].1.value, 3.0);

        // a slider commit: the bridge moved the pipeline's row (and rewrote
        // model.py) before broadcasting — the overlay follows the new value
        let _ = window.bridge.set_parameter("wall", 4.0);
        let _ = window.update(Message::Bridge(bridge::Message::ParameterCommitted {
            name: "wall".into(),
            value: 4.0,
        }));
        assert_eq!(
            window.viewer.parameters.sliders[0].1.value, 4.0,
            "the overlay's row follows the committed value"
        );
    }

    #[tokio::test]
    async fn bookkeeping_saves_through_the_bridge_s_metadata() {
        use engine::agent::session::TurnOutcome;
        use engine::project::versions;

        let root = std::env::temp_dir().join(format!("cad-meta-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("model.py"), "wall = 3\n").unwrap();
        let mut history = Vec::new();
        versions::record(
            &root,
            &mut history,
            "a 20 mm cube",
            Some("session-v1"),
            ::time::OffsetDateTime::now_utc(),
        )
        .unwrap();
        std::fs::write(root.join("model.py"), "wall = 5\n").unwrap();
        // v1 is what the window opens on
        let mut metadata = store::load_metadata(&root).unwrap();
        metadata.session_id = Some("session-v1".into());
        metadata.current_version = Some(1);
        store::save_metadata(&root, &metadata).unwrap();

        let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &Settings::default());

        // the driver's second record + the engine's mid-turn session write,
        // landing straight on disk BEHIND the window's back: v2 current, a
        // NEWER session id
        metadata.session_id = Some("engine-session-2".into());
        metadata.current_version = Some(2);
        store::save_metadata(&root, &metadata).unwrap();

        // the bookkeeping arms save — through the fresh copy, never the
        // window's open-time hold (which would still say no session, v1)
        let _ = window.update(Message::Bridge(bridge::Message::Event(
            EngineEvent::TurnFinished(TurnOutcome::Success {
                session_id: Some("engine-session-2".into()),
                text: String::new(),
            }),
        )));
        let _ = window.update(Message::Bridge(bridge::Message::TurnSuccessBookkeeping));
        let saved = store::load_metadata(&root).unwrap();
        assert_eq!(
            saved.current_version,
            Some(2),
            "the driver's version pointer survives the window's save points"
        );
        assert_eq!(
            saved.session_id.as_deref(),
            Some("engine-session-2"),
            "the engine's mid-turn session id survives the window's save points"
        );
        assert_eq!(
            window.metadata.current_version,
            Some(2),
            "the window's copy (the version menu's checkmark source) is in step"
        );

        // the restore-then-bookkeeping tail: restore v1 (disk moves to v1's
        // pointer and session), bookkeep again — both still standing
        let _ = window.update(Message::Restore(1));
        assert_eq!(
            std::fs::read_to_string(root.join("model.py")).unwrap(),
            "wall = 3\n"
        );
        let _ = window.update(Message::Bridge(bridge::Message::TurnSuccessBookkeeping));
        let after = store::load_metadata(&root).unwrap();
        assert_eq!(after.current_version, Some(1));
        assert_eq!(after.session_id.as_deref(), Some("session-v1"));
    }

    /// The compare wiring's window half (upstream `compare(with:)`,
    /// Project.swift:363-368): the picker's report reaches BOTH halves —
    /// the bridge stores the selection (and parses the version's parameter
    /// values) while the pane stages the version's MESH as the ghost from
    /// the version-store directory the history writes. `None` clears both.
    #[tokio::test]
    async fn compare_picker_drives_bridge_and_ghost() {
        use engine::project::versions;

        let root = std::env::temp_dir().join(format!("cad-compare-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("model.py"), "wall = 3\n").unwrap();
        // version 1 snapshots a floating box; the live output then becomes
        // a different (grounded) model, so the ghost is distinguishable
        // from the scene it floats over
        std::fs::write(
            root.join("output/model.stl"),
            crate::viewer_pane::stl_of_box([[0.0, 0.0, 40.0], [10.0, 10.0, 60.0]]),
        )
        .unwrap();
        let mut history = Vec::new();
        versions::record(
            &root,
            &mut history,
            "a floating box",
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        std::fs::write(
            root.join("output/model.stl"),
            crate::viewer_pane::stl_of_box([[0.0, 0.0, 0.0], [20.0, 20.0, 10.0]]),
        )
        .unwrap();

        let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &Settings::default());
        assert!(!window.viewer.comparing);

        let _ = window.update(Message::Viewer(crate::viewer_pane::Message::Parameters(
            parameters_view::Message::Compare(Some(1)),
        )));
        assert_eq!(
            window.bridge.compare_version(),
            Some(1),
            "the bridge holds the selection"
        );
        assert!(window.viewer.comparing);
        let ghost = window.viewer.scene.ghost.as_ref().unwrap();
        // the current scene's floor is model z 0 (the grounded box), so the
        // version's floating box stages at scene y 40..60 — the version's
        // mesh at the CURRENT pin
        assert!(
            (ghost.aabb_min[1] - 40.0).abs() < 1e-3,
            "version 1's mesh at the current pin: {:?}",
            ghost.aabb_min
        );
        assert!((ghost.aabb_max[1] - 60.0).abs() < 1e-3);

        let _ = window.update(Message::Viewer(crate::viewer_pane::Message::Parameters(
            parameters_view::Message::Compare(None),
        )));
        assert_eq!(window.bridge.compare_version(), None);
        assert!(!window.viewer.comparing);
        assert!(window.viewer.scene.ghost.is_none());
    }
}
