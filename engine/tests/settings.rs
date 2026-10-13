use engine::settings::{MeasurementUnit, PartsLayout, PrinterType, Settings};
use std::path::PathBuf;

fn tmp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cad-settings-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn defaults_match_upstream() {
    let s = Settings::default();
    assert_eq!(s.agent_engine, "claude");
    assert!(!s.restricted_bash);
    assert!(s.streaming_output);
    assert!(s.isolates_claude);
    assert_eq!(s.measurement_unit, MeasurementUnit::Millimeters);
    assert_eq!(s.printer_type, PrinterType::Fdm);
    assert!(!s.has_completed_setup);
    assert!(s.recent_projects.is_empty());
    assert_eq!(s.default_project_location, documents_dir());
    assert_eq!(
        s.parts_layout,
        PartsLayout::Exploded,
        "upstream default ?? .exploded"
    );
}

fn documents_dir() -> PathBuf {
    directories::UserDirs::new()
        .unwrap()
        .document_dir()
        .unwrap()
        .to_path_buf()
}

#[test]
fn json_uses_upstream_key_names() {
    let mut s = Settings::default();
    s.printer_model = Some("Bambu Lab P1S".into());
    s.recent_projects.push("/tmp/p1".into());
    let json = serde_json::to_string(&s).unwrap();
    for key in [
        "defaultProjectLocation",
        "agentEngine",
        "claudePath",
        "codexPath",
        "claudeModel",
        "codexModel",
        "claudeEffort",
        "restrictedBash",
        "streamingOutput",
        "isolatesClaude",
        "measurementUnit",
        "printerType",
        "printerModel",
        "hasCompletedSetup",
        "defaultLayout",
        "recentProjects",
        "partsLayout",
        "partsCollapsed",
        "parametersCollapsed",
    ] {
        assert!(json.contains(&format!("\"{key}\"")), "missing key {key}");
    }
    assert!(
        json.contains("\"millimeters\""),
        "units store upstream rawValues"
    );
    assert!(json.contains("\"exploded\""));
    let back: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.printer_model.as_deref(), Some("Bambu Lab P1S"));
}

#[test]
fn load_tolerates_unknown_and_invalid_values() {
    let d = tmp_dir();
    let p = d.join("settings.json");
    std::fs::write(
        &p,
        "{\"agentEngine\":\"codex\",\"measurementUnit\":\"furlongs\",\"futureKey\":42}",
    )
    .unwrap();
    let s = Settings::load(&p).unwrap();
    assert_eq!(s.agent_engine, "codex");
    assert_eq!(
        s.measurement_unit,
        MeasurementUnit::Millimeters,
        "unknown enum falls back, load never fails"
    );
    // upstream reads every key with ?? default: unknown enum rawValues and
    // non-string values (null) fall back per key instead of failing the load
    std::fs::write(
        &p,
        "{\"printerType\":\"resinXL\",\"partsLayout\":\"diagonal\",\"measurementUnit\":null}",
    )
    .unwrap();
    let s = Settings::load(&p).unwrap();
    assert_eq!(
        s.printer_type,
        PrinterType::Fdm,
        "unknown printerType -> default"
    );
    assert_eq!(
        s.parts_layout,
        PartsLayout::Exploded,
        "unknown partsLayout -> default"
    );
    assert_eq!(
        s.measurement_unit,
        MeasurementUnit::Millimeters,
        "null -> default"
    );
    assert!(Settings::load(&d.join("absent.json")).unwrap().agent_engine == "claude");
}

#[test]
fn save_is_atomic_and_creates_parents() {
    let d = tmp_dir();
    let p = d.join("a/b/settings.json");
    Settings::default().save(&p).unwrap();
    assert!(p.exists());
    let mut n = 0;
    for e in std::fs::read_dir(d.join("a/b")).unwrap() {
        if e.unwrap().file_name().to_string_lossy().ends_with(".tmp") {
            n += 1;
        }
    }
    assert_eq!(n, 0, "temp residue left behind");
}

#[test]
fn settings_path_is_lowercase_hyphenated() {
    let p = engine::settings::settings_path();
    assert!(
        p.ends_with("cad-studio/settings.json"),
        "got {p:?} — directories lowercases AND strips spaces"
    );
}

#[test]
fn units_convert_like_upstream() {
    assert_eq!(MeasurementUnit::Millimeters.mm_per_unit(), 1.0);
    assert_eq!(MeasurementUnit::Centimeters.mm_per_unit(), 10.0);
    assert_eq!(MeasurementUnit::Meters.mm_per_unit(), 1000.0);
    assert!((MeasurementUnit::Inches.mm_per_unit() - 25.4).abs() < f64::EPSILON);
    assert_eq!(MeasurementUnit::Millimeters.fraction_digits(), 1);
    assert_eq!(MeasurementUnit::Centimeters.fraction_digits(), 2);
    assert_eq!(MeasurementUnit::Inches.fraction_digits(), 2);
    assert_eq!(
        MeasurementUnit::Millimeters.prompt_name(),
        "millimeters (mm)"
    );
}

#[test]
fn print_guidelines_differ_per_printer_type() {
    let fdm = PrinterType::Fdm.prompt_guidelines();
    let resin = PrinterType::Resin.prompt_guidelines();
    assert!(fdm.contains("1.2"));
    assert!(resin.contains("0.8"));
    assert_ne!(fdm, resin);
}
