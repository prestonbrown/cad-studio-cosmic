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
use cosmic::widget::pane_grid::{self, ResizeEvent};
use cosmic::widget::text_editor;
use cosmic::widget::{Column, Row, Space, button, container, popover, text};
use cosmic::{Element, Task as CosmicTask};
use engine::agent::session::EngineEvent;
use engine::project::folder;
use engine::project::layout::{LayoutNode, PaneKind};
use engine::project::manifest::Manifest;
use engine::project::store::Action;
use engine::project::store::{self, ProjectMetadata};
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
use crate::settings_store::SettingsStore;
use crate::typography;
use crate::ui::surface::{self, Corner, Shadow};
use crate::ui::{self, Backdrop, Symbol, Tone};
use crate::version_menu;
use crate::viewer_pane::ViewerPane;

/// Upstream's default project window size (the WindowGroup
/// `.defaultSize(1320, 840)`) and minimum (ProjectWorkspace's
/// minWidth/minHeight).
pub const DEFAULT_SIZE: (f32, f32) = (1320.0, 840.0);
pub const MIN_SIZE: (f32, f32) = (960.0, 600.0);

/// The pane grid's divider thickness (upstream's 1 pt NSSplitView
/// hairline) — the spacing half of [`panes::clamp_resize`]'s contract.
const GRID_SPACING: f32 = 1.0;

/// The pane divider: a separator wash scaled to #D4D4D5 between #EBEBEC
/// panes (project-model.png).
const DIVIDER_SCALE: f32 = 1.17;

/// The pane grid's minimum pane extent (iced's default, named so the drag
/// hit-test computes the same regions the grid lays out).
const GRID_MIN_PANE: f32 = 50.0;

/// The toolbar band's height above its bottom hairline
/// (project-model.png: the divider at y=104 of the 2x capture).
const HEADER_HEIGHT: f32 = 52.0;

/// The ActivityViewer capsule's `.quaternary.opacity(0.6)` fill, scaled to
/// the measured #DFDFE1 over the toolbar's #EEEEF0.
const CAPSULE_SCALE: f32 = 0.76;

/// The toolbar's bottom hairline: a separator scaled to the measured
/// #C5C5C6 over #EEEEF0.
const HAIRLINE_SCALE: f32 = 2.1;

/// The capsule's `.font(.system(size: 12))` (ActivityViewer.swift:34) on
/// COSMIC's text scale.
const CAPSULE_TEXT: f32 = typography::scaled(12.0);
/// The header's labels and glyphs sit in the titlebar.
const TITLEBAR: Backdrop = Backdrop::Titlebar;

/// The version card's width: upstream's menu runs ~500 px, past the
/// window edge; an in-window popover is clamped to the window (libcosmic
/// popover.rs:375), so the card takes every pixel up to an 8 px margin.
/// The pill's left edge sits 354 px from the window's right edge (the
/// header's end group is fixed-width) and the card opens 115 px left of
/// it: 354 + 115 - 8.
const VERSION_CARD_WIDTH: f32 = 461.0;

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
    /// A chat row's "Configura l'agente…" action — the app layer presents
    /// the agent-missing sheet over whichever window shows (upstream
    /// ChatView.swift:101-102; the sheet, not the settings window).
    PresentAgentSheet,
    /// The shell's output watcher saw a settled change.
    OutputReload(ReloadKind),
    /// The pane grid's laid-out size (the resize clamp's input).
    GridResized(Size),
    /// The 1 s tick while working (elapsed label, focus flash).
    Tick,
    Command(Command),
    /// A pane-grid divider drag (clamped through [`panes::clamp_resize`]).
    Resized(ResizeEvent),
    /// A pane's grab handle was pressed (PaneContainer.swift:30-33): the
    /// drag runs on window-level cursor events until release.
    GripPressed(PaneKind),
    /// The pointer entered (`Some`) or left (`None`) a pane's grab handle.
    GripHovered(Option<PaneKind>),
    /// The cursor moved while a handle is held (window coordinates).
    PaneDragMoved(cosmic::iced::Point),
    /// The mouse button came up while a handle is held: drop.
    PaneDragEnded,
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
    /// An outside click dismissed whichever header card was open.
    CloseCards,
    /// A row of the Open Recent card: open that project (the app layer).
    OpenRecentPath(PathBuf),
    /// The Open Recent card's "Cancella menu" (the app layer clears the
    /// store and empties the card).
    ClearRecents,
    /// The header's close button (the app layer closes the window).
    CloseRequested,
    /// A window-chrome gesture on the header (the app layer owns the
    /// window id the platform call needs).
    Window(WindowOp),
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

/// The header's window gestures: drag-to-move, maximize toggle (button or
/// double-click), minimize, and the right-click window menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOp {
    Drag,
    ToggleMaximize,
    Minimize,
    Menu,
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
    /// Open the settings window (upstream's Settings scene; the port's
    /// placement is the File root — there is no app menu to hold it).
    Settings,
    /// Open the About window (upstream's app-info command group,
    /// Commands.swift:10-14; same placement note as Settings).
    About,
}

/// One open project's window state.
pub struct ProjectWindow {
    pub root: PathBuf,
    pub name: String,
    /// The settings every window read and write goes through (the app's
    /// store; its viewer pane shares it).
    settings: SettingsStore,
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
    /// The Open Recent card's snapshot of the recents list (the app
    /// layer owns the store and fills this when the File item fires).
    pub recent_card: Option<Vec<PathBuf>>,
    /// The focus command's one-second pane highlight.
    focus_flash: Option<PaneKind>,
    /// The pane whose grab handle is held (the drag's input gate).
    grip_held: Option<PaneKind>,
    /// The handle under the pointer (full opacity while hovered).
    grip_hover: Option<PaneKind>,
    /// The live drag once the cursor has moved (upstream `paneDrag`).
    pane_drag: Option<panes::PaneDrag>,
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
    let mut files = vec![folder::model_script(root)];
    if let Ok(entries) = std::fs::read_dir(folder::output_dir(root)) {
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
    pub fn new(root: PathBuf, settings: &SettingsStore) -> (Self, CosmicTask<bridge::Message>) {
        let (bridge, bridge_task) = Bridge::new(settings, root.clone());
        (Self::with_bridge(root, settings, bridge), bridge_task)
    }

    /// [`ProjectWindow::new`] over an already-launched bridge — the
    /// headless harness's seam: a cosmic Task's stream is private to the
    /// iced runtime, so tests launch through [`Bridge::launch`] (whose
    /// receiver they can tap) and fold the messages by hand exactly as the
    /// app maps them; the window then reads the same bridge's pipeline the
    /// live app reads.
    pub fn with_bridge(root: PathBuf, settings: &SettingsStore, bridge: Bridge) -> Self {
        let mut window = Self::assemble(root, settings, bridge);
        // the overlay opens populated (upstream's reload at open reads the
        // manifest's parameters, ProjectOutput.swift:48-51)
        let parameters = window.bridge.parameters();
        window.viewer.sync_parameters(&parameters);
        window
    }

    /// Every setting the window seeds from is read fresh from `store` now,
    /// so a write any window made before this one opened shows here.
    fn assemble(root: PathBuf, store: &SettingsStore, bridge: Bridge) -> Self {
        let settings = &store.load();
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let _ = store::prepare(&root);
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Project".to_string());
        let metadata = store::load_metadata_or_new(&root);
        let layout = metadata
            .layout
            .clone()
            .unwrap_or_else(|| panes::stored_layout(settings));
        let chat_visible = true;
        let grid = panes::from_layout(&panes::displayed(&layout, chat_visible));

        let output_dir = folder::output_dir(&root);
        let manifest = Manifest::load(&output_dir.join(folder::MANIFEST)).ok();
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

        let mut window = ProjectWindow {
            name,
            settings: store.clone(),
            metadata,
            layout,
            chat_visible,
            grid,
            size: DEFAULT_SIZE,
            chat: {
                // seed the engine from the settings snapshot so the empty
                // transcript's placeholder names the engine that will run
                // the first turn; each TurnStarted re-seeds per turn
                let mut chat = ChatState::new(store::load_chat(&root).unwrap_or_default());
                chat.engine = settings.agent_kind();
                chat
            },
            chat_pane: ChatPane::new(),
            viewer: {
                let mut pane = ViewerPane::new_with_settings(
                    &root,
                    settings.measurement_unit,
                    settings.printer_model.clone(),
                    store.clone(),
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
            recent_card: None,
            focus_flash: None,
            grip_held: None,
            grip_hover: None,
            pane_drag: None,
            last_size: Arc::new(Mutex::new(Size::default())),
            watch,
            events,
            relay: Arc::new(OutputRelay {
                id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                events: Mutex::new(Some(receiver)),
            }),
            root,
        };
        window.sync_version_markers();
        window
    }

    /// The chat's version markers read the history and the current
    /// version from the chat state's mirror (VersionMarker's
    /// `project.history` / `metadata.currentVersion`): refreshed at open,
    /// after a build records a version, and after a restore.
    fn sync_version_markers(&mut self) {
        self.chat.versions = self.bridge.versions().iter().map(|v| v.number).collect();
        self.chat.current_version = self.metadata.current_version;
    }

    /// Whether a turn is running (upstream `isBusy`).
    pub fn is_busy(&self) -> bool {
        self.activity.is_working()
    }

    fn output_dir(&self) -> PathBuf {
        folder::output_dir(&self.root)
    }

    /// An established save point (upstream `save()`, Project.swift:429-433):
    /// metadata with the current layout, timestamps bumped — written
    /// THROUGH the disk copy
    /// ([`store::load_metadata_or_new`]): the engine writes the session id into
    /// project.json mid-turn and `record_version` the version pointer, both
    /// behind the window's back, so those fields are re-read here, never
    /// held from open.
    fn save_metadata(&mut self) {
        let mut metadata = store::load_metadata_or_new(&self.root);
        metadata.layout = Some(self.layout.clone());
        metadata.updated_at = OffsetDateTime::now_utc();
        metadata.app_version = store::APP_VERSION.to_string();
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

    /// Apply a structure change (preset switch, pane move): the grid
    /// rebuilds and both the project manifest and the global default
    /// layout persist eagerly (upstream `setLayout` → `persistLayout`).
    fn apply_layout_persist(&mut self, node: LayoutNode) {
        self.layout = node.clone();
        self.chat_visible = true;
        self.grid = panes::from_layout(&panes::displayed(&node, true));
        self.save_metadata();
        let _ = panes::persist_layout(&self.root, &self.settings, &node);
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
                let version = number.map(|n| (n, folder::version_dir(&self.root, n)));
                self.viewer.set_compare(version);
                Task::none()
            }
            // the slider/field commit the pane emits (the pane's local
            // half — value snap, draft clear — already ran): the bridge
            // rewrites model.py and starts the build, and the working
            // activity crosses to the window the moment it does
            Message::Viewer(crate::viewer_pane::Message::ParameterCommit(name, value)) => {
                let _ = self.bridge.set_parameter(&name, value);
                self.set_activity(self.bridge.activity());
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
                    self.set_activity(self.bridge.activity());
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
                let _ = self
                    .settings
                    .update(|settings| settings.parameters_collapsed = collapsed);
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
                            Manifest::load(&self.output_dir().join(folder::MANIFEST)).ok();
                        self.schematic.reload(&files, manifest.as_ref());
                    }
                    // the 3D pane's own watch rebuilds the scene; the shell
                    // refreshes the activity model's warnings
                    ReloadKind::Mesh | ReloadKind::Plate => {
                        self.refresh_missing_outputs();
                    }
                    // a manifest change re-reads the parameters, whoever
                    // wrote it (ProjectOutput.swift:48-51)
                    ReloadKind::Manifest => {
                        self.refresh_missing_outputs();
                        let parameters = bridge::refresh_parameters(&self.root);
                        self.viewer.sync_parameters(&parameters);
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
            Message::GripPressed(kind) => {
                self.grip_held = Some(kind);
                Task::none()
            }
            Message::GripHovered(kind) => {
                self.grip_hover = kind;
                Task::none()
            }
            Message::PaneDragMoved(position) => {
                if let Some(pane) = self.grip_held {
                    // the grid sits under the header band and its hairline
                    let point =
                        cosmic::iced::Point::new(position.x, position.y - HEADER_HEIGHT - 1.0);
                    let regions = self.grid.layout().pane_regions(
                        GRID_SPACING,
                        GRID_MIN_PANE,
                        Size::new(self.size.0, self.size.1),
                    );
                    let hit = panes::locate(
                        regions
                            .into_iter()
                            .filter_map(|(id, bounds)| Some((*self.grid.panes.get(&id)?, bounds))),
                        point,
                    );
                    self.pane_drag = Some(panes::drag_to(pane, hit));
                }
                Task::none()
            }
            Message::PaneDragEnded => {
                self.grip_held = None;
                if let Some(drag) = self.pane_drag.take() {
                    if let Some(node) = panes::drop(&self.layout, &drag) {
                        self.apply_layout_persist(node);
                    }
                }
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
            Message::CloseCards => {
                self.version_card = None;
                self.shown_issues = None;
                self.recent_card = None;
                Task::none()
            }
            Message::CloseRequested
            | Message::Window(_)
            | Message::OpenRecentPath(_)
            | Message::ClearRecents
            | Message::App(_)
            | Message::PresentSetupSheet
            | Message::PresentAgentSheet => Task::none(),
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
        if self.bridge.restore(number).is_err() {
            // the busy refusal changed nothing
            return Task::none();
        }
        // "Versione N ripristinata" / "Impossibile ripristinare la versione
        // N" either way (Project.swift:319-321)
        self.set_activity(self.bridge.activity());
        if self.bridge.build_error().is_some() {
            return Task::none();
        }
        // the layout stays the window's: only the fields the bridge's save
        // owns cross over
        let fresh = store::load_metadata_or_new(&self.root);
        self.metadata.session_id = fresh.session_id;
        self.metadata.current_version = fresh.current_version;
        let engine = self.chat.engine;
        self.chat = ChatState::new(store::load_chat(&self.root).unwrap_or_default());
        self.chat.engine = engine;
        self.sync_version_markers();
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
                    self.set_activity(self.bridge.activity());
                }
            }
            chat_view::Message::SendSuggestion(suggestion) => {
                let _ = self.bridge.enqueue(suggestion.clone(), Vec::new());
                self.set_activity(self.bridge.activity());
            }
            chat_view::Message::Stop => self.bridge.cancel(),
            // a marker row's "Ripristina" (VersionMarker.swift)
            chat_view::Message::RestoreVersion(number) => return self.restore(*number),
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
                self.set_activity(self.bridge.activity());
            }
            // the "Configura l'agente…" row action presents the
            // AGENT-MISSING SHEET — upstream ChatView.swift:101-102 sets
            // `AgentLocator.current.isMissingSheetPresented = true`; there
            // is no settings route off the action or off that sheet
            // (Ruling 6). The app layer owns the sheet.
            chat_view::Message::RowAction(_, Action::ConfigureClaude) => {
                return CosmicTask::done(Message::PresentAgentSheet);
            }
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
                engine,
            } => {
                // the turn's engine (the engine's own pick) names its rows
                // from here on
                self.chat.engine = engine;
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
                // the bridge classifies every turn transition (it holds the
                // turn's engine); the window mirrors it
                self.set_activity(self.bridge.activity());
                ChatPane::scroll_to_bottom().map(Message::Chat)
            }
            bridge::Message::Event(event) => {
                // the chat reducer owns the rows; the activity mirrors after
                self.chat.apply(&event);
                match &event {
                    EngineEvent::ToolStarted { .. } => self.set_activity(self.bridge.activity()),
                    EngineEvent::TurnFinished(_) => {
                        self.refresh_missing_outputs();
                        self.set_activity(self.bridge.activity());
                        // a turn end is an established layout save point
                        self.save_metadata();
                    }
                    _ => {}
                }
                ChatPane::scroll_to_bottom().map(Message::Chat)
            }
            bridge::Message::Closed => {
                self.set_activity(self.bridge.activity());
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
                let fresh = store::load_metadata_or_new(&self.root);
                self.metadata.session_id = fresh.session_id;
                self.metadata.current_version = fresh.current_version;
                self.sync_version_markers();
                let parameters = self.bridge.parameters();
                self.viewer.sync_parameters(&parameters);
                self.set_activity(self.bridge.activity());
                ChatPane::scroll_to_bottom().map(Message::Chat)
            }
            bridge::Message::TurnSuccessBookkeeping => {
                self.refresh_missing_outputs();
                self.save_metadata();
                self.reload_chat_rows();
                // the turn's tail re-read the manifest (bridge
                // turn_success_tail): an agent-made parameter set shows
                // now, not at the next Compila
                let parameters = self.bridge.parameters();
                self.viewer.sync_parameters(&parameters);
                self.set_activity(self.bridge.activity());
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
                self.set_activity(self.bridge.activity());
                Task::none()
            }
            Command::Export => {
                // The menu item this shortcut rides is disabled without
                // model.3mf (Commands.swift:131's hasThreeMF gate) and NOT
                // while busy — upstream's menu/toolbar split, copied as-is.
                // The keybind is the menu item's, so the arm repeats only
                // its gate.
                if !self.output_dir().join(folder::MODEL_3MF).exists() {
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
                let plate = output.join(folder::PLATE_3MF);
                let target = if plate.exists() {
                    plate
                } else {
                    output.join(folder::MODEL_3MF)
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
        if self.grip_held.is_some() {
            subs.push(cosmic::iced::event::listen_with(
                |event, _status, _window| {
                    use cosmic::iced::{Event, mouse};
                    match event {
                        Event::Mouse(mouse::Event::CursorMoved { position }) => {
                            Some(Message::PaneDragMoved(position))
                        }
                        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                            Some(Message::PaneDragEnded)
                        }
                        _ => None,
                    }
                },
            ));
        }
        Subscription::batch(subs)
    }

    /// The whole window: header over the pane grid (upstream
    /// ProjectSplitView), with the open issue list as a popover.
    pub fn view(&self) -> Element<'_, Message> {
        self.view_with_focus(true)
    }

    /// The window frame; `focused` styles the header's window controls.
    pub fn view_with_focus(&self, focused: bool) -> Element<'_, Message> {
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
            .push(self.view_header(focused))
            .push(sized)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();

        // the version card, the issue list, and the Open Recent card are
        // popovers anchored to their own header controls (view_header)
        base
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
            let mut layers = cosmic::iced::widget::Stack::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .push(body);
            if let Some(highlight) = self.view_drop_highlight(*kind) {
                layers = layers.push(highlight);
            }
            let framed: Element<'_, Message> = container(layers.push(self.view_grip(*kind)))
                .width(Length::Fill)
                .height(Length::Fill)
                .class(flash_border(flashing))
                .into();
            pane_grid::Content::new(framed)
        })
        .spacing(GRID_SPACING)
        .min_size(GRID_MIN_PANE)
        .on_resize(6, Message::Resized);
        // the 1 px gaps between opaque panes show this fill: the
        // NSSplitView hairline, #D4D4D5 between #EBEBEC panes
        container(grid)
            .width(Length::Fill)
            .height(Length::Fill)
            .class(surface::fill(
                Tone::Separator,
                DIVIDER_SCALE,
                Corner::None,
                false,
            ))
            .into()
    }

    /// A pane's grab handle (PaneContainer.swift:19-37): `line.3.horizontal`
    /// at 10 pt semibold, secondary, in a 28×16 material capsule with a
    /// separator stroke, 6 px from the pane's top-left; 45 % opacity until
    /// hovered or dragging.
    fn view_grip(&self, kind: PaneKind) -> Element<'static, Message> {
        let lit =
            self.grip_hover == Some(kind) || self.pane_drag.is_some_and(|drag| drag.pane == kind);
        let opacity = if lit { 1.0 } else { 0.45 };
        let pill = container(
            ui::icon::icon(
                Symbol::Line3Horizontal,
                7.0,
                Tone::Secondary,
                Backdrop::Material,
            )
            .opacity(opacity * Tone::Secondary.alpha(&cosmic::theme::active(), Backdrop::Material)),
        )
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(16.0))
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .class(grip_class(opacity));
        let grip = cosmic::widget::mouse_area(pill)
            .on_press(Message::GripPressed(kind))
            .on_enter(Message::GripHovered(Some(kind)))
            .on_exit(Message::GripHovered(None))
            .interaction(cosmic::iced::mouse::Interaction::Grab);
        ui::button::with_help(
            container(grip).padding(6),
            fl!("trascina-per-spostare-il-pannello"),
            cosmic::widget::tooltip::Position::Bottom,
        )
    }

    /// The drop highlight over a pane while a drag is live
    /// (PaneContainer.swift:40-62): the target's edge half, inset 4, an
    /// 8-radius accent fill at 18 % with a 2 px accent stroke; the dragged
    /// pane itself takes a 6 % accent wash.
    fn view_drop_highlight(&self, kind: PaneKind) -> Option<Element<'static, Message>> {
        let drag = self.pane_drag?;
        let tint = |alpha: f32, stroke: bool| {
            cosmic::theme::Container::custom(move |theme| {
                let accent = Color::from(theme.cosmic().accent_color());
                cosmic::iced::widget::container::Style {
                    background: Some(cosmic::iced::Background::Color(Color {
                        a: alpha,
                        ..accent
                    })),
                    border: cosmic::iced::Border {
                        color: accent,
                        width: if stroke { 2.0 } else { 0.0 },
                        radius: if stroke { Corner::S } else { Corner::None }.radius(theme),
                    },
                    ..Default::default()
                }
            })
        };
        let fill = || Space::new().width(Length::Fill).height(Length::Fill);
        match (drag.target, drag.edge) {
            (Some(target), Some(edge)) if target == kind => {
                let mark: Element<'static, Message> =
                    container(fill()).class(tint(0.18, true)).into();
                // the edge half inset 4 on every side: padding 4, an 8 px
                // gutter between the halves
                let halves: Element<'static, Message> = match edge {
                    panes::DropEdge::Leading => {
                        Row::new().spacing(8).push(mark).push(fill()).into()
                    }
                    panes::DropEdge::Trailing => {
                        Row::new().spacing(8).push(fill()).push(mark).into()
                    }
                    panes::DropEdge::Top => Column::new().spacing(8).push(mark).push(fill()).into(),
                    panes::DropEdge::Bottom => {
                        Column::new().spacing(8).push(fill()).push(mark).into()
                    }
                };
                Some(container(halves).padding(4).into())
            }
            _ if drag.pane == kind => Some(container(fill()).class(tint(0.06, false)).into()),
            _ => None,
        }
    }

    /// The toolbar (ProjectWindow.swift:94-161 `ProjectToolbar`) as ONE
    /// header row: the command menu bar first (upstream's AppCommands live
    /// in the macOS global menu bar; COSMIC has none, so the menus ride the
    /// header start like cosmic-edit's), the navigation group (chat toggle,
    /// Compila, Stop), the principal ActivityViewer capsule centered on the
    /// window, the primary-action group (versions, reveal, export, slicer,
    /// parameters), then the window controls. 52 px tall to the hairline:
    /// the upstream capture's toolbar divider sits at y=104 of 1680
    /// (project-model.png, 2x).
    fn view_header(&self, focused: bool) -> Element<'_, Message> {
        let busy = self.is_busy();
        // upstream gates export and slicer on output.hasThreeMF
        let has_3mf = self.output_dir().join(folder::MODEL_3MF).exists();

        // icon sizes and button widths: the upstream capture's glyph bboxes
        // and center-to-center pitch (sidebar→play 38, play→stop 34, the
        // primary group 38), halved to the 1x basis
        let start = Row::new()
            .align_y(cosmic::iced::Alignment::Center)
            .push({
                // "Apri recenti" (Commands.swift:30-40) drops its card from
                // the menu bar
                let mut menu = popover(self.view_menu())
                    .position(cosmic::widget::popover::Position::Bottom)
                    .on_close(Message::CloseCards);
                if let Some(recents) = &self.recent_card {
                    menu = menu.popup(view_recents(recents));
                }
                menu
            })
            .push(Space::new().width(Length::Fixed(8.0)))
            .push(toolbar_button(
                Symbol::SidebarLeft,
                15.0,
                40.0,
                Some(Message::Command(Command::ShowChat)),
                fl!("mostra-o-nascondi-la-chat-0"),
            ))
            // "Compila modello" (ProjectWindow.swift:107-113), disabled
            // while busy
            .push(toolbar_button(
                Symbol::PlayFill,
                14.0,
                36.0,
                (!busy).then_some(Message::Command(Command::Build)),
                fl!("compila-modello-b"),
            ))
            // Stop (:115-121), disabled when idle
            .push(toolbar_button(
                Symbol::StopFill,
                13.5,
                32.0,
                busy.then_some(Message::Command(Command::Stop)),
                fl!("interrompi"),
            ));

        let end = Row::new()
            .align_y(cosmic::iced::Alignment::Center)
            .push({
                let mut versions = popover(crate::version_menu::version_button(
                    self.metadata.current_version,
                    self.version_card.is_some(),
                    Message::ToggleVersionCard,
                ))
                .position(cosmic::widget::popover::Position::Point(
                    cosmic::iced::Point::new(
                        version_menu::CARD_OFFSET.0,
                        version_menu::CARD_OFFSET.1,
                    ),
                ))
                .on_close(Message::CloseCards);
                if let Some(now) = self.version_card {
                    versions = versions.popup(version_menu::view_versions(
                        &version_menu::menu_entries(
                            &self.bridge.versions(),
                            self.metadata.current_version,
                        ),
                        busy,
                        now,
                        VERSION_CARD_WIDTH,
                        Message::Restore,
                    ));
                }
                versions
            })
            .push(Space::new().width(Length::Fixed(5.0)))
            .push(toolbar_button(
                Symbol::Folder,
                15.0,
                38.0,
                Some(Message::Command(Command::Reveal)),
                fl!("mostra-nel-finder"),
            ))
            // the toolbar's STRICTER export gate (!hasThreeMF || isBusy;
            // the menu entry stays clickable while busy — upstream's split)
            .push(toolbar_button(
                Symbol::SquareAndArrowUp,
                18.5,
                38.0,
                (has_3mf && !busy).then_some(Message::Command(Command::Export)),
                fl!("esporta-step-3mf-stl-tavola-pdf-e-model-py-in-un-archivio-zip-e"),
            ))
            .push(toolbar_button(
                Symbol::ArrowUpForwardApp,
                15.0,
                38.0,
                has_3mf.then_some(Message::Command(Command::OpenInSlicer)),
                fl!("apri-il-file-3mf-nello-slicer-predefinito"),
            ))
            // "Parametri" (:154-159)
            .push(toolbar_button(
                Symbol::SliderHorizontal3,
                14.5,
                38.0,
                Some(Message::Command(Command::ToggleParameters)),
                fl!("mostra-o-nascondi-i-parametri-i"),
            ));

        let bar: Element<'_, Message> = cosmic::widget::header_bar()
            .focused(focused)
            .on_close(Message::CloseRequested)
            .on_minimize(Message::Window(WindowOp::Minimize))
            .on_maximize(Message::Window(WindowOp::ToggleMaximize))
            .on_drag(Message::Window(WindowOp::Drag))
            .on_double_click(Message::Window(WindowOp::ToggleMaximize))
            .on_right_click(Message::Window(WindowOp::Menu))
            .start(start)
            .center(self.view_activity())
            .end(end)
            .into();
        // the bar's own height is libcosmic's fixed 47; the strip it is
        // centered in carries the same drag/maximize/menu wiring so the
        // whole 52 px band moves the window
        let band = cosmic::widget::mouse_area(
            container(bar)
                .width(Length::Fill)
                .height(Length::Fixed(HEADER_HEIGHT))
                .align_y(cosmic::iced::alignment::Vertical::Center),
        )
        .on_drag(Message::Window(WindowOp::Drag))
        .on_double_press(Message::Window(WindowOp::ToggleMaximize))
        .on_right_press(Message::Window(WindowOp::Menu));
        Column::with_capacity(2)
            .push(band)
            .push(
                container(Space::new().width(Length::Fill).height(Length::Fixed(1.0))).class(
                    surface::fill(Tone::Separator, HAIRLINE_SCALE, Corner::None, false),
                ),
            )
            .into()
    }

    /// The ActivityViewer capsule (Shared/ActivityViewer.swift): cube,
    /// project name (medium), compact chevron, status icon + text, elapsed
    /// timer, then the right-aligned error/warning counts — 12 pt, 10 pt
    /// side padding, 24 tall, 480 wide shrinking toward the 320 minimum as
    /// the header narrows, a `.quaternary` 6-radius fill (:7-37).
    fn view_activity(&self) -> Element<'_, Message> {
        let medium = crate::typography::font(cosmic::iced::font::Weight::Medium);
        let mut row = Row::new()
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center)
            .push(ui::icon::icon(
                Symbol::Cube,
                13.0,
                Tone::Secondary,
                TITLEBAR,
            ))
            .push(
                text(self.name.clone())
                    .size(CAPSULE_TEXT)
                    .font(medium)
                    .wrapping(cosmic::iced::widget::text::Wrapping::None)
                    .class(Tone::Primary.text(TITLEBAR)),
            )
            .push(ui::icon::icon(
                Symbol::ChevronCompactRight,
                8.5,
                Tone::Tertiary,
                TITLEBAR,
            ));
        // statusIcon (:41-55): mini spinner / green check / red octagon
        row = match &self.activity {
            Activity::Idle => row,
            // .controlSize(.mini) — the ~12 px spinner
            Activity::Working(_) => {
                row.push(cosmic::widget::progress_bar::indeterminate_circular().size(12.0))
            }
            Activity::Succeeded(_) => row.push(ui::icon::multicolor(
                Symbol::CheckmarkCircleFillMulticolor,
                13.0,
            )),
            Activity::Failed(_) => row.push(ui::icon::multicolor(
                Symbol::XmarkOctagonFillMulticolor,
                13.0,
            )),
        };
        row = row.push(
            text(self.activity.text())
                .size(CAPSULE_TEXT)
                .wrapping(cosmic::iced::widget::text::Wrapping::None)
                .class(Tone::Secondary.text(TITLEBAR)),
        );
        if let Some(start) = self.work_started_at {
            row = row.push(
                text(activity::elapsed_text(start.elapsed().as_secs()))
                    .size(CAPSULE_TEXT)
                    .font(cosmic::font::mono())
                    .class(Tone::Tertiary.text(TITLEBAR)),
            );
        }
        row = row.push(Space::new().width(Length::Fill));

        let issues = activity::issues(
            self.bridge.build_error().as_deref(),
            &self.missing_outputs,
            self.viewer.manifest_warnings(),
            self.viewer.motion_error(),
            self.viewer.collision_frames(),
        );
        for kind in [IssueKind::Error, IssueKind::Warning] {
            let count = issues.iter().filter(|i| i.kind == kind).count();
            if count == 0 {
                continue;
            }
            let open = self.shown_issues == Some(kind);
            // `.buttonStyle(.plain)` (:79): glyph + count, spacing 2
            let face = button::custom(
                Row::new()
                    .spacing(2)
                    .align_y(cosmic::iced::Alignment::Center)
                    .push(match kind {
                        IssueKind::Error => {
                            ui::icon::multicolor(Symbol::XmarkOctagonFillMulticolor, 12.5)
                        }
                        IssueKind::Warning => ui::icon::multicolor(
                            Symbol::ExclamationmarkTriangleFillMulticolor,
                            11.5,
                        ),
                    })
                    .push(
                        text(format!("{count}"))
                            .size(CAPSULE_TEXT)
                            .class(Tone::Primary.text(TITLEBAR)),
                    ),
            )
            .padding(0)
            .class(ui::button::hover_wash(false, Corner::Xs))
            .on_press(Message::ToggleIssues((!open).then_some(kind)));
            // the `.popover` on the issue button (:80-85)
            let mut anchored = popover(face)
                .position(cosmic::widget::popover::Position::Bottom)
                .on_close(Message::CloseCards);
            if open {
                anchored = anchored.popup(self.view_issue_list(kind));
            }
            row = row.push(anchored);
        }
        container(row)
            .padding([0, 10])
            .width(Length::Fixed(480.0))
            .height(Length::Fixed(24.0))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .class(surface::fill(
                Tone::Quaternary,
                CAPSULE_SCALE,
                Corner::S,
                false,
            ))
            .into()
    }

    /// The issue list popover (upstream `IssueList`).
    fn view_issue_list(&self, kind: IssueKind) -> Element<'_, Message> {
        let issues = activity::issues(
            self.bridge.build_error().as_deref(),
            &self.missing_outputs,
            self.viewer.manifest_warnings(),
            self.viewer.motion_error(),
            self.viewer.collision_frames(),
        );
        // VStack(spacing: 8) of Labels, .callout, padding 12, width 360
        // (ActivityViewer.swift:104-124); the popover closes on an outside
        // click like upstream's
        let mut list = Column::new()
            .spacing(8)
            .padding(12)
            .width(Length::Fixed(360.0));
        for issue in issues.iter().filter(|i| i.kind == kind) {
            list = list.push(
                Row::new()
                    .spacing(6)
                    .push(match kind {
                        IssueKind::Error => {
                            ui::icon::multicolor(Symbol::XmarkOctagonFillMulticolor, 13.0)
                        }
                        IssueKind::Warning => ui::icon::multicolor(
                            Symbol::ExclamationmarkTriangleFillMulticolor,
                            12.5,
                        ),
                    })
                    .push(text(issue.message.clone()).size(typography::CALLOUT)),
            );
        }
        container(list)
            .class(surface::card(Corner::M, Shadow::Popover))
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
            Item::Divider,
            Item::Button(fl!("settings"), None, AppRequest::Settings),
            Item::Button(fl!("informazioni-su-cad-studio"), None, AppRequest::About),
        ];
        let file_binds = std::collections::HashMap::new();
        let file_tree = cosmic::widget::menu::Tree::with_children(
            {
                let root_button: Element<'_, Message> = root(fl!("menu-file")).into();
                root_button
            },
            items(&file_binds, file_entries),
        );

        let model_binds = std::collections::HashMap::new();
        let model_tree = cosmic::widget::menu::Tree::with_children(
            {
                let root_button: Element<'_, Message> = root(fl!("modello")).into();
                root_button
            },
            items(&model_binds, self.model_entries()),
        );

        let entries = self.view_entries();
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
            .item_width(ItemWidth::Static(220))
            .item_height(ItemHeight::Dynamic(30))
            .into()
    }

    /// The Modello root's item list (Commands.swift:113-138): Compila,
    /// Stop, Esporta, Mostra nel Finder, Apri in slicer — each gate a
    /// Button↔ButtonDisabled swap, so the item SET never changes (the
    /// dynamic-items corruption family stays out of reach).
    fn model_entries(&self) -> Vec<cosmic::widget::menu::Item<Command, String>> {
        use cosmic::widget::menu::Item;
        let gate = |enabled: bool, label: String, command: Command| {
            if enabled {
                Item::Button(label, None, command)
            } else {
                Item::ButtonDisabled(label, None, command)
            }
        };
        let busy = self.is_busy();
        // the menu gates export and slicer on hasThreeMF only — NOT on busy
        // (upstream's menu/toolbar split)
        let has_3mf = self.output_dir().join(folder::MODEL_3MF).exists();
        vec![
            gate(!busy, fl!("compila-modello"), Command::Build),
            gate(busy, fl!("stop"), Command::Stop),
            Item::Divider,
            gate(has_3mf, fl!("esporta-pacchetto-2"), Command::Export),
            Item::Button(fl!("mostra-nel-finder"), None, Command::Reveal),
            gate(has_3mf, fl!("apri-in-slicer"), Command::OpenInSlicer),
        ]
    }

    /// The View root's item list (Commands.swift:55-105 order). Built from
    /// window state alone so the construction test can pin the entry set
    /// without mounting the menu bar.
    fn view_entries(&self) -> Vec<cosmic::widget::menu::Item<Command, String>> {
        use cosmic::widget::menu::Item;

        // Button↔ButtonDisabled keeps every root's item SET and labels
        // fixed (the Compila entry's busy gate is the running precedent),
        // outside the dynamic-items corruption family
        let gate = |enabled: bool, label: String, command: Command| {
            if enabled {
                Item::Button(label, None, command)
            } else {
                Item::ButtonDisabled(label, None, command)
            }
        };
        let output = self.output_dir();
        // upstream's mesh loads from model.3mf, else model.stl
        // (ProjectOutput.swift:138-141); hasSVG reads schematic.svg
        let has_mesh =
            output.join(folder::MODEL_3MF).exists() || output.join(folder::MODEL_STL).exists();
        let has_svg = output.join(folder::SCHEMATIC_SVG).exists();

        vec![
            // STATIC label: upstream flips "Mostra/Nascondi chat"
            // (Commands.swift:52), a text-only change inside the menu-overlay
            // corruption family; the label is upstream's own toolbar phrasing
            // (ProjectWindow.swift:103), like the parameters entry below
            Item::Button(fl!("mostra-o-nascondi-la-chat"), None, Command::ShowChat),
            Item::Divider,
            Item::Button(fl!("vai-alla-chat"), None, Command::FocusChat),
            Item::Button(fl!("vai-al-viewer-3d"), None, Command::FocusModel),
            Item::Button(fl!("vai-alla-schematica-2d"), None, Command::FocusSchematic),
            // ⌥⌘I (Commands.swift:69-73), directly after the focus trio as
            // upstream orders it. STATIC label — upstream flips
            // "Mostra/Nascondi parametri" with areParametersVisible, but a
            // text-only flip is inside the menu-overlay corruption family
            // the static-roots law rules out (Ruling 9's safe branch; the
            // divergence is carried in docs/research/carry-forwards.md).
            // The label is upstream's own tooltip phrasing
            // (ProjectWindow.swift:159), shortcut hint included.
            Item::Button(
                fl!("mostra-o-nascondi-i-parametri-i"),
                None,
                Command::ToggleParameters,
            ),
            // "Scrivi un messaggio" ⌘L (Commands.swift:74-76): focus(.chat)
            Item::Button(fl!("scrivi-un-messaggio"), None, Command::FocusChat),
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
            // `.disabled(project?.output.mesh == nil)` (Commands.swift:101):
            // a folder cannot be disabled, so its leaves are
            Item::Folder(
                fl!("viewer-3d"),
                vec![
                    gate(
                        has_mesh,
                        fl!("vista-isometrica"),
                        Command::ViewerPreset(Preset::Iso),
                    ),
                    gate(
                        has_mesh,
                        fl!("vista-frontale"),
                        Command::ViewerPreset(Preset::Front),
                    ),
                    gate(
                        has_mesh,
                        fl!("vista-dall-alto"),
                        Command::ViewerPreset(Preset::Top),
                    ),
                    gate(
                        has_mesh,
                        fl!("vista-da-destra"),
                        Command::ViewerPreset(Preset::Right),
                    ),
                    Item::Divider,
                    gate(has_mesh, fl!("adatta-alla-vista"), Command::ViewerFit),
                ],
            ),
            // `.disabled(project?.output.hasSVG != true)` (:110)
            Item::Folder(
                fl!("schematica-2d"),
                vec![
                    gate(has_svg, fl!("ingrandisci"), Command::SchematicZoomIn),
                    gate(has_svg, fl!("riduci"), Command::SchematicZoomOut),
                    Item::Divider,
                    gate(has_svg, fl!("adatta-alla-finestra"), Command::SchematicFit),
                ],
            ),
        ]
    }
}

/// The focus flash's pane border.
fn flash_border(flashing: bool) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        // opaque, so only the 1 px grid gaps show the divider fill
        let mut style = cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(
                theme.cosmic().background(false).base.into(),
            )),
            ..Default::default()
        };
        if flashing {
            style.border = cosmic::iced::Border {
                color: Color::from(theme.cosmic().accent_color()),
                width: 2.0,
                radius: Corner::S.radius(theme),
            };
        }
        style
    })
}

/// The grab handle's capsule: `.regularMaterial` over a `.separator`
/// stroke, the whole handle at `opacity`.
fn grip_class(opacity: f32) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let base = Color::from(theme.cosmic().background(false).base);
        let mut stroke = ui::tone::wash(theme, 0.15);
        stroke.a *= opacity;
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(Color {
                a: 0.85 * opacity,
                ..base
            })),
            border: cosmic::iced::Border {
                color: stroke,
                width: 1.0,
                radius: Corner::Capsule.radius(theme),
            },
            ..Default::default()
        }
    })
}

/// The "Apri recenti" card (Commands.swift:30-40 — a submenu upstream; a
/// card here because the list changes between rebuilds, which a cosmic menu
/// cannot survive: docs/research/libcosmic-menu-dynamic-items-bug.md): one
/// row per recent project by folder name, a divider, then "Cancella menu",
/// disabled when the list is empty.
fn view_recents(paths: &[PathBuf]) -> Element<'static, Message> {
    let row = |label: String, tone: Tone, on_press: Option<Message>| -> Element<'static, Message> {
        container(
            button::custom(
                container(
                    text(label)
                        .size(typography::BODY)
                        .wrapping(cosmic::iced::widget::text::Wrapping::None)
                        .class(tone.text(Backdrop::Material)),
                )
                .padding([0.0, 9.5])
                .height(Length::Fixed(22.0))
                .align_y(cosmic::iced::alignment::Vertical::Center),
            )
            .padding(0)
            .width(Length::Fill)
            .class(ui::button::hover_wash(false, Corner::Xs))
            .on_press_maybe(on_press),
        )
        .padding([0, 5])
        .into()
    };
    let mut list = Column::new().padding([5, 0]).width(Length::Fixed(240.0));
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        list = list.push(row(
            name,
            Tone::Primary,
            Some(Message::OpenRecentPath(path.clone())),
        ));
    }
    if !paths.is_empty() {
        list = list.push(
            container(
                container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
                    .class(surface::fill(Tone::Separator, 1.0, Corner::None, false)),
            )
            .padding([5, 14]),
        );
    }
    let clear_tone = if paths.is_empty() {
        Tone::Tertiary
    } else {
        Tone::Primary
    };
    list = list.push(row(
        fl!("cancella-menu"),
        clear_tone,
        (!paths.is_empty()).then_some(Message::ClearRecents),
    ));
    container(list)
        .class(surface::card(Corner::M, Shadow::Popover))
        .into()
}

/// One toolbar icon button: a tinted glyph `glyph_height` px tall (its
/// width from the symbol's view box) centered in a `width`×28 hit area
/// (disabled glyphs dim), with its upstream `.help` as tooltip and
/// accessible name.
fn toolbar_button(
    symbol: Symbol,
    glyph_height: f32,
    width: f32,
    on_press: Option<Message>,
    help: String,
) -> Element<'static, Message> {
    let tone = ui::button::content_tone(Tone::Secondary, &on_press);
    let face = container(ui::icon::icon(symbol, glyph_height, tone, TITLEBAR))
        .width(Length::Fixed(width))
        .height(Length::Fixed(28.0))
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    ui::button::with_help(
        button::custom(face)
            .padding(0)
            .class(ui::button::hover_wash(false, Corner::S))
            .on_press_maybe(on_press)
            .name(help.clone()),
        help,
        cosmic::widget::tooltip::Position::Bottom,
    )
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

/// The project window's shortcut table (upstream keyboardShortcut values, ⌘→Ctrl,
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
        // ⌥⌘I parameters (Commands.swift:69-73): the app-wide arm
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

    /// The View root carries the parameters toggle at upstream's position
    /// (Commands.swift:69-73, directly after the focus trio) under ONE
    /// unchanging label — upstream flips "Mostra/Nascondi parametri" with
    /// areParametersVisible; the port pins upstream's own tooltip phrasing
    /// (Ruling 9's safe branch) because label churn between rebuilds sits
    /// in the menu-overlay corruption family the static-roots law rules
    /// out. The item set survives the toggle's own rebuild identically.
    #[tokio::test]
    async fn view_menu_parameters_entry_is_static_at_upstream_position() {
        use cosmic::widget::menu::Item;

        fn describe(entries: &[Item<Command, String>]) -> Vec<String> {
            entries
                .iter()
                .flat_map(|entry| match entry {
                    Item::Button(label, _, action) => {
                        vec![format!("button {label:?} {action:?}")]
                    }
                    Item::ButtonDisabled(label, _, action) => {
                        vec![format!("disabled {label:?} {action:?}")]
                    }
                    Item::Folder(label, children) => {
                        let mut rows = vec![format!("folder {label:?}")];
                        rows.extend(describe(children));
                        rows
                    }
                    Item::Divider => vec!["divider".to_string()],
                    _ => vec!["other".to_string()],
                })
                .collect()
        }

        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-viewmenu-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root, &SettingsStore::in_temp_dir());

        let before = describe(&window.view_entries());
        // the message a menu CLICK produces (MenuAction::message) — the
        // entry is the second producer of this command beside the ⌥⌘I
        // keybind, but on the click path only: a keystroke routes through
        // the app's key router alone, so one input fires one command
        let _ = window.update(Message::Command(Command::ToggleParameters));
        assert!(
            !window.viewer.parameters_visible,
            "the command fired — the static-set assert below is not vacuous"
        );
        window.bridge.mark_python_ready_for_test();
        assert_eq!(
            before,
            describe(&window.view_entries()),
            "the toggle's own rebuild leaves every View item and label byte-identical"
        );

        let entries = window.view_entries();
        let mut hits: Vec<_> = entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry {
                Item::Button(label, _, Command::ToggleParameters) => Some((index, label)),
                _ => None,
            })
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "exactly one parameters entry in the View root"
        );
        let (index, label) = hits.pop().unwrap();
        assert!(
            matches!(&entries[index - 1], Item::Button(previous, _, Command::FocusSchematic)
                if previous == "Go to 2D Drawing"),
            "directly after the focus trio, upstream's View order"
        );
        assert_eq!(
            label,
            // upstream's static tooltip string (ProjectWindow.swift:159),
            // never flipping with the overlay's state
            "Show or hide parameters (⌥⌘I)"
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
        let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &SettingsStore::in_temp_dir());
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
        window.bridge.mark_python_ready_for_test();
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
        let (mut window, _bridge_task) = ProjectWindow::new(root, &SettingsStore::in_temp_dir());
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
        window.bridge.mark_python_ready_for_test();
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
        let (mut window, _bridge_task) = ProjectWindow::new(root, &SettingsStore::in_temp_dir());

        // a cancelled save panel is upstream's `guard … else { return }`
        let _ = window.update(Message::ExportChosen(Err("cancelled".into())));
        assert_eq!(
            window.activity,
            Activity::Idle,
            "a cancel leaves the row idle"
        );
        window.bridge.mark_python_ready_for_test();

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

    /// The header's ActivityViewer constructs in every activity state —
    /// including the Working row's mini spinner (construction is pure; the
    /// ring only reaches the GPU at draw time).
    #[tokio::test]
    async fn view_activity_constructs_in_every_state() {
        let root = std::env::temp_dir().join(format!("cad-act-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root, &SettingsStore::in_temp_dir());
        for activity in [
            Activity::Idle,
            Activity::Working("working".into()),
            Activity::Succeeded("done".into()),
            Activity::Failed("nope".into()),
        ] {
            window.activity = activity;
            let _ = window.view_activity();
        }
    }

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
        let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &SettingsStore::in_temp_dir());
        // 7.21: the overlay opens populated from the manifest
        assert_eq!(
            window.viewer.parameters.sliders.len(),
            1,
            "open syncs the manifest's set"
        );
        window.viewer.sync_parameters(&[]);

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

        // 7.21: an agent turn's bookkeeping re-syncs from the pipeline the
        // turn's tail refreshed
        window.viewer.sync_parameters(&[]);
        let _ = window.update(Message::Bridge(bridge::Message::TurnSuccessBookkeeping));
        assert_eq!(
            window.viewer.parameters.sliders.len(),
            1,
            "bookkeeping syncs the overlay"
        );

        // 7.21: a manifest rewrite (whoever wrote it) re-reads the set
        std::fs::write(root.join("model.py"), "wall = 4\nlid_h = 9\n").unwrap();
        std::fs::write(
            root.join("output/manifest.json"),
            r#"{"parameters": {"wall": 4, "lid_h": 9}}"#,
        )
        .unwrap();
        let _ = window.update(Message::OutputReload(ReloadKind::Manifest));
        let names: Vec<_> = window
            .viewer
            .parameters
            .sliders
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        assert_eq!(names.len(), 2, "the reload shows the new set: {names:?}");
        assert!(names.contains(&"lid_h".to_string()));
    }

    /// 6.4: a restore writes its status line either way
    /// (Project.swift:319-321) — "Version N restored" on success, "Could
    /// not restore version N" on failure.
    #[tokio::test]
    async fn restore_sets_the_status_line() {
        use engine::project::versions;
        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-restore-{}", uuid::Uuid::new_v4()));
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
            "a cube",
            None,
            ::time::OffsetDateTime::now_utc(),
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root, &SettingsStore::in_temp_dir());
        window.bridge.mark_python_ready_for_test();

        let _ = window.update(Message::Restore(1));
        assert_eq!(
            window.activity,
            Activity::Succeeded(fl!("versione-lld-ripristinata", version = 1))
        );
        let _ = window.update(Message::Restore(7));
        assert_eq!(
            window.activity,
            Activity::Failed(fl!("impossibile-ripristinare-la-versione-lld", version = 7))
        );
    }

    /// The chat's version markers mirror the history and the current
    /// version at open and after a restore, and a marker's restore
    /// button restores through the shell.
    #[tokio::test]
    async fn chat_markers_follow_the_history_and_restore() {
        use engine::project::versions;
        let root = std::env::temp_dir().join(format!("cad-markers-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let mut history = Vec::new();
        for (prompt, wall) in [("a cube", 3), ("thicker", 5)] {
            std::fs::write(root.join("model.py"), format!("wall = {wall}\n")).unwrap();
            versions::record(
                &root,
                &mut history,
                prompt,
                None,
                ::time::OffsetDateTime::now_utc(),
            )
            .unwrap();
        }
        let mut metadata =
            store::load_metadata(&root).unwrap_or_else(|_| store::ProjectMetadata::new("test"));
        metadata.current_version = Some(2);
        store::save_metadata(&root, &metadata).unwrap();

        let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &SettingsStore::in_temp_dir());
        window.bridge.mark_python_ready_for_test();
        assert_eq!(window.chat.versions, vec![1, 2], "mirrored at open");
        assert_eq!(window.chat.current_version, Some(2));

        let _ = window.update(Message::Chat(chat_view::Message::RestoreVersion(1)));
        assert_eq!(
            window.metadata.current_version,
            Some(1),
            "the shell restored"
        );
        assert_eq!(window.chat.current_version, Some(1), "the mirror followed");
        assert_eq!(window.chat.versions, vec![1, 2]);
        assert_eq!(
            std::fs::read_to_string(root.join("model.py")).unwrap(),
            "wall = 3\n"
        );
    }

    /// The menu gates (Commands.swift:101, 110, 122-137) flip Button ↔
    /// ButtonDisabled only — every root keeps its item set and labels —
    /// and the entries upstream has are present: Stop disabled when idle,
    /// Mostra nel Finder, Apri in slicer gated on the 3MF, "Scrivi un
    /// messaggio" in View, the Viewer 3D / Schematica leaves gated on a
    /// mesh / the SVG, the chat entry's label static.
    #[tokio::test]
    async fn menu_gates_swap_enablement_without_changing_the_item_set() {
        use cosmic::widget::menu::Item;
        fn describe(entries: &[Item<Command, String>]) -> Vec<(String, Option<Command>, bool)> {
            entries
                .iter()
                .flat_map(|entry| match entry {
                    Item::Button(label, _, action) => vec![(label.clone(), Some(*action), true)],
                    Item::ButtonDisabled(label, _, action) => {
                        vec![(label.clone(), Some(*action), false)]
                    }
                    Item::Folder(label, children) => {
                        let mut rows = vec![(label.clone(), None, true)];
                        rows.extend(describe(children));
                        rows
                    }
                    _ => vec![(String::from("-"), None, true)],
                })
                .collect()
        }
        let enabled = |rows: &[(String, Option<Command>, bool)], command: Command| {
            rows.iter()
                .find(|(_, action, _)| *action == Some(command))
                .map(|(_, _, on)| *on)
                .expect("entry present")
        };
        let _lang = crate::localize::pin_en();
        let root = std::env::temp_dir().join(format!("cad-gates-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &SettingsStore::in_temp_dir());
        window.bridge.mark_python_ready_for_test();

        let model_bare = describe(&window.model_entries());
        let view_bare = describe(&window.view_entries());
        assert!(
            !enabled(&model_bare, Command::Stop),
            "Stop is disabled when idle"
        );
        assert!(enabled(&model_bare, Command::Reveal));
        assert!(!enabled(&model_bare, Command::OpenInSlicer));
        assert!(
            !enabled(&view_bare, Command::ViewerFit),
            "no mesh: Viewer 3D gated"
        );
        assert!(
            !enabled(&view_bare, Command::SchematicFit),
            "no SVG: Schematica gated"
        );
        assert!(view_bare.iter().any(|(label, action, _)| {
            *action == Some(Command::FocusChat) && label == "Write a Message"
        }));

        std::fs::write(root.join("output/model.3mf"), b"x").unwrap();
        std::fs::write(root.join("output/schematic.svg"), b"<svg/>").unwrap();
        let _ = window.update(Message::Command(Command::ShowChat));
        let model_full = describe(&window.model_entries());
        let view_full = describe(&window.view_entries());
        assert!(enabled(&model_full, Command::OpenInSlicer));
        assert!(enabled(&view_full, Command::ViewerFit));
        assert!(enabled(&view_full, Command::SchematicZoomIn));

        let shape = |rows: &[(String, Option<Command>, bool)]| {
            rows.iter()
                .map(|(label, action, _)| (label.clone(), *action))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shape(&model_bare),
            shape(&model_full),
            "Modello's item set is fixed"
        );
        assert_eq!(
            shape(&view_bare),
            shape(&view_full),
            "View's item set and labels are fixed — the chat toggle included"
        );
    }

    /// 4.11: a pane drags by its grab handle to another pane's edge
    /// (PaneContainer.swift:30-33 → Project.dropPane): press, move over the
    /// target's edge (window coordinates, under the header), release.
    #[tokio::test]
    async fn grip_drag_moves_a_pane_to_the_target_edge() {
        let root = std::env::temp_dir().join(format!("cad-grip-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root, &SettingsStore::in_temp_dir());
        window.layout = LayoutNode::chat_leading();
        window.rebuild_grid();
        let _ = window.update(Message::GridResized(Size::new(1320.0, 787.0)));
        let before = window.layout.clone();

        // a press and release with no motion drops nothing
        let _ = window.update(Message::GripPressed(PaneKind::Chat));
        let _ = window.update(Message::PaneDragEnded);
        assert_eq!(window.layout, before);

        // chat → the right edge of the schematic pane (bottom-right of
        // chat_leading's grid)
        let _ = window.update(Message::GripPressed(PaneKind::Chat));
        let over = cosmic::iced::Point::new(1300.0, HEADER_HEIGHT + 1.0 + 700.0);
        let _ = window.update(Message::PaneDragMoved(over));
        assert_eq!(
            window.pane_drag.and_then(|drag| drag.target),
            Some(PaneKind::Schematic),
            "the drag targets the pane under the cursor"
        );
        let _ = window.update(Message::PaneDragEnded);
        assert_eq!(window.pane_drag, None);
        assert_eq!(
            window.layout,
            panes::moving(
                &before,
                PaneKind::Chat,
                panes::DropEdge::Trailing,
                PaneKind::Schematic
            )
        );
    }

    /// The header cards close together on an outside click.
    #[tokio::test]
    async fn close_cards_dismisses_every_header_card() {
        let root = std::env::temp_dir().join(format!("cad-cards-{}", uuid::Uuid::new_v4()));
        store::create_project(
            root.parent().unwrap(),
            root.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let (mut window, _bridge_task) = ProjectWindow::new(root, &SettingsStore::in_temp_dir());
        window.recent_card = Some(vec![PathBuf::from("/tmp/a")]);
        let _ = window.update(Message::ToggleVersionCard);
        assert!(window.version_card.is_some());
        let _ = view_recents(&[PathBuf::from("/tmp/a")]);
        let _ = view_recents(&[]);
        let _ = window.update(Message::CloseCards);
        assert!(window.version_card.is_none() && window.recent_card.is_none());
    }

    /// The metadata-clobber pin (T7 review F1): the window's save points
    /// (turn end, bookkeeping) must write project.json THROUGH the disk
    /// copy. The engine writes the session id into the same file mid-turn
    /// and `record_version` the version pointer — both BEHIND the window's
    /// back — so a save from the window's open-time copy reverts them. The
    /// restore-then-bookkeeping tail pins the same law after a restore
    /// moves both fields.
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

        let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &SettingsStore::in_temp_dir());

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
        window.bridge.mark_python_ready_for_test();
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

        let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &SettingsStore::in_temp_dir());
        assert!(!window.viewer.comparing);

        let _ = window.update(Message::Viewer(crate::viewer_pane::Message::Parameters(
            parameters_view::Message::Compare(Some(1)),
        )));
        assert_eq!(
            window.bridge.compare_version(),
            Some(1),
            "the bridge holds the selection"
        );
        window.bridge.mark_python_ready_for_test();
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
