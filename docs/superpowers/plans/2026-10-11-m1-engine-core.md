# M1 Engine Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the engine crate's domain core — settings, manifest, output-requirements, full 3MF/STL/animation parsers, project store, and the output-directory watcher — each TDD'd against the committed goldens, per spec §10 M1.

**Architecture:** Pure library code, zero GUI dependencies. Types follow the upstream on-disk contracts exactly (docs/research/upstream-contract.md §3/§4/§6) so project folders stay portable between the macOS app and the port. Parsing ported from the M0 spike (engine/tests/parse3mf.rs at bf77ad6 — copy conventions from there, NOT from the M0 plan doc).

**Tech Stack:** serde/serde_json, quick-xml 0.41, zip 8.6, notify 8, lopdf, time 0.3 (RFC3339), uuid v4, directories 6, glam.

**Spec:** docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md (§4.2 module map, §10 M1). Carry-forwards: docs/research/carry-forwards.md (M1 section). Behavioral authority: docs/research/upstream-contract.md.

## Global Constraints

- engine crate has NO GUI dependencies — never libcosmic, iced, or wgpu (spec §4.2).
- Tests run on thelio via `ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && git pull -q && cargo test -p engine"'`; no network, no venv — committed fixtures only.
- On-disk formats are port-1:1: JSON field names, ISO8601 dates, and shapes per upstream-contract.md §3/§4/§6. A project folder must be portable between the macOS app and the port.
- 3MF transforms use spec token order: 12 tokens, 4 rows × 3, row-vector application, translation at tokens 9/10/11 (carry-forwards.md M1; engine/tests/parse3mf.rs at bf77ad6 is the reference implementation).
- Tolerant decoding: unknown JSON fields ignored; missing optional fields default; malformed values dropped, never fatal (upstream decoder semantics, contract §3: "non-numeric entries dropped", "missing → []", "valid only when exactly 2 values, both > 0").
- Conventional commits; stage explicit paths, never `git add -A`/`-u`/`.`.
- thelio login shell is `bash -lc` (no zsh on the host).

## Review Focus

1. **Malformed/partial manifest.json from the agent** (missing warnings/parameters, non-numeric parameter values, bed with 0/1/3 entries or non-positive values) → defaults/drops per contract, never a parse error → Task 2 tests.
2. **3MF with unit ≠ millimeter silently mis-scaled** → unit attribute scales positions (centimeter ×10, micron ×0.001…) → Task 4 `units.3mf` fixture test.
3. **animation.json with unsorted frames or non-normalized quaternions** → frames sorted on load, quaternions normalized, missing translate/rotate default to identity → Task 5 tests.
4. **inotify event storms while the agent writes many files** → one ReloadKind per actually-changed file per debounce window; unchanged files never reload → Task 8 storm test.
5. **Crash mid-write corrupting project.json/chat.json** → atomic temp-file+rename saves, no residue on failure → Task 7 atomicity test.

---

### Task 1: Workspace deps + settings module

**Files:**
- Modify: `Cargo.toml` (workspace deps), `engine/Cargo.toml`, `engine/src/lib.rs`
- Create: `engine/src/settings.rs`, `engine/tests/settings.rs`

**Interfaces:**
- Produces (used by Tasks 2, 6, 7, later M2/M5): `engine::settings::{Settings, PrinterType, MeasurementUnit, load_settings, settings_path, PRINT_GUIDELINES}` with exact shapes below. Later tasks read settings via `Settings::load_or_default(path)`.

- [ ] **Step 1: Add dependencies**

Workspace `Cargo.toml` `[workspace.dependencies]` additions:

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
directories = "6"
time = { version = "0.3", features = ["serde", "formatting", "macros"] }
uuid = { version = "1", features = ["v4", "serde"] }
notify = "8"
glam = { version = "0.33", features = ["bytemuck"] }
```

`engine/Cargo.toml` gains `[dependencies] serde, serde_json, directories, time, uuid, notify` (all `workspace = true`), and `[dev-dependencies]` keeps quick-xml/zip until Task 4 promotes them.

- [ ] **Step 2: Write failing tests** — `engine/tests/settings.rs`:

```rust
use engine::settings::{MeasurementUnit, PrinterType, Settings};
use std::io::Write;

fn tmp_dir() -> std::path::PathBuf {
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
    assert!(s.has_completed_setup == false);
    assert!(s.recent_projects.is_empty());
    assert_eq!(s.default_project_location, super_dir_documents());
}

fn super_dir_documents() -> std::path::PathBuf {
    directories::UserDirs::new().unwrap().document_dir().unwrap().to_path_buf()
}

#[test]
fn roundtrip_json_field_names() {
    let mut s = Settings::default();
    s.printer_model = Some("Bambu Lab P1S".into());
    s.recent_projects.push("/tmp/p1".into());
    let json = serde_json::to_string(&s).unwrap();
    for key in ["defaultProjectLocation", "agentEngine", "claudePath", "codexPath",
                "claudeModel", "codexModel", "claudeEffort", "restrictedBash",
                "streamingOutput", "isolatesClaude", "measurementUnit", "printerType",
                "printerModel", "hasCompletedSetup", "defaultLayout", "recentProjects",
                "partsLayout", "partsCollapsed", "parametersCollapsed"] {
        assert!(json.contains(&format!("\"{key}\"")), "missing key {key}");
    }
    let back: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.printer_model.as_deref(), Some("Bambu Lab P1S"));
    assert_eq!(back.recent_projects.len(), 1);
}

#[test]
fn load_tolerates_unknown_and_missing_fields() {
    let d = tmp_dir();
    let p = d.join("settings.json");
    std::fs::write(&p, "{\"agentEngine\":\"codex\",\"futureKey\":42}").unwrap();
    let s = Settings::load(&p).unwrap();
    assert_eq!(s.agent_engine, "codex");
    assert_eq!(s.printer_type, PrinterType::Fdm); // default held
    let s2 = Settings::load(&d.join("absent.json")).unwrap();
    assert_eq!(s2.agent_engine, "claude");
}

#[test]
fn save_is_atomic_and_creates_parents() {
    let d = tmp_dir();
    let p = d.join("a/b/settings.json");
    Settings::default().save(&p).unwrap();
    assert!(p.exists());
    assert!(d.join("a/b").is_dir());
    // no temp residue
    let mut n = 0;
    for e in std::fs::read_dir(d.join("a/b")).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        if name.ends_with(".tmp") { n += 1; }
    }
    assert_eq!(n, 0, "temp residue left behind");
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
    assert_eq!(MeasurementUnit::Millimeters.prompt_name(), "millimeters (mm)");
}

#[test]
fn print_guidelines_differ_per_printer_type() {
    let fdm = PrinterType::Fdm.prompt_guidelines();
    let resin = PrinterType::Resin.prompt_guidelines();
    assert!(fdm.contains("1.2"));   // FDM wall minimum
    assert!(resin.contains("0.8")); // resin wall minimum
    assert_ne!(fdm, resin);
}
```

- [ ] **Step 3: Run → FAIL** (module missing): `ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && cargo test -p engine --test settings"'`

- [ ] **Step 4: Implement** — `engine/src/settings.rs` (shapes are the contract; guideline strings copied from upstream PrintPreferences.promptGuidelines — read them from the upstream clone at `/tmp/cad-studio-src/CADStudio/Settings/PrintPreferences.swift` on the Mac, or re-clone upstream if gone; transcribe the four English strings verbatim):

```rust
//! App settings: on-disk keys match upstream UserDefaults names (contract §6).
//! Stored as JSON at the XDG config path (~/.config/cad-studio/settings.json).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeasurementUnit {
    #[serde(rename = "mm")]
    Millimeters,
    #[serde(rename = "cm")]
    Centimeters,
    #[serde(rename = "m")]
    Meters,
    #[serde(rename = "in")]
    Inches,
}

impl MeasurementUnit {
    pub fn mm_per_unit(self) -> f64 {
        match self {
            MeasurementUnit::Millimeters => 1.0,
            MeasurementUnit::Centimeters => 10.0,
            MeasurementUnit::Meters => 1000.0,
            MeasurementUnit::Inches => 25.4,
        }
    }
    pub fn fraction_digits(self) -> usize {
        match self {
            MeasurementUnit::Millimeters => 1,
            MeasurementUnit::Centimeters => 2,
            MeasurementUnit::Meters => 3,
            MeasurementUnit::Inches => 2,
        }
    }
    pub fn prompt_name(self) -> &'static str {
        match self {
            MeasurementUnit::Millimeters => "millimeters (mm)",
            MeasurementUnit::Centimeters => "centimeters (cm)",
            MeasurementUnit::Meters => "meters (m)",
            MeasurementUnit::Inches => "inches (in)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrinterType {
    #[serde(rename = "fdm")]
    Fdm,
    #[serde(rename = "fdmMultiMaterial")]
    FdmMultiMaterial,
    #[serde(rename = "resin")]
    Resin,
    #[serde(rename = "sls")]
    Sls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PartsLayout {
    #[default]
    Assembled,
    Exploded,
    Plate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub default_project_location: PathBuf,
    pub agent_engine: String, // "claude" | "codex"
    pub claude_path: Option<String>,
    pub codex_path: Option<String>,
    pub claude_model: Option<String>,
    pub codex_model: Option<String>,
    pub claude_effort: Option<String>, // "" | low | medium | high
    pub restricted_bash: bool,
    pub streaming_output: bool,
    pub isolates_claude: bool,
    pub measurement_unit: MeasurementUnit,
    pub printer_type: PrinterType,
    pub printer_model: Option<String>,
    pub has_completed_setup: bool,
    pub default_layout: Option<serde_json::Value>, // LayoutNode JSON; typed in project module
    pub recent_projects: Vec<PathBuf>,
    pub parts_layout: PartsLayout,
    pub parts_collapsed: bool,
    pub parameters_collapsed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_project_location: directories::UserDirs::new()
                .and_then(|u| u.document_dir().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("/tmp")),
            agent_engine: "claude".into(),
            claude_path: None,
            codex_path: None,
            claude_model: None,
            codex_model: None,
            claude_effort: None,
            restricted_bash: false,
            streaming_output: true,
            isolates_claude: true,
            measurement_unit: MeasurementUnit::Millimeters,
            printer_type: PrinterType::Fdm,
            printer_model: None,
            has_completed_setup: false,
            default_layout: None,
            recent_projects: Vec::new(),
            parts_layout: PartsLayout::default(),
            parts_collapsed: false,
            parameters_collapsed: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl Settings {
    /// Tolerant load: unknown fields ignored, missing fields defaulted.
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Atomic save: serialize to `<path>.tmp` in a sibling, fsync, rename over.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self)?;
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

pub fn settings_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "CAD Studio")
        .map(|d| d.config_dir().join("settings.json"))
        .unwrap_or_else(|| PathBuf::from(".cad-studio-settings.json"))
}

impl PrinterType {
    /// English guideline strings injected via {{PRINTING_GUIDELINES}}; verbatim
    /// from upstream PrintPreferences.swift (contract §6).
    pub fn prompt_guidelines(self) -> &'static str {
        PRINT_GUIDELINES[self as usize]
    }
}

pub const PRINT_GUIDELINES: [&str; 4] = [
    // Verbatim from upstream PrintPreferences.swift:73-107 (Swift line-continuations joined); order = Fdm, FdmMultiMaterial, Resin, Sls.
    "Single-filament FDM. Unless told otherwise: walls ≥ 1.2 mm, fit clearance 0.2 mm, overhangs ≤ 45°, short bridges, fillets or chamfers on sharp edges where they do not affect function, a flat base for bed adhesion, teardrop or controlled-overhang horizontal holes. Multiple colors need manual filament changes.",
    "Multi-material FDM (AMS, MMU): several colors or materials are possible, one per 3MF part. Unless told otherwise: walls ≥ 1.2 mm, fit clearance 0.2 mm, overhangs ≤ 45°, fillets or chamfers on sharp edges, a flat base for bed adhesion. Keep color changes per layer low when possible.",
    "Resin printing (SLA, DLP, MSLA). Unless told otherwise: walls ≥ 0.8 mm (≥ 1.5 mm for load-bearing parts), fit clearance 0.1-0.15 mm, fine details down to 0.2 mm, hollow parts with drain holes ≥ 3 mm, avoid large flat areas parallel to the plate, plan for a tilted orientation and supports. The build volume is usually small: flag large parts.",
    "SLS powder printing: no supports needed and overhangs are free. Unless told otherwise: walls ≥ 0.8 mm, fit clearance 0.3-0.5 mm, closed cavities with powder escape holes ≥ 5 mm, avoid strongly varying thicknesses that warp. Colors cannot be printed: use them only to tell parts apart.",
];
```

(`thiserror` — add `thiserror = "2"` to workspace deps and engine.)

`engine/src/lib.rs`:

```rust
//! Domain engine for CAD Studio. No GUI dependencies, ever (spec §4.2).

pub mod settings;
```

- [ ] **Step 5: Run → PASS** on thelio; `cargo check --workspace` clean.
- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock engine/Cargo.toml engine/src/lib.rs engine/src/settings.rs engine/tests/settings.rs
git commit -m "feat(engine): settings module — upstream key names, XDG path, units, print guidelines"
git push thelio main
```

---

### Task 2: Manifest — tolerant decoder + validation

**Files:**
- Create: `engine/src/manifest.rs`, `engine/tests/manifest.rs`
- Modify: `engine/src/lib.rs` (`pub mod manifest;`)

**Interfaces:**
- Consumes: none (pure).
- Produces: `engine::manifest::{Manifest, Drawing, PartRef}` (shapes below); Task 6 consumes `Manifest` for missing-requirements; M2 renders `{{ENVIRONMENT}}` from it.

- [ ] **Step 1: Failing tests** — `engine/tests/manifest.rs`:

```rust
use engine::manifest::Manifest;
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
fn missing_optionals_default_and_malformed_drop() {
    let m: Manifest = serde_json::from_str(r#"{
        "parameters": {"a": 1.5, "b": "not-a-number", "c": true},
        "bed": [220.0, 0.0],
        "warnings": null
    }"#).unwrap();
    assert_eq!(m.parameters.get("a").copied(), Some(1.5));
    assert!(!m.parameters.contains_key("b"));
    assert!(!m.parameters.contains_key("c"));
    assert_eq!(m.bed, None, "bed with a non-positive entry is invalid");
    assert!(m.warnings.is_empty());
    assert_eq!(m.bounding_box, None);
}

#[test]
fn bed_arity_is_exactly_two_positive() {
    let one: Manifest = serde_json::from_str(r#"{"bed": [220.0]}"#).unwrap();
    let three: Manifest = serde_json::from_str(r#"{"bed": [1.0, 2.0, 3.0]}"#).unwrap();
    assert_eq!(one.bed, None);
    assert_eq!(three.bed, None);
    let ok: Manifest = serde_json::from_str(r#"{"bed": [220.0, 220.0]}"#).unwrap();
    assert_eq!(ok.bed, Some([220.0, 220.0]));
}
```

(with `use engine::manifest::BBox;` added to the first test's imports.)

- [ ] **Step 2: Run → FAIL** (`cargo test -p engine --test manifest`).
- [ ] **Step 3: Implement** — `engine/src/manifest.rs`:

```rust
//! manifest.json decoding per contract §3: tolerant, upstream field names.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct BBox {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub file: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartRef {
    pub name: String,
    pub quantity: u32,
    pub color: Option<String>, // hex or null
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub bounding_box: Option<BBox>,
    pub warnings: Vec<String>,
    /// Every numeric constant of model.py; non-numeric entries dropped on decode.
    pub parameters: BTreeMap<String, f64>,
    pub drawings: Vec<Drawing>,
    /// Valid only when exactly 2 positive values.
    pub bed: Option<[f64; 2]>,
    pub parts: Vec<PartRef>,
}

/// f64-or-drop: strings/bools/null silently removed, numbers kept.
fn f64_or_drop<'de, D: serde::Deserializer<'de>>(de: D) -> Result<BTreeMap<String, f64>, D::Error> {
    let raw: BTreeMap<String, serde_json::Value> = BTreeMap::deserialize(de)?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, v)| v.as_f64().map(|n| (k, n)))
        .collect())
}

/// [f64;2]-or-None: keep only exactly-two-all-positive.
fn bed_or_none<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Option<[f64; 2]>, D::Error> {
    let raw: Option<Vec<f64>> = Option::deserialize(de)?;
    Ok(raw.and_then(|v| match v.as_slice() {
        [a, b] if *a > 0.0 && *b > 0.0 => Some([*a, *b]),
        _ => None,
    }))
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, serde_json::Error> {
        serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| {
            serde::ser::Error::custom(e)
        })?)
    }
}
```

(Field-specific `#[serde(default, deserialize_with = "f64_or_drop")]` on `parameters` and `#[serde(default, deserialize_with = "bed_or_none")]` on `bed` — wire those two attributes.)

- [ ] **Step 4: Run → PASS**; commit `feat(engine): manifest tolerant decoder (drop-not-fail semantics, bed validation)`.

---

### Task 3: STL parser + engine mesh types

**Files:**
- Create: `engine/src/parsing/mod.rs`, `engine/src/parsing/stl.rs`, `engine/tests/stl.rs`
- Modify: `engine/src/lib.rs` (`pub mod parsing;`)

**Interfaces:**
- Produces: `engine::parsing::{Part, NEUTRAL}` where `Part { name: String, positions: Vec<[f32; 3]>, triangle_colors: Vec<Option<[u8; 3]>> }` (`positions.len() == 3 * triangle_colors.len()`; per-corner, unindexed — mirrors upstream meshes, contract §3); `engine::parsing::stl::{is_binary_stl, parse_stl}`. Task 4's 3MF parser emits the same `Part`.

- [ ] **Step 1: Failing tests** — `engine/tests/stl.rs`:

```rust
use engine::parsing::stl::{is_binary_stl, parse_stl};
use std::path::PathBuf;

fn fixture(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures").join(p)
}

#[test]
fn binary_predicate() {
    let golden = std::fs::read(fixture("golden/model.stl")).unwrap();
    let n = u32::from_le_bytes([golden[80], golden[81], golden[82], golden[83]]);
    assert!(is_binary_stl(golden.len() as u64, n));
    let ascii = std::fs::read(fixture("handmade/ascii.stl")).unwrap();
    let n = u32::from_le_bytes([ascii[80], ascii[81], ascii[82], ascii[83]]);
    assert!(!is_binary_stl(ascii.len() as u64, n));
}

#[test]
fn binary_golden_parses_with_known_geometry() {
    let bytes = std::fs::read(fixture("golden/model.stl")).unwrap();
    let part = parse_stl(&bytes).unwrap();
    assert_eq!(part.triangle_colors.len(), 24);
    assert_eq!(part.positions.len(), 24 * 3);
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &part.positions {
        for a in 0..3 { mn[a] = mn[a].min(p[a]); mx[a] = mx[a].max(p[a]); }
    }
    assert_eq!(mn, [-30.0, -30.0, -4.0]);
    assert_eq!(mx, [30.0, 30.0, 22.0]);
    assert!(part.triangle_colors.iter().all(|c| c.is_none()), "STL is single-color");
    assert_eq!(part.name, "model"); // from file stem
}

#[test]
fn ascii_fixture_parses_one_triangle() {
    let bytes = std::fs::read(fixture("handmade/ascii.stl")).unwrap();
    let part = parse_stl(&bytes).unwrap();
    assert_eq!(part.triangle_colors.len(), 1);
    assert_eq!(part.positions[0], [0.0, 0.0, 0.0]);
    assert_eq!(part.positions[1], [1.0, 0.0, 0.0]);
    assert_eq!(part.positions[2], [0.0, 1.0, 0.0]);
}

#[test]
fn truncated_binary_is_an_error_not_a_panic() {
    let golden = std::fs::read(fixture("golden/model.stl")).unwrap();
    let cut = &golden[..golden.len() - 20];
    assert!(parse_stl(cut).is_err());
}
```

- [ ] **Step 2: FAIL run; Step 3: Implement** — `engine/src/parsing/mod.rs`:

```rust
//! Mesh/part parsing shared by STL and 3MF paths (contract §3).

pub mod stl;
pub mod threemf; // added in Task 4 — create as empty `//! 3MF container+XML parser.` stub here ONLY if Task 3 must compile standalone; Task 4 fills it
pub mod animation; // Task 5 — same treatment

/// Uncolored triangles render in the app's neutral color (contract §3);
/// the mapping to RGB happens in the viewer, engine only says "no color".
pub type TriangleColor = Option<[u8; 3]>;

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    /// Per-corner positions, unindexed: 3 entries per triangle.
    pub positions: Vec<[f32; 3]>,
    /// One entry per triangle.
    pub triangle_colors: Vec<TriangleColor>,
}

impl Part {
    pub fn triangle_count(&self) -> usize { self.triangle_colors.len() }
    pub fn aabb(&self) -> ([f32; 3], [f32; 3]) {
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        for p in &self.positions {
            for a in 0..3 { mn[a] = mn[a].min(p[a]); mx[a] = mx[a].max(p[a]); }
        }
        (mn, mx)
    }
}
```

`engine/src/parsing/stl.rs` — port the M0 spike's proven binary reader (engine/tests/parse3mf.rs at bf77ad6 has none; the viewer's `viewer/src/render/mod.rs` `parse_binary_stl` + `is_binary_stl` DO — copy the addressing from there, NOT from the M0 plan doc), extend with ASCII:

```rust
//! STL: binary iff size == 84 + 50·n (checked by the caller), else ASCII
//! "solid … vertex x y z" triplets (contract §3). Single color.

use super::{Part, TriangleColor};
use std::io::{BufRead, BufReader};

pub fn is_binary_stl(file_len: u64, header_tri_count: u32) -> bool {
    file_len == 84 + 50 * header_tri_count as u64
}

#[derive(Debug, thiserror::Error)]
#[error("STL parse: {0}")]
pub enum StlError(String);

pub fn parse_stl(bytes: &[u8]) -> Result<Part, StlError> {
    if bytes.len() < 84 {
        return Err(StlError("file shorter than an STL header".into()));
    }
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]);
    if is_binary_stl(bytes.len() as u64, n) {
        Ok(parse_binary(bytes, n as usize)?)
    } else {
        parse_ascii(bytes)
    }
}

fn parse_binary(bytes: &[u8], n: usize) -> Result<Part, StlError> {
    let need = 84 + 50 * n;
    if bytes.len() < need {
        return Err(StlError(format!("truncated: {}/{} bytes", bytes.len(), need)));
    }
    let mut positions = Vec::with_capacity(n * 3);
    let _normal_scratch = [0f32; 3]; // normals recomputed by the viewer
    for i in 0..n {
        let off = 84 + i * 50;
        for c in 0..3 {
            let base = off + 12 + c * 12;
            let f = |k: usize| {
                f32::from_le_bytes(bytes[base + k * 4..base + k * 4 + 4].try_into().unwrap())
            };
            positions.push([f(0), f(1), f(2)]);
        }
    }
    Ok(Part {
        name: "model".into(),
        triangle_colors: vec![None; n],
        positions,
    })
}

fn parse_ascii(bytes: &[u8]) -> Result<Part, StlError> {
    let text = std::str::from_utf8(bytes).map_err(|e| StlError(e.to_string()))?;
    let mut positions = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("vertex") {
            let mut it = rest.split_whitespace().map(|v| v.parse::<f32>());
            let (Some(Ok(x)), Some(Ok(y)), Some(Ok(z)), None) =
                (it.next(), it.next(), it.next(), it.next())
            else {
                return Err(StlError(format!("bad vertex line: {line:?}")));
            };
            positions.push([x, y, z]);
        }
    }
    if positions.is_empty() || positions.len() % 3 != 0 {
        return Err(StlError(format!("{} vertices is not triangles", positions.len())));
    }
    Ok(Part {
        name: "model".into(),
        triangle_colors: vec![None; positions.len() / 3],
        positions,
    })
}
```

- [ ] **Step 4: PASS on thelio** (add `thiserror` dep with Task 1's batch if not already).
- [ ] **Step 5: Commit** `feat(engine): STL parser (binary + ASCII) and shared Part type`.

---

### Task 4: Full 3MF parser (custom, per ADR) + fixtures

**Files:**
- Create: `engine/src/parsing/threemf.rs`, `engine/tests/threemf.rs`, `fixtures/handmade/make_fixtures.py`
- Create (generated, committed): `fixtures/handmade/colorgroup.3mf`, `fixtures/handmade/units.3mf`, `fixtures/handmade/nested.3mf`
- Delete: `engine/tests/parse3mf.rs`, `engine/tests/threemf_spike.rs`
- Modify: `engine/Cargo.toml` (promote quick-xml/zip to real deps, DROP threemf dev-dep)

**Interfaces:**
- Consumes: `engine::parsing::{Part, TriangleColor}` (Task 3).
- Produces: `engine::parsing::threemf::parse_3mf(bytes: &[u8]) -> Result<Vec<Part>, ThreemfError>` — one `Part` per build item (no build items → one per object id, sorted; recursion depth cap 16; spec token order transforms; unit scaling; per-triangle pid/p1 colors; name = object name, else first descendant component's name, else `part {n+1}`).

- [ ] **Step 1: New handmade fixtures** — `fixtures/handmade/make_fixtures.py` (run once on the Mac, artifacts committed; deterministic, mirrors component.3mf's STORED-entry recipe):

```python
#!/usr/bin/env python3
"""Handmade 3MF edge-case fixtures (deterministic; committed artifacts)."""
import zipfile
from pathlib import Path

OUT = Path(__file__).resolve().parent
CT = ('<?xml version="1.0" encoding="UTF-8"?>'
      '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
      '<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>')

CUBE = """  <object id="2" name="cube" pid="1" pindex="0">
      <mesh><vertices>
        <vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="1" y="1" z="0"/><vertex x="0" y="1" z="0"/>
        <vertex x="0" y="0" z="1"/><vertex x="1" y="0" z="1"/><vertex x="1" y="1" z="1"/><vertex x="0" y="1" z="1"/>
      </vertices><triangles>
        <triangle v1="0" v2="2" v3="1" p1="0"/><triangle v1="0" v2="3" v3="2" p1="1"/>
        <triangle v1="4" v2="5" v3="6"/><triangle v1="4" v2="6" v3="7"/>
        <triangle v1="0" v2="1" v3="5"/><triangle v1="0" v2="5" v3="4"/>
        <triangle v1="1" v2="2" v3="6"/><triangle v1="1" v2="6" v3="5"/>
        <triangle v1="2" v2="3" v3="7"/><triangle v1="2" v2="7" v3="6"/>
        <triangle v1="3" v2="0" v3="4"/><triangle v1="3" v2="4" v3="7"/>
      </triangles></mesh>
    </object>"""

def write(name: str, model: str) -> None:
    with zipfile.ZipFile(OUT / name, "w", zipfile.ZIP_STORED) as z:
        z.writestr("[Content_Types].xml", CT)
        z.writestr("3D/3dmodel.model", model)

# colorgroup: colorgroup id=3 (green/red); cube's pid points at it; two triangles
# override with p1 (0=green, 1=red); the rest are uncolored (colorgroup has no
# object pindex -> triangle without p1 has no color)
write("colorgroup.3mf", f"""<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <colorgroup id="3"><color color="#00FF00FF"/><color color="#FF0000FF"/></colorgroup>
    <object id="2" name="cube" pid="3">
{CUBE}
    </object>
  </resources>
  <build><item objectid="2"/></build>
</model>""")

# units: centimeter cube — parser must scale x10 -> [0,10]
write("units.3mf", f"""<?xml version="1.0" encoding="UTF-8"?>
<model unit="centimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <basematerials id="1"><base name="blue" displaycolor="#1F78B5FF"/></basematerials>
    <object id="2" name="cmcube" pid="1" pindex="0">
{CUBE}
    </object>
  </resources>
  <build><item objectid="2"/></build>
</model>""")

# nested: build -> object 5 (components) -> object 4 (components) -> cube mesh,
# outer wrapper unnamed so the name falls through to the first descendant:
# "cmcube-less" case pins name propagation + multi-level transform composition
write("nested.3mf", f"""<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <basematerials id="1"><base name="blue" displaycolor="#1F78B5FF"/></basematerials>
    <object id="2" name="cube" pid="1" pindex="0">
{CUBE}
    </object>
    <object id="4">
      <components><component objectid="2" transform="1 0 0 0 1 0 0 0 1 5 0 0"/></components>
    </object>
    <object id="5">
      <components><component objectid="4" transform="1 0 0 0 1 0 0 0 1 0 7 0"/></components>
    </object>
  </resources>
  <build><item objectid="5"/></build>
</model>""")
print("wrote colorgroup.3mf units.3mf nested.3mf")
```

Run: `python3 fixtures/handmade/make_fixtures.py` on the Mac; commit all four files (script + 3 artifacts).

- [ ] **Step 2: Failing tests** — `engine/tests/threemf.rs`:

```rust
use engine::parsing::threemf::parse_3mf;
use std::path::PathBuf;

fn fixture(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures").join(p)
}
fn parse(p: &str) -> Vec<engine::parsing::Part> {
    parse_3mf(&std::fs::read(fixture(p)).unwrap()).unwrap()
}

#[test]
fn golden_two_named_colored_parts() {
    let parts = parse("golden/model.3mf");
    assert_eq!(parts.len(), 2, "one Part per build item");
    let by_name: std::collections::BTreeMap<&str, &engine::parsing::Part> =
        parts.iter().map(|p| (p.name.as_str(), p)).collect();
    let base = by_name.get("base").expect("name survived export");
    assert!(base.triangle_count() > 100);
    // per-triangle color from pid/pindex basematerials
    assert!(base.triangle_colors.iter().all(|c| c.is_some()));
    assert_eq!(base.triangle_colors[0], Some([0x1F, 0x78, 0xB5]));
    let lid = by_name.get("lid").unwrap();
    assert_eq!(lid.triangle_colors[0], Some([0xD6, 0x26, 0x29]));
}

#[test]
fn component_transform_spec_order() {
    let parts = parse("handmade/component.3mf");
    assert_eq!(parts.len(), 1);
    let (_, mx) = parts[0].aabb();
    assert!((mx[0] - 11.0).abs() < 1e-4, "+10mm x translate at tokens 9-11");
}

#[test]
fn unit_centimeter_scales_x10() {
    let parts = parse("handmade/units.3mf");
    assert_eq!(parts.len(), 1);
    let (_, mx) = parts[0].aabb();
    assert!((mx[0] - 10.0).abs() < 1e-4, "1cm cube -> 10mm");
}

#[test]
fn colorgroup_and_per_triangle_p1() {
    let parts = parse("handmade/colorgroup.3mf");
    assert_eq!(parts.len(), 1);
    let p = &parts[0];
    assert_eq!(p.triangle_colors[0], Some([0, 255, 0]), "p1=0 -> green");
    assert_eq!(p.triangle_colors[1], Some([255, 0, 0]), "p1=1 -> red");
    assert_eq!(p.triangle_colors[2], None, "no p1 and object has no pindex -> uncolored");
}

#[test]
fn nested_components_compose_and_name_falls_through() {
    let parts = parse("handmade/nested.3mf");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].name, "cube", "name from first descendant mesh");
    let (_, mx) = parts[0].aabb();
    assert!((mx[0] - 6.0).abs() < 1e-4, "+5 then +1 via x? see note");
    assert!((mx[1] - 8.0).abs() < 1e-4, "+7 then +1");
}
```

NOTE for the implementer on `nested.3mf` expectations: inner transform translates +5 x, outer +7 y (tokens 9/10 = x=5,y=0 then x=0,y=7 in the two matrices); cube spans [0,1]² so composed maxx = 1+5 = 6 and maxy = 1+7 = 8 — assert exactly that (the sketch above already does; treat these two lines as the spec).

- [ ] **Step 3: FAIL run on thelio.**
- [ ] **Step 4: Implement** `engine/src/parsing/threemf.rs`. Port the working single-pass reader from `engine/tests/parse3mf.rs` at bf77ad6 (read it first — its `resolve_parts` has the correct spec-order `apply()`), then extend to full contract §3 semantics: (a) two-phase — collect all objects/components/build items, then resolve; (b) recursion with depth cap 16 returning an error past it; (c) no build items → one Part per object id sorted; (d) triangle vertex-index resolution (spike skipped it — positions are now per-corner through v1/v2/v3 indices); (e) `basematerials` AND `colorgroup` color tables, `displaycolor`/`color` attrs as `#RRGGBB` or `#RRGGBBAA` (alpha dropped); per-triangle pid/p1 lookup order: triangle pid/p1 → object pid/pindex → None; (f) unit attribute scale: micron 0.001, centimeter 10, inch 25.4, foot 304.8, meter 1000, default 1 — applied to every position; (g) transforms composed parent→child (row-vector: child first, then parent), spec token order; (h) part name: object name → first descendant mesh object's name → `part {n+1}`; (i) components both with and without a `<components>` wrapper element (handmade fixture lacks it in component.3mf; nested.3mf has it — the reader must accept both: treat `<component>` children of an `<object>` as its components regardless of wrapper). ~350 lines expected; `ThreemfError(String)` with `thiserror`.

- [ ] **Step 5: PASS all five tests** on thelio. Then retire the spikes:

```bash
git rm engine/tests/parse3mf.rs engine/tests/threemf_spike.rs
```

(plain `git rm` is fine here — it stages exactly these two paths) and remove `threemf` from engine dev-deps; promote quick-xml/zip to `[dependencies]`. Full `cargo test -p engine` green (the spike tests' coverage now lives in threemf.rs tests + stl.rs tests).

- [ ] **Step 6: Commit**

```bash
git add engine/src/parsing/threemf.rs engine/tests/threemf.rs engine/Cargo.toml Cargo.lock fixtures/handmade/make_fixtures.py fixtures/handmade/colorgroup.3mf fixtures/handmade/units.3mf fixtures/handmade/nested.3mf
git commit -m "feat(engine): full 3MF parser — units, colorgroups, pid/p1, nested transforms; spikes retired"
git push thelio main
```

---

### Task 5: animation.json — parse + interpolation

**Files:**
- Create: `engine/src/parsing/animation.rs`, `engine/tests/animation.rs`

**Interfaces:**
- Consumes: `glam` (workspace).
- Produces: `engine::parsing::animation::{MotionStudy, Frame, Pose, Collision, StudyState}` and `MotionStudy::parse(bytes) -> Result<MotionStudy, String>`, `state_at(t: f64) -> StudyState`; M7's player consumes `state_at`.

- [ ] **Step 1: Failing tests** — `engine/tests/animation.rs`:

```rust
use engine::parsing::animation::{Frame, MotionStudy};
use std::path::PathBuf;

fn golden() -> std::path::PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden/animation.json")
}

#[test]
fn golden_parses_sorted_with_three_frames() {
    let s = MotionStudy::parse(&std::fs::read(golden()).unwrap()).unwrap();
    assert_eq!(s.title.as_deref(), Some("lid lift"));
    assert_eq!(s.frames.len(), 3);
    assert!(s.frames.windows(2).all(|w| w[0].t <= w[1].t), "sorted on load");
    assert_eq!(s.duration(), 2.0);
    assert_eq!(s.value_names(), vec!["Lift (mm)"]);
}

#[test]
fn malformed_entries_default_to_identity() {
    let s = MotionStudy::parse(br#"{"frames":[
        {"t": 1.0, "parts": {"a": {"rotate": [0.0, 0.70710678, 0.0, 0.70710678]}}},
        {"t": 0.0, "parts": {"a": {"translate": [1.0, 2.0]}}}
    ]}"#).unwrap();
    assert_eq!(s.frames[0].t, 0.0, "sorted");
    let a0 = &s.frames[0].parts["a"];
    assert_eq!(a0.translate, [1.0, 2.0, 0.0], "short translate padded");
    assert_eq!(a0.rotate, glam::Quat::IDENTITY, "missing rotate -> identity");
    let a1 = &s.frames[1].parts["a"];
    let n = a1.rotate.length();
    assert!((n - 1.0).abs() < 1e-5, "quaternion normalized on load");
}

#[test]
fn interpolation_lerps_translates_and_slerps_rotations() {
    let s = MotionStudy::parse(br#"{"frames":[
        {"t": 0.0, "parts": {"lid": {"translate": [0.0, 0.0, 0.0]}},
         "values": {"Lift (mm)": 0.0}, "collisions": []},
        {"t": 2.0, "parts": {"lid": {"translate": [0.0, 0.0, 10.0]}},
         "values": {"Lift (mm)": 10.0},
         "collisions": [{"parts": ["base", "lid"], "volume": 0.5}]}
    ]}"#).unwrap();
    let mid = s.state_at(1.0);
    assert_eq!(mid.parts["lid"].translate, [0.0, 0.0, 5.0]);
    assert!((mid.values["Lift (mm)"] - 5.0).abs() < 1e-9);
    // collisions come from the nearest frame (t=1.0 is equidistant -> the later one)
    assert_eq!(mid.collisions.len(), 1);
    assert_eq!(mid.collisions[0].parts, vec!["base".to_string(), "lid".to_string()]);
    // clamping outside the range
    let before = s.state_at(-1.0);
    assert_eq!(before.parts["lid"].translate, [0.0, 0.0, 0.0]);
    let after = s.state_at(9.0);
    assert_eq!(after.parts["lid"].translate, [0.0, 0.0, 10.0]);
}

#[test]
fn slerp_quaternions_halfway() {
    let s = MotionStudy::parse(br#"{"frames":[
        {"t": 0.0, "parts": {"p": {"rotate": [0.0, 0.0, 0.0, 1.0]}}},
        {"t": 1.0, "parts": {"p": {"rotate": [0.0, 0.0, 1.0, 0.0]}}}
    ]}"#).unwrap();
    let mid = s.state_at(0.5);
    // 180° about z; halfway is 90°: quat (0,0,sin45,cos45)
    let r = mid.parts["p"].rotate;
    assert!((r.z - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    assert!((r.w - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
}

#[test]
fn label_resolves_last_at_or_before_t() {
    let s = MotionStudy::parse(br#"{"frames":[
        {"t": 0.0, "label": "closed"},
        {"t": 1.0},
        {"t": 2.0, "label": "open"}
    ]}"#).unwrap();
    assert_eq!(s.state_at(0.5).label.as_deref(), Some("closed"));
    assert_eq!(s.state_at(1.5).label, None, "unlabeled frame at t=1 clears it");
    assert_eq!(s.state_at(2.0).label.as_deref(), Some("open"));
}

#[test]
fn single_frame_study_is_inert() {
    let s = MotionStudy::parse(br#"{"frames":[{"t": 0.0}]}"#).unwrap();
    assert!(!s.is_active()); // active only when frames.len() > 1 (contract §3)
}
```

- [ ] **Step 2: FAIL; Step 3: Implement** `engine/src/parsing/animation.rs` per contract §3 MotionStudy semantics: frames sorted by t on load; `translate` any-length array padded/truncated to 3; `rotate` 4 floats normalized (missing → identity); `values` per frame; `value_names()` = union of keys in first-frame-order then sorted; `state_at` finds surrounding frames, lerp translate + values, slerp rotate (`glam::Quat::slerp`), label = most recent frame with `t <= time` that HAS a label (None if the governing frame is unlabeled), collisions from the nearest frame (ties → later); clamps outside range; `is_active() = frames.len() > 1`; `duration() = last.t`. Deserialize with `serde` tolerant defaults (`#[serde(default)]`, unknown fields ignored) — hand-write `Deserialize` for `Pose` (padding/normalization are load-time transforms).

- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): animation.json study — tolerant parse, lerp/slerp state machine`.

---

### Task 6: Output requirements — expected outputs + missing list (PDF pages)

**Files:**
- Create: `engine/src/outputs.rs`, `engine/tests/outputs.rs`
- Modify: `engine/Cargo.toml` (`cargo add -p engine lopdf`)

**Interfaces:**
- Consumes: `engine::manifest::Manifest` (Task 2).
- Produces: `engine::outputs::{expected_outputs() -> [&'static str; 7], pdf_page_count(path) -> Result<u32, String>, missing_requirements(output_dir: &Path, manifest: Option<&Manifest>) -> Vec<String>}`. M2 builds the "Note from CAD Studio: … Add them in this turn as well" prompt from this Vec.

- [ ] **Step 1: Failing tests** — `engine/tests/outputs.rs`:

```rust
use engine::outputs::{expected_outputs, missing_requirements, pdf_page_count};
use std::path::PathBuf;

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
}

#[test]
fn expected_outputs_list_matches_contract() {
    assert_eq!(expected_outputs(), [
        "model.3mf", "model.stl", "model.step", "schematic.svg",
        "schematic.png", "schematic.pdf", "manifest.json",
    ]);
}

#[test]
fn golden_pdf_has_two_pages() {
    assert_eq!(pdf_page_count(&golden().join("schematic.pdf")).unwrap(), 2);
}

#[test]
fn complete_golden_has_no_missing() {
    let m = engine::manifest::Manifest::load(&golden().join("manifest.json")).unwrap();
    assert!(missing_requirements(&golden(), Some(&m)).is_empty());
}

#[test]
fn missing_pieces_are_reported() {
    // copy golden to a temp dir, delete plate + one expected output, break drawings
    let tmp = std::env::temp_dir().join(format!("cad-outputs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    for f in ["model.3mf", "model.stl", "model.step", "schematic.svg",
              "schematic.png", "schematic.pdf", "manifest.json", "plate.3mf"] {
        std::fs::copy(golden().join(f), tmp.join(f)).unwrap();
    }
    std::fs::remove_file(tmp.join("model.step")).unwrap();
    std::fs::remove_file(tmp.join("plate.3mf")).unwrap();
    let mut m = engine::manifest::Manifest::load(&tmp.join("manifest.json")).unwrap();
    m.drawings.clear();
    m.bed = None;
    let missing = missing_requirements(&tmp, Some(&m));
    assert!(missing.iter().any(|s| s.contains("model.step")));
    assert!(missing.iter().any(|s| s.contains("plate.3mf")));
    assert!(missing.iter().any(|s| s.contains("drawings")));
    assert!(missing.iter().any(|s| s.contains("bed")));
    // drawings cleared -> pdf page count (2) exceeds drawing pages (0) requirement
    assert!(missing.iter().any(|s| s.to_lowercase().contains("pdf")));
}

#[test]
fn per_piece_pages_demanded_when_parts_exist() {
    let tmp = std::env::temp_dir().join(format!("cad-pages-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    for f in ["model.3mf", "model.stl", "model.step", "schematic.svg",
              "schematic.png", "schematic.pdf", "manifest.json", "plate.3mf"] {
        std::fs::copy(golden().join(f), tmp.join(f)).unwrap();
    }
    let m = engine::manifest::Manifest::load(&tmp.join("manifest.json")).unwrap();
    let missing = missing_requirements(&tmp, Some(&m));
    // golden HAS schematic-2.svg; delete it and it becomes missing (2 parts > 1)
    assert!(missing.is_empty());
    std::fs::remove_file(tmp.join("schematic-2.svg")).unwrap();
    let missing = missing_requirements(&tmp, Some(&m));
    assert!(missing.iter().any(|s| s.contains("schematic-2")), "{missing:?}");
}
```

- [ ] **Step 2: FAIL; Step 3: Implement** `engine/src/outputs.rs` per contract §1 (AgentSession missing-list) + §3 (expectedOutputs): missing = expected outputs absent-or-empty + `plate.3mf` + per-piece `schematic-2.svg` when `manifest.parts.len() > 1` (i.e. distinct extra pages: for parts N, pages `schematic-2.svg…schematic-N.svg` — demand each missing one) + `drawings` key empty + `bed` key missing + `pdf_page_count(schematic.pdf) < (1 + max(drawings file numbers))`. Empty-file counts as missing (`metadata.len() == 0`). Strings are user/agent-facing English: e.g. `"model.step"`, `"plate.3mf (print plate)"`, `"schematic-2.svg (per-piece drawing page)"`, `"manifest.json: drawings (drawing page list)"`, `"manifest.json: bed (printer bed size)"`, `"schematic.pdf: needs a page per drawing sheet"`.

- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): output requirements — expected outputs, missing list, PDF page count`.

---

### Task 7: Project store — metadata, chat, layout, recents

**Files:**
- Create: `engine/src/project/mod.rs`, `engine/src/project/layout.rs`, `engine/tests/project.rs`
- Modify: `engine/src/lib.rs` (`pub mod project;`)

**Interfaces:**
- Consumes: `serde`/`time`/`uuid` (Task 1 deps).
- Produces: `engine::project::{ProjectMetadata, ChatMessage, ToolActivity, Role, Action, ToolState, RecentProjects, validate_project_name, load_metadata, save_metadata, load_chat, save_chat}` with `ProjectMetadata::new(app_version: &str) -> ProjectMetadata` (dates = now, everything else None/default), and `engine::project::layout::{LayoutNode, PaneKind, Axis}`. M2 persists turns; M4 renders chat/layout; M5 versions restore.

- [ ] **Step 1: Failing tests** — `engine/tests/project.rs` (representative core; implement all listed fns):

```rust
use engine::project::*;
use engine::project::layout::{Axis, LayoutNode, PaneKind};
use std::path::PathBuf;

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cad-proj-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn metadata_roundtrips_upstream_shape() {
    let t = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let m = ProjectMetadata {
        session_id: Some("abc-123".into()),
        current_version: Some(2),
        created_at: t,
        updated_at: t,
        app_version: "0.1.0".into(),
        layout: Some(LayoutNode::Split {
            axis: Axis::Row,
            first: Box::new(LayoutNode::Pane(PaneKind::Chat)),
            second: Box::new(LayoutNode::Pane(PaneKind::Model3D)),
            fraction: 0.34,
        }),
    };
    let json = serde_json::to_string(&m).unwrap();
    assert!(json.contains("\"sessionId\""));
    assert!(json.contains("\"currentVersion\""));
    assert!(json.contains("\"createdAt\""));
    assert!(json.contains("\"updatedAt\""));
    assert!(json.contains("\"appVersion\""));
    assert!(json.contains("\"pane\":\"chat\""));
    assert!(json.contains("\"axis\":\"row\""));
    assert!(json.contains("\"fraction\":0.34"));
    let back: ProjectMetadata = serde_json::from_str(&json).unwrap();
    assert_eq!(back.session_id.as_deref(), Some("abc-123"));
    assert_eq!(back.layout, m.layout);
}
```

(Camel-case field names per upstream Swift Codable output: sessionId, currentVersion, createdAt, updatedAt, appVersion, layout; dates RFC3339 ISO8601. For `ChatMessage`: `{"id": "<uuid>", "role": "user|assistant|tool|thinking|system|summary", "text": "…", "date": "<rfc3339>", "toolUseID": "…"|absent, "detail": "…"|absent, "action": "askClaudeToFix|openTerminal|configureClaude"|absent, "attachments": ["…"]|absent}` — contract §1 `ChatMessage`/`ToolActivity`.)

```rust
#[test]
fn chat_roundtrips_all_roles_and_tools() {
    let t = time::OffsetDateTime::from_unix_timestamp(1_700_000_500).unwrap();
    let msgs = vec![
        ChatMessage { id: uuid::Uuid::new_v4(), role: Role::User, text: "make a box".into(),
                      date: t, tool: None, detail: None, action: None, attachments: vec!["references/1-sketch.png".into()] },
        ChatMessage { id: uuid::Uuid::new_v4(), role: Role::Tool, text: String::new(), date: t,
                      tool: Some(ToolActivity { tool_use_id: "tu_1".into(), name: "Bash".into(),
                          input: "python model.py".into(), output: Some("ok".into()),
                          preview: Some("Compiles the model".into()), state: ToolState::Succeeded }),
                      detail: None, action: None, attachments: vec![] },
        ChatMessage { id: uuid::Uuid::new_v4(), role: Role::System, text: "build failed".into(),
                      date: t, tool: None, detail: Some("Traceback…".into()),
                      action: Some(Action::AskClaudeToFix), attachments: vec![] },
    ];
    let json = serde_json::to_string(&msgs).unwrap();
    let back: Vec<ChatMessage> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.len(), 3);
    assert_eq!(back[1].tool.as_ref().unwrap().tool_use_id, "tu_1");
    assert_eq!(back[2].action, Some(Action::AskClaudeToFix));
    assert_eq!(back[0].attachments.len(), 1);
}

#[test]
fn saves_are_atomic_no_tmp_residue() {
    let d = tmp();
    let mut m = ProjectMetadata::new("0.1.0");
    m.session_id = Some("s1".into());
    save_metadata(&d, &m).unwrap();
    assert!(d.join(".cadstudio/project.json").exists());
    save_chat(&d, &[]).unwrap();
    assert!(d.join(".cadstudio/chat.json").exists());
    let mut residue = 0;
    for e in std::fs::read_dir(d.join(".cadstudio")).unwrap() {
        if e.unwrap().file_name().to_string_lossy().ends_with(".tmp") { residue += 1; }
    }
    assert_eq!(residue, 0);
    let m2 = load_metadata(&d).unwrap();
    assert_eq!(m2.session_id.as_deref(), Some("s1"));
}

#[test]
fn project_name_validation_rules() {
    assert!(validate_project_name("Gearbox v2").is_ok());
    assert!(validate_project_name("").is_err());
    assert!(validate_project_name(".hidden").is_err(), "no leading dot");
    assert!(validate_project_name("a/b").is_err());
    assert!(validate_project_name("a:b").is_err());
    assert!(validate_project_name("plain-name 123").is_ok());
}

#[test]
fn recents_dedupe_evict_and_filter_missing() {
    let d = tmp();
    let mut r = RecentProjects::default();
    for i in 0..14 {
        let p = d.join(format!("p{i}"));
        std::fs::create_dir_all(&p).unwrap();
        r.add(p.clone());
    }
    assert_eq!(r.0.len(), 12, "max 12");
    r.add(d.join("p5")); // canonicalize-dedupe: moves to front, no growth
    assert_eq!(r.0.len(), 12);
    assert_eq!(r.0[0], d.join("p5").canonicalize().unwrap());
    std::fs::remove_dir_all(d.join("p0")).unwrap();
    let filtered = r.filter_existing();
    assert!(!filtered.0.contains(&d.join("p0").canonicalize().unwrap()));
}
```

- [ ] **Step 2: FAIL; Step 3: Implement** `engine/src/project/mod.rs` + `layout.rs`: serde with `#[serde(rename_all = "camelCase")]` and the exact enum tag shapes above (`#[serde(tag = "pane")]`-style for LayoutNode — hand-write the serde impls if tagged-enum JSON shape needs it: `{"pane":"chat"}` and `{"split":{"axis":"row","first":…,"second":…,"fraction":…}}`); `#[serde(with = "time::serde::rfc3339")]` on dates; atomic save via temp+rename (same pattern as Settings::save); `RecentProjects(pub Vec<PathBuf>)` with `add` (dedupe by canonicalized path, insert front, truncate 12) and `filter_existing`; `validate_project_name` per upstream rules (non-empty, no leading ".", no "/" or ":").

- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): project store — metadata/chat/layout serde shapes, atomic saves, recents, name validation`.

---

### Task 8: Output watcher — inotify, debounce, mod-time dedup

**Files:**
- Create: `engine/src/watch.rs`, `engine/tests/watch.rs`
- Modify: `engine/src/lib.rs` (`pub mod watch;`)

**Interfaces:**
- Consumes: `notify` 8 (workspace dep from Task 1).
- Produces: `engine::watch::{ReloadKind, watch_directory(dir: &Path, debounce: std::time::Duration) -> notify::Result<(WatchHandle, std::sync::mpsc::Receiver<ReloadKind>)>}` with `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)] pub enum ReloadKind { Manifest, Plate, Motion, Mesh, Schematic }`; `WatchHandle` stops on Drop. Mapping per contract §3: manifest.json→Manifest, plate.3mf→Plate, animation.json→Motion, model.3mf|model.stl→Mesh, schematic*.svg|schematic.png→Schematic. Nothing else watched; the watcher watches ONLY the `output/` directory (non-recursive).

- [ ] **Step 1: Failing tests** — `engine/tests/watch.rs`:

```rust
use engine::watch::{watch_directory, ReloadKind};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

fn tmp_output() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cad-watch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
fn write(path: &std::path::Path, bytes: &[u8]) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(bytes).unwrap();
}

const FAST: Duration = Duration::from_millis(30);

#[test]
fn maps_files_to_reload_kinds() {
    let d = tmp_output();
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    write(&d.join("manifest.json"), b"{}");
    write(&d.join("plate.3mf"), b"x");
    write(&d.join("animation.json"), b"{}");
    write(&d.join("model.3mf"), b"x");
    write(&d.join("model.stl"), b"x");
    write(&d.join("schematic.svg"), b"x");
    write(&d.join("schematic-2.svg"), b"x");
    write(&d.join("schematic.png"), b"x");
    write(&d.join("references-dont-care.txt"), b"x");
    std::thread::sleep(Duration::from_millis(200));
    let mut kinds = std::collections::BTreeSet::new();
    while let Ok(k) = rx.try_recv() { kinds.insert(k); }
    assert!(kinds.contains(&ReloadKind::Manifest));
    assert!(kinds.contains(&ReloadKind::Plate));
    assert!(kinds.contains(&ReloadKind::Motion));
    assert!(kinds.contains(&ReloadKind::Mesh));
    assert!(kinds.contains(&ReloadKind::Schematic));
}

#[test]
fn storm_collapses_to_one_reload_per_file() {
    let d = tmp_output();
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    // an agent-shaped burst: many events, same files, rapid
    for _ in 0..25 {
        write(&d.join("model.3mf"), b"storm");
        write(&d.join("manifest.json"), b"{}");
    }
    std::thread::sleep(Duration::from_millis(300));
    let mut mesh = 0;
    while let Ok(k) = rx.try_recv() { if k == ReloadKind::Mesh { mesh += 1; } }
    assert_eq!(mesh, 1, "debounce + mod-time dedup collapse the storm");
}

#[test]
fn second_write_after_quiet_emits_again() {
    let d = tmp_output();
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    write(&d.join("model.3mf"), b"one");
    std::thread::sleep(Duration::from_millis(150));
    write(&d.join("model.3mf"), b"two");
    std::thread::sleep(Duration::from_millis(150));
    let mut mesh = 0;
    while let Ok(k) = rx.try_recv() { if k == ReloadKind::Mesh { mesh += 1; } }
    assert_eq!(mesh, 2);
}

#[test]
fn untouched_files_never_reload() {
    let d = tmp_output();
    write(&d.join("model.3mf"), b"initial");
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    write(&d.join("manifest.json"), b"{}"); // only this changes
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(k) = rx.try_recv() {
        assert_ne!(k, ReloadKind::Mesh, "model.3mf never changed after watch start");
    }
}
```

- [ ] **Step 2: FAIL; Step 3: Implement** `engine/src/watch.rs`: `notify::recommended_watcher` on the dir (non-recursive, `RecursiveMode::NonRecursive`); a collector thread drains the event channel, maps each event's paths through the kind table (contract §3 — 3MF preferred over STL is the CONSUMER's job, watcher emits Mesh for either), and runs the debounce: coalesce events within the window; for each changed path compare `metadata().modified()` against a stored map — emit the kind only when the mod-time actually moved. `WatchHandle` wraps the watcher + closes the channel on Drop. The mod-time map starts EMPTY at watch start (a file written before watching exists in the map only after its first event — `untouched_files_never_reload` pins that pre-existing files with no events emit nothing).

- [ ] **Step 4: PASS on thelio** (timing tests use 30 ms debounce to stay fast; the production caller passes `Duration::from_millis(300)` per contract §3).
- [ ] **Step 5: Commit** `feat(engine): output watcher — non-recursive inotify, debounce, mod-time dedup, kind mapping`.

---

### Task 9: M1 wrap — lib surface, carry-forward strikes, full gate

**Files:**
- Modify: `engine/src/lib.rs`, `docs/research/carry-forwards.md`
- Test: whole workspace

**Interfaces:**
- Produces: final M1 lib surface (`pub mod` for settings, manifest, outputs, parsing, project, watch) with crate-level docs; carry-forwards M1 section struck.

- [ ] **Step 1: lib.rs docs**

```rust
//! CAD Studio domain engine.
//!
//! Library-only: no GUI dependencies (spec §4.2). Modules:
//! - [`settings`] — app settings (upstream UserDefaults key names, XDG path)
//! - [`manifest`] — tolerant manifest.json decoding
//! - [`outputs`] — expected outputs + agent-facing missing list
//! - [`parsing`] — 3MF/STL mesh parts and animation studies
//! - [`project`] — project metadata, chat history, layouts, recents
//! - [`watch`] — output/ directory reload watching
```

- [ ] **Step 2: Strike the M1 section** of `docs/research/carry-forwards.md`: convert each landed bullet to `~~strikethrough~~` with `(landed M1)` — parser rewrite, wrapper tolerance, no-cargo-cult (spikes deleted), threemf dev-dep dropped. Leave the golden-facts bullet UNSTRUCK (still true for later milestones).

- [ ] **Step 3: Full gate on thelio**

```bash
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && git pull -q && cargo test --workspace --all-targets 2>&1 | grep -E \"(running|test result|warning)\" | tail -20"'
```

Expected: all green, zero warnings — M0's 13 tests plus M1's (~35 new).

- [ ] **Step 4: Commit** `docs: M1 complete — lib surface, carry-forward strikes`; push; report.
