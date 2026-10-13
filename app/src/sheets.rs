//! Modal sheets: the FULL AgentMissingSheet port (Agent/AgentMissingSheet.swift)
//! and the first-run SetupSheet (App/SetupSheet.swift + PrintPreferences'
//! section). State machines are plain data the app layer folds messages
//! into; the views render them.
//!
//! The AgentMissingSheet's custom-path field writes the ENGINE settings
//! (`settings.claude_path`) — the single-writer law's one sanctioned path:
//! the app itself saves the engine settings file. Verify re-locates and
//! dismisses only when a working executable turns up.
//!
//! Cosmetic port notes: upstream's `CodeBlock` and pickers become a mono
//! block with a copy button and radio-style option lists (no nested popup
//! inside a modal). User-visible strings follow the landed modules'
//! convention: plain English values, each named for its en/main.ftl key,
//! pending the repo-wide `fl!()` swap.

use std::collections::HashMap;
use std::path::PathBuf;

use cosmic::iced::Color;
use cosmic::iced::Length;
use cosmic::iced::widget::container::Style as ContainerStyle;
use cosmic::iced::widget::text::Wrapping;
use cosmic::widget::{Column, Row, Space, button, container, text_input};
use cosmic::{Element, Task};
use engine::agent::locator::find_executable;
use engine::settings::{MeasurementUnit, PrinterType, Settings};

use crate::activity::ENGINE_NAME;

/// This module's text widget, pinned to cosmic's theme/renderer (the bare
/// alias would default to iced's own theme and stop converting into
/// [`Element`]s — see chat_view.rs).
type Text<'a> = cosmic::iced::widget::Text<'a, cosmic::theme::Theme, cosmic::Renderer>;

/// Both install commands the sheet shows for the Claude engine
/// (AgentMissingSheet.swift:14-19 — the codex arm has no engine yet).
pub const CLAUDE_INSTALL_COMMANDS: [&str; 2] = [
    "curl -fsSL https://claude.ai/install.sh | bash",
    "npm install -g @anthropic-ai/claude-code",
];

/// What to run to sign in (AgentEngine.loginCommand for claude).
pub const LOGIN_COMMAND: &str = "claude";

/// The custom-path field's placeholder (upstream
/// `"~/.local/bin/\(engine.command)"`).
pub const CUSTOM_PATH_PLACEHOLDER: &str = "~/.local/bin/claude";

// ─── agent-missing sheet ───────────────────────────────────────────────────

/// AgentMissingSheet messages. The effects (locate, launch a terminal) are
/// the shell's: it acts on `OpenTerminal`/`Verify`, then folds the message
/// into the state.
#[derive(Debug, Clone)]
pub enum AgentMessage {
    CustomPath(String),
    OpenTerminal,
    Close,
    Verify,
    /// A Verify finished: `false` = still nothing working.
    Verified(bool),
    /// The copy button on an install-command block.
    Copy(String),
}

/// AgentMissingSheet state.
#[derive(Debug, Default)]
pub struct AgentMissing {
    /// The custom executable path field. Staged live; persisted into
    /// `settings.claude_path` when Verify runs.
    pub custom_path: String,
    /// A Verify is in flight (upstream `locator.state == .checking`).
    pub checking: bool,
    /// The last Verify found nothing working (upstream's red
    /// "Nessun eseguibile funzionante trovato." line).
    pub last_verified_missing: bool,
}

impl AgentMissing {
    /// Stage the custom path into the engine settings (upstream's
    /// `@AppStorage` binding is live; the port batches the disk write to
    /// Verify, the only moment the value is consumed).
    pub fn stage_custom_path(&self, settings: &mut Settings) {
        let trimmed = self.custom_path.trim();
        settings.claude_path = (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
}

/// Fold one message into the sheet state. The shell-owned variants
/// (`OpenTerminal`, `Close`, `Verify`) change nothing here.
pub fn update_agent_missing(
    sheet: &mut AgentMissing,
    message: &AgentMessage,
) -> Task<AgentMessage> {
    match message {
        AgentMessage::CustomPath(path) => sheet.custom_path = path.clone(),
        AgentMessage::Verified(found) => {
            sheet.checking = false;
            sheet.last_verified_missing = !found;
        }
        AgentMessage::Copy(code) => return cosmic::iced::clipboard::write(code.clone()),
        AgentMessage::OpenTerminal | AgentMessage::Close | AgentMessage::Verify => {}
    }
    Task::none()
}

/// AgentMissingSheet (AgentMissingSheet.swift:21-73): warning head, the
/// instruction paragraph, both install command blocks, the custom-path
/// field, the missing line, and Terminal / Close / Verify.
pub fn view_agent_missing(sheet: &AgentMissing) -> Element<'_, AgentMessage> {
    let head = Row::new()
        .spacing(8)
        .push(icon("dialog-warning", 20))
        .push(bold(format!("{ENGINE_NAME} not found"), 18.0));
    let intro = body(format!(
        "CAD Studio uses {ENGINE_NAME} installed on this computer. Install it from a terminal with one of the commands below, then run `{LOGIN_COMMAND}` to sign in."
    ));

    let mut stack = Column::new().spacing(14).push(head).push(intro);
    for command in CLAUDE_INSTALL_COMMANDS {
        stack = stack.push(code_block(command));
    }
    stack = stack
        .push(body("Path to the executable:".to_string()))
        .push(
            text_input::text_input(CUSTOM_PATH_PLACEHOLDER, &sheet.custom_path)
                .on_input(AgentMessage::CustomPath)
                // Return runs Verify (upstream's default action)
                .on_submit(|_| AgentMessage::Verify)
                .font(cosmic::font::mono())
                .size(14),
        )
        .push_maybe(
            sheet
                .last_verified_missing
                .then(|| body("No working executable found.".to_string()).class(error_text())),
        )
        .push(
            Row::new()
                .spacing(8)
                .push(
                    button::text("Open Terminal")
                        .class(cosmic::theme::Button::Standard)
                        .on_press(AgentMessage::OpenTerminal),
                )
                .push(Space::new().width(Length::Fill))
                .push(
                    button::text("Close")
                        .class(cosmic::theme::Button::Standard)
                        .on_press(AgentMessage::Close),
                )
                .push(
                    button::text("Verify")
                        .class(cosmic::theme::Button::Suggested)
                        .on_press_maybe((!sheet.checking).then_some(AgentMessage::Verify)),
                ),
        );
    container(stack).padding(20).width(560.0).into()
}

// ─── first-run setup sheet ─────────────────────────────────────────────────

/// SetupSheet messages.
#[derive(Debug, Clone)]
pub enum SetupMessage {
    PrinterType(PrinterType),
    PrinterModel(String),
    Unit(MeasurementUnit),
    Continue,
}

/// SetupSheet state: the print preferences form (PrintPreferences.swift's
/// section) starting from the engine settings' current values.
#[derive(Debug, Clone)]
pub struct Setup {
    pub printer_type: PrinterType,
    pub printer_model: String,
    pub unit: MeasurementUnit,
}

impl Setup {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            printer_type: settings.printer_type,
            printer_model: settings.printer_model.clone().unwrap_or_default(),
            unit: settings.measurement_unit,
        }
    }

    /// Fold the form into the engine settings for "Continua"
    /// (SetupSheet.swift:31-34 writes the three preference keys).
    pub fn apply(&self, settings: &mut Settings) {
        settings.printer_type = self.printer_type;
        settings.printer_model =
            (!self.printer_model.trim().is_empty()).then(|| self.printer_model.trim().to_string());
        settings.measurement_unit = self.unit;
    }

    pub fn update(&mut self, message: &SetupMessage) {
        match message {
            SetupMessage::PrinterType(kind) => self.printer_type = *kind,
            SetupMessage::PrinterModel(model) => self.printer_model = model.clone(),
            SetupMessage::Unit(unit) => self.unit = *unit,
            SetupMessage::Continue => {}
        }
    }
}

/// Upstream `PrinterType.title` (PrintPreferences.swift:66-72).
pub fn printer_type_title(kind: PrinterType) -> &'static str {
    match kind {
        PrinterType::Fdm => "FDM (filament)",
        PrinterType::FdmMultiMaterial => "Multi-material FDM (AMS, MMU)",
        PrinterType::Resin => "Resin (SLA, DLP, MSLA)",
        PrinterType::Sls => "SLS (powder)",
    }
}

/// Upstream `MeasurementUnit.title` (PrintPreferences.swift:22-30).
pub fn measurement_unit_title(unit: MeasurementUnit) -> &'static str {
    match unit {
        MeasurementUnit::Millimeters => "Millimeters (mm)",
        MeasurementUnit::Centimeters => "Centimeters (cm)",
        MeasurementUnit::Meters => "Meters (m)",
        MeasurementUnit::Inches => "Inches (in)",
    }
}

pub const PRINTER_TYPES: [PrinterType; 4] = [
    PrinterType::Fdm,
    PrinterType::FdmMultiMaterial,
    PrinterType::Resin,
    PrinterType::Sls,
];

pub const MEASUREMENT_UNITS: [MeasurementUnit; 4] = [
    MeasurementUnit::Millimeters,
    MeasurementUnit::Centimeters,
    MeasurementUnit::Meters,
    MeasurementUnit::Inches,
];

/// SetupSheet (SetupSheet.swift:7-41): welcome head, the print preferences
/// form, Continue. Pickers render as radio-style option lists (no nested
/// popup inside the modal).
pub fn view_setup(sheet: &Setup) -> Element<'_, SetupMessage> {
    let head = Column::new()
        .spacing(4)
        .push(bold("Welcome to CAD Studio".to_string(), 18.0))
        .push(body(
            "Tell us how you print: you can change this any time in Settings.".to_string(),
        ));

    let mut form = Column::new().spacing(16);
    form = form.push(option_list(
        "Printer type:",
        PRINTER_TYPES.map(printer_type_title),
        sheet.printer_type as usize,
        |index| SetupMessage::PrinterType(PRINTER_TYPES[index]),
    ));
    form = form
        .push(
            Column::new()
                .spacing(6)
                .push(body("Printer model:".to_string()))
                .push(
                    text_input::text_input(
                        "Optional, e.g. Bambu Lab P1S",
                        &sheet.printer_model,
                    )
                    .on_input(SetupMessage::PrinterModel)
                    .size(14),
                ),
        )
        .push(option_list(
            "Measurement unit:",
            MEASUREMENT_UNITS.map(measurement_unit_title),
            sheet.unit as usize,
            |index| SetupMessage::Unit(MEASUREMENT_UNITS[index]),
        ))
        .push(
            body("The agent adapts wall thickness, clearances and printing advice to your printer and uses the chosen unit in responses and drawings. STL and 3MF files always stay in millimeters.".to_string())
                .class(secondary_text()),
        );

    let stack = Column::new().spacing(18).push(head).push(form).push(
        Row::new().push(Space::new().width(Length::Fill)).push(
            button::text("Continue")
                .class(cosmic::theme::Button::Suggested)
                .on_press(SetupMessage::Continue),
        ),
    );
    container(stack).padding(20).width(560.0).into()
}

/// One labeled radio-style option list; `pick` maps the option index to the
/// typed message. The picked row carries the check icon; unpicked rows
/// reserve the icon column so the labels align (upstream's Picker rows).
fn option_list<'a>(
    label: &str,
    options: [&'a str; 4],
    selected: usize,
    pick: fn(usize) -> SetupMessage,
) -> Element<'a, SetupMessage> {
    let mut list = Column::new().spacing(4).push(body(label.to_string()));
    for (index, option) in options.into_iter().enumerate() {
        let picked = index == selected;
        // the mark column: a check icon when picked, reserved blank space
        // otherwise, so the labels align (upstream's Picker rows)
        let mark: Element<'_, SetupMessage> = if picked {
            icon("object-select", 14).into()
        } else {
            Space::new().width(14).into()
        };
        list = list.push(
            button::custom(
                Row::new()
                    .spacing(8)
                    .push(mark)
                    .push(body(option.to_string()))
                    .width(Length::Fill),
            )
            .class(cosmic::theme::Button::Transparent)
            .padding([4, 8])
            .on_press(pick(index)),
        );
    }
    list.into()
}

// ─── terminal launch ───────────────────────────────────────────────────────

/// The terminal emulators "Open Terminal" tries, most preferred first:
/// COSMIC's own terminal, then the Debian-style metaname, then the common
/// desktop terminals, ending at the X11 fallback (upstream opens
/// Terminal.app — macOS has exactly one).
pub const TERMINAL_PREFERENCE: [&str; 9] = [
    "cosmic-term",
    "x-terminal-emulator",
    "kgx",
    "gnome-terminal",
    "konsole",
    "foot",
    "kitty",
    "alacritty",
    "xterm",
];

/// The first terminal in preference order the environment can actually run
/// (engine `ShellEnvironment.locate` semantics: PATH order with
/// `~/.local/bin` appended).
pub fn terminal_launcher(env: &HashMap<String, String>) -> Option<PathBuf> {
    TERMINAL_PREFERENCE
        .into_iter()
        .find_map(|name| find_executable(name, env))
}

// ─── shared view helpers (chat_view.rs's idioms) ──────────────────────────

/// Default-body text that wraps at word boundaries, owned.
fn body<'a>(label: impl Into<String>) -> Text<'a> {
    cosmic::widget::text(label.into()).wrapping(Wrapping::Word)
}

fn bold<'a>(label: impl Into<String>, size: f32) -> Text<'a> {
    body(label).size(size).font(cosmic::iced::font::Font {
        weight: cosmic::iced::font::Weight::Bold,
        ..cosmic::font::default()
    })
}

/// A themed icon widget by freedesktop name; size in px (cosmetic layer).
fn icon(name: &str, size: u16) -> cosmic::widget::Icon {
    cosmic::widget::icon::from_name(name).size(size).icon()
}

fn secondary_text() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| {
        let mut color = Color::from(theme.cosmic().background(false).on);
        color.a *= 0.65;
        cosmic::iced::widget::text::Style {
            color: Some(color),
            ..Default::default()
        }
    })
}

fn error_text() -> cosmic::theme::Text {
    cosmic::theme::Text::Color(Color::from_rgb(0.90, 0.32, 0.32))
}

/// The sheet's code block: mono text on a subtle rounded fill with a copy
/// button (upstream `CodeBlock`, MessageView.swift:114-144's shape).
fn code_block(code: &str) -> Element<'static, AgentMessage> {
    container(
        Row::new()
            .spacing(8)
            .push(
                cosmic::widget::text(code.to_string())
                    .font(cosmic::font::mono())
                    .wrapping(Wrapping::Glyph)
                    .width(Length::Fill),
            )
            .push(
                button::icon(
                    cosmic::widget::icon::from_name("edit-copy")
                        .size(14)
                        .handle(),
                )
                .on_press(AgentMessage::Copy(code.to_string()))
                .name("copy"),
            ),
    )
    .padding(8)
    .width(Length::Fill)
    .class(subtle_block())
    .into()
}

fn subtle_block() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let mut base = Color::from(theme.cosmic().background(false).component.base);
        base.a *= 0.5;
        let mut border_color = Color::from(theme.cosmic().background(false).on);
        border_color.a = 0.15;
        ContainerStyle {
            background: Some(cosmic::iced::Background::Color(base)),
            border: cosmic::iced::Border {
                color: border_color,
                width: 1.0,
                radius: theme.cosmic().corner_radii.radius_s.into(),
            },
            ..Default::default()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The custom path field round-trips into the engine settings: a typed
    /// path lands verbatim (trimmed), an emptied field clears the setting
    /// (upstream's empty string reads as "no custom path").
    #[test]
    fn agent_missing_custom_path_round_trips_into_settings() {
        let mut sheet = AgentMissing::default();
        let mut settings = Settings::default();
        assert_eq!(settings.claude_path, None);

        sheet.custom_path = "  ~/.local/bin/claude  ".into();
        sheet.stage_custom_path(&mut settings);
        assert_eq!(settings.claude_path.as_deref(), Some("~/.local/bin/claude"));

        sheet.custom_path.clear();
        sheet.stage_custom_path(&mut settings);
        assert_eq!(
            settings.claude_path, None,
            "an emptied field clears the path"
        );

        // the staged value survives an engine save/load round trip
        sheet.custom_path = "/opt/claude".into();
        sheet.stage_custom_path(&mut settings);
        let dir = std::env::temp_dir().join(format!("cad-sheet-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        settings.save(&path).unwrap();
        let reloaded = Settings::load(&path).unwrap();
        assert_eq!(reloaded.claude_path.as_deref(), Some("/opt/claude"));
    }

    /// Both install command blocks are the upstream Claude pair, verbatim
    /// (AgentMissingSheet.swift:16).
    #[test]
    fn install_commands_match_upstream() {
        assert_eq!(
            CLAUDE_INSTALL_COMMANDS,
            [
                "curl -fsSL https://claude.ai/install.sh | bash",
                "npm install -g @anthropic-ai/claude-code",
            ]
        );
    }

    /// The terminal preference order: COSMIC's terminal first, the X11
    /// fallback last, and lookup takes the first PRESENT candidate — not
    /// merely the first in the table.
    #[test]
    fn terminal_launch_preference_order() {
        assert_eq!(TERMINAL_PREFERENCE[0], "cosmic-term");
        assert_eq!(TERMINAL_PREFERENCE.last(), Some(&"xterm"));
        assert_eq!(TERMINAL_PREFERENCE.len(), 9);
        // no duplicates would make the order meaningless
        let mut sorted = TERMINAL_PREFERENCE.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), TERMINAL_PREFERENCE.len());

        let dir = std::env::temp_dir().join(format!("cad-term-{}", uuid::Uuid::new_v4()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // only a lower-preference terminal exists
        for name in ["gnome-terminal", "xterm"] {
            let p = bin.join(name);
            std::fs::write(&p, b"#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let env: HashMap<String, String> =
            HashMap::from([("PATH".to_string(), bin.to_string_lossy().into_owned())]);
        assert_eq!(
            terminal_launcher(&env),
            Some(bin.join("gnome-terminal")),
            "the first present candidate in preference order wins"
        );

        let empty: HashMap<String, String> = HashMap::new();
        // a found launcher is always one of the table's names
        let found = terminal_launcher(&empty);
        let known = TERMINAL_PREFERENCE.contains(
            &found
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or_default(),
        );
        assert!(
            found.is_none() || known,
            "a found launcher is always one of the table's names: {found:?}"
        );
    }

    /// The SetupSheet folds its form into the engine settings, matching
    /// upstream's three preference keys (SetupSheet.swift:31-34).
    #[test]
    fn setup_applies_print_preferences_into_settings() {
        let mut settings = Settings::default();
        let mut setup = Setup::from_settings(&settings);
        assert_eq!(setup.printer_type, PrinterType::Fdm);
        assert_eq!(setup.unit, MeasurementUnit::Millimeters);

        setup.update(&SetupMessage::PrinterType(PrinterType::Resin));
        setup.update(&SetupMessage::PrinterModel("  Bambu Lab P1S ".into()));
        setup.update(&SetupMessage::Unit(MeasurementUnit::Inches));
        setup.apply(&mut settings);

        assert_eq!(settings.printer_type, PrinterType::Resin);
        assert_eq!(settings.printer_model.as_deref(), Some("Bambu Lab P1S"));
        assert_eq!(settings.measurement_unit, MeasurementUnit::Inches);

        setup.update(&SetupMessage::PrinterModel("   ".into()));
        setup.apply(&mut settings);
        assert_eq!(
            settings.printer_model, None,
            "a blank model clears the field"
        );
    }
}
