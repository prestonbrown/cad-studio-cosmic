//! The settings window (upstream SettingsView.swift): four tabs — Generale,
//! Agente, Python, Avanzate — in a NEW non-modal window (Ruling 6: the
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
//! literal-\n key (one of the two; Task 1 preserved it through the
//! placeable migration), the version the crate's own.
//!
//! Ruling 1's quirk note: upstream's engine-picker footer says existing
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
use cosmic::iced::Length;
use cosmic::iced::widget::text::Wrapping;
use cosmic::widget::segmented_button::{Entity, SingleSelectModel};
use cosmic::widget::{Column, Row, Space, button, container, scrollable, text_input};
use engine::agent::locator::{AgentKind, AgentPath};
use engine::python::env::EnvState;
use engine::settings::{MeasurementUnit, PrinterType, Settings};

use crate::fl;
use crate::sheets::PrintPrefMessage;
use crate::ui::{Backdrop, Tone};

/// Upstream's window geometry (SettingsView.swift:19 `.frame(width: 600)`).
pub const WIDTH: f32 = 600.0;
/// The port's window height: upstream sizes vertically to the content; a
/// fixed height over a scrollable body keeps tab switches from resizing
/// the window.
pub const HEIGHT: f32 = 560.0;

/// The About window's geometry (upstream's is a small system panel).
pub const ABOUT_WIDTH: f32 = 400.0;
pub const ABOUT_HEIGHT: f32 = 280.0;

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
/// rows, the engine picker's segmented model, and the field mirrors. The
/// mirrors seed from the settings on open; every write folds the same
/// message into both the mirror ([`SettingsState::update`]) and the
/// persisted settings ([`stage`]) so the two never drift.
pub struct SettingsState {
    pub tab: Tab,
    /// Per-engine locate rows, [`ENGINES`] order.
    pub locate: [LocateState; 2],
    /// The engine picker's segmented model (Claude | Codex, upstream
    /// `.pickerStyle(.segmented)`); the entities ride the state so the
    /// view's 'static `on_activate` closure can identify them.
    engine_model: SingleSelectModel,
    engine_entities: [Entity; 2],
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
        let mut engine_model = SingleSelectModel::default();
        let mut ids = Vec::with_capacity(2);
        for kind in ENGINES {
            ids.push(
                engine_model
                    .insert()
                    .text(kind.display_name())
                    .data(kind)
                    .id(),
            );
        }
        let engine = settings.agent_kind();
        engine_model.activate_position(engine_index(engine) as u16);
        Self {
            tab: Tab::General,
            locate: [LocateState::Unknown, LocateState::Unknown],
            engine_model,
            engine_entities: [ids[0], ids[1]],
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

    /// The segmented picker's entity for `kind` (the view's 'static
    /// activation closure matches on it).
    pub(crate) fn engine_entity(&self, kind: AgentKind) -> Entity {
        self.engine_entities[engine_index(kind)]
    }

    /// The engine picker widget over the state's model (upstream
    /// `.pickerStyle(.segmented)`).
    pub(crate) fn engine_picker(
        &self,
    ) -> cosmic::widget::segmented_button::HorizontalSegmentedButton<
        '_,
        cosmic::widget::segmented_button::SingleSelect,
        Message,
    > {
        cosmic::widget::segmented_control::horizontal(&self.engine_model)
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
            Message::Engine(engine) => {
                self.engine = *engine;
                self.engine_model
                    .activate_position(engine_index(*engine) as u16);
            }
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

/// The ONE-writer fold: one field message → its engine-settings mutation
/// (upstream's @AppStorage bindings write live; the trims mirror
/// `AppSettings.string`, which reads empty-as-absent). Returns whether the
/// settings changed and must persist — the effect messages stage nothing.
pub fn stage(message: &Message, settings: &mut Settings) -> bool {
    let trimmed = |text: &str| (!text.trim().is_empty()).then(|| text.trim().to_string());
    match message {
        Message::Tab(_)
        | Message::ChooseLocation(_)
        | Message::PythonCheck
        | Message::PythonReinstall
        | Message::Verify(_)
        | Message::Located(_, _) => false,
        Message::LocationChosen(path) => {
            settings.default_project_location = path.clone();
            true
        }
        Message::Language(language) => {
            settings.language = language.as_setting().map(str::to_string);
            true
        }
        Message::PrintPref(pref) => {
            match pref {
                PrintPrefMessage::PrinterType(kind) => settings.printer_type = *kind,
                PrintPrefMessage::PrinterModel(model) => settings.printer_model = trimmed(model),
                PrintPrefMessage::Unit(unit) => settings.measurement_unit = *unit,
            }
            true
        }
        Message::Engine(engine) => {
            settings.agent_engine = match engine {
                AgentKind::Claude => "claude".to_string(),
                AgentKind::Codex => "codex".to_string(),
            };
            true
        }
        Message::CustomPath(kind, path) => {
            settings.set_agent_path(*kind, trimmed(path));
            true
        }
        Message::Model(kind, model) => {
            settings.set_agent_model(*kind, trimmed(model));
            true
        }
        Message::Effort(effort) => {
            settings.claude_effort = effort.as_setting().map(str::to_string);
            true
        }
        Message::RestrictedBash(on) => {
            settings.restricted_bash = *on;
            true
        }
        Message::Streaming(on) => {
            settings.streaming_output = *on;
            true
        }
        Message::Isolates(on) => {
            settings.isolates_claude = *on;
            true
        }
    }
}

// ─── the window view ────────────────────────────────────────────────────────

/// The window body (SettingsView.swift:3-21): the tab row, then the
/// selected tab. `python` is the app's shared environment state — the
/// Python tab renders it, the app's drivers own it.
pub fn view<'a>(state: &'a SettingsState, python: &EnvState) -> Element<'a, Message> {
    let mut tabs = Row::new().spacing(8).padding([16, 20, 8, 20]);
    for tab in Tab::ALL {
        let selected = state.tab == tab;
        tabs = tabs.push(
            button::text(tab.title())
                .class(if selected {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Standard
                })
                .on_press(Message::Tab(tab)),
        );
    }
    let body = match state.tab {
        Tab::General => view_general(state),
        Tab::Agent => view_agent(state),
        Tab::Python => view_python(python),
        Tab::Advanced => view_advanced(state),
    };
    Column::new()
        .push(
            container(tabs)
                .width(Length::Fill)
                .align_x(cosmic::iced::alignment::Horizontal::Center),
        )
        .push(cosmic::widget::divider::horizontal::default())
        .push(scrollable(
            container(body).width(Length::Fill).padding([16, 20]),
        ))
        .height(Length::Fill)
        .into()
}

/// Generale (SettingsView.swift:23-62): the default project location +
/// Scegli…, the language picker with the next-launch footer, then the
/// shared print-preferences section (:57).
fn view_general(state: &SettingsState) -> Element<'_, Message> {
    let home = std::env::var("HOME").unwrap_or_default();
    let location_row = Row::new()
        .spacing(8)
        .push(mono(abbreviate_with_tilde(&state.location, &home)))
        .push(Space::new().width(Length::Fill))
        .push(
            button::text(fl!("scegli-2"))
                .class(cosmic::theme::Button::Standard)
                .on_press(Message::ChooseLocation(state.location.clone())),
        );
    let language_titles = LanguageChoice::ALL
        .iter()
        .map(|choice| choice.title())
        .collect::<Vec<_>>();
    let language_row = LanguageChoice::ALL
        .iter()
        .position(|choice| *choice == state.language)
        .unwrap_or(0);
    section(fl!("posizione-dei-nuovi-progetti"))
        .push(location_row)
        .push(space())
        .push(section(fl!("lingua")))
        .push(cosmic::widget::dropdown(
            language_titles,
            Some(language_row),
            |index| Message::Language(LanguageChoice::ALL[index]),
        ))
        .push(footer(fl!(
            "la-nuova-lingua-viene-applicata-al-prossimo-avvio-di-cad-studio"
        )))
        .push(space())
        .push(
            crate::sheets::view_print_preferences(
                state.printer_type,
                &state.printer_model,
                state.measurement_unit,
            )
            .map(Message::PrintPref),
        )
        .into()
}

/// Agente (SettingsView.swift:64-169).
fn view_agent(state: &SettingsState) -> Element<'_, Message> {
    let claude_entity = state.engine_entity(AgentKind::Claude);
    let codex_entity = state.engine_entity(AgentKind::Codex);
    let fallback = state.engine;
    let engine_picker = state.engine_picker().on_activate(move |entity| {
        Message::Engine(if entity == codex_entity {
            AgentKind::Codex
        } else if entity == claude_entity {
            AgentKind::Claude
        } else {
            fallback
        })
    });
    let effort_titles = Effort::ALL.iter().map(|e| e.title()).collect::<Vec<_>>();
    let effort_row = Effort::ALL
        .iter()
        .position(|e| *e == state.effort)
        .unwrap_or(0);
    let mut column = section(fl!("agente-2"))
        .push(engine_picker)
        .push(footer(fl!(
            "cad-studio-usa-l-agente-installato-su-questo-mac-e-il-tuo-piano-puoi-cambiarlo-in-qualsiasi-momento-i-progetti-esistenti-continuano-con-l-agente-scelto-qui"
        )))
        .push(space())
        .push(engine_section(state))
        .push(space())
        .push(section(fl!("effort")))
        .push(cosmic::widget::dropdown(
            effort_titles,
            Some(effort_row),
            |index| Message::Effort(Effort::ALL[index]),
        ))
        .push(footer(fl!(
            "un-effort-pi-basso-fa-ragionare-meno-l-agente-prima-di-agire-le-risposte-arrivano-prima-ma-per-pezzi-complessi-potrebbero-servire-pi-correzioni"
        )));
    // claude-only restricted bash; codex-only the sandbox note
    // (SettingsView.swift:97-109)
    if state.engine == AgentKind::Claude {
        column = column
            .push(space())
            .push(
                section(fl!("bash-limitato")).push(
                    cosmic::widget::toggler(state.restricted_bash)
                        .on_toggle(Message::RestrictedBash),
                ),
            )
            .push(footer(fl!(
                "con-bash-limitato-l-agente-pu-eseguire-solo-l-interprete-python-dell-ambiente-di-cad-studio-e-alcuni-comandi-di-sola-lettura-ls-cat-head-pi-sicuro-ma-l-agente-ha-meno-autonomia-per-diagnosticare-i-problemi-e-alcuni-tentativi-potrebbero-essere-rifiutati"
            )));
    } else {
        column = column.push(space()).push(
            section(fl!("agente")).push(footer(fl!(
                "codex-lavora-nella-sua-sandbox-pu-scrivere-solo-nella-cartella-del-progetto-e-non-usa-la-rete"
            ))),
        );
    }
    column.into()
}

/// One engine's section (AgentEngineSection,
/// SettingsView.swift:116-169): executable and version by locate state,
/// the custom-path and model fields, Verifica right-aligned (:156-161).
fn engine_section(
    state: &SettingsState,
) -> Column<'_, Message, cosmic::theme::Theme, cosmic::Renderer> {
    let kind = state.engine;
    let slot = engine_index(kind);
    let home = std::env::var("HOME").unwrap_or_default();
    let executable: Element<'_, Message> = match &state.locate[slot] {
        LocateState::Found { path, .. } => mono(abbreviate_with_tilde(path, &home)).into(),
        LocateState::Checking => cosmic::widget::progress_bar::indeterminate_circular().into(),
        LocateState::Unknown => body(fl!("settings-none")).into(),
        LocateState::Missing => body(fl!("non-trovato-2"))
            .class(Tone::Danger.text(Backdrop::Plain))
            .into(),
    };
    let version: Element<'_, Message> = match &state.locate[slot] {
        LocateState::Found { version, .. } => mono(version.clone()).into(),
        _ => body(fl!("settings-none")).into(),
    };
    let checking = matches!(state.locate[slot], LocateState::Checking);
    section(fl!("percorso-dell-eseguibile"))
        .push(labeled_row(fl!("eseguibile"), executable))
        .push(labeled_row(fl!("versione-2"), version))
        .push(
            text_input::text_input(fl!("automatico"), state.custom_paths[slot].as_str())
                .on_input(move |path| Message::CustomPath(kind, path))
                .font(cosmic::font::mono())
                .size(14),
        )
        .push(
            text_input::text_input(
                fl!("predefinito-di", agent = kind.display_name()),
                state.models[slot].as_str(),
            )
            .on_input(move |model| Message::Model(kind, model))
            .font(cosmic::font::mono())
            .size(14),
        )
        .push(
            Row::new().push(Space::new().width(Length::Fill)).push(
                button::text(fl!("verifica"))
                    .class(cosmic::theme::Button::Standard)
                    .on_press_maybe((!checking).then_some(Message::Verify(kind))),
            ),
        )
}

/// Python (SettingsView.swift:171-215): interpreter path, build123d by
/// environment state, Verifica + Reinstalla… (disabled while installing,
/// :206), the packages footer.
fn view_python(python: &EnvState) -> Element<'static, Message> {
    let interpreter = engine::python::env::interpreter_path(&engine::python::env::venv_dir());
    let build123d: Element<'_, Message> = match python {
        EnvState::Ready { build123d_version } => mono(build123d_version.clone()).into(),
        EnvState::Installing | EnvState::Checking | EnvState::Unknown => {
            cosmic::widget::progress_bar::indeterminate_circular().into()
        }
        EnvState::Missing => body(fl!("non-installato"))
            .class(Tone::Secondary.text(Backdrop::Plain))
            .into(),
        EnvState::Failed(_) => body(fl!("non-funzionante"))
            .class(Tone::Danger.text(Backdrop::Plain))
            .into(),
    };
    let installing = matches!(python, EnvState::Installing);
    let actions = Row::new()
        .spacing(8)
        .push(Space::new().width(Length::Fill))
        .push(
            button::text(fl!("verifica"))
                .class(cosmic::theme::Button::Standard)
                .on_press_maybe((!installing).then_some(Message::PythonCheck)),
        )
        .push(
            button::text(fl!("reinstalla"))
                .class(cosmic::theme::Button::Standard)
                .on_press_maybe((!installing).then_some(Message::PythonReinstall)),
        );
    section(fl!("interprete"))
        .push(labeled_row(
            fl!("interprete"),
            mono(interpreter.to_string_lossy()).into(),
        ))
        .push(labeled_row(fl!("build123d"), build123d))
        .push(actions)
        .push(footer(fl!("pacchetti-build123d-matplotlib-numpy")))
        .into()
}

/// Avanzate (SettingsView.swift:217-242).
fn view_advanced(state: &SettingsState) -> Element<'_, Message> {
    let formats = vec![
        fl!("stream-json-in-tempo-reale"),
        fl!("json-risposta-unica"),
    ];
    let selected = usize::from(!state.streaming);
    section(fl!("formato-di-output"))
        .push(cosmic::widget::dropdown(
            formats,
            Some(selected),
            |index| Message::Streaming(index == 0),
        ))
        .push(footer(fl!(
            "vale-per-claude-code-il-formato-json-non-mostra-il-testo-in-streaming-n-le-singole-attivit-ma-pu-essere-utile-con-versioni-che-non-supportano-lo-streaming"
        )))
        .push(space())
        .push(
            cosmic::widget::toggler(state.isolates)
                .label(fl!(
                    "ignora-le-configurazioni-personali-dell-agente"
                ))
                .on_toggle(Message::Isolates),
        )
        .push(footer(fl!(
            "l-agente-cad-non-carica-le-tue-impostazioni-utente-hook-plugin-e-server-mcp-di-claude-code-config-toml-di-codex-cos-parte-pi-velocemente-e-risponde-in-modo-prevedibile-disattiva-se-la-tua-autenticazione-dipende-da-impostazioni-utente-ad-esempio-apikeyhelper"
        )))
        .into()
}

// ─── About ──────────────────────────────────────────────────────────────────

/// The About window (upstream `showAboutPanel`,
/// Commands.swift:141-152): app icon, name, version, and the credits
/// paragraph — the catalog's literal-\n key, rendered as its two lines.
/// Nothing is interactive, so the element is generic over the host's
/// message type.
pub fn view_about<Message: 'static>() -> Element<'static, Message> {
    let credits =
        fl!("software-open-source-rilasciato-con-licenza-mit-sviluppato-da-filippo-finke");
    let mut column = Column::new()
        .spacing(10)
        .push(Space::new().height(Length::Fill))
        .push(
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(crate::ui::icon::named(crate::sheets::APP_ICON, 64))
                .push(Space::new().width(Length::Fill)),
        )
        .push(bold("CAD Studio".to_string(), 20.0))
        .push(body(env!("CARGO_PKG_VERSION").to_string()));
    for line in credits.lines() {
        column = column.push(body(line.to_string()).class(Tone::Secondary.text(Backdrop::Plain)));
    }
    container(column.push(Space::new().height(Length::Fill)))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .class(cosmic::theme::Container::WindowBackground)
        .into()
}

// ─── view helpers ───────────────────────────────────────────────────────────

/// A titled group of rows (upstream's Form sections).
fn section<'a>(title: String) -> Column<'a, Message, cosmic::theme::Theme, cosmic::Renderer> {
    Column::new().spacing(10).push(bold(title, 14.0))
}

/// A section-gap row (upstream's grouped-form spacing).
fn space() -> Space {
    Space::new().height(14.0)
}

fn footer<'a>(text: String) -> Text<'a> {
    body(text).class(Tone::Secondary.text(Backdrop::Plain))
}

/// A "Label: value" row (upstream LabeledContent).
fn labeled_row<'a>(
    label: String,
    value: Element<'a, Message>,
) -> Row<'a, Message, cosmic::theme::Theme, cosmic::Renderer> {
    Row::new()
        .spacing(8)
        .push(body(label).width(Length::Fixed(150.0)))
        .push(value)
}

fn body<'a>(label: impl Into<String>) -> Text<'a> {
    cosmic::widget::text(label.into()).wrapping(Wrapping::Word)
}

fn mono<'a>(label: impl Into<String>) -> Text<'a> {
    body(label).font(cosmic::font::mono())
}

fn bold<'a>(label: impl Into<String>, size: f32) -> Text<'a> {
    body(label).size(size).font(cosmic::iced::font::Font {
        weight: cosmic::iced::font::Weight::Bold,
        ..cosmic::font::default()
    })
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
        assert!(!stage(&Message::Tab(Tab::Python), &mut settings));
        assert!(!stage(
            &Message::ChooseLocation(PathBuf::from("/x")),
            &mut settings
        ));
        assert!(!stage(&Message::PythonCheck, &mut settings));
        assert!(!stage(&Message::PythonReinstall, &mut settings));
        assert!(!stage(
            &Message::Located(AgentKind::Claude, None),
            &mut settings
        ));
        assert!(
            !stage(&Message::Verify(AgentKind::Claude), &mut settings),
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

        assert!(stage(
            &Message::LocationChosen(PathBuf::from("/home/me/Documents")),
            &mut settings
        ));
        assert_eq!(
            settings.default_project_location,
            PathBuf::from("/home/me/Documents")
        );

        assert!(stage(
            &Message::Language(LanguageChoice::Deutsch),
            &mut settings
        ));
        assert_eq!(settings.language.as_deref(), Some("de"));
        assert!(stage(
            &Message::Language(LanguageChoice::System),
            &mut settings
        ));
        assert_eq!(settings.language, None, "System clears the override");

        assert!(stage(&Message::Engine(AgentKind::Codex), &mut settings));
        assert_eq!(settings.agent_engine, "codex");

        assert!(stage(
            &Message::CustomPath(AgentKind::Codex, "  ~/.local/bin/codex  ".into()),
            &mut settings
        ));
        assert_eq!(settings.codex_path.as_deref(), Some("~/.local/bin/codex"));
        assert!(stage(
            &Message::CustomPath(AgentKind::Codex, "   ".into()),
            &mut settings
        ));
        assert_eq!(settings.codex_path, None, "a blank path clears the setting");
        assert_eq!(
            settings.claude_path, None,
            "the other engine's key is untouched"
        );

        assert!(stage(
            &Message::Model(AgentKind::Codex, " gpt-5.2-codex ".into()),
            &mut settings
        ));
        assert_eq!(settings.codex_model.as_deref(), Some("gpt-5.2-codex"));
        assert!(stage(
            &Message::Model(AgentKind::Codex, String::new()),
            &mut settings
        ));
        assert_eq!(settings.codex_model, None);

        assert!(stage(&Message::Effort(Effort::Low), &mut settings));
        assert_eq!(settings.claude_effort.as_deref(), Some("low"));
        assert!(stage(&Message::Effort(Effort::AgentDefault), &mut settings));
        assert_eq!(settings.claude_effort, None);

        assert!(stage(&Message::RestrictedBash(true), &mut settings));
        assert!(settings.restricted_bash);
        assert!(stage(&Message::Streaming(false), &mut settings));
        assert!(!settings.streaming_output);
        assert!(stage(&Message::Isolates(false), &mut settings));
        assert!(!settings.isolates_claude);

        assert!(stage(
            &Message::PrintPref(PrintPrefMessage::PrinterType(PrinterType::Sls)),
            &mut settings
        ));
        assert!(stage(
            &Message::PrintPref(PrintPrefMessage::PrinterModel("  P1S  ".into())),
            &mut settings
        ));
        assert!(stage(
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
            assert!(stage(&message, &mut settings));
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
}
