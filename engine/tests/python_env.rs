//! python::env tests (M5 T0): the install planner, version parsing, the
//! verification contract, progress nudging, the shared versions cell, and
//! the check/install drivers against an injected fake spawn.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use engine::python::env::{
    self, EnvEvent, EnvFailure, EnvLog, EnvStep, InstallError, PACKAGES, VERIFY_SCRIPT,
    VERSIONS_FALLBACK, is_supported_python, nudge_progress, parse_python_version,
    parse_verification, set_shared_versions, uv_install_plan, venv_install_plan, versions_display,
};

/// The versions cell is process-global: every test that resets or asserts
/// it — including the drivers, which set it on success — serializes here so
/// the reset test can never race a concurrent check/install.
static CELL_LOCK: Mutex<()> = Mutex::new(());

fn venv() -> PathBuf {
    std::env::temp_dir().join(format!("cad-env-{}", uuid::Uuid::new_v4()))
}

#[test]
fn uv_plan_matches_upstream_argv() {
    let dir = venv();
    let steps = uv_install_plan(Path::new("/usr/local/bin/uv"), &dir);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].progress, 0.1);
    assert_eq!(steps[0].step, EnvStep::CreatingVenvWithUv);
    assert_eq!(
        steps[0].argv,
        vec![
            "/usr/local/bin/uv",
            "venv",
            "--python",
            "3.12",
            dir.to_str().unwrap()
        ]
    );
    assert_eq!(steps[1].progress, 0.25);
    assert_eq!(steps[1].step, EnvStep::InstallingPackages);
    assert_eq!(
        steps[1].argv[1..5],
        [
            "pip",
            "install",
            "--python",
            dir.join("bin/python").to_str().unwrap()
        ]
    );
    assert_eq!(&steps[1].argv[5..8], &PACKAGES[..]);
}

#[test]
fn venv_plan_matches_upstream_argv() {
    let dir = venv();
    let steps = venv_install_plan(Path::new("/usr/bin/python3.12"), &dir);
    assert_eq!(steps.len(), 3);
    assert_eq!(
        (steps[0].progress, steps[0].step),
        (0.1, EnvStep::CreatingVenv)
    );
    assert_eq!(
        steps[0].argv,
        vec!["/usr/bin/python3.12", "-m", "venv", dir.to_str().unwrap()]
    );
    assert_eq!(
        (steps[1].progress, steps[1].step),
        (0.2, EnvStep::UpgradingPip)
    );
    assert_eq!(
        steps[1].argv[1..],
        ["-m", "pip", "install", "--upgrade", "pip"]
    );
    assert_eq!(
        (steps[2].progress, steps[2].step),
        (0.25, EnvStep::InstallingPackages)
    );
    assert_eq!(&steps[2].argv[4..7], &PACKAGES[..]);
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
    assert_eq!(
        env::python_candidates(),
        [
            "python3.13",
            "python3.12",
            "python3.11",
            "python3.10",
            "python3"
        ]
    );
}

#[test]
fn verification_output_parses_into_versions_line() {
    assert_eq!(VERIFY_SCRIPT.lines().count(), 3);
    assert!(VERIFY_SCRIPT.contains("import sys, build123d, matplotlib, numpy"));
    let two =
        parse_verification("0.8.1\nPython 3.12.4, build123d 0.8.1, matplotlib 3.9.2, numpy 2.1.0");
    assert_eq!(
        two.as_ref().map(|(v, l)| (v.as_str(), l.as_str())),
        Some((
            "0.8.1",
            "Python 3.12.4, build123d 0.8.1, matplotlib 3.9.2, numpy 2.1.0"
        ))
    );
    let one = parse_verification("0.8.1");
    assert_eq!(
        one.as_ref().map(|(v, l)| (v.as_str(), l.as_str())),
        Some(("0.8.1", ""))
    );
    // each line is trimmed before line 1/line 2 are taken; a padded or blank
    // line between them shifts nothing
    let padded = parse_verification("  0.8.1  \n\n  Python 3.12.4, build123d 0.8.1  \n");
    assert_eq!(
        padded.as_ref().map(|(v, l)| (v.as_str(), l.as_str())),
        Some(("0.8.1", "Python 3.12.4, build123d 0.8.1"))
    );
    assert_eq!(parse_verification(""), None);
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

/// Writes a shell script and returns a Spawn that runs it regardless of
/// argv, recording every argv it was asked to run.
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
        if done {
            break;
        }
    }
    events
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn check_missing_interpreter_reports_missing() {
    let dir = venv(); // never gets a bin/python
    let (spawn, _) = scripted_spawn(&dir, "exit 0\n");
    let events = drain(env::check(&dir, &spawn)).await;
    // Missing is its own failure kind (upstream .missing gates the setup
    // sheet's presentation); upstream has no display string for it.
    assert_eq!(
        events.last(),
        Some(&EnvEvent::Finished(Err(EnvFailure::Missing)))
    );
}

#[tokio::test]
async fn check_success_sets_shared_versions() {
    let _cell = CELL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = venv();
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let bin = dir.join("bin/python");
    std::fs::write(
        &bin,
        "#!/bin/sh\nprintf '0.8.1\\nPython 3.12.4, build123d 0.8.1\\n'\n",
    )
    .unwrap();
    make_executable(&bin);
    let (_spawn, seen) = scripted_spawn(&dir, "exit 0\n");
    // check must run the interpreter itself: point spawn at the real bin
    let spawn: env::Spawn = Arc::new(move |argv, cwd| {
        seen.lock().unwrap().push(argv.to_vec());
        engine::processes::spawn_streaming(argv, cwd, &shell_env_vec())
    });
    set_shared_versions(None);
    let events = drain(env::check(&dir, &spawn)).await;
    assert!(matches!(events.last(), Some(EnvEvent::Finished(Ok(_)))));
    assert_eq!(versions_display(), "Python 3.12.4, build123d 0.8.1");
    set_shared_versions(None);
}

#[tokio::test]
async fn check_failing_verification_reports_check_failed() {
    let _cell = CELL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = venv();
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let bin = dir.join("bin/python");
    std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
    make_executable(&bin);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let spawn: env::Spawn = Arc::new(move |argv, cwd| {
        seen.lock().unwrap().push(argv.to_vec());
        engine::processes::spawn_streaming(argv, cwd, &shell_env_vec())
    });
    set_shared_versions(Some("stale versions line".into()));
    let events = drain(env::check(&dir, &spawn)).await;
    match events.last() {
        Some(EnvEvent::Finished(Err(EnvFailure::CheckFailed))) => {}
        other => panic!("expected check failure, got {other:?}"),
    }
    // a failed check clears the stale cell, so display falls back
    assert_eq!(versions_display(), VERSIONS_FALLBACK);
    set_shared_versions(None);
}

#[tokio::test]
async fn install_uv_path_reaches_verified_ready() {
    let _cell = CELL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = venv();
    let bin = dir.join("bin/python");
    let (spawn, seen) = scripted_spawn(
        &dir,
        &format!(
            "mkdir -p '{0}'\nprintf '#!/bin/sh\\nprintf \"0.8.1\\\\nPython 3.12.4, build123d 0.8.1\\\\n\"\\n' > '{1}'\nchmod +x '{1}'\nexit 0\n",
            dir.join("bin").display(),
            bin.display()
        ),
    );
    // force the uv branch: probe finds uv
    let probe: env::Probe =
        Arc::new(|program, _| (program.file_name().unwrap() == "uv").then(|| (0, "uv 0.5".into())));
    let events = drain(env::install(&dir, &spawn, &probe)).await;
    assert!(matches!(events.last(), Some(EnvEvent::Finished(Ok(_)))));
    let milestones: Vec<(f64, EnvStep)> = events
        .iter()
        .filter_map(|e| match e {
            EnvEvent::Progress { value, step } => Some((*value, *step)),
            _ => None,
        })
        .collect();
    assert_eq!(
        milestones.first().unwrap(),
        &(0.1, EnvStep::CreatingVenvWithUv)
    );
    assert!(milestones.contains(&(0.95, EnvStep::Verifying)));
    assert_eq!(milestones.last().unwrap(), &(1.0, EnvStep::Installed));
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|argv| argv.windows(3).any(|w| w == ["venv", "--python", "3.12"]))
    );
    set_shared_versions(None);
}

#[tokio::test]
async fn install_failing_step_reports_command_name() {
    let dir = venv();
    let (spawn, _) = scripted_spawn(&dir, "echo oops >&2\nexit 1\n");
    let probe: env::Probe = Arc::new(|_, _| Some((0, "uv 0.5".to_string())));
    let events = drain(env::install(&dir, &spawn, &probe)).await;
    match events.last() {
        Some(EnvEvent::Finished(Err(EnvFailure::InstallFailed(error)))) => {
            assert_eq!(
                error,
                &InstallError::CommandFailed {
                    command: "uv".into()
                }
            );
        }
        other => panic!("expected command failure, got {other:?}"),
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EnvEvent::Log(EnvLog::Text(l)) if l.contains("oops")))
    );
    // upstream's catch sets the step label with the bar holding its place
    assert!(events.iter().any(|e| matches!(
        e,
        EnvEvent::Progress {
            value: 0.1,
            step: EnvStep::InstallFailed
        }
    )));
    // the error line closes the log as an id, after the step label
    assert!(events.iter().any(|e| matches!(e,
        EnvEvent::Log(EnvLog::Failure(InstallError::CommandFailed { command })) if command == "uv")));
}

#[tokio::test]
async fn install_without_uv_or_python_reports_python_not_found() {
    let dir = venv();
    let (spawn, _) = scripted_spawn(&dir, "exit 0\n");
    let probe: env::Probe = Arc::new(|_, _| None); // uv absent, every python absent
    let events = drain(env::install(&dir, &spawn, &probe)).await;
    match events.last() {
        Some(EnvEvent::Finished(Err(EnvFailure::InstallFailed(error)))) => {
            assert_eq!(error, &InstallError::PythonNotFound)
        }
        other => panic!("expected pythonNotFound, got {other:?}"),
    }
    // the search milestone (0.05) precedes the scan, so the failure label
    // carries that position
    assert!(events.iter().any(|e| matches!(
        e,
        EnvEvent::Progress {
            value: 0.05,
            step: EnvStep::InstallFailed
        }
    )));
}

#[tokio::test]
async fn install_rejects_candidates_whose_probe_fails() {
    let dir = venv();
    let (spawn, _) = scripted_spawn(&dir, "exit 0\n");
    // every probe answers, but never with exit 0: uv is rejected and so is
    // any located python — acceptance is exit 0 AND parses AND 3.10–3.13
    let probe: env::Probe = Arc::new(|_, _| Some((1, "Python 3.12.4".to_string())));
    let events = drain(env::install(&dir, &spawn, &probe)).await;
    match events.last() {
        Some(EnvEvent::Finished(Err(EnvFailure::InstallFailed(error)))) => {
            assert_eq!(error, &InstallError::PythonNotFound)
        }
        other => panic!("expected pythonNotFound, got {other:?}"),
    }
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, EnvEvent::Log(EnvLog::FoundPython { .. })))
    );
}
