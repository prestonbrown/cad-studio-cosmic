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
//! User-visible strings read through `fl!` at render time (the key names
//! the en/main.ftl entry). The card, glyphs, slider and ink levels are the
//! shared overlay look in [`crate::parts_overlay`]. A busy row's slider
//! keeps its colors while inert — the interactions themselves are gated
//! (upstream `.disabled(isBusy)`).

use std::collections::{BTreeMap, HashMap};

use cosmic::Element;
use cosmic::iced::widget::pick_list;
use cosmic::iced::widget::text::{Span, Wrapping};
use cosmic::iced::{Alignment, Color, Length};
use cosmic::widget::{Column, Row, Space, container, text, tooltip};
use engine::project::versions::ProjectVersion;

use crate::fl;
use crate::parameters::{ParametersPanel, SliderState, format_display};
use crate::parts_overlay::{KNOB, small_slider, with_rail_ends};
use crate::typography;
use crate::ui::surface::{self, Corner, Shadow};
use crate::ui::{Backdrop, Symbol, Tone, button, icon::icon};

/// The panel floats on a material.
const BACKDROP: Backdrop = Backdrop::Material;

/// The panel's fixed width (ParametersOverlay.swift:109).
const PANEL_WIDTH: f32 = 280.0;
/// The scroll area's height cap (ParametersOverlay.swift:77). Upstream's
/// scroll view grows with its content up to this; an iced scrollable needs
/// a bounded height up front, so the view estimates the content height
/// (one row ≈ head line + slider + spacing) and clamps it to the cap.
const SCROLL_MAX_HEIGHT: f32 = 340.0;
/// The name line's height (an 11 px monospaced line).
const NAME_LINE: f32 = NAME_TEXT + 3.0;
/// Names and values: upstream `.system(size: 11, design: .monospaced)`.
const NAME_TEXT: f32 = typography::scaled(11.0);
/// One row: the name line, 2 px, the slider (ParametersOverlay.swift:140).
const ROW_HEIGHT: f32 = NAME_LINE + 2.0 + KNOB;
/// Rows sit 10 apart (ParametersOverlay.swift:65).
const ROW_SPACING: f32 = 10.0;
/// The pop-up picker's accent badge (13 px square, measured), the gap
/// between the longest option and the badge, and the option text size.
const POPUP_BADGE: f32 = 13.0;
const POPUP_GAP: f32 = 7.0;
const POPUP_TEXT: f32 = typography::SMALL_CONTROL;

/// Upstream's orange previous-value tint (`foregroundStyle(.orange)`,
/// ParametersOverlay.swift:151) — same explicit color family as the chat
/// pane's warn tint, theme-independent like upstream's.
const ORANGE: Color = Color::from_rgb(1.0, 0.58, 0.02);

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
    let mut rows = Column::new().spacing(ROW_SPACING);
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
    let mut panel_view = Column::new()
        .push(header())
        .push(
            cosmic::widget::scrollable::scrollable(
                rows.padding([0.0, 12.0, 10.0, 12.0]).width(Length::Fill),
            )
            .height(Length::Fixed(scroll_height(panel.sliders.len())))
            .width(Length::Fill),
        )
        // upstream's Divider: a 1 pt rule that reads as nothing on the
        // material (measured ≤ 2 RGB units) — its height is kept
        .push(Space::new().height(1.0))
        .push(compare_footer(ctx));
    if ctx.compare.is_some() {
        panel_view = panel_view.push(compare_caption());
    }
    container(panel_view)
        .width(Length::Fixed(PANEL_WIDTH))
        .class(surface::card(Corner::S, Shadow::Overlay { radius: 8.0 }))
        .into()
}

/// The scroll area's height: every row, the gaps between them and the
/// 10 px bottom padding, capped at [`SCROLL_MAX_HEIGHT`].
fn scroll_height(rows: usize) -> f32 {
    let rows = rows as f32;
    (rows * ROW_HEIGHT + (rows - 1.0).max(0.0) * ROW_SPACING + 10.0).min(SCROLL_MAX_HEIGHT)
}

/// The panel's `slider.horizontal.3` glyph, centered in its 16 px cell.
fn slider_glyph() -> Element<'static, Message> {
    container(icon(Symbol::SliderHorizontal3, 13.6, Tone::Primary, BACKDROP))
        .width(16.0)
        .height(16.0)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

/// The collapsed state: the material capsule button that expands the
/// panel (ParametersOverlay.swift:13-25) — the slider glyph and
/// "Parameters" at `.callout` medium, padded 10 × 6.
fn collapsed_capsule() -> Element<'static, Message> {
    let label = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(slider_glyph())
        .push(
            text(fl!("parametri"))
                .size(typography::CALLOUT)
                .font(cosmic::font::semibold())
                .class(Tone::Primary.text(BACKDROP)),
        );
    let capsule = button::borderless(
        container(label)
            .padding([6.0, 10.0])
            .class(surface::card(Corner::Xl, Shadow::Overlay { radius: 6.0 })),
        Some(Message::Collapse(false)),
    )
    .name(fl!("espandi-i-parametri"));
    button::with_help(
        capsule,
        fl!("espandi-i-parametri"),
        cosmic::widget::tooltip::Position::Bottom,
    )
}

/// The expanded panel's header: the collapse button (glyph, bold
/// "Parameters" at `.headline`, the chevron) and the close ✕
/// (ParametersOverlay.swift:33-59), padded 12 / 12 / 8.
fn header() -> Element<'static, Message> {
    let collapse = button::borderless(
        Row::new()
            .spacing(6)
            .align_y(Alignment::Center)
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(slider_glyph())
                    .push(
                        text(fl!("parametri"))
                            .size(typography::HEADLINE)
                            .font(cosmic::font::semibold())
                            .class(Tone::Primary.text(BACKDROP)),
                    ),
            )
            .push(icon(Symbol::ChevronUp, 14.0, Tone::Secondary, BACKDROP)),
        Some(Message::Collapse(true)),
    )
    .name(fl!("comprimi-i-parametri"));
    let collapse = button::with_help(
        collapse,
        fl!("comprimi-i-parametri"),
        cosmic::widget::tooltip::Position::Bottom,
    );
    let close = button::icon(
        Symbol::Xmark,
        11.0,
        Tone::Secondary,
        BACKDROP,
        fl!("nascondi-parametri-i"),
        Some(Message::Close),
    );
    Row::new()
        .align_y(Alignment::Center)
        .push(collapse)
        .push(Space::new().width(Length::Fill))
        .push(close)
        .padding([12.0, 12.0, 8.0, 12.0])
        .into()
}

/// One parameter's row: the name line (monospace name, struck-through
/// previous value when comparing, the plain numeric field) over the
/// small slider (ParameterSlider's body, ParametersOverlay.swift:139-169).
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
        .align_y(Alignment::Center)
        .push(
            text(name.to_string())
                .font(cosmic::font::mono())
                .size(NAME_TEXT)
                .line_height(line_height(NAME_LINE))
                .width(Length::Fill)
                .wrapping(Wrapping::None)
                .ellipsize(cosmic::iced::core::text::Ellipsize::Middle(
                    cosmic::iced::core::text::EllipsizeHeightLimit::Lines(1),
                ))
                .class(if busy {
                    Tone::Tertiary.text(BACKDROP)
                } else {
                    Tone::Primary.text(BACKDROP)
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
    let track = small_slider(range, state.value, on_change, Length::Fill).on_release(if busy {
        Message::Inert
    } else {
        Message::Release(name.to_string())
    });

    Column::new()
        .spacing(2)
        .push(head.height(Length::Fixed(NAME_LINE)))
        .push(with_rail_ends(track, Length::Fill))
        .into()
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
        text(fl!("valore-nella-versione-a-confronto")),
        cosmic::widget::tooltip::Position::Bottom,
    )
    .into()
}

/// The live numeric field (ParametersOverlay.swift:154-159): upstream's
/// `.plain` text field — monospaced 11, trailing-aligned, 56 wide, no
/// border — showing the raw draft while one is staged, otherwise the live
/// value at 0…3 fraction digits. While the busy gate is on the field
/// mounts without handlers, which is iced's disabled input.
fn numeric_field<'a>(
    name: &'a str,
    state: &'a SliderState,
    draft: Option<&'a str>,
    busy: bool,
) -> Element<'a, Message> {
    let shown = draft
        .map(str::to_string)
        .unwrap_or_else(|| format_display(state.value, 3));
    let input = cosmic::iced::widget::text_input("", &shown)
        .width(56)
        .size(NAME_TEXT)
        .line_height(line_height(NAME_LINE))
        .padding(0)
        .font(cosmic::font::mono())
        .align_x(cosmic::iced::alignment::Horizontal::Right)
        .class(cosmic::theme::iced::TextInput::Search);
    let input = if busy {
        input
    } else {
        let for_input = name.to_string();
        let for_submit = name.to_string();
        input
            .on_input(move |text| Message::Draft(for_input.clone(), text))
            .on_submit(Message::Submit(for_submit))
    };
    input.into()
}

/// The "Compare with" footer (ParametersOverlay.swift:80-98): the
/// secondary label and the pop-up picker — the selection, then the
/// accent badge with the up/down chevrons; newest version first; dimmed
/// and inert when no versions exist.
fn compare_footer(ctx: &OverlayContext<'_>) -> Element<'static, Message> {
    let bottom = if ctx.compare.is_some() { 4.0 } else { 8.0 };
    let enabled = !ctx.versions.is_empty();
    let picker: Element<'static, Message> = if enabled {
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
        pick_list::PickList::new(options, selected, |option| {
            Message::Compare(option.number())
        })
        .text_size(POPUP_TEXT)
        // the list reserves one text size for its own (hidden) handle
        .padding([0.0, POPUP_BADGE + POPUP_GAP - POPUP_TEXT, 0.0, 0.0])
        .handle(pick_list::Handle::None)
        .into()
    } else {
        container(
            text(selected_label(ctx.compare))
                .size(POPUP_TEXT)
                .class(Tone::Tertiary.text(BACKDROP)),
        )
        .padding([0.0, POPUP_BADGE + POPUP_GAP, 0.0, 0.0])
        .into()
    };
    let badge = container(icon(Symbol::ChevronUpChevronDown, 12.0, Tone::OnAccent, BACKDROP))
        .width(POPUP_BADGE)
        .height(POPUP_BADGE)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(cosmic::theme::Container::custom(move |theme| {
            cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(if enabled {
                    Tone::Accent.color(theme, BACKDROP)
                } else {
                    Tone::Tertiary.color(theme, BACKDROP)
                })),
                border: cosmic::iced::Border {
                    radius: theme.cosmic().corner_radii.radius_xs.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));
    // the badge sits over the picker's right padding; it takes no input,
    // so presses reach the picker underneath
    let popup = cosmic::iced::widget::stack![
        picker,
        container(badge)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::Center),
    ];
    Row::new()
        .spacing(0)
        .align_y(Alignment::Center)
        .push(
            text(fl!("confronta-con"))
                .size(typography::SMALL_CONTROL)
                .class(Tone::Secondary.text(BACKDROP)),
        )
        .push(Space::new().width(Length::Fill))
        .push(popup)
        .padding([8.0, 12.0, bottom, 12.0])
        .into()
}

/// The caption under the footer while comparing
/// (ParametersOverlay.swift:100-106).
fn compare_caption() -> Element<'static, Message> {
    container(
        text(fl!("in-arancione-la-forma-della-versione-scelta-per-vederne-i-colori-ripristinala-dal-menu-versioni"))
            .size(typography::CAPTION)
            .class(Tone::Secondary.text(BACKDROP))
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
            Self::NoVersion => write!(f, "{}", fl!("nessuna")),
            // `versione-lld`
            Self::Version(number) => write!(f, "{}", fl!("versione-lld", version = number)),
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

// ─── shared bits ───────────────────────────────────────────────────────────

fn line_height(pixels: f32) -> cosmic::iced::widget::text::LineHeight {
    cosmic::iced::widget::text::LineHeight::Absolute(pixels.into())
}

fn rich_text_element(span: Span<'_, String>) -> Element<'_, Message> {
    cosmic::iced::widget::rich_text([span])
        .size(typography::CAPTION2)
        .font(cosmic::font::default())
        .wrapping(Wrapping::Word)
        .into()
}
