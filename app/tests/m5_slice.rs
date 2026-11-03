//! The M5 wrap slice: the milestone end to end, headless — T8's startup
//! path (a REAL `python::env::check` over a scripted venv, every event
//! folded into the sheet state, the Ready terminal mirrored into the
//! bridge's readiness guards), one fake-agent turn landing the golden
//! outputs through the engine watcher and recording version 1 (the
//! agent-turn outputs gate), a slider commit through the WINDOW's
//! interception (`viewer_pane::Message::ParameterCommit` → the bridge)
//! rewriting model.py and rebuilding through the fake interpreter (the
//! watcher-driven reload rebuilding the scene) for version 2, a second
//! commit recording version 3, `restore(2)` returning
//! model.py/chat/output to the first slider's snapshot, and the compare
//! picker parsing version 3's parameters from its stored model.py copy.
//! The driver-appended version rows must render LIVE in the window's
//! chat, and the build activity must reach the window (the header's
//! stop-button/elapsed consequence).
//!
//! The failure-path companion (`failed_build_renders_row_activity_and_fix_action`):
//! a failed Compila renders the "model.py failed to build." system row
//! with its AskClaudeToFix action live in the window, the activity and
//! `build_error` reach the header's issue-button sources, and the action
//! button drives `ask_agent_to_fix` as a real turn.
//!
//! The window rides ONE bridge — the tapped `Bridge::launch` handle
//! injected through `ProjectWindow::with_bridge` (a cosmic Task's stream
//! is private to the iced runtime, so the harness folds the messages by
//! hand exactly as the app maps them; the window then reads the same
//! pipeline the live app reads). The CAD_* env is per-spawn: both tests
//! serialize on one ENV_LOCK.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use app::activity::Activity;
use app::bridge::{Bridge, Message as BridgeMessage};
use app::settings_store::SettingsStore;
use app::sheets::SetupSheetState;
use app::windows::project::{Message, ProjectWindow};
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::channel::mpsc::UnboundedReceiver;
use engine::agent::session::EngineEvent;
use engine::project::store::{self, Action, Role, load_metadata};
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

/// A fresh project with a slider-compatible model.py.
fn project_with_script() -> PathBuf {
    let parent = std::env::temp_dir().join(format!("cad-m5-slice-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&parent).unwrap();
    let root = store::create_project(&parent, "M5Slice").expect("create_project scaffolds");
    std::fs::write(root.join("model.py"), "wall = 2\nheight = 10\n").unwrap();
    root
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

/// The window's chat transcript holds a row with this shape.
fn chat_has_row(window: &ProjectWindow, role: Role, text: &str) -> bool {
    window
        .chat
        .messages
        .iter()
        .any(|row| row.role == role && row.text == text)
}

#[tokio::test(flavor = "multi_thread")]
async fn startup_check_through_slider_to_restore() {
    app::localize::select("en");
    let guard = ENV_LOCK.lock().await;

    let root = project_with_script();

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

    // the window over ONE tapped bridge — the harness's stand-in for the
    // app's mapping of the window's own bridge stream
    let settings = Settings::default();
    let (bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(settings.clone(), root.clone());
    let mut window =
        ProjectWindow::with_bridge(root.clone(), &SettingsStore::in_temp_dir(), bridge);
    let _ = window.view(); // the empty-project frame builds
    assert!(
        !window.bridge.python_ready(),
        "the bridge starts Unknown, guards closed"
    );
    window.bridge.set_python_state(sheet.state.clone());
    assert!(
        window.bridge.python_ready(),
        "the sheet's Ready terminal unblocks the guards"
    );

    // ── the fake agent's first turn: outputs land, the watcher sees them,
    // the scene rebuilds, and the bookkeeping's version row renders LIVE
    window
        .bridge
        .enqueue("Make a box with a lid".into(), Vec::new())
        .unwrap();
    run_until(
        &mut window,
        &mut messages,
        |m| matches!(m, BridgeMessage::Event(EngineEvent::TurnFinished(_))),
        "the slice turn",
    )
    .await;
    run_until(
        &mut window,
        &mut messages,
        |m| matches!(m, BridgeMessage::TurnSuccessBookkeeping),
        "the turn's bookkeeping",
    )
    .await;
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(1));
    assert_eq!(
        window.metadata.current_version,
        Some(1),
        "the turn-end save point refreshes the menu pointer"
    );
    assert!(
        chat_has_row(&window, Role::Summary, "Version 1"),
        "the driver-appended version row renders live: {:?}",
        window.chat.messages.last()
    );
    assert_eq!(
        window.activity,
        Activity::Succeeded("Model generated".into())
    );

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

    // ── the slider commit through the WINDOW's interception (the pane
    // emits ParameterCommit; the shell forwards to the bridge): the
    // rewrite lands, the fake build copies the outputs back, and the
    // watcher-driven reload rebuilds the scene. The fake's copy mode
    // skips EXISTING files, so model.3mf comes out first to make the
    // rebuild observable: a corrupt 3MF (fresh mtime — the watch's
    // change-detection boundary) reloads the scene to the STL's
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

    let _ = window.update(Message::Viewer(app::viewer_pane::Message::ParameterCommit(
        "wall".into(),
        3.0,
    )));
    assert!(
        window.is_busy(),
        "the build's Working activity reaches the window (stop button, timer)"
    );
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
    assert!(!window.is_busy(), "the build ended");
    let build_kinds = collect_reloads(&reloads, &[ReloadKind::Mesh]);
    assert!(build_kinds.contains(&ReloadKind::Mesh), "{build_kinds:?}");
    forward_reloads(&mut window, &build_kinds);
    assert_eq!(
        window.viewer.scene.scene.parts.len(),
        2,
        "the watcher-driven reload rebuilt the 2-part scene"
    );

    // version 2 recorded, its row rendered live, the activity classified,
    // and the version menu's pointer current (the fresh-metadata refresh)
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(2));
    assert_eq!(
        window.metadata.current_version,
        Some(2),
        "the menu's current pointer follows a slider-only record"
    );
    assert!(
        chat_has_row(&window, Role::Summary, "Version 2"),
        "the slider build's version row renders live: {:?}",
        window.chat.messages.last()
    );
    assert_eq!(
        window.activity,
        Activity::Succeeded("Model generated".into())
    );
    assert_eq!(window.bridge.build_error(), None);

    // ── the second commit records the next version
    let _ = window.update(Message::Viewer(app::viewer_pane::Message::ParameterCommit(
        "height".into(),
        12.0,
    )));
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
    assert_eq!(window.metadata.current_version, Some(3));
    let _ = collect_reloads(&reloads, &[ReloadKind::Mesh]); // drain the burst

    // ── restore(2) through the window's own message: model.py, chat and
    // output return to the first slider's snapshot
    std::fs::write(root.join("output/sentinel.txt"), b"post-v3").unwrap();
    let _ = window.update(Message::Restore(2));
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 3\nheight = 10\n"
    );
    assert_eq!(load_metadata(&root).unwrap().current_version, Some(2));
    assert_eq!(
        window.chat.messages.last().unwrap().detail.as_deref(),
        Some("2"),
        "the window's transcript is the restored snapshot's"
    );
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
    window.bridge.compare_with(Some(3));
    let compared = window.bridge.comparison_parameters();
    assert_eq!(compared.get("wall"), Some(&3.0));
    assert_eq!(compared.get("height"), Some(&12.0));
    window.bridge.compare_with(None);
    assert!(window.bridge.comparison_parameters().is_empty());

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
    drop(messages);
    let _ = window.view(); // the restored shape builds a frame
    drop(guard);
}

/// The failure path (M4 whole-branch review MAJOR 2, fix round): a failed
/// Compila renders the build-failure row with its AskClaudeToFix action
/// live in the window's chat, the failed activity and `build_error`
/// reach the header's sources, and the row's action button drives
/// `ask_agent_to_fix` as a real turn.
#[tokio::test(flavor = "multi_thread")]
async fn failed_build_renders_row_activity_and_fix_action() {
    app::localize::select("en");
    let guard = ENV_LOCK.lock().await;

    let root = project_with_script();
    // SAFETY: fake env holds this suite's ENV_LOCK for the whole test.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var("CAD_FAKE_SCENARIO", fixture_fake("scenarios/slice.jsonl"));
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::set_var("CAD_FAKE_PY_MODE", "fail");
    }

    let settings = Settings::default();
    let (bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(settings.clone(), root.clone());
    let mut window =
        ProjectWindow::with_bridge(root.clone(), &SettingsStore::in_temp_dir(), bridge);
    window.bridge.set_python_state(EnvState::Ready {
        build123d_version: "0.8.1".into(),
    });

    // the menu's Compila command: the build fails in the fake interpreter
    let _ = window.update(Message::Command(app::windows::project::Command::Build));
    assert!(
        window.is_busy(),
        "the build's Working activity reaches the window"
    );
    run_until(
        &mut window,
        &mut messages,
        |m| matches!(m, BridgeMessage::BuildFinished { .. }),
        "the failed build",
    )
    .await;

    // the failure surfaces in the window: the row, the activity, the
    // issue-button source
    let row = window
        .chat
        .messages
        .iter()
        .find(|row| row.role == Role::System && row.text == "model.py failed to build.")
        .expect("the build-failure row renders live")
        .clone();
    assert_eq!(row.action, Some(Action::AskClaudeToFix));
    assert!(row.detail.as_deref().unwrap().contains("Traceback"));
    assert_eq!(window.activity, Activity::Failed("Script error".into()));
    assert!(!window.is_busy());
    assert_eq!(
        window.bridge.build_error().as_deref(),
        Some("ValueError: bad shape"),
        "the header's issue-button source carries the build error"
    );

    // the row's action button: the traceback goes back to the agent as a
    // turn (upstream askClaudeToFix, Project.swift:258-260)
    let _ = window.update(Message::Chat(app::chat_view::Message::RowAction(
        row.id,
        Action::AskClaudeToFix,
    )));
    run_until(
        &mut window,
        &mut messages,
        |m| {
            matches!(
                m,
                BridgeMessage::TurnStarted { prompt, .. }
                    if prompt.starts_with("Running model.py fails with this error")
            )
        },
        "the ask-claude-to-fix turn",
    )
    .await;
    // the turn must land before the env clears (the fake is mid-scenario)
    run_until(
        &mut window,
        &mut messages,
        |m| matches!(m, BridgeMessage::Event(EngineEvent::TurnFinished(_))),
        "the fix turn's terminal",
    )
    .await;

    // SAFETY: env cleanup still inside the ENV_LOCK window.
    unsafe {
        std::env::remove_var("CAD_TEST_CLAUDE");
        std::env::remove_var("CAD_FAKE_SCENARIO");
        std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
        std::env::remove_var("CAD_FAKE_PY_MODE");
    }
    drop(messages);
    drop(guard);
}

/// A refused send keeps the draft in the input bar (M5 branch review
/// MAJOR 1): python not verified, the user types and sends — the text
/// STAYS in the pane (and the attachments with it), the bridge's parked
/// copy clears, and once the environment verifies, the same text sends
/// and the bar empties.
#[tokio::test(flavor = "multi_thread")]
async fn refused_send_keeps_the_draft_in_the_input_bar() {
    let guard = ENV_LOCK.lock().await;

    let root = project_with_script();
    // SAFETY: fake env holds this suite's ENV_LOCK for the whole test.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var("CAD_FAKE_SCENARIO", fixture_fake("scenarios/pong.jsonl"));
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
    }

    let settings = Settings::default();
    let (bridge, mut messages) =
        Bridge::with_interpreter(fake_python()).launch(settings.clone(), root.clone());
    let mut window =
        ProjectWindow::with_bridge(root.clone(), &SettingsStore::in_temp_dir(), bridge);
    // python Unknown — the send gate refuses (the first-run canonical path)

    // the user types into the input bar, then attaches a reference
    let typed = "a 20 mm cube";
    let _ = window.update(Message::Chat(app::chat_view::Message::Draft(
        cosmic::widget::text_editor::Action::Edit(cosmic::iced::widget::text_editor::Edit::Paste(
            std::sync::Arc::new(typed.into()),
        )),
    )));
    assert_eq!(window.chat_pane.draft(), typed, "the typing staged");
    window.chat.draft_attachments = vec!["references/bracket.png".into()];

    // Send: refused on python readiness — the draft must survive
    let _ = window.update(Message::Chat(app::chat_view::Message::Send));
    assert_eq!(
        window.chat_pane.draft(),
        typed,
        "a refused send keeps the text in the input bar"
    );
    assert_eq!(window.chat.draft, typed, "the mirror re-staged");
    assert_eq!(
        window.chat.draft_attachments,
        vec!["references/bracket.png".to_string()],
        "the attachments stay staged"
    );
    assert!(
        window.bridge.draft_text_for_test().is_none(),
        "the bridge's parked copy cleared — one home for the draft"
    );
    // the refusal announced the sheet (drain it so the stream stays clean)
    run_until(
        &mut window,
        &mut messages,
        |m| matches!(m, BridgeMessage::PythonSheetRequested),
        "the refusal's sheet request",
    )
    .await;

    // the environment verifies; the SAME text now sends and the bar empties
    window.bridge.set_python_state(EnvState::Ready {
        build123d_version: "0.8.1".into(),
    });
    let _ = window.update(Message::Chat(app::chat_view::Message::Send));
    run_until(
        &mut window,
        &mut messages,
        |m| matches!(m, BridgeMessage::TurnStarted { prompt, .. } if prompt == typed),
        "the re-sent turn",
    )
    .await;
    assert_eq!(
        window.chat_pane.draft(),
        "",
        "a successful send empties the bar"
    );
    assert!(window.chat.draft_attachments.is_empty());
    run_until(
        &mut window,
        &mut messages,
        |m| matches!(m, BridgeMessage::Event(EngineEvent::TurnFinished(_))),
        "the turn's terminal",
    )
    .await;

    // SAFETY: env cleanup still inside the ENV_LOCK window.
    unsafe {
        std::env::remove_var("CAD_TEST_CLAUDE");
        std::env::remove_var("CAD_FAKE_SCENARIO");
        std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
    }
    drop(messages);
    drop(guard);
}

/// The parameters overlay's collapse state persists through the engine
/// settings (M5 branch review MINOR 1): the seed comes from
/// `Settings.parameters_collapsed` at open, and the toggle writes back
/// through the window's settings store (the single-writer law).
#[tokio::test(flavor = "multi_thread")]
async fn parameters_collapse_persists_through_engine_settings() {
    let root = project_with_script();
    // a disposable settings file — the real one is the user's
    let store = SettingsStore::at(root.parent().unwrap().join("settings.json"));
    store.update(|s| s.parameters_collapsed = true).unwrap();
    let settings = store.load();
    let (bridge, _messages) =
        Bridge::with_interpreter(fake_python()).launch(settings.clone(), root.clone());
    let mut window = ProjectWindow::with_bridge(root.clone(), &store, bridge);
    assert!(
        window.viewer.parameters.collapsed,
        "the pane seeds from the persisted setting"
    );

    let _ = window.update(Message::Viewer(app::viewer_pane::Message::Parameters(
        app::parameters_view::Message::Collapse(false),
    )));
    assert!(
        !window.viewer.parameters.collapsed,
        "the pane followed the toggle"
    );
    assert!(!store.load().parameters_collapsed, "the toggle persisted");

    // and a fresh window over the same store seeds the saved state
    let (bridge2, _messages2) =
        Bridge::with_interpreter(fake_python()).launch(settings.clone(), root.clone());
    let window2 = ProjectWindow::with_bridge(root.clone(), &store, bridge2);
    assert!(!window2.viewer.parameters.collapsed);
}
