//! Borderless buttons: an icon or a label with no chrome of its own
//! (SwiftUI's `.borderless` / `.plain` / `.link` styles), and the toolbar
//! look with a hover wash. A button is enabled exactly when it has a
//! message: `on_press: None` both disables it and draws its content at
//! the disabled tone, so the two can never disagree.

use cosmic::Element;
use cosmic::iced::Background;
use cosmic::widget::{button, tooltip};

use super::icon::{self, Symbol};
use super::surface::Corner;
use super::tone::{Backdrop, HOVER, PRESS, Tone, wash};
use crate::typography;

/// The tone a button's content draws in: `tone` while it has a message,
/// tertiary while disabled.
pub fn content_tone<M>(tone: Tone, on_press: &Option<M>) -> Tone {
    if on_press.is_some() {
        tone
    } else {
        Tone::Tertiary
    }
}

/// A button class that draws nothing in any state: the content carries
/// every pixel.
pub fn plain() -> cosmic::theme::Button {
    fn style(_theme: &cosmic::Theme) -> button::Style {
        button::Style::new()
    }
    cosmic::theme::Button::Custom {
        active: Box::new(|_, theme| style(theme)),
        disabled: Box::new(style),
        hovered: Box::new(|_, theme| style(theme)),
        pressed: Box::new(|_, theme| style(theme)),
    }
}

/// The hairline around a bordered toolbar pill.
const BORDER: f32 = 0.05;

/// The toolbar button: no fill at rest, a rounded wash on hover and press
/// (macOS toolbar items), or, `bordered`, the hairline pill a toolbar
/// `Menu` label sits in.
pub fn hover_wash(bordered: bool, corner: Corner) -> cosmic::theme::Button {
    let style = move |theme: &cosmic::Theme, fill: f32| {
        let mut style = button::Style::new();
        style.background = (fill > 0.0).then(|| Background::Color(wash(theme, fill)));
        style.border_radius = corner.radius(theme);
        if bordered {
            style.border_width = 1.0;
            style.border_color = wash(theme, BORDER);
        }
        style
    };
    cosmic::theme::Button::Custom {
        active: Box::new(move |_, theme| style(theme, 0.0)),
        disabled: Box::new(move |theme| style(theme, 0.0)),
        hovered: Box::new(move |_, theme| style(theme, HOVER)),
        pressed: Box::new(move |_, theme| style(theme, PRESS)),
    }
}

/// `content` as a borderless button: no padding, no fill.
pub fn borderless<'a, M: Clone + 'a>(
    content: impl Into<Element<'a, M>>,
    on_press: Option<M>,
) -> cosmic::widget::Button<'a, M> {
    button::custom(content)
        .padding(0)
        .class(plain())
        .on_press_maybe(on_press)
}

/// `element` with `help` as its tooltip, in macOS's 11 pt tooltip text.
pub fn with_help<'a, M: 'a>(
    element: impl Into<Element<'a, M>>,
    help: String,
    position: tooltip::Position,
) -> Element<'a, M> {
    tooltip(
        element,
        cosmic::widget::text(help).size(typography::SMALL_CONTROL),
        position,
    )
    .into()
}

/// An icon button (`Button { Image(systemName:) }.buttonStyle(.borderless)`):
/// the bare glyph `height` px tall, the help string as tooltip and
/// accessible name.
pub fn icon<M: Clone + 'static>(
    symbol: Symbol,
    height: f32,
    tone: Tone,
    backdrop: Backdrop,
    help: String,
    on_press: Option<M>,
) -> Element<'static, M> {
    let tone = content_tone(tone, &on_press);
    let body = borderless(icon::icon(symbol, height, tone, backdrop), on_press).name(help.clone());
    with_help(body, help, tooltip::Position::Bottom)
}

/// A text button (`Button("…")` under `.borderless`, or `.link` with
/// [`Tone::Accent`]): the label at `size` in `tone`.
pub fn text<M: Clone + 'static>(
    label: String,
    size: f32,
    tone: Tone,
    backdrop: Backdrop,
    on_press: Option<M>,
) -> Element<'static, M> {
    let tone = content_tone(tone, &on_press);
    borderless(
        cosmic::widget::text(label)
            .size(size)
            .class(tone.text(backdrop)),
        on_press,
    )
    .into()
}

/// The push button's padding: 20 px tall around body text, like
/// upstream's form buttons (shots/upstream/settings-general.png, 1x).
const PUSH_PADDING: [f32; 2] = [1.0, 9.0];
/// The push button's face in a dark theme: the `on` color at this alpha.
/// macOS dark controls are a translucent white lift, so the face carries
/// the surface under it (95 gray on a settings box, the orange wash on a
/// chat error row in shots/upstream/*-dark.png).
const PUSH_DARK_LIFT: f32 = 0.25;

/// The push button face under `wash` (0 at rest, `HOVER`, `PRESS`): the
/// lightest neutral in a light theme (macOS's white control) washed toward
/// `on`; a translucent `on` lift in a dark one, more opaque as it is washed.
fn push_face(theme: &cosmic::Theme, wash: f32) -> cosmic::iced::Color {
    let cosmic = theme.cosmic();
    let on: cosmic::iced::Color = cosmic.background(false).on.into();
    if cosmic.is_dark {
        cosmic::iced::Color {
            a: PUSH_DARK_LIFT + wash,
            ..on
        }
    } else {
        super::tone::mix(cosmic.palette.neutral_0.into(), on, wash)
    }
}

/// A push button's interaction state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushState {
    Rest,
    Hover,
    Press,
    Disabled,
}

/// A push button's look in `role` and `state`: a plain one is the face, a
/// separator hairline and the `on` wash on hover and press; an enabled
/// default one is the theme's accent button face, a disabled one the
/// plain face at rest.
fn push_style(theme: &cosmic::Theme, role: PushRole, state: PushState) -> button::Style {
    let mut style = button::Style::new();
    style.border_radius = Corner::S.radius(theme);
    let accent = &theme.cosmic().accent_button;
    let accent_face = match (role, state) {
        (PushRole::Default, PushState::Rest) => Some(accent.base),
        (PushRole::Default, PushState::Hover) => Some(accent.hover),
        (PushRole::Default, PushState::Press) => Some(accent.pressed),
        _ => None,
    };
    if let Some(face) = accent_face {
        style.background = Some(Background::Color(face.into()));
        return style;
    }
    let wash = match state {
        PushState::Hover => HOVER,
        PushState::Press => PRESS,
        PushState::Rest | PushState::Disabled => 0.0,
    };
    style.background = Some(Background::Color(push_face(theme, wash)));
    style.border_width = 1.0;
    style.border_color = Tone::Separator.color(theme, Backdrop::Plain);
    style
}

/// The push button class for `role`.
fn push_class(role: PushRole) -> cosmic::theme::Button {
    cosmic::theme::Button::Custom {
        active: Box::new(move |_, theme| push_style(theme, role, PushState::Rest)),
        disabled: Box::new(move |theme| push_style(theme, role, PushState::Disabled)),
        hovered: Box::new(move |_, theme| push_style(theme, role, PushState::Hover)),
        pressed: Box::new(move |_, theme| push_style(theme, role, PushState::Press)),
    }
}

/// Which push button: a plain one, or the sheet's default action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushRole {
    /// `Button("…")`: the push face, primary label.
    Plain,
    /// `.keyboardShortcut(.defaultAction)` / `.borderedProminent`: the
    /// accent face, the label on the accent.
    Default,
}

/// The tone a push button's label draws in for `role`, enabled or not.
pub fn push_label_tone<M>(role: PushRole, on_press: &Option<M>) -> Tone {
    match role {
        PushRole::Plain => content_tone(Tone::Primary, on_press),
        PushRole::Default if on_press.is_some() => Tone::OnAccent,
        PushRole::Default => Tone::Tertiary,
    }
}

/// A form's push button (`Button("…")` in a grouped Form, macOS's
/// bordered push style): `label` in body text on the push face; disabled
/// without a message.
pub fn push<'a, M: Clone + 'a>(label: String, on_press: Option<M>) -> Element<'a, M> {
    push_as(PushRole::Plain, label, on_press)
}

/// A push button in `role`: the sheets' Cancel / Continue rows put the
/// [`PushRole::Default`] one last.
pub fn push_as<'a, M: Clone + 'a>(
    role: PushRole,
    label: String,
    on_press: Option<M>,
) -> Element<'a, M> {
    push_sized(role, PushSize::Regular, label, on_press)
}

/// A push button's control size: regular (body text) or
/// `.controlSize(.small)` (the small control label in tighter padding,
/// 18 px tall: the chat rows' action buttons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushSize {
    Regular,
    Small,
}

impl PushSize {
    /// The label size and the padding around it.
    pub const fn metrics(self) -> (f32, [f32; 2]) {
        match self {
            Self::Regular => (typography::BODY, PUSH_PADDING),
            Self::Small => (typography::SMALL_CONTROL, [1.0, 7.0]),
        }
    }
}

/// A push button in `role` at `size`.
pub fn push_sized<'a, M: Clone + 'a>(
    role: PushRole,
    size: PushSize,
    label: String,
    on_press: Option<M>,
) -> Element<'a, M> {
    let tone = push_label_tone(role, &on_press);
    let (text_size, padding) = size.metrics();
    button::custom(
        cosmic::widget::text(label)
            .size(text_size)
            .class(tone.text(Backdrop::Plain)),
    )
    .padding(padding)
    .class(push_class(role))
    .on_press_maybe(on_press)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.controlSize(.small)` sets a smaller label in less padding than
    /// the regular push button.
    #[test]
    fn small_push_is_tighter_than_regular() {
        let (regular_text, regular_pad) = PushSize::Regular.metrics();
        let (small_text, small_pad) = PushSize::Small.metrics();
        assert!(small_text < regular_text);
        assert!(small_pad[1] < regular_pad[1]);
        assert_eq!(regular_pad, PUSH_PADDING);
    }

    #[test]
    fn a_default_push_labels_on_the_accent_until_disabled() {
        assert_eq!(
            push_label_tone(PushRole::Default, &Some(())),
            Tone::OnAccent
        );
        assert_eq!(
            push_label_tone(PushRole::Default, &None::<()>),
            Tone::Tertiary
        );
        assert_eq!(push_label_tone(PushRole::Plain, &Some(())), Tone::Primary);
        assert_eq!(
            push_label_tone(PushRole::Plain, &None::<()>),
            Tone::Tertiary
        );
    }

    #[test]
    fn a_dark_push_face_is_a_translucent_lift_that_hover_raises() {
        let dark = cosmic::Theme::dark();
        let rest = push_face(&dark, 0.0);
        let hover = push_face(&dark, HOVER);
        assert!(rest.a < 1.0, "a dark face must let the surface show through");
        assert!(hover.a > rest.a);
        let light = cosmic::Theme::light();
        assert_eq!(push_face(&light, 0.0).a, 1.0);
    }

    #[test]
    fn a_button_without_a_message_draws_disabled() {
        assert_eq!(content_tone(Tone::Accent, &Some(())), Tone::Accent);
        assert_eq!(content_tone(Tone::Accent, &None::<()>), Tone::Tertiary);
        assert_eq!(content_tone(Tone::Secondary, &None::<()>), Tone::Tertiary);
    }
}
