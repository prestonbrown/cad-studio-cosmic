//! Surfaces: the material, cards, capsules, washes, the text background
//! and dividers, every color from the theme and every radius a COSMIC
//! roundness class.

use cosmic::Element;
use cosmic::iced::widget::container::Style;
use cosmic::iced::{Background, Border, Color, Length, Shadow as IcedShadow, Vector};
use cosmic::widget::{Space, container};

use super::tone::{Tone, mix, wash};

/// A corner radius by CLASS, resolved from the user's COSMIC roundness
/// setting: upstream's literal point values only pick the class.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Corner {
    /// Square.
    None,
    /// Tiny controls: a segment, an issue count, a menu row's highlight.
    Xs,
    /// Small controls and cards: toolbar buttons, the activity capsule,
    /// bubbles and tool rows, the overlay cards (upstream 6-12 pt).
    S,
    /// Popovers and menus.
    M,
    /// A large rounded pill (the collapsed Parameters capsule).
    Xl,
    /// SwiftUI's `Capsule()`: half the height, a shape rather than a
    /// roundness token.
    Capsule,
}

impl Corner {
    pub fn radius(self, theme: &cosmic::Theme) -> cosmic::iced::border::Radius {
        let radii = &theme.cosmic().corner_radii;
        match self {
            Self::None => radii.radius_0.into(),
            Self::Xs => radii.radius_xs.into(),
            Self::S => radii.radius_s.into(),
            Self::M => radii.radius_m.into(),
            Self::Xl => radii.radius_xl.into(),
            Self::Capsule => 999.0.into(),
        }
    }
}

/// `.regularMaterial`'s fill: the window base nudged 5 % toward its `on`
/// color (upstream's card reads 228 over a 239 viewer), lighter in dark
/// mode. Opaque: iced has no backdrop blur, and the 3D scene must not
/// show through unblurred.
pub fn material(theme: &cosmic::Theme) -> Color {
    let background = theme.cosmic().background(false);
    mix(background.base.into(), background.on.into(), 0.05)
}

/// `textBackgroundColor` (white in light mode, #1E1E1E in dark), the
/// field and pane-bar background: the palette's lightest neutral in a
/// light theme; in a dark one the window background, which sits just
/// below the viewport's gray as macOS's does. (The component and
/// container surfaces read 245 / 252 in the light theme, not white.)
pub fn text_background(theme: &cosmic::Theme) -> Color {
    let cosmic = theme.cosmic();
    if cosmic.is_dark {
        cosmic.background(false).base.into()
    } else {
        cosmic.palette.neutral_0.into()
    }
}

/// A drop shadow under a floating surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shadow {
    /// An overlay card: `.shadow(color: .black.opacity(0.15), radius: r,
    /// y: 2)` (SwiftUI's radius is a Gaussian sigma, iced's blur a
    /// diameter).
    Overlay { radius: f32 },
    /// A popover or menu card.
    Popover,
}

impl Shadow {
    fn iced(self) -> IcedShadow {
        match self {
            Self::Overlay { radius } => IcedShadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.15),
                offset: Vector::new(0.0, 2.0),
                blur_radius: radius * 2.0,
            },
            Self::Popover => IcedShadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.18),
                offset: Vector::new(0.0, 6.0),
                blur_radius: 18.0,
            },
        }
    }
}

/// A floating card: the material, a separator hairline, a drop shadow.
pub fn card(corner: Corner, shadow: Shadow) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| Style {
        background: Some(Background::Color(material(theme))),
        border: Border {
            color: Tone::Separator.color(theme, super::tone::Backdrop::Plain),
            width: 1.0,
            radius: corner.radius(theme),
        },
        shadow: shadow.iced(),
        ..Default::default()
    })
}

/// The material alone in a rounded shape: no stroke, no shadow (the
/// measurement badge, the section and motion capsules).
pub fn material_fill(corner: Corner) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| Style {
        background: Some(Background::Color(material(theme))),
        border: Border {
            radius: corner.radius(theme),
            ..Default::default()
        },
        ..Default::default()
    })
}

/// A translucent `on` wash at `alpha` (`.quaternary` fills, bubbles, the
/// activity capsule), optionally stroked with the separator.
pub fn fill(alpha: f32, corner: Corner, stroke: bool) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| Style {
        background: Some(Background::Color(wash(theme, alpha))),
        border: Border {
            color: if stroke {
                wash(theme, Tone::Separator.alpha(super::tone::Backdrop::Plain))
            } else {
                Color::TRANSPARENT
            },
            width: if stroke { 1.0 } else { 0.0 },
            radius: corner.radius(theme),
        },
        ..Default::default()
    })
}

/// The text background as a container class.
pub fn text_background_class() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| Style {
        background: Some(Background::Color(text_background(theme))),
        ..Default::default()
    })
}

/// SwiftUI's `Divider()`: a 1 px separator line across the width.
pub fn divider<'a, M: 'a>() -> Element<'a, M> {
    container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .class(fill(
            Tone::Separator.alpha(super::tone::Backdrop::Plain),
            Corner::None,
            false,
        ))
        .into()
}
