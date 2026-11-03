//! macOS's grouped Form (`.formStyle(.grouped)`): each `Section` is a
//! rounded box of rows, label at the leading edge and control at the
//! trailing one, the rows split by inset hairlines, and the section footer
//! below the box in secondary text against the trailing edge. The settings
//! window's four tabs and the first-run sheet's print preferences draw
//! with it.
//!
//! Geometry measured off upstream (shots/upstream/settings-*.png and
//! setup-sheet.png at 1x): rows 37-38 px, the label, the control and the
//! hairlines inset 10 px from the box edge, 10 px between boxes, the box
//! 20 px in from the window or sheet edge, a footer's text flush with the
//! box's trailing edge. The fill and hairline are
//! washes of the theme's `on` color, so the box reads one step off its
//! surface in either appearance.

use cosmic::Element;
use cosmic::iced::widget::container::Style;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Background, Border, Length};
use cosmic::widget::{Column, Row, Space, container};

use super::surface::Corner;
use super::tone::{Backdrop, Tone, wash};

/// A form row's height.
pub const ROW_HEIGHT: f32 = 38.0;
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
    container(column)
        .width(Length::Fill)
        .class(cosmic::theme::Container::custom(|theme| Style {
            background: Some(Background::Color(wash(
                theme,
                Tone::Quaternary.alpha(theme, Backdrop::Plain) * FILL,
            ))),
            border: Border {
                color: wash(
                    theme,
                    Tone::Separator.alpha(theme, Backdrop::Plain) * HAIRLINE,
                ),
                width: 1.0,
                radius: Corner::S.radius(theme),
            },
            ..Default::default()
        }))
        .into()
}

/// The hairline between two rows, inset from both box edges.
fn hairline<'a, M: 'a>() -> Element<'a, M> {
    container(
        container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .class(cosmic::theme::Container::custom(|theme| Style {
                background: Some(Background::Color(wash(
                    theme,
                    Tone::Separator.alpha(theme, Backdrop::Plain) * HAIRLINE,
                ))),
                ..Default::default()
            })),
    )
    .padding([0.0, INSET])
    .into()
}

/// A `LabeledContent` / `Picker` / `Toggle` row: `label` leading, `control`
/// against the trailing edge, vertically centered.
pub fn row<'a, M: 'a>(label: String, control: impl Into<Element<'a, M>>) -> Element<'a, M> {
    Row::new()
        .push(label_text(label))
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

/// A grouped Form's `TextField`: no box of its own in any state (the row
/// is its frame), the placeholder tertiary, `mono` for the path and model
/// fields (`.fontDesign(.monospaced)`).
pub fn field<'a, M: Clone + 'static>(
    placeholder: String,
    value: &'a str,
    mono: bool,
    on_input: impl Fn(String) -> M + 'a,
) -> Element<'a, M> {
    let input = cosmic::widget::text_input::inline_input(placeholder, value)
        .on_input(on_input)
        .style(bare_field())
        .width(Length::Fixed(FIELD_WIDTH));
    if mono {
        input.font(cosmic::font::mono()).into()
    } else {
        input.into()
    }
}

/// The field style: transparent, unbordered, in every state.
fn bare_field() -> cosmic::theme::TextInput {
    fn appearance(theme: &cosmic::Theme) -> cosmic::widget::text_input::Appearance {
        cosmic::widget::text_input::Appearance {
            background: Background::Color(cosmic::iced::Color::TRANSPARENT),
            border_radius: Corner::None.radius(theme),
            border_offset: None,
            border_width: 0.0,
            border_color: cosmic::iced::Color::TRANSPARENT,
            label_color: Tone::Primary.color(theme, Backdrop::Plain),
            placeholder_color: Tone::Tertiary.color(theme, Backdrop::Plain),
            selected_text_color: Tone::OnAccent.solid(theme),
            icon_color: None,
            text_color: Some(Tone::Primary.color(theme, Backdrop::Plain)),
            selected_fill: Tone::Accent.solid(theme),
        }
    }
    cosmic::theme::TextInput::Custom {
        active: Box::new(appearance),
        error: Box::new(appearance),
        hovered: Box::new(appearance),
        focused: Box::new(appearance),
        disabled: Box::new(appearance),
    }
}
