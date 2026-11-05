//! The application shell (upstream CADStudioApp.swift): one
//! `cosmic::Application` over a welcome window and one window per open
//! project, the recents store, the first-run and agent-missing sheets, the
//! command set, and the platform integrations — startup orphan reap
//! + SIGTERM child cleanup, the xdg-desktop-portal file/folder choosers,
//! and reveal via DBus `FileManager1.ShowItems` with an `xdg-open`
//! fallback.
//!
//! Window lifecycle: the main window IS the welcome window; opening a
//! project closes it (upstream `dismissWindow(id: WelcomeWindow.id)`),
//! after the main-window pointer moves to a surviving window so the close
//! does not exit the app; the last window closing exits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};
use cosmic::app::ApplicationExt;
use cosmic::app::Core;
use cosmic::cosmic_config::Config;
use cosmic::iced::keyboard::{Key, Modifiers};
use cosmic::iced::window;
use cosmic::iced::{Length, Subscription, Task};
use cosmic::widget::{Column, container, text};
use cosmic::{Action, Application, Element};
use engine::agent::locator::{self, AgentKind};
use engine::agent::session::{EngineEvent, TurnOutcome};
use engine::project::store;
use engine::python::env::{
    EnvEvent, EnvHandle, EnvState, check, command_probe, reinstall, streaming_spawn, venv_dir,
};

use crate::fl;
use crate::recents::{self, Recents};
use crate::settings_store::SettingsStore;
use crate::sheets;
use crate::shortcuts::{Context, Shortcut};
use crate::windows::project::{self, ProjectWindow};
use crate::windows::settings::{self, SettingsState};
use crate::windows::welcome::{self, Welcome};

/// Which window a [`window::Id`] names.
pub enum WindowKind {
    Welcome,
    Project(ProjectWindow),
    /// The settings window (upstream's `Settings` scene, here a
    /// separate non-modal window, not a body-replacing sheet).
    Settings(SettingsState),
    /// The About window (upstream `orderFrontStandardAboutPanel`).
    About,
    /// A project window whose load failed, holding the error text
    /// (ProjectWindow.swift:16-22).
    LoadError(String),
}

/// The load-error window's minimum frame (ProjectWindow.swift:22
/// `.frame(minWidth: 480, minHeight: 320)`).
const LOAD_ERROR_SIZE: (f32, f32) = (480.0, 320.0);

/// The load-error view's glyph (upstream `exclamationmark.triangle`).
const ICON_WARNING: &[u8] = include_bytes!("../resources/icons/exclamationmark.triangle.svg");

/// App-level commands reachable from any window: the global shortcuts and
/// the project window's File menu (upstream's global CommandGroups; ⌘→Ctrl,
/// ⇧⌘1 keeps upstream's Welcome binding, ⌘, the standard Settings scene's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    NewProject,
    OpenProject,
    /// The recents list lives in the welcome window; from a project window
    /// it drops the Open Recent card from the File root.
    OpenRecent,
    Welcome,
    /// Open the settings window (upstream's Settings scene; the port's
    /// placement is the File root — there is no app menu to hold it).
    Settings,
    /// Open the About window (upstream's app-info command group,
    /// Commands.swift:10-14; same placement note as Settings).
    About,
    /// Close the window the key landed in (the window's File menu).
    CloseWindow,
    /// Close every window; a running turn stops with its window.
    Quit,
    /// Toggle the focused window's fullscreen mode (no menu entry).
    ToggleFullscreen,
}

#[derive(Debug, Clone)]
pub enum Message {
    Welcome(welcome::Message),
    Project(window::Id, project::Message),
    /// The drive harness's chat send: stages the text as the project
    /// window's draft and sends it through the real shell path — a
    /// keyboard-free user turn (CAD_STUDIO_DRIVE's `chat` command).
    DriveChat(String),
    /// The drive harness's export answer: feeds the focused project window
    /// the SaveFile portal's destination directly (CAD_STUDIO_DRIVE's
    /// `saveas` command) — the portal itself needs a user, so a scripted
    /// export never opens it; the zip + activity + reveal path downstream
    /// is the real one.
    DriveSaveAs(PathBuf),
    /// The drive harness's version-card toggle: flips the project window's
    /// version-history card open/closed (CAD_STUDIO_DRIVE's `versioncard`
    /// command) — the toolbar clock button's own message, so the frame
    /// pass can capture the card presented.
    DriveVersionCard,
    /// The drive harness's Build: the toolbar Build Model button's own
    /// command on the focused project window (CAD_STUDIO_DRIVE's `build`).
    DriveBuild,
    /// The drive harness's project-window message (CAD_STUDIO_DRIVE's
    /// `viewer`, `restore`, `layout`, `cancel` and `attach`): delivered to
    /// the project window as the control that sends it would.
    DriveProject(project::Message),
    /// The drive harness's parameter commit (CAD_STUDIO_DRIVE's `param`):
    /// the project window's n-th parameter row (1-based) committed as a
    /// slider release would.
    DriveParameter(usize, crate::drive::ParamValue),
    /// The agent-missing sheet (presented over the focused window).
    AgentSheet(sheets::AgentMessage),
    /// The first-run setup sheet.
    SetupSheet(sheets::SetupMessage),
    /// The python setup sheet (PythonSetupSheet + the driver events it
    /// reduces).
    PythonSheet(sheets::PythonMessage),
    /// A settings-window interaction (tab switches, field writes, the
    /// effect requests the app performs).
    Settings(window::Id, settings::Message),
    /// Open (or focus) the settings window at one tab — the ⌘, command,
    /// the File-menu item, and the drive harness's route.
    OpenSettings(settings::Tab),
    /// Open (or focus) the About window — the app-info command group
    /// (Commands.swift:10-14).
    OpenAbout,
    /// A settings-tab locate finished for one engine.
    SettingsLocated(locator::AgentKind, Option<locator::AgentPath>),
    /// The folder portal answered the settings tab's Scegli….
    SettingsLocationChosen(Result<Vec<PathBuf>, String>),
    /// A chat row's "Configura l'agente…" action: present the
    /// agent-missing sheet (ChatView.swift:101-102 — upstream routes to
    /// the sheet, NOT to settings).
    PresentAgentSheet,
    /// A key press the focused window did not consume.
    KeyPressed(window::Id, Key, Modifiers),
    /// The keyboard modifier set changed while `id`'s window had focus.
    ModifiersChanged(window::Id, Modifiers),
    /// A window's header close button: libcosmic's `on_close` is
    /// message-only, so the handler must issue the `window::close` itself.
    CloseWindow(window::Id),
    /// Header-bar drag: start an interactive move (libcosmic's shell wires
    /// `Action::Drag`; this port's custom `decorate` emits it as a message).
    DragWindow(window::Id),
    /// Header-bar double-click: toggle maximized.
    ToggleMaximize(window::Id),
    /// Header-bar right-click: the window menu.
    WindowMenu(window::Id),
    /// A window finished closing (the platform Closed event).
    WindowClosed(window::Id),
    /// The startup/verify locate finished (`true` = a working executable).
    AgentLocated(bool),
    /// The parent-folder portal answered for the new-project sheet.
    ParentFolderChosen(Result<Vec<PathBuf>, String>),
    /// The open-existing portal answered.
    OpenFolderChosen(Result<Vec<PathBuf>, String>),
    /// `create_project` finished for the new-project sheet.
    ProjectCreated(Result<PathBuf, String>),
    /// Files were dropped onto `id`'s window (winit's window-level drop
    /// surface — X11 Xdnd and macOS; winit's Wayland backend has no file
    /// drops, the chat pane's dnd_destination covers it there).
    FilesDropped(window::Id, Vec<PathBuf>),
    /// SIGTERM arrived: terminate the engine's children and exit.
    SigTerm,
    /// A no-op ack for futures that must return a message.
    Noop,
}

/// One folder-portal request: its title and the directory it opens at.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderPick {
    title: String,
    start: Option<PathBuf>,
}

pub struct CadStudio {
    core: Core,
    windows: HashMap<window::Id, WindowKind>,
    welcome: Welcome,
    recents: Recents,
    config: Config,
    /// The engine settings, read fresh at every use and written only
    /// through [`SettingsStore::update`]; every project window shares it.
    settings: SettingsStore,
    /// The agent-missing sheet, when presented (upstream
    /// `AgentLocator.isMissingSheetPresented`).
    agent_missing: Option<sheets::AgentMissing>,
    /// The first-run setup sheet, when presented.
    setup: Option<sheets::Setup>,
    /// The python environment state + its sheet (upstream
    /// `PythonEnvironment.shared`: the state exists from launch, the sheet
    /// presents on top of it).
    python: sheets::PythonSetup,
    /// The drive harness's probe (only with CAD_STUDIO_DRIVE): the project
    /// window's activity, published after every update.
    drive_probe: Option<Arc<crate::drive::TurnProbe>>,
}

impl Application for CadStudio {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "net.brownhouse.CadStudio";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    /// Startup (CADStudioApp.swift:37-54): reap any crash orphans and arm
    /// the SIGTERM handler BEFORE any window opens, load the settings and
    /// the recents, decide the first-run sheet, and kick the agent locate
    /// and the python check (:50-52 — the check runs before the locate).
    fn init(mut core: Core, _flags: ()) -> (Self, Task<Action<Message>>) {
        // Every window draws its own header (`decorate`, and the project
        // toolbar's one-row header); libcosmic's main-window template would
        // stack its own header bar above whichever window is main.
        core.window.use_template = false;
        engine::processes::reap_orphans();

        let config = Config::new(Self::APP_ID, 1).expect("cosmic-config for app UI state");
        let settings = SettingsStore::global();
        let recents = Recents::load(config.clone());
        let setup =
            recents::needs_setup(&config).then(|| sheets::Setup::from_settings(&settings.load()));
        let mut python = sheets::PythonSetup::default();
        // the sheet state the check reduces from — the drivers emit only
        // their events, so Checking is the app's own preset
        python.state.begin_checking();

        // the optional UI drive harness (CAD_STUDIO_DRIVE): a script of
        // user-action messages streamed into the loop — the smoke-run
        // replacement for synthetic input, which COSMIC 1.6 gates
        let (drive, drive_probe) = crate::drive::startup_task();
        let app = CadStudio {
            windows: HashMap::from([(
                core.main_window_id()
                    .expect("the welcome window is the main window"),
                WindowKind::Welcome,
            )]),
            core,
            welcome: Welcome::new(),
            recents,
            config,
            settings,
            agent_missing: None,
            setup,
            python,
            drive_probe,
        };

        // locate the agent off the UI thread; a miss presents the sheet
        let settings = app.settings.load();
        let engine = settings.agent_kind();
        let locate = Task::future(locate_agent(
            engine,
            settings.agent_path(engine).map(str::to_string),
        ))
        .map(|found| Action::App(Message::AgentLocated(found)));
        let sigterm = Task::stream(sigterm_stream()).map(Action::App);
        // the startup python check: its terminal folds into the sheet
        // state (a missing/failed venv presents the sheet once first-run
        // setup is done)
        let python_check = python_events(check(&venv_dir(), &streaming_spawn()));
        let drive = drive.map(Action::App);
        (app, Task::batch([locate, sigterm, python_check, drive]))
    }

    fn update(&mut self, message: Message) -> Task<Action<Message>> {
        let sheet_was_up = self.modal_sheet_up();
        let task = match message {
            Message::Welcome(msg) => self.update_welcome(msg),
            Message::Project(id, msg) => self.update_project(id, msg),
            Message::DriveChat(text) => self
                .project_window()
                .map(|id| {
                    Task::batch([
                        Task::done(Message::Project(
                            id,
                            project::Message::Chat(crate::chat_view::Message::SetDraft(
                                text.clone(),
                            )),
                        )),
                        Task::done(Message::Project(
                            id,
                            project::Message::Chat(crate::chat_view::Message::Send),
                        )),
                    ])
                    .map(Action::App)
                })
                .unwrap_or_else(Task::none),
            Message::DriveSaveAs(path) => self
                .project_window()
                .map(|id| {
                    Task::done(Message::Project(
                        id,
                        project::Message::ExportChosen(Ok(path)),
                    ))
                })
                .unwrap_or_else(Task::none)
                .map(Action::App),
            Message::DriveVersionCard => self
                .project_window()
                .map(|id| Task::done(Message::Project(id, project::Message::ToggleVersionCard)))
                .unwrap_or_else(Task::none)
                .map(Action::App),
            Message::DriveBuild => self
                .project_window()
                .map(|id| {
                    Task::done(Message::Project(
                        id,
                        project::Message::Command(project::Command::Build),
                    ))
                })
                .unwrap_or_else(Task::none)
                .map(Action::App),
            Message::DriveProject(msg) => self
                .project_window()
                .map(|id| Task::done(Message::Project(id, msg)))
                .unwrap_or_else(Task::none)
                .map(Action::App),
            Message::DriveParameter(row, value) => self.drive_parameter(row, value),
            Message::AgentSheet(msg) => self.update_agent_sheet(msg),
            Message::SetupSheet(msg) => self.update_setup_sheet(msg),
            Message::PythonSheet(msg) => self.update_python_sheet(msg),
            Message::Settings(id, msg) => self.update_settings(id, msg),
            Message::OpenSettings(tab) => self.open_settings(tab),
            Message::OpenAbout => self.open_about(),
            Message::SettingsLocated(kind, found) => {
                // the one settings window's locate row resolves (open-or-
                // focus means there is at most one)
                for window in self.windows.values_mut() {
                    if let WindowKind::Settings(state) = window {
                        state.update(&settings::Message::Located(kind, found.clone()));
                    }
                }
                Task::none()
            }
            Message::SettingsLocationChosen(result) => match result {
                Ok(paths) => paths
                    .first()
                    .map(|path| {
                        let msg = settings::Message::LocationChosen(path.clone());
                        self.update_settings_any(msg)
                    })
                    .unwrap_or_else(Task::none),
                Err(_cancelled) => Task::none(),
            },
            Message::PresentAgentSheet => {
                // ChatView.swift:101-102: `AgentLocator.current
                // .isMissingSheetPresented = true` — the sheet, never the
                // settings window. The field seeds from the persisted
                // custom path (upstream's @AppStorage binding shows the
                // stored value), and no red missing line: this action
                // rides an auth-failed row, whose engine by definition
                // ran.
                self.present_agent_missing(false)
            }
            Message::KeyPressed(id, key, modifiers) => self.update_key_press(id, key, modifiers),
            Message::ModifiersChanged(id, modifiers) => self.update_modifiers(id, modifiers),
            Message::CloseWindow(id) => self.close_window(id),
            Message::DragWindow(id) => cosmic::iced::window::drag(id),
            Message::ToggleMaximize(id) => cosmic::iced::window::toggle_maximize(id),
            Message::WindowMenu(id) => cosmic::iced::window::show_system_menu(id),
            Message::WindowClosed(id) => self.window_closed(id),
            Message::AgentLocated(found) => {
                if found {
                    self.agent_missing = None;
                    Task::none()
                } else {
                    // the missing line shows from first presentation
                    // (AgentMissingSheet.swift:42 gates on
                    // `locator.state == .missing`, not on a Verify)
                    self.present_agent_missing(true)
                }
            }
            Message::ParentFolderChosen(Ok(parents)) => {
                let Some(sheet) = self.welcome.creating.as_mut() else {
                    return Task::none();
                };
                let Some(parent) = parents.first().cloned() else {
                    // no usable folder came back: the sheet stops waiting,
                    // so Avanti and Return ask again
                    sheet.awaiting_parent = false;
                    return Task::none();
                };
                let name = sheet.trimmed_name().to_string();
                Task::future(async move {
                    Message::ProjectCreated(
                        store::create_project(&parent, &name).map_err(|why| why.to_string()),
                    )
                })
                .map(Action::App)
            }
            Message::ParentFolderChosen(Err(_cancelled)) => {
                // the sheet stops waiting; a cancelled pick leaves it open
                if let Some(sheet) = self.welcome.creating.as_mut() {
                    sheet.awaiting_parent = false;
                }
                Task::none()
            }
            Message::OpenFolderChosen(result) => match result {
                Ok(paths) => paths
                    .first()
                    .map(|path| self.open_project(path.clone()))
                    .unwrap_or_else(Task::none),
                Err(_cancelled) => Task::none(),
            },
            Message::ProjectCreated(result) => match result {
                Ok(root) => {
                    self.welcome.creating = None;
                    self.open_project(root)
                }
                Err(why) => {
                    self.welcome
                        .update_sheet(&welcome::new_project::Message::Created(Err(why)));
                    Task::none()
                }
            },
            Message::FilesDropped(id, paths) => {
                // the drop lands in the chat's draft attachments wherever
                // on the window it fell — window-level is this surface's
                // whole granularity (upstream scopes it to the chat view;
                // the Wayland dnd_destination is the per-pane port)
                let msg = project::Message::Chat(crate::chat_view::Message::AttachContent(
                    crate::attachments::ClipboardContent::Files(paths),
                ));
                self.update_project(id, msg)
            }
            Message::SigTerm => {
                // upstream's handler: terminateAll() then exit(0)
                engine::processes::terminate_all();
                cosmic::iced::exit()
            }
            Message::Noop => Task::none(),
        };
        if let Some(probe) = &self.drive_probe {
            probe.publish(&self.project_activity());
        }
        // a sheet coming up over a project releases the focused text
        // widget: the sheet's shade blocks only the pointer, and a focused
        // chat editor would otherwise eat the sheet's Return/Esc
        if !sheet_was_up && self.modal_sheet_up() {
            Task::batch([task, release_focus()])
        } else {
            task
        }
    }

    fn view(&self) -> Element<'_, Message> {
        self.view_window(self.core.main_window_id().expect("main window"))
    }

    /// The per-window views. An unknown id (a window mid-open) shows a
    /// placeholder rather than panicking.
    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        let focused = self.core.focused_window() == Some(id);
        match self.windows.get(&id) {
            Some(WindowKind::Welcome) => {
                // The first-run and new-project sheets present over the
                // dimmed welcome body (WelcomeWindow.swift:23-31) as a
                // layer of the window's own view, not a modal popover:
                // libcosmic's modal popovers capture mouse events
                // window-wide, killing the header's drag and close.
                let body = self
                    .welcome
                    .view(self.recents.paths())
                    .map(Message::Welcome);
                if let Some(setup) = &self.setup {
                    let card: Element<'_, Message> =
                        sheets::view_setup(setup).map(Message::SetupSheet);
                    let presented = sheets::present(body, card, Message::Noop);
                    return self.decorate_borderless(id, focused, presented);
                }
                let body = match self.welcome.creating.as_ref() {
                    Some(sheet) => {
                        let card: Element<'_, Message> = welcome::new_project::view(sheet)
                            .map(|m| Message::Welcome(welcome::Message::NewSheet(m)));
                        sheets::present(body, card, Message::Noop)
                    }
                    None => body,
                };
                let with_sheet = self.apply_sheets(body);
                self.decorate_borderless(id, focused, with_sheet)
            }
            Some(WindowKind::Project(project)) => {
                // ONE header: the window's own view_header is the ProjectToolbar
                // port and already carries the close button (ProjectWindow.swift:54-56
                // `.toolbar { ProjectToolbar(project) }` + `.toolbar(removing: .title)`);
                // decorating again would stack a second bar.
                let body = project
                    .view_with_focus(focused)
                    .map(move |m| Message::Project(id, m));
                // the app-level sheets present over project windows too
                // (upstream attaches the python and agent-missing sheets to
                // ProjectWindow, ProjectWindow.swift:59-68; the first-run
                // sheet is welcome-only and can never be up here)
                let with_sheet = self.apply_sheets(body);
                container(with_sheet)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .class(cosmic::theme::Container::WindowBackground)
                    .into()
            }
            // upstream's sheets attach to ProjectWindow only
            // (ProjectWindow.swift:59-68), so the settings and About
            // windows carry no sheets of their own.
            Some(WindowKind::Settings(state)) => {
                let body = settings::view(state, &self.python.state.state)
                    .map(move |m| Message::Settings(id, m));
                // the window title names the selected tab, as upstream's
                // Settings scene does
                self.decorate(id, focused, body, &state.tab.title())
            }
            Some(WindowKind::About) => {
                // AppKit's About panel: a titlebar with no title
                let body: cosmic::Element<'_, Message> = settings::view_about();
                self.decorate(id, focused, body, "")
            }
            Some(WindowKind::LoadError(error)) => {
                self.decorate(id, focused, view_load_error(error), &fl!("cad-studio"))
            }
            None => self.decorate(
                id,
                focused,
                Column::new()
                    .push(text(fl!("cad-studio")))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                &fl!("cad-studio"),
            ),
        }
    }

    /// The window subscriptions: every project window's panes and relays,
    /// plus the keyboard router.
    fn subscription(&self) -> Subscription<Message> {
        let mut subs: Vec<Subscription<Message>> = Vec::new();
        for (id, kind) in &self.windows {
            if let WindowKind::Project(project) = kind {
                // `Subscription::map` takes only non-capturing closures, so
                // the window id rides through `with` and the tuple-struct
                // constructor does the mapping
                subs.push(
                    project
                        .subscription()
                        .with(*id)
                        .map(|(id, m)| Message::Project(id, m)),
                );
            }
        }
        subs.push(cosmic::iced::event::listen_with(key_events));
        Subscription::batch(subs)
    }

    /// A window finished closing: drop it, exit when it was the last one,
    /// and move the main-window pointer off it first — closing the pointer
    /// window otherwise exits the app with projects still open.
    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::WindowClosed(id))
    }
}

// ─── app-level update arms ─────────────────────────────────────────────────

impl CadStudio {
    /// The project window's id — the drive harness's windowless commands
    /// (chat, saveas, versioncard) all target it; new-project is
    /// open-or-focus, so there is at most one.
    fn project_window(&self) -> Option<window::Id> {
        self.windows
            .keys()
            .find(|id| matches!(self.windows.get(id), Some(WindowKind::Project(_))))
            .copied()
    }

    /// The project window's activity (idle without one).
    fn project_activity(&self) -> crate::activity::Activity {
        match self.project_window().and_then(|id| self.windows.get(&id)) {
            Some(WindowKind::Project(project)) => project.activity.clone(),
            _ => crate::activity::Activity::Idle,
        }
    }

    /// The drive `param` line: resolve the row against the project window's
    /// parameter rows and commit it; a row past the end fails the script.
    fn drive_parameter(
        &mut self,
        row: usize,
        value: crate::drive::ParamValue,
    ) -> Task<Action<Message>> {
        let Some(id) = self.project_window() else {
            crate::drive::log("FAIL param: no project window");
            return Task::none();
        };
        let Some(WindowKind::Project(project)) = self.windows.get(&id) else {
            return Task::none();
        };
        let rows = project.bridge.parameters();
        let Some(parameter) = rows.get(row - 1) else {
            crate::drive::log(&format!(
                "FAIL param: row {row} of {} parameters",
                rows.len()
            ));
            return Task::none();
        };
        let committed = value.apply(parameter.value);
        crate::drive::log(&format!(
            "param {} {} -> {committed}",
            parameter.name, parameter.value
        ));
        Task::done(Message::Project(
            id,
            project::Message::Viewer(crate::viewer_pane::Message::ParameterCommit(
                parameter.name.clone(),
                committed,
            )),
        ))
        .map(Action::App)
    }

    fn update_welcome(&mut self, msg: welcome::Message) -> Task<Action<Message>> {
        match msg {
            welcome::Message::NewProject => self.new_project(),
            welcome::Message::OpenExisting => choose_existing(),
            welcome::Message::OpenRecent(path) => self.open_project(path),
            welcome::Message::RevealRecent(path) => Task::future(async move {
                reveal_path(path).await;
                Message::Noop
            })
            .map(Action::App),
            welcome::Message::RemoveRecent(path) => {
                self.recents.remove(path);
                Task::none()
            }
            // a context-menu entry resolves against the recents store first
            welcome::Message::RecentMenu(index, kind) => {
                let Some(path) = self.recents.paths().get(index).cloned() else {
                    return Task::none();
                };
                self.update_welcome(match kind {
                    welcome::RecentMenuKind::Open => welcome::Message::OpenRecent(path),
                    welcome::RecentMenuKind::Reveal => welcome::Message::RevealRecent(path),
                    welcome::RecentMenuKind::Remove => welcome::Message::RemoveRecent(path),
                })
            }
            welcome::Message::NewSheet(msg) => self.update_new_project_sheet(msg),
        }
    }

    /// Present the agent-missing sheet, its path field focused as a SwiftUI
    /// sheet focuses its first field. A sheet already up keeps its state
    /// and only takes the focus again: one raised at launch, before any
    /// window drew it, gets its focus when a later send presents it. A new
    /// sheet's field opens on the persisted custom path (upstream's
    /// @AppStorage binding, so an untouched Verify cannot clear a stored
    /// path); `missing` shows the red missing line from the start.
    fn present_agent_missing(&mut self, missing: bool) -> Task<Action<Message>> {
        if self.agent_missing.is_none() {
            let settings = self.settings.load();
            let kind = settings.agent_kind();
            self.agent_missing = Some(sheets::AgentMissing {
                kind,
                custom_path: settings.agent_path(kind).unwrap_or_default().to_string(),
                last_verified_missing: missing,
                ..sheets::AgentMissing::default()
            });
        }
        cosmic::widget::text_input::focus(sheets::agent_path_field_id())
    }

    /// The new-project command presents the sheet AND makes sure the
    /// welcome window carrying it is on screen (upstream ⌘N:
    /// openWindow(id: WelcomeWindow.id) + isCreatingProject = true).
    fn new_project(&mut self) -> Task<Action<Message>> {
        if self.welcome.creating.is_some() {
            return self.show_welcome();
        }
        self.welcome.creating = Some(welcome::new_project::NewProject::default());
        // the sheet comes up with its name field focused, as a SwiftUI
        // sheet focuses its first field
        Task::batch([
            self.show_welcome(),
            cosmic::widget::text_input::focus(welcome::new_project::name_field_id()),
        ])
    }

    fn update_new_project_sheet(
        &mut self,
        msg: welcome::new_project::Message,
    ) -> Task<Action<Message>> {
        match msg {
            welcome::new_project::Message::Cancel => {
                self.welcome.creating = None;
                Task::none()
            }
            welcome::new_project::Message::Advance => {
                // fold the sheet state: the fold sets awaiting_parent
                // only for a valid name (upstream create()'s
                // `guard isNameValid`, NewProjectSheet.swift:57) — an
                // invalid Return asks nothing, and a pick already in
                // flight is not asked twice (upstream's panel is modal)
                if self
                    .welcome
                    .creating
                    .as_ref()
                    .is_some_and(|s| s.awaiting_parent)
                {
                    return Task::none();
                }
                self.welcome.update_sheet(&msg);
                let Some(sheet) = self
                    .welcome
                    .creating
                    .as_ref()
                    .filter(|sheet| sheet.awaiting_parent)
                else {
                    return Task::none();
                };
                let pick = self.parent_folder_pick(sheet.trimmed_name());
                Task::future(async move {
                    let future = choose_folder_at(&pick.title, pick.start.as_deref());
                    Message::ParentFolderChosen(future.await)
                })
                .map(Action::App)
            }
            welcome::new_project::Message::Created(_) | welcome::new_project::Message::Name(_) => {
                self.welcome.update_sheet(&msg);
                Task::none()
            }
        }
    }

    /// The new-project parent-folder panel: titled for the name, opening
    /// at the Settings default project location
    /// (ProjectPanels.swift:15-24 `panel.directoryURL =
    /// AppSettings.defaultProjectLocation`).
    fn parent_folder_pick(&self, name: &str) -> FolderPick {
        FolderPick {
            title: fl!("scegli-dove-creare-la-cartella", name = name.to_string()),
            start: Some(self.settings.load().default_project_location),
        }
    }

    /// A project window's message: its app requests are the app's, the
    /// rest is the window's own update.
    fn update_project(&mut self, id: window::Id, msg: project::Message) -> Task<Action<Message>> {
        if let project::Message::App(request) = msg {
            return self.app_request(id, request);
        }
        // the pane-drag listener hears every window's cursor; only this
        // window's events drive its drag
        if let project::Message::PaneDragMoved(source, _)
        | project::Message::PaneDragEnded(source) = msg
        {
            if source != id {
                return Task::none();
            }
        }
        let Some(WindowKind::Project(project)) = self.windows.get_mut(&id) else {
            return Task::none();
        };
        // a turn that found no agent executable presents the agent-missing
        // sheet beside the window's own ConfigureClaude row
        // (AgentSession.swift:41-50 `locator.isMissingSheetPresented = true`)
        let agent_missing = matches!(
            &msg,
            project::Message::Bridge(crate::bridge::Message::Event(EngineEvent::TurnFinished(
                TurnOutcome::AgentMissing
            )))
        );
        let task = project
            .update(msg)
            .map(move |m| Message::Project(id, m))
            .map(Action::App);
        if agent_missing {
            return Task::batch([task, self.update(Message::AgentLocated(false))]);
        }
        task
    }

    /// What project window `id` asked of the app layer.
    fn app_request(
        &mut self,
        id: window::Id,
        request: project::AppRequest,
    ) -> Task<Action<Message>> {
        if !matches!(self.windows.get(&id), Some(WindowKind::Project(_))) {
            return Task::none();
        }
        match request {
            project::AppRequest::Command(command) => self.app_command(id, command),
            project::AppRequest::Close => self.close_window(id),
            project::AppRequest::Window(op) => match op {
                project::WindowOp::Drag => cosmic::iced::window::drag(id),
                project::WindowOp::ToggleMaximize => cosmic::iced::window::toggle_maximize(id),
                project::WindowOp::Minimize => cosmic::iced::window::minimize(id, true),
                project::WindowOp::Menu => cosmic::iced::window::show_system_menu(id),
            },
            project::AppRequest::PresentSetupSheet => {
                // a python-readiness refusal presents the setup sheet over
                // whatever window is up, UNCONDITIONALLY (Project.swift:184-197
                // set isSetupSheetPresented = true; the hasCompletedSetup gate
                // belongs to the startup check's terminal only)
                self.python.presented = true;
                Task::none()
            }
            project::AppRequest::PresentAgentSheet => self.update(Message::PresentAgentSheet),
            project::AppRequest::OpenRecentPath(path) => {
                self.set_recent_card(id, None);
                self.open_project(path)
            }
            project::AppRequest::ClearRecents => {
                // Commands.swift:36-40 `recents.clear()`
                self.set_recent_card(id, Some(Vec::new()));
                self.recents.clear();
                Task::none()
            }
        }
    }

    /// Show (`Some`) or close project window `id`'s Open Recent card.
    fn set_recent_card(&mut self, id: window::Id, card: Option<Vec<PathBuf>>) {
        if let Some(WindowKind::Project(project)) = self.windows.get_mut(&id) {
            project.recent_card = card;
        }
    }

    /// An app-level command from window `id`: a shortcut pressed there, or
    /// a project window's File menu.
    fn app_command(&mut self, id: window::Id, command: AppCommand) -> Task<Action<Message>> {
        match command {
            AppCommand::NewProject => self.new_project(),
            AppCommand::OpenProject => choose_existing(),
            // "Apri recenti" drops the recents card from the File root
            AppCommand::OpenRecent => {
                self.set_recent_card(id, Some(self.recents.paths().to_vec()));
                Task::none()
            }
            AppCommand::Welcome => self.show_welcome(),
            AppCommand::Settings => self.open_settings(settings::Tab::General),
            AppCommand::About => self.open_about(),
            AppCommand::CloseWindow => self.close_window(id),
            AppCommand::Quit => {
                engine::processes::terminate_all();
                let ids: Vec<_> = self.windows.keys().copied().collect();
                Task::batch(ids.into_iter().map(|id| self.close_window(id)))
            }
            AppCommand::ToggleFullscreen => window::mode(id)
                .then(move |mode| {
                    window::set_mode(
                        id,
                        if mode == window::Mode::Fullscreen {
                            window::Mode::Windowed
                        } else {
                            window::Mode::Fullscreen
                        },
                    )
                })
                .map(Action::App),
        }
    }

    fn update_agent_sheet(&mut self, msg: sheets::AgentMessage) -> Task<Action<Message>> {
        let Some(sheet) = self.agent_missing.as_mut() else {
            return Task::none();
        };
        match &msg {
            sheets::AgentMessage::Close => {
                self.agent_missing = None;
                return Task::none();
            }
            sheets::AgentMessage::OpenTerminal => {
                launch_terminal();
            }
            sheets::AgentMessage::Verify => {
                // persist the custom path first — locate consults it
                let mut settings = self.settings.load();
                sheet.stage_custom_path(&mut settings);
                let _ = self
                    .settings
                    .update(|stored| sheet.stage_custom_path(stored));
                sheet.checking = true;
                self.reseed_settings_windows();
                let engine = settings.agent_kind();
                let future = locate_agent(engine, settings.agent_path(engine).map(str::to_string));
                return Task::future(async move {
                    let found = future.await;
                    Message::AgentSheet(sheets::AgentMessage::Verified(found))
                })
                .map(Action::App);
            }
            sheets::AgentMessage::CustomPath(_) | sheets::AgentMessage::Copy(_) => {}
            // a successful Verify dismisses the sheet (AgentMissingSheet's
            // `if locator.executable != nil { isMissingSheetPresented = false }`)
            sheets::AgentMessage::Verified(true) => {
                self.agent_missing = None;
                return Task::none();
            }
            sheets::AgentMessage::Verified(false) => {}
        }
        let task =
            sheets::update_agent_missing(self.agent_missing.as_mut().expect("checked above"), &msg)
                .map(Message::AgentSheet);
        task.map(Action::App)
    }

    fn update_setup_sheet(&mut self, msg: sheets::SetupMessage) -> Task<Action<Message>> {
        let Some(setup) = self.setup.as_mut() else {
            return Task::none();
        };
        if matches!(msg, sheets::SetupMessage::Continue) {
            // the three preference keys into the engine settings, the flag
            // into cosmic-config (SetupSheet.swift:31-34)
            let _ = self.settings.update(|settings| setup.apply(settings));
            self.push_settings_to_windows();
            let _ = recents::complete_setup(&self.config);
            self.setup = None;
            // the first-run sheet coming down presents the python sheet
            // whenever the environment is not ready yet
            // (WelcomeWindow.swift:23's onDismiss)
            if !matches!(self.python.state.state, EnvState::Ready { .. }) {
                self.python.presented = true;
            }
            return Task::none();
        }
        setup.update(&msg);
        Task::none()
    }

    /// The python setup sheet: the sheet-side halves of upstream's
    /// PythonEnvironment (presentation, install kicks) over the pure
    /// reducer in `sheets`.
    fn update_python_sheet(&mut self, msg: sheets::PythonMessage) -> Task<Action<Message>> {
        match msg {
            // interactiveDismissDisabled(isInstalling): an install blocks
            // dismissal, a check does not
            sheets::PythonMessage::Close => {
                if !sheets::dismiss_blocked(&self.python.state.state) {
                    self.python.presented = false;
                }
            }
            // the one Install button runs reinstall() under the
            // not-installing guard (PythonEnvironment.install:55,
            // PythonSetupSheet.swift:78) — the production seams drive it
            sheets::PythonMessage::Install => {
                if !matches!(self.python.state.state, EnvState::Installing) {
                    self.python.state.begin_installing();
                    self.sync_python_state();
                    return python_events(reinstall(
                        &venv_dir(),
                        &streaming_spawn(),
                        &command_probe(),
                    ));
                }
            }
            sheets::PythonMessage::ToggleLog => self.python.shows_log = !self.python.shows_log,
            sheets::PythonMessage::Event(event) => {
                // which driver spoke: only a check kick ever leaves
                // Checking behind (installs leave Installing)
                let from_check = matches!(self.python.state.state, EnvState::Checking);
                let terminal = matches!(event, EnvEvent::Finished(_));
                self.python.state.apply(&event);
                self.sync_python_state();
                // upstream check()'s presentation (PythonEnvironment.swift:38,
                // 45): a failed or missing CHECK terminal presents the
                // sheet, gated by hasCompletedSetup — the port's flag is
                // the cosmic-config first-run key (the engine settings
                // field of that name stays dead by the single-writer law,
                // recents.rs)
                if from_check
                    && terminal
                    && !matches!(self.python.state.state, EnvState::Ready { .. })
                    && !recents::needs_setup(&self.config)
                {
                    self.python.presented = true;
                }
            }
        }
        Task::none()
    }

    /// Push the python state into every open project's bridge (upstream's
    /// `PythonEnvironment.shared` singleton: one environment, every
    /// Project reads it; here each bridge holds a copy the app mirrors on
    /// every change).
    fn sync_python_state(&mut self) {
        let state = self.python.state.state.clone();
        for window in self.windows.values_mut() {
            if let WindowKind::Project(project) = window {
                project.bridge.set_python_state(state.clone());
            }
        }
    }

    /// Settings for a `window::open` secondary window. libcosmic seeds only
    /// the MAIN window's platform identity: its `app::run` copies
    /// `Application::APP_ID` into the main window's Wayland app_id / X11
    /// WM_CLASS, while every window opened through `window::open` defaults
    /// to an EMPTY `application_id` (iced hands each window's own settings
    /// to winit verbatim). Without this, the project/settings/About windows
    /// report no class and the dock splits them from the welcome window's
    /// group; seeding APP_ID here makes every window report the class the
    /// desktop entry's `StartupWMClass` names (packaging under
    /// `app/resources/packaging/`).
    fn window_settings(
        size: cosmic::iced::Size,
        min_size: Option<cosmic::iced::Size>,
    ) -> window::Settings {
        window::Settings {
            size,
            min_size,
            // The client-side decoration setup libcosmic gives the main
            // window (`app::run` with the default `client_decorations` and
            // `transparent` settings): the app draws its own header and
            // window controls, so the compositor adds no frame, and the
            // surface is transparent so the rounded corners show the
            // desktop instead of a black fill.
            decorations: false,
            transparent: true,
            #[cfg(target_os = "linux")]
            platform_specific: window::PlatformSpecific {
                application_id: Self::APP_ID.to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // ─── the settings + About windows ──────────────────────────────────────

    /// Open (or focus) the settings window at `tab` (upstream's ⌘,
    /// Settings scene — `Settings { SettingsView() }`,
    /// CADStudioApp.swift:31-33). A fresh open seeds the mirror from the
    /// current settings and kicks BOTH engines' locates — upstream's
    /// per-section auto-locate on appear (SettingsView.swift:163-167),
    /// run eagerly so whichever section shows is already resolving.
    fn open_settings(&mut self, tab: settings::Tab) -> Task<Action<Message>> {
        if let Some((id, WindowKind::Settings(state))) = self
            .windows
            .iter_mut()
            .find(|(_, kind)| matches!(kind, WindowKind::Settings(_)))
        {
            let before = state.height();
            state.update(&settings::Message::Tab(tab));
            let focus = *id;
            let resize = settings_resize(focus, before, state.height());
            let title = self.set_window_title(tab.title(), focus);
            return Task::batch([resize, title, window::gain_focus(focus).map(Action::App)]);
        }
        let settings = self.settings.load();
        let mut state = SettingsState::new(&settings);
        state.update(&settings::Message::Tab(tab));
        let (id, spawn) = window::open(Self::window_settings(
            cosmic::iced::Size::new(settings::WIDTH, state.height()),
            None,
        ));
        self.windows.insert(id, WindowKind::Settings(state));
        // the window title follows the selected tab, as the header does
        let title = self.set_window_title(tab.title(), id);
        let locates = settings::ENGINES.map(|kind| {
            let custom = settings.agent_path(kind).map(str::to_string);
            Task::future(locate_agent_path(kind, custom))
                .map(move |found| Action::App(Message::SettingsLocated(kind, found)))
        });
        Task::batch(
            [spawn.map(|_id| Action::App(Message::Noop)), title]
                .into_iter()
                .chain(locates),
        )
    }

    /// Open (or focus) the About window (upstream's app-info command).
    fn open_about(&mut self) -> Task<Action<Message>> {
        if let Some((id, _)) = self
            .windows
            .iter()
            .find(|(_, kind)| matches!(kind, WindowKind::About))
        {
            let focus = *id;
            return window::gain_focus(focus).map(Action::App);
        }
        let (id, spawn) = window::open(Self::window_settings(
            cosmic::iced::Size::new(settings::ABOUT_WIDTH, settings::ABOUT_HEIGHT),
            None,
        ));
        self.windows.insert(id, WindowKind::About);
        let title = self.set_window_title(fl!("informazioni-su-cad-studio"), id);
        Task::batch([spawn.map(|_id| Action::App(Message::Noop)), title])
    }

    /// One settings-window message: the write (load-modify-save, the
    /// ONE-writer law), the mirror fold on success, then the effects.
    fn update_settings(&mut self, id: window::Id, msg: settings::Message) -> Task<Action<Message>> {
        if !matches!(self.windows.get(&id), Some(WindowKind::Settings(_))) {
            return Task::none();
        }
        let before = self.settings_height();
        let task = self.update_settings_any(msg.clone());
        let resize = match (before, self.settings_height()) {
            (Some(before), Some(after)) => settings_resize(id, before, after),
            _ => Task::none(),
        };
        let title = match msg {
            settings::Message::Tab(tab) => self.set_window_title(tab.title(), id),
            _ => Task::none(),
        };
        Task::batch([task, resize, title])
    }

    /// The open settings window's content height, if one is open.
    fn settings_height(&self) -> Option<f32> {
        self.windows.values().find_map(|window| match window {
            WindowKind::Settings(state) => Some(state.height()),
            _ => None,
        })
    }

    /// Fold one message into the open settings window's mirror (there is
    /// at most one — open-or-focus).
    fn fold_settings_state(&mut self, msg: &settings::Message) {
        for window in self.windows.values_mut() {
            if let WindowKind::Settings(state) = window {
                state.update(msg);
            }
        }
    }

    /// Re-seed the open settings window's mirror from the settings file, so
    /// a write made elsewhere (the agent sheet's path) or a refused one
    /// leaves the window showing what is persisted.
    fn reseed_settings_windows(&mut self) {
        let fresh = self.settings.load();
        for window in self.windows.values_mut() {
            if let WindowKind::Settings(state) = window {
                state.reseed(&fresh);
            }
        }
    }

    /// The write, the mirror fold and the effects of one settings message,
    /// against whichever settings window is open (there is at most one; the
    /// portal's answer arrives here when the window that asked has since
    /// closed).
    fn update_settings_any(&mut self, msg: settings::Message) -> Task<Action<Message>> {
        // the write: engine settings.json only, through the one store; an
        // effect message (a tab switch, a locate) writes nothing. The
        // mirror takes the message only when the write landed; a refused
        // save re-seeds it from disk instead of showing an unsaved value.
        if settings::persists(&msg) {
            let saved = self
                .settings
                .update(|persisted| {
                    settings::stage(&msg, persisted);
                })
                .is_ok();
            if saved {
                self.push_settings_to_windows();
                self.fold_settings_state(&msg);
            } else {
                self.reseed_settings_windows();
            }
        } else {
            self.fold_settings_state(&msg);
        }
        match msg {
            settings::Message::ChooseLocation(starting) => {
                let title = fl!("scegli-la-cartella-del-progetto-da-aprire");
                Task::future(async move {
                    let future = choose_folder_at(&title, Some(&starting));
                    Message::SettingsLocationChosen(future.await)
                })
                .map(Action::App)
            }
            settings::Message::Verify(kind) => {
                let custom = self.settings.load().agent_path(kind).map(str::to_string);
                Task::future(locate_agent_path(kind, custom))
                    .map(move |found| Action::App(Message::SettingsLocated(kind, found)))
            }
            // the Python tab's Verifica: the environment check re-run —
            // the manual retry a failed startup check lacked (its
            // follow-up's close). A failed terminal presents the sheet
            // over the project windows exactly like the startup one
            // (PythonEnvironment.swift:38,45 — every check presents).
            settings::Message::PythonCheck => {
                self.python.state.begin_checking();
                python_events(check(&venv_dir(), &streaming_spawn()))
            }
            // Reinstalla…: the sheet over the project windows + the
            // reinstall kick (SettingsView.swift:201-204 — upstream sets
            // the flag then reinstalls; the install guard refusals while
            // one already runs)
            settings::Message::PythonReinstall => {
                self.python.presented = true;
                self.update_python_sheet(sheets::PythonMessage::Install)
            }
            // the write above (or the window's own fold) is the whole effect
            settings::Message::Tab(_)
            | settings::Message::LocationChosen(_)
            | settings::Message::Language(_)
            | settings::Message::PrintPref(_)
            | settings::Message::Engine(_)
            | settings::Message::Located(_, _)
            | settings::Message::CustomPath(_, _)
            | settings::Message::Model(_, _)
            | settings::Message::Effort(_)
            | settings::Message::RestrictedBash(_)
            | settings::Message::Streaming(_)
            | settings::Message::Isolates(_) => Task::none(),
        }
    }

    /// A python or agent-missing sheet is up (both present over the
    /// project and welcome bodies).
    fn modal_sheet_up(&self) -> bool {
        self.python.presented || self.agent_missing.is_some()
    }

    /// What Return / Esc does on the sheet topmost over window `id` — the
    /// `.keyboardShortcut(.defaultAction / .cancelAction)` buttons, which
    /// do nothing while disabled. `None`: no sheet there, or the sheet has
    /// no enabled button for that role.
    fn sheet_key_message(&self, id: window::Id, role: SheetKey) -> Option<Message> {
        match self.windows.get(&id)? {
            WindowKind::Welcome => {
                // the first-run sheet replaces the whole body: Continua is
                // its only, default, button (SetupSheet.swift:31-35)
                if self.setup.is_some() {
                    return (role == SheetKey::Default)
                        .then_some(Message::SetupSheet(sheets::SetupMessage::Continue));
                }
                if self.modal_sheet_up() {
                    return self.modal_sheet_key(role);
                }
                // NewProjectSheet.swift:34-39: Annulla cancels, Avanti is
                // the default and is disabled for an invalid name
                let sheet = self.welcome.creating.as_ref()?;
                let msg = match role {
                    SheetKey::Cancel => welcome::new_project::Message::Cancel,
                    SheetKey::Default
                        if sheet.validation_error().is_none() && !sheet.awaiting_parent =>
                    {
                        welcome::new_project::Message::Advance
                    }
                    SheetKey::Default => return None,
                };
                Some(Message::Welcome(welcome::Message::NewSheet(msg)))
            }
            WindowKind::Project(_) => self.modal_sheet_key(role),
            WindowKind::Settings(_) | WindowKind::About | WindowKind::LoadError(_) => None,
        }
    }

    /// Return / Esc on the python sheet (over the agent sheet, as
    /// `apply_sheets` stacks them).
    fn modal_sheet_key(&self, role: SheetKey) -> Option<Message> {
        if self.python.presented {
            // PythonSetupSheet.swift:62-82: Fine (default) when ready;
            // Più tardi (cancel) + Installa (default) otherwise; nothing
            // while installing or checking
            let msg = match (sheets::buttons(&self.python.state.state), role) {
                (sheets::SetupButtons::Done, SheetKey::Default) => sheets::PythonMessage::Close,
                (sheets::SetupButtons::LaterAndInstall, SheetKey::Default) => {
                    sheets::PythonMessage::Install
                }
                (sheets::SetupButtons::LaterAndInstall, SheetKey::Cancel) => {
                    sheets::PythonMessage::Close
                }
                _ => return None,
            };
            return Some(Message::PythonSheet(msg));
        }
        // AgentMissingSheet.swift:56-69: Chiudi (cancel), Verifica
        // (default, disabled while checking)
        let sheet = self.agent_missing.as_ref()?;
        let msg = match role {
            SheetKey::Cancel => sheets::AgentMessage::Close,
            SheetKey::Default if !sheet.checking => sheets::AgentMessage::Verify,
            SheetKey::Default => return None,
        };
        Some(Message::AgentSheet(msg))
    }

    /// The settings that live in project windows reach every open project:
    /// the measurement unit goes to the viewer (upstream ModelPane reads
    /// `@AppStorage(measurementUnitKey)`, so a Settings change re-renders
    /// the size label and measure badge live, ModelPane.swift:7), and the
    /// engine goes to the chat's empty state.
    fn push_settings_to_windows(&mut self) {
        let unit = self.settings.load().measurement_unit;
        for window in self.windows.values_mut() {
            if let WindowKind::Project(project) = window {
                project.viewer.unit = unit;
                project.refresh_engine();
            }
        }
    }

    /// Route an unconsumed key press: app-level shortcuts act globally,
    /// project commands go to the window the key landed in.
    fn update_key_press(
        &mut self,
        id: window::Id,
        key: Key,
        modifiers: Modifiers,
    ) -> Task<Action<Message>> {
        if let Some(role) = sheet_key_role(&key, &modifiers) {
            if let Some(msg) = self.sheet_key_message(id, role) {
                return self.update(msg);
            }
        }
        if let Some(command) = app_shortcut(&key, &modifiers) {
            return self.app_command(id, command);
        }
        let context = match self.windows.get(&id) {
            Some(WindowKind::Project(project)) => project.key_context(),
            _ => Context::default(),
        };
        if let Some(command) = project::project_shortcut_in(&key, &modifiers, context) {
            return self.update_project(id, project::Message::Command(command));
        }
        // anything else is the key window's pane's (Space, Esc)
        if matches!(self.windows.get(&id), Some(WindowKind::Project(_))) {
            return self.update_project(
                id,
                project::Message::Viewer(crate::viewer_pane::Message::Key(key, modifiers)),
            );
        }
        Task::none()
    }

    /// A modifier change reaches the pane of the window it landed in, the
    /// pan-vs-zoom gate for that window's scroll and primary drag.
    fn update_modifiers(&mut self, id: window::Id, modifiers: Modifiers) -> Task<Action<Message>> {
        if !matches!(self.windows.get(&id), Some(WindowKind::Project(_))) {
            return Task::none();
        }
        self.update_project(
            id,
            project::Message::Viewer(crate::viewer_pane::Message::ModifiersChanged(modifiers)),
        )
    }

    /// Open (or focus) a project window (upstream `openWindow(value:)`):
    /// load the project, register the window, add the recent, dismiss the
    /// welcome window.
    fn open_project(&mut self, root: PathBuf) -> Task<Action<Message>> {
        // already open? focus it
        for (id, kind) in &self.windows {
            if let WindowKind::Project(project) = kind {
                if project.root == root {
                    let focus = *id;
                    return window::gain_focus(focus).map(Action::App);
                }
            }
        }
        if let Err(why) = store::prepare(&root) {
            // ProjectWindow.swift:16-22, 42-44: the window opens on the
            // "Impossibile aprire il progetto" view carrying the error; no
            // recent is added and the welcome window stays
            eprintln!("cad-studio: cannot open {}: {why}", root.display());
            return self.open_load_error(crate::engine_text::project_store_error(&why));
        }
        let (window, bridge_task) = ProjectWindow::new(root.clone(), &self.settings);
        let name = window.name.clone();
        let (id, spawn) = window::open(Self::window_settings(
            cosmic::iced::Size::new(project::DEFAULT_SIZE.0, project::DEFAULT_SIZE.1),
            Some(cosmic::iced::Size::new(
                project::MIN_SIZE.0,
                project::MIN_SIZE.1,
            )),
        ));
        self.windows.insert(id, WindowKind::Project(window));
        // the window inherits the app-level python state (a check that
        // finished before the window opened — upstream's shared singleton)
        if let Some(WindowKind::Project(project)) = self.windows.get_mut(&id) {
            project
                .bridge
                .set_python_state(self.python.state.state.clone());
        }
        self.recents.add(&root);

        let title = self.set_window_title(name, id);
        let dismiss = self.dismiss_welcome();
        Task::batch([
            spawn.map(|_id| Action::App(Message::Noop)),
            title,
            dismiss,
            bridge_task
                .map(move |m| Message::Project(id, project::Message::Bridge(m)))
                .map(Action::App),
        ])
    }

    /// A project window whose load failed (ProjectWindow.swift:16-22): a
    /// 480x320 window showing the failure and the error text.
    fn open_load_error(&mut self, error: String) -> Task<Action<Message>> {
        let (id, spawn) = window::open(Self::window_settings(
            cosmic::iced::Size::new(LOAD_ERROR_SIZE.0, LOAD_ERROR_SIZE.1),
            Some(cosmic::iced::Size::new(
                LOAD_ERROR_SIZE.0,
                LOAD_ERROR_SIZE.1,
            )),
        ));
        self.windows.insert(id, WindowKind::LoadError(error));
        let title = self.set_window_title(fl!("cad-studio"), id);
        Task::batch([spawn.map(|_id| Action::App(Message::Noop)), title])
    }

    /// Close the welcome window if it is open, moving the main-window
    /// pointer to a surviving project window first (ProjectWindow.swift:41
    /// `dismissWindow(id: WelcomeWindow.id)`).
    fn dismiss_welcome(&mut self) -> Task<Action<Message>> {
        let Some(welcome_id) = self
            .windows
            .iter()
            .find(|(_, k)| matches!(k, WindowKind::Welcome))
            .map(|(id, _)| *id)
        else {
            return Task::none();
        };
        self.move_main_off(welcome_id);
        window::close(welcome_id).map(Action::App)
    }

    /// Open (or focus) the welcome window (the ⇧⌘1 command).
    fn show_welcome(&mut self) -> Task<Action<Message>> {
        if let Some((welcome_id, _)) = self
            .windows
            .iter()
            .find(|(_, k)| matches!(k, WindowKind::Welcome))
        {
            let focus = *welcome_id;
            return window::gain_focus(focus).map(Action::App);
        }
        let (id, spawn) = window::open(Self::window_settings(
            cosmic::iced::Size::new(
                welcome::ACTIONS_WIDTH + welcome::RECENTS_WIDTH,
                welcome::WINDOW_HEIGHT,
            ),
            None,
        ));
        self.windows.insert(id, WindowKind::Welcome);
        let title = self.set_window_title(fl!("benvenuto-in-cad-studio"), id);
        Task::batch([spawn.map(|_id| Action::App(Message::Noop)), title])
    }

    fn close_window(&mut self, id: window::Id) -> Task<Action<Message>> {
        self.move_main_off(id);
        window::close(id).map(Action::App)
    }

    /// Before `id` closes: if it is the main window, point main at any
    /// surviving window so the close does not exit the app.
    fn move_main_off(&mut self, id: window::Id) {
        if self.core.main_window_id() == Some(id) {
            let survivor = self.windows.keys().copied().find(|other| *other != id);
            self.core.set_main_window_id(survivor);
        }
    }

    /// A window's close finished: forget it; the last window closing exits.
    fn window_closed(&mut self, id: window::Id) -> Task<Action<Message>> {
        self.windows.remove(&id);
        if self.windows.is_empty() {
            engine::processes::terminate_all();
            return cosmic::iced::exit();
        }
        Task::none()
    }

    /// Wrap a window's body with the app-level sheets (python setup, then
    /// agent-missing; ProjectWindow.swift:59-68 stacks python over
    /// agent-missing), each presented over the dimmed body like every
    /// other sheet ([`sheets::present`]): the body takes no presses while
    /// one is up, and the window chrome stays live.
    fn apply_sheets<'a>(&'a self, body: Element<'a, Message>) -> Element<'a, Message> {
        let card: Element<'_, Message> = if self.python.presented {
            sheets::view_python_setup(&self.python).map(Message::PythonSheet)
        } else if let Some(sheet) = &self.agent_missing {
            sheets::view_agent_missing(sheet).map(Message::AgentSheet)
        } else {
            return body;
        };
        sheets::present(body, card, Message::Noop)
    }

    /// The welcome window's chrome (WelcomeWindow.swift: a full-size
    /// content view with no title row): the body fills the window, the
    /// close control floats over its top corner, and any press on the body
    /// that no control captures drags the window (right-click: the window
    /// menu).
    fn decorate_borderless<'a>(
        &self,
        id: window::Id,
        focused: bool,
        body: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let close = cosmic::widget::button::icon(
            cosmic::widget::icon::from_name("window-close-symbolic").size(16),
        )
        .padding(8)
        .class(cosmic::theme::Button::HeaderBar)
        .selected(focused)
        .on_press(Message::CloseWindow(id));
        let controls = container(close)
            .width(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Right)
            .padding([7, 7]);
        let draggable =
            cosmic::widget::mouse_area(container(body).width(Length::Fill).height(Length::Fill))
                .on_drag(Message::DragWindow(id))
                .on_right_press(Message::WindowMenu(id));
        container(
            cosmic::iced::widget::Stack::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .push(draggable)
                .push(controls),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .class(cosmic::theme::Container::WindowBackground)
        .into()
    }

    /// Give a window its CSD header bar and background.
    fn decorate<'a>(
        &self,
        id: window::Id,
        focused: bool,
        body: Element<'a, Message>,
        title: &str,
    ) -> Element<'a, Message> {
        let _ = id;
        cosmic::widget::container(
            cosmic::widget::column::with_capacity(2)
                .push(
                    cosmic::widget::header_bar()
                        .title(title.to_string())
                        .focused(focused)
                        .on_close(Message::CloseWindow(id))
                        // libcosmic's shell wires these on its own header
                        // bars (src/app/mod.rs:782-784); the port's custom
                        // decorate must wire them itself or the window
                        // cannot be moved, maximized, or menu'd.
                        .on_drag(Message::DragWindow(id))
                        .on_double_click(Message::ToggleMaximize(id))
                        .on_right_click(Message::WindowMenu(id)),
                )
                .push(body),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .class(cosmic::theme::Container::WindowBackground)
        .into()
    }
}

/// The load-error window body (ProjectWindow.swift:16-22's
/// ContentUnavailableView): the warning glyph, "Impossibile aprire il
/// progetto", and the error text, centered.
fn view_load_error(error: &str) -> Element<'_, Message> {
    let glyph =
        cosmic::iced::widget::svg(cosmic::iced::widget::svg::Handle::from_memory(ICON_WARNING))
            .width(Length::Fixed(40.0))
            .height(Length::Fixed(40.0))
            // the glyph inks in the theme's on-background color (an
            // uncolored svg draws black in dark mode)
            .class(cosmic::theme::Svg::custom(|theme| {
                cosmic::iced::widget::svg::Style {
                    color: Some(theme.cosmic().background(false).on.into()),
                }
            }));
    container(
        Column::new()
            .spacing(8)
            .push(glyph)
            .push(
                text(fl!("impossibile-aprire-il-progetto"))
                    .size(crate::typography::TITLE)
                    .font(crate::typography::font(cosmic::iced::font::Weight::Bold)),
            )
            .push(
                text(error)
                    .size(crate::typography::BODY)
                    .align_x(cosmic::iced::alignment::Horizontal::Center),
            )
            .align_x(cosmic::iced::alignment::Alignment::Center),
    )
    .padding(20)
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(cosmic::iced::alignment::Horizontal::Center)
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}

/// A sheet button's keyboard role (SwiftUI `.keyboardShortcut`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetKey {
    /// `.defaultAction`: Return.
    Default,
    /// `.cancelAction`: Esc.
    Cancel,
}

/// Bare Return / Esc as a sheet role; any command modifier makes it a
/// different shortcut.
pub fn sheet_key_role(key: &Key, modifiers: &Modifiers) -> Option<SheetKey> {
    use cosmic::iced::keyboard::key::Named;
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key {
        Key::Named(Named::Enter) => Some(SheetKey::Default),
        Key::Named(Named::Escape) => Some(SheetKey::Cancel),
        _ => None,
    }
}

/// Resize settings window `id` when its tab or engine changed the height
/// its content calls for (upstream's per-tab window size).
fn settings_resize(id: window::Id, before: f32, after: f32) -> Task<Action<Message>> {
    if before == after {
        return Task::none();
    }
    window::resize(id, cosmic::iced::Size::new(settings::WIDTH, after))
}

/// Drop keyboard focus from every focusable widget.
fn release_focus() -> Task<Action<Message>> {
    cosmic::iced::advanced::widget::operate(
        cosmic::iced::advanced::widget::operation::focusable::unfocus::<()>(),
    )
    .discard()
}

/// The app-level bindings (upstream's global commands; ⌘→Ctrl, ⌘, the
/// standard Settings scene's binding).
const APP_BINDINGS: &[(Shortcut, AppCommand)] = &[
    (Shortcut::ctrl("n"), AppCommand::NewProject),
    (Shortcut::ctrl("o"), AppCommand::OpenProject),
    (Shortcut::ctrl("1").with_shift(), AppCommand::Welcome),
    (Shortcut::ctrl(","), AppCommand::Settings),
    (Shortcut::ctrl("w"), AppCommand::CloseWindow),
    (Shortcut::ctrl("q"), AppCommand::Quit),
    (Shortcut::bare("F11"), AppCommand::ToggleFullscreen),
];

/// The app command a key press triggers.
pub fn app_shortcut(key: &Key, modifiers: &Modifiers) -> Option<AppCommand> {
    APP_BINDINGS
        .iter()
        .find(|(shortcut, _)| shortcut.matches(key, modifiers))
        .map(|(_, command)| *command)
}

/// Route the shell-level events the focused window's widgets did not
/// consume, tagged with the window they landed in: unconsumed key presses
/// (the shortcut routers), modifier changes (that window's pan gate, reset
/// when the window loses focus) and window-level file drops.
fn key_events(
    event: cosmic::iced::Event,
    status: cosmic::iced::event::Status,
    id: window::Id,
) -> Option<Message> {
    match event {
        cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
            key,
            modifiers,
            ..
        }) => {
            if matches!(status, cosmic::iced::event::Status::Captured) {
                return None;
            }
            Some(Message::KeyPressed(id, key, modifiers))
        }
        // modifier state is physical: it follows the keys whatever a widget
        // did with the press, so capture status does not apply
        cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::ModifiersChanged(
            modifiers,
        )) => Some(Message::ModifiersChanged(id, modifiers)),
        // a window that loses focus no longer sees the keys, so whatever
        // it last held is released rather than left to stick
        cosmic::iced::Event::Window(cosmic::iced::window::Event::Unfocused) => {
            Some(Message::ModifiersChanged(id, Modifiers::empty()))
        }
        // the drop is not a widget event on this surface — capture status
        // does not apply
        cosmic::iced::Event::Window(cosmic::iced::window::Event::FileDropped(paths)) => {
            Some(Message::FilesDropped(id, paths))
        }
        _ => None,
    }
}

/// Locate the agent off the UI thread (upstream `AgentLocator.locate()`,
/// through `AgentLocator.current`, which picks the engine by the setting):
/// the settings' custom path for THAT engine, then PATH, then the fixed
/// candidates, each validated by `--version`. The settings tab's rows and
/// Verifica consume the located path + version; the startup/agent-sheet
/// checks only need the verdict.
async fn locate_agent_path(
    kind: AgentKind,
    custom_path: Option<String>,
) -> Option<locator::AgentPath> {
    tokio::task::spawn_blocking(move || locator::find_agent(kind, custom_path.as_deref()))
        .await
        .unwrap_or(None)
}

/// [`locate_agent_path`] as the sheets' boolean verdict.
async fn locate_agent(kind: AgentKind, custom_path: Option<String>) -> bool {
    locate_agent_path(kind, custom_path).await.is_some()
}

/// Stream one python driver's events to the app as sheet messages — every
/// [`EnvEvent`] in arrival order (the drain law), so the log and the
/// progress bar move live through an install.
fn python_events(handle: EnvHandle) -> Task<Action<Message>> {
    use cosmic::iced::futures::SinkExt;
    Task::stream(cosmic::iced::stream::channel(
        16,
        move |mut output: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
            let EnvHandle { mut events } = handle;
            while let Some(event) = events.recv().await {
                // the terminal must land: a dropped Finished would leave
                // the sheet stuck Installing, so it sends awaited
                // (backpressure, the Sink trait) where the streaming body
                // merely tries
                let terminal = matches!(event, EnvEvent::Finished(_));
                let message = Message::PythonSheet(sheets::PythonMessage::Event(event));
                if terminal {
                    let _ = output.send(message).await;
                } else {
                    let _ = output.try_send(message);
                }
            }
        },
    ))
    .map(Action::App)
}

/// The SIGTERM watcher (CADStudioApp.swift:42-49's dispatch source).
fn sigterm_stream() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(
        1,
        |output: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
            let Ok(mut signal) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            else {
                std::future::pending::<()>().await;
                return;
            };
            let mut sender = output.clone();
            while signal.recv().await.is_some() {
                let _ = sender.try_send(Message::SigTerm);
            }
            std::future::pending::<()>().await;
        },
    )
}

/// Launch the user's terminal (AgentMissingSheet's "Apri il Terminale";
/// upstream opens Terminal.app — the port takes the first terminal in
/// [`sheets::TERMINAL_PREFERENCE`] the environment can run).
pub fn launch_terminal() {
    let env = locator::shell_env();
    if let Some(terminal) = sheets::terminal_launcher(&env) {
        let _ = std::process::Command::new(&terminal).spawn();
    }
}

// ─── platform integrations (the app layer owns all I/O the windows need) ──

/// Ask the xdg-desktop-portal for one directory (upstream
/// `NSOpenPanel`/`chooseParentFolder`), optionally starting somewhere
/// (upstream `chooseFolder(startingAt:)`, SettingsView.swift:34).
pub async fn choose_folder_at(
    title: &str,
    starting_at: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let mut request = SelectedFiles::open_file().title(title).directory(true);
    if let Some(folder) = starting_at {
        request = request
            .current_folder(folder.to_path_buf())
            .map_err(|why| why.to_string())?;
    }
    let response = request.send().await.map_err(|why| why.to_string())?;
    let files = response.response().map_err(|why| why.to_string())?;
    Ok(files
        .uris()
        .iter()
        .filter_map(|uri| uri.to_file_path().ok())
        .collect())
}

/// [`choose_folder_at`] with no starting directory.
/// The open-existing folder portal; its answer comes back as
/// [`Message::OpenFolderChosen`].
fn choose_existing() -> Task<Action<Message>> {
    let title = fl!("scegli-la-cartella-del-progetto-da-aprire");
    Task::future(async move {
        let future = choose_folder(&title);
        Message::OpenFolderChosen(future.await)
    })
    .map(Action::App)
}

pub async fn choose_folder(title: &str) -> Result<Vec<PathBuf>, String> {
    choose_folder_at(title, None).await
}

/// Ask the xdg-desktop-portal for files (the chat attach button).
pub async fn choose_files(title: &str) -> Result<Vec<PathBuf>, String> {
    let request = SelectedFiles::open_file().title(title).multiple(true);
    let response = request.send().await.map_err(|why| why.to_string())?;
    let files = response.response().map_err(|why| why.to_string())?;
    Ok(files
        .uris()
        .iter()
        .filter_map(|uri| uri.to_file_path().ok())
        .collect())
}

/// Ask the xdg-desktop-portal where to save the export (upstream
/// `chooseExportDestination`, ProjectPanels.swift:24-33): suggested name
/// `"<project>.zip"`, starting in Downloads, ZIP-only filter, the accept
/// button is upstream's "Esporta" prompt. `Err` is a cancelled panel.
pub async fn choose_save_file(suggested_name: &str) -> Result<PathBuf, String> {
    let title = fl!("esporta-pacchetto-2");
    let accept = fl!("esporta");
    let current_name = format!("{suggested_name}.zip");
    let mut request = SelectedFiles::save_file()
        .title(title.as_str())
        .accept_label(accept.as_str())
        .current_name(current_name.as_str())
        .filter(FileFilter::new("ZIP").mimetype("application/zip"));
    if let Some(downloads) = dirs::download_dir() {
        request = request
            .current_folder(downloads)
            .map_err(|why| why.to_string())?;
    }
    let response = request.send().await.map_err(|why| why.to_string())?;
    let files = response.response().map_err(|why| why.to_string())?;
    files
        .uris()
        .first()
        .and_then(|uri| uri.to_file_path().ok())
        .ok_or_else(|| "no destination".to_string())
}

/// Run upstream's export subprocess — `/usr/bin/zip -j -q <dest>
/// <files…>` (Project.swift:444, argv only, never a shell) — after
/// REMOVING a pre-existing destination: `zip -j` into an existing archive
/// adds or updates entries, leaving stale ones behind. Status 0 →
/// `Some(destination)` for the reveal; anything else — zip not installed
/// included — `None` (the caller's "Esportazione non riuscita"). The
/// removal takes files only: upstream's removeItem would also rmtree a
/// directory at the destination, and a portal-confirmed overwrite only
/// ever names a file.
pub async fn zip_package(destination: PathBuf, files: Vec<PathBuf>) -> Option<PathBuf> {
    let _ = std::fs::remove_file(&destination);
    let status = tokio::process::Command::new("/usr/bin/zip")
        .arg("-j")
        .arg("-q")
        .arg(&destination)
        .args(&files)
        .status()
        .await;
    match status {
        Ok(status) if status.success() => Some(destination),
        _ => None,
    }
}

/// Reveal `path` in the file manager: DBus `org.freedesktop.FileManager1`
/// `ShowItems` (upstream `NSWorkspace.activateFileViewerSelecting`), with
/// an `xdg-open` of the parent directory as the fallback.
pub async fn reveal_path(path: PathBuf) {
    use zbus::Proxy;
    let uri = format!("file://{}", path.display());
    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(_) => return open_parent(&path),
    };
    let proxy = Proxy::new(
        &connection,
        "org.freedesktop.FileManager1",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1",
    )
    .await;
    let shown = match proxy {
        Ok(proxy) => proxy
            .call_method("ShowItems", &(vec![uri], ""))
            .await
            .is_ok(),
        Err(_) => false,
    };
    if !shown {
        open_parent(&path);
    }
}

fn open_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        open_path(parent);
    }
}

/// Open `path` with the system opener (upstream `NSWorkspace.open`): the
/// slicer route for 3MF files.
pub fn open_path(path: &Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Open a URL with the system opener (upstream `NSWorkspace.open` for a
/// tapped markdown link).
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Secondary windows draw their own header and window controls, as the
    /// main window does: no compositor frame, transparent surface.
    #[test]
    fn secondary_windows_are_client_decorated_and_transparent() {
        let settings = CadStudio::window_settings(cosmic::iced::Size::new(800.0, 600.0), None);
        assert!(
            !settings.decorations,
            "the compositor would draw a second frame"
        );
        assert!(settings.transparent, "rounded corners would fill black");
    }

    /// `zip_package` rebuilds the destination from scratch every run —
    /// `zip -j` into an existing archive would ADD or UPDATE entries,
    /// leaving the stale ones behind (Project.swift:444's removeItem).
    /// Re-exporting a different file set must leave exactly the new set:
    /// the first round's entry may not survive the second round. Skipped
    /// where `/usr/bin/zip` is not installed (the failed-activity path is
    /// that machine's reality).
    #[tokio::test]
    async fn zip_package_replaces_a_pre_existing_destination() {
        if !Path::new("/usr/bin/zip").exists() {
            eprintln!("skipping: /usr/bin/zip not installed");
            return;
        }
        let dir = std::env::temp_dir().join(format!("cad-zip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("one.txt"), b"one").unwrap();
        std::fs::write(dir.join("two.txt"), b"two").unwrap();
        let destination = dir.join("package.zip");

        let first = zip_package(destination.clone(), vec![dir.join("one.txt")])
            .await
            .expect("the first export zips");
        assert_eq!(first, destination);
        let second = zip_package(destination.clone(), vec![dir.join("two.txt")])
            .await
            .expect("the second export zips over the first");
        assert_eq!(second, destination);

        let archive = std::fs::read(&destination).unwrap();
        let mut names: Vec<String> = zip::ZipArchive::new(std::io::Cursor::new(archive))
            .unwrap()
            .file_names()
            .map(str::to_string)
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["two.txt".to_string()],
            "the rebuilt archive carries exactly the second round's set — the stale one.txt is gone"
        );
    }

    /// The app-level bindings (Commands.swift:16-49): ⌘N, ⌘O, ⇧⌘1, ⌘,
    /// (the standard Settings scene, CADStudioApp.swift:31-33) — and
    /// nothing on bare keys.
    #[test]
    fn window_shortcuts_close_quit_and_fullscreen() {
        let ctrl = Modifiers::CTRL;
        assert_eq!(
            app_shortcut(&Key::Character("w".into()), &ctrl),
            Some(AppCommand::CloseWindow)
        );
        assert_eq!(
            app_shortcut(&Key::Character("q".into()), &ctrl),
            Some(AppCommand::Quit)
        );
        assert_eq!(
            app_shortcut(
                &Key::Named(cosmic::iced::keyboard::key::Named::F11),
                &Modifiers::empty()
            ),
            Some(AppCommand::ToggleFullscreen)
        );
    }

    /// A key a focused widget captured (typing "f" in the chat) never
    /// reaches the shortcut router.
    #[test]
    fn a_captured_key_is_not_routed() {
        let press = cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
            key: Key::Character("f".into()),
            modified_key: Key::Character("f".into()),
            physical_key: cosmic::iced::keyboard::key::Physical::Code(
                cosmic::iced::keyboard::key::Code::KeyF,
            ),
            location: cosmic::iced::keyboard::Location::Standard,
            modifiers: Modifiers::empty(),
            text: Some("f".into()),
            repeat: false,
        });
        let id = window::Id::unique();
        assert!(key_events(press.clone(), cosmic::iced::event::Status::Captured, id).is_none());
        assert!(key_events(press, cosmic::iced::event::Status::Ignored, id).is_some());
    }

    #[test]
    fn app_shortcut_table_matches_upstream() {
        let ctrl = Modifiers::CTRL;
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;

        assert_eq!(
            app_shortcut(&Key::Character("n".into()), &ctrl),
            Some(AppCommand::NewProject)
        );
        assert_eq!(
            app_shortcut(&Key::Character("o".into()), &ctrl),
            Some(AppCommand::OpenProject)
        );
        assert_eq!(
            app_shortcut(&Key::Character("1".into()), &ctrl_shift),
            Some(AppCommand::Welcome),
            "⇧⌘1 is the welcome window command"
        );
        assert_eq!(
            app_shortcut(&Key::Character(",".into()), &ctrl),
            Some(AppCommand::Settings),
            "⌘, opens the settings window"
        );
        assert_eq!(
            app_shortcut(&Key::Character("1".into()), &Modifiers::empty()),
            None
        );
        assert_eq!(app_shortcut(&Key::Character("x".into()), &ctrl), None);
        assert_eq!(
            app_shortcut(&Key::Character(",".into()), &Modifiers::empty()),
            None,
            "bare comma is just typing"
        );
    }
}

/// The grouping identity law: the app id, the icon lookups (sheets'
/// APP_ICON), and the desktop file's StartupWMClass/Icon must all be the
/// same string or COSMIC window grouping breaks silently. tests/ cannot
/// reach cosmic's types, so the pin lives here.
#[cfg(test)]
mod packaging_identity {
    use cosmic::Application;

    #[test]
    fn app_id_equals_icon_lookup_name() {
        assert_eq!(
            super::CadStudio::APP_ID,
            crate::sheets::APP_ICON,
            "APP_ID and the sheets' icon-lookup name drifted — grouping and icon resolution both break"
        );
    }
}

/// The shell's behaviors driven through `update` on a headless app: a
/// default `Core`, a temp settings file and cosmic-config dir, and the
/// welcome window registered under a fresh id.
#[cfg(test)]
mod shell_behavior {
    use super::*;
    use cosmic::iced::Size;
    use cosmic::iced::keyboard::key::Named;
    use engine::project::layout::PaneKind;
    use engine::settings::MeasurementUnit;

    struct Fixture {
        app: CadStudio,
        welcome: window::Id,
        dir: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = std::env::temp_dir().join(format!("cad-shell-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = Config::with_custom_path("cad-studio-tests", 1, dir.join("config")).unwrap();
        let welcome = window::Id::unique();
        let app = CadStudio {
            core: Core::default(),
            windows: HashMap::from([(welcome, WindowKind::Welcome)]),
            welcome: Welcome::new(),
            recents: Recents::load(config.clone()),
            config,
            settings: SettingsStore::at(dir.join("settings.json")),
            agent_missing: None,
            setup: None,
            python: sheets::PythonSetup::default(),
            drive_probe: None,
        };
        Fixture { app, welcome, dir }
    }

    fn key(app: &mut CadStudio, id: window::Id, named: Named) {
        let _ = app.update(Message::KeyPressed(
            id,
            Key::Named(named),
            Modifiers::empty(),
        ));
    }

    fn new_project_sheet(app: &mut CadStudio, name: &str) {
        let _ = app.update(Message::Welcome(welcome::Message::NewProject));
        let _ = app.update(Message::Welcome(welcome::Message::NewSheet(
            welcome::new_project::Message::Name(name.to_string()),
        )));
    }

    fn awaiting_parent(app: &CadStudio) -> bool {
        app.welcome
            .creating
            .as_ref()
            .expect("the sheet is up")
            .awaiting_parent
    }

    /// A real project on disk, opened through the app's own path.
    fn open_real_project(f: &mut Fixture, name: &str) -> window::Id {
        store::create_project(&f.dir, name).unwrap();
        let _ = f.app.open_project(f.dir.join(name));
        f.app
            .project_window()
            .expect("the project window registered")
    }

    /// Real projects on disk carrying the golden motion output, each opened
    /// through the app's own path; their window ids in `names` order.
    fn open_projects_with_motion<const N: usize>(
        f: &mut Fixture,
        names: [&str; N],
    ) -> Vec<window::Id> {
        names
            .into_iter()
            .map(|name| {
                store::create_project(&f.dir, name).unwrap();
                let root = f.dir.join(name);
                std::fs::create_dir_all(root.join("output")).unwrap();
                std::fs::copy(
                    concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../fixtures/golden/animation.json"
                    ),
                    root.join("output/animation.json"),
                )
                .unwrap();
                let _ = f.app.open_project(root.clone());
                f.app
                    .windows
                    .iter()
                    .find_map(|(id, kind)| match kind {
                        WindowKind::Project(p) if p.root == root => Some(*id),
                        _ => None,
                    })
                    .expect("the project window registered")
            })
            .collect()
    }

    /// Two project windows: a modifier change tagged with one window moves
    /// that window's pan gate only.
    #[tokio::test]
    async fn modifiers_reach_only_the_key_window() {
        let mut f = fixture();
        let ids = open_projects_with_motion(&mut f, ["A", "B"]);
        let pan = |app: &CadStudio, id| match app.windows.get(&id) {
            Some(WindowKind::Project(p)) => p.viewer.pan_modifiers(),
            _ => panic!("project window"),
        };
        assert!(!pan(&f.app, ids[0]) && !pan(&f.app, ids[1]));
        let _ = f
            .app
            .update(Message::ModifiersChanged(ids[0], Modifiers::SHIFT));
        assert!(pan(&f.app, ids[0]), "the key window's gate opened");
        assert!(!pan(&f.app, ids[1]), "the other window's gate stayed shut");
        let _ = f
            .app
            .update(Message::ModifiersChanged(ids[0], Modifiers::empty()));
        assert!(!pan(&f.app, ids[0]), "the release closed it again");
    }

    /// The event filter tags a modifier change with its window and forwards
    /// it whatever a widget did with the press.
    #[test]
    fn key_events_tags_modifier_changes_with_their_window() {
        use cosmic::iced::event::Status;
        let id = window::Id::unique();
        for status in [Status::Ignored, Status::Captured] {
            let event = cosmic::iced::Event::Keyboard(
                cosmic::iced::keyboard::Event::ModifiersChanged(Modifiers::ALT),
            );
            assert!(
                matches!(
                    key_events(event, status, id),
                    Some(Message::ModifiersChanged(w, m)) if w == id && m == Modifiers::ALT
                ),
                "{status:?}"
            );
        }
    }

    /// Two project windows: losing focus releases the modifiers that
    /// window held and leaves the other window's alone.
    #[tokio::test]
    async fn losing_focus_releases_the_window_modifiers() {
        let mut f = fixture();
        let ids = open_projects_with_motion(&mut f, ["A", "B"]);
        let pan = |app: &CadStudio, id| match app.windows.get(&id) {
            Some(WindowKind::Project(p)) => p.viewer.pan_modifiers(),
            _ => panic!("project window"),
        };
        for id in &ids {
            let _ = f.app.update(Message::ModifiersChanged(*id, Modifiers::ALT));
        }
        let unfocused = key_events(
            cosmic::iced::Event::Window(cosmic::iced::window::Event::Unfocused),
            cosmic::iced::event::Status::Ignored,
            ids[0],
        )
        .expect("focus loss routes a message");
        let _ = f.app.update(unfocused);
        assert!(!pan(&f.app, ids[0]), "the unfocused window released");
        assert!(pan(&f.app, ids[1]), "the other window kept its state");
    }

    /// Two project windows: an editor-unfocused Space tagged with one
    /// window toggles playback in that window's pane only.
    #[tokio::test]
    async fn space_toggles_playback_only_in_the_key_window() {
        use engine::watch::ReloadKind;
        let mut f = fixture();
        let ids = open_projects_with_motion(&mut f, ["A", "B"]);
        for id in &ids {
            let Some(WindowKind::Project(p)) = f.app.windows.get_mut(id) else {
                unreachable!()
            };
            let _ = p
                .viewer
                .update(crate::viewer_pane::Message::Reload(ReloadKind::Motion));
            let _ = p
                .viewer
                .update(crate::viewer_pane::Message::ToggleAnimation);
        }
        let playing = |app: &CadStudio, id| match app.windows.get(&id) {
            Some(WindowKind::Project(p)) => p.viewer.tools.player.is_playing,
            _ => panic!("project window"),
        };
        let before = (playing(&f.app, ids[0]), playing(&f.app, ids[1]));
        let _ = f.app.update(Message::KeyPressed(
            ids[0],
            Key::Character(" ".into()),
            Modifiers::empty(),
        ));
        assert_ne!(playing(&f.app, ids[0]), before.0, "the key window toggled");
        assert_eq!(
            playing(&f.app, ids[1]),
            before.1,
            "the other window did not"
        );
    }

    /// 2.17 / 11.2: the new-project parent panel opens at Settings'
    /// default project location (ProjectPanels.swift:20).
    #[test]
    fn the_parent_folder_panel_opens_at_the_default_project_location() {
        let f = fixture();
        let location = f.dir.join("Progetti");
        f.app
            .settings
            .update(|settings| settings.default_project_location = location.clone())
            .unwrap();
        let pick = f.app.parent_folder_pick("Staffa");
        assert_eq!(pick.start, Some(location));
        assert!(
            pick.title.contains("Staffa"),
            "titled for the name: {}",
            pick.title
        );
    }

    /// 2.15: Return (the field's on_submit and the window-level key) on
    /// an empty or invalid name asks for no folder (NewProjectSheet.swift:57
    /// `guard isNameValid`); a valid one does.
    #[test]
    fn return_on_an_invalid_project_name_does_not_advance() {
        let mut f = fixture();
        for bad in ["", "   ", ".hidden", "a/b", "a:b"] {
            new_project_sheet(&mut f.app, bad);
            let _ = f.app.update(Message::Welcome(welcome::Message::NewSheet(
                welcome::new_project::Message::Advance,
            )));
            assert!(!awaiting_parent(&f.app), "on_submit advanced on {bad:?}");
            key(&mut f.app, f.welcome, Named::Enter);
            assert!(!awaiting_parent(&f.app), "Return advanced on {bad:?}");
            let _ = f.app.update(Message::Welcome(welcome::Message::NewSheet(
                welcome::new_project::Message::Cancel,
            )));
        }
        new_project_sheet(&mut f.app, "Staffa a L");
        key(&mut f.app, f.welcome, Named::Enter);
        assert!(
            awaiting_parent(&f.app),
            "Return runs Avanti on a valid name"
        );
    }

    /// 2.15: a folder pick that yields no usable folder (no URIs, or
    /// none that maps to a local path) leaves the sheet live: Return asks
    /// for a folder again.
    #[test]
    fn an_empty_folder_pick_leaves_the_new_project_sheet_live() {
        let mut f = fixture();
        new_project_sheet(&mut f.app, "Staffa");
        key(&mut f.app, f.welcome, Named::Enter);
        assert!(awaiting_parent(&f.app));
        let _ = f.app.update(Message::ParentFolderChosen(Ok(Vec::new())));
        assert!(!awaiting_parent(&f.app), "the sheet stopped waiting");
        key(&mut f.app, f.welcome, Named::Enter);
        assert!(awaiting_parent(&f.app), "Return asks for a folder again");
    }

    /// 2.16 / 3.10: Esc is the new-project sheet's Annulla.
    #[test]
    fn esc_cancels_the_new_project_sheet() {
        let mut f = fixture();
        new_project_sheet(&mut f.app, "Staffa");
        key(&mut f.app, f.welcome, Named::Escape);
        assert!(f.app.welcome.creating.is_none());
    }

    /// 3.10: the first-run sheet's Continua is the default action; it has
    /// no cancel (SetupSheet.swift:31-35, interactiveDismissDisabled).
    #[test]
    fn return_continues_the_first_run_sheet_and_esc_does_not_dismiss_it() {
        let mut f = fixture();
        f.app.setup = Some(sheets::Setup::from_settings(&f.app.settings.load()));
        key(&mut f.app, f.welcome, Named::Escape);
        assert!(
            f.app.setup.is_some(),
            "Esc must not dismiss the first-run sheet"
        );
        key(&mut f.app, f.welcome, Named::Enter);
        assert!(f.app.setup.is_none(), "Return runs Continua");
        assert!(
            f.app.settings.path().exists(),
            "Continua persisted the preferences"
        );
    }

    /// 3.10: the python sheet — Più tardi on Esc, Fine on Return once
    /// ready, and nothing at all while an install runs
    /// (PythonSetupSheet.swift:62-82).
    #[test]
    fn python_sheet_keys_follow_its_button_roles() {
        let mut f = fixture();
        f.app.python.presented = true;
        f.app.python.state.state = EnvState::Missing;
        key(&mut f.app, f.welcome, Named::Escape);
        assert!(!f.app.python.presented, "Esc is Più tardi");

        f.app.python.presented = true;
        f.app.python.state.state = EnvState::Ready {
            build123d_version: "0.9".to_string(),
        };
        key(&mut f.app, f.welcome, Named::Escape);
        assert!(f.app.python.presented, "Fine has no cancel role");
        key(&mut f.app, f.welcome, Named::Enter);
        assert!(!f.app.python.presented, "Return is Fine");

        f.app.python.presented = true;
        f.app.python.state.state = EnvState::Installing;
        key(&mut f.app, f.welcome, Named::Escape);
        key(&mut f.app, f.welcome, Named::Enter);
        assert!(f.app.python.presented, "an install blocks both keys");
        assert!(matches!(f.app.python.state.state, EnvState::Installing));
    }

    /// 3.10: the agent-missing sheet over a project window — Chiudi on
    /// Esc, Verifica on Return unless a check runs
    /// (AgentMissingSheet.swift:56-69).
    #[tokio::test]
    async fn agent_sheet_keys_follow_its_button_roles_over_a_project_window() {
        let mut f = fixture();
        let project = open_real_project(&mut f, "Staffa");
        f.app.agent_missing = Some(sheets::AgentMissing {
            checking: true,
            ..sheets::AgentMissing::default()
        });
        key(&mut f.app, project, Named::Enter);
        assert!(
            f.app.agent_missing.is_some(),
            "Verifica is disabled while checking"
        );

        f.app.agent_missing.as_mut().unwrap().checking = false;
        key(&mut f.app, project, Named::Enter);
        assert!(
            f.app.agent_missing.as_ref().is_some_and(|s| s.checking),
            "Return runs Verifica"
        );

        key(&mut f.app, project, Named::Escape);
        assert!(f.app.agent_missing.is_none(), "Esc is Chiudi");
    }

    /// 7.12: a unit change in Settings reaches an already-open project's
    /// viewer (ModelPane.swift:7 reads the @AppStorage key live).
    #[tokio::test]
    async fn a_settings_unit_change_reaches_open_viewers() {
        let mut f = fixture();
        let project = open_real_project(&mut f, "Staffa");
        let _ = f.app.update_settings_any(settings::Message::PrintPref(
            sheets::PrintPrefMessage::Unit(MeasurementUnit::Inches),
        ));
        let Some(WindowKind::Project(window)) = f.app.windows.get(&project) else {
            panic!("the project window is gone");
        };
        assert_eq!(window.viewer.unit, MeasurementUnit::Inches);
    }

    /// An engine flip in Settings reaches an already-open project's empty
    /// transcript.
    #[tokio::test]
    async fn a_settings_engine_change_reaches_open_projects() {
        let mut f = fixture();
        let project = open_real_project(&mut f, "Staffa");
        let _ = f.app.update_settings_any(settings::Message::Engine(
            engine::agent::locator::AgentKind::Codex,
        ));
        let Some(WindowKind::Project(window)) = f.app.windows.get(&project) else {
            panic!("the project window is gone");
        };
        assert_eq!(window.engine, engine::agent::locator::AgentKind::Codex);
    }

    /// With a drive script running, every update publishes the project
    /// window's activity to the drive probe, so a working-to-settled
    /// activity reads as one finished turn and a `mark` can log the kind.
    #[tokio::test]
    async fn updates_publish_project_activity_to_the_drive_probe() {
        let mut f = fixture();
        let project = open_real_project(&mut f, "Staffa");
        let probe = Arc::new(crate::drive::TurnProbe::default());
        f.app.drive_probe = Some(probe.clone());
        let set = |app: &mut CadStudio, activity: crate::activity::Activity| {
            let Some(WindowKind::Project(window)) = app.windows.get_mut(&project) else {
                panic!("the project window is gone");
            };
            window.activity = activity;
            let _ = app.update(Message::Noop);
        };
        set(
            &mut f.app,
            crate::activity::Activity::Working("turn".into()),
        );
        assert!(probe.busy());
        assert_eq!(probe.finished(), 0);
        set(
            &mut f.app,
            crate::activity::Activity::Failed("build".into()),
        );
        assert!(!probe.busy());
        assert_eq!(probe.activity(), "failed");
        assert_eq!(probe.finished(), 1);
    }

    /// Settings a project window writes (the parameters overlay's collapse
    /// toggle, a layout preset's global default layout) reach the next
    /// project window the app opens, without a restart.
    #[tokio::test]
    async fn a_project_window_settings_write_reaches_the_next_window_opened() {
        let mut f = fixture();
        let first = open_real_project(&mut f, "Uno");
        let _ = f.app.update_project(
            first,
            project::Message::Viewer(crate::viewer_pane::Message::Parameters(
                crate::parameters_view::Message::Collapse(true),
            )),
        );
        let _ = f.app.update_project(
            first,
            project::Message::Command(project::Command::LayoutChatBottom),
        );

        store::create_project(&f.dir, "Due").unwrap();
        let _ = f.app.open_project(f.dir.join("Due"));
        let second = f
            .app
            .windows
            .values()
            .find_map(|kind| match kind {
                WindowKind::Project(window) if window.name == "Due" => Some(window),
                _ => None,
            })
            .expect("the second project window registered");
        assert!(
            second.viewer.parameters.collapsed,
            "the new window seeds the last collapse toggle"
        );
        assert_eq!(
            second.layout,
            engine::project::layout::LayoutNode::chat_bottom(),
            "the new project opens on the last default layout"
        );
    }

    /// 2.10: an unreadable project opens the load-error window with the
    /// error (ProjectWindow.swift:16-22, 42-44); no recent is added and
    /// the welcome window stays.
    /// The File menu's recents entries reach the app through the window's
    /// one `App` variant: Apri recenti fills the card from the store,
    /// Cancella menu empties both.
    #[tokio::test]
    async fn file_menu_recents_requests_reach_the_app() {
        let mut f = fixture();
        let id = open_real_project(&mut f, "Recenti");
        let card = |app: &CadStudio| match app.windows.get(&id) {
            Some(WindowKind::Project(window)) => window.recent_card.clone(),
            _ => panic!("the project window is open"),
        };
        let request = |request| Message::Project(id, project::Message::App(request));

        let _ = f.app.update(request(project::AppRequest::Command(
            AppCommand::OpenRecent,
        )));
        let stored = f.app.recents.paths().to_vec();
        assert!(!stored.is_empty(), "opening the project recorded it");
        assert_eq!(card(&f.app), Some(stored));

        let _ = f.app.update(request(project::AppRequest::ClearRecents));
        assert_eq!(card(&f.app), Some(Vec::new()));
        assert!(f.app.recents.paths().is_empty());
    }

    /// The pane-drag listener hears every window's cursor: while a grip is
    /// held in one project window, another window's motion and release
    /// must not retarget or drop its drag.
    #[tokio::test]
    async fn a_pane_drag_follows_only_its_own_window() {
        let mut f = fixture();
        let id = open_real_project(&mut f, "Trascina");
        let other = window::Id::unique();
        let drag = |app: &CadStudio| match app.windows.get(&id) {
            Some(WindowKind::Project(window)) => window.pane_drag,
            _ => panic!("the project window is open"),
        };
        let over = cosmic::iced::Point::new(1300.0, 700.0);
        let size = Size::new(1320.0, 787.0);
        let _ = f
            .app
            .update_project(id, project::Message::GridResized(size));
        let _ = f
            .app
            .update_project(id, project::Message::GripPressed(PaneKind::Chat));

        let moved = |window| project::Message::PaneDragMoved(window, over);
        let _ = f.app.update_project(id, moved(other));
        assert_eq!(drag(&f.app), None, "another window's cursor is ignored");

        let _ = f.app.update_project(id, moved(id));
        let own = "the window's own cursor drives the drag";
        assert!(drag(&f.app).is_some(), "{own}");
        let _ = f
            .app
            .update_project(id, project::Message::PaneDragEnded(other));
        let foreign = "another window's release drops nothing";
        assert!(drag(&f.app).is_some(), "{foreign}");
    }

    #[test]
    fn opening_an_unreadable_project_shows_the_load_error_window() {
        let _lang = crate::localize::pin_en();
        let mut f = fixture();
        let missing = f.dir.join("non-esiste");
        let _ = f.app.open_project(missing.clone());
        let errors: Vec<&String> = f
            .app
            .windows
            .values()
            .filter_map(|kind| match kind {
                WindowKind::LoadError(error) => Some(error),
                _ => None,
            })
            .collect();
        assert_eq!(errors.len(), 1, "one load-error window");
        // the catalog's notAFolder sentence, path kept (ProjectStore.swift:30)
        assert_eq!(
            errors[0],
            &format!("“{}” isn’t a folder.", missing.display())
        );
        assert!(
            f.app.recents.paths().is_empty(),
            "a failed open adds no recent"
        );
        assert!(matches!(
            f.app.windows.get(&f.welcome),
            Some(WindowKind::Welcome)
        ));
    }

    /// 3.9: a turn that finds no agent executable presents the
    /// agent-missing sheet with its missing line (AgentSession.swift:41-50);
    /// an ordinary failed turn does not.
    #[tokio::test]
    async fn an_agent_missing_turn_presents_the_agent_sheet() {
        let mut f = fixture();
        let project = open_real_project(&mut f, "Staffa");
        let finished = |outcome| {
            Message::Project(
                project,
                project::Message::Bridge(crate::bridge::Message::Event(EngineEvent::TurnFinished(
                    outcome,
                ))),
            )
        };
        let _ = f.app.update(finished(TurnOutcome::StartFailed {
            error: "exec".to_string(),
        }));
        assert!(
            f.app.agent_missing.is_none(),
            "a start failure is not a missing agent"
        );
        let _ = f.app.update(finished(TurnOutcome::AgentMissing));
        assert!(
            f.app
                .agent_missing
                .as_ref()
                .is_some_and(|sheet| sheet.last_verified_missing),
            "the sheet presents with its missing line"
        );
    }

    /// Only bare Return / Esc are sheet keys.
    #[test]
    fn sheet_key_roles_are_bare_return_and_esc() {
        let none = Modifiers::empty();
        assert_eq!(
            sheet_key_role(&Key::Named(Named::Enter), &none),
            Some(SheetKey::Default)
        );
        assert_eq!(
            sheet_key_role(&Key::Named(Named::Escape), &none),
            Some(SheetKey::Cancel)
        );
        assert_eq!(
            sheet_key_role(&Key::Named(Named::Enter), &Modifiers::CTRL),
            None
        );
        assert_eq!(sheet_key_role(&Key::Named(Named::Tab), &none), None);
    }

    fn open_settings_window(f: &mut Fixture) -> window::Id {
        let id = window::Id::unique();
        let state = SettingsState::new(&f.app.settings.load());
        f.app.windows.insert(id, WindowKind::Settings(state));
        id
    }

    fn settings_state(f: &Fixture, id: window::Id) -> &SettingsState {
        match f.app.windows.get(&id) {
            Some(WindowKind::Settings(state)) => state,
            _ => panic!("the settings window is gone"),
        }
    }

    /// The agent sheet's Verify persists the custom path; an open settings
    /// window shows it without a reopen.
    #[test]
    fn settings_window_reseeds_when_agent_path_changes_underneath() {
        let mut f = fixture();
        let id = open_settings_window(&mut f);
        assert_eq!(settings_state(&f, id).custom_paths[0], "");
        f.app.agent_missing = Some(sheets::AgentMissing {
            kind: AgentKind::Claude,
            custom_path: "/opt/tools/claude".to_string(),
            ..sheets::AgentMissing::default()
        });
        let _ = f
            .app
            .update(Message::AgentSheet(sheets::AgentMessage::Verify));
        assert_eq!(settings_state(&f, id).custom_paths[0], "/opt/tools/claude");
    }

    /// A settings write the store refuses leaves the window's mirror on
    /// the persisted value.
    #[test]
    fn a_failed_settings_save_does_not_leave_the_mirror_ahead_of_the_file() {
        let mut f = fixture();
        let id = open_settings_window(&mut f);
        std::fs::write(f.app.settings.path(), "not json").unwrap();
        let _ = f.app.update(Message::Settings(
            id,
            settings::Message::Model(AgentKind::Claude, "opus".to_string()),
        ));
        assert_eq!(
            settings_state(&f, id).models[0],
            "",
            "the refused save is not shown"
        );
        std::fs::write(f.app.settings.path(), "{}").unwrap();
        let _ = f.app.update(Message::Settings(
            id,
            settings::Message::Model(AgentKind::Claude, "opus".to_string()),
        ));
        assert_eq!(settings_state(&f, id).models[0], "opus", "a saved one is");
    }

    /// Verifica after a failed environment check starts a new check.
    #[tokio::test]
    async fn verifica_after_a_failed_check_reruns_the_check() {
        let mut f = fixture();
        let id = open_settings_window(&mut f);
        f.app.python.state.state = EnvState::Failed("broken".to_string());
        let _ = f
            .app
            .update(Message::Settings(id, settings::Message::PythonCheck));
        assert!(
            matches!(f.app.python.state.state, EnvState::Checking),
            "the check restarts"
        );
    }
}
