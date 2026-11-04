//! build pipeline tests: parameter rebuild end-to-end with the
//! fake interpreter, refusal guards, failure rows, and the version-record
//! gates split by path (turn vs build). No network; the interpreter is
//! the fake_python.py wrapper, the agent (one test) is fake_claude.py.

use app::activity::Activity;
use app::bridge::{Bridge, BuildRefusal, Message};
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::channel::mpsc::UnboundedReceiver;
use engine::agent::session::EngineEvent;
use engine::project::store::{Role, load_chat, load_metadata};
use engine::settings::Settings;
use std::path::PathBuf;
use std::time::Duration;

/// The fakes read their env at spawn time and parallel #[tokio::test]s
/// share one process: every setter/clearer of a CAD_FAKE_* or
/// CAD_TEST_CLAUDE var holds this lock for the whole test (the
/// house pattern — engine/tests/session_lifecycle.rs, bridge_e2e.rs).
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn fake_python() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake-agent/fake_python.py")
}
fn fake_agent() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake-agent/fake_claude.py")
}
fn fixture_fake(p: &str) -> PathBuf {
    fake_agent().parent().unwrap().join(p)
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

/// A directory-mode scenario: run n replays `sources[n-1]`
/// (bridge_e2e.rs's scenario_dir, inlined for this suite's binary).
fn scenario_dir(sources: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cad-pipe-scenario-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    for (n, source) in sources.iter().enumerate() {
        std::fs::copy(fixture_fake(source), dir.join(format!("{}.jsonl", n + 1))).unwrap();
    }
    dir
}

/// The tool-use scenario parked mid-tool (bridge_e2e.rs's
/// scenario_dir_mid_tool): the fake sleeps 5 s after the tool rows open,
/// before any tool result arrives — a deterministic window a busy-guard
/// assert can land inside.
fn parked_tool_use() -> String {
    let original = std::fs::read_to_string(fixture_fake("scenarios/tool-use.jsonl")).unwrap();
    let mut parked = String::new();
    let mut slept = false;
    for line in original.lines() {
        if !slept && line.starts_with("{\"type\":\"user\"") {
            parked.push_str("#sleep 5000\n");
            slept = true;
        }
        parked.push_str(line);
        parked.push('\n');
    }
    assert!(
        slept,
        "the tool-use scenario has a tool result line to park before"
    );
    parked
}

/// Drains bridge messages until `pred`, bounded (5 s) — never a fixed
/// sleep; bridge_e2e.rs's `recv` loop inlined as a predicate.
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
    app::localize::select("en");
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
    // setParameter guards isBusy ONLY (Project.swift:343-362) —
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
    // SAFETY: agent-fake env holds this suite's ENV_LOCK (bridge_e2e's
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

/// The 1 s auto-run starts its turn
/// DRIVER-side — busy must cover a queue-started turn exactly like a
/// handle-issued send (upstream sendNextQueued → send sets
/// activity = .working, Project.swift:302-306 → 203).
#[tokio::test]
async fn set_parameter_refused_busy_during_auto_run_turn() {
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    let scenario = scenario_dir(&["scenarios/pong.jsonl"]);
    std::fs::write(scenario.join("2.jsonl"), parked_tool_use()).unwrap();
    // SAFETY: agent-fake env holds this suite's ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fake_agent());
        std::env::set_var("CAD_FAKE_SCENARIO", scenario);
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
    }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.enqueue("first".into(), Vec::new()).unwrap();
    bridge.enqueue("second".into(), Vec::new()).unwrap(); // queues → auto-runs
    // the second TurnStarted is the AUTO-RUN path's start
    run_until(
        &mut messages,
        |m| matches!(m, Message::TurnStarted { prompt, .. } if prompt == "second"),
    )
    .await;
    // a tool row open in the parked turn: the turn is indisputably running
    run_until(&mut messages, |m| {
        matches!(m, Message::Event(EngineEvent::ToolStarted { .. }))
    })
    .await;
    assert_eq!(bridge.set_parameter("wall", 3.0), Err(BuildRefusal::Busy));
    bridge.cancel(); // end the parked turn within the test's bounds
    run_until(&mut messages, |m| {
        matches!(m, Message::Event(EngineEvent::TurnFinished(_)))
    })
    .await;
    unsafe {
        std::env::remove_var("CAD_FAKE_SCENARIO");
        std::env::remove_var("CAD_TEST_CLAUDE");
        std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
    }
}

/// The resume path (`resume_queue`,
/// upstream sendQueuedNow → sendNextQueued → send) raises busy the same
/// way — a build issued during a resumed turn refuses.
#[tokio::test]
async fn start_build_refused_busy_during_resumed_turn() {
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    // run 1 fails so the queue pauses; the resume starts run 2 parked
    let scenario = scenario_dir(&["scenarios/exit-code-failure.jsonl"]);
    std::fs::write(scenario.join("2.jsonl"), parked_tool_use()).unwrap();
    // SAFETY: agent-fake env holds this suite's ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fake_agent());
        std::env::set_var("CAD_FAKE_SCENARIO", scenario);
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
    }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.enqueue("first".into(), Vec::new()).unwrap();
    bridge.enqueue("second".into(), Vec::new()).unwrap();
    // the failed first turn pauses the queue — second waits for the resume
    run_until(&mut messages, |m| {
        matches!(m, Message::Event(EngineEvent::TurnFinished(_)))
    })
    .await;
    bridge.resume_queue();
    run_until(
        &mut messages,
        |m| matches!(m, Message::TurnStarted { prompt, .. } if prompt == "second"),
    )
    .await;
    run_until(&mut messages, |m| {
        matches!(m, Message::Event(EngineEvent::ToolStarted { .. }))
    })
    .await;
    assert_eq!(bridge.start_build(None), Err(BuildRefusal::Busy));
    bridge.cancel();
    run_until(&mut messages, |m| {
        matches!(m, Message::Event(EngineEvent::TurnFinished(_)))
    })
    .await;
    unsafe {
        std::env::remove_var("CAD_FAKE_SCENARIO");
        std::env::remove_var("CAD_TEST_CLAUDE");
        std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
    }
}

/// Upstream's `try? String(contentsOf:)`
/// guard (Project.swift:344) — ANY read failure of model.py returns
/// silently, before anything happens: no rewrite, no failed activity.
#[tokio::test]
async fn set_parameter_with_unreadable_script_returns_silently() {
    let root = project();
    std::fs::remove_file(root.join("model.py")).unwrap();
    let (mut bridge, _messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    assert_eq!(bridge.set_parameter("wall", 3.0), Ok(()));
    assert_eq!(bridge.activity(), Activity::Idle);
}

/// Restore (Project.swift:308-321): the snapshot's files come back
/// (model.py, chat.json), metadata points at the restored version with the
/// snapshot's session id, and the compare picker's parameters parse from a
/// version's stored model.py copy.
#[tokio::test]
async fn restore_returns_project_to_the_snapshot() {
    let _guard = ENV_LOCK.lock().await; // sets CAD_FAKE_PY_*
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
    let sid_v1 = load_metadata(&root).unwrap().session_id.clone();
    bridge.set_parameter("wall", 5.0).unwrap();
    run_until(&mut messages, |m| {
        matches!(m, Message::BuildFinished { .. })
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 5\nheight = 10\n"
    );
    bridge.restore(1).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 10\n"
    );
    let metadata = load_metadata(&root).unwrap();
    assert_eq!(metadata.current_version, Some(1));
    assert_eq!(metadata.session_id, sid_v1);
    // the chat transcript is the snapshot's (summary row of v1 is the last)
    let chat = load_chat(&root).unwrap();
    assert_eq!(chat.last().unwrap().detail.as_deref(), Some("1"));
    // compare: previous parameter values are parsed from version 2's stored
    // model.py copy (the slider rewrite landed there before the snapshot)
    bridge.compare_with(Some(2));
    assert_eq!(bridge.comparison_parameters().get("wall"), Some(&5.0));
    bridge.compare_with(None);
    assert!(bridge.comparison_parameters().is_empty());
    unsafe {
        std::env::remove_var("CAD_FAKE_PY_MODE");
        std::env::remove_var("CAD_FAKE_PY_FIXTURES");
    }
}

/// Restore refuses while a turn or build runs (upstream `guard !isBusy`).
#[tokio::test]
async fn restore_refused_while_busy() {
    let _guard = ENV_LOCK.lock().await;
    let root = project();
    // SAFETY: fake-python env holds this suite's ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_FAKE_PY_MODE", "hang");
    }
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test();
    bridge.start_build(None).unwrap();
    assert_eq!(bridge.restore(1), Err(BuildRefusal::Busy));
    bridge.cancel();
    run_until(&mut messages, |m| {
        matches!(m, Message::BuildFinished { .. })
    })
    .await;
    unsafe {
        std::env::remove_var("CAD_FAKE_PY_MODE");
    }
}
