//! The settings window (upstream SettingsView.swift): four tabs — Generale,
//! Agente, Python, Avanzate — in a separate non-modal window (the
//! sheets' body-replacement pattern is for modal gates; settings must not
//! block the app). Every field writes through the ONE-writer law: the
//! engine settings.json via load-modify-save (the app layer owns the
//! save), never cosmic-config — cosmic-config stays window/UI state, and
//! the selected tab is deliberately ephemeral.
//!
//! Upstream citations: Generale SettingsView.swift:23-62 (location +
//! Scegli…, language picker writing the override with the next-launch
//! footer, PrintPreferencesSection :57), Agente :64-169 (engine segmented
//! picker + footer, per-engine AgentEngineSection :116-169 — executable
//! and version by locate state, custom path, model, Verifica; effort
//! picker :87-96; claude-only restricted-bash :97-103; codex-only sandbox
//! note :104-109), Python :171-215 (interpreter, build123d by state,
//! Verifica + Reinstalla…, packages footer), Avanzate :217-242 (streaming
//! picker, isolate toggle + apiKeyHelper footer). About
//! (Commands.swift:141-152): the credits paragraph is the catalog's
//! literal-\n key (one of the two), the version the crate's own.
//!
//! A quirk note: upstream's engine-picker footer says existing
//! projects "continue with the agent chosen here", but the code re-reads
//! the setting per turn — a mid-conversation flip redirects the very next
//! turn of every open project. The footer text is upstream's verbatim;
//! the code wins (an upstream quirk, recorded here rather than papered
//! over).
//!
//! The state is a plain mirror the app seeds on open and every write
//! folds into twice — once here ([`SettingsState::update`]), once into
//! the persisted settings ([`stage`]). The effects (folder portal,
//! locates, python check/reinstall) belong to the app layer, which maps
//! the effect messages and reports the outcomes back.

use std::path::{Path, PathBuf};

use cosmic::Element;
use cosmic::iced::Alignment;
use cosmic::iced::Length;
use cosmic::iced::font::Weight;
use cosmic::iced::widget::text::Wrapping;
use cosmic::widget::{Column, Row, Space, container, scrollable};
use engine::agent::locator::{AgentKind, AgentPath};
use engine::python::env::EnvState;
use engine::settings::{MeasurementUnit, PrinterType, Settings};

use crate::sheets::PrintPrefMessage;
use crate::ui::{self, Backdrop, Corner, Symbol, Tone, form};
use crate::{fl, typography};

/// Upstream's window geometry (SettingsView.swift:19 `.frame(width: 600)`).
pub const WIDTH: f32 = 600.0;
/// The window height for `tab` with `engine` selected: upstream's Settings
/// window fits each tab's content (shots/upstream/settings-*.png, 1x:
/// General 413, Agent 633, Python 260, Advanced 323), so the port resizes
/// on every tab or engine switch. Each value is the tab's content under
/// the port's header plus the form's bottom margin, measured at the
/// default text scale in English; the body still scrolls should a longer
/// translation or a larger text scale run past it.
pub fn height(tab: Tab, engine: AgentKind) -> f32 {
    match (tab, engine) {
        (Tab::General, _) => 463.0,
        (Tab::Agent, AgentKind::Claude) => 667.0,
        (Tab::Agent, AgentKind::Codex) => 613.0,
        (Tab::Python, _) => 279.0,
        (Tab::Advanced, _) => 360.0,
    }
}

/// The About window's geometry: AppKit's standard About panel, 284 wide
/// (shots/upstream/about.png), the header, then the content as tall as
/// its text runs.
pub const ABOUT_WIDTH: f32 = 284.0;
pub const ABOUT_HEIGHT: f32 = 270.0;
/// The About content's top inset under the header: the icon's frame
/// starts 8 px below the titlebar, as under AppKit's.
const ABOUT_TOP: f32 = 8.0;

/// The two engines, picker order (AgentEngine.allCases).
pub const ENGINES: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];

/// [`ENGINES`]'s index for `kind` (Claude first, per upstream).
pub(crate) fn engine_index(kind: AgentKind) -> usize {
    match kind {
        AgentKind::Claude => 0,
        AgentKind::Codex => 1,
    }
}

/// This module's text widget, pinned to cosmic's theme/renderer (the bare
/// alias would default to iced's own theme and stop converting into
/// [`Element`]s — see chat_view.rs).
type Text<'a> = cosmic::iced::widget::Text<'a, cosmic::theme::Theme, cosmic::Renderer>;

// ─── the four tabs ──────────────────────────────────────────────────────────

/// The four tabs (SettingsView.swift:6-17), opened at Generale like a
/// fresh upstream Settings scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    General,
    Agent,
    Python,
    Advanced,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::General, Tab::Agent, Tab::Python, Tab::Advanced];

    /// The tab's title (its catalog key: `generale`, `agente`, `python`,
    /// `avanzate`).
    pub fn title(self) -> String {
        match self {
            Tab::General => fl!("generale"),
            Tab::Agent => fl!("agente"),
            Tab::Python => fl!("python"),
            Tab::Advanced => fl!("avanzate"),
        }
    }

    /// The tab item's symbol (SettingsView.swift:6-17's `systemImage`).
    pub fn symbol(self) -> Symbol {
        match self {
            Tab::General => Symbol::Gearshape,
            Tab::Agent => Symbol::Sparkles,
            Tab::Python => Symbol::ChevronLeftForwardslashChevronRight,
            Tab::Advanced => Symbol::Gearshape2,
        }
    }
}

// ─── picker-backed settings values ──────────────────────────────────────────

/// The Generale tab's language picker (SettingsView.swift:257-273,
/// upstream `AppLanguage`): `System` clears the override so the desktop
/// request decides again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageChoice {
    System,
    Italiano,
    English,
    Deutsch,
    Francais,
}

impl LanguageChoice {
    pub const ALL: [LanguageChoice; 5] = [
        LanguageChoice::System,
        LanguageChoice::Italiano,
        LanguageChoice::English,
        LanguageChoice::Deutsch,
        LanguageChoice::Francais,
    ];

    /// The picker row's title. The language names are proper nouns shared
    /// by every catalog (upstream hardcodes them in `AppLanguage.title`).
    pub fn title(self) -> String {
        match self {
            LanguageChoice::System => fl!("lingua-di-sistema"),
            LanguageChoice::Italiano => fl!("italiano"),
            LanguageChoice::English => fl!("english"),
            LanguageChoice::Deutsch => fl!("deutsch"),
            LanguageChoice::Francais => fl!("fran-ais"),
        }
    }

    /// The persisted override (`settings.language`): `None` follows the
    /// desktop request (upstream removes `AppleLanguages`).
    pub fn as_setting(self) -> Option<&'static str> {
        match self {
            LanguageChoice::System => None,
            LanguageChoice::Italiano => Some("it"),
            LanguageChoice::English => Some("en"),
            LanguageChoice::Deutsch => Some("de"),
            LanguageChoice::Francais => Some("fr"),
        }
    }

    /// Map a stored override back to the picker's selection; an unknown
    /// tag reads System (upstream's picker would show nothing selected —
    /// the port picks the safe row).
    pub fn from_setting(language: Option<&str>) -> Self {
        match language {
            Some("it") => LanguageChoice::Italiano,
            Some("en") => LanguageChoice::English,
            Some("de") => LanguageChoice::Deutsch,
            Some("fr") => LanguageChoice::Francais,
            _ => LanguageChoice::System,
        }
    }
}

/// The Agente tab's effort picker (SettingsView.swift:87-92): "Predefinito
/// dell'agente" is the empty string upstream stores — the engine's own
/// default, no effort flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effort {
    #[default]
    AgentDefault,
    Low,
    Medium,
    High,
}

impl Effort {
    pub const ALL: [Effort; 4] = [
        Effort::AgentDefault,
        Effort::Low,
        Effort::Medium,
        Effort::High,
    ];

    pub fn title(self) -> String {
        match self {
            Effort::AgentDefault => fl!("predefinito-dell-agente"),
            Effort::Low => fl!("basso-pi-veloce"),
            Effort::Medium => fl!("medio"),
            Effort::High => fl!("alto-pi-accurato"),
        }
    }

    /// The stored `claude_effort` value (`None` = upstream's empty string).
    pub fn as_setting(self) -> Option<&'static str> {
        match self {
            Effort::AgentDefault => None,
            Effort::Low => Some("low"),
            Effort::Medium => Some("medium"),
            Effort::High => Some("high"),
        }
    }

    /// Map a stored effort back to the picker's selection; unknown strings
    /// read AgentDefault.
    pub fn from_setting(effort: Option<&str>) -> Self {
        match effort {
            Some("low") => Effort::Low,
            Some("medium") => Effort::Medium,
            Some("high") => Effort::High,
            _ => Effort::AgentDefault,
        }
    }
}

// ─── the locator row ────────────────────────────────────────────────────────

/// One engine's locate row (upstream `AgentLocator.State`), as the Agente
/// tab's Eseguibile/Versione pair renders it (SettingsView.swift:129-151).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LocateState {
    /// Nothing asked yet (upstream `.unknown`, rendered "—").
    #[default]
    Unknown,
    /// A locate is in flight (upstream `.checking`, rendered as a spinner).
    Checking,
    /// A working executable + its `--version` output.
    Found { path: PathBuf, version: String },
    /// Nothing working (upstream `.missing`, rendered red).
    Missing,
}

impl LocateState {
    /// A locate kicked for this engine (the section's auto-locate on
    /// appear, SettingsView.swift:163-167, and the Verifica button).
    pub fn begin(&mut self) {
        *self = LocateState::Checking;
    }

    /// A locate finished: a validated executable lands Found, everything
    /// else Missing.
    pub fn resolve(&mut self, found: Option<AgentPath>) {
        *self = match found {
            Some(agent) => LocateState::Found {
                path: agent.path,
                version: agent.version,
            },
            None => LocateState::Missing,
        };
    }
}

// ─── the window state ───────────────────────────────────────────────────────

/// The settings window's state: the selected tab, the per-engine locate
/// rows, and the field mirrors. The
/// mirrors seed from the settings on open; every write folds the same
/// message into both the mirror ([`SettingsState::update`]) and the
/// persisted settings ([`stage`]) so the two never drift.
pub struct SettingsState {
    pub tab: Tab,
    /// Per-engine locate rows, [`ENGINES`] order.
    pub locate: [LocateState; 2],
    /// Generale: the default project location and the language override.
    pub location: PathBuf,
    pub language: LanguageChoice,
    /// The shared print-preferences values (the Generale tab's third
    /// section renders them).
    pub printer_type: PrinterType,
    pub printer_model: String,
    pub measurement_unit: MeasurementUnit,
    /// Agente: the engine, per-engine custom paths and models (text
    /// buffers, trimmed on write), the effort, restricted bash.
    pub engine: AgentKind,
    pub custom_paths: [String; 2],
    pub models: [String; 2],
    pub effort: Effort,
    pub restricted_bash: bool,
    /// Avanzate: streaming output and the agent-isolation toggle.
    pub streaming: bool,
    pub isolates: bool,
}

impl SettingsState {
    /// The state over the current settings, locate rows Unknown (the app
    /// kicks both locates when it opens the window — upstream's
    /// auto-locate on section appear).
    pub fn new(settings: &Settings) -> Self {
        let engine = settings.agent_kind();
        Self {
            tab: Tab::General,
            locate: [LocateState::Unknown, LocateState::Unknown],
            location: settings.default_project_location.clone(),
            language: LanguageChoice::from_setting(settings.language.as_deref()),
            printer_type: settings.printer_type,
            printer_model: settings.printer_model.clone().unwrap_or_default(),
            measurement_unit: settings.measurement_unit,
            engine,
            custom_paths: [
                settings.claude_path.clone().unwrap_or_default(),
                settings.codex_path.clone().unwrap_or_default(),
            ],
            models: [
                settings.claude_model.clone().unwrap_or_default(),
                settings.codex_model.clone().unwrap_or_default(),
            ],
            effort: Effort::from_setting(settings.claude_effort.as_deref()),
            restricted_bash: settings.restricted_bash,
            streaming: settings.streaming_output,
            isolates: settings.isolates_claude,
        }
    }

    /// Replace every settings-derived field with `settings`, keeping the
    /// selected tab and the locate rows.
    pub fn reseed(&mut self, settings: &Settings) {
        let fresh = Self::new(settings);
        *self = Self {
            tab: self.tab,
            locate: std::mem::take(&mut self.locate),
            ..fresh
        };
    }

    /// The window height this state's tab and engine call for ([`height`]).
    pub fn height(&self) -> f32 {
        height(self.tab, self.engine)
    }

    /// Fold one message into the mirror — the field messages' other half
    /// is [`stage`]; the effect messages keep their state in the app.
    pub fn update(&mut self, message: &Message) {
        match message {
            Message::Tab(tab) => self.tab = *tab,
            Message::ChooseLocation(_) => {}
            Message::LocationChosen(path) => self.location = path.clone(),
            Message::Language(language) => self.language = *language,
            Message::PrintPref(pref) => match pref {
                PrintPrefMessage::PrinterType(kind) => self.printer_type = *kind,
                PrintPrefMessage::PrinterModel(model) => self.printer_model = model.clone(),
                PrintPrefMessage::Unit(unit) => self.measurement_unit = *unit,
            },
            Message::Engine(engine) => self.engine = *engine,
            Message::CustomPath(kind, path) => {
                self.custom_paths[engine_index(*kind)] = path.clone()
            }
            Message::Model(kind, model) => self.models[engine_index(*kind)] = model.clone(),
            Message::Effort(effort) => self.effort = *effort,
            Message::RestrictedBash(on) => self.restricted_bash = *on,
            Message::PythonCheck | Message::PythonReinstall => {}
            Message::Verify(kind) => self.locate[engine_index(*kind)].begin(),
            Message::Located(kind, found) => {
                self.locate[engine_index(*kind)].resolve(found.clone())
            }
            Message::Streaming(on) => self.streaming = *on,
            Message::Isolates(on) => self.isolates = *on,
        }
    }
}

/// One settings-window interaction. The effect variants (portal, locates,
/// python drivers) are the app layer's: it performs them and reports the
/// outcome back through the matching variant.
#[derive(Debug, Clone)]
pub enum Message {
    /// A tab header pressed.
    Tab(Tab),
    /// "Scegli…" — ask the folder portal, starting at the current
    /// location (SettingsView.swift:33-38).
    ChooseLocation(PathBuf),
    /// The portal answered with the new default location.
    LocationChosen(PathBuf),
    /// The language picker (writes the override; System clears it).
    Language(LanguageChoice),
    /// The shared print-preferences section.
    PrintPref(PrintPrefMessage),
    /// The engine segmented picker.
    Engine(AgentKind),
    /// An engine's Verifica (SettingsView.swift:158-160).
    Verify(AgentKind),
    /// A locate finished for one engine.
    Located(AgentKind, Option<AgentPath>),
    /// The per-engine custom-path field (live, like @AppStorage).
    CustomPath(AgentKind, String),
    /// The per-engine model field.
    Model(AgentKind, String),
    /// The effort picker.
    Effort(Effort),
    /// The claude-only restricted-bash toggle.
    RestrictedBash(bool),
    /// The Python tab's Verifica — the environment check re-run
    /// (SettingsView.swift:198-200). The port's manual retry for a failed
    /// startup check (the failed-venv-check-no-retry follow-up's close).
    PythonCheck,
    /// The Python tab's Reinstalla… — presents the setup sheet over the
    /// project windows and kicks the reinstall
    /// (SettingsView.swift:201-204).
    PythonReinstall,
    /// The streaming-output picker (true = stream-json).
    Streaming(bool),
    /// The agent-isolation toggle.
    Isolates(bool),
}

/// Whether `message` is a field write that must persist. The decision
/// rests on the message's kind alone, so a caller can make it before
/// loading the settings; the effect messages persist nothing.
pub fn persists(message: &Message) -> bool {
    match message {
        Message::Tab(_)
        | Message::ChooseLocation(_)
        | Message::PythonCheck
        | Message::PythonReinstall
        | Message::Verify(_)
        | Message::Located(_, _) => false,
        Message::LocationChosen(_)
        | Message::Language(_)
        | Message::PrintPref(_)
        | Message::Engine(_)
        | Message::CustomPath(_, _)
        | Message::Model(_, _)
        | Message::Effort(_)
        | Message::RestrictedBash(_)
        | Message::Streaming(_)
        | Message::Isolates(_) => true,
    }
}

/// The ONE-writer fold: one field message → its engine-settings mutation
/// (upstream's @AppStorage bindings write live; the trims mirror
/// `AppSettings.string`, which reads empty-as-absent). An effect message
/// ([`persists`] false) leaves `settings` alone.
pub fn stage(message: &Message, settings: &mut Settings) {
    let trimmed = |text: &str| (!text.trim().is_empty()).then(|| text.trim().to_string());
    match message {
        Message::Tab(_)
        | Message::ChooseLocation(_)
        | Message::PythonCheck
        | Message::PythonReinstall
        | Message::Verify(_)
        | Message::Located(_, _) => {}
        Message::LocationChosen(path) => {
            settings.default_project_location = path.clone();
        }
        Message::Language(language) => {
            settings.language = language.as_setting().map(str::to_string);
        }
        Message::PrintPref(pref) => match pref {
            PrintPrefMessage::PrinterType(kind) => settings.printer_type = *kind,
            PrintPrefMessage::PrinterModel(model) => settings.printer_model = trimmed(model),
            PrintPrefMessage::Unit(unit) => settings.measurement_unit = *unit,
        },
        Message::Engine(engine) => {
            settings.agent_engine = match engine {
                AgentKind::Claude => "claude".to_string(),
                AgentKind::Codex => "codex".to_string(),
            };
        }
        Message::CustomPath(kind, path) => {
            settings.set_agent_path(*kind, trimmed(path));
        }
        Message::Model(kind, model) => {
            settings.set_agent_model(*kind, trimmed(model));
        }
        Message::Effort(effort) => {
            settings.claude_effort = effort.as_setting().map(str::to_string);
        }
        Message::RestrictedBash(on) => {
            settings.restricted_bash = *on;
        }
        Message::Streaming(on) => {
            settings.streaming_output = *on;
        }
        Message::Isolates(on) => {
            settings.isolates_claude = *on;
        }
    }
}

// ─── the window view ────────────────────────────────────────────────────────

/// The window body (SettingsView.swift:3-21): the toolbar's tab items,
/// then the selected tab's grouped Form. `python` is the app's shared
/// environment state — the Python tab renders it, the app's drivers own
/// it.
pub fn view<'a>(state: &'a SettingsState, python: &EnvState) -> Element<'a, Message> {
    let mut tabs = Row::new().spacing(TAB_GAP).align_y(Alignment::Center);
    for tab in Tab::ALL {
        tabs = tabs.push(tab_item(tab, state.tab == tab));
    }
    let parts = match state.tab {
        Tab::General => view_general(state),
        Tab::Agent => view_agent(state),
        Tab::Python => view_python(python),
        Tab::Advanced => view_advanced(state),
    };
    Column::new()
        .push(
            // the tab pills hang right under the header and sit 28 px
            // above the first box, as under upstream's titlebar
            container(tabs)
                .width(Length::Fill)
                .padding([0, 0, 7, 0])
                .align_x(Alignment::Center),
        )
        .push(scrollable(
            container(form::form(parts)).width(Length::Fill).padding([
                form::MARGIN,
                form::MARGIN,
                form::MARGIN - FOOTER_TAIL,
                form::MARGIN,
            ]),
        ))
        .height(Length::Fill)
        .into()
}

/// Between two toolbar tab items.
const TAB_GAP: f32 = 2.0;
/// The tab item's symbol height.
const TAB_SYMBOL: f32 = 22.0;
/// The selected tab item's fill against [`Tone::Quaternary`] (222 on the
/// 236 toolbar).
const TAB_SELECTED_FILL: f32 = 0.55;
/// The last footer's own bottom gap, folded into the window's margin.
const FOOTER_TAIL: f32 = 16.0;

/// One toolbar tab item (the settings `TabView`'s toolbar style): the
/// symbol over the label; the selected item in the accent on a rounded
/// wash, the others secondary.
fn tab_item(tab: Tab, selected: bool) -> Element<'static, Message> {
    let tone = if selected {
        Tone::Accent
    } else {
        Tone::Secondary
    };
    let face = Column::new()
        .spacing(3)
        .align_x(Alignment::Center)
        .push(ui::icon::icon(
            tab.symbol(),
            TAB_SYMBOL,
            tone,
            Backdrop::Titlebar,
        ))
        .push(
            cosmic::widget::text(tab.title())
                .size(typography::SUBHEADLINE)
                .class(tone.text(Backdrop::Titlebar)),
        );
    let cell = container(face)
        .padding([3, 8])
        .height(Length::Fixed(45.0))
        .align_y(Alignment::Center);
    let cell = if selected {
        cell.class(ui::surface::fill(
            Tone::Quaternary,
            TAB_SELECTED_FILL,
            Corner::S,
            None,
        ))
    } else {
        cell
    };
    ui::button::borderless(cell, Some(Message::Tab(tab))).into()
}

/// Generale (SettingsView.swift:23-62): the default project location +
/// Scegli…, the language picker with the next-launch footer, then the
/// shared print-preferences section (:57).
fn view_general(state: &SettingsState) -> Vec<Element<'_, Message>> {
    let home = std::env::var("HOME").unwrap_or_default();
    let location = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(path_text(abbreviate_with_tilde(&state.location, &home)))
        .push(ui::button::push(
            fl!("scegli-2"),
            Some(Message::ChooseLocation(state.location.clone())),
        ));
    let language_titles = LanguageChoice::ALL
        .iter()
        .map(|choice| choice.title())
        .collect::<Vec<_>>();
    let language_row = LanguageChoice::ALL
        .iter()
        .position(|choice| *choice == state.language)
        .unwrap_or(0);
    let mut parts = vec![
        form::section(vec![form::row(
            fl!("posizione-dei-nuovi-progetti"),
            location,
        )]),
        form::section(vec![form::row(
            fl!("lingua"),
            ui::picker::picker(language_titles, Some(language_row), |index| {
                Message::Language(LanguageChoice::ALL[index])
            }),
        )]),
        form::footer(fl!(
            "la-nuova-lingua-viene-applicata-al-prossimo-avvio-di-cad-studio"
        )),
    ];
    parts.extend(
        crate::sheets::print_preferences_parts(
            state.printer_type,
            &state.printer_model,
            state.measurement_unit,
        )
        .map(|part| part.map(Message::PrintPref)),
    );
    parts
}

/// The engine picker's width (upstream's segmented control spans about
/// half the row).
const ENGINE_PICKER_WIDTH: f32 = 300.0;

/// Agente (SettingsView.swift:64-169).
fn view_agent(state: &SettingsState) -> Vec<Element<'_, Message>> {
    let engines = ENGINES
        .iter()
        .map(|kind| {
            (
                kind.display_name().to_string(),
                Some(Message::Engine(*kind)),
            )
        })
        .collect();
    let picker = container(ui::segmented::segmented(
        engines,
        Some(engine_index(state.engine)),
        ui::segmented::Look::Raised,
        ui::segmented::Size::Regular,
        Length::Fill,
    ))
    .width(Length::Fixed(ENGINE_PICKER_WIDTH));
    let effort_titles = Effort::ALL.iter().map(|e| e.title()).collect::<Vec<_>>();
    let effort_row = Effort::ALL
        .iter()
        .position(|e| *e == state.effort)
        .unwrap_or(0);
    let mut parts = vec![
        form::section(vec![form::row(fl!("agente-2"), picker)]),
        form::footer(fl!(
            "cad-studio-usa-l-agente-installato-su-questo-mac-e-il-tuo-piano-puoi-cambiarlo-in-qualsiasi-momento-i-progetti-esistenti-continuano-con-l-agente-scelto-qui"
        )),
        engine_section(state),
        form::section(vec![form::row(
            fl!("effort"),
            ui::picker::picker(effort_titles, Some(effort_row), |index| {
                Message::Effort(Effort::ALL[index])
            }),
        )]),
        form::footer(fl!(
            "un-effort-pi-basso-fa-ragionare-meno-l-agente-prima-di-agire-le-risposte-arrivano-prima-ma-per-pezzi-complessi-potrebbero-servire-pi-correzioni"
        )),
    ];
    // claude-only restricted bash; codex-only the sandbox note
    // (SettingsView.swift:97-109)
    if state.engine == AgentKind::Claude {
        parts.push(form::section(vec![form::row(
            fl!("bash-limitato"),
            form::switch(state.restricted_bash, Message::RestrictedBash),
        )]));
        parts.push(form::footer(fl!(
            "con-bash-limitato-l-agente-pu-eseguire-solo-l-interprete-python-dell-ambiente-di-cad-studio-e-alcuni-comandi-di-sola-lettura-ls-cat-head-pi-sicuro-ma-l-agente-ha-meno-autonomia-per-diagnosticare-i-problemi-e-alcuni-tentativi-potrebbero-essere-rifiutati"
        )));
    } else {
        parts.push(form::section(vec![form::full_row(
            body(fl!(
                "codex-lavora-nella-sua-sandbox-pu-scrivere-solo-nella-cartella-del-progetto-e-non-usa-la-rete"
            ))
            .class(Tone::Secondary.text(Backdrop::Plain)),
        )]));
    }
    parts
}

/// One engine's section (AgentEngineSection,
/// SettingsView.swift:116-169): executable and version by locate state,
/// the custom-path and model fields (monospaced, labels included), Verifica
/// right-aligned (:156-161).
fn engine_section(state: &SettingsState) -> Element<'_, Message> {
    let kind = state.engine;
    let slot = engine_index(kind);
    let home = std::env::var("HOME").unwrap_or_default();
    let executable: Element<'_, Message> = match &state.locate[slot] {
        LocateState::Found { path, .. } => path_text(abbreviate_with_tilde(path, &home)).into(),
        LocateState::Checking => cosmic::widget::progress_bar::indeterminate_circular()
            .size(16.0)
            .into(),
        LocateState::Unknown => value_text(fl!("settings-none")).into(),
        LocateState::Missing => body(fl!("non-trovato-2"))
            .class(Tone::Danger.text(Backdrop::Plain))
            .into(),
    };
    let version: Element<'_, Message> = match &state.locate[slot] {
        LocateState::Found { version, .. } => value_text(version.clone())
            .font(cosmic::font::mono())
            .into(),
        _ => value_text(fl!("settings-none")).into(),
    };
    let checking = matches!(state.locate[slot], LocateState::Checking);
    form::section(vec![
        form::row(fl!("eseguibile"), executable),
        form::row(fl!("versione-2"), version),
        form::styled_row(
            form::label_text(fl!("percorso-personalizzato")).font(cosmic::font::mono()),
            form::field(
                fl!("automatico"),
                state.custom_paths[slot].as_str(),
                true,
                move |path| Message::CustomPath(kind, path),
            ),
        ),
        form::styled_row(
            form::label_text(fl!("modello-2")).font(cosmic::font::mono()),
            form::field(
                fl!("predefinito-di", agent = kind.display_name()),
                state.models[slot].as_str(),
                true,
                move |model| Message::Model(kind, model),
            ),
        ),
        form::full_row(
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(ui::button::push(
                    fl!("verifica"),
                    (!checking).then_some(Message::Verify(kind)),
                )),
        ),
    ])
}

/// Python (SettingsView.swift:171-215): interpreter path, build123d by
/// environment state, Verifica + Reinstalla… (disabled while installing,
/// :206), the packages footer.
fn view_python(python: &EnvState) -> Vec<Element<'static, Message>> {
    let home = std::env::var("HOME").unwrap_or_default();
    let build123d: Element<'_, Message> = match python {
        EnvState::Ready { build123d_version } => value_text(build123d_version.clone())
            .font(cosmic::font::mono())
            .into(),
        EnvState::Installing | EnvState::Checking | EnvState::Unknown => {
            cosmic::widget::progress_bar::indeterminate_circular()
                .size(16.0)
                .into()
        }
        EnvState::Missing => value_text(fl!("non-installato")).into(),
        EnvState::Failed(_) => body(fl!("non-funzionante"))
            .class(Tone::Danger.text(Backdrop::Plain))
            .into(),
    };
    let installing = matches!(python, EnvState::Installing);
    let actions = Row::new()
        .spacing(8)
        .push(Space::new().width(Length::Fill))
        .push(ui::button::push(
            fl!("verifica"),
            (!installing).then_some(Message::PythonCheck),
        ))
        .push(ui::button::push(
            fl!("reinstalla"),
            (!installing).then_some(Message::PythonReinstall),
        ));
    vec![
        form::section(vec![
            form::row(
                fl!("interprete"),
                path_text(interpreter_label(&home)),
            ),
            form::row(fl!("build123d"), build123d),
            form::full_row(actions),
        ]),
        form::footer(fl!("pacchetti-build123d-matplotlib-numpy")),
    ]
}

/// The Python tab's interpreter line: the venv's interpreter,
/// `~/`-abbreviated against `home`.
fn interpreter_label(home: &str) -> String {
    let interpreter = engine::python::env::interpreter_path(&engine::python::env::venv_dir());
    abbreviate_with_tilde(&interpreter, home)
}

/// Avanzate (SettingsView.swift:217-242).
fn view_advanced(state: &SettingsState) -> Vec<Element<'_, Message>> {
    let formats = vec![
        fl!("stream-json-in-tempo-reale"),
        fl!("json-risposta-unica"),
    ];
    let selected = usize::from(!state.streaming);
    vec![
        form::section(vec![form::row(
            fl!("formato-di-output"),
            ui::picker::picker(formats, Some(selected), |index| {
                Message::Streaming(index == 0)
            }),
        )]),
        form::footer(fl!(
            "vale-per-claude-code-il-formato-json-non-mostra-il-testo-in-streaming-n-le-singole-attivit-ma-pu-essere-utile-con-versioni-che-non-supportano-lo-streaming"
        )),
        form::section(vec![form::row(
            fl!("ignora-le-configurazioni-personali-dell-agente"),
            form::switch(state.isolates, Message::Isolates),
        )]),
        form::footer(fl!(
            "l-agente-cad-non-carica-le-tue-impostazioni-utente-hook-plugin-e-server-mcp-di-claude-code-config-toml-di-codex-cos-parte-pi-velocemente-e-risponde-in-modo-prevedibile-disattiva-se-la-tua-autenticazione-dipende-da-impostazioni-utente-ad-esempio-apikeyhelper"
        )),
    ]
}

// ─── About ──────────────────────────────────────────────────────────────────

/// The About window (upstream `showAboutPanel`,
/// Commands.swift:141-152, AppKit's standard About panel): the 64 px app
/// icon, the bold name, the version, the credits paragraph centered in
/// the small system font and secondary, and the copyright line. Nothing
/// is interactive, so the element is generic over the host's message type.
pub fn view_about<Message: 'static>() -> Element<'static, Message> {
    let small = |label: String| {
        body(label)
            .size(typography::SMALL_CONTROL)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
    };
    let column = Column::new()
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .push(ui::icon::app(64.0))
        .push(Space::new().height(6))
        .push(
            body(fl!("cad-studio"))
                .size(typography::TITLE3)
                .font(crate::typography::font(Weight::Bold))
                .class(Tone::Primary.text(Backdrop::Plain)),
        )
        .push(Space::new().height(6))
        .push(
            small(fl!("versione", version = env!("CARGO_PKG_VERSION")))
                .class(Tone::Primary.text(Backdrop::Plain)),
        )
        .push(Space::new().height(6))
        .push(
            small(fl!(
                "software-open-source-rilasciato-con-licenza-mit-sviluppato-da-filippo-finke"
            ))
            .class(Tone::Secondary.text(Backdrop::Plain)),
        )
        .push(Space::new().height(6))
        .push(small(fl!("about-copyright")).class(Tone::Primary.text(Backdrop::Plain)));
    container(column)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([ABOUT_TOP, 20.0, 24.0, 20.0])
        .class(cosmic::theme::Container::WindowBackground)
        .into()
}

// ─── view helpers ───────────────────────────────────────────────────────────

/// A `LabeledContent` value: secondary, as a grouped Form draws it.
fn value_text<'a>(label: impl Into<String>) -> Text<'a> {
    body(label).class(Tone::Secondary.text(Backdrop::Plain))
}

/// Upstream `PathText`: a monospaced, single-line, tilde-abbreviated path.
fn path_text<'a>(path: String) -> Text<'a> {
    value_text(path)
        .font(cosmic::font::mono())
        .wrapping(Wrapping::None)
}

fn body<'a>(label: impl Into<String>) -> Text<'a> {
    cosmic::widget::text(label.into()).wrapping(Wrapping::Word)
}

/// `~/`-abbreviate `path` against `home` (upstream
/// `NSString.abbreviatingWithTildeInPath` — `~user` forms never collapse).
pub fn abbreviate_with_tilde(path: &Path, home: &str) -> String {
    if home.is_empty() {
        return path.display().to_string();
    }
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix(home) {
        if rest.is_empty() {
            "~".to_string()
        } else if rest.starts_with('/') {
            format!("~{rest}")
        } else {
            text
        }
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window follows the content like upstream's Settings scene:
    /// each tab its own height, in upstream's order (Python shortest,
    /// Agent tallest), and the Agent tab shrinks when Codex drops the
    /// restricted-bash section.
    #[test]
    fn the_window_height_follows_the_tab_and_engine() {
        let mut state = SettingsState::new(&Settings::default());
        let general = state.height();
        state.update(&Message::Tab(Tab::Agent));
        let claude = state.height();
        state.update(&Message::Engine(AgentKind::Codex));
        let codex = state.height();
        state.update(&Message::Tab(Tab::Python));
        let python = state.height();
        state.update(&Message::Tab(Tab::Advanced));
        let advanced = state.height();
        assert!(python < advanced && advanced < general && general < claude);
        assert!(codex < claude && codex > general);
        // the engine matters only on the Agent tab
        assert_eq!(height(Tab::General, AgentKind::Codex), general);
    }

    /// The mirror seeds from the settings exactly (every field the tabs
    /// render), claude-first defaults included.
    #[test]
    fn state_mirrors_the_settings() {
        let settings = Settings::default();
        let state = SettingsState::new(&settings);
        assert_eq!(state.tab, Tab::General);
        assert_eq!(state.engine, AgentKind::Claude);
        assert_eq!(state.language, LanguageChoice::System);
        assert_eq!(state.effort, Effort::AgentDefault);
        assert!(!state.restricted_bash);
        assert!(state.streaming, "streaming defaults on (object ?? true)");
        assert!(state.isolates, "isolation defaults on (object ?? true)");
        assert_eq!(state.custom_paths, [String::new(), String::new()]);
        assert_eq!(state.locate, [LocateState::Unknown, LocateState::Unknown]);

        let codex = Settings {
            agent_engine: "codex".into(),
            claude_path: Some("/opt/claude".into()),
            codex_path: Some("~/.cargo/bin/codex".into()),
            claude_model: Some("opus".into()),
            codex_model: Some("gpt-5.2-codex".into()),
            claude_effort: Some("high".into()),
            restricted_bash: true,
            streaming_output: false,
            isolates_claude: false,
            language: Some("it".into()),
            printer_type: PrinterType::Resin,
            printer_model: Some("P1S".into()),
            measurement_unit: MeasurementUnit::Inches,
            default_project_location: PathBuf::from("/tmp/projects"),
            ..Settings::default()
        };

        let state = SettingsState::new(&codex);
        assert_eq!(state.engine, AgentKind::Codex);
        assert_eq!(state.language, LanguageChoice::Italiano);
        assert_eq!(state.effort, Effort::High);
        assert!(state.restricted_bash);
        assert!(!state.streaming);
        assert!(!state.isolates);
        assert_eq!(
            state.custom_paths,
            ["/opt/claude".to_string(), "~/.cargo/bin/codex".to_string()]
        );
        assert_eq!(
            state.models,
            ["opus".to_string(), "gpt-5.2-codex".to_string()]
        );
        assert_eq!(state.printer_type, PrinterType::Resin);
        assert_eq!(state.printer_model, "P1S");
        assert_eq!(state.measurement_unit, MeasurementUnit::Inches);
        assert_eq!(state.location, PathBuf::from("/tmp/projects"));
    }

    /// Every field message stages its one settings mutation — the
    /// ONE-writer fold — and the effect messages stage nothing. Text
    /// writes trim, and blanks clear (AppSettings.string's empty-is-nil).
    #[test]
    fn field_messages_stage_their_settings_writes() {
        let mut settings = Settings::default();
        let stage_and_persists = |message: &Message, settings: &mut Settings| {
            stage(message, settings);
            persists(message)
        };
        assert!(!stage_and_persists(
            &Message::Tab(Tab::Python),
            &mut settings
        ));
        assert!(!stage_and_persists(
            &Message::ChooseLocation(PathBuf::from("/x")),
            &mut settings
        ));
        assert!(!stage_and_persists(&Message::PythonCheck, &mut settings));
        assert!(!stage_and_persists(
            &Message::PythonReinstall,
            &mut settings
        ));
        assert!(!stage_and_persists(
            &Message::Located(AgentKind::Claude, None),
            &mut settings
        ));
        assert!(
            !stage_and_persists(&Message::Verify(AgentKind::Claude), &mut settings),
            "Verify stages nothing"
        );
        assert_eq!(
            (
                &settings.agent_engine,
                &settings.language,
                &settings.claude_effort,
                settings.restricted_bash,
                settings.streaming_output,
                settings.isolates_claude,
            ),
            (
                &"claude".to_string(),
                &None::<String>,
                &None::<String>,
                false,
                true,
                true,
            ),
            "no effect message writes anything"
        );

        assert!(stage_and_persists(
            &Message::LocationChosen(PathBuf::from("/home/me/Documents")),
            &mut settings
        ));
        assert_eq!(
            settings.default_project_location,
            PathBuf::from("/home/me/Documents")
        );

        assert!(stage_and_persists(
            &Message::Language(LanguageChoice::Deutsch),
            &mut settings
        ));
        assert_eq!(settings.language.as_deref(), Some("de"));
        assert!(stage_and_persists(
            &Message::Language(LanguageChoice::System),
            &mut settings
        ));
        assert_eq!(settings.language, None, "System clears the override");

        assert!(stage_and_persists(
            &Message::Engine(AgentKind::Codex),
            &mut settings
        ));
        assert_eq!(settings.agent_engine, "codex");

        assert!(stage_and_persists(
            &Message::CustomPath(AgentKind::Codex, "  ~/.local/bin/codex  ".into()),
            &mut settings
        ));
        assert_eq!(settings.codex_path.as_deref(), Some("~/.local/bin/codex"));
        assert!(stage_and_persists(
            &Message::CustomPath(AgentKind::Codex, "   ".into()),
            &mut settings
        ));
        assert_eq!(settings.codex_path, None, "a blank path clears the setting");
        assert_eq!(
            settings.claude_path, None,
            "the other engine's key is untouched"
        );

        assert!(stage_and_persists(
            &Message::Model(AgentKind::Codex, " gpt-5.2-codex ".into()),
            &mut settings
        ));
        assert_eq!(settings.codex_model.as_deref(), Some("gpt-5.2-codex"));
        assert!(stage_and_persists(
            &Message::Model(AgentKind::Codex, String::new()),
            &mut settings
        ));
        assert_eq!(settings.codex_model, None);

        assert!(stage_and_persists(
            &Message::Effort(Effort::Low),
            &mut settings
        ));
        assert_eq!(settings.claude_effort.as_deref(), Some("low"));
        assert!(stage_and_persists(
            &Message::Effort(Effort::AgentDefault),
            &mut settings
        ));
        assert_eq!(settings.claude_effort, None);

        assert!(stage_and_persists(
            &Message::RestrictedBash(true),
            &mut settings
        ));
        assert!(settings.restricted_bash);
        assert!(stage_and_persists(
            &Message::Streaming(false),
            &mut settings
        ));
        assert!(!settings.streaming_output);
        assert!(stage_and_persists(&Message::Isolates(false), &mut settings));
        assert!(!settings.isolates_claude);

        assert!(stage_and_persists(
            &Message::PrintPref(PrintPrefMessage::PrinterType(PrinterType::Sls)),
            &mut settings
        ));
        assert!(stage_and_persists(
            &Message::PrintPref(PrintPrefMessage::PrinterModel("  P1S  ".into())),
            &mut settings
        ));
        assert!(stage_and_persists(
            &Message::PrintPref(PrintPrefMessage::Unit(MeasurementUnit::Centimeters)),
            &mut settings
        ));
        assert_eq!(settings.printer_type, PrinterType::Sls);
        assert_eq!(settings.printer_model.as_deref(), Some("P1S"));
        assert_eq!(settings.measurement_unit, MeasurementUnit::Centimeters);
    }

    /// The staged writes survive a save/load round trip — what the window
    /// wrote is what the next launch reads.
    #[test]
    fn staged_writes_round_trip_through_disk() {
        let dir = std::env::temp_dir().join(format!("cad-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        let mut settings = Settings::default();
        for message in [
            Message::Engine(AgentKind::Codex),
            Message::CustomPath(AgentKind::Codex, "~/.cargo/bin/codex".into()),
            Message::Model(AgentKind::Codex, "gpt-5.2-codex".into()),
            Message::Effort(Effort::Medium),
            Message::Language(LanguageChoice::Francais),
            Message::LocationChosen(PathBuf::from("/tmp/cad")),
            Message::Streaming(false),
            Message::Isolates(false),
            Message::RestrictedBash(true),
        ] {
            assert!(persists(&message));
            stage(&message, &mut settings);
        }
        settings.save(&file).unwrap();
        let reloaded = Settings::load(&file).unwrap();
        assert_eq!(reloaded.agent_kind(), AgentKind::Codex);
        assert_eq!(reloaded.codex_path.as_deref(), Some("~/.cargo/bin/codex"));
        assert_eq!(reloaded.codex_model.as_deref(), Some("gpt-5.2-codex"));
        assert_eq!(reloaded.claude_effort.as_deref(), Some("medium"));
        assert_eq!(reloaded.language.as_deref(), Some("fr"));
        assert_eq!(reloaded.default_project_location, PathBuf::from("/tmp/cad"));
        assert!(!reloaded.streaming_output);
        assert!(!reloaded.isolates_claude);
        assert!(reloaded.restricted_bash);
        // and the mirror over the reloaded settings agrees
        let state = SettingsState::new(&reloaded);
        assert_eq!(state.engine, AgentKind::Codex);
        assert_eq!(state.language, LanguageChoice::Francais);
        assert_eq!(state.effort, Effort::Medium);
    }

    /// The update fold mirrors every field message into the state (the
    /// other half of each write), switches tabs, and drives the locate
    /// rows per engine.
    #[test]
    fn update_folds_the_mirror_half() {
        let mut state = SettingsState::new(&Settings::default());
        state.update(&Message::Tab(Tab::Advanced));
        assert_eq!(state.tab, Tab::Advanced);

        state.update(&Message::Engine(AgentKind::Codex));
        assert_eq!(state.engine, AgentKind::Codex);

        state.update(&Message::CustomPath(
            AgentKind::Claude,
            "/opt/claude".into(),
        ));
        state.update(&Message::Model(AgentKind::Claude, "opus".into()));
        assert_eq!(state.custom_paths[0], "/opt/claude");
        assert_eq!(state.models[0], "opus");
        assert_eq!(
            state.custom_paths[1],
            String::new(),
            "the codex buffer is untouched by claude writes"
        );

        state.update(&Message::Effort(Effort::High));
        state.update(&Message::RestrictedBash(true));
        state.update(&Message::Streaming(false));
        state.update(&Message::Isolates(false));
        state.update(&Message::Language(LanguageChoice::English));
        state.update(&Message::LocationChosen(PathBuf::from("/tmp/loc")));
        state.update(&Message::PrintPref(PrintPrefMessage::PrinterModel(
            "P1S".into(),
        )));
        assert_eq!(state.effort, Effort::High);
        assert!(state.restricted_bash);
        assert!(!state.streaming);
        assert!(!state.isolates);
        assert_eq!(state.language, LanguageChoice::English);
        assert_eq!(state.location, PathBuf::from("/tmp/loc"));
        assert_eq!(state.printer_model, "P1S");
    }

    /// The locate rows: Verify marks the CURRENT engine checking (upstream
    /// locates the section shown), and a resolve lands Found or Missing
    /// without touching the other engine's row.
    #[test]
    fn locate_rows_transition_per_engine() {
        let mut state = SettingsState::new(&Settings::default());
        state.update(&Message::Engine(AgentKind::Codex));
        state.update(&Message::Verify(AgentKind::Codex));
        assert_eq!(state.locate, [LocateState::Unknown, LocateState::Checking]);

        state.update(&Message::Located(
            AgentKind::Codex,
            Some(AgentPath {
                path: PathBuf::from("/usr/bin/codex"),
                version: "codex 1.2".into(),
            }),
        ));
        assert_eq!(
            state.locate[1],
            LocateState::Found {
                path: PathBuf::from("/usr/bin/codex"),
                version: "codex 1.2".into()
            }
        );
        assert_eq!(state.locate[0], LocateState::Unknown);

        state.update(&Message::Verify(AgentKind::Claude));
        state.update(&Message::Located(AgentKind::Claude, None));
        assert_eq!(state.locate[0], LocateState::Missing);
        assert!(matches!(state.locate[1], LocateState::Found { .. }));
    }

    /// The language and effort pickers round-trip their settings values,
    /// and unknown stored tags read the safe row.
    #[test]
    fn language_and_effort_map_both_ways() {
        for (choice, tag) in [
            (LanguageChoice::Italiano, Some("it")),
            (LanguageChoice::English, Some("en")),
            (LanguageChoice::Deutsch, Some("de")),
            (LanguageChoice::Francais, Some("fr")),
            (LanguageChoice::System, None),
        ] {
            assert_eq!(choice.as_setting(), tag);
            assert_eq!(LanguageChoice::from_setting(tag), choice);
        }
        assert_eq!(
            LanguageChoice::from_setting(Some("xx")),
            LanguageChoice::System,
            "an unknown tag reads System"
        );
        for (effort, tag) in [
            (Effort::Low, Some("low")),
            (Effort::Medium, Some("medium")),
            (Effort::High, Some("high")),
            (Effort::AgentDefault, None),
        ] {
            assert_eq!(effort.as_setting(), tag);
            assert_eq!(Effort::from_setting(tag), effort);
        }
        assert_eq!(Effort::from_setting(Some("turbo")), Effort::AgentDefault);
    }

    /// `~/`-abbreviation matches NSString.abbreviatingWithTildeInPath's
    /// cases: the home itself, a child, a non-child, an empty home, and a
    /// home-prefixed-but-not-boundary path.
    #[test]
    fn tilde_abbreviation_cases() {
        assert_eq!(
            abbreviate_with_tilde(Path::new("/home/me"), "/home/me"),
            "~"
        );
        assert_eq!(
            abbreviate_with_tilde(Path::new("/home/me/Documents/a.png"), "/home/me"),
            "~/Documents/a.png"
        );
        assert_eq!(
            abbreviate_with_tilde(Path::new("/usr/local/bin/claude"), "/home/me"),
            "/usr/local/bin/claude"
        );
        assert_eq!(
            abbreviate_with_tilde(Path::new("/home/metric"), "/home/me"),
            "/home/metric",
            "a path that merely prefixes home does not collapse"
        );
        assert_eq!(
            abbreviate_with_tilde(Path::new("/home/me/x"), ""),
            "/home/me/x",
            "no home to abbreviate against"
        );
    }

    /// Every tab's body constructs (the tree-panic law: Element
    /// construction is load-bearing — a widget tree that cannot build
    /// cannot be fixed by review), over the states that switch its
    /// branches: both engines' Agente sections, every python state, both
    /// streaming positions, and About.
    #[test]
    fn every_tab_constructs_over_its_branches() {
        let _lang = crate::localize::pin_en();
        let mut state = SettingsState::new(&Settings::default());
        for tab in Tab::ALL {
            state.update(&Message::Tab(tab));
            let _ = view(&state, &EnvState::Unknown);
        }
        // Agente under both engines, with a located and a missing row
        state.update(&Message::Engine(AgentKind::Codex));
        state.update(&Message::Located(AgentKind::Codex, None));
        let _ = view(&state, &EnvState::Unknown);
        state.update(&Message::Engine(AgentKind::Claude));
        state.update(&Message::Located(
            AgentKind::Claude,
            Some(AgentPath {
                path: PathBuf::from("/usr/bin/claude"),
                version: "2.0".into(),
            }),
        ));
        let _ = view(&state, &EnvState::Unknown);
        state.update(&Message::Verify(AgentKind::Claude));
        let _ = view(&state, &EnvState::Unknown);

        // Python over every environment state
        state.update(&Message::Tab(Tab::Python));
        for python in [
            EnvState::Unknown,
            EnvState::Checking,
            EnvState::Installing,
            EnvState::Missing,
            EnvState::Failed("nope".into()),
            EnvState::Ready {
                build123d_version: "0.8.1".into(),
            },
        ] {
            let _ = view(&state, &python);
        }

        // Avanzate at both picker positions
        state.update(&Message::Tab(Tab::Advanced));
        state.update(&Message::Streaming(true));
        let _ = view(&state, &EnvState::Unknown);
        state.update(&Message::Streaming(false));
        let _ = view(&state, &EnvState::Unknown);

        // About
        let _: Element<'static, ()> = view_about();
    }

    /// The interpreter line shows the venv's python under `~`, not the
    /// absolute home path.
    #[test]
    fn the_interpreter_line_is_tilde_abbreviated() {
        let home = engine::storage::data_dir()
            .ancestors()
            .nth(3)
            .expect("the data dir sits under the home directory")
            .display()
            .to_string();
        assert_eq!(
            interpreter_label(&home),
            "~/.local/share/cad-studio/venv/bin/python"
        );
    }

    /// A reseed replaces every settings-derived field and keeps the tab
    /// and the locate rows.
    #[test]
    fn reseed_takes_the_settings_and_keeps_the_tab() {
        let mut state = SettingsState::new(&Settings::default());
        state.tab = Tab::Agent;
        state.locate[0] = LocateState::Missing;
        let mut changed = Settings::default();
        changed.set_agent_path(AgentKind::Claude, Some("/opt/claude".to_string()));
        state.reseed(&changed);
        assert_eq!(state.custom_paths[0], "/opt/claude");
        assert_eq!(state.tab, Tab::Agent);
        assert_eq!(state.locate[0], LocateState::Missing);
    }
}
