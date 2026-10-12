//! The seven upstream `missingRequirements` branches (vendored
//! Project.swift:221-243), each with its precondition actually engineered
//! on disk. The notes are exact strings the agent-facing prompt shows.

use engine::project::manifest::Manifest;
use engine::project::outputs::{
    expected_outputs, missing_requirements, pdf_page_count, MissingCtx,
};
use std::path::PathBuf;

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
}
fn fixture(p: &str) -> PathBuf {
    golden().join(p)
}

/// The two directories every project holds (upstream `ProjectFolder`: the
/// root and its `output` subdir), borrowed into each [`MissingCtx`].
struct Project {
    root: PathBuf,
    output: PathBuf,
}

/// A project with the real on-disk layout (contract §85): `model.py` at the
/// project root, everything the script produces under `output/` — here all
/// seven expected outputs, the plate and two drawing pages (schematic.svg +
/// schematic-2.svg), the state in which `missing_requirements` reports
/// nothing.
fn complete_project() -> Project {
    let root = std::env::temp_dir().join(format!("cad-outputs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    std::fs::write(root.join("model.py"), "# model\n").unwrap();
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
        std::fs::copy(golden().join(f), root.join("output").join(f)).unwrap();
    }
    Project { output: root.join("output"), root }
}

fn manifest_at(p: &Project) -> Manifest {
    Manifest::load(&p.output.join("manifest.json")).unwrap()
}

fn ctx<'a>(
    p: &'a Project,
    parts: usize,
    printer: Option<&'a str>,
    man: Option<&'a Manifest>,
) -> MissingCtx<'a> {
    MissingCtx {
        project_root: &p.root,
        output_dir: &p.output,
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
    let p = complete_project();
    let man = manifest_at(&p);
    assert!(missing_requirements(&ctx(&p, 2, Some("Bambu Lab P1S"), Some(&man))).is_empty());
}

#[test]
fn no_model_py_means_no_note() {
    // The script gate comes before every other branch: a project that has
    // no model.py yet (here: a root that doesn't even exist) gets no notes.
    let p = Project {
        root: PathBuf::from("/nonexistent"),
        output: PathBuf::from("/nonexistent/output"),
    };
    let missing = missing_requirements(&ctx(&p, 2, Some("P1S"), None));
    assert!(missing.is_empty(), "no model.py -> no note, got: {missing:?}");
}

#[test]
fn missing_or_unparseable_manifest_demands_no_drawings_or_bed() {
    // Upstream sits both manifest notes inside `if let manifest` — when
    // manifest.json is missing or unparseable the caller passes None (a
    // default() manifest must never stand in) and neither `drawings` nor
    // `bed` is demanded, even with several drawing pages and a printer.
    let p = complete_project(); // 2 drawing pages on disk
    let missing = missing_requirements(&ctx(&p, 2, Some("P1S"), None));
    assert!(
        missing.is_empty(),
        "no manifest -> no manifest notes, got: {missing:?}"
    );
}

#[test]
fn missing_expected_outputs_and_plate_are_bare_names() {
    let p = complete_project();
    std::fs::remove_file(p.output.join("model.step")).unwrap();
    std::fs::remove_file(p.output.join("plate.3mf")).unwrap();
    let man = manifest_at(&p);
    let missing = missing_requirements(&ctx(&p, 2, Some("P1S"), Some(&man)));
    assert_eq!(
        missing,
        ["model.step".to_string(), "plate.3mf (print plate)".to_string()]
    );
}

#[test]
fn per_piece_page_demanded_only_with_parts_and_only_page_two() {
    let p = complete_project();
    std::fs::remove_file(p.output.join("schematic-2.svg")).unwrap();
    let man = manifest_at(&p);
    // two pieces, page 2 absent -> demanded; with the drawing page gone the
    // disk holds a single page, so no other branch can fire alongside it
    let missing = missing_requirements(&ctx(&p, 2, Some("P1S"), Some(&man)));
    assert_eq!(
        missing,
        ["one drawing page per piece (schematic-2.svg, …) with the parts list on page 1"
            .to_string()]
    );
    // single piece -> not demanded
    let missing1 = missing_requirements(&ctx(&p, 1, Some("P1S"), Some(&man)));
    assert!(missing1.is_empty(), "parts_count 1 -> no per-piece note, got: {missing1:?}");
}

#[test]
fn drawings_demanded_only_when_disk_has_multiple_pages_and_manifest_lacks_them() {
    let p = complete_project(); // disk: schematic.svg + schematic-2.svg
    let mut man = manifest_at(&p);
    man.drawings.clear();
    let missing = missing_requirements(&ctx(&p, 2, Some("P1S"), Some(&man)));
    assert!(
        missing.iter().any(|s| s.contains("`drawings` in manifest.json")),
        "{missing:?}"
    );
    // single disk page -> not demanded even with empty manifest drawings
    std::fs::remove_file(p.output.join("schematic-2.svg")).unwrap();
    let missing = missing_requirements(&ctx(&p, 2, Some("P1S"), Some(&man)));
    assert!(
        !missing.iter().any(|s| s.contains("`drawings` in manifest.json")),
        "{missing:?}"
    );
}

#[test]
fn bed_demanded_only_with_a_configured_printer() {
    let p = complete_project();
    let mut man = manifest_at(&p);
    man.bed = None;
    let with_printer = missing_requirements(&ctx(&p, 2, Some("P1S"), Some(&man)));
    assert!(with_printer.iter().any(|s| s.contains("`bed` in manifest.json")));
    let no_printer = missing_requirements(&ctx(&p, 2, None, Some(&man)));
    assert!(!no_printer.iter().any(|s| s.contains("`bed` in manifest.json")));
}

#[test]
fn pdf_demanded_when_fewer_pages_than_drawing_pages() {
    let p = complete_project(); // 2 drawing pages on disk
    std::fs::copy(fixture("../handmade/one-page.pdf"), p.output.join("schematic.pdf")).unwrap();
    let man = manifest_at(&p);
    let missing = missing_requirements(&ctx(&p, 2, Some("P1S"), Some(&man)));
    assert!(
        missing.iter().any(|s| s.contains("schematic.pdf with all drawing pages")),
        "{missing:?}"
    );
}

#[test]
fn unreadable_pdf_counts_as_zero_pages() {
    // upstream `PDFDocument(url:)?.pageCount ?? 0`: a missing schematic.pdf
    // reads as 0 pages, so the merged-pdf note fires alongside the bare-name
    // note from the expected-outputs list
    let p = complete_project();
    std::fs::remove_file(p.output.join("schematic.pdf")).unwrap();
    let man = manifest_at(&p);
    let missing = missing_requirements(&ctx(&p, 2, Some("P1S"), Some(&man)));
    assert!(
        missing.iter().any(|s| s.contains("schematic.pdf with all drawing pages")),
        "{missing:?}"
    );
    assert!(missing.contains(&"schematic.pdf".to_string()), "{missing:?}");
}
