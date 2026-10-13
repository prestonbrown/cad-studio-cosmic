# M5 Python + Params + Versions Implementation Plan (rev 2 — independent review applied: 3B/4M/12m/4N folded)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Python layer the whole product contract stands on — venv creation/verification with the setup sheet, parameter sliders that rewrite `model.py` and rebuild it agent-less, and version snapshot/restore with the version menu and compare-picker — so a slider drag produces a new rendered model and a restorable version, exactly as upstream does.

**Architecture:** Engine-first, per spec §4.2: new `engine::python::{env, model}` (venv state machine + install planner with injected spawn/probe seams — discovery acceptance is Probe-mediated; `model.py` runner over the existing `processes::spawn_streaming`) and `engine::project::{parameters, versions}` (regex rewrite + version store) — all GUI-free and unit-testable with fakes, no network. The app extends M4's Bridge with a build pipeline (drive `model.py`, record versions, gate sends on a ready Python env), and adds three UI pieces around pure models: the ParametersOverlay, the VersionMenu, and the PythonSetupSheet. The engine's session prompt already carries the venv interpreter path; M5 makes it carry the verified package-versions line too, via a shared versions cell that mirrors upstream's `PythonEnvironment.shared` singleton.

**Tech Stack:** Rust edition 2024; new workspace dep `regex = "1"` (parameter rewrite; no backreferences/lookaround needed, ICU-compatible for this pattern). Everything else is already pinned: libcosmic git rev `dbc62f2` + wgpu 28, tokio, serde/time/uuid/thiserror/directories. No new app-side dependencies.

**Spec:** docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md AS AMENDED (§4.2 engine crate `python/` + `project/versions.rs`/`parameters.rs`, §4.4 app crate slider/Python-setup-sheet lines, §10 M5). **Pending spec amendments the controller owns** (rev-1 review adjudications, do not edit the spec here): G1 §10 compare split — M5 owns the picker, `comparison_parameters`, and the struck-through previous values; M6 only the ghost mesh; G2 §4.2 — the session prompt's python line reads the shared versions cell, unverified renders the exact fallback; G3 §3 — recording triggers per BLOCKER 3 below (turn-path outputs gate; build path records on `has_changes` alone). Behavior source of truth: docs/research/upstream-contract.md §2 (Python environment), §4 (version history), §5 (parameters overlay); vendored Swift at fixtures/upstream-source/ ed2ccaf — Python/{PythonEnvironment,ModelBuilder,PythonSetupSheet}.swift, Project/VersionHistory.swift, Project.swift:308-416 (restore/record/setParameter/build), Viewer3D/ParametersOverlay.swift, Shared/VersionMenu.swift. Carry-forwards M5 section (docs/research/carry-forwards.md:76-100) is BINDING. Where this plan and a Swift file disagree, the Swift file wins and the discrepancy goes to the controller. This plan assumes M4 is merged (Bridge, viewer_pane, sheets, activity, app exist as the M4 plan declares them).

## Global Constraints

- Engine stays GUI-free (spec §4.2): new modules `engine/src/python/`, `engine/src/project/{parameters,versions}.rs` import no cosmic/iced types. User-visible strings the ENGINE emits are English display consts mirroring the en catalog values (house pattern: `BUSY_FRESH_MESSAGE` in session.rs); app-owned static labels use `fl!()` with keys from app/i18n/en/main.ftl (all M5 keys already exist in the 336-key corpus — grep the slug, never invent a new one).
- Venv location `~/.local/share/cad-studio/venv`, interpreter `<venv>/bin/python` — reached through ONE canonical data-dir helper (`directories::ProjectDirs::from("", "", "cad-studio").data_dir()`, the convention `processes::registry_path()` already chose; on macOS that resolves `~/Library/Application Support/cad-studio/venv` — thelio is Linux and authoritative). `session.rs::app_venv_python()` is rerouted through the same helper in T4 so the `{{PYTHON}}` prompt path and the venv helper can never diverge (MINOR 10). Packages `["build123d", "matplotlib", "numpy"]`, unpinned.
- Install commands verbatim (PythonEnvironment.swift:80-98): uv path `uv venv --python 3.12 <dir>` then `uv pip install --python <interpreter> build123d matplotlib numpy`; fallback `<python> -m venv <dir>`, `<interpreter> -m pip install --upgrade pip`, `<interpreter> -m pip install build123d matplotlib numpy`. Python candidates in order `python3.13, python3.12, python3.11, python3.10, python3`, accepted only when `--version` parses to 3.10–3.13.
- Progress milestones 0.05 (python search) / 0.1 (venv create) / 0.2 (pip upgrade) / 0.25 (install) / 0.95 (verify) / 1.0 (done), each advancing BEFORE its step runs; per-output-line nudge `+0.004` only while `0.25 ≤ progress < 0.9`.
- Verification script EXACTLY (PythonEnvironment.swift:135-139):
  `import sys, build123d, matplotlib, numpy\nprint(build123d.__version__)\nprint(f"Python {sys.version.split()[0]}, build123d {build123d.__version__}, matplotlib {matplotlib.__version__}, numpy {numpy.__version__}")`
  Line 1 = build123d version; line 2 = the `versions` string. Empty/failed verification displays upstream's exact fallback `"build123d, matplotlib, numpy"` (carry-forward; AgentSession.swift:206).
- Model runs: `<interpreter> model.py`, cwd = project root, env = `locator::shell_env()` + `MPLBACKEND=Agg` + `PYTHONUNBUFFERED=1`, stdin null (spawn_streaming guarantees it). Failure detail = `(stderr if non-empty else stdout)` trimmed; `buildError` = its LAST line.
- Parameter regex verbatim (Project.swift:346, upstream-contract.md §2:80): `(?m)^\s*<regex-escaped-name>\s*(?::[^=\n]+)?=\s*([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)` — first match only, only the captured literal is replaced; number formatting: integer string iff `value == value.rounded() && !original.contains('.')`, else shortest round-trip (`{}`); write is atomic (temp + rename).
- Version snapshot layout `<project>/.cadstudio/versions/<n>/{model.py, output/* (full copy), chat.json, version.json}`; `version.json` keys `number`, `created_at` (ISO 8601), `prompt`, `session_id` (nullable). Restore: copy `model.py` + `chat.json` back, DELETE every file in `output/`, copy the version's `output/` in; the caller swaps `metadata.session_id` ← `version.session_id` and `current_version` ← `n`.
- Version recording gates (upstream parity, BLOCKER 3): after an **agent-turn success**, only when ALL of `expected_outputs()` exist (the 7 files — NOT the richer prompt-note list) — Project.swift:269-271; after a **manual or slider build success**, unconditionally — `buildDidFinish` calls `recordVersion` with no outputs check, Project.swift:397-401; BOTH paths additionally gated on `model.py` differing from the current version's copy (`has_changes`, the only guard inside `recordVersion`, Project.swift:327). A `summary` chat row ("Version N", detail = number) is appended before the snapshot is taken.
- Slider model (ParametersOverlay.swift:190-198): range around v: `v > 0 → 0..max(2v, 1)`, `v < 0 → 2v..max(-2v, 1)`, `0 → -10..10`; commit rounds the slider's LIVE value to integer when the PARAMETER's COMMITTED value is integral with `|v| ≥ 1` (upstream `isInteger` keys on `parameter.value`, ParametersOverlay.swift:134-136 — parameter 3, drag 4.4 commits 4), else to 3 decimals; a commit equal to the parameter's current value is a no-op.
- Tests never run real `uv`/`pip`/`build123d` and never touch the network: process tests use scripted shell/Python fakes under `fixtures/fake-agent/` with injected spawn seams or argv-dump files (house patterns: fake_claude.py, ENV_LOCK in engine/tests/session_lifecycle.rs:22). App async tests never `env::set_var`; engine tests that set env vars hold ENV_LOCK and wrap in `unsafe` with a SAFETY comment.
- Implementers run `cargo check -p <crate>` + the task's focused tests locally ONLY; thelio full-suite gates are controller-owned, single-invocation, never stacked, HEAD-verified after pull.
- Edition 2024 (no bare `env::set_var`), libcosmic rev `dbc62f2d899cdbfbe43d170689ddc0290db663f4` + wgpu 28 exact, conventional commits, stage explicit paths only (never `git add -A`/`-u`, never `git rm`). Parallel-dispatch cap ≤ 5 agents, reviewers staggered.

## Review Focus

Five failure modes the spec implies but ordinary task tests might miss — each pinned to its owning task's named test:

1. **The rewrite regex corrupts model.py on adversarial lines** — comparisons (`total == 3`), comments (`# width = 5`), name-prefix collisions (`width_mm` when rewriting `width`), typed/indented/spaced assignments must each either match surgically or refuse, never mangle (T1 `adversarial_lines_never_match_or_mangle`).
2. **A rebuild or restore racing an agent turn** — `setParameter`, `build`, and `restore` all guard on busy upstream; a rebuild issued mid-turn must be refused, not interleaved (T6 `set_parameter_and_build_refused_while_busy`).
3. **The version-record gates split by path** — an agent-turn success with no `model.3mf` must NOT bump the version or append a summary row (outputs gate is turn-path only, Project.swift:269-271); a successful manual/slider build records on `has_changes` ALONE even with outputs missing (Project.swift:397-401 → :327); and a second build with unchanged `model.py` records nothing (T6 `turn_success_without_outputs_records_no_version`, `build_with_missing_outputs_records_on_changes_alone`, `unchanged_model_py_records_no_second_version`).
4. **Unverified env poisons the agent prompt** — before any verification the `{{ENVIRONMENT}}` python line must read `build123d, matplotlib, numpy`, and after verification the real versions line (T4 `python_display_fallback_and_verified_line`).
5. **Shutdown erases still-live children from the pid registry** — a just-cancelled child this process still owns must remain listed so a later launch can reap it (T4 `reap_keeps_live_children_listed`).

---

### Task 0: Engine `python::env` — venv state machine and install planner

**Files:**
- Create: `engine/src/python/mod.rs` (`pub mod env; pub mod model;` — create `model.rs` as an empty `//!` placeholder doc file so the module compiles; Task 2 fills it), `engine/src/python/env.rs`
- Modify: `engine/src/lib.rs` (register `pub mod python;`), `engine/src/agent/locator.rs` (add `find_executable`), `Cargo.toml` + `engine/Cargo.toml` (no new deps this task)
- Test: `engine/tests/python_env.rs`

**Interfaces:**
- Consumes: `crate::agent::locator::{shell_env, find_executable, executable_dirs}` (the latter two new), `crate::processes::{spawn_streaming, RunningProcess}`, `crate::settings` paths pattern (`directories::ProjectDirs::from("", "", "cad-studio").data_dir()`).
- Produces (exact; later tasks and the app consume verbatim):
  - `pub const PACKAGES: [&str; 3] = ["build123d", "matplotlib", "numpy"];`
  - `pub const VERSIONS_FALLBACK: &str = "build123d, matplotlib, numpy";`
  - `pub const VERIFY_SCRIPT: &str;` — the two-line script verbatim from Global Constraints.
  - `pub fn venv_dir() -> PathBuf` — `<data_dir>/cad-studio/venv`, built on the SAME `directories::ProjectDirs::from("", "", "cad-studio").data_dir()` base as `processes::registry_path()` (MINOR 10: one canonical data-dir convention — on Linux the Global-Constraint path byte-for-byte, on macOS `~/Library/Application Support/cad-studio/venv`, thelio authoritative; T4 reroutes `session.rs::app_venv_python()` through this helper). Overridable for tests by the injected dir parameter everywhere else (only the path helper is ambient; all functions take `dir: &Path`).
  - `pub fn interpreter_path(dir: &Path) -> PathBuf` — `dir/bin/python`.
  - `#[derive(Debug, Clone, PartialEq)] pub enum EnvState { Unknown, Checking, Missing, Installing, Ready { build123d_version: String }, Failed(String) }`
  - `pub struct InstallStep { pub progress: f64, pub step: &'static str, pub argv: Vec<String> }` (program first)
  - `pub fn uv_install_plan(uv: &Path, dir: &Path) -> Vec<InstallStep>` / `pub fn venv_install_plan(python: &Path, dir: &Path) -> Vec<InstallStep>`
  - `pub fn parse_python_version(output: &str) -> Option<(u32, u32)>` — `"Python 3.12.4\n"` → `Some((3, 12))`
  - `pub fn is_supported_python(major: u32, minor: u32) -> bool` — `(major, minor) == (3, 10..=13)`
  - `pub fn python_candidates() -> [&'static str; 5]`
  - `pub fn parse_verification(output: &str) -> Option<(String, String)>` — `(build123d_version, versions_line)`; each line TRIMMED before use (upstream trims per line, PythonEnvironment.swift:143 — `\r\n` interpreters parse identically; MINOR 6); `""` versions when only one line printed.
  - `pub fn nudge_progress(progress: f64) -> f64`
  - The shared versions cell (upstream `PythonEnvironment.shared.versions` parity; one venv serves every project window): `pub fn shared_versions() -> Arc<std::sync::RwLock<Option<String>>>` (a `static` LazyLock), `pub fn set_shared_versions(Option<String>)`, `pub fn versions_display() -> String` — cell value or `VERSIONS_FALLBACK`.
  - Spawn seam: `pub type Spawn = std::sync::Arc<dyn Fn(&[String], &Path) -> std::io::Result<RunningProcess> + Send + Sync>;` and `pub fn streaming_spawn() -> Spawn` (wraps `processes::spawn_streaming` with `locator::shell_env()` as env). Probe seam: `pub type Probe = std::sync::Arc<dyn Fn(&Path, &[String]) -> Option<(i32, String)> + Send + Sync>;` (`(exit_code, combined stdout+stderr)`) and `pub fn command_probe() -> Probe` (std::process::Command, `Stdio::null()` stdin, env = `shell_env()`). BLOCKER 2, ADJUDICATED as probe-mediated discovery (no Locate seam): uv is located by probing `locator::executable_dirs(&shell_env())` candidates — `probe(dir.join("uv"), ["--version"])`, exit 0 wins — and each python candidate is `locator::find_executable(name, &shell_env())` then probed; ACCEPTANCE is always the injected Probe, so no host binary is ever executed or trusted without the seam's consent and tests steer discovery entirely through fakes (a `None`/nonzero probe makes even a uv-present host take the python branch), satisfying the Global Constraint "tests never run real uv".
  - Events + driver (MAJOR 1, adjudicated shape (b) — typed failure so `Missing` reaches T8's reducer through the terminal event; no engine-side Started marker: the app sets `EnvState::Checking`/`Installing` itself when it kicks the drivers, the review's sanctioned alternative): `#[derive(Debug, Clone, PartialEq)] pub enum EnvEvent { Log(String), Progress { value: f64, step: String }, Finished(Result<EnvReady, EnvFailure>) }` — `check` emits only its terminal; `install`/`reinstall` stream Log/Progress then the terminal. `#[derive(Debug, Clone, PartialEq)] pub enum EnvFailure { Missing, CheckFailed(String), InstallFailed(String) }` with `pub fn message(&self) -> String` — `Missing` → `""` (upstream has no missing string), the others return the exact EN catalog string they carry; terminal mapping: interpreter absent → `Err(EnvFailure::Missing)`, check parse/exit failure → `Err(EnvFailure::CheckFailed(CHECK_FAILED_MESSAGE))`, every install error → `Err(EnvFailure::InstallFailed(<message>))`. `pub struct EnvReady { pub build123d_version: String, pub versions: String }`; `pub struct EnvHandle { pub events: tokio::sync::mpsc::Receiver<EnvEvent> }`; `pub fn check(dir: &Path, spawn: &Spawn) -> EnvHandle`; `pub fn install(dir: &Path, spawn: &Spawn, probe: &Probe) -> EnvHandle`; `pub fn reinstall(dir: &Path, spawn: &Spawn, probe: &Probe) -> EnvHandle` (removes `dir` ignoring errors, then `install`).
  - Error consts the driver emits, NAMED so tests pin them (EN catalog values, Global Constraints pattern): `pub const PYTHON_NOT_FOUND_MESSAGE: &str = "Neither uv nor Python 3.10–3.13 was found. Install uv (brew install uv) or Python 3.12 and try again."`; `pub const CHECK_FAILED_MESSAGE: &str = "The Python environment exists, but build123d or matplotlib can’t be imported."`; `pub const VERIFICATION_FAILED_MESSAGE: &str = "build123d and matplotlib can’t be imported in the new environment."`; `pub fn command_failed_message(name: &str) -> String` → `"The command “{name}” failed. Check the log for details."`; failure milestone step `"Installation failed"` (EN value of `installazione-non-riuscita`).
- locator additions: `pub fn find_executable(name: &str, env: &std::collections::HashMap<String, String>) -> Option<PathBuf>` — first search dir containing an executable file `name` — and `pub(crate) fn executable_dirs(env) -> Vec<PathBuf>` (the PATH dirs plus locator's fallbacks, `~/.local/bin` appended when absent; `find_executable` = `executable_dirs` + the private `is_executable_file`). Extraction only; no behavior change to existing locator fns.

**Driver semantics (transcribe PythonEnvironment.swift:34-146):**
- `check` emits ONLY its terminal. When `interpreter_path(dir)` is not an executable file: `Finished(Err(EnvFailure::Missing))` — no spawn (upstream `check()` distinguishes `missing` from `failed`, PythonEnvironment.swift:36-46; the typed `Missing` is what lets the app present the sheet only when `has_completed_setup`, MAJOR 1). Else run `[interpreter, "-c", VERIFY_SCRIPT]` through `spawn`: nonzero exit or unparseable output → `Finished(Err(EnvFailure::CheckFailed(CHECK_FAILED_MESSAGE)))` AND `set_shared_versions(None)` (NOTE 2: upstream leaves a stale `versions` string after a failed re-check — clearing is deliberate and equivalent in every reachable state; the module doc records the choice); parse ok → `set_shared_versions(Some(versions_line))` and `Finished(Ok(EnvReady { build123d_version, versions }))`.
- `install` (no engine-side started marker — the app sets `EnvState::Installing` when it kicks the driver): every milestone goes through `advance(value, step)` = `Progress { value, step }` THEN `Log(step)` (the milestone text lands in the log stream, upstream `advance()` appends it, PythonEnvironment.swift:148-152; MINOR 5), each advancing BEFORE its step runs; the step runner emits `Log("$ " + argv.join(" "))` then every output/stderr line → `Log(line)` + `Progress { value: nudge_progress(cur), step: cur_step }` (nudge accumulator step-local). Discovery (BLOCKER 2, probe-mediated): `locate_uv` probes `executable_dirs(&shell_env())` candidates' `uv` — first whose probe exits 0 wins — → uv plan; else `locate_python` scans `python_candidates()` in order: `find_executable(name, env)`, then `probe(path, ["--version"])`, accepting only exit code 0 ∧ `parse_python_version` ok ∧ `is_supported_python` (MINOR 9 — a parseable version behind a nonzero exit is skipped, PythonEnvironment.swift:103-108), emitting `Log("Found {version} at {path}")` (EN value of the ftl trovato-key) on the winner → venv plan; no winner → `PYTHON_NOT_FOUND_MESSAGE`. Search opens with `advance(0.05, STEP_SEARCHING)`. After the plan: `advance(0.95, "Verifying the installation…")`, run the verification through `spawn`; failure → `VERIFICATION_FAILED_MESSAGE`; success → `advance(1.0, "Components installed")`, set the shared cell, `Finished(Ok(..))`. ANY install failure first emits `Progress { value: <progress at failure>, step: "Installation failed" }` (EN `installazione-non-riuscita`), then `Log(message())`, then `Finished(Err(EnvFailure::InstallFailed(message)))`; a step exiting nonzero produces `command_failed_message(<that step's argv[0] file name>)`. Creates `dir`'s parent before the first step.
- All async work runs on `tokio::spawn`; `EnvHandle.events` is a bounded (256) channel, same pacing contract as turns.

- [ ] **Step 1: Write the failing tests** — `engine/tests/python_env.rs`:

```rust
//! python::env tests (M5 T0): the install planner, version parsing, the
//! verification contract, progress nudging, the shared versions cell, and
//! the check/install drivers against injected fake spawn/probe seams
//! (BLOCKER 2, adjudicated: discovery acceptance is Probe-mediated, so
//! the fakes steer it — no host binary is ever trusted or executed).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use engine::python::env::{
    self, EnvEvent, EnvFailure, PACKAGES, VERSIONS_FALLBACK, VERIFY_SCRIPT,
    uv_install_plan, venv_install_plan, parse_python_version, is_supported_python,
    parse_verification, nudge_progress, versions_display, set_shared_versions,
    CHECK_FAILED_MESSAGE,
};

/// The versions cell is process-global: every test that resets or asserts
/// it — including the drivers, which set it on success — serializes here
/// so a reset can never race a concurrent check/install.
static CELL_LOCK: Mutex<()> = Mutex::new(());

fn venv() -> PathBuf { std::env::temp_dir().join(format!("cad-env-{}", uuid::Uuid::new_v4())) }

#[test]
fn uv_plan_matches_upstream_argv() {
    let dir = venv();
    let steps = uv_install_plan(Path::new("/usr/local/bin/uv"), &dir);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].progress, 0.1);
    assert_eq!(steps[0].step, "Creating the virtual environment with uv…");
    assert_eq!(steps[0].argv, vec!["/usr/local/bin/uv", "venv", "--python", "3.12",
        dir.to_str().unwrap()]);
    assert_eq!(steps[1].progress, 0.25);
    assert_eq!(steps[1].step, "Installing build123d, matplotlib and numpy…");
    assert_eq!(steps[1].argv[1..5], ["pip", "install", "--python",
        dir.join("bin/python").to_str().unwrap()]);
    assert_eq!(&steps[1].argv[5..8], &PACKAGES[..]);
}

#[test]
fn venv_plan_matches_upstream_argv() {
    let dir = venv();
    let steps = venv_install_plan(Path::new("/usr/bin/python3.12"), &dir);
    assert_eq!(steps.len(), 3);
    assert_eq!((steps[0].progress, steps[0].step), (0.1, "Creating the virtual environment…"));
    assert_eq!(steps[0].argv, vec!["/usr/bin/python3.12", "-m", "venv", dir.to_str().unwrap()]);
    assert_eq!((steps[1].progress, steps[1].step), (0.2, "Updating pip…"));
    assert_eq!(steps[1].argv[1..], ["-m", "pip", "install", "--upgrade", "pip"]);
    assert_eq!((steps[2].progress, steps[2].step),
        (0.25, "Installing build123d, matplotlib and numpy…"));
    assert_eq!(&steps[2].argv[5..8], &PACKAGES[..]);
}

#[test]
fn python_version_parsing_and_support_window() {
    assert_eq!(parse_python_version("Python 3.12.4\n"), Some((3, 12)));
    assert_eq!(parse_python_version("Python 3.10\n"), Some((3, 10)));
    assert_eq!(parse_python_version("Python 3.9.7"), Some((3, 9)));
    assert_eq!(parse_python_version("Python 4.1"), Some((4, 1)));
    assert_eq!(parse_python_version("py3.12"), None);
    assert!(is_supported_python(3, 10) && is_supported_python(3, 13));
    assert!(!is_supported_python(3, 9) && !is_supported_python(4, 0));
    assert_eq!(env::python_candidates(),
        ["python3.13", "python3.12", "python3.11", "python3.10", "python3"]);
}

#[test]
fn verification_output_parses_into_versions_line() {
    assert_eq!(VERIFY_SCRIPT.lines().count(), 3);
    assert!(VERIFY_SCRIPT.contains("import sys, build123d, matplotlib, numpy"));
    let two = "0.8.1\nPython 3.12.4, build123d 0.8.1, matplotlib 3.9.2, numpy 2.1.0";
    assert_eq!(parse_verification(two).map(|(v, l)| (v.as_str(), l.as_str())),
        Some(("0.8.1", "Python 3.12.4, build123d 0.8.1, matplotlib 3.9.2, numpy 2.1.0")));
    assert_eq!(parse_verification("0.8.1").map(|(v, l)| (v.as_str(), l.as_str())),
        Some(("0.8.1", "")));
    assert_eq!(parse_verification(""), None);
    // MINOR 6: lines are trimmed — \r\n interpreters parse the same
    assert_eq!(parse_verification("0.8.1 \r\nPython 3.12.4, build123d 0.8.1\r\n"),
        Some(("0.8.1".into(), "Python 3.12.4, build123d 0.8.1".into())));
}

#[test]
fn nudge_only_inside_the_install_band() {
    assert_eq!(nudge_progress(0.1), 0.1);
    assert_eq!(nudge_progress(0.25), 0.254);
    assert_eq!((nudge_progress(0.899) * 1000.0).round(), 903.0);
    assert_eq!(nudge_progress(0.9), 0.9);
}

#[test]
fn versions_display_uses_upstream_fallback_until_verified() {
    // reset-test serialized against any other versions_display test
    let _cell = CELL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_shared_versions(None);
    assert_eq!(versions_display(), VERSIONS_FALLBACK);
    assert_eq!(VERSIONS_FALLBACK, "build123d, matplotlib, numpy");
    set_shared_versions(Some("Python 3.12.4, build123d 0.8.1".into()));
    assert_eq!(versions_display(), "Python 3.12.4, build123d 0.8.1");
    set_shared_versions(None);
}
```

  Driver tests (same file) use fake `Spawn`s that run `/bin/sh` scripts through the real `spawn_streaming` (so `RunningProcess` is genuine), steered by fake `Probe`s — BLOCKER 2 (adjudicated): discovery acceptance is the Probe's, so no host binary is trusted; and the spawn is argv-aware so one seam can both fake install steps and answer the verification:

```rust
/// Writes a shell script and returns a Spawn that runs it regardless of argv,
/// recording every argv it was asked to run.
fn scripted_spawn(dir: &Path, body: &str) -> (env::Spawn, Arc<Mutex<Vec<Vec<String>>>>) {
    std::fs::create_dir_all(dir).unwrap();
    let script = dir.join(format!("step-{}.sh", uuid::Uuid::new_v4()));
    std::fs::write(&script, body).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = Arc::clone(&seen);
    let script = script.to_path_buf();
    (
        Arc::new(move |argv: &[String], cwd: &Path| {
            seen2.lock().unwrap().push(argv.to_vec());
            let mut full = vec!["/bin/sh".to_string(), script.to_string_lossy().into_owned()];
            full.extend(argv.iter().cloned());
            engine::processes::spawn_streaming(&full, cwd, &shell_env_vec())
        }),
        seen,
    )
}

/// BLOCKER 2's argv-aware Spawn: install steps run the setup script
/// (which mints `<venv>/bin/python`); the verification step — whose
/// argv[0] IS that interpreter — runs the REAL argv, so the same seam
/// fakes installs and answers verification.
fn argv_aware_spawn(dir: &Path, bin: &Path, body: &str)
    -> (env::Spawn, Arc<Mutex<Vec<Vec<String>>>>) {
    std::fs::create_dir_all(dir).unwrap();
    let script = dir.join(format!("step-{}.sh", uuid::Uuid::new_v4()));
    std::fs::write(&script, body).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = Arc::clone(&seen);
    let (script, bin) = (script.to_path_buf(), bin.to_path_buf());
    (
        Arc::new(move |argv: &[String], cwd: &Path| {
            seen2.lock().unwrap().push(argv.to_vec());
            if argv.first().map(Path::new) == Some(bin.as_path()) {
                return engine::processes::spawn_streaming(argv, cwd, &shell_env_vec());
            }
            let mut full = vec!["/bin/sh".to_string(), script.to_string_lossy().into_owned()];
            full.extend(argv.iter().cloned());
            engine::processes::spawn_streaming(&full, cwd, &shell_env_vec())
        }),
        seen,
    )
}

/// The setup script that mints a verification-answering venv interpreter.
fn minting_script(bin: &Path) -> String {
    format!(
        "mkdir -p '{0}/bin'\nprintf '#!/bin/sh\\nprintf \"0.8.1\\\\nPython 3.12.4, build123d 0.8.1\\\\n\"\\n' > '{0}'\nchmod +x '{0}'\nexit 0\n",
        bin.display())
}

/// A Probe that "finds" uv: any candidate whose file name is uv exits 0.
/// BLOCKER 2's steering wheel — locate_uv accepts the first executable
/// dir whose uv the probe passes, so the fake decides, not the host.
fn uv_probe() -> env::Probe {
    Arc::new(|path: &Path, _| {
        (path.file_name().unwrap() == "uv").then(|| (0, "uv 0.5".into()))
    })
}

fn shell_env_vec() -> Vec<(String, String)> {
    engine::agent::locator::shell_env().into_iter().collect()
}

async fn drain(mut handle: env::EnvHandle) -> Vec<EnvEvent> {
    let mut events = Vec::new();
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), handle.events.recv()).await
    {
        let done = matches!(event, EnvEvent::Finished(_));
        events.push(event);
        if done { break }
    }
    events
}

#[tokio::test]
async fn check_missing_interpreter_reports_missing() {
    let dir = venv(); // never gets a bin/python
    let (spawn, _) = scripted_spawn(&dir, "exit 0\n");
    let events = drain(env::check(&dir, &spawn)).await;
    // MAJOR 1 (shape (b)): the typed Missing terminal — what T8's reducer
    // maps to EnvState::Missing. No "not verified" string anywhere.
    assert!(matches!(events.last(),
        Some(EnvEvent::Finished(Err(EnvFailure::Missing)))),
        "expected Missing terminal, got {events:?}");
}

#[tokio::test]
async fn check_success_sets_shared_versions() {
    let _cell = CELL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = venv();
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let bin = dir.join("bin/python");
    std::fs::write(&bin, "#!/bin/sh\nprintf '0.8.1\\nPython 3.12.4, build123d 0.8.1\\n'\n").unwrap();
    make_executable(&bin);
    // check must run the interpreter itself: argv-pass-through Spawn
    let (spawn, _seen) = argv_aware_spawn(&dir, &bin, "exit 0\n");
    set_shared_versions(None);
    let events = drain(env::check(&dir, &spawn)).await;
    assert!(matches!(events.last(), Some(EnvEvent::Finished(Ok(_)))));
    assert_eq!(versions_display(), "Python 3.12.4, build123d 0.8.1");
    set_shared_versions(None);
}

#[tokio::test]
async fn install_uv_path_reaches_verified_ready() {
    let _cell = CELL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = venv();
    let bin = dir.join("bin/python");
    let (spawn, seen) = argv_aware_spawn(&dir, &bin, &minting_script(&bin));
    let events = drain(env::install(&dir, &spawn, &uv_probe())).await;
    assert!(matches!(events.last(), Some(EnvEvent::Finished(Ok(_)))));
    let milestones: Vec<(f64, String)> = events.iter().filter_map(|e| match e {
        EnvEvent::Progress { value, step } => Some((*value, step.clone())),
        _ => None }).collect();
    assert_eq!(milestones.first().unwrap(), &(0.1, "Creating the virtual environment with uv…".into()));
    assert!(milestones.contains(&(0.95, "Verifying the installation…".into())));
    assert_eq!(milestones.last().unwrap(), &(1.0, "Components installed".into()));
    // MINOR 5: the milestone text itself lands in the log stream
    assert!(events.iter().any(|e| matches!(e,
        EnvEvent::Log(l) if l == "Creating the virtual environment with uv…")));
    assert!(seen.lock().unwrap().iter().any(|argv| argv.windows(3)
        .any(|w| w == ["venv", "--python", "3.12"])));
    set_shared_versions(None);
}

#[tokio::test]
async fn install_failing_step_reports_command_name() {
    let dir = venv();
    let (spawn, _) = scripted_spawn(&dir, "echo oops >&2\nexit 1\n");
    let events = drain(env::install(&dir, &spawn, &uv_probe())).await;
    match events.last() {
        Some(EnvEvent::Finished(Err(EnvFailure::InstallFailed(message)))) =>
            assert_eq!(message, "The command “uv” failed. Check the log for details."),
        other => panic!("expected command failure, got {other:?}"),
    }
    // the failure milestone fires before the terminal (fix-round finding 3)
    let steps: Vec<&String> = events.iter().filter_map(|e| match e {
        EnvEvent::Progress { step, .. } => Some(step),
        _ => None }).collect();
    assert!(steps.contains(&&"Installation failed".to_string()));
    assert!(events.iter().any(|e| matches!(e, EnvEvent::Log(l) if l.contains("oops"))));
}

#[tokio::test]
async fn install_without_uv_or_python_reports_python_not_found() {
    let dir = venv();
    let (spawn, _) = scripted_spawn(&dir, "exit 0\n");
    let probe: env::Probe = Arc::new(|_, _| None); // rejects EVERY candidate
    let events = drain(env::install(&dir, &spawn, &probe)).await;
    match events.last() {
        Some(EnvEvent::Finished(Err(EnvFailure::InstallFailed(m)))) =>
            assert!(m.starts_with("Neither uv nor Python 3.10–3.13")),
        other => panic!("expected pythonNotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn python_scan_requires_exit_zero() {
    // MINOR 9: a candidate whose --version exits nonzero is skipped even
    // when its output parses (PythonEnvironment.swift:103-108). Candidate
    // ENUMERATION is host-coupled (find_executable); ACCEPTANCE is the
    // probe's, so the fake decides: python3.13 always rejected on exit
    // code, the next enumerated candidate accepted with a fake version.
    let _cell = CELL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = venv();
    let bin = dir.join("bin/python");
    let (spawn, _seen) = argv_aware_spawn(&dir, &bin, &minting_script(&bin));
    let probe: env::Probe = Arc::new(|path: &Path, _| match path.file_name().unwrap().to_str() {
        Some("uv") => None,                                        // no uv → scan runs
        Some("python3.13") => Some((1, "Python 3.13.0\n".into())), // exit ≠ 0 → skipped
        _ => Some((0, "Python 3.12.4\n".into())),                  // accepted
    });
    let events = drain(env::install(&dir, &spawn, &probe)).await;
    assert!(matches!(events.last(), Some(EnvEvent::Finished(Ok(_)))));
    assert!(events.iter().any(|e| matches!(e,
        EnvEvent::Log(l) if l.starts_with("Found Python 3.12.4 at"))));
    set_shared_versions(None);
}
```

  (`make_executable` = `std::os::unix::fs::PermissionsExt` setting `0o755`, same as processes tests.)

- [ ] **Step 2: Run to verify RED** — `cargo test -p engine --test python_env` → compile error: no `python` module.
- [ ] **Step 3: Implement** `engine/src/python/{mod.rs, env.rs}`, `locator::{find_executable, executable_dirs}` (extraction only — reuse `is_executable_file`; no behavior change to existing locator fns), `lib.rs` registration. Keep the pure functions free of process spawning; only `streaming_spawn`/`command_probe`/drivers touch processes or the host. The module doc records NOTE 2's choice (a failed `check` clears the shared versions cell; upstream leaves the stale string — equivalent in every reachable state).
- [ ] **Step 4: Run to verify GREEN** — `cargo test -p engine --test python_env` PASS; `cargo test -p engine` (existing suites unaffected).
- [ ] **Step 5: Commit** — `git add engine/src/python engine/src/lib.rs engine/src/agent/locator.rs engine/tests/python_env.rs && git commit -m "feat(engine): python env — venv install planner, verification, shared versions cell"`

### Task 1: Engine `project::parameters` — ordered extraction and regex rewrite

**Files:**
- Create: `engine/src/project/parameters.rs`
- Modify: `engine/src/project/mod.rs` (register), `Cargo.toml` (workspace: `regex = "1"`), `engine/Cargo.toml` (`regex.workspace = true`), `engine/src/project/store.rs` (make `atomic_write` `pub(crate)` — it is currently private; parameters reuses it for model.py)
- Test: `engine/tests/parameters.rs`

**Interfaces:**
- Consumes: `atomic_write` from `store.rs`; `Manifest::parameters: BTreeMap<String, f64>`.
- Produces:
  - `#[derive(Debug, Clone, PartialEq)] pub struct ModelParameter { pub name: String, pub value: f64 }`
  - `pub fn ordered_parameters(manifest: &std::collections::BTreeMap<String, f64>, script: &str) -> Vec<ModelParameter>` — sorted by first index of the name as a plain substring in `script` (missing → `script.len()`, i.e. last), ties alphabetical (ProjectOutput.swift:110-120).
  - `#[derive(Debug, thiserror::Error)] pub enum ParameterError { #[error("parameter {name:?} not found in model.py")] NotFound { name: String }, #[error(transparent)] Io(#[from] std::io::Error) }`
  - `#[derive(Debug, Clone, PartialEq)] pub struct Rewrite { pub script: String, pub literal: String }` — `literal` is the exact text written in place of the captured number.
  - `pub fn format_number(value: f64, original: &str) -> String` — `value == value.trunc() && !original.contains('.')` → `i64` string (upstream `Int(value)` truncates toward zero); else `{}` shortest round-trip. Module doc records the one divergence: Swift's `String(Double)` switches to exponent notation beyond ~1e16; Rust's `{}` never does — a written-out literal is still valid Python and slider commits (3-decimal rounding) cannot reach that magnitude.
  - `pub fn rewrite_value(script: &str, name: &str, value: f64) -> Result<Rewrite, ParameterError>` — build `format!("(?m)^\\s*{}\\s*(?::[^=\\n]+)?=\\s*([-+]?(?:\\d+\\.?\\d*|\\.\\d+)(?:[eE][-+]?\\d+)?)", regex::escape(name))`, first match only, replace only capture group 1.
  - `pub fn set_parameter(project_root: &Path, name: &str, value: f64) -> Result<Rewrite, ParameterError>` — read `model.py`, `rewrite_value`, `atomic_write` back, return the rewrite (the caller builds the version prompt `"{name} = {literal}"`).

- [ ] **Step 1: Write the failing tests** — `engine/tests/parameters.rs`:

```rust
//! parameters.rs tests (M5 T1): ordering, the rewrite regex against a
//! corpus of adversarial lines, integer formatting, and the atomic
//! set_parameter round-trip.

use std::collections::BTreeMap;

use engine::project::parameters::{
    ModelParameter, format_number, ordered_parameters, rewrite_value, set_parameter,
};

#[test]
fn parameters_order_follows_first_occurrence_in_model_py() {
    let script = "z_unused = 1\na = 2\nmiddle()\nb = 3\n";
    let manifest: BTreeMap<String, f64> =
        [("b".to_string(), 3.0), ("a".to_string(), 2.0), ("zz".to_string(), 9.0)].into();
    assert_eq!(ordered_parameters(&manifest, script),
        [ModelParameter { name: "a".into(), value: 2.0 },
         ModelParameter { name: "b".into(), value: 3.0 },
         ModelParameter { name: "zz".into(), value: 9.0 }]);
    // both absent: alphabetical
    let manifest2: BTreeMap<String, f64> =
        [("q".to_string(), 1.0), ("c".to_string(), 2.0)].into();
    assert_eq!(ordered_parameters(&manifest2, "nothing here").iter()
        .map(|p| p.name.as_str()).collect::<Vec<_>>(), ["c", "q"]);
    // empty script edge: same as absent
    assert_eq!(ordered_parameters(&manifest2, "").len(), 2);
}

#[test]
fn rewrite_hits_typed_indented_and_spaced_assignments() {
    let cases = [
        ("wall = 2.5", "wall", 3.0, "wall = 3"),
        ("height: float = 10", "height", 12.5, "height: float = 12.5"),
        ("depth : float=4.0", "depth", 5.0, "depth : float=5"),
        ("count=7", "count", 2.0, "count=2"),
        ("  inner = 4", "inner", 6.0, "  inner = 6"),
        ("gap = 1e-3", "gap", 0.5, "gap = 0.5"),
        ("span = 1.2E+2", "span", 130.0, "span = 130"),
        ("off = -2.5", "off", 4.0, "off = 4"),
        ("tiny = .3", "tiny", 2.0, "tiny = 2"),
        ("r = 2.", "r", 3.0, "r = 3"),
    ];
    for (line, name, value, expected) in cases {
        let rewrite = rewrite_value(line, name, value).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert_eq!(rewrite.script, expected, "line: {line}");
    }
}

#[test]
fn adversarial_lines_never_match_or_mangle() {
    let script = "total == 3\n# width = 5\nwidth_mm = 3\nx = y = 3\nwall: str = \"5\"\n";
    // comparison operator: the name has a plain assignment nowhere
    assert!(matches!(rewrite_value(script, "total", 1.0),
        Err(engine::project::parameters::ParameterError::NotFound { .. })));
    // commented assignment
    assert!(matches!(rewrite_value(script, "width", 1.0),
        Err(engine::project::parameters::ParameterError::NotFound { .. })));
    // prefix collision: `width` must not match `width_mm`
    assert!(matches!(rewrite_value(script, "width_mm", 1.0).unwrap().script.as_str(),
        "width_mm = 1"));
    // non-numeric rhs
    assert!(matches!(rewrite_value(script, "x", 1.0),
        Err(engine::project::parameters::ParameterError::NotFound { .. })));
    // string literal rhs
    assert!(matches!(rewrite_value(script, "wall", 1.0),
        Err(engine::project::parameters::ParameterError::NotFound { .. })));
    // name with regex metacharacters must be escaped, still matchable
    let dotted = "part.width = 2\n";
    assert_eq!(rewrite_value(dotted, "part.width", 4.0).unwrap().script, "part.width = 4");
    // a blank line before the target (multiline \s spans it) still rewrites surgically
    assert_eq!(rewrite_value("\n\nsize = 3", "size", 4.0).unwrap().script, "\n\nsize = 4");
    // first match wins when several assignments share a name
    assert_eq!(rewrite_value("k = 1\nk = 2\n", "k", 9.0).unwrap().script, "k = 9\nk = 2\n");
    // untouched trailing content is preserved byte-for-byte
    let with_tail = "wall = 2.5  # comment\nnext = 1\n";
    assert_eq!(rewrite_value(with_tail, "wall", 3.0).unwrap().script,
        "wall = 3  # comment\nnext = 1\n");
}

#[test]
fn integer_formatting_follows_the_original_literal() {
    assert_eq!(format_number(30.0, "20"), "30");
    assert_eq!(format_number(30.5, "20"), "30.5");   // not rounded
    assert_eq!(format_number(30.0, "20.0"), "30");   // original had a dot: shortest form
    assert_eq!(format_number(-4.0, "-3"), "-4");
    assert_eq!(format_number(0.001234, "9"), "0.001234");
}

#[test]
fn set_parameter_round_trips_atomically() {
    let root = std::env::temp_dir().join(format!("cad-param-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("model.py"), "wall = 2.5\n").unwrap();
    let rewrite = set_parameter(&root, "wall", 4.0).unwrap();
    assert_eq!(rewrite.literal, "4");
    assert_eq!(std::fs::read_to_string(root.join("model.py")).unwrap(), "wall = 4\n");
    assert!(matches!(set_parameter(&root, "nope", 1.0).unwrap_err(),
        engine::project::parameters::ParameterError::NotFound { .. }));
    assert!(!root.join("model.py.tmp").exists()); // temp file renamed away
}
```

- [ ] **Step 2: RED** — `cargo test -p engine --test parameters` → module missing.
- [ ] **Step 3: Implement** — the regex is compiled per call (`Regex::new`); corpus inputs are tiny. `regex::escape` replaces `NSRegularExpression.escapedPattern`. Use `find_iter`/`captures` and rebuild the string by byte range so ONLY the capture changes.
- [ ] **Step 4: GREEN** — focused suite + `cargo test -p engine`.
- [ ] **Step 5: Commit** — `git add engine/src/project/parameters.rs engine/src/project/mod.rs engine/src/project/store.rs engine/tests/parameters.rs Cargo.toml engine/Cargo.toml && git add Cargo.lock && git commit -m "feat(engine): parameter extraction and model.py regex rewrite"` (`regex` is new to the workspace: the regenerated root `Cargo.lock` MUST be staged — house landmine, cf. 561f1ec; stage it only when changed)

### Task 2: Engine `python::model` — run model.py, classify the outcome

**Files:**
- Create: fills `engine/src/python/model.rs`, new fixture `fixtures/fake-agent/fake_python.py`
- Modify: nothing else (module registered in T0)
- Test: `engine/tests/python_model.rs`

**Interfaces:**
- Consumes: `crate::processes::spawn_streaming`, `crate::agent::locator::shell_env`.
- Produces:
  - `pub fn tool_environment() -> Vec<(String, String)>` — `shell_env()` + `MPLBACKEND=Agg` + `PYTHONUNBUFFERED=1` (ModelBuilder.swift:13-19).
  - `#[derive(Debug, Clone, PartialEq)] pub enum BuildOutcome { Success, Failed { last_line: Option<String>, output: String }, Interrupted }` — `output` = `(stderr if non-empty else stdout)` trimmed; `last_line` = its last line (`None` when the text is empty; the app maps that to the "Script error" catalog string).
  - `pub fn failure_text(stderr: &str, stdout: &str) -> String` and `pub fn last_line(text: &str) -> Option<String>` as the pure halves (testable without processes).
  - `#[derive(Debug, Clone, PartialEq)] pub enum BuildEvent { Line(String), Finished(BuildOutcome) }` — `Line` is stdout-or-stderr, arrival order (upstream streams both; display only on error).
  - `pub struct BuildHandle { pub events: tokio::sync::mpsc::Receiver<BuildEvent>, cancel: tokio::sync::watch::Sender<bool>, pid: Arc<Mutex<Option<u32>>> }` with `pub fn cancel(&self)` and `pub fn pid(&self) -> Option<u32>` — same shape/semantics as `agent::session::TurnHandle` (SIGINT→3 s→SIGTERM through `RunningProcess::interrupt`; a cancel observed at exit forces `Interrupted` whatever the code).
  - `pub fn run_model(interpreter: &Path, project_root: &Path) -> std::io::Result<BuildHandle>` — argv `[interpreter, "model.py"]`, cwd `project_root`, env `tool_environment()`, must be called inside a tokio runtime.

**fixture `fixtures/fake-agent/fake_python.py`** (modeled on fake_claude.py; mode via env, argv dump via env):

```python
#!/usr/bin/env python3
"""Fake venv python for engine tests (M5 T2/T6).

Env: CAD_FAKE_PY_MODE   = copy | fail | fail_stdout | hang | verify (default copy)
     CAD_FAKE_PY_FIXTURES = dir whose files are copied into ./output/ (mode copy)
     CAD_FAKE_PY_ARGV_OUT = file to append one JSON line {"argv","cwd","env"} per run
"""
import json, os, shutil, sys, time
from pathlib import Path

argv_out = os.environ.get("CAD_FAKE_PY_ARGV_OUT")
if argv_out:
    env = {k: os.environ.get(k) for k in ("MPLBACKEND", "PYTHONUNBUFFERED")}
    with open(argv_out, "a") as f:
        f.write(json.dumps({"argv": sys.argv[1:], "cwd": os.getcwd(), "env": env}) + "\n")

mode = os.environ.get("CAD_FAKE_PY_MODE", "copy")
if mode == "fail":
    sys.stdout.write("generating model\n")
    sys.stderr.write("Traceback (most recent call last):\n  File \"model.py\", line 5\n"
                     "ValueError: bad shape\n")
    sys.exit(1)
if mode == "fail_stdout":
    sys.stdout.write("line one\nboom: no stderr at all\n")
    sys.exit(3)
if mode == "hang":
    while True:
        time.sleep(0.2)
if mode == "verify":
    print("0.8.1")
    print("Python 3.12.4, build123d 0.8.1, matplotlib 3.9.2, numpy 2.1.0")
    sys.exit(0)
# copy
src = Path(os.environ["CAD_FAKE_PY_FIXTURES"])
out = Path("output")
out.mkdir(exist_ok=True)
for name in ("model.3mf", "model.stl", "model.step", "schematic.svg", "schematic.png",
             "schematic.pdf", "manifest.json"):
    dst = out / name
    if not dst.exists():
        shutil.copy2(src / name, dst)
sys.exit(0)
```

  (Source fixtures live in `fixtures/golden/` — the M0 build123d-generated outputs; all 7 expected files verified present (`model.3mf/stl/step`, `schematic.svg/png/pdf`, `manifest.json`). Copy from there in tests via `CARGO_MANIFEST_DIR/../fixtures/golden`. `fixtures/upstream/` holds only `CADSystemPrompt.md` + `Localizable.xcstrings` — BLOCKER 1.)

- [ ] **Step 1: Failing tests** — `engine/tests/python_model.rs`:

```rust
//! python::model tests (M5 T2): run_model against the fake interpreter —
//! argv/cwd/env contract, success, stderr/stdout failure text, interruption.

use std::path::{Path, PathBuf};
use std::time::Duration;

use engine::python::model::{self, BuildEvent, BuildOutcome, failure_text, last_line};

fn fake_python() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake-agent/fake_python.py")
}

fn tmp_project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("cad-build-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    std::fs::write(root.join("model.py"), "print('hi')\n").unwrap();
    root
}

/// The env the fake reads must be set before spawn and is process-global:
/// serialize with ENV_LOCK like session_lifecycle.rs.
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

async fn drive(root: &Path, cancel_after: Option<Duration>) -> (Vec<BuildEvent>, Vec<String>) {
    let dump = root.join("argv-dump.jsonl");
    let _ = std::fs::remove_file(&dump); // per-run dump: read only this invocation
    // SAFETY: setters hold ENV_LOCK; every drive call is inside the lock.
    unsafe {
        std::env::set_var("CAD_FAKE_PY_ARGV_OUT", &dump);
        std::env::remove_var("CAD_FAKE_PY_FIXTURES");
    }
    let mut handle = model::run_model(&fake_python(), root).unwrap();
    if let Some(delay) = cancel_after {
        let h = &handle;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            h.cancel();
        });
    }
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while let Ok(Some(event)) =
        tokio::time::timeout_at(deadline, handle.events.recv()).await
    {
        let done = matches!(event, BuildEvent::Finished(_));
        events.push(event);
        if done { break }
    }
    let argv = std::fs::read_to_string(&dump).unwrap_or_default();
    let recorded: Vec<String> = argv.lines().filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["argv"]
            .as_str().unwrap().to_string()).collect();
    (events, recorded)
}

#[tokio::test]
async fn success_runs_model_py_in_project_root_with_tool_env() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmp_project();
    // SAFETY: holds ENV_LOCK.
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "verify") };
    let (events, recorded) = drive(&root, None).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE") };
    assert_eq!(recorded, vec!["model.py".to_string()]);
    let dump = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(root.join("argv-dump.jsonl")).unwrap()).unwrap();
    assert_eq!(dump["cwd"].as_str().unwrap(), root.to_str().unwrap());
    assert_eq!(dump["env"]["MPLBACKEND"].as_str(), Some("Agg"));
    assert_eq!(dump["env"]["PYTHONUNBUFFERED"].as_str(), Some("1"));
    assert!(matches!(events.last(), Some(BuildEvent::Finished(BuildOutcome::Success))));
}

#[tokio::test]
async fn stderr_failure_carries_last_line_and_full_text() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmp_project();
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "fail") };
    let (events, _) = drive(&root, None).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE") };
    match events.last() {
        Some(BuildEvent::Finished(BuildOutcome::Failed { last_line, output })) => {
            assert_eq!(last_line.as_deref(), Some("ValueError: bad shape"));
            assert!(output.starts_with("Traceback"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn stdout_only_failure_uses_stdout_text() {
    assert_eq!(failure_text("", "line one\nboom: no stderr at all\n"),
        "line one\nboom: no stderr at all");
    assert_eq!(failure_text("real stderr\n", "noise\n"), "real stderr");
    assert_eq!(last_line("a\nb\n"), Some("b".to_string()));
    assert_eq!(last_line(""), None);
    let _guard = ENV_LOCK.lock().await;
    let root = tmp_project();
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "fail_stdout") };
    let (events, _) = drive(&root, None).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE") };
    match events.last() {
        Some(BuildEvent::Finished(BuildOutcome::Failed { last_line, .. })) =>
            assert_eq!(last_line.as_deref(), Some("boom: no stderr at all")),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_reports_interrupted() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmp_project();
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "hang") };
    let (events, _) = drive(&root, Some(Duration::from_millis(300))).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE") };
    assert!(matches!(events.last(), Some(BuildEvent::Finished(BuildOutcome::Interrupted))));
}
```

  (The argv dump is per-run: `drive` removes the dump file before spawning — `let _ = std::fs::remove_file(&dump);` ahead of `run_model` — so each test reads only its own invocation. No wrapper helper: ENV_LOCK serializes `CAD_FAKE_PY_MODE`.)

- [ ] **Step 2: RED** — `cargo test -p engine --test python_model` → `model` module items missing.
- [ ] **Step 3: Implement** — mirror `session.rs`'s launch/select loop at small scale: one `RunningProcess`, select over `proc.lines.recv()` and `cancel.changed()`, accumulate `stdout_text`/`stderr_text` (both newline-terminated like `drain` does), terminal classification `interrupted ? Interrupted : status == 0 ? Success : Failed`.
- [ ] **Step 4: GREEN** — focused + full engine suite.
- [ ] **Step 5: Commit** — `git add engine/src/python/model.rs engine/tests/python_model.rs fixtures/fake-agent/fake_python.py && git commit -m "feat(engine): model.py runner — streamed build with last-line error classification"`

### Task 3: Engine `project::versions` — snapshot, restore, compare parameters

**Files:**
- Create: `engine/src/project/versions.rs`
- Modify: `engine/src/project/mod.rs`, `engine/src/project/outputs.rs` (add `missing_files`), `engine/src/project/store.rs` (`pub(crate)` atomic_write if T1 has not landed first — reuse whatever visibility T1 left)
- Test: `engine/tests/versions.rs`

**Interfaces:**
- Consumes: `crate::project::store::{iso8601_utc, atomic_write}`; `crate::project::manifest::Manifest`; `crate::project::outputs::expected_outputs`.
- Produces:
  - `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] pub struct ProjectVersion { pub number: i64, #[serde(with = "crate::project::store::iso8601_utc")] pub created_at: time::OffsetDateTime, pub prompt: String, pub session_id: Option<String> }` — snake-case keys match upstream CodingKeys.
  - `#[derive(Debug, thiserror::Error)] pub enum VersionError { #[error(transparent)] Io(#[from] std::io::Error), #[error(transparent)] Json(#[from] serde_json::Error) }`
  - `pub fn versions_root(project_root: &Path) -> PathBuf` — `<root>/.cadstudio/versions`
  - `pub fn load_versions(project_root: &Path) -> Vec<ProjectVersion>` — every `versions/<n>/version.json` that parses, sorted ascending by number (upstream `compactMap` + sort).
  - `pub fn next_number(versions: &[ProjectVersion]) -> i64` — `last.number + 1`, `1` when empty.
  - `pub fn has_changes(project_root: &Path, versions: &[ProjectVersion], current: Option<i64>) -> bool` — `true` when `current` names no loaded version; else byte-compare current `model.py` against the version's copy (VersionHistory.swift:37-42).
  - `pub fn record(project_root: &Path, versions: &mut Vec<ProjectVersion>, prompt: &str, session_id: Option<&str>, now: time::OffsetDateTime) -> Result<ProjectVersion, VersionError>` — create `versions/<n>/`, copy-if-present `model.py`, `output/` (recursive), `.cadstudio/chat.json`, write `version.json`, append + return.
  - `pub fn restore(project_root: &Path, version: &ProjectVersion) -> Result<(), VersionError>` — replace `model.py` and `.cadstudio/chat.json` (remove-then-copy-if-present), remove every entry in `output/`, copy the version's `output/` contents in (VersionHistory.swift:56-67). Does NOT touch metadata (caller owns `session_id`/`current_version`).
  - `pub fn comparison_parameters(project_root: &Path, number: Option<i64>) -> std::collections::BTreeMap<String, f64>` — `number`'s `output/manifest.json` parameters; empty map when `None`, unreadable, or unparseable.
  - `pub fn missing_files(output_dir: &Path) -> Vec<String>` (in outputs.rs) — `expected_outputs()` filtered on existence, mapped to names, upstream report order (ProjectOutput.swift:32-36). This is the RECORD gate, deliberately narrower than `missing_requirements`.

- [ ] **Step 1: Failing tests** — `engine/tests/versions.rs`:

```rust
//! versions.rs tests (M5 T3): version.json on-disk shape, record's copy
//! set, restore's swap semantics, change detection, comparison parameters,
//! and the missing-files record gate.

use std::collections::BTreeMap;

use engine::project::outputs::missing_files;
use engine::project::versions::{
    ProjectVersion, comparison_parameters, has_changes, load_versions, next_number,
    record, restore, versions_root,
};

fn project() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("cad-vers-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    std::fs::create_dir_all(root.join(".cadstudio")).unwrap();
    std::fs::write(root.join("model.py"), "wall = 2\n").unwrap();
    std::fs::write(root.join("output/manifest.json"),
        r#"{"parameters": {"wall": 2.0}, "warnings": []}"#).unwrap();
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
    let v = record(&root, &mut versions, "prompt", Some("sid-1"),
        time::OffsetDateTime::from_unix_timestamp(1_763_000_000).unwrap()).unwrap();
    assert_eq!(v.number, 1);
    let json = std::fs::read_to_string(
        versions_root(&root).join("1/version.json")).unwrap();
    for key in ["\"number\"", "\"created_at\"", "\"prompt\"", "\"session_id\""] {
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
    record(&root, &mut versions, "p", None,
        time::OffsetDateTime::now_utc()).unwrap();
    let dir = versions_root(&root).join("1");
    assert_eq!(std::fs::read_to_string(dir.join("model.py")).unwrap(), "wall = 2\n");
    assert_eq!(std::fs::read_to_string(dir.join("output/model.3mf")).unwrap(), "3mf");
    assert!(dir.join("output/nested/extra.txt").exists());
    assert_eq!(std::fs::read_to_string(dir.join("chat.json")).unwrap(), "[]");
}

#[test]
fn record_without_model_or_chat_skips_those_files() {
    let root = project();
    std::fs::remove_file(root.join("model.py")).unwrap();
    std::fs::remove_file(root.join(".cadstudio/chat.json")).unwrap();
    let mut versions = vec![];
    record(&root, &mut versions, "p", None,
        time::OffsetDateTime::now_utc()).unwrap();
    assert!(!versions_root(&root).join("1/model.py").exists());
    assert!(!versions_root(&root).join("1/chat.json").exists());
    assert!(versions_root(&root).join("1/version.json").exists());
}

#[test]
fn restore_swaps_everything_and_clears_output() {
    let root = project();
    let mut versions = vec![];
    let v1 = record(&root, &mut versions, "v1", Some("sid-1"),
        time::OffsetDateTime::now_utc()).unwrap();
    std::fs::write(root.join("model.py"), "wall = 9\n").unwrap();
    std::fs::write(root.join("output/stale.txt"), b"stale").unwrap();
    std::fs::remove_file(root.join("output/manifest.json")).unwrap();
    let _v2 = record(&root, &mut versions, "v2", Some("sid-2"),
        time::OffsetDateTime::now_utc()).unwrap();
    restore(&root, &v1).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("model.py")).unwrap(), "wall = 2\n");
    assert!(root.join("output/manifest.json").exists());
    assert!(!root.join("output/stale.txt").exists());   // v2's file gone with v2's output
    // current chat.json is the version's copy (byte-identical here; the
    // caller reloads state from disk)
    assert!(root.join(".cadstudio/chat.json").exists());
    // missing version output dir: tolerated, output/ simply emptied
    let orphan = at(9);
    std::fs::create_dir_all(versions_root(&root).join("9")).unwrap();
    std::fs::write(versions_root(&root).join("9/version.json"),
        serde_json::to_string(&orphan).unwrap()).unwrap();
    restore(&root, &orphan).unwrap();
    assert!(root.join("output").read_dir().unwrap().count() == 0);
}

#[test]
fn has_changes_follows_model_py_bytes() {
    let root = project();
    let mut versions = vec![];
    record(&root, &mut versions, "p", None, time::OffsetDateTime::now_utc()).unwrap();
    assert!(!has_changes(&root, &versions, Some(1)));
    std::fs::write(root.join("model.py"), "wall = 3\n").unwrap();
    assert!(has_changes(&root, &versions, Some(1)));
    assert!(has_changes(&root, &versions, None));       // no current version
    assert!(has_changes(&root, &versions, Some(42)));   // unknown current
}

#[test]
fn next_number_is_last_plus_one() {
    assert_eq!(next_number(&[]), 1);
    let vs = vec![at(1), at(3)];
    assert_eq!(next_number(&vs), 4);
}

#[test]
fn comparison_parameters_read_the_versions_manifest() {
    let root = project();
    let mut versions = vec![];
    record(&root, &mut versions, "p", None, time::OffsetDateTime::now_utc()).unwrap();
    assert_eq!(comparison_parameters(&root, Some(1)),
        BTreeMap::from([("wall".to_string(), 2.0)]));
    assert!(comparison_parameters(&root, None).is_empty());
    assert!(comparison_parameters(&root, Some(77)).is_empty());
}

#[test]
fn missing_files_lists_expected_outputs_in_order() {
    let root = project();
    assert_eq!(missing_files(&root.join("output")),
        ["model.3mf", "model.stl", "model.step", "schematic.svg", "schematic.png",
         "schematic.pdf"]); // manifest.json exists
    std::fs::remove_file(root.join("output/manifest.json")).unwrap();
    let all = missing_files(&root.join("output"));
    assert_eq!(all.last().map(String::as_str), Some("manifest.json"));
}
```

- [ ] **Step 2: RED** — module missing.
- [ ] **Step 3: Implement** — recursive copy for `output/` (`fs_extra`-free: a small walk with `read_dir` + `create_dir_all`); `record`'s copy-if-present uses `fs::copy`.
- [ ] **Step 4: GREEN** — focused + full engine suite.
- [ ] **Step 5: Commit** — `git add engine/src/project/versions.rs engine/src/project/outputs.rs engine/src/project/mod.rs engine/tests/versions.rs && git commit -m "feat(engine): version snapshot/restore with comparison parameters and record gate"`

### Task 4: Engine carry-forwards — prompt versions line, reap fix, classifier tests

**Files:**
- Modify: `engine/src/agent/session.rs` (drop `PYTHON_UNVERIFIED`, read the versions cell), `engine/src/processes.rs` (`reap_orphans_at` keeps live children), `engine/tests/tool_summary.rs` (empty-root divergence test), `engine/tests/claude_runner.rs` (its `environment_bullets` literal at :235-238 changes to the fallback text), `engine/tests/processes.rs` (reap test)
- Test: additions named below

**Interfaces:**
- Consumes: T0's `python::env::{versions_display, set_shared_versions}`.
- Produces: unchanged public shapes — `prompt_ctx` now sets `python_display`/`environment` from `versions_display()`; `reap_orphans_at(path) -> Vec<u32>` still returns killed pids but now persists the still-live un-killed entries instead of emptying the registry (carry-forward: a child THIS process cancelled but that outlives the app must stay known to a later launch's reap).

**session.rs change (exact):** delete `const PYTHON_UNVERIFIED`; in `prompt_ctx` replace both uses with `crate::python::env::versions_display()` — one `let python_display = ...` binding used for `python_display` and `environment_bullets(root, &python_display)`. `app_venv_python()` (session.rs:569-571, hardcoded `$HOME/.local/share/cad-studio/venv/bin/python`) is rerouted through `crate::python::env::interpreter_path(&crate::python::env::venv_dir())` — ONE canonical data-dir helper (MINOR 10; on Linux byte-identical to today's path, so no prompt or allowlist behavior changes; macOS note in T0). The module docs' "not verified until M5" line updates to state the fallback rule.

**processes.rs change (exact):** in `reap_orphans_at`, collect `remaining: BTreeSet<u32>` = registry pids whose `ps(pid).is_some()` and that were not signalled; `persist(path, &remaining)` replaces `persist(path, &BTreeSet::new())`. Update the module doc bullet that says "After a reap the file is emptied".

- [ ] **Step 1: Failing tests** — add to `engine/tests/claude_runner.rs`:

```rust
#[test]
fn python_display_fallback_and_verified_line() {
    // environment_bullets takes the display string; the session sources it
    // from the shared cell. Pin BOTH halves of the rule here.
    engine::python::env::set_shared_versions(None);
    assert_eq!(engine::python::env::versions_display(), "build123d, matplotlib, numpy");
    let root = bullets_root(); // existing helper or inline temp root with model.py
    let bullets = engine::agent::claude::environment_bullets(&root,
        &engine::python::env::versions_display());
    assert_eq!(bullets[0], "- Python packages: build123d, matplotlib, numpy");
    engine::python::env::set_shared_versions(
        Some("Python 3.12.4, build123d 0.8.1".into()));
    let bullets = engine::agent::claude::environment_bullets(&root,
        &engine::python::env::versions_display());
    assert_eq!(bullets[0], "- Python packages: Python 3.12.4, build123d 0.8.1");
    engine::python::env::set_shared_versions(None);
}
```

  plus a session-level assertion appended to an existing prompt-context test (or a new `rendered_prompt_carries_versions_line` using `render_system_prompt` on a template containing `{{ENVIRONMENT}}` with the cell set) asserting the bullet lands in the rendered prompt. Add to `engine/tests/processes.rs`:

```rust
#[tokio::test]
async fn reap_keeps_live_children_listed() {
    let dir = tmp_dir();
    let registry = dir.join("registry");
    // SAFETY: registry env holds this test's ENV_LOCK (file shows the pattern).
    unsafe { std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", &registry) };
    let proc = spawn_script(&dir, "sleep 5\n").await;   // our OWN live child
    // (spawn_script's spawn already registered the pid — no register_at needed)
    let killed = reap_orphans_at(&registry);
    assert!(killed.is_empty());                          // ppid != 1: not touched
    let text = std::fs::read_to_string(&registry).unwrap();
    assert!(text.contains(&proc.pid.to_string()),        // …but still listed
        "live child erased from registry: {text}");
    let _ = proc.interrupt();                            // clean up…
    // …and AWAIT the exit before reaping: SIGINT is asynchronous, and a
    // still-live child would legitimately stay listed (MINOR 4).
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5), proc.exit_status).await;
    let _ = reap_orphans_at(&registry);                  // gone → dropped by persist
    assert_eq!(std::fs::read_to_string(&registry).unwrap(), "");
}
```

  (Adapt to the file's real helpers — `spawn_script` already points the registry at a per-test path via env; the point is the two assertions: killed empty + still listed, then empty after death. The existing `reap` test that asserts the file is emptied after reaping a DEAD pid must keep passing — `persist` drops unresolvable pids.) Add to `engine/tests/tool_summary.rs`:

```rust
#[test]
fn short_command_empty_root_strips_any_cd() {
    // Carry-forward divergence: upstream requires the cd target to contain
    // the project root; the port's root-free classification (summarize's
    // Bash path) strips ANY leading cd, so a foreign-cd command classifies
    // one class up from upstream. Preserved deliberately.
    assert_eq!(short_command("cd /elsewhere && python x.py", "", ""),
        "python x.py");
    assert_eq!(summarize("Bash", &json!({"command": "cd /elsewhere && python model.py"})),
        "Building the model"); // upstream: "Working on the project"
    // The other two preserved divergences, restated where they live:
    assert_eq!(live_preview(r#"{"new_string": "a\nb"}"#).as_deref(), Some("a\nb"));
    assert_eq!(summarize("Read", &json!({"file_path": "/w/references/model.py"})),
        "Studying the reference"); // segment match, not just prefix
}
```

- [ ] **Step 2: RED** — `versions_display` missing until T0 lands (gate on T0); the reap test fails against today's unconditional empty; the tool_summary test fails? (it should PASS today — that is fine and intended as a pin, note it in the commit).
- [ ] **Step 3: Implement** the two source changes; fix `claude_runner.rs:235-238` to the fallback string.
- [ ] **Step 4: GREEN** — `cargo test -p engine` whole crate (session suites re-run: no prompt text they assert changes except the bullets literal).
- [ ] **Step 5: Commit** — `git add engine/src/agent/session.rs engine/src/processes.rs engine/tests/tool_summary.rs engine/tests/claude_runner.rs engine/tests/processes.rs && git commit -m "feat(engine): M5 carry-forwards — prompt versions fallback, reap keeps live children"`

### Task 5: App parameter model + ParametersOverlay

**Files:**
- Create: `app/src/parameters.rs` (pure), `app/src/parameters_view.rs` (widgets)
- Modify: `app/src/lib.rs` (register), `app/src/viewer_pane.rs` (mount the overlay), `app/src/app.rs` (⌥⌘I command + `are_parameters_visible` state), `app/Cargo.toml` (nothing new)
- Test: `app/tests/parameters_model.rs`

**Interfaces:**
- Consumes: M4's `Bridge` (declared M4 T3), `engine::project::parameters::ModelParameter`, `engine::Settings.parameters_collapsed` (persisted collapse state — ONE writer rule: save through engine `settings_path()`), M4's `activity` busy flag, `engine::project::versions::comparison_parameters`.
- Produces:
  - `#[derive(Debug, Clone, PartialEq)] pub struct SliderState { pub value: f64, pub min: f64, pub max: f64, pub editing: bool }`
  - `impl SliderState { pub fn new(value: f64) -> Self; pub fn range_around(value: f64) -> (f64, f64); pub fn is_integer_value(committed: f64) -> bool; pub fn commit_value(&self, current: f64) -> f64; pub fn commit(&self, current: f64) -> Option<f64>; pub fn sync_from(&mut self, value: f64) }`
    - `range_around`: `v > 0 → (0.0, max(2v, 1.0))`, `v < 0 → (2v, max(-2v, 1.0))`, else `(-10.0, 10.0)`.
    - `is_integer_value(committed)`: `committed == committed.round() && committed.abs() >= 1.0` — keys on the PARAMETER's COMMITTED value, not the slider's live value (upstream `isInteger` reads `parameter.value`, ParametersOverlay.swift:134-136; MAJOR 2).
    - `commit_value(current)`: `is_integer_value(current)` → `self.value.rounded()`; else `(self.value * 1000.0).round() / 1000.0` — the rule chosen by the committed value, applied to the LIVE value.
    - `commit(current)`: `let v = self.commit_value(current); (v != current).then_some(v)` (after range re-fit: if outside, recompute range around `v` — the state mutates only via `sync_from`/setters; the returned value is the commit).
    - `sync_from(value)`: when `!editing`, set `value` and re-fit range when it no longer contains the parameter's value.
  - `#[derive(Debug, Clone, PartialEq)] pub struct ParametersPanel { pub collapsed: bool, pub sliders: Vec<(String, SliderState)> }` with `pub fn from_parameters(params: &[ModelParameter], collapsed: bool) -> Self` and `pub fn sync(&mut self, params: &[ModelParameter])` (upstream `onChange(of: parameter.value)` — keeps user edits mid-drag).
  - `parameters_view::view_parameters(panel: &ParametersPanel, previous: &BTreeMap<String, f64>, busy: bool, on_commit: impl Fn(String, f64) + 'static, on_collapse: ..., on_close: ..., compare: ...) -> cosmic::Element<app::Message>` — collapsed → capsule button (`fl!("parametri")`, slider icon); expanded → the panel: per-parameter row (monospace name; strikethrough orange previous value when `previous.contains_key(name) && previous[name] != value`, formatted with fraction digits 0…2 — ParametersOverlay.swift:148, NOTE 1 — while the live field uses 0…3; numeric text field; slider), footer "Compare with" picker (`fl!("confronta-con")`, `fl!("nessuna")`, `Versione {n}` entries, disabled when no versions), caption when comparing (`fl!("in-arancione-la-forma-della-versione-scelta-per-vederne-i-colori-ripristinala-dal-menu-versioni")`). Disabled entirely while `busy`. Panel width 280, max height 340 for the scroll area.
- App command: ⌥⌘I toggles `are_parameters_visible` (upstream Commands.swift:69-73); the overlay's close button sets it false; default true.

- [ ] **Step 1: Failing tests** — `app/tests/parameters_model.rs`:

```rust
//! parameters model tests (M5 T5): the upstream range heuristic, commit
//! rounding, no-op commits, external sync, and panel state.

use app::parameters::{ParametersPanel, SliderState};
use engine::project::parameters::ModelParameter;

fn p(name: &str, value: f64) -> ModelParameter {
    ModelParameter { name: name.into(), value }
}

#[test]
fn range_heuristic_matches_upstream() {
    assert_eq!(SliderState::range_around(5.0), (0.0, 10.0));   // 2v
    assert_eq!(SliderState::range_around(0.3), (0.0, 1.0));    // max(2v, 1)
    assert_eq!(SliderState::range_around(-2.0), (-4.0, 4.0));
    assert_eq!(SliderState::range_around(-0.2), (-0.4, 1.0));  // max(-2v, 1)
    assert_eq!(SliderState::range_around(0.0), (-10.0, 10.0));
}

#[test]
fn commit_rounds_integers_and_three_decimals() {
    // The integer rule keys on the parameter's COMMITTED value (upstream
    // isInteger reads parameter.value, ParametersOverlay.swift:134-136) and
    // is applied to the slider's LIVE value.
    let s = SliderState::new(4.4);       // parameter at 3 → integer rule
    assert_eq!(s.commit_value(3.0), 4.0);
    assert_eq!(s.commit(3.0), Some(4.0));
    let s = SliderState::new(4.0004);    // committed 3.5 → 3-decimal rule
    assert_eq!(s.commit_value(3.5), 4.0);
    let s = SliderState::new(0.12345);   // committed 0.5 → 3-decimal rule
    assert_eq!(s.commit_value(0.5), 0.123);
    let s = SliderState::new(-1.6);      // committed -3 → integer rule
    assert_eq!(s.commit_value(-3.0), -2.0);
}

#[test]
fn commit_equal_to_current_is_noop() {
    let s = SliderState::new(4.0);
    assert_eq!(s.commit(4.0), None);
    let s = SliderState::new(6.0);       // drag to 6 with the parameter at 5
    assert_eq!(s.commit(5.0), Some(6.0));
}

#[test]
fn sync_follows_external_value_only_when_not_editing() {
    // outside the current range → re-fit (0.3's range is 0..=1; 9 escapes it)
    let mut s = SliderState::new(0.3);
    s.sync_from(9.0);
    assert_eq!(s.value, 9.0);
    assert_eq!((s.min, s.max), (0.0, 18.0));  // re-fit around 9
    // contained → range untouched (upstream refits only on escape)
    let mut c = SliderState::new(5.0);        // range 0..=10
    c.sync_from(9.0);
    assert_eq!((c.value, c.min, c.max), (9.0, 0.0, 10.0));
    c.editing = true;
    c.value = 1.0;                       // user dragging
    c.sync_from(9.0);
    assert_eq!(c.value, 1.0);            // untouched mid-edit
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
    assert_eq!(panel.sliders.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["height", "depth"]);
}
```

- [ ] **Step 2: RED** → **Step 3: Implement** pure model then the thin widget file (fl! keys: `parametri`, `comprimi-i-parametri`, `espandi-i-parametri`, `nascondi-parametri`, `nascondi-parametri-i`, `confronta-con`, `nessuna`, `versione-lld`, `valore-nella-versione-a-confronto`, `in-arancione-la-forma-…`). The commit path calls the injected `on_commit(name, value)` — wired in T6 to `Bridge::set_parameter`.
- [ ] **Step 4: GREEN** — `cargo test -p app --test parameters_model`; `cargo check -p app` for the widget wiring.
- [ ] **Step 5: Commit** — `git add app/src/parameters.rs app/src/parameters_view.rs app/src/lib.rs app/src/viewer_pane.rs app/src/app.rs app/tests/parameters_model.rs && git commit -m "feat(app): parameter sliders overlay with compare picker"`

### Task 6: App build pipeline — set_parameter, build, version recording, send gating

**Files:**
- Create: `app/tests/build_pipeline.rs`
- Modify: `app/src/bridge.rs` (M4's file), `app/src/chat.rs` (build-failure row helper if the reducer lacks the shape), `app/tests/bridge_e2e.rs` (M4's suite: `enqueue` gains the `Result`/guard signature — its callers are updated in the SAME commit so the landed suite never breaks mid-merge, MINOR 11), `app/Cargo.toml` (dev-deps only if needed)
- Test: `app/tests/build_pipeline.rs`

**Interfaces:**
- Consumes: T1 `set_parameter`, T2 `python::model::{run_model, BuildEvent, BuildOutcome}`, T3 `{load_versions, record, has_changes, next_number, missing_files}`, T0 `python::env` ready-state mirror, M4 Bridge/ChatState/activity, `engine::project::store::{save_chat, load_chat, save_metadata, load_metadata, ChatMessage, Role, Action}`, `engine::project::manifest::Manifest::load` for parameter refresh.
- Produces (Bridge additions; exact):
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum BuildRefusal { Busy, PythonNotReady, NoModelScript }`
  - `impl Bridge { pub fn set_parameter(&mut self, name: &str, value: f64) -> Result<(), BuildRefusal>; pub fn start_build(&mut self, version_prompt: Option<String>) -> Result<(), BuildRefusal>; pub fn python_ready(&self) -> bool; pub fn parameters(&self) -> &[ModelParameter]; pub fn build_error(&self) -> Option<&str>; pub fn missing_outputs(&self) -> &[String]; pub fn versions(&self) -> &[ProjectVersion]; }`
  - Bridge state fields added: `build: Option<python::model::BuildHandle>`, `versions: Vec<ProjectVersion>`, `parameters: Vec<ModelParameter>`, `build_error: Option<String>`, `missing_outputs: Vec<String>`, `python_state: EnvState` (set from T8's startup check / sheet completion), `interpreter: PathBuf` (the venv interpreter, overrideable for tests via `Bridge::with_interpreter`).
  - Message variants added (on M4's `app::bridge::Message` stream, forwarded into `app::Message` by app.rs; MINOR 12 — no placeholder shapes): `ParameterCommitted { name: String, value: f64 }`, `BuildEventBridge(engine::python::model::BuildEvent)` (the build's forward of each streamed line/finish, parallel to M4's `Event(EngineEvent)`), `BuildFinished { outcome: BuildOutcome, version_prompt: Option<String> }`, `TurnSuccessBookkeeping` (the agentDidFinish tail M4 left at save_chat; emitted only AFTER the tail's disk writes — refresh, `save_chat`, `save_metadata` — have landed, so a test waiting on it can assert disk state).
- Semantics (Project.swift:262-337, 343-416 transcribed):
  - `set_parameter` (MINOR 1 — upstream `setParameter` guards `isBusy` ONLY, Project.swift:343-362): refuse `Busy` when a turn or build is running; read-rewrite via `engine::project::parameters::set_parameter` — the file is rewritten FIRST, whatever python's state; on `NotFound` → activity `Failed(fl!("parametro-non-trovato-in-model-py", name))` and return `Ok(())` (upstream sets the failed activity, not an error propagation); on success → `start_build(Some(format!("{name} = {}", rewrite.literal)))`, whose own ready-guard refuses `PythonNotReady` and triggers the sheet with the file already rewritten (upstream parity: the rewrite survives a refused build).
  - `start_build`: guards Busy / PythonNotReady / `NoModelScript` (when `model.py` absent → activity `Failed(fl!("nessun-model-py-da-compilare"))`); activity `Working(fl!("compilazione-di-model-py"))`; clear `build_error`; spawn `run_model(&self.interpreter, root)` and drain `BuildEvent`s as Messages (same drain-stalls-cancel discipline as turns).
  - `BuildFinished`: `Interrupted` → activity `Failed(fl!("compilazione-interrotta"))`; `Success` → refresh outputs (re-parse manifest + parameters, idempotent), `missing_outputs = missing_files(output)`, activity `Succeeded(fl!("modello-generato"))`, `record_version(version_prompt.unwrap_or(fl!("compilazione-manuale-di-model-py")))`, `continue_queue(succeeded: true)`; `Failed { last_line, output }` → `build_error = last_line.unwrap_or(fl!("errore-nello-script"))`, activity `Failed(fl!("errore-nello-script"))`, `continue_queue(succeeded: false)`, append+persist a system row: `text = fl!("la-compilazione-di-model-py-non-riuscita")`, `detail = Some(output)`, `action = Some(Action::AskClaudeToFix)`.
  - `record_version(prompt)`: `if !has_changes(root, &versions, metadata.current_version) → return`; append the Summary `ChatMessage` — constructed EXPLICITLY, no `..` spread (`ChatMessage` derives no `Default`; id/created_at per store.rs's row constructor, NOTE 3) with `role: Summary, text: fl!("versione-lld", next_number), detail: Some(next_number.to_string())` — then `save_chat`; `versions::record(...)`; on success set `metadata.current_version = Some(v.number)` + `save_metadata`; on engine error → `build_error = err.to_string()` (upstream parity: the summary row is already in).
  - Turn-success tail (`TurnSuccessBookkeeping`, upstream agentDidFinish): refresh outputs, `missing_outputs = missing_files(...)`; when empty → `Succeeded(fl!("modello-generato"))` + `record_version(last user row text or "")`; else `Succeeded(fl!("risposta-completata"))`; then M4's queue continuation.
  - Send gating: M4's send/enqueue paths gain the `python_ready()` guard — a refused send keeps the draft text (upstream `chat.draft = text`) and signals the sheet (a `Message::PythonSheetRequested` the app maps to presenting T8's sheet; the Bridge itself never touches windows).
  - `ask_agent_to_fix(traceback)` (M4 declared the action; M5 delivers the handler): sends `fl!("l-esecuzione-di-model-py-non-riesce-con-questo-errore-correggi-lo-script", traceback)` as a normal turn.
  - `cancel()` extended: cancels the running build handle too (upstream `task?.cancel()` owns both).

- [ ] **Step 1: Failing tests** — `app/tests/build_pipeline.rs` (fake python from T2; output fixtures from `fixtures/golden`; the fake AGENT from M4's harness for the turn-path test; house bounded-poll, no fixed sleeps):

```rust
//! build pipeline tests (M5 T6): parameter rebuild end-to-end with the
//! fake interpreter, refusal guards, failure rows, and the version-record
//! gates split by path (turn vs build). No network; the interpreter is
//! the fake_python.py wrapper, the agent (one test) is fake_claude.py.

use app::bridge::{Bridge, BuildRefusal, Message};
use cosmic::iced::futures::channel::mpsc::UnboundedReceiver;
use cosmic::iced::futures::StreamExt;
use engine::project::store::{load_chat, load_metadata, Role};
use engine::settings::Settings;
use std::path::PathBuf;
use std::time::Duration;

/// The fakes read their env at spawn time and parallel #[tokio::test]s
/// share one process: every setter/clearer of a CAD_FAKE_* or
/// CAD_TEST_CLAUDE var holds this lock for the whole test (MAJOR 4;
/// house pattern — engine/tests/session_lifecycle.rs, M4 bridge_e2e.rs).
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn fake_python() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake-agent/fake_python.py")
}
fn fake_agent() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake-agent/fake_claude.py")
}
fn golden_fixtures() -> PathBuf {
    // BLOCKER 1: the build123d outputs live in fixtures/golden —
    // fixtures/upstream holds only the prompt + xcstrings.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
}

fn project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("cad-pipe-{}", uuid::Uuid::new_v4()));
    engine::project::store::create_project(
        root.parent().unwrap(), root.file_name().unwrap().to_str().unwrap()).unwrap();
    std::fs::write(root.join("model.py"), "wall = 2\nheight = 10\n").unwrap();
    root
}

/// Drains bridge messages until `pred`, bounded (5 s) — never a fixed
/// sleep; M4 bridge_e2e.rs's `recv` loop inlined as a predicate (MINOR 12).
async fn run_until(messages: &mut UnboundedReceiver<Message>, pred: impl Fn(&Message) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, messages.next()).await {
            Ok(Some(message)) if pred(&message) => return,
            Ok(Some(_)) => continue,
            Ok(None) => panic!("message stream ended before predicate"),
            Err(_) => panic!("timed out waiting for predicate"),
        }
    }
}

#[tokio::test]
async fn slider_rebuild_rewrites_model_records_version_and_outputs() {
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    // SAFETY: fake-python env holds this suite's ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_FAKE_PY_MODE", "copy");
        std::env::set_var("CAD_FAKE_PY_FIXTURES", golden_fixtures());
    }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.set_parameter("wall", 3.0).unwrap();
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    assert_eq!(std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 10\n");
    assert!(root.join("output/model.3mf").exists());
    let metadata = load_metadata(&root).unwrap();
    assert_eq!(metadata.current_version, Some(1));
    let chat = load_chat(&root).unwrap();
    assert_eq!(chat.last().unwrap().role, Role::Summary);
    assert_eq!(chat.last().unwrap().text, "Version 1");
    assert_eq!(chat.last().unwrap().detail.as_deref(), Some("1"));
    // second commit: a NEW version only when model.py changed (it did)
    bridge.set_parameter("height", 12.0).unwrap();
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(2));
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE"); }
}
```

  (Summary-row text asserts the EN catalog value "Version 1" — the fl key `versione-lld`; if the Bridge stores the fl-formatted string, format via the app's own `fl!` in the assertion context. `Bridge::with_interpreter(...).launch(Settings::default(), root)` names the test-construction seam — it composes with M4's landed `Bridge::launch(Settings, root) -> (Bridge, UnboundedReceiver<Message>)`: the wither overrides the interpreter and leaves agent selection to the engine's env seam (`CAD_TEST_CLAUDE`), exactly how M4's bridge_e2e points turns at the fake; assertions are the contract.) Additional tests, same harness:

```rust
#[tokio::test]
async fn set_parameter_and_build_refused_while_busy() {
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "hang");
             std::env::set_var("CAD_FAKE_PY_FIXTURES", golden_fixtures()); }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    assert_eq!(bridge.set_parameter("wall", 3.0), Err(BuildRefusal::Busy));
    assert_eq!(bridge.start_build(None), Err(BuildRefusal::Busy));
    bridge.cancel();
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE"); }
}

#[tokio::test]
async fn set_parameter_without_ready_python_still_rewrites_model_py() {
    // MINOR 1: setParameter guards isBusy ONLY (Project.swift:343-362) —
    // the file is rewritten first; build()'s ready-guard refuses and
    // presents the sheet, leaving the rewrite in place.
    let root = project();
    let (mut bridge, _messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    // python_state Unknown — deliberately NOT marked ready; no env, no lock
    assert_eq!(bridge.set_parameter("wall", 3.0), Ok(()));
    assert_eq!(std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 10\n");
    assert_eq!(load_metadata(&root).unwrap().current_version, None); // no build ran
    assert!(bridge.sheet_requested_for_test());
}

#[tokio::test]
async fn build_failure_rows_ask_agent_to_fix_with_traceback() {
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "fail"); }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    assert_eq!(bridge.build_error(), Some("ValueError: bad shape"));
    let chat = load_chat(&root).unwrap();
    let last = chat.last().unwrap();
    assert_eq!(last.role, Role::System);
    assert_eq!(last.action, Some(engine::project::store::Action::AskClaudeToFix));
    assert!(last.detail.as_deref().unwrap().contains("Traceback"));
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE"); }
}

#[tokio::test]
async fn turn_success_without_outputs_records_no_version() {
    // BLOCKER 3: the outputs gate is TURN-PATH only (agentDidFinish,
    // Project.swift:269-271). Fake agent, pong scenario — a successful
    // turn that writes no output files records nothing.
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    // SAFETY: agent-fake env holds this suite's ENV_LOCK (M4 bridge_e2e
    // set_env pattern: fake agent, scenario, per-test registry).
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fake_agent());
        std::env::set_var("CAD_FAKE_SCENARIO",
            fake_agent().parent().unwrap().join("scenarios/pong.jsonl"));
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
    }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.enqueue("a 20 mm cube".into(), Vec::new()).unwrap();
    // emitted after the tail's disk writes, so the asserts below are safe
    run_until(&mut messages, |m| matches!(m, Message::TurnSuccessBookkeeping)).await;
    assert_eq!(load_metadata(&root).unwrap().current_version, None);
    assert!(!load_chat(&root).unwrap().iter().any(|m| m.role == Role::Summary));
    unsafe {
        std::env::remove_var("CAD_FAKE_SCENARIO");
        std::env::remove_var("CAD_TEST_CLAUDE");
        std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
    }
}

#[tokio::test]
async fn build_with_missing_outputs_records_on_changes_alone() {
    // BLOCKER 3: build-path parity (Project.swift:397-401 → recordVersion's
    // only internal guard is hasChanges, :327) — a successful build with NO
    // outputs still records v1 because model.py differs from nothing.
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "verify"); } // exits 0, writes nothing
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(1));
    assert_eq!(load_chat(&root).unwrap().last().unwrap().role, Role::Summary);
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE"); }
}

#[tokio::test]
async fn unchanged_model_py_records_no_second_version() {
    // BLOCKER 3: has_changes is the build path's one guard — a second
    // successful build that did not touch model.py records nothing.
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "verify"); }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(1));
    bridge.start_build(None).unwrap();               // same model.py bytes
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(1)); // still 1
    assert_eq!(load_chat(&root).unwrap().iter()
        .filter(|m| m.role == Role::Summary).count(), 1);
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE"); }
}

#[tokio::test]
async fn send_refused_without_ready_python_and_keeps_draft() {
    let root = project();
    let (mut bridge, _messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    // python_state Unknown: sending must refuse, keep the draft, request the sheet
    let refused = bridge.enqueue("make a cube".into(), Vec::new()).unwrap_err();
    assert_eq!(refused, BuildRefusal::PythonNotReady);
    assert_eq!(bridge.draft_text_for_test(), Some("make a cube".to_string()));
    assert!(bridge.sheet_requested_for_test());
}

#[test]
fn parameter_list_refreshes_from_manifest_in_model_py_order() {
    let root = project();
    std::fs::write(root.join("output/manifest.json"),
        r#"{"parameters": {"height": 10, "wall": 2}}"#).unwrap();
    let params = app::bridge::refresh_parameters(&root); // pure helper the Bridge calls
    assert_eq!(params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["wall", "height"]);
}
```

- [ ] **Step 2: RED** → **Step 3: Implement** the Bridge extensions + `refresh_parameters` (pure: `Manifest::load` → `ordered_parameters` with the current `model.py`).
- [ ] **Step 4: GREEN** — focused suite + `cargo test -p app`.
- [ ] **Step 5: Commit** — `git add app/src/bridge.rs app/src/chat.rs app/tests/build_pipeline.rs app/tests/bridge_e2e.rs && git commit -m "feat(app): slider rebuild pipeline — model.py rewrite, agent-less build, version recording"` (bridge_e2e.rs included when its `enqueue` callers were updated for the Result signature, MINOR 11)

### Task 7: App VersionMenu + restore + compare wiring

**Files:**
- Create: `app/src/version_menu.rs` (pure entries + widget), `app/tests/version_menu_model.rs`
- Modify: `app/src/bridge.rs` (restore/compare), `app/src/app.rs` (menu in the toolbar, Compila ⌘B command), `app/src/parameters_view.rs` (compare picker emits the message)
- Test: `app/tests/version_menu_model.rs`, restore e2e in `app/tests/build_pipeline.rs` (append)

**Interfaces:**
- Consumes: T3 engine versions API, T6 Bridge state.
- Produces:
  - `#[derive(Debug, Clone, PartialEq)] pub struct VersionEntry { pub number: i64, pub created_at: time::OffsetDateTime, pub prompt_summary: String, pub is_current: bool }`
  - `pub fn prompt_summary(prompt: &str) -> String` — first line; longer than 60 chars → first 60 + "…" (VersionMenu.swift:40-43).
  - `pub fn menu_entries(versions: &[ProjectVersion], current: Option<i64>) -> Vec<VersionEntry>` — newest first (upstream `.reversed()`).
  - `impl Bridge { pub fn restore(&mut self, number: i64) -> Result<(), BuildRefusal>; pub fn compare_with(&mut self, number: Option<i64>); pub fn comparison_parameters(&self) -> &BTreeMap<String, f64>; }`
    - `restore`: refuse Busy; `versions::restore(root, version)`; on success set `metadata.session_id = version.session_id.clone()`, `metadata.current_version = Some(number)`, `save_metadata`; reload chat state (`ChatState::new(load_chat(root).unwrap_or_default())`), refresh outputs/parameters idempotently (same reload the watcher path uses), clear `build_error` + `missing_outputs`, activity `Succeeded(fl!("versione-lld-ripristinata", number))`; on engine error → `Failed(fl!("impossibile-ripristinare-la-versione-lld", number))` + `build_error = err`. Note parity: upstream does NOT reset `compare_version` on restore; the port keeps it.
    - `compare_with(number)`: store `compare_version = number`; `comparison_parameters = versions::comparison_parameters(root, number)`; T5's overlay renders previous values from it (the ghost MESH is M6 — documented out of scope).
  - Widget: menu label clock icon + `v{current}` when set; empty → `fl!("nessuna-versione-salvata")`; entries disabled while busy, section header `fl!(project.is_busy ? "disponibili-al-termine-della-generazione" : "torna-a-una-versione")`, current entry checkmarked, subtitle = relative + abbreviated date. Command "Compila modello" ⌘B added (disabled when busy; Project.swift isBusy guard) — `fl!("compila-modello")` (grep the exact slug; the corpus contains it).
- Append to `app/tests/build_pipeline.rs`:

```rust
#[tokio::test]
async fn restore_returns_project_to_the_snapshot() {
    let _guard = ENV_LOCK.lock().await; // sets CAD_FAKE_PY_* (MAJOR 4)
    let root = project();
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "copy");
             std::env::set_var("CAD_FAKE_PY_FIXTURES", golden_fixtures()); }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.set_parameter("wall", 3.0).unwrap();
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    let sid_v1 = load_metadata(&root).unwrap().session_id.clone();
    bridge.set_parameter("wall", 5.0).unwrap();
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    assert_eq!(std::fs::read_to_string(root.join("model.py")).unwrap(), "wall = 5\nheight = 10\n");
    bridge.restore(1).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 10\n");
    let metadata = load_metadata(&root).unwrap();
    assert_eq!(metadata.current_version, Some(1));
    assert_eq!(metadata.session_id, sid_v1);
    // the chat transcript is the snapshot's (summary row of v1 is the last)
    let chat = load_chat(&root).unwrap();
    assert_eq!(chat.last().unwrap().detail.as_deref(), Some("1"));
    // compare: previous parameter values come from version 2's manifest
    bridge.compare_with(Some(2));
    assert_eq!(bridge.comparison_parameters().get("wall"), Some(&5.0));
    bridge.compare_with(None);
    assert!(bridge.comparison_parameters().is_empty());
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE"); }
}

#[tokio::test]
async fn restore_refused_while_busy() {
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "hang"); }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    assert_eq!(bridge.restore(1), Err(BuildRefusal::Busy));
    bridge.cancel();
    run_until(&mut messages, |m| matches!(m, Message::BuildFinished { .. })).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE"); }
}
```

  Pure tests in `app/tests/version_menu_model.rs`:

```rust
#[test]
fn prompt_summary_takes_first_line_capped_at_sixty() {
    assert_eq!(app::version_menu::prompt_summary("line one\nline two"), "line one");
    let long = "x".repeat(61);
    assert_eq!(app::version_menu::prompt_summary(&long), format!("{}…", "x".repeat(60)));
    assert_eq!(app::version_menu::prompt_summary(""), "");
}

#[test]
fn entries_are_newest_first_with_current_marked() {
    let vs = (1..=3).map(|n| engine::project::versions::ProjectVersion {
        number: n,
        created_at: time::OffsetDateTime::now_utc(),
        prompt: format!("p{n}"),
        session_id: None,
    }).collect::<Vec<_>>();
    let entries = app::version_menu::menu_entries(&vs, Some(2));
    assert_eq!(entries.iter().map(|e| e.number).collect::<Vec<_>>(), [3, 2, 1]);
    assert!(entries[1].is_current && !entries[0].is_current);
}
```

- [ ] **Step 2: RED** → **Step 3: Implement** → **Step 4: GREEN** (focused + `cargo test -p app`).
- [ ] **Step 5: Commit** — `git add app/src/version_menu.rs app/src/bridge.rs app/src/app.rs app/src/parameters_view.rs app/tests/version_menu_model.rs app/tests/build_pipeline.rs && git commit -m "feat(app): version menu, restore, and compare wiring"`

### Task 8: PythonSetupSheet, startup check, M5 wrap

**Files:**
- Create: `app/tests/setup_sheet.rs`
- Modify: `app/src/sheets.rs` (PythonSetupSheet), `app/src/bridge.rs` or `app/src/app.rs` (startup `check` + reinstall), `docs/research/carry-forwards.md` (strike the landed M5 lines), `app/src/lib.rs` docs
- Test: `app/tests/setup_sheet.rs` + the wrap slice

**Interfaces:**
- Consumes: T0 `python::env::{EnvEvent, EnvFailure, EnvState, check, reinstall, install}`, M4 sheets style, `engine::Settings::has_completed_setup` (sheet auto-presents only when setup was completed — upstream `isSetupSheetPresented = AppSettings.hasCompletedSetup`).
- Produces:
  - `#[derive(Debug, Clone, PartialEq)] pub struct SetupSheetState { pub state: EnvState, pub log: String, pub progress: f64, pub step: String }`
  - `impl SetupSheetState { pub fn new() -> Self; pub fn apply(&mut self, event: &EnvEvent) }` — Log appends `+ "\n"`; Progress sets both fields; `Finished(Ok)` → `Ready { build123d_version }` (T6 reads this for `python_ready`); `Finished(Err(EnvFailure::Missing))` → `EnvState::Missing`; `Finished(Err(EnvFailure::CheckFailed(m) | InstallFailed(m)))` → `Failed(m)` (MAJOR 1, shape (b): Missing reaches the reducer through the typed terminal). `Checking`/`Installing` are APP-side (the review's sanctioned alternative): the sheet sets its own state to `Checking` when it kicks `check` and `Installing` when it kicks `install`/`reinstall`. The sheet stays non-dismissable while `Installing` ONLY (upstream `interactiveDismissDisabled(python.isInstalling)`, PythonSetupSheet.swift:58 — dismissable during `Checking`; MINOR 8).
  - `pub enum SetupButtons { Done, Busy, LaterAndInstall }` + `pub fn buttons(state: &EnvState) -> SetupButtons` (PythonSetupSheet.swift:61-82: ready → Done; installing/checking → disabled; else Later + Install).
  - Sheet view: header `fl!("installazione-componenti")` + the corpus description line (`cad-studio-usa-un-ambiente-python-dedicato-…`), progress bar + step when installing or progress > 0, failure label when Failed, `fl!("mostra-dettagli")` disclosure with the monospaced log, width 520.
  - Startup wiring: on project-window open (upstream checks per Project init? No — upstream checks at app start and on demand; the app runs `check(venv_dir(), &streaming_spawn())` once at `init` with the sheet state preset to `Checking`, mirrors the terminal into the Bridge's `python_state` and the sheet state; `Install`/`Reinstall` buttons set the sheet to `Installing` and call `install`/`reinstall` with the production seams — `streaming_spawn()`, `command_probe()` — draining their EnvEvents as Messages). Note: a `Missing` TERMINAL is what presents the sheet, and only when `has_completed_setup` (upstream parity, PythonEnvironment.swift:36-46; the typed failure exists for exactly this, MAJOR 1).

- [ ] **Step 1: Failing tests** — `app/tests/setup_sheet.rs`:

```rust
//! setup sheet tests (M5 T8): event-to-state reduction and button model —
//! synthetic EnvEvents, no processes.

use app::sheets::{SetupButtons, SetupSheetState};
use engine::python::env::{EnvEvent, EnvState};

#[test]
fn events_reduce_to_sheet_state() {
    let mut s = SetupSheetState::new();
    assert_eq!(s.state, EnvState::Unknown);
    s.apply(&EnvEvent::Progress { value: 0.1, step: "Creating the virtual environment…".into() });
    assert_eq!((s.progress, s.step.as_str()), (0.1, "Creating the virtual environment…"));
    s.apply(&EnvEvent::Log("$ uv venv".into()));
    s.apply(&EnvEvent::Log("resolving…".into()));
    assert!(s.log.contains("$ uv venv\n"));
    s.apply(&EnvEvent::Finished(Ok(engine::python::env::EnvReady {
        build123d_version: "0.8.1".into(),
        versions: "Python 3.12.4, build123d 0.8.1".into(),
    })));
    assert_eq!(s.state, EnvState::Ready { build123d_version: "0.8.1".into() });
    // no versions_display() assertion here (MINOR 7): the shared cell is
    // set by the T0 drivers, not by the sheet reducer — T0 pins it.
    let mut f = SetupSheetState::new();
    // MAJOR 1 (shape (b)): typed failures — Missing is its own sheet state
    f.apply(&EnvEvent::Finished(Err(engine::python::env::EnvFailure::Missing)));
    assert_eq!(f.state, EnvState::Missing);
    f.apply(&EnvEvent::Finished(Err(engine::python::env::EnvFailure::InstallFailed(
        "The command “uv” failed. Check the log for details.".into()))));
    assert_eq!(f.state, EnvState::Failed(
        "The command “uv” failed. Check the log for details.".into()));
}

#[test]
fn button_model_matches_upstream_states() {
    assert!(matches!(app::sheets::buttons(&EnvState::Ready { build123d_version: String::new() }), SetupButtons::Done));
    assert!(matches!(app::sheets::buttons(&EnvState::Installing), SetupButtons::Busy));
    assert!(matches!(app::sheets::buttons(&EnvState::Checking), SetupButtons::Busy));
    assert!(matches!(app::sheets::buttons(&EnvState::Unknown), SetupButtons::LaterAndInstall));
    assert!(matches!(app::sheets::buttons(&EnvState::Missing), SetupButtons::LaterAndInstall));
    assert!(matches!(app::sheets::buttons(&EnvState::Failed("x".into())), SetupButtons::LaterAndInstall));
}
```

  The wrap slice (append to `app/tests/build_pipeline.rs` or a new `app/tests/m5_slice.rs`): extend M4's vertical slice — after the fake agent's first turn, `set_parameter("wall", 3.0)` → model.py rewritten → fake build (`copy` mode, `CAD_FAKE_PY_FIXTURES` = `fixtures/golden`) copies the outputs → watcher-driven reload rebuilds the scene → version 1 recorded with a summary row → `restore(1)` after a second rebuild returns model.py/chat/output to the snapshot → compare picker exposes version 2's parameters. Asserted against bridge + pure models, headless.
- [ ] **Step 2: RED** → **Step 3: Implement** sheet state + view + startup wiring; strike carry-forward M5 lines (`PYTHON_UNVERIFIED` fallback line, the three classifier-divergence bullets, the reap bullet).
- [ ] **Step 4: GREEN** — `cargo test -p app`; controller runs the thelio full-suite gate (`cargo test --workspace --all-targets`) single-invocation, HEAD-verified.
- [ ] **Step 5: Commit** — `git add app/src/sheets.rs app/src/app.rs app/src/bridge.rs app/tests/setup_sheet.rs docs/research/carry-forwards.md && git commit -m "feat(app): python setup sheet and startup check; docs: M5 carry-forwards struck"`

---

**Execution note:** Engine tasks 0-3 touch disjoint files EXCEPT T1/T3 (MINOR 2): both modify `engine/src/project/mod.rs` (registration) and `engine/src/project/store.rs` (atomic_write visibility) — serialize T1 → T3, or dispatch parallel and let the controller union the registration lines and the visibility change at merge (trivial, but the merge must not drop either). Otherwise disjoint (python/env+locator+mod; python/model+fixture; project/versions+outputs) — dispatch as parallel worktrees after T0's `python/mod.rs` skeleton lands (T2 needs it; T0 creates `model.rs` as a stub, so in practice dispatch T0 first, then 1 → 3 (serial) with 2 parallel, then 4 (needs T0's cell + touches session.rs/processes.rs, disjoint from T1/T3). App tasks gate on M4 merged + engine merged: T5 is fresh files (parallel with nothing app-side once viewer_pane/app.rs exist from M4); T6 after T5's `on_commit` seam + engine 1/2/3; T7 after T6 (both edit bridge.rs — serial); T8 last, owns the wrap + doc strikes. Controller merges serially; thelio gates single-invocation, never stacked; the merge checklist stages the regenerated root `Cargo.lock` whenever a manifest changed (T1 adds `regex`, MINOR 3). Reviewers stagger, never concurrent with a 4-implementer wave; ≤5 agents total.

**Self-review notes (kept for the controller):** spec coverage — §10 M5's three nouns map to T0/T2 (env setup), T1/T5/T6 (sliders rebuild), T3/T7 (snapshot/restore); PythonSetupSheet (§4.4) is T8; carry-forwards all owned (fallback text → T4; three divergences → T4 tests; reap → T4). Type consistency checked across task Interfaces (Spawn/Probe/EnvEvent/EnvFailure from T0 used verbatim in T8; Rewrite.literal consumed by T6's version prompt; BuildRefusal shared by T6/T7 — now Busy-only in `set_parameter`, MINOR 1). Rev-2 fold self-check: every review finding applied (3 BLOCKER / 4 MAJOR / 12 MINOR / 4 NOTE); Review Focus #3 now names the same three T6 tests the task defines (no BLOCKER-3-class self-contradiction); Global Constraints' slider-commit line and version-gate line agree with T5/T6 semantics; produced/consumed types re-checked after the seam and vocabulary adjudications. T0 amendments follow the fix-round adjudications (probe-mediated discovery; EnvFailure shape (b) with the failure milestone) so the plan matches the in-flight code. Known deliberate scope lines: compare ghost mesh (M6), Compila command wired here but motion/parts overlays not touched, `parameters_collapsed` persisted through engine Settings per the ONE-writer rule.
