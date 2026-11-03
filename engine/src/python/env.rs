//! Python venv lifecycle: the install planner, the verification contract,
//! and the check/install drivers (PythonEnvironment.swift).
//!
//! GUI-free by the spec's engine rule: the engine has no catalog, so every
//! user-visible string travels as a stable id the app localizes on receipt
//! ([`EnvStep`], [`InstallError`], [`EnvLog`]; process output stays
//! verbatim), and process work flows through two injectable seams — [`Spawn`] for the streamed install steps and
//! `check`'s verification, [`Probe`] for `--version` discovery — so tests
//! script fakes and never run real uv/pip/build123d.
//!
//! Two divergences from upstream, both forced by the plan's own test
//! fixtures:
//! - uv discovery walks the same dirs [`find_executable`][locator] searches
//!   but resolves the winner through the [`Probe`] seam (upstream's
//!   `locate` is a plain file check). A test must be able to force the uv
//!   branch with a fake probe on a machine without uv and to suppress it
//!   on a machine with one; `command_probe` fails fast on nonexistent
//!   files, so real-world discovery behaves the same.
//! - the post-install verification runs the interpreter directly instead
//!   of through [`Spawn`]: upstream's `build123dVersion()` is a buffered
//!   ProcessRunner run with no line handler — unlike the streamed install
//!   steps — and the install tests script the spawn seam with output-less
//!   steps while the freshly written interpreter must really run.
//!   `check`'s verification keeps going through the seam.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, RwLock};

use tokio::sync::mpsc;

use crate::agent::locator::{self, shell_env};
use crate::processes::{Line, RunningProcess, spawn_streaming};

/// The packages every venv install gets, unpinned, upstream order
/// (PythonEnvironment.swift:16).
pub const PACKAGES: [&str; 3] = ["build123d", "matplotlib", "numpy"];

/// What the versions line shows until a verification succeeds — upstream's
/// pre-verification display value (AgentSession.swift:206).
pub const VERSIONS_FALLBACK: &str = "build123d, matplotlib, numpy";

/// The verification script (PythonEnvironment.swift:135-139): line 1 is
/// the build123d version, line 2 the full versions line the prompt and the
/// shared cell carry.
pub const VERIFY_SCRIPT: &str = "import sys, build123d, matplotlib, numpy\nprint(build123d.__version__)\nprint(f\"Python {sys.version.split()[0]}, build123d {build123d.__version__}, matplotlib {matplotlib.__version__}, numpy {numpy.__version__}\")";

enum_with_all! {
/// A progress milestone's step label (upstream `advance(to:step:)`'s
/// localized strings, PythonEnvironment.swift:68-97). The engine has no
/// catalog, so it names the step and the app localizes it — the ids are the
/// stable contract; each variant names the catalog key it maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvStep {
    /// `ricerca-di-python-3-10-3-13`
    SearchingPython,
    /// `creazione-dell-ambiente-virtuale-con-uv`
    CreatingVenvWithUv,
    /// `creazione-dell-ambiente-virtuale`
    CreatingVenv,
    /// `aggiornamento-di-pip`
    UpgradingPip,
    /// `installazione-di-build123d-matplotlib-e-numpy`
    InstallingPackages,
    /// `verifica-dell-installazione`
    Verifying,
    /// `componenti-installati`
    Installed,
    /// `installazione-non-riuscita`
    InstallFailed,
}
}

/// Why an install failed (upstream `PythonSetupError`, plus the spawn
/// errors ProcessRunner throws). Named, not worded: the app localizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// `non-stato-trovato-n-uv-n-un-python-3-10-3-13-…` (pythonNotFound).
    PythonNotFound,
    /// `il-comando-non-riuscito-controlla-il-log-per-i-dettagli`
    /// (commandFailed) — `command` is the program's last path component.
    CommandFailed { command: String },
    /// `build123d-e-matplotlib-non-si-importano-correttamente-…`
    /// (verificationFailed).
    VerificationFailed,
    /// The step's program could not be spawned; `error` is the OS error
    /// text, shown verbatim like upstream's `localizedDescription`.
    SpawnFailed { command: String, error: String },
}

/// One install-log line. Process output and command echoes are verbatim;
/// the lines upstream writes with `String(localized:)` travel as ids.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvLog {
    /// A `$ argv…` echo or one line of a step's output.
    Text(String),
    /// A milestone's step label (upstream `advance` logs it too).
    Step(EnvStep),
    /// `trovato-in`: the venv fallback's chosen interpreter.
    FoundPython { version: String, path: PathBuf },
    /// The failed install's error line.
    Failure(InstallError),
}

/// Upstream `State` (PythonEnvironment.swift:6-13), app-side: the drivers
/// speak [`EnvEvent`]s and the app reduces them into this.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvState {
    Unknown,
    Checking,
    Missing,
    Installing,
    Ready { build123d_version: String },
    Failed(String),
}

/// A verified environment: the build123d version and the full versions
/// line.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvReady {
    pub build123d_version: String,
    pub versions: String,
}

/// One install step: the milestone to advance to before running it, the
/// step text, and the argv to run (program first).
pub struct InstallStep {
    pub progress: f64,
    pub step: EnvStep,
    pub argv: Vec<String>,
}

/// Driver events: `install` streams log lines and progress (a failed
/// install closes with the [`EnvStep::InstallFailed`] label at the bar's
/// last position, then the error line); `check` emits only its terminal
/// event. The install-kickoff signal (`EnvState::Installing`) is app-side.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvEvent {
    Log(EnvLog),
    Progress { value: f64, step: EnvStep },
    Finished(Result<EnvReady, EnvFailure>),
}

/// Why a driver finished in failure. `Missing` (no executable interpreter
/// at `dir/bin/python`) gates the setup sheet's presentation apart from
/// actual failures upstream (Swift:36-46) and carries no label; `CheckFailed`
/// is the `l-ambiente-python-esiste-ma-build123d-o-matplotlib-non-si-importano`
/// state (Swift:43), `InstallFailed` any install error.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvFailure {
    Missing,
    CheckFailed,
    InstallFailed(InstallError),
}

/// A driver's event stream; bounded (256) with the same pacing contract as
/// turns — senders await, so a draining reader paces the install.
pub struct EnvHandle {
    pub events: mpsc::Receiver<EnvEvent>,
}

/// The canonical app-wide venv location, `<data_dir>/venv`
/// ([`crate::storage::data_dir`]): `~/.local/share/cad-studio/venv` on
/// Linux (spec §2/§6), `~/Library/Application Support/cad-studio/venv` on a
/// macOS dev box. The agent session feeds this same path to `{{PYTHON}}`
/// and the restricted-bash allowlist. Only this helper is ambient; every
/// other function takes `dir` injected for tests.
pub fn venv_dir() -> PathBuf {
    crate::storage::data_dir().join("venv")
}

/// `dir/bin/python` (PythonEnvironment.interpreter).
pub fn interpreter_path(dir: &Path) -> PathBuf {
    dir.join("bin/python")
}

/// The uv plan (PythonEnvironment.createWithUV:80-85): create the venv
/// pinned to CPython 3.12, then install the packages into its
/// interpreter.
pub fn uv_install_plan(uv: &Path, dir: &Path) -> Vec<InstallStep> {
    let uv = uv.to_string_lossy().into_owned();
    vec![
        InstallStep {
            progress: 0.1,
            step: EnvStep::CreatingVenvWithUv,
            argv: vec![
                uv.clone(),
                "venv".into(),
                "--python".into(),
                "3.12".into(),
                dir.to_string_lossy().into_owned(),
            ],
        },
        InstallStep {
            progress: 0.25,
            step: EnvStep::InstallingPackages,
            argv: install_argv(
                &uv,
                &[
                    "pip",
                    "install",
                    "--python",
                    &interpreter_path(dir).to_string_lossy(),
                ],
            ),
        },
    ]
}

/// The venv-fallback plan (PythonEnvironment.createWithVenv:92-97): bare
/// venv, pip upgrade, then the same package install.
pub fn venv_install_plan(python: &Path, dir: &Path) -> Vec<InstallStep> {
    let python = python.to_string_lossy().into_owned();
    let interpreter = interpreter_path(dir).to_string_lossy().into_owned();
    vec![
        InstallStep {
            progress: 0.1,
            step: EnvStep::CreatingVenv,
            argv: vec![
                python.clone(),
                "-m".into(),
                "venv".into(),
                dir.to_string_lossy().into_owned(),
            ],
        },
        InstallStep {
            progress: 0.2,
            step: EnvStep::UpgradingPip,
            argv: vec![
                interpreter.clone(),
                "-m".into(),
                "pip".into(),
                "install".into(),
                "--upgrade".into(),
                "pip".into(),
            ],
        },
        InstallStep {
            progress: 0.25,
            step: EnvStep::InstallingPackages,
            argv: install_argv(&interpreter, &["-m", "pip", "install"]),
        },
    ]
}

/// `<program> <middle…> PACKAGES` — both install paths end with the same
/// unpinned package list.
fn install_argv(program: &str, middle: &[&str]) -> Vec<String> {
    let mut argv = vec![program.to_string()];
    argv.extend(middle.iter().map(|arg| (*arg).to_string()));
    argv.extend(PACKAGES.iter().map(|package| (*package).to_string()));
    argv
}

/// `"Python 3.12.4\n"` → `Some((3, 12))`; versions outside the support
/// window still parse — the window is [`is_supported_python`]'s call.
/// Anything without the `Python ` prefix and two numeric components is
/// `None`.
pub fn parse_python_version(output: &str) -> Option<(u32, u32)> {
    let version = output.trim().strip_prefix("Python ")?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// The 3.10–3.13 window (PythonEnvironment.compatiblePython:108).
pub fn is_supported_python(major: u32, minor: u32) -> bool {
    major == 3 && (10..=13).contains(&minor)
}

/// Candidate interpreter names, best first (compatiblePython:101).
pub fn python_candidates() -> [&'static str; 5] {
    [
        "python3.13",
        "python3.12",
        "python3.11",
        "python3.10",
        "python3",
    ]
}

/// Splits verification stdout (PythonEnvironment.build123dVersion:143-146):
/// the first line is the build123d version, the second (empty when absent)
/// the versions line. Nothing parseable → `None`.
pub fn parse_verification(output: &str) -> Option<(String, String)> {
    let mut lines = output
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let build123d_version = lines.next()?.to_string();
    let versions = lines.next().unwrap_or_default().to_string();
    Some((build123d_version, versions))
}

/// The per-output-line nudge (nudgeProgress:154-158): `+0.004` only while
/// `0.25 ≤ progress < 0.9`, so the bar never creeps into the verify band.
pub fn nudge_progress(progress: f64) -> f64 {
    if (0.25..0.9).contains(&progress) {
        progress + 0.004
    } else {
        progress
    }
}

/// Upstream `PythonEnvironment.shared.versions` parity: one venv serves
/// every project window, so the verified versions line is process-global.
static SHARED_VERSIONS: LazyLock<Arc<RwLock<Option<String>>>> =
    LazyLock::new(|| Arc::new(RwLock::new(None)));

/// The shared versions cell itself, for embedders that hold it alongside
/// their own state.
pub fn shared_versions() -> Arc<RwLock<Option<String>>> {
    Arc::clone(&SHARED_VERSIONS)
}

/// Overwrites the shared cell (the drivers do this on verification
/// outcomes).
pub fn set_shared_versions(value: Option<String>) {
    let mut cell = SHARED_VERSIONS.write().unwrap_or_else(|e| e.into_inner());
    *cell = value;
}

/// The display line: the verified versions line, or upstream's fallback
/// while no verification has succeeded. An empty verified line (the
/// single-line-output case) falls back too.
pub fn versions_display() -> String {
    let cell = SHARED_VERSIONS.read().unwrap_or_else(|e| e.into_inner());
    cell.as_ref()
        .filter(|line| !line.is_empty())
        .cloned()
        .unwrap_or_else(|| VERSIONS_FALLBACK.to_string())
}

/// Spawn seam for the streamed install steps and `check`'s verification.
pub type Spawn = Arc<dyn Fn(&[String], &Path) -> io::Result<RunningProcess> + Send + Sync>;

/// The production spawn: `processes::spawn_streaming` under the
/// login-shell environment agents run with.
pub fn streaming_spawn() -> Spawn {
    Arc::new(|argv: &[String], cwd: &Path| {
        let env: Vec<(String, String)> = shell_env().into_iter().collect();
        spawn_streaming(argv, cwd, &env)
    })
}

/// Probe seam for `--version` discovery: `(exit code, combined
/// stdout+stderr)` — upstream reads both streams and trims.
pub type Probe = Arc<dyn Fn(&Path, &[String]) -> Option<(i32, String)> + Send + Sync>;

/// The production probe: a buffered std Command with null stdin under the
/// shell environment (locator's `validate` is the same shape).
pub fn command_probe() -> Probe {
    Arc::new(|program: &Path, args: &[String]| {
        let output = std::process::Command::new(program)
            .args(args)
            .envs(&shell_env())
            .stdin(Stdio::null())
            .output()
            .ok()?;
        let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
        Some((output.status.code().unwrap_or(-1), combined))
    })
}

/// Checks an existing venv (PythonEnvironment.check:34-47): a
/// non-executable interpreter fails with [`EnvFailure::Missing`] without
/// running anything (upstream's `.missing`), otherwise the verification
/// runs through `spawn` — success sets the shared cell, failure clears it
/// and reports [`EnvFailure::CheckFailed`]. The only event is the terminal
/// `Finished`.
pub fn check(dir: &Path, spawn: &Spawn) -> EnvHandle {
    let (events, handle) = channel();
    let dir = dir.to_path_buf();
    let spawn = Arc::clone(spawn);
    tokio::spawn(async move {
        let outcome = check_run(&dir, &spawn).await;
        let _ = events.send(EnvEvent::Finished(outcome)).await;
    });
    handle
}

/// Installs (or repairs) the venv (PythonEnvironment.install:54-78). The
/// `isInstalling` re-entry guard lives with the app's state, which owns
/// [`EnvState`]; each call here drives one install to a terminal event —
/// a failing one closing with the `Installation failed` label and the
/// error line in the log before [`EnvEvent::Finished`].
pub fn install(dir: &Path, spawn: &Spawn, probe: &Probe) -> EnvHandle {
    let (events, handle) = channel();
    let dir = dir.to_path_buf();
    let spawn = Arc::clone(spawn);
    let probe = Arc::clone(probe);
    tokio::spawn(async move {
        let outcome = install_run(&dir, &spawn, &probe, &events).await;
        let _ = events.send(EnvEvent::Finished(outcome)).await;
    });
    handle
}

/// Removes `dir` ignoring errors, then installs
/// (PythonEnvironment.reinstall:49-52).
pub fn reinstall(dir: &Path, spawn: &Spawn, probe: &Probe) -> EnvHandle {
    let _ = fs::remove_dir_all(dir);
    install(dir, spawn, probe)
}

fn channel() -> (mpsc::Sender<EnvEvent>, EnvHandle) {
    let (sender, events) = mpsc::channel(256);
    (sender, EnvHandle { events })
}

async fn check_run(dir: &Path, spawn: &Spawn) -> Result<EnvReady, EnvFailure> {
    let interpreter = interpreter_path(dir);
    if !locator::is_executable_file(&interpreter) {
        // Upstream's .missing: no process runs and the versions cell keeps
        // whatever a previous verification wrote.
        return Err(EnvFailure::Missing);
    }
    let argv = vec![
        interpreter.to_string_lossy().into_owned(),
        "-c".to_string(),
        VERIFY_SCRIPT.to_string(),
    ];
    let verified = run_captured(spawn, &argv, work_dir(dir))
        .await
        .and_then(|(ok, output)| ok.then_some(output))
        .and_then(|output| parse_verification(&output));
    match verified {
        Some((build123d_version, versions)) => {
            set_shared_versions(Some(versions.clone()));
            Ok(EnvReady {
                build123d_version,
                versions,
            })
        }
        None => {
            set_shared_versions(None);
            Err(EnvFailure::CheckFailed)
        }
    }
}

async fn install_run(
    dir: &Path,
    spawn: &Spawn,
    probe: &Probe,
    events: &mpsc::Sender<EnvEvent>,
) -> Result<EnvReady, EnvFailure> {
    let env = shell_env();
    if let Some(parent) = dir.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        let _ = fs::create_dir_all(parent);
    }
    let steps = match locate_uv(&env, probe) {
        Some(uv) => uv_install_plan(&uv, dir),
        None => {
            // the search milestone precedes the scan, so its failure label
            // carries 0.05, not 0
            advance(events, 0.05, EnvStep::SearchingPython).await;
            match locate_python(&env, probe, events).await {
                Some(python) => venv_install_plan(&python, dir),
                None => {
                    return Err(fail(events, 0.05, InstallError::PythonNotFound).await);
                }
            }
        }
    };
    let cwd = work_dir(dir).to_path_buf();
    for step in &steps {
        advance(events, step.progress, step.step).await;
        if let Err(failure) =
            run_step(&step.argv, &cwd, spawn, events, step.progress, step.step).await
        {
            return Err(fail(events, step.progress, failure).await);
        }
    }
    advance(events, 0.95, EnvStep::Verifying).await;
    let verified = verify_direct(dir, &env)
        .await
        .and_then(|(ok, output)| ok.then_some(output))
        .and_then(|output| parse_verification(&output));
    let Some((build123d_version, versions)) = verified else {
        return Err(fail(events, 0.95, InstallError::VerificationFailed).await);
    };
    advance(events, 1.0, EnvStep::Installed).await;
    set_shared_versions(Some(versions.clone()));
    Ok(EnvReady {
        build123d_version,
        versions,
    })
}

/// Closes out a failed install (Swift:73-76): the step label flips to the
/// [`EnvStep::InstallFailed`] with the bar holding the milestone the
/// install died at (upstream's per-line nudges may have crept it slightly
/// past), the error line lands in the log, and the failure travels on to
/// the terminal event.
async fn fail(events: &mpsc::Sender<EnvEvent>, progress: f64, error: InstallError) -> EnvFailure {
    let _ = events
        .send(EnvEvent::Progress {
            value: progress,
            step: EnvStep::InstallFailed,
        })
        .await;
    let _ = events
        .send(EnvEvent::Log(EnvLog::Failure(error.clone())))
        .await;
    EnvFailure::InstallFailed(error)
}

/// uv wins over the venv fallback (PythonEnvironment.install:62-66).
/// Resolution goes through the Probe seam over the dirs `find_executable`
/// searches — see the module doc for why discovery must be seam-mediated.
fn locate_uv(env: &HashMap<String, String>, probe: &Probe) -> Option<PathBuf> {
    locator::executable_dirs(env)
        .into_iter()
        .map(|dir| dir.join("uv"))
        .find_map(|candidate| {
            let (code, _) = probe(&candidate, &["--version".to_string()])?;
            (code == 0).then_some(candidate)
        })
}

/// The venv fallback's interpreter hunt
/// (PythonEnvironment.compatiblePython:100-114): candidates in order, each
/// probed with `--version`, accepted only inside the 3.10–3.13 window; the
/// winner is logged (EN catalog `trovato-in`).
async fn locate_python(
    env: &HashMap<String, String>,
    probe: &Probe,
    events: &mpsc::Sender<EnvEvent>,
) -> Option<PathBuf> {
    for name in python_candidates() {
        let Some(candidate) = locator::find_executable(name, env) else {
            continue;
        };
        let Some((code, output)) = probe(&candidate, &["--version".to_string()]) else {
            continue;
        };
        if code != 0 {
            continue;
        }
        let Some((major, minor)) = parse_python_version(&output) else {
            continue;
        };
        if !is_supported_python(major, minor) {
            continue;
        }
        let _ = events
            .send(EnvEvent::Log(EnvLog::FoundPython {
                version: output.trim().to_string(),
                path: candidate.clone(),
            }))
            .await;
        return Some(candidate);
    }
    None
}

/// One streamed step (PythonEnvironment.runStep:116-132): the command line
/// into the log, then every output line logged with a progress nudge. A
/// nonzero exit is the commandFailed error, named by the program's last
/// path component. The nudge accumulator is step-local — the preceding
/// advance set the milestone, and the next step's advance resets it.
async fn run_step(
    argv: &[String],
    cwd: &Path,
    spawn: &Spawn,
    events: &mpsc::Sender<EnvEvent>,
    progress: f64,
    step: EnvStep,
) -> Result<(), InstallError> {
    let _ = events
        .send(EnvEvent::Log(EnvLog::Text(format!("$ {}", argv.join(" ")))))
        .await;
    let name = argv
        .first()
        .and_then(|program| Path::new(program).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let process = spawn(argv, cwd).map_err(|e| InstallError::SpawnFailed {
        command: name.clone(),
        error: e.to_string(),
    })?;
    let RunningProcess {
        mut lines,
        exit_status,
        ..
    } = process;
    let mut progress = progress;
    while let Some(line) = lines.recv().await {
        let (Line::Stdout(text) | Line::Stderr(text)) = line;
        let _ = events.send(EnvEvent::Log(EnvLog::Text(text))).await;
        progress = nudge_progress(progress);
        let _ = events
            .send(EnvEvent::Progress {
                value: progress,
                step,
            })
            .await;
    }
    if exit_status
        .await
        .map(|status| status.success())
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(InstallError::CommandFailed { command: name })
    }
}

/// Runs one command through the seam and returns `(exited 0, stdout)`,
/// waiting for the child to be reaped. Stderr lines flow past but are not
/// collected — upstream parses `output` only.
async fn run_captured(spawn: &Spawn, argv: &[String], cwd: &Path) -> Option<(bool, String)> {
    let process = spawn(argv, cwd).ok()?;
    let RunningProcess {
        mut lines,
        exit_status,
        ..
    } = process;
    let mut stdout = String::new();
    while let Some(line) = lines.recv().await {
        if let Line::Stdout(text) = line {
            stdout.push_str(&text);
            stdout.push('\n');
        }
    }
    let status = exit_status.await.ok()?;
    Some((status.success(), stdout))
}

/// The post-install verification, run directly rather than through the
/// Spawn seam (module doc): buffered output, no streaming, environment
/// replaced wholesale like every ProcessRunner run.
async fn verify_direct(dir: &Path, env: &HashMap<String, String>) -> Option<(bool, String)> {
    let output = tokio::process::Command::new(interpreter_path(dir))
        .arg("-c")
        .arg(VERIFY_SCRIPT)
        .current_dir(work_dir(dir))
        .env_clear()
        .envs(
            env.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;
    Some((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

/// One upstream `advance(to:step:)`: the progress milestone, then the step
/// text into the log.
async fn advance(events: &mpsc::Sender<EnvEvent>, value: f64, step: EnvStep) {
    let _ = events.send(EnvEvent::Progress { value, step }).await;
    let _ = events.send(EnvEvent::Log(EnvLog::Step(step))).await;
}

/// Where a driver's children run: the venv's parent (created by install) —
/// a stable existing directory even when `dir` itself does not exist yet.
fn work_dir(dir: &Path) -> &Path {
    dir.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(dir)
}
