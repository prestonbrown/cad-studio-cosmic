//! parameters model tests: the upstream range heuristic, commit
//! rounding, no-op commits, external sync, and panel state.

use app::parameters::{ParametersPanel, SliderState};
use engine::project::parameters::ModelParameter;

fn p(name: &str, value: f64) -> ModelParameter {
    ModelParameter {
        name: name.into(),
        value,
    }
}

#[test]
fn range_heuristic_matches_upstream() {
    assert_eq!(SliderState::range_around(5.0), (0.0, 10.0)); // 2v
    assert_eq!(SliderState::range_around(0.3), (0.0, 1.0)); // max(2v, 1)
    assert_eq!(SliderState::range_around(-2.0), (-4.0, 4.0));
    assert_eq!(SliderState::range_around(-0.2), (-0.4, 1.0)); // max(-2v, 1)
    assert_eq!(SliderState::range_around(0.0), (-10.0, 10.0));
}

#[test]
fn commit_rounds_integers_and_three_decimals() {
    // The integer rule keys on the parameter's COMMITTED value (upstream
    // isInteger reads parameter.value, ParametersOverlay.swift:134-136) and
    // is applied to the slider's LIVE value.
    let s = SliderState::new(4.4); // parameter at 3 → integer rule
    assert_eq!(s.commit_value(3.0), 4.0);
    assert_eq!(s.commit(3.0), Some(4.0));
    let s = SliderState::new(4.0004); // committed 3.5 → 3-decimal rule
    assert_eq!(s.commit_value(3.5), 4.0);
    let s = SliderState::new(0.12345); // committed 0.5 → 3-decimal rule
    assert_eq!(s.commit_value(0.5), 0.123);
    let s = SliderState::new(-1.6); // committed -3 → integer rule
    assert_eq!(s.commit_value(-3.0), -2.0);
}

#[test]
fn commit_equal_to_current_is_noop() {
    let s = SliderState::new(4.0);
    assert_eq!(s.commit(4.0), None);
    let s = SliderState::new(6.0); // drag to 6 with the parameter at 5
    assert_eq!(s.commit(5.0), Some(6.0));
}

#[test]
fn sync_follows_external_value_only_when_not_editing() {
    // outside the current range → re-fit (0.3's range is 0..=1; 9 escapes it)
    let mut s = SliderState::new(0.3);
    s.sync_from(9.0);
    assert_eq!(s.value, 9.0);
    assert_eq!((s.min, s.max), (0.0, 18.0)); // re-fit around 9
    // contained → range untouched (upstream refits only on escape)
    let mut c = SliderState::new(5.0); // range 0..=10
    c.sync_from(9.0);
    assert_eq!((c.value, c.min, c.max), (9.0, 0.0, 10.0));
    c.editing = true;
    c.value = 1.0; // user dragging
    c.sync_from(9.0);
    assert_eq!(c.value, 1.0); // untouched mid-edit
}

#[test]
fn panel_builds_and_syncs_from_parameters() {
    let params = vec![p("wall", 2.0), p("height", 10.0)];
    let mut panel = ParametersPanel::from_parameters(&params, false);
    assert_eq!(panel.sliders.len(), 2);
    assert_eq!(panel.sliders[0].0, "wall");
    let params2 = vec![p("wall", 3.0), p("height", 10.0)];
    panel.sync(&params2);
    assert_eq!(panel.sliders[0].1.value, 3.0);
    // a NEW parameter appears; a removed one drops
    let params3 = vec![p("height", 10.0), p("depth", 1.0)];
    panel.sync(&params3);
    assert_eq!(
        panel
            .sliders
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["height", "depth"]
    );
}

#[test]
fn integer_rule_boundary_is_one() {
    // integral AND |v| >= 1 (ParametersOverlay.swift:134-136)
    assert!(SliderState::is_integer_value(1.0));
    assert!(SliderState::is_integer_value(-3.0));
    assert!(!SliderState::is_integer_value(0.9), "integral but |v| < 1");
    assert!(
        !SliderState::is_integer_value(3.5),
        "|v| >= 1 but fractional"
    );
    let s = SliderState::new(-1.6);
    assert_eq!(s.commit_value(-1.0), -2.0, "-1 is on the integer side");
}

#[test]
fn typed_escape_refits_range_through_sync() {
    // The text field is not range-bound: a typed 30 escapes the 0..=10
    // slider. commit returns it and sync_from re-fits — together the two
    // cover what upstream's in-commit refit plus the onChange round-trip
    // do in one step (ParametersOverlay.swift:184-187).
    let mut s = SliderState::new(5.0);
    s.value = 30.0; // typed into the field
    let v = s.commit(5.0).unwrap();
    assert_eq!(v, 30.0);
    s.sync_from(v);
    assert_eq!((s.min, s.max), (0.0, 60.0));
}

#[test]
fn display_format_trims_to_the_digit_cap() {
    // Swift .number.precision(.fractionLength(0...n)): at most n fraction
    // digits, trailing zeros trimmed (the previous value caps at 2, the
    // live field at 3).
    use app::parameters::format_display;
    assert_eq!(format_display(2.0, 2), "2");
    assert_eq!(format_display(2.5, 2), "2.5");
    assert_eq!(format_display(0.12345, 2), "0.12");
    assert_eq!(format_display(0.12345, 3), "0.123");
    assert_eq!(format_display(4.0004, 3), "4");
    assert_eq!(format_display(-0.5, 2), "-0.5");
    assert_eq!(format_display(1234.5678, 3), "1234.568");
    assert_eq!(
        format_display(100.0, 2),
        "100",
        "integer zeros are not trimmed"
    );
    assert_eq!(format_display(100.0, 0), "100");
}

#[test]
fn panel_sync_keeps_mid_drag_edits() {
    // upstream's onChange(of: parameter.value) guard: a row mid-drag keeps
    // the user's edit when an external change lands (ParametersOverlay.swift:172-178)
    let mut panel = ParametersPanel::from_parameters(&[p("wall", 2.0)], false);
    panel.sliders[0].1.editing = true;
    panel.sliders[0].1.value = 7.0; // thumb mid-drag
    panel.sync(&[p("wall", 3.0)]); // external change lands
    assert_eq!(panel.sliders[0].1.value, 7.0);
    // the drag ending settles through the commit path, and the NEXT sync
    // follows again
    panel.sliders[0].1.editing = false;
    panel.sync(&[p("wall", 3.0)]);
    assert_eq!(panel.sliders[0].1.value, 3.0);
}
