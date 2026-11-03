//! Upstream text styles mapped onto COSMIC's text scale.
//!
//! Upstream styles text with SwiftUI's semantic fonts (`.body`, `.callout`,
//! `.caption`, …) and a few `.system(size:)` literals, on macOS's 13 pt body.
//! The port keeps COSMIC's own scale (libcosmic body is 14 px, and the font
//! family is the user's) and reproduces upstream's HIERARCHY: each macOS
//! style keeps its ratio to body. A view names the upstream style it
//! reproduces — `typography::CAPTION`, or `typography::scaled(11.0)` for an
//! upstream `.system(size: 11)` — instead of a bare number.

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

#[cfg(test)]
mod tests {
    use super::*;

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
