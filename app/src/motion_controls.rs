//! Motion controls — the pure halves of upstream
//! `Viewer3D/MotionControls.swift`, the capsule that plays, scrubs, and
//! loops a motion study over the 3D pane. Everything here is the number
//! and text formatting SwiftUI does through `formatted(...)`, pinned by
//! test against the two shapes upstream renders:
//!
//! - [`time_readout`] — the `time / duration s` readout
//!   (MotionControls.swift:31, `fractionLength(2)`).
//! - [`format_significant`] — `.precision(.significantDigits(min...max))`
//!   for the values row (1...3, :58) and the collision volume (1...2, :86),
//!   including thousands grouping.
//! - [`speed_label`] — the speed menu's `\(speed)×` labels (:38, :44).
//! - [`collision_text`] — the collision pairs line (:81-89); the `↔`,
//!   `mm³`, and separators are upstream's literals.
//!
//! The widget half — `view_motion_controls`, the rows those formatters
//! fill — joins this module with the pane's playback messages
//! (`viewer_pane::Message`'s `TogglePlayback`/`Scrub`/`SetSpeed`/
//! `ToggleLoops`) and the M6-T9 `fl!` scaffold it renders through; until
//! then the pane mounts nothing from here.
//!
//! Ordering law for the values row (the milestone's sixth review focus):
//! `StudyState.values` is a `BTreeMap` and iterates sorted, while
//! upstream's row order is first-frame appearance — the view must iterate
//! [`MotionStudy::value_names`] and read the map by key
//! (engine/src/parsing/animation.rs, `StudyState.values`'s own doc);
//! `values_render_in_first_appearance_order` pins the seam this module
//! owns.

use engine::parsing::animation::Collision;

/// The readout beside the scrub slider: both times at exactly two
/// fraction digits, separated by upstream's literal `" / "` and `" s"`
/// (MotionControls.swift:31).
pub fn time_readout(time: f64, duration: f64) -> String {
    format!("{time:.2} / {duration:.2} s")
}

/// Swift's `.formatted(.number.precision(.significantDigits(min...max)))`
/// for the shapes MotionControls renders: rounded to `max_digits`
/// significant digits, fractional trailing zeros stripped down to
/// `min_digits`, thousands grouped with `,`. Zero and non-finite input
/// render `"0"` (Swift prints signed zero plainly; the study never
/// carries infinities).
pub fn format_significant(value: f64, min_digits: usize, max_digits: usize) -> String {
    if value == 0.0 || !value.is_finite() {
        return "0".to_string();
    }
    // Round at the decade level first, but ONLY when the integer digits
    // exceed the budget (1234.5 at 3 -> 1230): the `{:.decimals}` below
    // stops at zero decimals and would otherwise print every integer
    // digit. With headroom left the format itself rounds at the right
    // digit — a units-level pre-round here would snap 0.5 to 1.
    let magnitude = value.abs().log10().floor() as i32 + 1; // integer digits
    let value = if magnitude > max_digits as i32 {
        let round_to = 10f64.powi(magnitude - max_digits as i32);
        (value / round_to).round() * round_to
    } else {
        value
    };
    // Re-derive: a carry can grow the magnitude (999.5 -> 1000)
    let magnitude = value.abs().log10().floor() as i32 + 1;
    let decimals = (max_digits as i32 - magnitude).clamp(0, 18) as usize;
    let mut digits = format!("{value:.decimals$}");
    let significant = |s: &str| {
        s.trim_start_matches(['-', '0'])
            .chars()
            .filter(|c| c.is_ascii_digit())
            .count()
    };
    while digits.contains('.') && digits.ends_with('0') && significant(&digits) > min_digits {
        digits.pop();
    }
    if digits.ends_with('.') {
        digits.pop();
    }
    group_thousands(&digits)
}

/// Insert `,` every three integer digits of a decimal string
/// (`1230` → `1,230`, `-1234567.89` → `-1,234,567.89`) — Swift's
/// `formatted()` groups; the fraction, if any, rides along untouched.
fn group_thousands(digits: &str) -> String {
    let (sign, digits) = digits
        .strip_prefix('-')
        .map_or(("", digits), |rest| ("-", rest));
    let (integer, fraction) = digits
        .split_once('.')
        .map_or((digits, None), |(i, f)| (i, Some(f)));
    let mut grouped = String::with_capacity(digits.len() + integer.len() / 3);
    for (index, digit) in integer.chars().enumerate() {
        if index > 0 && (integer.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction {
        Some(fraction) => {
            grouped.push('.');
            grouped.push_str(fraction);
        }
        None => {}
    }
    format!("{sign}{grouped}")
}

/// The speed menu's label for one speed: Rust's `Display` prints the
/// shortest form, matching Swift's `\(speed.formatted())` — `1×`, not
/// `1.0×` (MotionControls.swift:38,44).
pub fn speed_label(speed: f64) -> String {
    format!("{speed}×")
}

/// The collisions line: each pair's names joined by upstream's literal
/// ` ↔ `, its volume (when measured) appended at 1...2 significant
/// digits with the literal ` mm³`, pairs joined by `", "`
/// (MotionControls.swift:81-89).
pub fn collision_text(collisions: &[Collision]) -> String {
    collisions
        .iter()
        .map(|collision| {
            let pair = collision.parts.join(" ↔ ");
            match collision.volume {
                Some(volume) => format!("{pair} {} mm³", format_significant(volume, 1, 2)),
                None => pair,
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}
