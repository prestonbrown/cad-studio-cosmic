//! The small macOS slider (`.controlSize(.small)`) the overlay cards share:
//! the Parts card's explode and section sliders and the Parameters rows.

use std::rc::Rc;

use cosmic::Element;
use cosmic::iced::widget::slider;
use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::widget::{Row, Space, container};

use super::tone::{Backdrop, Tone, mix};

/// The small macOS slider: a 4 px rounded rail (accent fill, faint `on`
/// remainder) and a 16 px round knob — white with a hairline ring in
/// light mode, a mid neutral in dark mode (upstream's dark knob reads 146
/// on a 38 card, project-model-dark.png).
pub fn small_slider_class() -> cosmic::theme::iced::Slider {
    let style = Rc::new(|theme: &cosmic::Theme| {
        let on = Tone::Primary.solid(theme);
        let knob = if theme.cosmic().is_dark {
            mix(on, Color::BLACK, DARK_KNOB_SHADE)
        } else {
            Color::WHITE
        };
        slider::Style {
            rail: slider::Rail {
                backgrounds: (
                    Background::Color(Tone::Accent.solid(theme)),
                    Background::Color(Tone::Quaternary.color(theme, Backdrop::Material)),
                ),
                width: 4.0,
                // square ends: the rounded caps come from `with_rail_ends`
                border: Border::default(),
            },
            handle: slider::Handle {
                shape: slider::HandleShape::Circle { radius: 8.0 },
                background: Background::Color(knob),
                border_width: 1.0,
                border_color: Color {
                    a: KNOB_RING_ALPHA,
                    ..on
                },
            },
            breakpoint: slider::Breakpoint { color: on },
        }
    });
    cosmic::theme::iced::Slider::Custom {
        active: style.clone(),
        hovered: style.clone(),
        dragging: style,
    }
}

/// The dark knob: the `on` color pressed this far toward black.
const DARK_KNOB_SHADE: f32 = 0.37;

/// The small slider's knob diameter (and row height).
pub const KNOB: f32 = 16.0;

/// The light knob's hairline ring: a separator-class stroke, a touch
/// stronger than [`Tone::Separator`] so the white knob holds its edge on
/// the light rail.
const KNOB_RING_ALPHA: f32 = 0.18;

/// A small slider over `range`: the [`small_slider_class`] look, a
/// [`KNOB`]-sized knob, and the rail reaching the full width. iced insets
/// its rail by half the knob at both ends (the knob's travel), where
/// macOS runs the rail edge to edge; the two half-knob end caps are
/// drawn underneath — the leading cap in the fill color (the value never
/// sits left of the range start), the trailing one in the track color.
pub fn small_slider<'a, T, M>(
    range: std::ops::RangeInclusive<T>,
    value: T,
    on_change: impl Fn(T) -> M + 'a,
    width: Length,
) -> cosmic::iced::widget::Slider<'a, T, M, cosmic::Theme>
where
    T: Copy + From<u8> + PartialOrd,
    M: Clone + 'a,
{
    cosmic::iced::widget::Slider::new(range, value, on_change)
        .width(width)
        .height(KNOB)
        .handle_width(KNOB)
        .handle_height(KNOB)
        .class(small_slider_class())
}

/// Wrap a [`small_slider`] (or its tooltip) with the rail end caps.
pub fn with_rail_ends<'a, M: 'a>(
    track: impl Into<Element<'a, M>>,
    width: Length,
) -> Element<'a, M> {
    let cap = |leading: bool| {
        container(Space::new().width(KNOB / 2.0).height(4.0)).class(
            cosmic::theme::Container::custom(move |theme| cosmic::iced::widget::container::Style {
                background: Some(Background::Color(if leading {
                    Tone::Accent.solid(theme)
                } else {
                    Tone::Quaternary.color(theme, Backdrop::Material)
                })),
                border: Border {
                    radius: if leading {
                        [2.0, 0.0, 0.0, 2.0].into()
                    } else {
                        [0.0, 2.0, 2.0, 0.0].into()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
        )
    };
    let ends = Row::new()
        .height(KNOB)
        .align_y(Alignment::Center)
        .push(cap(true))
        .push(Space::new().width(Length::Fill))
        .push(cap(false));
    cosmic::iced::widget::stack![ends, track.into()]
        .width(width)
        .into()
}
