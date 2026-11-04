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
/// The push button's face in a dark theme, against the window's `on`
/// color (macOS dark controls sit a step above their surface).
const PUSH_DARK_FACE: f32 = 0.2;

/// The push button face: the lightest neutral in a light theme (macOS's
/// white control), a lift off the window in a dark one.
fn push_face(theme: &cosmic::Theme) -> cosmic::iced::Color {
    let cosmic = theme.cosmic();
    if cosmic.is_dark {
        let background = cosmic.background(false);
        super::tone::mix(background.base.into(), background.on.into(), PUSH_DARK_FACE)
    } else {
        cosmic.palette.neutral_0.into()
    }
}

/// The push button class: the face, a separator hairline, the `on` wash
/// on hover and press.
fn push_class() -> cosmic::theme::Button {
    let style = |theme: &cosmic::Theme, fill: f32| {
        let mut style = button::Style::new();
        style.background = Some(Background::Color(super::tone::mix(
            push_face(theme),
            theme.cosmic().background(false).on.into(),
            fill,
        )));
        style.border_radius = Corner::S.radius(theme);
        style.border_width = 1.0;
        style.border_color = Tone::Separator.color(theme, Backdrop::Plain);
        style
    };
    cosmic::theme::Button::Custom {
        active: Box::new(move |_, theme| style(theme, 0.0)),
        disabled: Box::new(move |theme| style(theme, 0.0)),
        hovered: Box::new(move |_, theme| style(theme, HOVER)),
        pressed: Box::new(move |_, theme| style(theme, PRESS)),
    }
}

/// A form's push button (`Button("…")` in a grouped Form, macOS's
/// bordered push style): `label` in body text on the push face; disabled
/// without a message.
pub fn push<'a, M: Clone + 'a>(label: String, on_press: Option<M>) -> Element<'a, M> {
    let tone = content_tone(Tone::Primary, &on_press);
    button::custom(
        cosmic::widget::text(label)
            .size(typography::BODY)
            .class(tone.text(Backdrop::Plain)),
    )
    .padding(PUSH_PADDING)
    .class(push_class())
    .on_press_maybe(on_press)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_without_a_message_draws_disabled() {
        assert_eq!(content_tone(Tone::Accent, &Some(())), Tone::Accent);
        assert_eq!(content_tone(Tone::Accent, &None::<()>), Tone::Tertiary);
        assert_eq!(content_tone(Tone::Secondary, &None::<()>), Tone::Tertiary);
    }
}
