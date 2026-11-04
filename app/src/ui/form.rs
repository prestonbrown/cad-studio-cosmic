//! macOS's grouped Form (`.formStyle(.grouped)`): each `Section` is a
//! rounded box of rows, label at the leading edge and control at the
//! trailing one, the rows split by inset hairlines, and the section footer
//! below the box in secondary text against the trailing edge. The settings
//! window's four tabs and the first-run sheet's print preferences draw
//! with it.
//!
//! Geometry measured off upstream (shots/upstream/settings-*.png and
//! setup-sheet.png at 1x): rows 36 px plus a 1 px hairline, the label, the control and the
//! hairlines inset 10 px from the box edge, 10 px between boxes, the box
//! 20 px in from the window or sheet edge, a footer's text flush with the
//! box's trailing edge. The fill and hairline are
//! washes of the theme's `on` color, so the box reads one step off its
//! surface in either appearance.

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Background, Length};
use cosmic::widget::{Column, Row, Space, container};

use super::surface::{self, Axis, Corner};
use super::tone::{Backdrop, Tone};

/// A form row's height.
pub const ROW_HEIGHT: f32 = 36.0;
/// The label / control / hairline inset from the box edge.
pub const INSET: f32 = 10.0;
/// The box's inset from the window or sheet edge.
pub const MARGIN: f32 = 20.0;
/// Between two boxes.
pub const SECTION_GAP: f32 = 10.0;
/// Below a footer, before the next box (on top of the section gap).
const FOOTER_BOTTOM: f32 = 16.0;
/// A footer's leading inset: its lines run nearly box-wide, flush with
/// the box's trailing edge.
const FOOTER_LEADING: f32 = 6.0;

/// The box fill against [`Tone::Quaternary`] (upstream's box sits 4
/// levels off its surface: 232 on 236, 251 on 255).
const FILL: f32 = 0.3;
/// The box stroke and row hairlines against [`Tone::Separator`] (10
/// levels off the surface).
const HAIRLINE: f32 = 0.5;

/// One grouped section: `rows` in the rounded box, hairlines between them.
pub fn section<'a, M: 'a>(rows: Vec<Element<'a, M>>) -> Element<'a, M> {
    let count = rows.len();
    let mut column = Column::new();
    for (index, row) in rows.into_iter().enumerate() {
        column = column.push(row);
        if index + 1 < count {
            column = column.push(hairline());
        }
    }
    // the stroke sits outside the rows, as AppKit draws the box
    container(column)
        .padding(1)
        .width(Length::Fill)
        .class(surface::fill(Tone::Quaternary, FILL, Corner::S, Some(HAIRLINE)))
        .into()
}

/// The hairline between two rows, inset from both box edges.
fn hairline<'a, M: 'a>() -> Element<'a, M> {
    container(surface::divider_scaled(Axis::Horizontal, HAIRLINE))
        .padding([0.0, INSET])
        .into()
}

/// A `LabeledContent` / `Picker` / `Toggle` row: `label` leading, `control`
/// against the trailing edge, vertically centered.
pub fn row<'a, M: 'a>(label: String, control: impl Into<Element<'a, M>>) -> Element<'a, M> {
    styled_row(label_text(label), control)
}

/// A [`row`] whose label the caller has styled (a monospaced label beside
/// a monospaced field: `.fontDesign(.monospaced)` on a labeled `TextField`
/// sets both).
pub fn styled_row<'a, M: 'a>(
    label: cosmic::widget::Text<'a, cosmic::Theme>,
    control: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    Row::new()
        .push(label)
        .push(Space::new().width(Length::Fill))
        .push(control)
        .spacing(12.0)
        .padding([0.0, INSET])
        .height(Length::Fixed(ROW_HEIGHT))
        .align_y(Alignment::Center)
        .into()
}

/// A row whose content spans the box (a buttons row, a bare note).
pub fn full_row<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fixed(ROW_HEIGHT))
        .padding([0.0, INSET])
        .align_y(Alignment::Center)
        .into()
}

/// A row's leading label: body text, primary.
pub fn label_text<'a>(label: String) -> cosmic::widget::Text<'a, cosmic::Theme> {
    cosmic::widget::text(label)
        .wrapping(Wrapping::None)
        .class(Tone::Primary.text(Backdrop::Plain))
}

/// A section footer: secondary body text against the trailing edge, with
/// the gaps that separate it from its box and from the next one.
pub fn footer<'a, M: 'a>(text: String) -> Element<'a, M> {
    container(
        cosmic::widget::text(text)
            .wrapping(Wrapping::Word)
            .width(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Right)
            .class(Tone::Secondary.text(Backdrop::Plain)),
    )
    .padding([0.0, 0.0, FOOTER_BOTTOM, FOOTER_LEADING])
    .width(Length::Fill)
    .into()
}

/// A whole grouped Form: `parts` (sections and footers) stacked at the
/// section gap.
pub fn form<'a, M: 'a>(parts: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut column = Column::new().spacing(SECTION_GAP).width(Length::Fill);
    for part in parts {
        column = column.push(part);
    }
    column.into()
}

/// The width a form row's text field takes at the trailing edge.
pub const FIELD_WIDTH: f32 = 280.0;

/// A grouped Form's `TextField`: at rest the value (or the tertiary
/// placeholder) sits against the row's trailing edge with no box of its
/// own, the row being its frame; while focused it edits in place on the
/// text background. `mono` for the path and model fields
/// (`.fontDesign(.monospaced)`).
///
/// libcosmic's text input aligns its text by the paragraph's direction
/// alone (`effective_alignment`, src/widget/text_input/input.rs:3551 at
/// the pinned rev) and lays out at the width it is given, so it cannot
/// show a left-to-right value trailing-aligned. The resting value is a
/// right-aligned text under the input, and the input draws its own text
/// and background only in the focused style.
pub fn field<'a, M: Clone + 'static>(
    placeholder: String,
    value: &'a str,
    mono: bool,
    on_input: impl Fn(String) -> M + 'a,
) -> Element<'a, M> {
    let font = if mono {
        cosmic::font::mono()
    } else {
        cosmic::font::default()
    };
    let resting = if value.is_empty() {
        cosmic::widget::text(placeholder.clone()).class(Tone::Tertiary.text(Backdrop::Plain))
    } else {
        cosmic::widget::text(value.to_owned()).class(Tone::Primary.text(Backdrop::Plain))
    };
    let resting = resting
        .font(font)
        .wrapping(Wrapping::None)
        .width(Length::Fill)
        .align_x(cosmic::iced::alignment::Horizontal::Right);
    let input = cosmic::widget::text_input::inline_input(placeholder, value)
        .on_input(on_input)
        .style(field_style())
        .font(font)
        .width(Length::Fill);
    container(cosmic::iced::widget::stack![
        container(resting)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::Center),
        container(input)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::Center),
    ])
    .width(Length::Fixed(FIELD_WIDTH))
    .height(Length::Fixed(ROW_HEIGHT))
    .into()
}

/// The field style: invisible at rest (the resting text shows instead),
/// the text background with primary text and a tertiary placeholder while
/// focused.
fn field_style() -> cosmic::theme::TextInput {
    fn appearance(theme: &cosmic::Theme, focused: bool) -> cosmic::widget::text_input::Appearance {
        let shown = |color: cosmic::iced::Color| {
            if focused {
                color
            } else {
                cosmic::iced::Color::TRANSPARENT
            }
        };
        cosmic::widget::text_input::Appearance {
            background: Background::Color(shown(surface::text_background(theme))),
            border_radius: Corner::Xs.radius(theme),
            border_offset: None,
            border_width: 0.0,
            border_color: cosmic::iced::Color::TRANSPARENT,
            label_color: Tone::Primary.color(theme, Backdrop::Plain),
            placeholder_color: shown(Tone::Tertiary.color(theme, Backdrop::Plain)),
            selected_text_color: Tone::OnAccent.solid(theme),
            icon_color: None,
            text_color: Some(shown(Tone::Primary.color(theme, Backdrop::Plain))),
            selected_fill: Tone::Accent.solid(theme),
        }
    }
    cosmic::theme::TextInput::Custom {
        active: Box::new(|theme| appearance(theme, false)),
        error: Box::new(|theme| appearance(theme, false)),
        hovered: Box::new(|theme| appearance(theme, false)),
        focused: Box::new(|theme| appearance(theme, true)),
        disabled: Box::new(|theme| appearance(theme, false)),
    }
}

/// A grouped Form's `Toggle`: macOS's small switch, 28 x 16 pt at the
/// trailing edge (shots/upstream/settings-agent.png, the Restricted Bash
/// row), a white knob over a gray track that fills with the accent when
/// on.
///
/// libcosmic's toggler lays its switch out at a fixed 48 x 24
/// (src/widget/toggler.rs:223 at the pinned rev; `size` does not reach
/// it), so the switch is drawn from containers.
pub fn switch<'a, M: Clone + 'static>(
    on: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    let knob = container(Space::new().width(SWITCH_KNOB).height(SWITCH_KNOB)).class(
        cosmic::theme::Container::custom(|theme| cosmic::iced::widget::container::Style {
            background: Some(Background::Color(Tone::OnFill.solid(theme))),
            border: cosmic::iced::Border {
                radius: Corner::Capsule.radius(theme),
                ..Default::default()
            },
            shadow: surface::Shadow::Control.iced(),
            ..Default::default()
        }),
    );
    let track = container(knob)
        .width(SWITCH_WIDTH)
        .height(SWITCH_HEIGHT)
        .padding(SWITCH_INSET)
        .align_x(if on { Alignment::End } else { Alignment::Start })
        .align_y(Alignment::Center)
        .class(cosmic::theme::Container::custom(move |theme| {
            cosmic::iced::widget::container::Style {
                background: Some(Background::Color(if on {
                    Tone::Accent.solid(theme)
                } else {
                    super::tone::wash(
                        theme,
                        Tone::Quaternary.alpha(theme, Backdrop::Plain) * SWITCH_OFF,
                    )
                })),
                border: cosmic::iced::Border {
                    radius: Corner::Capsule.radius(theme),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));
    cosmic::widget::button::custom(track)
        .padding(0)
        .class(super::button::plain())
        .on_press(on_toggle(!on))
        .into()
}

/// The switch's track, in upstream points scaled by the toolkit's body
/// ratio as the segmented control's are.
const SWITCH_WIDTH: f32 = 28.0 * crate::typography::COSMIC_BODY / crate::typography::MACOS_BODY;
const SWITCH_HEIGHT: f32 = 16.0 * crate::typography::COSMIC_BODY / crate::typography::MACOS_BODY;
/// The knob's inset from the track.
const SWITCH_INSET: f32 = 1.5;
const SWITCH_KNOB: f32 = SWITCH_HEIGHT - 2.0 * SWITCH_INSET;
/// The off track against [`Tone::Quaternary`] (upstream's reads 32 levels
/// under the form box).
const SWITCH_OFF: f32 = 1.4;
