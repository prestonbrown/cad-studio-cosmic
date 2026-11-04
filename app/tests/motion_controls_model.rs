use app::motion_controls::{collision_text, format_significant, speed_label, time_readout};
use engine::parsing::animation::Collision;

#[test]
fn time_readout_is_two_fraction_digits() {
    assert_eq!(time_readout(0.0, 2.0), "0.00 / 2.00 s");
    assert_eq!(time_readout(1.25, 2.0), "1.25 / 2.00 s");
}

#[test]
fn significant_digits_match_the_two_swift_shapes() {
    // values row (1...3): MotionControls.swift:58
    assert_eq!(format_significant(10.0, 1, 3), "10");
    assert_eq!(format_significant(0.5, 1, 3), "0.5");
    assert_eq!(format_significant(3.14159, 1, 3), "3.14");
    assert_eq!(format_significant(1234.5, 1, 3), "1,230", "grouped, 3 sig");
    assert_eq!(format_significant(0.0, 1, 3), "0");
    // volume (1...2): MotionControls.swift:86
    assert_eq!(format_significant(0.5, 1, 2), "0.5");
    assert_eq!(format_significant(12.345, 1, 2), "12");
    assert_eq!(
        format_significant(1.0, 2, 2),
        "1.0",
        "min binds: keep the .0"
    );
}

/// Exact decimal ties round HALF-UP like Swift/ICU
/// (`.precision(.significantDigits(…))`), where Rust's `{:.N}` rounds ties
/// to even: 0.125 at two digits is "0.13", never "0.12"; the integer cut
/// (magnitude == max digits) ties the same way (998.5 → "999").
#[test]
fn exact_ties_round_half_up_like_icu() {
    assert_eq!(format_significant(0.125, 1, 2), "0.13");
    assert_eq!(format_significant(0.625, 1, 2), "0.63");
    assert_eq!(format_significant(998.5, 1, 3), "999");
}

#[test]
fn speed_labels_are_shortest_display() {
    for (speed, label) in [
        (0.1, "0.1×"),
        (0.25, "0.25×"),
        (0.5, "0.5×"),
        (1.0, "1×"),
        (2.0, "2×"),
    ] {
        assert_eq!(speed_label(speed), label);
    }
}

#[test]
fn collision_text_pairs_volumes_and_joins() {
    let one = Collision {
        parts: vec!["base".into(), "lid".into()],
        volume: Some(0.5),
    };
    assert_eq!(collision_text(&[one.clone()]), "base ↔ lid 0.5 mm³");
    let unmeasured = Collision {
        parts: vec!["cam".into(), "lid".into()],
        volume: None,
    };
    assert_eq!(
        collision_text(&[unmeasured.clone()]),
        "cam ↔ lid",
        "no volume: the pair alone"
    );
    assert_eq!(
        collision_text(&[one, unmeasured]),
        "base ↔ lid 0.5 mm³, cam ↔ lid"
    );
}

#[test]
fn values_render_in_first_appearance_order() {
    // a synthetic study whose total order is NOT alphabetical: within a
    // frame the engine iterates a sorted BTreeMap, then appends each
    // frame's new names — so Mid (frame 2; sorts before Zed) trails Zed
    let study = engine::parsing::animation::MotionStudy::parse(
        br#"{"frames":[
            {"t":0,"values":{"Zed":1.0,"Alpha":2.0}},
            {"t":1,"values":{"Mid":3.0,"Alpha":2.5}}
        ]}"#,
    )
    .unwrap();
    assert_eq!(
        study.value_names(),
        ["Alpha", "Zed", "Mid"],
        "engine order: sorted within a frame, first-appearance across frames"
    );
    // at t=0.5 every name is interpolated into the state: value_names order
    // (Alpha, Zed, Mid) and BTreeMap order (Alpha, Mid, Zed) differ exactly
    // here — at t=0 Mid is absent and the two orders coincide
    let state = study.state_at(0.5);
    let rendered: Vec<&str> = study
        .value_names()
        .iter()
        .filter_map(|n| state.values.get(*n).map(|_| *n))
        .collect();
    assert_eq!(
        rendered,
        ["Alpha", "Zed", "Mid"],
        "the controls must iterate value_names(), never the BTreeMap"
    );
}

/// The speed menu lists every speed then Ripeti, checks the current speed
/// and an on loop, and each entry sends its own message.
#[test]
fn speed_menu_holds_the_speeds_then_ripeti() {
    use app::motion_controls::{speed_menu_items, speed_menu_pick};
    use app::viewer_pane::Message;
    use viewer::playback::SPEEDS;
    let items = speed_menu_items(1.0, true);
    assert_eq!(items.len(), SPEEDS.len() + 1);
    let checked: Vec<usize> = (0..items.len())
        .filter(|i| items[*i].starts_with('✓'))
        .collect();
    let one = SPEEDS.iter().position(|s| *s == 1.0).unwrap();
    assert_eq!(checked, [one, SPEEDS.len()]);
    assert!(items[one].ends_with(&speed_label(1.0)));
    let off = speed_menu_items(1.0, false);
    assert!(!off[SPEEDS.len()].starts_with('✓'));
    for (index, speed) in SPEEDS.iter().enumerate() {
        assert!(matches!(speed_menu_pick(index), Message::SetSpeed(s) if s == *speed));
    }
    assert!(matches!(
        speed_menu_pick(SPEEDS.len()),
        Message::ToggleLoops
    ));
}
