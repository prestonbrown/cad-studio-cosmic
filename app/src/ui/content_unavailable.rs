//! SwiftUI's `ContentUnavailableView`: a tertiary glyph, the bold
//! secondary title, the secondary description, centered in the space it
//! is given. The panes that show one (the 3D model pane, the schematic
//! pane) supply the symbol, the strings, and their own background.

use cosmic::iced::Length;
use cosmic::widget::{Column, Container, Space, container, text};

use super::icon::{Symbol, icon};
use super::tone::{Backdrop, Tone};

/// The glyph's height: SF's `ContentUnavailableView` label image size.
pub const GLYPH_HEIGHT: f32 = 40.0;
/// Glyph to title.
pub const GLYPH_GAP: f32 = 14.0;
/// Title to description.
pub const TITLE_GAP: f32 = 6.0;

/// The placeholder, filling its parent with the column centered.
pub fn content_unavailable<'a, M: 'a>(
    symbol: Symbol,
    title: String,
    description: String,
    backdrop: Backdrop,
) -> Container<'a, M, cosmic::Theme, cosmic::Renderer> {
    container(
        Column::new()
            .push(icon(symbol, GLYPH_HEIGHT, Tone::Tertiary, backdrop))
            .push(Space::new().height(Length::Fixed(GLYPH_GAP)))
            .push(
                text(title)
                    .size(crate::typography::TITLE)
                    .font(crate::typography::font(cosmic::iced::font::Weight::Bold))
                    .class(Tone::Secondary.text(backdrop)),
            )
            .push(Space::new().height(Length::Fixed(TITLE_GAP)))
            .push(
                text(description)
                    .size(crate::typography::BODY)
                    .align_x(cosmic::iced::alignment::Horizontal::Center)
                    .class(Tone::Secondary.text(backdrop)),
            )
            .align_x(cosmic::iced::alignment::Alignment::Center),
    )
    .padding(20)
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(cosmic::iced::alignment::Horizontal::Center)
    .align_y(cosmic::iced::alignment::Vertical::Center)
}
