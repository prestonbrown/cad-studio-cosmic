//! The engine a turn runs is the engine the window names: a picker flip
//! written to the settings file mid-turn must not relabel the running
//! turn's status line. The engine decides a turn's engine when the turn
//! starts; the bridge classifies every transition with it and the window
//! mirrors the bridge, never re-reading the settings.

use std::path::PathBuf;
use std::time::Duration;

use app::activity::Activity;
use app::bridge::{Bridge, Message as BridgeMessage};
use app::settings_store::SettingsStore;
use app::windows::project::{Message, ProjectWindow};
use cosmic::iced::futures::StreamExt;
use engine::agent::session::EngineEvent;
use engine::project::store;
use engine::settings::Settings;

static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn fixture_fake(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/fake-agent")
        .join(p)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mid_turn_picker_flip_does_not_rename_the_running_turn() {
    let _guard = ENV_LOCK.lock().await;
    let parent = std::env::temp_dir().join(format!("cad-turn-name-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&parent).unwrap();
    let root = store::create_project(&parent, "Flip").unwrap();
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var(
            "CAD_FAKE_SCENARIO",
            fixture_fake("scenarios/tool-use.jsonl"),
        );
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::set_var("CAD_FAKE_ARGV_OUT", root.join("argv-dump.jsonl"));
        std::env::remove_var("CAD_FAKE_STDERR");
    }

    // the turn starts under claude
    let settings = Settings::default();
    let (bridge, mut messages) = Bridge::launch(settings.clone(), root.clone());
    // the picker already says codex in the window's settings by the time
    // the turn's events land
    let store = SettingsStore::at(parent.join("settings.json"));
    let mut window = ProjectWindow::with_bridge(root.clone(), &store, bridge);
    store.update(|s| s.agent_engine = "codex".into()).unwrap();
    window.bridge.mark_python_ready_for_test();
    window
        .bridge
        .enqueue("make a bracket".into(), Vec::new())
        .expect("the send is not refused");

    let mut seen: Vec<Activity> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let message = match tokio::time::timeout_at(deadline, messages.next()).await {
            Ok(Some(message)) => message,
            other => panic!("no TurnFinished: {other:?} after {seen:?}"),
        };
        let finished = matches!(message, BridgeMessage::Event(EngineEvent::TurnFinished(_)));
        let _ = window.update(Message::Bridge(message));
        seen.push(window.activity.clone());
        if finished {
            break;
        }
    }

    let working: Vec<String> = seen
        .iter()
        .filter(|activity| activity.is_working())
        .map(Activity::text)
        .collect();
    assert!(
        working.iter().any(|text| text.contains(" · ")),
        "a tool line was mirrored: {seen:?}"
    );
    assert!(
        working.iter().all(|text| text.contains("Claude Code")),
        "every working line names the turn's engine: {seen:?}"
    );
    assert!(
        seen.iter()
            .all(|activity| !activity.text().contains("Codex")),
        "nothing names the flipped-to engine: {seen:?}"
    );

    unsafe {
        std::env::remove_var("CAD_TEST_CLAUDE");
        std::env::remove_var("CAD_FAKE_SCENARIO");
        std::env::remove_var("CAD_STUDIO_RUNNING_PROCESSES");
        std::env::remove_var("CAD_FAKE_ARGV_OUT");
    }
}
