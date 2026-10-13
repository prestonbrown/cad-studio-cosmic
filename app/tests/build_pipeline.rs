//! build pipeline tests (M5 T6): parameter rebuild end-to-end with the
//! fake interpreter, refusal guards, failure rows, and the version-record
//! gates split by path (turn vs build). No network; the interpreter is
//! the fake_python.py wrapper, the agent (one test) is fake_claude.py.

use app::bridge::{Bridge, BuildRefusal, Message};
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::channel::mpsc::UnboundedReceiver;
use engine::project::store::{Role, load_chat, load_metadata};
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
        root.parent().unwrap(),
        root.file_name().unwrap().to_str().unwrap(),
    )
    .unwrap();
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
    run_until(&mut messages, |m| {
        matches!(m, Message::BuildFinished { .. })
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 10\n"
    );
    assert!(root.join("output/model.3mf").exists());
    let metadata = load_metadata(&root).unwrap();
    assert_eq!(metadata.current_version, Some(1));
    let chat = load_chat(&root).unwrap();
    assert_eq!(chat.last().unwrap().role, Role::Summary);
    assert_eq!(chat.last().unwrap().text, "Version 1");
    assert_eq!(chat.last().unwrap().detail.as_deref(), Some("1"));
    // second commit: a NEW version only when model.py changed (it did)
    bridge.set_parameter("height", 12.0).unwrap();
    run_until(&mut messages, |m| {
        matches!(m, Message::BuildFinished { .. })
    })
    .await;
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(2));
    unsafe {
        std::env::remove_var("CAD_FAKE_PY_MODE");
        std::env::remove_var("CAD_FAKE_PY_FIXTURES");
    }
}

#[tokio::test]
async fn set_parameter_and_build_refused_while_busy() {
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    // SAFETY: fake-python env holds this suite's ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_FAKE_PY_MODE", "hang");
        std::env::set_var("CAD_FAKE_PY_FIXTURES", golden_fixtures());
    }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    assert_eq!(bridge.set_parameter("wall", 3.0), Err(BuildRefusal::Busy));
    assert_eq!(bridge.start_build(None), Err(BuildRefusal::Busy));
    bridge.cancel();
    run_until(&mut messages, |m| {
        matches!(m, Message::BuildFinished { .. })
    })
    .await;
    unsafe {
        std::env::remove_var("CAD_FAKE_PY_MODE");
        std::env::remove_var("CAD_FAKE_PY_FIXTURES");
    }
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
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 10\n"
    );
    assert_eq!(load_metadata(&root).unwrap().current_version, None); // no build ran
    assert!(bridge.sheet_requested_for_test());
}

#[tokio::test]
async fn build_failure_rows_ask_agent_to_fix_with_traceback() {
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    // SAFETY: fake-python env holds this suite's ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_FAKE_PY_MODE", "fail");
    }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    run_until(&mut messages, |m| {
        matches!(m, Message::BuildFinished { .. })
    })
    .await;
    assert_eq!(
        bridge.build_error().as_deref(),
        Some("ValueError: bad shape")
    );
    let chat = load_chat(&root).unwrap();
    let last = chat.last().unwrap();
    assert_eq!(last.role, Role::System);
    assert_eq!(
        last.action,
        Some(engine::project::store::Action::AskClaudeToFix)
    );
    assert!(last.detail.as_deref().unwrap().contains("Traceback"));
    unsafe {
        std::env::remove_var("CAD_FAKE_PY_MODE");
    }
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
        std::env::set_var(
            "CAD_FAKE_SCENARIO",
            fake_agent().parent().unwrap().join("scenarios/pong.jsonl"),
        );
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
    }
    let (bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.enqueue("a 20 mm cube".into(), Vec::new()).unwrap();
    // emitted after the tail's disk writes, so the asserts below are safe
    run_until(&mut messages, |m| {
        matches!(m, Message::TurnSuccessBookkeeping)
    })
    .await;
    assert_eq!(load_metadata(&root).unwrap().current_version, None);
    assert!(
        !load_chat(&root)
            .unwrap()
            .iter()
            .any(|m| m.role == Role::Summary)
    );
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
    // SAFETY: fake-python env holds this suite's ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_FAKE_PY_MODE", "verify");
    } // exits 0, writes nothing
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    run_until(&mut messages, |m| {
        matches!(m, Message::BuildFinished { .. })
    })
    .await;
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(1));
    assert_eq!(
        load_chat(&root).unwrap().last().unwrap().role,
        Role::Summary
    );
    unsafe {
        std::env::remove_var("CAD_FAKE_PY_MODE");
    }
}

#[tokio::test]
async fn unchanged_model_py_records_no_second_version() {
    // BLOCKER 3: has_changes is the build path's one guard — a second
    // successful build that did not touch model.py records nothing.
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    // SAFETY: fake-python env holds this suite's ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_FAKE_PY_MODE", "verify");
    }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    run_until(&mut messages, |m| {
        matches!(m, Message::BuildFinished { .. })
    })
    .await;
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(1));
    bridge.start_build(None).unwrap(); // same model.py bytes
    run_until(&mut messages, |m| {
        matches!(m, Message::BuildFinished { .. })
    })
    .await;
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(1)); // still 1
    assert_eq!(
        load_chat(&root)
            .unwrap()
            .iter()
            .filter(|m| m.role == Role::Summary)
            .count(),
        1
    );
    unsafe {
        std::env::remove_var("CAD_FAKE_PY_MODE");
    }
}

#[tokio::test]
async fn send_refused_without_ready_python_and_keeps_draft() {
    let root = project();
    let (bridge, _messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    // python_state Unknown: sending must refuse, keep the draft, request the sheet
    let refused = bridge
        .enqueue("make a cube".into(), Vec::new())
        .unwrap_err();
    assert_eq!(refused, BuildRefusal::PythonNotReady);
    assert_eq!(
        bridge.draft_text_for_test(),
        Some("make a cube".to_string())
    );
    assert!(bridge.sheet_requested_for_test());
}

#[test]
fn parameter_list_refreshes_from_manifest_in_model_py_order() {
    let root = project();
    std::fs::write(
        root.join("output/manifest.json"),
        r#"{"parameters": {"height": 10, "wall": 2}}"#,
    )
    .unwrap();
    let params = app::bridge::refresh_parameters(&root); // pure helper the Bridge calls
    assert_eq!(
        params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["wall", "height"]
    );
}
