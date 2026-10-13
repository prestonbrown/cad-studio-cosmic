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
//! Commands (the M4 subset only): show-chat ⌘0, focus ⌘1/2/3, layout
//! presets, viewer presets + fit, schematic zoom/fit, stop, reveal,
//! open-in-slicer. No export/output command (M8), no Compila (M5), no
//! params pane (M6). The keyboard bindings are one pure table,
//! [`project_shortcut`]; the header's menu renders the same commands.
//!
//! User-visible strings follow the landed modules' convention: plain
//! English values, each named for its en/main.ftl key, pending the
//! repo-wide `fl!()` swap.

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
use crate::bridge::{self, Bridge};
use crate::chat::ChatState;
use crate::chat_view::{self, ChatPane, QueueItem};
use crate::panes;
use crate::schematic::SchematicPane;
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
/// ProjectToolbar, M4 subset). Doubles as the menu action type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    ShowChat,
    FocusChat,
    FocusModel,
    FocusSchematic,
    LayoutChatLeading,
    LayoutChatTrailing,
    LayoutChatBottom,
    LayoutThreeColumns,
    ViewerPreset(Preset),
    ViewerFit,
    SchematicZoomIn,
    SchematicZoomOut,
    SchematicFit,
    Stop,
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
    /// An issue button opened/closed its list (`None` closes).
    ToggleIssues(Option<IssueKind>),
    /// The header's close button (the app layer closes the window).
    CloseRequested,
    /// An app-level request bubbled from the File menu (the window cannot
    /// act on windows or portals itself).
    App(AppRequest),
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

impl ProjectWindow {
    /// Open a project (upstream `Project.init`, Project.swift:91-105):
    /// prepare the folder scaffolding, load metadata + the stored layout,
    /// load the chat (absent/corrupt opens empty), build the panes over
    /// `output/`, and launch the bridge. Returns the window plus the
    /// bridge's message-stream task — the app maps it into this window's
    /// messages. Must run inside the tokio runtime cosmic runs on.
    pub fn new(root: PathBuf, settings: &Settings) -> (Self, CosmicTask<bridge::Message>) {
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

        let (bridge, bridge_task) = Bridge::new(settings.clone(), root.clone());

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

        (
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
                viewer: ViewerPane::new(
                    &root,
                    settings.measurement_unit,
                    settings.printer_model.clone(),
                ),
                schematic: SchematicPane::new(&files, manifest.as_ref()),
                bridge,
                queue: Vec::new(),
                queue_paused: false,
                activity: Activity::Idle,
                work_started_at: None,
                missing_outputs: activity::missing_output_files(&output_dir),
                shown_issues: None,
                focus_flash: None,
                last_size: Arc::new(Mutex::new(Size::default())),
                watch,
                events,
                relay: Arc::new(OutputRelay {
                    id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                    events: Mutex::new(Some(receiver)),
                }),
                root,
            },
            bridge_task,
        )
    }

    /// Whether a turn is running (upstream `isBusy`).
    pub fn is_busy(&self) -> bool {
        self.activity.is_working()
    }

    fn output_dir(&self) -> PathBuf {
        self.root.join("output")
    }

    /// An established save point (upstream `save()`, Project.swift:429-433):
    /// metadata with the current layout, timestamps bumped.
    fn save_metadata(&mut self) {
        self.metadata.layout = Some(self.layout.clone());
        self.metadata.updated_at = OffsetDateTime::now_utc();
        self.metadata.app_version = env!("CARGO_PKG_VERSION").to_string();
        let _ = store::save_metadata(&self.root, &self.metadata);
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
                self.chat.draft_attachments.extend(staged);
                Task::none()
            }
            Message::AttachChosen(Err(_cancelled)) => Task::none(),
            Message::ToggleIssues(kind) => {
                self.shown_issues = kind;
                Task::none()
            }
            Message::CloseRequested | Message::App(_) => Task::none(), // the app layer acts
        }
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
                    self.chat_pane.sent();
                    self.bridge.enqueue(text, attachments);
                }
            }
            chat_view::Message::SendSuggestion(suggestion) => {
                self.bridge.enqueue(suggestion.clone(), Vec::new());
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
            // a system row's Open Terminal action (MessageView.swift:71-77);
            // AskClaudeToFix has no build errors to read until M5 and
            // ConfigureClaude is the M8 settings window
            chat_view::Message::RowAction(_, Action::OpenTerminal) => {
                crate::app::launch_terminal();
            }
            chat_view::Message::RowAction(_, _) => {}
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
            bridge::Message::TurnStarted { .. } => {
                self.set_activity(activity::next(activity::Signal::TurnStarted, false));
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
                        ));
                        // a turn end is an established layout save point
                        self.save_metadata();
                    }
                    _ => {}
                }
                ChatPane::scroll_to_bottom().map(Message::Chat)
            }
            bridge::Message::Closed => {
                self.set_activity(activity::next(activity::Signal::Closed, false));
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
        let events = self.events.clone();
        let last_size = self.last_size.clone();
        let this = self;
        // the grid reports its laid-out size through the shell relay: the
        // resize clamp needs the GRID extent, and the window size would
        // ignore the header (the panes' responsive report pattern)
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

        // the issue list rides as a non-modal popover over the content
        let mut pop = popover(base);
        if let Some(kind) = self.shown_issues {
            pop = pop.popup(self.view_issue_list(kind));
        }
        pop.into()
    }

    /// The resizable pane grid (rebuilt per layout — iced elements are
    /// single-use), filling the window below the header.
    fn view_grid(&self) -> Element<'_, Message> {
        let grid = pane_grid::PaneGrid::new(&self.grid, |_pane, kind: &PaneKind, _| {
            let body: Element<'_, Message> = match kind {
                PaneKind::Chat => chat_view::view_chat(
                    &self.chat_pane,
                    &self.chat,
                    self.is_busy(),
                    &self.queue,
                    self.queue_paused,
                )
                .map(Message::Chat),
                PaneKind::Model3D => self.viewer.view_model_3d(None).map(Message::Viewer),
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
    /// ActivityViewer | commands menu + reveal + slicer.
    fn view_header(&self) -> Element<'_, Message> {
        let chat_toggle = button::icon(
            cosmic::widget::icon::from_name("sidebar-hide")
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
        let slicer = button::icon(
            cosmic::widget::icon::from_name("x-app-export")
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
            .end(self.view_menu())
            .end(reveal)
            .end(slicer)
            .into()
    }

    /// The minimal ActivityViewer port (Shared/ActivityViewer.swift):
    /// project name, status icon + text, elapsed, and the error/warning
    /// issue buttons — the upstream home of output warnings. The full port
    /// (manifest warnings, motion issues) is M6.
    fn view_activity(&self) -> Element<'_, Message> {
        let mut row = Row::new()
            .spacing(6)
            .push(text(self.name.clone()).size(12))
            .push(text("›".to_string()).size(12).class(dim_text()));
        row = match &self.activity {
            Activity::Idle => row.push(text("Ready".to_string()).size(12).class(dim_text())),
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

        let issues = activity::issues(None, &self.missing_outputs);
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
        let issues = activity::issues(None, &self.missing_outputs);
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
                button::text("Close")
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

    /// The commands menu bar (the M4 subset of upstream AppCommands): a
    /// "File" root bubbling the app-level commands, and a "View" root over
    /// the pane/layout/viewer/schematic commands.
    fn view_menu(&self) -> Element<'static, Message> {
        use cosmic::widget::menu::{Item, items, root};

        let file_entries = vec![
            Item::Button("New Project…", None, AppRequest::NewProject),
            Item::Button("Open Project…", None, AppRequest::OpenProject),
            Item::Button("Open Recent…", None, AppRequest::OpenRecent),
            Item::Divider,
            Item::Button("Welcome Window", None, AppRequest::Welcome),
        ];
        let file_binds = std::collections::HashMap::new();
        let file_tree = cosmic::widget::menu::Tree::with_children(
            {
                let root_button: Element<'_, Message> = root("File").into();
                root_button
            },
            items(&file_binds, file_entries),
        );

        let entries = vec![
            Item::Button(
                if self.chat_visible {
                    "Hide Chat"
                } else {
                    "Show Chat"
                },
                None,
                Command::ShowChat,
            ),
            Item::Divider,
            Item::Button("Go to Chat", None, Command::FocusChat),
            Item::Button("Go to 3D Viewer", None, Command::FocusModel),
            Item::Button("Go to 2D Schematic", None, Command::FocusSchematic),
            Item::Divider,
            Item::Folder(
                "Layout",
                vec![
                    Item::Button("Chat on Left", None, Command::LayoutChatLeading),
                    Item::Button("Chat on Right", None, Command::LayoutChatTrailing),
                    Item::Button("Chat at Bottom", None, Command::LayoutChatBottom),
                    Item::Button("Three Columns", None, Command::LayoutThreeColumns),
                ],
            ),
            Item::Folder(
                "3D Viewer",
                vec![
                    Item::Button("Isometric View", None, Command::ViewerPreset(Preset::Iso)),
                    Item::Button("Front View", None, Command::ViewerPreset(Preset::Front)),
                    Item::Button("Top View", None, Command::ViewerPreset(Preset::Top)),
                    Item::Button("Right View", None, Command::ViewerPreset(Preset::Right)),
                    Item::Divider,
                    Item::Button("Fit to View", None, Command::ViewerFit),
                ],
            ),
            Item::Folder(
                "2D Schematic",
                vec![
                    Item::Button("Zoom In", None, Command::SchematicZoomIn),
                    Item::Button("Zoom Out", None, Command::SchematicZoomOut),
                    Item::Divider,
                    Item::Button("Fit to Window", None, Command::SchematicFit),
                ],
            ),
        ];
        let key_binds = std::collections::HashMap::new();
        let root_button: Element<'_, Message> = root("View").into();
        let tree =
            cosmic::widget::menu::Tree::with_children(root_button, items(&key_binds, entries));
        cosmic::widget::menu::bar(vec![file_tree, tree]).into()
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
        (true, false, false, "+") | (true, false, false, "=") => Some(SchematicZoomIn),
        (true, false, false, "-") => Some(SchematicZoomOut),
        // ⌘. stop — the period key arrives as a character
        (true, false, false, ".") => Some(Stop),
        (true, false, true, "r") => Some(Reveal),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shortcut in the table is a command the M4 window implements,
    /// and the upstream bindings land on the expected commands
    /// (Commands.swift:48-137); the M5/M8 bindings are deliberately
    /// absent.
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
        // ⌘B (build, M5) and ⇧⌘E (export, M8) are deliberately absent
        assert_eq!(project_shortcut(&Key::Character("b".into()), &ctrl), None);
        assert_eq!(
            project_shortcut(&Key::Character("e".into()), &ctrl_shift),
            None
        );
        assert_eq!(
            project_shortcut(&Key::Character("1".into()), &Modifiers::empty()),
            None,
            "no binding without Ctrl"
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
}
