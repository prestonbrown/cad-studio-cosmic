//! The welcome window (upstream WelcomeWindow.swift): the actions pane —
//! app title, version, new/open buttons — beside the recents list with
//! `schematic.png` thumbnails, plus the new-project sheet
//! (NewProjectSheet.swift) whose create flow is split with the app layer:
//! the sheet owns the name, the app runs the parent-folder portal and
//! `create_project`.
//!
//! User-visible strings follow the landed modules' convention: plain
//! English values, each named for its en/main.ftl key, pending the
//! repo-wide `fl!()` swap.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cosmic::iced::Length;
use cosmic::iced::widget::image::Handle;
use cosmic::widget::menu::action::MenuAction;
use cosmic::widget::menu::{Item, items};
use cosmic::widget::{Column, Row, Space, button, container, scrollable, text, text_input};
use cosmic::{Element, Task};
use engine::project::store::{self, ProjectNameError};

/// Upstream's welcome geometry (WelcomeWindow.swift:14-21): a 480 px
/// actions pane beside a 300 px recents list, 460 px tall. The port runs
/// 560 px: the first-run setup sheet must fit inside the window with its
/// action row pinned (macOS sheets may exceed the window; popovers may
/// not), and 460 clipped it.
pub const ACTIONS_WIDTH: f32 = 480.0;
pub const RECENTS_WIDTH: f32 = 300.0;
pub const WINDOW_HEIGHT: f32 = 560.0;

/// The new-project sheet (NewProjectSheet.swift).
pub mod new_project {
    use super::*;

    /// The sheet's messages; the portal/create effects are the app's — it
    /// acts on `Advance` (asking for the parent folder) and reports the
    /// outcome back as `Created`.
    #[derive(Debug, Clone)]
    pub enum Message {
        Name(String),
        Cancel,
        /// "Avanti": validate, then the app asks for the parent folder.
        Advance,
        /// `create_project` finished.
        Created(Result<PathBuf, String>),
    }

    /// The sheet's state: the name field and the failure alert
    /// (NewProjectSheet.swift:5-8).
    #[derive(Debug, Default)]
    pub struct NewProject {
        pub name: String,
        /// The "Impossibile creare il progetto" alert's message.
        pub error: Option<String>,
        /// A parent-folder pick is in flight (the sheet waits).
        pub awaiting_parent: bool,
    }

    impl NewProject {
        /// The name the sheet would create with
        /// (NewProjectSheet.swift:9-11).
        pub fn trimmed_name(&self) -> &str {
            self.name.trim()
        }

        /// Upstream's `isNameValid` as the engine's verdict
        /// (NewProjectSheet.swift:13-18 — the same rules
        /// [`store::validate_project_name`] encodes).
        pub fn validation_error(&self) -> Option<ProjectNameError> {
            store::validate_project_name(self.trimmed_name()).err()
        }
    }

    /// Fold one message into the sheet state.
    pub fn update(sheet: &mut NewProject, message: &Message) {
        match message {
            Message::Name(name) => sheet.name = name.clone(),
            Message::Cancel => {}
            Message::Advance => sheet.awaiting_parent = true,
            Message::Created(Ok(_)) => {}
            Message::Created(Err(why)) => {
                sheet.awaiting_parent = false;
                sheet.error = Some(why.clone());
            }
        }
    }

    /// The sheet body (NewProjectSheet.swift:20-54): heading, the name
    /// field, the folder note, Cancel/Avanti.
    pub fn view(sheet: &NewProject) -> Element<'_, Message> {
        let valid = sheet.validation_error().is_none();
        let mut stack = Column::new()
            .spacing(16)
            .push(text("Choose the options for the new project:").size(18))
            .push(
                Column::new().spacing(6).push(text("Project name:")).push(
                    text_input::text_input("L bracket", &sheet.name)
                        .on_input(Message::Name)
                        // Return runs Avanti (upstream `.onSubmit(create)`,
                        // NewProjectSheet.swift:27)
                        .on_submit(|_| Message::Advance)
                        .size(14),
                ),
            )
            .push(
                text("In the next step you will choose where to create the project folder.")
                    .class(dim_text()),
            );
        if let Some(error) = &sheet.error {
            stack = stack.push(text(format!("Cannot create the project: {error}")).class(
                cosmic::theme::Text::Color(cosmic::iced::Color::from_rgb(0.90, 0.32, 0.32)),
            ));
        }
        stack = stack.push(
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(
                    button::text("Cancel")
                        .class(cosmic::theme::Button::Standard)
                        .on_press(Message::Cancel),
                )
                .push(Space::new().width(8))
                .push(
                    button::text("Next")
                        .class(cosmic::theme::Button::Suggested)
                        .on_press_maybe(valid.then_some(Message::Advance)),
                )
                .spacing(8),
        );
        // Dialog card (opaque) — a bare container let the welcome content
        // behind bleed through the sheet in its popover.
        container(stack)
            .padding(20)
            .width(480.0)
            .class(cosmic::theme::Container::Dialog(true))
            .into()
    }

    fn dim_text() -> cosmic::theme::Text {
        cosmic::theme::Text::Custom(|theme| {
            let mut color = cosmic::iced::Color::from(theme.cosmic().background(false).on);
            color.a *= 0.65;
            cosmic::iced::widget::text::Style {
                color: Some(color),
                ..Default::default()
            }
        })
    }
}

/// The welcome window's messages.
#[derive(Debug, Clone)]
pub enum Message {
    NewProject,
    OpenExisting,
    OpenRecent(PathBuf),
    RevealRecent(PathBuf),
    RemoveRecent(PathBuf),
    NewSheet(new_project::Message),
    /// A recents context-menu entry, by row index (the app resolves the
    /// path against its recents store — menu actions are Copy, paths are
    /// not).
    RecentMenu(usize, RecentMenuKind),
}

/// What a recents row's context menu can do (WelcomeWindow.swift:68-76).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentMenuKind {
    Open,
    Reveal,
    Remove,
}

/// The context menu's action: the entry kind plus the row's index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentMenuAction {
    pub kind: RecentMenuKind,
    pub index: usize,
}

impl MenuAction for RecentMenuAction {
    type Message = Message;

    fn message(&self) -> Message {
        Message::RecentMenu(self.index, self.kind)
    }
}

/// The welcome window's own state: the new-project sheet (when presented)
/// and the recents' thumbnail cache. The recents LIST lives in the app
/// (shared with the recents menu); the view borrows it.
#[derive(Debug, Default)]
pub struct Welcome {
    pub creating: Option<new_project::NewProject>,
    /// `schematic.png` per project root, loaded once (upstream
    /// `NSImage(contentsOf:)` per row; `None` renders the fallback icon).
    /// Interior mutability: views are `&self` in iced, and a decode on the
    /// render path must not re-run every frame.
    thumbnails: std::cell::RefCell<HashMap<PathBuf, Option<Handle>>>,
}

impl Welcome {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NewProject => {
                if self.creating.is_none() {
                    self.creating = Some(new_project::NewProject::default());
                }
            }
            Message::NewSheet(msg) => {
                let Some(sheet) = self.creating.as_mut() else {
                    return Task::none();
                };
                new_project::update(sheet, &msg);
            }
            // the app layer owns these (portals, windows, recents store)
            Message::OpenExisting
            | Message::OpenRecent(_)
            | Message::RevealRecent(_)
            | Message::RemoveRecent(_)
            | Message::RecentMenu(_, _) => {}
        }
        Task::none()
    }

    /// The window body (WelcomeWindow.swift:13-88): actions pane, divider,
    /// recents list. Sheets are the app layer's popover.
    pub fn view(&self, recents: &[PathBuf]) -> Element<'_, Message> {
        // Upstream's actions VStack(spacing: 0): Spacer / app icon 128 /
        // title / version (+2 top) / Spacer / buttons (spacing 8, 320) /
        // Spacer — vertically distributed, not top-hung.
        let actions = Column::new()
            .spacing(0)
            .push(Space::new().height(Length::Fill))
            .push_maybe(app_icon().map(|h| cosmic::widget::image(h).width(128).height(128)))
            .push(
                text("CAD Studio".to_string())
                    .size(36)
                    .font(cosmic::iced::font::Font {
                        weight: cosmic::iced::font::Weight::Bold,
                        ..cosmic::font::default()
                    }),
            )
            .push(
                container(text(format!("Version {}", env!("CARGO_PKG_VERSION"))).class(dim_text()))
                    .padding([2, 0, 0, 0]),
            )
            .push(Space::new().height(Length::Fill))
            .push(action_button(
                "list-add",
                "Create New Project…",
                Message::NewProject,
            ))
            .push(Space::new().height(8))
            .push(action_button(
                "folder",
                "Open Existing Project…",
                Message::OpenExisting,
            ))
            .push(Space::new().height(Length::Fill))
            .width(Length::Fill)
            .align_x(cosmic::iced::alignment::Alignment::Center);

        let recents_list = self.view_recents(recents);
        Row::new()
            .push(container(actions).width(ACTIONS_WIDTH).padding([0, 40]))
            .push(container(
                cosmic::widget::divider::vertical::default().height(Length::Fill),
            ))
            .push(recents_list)
            .height(Length::Fill)
            .into()
    }

    /// The recents sidebar (WelcomeWindow.swift:63-88): rows with the
    /// project's `schematic.png` thumbnail, name and parent path, a
    /// context menu, and the empty state.
    fn view_recents(&self, recents: &[PathBuf]) -> Element<'_, Message> {
        let list: Element<'_, Message> = if recents.is_empty() {
            container(
                text("No recent projects".to_string())
                    .class(dim_text())
                    .width(Length::Fill),
            )
            .align_x(cosmic::iced::alignment::Alignment::Center)
            .align_y(cosmic::iced::alignment::Alignment::Center)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            let mut rows = Column::new().spacing(2).padding(8);
            for (index, path) in recents.iter().enumerate() {
                rows = rows.push(self.recent_row(index, path));
            }
            scrollable(rows)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };
        container(list)
            .width(RECENTS_WIDTH)
            .height(Length::Fill)
            .into()
    }

    /// One recents row (upstream `RecentProjectRow`): thumbnail, name,
    /// tilde-abbreviated parent, wrapped in the row's context menu.
    fn recent_row(&self, index: usize, path: &Path) -> Element<'_, Message> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent = path
            .parent()
            .map(|p| abbreviate_tilde(p))
            .unwrap_or_default();

        let thumbnail: Element<'_, Message> = match self.thumbnail(path) {
            Some(handle) => cosmic::widget::image(handle).width(40).height(30).into(),
            None => container(
                // `cube` upstream; the theme fallback is a package glyph
                cosmic::widget::icon::from_name("package-x-generic")
                    .size(18)
                    .icon(),
            )
            .width(40)
            .height(30)
            .align_x(cosmic::iced::alignment::Alignment::Center)
            .align_y(cosmic::iced::alignment::Alignment::Center)
            .into(),
        };

        let row = Row::new()
            .spacing(10)
            .push(thumbnail)
            .push(
                Column::new()
                    .spacing(2)
                    // the name needs the explicit class: it sits under a
                    // Transparent button, whose inherited text color is
                    // fully transparent
                    .push(text(name.clone()).size(14).class(primary_text()))
                    .push(text(parent).size(11).class(dim_text())),
            )
            .padding([3, 6])
            .width(Length::Fill);

        let open = Message::OpenRecent(path.to_path_buf());
        let button = button::custom(row)
            .class(cosmic::theme::Button::Transparent)
            .on_press(open);

        // the row's context menu (WelcomeWindow.swift:68-81); the index
        // travels in the Copy action, the app resolves it to this row
        let actions = move |kind: RecentMenuKind| RecentMenuAction { kind, index };
        let menu_items = items(
            &std::collections::HashMap::new(),
            vec![
                Item::Button("Open", None, actions(RecentMenuKind::Open)),
                Item::Button("Reveal", None, actions(RecentMenuKind::Reveal)),
                Item::Divider,
                Item::Button("Remove from List", None, actions(RecentMenuKind::Remove)),
            ],
        );
        cosmic::widget::context_menu(button, Some(menu_items)).into()
    }

    /// The project's `schematic.png` as an image handle, cached per root
    /// (upstream `ProjectFolder(root:).png`).
    fn thumbnail(&self, root: &Path) -> Option<Handle> {
        let mut cache = self.thumbnails.borrow_mut();
        if let Some(hit) = cache.get(root) {
            return hit.clone();
        }
        let loaded = load_thumbnail(&root.join("output").join("schematic.png"));
        cache.insert(root.to_path_buf(), loaded.clone());
        loaded
    }
}

/// The app icon (upstream `NSApp.applicationIconImage`, shown 128×128
/// above the wordmark): the original app's actual icon PNG, decoded once.
fn app_icon() -> Option<Handle> {
    thread_local! {
        static CACHED: Option<Handle> = {
            const PNG: &[u8] = include_bytes!("../../resources/app-icon.png");
            let decoded = image::load_from_memory(PNG).ok()?.to_rgba8();
            let (width, height) = decoded.dimensions();
            Some(Handle::from_rgba(width, height, decoded.into_raw()))
        };
    }
    CACHED.with(Clone::clone)
}

/// Decode a PNG into an image handle (upstream `NSImage(contentsOf:)` +
/// `aspectRatio(contentMode: .fit)`; the fit sizing is the widget's).
pub fn load_thumbnail(png: &Path) -> Option<Handle> {
    let bytes = std::fs::read(png).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    Some(Handle::from_rgba(width, height, decoded.into_raw()))
}

/// Upstream `abbreviatingWithTildeInPath` — the leading home directory
/// becomes `~`.
pub fn abbreviate_tilde(path: &Path) -> String {
    let text = path.to_string_lossy();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => text
            .strip_prefix(home.as_str())
            .map(|rest| format!("~{rest}"))
            .unwrap_or_else(|| text.into_owned()),
        _ => text.into_owned(),
    }
}

/// The action buttons (upstream `WelcomeActionButton`): icon + label on a
/// subtle rounded fill.
fn action_button<'a>(icon_name: &str, label: &str, message: Message) -> Element<'a, Message> {
    button::custom(
        Row::new()
            .spacing(10)
            .push(
                cosmic::widget::icon::from_name(icon_name.to_string())
                    .size(18)
                    .icon(),
            )
            .push(text(label.to_string()))
            .push(Space::new().width(Length::Fill))
            .padding([0, 12])
            .height(40)
            .width(Length::Fill),
    )
    .class(cosmic::theme::Button::Standard)
    .on_press(message)
    .width(320)
    .into()
}

fn dim_text() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| {
        let mut color = cosmic::iced::Color::from(theme.cosmic().background(false).on);
        color.a *= 0.65;
        cosmic::iced::widget::text::Style {
            color: Some(color),
            ..Default::default()
        }
    })
}

/// The ambient text color at full strength. For text under a
/// `Button::Transparent`: cosmic's transparent style carries a
/// fully-transparent inherited text color, so un-colored text inside a
/// Transparent button (the recents rows) renders invisible.
fn primary_text() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| cosmic::iced::widget::text::Style {
        color: Some(theme.cosmic().background(false).on.into()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sheet's validity mirrors the engine's name rules exactly
    /// (NewProjectSheet.swift:13-18 → validate_project_name): the button
    /// gates on the same verdict `create_project` will enforce.
    #[test]
    fn name_validation_wires_the_engine_rules() {
        let mut sheet = new_project::NewProject::default();
        assert_eq!(
            sheet.validation_error(),
            Some(ProjectNameError::Empty),
            "empty gates Avanti"
        );
        assert_eq!(sheet.trimmed_name(), "");

        sheet.name = "  Staffa a L  ".into();
        assert_eq!(sheet.trimmed_name(), "Staffa a L");
        assert_eq!(sheet.validation_error(), None, "plain names pass");

        for (bad, expected) in [
            (".hidden", Some(ProjectNameError::LeadingDot)),
            ("a/b", Some(ProjectNameError::BadCharacter('/'))),
            ("a:b", Some(ProjectNameError::BadCharacter(':'))),
        ] {
            sheet.name = bad.into();
            assert_eq!(
                sheet.validation_error(),
                expected,
                "{bad} gates Avanti with the engine verdict"
            );
        }
    }

    /// A create failure lands in the sheet's alert and clears the
    /// awaiting-parent state (NewProjectSheet.swift:56-65's error path).
    #[test]
    fn create_failure_lands_in_the_alert() {
        let mut sheet = new_project::NewProject::default();
        sheet.name = "bracket".into();
        new_project::update(&mut sheet, &new_project::Message::Advance);
        assert!(sheet.awaiting_parent);

        new_project::update(
            &mut sheet,
            &new_project::Message::Created(Err("already exists".into())),
        );
        assert_eq!(sheet.error.as_deref(), Some("already exists"));
        assert!(!sheet.awaiting_parent);
    }

    /// The tilde abbreviation only touches the home prefix.
    #[test]
    fn parent_paths_abbreviate_the_home_prefix() {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return; // no HOME to abbreviate against
        }
        assert_eq!(
            abbreviate_tilde(&Path::new(&home).join("projects")),
            "~/projects"
        );
        assert_eq!(
            abbreviate_tilde(Path::new("/opt")),
            "/opt",
            "paths outside home are untouched"
        );
    }

    /// A real PNG decodes into a handle; a missing file stays the fallback
    /// icon (upstream's `if let NSImage(...)`).
    #[test]
    fn thumbnail_loads_or_falls_back() {
        let dir = std::env::temp_dir().join(format!("cad-thumb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load_thumbnail(&dir.join("schematic.png")), None);

        // a 1×1 red PNG
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        image::save_buffer(
            dir.join("schematic.png"),
            &img,
            1,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        assert!(load_thumbnail(&dir.join("schematic.png")).is_some());
    }
}
