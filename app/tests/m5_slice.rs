//! The M5 wrap slice: the milestone end to end, headless — T8's startup
//! path (a REAL `python::env::check` over a scripted venv, every event
//! folded into the sheet state, the Ready terminal mirrored into the
//! bridge's readiness guards), one fake-agent turn landing the golden
//! outputs through the engine watcher and recording version 1 (the
//! agent-turn outputs gate), a slider commit rewriting model.py and
//! rebuilding through the fake interpreter (the watcher-driven reload
//! rebuilding the scene) for version 2, a second commit recording version
//! 3, `restore(2)` returning model.py/chat/output to the first slider's
//! snapshot, and the compare picker parsing version 3's parameters from
//! its stored model.py copy.
//!
//! Asserted against the bridge and the pure models (disk + sheet
//! reducer); the window folds every message exactly as the app layer maps
//! them, minus the iced runtime (the M4 vertical slice's harness). One
//! test holds the whole story — the CAD_* env is per-spawn, so the phases
//! share one ENV_LOCK window.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use app::bridge::{Bridge, Message as BridgeMessage};
use app::sheets::SetupSheetState;
use app::windows::project::{Message, ProjectWindow};
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::channel::mpsc::UnboundedReceiver;
use engine::agent::session::EngineEvent;
use engine::project::store::{self, Role, load_chat, load_metadata};
use engine::python::env::{self, EnvEvent, EnvState};
use engine::settings::Settings;
use engine::watch::{ReloadKind, watch_directory};

/// The fakes read their env at spawn time and parallel #[tokio::test]s
/// share one process: every setter/clearer of a CAD_FAKE_* or
/// CAD_TEST_CLAUDE var holds this lock for the whole test (house pattern
/// — build_pipeline.rs, vertical_slice.rs).
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn fake_python() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake-agent/fake_python.py")
}

fn fixture_fake(p: &str) -> PathBuf {
    fake_python().parent().unwrap().join(p)
}

fn fixture_golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
}

/// A venv whose `bin/python` verifies: the two VERIFY_SCRIPT lines.
fn scripted_venv() -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("cad-m5-venv-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let bin = dir.join("bin/python");
    std::fs::write(
        &bin,
        "#!/bin/sh\nprintf '0.8.1\\nPython 3.12.4, build123d 0.8.1, matplotlib 3.9.2, numpy 2.1.0\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn shell_env_vec() -> Vec<(String, String)> {
    engine::agent::locator::shell_env().into_iter().collect()
}

/// One message, bounded — never a fixed sleep.
async fn recv(messages: &mut UnboundedReceiver<BridgeMessage>, what: &str) -> BridgeMessage {
    match tokio::time::timeout(Duration::from_secs(5), messages.next()).await {
        Ok(Some(message)) => message,
        Ok(None) => panic!("message stream ended while waiting for {what}"),
        Err(_) => panic!("timed out waiting for {what}"),
    }
}

/// Drain until `pred`, folding every message into the window as it
/// arrives (build_pipeline.rs's `run_until`, plus the slice's folding).
async fn run_until(
    window: &mut ProjectWindow,
    messages: &mut UnboundedReceiver<BridgeMessage>,
    pred: impl Fn(&BridgeMessage) -> bool,
    what: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let message = match tokio::time::timeout_at(deadline, messages.next()).await {
            Ok(Some(message)) => message,
            Ok(None) => panic!("message stream ended while waiting for {what}"),
            Err(_) => panic!("timed out waiting for {what}"),
        };
        let hit = pred(&message);
        let _ = window.update(Message::Bridge(message));
        if hit {
            return;
        }
    }
}

/// Drain the check driver's events into the sheet state, bounded.
async fn run_check(venv: &Path) -> SetupSheetState {
    let spawn: env::Spawn = Arc::new(|argv: &[String], cwd: &Path| {
        engine::processes::spawn_streaming(argv, cwd, &shell_env_vec())
    });
    let mut sheet = SetupSheetState::new();
    sheet.begin_checking();
    assert_eq!(sheet.state, EnvState::Checking);
    let mut handle = env::check(venv, &spawn);
    loop {
        let event = match tokio::time::timeout(Duration::from_secs(5), handle.events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => panic!("check stream ended without a terminal"),
            Err(_) => panic!("timed out waiting for the check terminal"),
        };
        let terminal = matches!(event, EnvEvent::Finished(_));
        sheet.apply(&event);
        if terminal {
            return sheet;
        }
    }
}

/// Collect the watcher's next burst: a quiet half-second after the last
/// emission, or the wanted kinds all seen, or the 5 s deadline ends it.
fn collect_reloads(
    reloads: &std::sync::mpsc::Receiver<ReloadKind>,
    wanted: &[ReloadKind],
) -> Vec<ReloadKind> {
    let mut kinds = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match reloads.recv_timeout(Duration::from_millis(500)) {
            Ok(kind) => kinds.push(kind),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let done = wanted.iter().all(|kind| kinds.contains(kind));
                if done || std::time::Instant::now() >= deadline {
                    break;
                }
            }
        }
    }
    kinds
}

/// Forward one reload of each kind to the window, as its two relays would
/// (the viewer pane's watch feeds Message::Viewer, the shell's feeds
/// Message::OutputReload — vertical_slice.rs's mapping).
fn forward_reloads(window: &mut ProjectWindow, kinds: &[ReloadKind]) {
    let mut seen = Vec::new();
    for kind in kinds {
        if seen.contains(kind) {
            continue;
        }
        seen.push(*kind);
        let _ = window.update(Message::Viewer(app::viewer_pane::Message::Reload(*kind)));
        let _ = window.update(Message::OutputReload(*kind));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn startup_check_through_slider_to_restore() {
    let guard = ENV_LOCK.lock().await;

    // ── the project, with a slider-compatible model.py (the fake agent's
    // #copy-outputs writes model.py only when absent, so this one survives
    // the turn — the script the agent "wrote", single assignments)
    let parent = std::env::temp_dir().join(format!("cad-m5-slice-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&parent).unwrap();
    let root = store::create_project(&parent, "M5Slice").expect("create_project scaffolds");
    std::fs::write(root.join("model.py"), "wall = 2\nheight = 10\n").unwrap();

    // the platform watch over output/ — the same engine call the window's
    // two watchers make
    let (_watch, reloads) = watch_directory(&root.join("output"), Duration::from_millis(300))
        .expect("output/ exists, the watch registers");

    // SAFETY: fake env holds this suite's ENV_LOCK for the whole test.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var("CAD_FAKE_SCENARIO", fixture_fake("scenarios/slice.jsonl"));
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::set_var("CAD_FAKE_ARGV_OUT", root.join("argv-dump.jsonl"));
        std::env::set_var("CAD_FAKE_COPY_FROM", fixture_golden());
        std::env::set_var("CAD_FAKE_PY_MODE", "copy");
        std::env::set_var("CAD_FAKE_PY_FIXTURES", fixture_golden());
    }

    // ── T8's startup path through its real surfaces: the check verifies
    // the scripted venv, the sheet state reduces its terminal, and the
    // mirror flips the bridge's readiness guards (what CadStudio::init +
    // update_python_sheet do, minus the iced runtime)
    let sheet = run_check(&scripted_venv()).await;
    assert_eq!(
        sheet.state,
        EnvState::Ready {
            build123d_version: "0.8.1".into()
        }
    );

    let settings = Settings::default();
    let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &settings);
    let _ = window.view(); // the empty-project frame builds
    let (mut bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(settings, root.clone());
    assert!(
        !bridge.python_ready(),
        "the bridge starts Unknown, guards closed"
    );
    bridge.set_python_state(sheet.state.clone());
    assert!(
        bridge.python_ready(),
        "the sheet's Ready terminal unblocks the guards"
    );

    // ── the fake agent's first turn: outputs land, the watcher sees them,
    // the scene rebuilds (the M4 slice's ground)
    bridge
        .enqueue("Make a box with a lid".into(), Vec::new())
        .unwrap();
    loop {
        match recv(&mut messages, "the slice turn").await {
            BridgeMessage::Event(event) => {
                let finished = matches!(event, EngineEvent::TurnFinished(_));
                let _ = window.update(Message::Bridge(BridgeMessage::Event(event)));
                if finished {
                    break;
                }
            }
            BridgeMessage::Closed => panic!("the turn closed without a terminal"),
            BridgeMessage::PythonSheetRequested => {
                panic!("the send was refused: python not verified")
            }
            other => {
                let _ = window.update(Message::Bridge(other));
            }
        }
    }
    let turn_kinds = collect_reloads(
        &reloads,
        &[
            ReloadKind::Mesh,
            ReloadKind::Plate,
            ReloadKind::Schematic,
            ReloadKind::Manifest,
        ],
    );
    for kind in [ReloadKind::Mesh, ReloadKind::Schematic] {
        assert!(
            turn_kinds.contains(&kind),
            "watcher never saw {kind:?}: {turn_kinds:?}"
        );
    }
    forward_reloads(&mut window, &turn_kinds);
    assert_eq!(window.viewer.scene.scene.parts.len(), 2);

    // the TURN recorded version 1 — the agent-turn outputs gate
    // (agentDidFinish: complete outputs + a changed model.py), so the
    // slider versions start at 2
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(1));
    let chat = load_chat(&root).unwrap();
    assert_eq!(chat.last().unwrap().role, Role::Summary);
    assert_eq!(chat.last().unwrap().text, "Version 1");
    assert_eq!(chat.last().unwrap().detail.as_deref(), Some("1"));

    // ── the slider commit: the rewrite lands, the fake build copies the
    // outputs back, the watcher-driven reload rebuilds the scene. The
    // fake's copy mode skips EXISTING files, so model.3mf comes out first
    // to make the rebuild observable: a corrupt 3MF (fresh mtime — the
    // watch's change-detection boundary) reloads the scene to the STL's
    // single-part fallback (the M0 carry-forward's documented shape), and
    // the rebuild's re-copy brings the 2-part model back through a real
    // watcher event. Deletes emit nothing (watch.rs: a vanished file has
    // nothing to reload from), which is why the removal below needs no
    // drain.
    std::fs::write(root.join("output/model.3mf"), b"not a 3mf").unwrap();
    let stale_kinds = collect_reloads(&reloads, &[ReloadKind::Mesh]);
    assert!(stale_kinds.contains(&ReloadKind::Mesh), "{stale_kinds:?}");
    forward_reloads(&mut window, &stale_kinds);
    assert_eq!(
        window.viewer.scene.scene.parts.len(),
        1,
        "a corrupt 3MF reloads as the STL's single part"
    );
    std::fs::remove_file(root.join("output/model.3mf")).unwrap();

    bridge.set_parameter("wall", 3.0).unwrap();
    run_until(
        &mut window,
        &mut messages,
        |m| matches!(m, BridgeMessage::BuildFinished { .. }),
        "the slider build",
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 10\n"
    );
    assert!(
        root.join("output/model.3mf").is_file(),
        "the fake build re-copied the outputs"
    );
    let build_kinds = collect_reloads(&reloads, &[ReloadKind::Mesh]);
    assert!(build_kinds.contains(&ReloadKind::Mesh), "{build_kinds:?}");
    forward_reloads(&mut window, &build_kinds);
    assert_eq!(
        window.viewer.scene.scene.parts.len(),
        2,
        "the watcher-driven reload rebuilt the 2-part scene"
    );

    // the slider build recorded version 2, with its summary row
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(2));
    let chat = load_chat(&root).unwrap();
    assert_eq!(chat.last().unwrap().role, Role::Summary);
    assert_eq!(chat.last().unwrap().text, "Version 2");
    assert_eq!(chat.last().unwrap().detail.as_deref(), Some("2"));

    // ── the second commit records the next version
    bridge.set_parameter("height", 12.0).unwrap();
    run_until(
        &mut window,
        &mut messages,
        |m| matches!(m, BridgeMessage::BuildFinished { .. }),
        "the second build",
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 12\n"
    );
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(3));
    let _ = collect_reloads(&reloads, &[ReloadKind::Mesh]); // drain the burst

    // ── restore(2), the first slider's snapshot: model.py, chat and
    // output return to it
    std::fs::write(root.join("output/sentinel.txt"), b"post-v3").unwrap();
    bridge.restore(2).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 10\n"
    );
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(2));
    let chat = load_chat(&root).unwrap();
    assert_eq!(chat.last().unwrap().detail.as_deref(), Some("2"));
    assert!(
        !root.join("output/sentinel.txt").exists(),
        "the output dir is the snapshot's, not v3's"
    );
    assert!(
        root.join("output/model.3mf").is_file(),
        "the snapshot's outputs are back"
    );

    // ── the compare picker: version 3's parameters, parsed from its
    // stored model.py copy
    bridge.compare_with(Some(3));
    let compared = bridge.comparison_parameters();
    assert_eq!(compared.get("wall"), Some(&3.0));
    assert_eq!(compared.get("height"), Some(&12.0));
    bridge.compare_with(None);
    assert!(bridge.comparison_parameters().is_empty());

    // SAFETY: env cleanup still inside the ENV_LOCK window.
    unsafe {
        std::env::remove_var("CAD_TEST_CLAUDE");
        std::env::remove_var("CAD_FAKE_SCENARIO");
        std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
        std::env::remove_var("CAD_FAKE_ARGV_OUT");
        std::env::remove_var("CAD_FAKE_COPY_FROM");
        std::env::remove_var("CAD_FAKE_PY_MODE");
        std::env::remove_var("CAD_FAKE_PY_FIXTURES");
    }
    drop(bridge);
    drop(guard);
    let _ = window.view(); // the restored shape builds a frame
}
