//! viewer tools model tests (M6 T8): the ModelViewerState port —
//! measure-point lifecycle, layout effects, isolate, and the double-click
//! window.

use app::viewer_tools::{DOUBLE_CLICK_WINDOW, EXPLODE_RANGE, ViewerTools, is_double_click};
use engine::settings::PartsLayout as PL;
use std::time::{Duration, Instant};
use viewer::tools::section::SectionAxis;

#[test]
fn measure_points_cap_at_two_third_restarts() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    t.toggle_measuring();
    assert!(t.measure_points.is_empty(), "toggles clear");
    t.add_measure_point([0.0; 3]);
    t.add_measure_point([1.0, 2.0, 3.0]);
    assert_eq!(t.measure_points.len(), 2);
    t.add_measure_point([9.0; 3]);
    assert_eq!(t.measure_points, [[9.0, 9.0, 9.0]], "third click restarts");
}

#[test]
fn state_changes_clear_measurement() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    t.toggle_measuring();
    t.add_measure_point([1.0; 3]);
    t.set_layout(PL::Plate);
    assert!(t.measure_points.is_empty(), "layout change clears");
    t.add_measure_point([1.0; 3]);
    t.mesh_changed();
    assert!(t.measure_points.is_empty(), "mesh change clears");
    t.add_measure_point([1.0; 3]);
    t.toggle_measuring();
    assert!(t.measure_points.is_empty(), "toggle-off clears");
    t.toggle_measuring();
    t.clear_measurement();
    assert!(t.measure_points.is_empty(), "Esc clears");
}

#[test]
fn effective_layout_forces_assembled_when_comparing() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    assert_eq!(t.effective_layout(2, false), PL::Exploded);
    assert_eq!(t.effective_layout(2, true), PL::Assembled);
    assert_eq!(
        t.effective_layout(1, false),
        PL::Assembled,
        "exploded needs 2 parts"
    );
    assert_eq!(t.effective_layout(1, false), t.effective_layout(1, false));
    t.layout = PL::Plate;
    assert_eq!(
        t.effective_layout(1, true),
        PL::Assembled,
        "comparing wins over plate too"
    );
    assert_eq!(
        ViewerTools::available_layouts(2),
        [PL::Assembled, PL::Exploded, PL::Plate]
    );
    assert_eq!(
        ViewerTools::available_layouts(1),
        [PL::Assembled, PL::Plate]
    );
}

#[test]
fn isolate_toggles_others_hidden() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    let all = ["a".to_string(), "b".into(), "c".into()];
    t.isolate("a", &all);
    assert_eq!(t.hidden, ["b", "c"].map(String::from).into_iter().collect());
    t.isolate("a", &all);
    assert!(t.hidden.is_empty(), "second double-click un-isolates");
}

#[test]
fn isolate_from_partial_state_assigns_others() {
    // PartsOverlay.swift:145-148 is compare-and-assign, not a per-row
    // toggle: from a partially populated hidden set, isolate assigns
    // exactly the others — an eye-hidden part STAYS hidden and the target
    // becomes visible — and a hidden target is revealed.
    let mut t = ViewerTools::new(PL::Exploded, false);
    let all = ["a".to_string(), "b".into(), "c".into()];
    t.toggle_hidden("b");
    t.isolate("a", &all);
    assert_eq!(
        t.hidden,
        ["b", "c"].map(String::from).into_iter().collect(),
        "eye-hidden stays hidden, target visible"
    );
    t.toggle_hidden("a");
    t.isolate("a", &all);
    assert_eq!(
        t.hidden,
        ["b", "c"].map(String::from).into_iter().collect(),
        "target revealed even when it was hidden"
    );
}

#[test]
fn double_click_window() {
    let now = Instant::now();
    assert!(is_double_click(
        Some((
            now - DOUBLE_CLICK_WINDOW + Duration::from_millis(50),
            "a".into()
        )),
        now,
        "a"
    ));
    assert!(!is_double_click(
        Some((now - Duration::from_secs(2), "a".into())),
        now,
        "a"
    ));
    assert!(
        !is_double_click(Some((now, "a".into())), now, "b"),
        "different rows"
    );
    assert!(!is_double_click(None, now, "a"));
}

#[test]
fn defaults_match_model_viewer_state() {
    // ModelViewerState.swift:21-47: grid on, wireframe/measure/section off,
    // axis X, slider mid, explode 1.0, sets empty; layout and collapsed
    // arrive from engine settings (Exploded/false upstream defaults).
    let t = ViewerTools::new(PL::Exploded, false);
    assert!(t.shows_grid);
    assert!(!t.wireframe);
    assert!(!t.measuring);
    assert!(!t.sectioning);
    assert_eq!(t.section_axis, SectionAxis::X);
    assert_eq!(t.section_position, 0.5);
    assert_eq!(t.layout, PL::Exploded);
    assert_eq!(t.explode_amount, 1.0);
    assert!(t.hidden.is_empty());
    assert!(t.attached.is_empty());
    assert!(t.measure_points.is_empty());
    assert!(!t.collapsed);
    assert_eq!((*EXPLODE_RANGE.start(), *EXPLODE_RANGE.end()), (0.0, 2.0));
    assert!(EXPLODE_RANGE.contains(&t.explode_amount));
}

#[test]
fn toggles_and_sectioning() {
    // set_sectioning is a plain set (the pane owns the rebuild side
    // effect); toggle_hidden/toggle_attached are membership toggles.
    let mut t = ViewerTools::new(PL::Exploded, false);
    t.set_sectioning(true);
    assert!(t.sectioning);
    t.set_sectioning(false);
    assert!(!t.sectioning);
    t.toggle_hidden("a");
    assert!(t.hidden.contains("a"));
    t.toggle_hidden("a");
    assert!(t.hidden.is_empty());
    t.toggle_attached("a");
    t.toggle_attached("b");
    assert_eq!(t.attached.len(), 2);
    t.toggle_attached("b");
    assert_eq!(t.attached.len(), 1);
}
