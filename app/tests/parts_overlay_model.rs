//! Pure-helper tests for the parts-overlay family: the measure
//! badge's text in all three states (ModelPane.swift:163-179) and the
//! legend's 12-dot cap plus count label (ModelPane.swift:202-213).
//!
//! The fl!-produced states assert ENGLISH catalog values, so every test
//! pins the loader to `en` first — the host locale must not decide test
//! outcomes (`localize::select` is the test seam for that).

use app::localize;
use app::parts_overlay::badge_text;
use app::viewer_pane::{legend_dots, legend_label};
use engine::parsing::Part;
use engine::settings::MeasurementUnit as U;
use viewer::tools::arrangement::legend_colors;

/// One single-triangle part carrying exactly one color.
fn colored_part(name: &str, color: [u8; 3]) -> Part {
    Part {
        name: name.into(),
        positions: vec![[0.0; 3], [1.0; 3], [0.0, 1.0, 0.0]],
        triangle_colors: vec![Some(color)],
    }
}

#[test]
fn badge_texts_all_three_states() {
    localize::select("en");
    let unit = U::Millimeters;
    assert_eq!(badge_text(&[], unit), "Click a point on the model");
    assert_eq!(badge_text(&[[0.0; 3]], unit), "Click the second point");
    // 3-4-5 diagonal: total 5.0, dX 3, dY 4 (MODEL), dZ 0 — symbol on
    // EVERY term, Δ a literal glyph like upstream's badge
    assert_eq!(
        badge_text(&[[0.0, 0.0, 0.0], [3.0, -4.0, 0.0]], unit),
        "5.0 mm · ΔX 3.0 mm · ΔY 4.0 mm · ΔZ 0.0 mm"
    );
    assert_eq!(
        badge_text(&[[0.0, 0.0, 0.0], [25.4, 0.0, 0.0]], U::Inches),
        "1.00 in · ΔX 1.00 in · ΔY 0.00 in · ΔZ 0.00 in"
    );
}

#[test]
fn legend_caps_at_twelve_dots_and_counts_all() {
    localize::select("en");
    // 13 distinct colors in first-appearance order
    let colors: Vec<[u8; 3]> = (0..13u8).map(|i| [i, 255 - i, i * 17]).collect();
    let parts: Vec<Part> = colors
        .iter()
        .enumerate()
        .map(|(i, c)| colored_part(&format!("part{i}"), *c))
        .collect();
    let all = legend_colors(&parts);
    assert_eq!(all.len(), 13, "every distinct color lists");
    assert_eq!(legend_dots(&all), &all[..12], "dots cap at twelve");
    // the label counts EVERY color, not the capped twelve (upstream
    // `mesh.colors.count`), with the %lld placeholder carrying the count
    assert_eq!(legend_label(all.len()), "13 colors");
    assert_eq!(legend_label(0), "0 colors");
    assert_eq!(legend_label(1), "1 colors", "no pluralization upstream");
}
