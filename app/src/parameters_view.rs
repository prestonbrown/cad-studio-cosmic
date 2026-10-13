//! Parameter overlay widgets — the port of upstream
//! `Viewer3D/ParametersOverlay.swift` onto the pure model in
//! [`crate::parameters`]. This module only MAPS: every behavioral rule
//! (range heuristics, commit rounding, the sync guard) lives in the model;
//! the view renders its state verbatim and routes every interaction to a
//! [`Message`].
//!
//! The overlay renders through [`view_parameters`] with the widget state
//! its owner holds: the panel (pure model rows), the text-field drafts, the
//! committed parameter values (the commit rule's input and the
//! previous-value comparison), and an [`OverlayContext`] with everything
//! only the shell knows — the compared version's parameter values, the
//! busy gate, the version list, and the compare picker's selection.
//!
//! Split of message handling (the chat pane's contract): the MOUNT owns
//! what is purely widget state (drags, drafts, collapse, close) and turns
//! [`Message::Release`]/[`Message::Submit`] into commit values; the SHELL
//! intercepts the rest — `Message::Collapse` (persist
//! `Settings.parameters_collapsed` through the engine's `settings_path()`,
//! then forward), `Message::Compare` (the bridge's compare wiring), and the
//! mount's emitted `ParameterCommit` (`Bridge::set_parameter`). The pane
//! mounts the overlay in [`crate::viewer_pane::ViewerPane::view_model_3d`].
//!
//! User-visible strings are the `app/i18n/en/main.ftl` catalog values; each
//! constant names its key (the `fl!()` swap lands with the i18n scaffold in
//! the windows task). Icon names are freedesktop-theme approximations of
//! upstream SF Symbols; the theme in play decides what actually shows
//! (cosmetic — flagged for the shell task). Also cosmetic and flagged: the
//! panel has no drop shadow (no widget shadows in iced), and a busy row's
//! slider keeps its colors while inert — the interactions themselves are
//! gated (upstream `.disabled(isBusy)`).

use std::collections::{BTreeMap, HashMap};

use cosmic::Element;
use cosmic::iced::widget::text::{Span, Wrapping};
use cosmic::iced::widget::{pick_list, slider};
use cosmic::iced::{Color, Length};
use cosmic::widget::{Column, Row, Space, button, container, text, text_input, tooltip};
use engine::project::versions::ProjectVersion;

use crate::parameters::{ParametersPanel, SliderState, format_display};

/// This module's text widget, pinned to cosmic's theme/renderer (same
/// invariance note as the chat pane's).
type Text<'a> = cosmic::iced::widget::Text<'a, cosmic::theme::Theme, cosmic::Renderer>;

// ─── catalog strings (each constant names its en/main.ftl key) ─────────────

/// `parametri`
const PARAMETRI: &str = "Parameters";
/// `comprimi-i-parametri`
const COLLAPSE_PARAMETERS: &str = "Collapse parameters";
/// `espandi-i-parametri`
const EXPAND_PARAMETERS: &str = "Expand parameters";
/// `nascondi-parametri`
const HIDE_PARAMETERS: &str = "Hide Parameters";
/// `nascondi-parametri-i`
const HIDE_PARAMETERS_SHORTCUT: &str = "Hide Parameters (⌥⌘I)";
/// `confronta-con`
const COMPARE_WITH: &str = "Compare with";
/// `nessuna`
const NONE: &str = "None";
/// `valore-nella-versione-a-confronto`
const PREVIOUS_TOOLTIP: &str = "Value in the compared version";
/// `in-arancione-la-forma-della-versione-scelta-per-vederne-i-colori-ripristinala-dal-menu-versioni`
const COMPARE_CAPTION: &str = "Orange shows the shape of the chosen version. To see its colors, restore it from the Versions menu.";

/// The panel's fixed width (ParametersOverlay.swift:109).
const PANEL_WIDTH: f32 = 280.0;
/// The scroll area's height cap (ParametersOverlay.swift:77). Upstream's
/// scroll view grows with its content up to this; an iced scrollable needs
/// a bounded height up front, so the view estimates the content height
/// (one row ≈ head line + slider + spacing) and clamps it to the cap.
const SCROLL_MAX_HEIGHT: f32 = 340.0;
const ROW_HEIGHT_ESTIMATE: f32 = 46.0;

/// Upstream's orange previous-value tint (`foregroundStyle(.orange)`,
/// ParametersOverlay.swift:151) — same explicit color family as the chat
/// pane's warn tint, theme-independent like upstream's.
const ORANGE: Color = Color::from_rgb(1.0, 0.58, 0.02);

/// Upstream's `slider.horizontal.3` — freedesktop approximation
/// (cosmetic; the icon theme decides).
const SLIDER_ICON: &str = "preferences-desktop";

// ─── messages ──────────────────────────────────────────────────────────────

/// The overlay's messages. Variants without a comment are handled entirely
/// by the mount; the commented ones the shell intercepts (module docs).
#[derive(Clone, Debug)]
pub enum Message {
    /// A slider drag's live value (drags are in progress until
    /// [`Message::Release`]).
    Drag(String, f64),
    /// The slider was released — the commit path.
    Release(String),
    /// A text-field keystroke; the payload is the raw draft.
    Draft(String, String),
    /// The field was submitted (Return) — the commit path.
    Submit(String),
    /// The collapse button; the payload is the new collapsed state. The
    /// SHELL persists it, then forwards.
    Collapse(bool),
    Close,
    /// The compare picker's selection (`None` = "Nessuna"). The SHELL owns
    /// the state; the view only reports.
    Compare(Option<i64>),
    /// The busy-gated slider's placeholder emission: an iced slider cannot
    /// be built without an `on_change`, so while the busy gate is on it
    /// emits this and the mount drops it (upstream `.disabled(isBusy)`).
    Inert,
}

// ─── inputs ────────────────────────────────────────────────────────────────

/// What only the shell knows about the overlay: the compared version's
/// parameter values, the busy gate, the version list (ascending, like
/// `load_versions`; the picker shows them newest-first), and the compare
/// selection.
pub struct OverlayContext<'a> {
    pub previous: &'a BTreeMap<String, f64>,
    pub busy: bool,
    pub versions: &'a [ProjectVersion],
    pub compare: Option<i64>,
}

// ─── view ──────────────────────────────────────────────────────────────────

/// The whole overlay: a capsule button when collapsed, otherwise the panel
/// (header, scrollable rows, compare footer). The element borrows the
/// widget state (`panel`, `drafts`, `committed`); everything it renders
/// from `ctx` is copied or formatted into owned strings, so the context's
/// own lifetime is free.
pub fn view_parameters<'a>(
    panel: &'a ParametersPanel,
    drafts: &'a HashMap<String, String>,
    committed: &'a BTreeMap<String, f64>,
    ctx: &OverlayContext<'_>,
) -> Element<'a, Message> {
    if panel.collapsed {
        return collapsed_capsule();
    }
    let mut rows = Column::new().spacing(10);
    for (name, state) in &panel.sliders {
        rows = rows.push(parameter_row(
            name,
            state,
            drafts.get(name).map(String::as_str),
            committed.get(name).copied(),
            ctx.previous.get(name).copied(),
            ctx.busy,
        ));
    }
    let scroll_height =
        (panel.sliders.len() as f32 * ROW_HEIGHT_ESTIMATE + 10.0).min(SCROLL_MAX_HEIGHT);
    let mut panel_view = Column::new()
        .push(header())
        .push(
            cosmic::widget::scrollable::scrollable(
                rows.padding([0.0, 12.0, 10.0, 12.0]).width(Length::Fill),
            )
            .height(Length::Fixed(scroll_height))
            .width(Length::Fill),
        )
        .push(cosmic::widget::divider::horizontal::default())
        .push(compare_footer(ctx));
    if ctx.compare.is_some() {
        panel_view = panel_view.push(compare_caption());
    }
    container(panel_view)
        .width(Length::Fixed(PANEL_WIDTH))
        .class(panel_style())
        .into()
}

/// The collapsed state: the capsule button that expands the panel
/// (ParametersOverlay.swift:13-25).
fn collapsed_capsule() -> Element<'static, Message> {
    let label = Row::new()
        .spacing(6)
        .push(icon(SLIDER_ICON, 14))
        .push(text(PARAMETRI).size(14));
    let capsule = button::custom(container(label).padding([6.0, 10.0]))
        .class(cosmic::theme::Button::Standard)
        .on_press(Message::Collapse(false));
    tooltip(
        capsule,
        text(EXPAND_PARAMETERS),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into()
}

/// The expanded panel's header: the collapse button (label + chevron) and
/// the close button (ParametersOverlay.swift:33-59).
fn header() -> Element<'static, Message> {
    let collapse = button::custom(
        Row::new()
            .spacing(6)
            .push(icon(SLIDER_ICON, 14))
            .push(text(PARAMETRI).size(14))
            .push(secondary_text("⌃").size(10)), // chevron.up stand-in
    )
    .class(cosmic::theme::Button::Text)
    .on_press(Message::Collapse(true));
    let collapse = tooltip(
        collapse,
        text(COLLAPSE_PARAMETERS),
        cosmic::widget::tooltip::Position::FollowCursor,
    );
    let close = button::icon(icon_handle("window-close", 12))
        .on_press(Message::Close)
        .name(HIDE_PARAMETERS)
        .description(HIDE_PARAMETERS_SHORTCUT);
    Row::new()
        .push(collapse)
        .push(Space::new().width(Length::Fill))
        .push(close)
        .padding([12.0, 12.0, 8.0, 12.0])
        .into()
}

/// One parameter's row: the name line (monospace name, struck-through
/// previous value when comparing, the numeric field) over the slider
/// (ParameterSlider's body, ParametersOverlay.swift:139-169).
fn parameter_row<'a>(
    name: &'a str,
    state: &'a SliderState,
    draft: Option<&'a str>,
    committed: Option<f64>,
    previous: Option<f64>,
    busy: bool,
) -> Element<'a, Message> {
    let mut head = Row::new()
        .spacing(0)
        .push(
            text(name.to_string())
                .font(cosmic::font::mono())
                .size(11)
                .width(Length::Fill)
                .wrapping(Wrapping::None)
                .ellipsize(cosmic::iced::core::text::Ellipsize::Middle(
                    cosmic::iced::core::text::EllipsizeHeightLimit::Lines(1),
                ))
                .class(if busy {
                    secondary_text_class()
                } else {
                    cosmic::theme::Text::Default
                }),
        )
        .push(Space::new().width(6));
    // The previous value shows only while it differs from the parameter's
    // COMMITTED value (ParametersOverlay.swift:147).
    if let (Some(previous), Some(committed)) = (previous, committed) {
        if previous != committed {
            head = head.push(previous_value(previous));
        }
    }
    head = head.push(numeric_field(name, state, draft, busy));

    let range = state.min..=state.max;
    // One closure carrying the gate: a busy slider reports inert (iced
    // sliders cannot be built without an on_change).
    let drag_name = name.to_string();
    let on_change = move |value: f64| {
        if busy {
            Message::Inert
        } else {
            Message::Drag(drag_name.clone(), value)
        }
    };
    let track = slider::Slider::new(range, state.value, on_change)
        .on_release(if busy {
            Message::Inert
        } else {
            Message::Release(name.to_string())
        })
        .width(Length::Fill);

    Column::new().spacing(2).push(head).push(track).into()
}

/// The struck-through orange previous value
/// (ParametersOverlay.swift:148-153) — fraction digits 0…2 (NOTE 1: two,
/// one fewer than the live field's three).
fn previous_value(value: f64) -> Element<'static, Message> {
    let struck: Span<'_, String> = Span::new(format_display(value, 2))
        .strikethrough(true)
        .color(ORANGE);
    let struck = rich_text_element(struck);
    tooltip(
        struck,
        text(PREVIOUS_TOOLTIP),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into()
}

/// The live numeric field: the raw draft while one is staged, otherwise the
/// live value at 0…3 fraction digits (ParametersOverlay.swift:154-159).
/// While the busy gate is on the field mounts without handlers, which is
/// iced's disabled input.
fn numeric_field<'a>(
    name: &'a str,
    state: &'a SliderState,
    draft: Option<&'a str>,
    busy: bool,
) -> Element<'a, Message> {
    let shown = draft
        .map(str::to_string)
        .unwrap_or_else(|| format_display(state.value, 3));
    let input = text_input::text_input("", shown).width(56).size(11);
    let input = if busy {
        input
    } else {
        let for_input = name.to_string();
        let for_submit = name.to_string();
        input
            .on_input(move |text| Message::Draft(for_input.clone(), text))
            .on_submit_maybe(Some(move |_text| Message::Submit(for_submit.clone())))
    };
    input.into()
}

/// The "Compare with" footer (ParametersOverlay.swift:80-98): the label and
/// the picker, newest version first, disabled (a plain label — pick lists
/// cannot be built inert) when no versions exist.
fn compare_footer(ctx: &OverlayContext<'_>) -> Element<'static, Message> {
    let bottom = if ctx.compare.is_some() { 4.0 } else { 8.0 };
    let mut footer = Row::new()
        .spacing(0)
        .push(secondary_text(COMPARE_WITH))
        .push(Space::new().width(Length::Fill));
    if ctx.versions.is_empty() {
        footer = footer.push(secondary_text(selected_label(ctx.compare)));
    } else {
        let mut options = vec![CompareOption::NoVersion];
        options.extend(
            ctx.versions
                .iter()
                .rev()
                .map(|version| CompareOption::Version(version.number)),
        );
        let selected = match ctx.compare {
            Some(number) => Some(CompareOption::Version(number)),
            None => Some(CompareOption::NoVersion),
        };
        footer = footer.push(
            pick_list::PickList::new(options, selected, |option| {
                Message::Compare(option.number())
            })
            .placeholder(NONE),
        );
    }
    footer.padding([8.0, 12.0, bottom, 12.0]).into()
}

/// The caption under the footer while comparing
/// (ParametersOverlay.swift:100-106).
fn compare_caption() -> Element<'static, Message> {
    container(
        secondary_text(COMPARE_CAPTION)
            .size(12)
            .wrapping(Wrapping::Word)
            .width(Length::Fill),
    )
    .padding([0.0, 12.0, 12.0, 12.0])
    .width(Length::Fill)
    .into()
}

// ─── compare options ───────────────────────────────────────────────────────

/// The picker's entries: "Nessuna" plus one "Versione n" per version
/// (ParametersOverlay.swift:88-91).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOption {
    NoVersion,
    Version(i64),
}

impl CompareOption {
    fn number(self) -> Option<i64> {
        match self {
            Self::NoVersion => None,
            Self::Version(number) => Some(number),
        }
    }
}

impl std::fmt::Display for CompareOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // `nessuna`
            Self::NoVersion => write!(f, "{NONE}"),
            // `versione-lld`
            Self::Version(number) => write!(f, "Version {number}"),
        }
    }
}

/// The picker's current label — also what the disabled footer shows.
fn selected_label(compare: Option<i64>) -> String {
    match compare {
        Some(number) => CompareOption::Version(number).to_string(),
        None => CompareOption::NoVersion.to_string(),
    }
}

// ─── shared bits (the chat pane keeps private twins; modules stay
// self-contained rather than cross-importing widgets) ───────────────────────

fn secondary_text<'a>(body: impl Into<String>) -> Text<'a> {
    text(body.into()).class(secondary_text_class())
}

fn secondary_text_class() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| {
        let mut color = Color::from(theme.cosmic().background(false).on);
        color.a *= 0.65;
        cosmic::iced::widget::text::Style {
            color: Some(color),
            ..Default::default()
        }
    })
}

/// The panel surface: solid theme base with a hairline border, radius 12
/// (upstream's `.regularMaterial` + separator stroke,
/// ParametersOverlay.swift:110-111; the drop shadow has no iced port).
fn panel_style() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let base = Color::from(theme.cosmic().background(false).base);
        let mut border = Color::from(theme.cosmic().background(false).on);
        border.a = 0.15;
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(base)),
            border: cosmic::iced::Border {
                color: border,
                width: 1.0,
                radius: 12.0.into(),
            },
            ..Default::default()
        }
    })
}

fn rich_text_element(span: Span<'_, String>) -> Element<'_, Message> {
    cosmic::iced::widget::rich_text([span])
        .size(10) // upstream's caption2
        .font(cosmic::font::default())
        .wrapping(Wrapping::Word)
        .into()
}

fn icon(name: &str, size: u16) -> cosmic::widget::Icon {
    cosmic::widget::icon::from_name(name).size(size).icon()
}

fn icon_handle(name: &str, size: u16) -> cosmic::widget::icon::Handle {
    cosmic::widget::icon::from_name(name).size(size).handle()
}
