//! Upstream `CodeBlock` (MessageView.swift:114-144): 11 pt mono text with
//! padding 8 (+24 trailing for the button), horizontally scrollable, on a
//! quaternary fill in a small-radius rectangle, with a borderless copy
//! button overlaid top-trailing. The chat's code and tool rows and the
//! agent-missing sheet's install commands draw it.

use cosmic::Element;
use cosmic::iced::Length;
use cosmic::iced::alignment::Horizontal;
use cosmic::iced::widget::text::{LineHeight, Wrapping};
use cosmic::widget::{button, container, tooltip};

use super::icon::{self, Symbol};
use super::surface::{self, Corner};
use super::tone::{Backdrop, Tone};
use crate::fl;
use crate::typography;

/// `.system(size: 11)` mono on its 14 pt line.
const SIZE: f32 = typography::SMALL_CONTROL;
const LINE_HEIGHT: f32 = typography::scaled(14.0);
/// The copy glyph's cell.
const COPY_GLYPH: f32 = 12.0;

/// `code` as a code block, cut to `line_limit` lines when given (SwiftUI
/// `lineLimit`); the copy button sends `on_copy`.
pub fn code_block<'a, M: Clone + 'a>(
    code: &str,
    line_limit: Option<usize>,
    on_copy: M,
) -> Element<'a, M> {
    let shown = match line_limit {
        Some(limit) => clip_lines(code, limit),
        None => code.to_string(),
    };
    let text = cosmic::widget::text(shown)
        .size(SIZE)
        .line_height(LineHeight::Absolute(LINE_HEIGHT.into()))
        .font(cosmic::font::mono())
        .wrapping(Wrapping::None)
        .class(Tone::Primary.text(Backdrop::Plain));
    let scroller = cosmic::widget::scrollable::horizontal(container(text).padding([
        8.0,
        8.0 + 24.0,
        8.0,
        8.0,
    ]))
    .width(Length::Fill);
    let copy = super::button::with_help(
        button::custom(icon::in_cell(
            Symbol::DocOnDoc,
            COPY_GLYPH,
            COPY_GLYPH,
            Tone::Secondary,
            Backdrop::Plain,
        ))
        .padding(3)
        .class(cosmic::theme::Button::Icon)
        .on_press(on_copy)
        .name(fl!("copia")),
        fl!("copia"),
        tooltip::Position::FollowCursor,
    );
    cosmic::iced::widget::stack![
        container(scroller)
            .width(Length::Fill)
            .class(surface::fill(Tone::Quaternary, 0.5, Corner::S, false)),
        container(copy)
            .width(Length::Fill)
            .padding(3)
            .align_x(Horizontal::Right),
    ]
    .into()
}

/// SwiftUI `lineLimit(n)` over a code string: the first `n` lines, the
/// last of them ending in "…" when more were cut.
pub fn clip_lines(code: &str, limit: usize) -> String {
    let lines: Vec<&str> = code.lines().collect();
    if lines.len() <= limit {
        return code.to_string();
    }
    let mut out = lines[..limit].join("\n");
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// lineLimit(12) parity for the tool input card: twelve lines pass
    /// through untouched; past that the first twelve show and the last
    /// visible one ends in an ellipsis.
    #[test]
    fn clip_lines_caps_at_limit() {
        let twelve: String = (1..=12)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(clip_lines(&twelve, 12), twelve);
        let thirteen = format!("{twelve}\n13");
        let clipped = clip_lines(&thirteen, 12);
        assert_eq!(clipped.lines().count(), 12);
        assert!(clipped.ends_with("12…"), "{clipped}");
        assert_eq!(clip_lines("one", 12), "one");
        assert_eq!(clip_lines("", 12), "");
    }
}
