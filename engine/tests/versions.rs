//! versions.rs tests: version.json on-disk shape, record's copy
//! set, restore's swap semantics, change detection, comparison parameters,
//! and the missing-files record gate.

use std::collections::BTreeMap;

use engine::project::outputs::missing_files;
use engine::project::versions::{
    ProjectVersion, comparison_parameters, has_changes, load_versions, next_number, record, restore,
};

fn project() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("cad-vers-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    std::fs::create_dir_all(root.join(".cadstudio")).unwrap();
    std::fs::write(root.join("model.py"), "wall = 2\n").unwrap();
    std::fs::write(
        root.join("output/manifest.json"),
        r#"{"parameters": {"wall": 2.0}, "warnings": []}"#,
    )
    .unwrap();
    std::fs::write(root.join(".cadstudio/chat.json"), "[]").unwrap();
    root
}

fn at(number: i64) -> ProjectVersion {
    ProjectVersion {
        number,
        created_at: time::OffsetDateTime::from_unix_timestamp(1_763_000_000).unwrap(),
        prompt: "a 20 mm cube".into(),
        session_id: (number > 1).then(|| format!("sid-{number}")),
    }
}

#[test]
fn version_json_round_trips_upstream_keys() {
    let root = project();
    let mut versions = vec![];
    let v = record(
        &root,
        &mut versions,
        "prompt",
        Some("sid-1"),
        time::OffsetDateTime::from_unix_timestamp(1_763_000_000).unwrap(),
    )
    .unwrap();
    assert_eq!(v.number, 1);
    let json = std::fs::read_to_string(
        engine::project::folder::versions_dir(&root).join("1/version.json"),
    )
    .unwrap();
    for key in [
        "\"number\"",
        "\"created_at\"",
        "\"prompt\"",
        "\"session_id\"",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
    assert_eq!(load_versions(&root), vec![v]);
}

#[test]
fn record_copies_the_full_snapshot_set() {
    let root = project();
    std::fs::write(root.join("output/model.3mf"), b"3mf").unwrap();
    std::fs::create_dir_all(root.join("output/nested")).unwrap();
    std::fs::write(root.join("output/nested/extra.txt"), b"x").unwrap();
    let mut versions = vec![];
    record(
        &root,
        &mut versions,
        "p",
        None,
        time::OffsetDateTime::now_utc(),
    )
    .unwrap();
    let dir = engine::project::folder::versions_dir(&root).join("1");
    assert_eq!(
        std::fs::read_to_string(dir.join("model.py")).unwrap(),
        "wall = 2\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("output/model.3mf")).unwrap(),
        "3mf"
    );
    assert!(dir.join("output/nested/extra.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dir.join("chat.json")).unwrap(),
        "[]"
    );
}

#[test]
fn record_without_model_or_chat_skips_those_files() {
    let root = project();
    std::fs::remove_file(root.join("model.py")).unwrap();
    std::fs::remove_file(root.join(".cadstudio/chat.json")).unwrap();
    let mut versions = vec![];
    record(
        &root,
        &mut versions,
        "p",
        None,
        time::OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(
        !engine::project::folder::versions_dir(&root)
            .join("1/model.py")
            .exists()
    );
    assert!(
        !engine::project::folder::versions_dir(&root)
            .join("1/chat.json")
            .exists()
    );
    assert!(
        engine::project::folder::versions_dir(&root)
            .join("1/version.json")
            .exists()
    );
}

#[test]
fn restore_swaps_everything_and_clears_output() {
    let root = project();
    let mut versions = vec![];
    let v1 = record(
        &root,
        &mut versions,
        "v1",
        Some("sid-1"),
        time::OffsetDateTime::now_utc(),
    )
    .unwrap();
    std::fs::write(root.join("model.py"), "wall = 9\n").unwrap();
    std::fs::write(root.join("output/stale.txt"), b"stale").unwrap();
    std::fs::remove_file(root.join("output/manifest.json")).unwrap();
    let _v2 = record(
        &root,
        &mut versions,
        "v2",
        Some("sid-2"),
        time::OffsetDateTime::now_utc(),
    )
    .unwrap();
    restore(&root, &v1).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 2\n"
    );
    assert!(root.join("output/manifest.json").exists());
    assert!(!root.join("output/stale.txt").exists()); // v2's file gone with v2's output
    // current chat.json is the version's copy (byte-identical here; the
    // caller reloads state from disk)
    assert!(root.join(".cadstudio/chat.json").exists());
    // missing version output dir: tolerated, output/ simply emptied
    let orphan = at(9);
    std::fs::create_dir_all(engine::project::folder::versions_dir(&root).join("9")).unwrap();
    std::fs::write(
        engine::project::folder::versions_dir(&root).join("9/version.json"),
        serde_json::to_string(&orphan).unwrap(),
    )
    .unwrap();
    restore(&root, &orphan).unwrap();
    assert!(root.join("output").read_dir().unwrap().count() == 0);
}

#[test]
fn has_changes_follows_model_py_bytes() {
    let root = project();
    let mut versions = vec![];
    record(
        &root,
        &mut versions,
        "p",
        None,
        time::OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(!has_changes(&root, &versions, Some(1)));
    std::fs::write(root.join("model.py"), "wall = 3\n").unwrap();
    assert!(has_changes(&root, &versions, Some(1)));
    assert!(has_changes(&root, &versions, None)); // no current version
    assert!(has_changes(&root, &versions, Some(42))); // unknown current
}

#[test]
fn next_number_is_last_plus_one() {
    assert_eq!(next_number(&[]), 1);
    let vs = vec![at(1), at(3)];
    assert_eq!(next_number(&vs), 4);
}

#[test]
fn comparison_parameters_parse_the_versions_model_copy() {
    let root = project();
    // the manifest disagrees with the script on purpose: a slider-style
    // rewrite leaves the recorded manifest stale, and only the version's
    // model.py copy holds the value the version was recorded with — a
    // manifest-reading comparison must fail here
    std::fs::write(
        root.join("output/manifest.json"),
        r#"{"parameters": {"wall": 7.0}, "warnings": []}"#,
    )
    .unwrap();
    let mut versions = vec![];
    record(
        &root,
        &mut versions,
        "p",
        None,
        time::OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(
        comparison_parameters(&root, Some(1)),
        BTreeMap::from([("wall".to_string(), 2.0)])
    );
    assert!(comparison_parameters(&root, None).is_empty());
    assert!(comparison_parameters(&root, Some(77)).is_empty());
}

#[test]
fn missing_files_lists_expected_outputs_in_order() {
    let root = project();
    assert_eq!(
        missing_files(&root.join("output")),
        [
            "model.3mf",
            "model.stl",
            "model.step",
            "schematic.svg",
            "schematic.png",
            "schematic.pdf"
        ]
    ); // manifest.json exists
    std::fs::remove_file(root.join("output/manifest.json")).unwrap();
    let all = missing_files(&root.join("output"));
    assert_eq!(all.last().map(String::as_str), Some("manifest.json"));
}
