//! The application shell (upstream CADStudioApp.swift): one
//! `cosmic::Application` over a welcome window and one window per open
//! project, the recents store, the first-run and agent-missing sheets, the
//! M4 command subset, and the platform integrations — startup orphan reap
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

use ashpd::desktop::file_chooser::SelectedFiles;
use cosmic::app::ApplicationExt;
use cosmic::app::Core;
use cosmic::cosmic_config::Config;
use cosmic::iced::keyboard::{Key, Modifiers};
use cosmic::iced::window;
use cosmic::iced::{Length, Subscription, Task};
use cosmic::widget::{Column, container, popover, text};
use cosmic::{Action, Application, Element};
use engine::agent::locator::{self, AgentKind};
use engine::project::store;
use engine::python::env::{
    EnvEvent, EnvHandle, EnvState, check, command_probe, reinstall, streaming_spawn, venv_dir,
};
use engine::settings::{Settings as EngineSettings, settings_path};

use crate::recents::{self, Recents};
use crate::sheets;
use crate::windows::project::{self, ProjectWindow};
use crate::windows::welcome::{self, Welcome};

/// Which window a [`window::Id`] names.
pub enum WindowKind {
    Welcome,
    Project(ProjectWindow),
}

/// App-level commands reachable from any window (upstream's global
/// CommandGroups; ⌘→Ctrl, ⇧⌘1 keeps upstream's Welcome binding).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppShortcut {
    NewProject,
    OpenProject,
    Welcome,
}

#[derive(Debug, Clone)]
pub enum Message {
    Welcome(welcome::Message),
    Project(window::Id, project::Message),
    /// The agent-missing sheet (presented over the focused window).
    AgentSheet(sheets::AgentMessage),
    /// The first-run setup sheet.
    SetupSheet(sheets::SetupMessage),
    /// The python setup sheet (PythonSetupSheet + the driver events it
    /// reduces).
    PythonSheet(sheets::PythonMessage),
    /// A key press the focused window did not consume.
    KeyPressed(window::Id, Key, Modifiers),
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
    /// The file portal answered for the chat attach button.
    AttachChosen(window::Id, Result<Vec<PathBuf>, String>),
    /// SIGTERM arrived: terminate the engine's children and exit.
    SigTerm,
    /// A no-op ack for futures that must return a message.
    Noop,
}

pub struct CadStudio {
    core: Core,
    windows: HashMap<window::Id, WindowKind>,
    welcome: Welcome,
    recents: Recents,
    config: Config,
    settings: EngineSettings,
    settings_file: PathBuf,
    /// The agent-missing sheet, when presented (upstream
    /// `AgentLocator.isMissingSheetPresented`).
    agent_missing: Option<sheets::AgentMissing>,
    /// The first-run setup sheet, when presented.
    setup: Option<sheets::Setup>,
    /// The python environment state + its sheet (upstream
    /// `PythonEnvironment.shared`: the state exists from launch, the sheet
    /// presents on top of it).
    python: sheets::PythonSetup,
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
    fn init(core: Core, _flags: ()) -> (Self, Task<Action<Message>>) {
        engine::processes::reap_orphans();

        let config = Config::new(Self::APP_ID, 1).expect("cosmic-config for app UI state");
        let settings_file = settings_path();
        let settings = EngineSettings::load(&settings_file).unwrap_or_default();
        let recents = Recents::load(config.clone());
        let setup = recents::needs_setup(&config).then(|| sheets::Setup::from_settings(&settings));
        let mut python = sheets::PythonSetup::default();
        // the sheet state the check reduces from — the drivers emit only
        // their events, so Checking is the app's own preset
        python.state.begin_checking();

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
            settings_file,
            agent_missing: None,
            setup,
            python,
        };

        // locate the agent off the UI thread; a miss presents the sheet
        let locate = Task::future(locate_agent(app.settings.claude_path.clone()))
            .map(|found| Action::App(Message::AgentLocated(found)));
        let sigterm = Task::stream(sigterm_stream()).map(Action::App);
        // the startup python check: its terminal folds into the sheet
        // state (a missing/failed venv presents the sheet once first-run
        // setup is done)
        let python_check = python_events(check(&venv_dir(), &streaming_spawn()));
        // the optional UI drive harness (CAD_STUDIO_DRIVE): a script of
        // user-action messages streamed into the loop — the smoke-run
        // replacement for synthetic input, which COSMIC 1.6 gates
        let drive = crate::drive::startup_task().map(Action::App);
        (app, Task::batch([locate, sigterm, python_check, drive]))
    }

    fn update(&mut self, message: Message) -> Task<Action<Message>> {
        match message {
            Message::Welcome(msg) => self.update_welcome(msg),
            Message::Project(id, msg) => self.update_project(id, msg),
            Message::AgentSheet(msg) => self.update_agent_sheet(msg),
            Message::SetupSheet(msg) => self.update_setup_sheet(msg),
            Message::PythonSheet(msg) => self.update_python_sheet(msg),
            Message::KeyPressed(id, key, modifiers) => self.update_key_press(id, key, modifiers),
            Message::CloseWindow(id) => self.close_window(id),
            Message::DragWindow(id) => cosmic::iced::window::drag(id),
            Message::ToggleMaximize(id) => cosmic::iced::window::toggle_maximize(id),
            Message::WindowMenu(id) => cosmic::iced::window::show_system_menu(id),
            Message::WindowClosed(id) => self.window_closed(id),
            Message::AgentLocated(found) => {
                if found {
                    self.agent_missing = None;
                } else if self.agent_missing.is_none() {
                    // the missing line shows from first presentation
                    // (AgentMissingSheet.swift:42 gates on
                    // `locator.state == .missing`, not on a Verify)
                    self.agent_missing = Some(sheets::AgentMissing {
                        last_verified_missing: true,
                        ..sheets::AgentMissing::default()
                    });
                }
                Task::none()
            }
            Message::ParentFolderChosen(Ok(parents)) => {
                let Some(sheet) = self.welcome.creating.as_ref() else {
                    return Task::none();
                };
                let Some(parent) = parents.first().cloned() else {
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
                    let _ = self.welcome.update(welcome::Message::NewSheet(
                        welcome::new_project::Message::Created(Err(why)),
                    ));
                    Task::none()
                }
            },
            Message::AttachChosen(id, result) => {
                let msg = project::Message::AttachChosen(result);
                self.update_project(id, msg)
            }
            Message::SigTerm => {
                // upstream's handler: terminateAll() then exit(0)
                engine::processes::terminate_all();
                cosmic::iced::exit()
            }
            Message::Noop => Task::none(),
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
                // The first-run setup sheet REPLACES the welcome body.
                // Presented as a popover it had to be modal (upstream is
                // dismiss-blocked), and libcosmic's modal popovers capture
                // mouse events window-wide — killing the header bar's
                // drag/close. As the body, the card centers in the window
                // and the chrome stays live.
                if let Some(setup) = &self.setup {
                    let card: Element<'_, Message> =
                        sheets::view_setup(setup).map(Message::SetupSheet);
                    let centered = container(card)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(cosmic::iced::alignment::Horizontal::Center)
                        .align_y(cosmic::iced::alignment::Vertical::Center)
                        .into();
                    return self.decorate(id, focused, centered, "Welcome to CAD Studio");
                }
                let body = self
                    .welcome
                    .view(self.recents.paths())
                    .map(Message::Welcome);
                // The new-project sheet presents over the welcome window
                // (NewProjectSheet.swift attaches there). It also replaces
                // the body — a modal popover would kill the window chrome
                // (see the setup branch above).
                let body = match self.welcome.creating.as_ref() {
                    Some(sheet) => {
                        let card: Element<'_, Message> = welcome::new_project::view(sheet)
                            .map(|m| Message::Welcome(welcome::Message::NewSheet(m)));
                        container(card)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .align_x(cosmic::iced::alignment::Horizontal::Center)
                            .align_y(cosmic::iced::alignment::Vertical::Center)
                            .into()
                    }
                    None => body,
                };
                let with_sheet = self.apply_sheets(body);
                self.decorate(id, focused, with_sheet, "Welcome to CAD Studio")
            }
            Some(WindowKind::Project(project)) => {
                // ONE header: the window's own view_header is the ProjectToolbar
                // port and already carries the close button (ProjectWindow.swift:54-56
                // `.toolbar { ProjectToolbar(project) }` + `.toolbar(removing: .title)`);
                // decorating again would stack a second bar.
                let body = project.view().map(move |m| Message::Project(id, m));
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
            None => self.decorate(
                id,
                focused,
                Column::new()
                    .push(text("CAD Studio".to_string()))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                "CAD Studio",
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
    fn update_welcome(&mut self, msg: welcome::Message) -> Task<Action<Message>> {
        // the new-project command presents the sheet AND makes sure the
        // welcome window carrying it is on screen (upstream ⌘N:
        // openWindow(id: WelcomeWindow.id) + isCreatingProject = true)
        if matches!(msg, welcome::Message::NewProject) {
            if self.welcome.creating.is_none() {
                self.welcome.creating = Some(welcome::new_project::NewProject::default());
            }
            return self.show_welcome();
        }
        // a context-menu entry resolves against the recents store first
        if let welcome::Message::RecentMenu(index, kind) = &msg {
            let path = self.recents.paths().get(*index).cloned();
            if let Some(path) = path {
                let translated = match kind {
                    welcome::RecentMenuKind::Open => welcome::Message::OpenRecent(path),
                    welcome::RecentMenuKind::Reveal => welcome::Message::RevealRecent(path),
                    welcome::RecentMenuKind::Remove => welcome::Message::RemoveRecent(path),
                };
                return self.update_welcome(translated);
            }
            return Task::none();
        }
        match &msg {
            welcome::Message::OpenExisting => {
                let future = choose_folder("Choose a project folder");
                return Task::future(async move { Message::OpenFolderChosen(future.await) })
                    .map(Action::App);
            }
            welcome::Message::OpenRecent(path) => {
                let path = path.clone();
                return self.open_project(path);
            }
            welcome::Message::RevealRecent(path) => {
                let path = path.clone();
                return Task::future(async move {
                    reveal_path(path).await;
                    Message::Noop
                })
                .map(Action::App);
            }
            welcome::Message::RemoveRecent(path) => {
                let path = path.clone();
                self.recents.remove(path);
            }
            welcome::Message::NewProject | welcome::Message::RecentMenu(_, _) => {}
            welcome::Message::NewSheet(sheet_msg) => match sheet_msg {
                welcome::new_project::Message::Cancel => {
                    self.welcome.creating = None;
                }
                welcome::new_project::Message::Advance => {
                    // fold the sheet state (awaiting_parent), then ask
                    // where to create — the name already validated
                    let _ = self.welcome.update(msg.clone());
                    let future = choose_folder("Choose where to create the project");
                    return Task::future(async move { Message::ParentFolderChosen(future.await) })
                        .map(Action::App);
                }
                welcome::new_project::Message::Created(_)
                | welcome::new_project::Message::Name(_) => {}
            },
        }
        let task = self.welcome.update(msg).map(Message::Welcome);
        task.map(Action::App)
    }

    fn update_project(&mut self, id: window::Id, msg: project::Message) -> Task<Action<Message>> {
        let Some(WindowKind::Project(project)) = self.windows.get_mut(&id) else {
            return Task::none();
        };
        // the attach button needs the app's portal — intercept before the
        // window's own update routes it into widget state
        if matches!(
            msg,
            project::Message::Chat(crate::chat_view::Message::Attach)
        ) {
            return project
                .attach()
                .map(move |m| Message::Project(id, m))
                .map(Action::App);
        }
        if matches!(msg, project::Message::CloseRequested) {
            return self.close_window(id);
        }
        if matches!(msg, project::Message::PresentSetupSheet) {
            // a python-readiness refusal presents the setup sheet over
            // whatever window is up, UNCONDITIONALLY (Project.swift:184-197
            // set isSetupSheetPresented = true; the hasCompletedSetup gate
            // belongs to the startup check's terminal only)
            self.python.presented = true;
            return Task::none();
        }
        if let project::Message::App(request) = msg {
            let translated = match request {
                project::AppRequest::NewProject => welcome::Message::NewProject,
                project::AppRequest::OpenProject => welcome::Message::OpenExisting,
                // the recents list lives in the welcome window
                project::AppRequest::OpenRecent | project::AppRequest::Welcome => {
                    return self.show_welcome();
                }
            };
            return self.update_welcome(translated);
        }
        let task = project.update(msg).map(move |m| Message::Project(id, m));
        task.map(Action::App)
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
                let mut settings = EngineSettings::load(&self.settings_file).unwrap_or_default();
                sheet.stage_custom_path(&mut settings);
                self.settings = settings.clone();
                let _ = settings.save(&self.settings_file);
                sheet.checking = true;
                let future = locate_agent(self.settings.claude_path.clone());
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
            let mut settings = EngineSettings::load(&self.settings_file).unwrap_or_default();
            setup.apply(&mut settings);
            if let Err(why) = settings.save(&self.settings_file) {
                eprintln!("cad-studio: setup save failed: {why}");
            } else {
                self.settings = settings;
            }
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

    /// Route an unconsumed key press: app-level shortcuts act globally,
    /// project commands go to the window the key landed in.
    fn update_key_press(
        &mut self,
        id: window::Id,
        key: Key,
        modifiers: Modifiers,
    ) -> Task<Action<Message>> {
        match app_shortcut(&key, &modifiers) {
            Some(AppShortcut::NewProject) => {
                return self.update_welcome(welcome::Message::NewProject);
            }
            Some(AppShortcut::OpenProject) => {
                return self.update_welcome(welcome::Message::OpenExisting);
            }
            Some(AppShortcut::Welcome) => {
                return self.show_welcome();
            }
            None => {}
        }
        if let Some(command) = project::project_shortcut(&key, &modifiers) {
            return self.update_project(id, project::Message::Command(command));
        }
        Task::none()
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
            // upstream throws and ProjectWindow shows the load-error view;
            // the M4 port declines to open (the surface is noted for M6)
            eprintln!("cad-studio: cannot open {}: {why}", root.display());
            return Task::none();
        }
        let (window, bridge_task) = ProjectWindow::new(root.clone(), &self.settings);
        let name = window.name.clone();
        let (id, spawn) = window::open(window::Settings {
            size: cosmic::iced::Size::new(project::DEFAULT_SIZE.0, project::DEFAULT_SIZE.1),
            min_size: Some(cosmic::iced::Size::new(
                project::MIN_SIZE.0,
                project::MIN_SIZE.1,
            )),
            ..Default::default()
        });
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
        let (id, spawn) = window::open(window::Settings {
            size: cosmic::iced::Size::new(
                welcome::ACTIONS_WIDTH + welcome::RECENTS_WIDTH,
                welcome::WINDOW_HEIGHT,
            ),
            ..Default::default()
        });
        self.windows.insert(id, WindowKind::Welcome);
        let title = self.set_window_title("Welcome to CAD Studio".to_string(), id);
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
    /// agent-missing). The first-run sheet is NOT here: it replaces the
    /// welcome body instead (see `view_window`) because a modal popover
    /// captures mouse events window-wide, killing the window chrome.
    /// These two are modal gates over the project canvas — blocking the
    /// content behind them is the intent.
    fn apply_sheets<'a>(&'a self, body: Element<'a, Message>) -> Element<'a, Message> {
        let mut pop = popover(body);
        let mut presented = false;
        if self.python.presented {
            let sheet: Element<'_, Message> =
                sheets::view_python_setup(&self.python).map(Message::PythonSheet);
            pop = pop.popup(sheet);
            presented = true;
        } else if let Some(sheet) = &self.agent_missing {
            let sheet: Element<'_, Message> =
                sheets::view_agent_missing(sheet).map(Message::AgentSheet);
            pop = pop.popup(sheet);
            presented = true;
        }
        if presented {
            pop = pop.modal(true);
        }
        pop.into()
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

/// The app-level shortcut table (upstream's global commands; ⌘→Ctrl).
pub fn app_shortcut(key: &Key, modifiers: &Modifiers) -> Option<AppShortcut> {
    let ctrl = modifiers.control();
    let shift = modifiers.shift();
    let alt = modifiers.alt();
    let character = match key {
        Key::Character(ch) => ch.to_lowercase(),
        _ => return None,
    };
    match (ctrl, shift, alt, character.as_str()) {
        (true, false, false, "n") => Some(AppShortcut::NewProject),
        (true, false, false, "o") => Some(AppShortcut::OpenProject),
        (true, true, false, "1") => Some(AppShortcut::Welcome),
        _ => None,
    }
}

/// Route key presses the focused window's widgets did not consume, tagged
/// with the window they landed in.
fn key_events(
    event: cosmic::iced::Event,
    status: cosmic::iced::event::Status,
    id: window::Id,
) -> Option<Message> {
    let cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
        key,
        modifiers,
        ..
    }) = event
    else {
        return None;
    };
    if matches!(status, cosmic::iced::event::Status::Captured) {
        return None;
    }
    Some(Message::KeyPressed(id, key, modifiers))
}

/// Locate the agent off the UI thread (upstream `AgentLocator.locate()`):
/// the settings' custom path, then PATH, then the fixed candidates, each
/// validated by `--version`.
async fn locate_agent(custom_path: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || {
        locator::find_agent(AgentKind::Claude, custom_path.as_deref()).is_some()
    })
    .await
    .unwrap_or(false)
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
/// `NSOpenPanel`/`chooseParentFolder`).
pub async fn choose_folder(title: &str) -> Result<Vec<PathBuf>, String> {
    let request = SelectedFiles::open_file().title(title).directory(true);
    let response = request.send().await.map_err(|why| why.to_string())?;
    let files = response.response().map_err(|why| why.to_string())?;
    Ok(files
        .uris()
        .iter()
        .filter_map(|uri| uri.to_file_path().ok())
        .collect())
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

    /// The app-level bindings (Commands.swift:16-49): ⌘N, ⌘O, ⇧⌘1 — and
    /// nothing on bare keys.
    #[test]
    fn app_shortcut_table_matches_upstream() {
        let ctrl = Modifiers::CTRL;
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;

        assert_eq!(
            app_shortcut(&Key::Character("n".into()), &ctrl),
            Some(AppShortcut::NewProject)
        );
        assert_eq!(
            app_shortcut(&Key::Character("o".into()), &ctrl),
            Some(AppShortcut::OpenProject)
        );
        assert_eq!(
            app_shortcut(&Key::Character("1".into()), &ctrl_shift),
            Some(AppShortcut::Welcome),
            "⇧⌘1 is the welcome window command"
        );
        assert_eq!(
            app_shortcut(&Key::Character("1".into()), &Modifiers::empty()),
            None
        );
        assert_eq!(app_shortcut(&Key::Character("q".into()), &ctrl), None);
    }
}
