//! Motion controls — the port of upstream
//! `Viewer3D/MotionControls.swift`, the capsule that plays, scrubs, and
//! loops a motion study over the 3D pane: [`view_motion_controls`]
//! renders it and the number/text helpers below are the SwiftUI
//! `formatted(...)` shapes it fills, pinned by test:
//!
//! - [`time_readout`] — the `time / duration s` readout
//!   (MotionControls.swift:31, `fractionLength(2)`), formatted through the
//!   `s` catalog key.
//! - [`format_significant`] — `.precision(.significantDigits(min...max))`
//!   for the values row (1...3, :58) and the collision volume (1...2, :86),
//!   including thousands grouping and ICU's half-up exact-tie rounding.
//! - [`speed_label`] — the speed menu's `\(speed)×` labels (:38, :44)
//!   formatted through the `speed-option` catalog key.
//! - [`collision_text`] — the collision pairs line (:81-89); the `↔`,
//!   `mm³`, and separators are upstream's literals (no catalog key backs
//!   them).
//!
//! The pane floats the capsule bottom-center while animating (T7's
//! mount, where the section controls mount); this module returns the
//! capsule itself. Divergences from upstream, all cosmetic or
//! plan-ruled:
//!
//! - Upstream nests the speed picker and the Ripeti toggle in ONE menu
//!   (:35-49); the port shows the speed dropdown with an adjacent loop
//!   button — RULED at M8 plan review (Ruling 12's safe branch): cosmic's
//!   menu overlay corrupts widget state on dynamic-ish content while the
//!   corruption boundary is unpinned (docs/research/
//!   libcosmic-menu-dynamic-items-bug.md, struck plan Task 2), so the
//!   single-menu shape is unreachable evidence-wise. Revisit only if M9
//!   pins the boundary.
//! - iced sliders and cosmic dropdowns carry no accessibility text at the
//!   pinned rev, so their descriptions render as tooltips (the parts
//!   overlay's slider pattern); buttons get real descriptions.
//! - The phase label's `.fontWeight(.medium)` maps to cosmic's
//!   `semibold` — the pinned rev has no medium font helper (its own
//!   caption-heading preset makes the same step).
//! - The warning glyph is `dialog-warning`, the freedesktop stand-in for
//!   `exclamationmark.triangle.fill`; the icon theme decides its tint —
//!   the text beside it carries the red.
//!
//! Ordering law for the values row (the milestone's sixth review focus):
//! `StudyState.values` is a `BTreeMap` and iterates sorted, while
//! upstream's row order is first-frame appearance — the view must iterate
//! [`MotionStudy::value_names`] and read the map by key
//! (engine/src/parsing/animation.rs, `StudyState.values`'s own doc);
//! `values_render_in_first_appearance_order` pins the seam this module
//! owns.

use cosmic::Element;
use cosmic::iced::widget::slider;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{Column, Row, button, container, dropdown, text, tooltip};
use engine::parsing::animation::{Collision, MotionStudy, StudyState};
use viewer::playback::{MotionPlayer, SCRUB_FLOOR, SPEEDS};

use crate::fl;
use crate::ui::surface::{self, Corner};
use crate::ui::{Backdrop, Symbol, Tone, icon::icon};
use crate::viewer_pane::Message;

/// This module's text widget, pinned to cosmic's theme/renderer (the
/// chat pane's invariance note).
type Text<'a> = cosmic::iced::widget::Text<'a, cosmic::theme::Theme, cosmic::Renderer>;

/// SwiftUI's `.caption`, the capsule's text style.
const CAPTION: f32 = crate::typography::CAPTION;

/// The capsule is a material (MotionControls.swift:77).
const BACKDROP: Backdrop = Backdrop::Material;

/// The readout beside the scrub slider: both times at exactly two fraction
/// digits, joined by the `s` catalog key's separators
/// (MotionControls.swift:31).
pub fn time_readout(time: f64, duration: f64) -> String {
    fl!(
        "s",
        time = format!("{time:.2}"),
        duration = format!("{duration:.2}")
    )
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
    // Exact decimal ties round HALF-UP, away from zero, like Swift/ICU —
    // Rust's `{:.n}` rounds ties to even (0.125 -> "0.12" at two digits).
    // Rounding at the target digit first leaves the format a value whose
    // last digit is already decided; the scaled magnitudes in this domain
    // (decimals <= 18, large only for values tiny enough to stay exact)
    // survive the round-trip through `{:.decimals}` untouched.
    let scale = 10f64.powi(decimals as i32);
    let units = (value.abs() * scale + 0.5).floor();
    let value = (units / scale).copysign(value);
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
/// `1.0×` — with the `×` from the `speed-option` catalog key
/// (MotionControls.swift:38,44).
pub fn speed_label(speed: f64) -> String {
    fl!("speed-option", speed = speed)
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

// ─── view ──────────────────────────────────────────────────────────────────

/// The whole capsule (MotionControls.swift:9-79): row 1 carries play or
/// pause, the scrub slider, the time readout, the speed dropdown, and the
/// loop toggle; row 2 appears only when the state at `player.time` has a
/// label, values, or collisions (:51). Everything rendered is copied or
/// formatted into owned strings, so the element's lifetime is free of
/// the references passed in (the parameters overlay's rule).
pub fn view_motion_controls<'a>(
    player: &MotionPlayer,
    study: &MotionStudy,
) -> Element<'a, Message> {
    let state = study.state_at(player.time);
    let mut capsule = Column::new().spacing(6).push(controls_row(player, study));
    if state.label.is_some() || !state.values.is_empty() || !state.collisions.is_empty() {
        capsule = capsule.push(state_row(study, &state));
    }
    container(capsule)
        .padding([8.0, 12.0]) // vertical 8, horizontal 12 (:74-75)
        // .frame(maxWidth: 560) (:76): the slider is flexible, so the
        // capsule takes the full 560 wherever the pane allows it
        .width(Length::Fill)
        .max_width(560.0)
        .class(surface::material_fill(Corner::S)) // RoundedRectangle(cornerRadius: 10) (:77)
        .into()
}

/// Row 1 (MotionControls.swift:12-50), spacing 10: the play/pause button
/// (16 px icon, `.frame(width: 16)`), the scrub slider over
/// `0..=max(duration, 0.001)` (:23), the mono readout, the speed
/// dropdown, and the loop toggle.
fn controls_row<'a>(player: &MotionPlayer, study: &MotionStudy) -> Element<'a, Message> {
    // play.fill / pause.fill in a 16 pt frame, borderless (:14-20)
    // pause.fill draws on a 16-unit grid; play.fill is cropped to its
    // glyph, 12 px tall like the grid's triangle
    let (glyph, help) = if player.is_playing {
        (icon(Symbol::PauseFill, 16.0, Tone::Secondary, BACKDROP), fl!("pausa"))
    } else {
        (icon(Symbol::PlayFill, 12.2, Tone::Secondary, BACKDROP), fl!("riproduci"))
    };
    let cell = container(glyph)
        .width(16.0)
        .height(16.0)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center);
    let play = crate::ui::button::with_help(
        crate::ui::button::borderless(cell, Some(Message::TogglePlayback))
            .description(help.clone()),
        help,
        tooltip::Position::FollowCursor,
    );
    let scrub = tooltip(
        slider(
            0.0..=study.duration().max(SCRUB_FLOOR),
            player.time,
            Message::Scrub,
        )
        .width(Length::Fill),
        text(fl!("tempo-dell-animazione")),
        tooltip::Position::FollowCursor,
    );
    let readout = mono_dim(time_readout(player.time, study.duration())).size(CAPTION);
    // The closed dropdown shows the selected entry's label, which is
    // exactly speed_label(player.speed) for every speed the menu can set.
    let speeds: Vec<String> = SPEEDS.iter().map(|speed| speed_label(*speed)).collect();
    let selected = SPEEDS.iter().position(|speed| *speed == player.speed);
    let speed = tooltip(
        dropdown(speeds, selected, |index| Message::SetSpeed(SPEEDS[index])),
        text(fl!("velocit-di-riproduzione")), // the menu's help (:49)
        tooltip::Position::FollowCursor,
    );
    // The on-state shows through the class (the parts picker's idiom):
    // cosmic's builder buttons carry no selected flag at this pin.
    let loop_toggle = button::text(fl!("ripeti"))
        .class(if player.loops {
            cosmic::theme::Button::Suggested
        } else {
            cosmic::theme::Button::Standard
        })
        .on_press(Message::ToggleLoops);
    Row::new()
        .spacing(10)
        .align_y(cosmic::iced::Alignment::Center)
        .push(play)
        .push(scrub)
        .push(readout)
        .push(speed)
        .push(loop_toggle)
        .into()
}

/// Row 2 (MotionControls.swift:51-70), spacing 12: the phase label at
/// medium weight, each value as `name formatted` in mono dim — iterated
/// in `value_names()` order, never the BTreeMap's (the module's ordering
/// law) — and, when the state carries collisions, the warning glyph and
/// the joined pairs in red.
fn state_row<'a>(study: &MotionStudy, state: &StudyState) -> Element<'a, Message> {
    let mut row = Row::new().spacing(12);
    if let Some(label) = &state.label {
        row = row.push(
            text(label.clone())
                .size(CAPTION)
                .font(cosmic::font::semibold())
                .class(Tone::Primary.text(BACKDROP)),
        );
    }
    for name in study.value_names() {
        if let Some(value) = state.values.get(name) {
            row = row.push(
                mono_dim(fl!(
                    "motion-value-row",
                    name = name,
                    value = format_significant(*value, 1, 3)
                ))
                .size(CAPTION),
            );
        }
    }
    if !state.collisions.is_empty() {
        row = row.push(
            Row::new()
                .spacing(4)
                .align_y(cosmic::iced::Alignment::Center)
                .push(icon(Symbol::ExclamationmarkTriangleFill, 11.0, Tone::Danger, BACKDROP))
                .push(
                    text(collision_text(&state.collisions))
                        .size(CAPTION)
                        .wrapping(cosmic::iced::widget::text::Wrapping::None)
                        .class(Tone::Danger.text(BACKDROP)),
                ),
        );
    }
    row.into()
}

/// The readout and the values row's dim monospace (upstream's caption
/// `.monospacedDigit().foregroundStyle(.secondary)`).
fn mono_dim(body: String) -> Text<'static> {
    text(body)
        .font(cosmic::font::mono())
        .class(Tone::Secondary.text(BACKDROP))
}

