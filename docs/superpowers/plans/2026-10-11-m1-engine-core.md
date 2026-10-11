# M1 Engine Core Implementation Plan (rev 2 — post independent review)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the engine crate's domain core — settings, manifest, output-requirements, full 3MF/STL/animation parsers, project store, and the output-directory watcher — each TDD'd against the committed goldens, per spec §10 M1.

**Architecture:** Pure library code, zero GUI dependencies. Types follow the upstream on-disk contracts exactly, verified against the **vendored upstream sources** at `fixtures/upstream-source/` (7 Swift files; the fidelity authority for every JSON shape, default, and string this plan asserts) plus docs/research/upstream-contract.md §3/§4/§6. Parsing ports the M0 spike (engine/tests/parse3mf.rs at bf77ad6 — copy conventions from there, NOT from the M0 plan doc).

**Tech Stack:** serde/serde_json, quick-xml 0.41, zip 8.6, notify 8, lopdf (workspace-pinned), time 0.3 (`serde-well-known` + `macros`), uuid v4, directories 6, glam.

**Spec:** docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md (§4.2, §10 M1). Carry-forwards: docs/research/carry-forwards.md (M1 section).

## Global Constraints

- engine crate has NO GUI dependencies — never libcosmic, iced, or wgpu (spec §4.2).
- Tests run on thelio via `ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && git pull -q && cargo test -p engine"'`; no network, no venv — committed fixtures only.
- On-disk formats are port-1:1 per `fixtures/upstream-source/` (authoritative) — project.json uses snake_case CodingKeys (`session_id`…), chat.json uses property names (`toolUseID`), dates are whole-second UTC ISO8601.
- 3MF transforms use spec token order: 12 tokens, 4 rows × 3, row-vector application, translation at tokens 9/10/11 (engine/tests/parse3mf.rs at bf77ad6 is the reference).
- Tolerant decoding everywhere: malformed values drop that field, never the file (upstream `try? ?? default` semantics).
- NEVER `git rm` (user git-safety rule): delete with plain `rm`, stage the deletion with `git add <path>`.
- Conventional commits; stage explicit paths, never `git add -A`/`-u`/`.`.
- thelio login shell is `bash -lc` (no zsh on the host).

## Review Focus

1. **Malformed manifest.json** (non-numeric params, bad bed entries, malformed drawings) → per-field drop, file still loads → Task 2.
2. **3MF unit ≠ mm** → positions scaled (centimeter ×10…) → Task 4 `units.3mf`.
3. **animation.json unsorted frames / non-unit quaternions / wrong-arity arrays** → sorted, normalized, arity-else-identity → Task 5.
4. **inotify storms during agent writes** → one ReloadKind per changed file per quiet window (trailing-edge debounce) → Task 8.
5. **Crash mid-write corrupting project.json/chat.json** → atomic temp+rename → Task 7.

---

### Task 1: Workspace deps + settings module (+ vendored sources)

**Files:**
- Create: `engine/src/settings.rs`, `engine/tests/settings.rs`
- Create (this task, already staged on disk): `fixtures/upstream-source/*.swift` (7 files)
- Modify: `Cargo.toml` (workspace deps), `engine/Cargo.toml`, `engine/src/lib.rs`

**Interfaces:**
- Produces: `engine::settings::{Settings, PrinterType, MeasurementUnit, PartsLayout, settings_path}` with `Settings::load(&Path) -> Result<Settings, SettingsError>` (missing file → default) and `Settings::save(&Path)` (atomic). Tasks 2/6/7 and M2+ consume these.

- [ ] **Step 0: Commit the vendored upstream sources** (already copied to fixtures/upstream-source/ — AppSettings.swift, ChatMessage.swift, MotionStudy.swift, PrintPreferences.swift, Project.swift, ProjectOutput.swift, ProjectStore.swift):

```bash
git add fixtures/upstream-source/AppSettings.swift fixtures/upstream-source/ChatMessage.swift fixtures/upstream-source/MotionStudy.swift fixtures/upstream-source/PrintPreferences.swift fixtures/upstream-source/Project.swift fixtures/upstream-source/ProjectOutput.swift fixtures/upstream-source/ProjectStore.swift
git commit -m "docs: vendor upstream Swift sources as M1+ fidelity authority"
```

- [ ] **Step 1: Add dependencies.** Workspace `[workspace.dependencies]`:

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
directories = "6"
time = { version = "0.3", features = ["serde-well-known", "macros"] }
uuid = { version = "1", features = ["v4", "serde"] }
notify = "8"
lopdf = "0.34"
thiserror = "2"
glam = { version = "0.33", features = ["bytemuck"] }
```

(`serde-well-known` is load-bearing: `time::serde::rfc3339`'s deserialize half is feature-gated behind parsing — `formatting` alone, as first drafted, does not compile a Deserialize derive.)

engine `[dependencies]`: serde, serde_json, directories, time, uuid, notify, thiserror (all `workspace = true`). Dev-deps keep quick-xml/zip until Task 4 promotes them.

- [ ] **Step 2: Failing tests** — `engine/tests/settings.rs`:

```rust
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
    assert_eq!(s.parts_layout, PartsLayout::Exploded, "upstream default ?? .exploded");
}

fn documents_dir() -> PathBuf {
    directories::UserDirs::new().unwrap().document_dir().unwrap().to_path_buf()
}

#[test]
fn json_uses_upstream_key_names() {
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
    assert!(json.contains("\"millimeters\""), "units store upstream rawValues");
    assert!(json.contains("\"exploded\""));
    let back: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.printer_model.as_deref(), Some("Bambu Lab P1S"));
}

#[test]
fn load_tolerates_unknown_and_invalid_values() {
    let d = tmp_dir();
    let p = d.join("settings.json");
    std::fs::write(&p, "{\"agentEngine\":\"codex\",\"measurementUnit\":\"furlongs\",\"futureKey\":42}").unwrap();
    let s = Settings::load(&p).unwrap();
    assert_eq!(s.agent_engine, "codex");
    assert_eq!(s.measurement_unit, MeasurementUnit::Millimeters, "unknown enum falls back, load never fails");
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
        if e.unwrap().file_name().to_string_lossy().ends_with(".tmp") { n += 1; }
    }
    assert_eq!(n, 0, "temp residue left behind");
}

#[test]
fn settings_path_is_lowercase_hyphenated() {
    let p = engine::settings::settings_path();
    assert!(p.ends_with("cad-studio/settings.json"), "got {p:?} — directories lowercases AND strips spaces");
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
    assert!(fdm.contains("1.2"));
    assert!(resin.contains("0.8"));
    assert_ne!(fdm, resin);
}
```

- [ ] **Step 3: Run → FAIL** (`cargo test -p engine --test settings` on thelio after push).

- [ ] **Step 4: Implement** `engine/src/settings.rs`. Key decisions, each traceable to `fixtures/upstream-source/AppSettings.swift` + `PrintPreferences.swift`:

```rust
//! App settings. Key names and enum rawValues match upstream UserDefaults
//! (fixtures/upstream-source/AppSettings.swift); stored as JSON at
//! ~/.config/cad-studio/settings.json (directories lowercases and removes
//! spaces, so the app id must already be "cad-studio").

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeasurementUnit {
    Millimeters, Centimeters, Meters, Inches,
}
// upstream stores rawValues "millimeters"/"centimeters"/"meters"/"inches";
// unknown values fall back to the default (upstream regionDefault does the
// same on invalid) — serde via from_str match, not derive.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartsLayout {
    #[serde(rename = "assembled")]
    Assembled,
    #[serde(rename = "exploded")]
    Exploded,
    #[serde(rename = "plate")]
    Plate,
}
```

Hand-write `Serialize`/`Deserialize` for `MeasurementUnit` with the four full-word strings and unknown→`Err`→caught-by-field-default? No: implement `Deserialize` returning the default on unknown (a custom visitor that maps `millimeters`→Millimeters, … , `_ => Millimeters`). `Serialize` emits the same strings.

`Settings` struct carries the 19 fields of the test's key list with `#[serde(rename_all = "camelCase", default)]` and `Default` impl per upstream defaults (`parts_layout: PartsLayout::Exploded`; default location = Documents). `load`/`save`/`settings_path` as in the interface; `save` = serialize to `path.with_extension("json.tmp")`, sync, rename. `settings_path` = `directories::ProjectDirs::from("", "", "cad-studio").config_dir().join("settings.json")` (note: NOT "CAD Studio" — the crate lowercases and strips spaces). `PrinterType::prompt_guidelines()` returns `PRINT_GUIDELINES[...]` — the four strings verbatim from `fixtures/upstream-source/PrintPreferences.swift:73-107`:

```rust
pub const PRINT_GUIDELINES: [&str; 4] = [
    "Single-filament FDM. Unless told otherwise: walls ≥ 1.2 mm, fit clearance 0.2 mm, overhangs ≤ 45°, short bridges, fillets or chamfers on sharp edges where they do not affect function, a flat base for bed adhesion, teardrop or controlled-overhang horizontal holes. Multiple colors need manual filament changes.",
    "Multi-material FDM (AMS, MMU): several colors or materials are possible, one per 3MF part. Unless told otherwise: walls ≥ 1.2 mm, fit clearance 0.2 mm, overhangs ≤ 45°, fillets or chamfers on sharp edges, a flat base for bed adhesion. Keep color changes per layer low when possible.",
    "Resin printing (SLA, DLP, MSLA). Unless told otherwise: walls ≥ 0.8 mm (≥ 1.5 mm for load-bearing parts), fit clearance 0.1-0.15 mm, fine details down to 0.2 mm, hollow parts with drain holes ≥ 3 mm, avoid large flat areas parallel to the plate, plan for a tilted orientation and supports. The build volume is usually small: flag large parts.",
    "SLS powder printing: no supports needed and overhangs are free. Unless told otherwise: walls ≥ 0.8 mm, fit clearance 0.3-0.5 mm, closed cavities with powder escape holes ≥ 5 mm, avoid strongly varying thicknesses that warp. Colors cannot be printed: use them only to tell parts apart.",
];
```

`SettingsError` via `thiserror` (Io/Json). `engine/src/lib.rs`: `pub mod settings;` + crate doc header (final form lands in Task 9).

- [ ] **Step 5: PASS; Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock engine/Cargo.toml engine/src/lib.rs engine/src/settings.rs engine/tests/settings.rs
git commit -m "feat(engine): settings module — upstream keys/rawValues, XDG path, units, print guidelines"
git push thelio main
```

---

### Task 2: Manifest — hand-written tolerant Deserialize

**Files:**
- Create: `engine/src/project/mod.rs`, `engine/src/project/manifest.rs`, `engine/tests/manifest.rs`
- Modify: `engine/src/lib.rs` (`pub mod project;`)

(Module placement per spec §4.2: manifest lives under `project/`.)

**Interfaces:**
- Produces: `engine::project::manifest::{Manifest, BBox, Drawing, PartRef}` + `Manifest::load(&Path) -> Result<Manifest, serde_json::Error>`; Task 6 and M2 consume.

- [ ] **Step 1: Failing tests** — `engine/tests/manifest.rs`:

```rust
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
```

- [ ] **Step 2: FAIL; Step 3: Implement** `engine/src/project/manifest.rs` with a **hand-written `Deserialize`** (per-key tolerance — derives cannot express "malformed value drops the field"):

```rust
//! manifest.json decoding. Every field is best-effort (upstream wraps each
//! decode in try? ?? default): malformed values drop the field, never the file.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct BBox { pub x: f64, pub y: f64, pub z: f64 }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drawing { pub file: String, pub title: String }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartRef { pub name: String, pub quantity: u32, pub color: Option<String> }

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Manifest {
    pub name: Option<String>,
    pub bounding_box: Option<BBox>,
    pub warnings: Vec<String>,
    pub parameters: BTreeMap<String, f64>,
    pub drawings: Vec<Drawing>,
    pub bed: Option<[f64; 2]>,
    pub parts: Vec<PartRef>,
}

impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(de)?;
        let obj = v.as_object().cloned().unwrap_or_default();
        let strings = |k: &str| -> Vec<String> {
            obj.get(k).and_then(|v| v.as_array()).map(|a| {
                a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
            }).unwrap_or_default()
        };
        let bed = obj.get("bed").and_then(|v| v.as_array()).and_then(|a| {
            let (Some(x), Some(y), None) = (a.first().and_then(|v| v.as_f64()),
                                            a.get(1).and_then(|v| v.as_f64()),
                                            a.get(2)) else { return None };
            (x > 0.0 && y > 0.0).then_some([x, y])
        });
        let parameters = obj.get("parameters").and_then(|v| v.as_object()).map(|m| {
            m.iter().filter_map(|(k, v)| v.as_f64().map(|n| (k.clone(), n))).collect()
        }).unwrap_or_default();
        let drawings = obj.get("drawings").and_then(|v| v.as_array()).map(|a| {
            a.iter().filter_map(|e| {
                let f = e.get("file")?.as_str()?.to_string();
                let t = e.get("title")?.as_str()?.to_string();
                Some(Drawing { file: f, title: t })
            }).collect()
        }).unwrap_or_default();
        let parts = obj.get("parts").and_then(|v| v.as_array()).map(|a| {
            a.iter().filter_map(|e| {
                Some(PartRef {
                    name: e.get("name")?.as_str()?.to_string(),
                    quantity: e.get("quantity").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
                    color: e.get("color").and_then(|v| v.as_str()).map(String::from),
                })
            }).collect()
        }).unwrap_or_default();
        let bounding_box = obj.get("bounding_box").and_then(|e| {
            Some(BBox {
                x: e.get("x").and_then(|v| v.as_f64())?,
                y: e.get("y").and_then(|v| v.as_f64())?,
                z: e.get("z").and_then(|v| v.as_f64())?,
            })
        });
        Ok(Manifest {
            name: obj.get("name").and_then(|v| v.as_str()).map(String::from),
            bounding_box, warnings: strings("warnings"), parameters, drawings, bed, parts,
        })
    }
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, serde_json::Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| serde_json::Error::io(e))?;
        serde_json::from_str(&text)
    }
}
```

(If `serde_json::Error::io` is not public in the locked version, use `serde_json::from_str` on a read that maps the io error to a custom message via `serde::de::Error::custom` — compile decides; the behavior contract is: missing file is an Err, unlike settings.)

- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): manifest — hand-written per-field tolerant deserialize`.

---

### Task 3: STL parser + engine mesh types

**Files:**
- Create: `engine/src/parsing/mod.rs`, `engine/src/parsing/stl.rs`, `engine/tests/stl.rs`
- Modify: `engine/src/lib.rs` (`pub mod parsing;`)

**Interfaces:**
- Produces: `engine::parsing::{Part, TriangleColor}` — `Part { name: String, positions: Vec<[f32; 3]>, triangle_colors: Vec<TriangleColor> }` (per-corner unindexed; `positions.len() == 3 * triangle_colors.len()`), `Part::aabb()`, `Part::triangle_count()`; `TriangleColor = Option<[u8; 3]>`. `engine::parsing::stl::{is_binary_stl(len, n), parse_stl(bytes) -> Result<Part, StlError>}`.

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
    assert_eq!(part.triangle_count(), 24);
    assert_eq!(part.positions.len(), 24 * 3);
    let (mn, mx) = part.aabb();
    assert_eq!(mn, [-30.0, -30.0, -4.0]);
    assert_eq!(mx, [30.0, 30.0, 22.0]);
    assert!(part.triangle_colors.iter().all(|c| c.is_none()));
    assert_eq!(part.name, "model"); // fixed name: STL carries none; callers rename per display need
}

#[test]
fn ascii_fixture_parses_one_triangle() {
    let bytes = std::fs::read(fixture("handmade/ascii.stl")).unwrap();
    let part = parse_stl(&bytes).unwrap();
    assert_eq!(part.triangle_count(), 1);
    assert_eq!(part.positions[0], [0.0, 0.0, 0.0]);
    assert_eq!(part.positions[1], [1.0, 0.0, 0.0]);
    assert_eq!(part.positions[2], [0.0, 1.0, 0.0]);
}

#[test]
fn ascii_skips_bad_lines_errors_only_on_zero_triangles() {
    // upstream skips unparseable vertex lines; error only if nothing parsed
    let text = b"solid x\n  facet normal 0 0 1\n    outer loop\n      vertex 0 0 0\n      vertex bogus\n      vertex 1 0 0\n      vertex 0 1 0\n    endloop\n  endfacet\nendsolid x\n";
    let part = parse_stl(text).unwrap();
    assert_eq!(part.triangle_count(), 1, "bad line skipped, three good vertices still form the triangle");
    assert!(parse_stl(b"solid empty\nendsolid empty\n").is_err());
}

#[test]
fn truncated_binary_is_an_error_not_a_panic() {
    let golden = std::fs::read(fixture("golden/model.stl")).unwrap();
    assert!(parse_stl(&golden[..golden.len() - 20]).is_err());
}
```

- [ ] **Step 2: FAIL; Step 3: Implement.** `engine/src/parsing/mod.rs`:

```rust
//! Mesh/part parsing shared by STL and 3MF (contract §3).

pub mod stl;

/// Uncolored triangles render in the app's neutral color; engine only says
/// "no color", the viewer picks RGB (contract §3).
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

`engine/src/parsing/stl.rs` — binary path: port the M0-proven addressing from `viewer/src/render/mod.rs`'s `parse_binary_stl` (float k at record offset 4k within the 50-byte record after the 12-byte normal; verify against its committed unit test). ASCII path per upstream semantics:

```rust
fn parse_ascii(bytes: &[u8]) -> Result<Part, StlError> {
    let text = std::str::from_utf8(bytes).map_err(|e| StlError(e.to_string()))?;
    let mut positions = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("vertex") else { continue };
        let mut it = rest.split_whitespace().filter_map(|v| v.parse::<f32>().ok());
        if let (Some(x), Some(y), Some(z)) = (it.next(), it.next(), it.next()) {
            positions.push([x, y, z]);
        } // unparseable line: skipped (upstream behavior)
    }
    if positions.is_empty() || positions.len() % 3 != 0 {
        return Err(StlError(format!("{} vertices do not form triangles", positions.len())));
    }
    Ok(Part {
        name: "model".into(),
        triangle_colors: vec![None; positions.len() / 3],
        positions,
    })
}
```

(`is_binary_stl`, `parse_stl` dispatch, binary loop, truncation check: as in rev 1 — unchanged.)

- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): STL parser (binary + ASCII skip-semantics) and shared Part type`.

---

### Task 4: Full 3MF parser + fixtures

**Files:**
- Create: `engine/src/parsing/threemf.rs`, `engine/tests/threemf.rs`, `fixtures/handmade/make_fixtures.py`
- Create (generated, committed): `fixtures/handmade/colorgroup.3mf`, `fixtures/handmade/units.3mf`, `fixtures/handmade/nested.3mf`
- Delete (plain `rm`, staged via `git add`): `engine/tests/parse3mf.rs`, `engine/tests/threemf_spike.rs`
- Modify: `engine/Cargo.toml` (promote quick-xml/zip to deps, drop threemf dev-dep)

**Interfaces:**
- Consumes: `engine::parsing::{Part, TriangleColor}` (Task 3).
- Produces: `parse_3mf(bytes: &[u8]) -> Result<Vec<Part>, ThreemfError>` — one Part per build item; no build items → one per object id **sorted as strings** (upstream: "10" < "2"); recursion depth cap 16; spec token order transforms; unit scaling; per-triangle pid/p1; name = object name → first descendant mesh's **non-empty** name → `part {n+1}`.

- [ ] **Step 1: Fixtures** — `fixtures/handmade/make_fixtures.py` exactly as rev 1 (colorgroup with two `p1` overrides on the first two triangles and object `pid="3"` pointing at a colorgroup with NO pindex; units with `unit="centimeter"`; nested with two transform-composing levels). Run `python3 fixtures/handmade/make_fixtures.py` on the Mac; commit artifacts.

- [ ] **Step 2: Failing tests** — `engine/tests/threemf.rs` (rev 1's five tests with these corrections):

```rust
// golden_two_named_colored_parts: the golden's parts have 12 triangles each —
assert_eq!(base.triangle_count(), 12);
assert_eq!(lid.triangle_count(), 12);
// (build123d's Box meshes are 12-triangle; rev 1's "> 100" was wrong)

// component_transform_spec_order: unchanged (maxx == 11.0 ± 1e-4)
// unit_centimeter_scales_x10: unchanged (maxx == 10.0 ± 1e-4)
// colorgroup_and_per_triangle_p1: unchanged (green/red/None on triangles 0/1/2)
// nested_components_compose_and_name_falls_through: unchanged
//   (name "cube", maxx 6.0, maxy 8.0)
```

Plus one new test pinning the no-build-items fallback order:

```rust
#[test]
fn no_build_items_sorts_object_ids_as_strings() {
    // objects 10 and 2, no <build>: parts come back "10" before "2"
    let xml = br#"<?xml version="1.0"?><model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources></resources></model>"#;
    // craft via make_fixtures.py addition: two tiny objects with ids 2 and 10,
    # // named "b" and "a", no build section — assert parts[0].name == the object-10 part
}
```

(Concretely: extend `make_fixtures.py` with `nosort.3mf` — objects id=10 name="ten", id=2 name="two", each the CUBE, no `<build>` — and assert `parse_3mf` returns the id-10 part FIRST: `assert_eq!(parts[0].name, "ten")`.)

- [ ] **Step 3: FAIL; Step 4: Implement** `engine/src/parsing/threemf.rs` by porting `engine/tests/parse3mf.rs` @ bf77ad6 (read it first — its `apply()` is the spec-order reference) and extending per contract §3 + upstream quirks: two-phase collect-then-resolve; recursion cap 16 (error past); string-sorted fallback; per-corner positions through v1/v2/v3 indices; `basematerials` AND `colorgroup` tables (`displaycolor`/`color`, `#RRGGBB`/`#RRGGBBAA`, alpha dropped); triangle pid/p1 → object pid/pindex → None; unit scale micron 0.001 / centimeter 10 / inch 25.4 / foot 304.8 / meter 1000 / default 1; transforms composed child-first-then-parent (row-vector); name chain skipping empty names; `<component>` accepted with or without a `<components>` wrapper.

- [ ] **Step 5: PASS on thelio.** Retire the spikes (git-safety compliant):

```bash
rm engine/tests/parse3mf.rs engine/tests/threemf_spike.rs
```

promote quick-xml/zip to engine `[dependencies]`, remove `threemf` from dev-deps, and stage the deletions in the task commit (below).

- [ ] **Step 6: Commit**

```bash
git add engine/src/parsing/threemf.rs engine/tests/threemf.rs engine/Cargo.toml Cargo.lock fixtures/handmade/make_fixtures.py fixtures/handmade/colorgroup.3mf fixtures/handmade/units.3mf fixtures/handmade/nested.3mf fixtures/handmade/nosort.3mf engine/tests/parse3mf.rs engine/tests/threemf_spike.rs
git commit -m "feat(engine): full 3MF parser — units, colorgroups, pid/p1, nested transforms, string-sorted fallback; spikes retired"
git push thelio main
```

---

### Task 5: animation.json — parse + interpolation

**Files:**
- Create: `engine/src/parsing/animation.rs`, `engine/tests/animation.rs`

**Interfaces:**
- Produces: `engine::parsing::animation::{MotionStudy, Frame, Pose, Collision, StudyState}`; `MotionStudy::parse(&[u8]) -> Result<MotionStudy, String>`, `state_at(f64) -> StudyState`, `is_active()`, `duration()`, `value_names() -> Vec<&str>`. M7 consumes `state_at`.

Semantics corrected against `fixtures/upstream-source/MotionStudy.swift`:
- `translate` array of exactly 3 → value; **any other arity → `[0,0,0]`** (zero, not padded).
- `rotate` array of exactly 4 → normalized on load; else `Quat::IDENTITY`.
- Label at time t = `frames.iter().rev().find(|f| f.t <= t && f.label.is_some())` — an unlabeled frame does NOT clear an earlier label.
- A part present in only one of two surrounding frames keeps that frame's pose for the whole span (upstream holds it).
- "At most 4 values" is the agent's writing constraint — the parser does NOT enforce it.
- `value_names()`: union of `values` keys, first-seen frame order, then sorted (upstream valueNames).

- [ ] **Step 1: Failing tests** — `engine/tests/animation.rs` (rev 1's suite with these corrections):

```rust
#[test]
fn malformed_entries_default_to_identity() {
    let s = MotionStudy::parse(br#"{"frames":[
        {"t": 1.0, "parts": {"a": {"rotate": [0.0, 2.0, 0.0, 2.0]}}},
        {"t": 0.0, "parts": {"a": {"translate": [1.0, 2.0]}}}
    ]}"#).unwrap();
    assert_eq!(s.frames[0].t, 0.0, "sorted");
    let a0 = &s.frames[0].parts["a"];
    assert_eq!(a0.translate, [0.0, 0.0, 0.0], "wrong-arity translate -> zero (upstream), not padded");
    assert_eq!(a0.rotate, glam::Quat::IDENTITY);
    let a1 = &s.frames[1].parts["a"];
    let r = a1.rotate; // [0,2,0,2] -> normalized [0, √2/2, 0, √2/2]
    assert!((r.y - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5, "NON-UNIT input normalized (length was 2√2)");
    assert!((r.w - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
}

#[test]
fn label_persists_through_unlabeled_frames() {
    let s = MotionStudy::parse(br#"{"frames":[
        {"t": 0.0, "label": "closed"},
        {"t": 1.0},
        {"t": 2.0, "label": "open"}
    ]}"#).unwrap();
    assert_eq!(s.state_at(0.5).label.as_deref(), Some("closed"));
    assert_eq!(s.state_at(1.5).label.as_deref(), Some("closed"), "unlabeled frame does NOT clear");
    assert_eq!(s.state_at(2.0).label.as_deref(), Some("open"));
}

#[test]
fn part_missing_from_one_frame_holds_its_pose() {
    let s = MotionStudy::parse(br#"{"frames":[
        {"t": 0.0, "parts": {"lid": {"translate": [0.0, 0.0, 0.0]}}},
        {"t": 2.0, "parts": {"lid": {"translate": [0.0, 0.0, 10.0]},
                             "cam": {"translate": [1.0, 0.0, 0.0]}}}
    ]}"#).unwrap();
    let mid = s.state_at(1.0);
    assert_eq!(mid.parts["cam"].translate, [1.0, 0.0, 0.0], "part only in the later frame holds that pose across the span");
    assert_eq!(mid.parts["lid"].translate, [0.0, 0.0, 5.0]);
}
```

(Plus rev 1's unchanged tests: `golden_parses_sorted_with_three_frames`, `interpolation_lerps_translates_and_slerps_rotations` — with the collision-nearest-frame tie→later rule — `slerp_quaternions_halfway`, `single_frame_study_is_inert`.)

- [ ] **Step 2: FAIL; Step 3: Implement** per the corrected semantics above (hand-written `Deserialize` for `Pose` for the arity rules; `StudyState { parts: HashMap<String, Pose>, values: BTreeMap<String, f64>, label: Option<String>, collisions: Vec<Collision> }`).

- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): animation study — upstream-faithful defaults, held poses, persistent labels, lerp/slerp`.

---

### Task 6: Output requirements — redesigned on `Project.missingRequirements`

**Files:**
- Create: `engine/src/project/outputs.rs`, `engine/tests/outputs.rs`, `fixtures/handmade/one-page.pdf`
- Modify: `engine/Cargo.toml` (lopdf via workspace)

**Interfaces:**
- Consumes: `engine::project::manifest::Manifest` (Task 2), `lopdf` (workspace dep).
- Produces:

```rust
pub struct MissingCtx<'a> {
    pub project_root: &'a Path,      // model.py existence gates everything
    pub parts_count: usize,          // from the PARSED 3MF (upstream output.parts.count), 0 if unparsed
    pub printer_model: Option<&'a str>,
    pub manifest: &'a Manifest,
}
pub fn expected_outputs() -> [&'static str; 7]
pub fn pdf_page_count(path: &Path) -> Result<u32, String>
pub fn missing_requirements(ctx: &MissingCtx) -> Vec<String>
```

Semantics verbatim from `fixtures/upstream-source/Project.swift:221-243` (read it; it is the authority):
1. `model.py` missing → return `Vec::new()` (no note on first turn).
2. Expected outputs: reported when the file does not exist (existence only — NOT size) as the bare filename.
3. `plate.3mf` missing → `"plate.3mf (print plate)"` (unconditional on parts count — single-piece objects still get plates).
4. `parts_count > 1 && !exists(schematic-2.svg)` → `"one drawing page per piece (schematic-2.svg, …) with the parts list on page 1"` — a single check for page 2 only.
5. `drawing_pages_on_disk().count() > 1 && manifest.drawings.is_empty()` → ``"`drawings` in manifest.json (page titles)"`` where drawing pages = `schematic.svg` + `schematic-<int>.svg` discovered on disk (upstream ProjectOutput.discoverDrawingPages — sorted by the integer).
6. `manifest.bed.is_none() && printer_model.is_some()` → ``"`bed` in manifest.json (bed size of the user's printer)"``.
7. `drawing_pages_on_disk().count() > 1 && pdf_page_count(schematic.pdf)? < drawing_pages.count()` → `"schematic.pdf with all drawing pages in one file (PdfPages)"`.

- [ ] **Step 1: One-page PDF fixture** — write a minimal valid 1-page PDF to `fixtures/handmade/one-page.pdf` by hand (~15 lines of PDF text; verify with `python3 -c "import zlib" ` nothing needed — plain PDF, no streams). Verify on thelio: `cargo test` pdf_page_count on it == 1.

- [ ] **Step 2: Failing tests** — `engine/tests/outputs.rs` (each branch's precondition actually engineered):

```rust
use engine::project::manifest::Manifest;
use engine::project::outputs::{expected_outputs, missing_requirements, pdf_page_count, MissingCtx};
use std::path::PathBuf;

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
}
fn fixture(p: &str) -> PathBuf { golden().join(p) } // schematic-2 lives in golden

fn copy_golden(extra: &[&str]) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("cad-outputs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let all = ["model.3mf", "model.stl", "model.step", "schematic.svg",
               "schematic.png", "schematic.pdf", "manifest.json", "schematic-2.svg"];
    for f in all { std::fs::copy(golden().join(f), tmp.join(f)).unwrap(); }
    for f in extra { std::fs::copy(fixture(f), tmp.join(f)).unwrap(); }
    tmp
}
fn m(root: &PathBuf) -> Manifest { Manifest::load(&root.join("manifest.json")).unwrap() }

#[test]
fn expected_outputs_list_matches_contract() {
    assert_eq!(expected_outputs(), [
        "model.3mf", "model.stl", "model.step", "schematic.svg",
        "schematic.png", "schematic.pdf", "manifest.json",
    ]);
}

#[test]
fn golden_pdf_has_two_pages_and_one_page_fixture_has_one() {
    assert_eq!(pdf_page_count(&golden().join("schematic.pdf")).unwrap(), 2);
    assert_eq!(pdf_page_count(&fixture("../handmade/one-page.pdf")).unwrap(), 1);
}

#[test]
fn complete_golden_reports_nothing() {
    let root = copy_golden(&[]);
    let man = m(&root);
    assert!(missing_requirements(&MissingCtx {
        project_root: &root, parts_count: 2,
        printer_model: Some("Bambu Lab P1S"), manifest: &man,
    }).is_empty());
}

#[test]
fn no_model_py_means_no_note() {
    let root = copy_golden(&[]);
    std::fs::remove_file(root.join("model.py")).ok(); // never existed — fine
    // craft a root WITHOUT model.py: use a fresh dir with only output files
    let out_only = root.join("output"); std::fs::create_dir_all(&out_only).unwrap();
    // (the ctx takes project_root; model.py checked at root)
    let man = m(&root);
    let missing = missing_requirements(&MissingCtx {
        project_root: "/nonexistent", parts_count: 2,
        printer_model: Some("P1S"), manifest: &man,
    });
    assert!(missing.is_empty(), "guard: no model.py -> no note");
}

#[test]
fn missing_expected_outputs_and_plate_are_bare_names() {
    let root = copy_golden(&[]);
    std::fs::remove_file(root.join("model.step")).unwrap();
    std::fs::remove_file(root.join("plate.3mf")).unwrap(); // never copied — absence is the point
    let man = m(&root);
    let missing = missing_requirements(&MissingCtx {
        project_root: &root, parts_count: 2, printer_model: Some("P1S"), manifest: &man,
    });
    assert!(missing.contains(&"model.step".to_string()));
    assert!(missing.contains(&"plate.3mf (print plate)".to_string()));
}

#[test]
fn per_piece_page_demanded_only_with_parts_and_only_page_two() {
    let root = copy_golden(&[]);
    std::fs::remove_file(root.join("schematic-2.svg")).unwrap();
    let man = m(&root);
    let missing = missing_requirements(&MissingCtx {
        project_root: &root, parts_count: 2, printer_model: Some("P1S"), manifest: &man,
    });
    assert!(missing.iter().any(|s| s.contains("one drawing page per piece")), "{missing:?}");
    // single part -> not demanded
    let missing1 = missing_requirements(&MissingCtx {
        project_root: &root, parts_count: 1, printer_model: Some("P1S"), manifest: &man,
    });
    assert!(!missing1.iter().any(|s| s.contains("one drawing page per piece")));
}

#[test]
fn drawings_demanded_only_when_disk_has_multiple_pages_and_manifest_lacks_them() {
    let root = copy_golden(&[]); // disk: schematic.svg + schematic-2.svg (2 pages)
    let mut man = m(&root);
    man.drawings.clear();
    let missing = missing_requirements(&MissingCtx {
        project_root: &root, parts_count: 2, printer_model: Some("P1S"), manifest: &man,
    });
    assert!(missing.iter().any(|s| s.contains("`drawings` in manifest.json")));
    // single disk page -> not demanded even with empty manifest drawings
    std::fs::remove_file(root.join("schematic-2.svg")).unwrap();
    let missing = missing_requirements(&MissingCtx {
        project_root: &root, parts_count: 2, printer_model: Some("P1S"), manifest: &man,
    });
    assert!(!missing.iter().any(|s| s.contains("`drawings` in manifest.json")));
}

#[test]
fn bed_demanded_only_with_a_configured_printer() {
    let root = copy_golden(&[]);
    let mut man = m(&root);
    man.bed = None;
    let with_printer = missing_requirements(&MissingCtx {
        project_root: &root, parts_count: 2, printer_model: Some("P1S"), manifest: &man,
    });
    assert!(with_printer.iter().any(|s| s.contains("`bed` in manifest.json")));
    let no_printer = missing_requirements(&MissingCtx {
        project_root: &root, parts_count: 2, printer_model: None, manifest: &man,
    });
    assert!(!no_printer.iter().any(|s| s.contains("`bed` in manifest.json")));
}

#[test]
fn pdf_demanded_when_fewer_pages_than_drawing_pages() {
    let root = copy_golden(&[]); // 2 drawing pages on disk
    std::fs::copy(fixture("../handmade/one-page.pdf"), root.join("schematic.pdf")).unwrap();
    let man = m(&root);
    let missing = missing_requirements(&MissingCtx {
        project_root: &root, parts_count: 2, printer_model: Some("P1S"), manifest: &man,
    });
    assert!(missing.iter().any(|s| s.contains("schematic.pdf with all drawing pages")), "{missing:?}");
}
```

- [ ] **Step 3: FAIL; Step 4: Implement** `engine/src/project/outputs.rs` exactly per the seven-branch semantics (Project.swift:221-243 vendored — transcribe conditions, don't paraphrase; `pdf_page_count` via `lopdf::Document::load(path).map_err(...)?.get_pages().len()`).

- [ ] **Step 5: PASS; Step 6: Commit** `feat(engine): output requirements — upstream missingRequirements branches verbatim, PDF page count`.

---

### Task 7: Project store — metadata, chat, layout, recents

**Files:**
- Create: `engine/src/project/layout.rs`, `engine/src/project/store.rs`, `engine/tests/project.rs`
- Modify: `engine/src/project/mod.rs` (re-exports)

**Interfaces:**
- Produces: `engine::project::{ProjectMetadata, ChatMessage, ToolActivity, Role, Action, ToolState, RecentProjects, validate_project_name, load_metadata, save_metadata, load_chat, save_chat}` + `ProjectMetadata::new(app_version: &str)`; `engine::project::layout::{LayoutNode, PaneKind, Axis}`.

Shapes per `fixtures/upstream-source/ProjectStore.swift` + `ChatMessage.swift`:
- project.json keys are the **explicit snake_case CodingKeys**: `session_id`, `current_version`, `created_at`, `updated_at`, `app_version`, `layout`.
- Dates: whole-second UTC ISO8601 (`...Z`) — upstream's `.iso8601` formatter rejects fractional seconds, so a time-rfc3339 default with nanoseconds would be unreadable by the macOS app. Custom serde helpers:

```rust
pub mod iso8601_utc {
    use serde::{Deserializer, Serializer};
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    pub fn serialize<S: Serializer>(t: &OffsetDateTime, s: S) -> Result<S::Ok, S::Error> {
        let t = t.to_utc().replace_nanosecond(0).map_err(serde::ser::Error::custom)?;
        s.serialize_str(&t.format(&Rfc3339).map_err(serde::ser::Error::custom)?)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<OffsetDateTime, D::Error> {
        let text = String::deserialize(d)?;
        OffsetDateTime::parse(&text, &Rfc3339).map_err(serde::de::Error::custom)
        // parses fractional seconds too — tolerant on read, strict-truncated on write
    }
}
```

- ToolActivity's field is `toolUseID` (property name — needs `#[serde(rename = "toolUseID")]` since camelCase rename yields `toolUseId`).
- LayoutNode JSON: `{"pane":"chat"|"model3D"|"schematic"}` / `{"split":{"axis":"row"|"column","first":…,"second":…,"fraction":…}}` — `PaneKind::Model3D` serializes as `"model3D"` (mind the casing; hand-write the serde if the derive fights it).

- [ ] **Step 1: Failing tests** — `engine/tests/project.rs` (rev 1's suite with corrected shapes):

```rust
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
    for k in ["\"session_id\"", "\"current_version\"", "\"created_at\"",
              "\"updated_at\"", "\"app_version\""] {
        assert!(json.contains(k), "snake_case CodingKeys: missing {k}");
    }
    assert!(json.contains("\"pane\":\"chat\""));
    assert!(json.contains("\"pane\":\"model3D\""), "exact casing");
    assert!(json.contains("\"axis\":\"row\""));
    assert!(json.contains("\"fraction\":0.34"));
    assert!(!json.contains('.'), "whole-second dates, no fractional seconds: {json}");
    assert!(json.contains('Z'), "UTC");
    let back: ProjectMetadata = serde_json::from_str(&json).unwrap();
    assert_eq!(back.layout, m.layout);
}
```

(Plus rev 1's tests with these deltas: `chat_roundtrips_all_roles_and_tools` asserts `json.contains("\"toolUseID\"")`; `saves_are_atomic_no_tmp_residue`, `project_name_validation_rules`, `recents_dedupe_evict_and_filter_missing` unchanged; loads go through `.cadstudio/project.json` + `.cadstudio/chat.json` paths.)

- [ ] **Step 2: FAIL; Step 3: Implement** `store.rs` + `layout.rs` per the shapes above (atomic temp+rename saves; `RecentProjects` add/dedupe-by-canonicalized/max-12/filter-existing; `validate_project_name`: non-empty, no leading ".", no "/" or ":").
- **Step 4: PASS; Step 5: Commit** `feat(engine): project store — snake_case metadata, whole-second UTC dates, toolUseID, atomic saves, recents`.

---

### Task 8: Output watcher

**Files:**
- Create: `engine/src/watch.rs`, `engine/tests/watch.rs`
- Modify: `engine/src/lib.rs` (`pub mod watch;`)

**Interfaces:**
- Produces: `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)] pub enum ReloadKind { Manifest, Plate, Motion, Mesh, Schematic }`; `watch_directory(dir: &Path, debounce: Duration) -> notify::Result<(WatchHandle, Receiver<ReloadKind>)>`. **The inotify watch is registered synchronously before the function returns** (events for writes made immediately after are never missed). Mapping per contract §3 (manifest.json/plate.3mf/animation.json/model.3mf|model.stl/schematic*.svg|png). Non-recursive, output/ only. `WatchHandle` stops on Drop.

- [ ] **Step 1: Failing tests** — rev 1's four tests unchanged (`maps_files_to_reload_kinds`, `storm_collapses_to_one_reload_per_file`, `second_write_after_quiet_emits_again`, `untouched_files_never_reload`) at `FAST = 30ms`.
- [ ] **Step 2: FAIL; Step 3: Implement** with **trailing-edge debounce pinned**: the collector resets a per-path timer on every event and emits only after `debounce` of quiet — so N writes inside one window collapse to exactly one reload (this is what makes the storm test deterministic rather than flaky on a loaded box). Mod-time dedup map starts empty (pre-existing files emit nothing without events); a path re-emits when its mod-time moves again after a quiet period.
- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): output watcher — trailing-edge debounce, mod-time dedup, kind mapping`.

---

### Task 9: M1 wrap

**Files:**
- Modify: `engine/src/lib.rs`, `engine/src/project/mod.rs`, `docs/research/carry-forwards.md`

- [ ] **Step 1: lib.rs crate docs** — module map (settings, project::{manifest,outputs,store,layout}, parsing::{stl,threemf,animation}, watch) + a note that the `EngineCommand`/`EngineEvent` surface (spec §4.2) arrives with M2's agent session work.
- [ ] **Step 2: carry-forwards.md strikes**: strike M1 bullets 1–4 as landed; strike bullet 5 (contract:102 phrasing — consumed by Task 4's spec-order work); leave golden-facts. ADD two new lines: (a) M4: mesh reload prefers 3MF, falls back to STL with parts `[]` — consumer-side, per upstream parseMesh; (b) M4: project-folder scaffolding (upstream ProjectStore.prepare/create: create output/, .cadstudio/logs, default project.json — GAP deliberately deferred from M1); (c) M8: AppleLanguages setting intentionally omitted in M1 (language override lands with i18n).
- [ ] **Step 3: Full gate**

```bash
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && git pull -q && cargo test --workspace --all-targets 2>&1 | grep -E \"(running|test result|warning)\" | tail -20"'
```

Expected: M0's surviving 8 tests (5 spike tests retired in Task 4) + ~40 new, zero warnings.
- [ ] **Step 4: Commit** `docs: M1 complete — lib surface, carry-forward updates`; push; report.
