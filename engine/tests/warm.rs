//! python::warm tests: the fork-server protocol over a bare system python3
//! (no build123d — the server's module list is argv, so the fork/pipe
//! machinery is testable without the venv).

use std::path::PathBuf;
use std::time::Duration;

use engine::python::model::BuildEvent;
use engine::python::warm::WarmBuilder;

/// The tests register build children in the process registry, whose path is
/// a process-global env override: serialize like python_model's ENV_LOCK.
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn system_python() -> PathBuf {
    PathBuf::from("python3")
}

/// A temp project whose model.py is `script`.
fn tmp_project(tag: &str, script: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cad-warm-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("model.py"), script).unwrap();
    root
}

/// A warm server over the system python with no warm modules, plus the
/// registry override scoped to the test's lifetime.
async fn server(tag: &str) -> (WarmBuilder, PathBuf) {
    let _env = std::env::var_os("CAD_STUDIO_RUNNING_PROCESSES");
    // SAFETY: every server() call is inside ENV_LOCK.
    unsafe {
        std::env::set_var(
            "CAD_STUDIO_RUNNING_PROCESSES",
            std::env::temp_dir().join(format!("cad-warm-registry-{tag}.jsonl")),
        );
    }
    let dir = std::env::temp_dir().join(format!("cad-warm-server-{tag}-{}", uuid::Uuid::new_v4()));
    let builder = WarmBuilder::start_with_modules(&system_python(), &dir, &[])
        .await
        .expect("system python starts the server");
    (builder, dir)
}

/// Drain a build to its Finished event (10 s bound).
async fn finish(handle: &mut engine::python::model::BuildHandle) -> Vec<BuildEvent> {
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, handle.events.recv()).await {
        let done = matches!(event, BuildEvent::Finished(_));
        events.push(event);
        if done {
            break;
        }
    }
    events
}

#[tokio::test]
async fn a_successful_build_streams_both_streams_then_success() {
    let _lock = ENV_LOCK.lock().await;
    let (builder, _dir) = server("ok").await;
    let root = tmp_project(
        "ok",
        "import sys\nprint('to stdout')\nprint('to stderr', file=sys.stderr)\n",
    );
    let mut handle = builder.run(&root).expect("warm run starts");
    let events = finish(&mut handle).await;
    assert!(events.contains(&BuildEvent::Line("to stdout".into())));
    assert!(events.contains(&BuildEvent::Line("to stderr".into())));
    assert!(matches!(
        events.last(),
        Some(BuildEvent::Finished(engine::python::model::BuildOutcome::Success))
    ));
}

#[tokio::test]
async fn a_failing_build_carries_the_stderr_text_and_its_last_line() {
    let _lock = ENV_LOCK.lock().await;
    let (builder, _) = server("fail").await;
    let root = tmp_project(
        "fail",
        "import sys\nprint('noise')\nprint('boom one', file=sys.stderr)\nprint('boom last', file=sys.stderr)\nsys.exit(1)\n",
    );
    let mut handle = builder.run(&root).expect("warm run starts");
    let events = finish(&mut handle).await;
    let Some(BuildEvent::Finished(engine::python::model::BuildOutcome::Failed {
        last_line,
        output,
    })) = events.last()
    else {
        panic!("expected a failed outcome, got {:?}", events.last());
    };
    assert_eq!(last_line.as_deref(), Some("boom last"));
    assert!(output.contains("boom one"), "output keeps the whole text: {output}");
}

#[tokio::test]
async fn consecutive_builds_from_one_server_are_clean() {
    let _lock = ENV_LOCK.lock().await;
    let (builder, _) = server("twice").await;
    for expected in (1..=2).map(|n| format!("run {n}")) {
        let root = tmp_project("twice", &format!("print('{expected}')\n"));
        let mut handle = builder.run(&root).expect("warm run starts");
        let events = finish(&mut handle).await;
        assert!(events.contains(&BuildEvent::Line(expected)));
        assert!(matches!(
            events.last(),
            Some(BuildEvent::Finished(engine::python::model::BuildOutcome::Success))
        ));
    }
}

#[tokio::test]
async fn cancelling_interrupts_the_fork_child() {
    let _lock = ENV_LOCK.lock().await;
    let (builder, _) = server("cancel").await;
    let root = tmp_project("cancel", "import time\nprint('working')\ntime.sleep(30)\n");
    let mut handle = builder.run(&root).expect("warm run starts");
    // give the child a moment to be inside the sleep, then cancel
    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.cancel();
    let events = finish(&mut handle).await;
    assert!(matches!(
        events.last(),
        Some(BuildEvent::Finished(engine::python::model::BuildOutcome::Interrupted))
    ));
}

#[tokio::test]
async fn a_dead_server_fails_the_run_not_the_caller() {
    let _lock = ENV_LOCK.lock().await;
    let (builder, _) = server("dead").await;
    assert!(builder.alive());
    engine::processes::terminate_pid(builder.pid());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!builder.alive(), "server is gone once signalled");
    let root = tmp_project("dead", "print('never')\n");
    let error = match builder.run(&root) {
        Ok(_) => panic!("a dead server cannot run"),
        Err(error) => error,
    };
    assert!(!error.to_string().is_empty());
}

#[tokio::test]
async fn dropping_the_builder_stops_the_server_process() {
    let _lock = ENV_LOCK.lock().await;
    let (builder, dir) = server("drop").await;
    assert!(builder.alive());
    drop(builder);
    assert!(!std::path::Path::new(&dir).join("warm.sock").exists());
}

/// The agent's fast path: the client script streams a warm run's output
/// and exits with its code, and falls back to a plain run (same output,
/// same exit) when no server is live.
#[tokio::test]
async fn the_client_runs_warm_and_falls_back_cold() {
    use std::process::Command;

    let _lock = ENV_LOCK.lock().await;
    let (builder, dir) = server("client").await;
    let client = dir.join("cad_warm_model.py");
    let warm = tmp_project(
        "client",
        "import sys\nprint('via stdout')\nprint('via stderr', file=sys.stderr)\n",
    );

    let output = Command::new("python3")
        .arg(&client)
        .current_dir(&warm)
        .env("CAD_STUDIO_WARM_DIR", &dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "warm run exits 0");
    assert!(String::from_utf8_lossy(&output.stdout).contains("via stdout"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("via stderr"));

    // cold fallback: a dead socket dir must behave like a plain run
    let cold = tmp_project("client-cold", "print('plain run')\n");
    let output = Command::new("python3")
        .arg(&client)
        .current_dir(&cold)
        .env("CAD_STUDIO_WARM_DIR", dir.join("nowhere"))
        .output()
        .unwrap();
    assert!(output.status.success(), "fallback exits 0");
    assert!(String::from_utf8_lossy(&output.stdout).contains("plain run"));

    // failure propagation through the warm path
    let failing = tmp_project("client-fail", "import sys\nsys.exit(3)\n");
    let output = Command::new("python3")
        .arg(&client)
        .current_dir(&failing)
        .env("CAD_STUDIO_WARM_DIR", &dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "the child's exit code travels");
    drop(builder);
}
