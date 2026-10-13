use engine::parsing::animation::MotionStudy;
use std::path::PathBuf;

fn golden() -> std::path::PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden/animation.json")
}

#[test]
fn golden_parses_sorted_with_three_frames() {
    let s = MotionStudy::parse(&std::fs::read(golden()).unwrap()).unwrap();
    assert_eq!(s.title.as_deref(), Some("lid lift"));
    assert_eq!(s.frames.len(), 3);
    assert!(
        s.frames.windows(2).all(|w| w[0].t <= w[1].t),
        "sorted on load"
    );
    assert_eq!(s.duration(), 2.0);
    assert_eq!(s.value_names(), vec!["Lift (mm)"]);
}

#[test]
fn malformed_entries_default_to_identity() {
    let s = MotionStudy::parse(
        br#"{"frames":[
        {"t": 1.0, "parts": {"a": {"rotate": [0.0, 2.0, 0.0, 2.0]}}},
        {"t": 0.0, "parts": {"a": {"translate": [1.0, 2.0]}}}
    ]}"#,
    )
    .unwrap();
    assert_eq!(s.frames[0].t, 0.0, "sorted");
    let a0 = &s.frames[0].parts["a"];
    assert_eq!(
        a0.translate,
        [0.0, 0.0, 0.0],
        "wrong-arity translate -> zero (upstream), not padded"
    );
    assert_eq!(a0.rotate, glam::Quat::IDENTITY);
    let a1 = &s.frames[1].parts["a"];
    let r = a1.rotate; // [0,2,0,2] -> normalized [0, √2/2, 0, √2/2]
    assert!(
        (r.y - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
        "NON-UNIT input normalized (length was 2√2)"
    );
    assert!((r.w - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
}

#[test]
fn non_numeric_array_entry_zeros_the_whole_field() {
    // Upstream decodes [Float] all-or-nothing (try? ?? []): one non-number
    // drops the whole array, so the arity gate sees nothing and the pose is
    // zero/identity. Filter-mapping entries instead would read [1,2,4].
    let s = MotionStudy::parse(
        br#"{"frames":[
        {"t": 0.0, "parts": {
            "a": {"translate": [1.0, 2.0, "x", 4.0]},
            "b": {"rotate": [1.0, "x", 2.0, 3.0]}
        }}
    ]}"#,
    )
    .unwrap();
    let f = &s.frames[0];
    assert_eq!(
        f.parts["a"].translate,
        [0.0, 0.0, 0.0],
        "one bad entry nulls the whole translate"
    );
    assert_eq!(
        f.parts["b"].rotate,
        glam::Quat::IDENTITY,
        "one bad entry nulls the whole rotate"
    );
}

#[test]
fn interpolation_lerps_translates_and_slerps_rotations() {
    let s = MotionStudy::parse(
        br#"{"frames":[
        {"t": 0.0, "parts": {"lid": {"translate": [0.0, 0.0, 0.0]}},
         "values": {"Lift (mm)": 0.0}, "collisions": []},
        {"t": 2.0, "parts": {"lid": {"translate": [0.0, 0.0, 10.0]}},
         "values": {"Lift (mm)": 10.0},
         "collisions": [{"parts": ["base", "lid"], "volume": 0.5}]}
    ]}"#,
    )
    .unwrap();
    let mid = s.state_at(1.0);
    assert_eq!(mid.parts["lid"].translate, [0.0, 0.0, 5.0]);
    assert!((mid.values["Lift (mm)"] - 5.0).abs() < 1e-9);
    // collisions come from the nearest frame (t=1.0 is equidistant -> the later one)
    assert_eq!(mid.collisions.len(), 1);
    assert_eq!(
        mid.collisions[0].parts,
        vec!["base".to_string(), "lid".to_string()]
    );
    let before = s.state_at(-1.0);
    assert_eq!(before.parts["lid"].translate, [0.0, 0.0, 0.0]);
    let after = s.state_at(9.0);
    assert_eq!(after.parts["lid"].translate, [0.0, 0.0, 10.0]);
}

#[test]
fn slerp_quaternions_halfway() {
    let s = MotionStudy::parse(
        br#"{"frames":[
        {"t": 0.0, "parts": {"p": {"rotate": [0.0, 0.0, 0.0, 1.0]}}},
        {"t": 1.0, "parts": {"p": {"rotate": [0.0, 0.0, 1.0, 0.0]}}}
    ]}"#,
    )
    .unwrap();
    let mid = s.state_at(0.5);
    let r = mid.parts["p"].rotate;
    assert!((r.z - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    assert!((r.w - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
}

#[test]
fn label_persists_through_unlabeled_frames() {
    let s = MotionStudy::parse(
        br#"{"frames":[
        {"t": 0.0, "label": "closed"},
        {"t": 1.0},
        {"t": 2.0, "label": "open"}
    ]}"#,
    )
    .unwrap();
    assert_eq!(s.state_at(0.5).label.as_deref(), Some("closed"));
    assert_eq!(
        s.state_at(1.5).label.as_deref(),
        Some("closed"),
        "unlabeled frame does NOT clear"
    );
    assert_eq!(s.state_at(2.0).label.as_deref(), Some("open"));
}

#[test]
fn part_missing_from_one_frame_holds_its_pose() {
    let s = MotionStudy::parse(
        br#"{"frames":[
        {"t": 0.0, "parts": {"lid": {"translate": [0.0, 0.0, 0.0]}}},
        {"t": 2.0, "parts": {"lid": {"translate": [0.0, 0.0, 10.0]},
                             "cam": {"translate": [1.0, 0.0, 0.0]}}}
    ]}"#,
    )
    .unwrap();
    let mid = s.state_at(1.0);
    assert_eq!(
        mid.parts["cam"].translate,
        [1.0, 0.0, 0.0],
        "part only in the later frame holds that pose across the span"
    );
    assert_eq!(mid.parts["lid"].translate, [0.0, 0.0, 5.0]);
}

#[test]
fn single_frame_study_is_inert() {
    let s = MotionStudy::parse(br#"{"frames":[{"t": 0.0}]}"#).unwrap();
    assert!(!s.is_active());
}
