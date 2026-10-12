use engine::project::manifest::{BBox, Manifest};
use std::path::PathBuf;

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
}

#[test]
fn golden_manifest_decodes() {
    let m = Manifest::load(&golden().join("manifest.json")).unwrap();
    assert_eq!(m.parameters.get("WALL").copied(), Some(2.0));
    assert_eq!(m.parts.len(), 2);
    assert_eq!(m.parts[0].name, "base");
    assert_eq!(m.parts[0].quantity, 1);
    assert_eq!(m.bed, Some([220.0, 220.0]));
    assert_eq!(m.drawings.len(), 2);
    assert_eq!(m.drawings[1].file, "schematic-2.svg");
    assert_eq!(m.bounding_box, Some(BBox { x: 60.0, y: 60.0, z: 26.0 }));
    assert_eq!(m.warnings, vec!["fixture warning".to_string()]);
}

#[test]
fn malformed_fields_drop_not_error() {
    // upstream wraps every field in try? ?? default (Manifest.swift):
    // a bad value kills the FIELD, never the file
    let m: Manifest = serde_json::from_str(r#"{
        "parameters": {"a": 1.5, "b": "not-a-number", "c": true},
        "bed": [220.0, "oops"],
        "warnings": null,
        "drawings": [{"file": 7, "title": "ok"}, {"file": "s.svg", "title": "fine"}],
        "bounding_box": {"x": "wide"}
    }"#).unwrap();
    assert_eq!(m.parameters.get("a").copied(), Some(1.5));
    assert!(!m.parameters.contains_key("b"));
    assert!(!m.parameters.contains_key("c"));
    assert_eq!(m.bed, None, "non-numeric bed entry drops the field");
    assert_eq!(m.drawings.len(), 1, "malformed drawing entry skipped");
    assert_eq!(m.drawings[0].file, "s.svg");
    assert_eq!(m.bounding_box, None);
    assert!(m.warnings.is_empty());
}

#[test]
fn bed_arity_is_exactly_two_positive() {
    for bad in [r#"{"bed": [220.0]}"#, r#"{"bed": [1.0, 2.0, 3.0]}"#, r#"{"bed": [220.0, 0.0]}"#] {
        let m: Manifest = serde_json::from_str(bad).unwrap();
        assert_eq!(m.bed, None);
    }
    let ok: Manifest = serde_json::from_str(r#"{"bed": [220.0, 220.0]}"#).unwrap();
    assert_eq!(ok.bed, Some([220.0, 220.0]));
}
