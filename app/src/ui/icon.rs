//! The SF Symbols upstream draws, as one registry.
//!
//! Each [`Symbol`] is named after the SF Symbol it stands in for and ships
//! as `app/resources/icons/<sf.name>.svg`: the Cosmic icon theme has none
//! of these shapes, so each SVG is drawn to the symbol. Monochrome SVGs
//! are single-color masks drawn in black; [`icon`] tints them with a
//! [`Tone`] and applies the tone's alpha as the icon's opacity (the svg
//! tint keeps only the glyph's coverage, never the tint's alpha), so a
//! glyph composites correctly over whatever surface it sits on. The
//! `.multicolor` variants carry the system colors SF's multicolor
//! rendering draws (a yellow warning triangle with a white mark) and are
//! drawn untinted through [`multicolor`].
//!
//! An SVG's view box is cropped to its glyph where the symbol's bounding
//! box was measured off upstream, or is a square 16-unit grid; either way
//! [`icon`] sizes by HEIGHT and takes the width from the view box.

use cosmic::iced::Length;
use cosmic::widget::icon::{self as cosmic_icon, Icon};

use super::tone::{Backdrop, Tone};

macro_rules! symbols {
    ($($variant:ident => $name:literal,)*) => {
        /// An SF Symbol the port draws.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Symbol {
            $($variant,)*
        }

        impl Symbol {
            /// Every symbol, in registry order.
            pub const ALL: &'static [Symbol] = &[$(Symbol::$variant,)*];

            /// The SF Symbol name (and the SVG's file stem).
            pub const fn name(self) -> &'static str {
                match self {
                    $(Symbol::$variant => $name,)*
                }
            }

            /// The SVG source.
            pub fn svg(self) -> &'static [u8] {
                match self {
                    $(Symbol::$variant => include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/resources/icons/",
                        $name,
                        ".svg"
                    )),)*
                }
            }
        }
    };
}

symbols! {
    ArrowDownRightAndArrowUpLeft => "arrow.down.right.and.arrow.up.left",
    ArrowUpCircleFill => "arrow.up.circle.fill",
    ArrowUpForwardApp => "arrow.up.forward.app",
    ArrowUpLeftAndArrowDownRight => "arrow.up.left.and.arrow.down.right",
    Brain => "brain",
    Checklist => "checklist",
    Checkmark => "checkmark",
    CheckmarkCircleFillMulticolor => "checkmark.circle.fill.multicolor",
    ChevronCompactRight => "chevron.compact.right",
    ChevronDown => "chevron.down",
    ChevronLeft => "chevron.left",
    ChevronRight => "chevron.right",
    ChevronUp => "chevron.up",
    ChevronUpChevronDown => "chevron.up.chevron.down",
    Clock => "clock",
    ClockArrowCirclepath => "clock.arrow.circlepath",
    Cube => "cube",
    CubeTransparent => "cube.transparent",
    DocBadgePlus => "doc.badge.plus",
    DocOnDoc => "doc.on.doc",
    DocTextMagnifyingglass => "doc.text.magnifyingglass",
    ExclamationmarkTriangle => "exclamationmark.triangle",
    ExclamationmarkTriangleFill => "exclamationmark.triangle.fill",
    ExclamationmarkTriangleFillMulticolor => "exclamationmark.triangle.fill.multicolor",
    Eye => "eye",
    EyeSlash => "eye.slash",
    Folder => "folder",
    Grid => "grid",
    InfoCircleFill => "info.circle.fill",
    Line3Horizontal => "line.3.horizontal",
    Link => "link",
    LinkBadgePlus => "link.badge.plus",
    MinusMagnifyingglass => "minus.magnifyingglass",
    Paperclip => "paperclip",
    PauseCircle => "pause.circle",
    PauseFill => "pause.fill",
    Pencil => "pencil",
    Photo => "photo",
    PlayCircle => "play.circle",
    PlayFill => "play.fill",
    PlusMagnifyingglass => "plus.magnifyingglass",
    Ruler => "ruler",
    SidebarLeft => "sidebar.left",
    SliderHorizontal3 => "slider.horizontal.3",
    Sparkles => "sparkles",
    SquareAndArrowUp => "square.and.arrow.up",
    SquareSplitDiagonal => "square.split.diagonal",
    SquareStack3dUp => "square.stack.3d.up",
    StopCircleFill => "stop.circle.fill",
    StopFill => "stop.fill",
    Terminal => "terminal",
    TextBadgePlus => "text.badge.plus",
    WrenchAndScrewdriver => "wrench.and.screwdriver",
    Xmark => "xmark",
    XmarkCircleFill => "xmark.circle.fill",
    XmarkOctagonFillMulticolor => "xmark.octagon.fill.multicolor",
}

impl Symbol {
    /// The view box's width over its height.
    pub fn aspect(self) -> f32 {
        view_box(self.svg()).map_or(1.0, |[_, _, width, height]| width / height)
    }
}

/// The `viewBox` of an SVG source.
fn view_box(svg: &[u8]) -> Option<[f32; 4]> {
    let source = std::str::from_utf8(svg).ok()?;
    let start = source.find("viewBox=\"")? + "viewBox=\"".len();
    let end = start + source[start..].find('"')?;
    let mut numbers = source[start..end]
        .split([' ', ','])
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<f32>().ok());
    let mut next = || numbers.next().flatten();
    let parsed = [next()?, next()?, next()?, next()?];
    (parsed[2] > 0.0 && parsed[3] > 0.0).then_some(parsed)
}

/// A symbol `height` px tall in `tone` over `backdrop`.
pub fn icon(symbol: Symbol, height: f32, tone: Tone, backdrop: Backdrop) -> Icon {
    sized(
        cosmic_icon::from_svg_bytes(symbol.svg())
            .symbolic(true)
            .icon(),
        symbol,
        height,
    )
    .opacity(tone.alpha(&cosmic::theme::active(), backdrop))
    .class(cosmic::theme::Svg::custom(move |theme| {
        cosmic::iced::widget::svg::Style {
            color: Some(tone.solid(theme)),
        }
    }))
}

/// `symbol` `height` px tall in `tone`, centered in a `cell` px square: a
/// glyph that keeps its slot when the registry crops it tighter than the
/// grid it is laid out on.
pub fn in_cell<'a, M: 'a>(
    symbol: Symbol,
    height: f32,
    cell: f32,
    tone: Tone,
    backdrop: Backdrop,
) -> cosmic::Element<'a, M> {
    cosmic::widget::container(icon(symbol, height, tone, backdrop))
        .width(Length::Fixed(cell))
        .height(Length::Fixed(cell))
        .align_x(cosmic::iced::Alignment::Center)
        .align_y(cosmic::iced::Alignment::Center)
        .into()
}

/// A system icon by freedesktop name, `size` px: where upstream draws a
/// system-provided image rather than an SF Symbol (a file's type icon from
/// `NSWorkspace`, the app icon), the icon theme supplies it.
pub fn named(name: &str, size: u16) -> Icon {
    cosmic::widget::icon::from_name(name).size(size).icon()
}

/// A `.multicolor` symbol `height` px tall, in its own colors.
pub fn multicolor(symbol: Symbol, height: f32) -> Icon {
    sized(
        cosmic_icon::from_svg_bytes(symbol.svg()).icon(),
        symbol,
        height,
    )
}

fn sized(icon: Icon, symbol: Symbol, height: f32) -> Icon {
    icon.size(height.round() as u16)
        .width(Length::Fixed(height * symbol.aspect()))
        .height(Length::Fixed(height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn icon_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/icons")
    }

    #[test]
    fn every_symbol_ships_a_drawable_svg() {
        for symbol in Symbol::ALL {
            let svg = symbol.svg();
            assert!(
                std::str::from_utf8(svg).is_ok_and(|s| s.contains("<svg")),
                "{} is not an svg",
                symbol.name()
            );
            assert!(view_box(svg).is_some(), "{} has no view box", symbol.name());
        }
    }

    #[test]
    fn symbol_names_are_unique() {
        let names: HashSet<&str> = Symbol::ALL.iter().map(|symbol| symbol.name()).collect();
        assert_eq!(names.len(), Symbol::ALL.len());
    }

    #[test]
    fn no_svg_ships_without_a_symbol() {
        let names: HashSet<&str> = Symbol::ALL.iter().map(|symbol| symbol.name()).collect();
        for entry in std::fs::read_dir(icon_dir()).unwrap() {
            let path = entry.unwrap().path();
            assert!(
                path.is_file(),
                "{} is not a file: one flat directory",
                path.display()
            );
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            let stem = file
                .strip_suffix(".svg")
                .unwrap_or_else(|| panic!("{file} is not an svg"));
            assert!(names.contains(stem), "{file} has no Symbol");
        }
    }

    #[test]
    fn the_view_box_sets_the_width() {
        assert_eq!(
            view_box(br#"<svg viewBox="3 0 10 16">"#),
            Some([3.0, 0.0, 10.0, 16.0])
        );
        assert_eq!(
            view_box(br#"<svg viewBox="0,0,4,2">"#),
            Some([0.0, 0.0, 4.0, 2.0])
        );
        assert_eq!(view_box(b"<svg>"), None);
        assert_eq!(view_box(br#"<svg viewBox="0 0 0 2">"#), None);
        assert_eq!(Symbol::Xmark.aspect(), 1.0);
    }
}
