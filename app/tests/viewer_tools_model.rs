//! viewer tools model tests: the ModelViewerState port —
//! measure-point lifecycle, layout effects, isolate, the double-click
//! window, and the isAnimating semantics (mutual exclusion with the
//! section/measure tools, assembled-forced layout, motion gate).

use app::viewer_tools::{DOUBLE_CLICK_WINDOW, EXPLODE_RANGE, ViewerTools, is_double_click};
use engine::settings::PartsLayout as PL;
use std::time::{Duration, Instant};
use viewer::playback::MotionPlayer;
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
    t.set_animating(true);
    assert_eq!(
        t.effective_layout(2, false),
        PL::Assembled,
        "animating wins over plate too"
    );
    t.set_animating(false);
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
    assert!(!t.animating, "animation off until set_animating");
    assert_eq!(
        t.player,
        MotionPlayer::default(),
        "player idle at 0/1×/looping"
    );
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

#[test]
fn set_animating_ports_the_didset_semantics() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    t.sectioning = true;
    t.measuring = true;
    t.player.time = 1.25;
    t.set_animating(true);
    assert!(t.animating);
    assert_eq!(t.player.time, 0.0, "time resets on every toggle");
    assert!(t.player.is_playing);
    assert!(
        !t.sectioning && !t.measuring,
        "starting animation clears both"
    );
    // toggling off keeps the (already cleared) tools and stops the player
    t.player.time = 0.5;
    t.set_animating(false);
    assert!(!t.animating && !t.player.is_playing);
    assert_eq!(t.player.time, 0.0, "off resets the clock too");
    assert!(!t.sectioning && !t.measuring, "off does not re-enable them");
}

#[test]
fn animating_forces_assembled_layout() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    assert_eq!(t.effective_layout(2, false), PL::Exploded);
    t.set_animating(true);
    assert_eq!(t.effective_layout(2, false), PL::Assembled);
    assert_eq!(t.effective_layout(2, true), PL::Assembled);
    t.set_animating(false);
    assert_eq!(
        t.effective_layout(2, false),
        PL::Exploded,
        "layout intent survives"
    );
    assert_eq!(t.layout, PL::Exploded);
}

#[test]
fn motion_gate_needs_study_and_two_parts() {
    // motion_available takes &self (like the pane's other gates), so no mut
    let t = ViewerTools::new(PL::Exploded, false);
    assert!(
        !t.motion_available(2, false),
        "no study: the ask button shows instead"
    );
    assert!(t.motion_available(2, true));
    assert!(
        !t.motion_available(1, true),
        "single-part model never animates (ModelPane.swift:13-15)"
    );
}
