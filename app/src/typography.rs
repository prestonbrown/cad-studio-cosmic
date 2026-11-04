//! Upstream text styles mapped onto COSMIC's text scale.
//!
//! Upstream styles text with SwiftUI's semantic fonts (`.body`, `.callout`,
//! `.caption`, …) and a few `.system(size:)` literals, on macOS's 13 pt body.
//! The port keeps COSMIC's own scale (libcosmic body is 14 px, and the font
//! family is the user's) and reproduces upstream's HIERARCHY: each macOS
//! style keeps its ratio to body. A view names the upstream style it
//! reproduces — `typography::CAPTION`, or `typography::scaled(11.0)` for an
//! upstream `.system(size: 11)` — instead of a bare number.
//!
//! Weights go through [`font`], which maps an upstream weight onto a face the
//! interface family actually has.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cosmic::iced::Font;
use cosmic::iced::font::{Family, Weight};

/// libcosmic's body size (`cosmic::widget::text::body`, the app default).
pub const COSMIC_BODY: f32 = 14.0;
/// macOS `.body`, the basis every upstream size is relative to.
pub const MACOS_BODY: f32 = 13.0;

/// An upstream point size on COSMIC's scale, rounded to the half pixel.
pub const fn scaled(macos_pt: f32) -> f32 {
    let raw = macos_pt * COSMIC_BODY / MACOS_BODY;
    // const-friendly round-to-0.5: floor(raw * 2 + 0.5) / 2
    ((raw * 2.0 + 0.5) as i32) as f32 / 2.0
}

/// `.largeTitle` (26 pt)
pub const LARGE_TITLE: f32 = scaled(26.0);
/// `.title` (22 pt)
pub const TITLE: f32 = scaled(22.0);
/// `.title2` (17 pt)
pub const TITLE2: f32 = scaled(17.0);
/// `.title3` (15 pt)
pub const TITLE3: f32 = scaled(15.0);
/// `.headline` — semibold at body size.
pub const HEADLINE: f32 = COSMIC_BODY;
/// `.body`
pub const BODY: f32 = COSMIC_BODY;
/// `.callout` (12 pt)
pub const CALLOUT: f32 = scaled(12.0);
/// `.subheadline` (11 pt)
pub const SUBHEADLINE: f32 = scaled(11.0);
/// `.footnote` (10 pt)
pub const FOOTNOTE: f32 = scaled(10.0);
/// `.caption` (10 pt)
pub const CAPTION: f32 = scaled(10.0);
/// `.caption2` (10 pt)
pub const CAPTION2: f32 = scaled(10.0);
/// The small control size's label (`.controlSize(.small)`: 11 pt).
pub const SMALL_CONTROL: f32 = scaled(11.0);

/// An average glyph advance, for estimating a label's width (iced measures
/// no text at view time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// Words in the interface font: upstream's segment labels advance
    /// 5 pt per 11 pt.
    Letters,
    /// Digit-heavy labels (`1/2 · Assembly`, `1.5×`): a digit advances
    /// ~0.57 em in the interface families.
    Digits,
}

impl Advance {
    const fn em(self) -> f32 {
        match self {
            Self::Letters => 5.0 / 11.0,
            Self::Digits => 0.57,
        }
    }
}

/// `text`'s estimated width at `size`.
pub fn estimated_width(text: &str, size: f32, advance: Advance) -> f32 {
    text.chars().count() as f32 * size * advance.em()
}

/// The interface font at the weight an upstream `.fontWeight(_)` asks for,
/// resolved against the faces the user's interface family actually carries.
///
/// `.headline`: `text` semibold at the headline size.
pub fn headline<'a>(
    text: cosmic::iced::widget::Text<'a, cosmic::Theme, cosmic::Renderer>,
) -> cosmic::iced::widget::Text<'a, cosmic::Theme, cosmic::Renderer> {
    text.size(HEADLINE).font(font(Weight::Semibold))
}

/// Every weighted text in the views goes through here. See [`weight`].
pub fn font(upstream: Weight) -> Font {
    let base = cosmic::font::default();
    Font {
        weight: weight(upstream, base.family),
        ..base
    }
}

/// The weight to request for an upstream weight in `family`.
///
/// The font system matches a missing weight by CSS rules, which send
/// Medium (500) DOWN to Regular when the family has no Medium face — Open
/// Sans, COSMIC's default, ships 300/400/600/700/800 — so `.medium` would
/// render lighter than upstream, where it reads as a step above body.
/// [`resolve_weight`] makes that step land on a heavier face instead.
pub fn weight(upstream: Weight, family: Family) -> Weight {
    from_number(resolve_weight(number(upstream), &available_weights(family)))
}

/// The weight to request for `upstream` given the `available` upright faces.
///
/// An available weight is used as is; a weight at or under Regular is left
/// to the font system's own matching. A heavier weight the family lacks
/// becomes the nearest available face heavier than Regular (ties go to the
/// heavier face), so emphasis never collapses to body weight.
pub fn resolve_weight(upstream: u16, available: &[u16]) -> u16 {
    if upstream <= REGULAR || available.is_empty() || available.contains(&upstream) {
        return upstream;
    }
    available
        .iter()
        .copied()
        .filter(|&w| w > REGULAR)
        .min_by_key(|&w| (w.abs_diff(upstream), std::cmp::Reverse(w)))
        .unwrap_or(upstream)
}

const REGULAR: u16 = 400;

/// The upright, normal-width weights `family` carries in the font system,
/// cached per family until the font system loads another font.
///
/// The renderer may hold the font system, and a view never blocks on it:
/// while it is held the family's last known weights answer (a font load
/// in between only makes them one version stale), and only a family never
/// seen before falls back to the font system's own matching for that
/// frame.
fn available_weights(family: Family) -> Vec<u16> {
    use cosmic::iced::advanced::graphics::text::{cosmic_text::fontdb, font_system};
    type Cache = HashMap<Family, (cosmic::iced::advanced::graphics::text::Version, Vec<u16>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(system) = font_system().try_read() else {
        return cache
            .get(&family)
            .map(|(_, weights)| weights.clone())
            .unwrap_or_default();
    };
    let version = system.version();
    if let Some((cached, weights)) = cache.get(&family)
        && *cached == version
    {
        return weights.clone();
    }
    let db = system.db();
    let name = match family {
        Family::Name(name) => name.to_owned(),
        Family::Serif => db.family_name(&fontdb::Family::Serif).to_owned(),
        Family::SansSerif => db.family_name(&fontdb::Family::SansSerif).to_owned(),
        Family::Cursive => db.family_name(&fontdb::Family::Cursive).to_owned(),
        Family::Fantasy => db.family_name(&fontdb::Family::Fantasy).to_owned(),
        Family::Monospace => db.family_name(&fontdb::Family::Monospace).to_owned(),
    };
    let mut weights: Vec<u16> = db
        .faces()
        .filter(|face| {
            face.style == fontdb::Style::Normal
                && face.stretch == fontdb::Stretch::Normal
                && face
                    .families
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case(&name))
        })
        .map(|face| face.weight.0)
        .collect();
    weights.sort_unstable();
    weights.dedup();
    cache.insert(family, (version, weights.clone()));
    weights
}

fn number(weight: Weight) -> u16 {
    match weight {
        Weight::Thin => 100,
        Weight::ExtraLight => 200,
        Weight::Light => 300,
        Weight::Normal => 400,
        Weight::Medium => 500,
        Weight::Semibold => 600,
        Weight::Bold => 700,
        Weight::ExtraBold => 800,
        Weight::Black => 900,
    }
}

fn from_number(weight: u16) -> Weight {
    match weight {
        ..=150 => Weight::Thin,
        151..=250 => Weight::ExtraLight,
        251..=350 => Weight::Light,
        351..=450 => Weight::Normal,
        451..=550 => Weight::Medium,
        551..=650 => Weight::Semibold,
        651..=750 => Weight::Bold,
        751..=850 => Weight::ExtraBold,
        _ => Weight::Black,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open Sans as COSMIC installs it: Light, Regular, Semibold, Bold, ExtraBold.
    const OPEN_SANS: [u16; 5] = [300, 400, 600, 700, 800];

    #[test]
    fn a_missing_medium_steps_up_to_semibold_not_down_to_regular() {
        assert_eq!(resolve_weight(500, &OPEN_SANS), 600);
    }

    #[test]
    fn an_installed_weight_is_used_as_is() {
        assert_eq!(resolve_weight(500, &[400, 500, 700]), 500);
        assert_eq!(resolve_weight(600, &OPEN_SANS), 600);
        assert_eq!(resolve_weight(700, &OPEN_SANS), 700);
    }

    #[test]
    fn a_missing_heavy_weight_takes_the_nearest_heavier_than_regular_face() {
        // equidistant 500 and 700: the heavier keeps the emphasis
        assert_eq!(resolve_weight(600, &[400, 500, 700]), 700);
        assert_eq!(resolve_weight(700, &[400, 500]), 500);
        // nothing above Regular: leave it to the font system
        assert_eq!(resolve_weight(600, &[300, 400]), 600);
    }

    #[test]
    fn regular_and_lighter_and_unknown_families_are_untouched() {
        assert_eq!(resolve_weight(400, &[600]), 400);
        assert_eq!(resolve_weight(300, &[400, 600]), 300);
        assert_eq!(resolve_weight(500, &[]), 500);
    }

    #[test]
    fn weight_numbers_round_trip() {
        for w in [
            Weight::Thin,
            Weight::ExtraLight,
            Weight::Light,
            Weight::Normal,
            Weight::Medium,
            Weight::Semibold,
            Weight::Bold,
            Weight::ExtraBold,
            Weight::Black,
        ] {
            assert_eq!(from_number(number(w)), w);
        }
    }

    #[test]
    fn body_is_cosmics_and_the_hierarchy_is_upstreams() {
        assert_eq!(BODY, 14.0);
        assert_eq!(scaled(13.0), BODY);
        assert_eq!(CALLOUT, 13.0);
        assert_eq!(SUBHEADLINE, 12.0);
        assert_eq!(CAPTION, 11.0);
        assert_eq!(TITLE, 23.5);
        assert!(CAPTION < CALLOUT && CALLOUT < BODY && BODY < TITLE3);
    }
}
