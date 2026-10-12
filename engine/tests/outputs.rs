//! The seven upstream `missingRequirements` branches (vendored
//! Project.swift:221-243), each with its precondition actually engineered
//! on disk. The notes are exact strings the agent-facing prompt shows.

use engine::project::manifest::Manifest;
use engine::project::outputs::{
    expected_outputs, missing_requirements, pdf_page_count, MissingCtx,
};
use std::path::{Path, PathBuf};

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
}
fn fixture(p: &str) -> PathBuf {
    golden().join(p)
}

/// A project root holding model.py, all seven expected outputs, the plate
/// and two drawing pages (schematic.svg + schematic-2.svg): the state in
/// which `missing_requirements` reports nothing.
fn complete_project() -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("cad-outputs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("model.py"), "# model\n").unwrap();
    for f in [
        "model.3mf",
        "model.stl",
        "model.step",
        "schematic.svg",
        "schematic.png",
        "schematic.pdf",
        "manifest.json",
        "schematic-2.svg",
        "plate.3mf",
    ] {
        std::fs::copy(golden().join(f), tmp.join(f)).unwrap();
    }
    tmp
}

fn manifest_at(root: &Path) -> Manifest {
    Manifest::load(&root.join("manifest.json")).unwrap()
}

fn ctx<'a>(root: &'a Path, parts: usize, printer: Option<&'a str>, man: &'a Manifest) -> MissingCtx<'a> {
    MissingCtx {
        project_root: root,
        parts_count: parts,
        printer_model: printer,
        manifest: man,
    }
}

#[test]
fn expected_outputs_list_matches_contract() {
    assert_eq!(
        expected_outputs(),
        [
            "model.3mf",
            "model.stl",
            "model.step",
            "schematic.svg",
            "schematic.png",
            "schematic.pdf",
            "manifest.json",
        ]
    );
}

#[test]
fn golden_pdf_has_two_pages_and_one_page_fixture_has_one() {
    assert_eq!(pdf_page_count(&golden().join("schematic.pdf")).unwrap(), 2);
    assert_eq!(pdf_page_count(&fixture("../handmade/one-page.pdf")).unwrap(), 1);
}

#[test]
fn complete_project_reports_nothing() {
    let root = complete_project();
    let man = manifest_at(&root);
    assert!(missing_requirements(&ctx(&root, 2, Some("Bambu Lab P1S"), &man)).is_empty());
}

#[test]
fn no_model_py_means_no_note() {
    // The script gate comes before every other branch: a project that has
    // no model.py yet (here: a root that doesn't even exist) gets no notes.
    let man = manifest_at(&complete_project());
    let missing = missing_requirements(&ctx(Path::new("/nonexistent"), 2, Some("P1S"), &man));
    assert!(missing.is_empty(), "no model.py -> no note, got: {missing:?}");
}

#[test]
fn missing_expected_outputs_and_plate_are_bare_names() {
    let root = complete_project();
    std::fs::remove_file(root.join("model.step")).unwrap();
    std::fs::remove_file(root.join("plate.3mf")).unwrap();
    let man = manifest_at(&root);
    let missing = missing_requirements(&ctx(&root, 2, Some("P1S"), &man));
    assert_eq!(
        missing,
        ["model.step".to_string(), "plate.3mf (print plate)".to_string()]
    );
}

#[test]
fn per_piece_page_demanded_only_with_parts_and_only_page_two() {
    let root = complete_project();
    std::fs::remove_file(root.join("schematic-2.svg")).unwrap();
    let man = manifest_at(&root);
    // two pieces, page 2 absent -> demanded; with the drawing page gone the
    // disk holds a single page, so no other branch can fire alongside it
    let missing = missing_requirements(&ctx(&root, 2, Some("P1S"), &man));
    assert_eq!(
        missing,
        ["one drawing page per piece (schematic-2.svg, …) with the parts list on page 1"
            .to_string()]
    );
    // single piece -> not demanded
    let missing1 = missing_requirements(&ctx(&root, 1, Some("P1S"), &man));
    assert!(missing1.is_empty(), "parts_count 1 -> no per-piece note, got: {missing1:?}");
}

#[test]
fn drawings_demanded_only_when_disk_has_multiple_pages_and_manifest_lacks_them() {
    let root = complete_project(); // disk: schematic.svg + schematic-2.svg
    let mut man = manifest_at(&root);
    man.drawings.clear();
    let missing = missing_requirements(&ctx(&root, 2, Some("P1S"), &man));
    assert!(
        missing.iter().any(|s| s.contains("`drawings` in manifest.json")),
        "{missing:?}"
    );
    // single disk page -> not demanded even with empty manifest drawings
    std::fs::remove_file(root.join("schematic-2.svg")).unwrap();
    let missing = missing_requirements(&ctx(&root, 2, Some("P1S"), &man));
    assert!(
        !missing.iter().any(|s| s.contains("`drawings` in manifest.json")),
        "{missing:?}"
    );
}

#[test]
fn bed_demanded_only_with_a_configured_printer() {
    let root = complete_project();
    let mut man = manifest_at(&root);
    man.bed = None;
    let with_printer = missing_requirements(&ctx(&root, 2, Some("P1S"), &man));
    assert!(with_printer.iter().any(|s| s.contains("`bed` in manifest.json")));
    let no_printer = missing_requirements(&ctx(&root, 2, None, &man));
    assert!(!no_printer.iter().any(|s| s.contains("`bed` in manifest.json")));
}

#[test]
fn pdf_demanded_when_fewer_pages_than_drawing_pages() {
    let root = complete_project(); // 2 drawing pages on disk
    std::fs::copy(fixture("../handmade/one-page.pdf"), root.join("schematic.pdf")).unwrap();
    let man = manifest_at(&root);
    let missing = missing_requirements(&ctx(&root, 2, Some("P1S"), &man));
    assert!(
        missing.iter().any(|s| s.contains("schematic.pdf with all drawing pages")),
        "{missing:?}"
    );
}
