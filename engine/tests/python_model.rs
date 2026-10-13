//! python::model tests (M5 T2): run_model against the fake interpreter —
//! argv/cwd/env contract, success, stderr/stdout failure text, output
//! materialization, interruption.

use std::path::{Path, PathBuf};
use std::time::Duration;

use engine::python::model::{self, BuildEvent, BuildOutcome, failure_text, last_line};

fn fake_python() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake-agent/fake_python.py")
}

/// The M0 build123d-generated outputs the fake interpreter copies in copy
/// mode — the same files a real model.py run must materialize.
fn golden_fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
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

/// Runs one build to its Finished event (8 s bound). `cancel_after` waits
/// that long for a first event, then cancels from this task — the handle
/// cannot be shared with a spawned canceller (its receiver is not Clone),
/// and in hang mode nothing arrives to lose. Returns the events plus the
/// argv of every invocation the dump recorded.
async fn drive(
    root: &Path,
    cancel_after: Option<Duration>,
    fixtures: Option<&Path>,
) -> (Vec<BuildEvent>, Vec<Vec<String>>) {
    let dump = root.join("argv-dump.jsonl");
    let _ = std::fs::remove_file(&dump); // per-run dump: read only this invocation
    // SAFETY: setters hold ENV_LOCK; every drive call is inside the lock.
    unsafe {
        std::env::set_var("CAD_FAKE_PY_ARGV_OUT", &dump);
        match fixtures {
            Some(dir) => std::env::set_var("CAD_FAKE_PY_FIXTURES", dir),
            None => std::env::remove_var("CAD_FAKE_PY_FIXTURES"),
        }
    }
    let mut handle = model::run_model(&fake_python(), root).unwrap();
    let mut events = Vec::new();
    if let Some(delay) = cancel_after {
        if let Ok(Some(event)) = tokio::time::timeout(delay, handle.events.recv()).await {
            events.push(event); // a line that beat the cancel stays in order
        }
        handle.cancel();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, handle.events.recv()).await {
        let done = matches!(event, BuildEvent::Finished(_));
        events.push(event);
        if done {
            break;
        }
    }
    let argv = std::fs::read_to_string(&dump).unwrap_or_default();
    let recorded: Vec<Vec<String>> = argv
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        })
        .collect();
    (events, recorded)
}

#[tokio::test]
async fn success_runs_model_py_in_project_root_with_tool_env() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmp_project();
    // SAFETY: holds ENV_LOCK.
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "verify") };
    let (events, recorded) = drive(&root, None, None).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE") };
    assert_eq!(recorded, vec![vec!["model.py".to_string()]]);
    let dump = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(root.join("argv-dump.jsonl")).unwrap(),
    )
    .unwrap();
    // python's getcwd resolves symlinks (/var → /private/var on macOS), so
    // compare canonicalized paths
    let canonical_root = std::fs::canonicalize(&root).unwrap();
    assert_eq!(
        Path::new(dump["cwd"].as_str().unwrap()),
        canonical_root,
        "cwd is the project root"
    );
    assert_eq!(dump["env"]["MPLBACKEND"].as_str(), Some("Agg"));
    assert_eq!(dump["env"]["PYTHONUNBUFFERED"].as_str(), Some("1"));
    assert!(matches!(
        events.last(),
        Some(BuildEvent::Finished(BuildOutcome::Success))
    ));
}

#[tokio::test]
async fn stderr_failure_carries_last_line_and_full_text() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmp_project();
    // SAFETY: holds ENV_LOCK.
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "fail") };
    let (events, _) = drive(&root, None, None).await;
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
    assert_eq!(
        failure_text("", "line one\nboom: no stderr at all\n"),
        "line one\nboom: no stderr at all"
    );
    assert_eq!(failure_text("real stderr\n", "noise\n"), "real stderr");
    assert_eq!(last_line("a\nb\n"), Some("b".to_string()));
    assert_eq!(last_line(""), None);
    let _guard = ENV_LOCK.lock().await;
    let root = tmp_project();
    // SAFETY: holds ENV_LOCK.
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "fail_stdout") };
    let (events, _) = drive(&root, None, None).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE") };
    match events.last() {
        Some(BuildEvent::Finished(BuildOutcome::Failed { last_line, .. })) => {
            assert_eq!(last_line.as_deref(), Some("boom: no stderr at all"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn copy_mode_materializes_the_expected_outputs() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmp_project();
    // SAFETY: holds ENV_LOCK.
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "copy") };
    let (events, _) = drive(&root, None, Some(&golden_fixtures())).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE") };
    assert!(
        matches!(
            events.last(),
            Some(BuildEvent::Finished(BuildOutcome::Success))
        ),
        "{events:?}"
    );
    for name in [
        "model.3mf",
        "model.stl",
        "model.step",
        "schematic.svg",
        "schematic.png",
        "schematic.pdf",
        "manifest.json",
    ] {
        let path = root.join("output").join(name);
        let size = std::fs::metadata(&path)
            .map(|m| m.len())
            .unwrap_or_else(|e| panic!("{name} not materialized: {e}"));
        assert!(size > 0, "{name} copied non-empty");
    }
}

#[tokio::test]
async fn cancel_reports_interrupted() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmp_project();
    // SAFETY: holds ENV_LOCK.
    unsafe { std::env::set_var("CAD_FAKE_PY_MODE", "hang") };
    let (events, _) = drive(&root, Some(Duration::from_millis(300)), None).await;
    unsafe { std::env::remove_var("CAD_FAKE_PY_MODE") };
    assert!(matches!(
        events.last(),
        Some(BuildEvent::Finished(BuildOutcome::Interrupted))
    ));
}
