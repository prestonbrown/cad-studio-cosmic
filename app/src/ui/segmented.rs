//! The macOS segmented control (`.pickerStyle(.segmented)`) at the small
//! or the regular control size, drawn from containers: libcosmic's
//! segmented button draws its own 32 px pill-and-accent look and borrows a
//! model.
//!
//! Upstream measures 18 pt tall with 11 pt labels at the small size and
//! 22 pt with 13 pt (body) labels at the regular one; the port keeps those
//! proportions to its label sizes, so the control grows with the toolkit's
//! text scale. Every segment is as wide as the widest label needs, as
//! AppKit sizes them.

use cosmic::Element;
use cosmic::iced::widget::container::Style;
use cosmic::iced::{Alignment, Background, Border, Length};
use cosmic::widget::{Row, Space, button, container, text};

use super::surface::{Corner, Shadow as SurfaceShadow, material, text_background};
use super::tone::{Backdrop, Tone, mix, wash};
use crate::typography;

/// Upstream metrics in points, scaled by the toolkit's body ratio.
const fn pt(points: f32) -> f32 {
    points * typography::COSMIC_BODY / typography::MACOS_BODY
}

/// The selected face's inset from the track.
const INSET: f32 = 1.0;

/// AppKit's control size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// `.controlSize(.small)`: the viewer bar and the overlay cards.
    Small,
    /// The default size: a grouped Form's picker row.
    Regular,
}

impl Size {
    /// The label: macOS's 11 pt small system font, or the body font.
    pub const fn label(self) -> f32 {
        match self {
            Self::Small => typography::SMALL_CONTROL,
            Self::Regular => typography::BODY,
        }
    }

    /// The control's height.
    pub const fn height(self) -> f32 {
        match self {
            Self::Small => pt(18.0),
            Self::Regular => pt(22.0),
        }
    }

    /// The hairline between two unselected segments.
    const fn divider_height(self) -> f32 {
        self.height() / 2.0
    }

    /// A label's average advance (5 pt at 11 pt labels).
    const fn char_width(self) -> f32 {
        pt(5.0) * self.label() / typography::SMALL_CONTROL
    }

    /// A segment's side padding (AppKit pads 6 pt each side of the widest
    /// label at the small size).
    const fn side_padding(self) -> f32 {
        pt(6.0) * self.label() / typography::SMALL_CONTROL
    }
}

/// How the selected segment shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// In a plain bar: a raised face on the text background, hairline and
    /// soft shadow (the viewer bar's preset picker).
    Raised,
    /// On a material: a filled gray pill under a semibold white label (the
    /// Parts card's layout picker, the section axis picker).
    Filled,
}

impl Look {
    fn backdrop(self) -> Backdrop {
        match self {
            Self::Raised => Backdrop::Plain,
            Self::Filled => Backdrop::Material,
        }
    }
}

/// One segment's width when the control sizes itself: the widest label's
/// estimated advance plus the side padding (iced has no text measurement
/// at view time).
pub fn segment_width<S: AsRef<str>>(labels: &[S], size: Size) -> f32 {
    let widest = labels
        .iter()
        .map(|label| label.as_ref().chars().count())
        .max()
        .unwrap_or(0);
    widest as f32 * size.char_width() + 2.0 * size.side_padding()
}

/// The track under the segments.
fn track(theme: &cosmic::Theme, look: Look) -> cosmic::iced::Color {
    match look {
        Look::Raised => wash(theme, 0.05),
        // an inset on the material, pulled toward the text background
        // (lighter than the card in light mode, recessed in dark mode)
        Look::Filled => mix(material(theme), text_background(theme), 0.3),
    }
}

/// The selected segment's face.
fn face(theme: &cosmic::Theme, look: Look) -> Style {
    let radius = Corner::Xs.radius(theme);
    match look {
        Look::Raised => Style {
            background: Some(Background::Color(if theme.cosmic().is_dark {
                wash(theme, 0.28)
            } else {
                text_background(theme)
            })),
            border: Border {
                color: wash(theme, 0.12),
                width: 0.5,
                radius,
            },
            shadow: SurfaceShadow::Control.iced(),
            ..Default::default()
        },
        // the track pressed toward `on`: 178 on 242 in light mode
        Look::Filled => Style {
            background: Some(Background::Color(mix(
                track(theme, look),
                Tone::Primary.solid(theme),
                0.26,
            ))),
            border: Border {
                radius,
                ..Default::default()
            },
            ..Default::default()
        },
    }
}

/// A segmented control over `items` (label, the message its press sends
/// or `None` for a disabled segment), `selected` drawn per `look`, at
/// `size`.
/// `width` is `Length::Fill` for equal segments filling the space, or
/// `Length::Shrink` for segments sized by [`segment_width`].
pub fn segmented<M: Clone + 'static>(
    items: Vec<(String, Option<M>)>,
    selected: Option<usize>,
    look: Look,
    size: Size,
    width: Length,
) -> Element<'static, M> {
    let fixed = (width == Length::Shrink).then(|| {
        let labels: Vec<&str> = items.iter().map(|(label, _)| label.as_str()).collect();
        segment_width(&labels, size)
    });
    let backdrop = look.backdrop();
    let mut row = Row::new()
        .height(Length::Fixed(size.height() - 2.0 * INSET))
        .align_y(Alignment::Center);
    for (index, (label, message)) in items.into_iter().enumerate() {
        if index > 0 {
            // a hairline divides two unselected neighbours; next to the
            // selected face the face itself is the boundary
            let shown = selected != Some(index - 1) && selected != Some(index);
            row = row.push(
                container(Space::new().width(1.0).height(size.divider_height())).class(
                    cosmic::theme::Container::custom(move |theme| Style {
                        background: shown
                            .then(|| Tone::Separator.color(theme, Backdrop::Plain).into()),
                        ..Default::default()
                    }),
                ),
            );
        }
        let is_selected = selected == Some(index);
        let tone = match (message.is_some(), is_selected, look) {
            (false, _, _) => Tone::Tertiary,
            (true, true, Look::Filled) => Tone::OnFill,
            (true, _, _) => Tone::Primary,
        };
        let mut label = text(label).size(size.label()).class(tone.text(backdrop));
        if is_selected && look == Look::Filled {
            label = label.font(typography::font(cosmic::iced::font::Weight::Semibold));
        }
        let face = container(label)
            .width(fixed.map_or(Length::Fill, Length::Fixed))
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .class(cosmic::theme::Container::custom(move |theme| {
                if is_selected {
                    face(theme, look)
                } else {
                    Style::default()
                }
            }));
        row = row.push(
            button::custom(face)
                .padding(0)
                .width(fixed.map_or(Length::FillPortion(1), Length::Fixed))
                .class(super::button::plain())
                .on_press_maybe(message),
        );
    }
    container(row)
        .padding(INSET)
        .width(width)
        .class(cosmic::theme::Container::custom(move |theme| Style {
            background: Some(Background::Color(track(theme, look))),
            // concentric with the face: one inset wider
            border: Border {
                radius: theme
                    .cosmic()
                    .corner_radii
                    .radius_xs
                    .map(|r| r + INSET)
                    .into(),
                ..Default::default()
            },
            ..Default::default()
        }))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_size_to_the_widest_label() {
        let small = Size::Small;
        let narrow = segment_width(&["Iso", "Top"], small);
        let wide = segment_width(&["Iso", "Fronte"], small);
        assert!(wide > narrow);
        assert_eq!(
            segment_width(&["Iso", "Fronte"], small),
            segment_width(&["Fronte"], small)
        );
        // upstream's "Front" segment: 37.5 pt at 11 pt labels
        assert!((segment_width(&["Front"], small) - pt(37.0)).abs() < 1.0);
        assert!(segment_width(&["Front"], Size::Regular) > segment_width(&["Front"], small));
    }

    #[test]
    fn the_control_scales_with_its_label() {
        let scale = typography::COSMIC_BODY / typography::MACOS_BODY;
        assert!((Size::Small.height() - 18.0 * scale).abs() < 1e-4);
        assert!((Size::Small.divider_height() - 9.0 * scale).abs() < 1e-4);
        assert!((Size::Regular.height() - 22.0 * scale).abs() < 1e-4);
        assert_eq!(Size::Regular.label(), typography::BODY);
    }

    #[test]
    fn every_shape_builds() {
        #[derive(Clone)]
        struct Pick;
        for look in [Look::Raised, Look::Filled] {
            for width in [Length::Fill, Length::Shrink] {
                let items = vec![("A".to_string(), Some(Pick)), ("B".to_string(), None)];
                for size in [Size::Small, Size::Regular] {
                    let _ = segmented(items.clone(), Some(0), look, size, width);
                    let _ = segmented::<Pick>(Vec::new(), None, look, size, width);
                }
            }
        }
    }
}
