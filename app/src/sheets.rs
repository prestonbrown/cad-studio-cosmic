//! Modal sheets: the FULL AgentMissingSheet port (Agent/AgentMissingSheet.swift),
//! the first-run SetupSheet (App/SetupSheet.swift + PrintPreferences'
//! section), and the PythonSetupSheet port (Python/PythonSetupSheet.swift)
//! with the sheet-side half of PythonEnvironment's state machine — the
//! event-to-state reduction the engine's check/install drivers feed.
//! State machines are plain data the app layer folds messages into; the
//! views render them.
//!
//! The AgentMissingSheet is per-engine (upstream presents one locator per
//! engine; the sheet's `kind` picks the name, install commands, login
//! command, and path field): the custom-path field writes the ENGINE
//! settings key of that engine — the single-writer law's one sanctioned
//! path: the app itself saves the engine settings file. Verify re-locates
//! and dismisses only when a working executable turns up.
//!
//! Cosmetic port notes: upstream's `CodeBlock` becomes a mono block with a
//! copy button; pickers are form pickers ([`ui::picker`]), and the print-preferences
//! section renders as a grouped Form ([`ui::form`]). Sheets are COSMIC
//! dialog cards over the window's dimmed body ([`present`]); the python
//! and agent-missing sheets scroll their content above pinned action rows
//! so nothing clips in a short window; the
//! PythonSetupSheet's DisclosureGroup becomes a toggle button over the
//! mono log. User-visible strings read through `fl!` at render time (the
//! key names the en/main.ftl entry).

use std::collections::HashMap;
use std::path::PathBuf;

use cosmic::iced::Length;
use cosmic::iced::widget::text::Wrapping;
use cosmic::widget::{Column, Row, Space, button, container, scrollable, text_input};
use cosmic::{Element, Task};
use engine::agent::locator::{AgentKind, find_executable};
use engine::python::env::{EnvEvent, EnvFailure, EnvState};
use engine::settings::{MeasurementUnit, PrinterType, Settings};

use crate::ui::{self, Backdrop, Corner, Symbol, Tone};
use crate::{fl, typography};

/// This module's text widget, pinned to cosmic's theme/renderer (the bare
/// alias would default to iced's own theme and stop converting into
/// [`Element`]s — see chat_view.rs).
type Text<'a> = cosmic::iced::widget::Text<'a, cosmic::theme::Theme, cosmic::Renderer>;

/// The install commands the sheet shows per engine
/// (AgentMissingSheet.swift:14-19). The codex arm ports upstream's
/// `npm install -g @openai/codex` ONLY — the second upstream line
/// (`brew install --cask codex`) is macOS-only and does not port.
pub fn install_commands(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => &[
            "curl -fsSL https://claude.ai/install.sh | bash",
            "npm install -g @anthropic-ai/claude-code",
        ],
        AgentKind::Codex => &["npm install -g @openai/codex"],
    }
}

/// The custom-path field's placeholder (upstream
/// `"~/.local/bin/\(engine.command)"`, catalog `local-bin`).
pub fn custom_path_placeholder(kind: AgentKind) -> String {
    fl!("local-bin", command = kind.command())
}

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
    /// The engine the sheet presents install instructions for (upstream
    /// one sheet per locator; the kind comes from the agent_engine setting
    /// when the sheet goes up).
    pub kind: AgentKind,
    /// The custom executable path field. Staged live; persisted into the
    /// engine's path setting (`claude_path`/`codex_path`) when Verify runs.
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
        settings.set_agent_path(
            self.kind,
            (!trimmed.is_empty()).then(|| trimmed.to_string()),
        );
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
/// instruction paragraph, the engine's install command blocks, the
/// custom-path field, the missing line, and Terminal / Close / Verify.
pub fn view_agent_missing(sheet: &AgentMissing) -> Element<'_, AgentMessage> {
    let engine = sheet.kind.display_name();
    let head = Row::new()
        .spacing(8)
        .push(ui::icon::icon(
            Symbol::ExclamationmarkTriangleFill,
            20.0,
            Tone::Warning,
            Backdrop::Plain,
        ))
        .align_y(cosmic::iced::Alignment::Center)
        .push(headline(fl!("non-trovato", agent = engine)));
    let intro = callout(fl!(
        "cad-studio-usa-installato-su-questo-mac-installalo-dal-terminale-con-uno-dei-comandi-seguenti-poi-esegui-per-effettuare-l-accesso",
        arg = engine,
        arg_2 = sheet.kind.login_command()
    ));

    let mut stack = Column::new().spacing(14).push(head).push(intro);
    for command in install_commands(sheet.kind) {
        stack = stack.push(ui::code_block::code_block(
            command,
            None,
            AgentMessage::Copy(command.to_string()),
        ));
    }
    // `LabeledContent` at `.callout`: the label beside the field
    // (AgentMissingSheet.swift:33-38)
    stack = stack
        .push(
            Row::new()
                .spacing(8)
                .align_y(cosmic::iced::Alignment::Center)
                .push(callout(fl!("percorso-dell-eseguibile")))
                .push(
                    text_input::text_input(custom_path_placeholder(sheet.kind), &sheet.custom_path)
                        .on_input(AgentMessage::CustomPath)
                        // Return runs Verify (upstream's default action)
                        .on_submit(|_| AgentMessage::Verify)
                        .font(cosmic::font::mono())
                        .size(typography::CALLOUT)
                        .width(Length::Fill),
                ),
        )
        .push_maybe(sheet.last_verified_missing.then(|| {
            callout(fl!("nessun-eseguibile-funzionante-trovato"))
                .class(Tone::Danger.text(Backdrop::Plain))
        }));
    let actions = Row::new()
        .spacing(8)
        .push(
            button::text(fl!("apri-il-terminale"))
                .class(cosmic::theme::Button::Standard)
                .on_press(AgentMessage::OpenTerminal),
        )
        .push(Space::new().width(Length::Fill))
        .push(
            button::text(fl!("chiudi"))
                .class(cosmic::theme::Button::Standard)
                .on_press(AgentMessage::Close),
        )
        .push(
            button::text(fl!("verifica"))
                .class(cosmic::theme::Button::Suggested)
                .on_press_maybe((!sheet.checking).then_some(AgentMessage::Verify)),
        );
    // Dialog card: opaque, rounded, shadowed, so the dimmed body never
    // shows through the sheet.
    // The content scrolls above the cap; the action row stays pinned.
    let card = Column::new()
        .spacing(14)
        .push(container(scrollable(stack)).max_height(420.0))
        .push(actions);
    container(card)
        .padding(20)
        .width(SHEET_WIDTH)
        .class(cosmic::theme::Container::Dialog(true))
        .into()
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

/// Upstream `PrinterType.title` (PrintPreferences.swift:66-72), catalog
/// keys per option.
pub fn printer_type_title(kind: PrinterType) -> String {
    match kind {
        PrinterType::Fdm => fl!("fdm-filamento"),
        PrinterType::FdmMultiMaterial => fl!("fdm-multimateriale-ams-mmu"),
        PrinterType::Resin => fl!("resina-sla-dlp-msla"),
        PrinterType::Sls => fl!("sls-polvere"),
    }
}

/// Upstream `MeasurementUnit.title` (PrintPreferences.swift:22-30),
/// catalog keys per option.
pub fn measurement_unit_title(unit: MeasurementUnit) -> String {
    match unit {
        MeasurementUnit::Millimeters => fl!("millimetri-mm"),
        MeasurementUnit::Centimeters => fl!("centimetri-cm"),
        MeasurementUnit::Meters => fl!("metri-m"),
        MeasurementUnit::Inches => fl!("pollici-in"),
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

/// One print-preferences edit (the shared section's message): the same
/// three writes upstream's `PrintPreferencesSection` binds with
/// @AppStorage; WHERE they persist is the caller's policy (the first-run
/// sheet batches to Continue, the settings window writes live).
#[derive(Debug, Clone)]
pub enum PrintPrefMessage {
    PrinterType(PrinterType),
    PrinterModel(String),
    Unit(MeasurementUnit),
}

/// The PrintPreferencesSection port (PrintPreferences.swift:111-134): one
/// grouped section — the printer-type picker, the model field, the unit
/// picker — and its footer. One renderer for both surfaces that embed it:
/// the first-run SetupSheet and the settings window's Generale tab
/// (SettingsView.swift:57). Returns the section and the footer as two
/// form parts, for the caller's [`ui::form::form`].
pub fn print_preferences_parts<'a>(
    printer_type: PrinterType,
    printer_model: &'a str,
    unit: MeasurementUnit,
) -> [Element<'a, PrintPrefMessage>; 2] {
    let printer_types = PRINTER_TYPES.map(printer_type_title).to_vec();
    let units = MEASUREMENT_UNITS.map(measurement_unit_title).to_vec();
    let section = ui::form::section(vec![
        ui::form::row(
            fl!("tipo-di-stampante"),
            ui::picker::picker(printer_types, Some(printer_type as usize), |index| {
                PrintPrefMessage::PrinterType(PRINTER_TYPES[index])
            }),
        ),
        ui::form::row(
            fl!("modello-2"),
            ui::form::field(
                fl!("facoltativo-ad-esempio-bambu-lab-p1s"),
                printer_model,
                false,
                PrintPrefMessage::PrinterModel,
            ),
        ),
        ui::form::row(
            fl!("unit-di-misura"),
            ui::picker::picker(units, Some(unit as usize), |index| {
                PrintPrefMessage::Unit(MEASUREMENT_UNITS[index])
            }),
        ),
    ]);
    let footer = ui::form::footer(fl!(
        "l-agente-adatta-spessori-giochi-e-consigli-di-stampa-alla-tua-stampante-e-usa-l-unit-scelta-nelle-risposte-e-nella-tavola-i-file-stl-e-3mf-restano-sempre-in-millimetri"
    ));
    [section, footer]
}

/// SetupSheet (SetupSheet.swift:6-41): the 64 px app icon beside the
/// headline/callout stack (`HStack(spacing: 14)`, :8-22), the
/// print-preferences section as a grouped Form 20 px below it (:23-28),
/// and Continue bottom-right (:29-36), on upstream's 520-wide frame
/// (:39). The picks batch into the settings on Continue (the port's
/// staged write; upstream's section binds @AppStorage live and Continue
/// writes only hasCompletedSetup).
pub fn view_setup(sheet: &Setup) -> Element<'_, SetupMessage> {
    let head = sheet_head(
        fl!("benvenuto-in-cad-studio"),
        fl!("indica-come-stampi-potrai-cambiarlo-in-qualsiasi-momento-nelle-impostazioni"),
        cosmic::iced::Alignment::Center,
    );
    let form = ui::form::form(
        print_preferences_parts(sheet.printer_type, &sheet.printer_model, sheet.unit)
            .into_iter()
            .collect(),
    );
    let form: Element<'_, SetupMessage> = form.map(|message| match message {
        PrintPrefMessage::PrinterType(kind) => SetupMessage::PrinterType(kind),
        PrintPrefMessage::PrinterModel(model) => SetupMessage::PrinterModel(model),
        PrintPrefMessage::Unit(unit) => SetupMessage::Unit(unit),
    });

    let actions = Row::new().push(Space::new().width(Length::Fill)).push(
        button::text(fl!("continua"))
            .class(cosmic::theme::Button::Suggested)
            .on_press(SetupMessage::Continue),
    );
    // the form's footer carries its own bottom gap before Continue
    let stack = Column::new()
        .push(head)
        .push(Space::new().height(ui::form::MARGIN))
        .push(form)
        .push(actions);
    container(stack)
        .padding(20)
        .width(SHEET_WIDTH)
        .class(cosmic::theme::Container::Dialog(true))
        .into()
}

/// The sheets' frame width (`.frame(width: 520)`).
const SHEET_WIDTH: f32 = 520.0;

/// The app-icon head the setup and python sheets open with: the 64 px icon
/// beside the `.headline` title over the secondary `.callout` description
/// (`HStack(spacing: 14)` / `VStack(alignment: .leading, spacing: 4)`).
fn sheet_head<'a, M: 'a>(
    title: String,
    description: String,
    align: cosmic::iced::Alignment,
) -> Element<'a, M> {
    Row::new()
        .spacing(14)
        .align_y(align)
        .push(ui::icon::app(64.0))
        .push(
            Column::new()
                .spacing(4)
                .push(headline(title))
                .push(callout(description).class(Tone::Secondary.text(Backdrop::Plain))),
        )
        .into()
}

/// `card` presented as a sheet over `body`: the body dimmed under the
/// theme's dialog shade, the card centered on it. Presses and wheel
/// scrolls that miss the card land on the shade (`swallow`), never on the
/// body below.
pub fn present<'a, M: Clone + 'a>(
    body: Element<'a, M>,
    card: Element<'a, M>,
    swallow: M,
) -> Element<'a, M> {
    let shade = cosmic::widget::mouse_area(
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::Alignment::Center)
            .align_y(cosmic::iced::Alignment::Center)
            .class(cosmic::theme::Container::custom(|theme| {
                cosmic::iced::widget::container::Style {
                    background: Some(cosmic::iced::Background::Color(theme.cosmic().shade.into())),
                    ..Default::default()
                }
            })),
    )
    .on_press(swallow.clone())
    .on_scroll(move |_| swallow.clone());
    cosmic::iced::widget::Stack::new()
        .push(body)
        .push(shade)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

// ─── python setup sheet ─────────────────────────────────────────────────────

/// The app icon's freedesktop name, which the desktop file and the
/// installed icon carry. Keep in sync with `CadStudio::APP_ID`.
pub const APP_ICON: &str = "net.brownhouse.CadStudio";

/// PythonSetupSheet messages. The driver kicks are the shell's: it acts on
/// `Install` (reinstall through the engine seams) and folds `Event`s into
/// the state as they arrive.
#[derive(Debug, Clone)]
pub enum PythonMessage {
    /// "Fine"/"Più tardi": `isSetupSheetPresented = false` — ignored while
    /// an install runs (upstream `interactiveDismissDisabled`).
    Close,
    /// "Installa": reinstall (the button's action, PythonSetupSheet.swift:78).
    Install,
    /// The "Mostra dettagli" disclosure.
    ToggleLog,
    /// One driver event, in arrival order.
    Event(EnvEvent),
}

/// The sheet-side environment state (upstream `PythonEnvironment`'s
/// observed fields): the reduced [`EnvState`], the appended log, and the
/// progress bar's value + step label. The drivers emit [`EnvEvent`]s; this
/// reducer is the ONLY writer of the state a Ready terminal leaves behind.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupSheetState {
    pub state: EnvState,
    pub log: String,
    pub progress: f64,
    pub step: String,
}

impl SetupSheetState {
    pub fn new() -> Self {
        Self {
            state: EnvState::Unknown,
            log: String::new(),
            progress: 0.0,
            step: String::new(),
        }
    }

    /// Fold one driver event (upstream's appendLog/advance/state writes):
    /// a log line appends with its newline, a progress milestone sets the
    /// bar and the step together, and a terminal sets the state — `Ok`
    /// maps to `Ready`, and the typed failures split `Missing` (its own
    /// state, the presentation gate upstream reads) from the
    /// message-carrying `CheckFailed`/`InstallFailed`.
    pub fn apply(&mut self, event: &EnvEvent) {
        match event {
            EnvEvent::Log(line) => {
                self.log.push_str(&crate::engine_text::env_log(line));
                self.log.push('\n');
            }
            EnvEvent::Progress { value, step } => {
                self.progress = *value;
                self.step = crate::engine_text::env_step(*step);
            }
            EnvEvent::Finished(Ok(ready)) => {
                self.state = EnvState::Ready {
                    build123d_version: ready.build123d_version.clone(),
                };
            }
            EnvEvent::Finished(Err(EnvFailure::Missing)) => self.state = EnvState::Missing,
            EnvEvent::Finished(Err(failure)) => {
                self.state = EnvState::Failed(crate::engine_text::env_failure(failure));
            }
        }
    }

    /// Enter `Checking` — the app's own kick of `check` (the drivers never
    /// emit this; upstream `check()` sets `.checking` first thing,
    /// PythonEnvironment.swift:35). The log is kept: a check adds nothing
    /// to it, and a retry after a failed install should not lose the run.
    pub fn begin_checking(&mut self) {
        self.state = EnvState::Checking;
    }

    /// Enter `Installing` with the previous run cleared (upstream
    /// install()'s `state = .installing; log = ""; progress = 0`,
    /// PythonEnvironment.swift:56-58) — a failed run's log and bar never
    /// bleed into the retry.
    pub fn begin_installing(&mut self) {
        self.state = EnvState::Installing;
        self.log.clear();
        self.progress = 0.0;
        self.step.clear();
    }
}

impl Default for SetupSheetState {
    fn default() -> Self {
        Self::new()
    }
}

/// The sheet's button machine (PythonSetupSheet.swift:61-82): Done when
/// ready, one disabled Installing… while a check or install runs, Later +
/// Install in every other state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupButtons {
    Done,
    Busy,
    LaterAndInstall,
}

/// Classify a state's button row (PythonSetupSheet.swift:63-81).
pub fn buttons(state: &EnvState) -> SetupButtons {
    match state {
        EnvState::Ready { .. } => SetupButtons::Done,
        EnvState::Installing | EnvState::Checking => SetupButtons::Busy,
        EnvState::Unknown | EnvState::Missing | EnvState::Failed(_) => {
            SetupButtons::LaterAndInstall
        }
    }
}

/// Whether the sheet may be dismissed: blocked while an install runs ONLY
/// (upstream `interactiveDismissDisabled(python.isInstalling)`,
/// PythonSetupSheet.swift:58 — a check does NOT block dismissal).
pub fn dismiss_blocked(state: &EnvState) -> bool {
    matches!(state, EnvState::Installing)
}

/// The PythonSetupSheet's whole state: the reduced environment state plus
/// the sheet's own log disclosure (upstream `@State private var showsLog`)
/// and presentation flag (upstream `PythonEnvironment.isSetupSheetPresented`
/// — the state outlives the sheet, so a check runs at startup whether or
/// not the sheet is up).
#[derive(Debug, Default)]
pub struct PythonSetup {
    pub state: SetupSheetState,
    pub shows_log: bool,
    pub presented: bool,
}

/// PythonSetupSheet (PythonSetupSheet.swift:7-59): the app-icon head with
/// the corpus description, the progress bar + step while an install runs
/// (or the bar already moved), the failure line, the "Show Details"
/// disclosure over the monospaced log, and the state-driven button row.
/// Width 520, padding 20.
pub fn view_python_setup(sheet: &PythonSetup) -> Element<'_, PythonMessage> {
    let head = sheet_head(
        fl!("installazione-componenti"),
        fl!(
            "cad-studio-usa-un-ambiente-python-dedicato-con-build123d-matplotlib-e-numpy-per-generare-modelli-e-tavole-tecniche-l-installazione-scarica-circa-200-mb"
        ),
        cosmic::iced::Alignment::Start,
    );
    let state = &sheet.state;
    let mut stack = Column::new().spacing(14).push(head);
    // the bar shows while installing, or once any progress moved (a failed
    // run keeps its last position — upstream's same condition, :23)
    let installing = matches!(state.state, EnvState::Installing);
    if installing || state.progress > 0.0 {
        stack = stack.push(
            Column::new()
                .spacing(4)
                .push(cosmic::iced::widget::progress_bar(
                    0.0..=1.0,
                    state.progress as f32,
                ))
                .push(callout(state.step.clone()).class(Tone::Secondary.text(Backdrop::Plain))),
        );
    }
    if let EnvState::Failed(message) = &state.state {
        stack = stack.push(
            Row::new()
                .spacing(8)
                .push(ui::icon::multicolor(
                    Symbol::XmarkOctagonFillMulticolor,
                    18.0,
                ))
                .push(callout(message.clone()).class(Tone::Danger.text(Backdrop::Plain))),
        );
    }
    // the disclosure (upstream DisclosureGroup): the label toggles the
    // mono log's visibility and stays "Show Details" either way — the
    // Text class, not Transparent: cosmic's transparent style carries a
    // fully-transparent inherited text color, which renders
    // button::text's self-colored label invisible
    stack = stack.push(
        button::text(fl!("mostra-dettagli"))
            .class(cosmic::theme::Button::Text)
            .on_press(PythonMessage::ToggleLog),
    );
    if sheet.shows_log {
        stack = stack.push(
            container(
                cosmic::widget::scrollable(
                    cosmic::widget::text(state.log.clone())
                        .font(cosmic::font::mono())
                        .size(11)
                        .wrapping(Wrapping::Glyph),
                )
                .height(180.0),
            )
            .padding(8)
            .width(Length::Fill)
            // `textBackgroundColor` in a 6 pt rounded rectangle
            .class(cosmic::theme::Container::custom(|theme| {
                cosmic::iced::widget::container::Style {
                    background: Some(cosmic::iced::Background::Color(
                        ui::surface::text_background(theme),
                    )),
                    border: cosmic::iced::Border {
                        radius: Corner::S.radius(theme),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })),
        );
    }
    let actions = Row::new()
        .spacing(8)
        .push(Space::new().width(Length::Fill))
        .push(python_buttons(&state.state));
    // Dialog card (opaque) with the log-bearing content scrollable above
    // the cap and the state-driven buttons pinned below it.
    let card = Column::new()
        .spacing(14)
        .push(container(scrollable(stack)).max_height(420.0))
        .push(actions);
    container(card)
        .padding(20)
        .width(SHEET_WIDTH)
        .class(cosmic::theme::Container::Dialog(true))
        .into()
}

/// The button row for one state (PythonSetupSheet.swift:61-82): the busy
/// button carries no press handler, matching upstream's `.disabled(true)`.
fn python_buttons(state: &EnvState) -> Element<'static, PythonMessage> {
    match buttons(state) {
        SetupButtons::Done => button::text(fl!("fine"))
            .class(cosmic::theme::Button::Suggested)
            .on_press(PythonMessage::Close)
            .into(),
        SetupButtons::Busy => button::text(fl!("installazione-in-corso")).into(),
        SetupButtons::LaterAndInstall => Row::new()
            .spacing(8)
            .push(
                button::text(fl!("pi-tardi"))
                    .class(cosmic::theme::Button::Standard)
                    .on_press(PythonMessage::Close),
            )
            .push(
                button::text(fl!("installa"))
                    .class(cosmic::theme::Button::Suggested)
                    .on_press(PythonMessage::Install),
            )
            .into(),
    }
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

/// `.headline`: semibold at body size.
fn headline<'a>(label: impl Into<String>) -> Text<'a> {
    body(label)
        .size(typography::HEADLINE)
        .font(crate::typography::font(cosmic::iced::font::Weight::Semibold))
}

/// `.callout`.
fn callout<'a>(label: impl Into<String>) -> Text<'a> {
    body(label).size(typography::CALLOUT)
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

        // the codex sheet stages into the CODEX key and never touches
        // claude's (upstream binds the field to engine.pathKey)
        let mut sheet = AgentMissing {
            kind: AgentKind::Codex,
            custom_path: "  ~/.cargo/bin/codex  ".into(),
            ..Default::default()
        };
        sheet.stage_custom_path(&mut settings);
        assert_eq!(settings.codex_path.as_deref(), Some("~/.cargo/bin/codex"));
        assert_eq!(settings.claude_path.as_deref(), Some("/opt/claude"));
        sheet.custom_path.clear();
        sheet.stage_custom_path(&mut settings);
        assert_eq!(settings.codex_path, None);
    }

    /// The install command blocks per engine (AgentMissingSheet.swift:14-19)
    /// — the Claude pair verbatim; the codex arm upstream pairs
    /// `npm install -g @openai/codex` with `brew install --cask codex`,
    /// which is macOS-only and does not port (the LINUX ruling leaves npm
    /// as the one codex command).
    #[test]
    fn install_commands_per_engine() {
        assert_eq!(
            install_commands(AgentKind::Claude),
            [
                "curl -fsSL https://claude.ai/install.sh | bash",
                "npm install -g @anthropic-ai/claude-code",
            ]
        );
        assert_eq!(
            install_commands(AgentKind::Codex),
            ["npm install -g @openai/codex"],
            "brew is macOS-only; npm only on Linux"
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

    /// Element construction is pure (the python sheet's construction-test
    /// pattern): the setup sheet builds a frame for every option its
    /// pickers offer, in both the shared section's own context and the
    /// sheet's.
    #[test]
    fn setup_sheet_view_builds_in_every_picker_shape() {
        let mut sheet = Setup::from_settings(&Settings::default());
        let _ = view_setup(&sheet);
        for kind in PRINTER_TYPES {
            sheet.printer_type = kind;
            let _ = view_setup(&sheet);
        }
        for unit in MEASUREMENT_UNITS {
            sheet.unit = unit;
            let _ = view_setup(&sheet);
        }
        sheet.printer_model = "Bambu Lab P1S".into();
        let _ = view_setup(&sheet);
        let _ = print_preferences_parts(
            PrinterType::FdmMultiMaterial,
            &String::new(),
            MeasurementUnit::Inches,
        );
    }
}
