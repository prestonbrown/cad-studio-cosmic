//! Bridge e2e: the fake agent driven through `Bridge::launch` — the drain
//! loop's forwarding, the send queue's pause/auto-run/resume rules, the
//! persisted transcript, and the cancel law under a flood with a slow
//! consumer. The harness is the engine tests' ENV_LOCK + bounded-poll
//! pattern (engine/tests/session_lifecycle.rs:95-135): the fake reads its
//! env at spawn time and the driver spawns asynchronously, so the lock is
//! held across every [set env → launch → wait] window in a test.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use app::bridge::{Bridge, Message};
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::channel::mpsc::UnboundedReceiver;
use engine::agent::session::{EngineEvent, TurnOutcome};
use engine::project::store::{Action, Role, ToolState, load_chat};
use engine::settings::Settings;

/// The fake reads its env at spawn time and the session snapshots the login
/// environment per attempt, so every [set env → launch → message] window
/// must be exclusive across the parallel test threads.
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const PONG_SID: &str = "94fcba3f-44f9-4003-b32d-c469499dc529";

fn fixture_fake(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/fake-agent")
        .join(p)
}

fn mk_project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("cad-bridge-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    root
}

/// A directory-mode scenario: run n replays `sources[n-1]`.
fn scenario_dir(tag: &str, sources: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cad-bridge-scenario-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for (n, source) in sources.iter().enumerate() {
        std::fs::copy(fixture_fake(source), dir.join(format!("{}.jsonl", n + 1))).unwrap();
    }
    dir
}

/// A copy of the tool-use scenario with the turn parked mid-tool: the fake
/// sleeps 5 s after the tool rows open, before any tool result arrives —
/// the state a cancel interrupts.
fn scenario_dir_mid_tool(tag: &str) -> PathBuf {
    let dir = scenario_dir(tag, &["scenarios/pong.jsonl", "scenarios/pong.jsonl"]);
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
    std::fs::write(dir.join("1.jsonl"), parked).unwrap();
    dir
}

/// Point the CAD_* env at one scripted sequence of fake invocations. The
/// caller holds ENV_LOCK for as long as spawns can still happen.
fn set_env(root: &Path, scenario: &Path, stderr: Option<&Path>, dump: &Path) {
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var("CAD_FAKE_SCENARIO", scenario);
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::set_var("CAD_FAKE_ARGV_OUT", dump);
        match stderr {
            Some(path) => std::env::set_var("CAD_FAKE_STDERR", path),
            None => std::env::remove_var("CAD_FAKE_STDERR"),
        }
    }
}

/// The argv of every fake invocation, in spawn order.
fn dump_argv(path: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("argv dump missing: {e}"))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

/// One message, bounded — never a fixed sleep.
async fn recv(messages: &mut UnboundedReceiver<Message>, what: &str) -> Message {
    match tokio::time::timeout(Duration::from_secs(5), messages.next()).await {
        Ok(Some(message)) => message,
        Ok(None) => panic!("message stream ended while waiting for {what}"),
        Err(_) => panic!("timed out waiting for {what}"),
    }
}

/// That NO TURN starts within `window` — the bounded absence check for
/// "the queue did not auto-run". Queue notifications may still arrive
/// (pause announcements land just after the failed turn's terminal).
async fn assert_silent(messages: &mut UnboundedReceiver<Message>, window: Duration) {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        match tokio::time::timeout_at(deadline, messages.next()).await {
            Ok(Some(Message::QueueChanged { .. })) => continue,
            Ok(Some(message)) => panic!("expected no turn activity, got {message:?}"),
            Ok(None) => panic!("message stream ended, expected silence"),
            Err(_) => return,
        }
    }
}

/// Drain the message stream through one turn's TurnStarted…TurnFinished,
/// returning the forwarded events. Queue notifications pass through
/// unasserted (their counts are behavior-checked where the queue is under
/// test); each helper call is one turn.
async fn drain_turn_messages(
    messages: &mut UnboundedReceiver<Message>,
    prompt: &str,
) -> Vec<EngineEvent> {
    loop {
        match recv(messages, "TurnStarted").await {
            Message::TurnStarted { prompt: seen, .. } => {
                assert_eq!(seen, prompt, "{seen:?}");
                break;
            }
            // the PREVIOUS turn's success tail announces after its terminal
            Message::QueueChanged { .. } | Message::TurnSuccessBookkeeping => continue,
            other => panic!("expected TurnStarted for {prompt:?}, got {other:?}"),
        }
    }
    let mut events = Vec::new();
    loop {
        match recv(messages, "an event").await {
            Message::Event(event) => {
                let finished = matches!(event, EngineEvent::TurnFinished(_));
                events.push(event);
                if finished {
                    return events;
                }
            }
            Message::QueueChanged { .. } => continue,
            other => panic!("expected an engine event, got {other:?}"),
        }
    }
}

/// The last event is the turn's terminal outcome.
fn finished(events: &[EngineEvent]) -> &TurnOutcome {
    match events.last() {
        Some(EngineEvent::TurnFinished(outcome)) => outcome,
        other => panic!("last event is TurnFinished, got {other:?}"),
    }
}

// ─── the named suite ─────────────────────────────────────────────────────────

/// A successful send streams every engine event verbatim to the app layer,
/// persists the transcript (user rows included) at each turn end, and a
/// send that arrived mid-turn runs next after the 1.0 s queue delay. The
/// spawned argv carries the rendered CAD system prompt — the session the
/// bridge builds runs no turn without the contract.
#[tokio::test(flavor = "multi_thread")]
async fn send_success_streams_and_saves() {
    let root = mk_project();
    let scenario = scenario_dir("ok", &["scenarios/pong.jsonl", "scenarios/pong.jsonl"]);
    let dump = root.join("argv-dump.jsonl");

    let guard = ENV_LOCK.lock().await;
    set_env(&root, &scenario, None, &dump);
    let (bridge, mut messages) = Bridge::launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test(); // the send gate refuses without a ready python
    bridge.enqueue("first".into(), Vec::new()).unwrap();
    bridge.enqueue("second".into(), Vec::new()).unwrap(); // lands while the first runs → FIFO

    let events = drain_turn_messages(&mut messages, "first").await;
    assert_eq!(
        events,
        vec![
            EngineEvent::Started {
                session_id: Some(PONG_SID.into())
            },
            EngineEvent::ThinkingStarted,
            EngineEvent::Thinking {
                estimated_tokens: 50
            },
            EngineEvent::Thinking {
                estimated_tokens: 192
            },
            EngineEvent::ThinkingFinished,
            EngineEvent::TextStreaming {
                text: "pong".into()
            },
            EngineEvent::TextConfirmed {
                text: "pong".into()
            },
            EngineEvent::TurnFinished(TurnOutcome::Success {
                session_id: Some(PONG_SID.into()),
                text: "pong".into(),
            }),
        ],
        "every event forwards verbatim, in order"
    );
    let first_ended = Instant::now();

    // the queued send auto-runs after the 1.0 s delay — not before it, and
    // bounded above
    let second_events = drain_turn_messages(&mut messages, "second").await;
    let gap = first_ended.elapsed();
    assert!(
        gap >= Duration::from_millis(950),
        "auto-run came too early: {gap:?}"
    );
    assert!(
        gap < Duration::from_secs(5),
        "auto-run came too late: {gap:?}"
    );
    assert!(matches!(
        finished(&second_events),
        TurnOutcome::Success { .. }
    ));
    drop(bridge);
    drop(guard);

    // the persisted transcript: both user rows, both turns' content
    let saved = load_chat(&root).unwrap();
    let user_rows: Vec<&str> = saved
        .iter()
        .filter(|row| row.role == Role::User)
        .map(|row| row.text.as_str())
        .collect();
    assert_eq!(user_rows, ["first", "second"], "{saved:?}");
    assert_eq!(
        saved.iter().filter(|row| row.text == "pong").count(),
        2,
        "{saved:?}"
    );
    assert!(
        saved.iter().any(|row| row.text.starts_with("Thought for")),
        "the thinking rows persisted: {saved:?}"
    );

    // the session carried the CAD contract into the spawned argv
    let argv = dump_argv(&dump);
    assert_eq!(argv.len(), 2, "two spawns: {argv:?}");
    let prompt = argv[0]
        .windows(2)
        .find(|pair| pair[0] == "--append-system-prompt")
        .map(|pair| pair[1].as_str())
        .expect("--append-system-prompt in argv");
    assert!(
        prompt.contains("expert CAD engineer"),
        "the template rendered: {prompt}"
    );
    assert!(
        !prompt.contains("{{PROJECT_NAME}}"),
        "substitutions applied: {prompt}"
    );
}

/// An auth failure rows the open-terminal action, persists it, and pauses
/// the queue — the queued send does not auto-run until `resume_queue`.
#[tokio::test(flavor = "multi_thread")]
async fn auth_failure_pauses_queue_and_rows_action() {
    let root = mk_project();
    let scenario = scenario_dir(
        "auth",
        &["scenarios/auth-error.jsonl", "scenarios/pong.jsonl"],
    );
    let dump = root.join("argv-dump.jsonl");

    let guard = ENV_LOCK.lock().await;
    set_env(
        &root,
        &scenario,
        Some(&fixture_fake("scenarios/auth-error.stderr")),
        &dump,
    );
    let (bridge, mut messages) = Bridge::launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test(); // the send gate refuses without a ready python
    bridge.enqueue("first".into(), Vec::new()).unwrap();
    bridge.enqueue("second".into(), Vec::new()).unwrap();

    let events = drain_turn_messages(&mut messages, "first").await;
    match finished(&events) {
        TurnOutcome::Failed { auth, error, .. } => {
            assert!(auth, "{error}");
            assert!(error.contains("Unauthorized"), "{error}");
        }
        other => panic!("auth failure, got {other:?}"),
    }

    // the queue pauses: past the 1.0 s auto-run window, nothing starts
    assert_silent(&mut messages, Duration::from_millis(1800)).await;

    // the persisted auth row carries the login action
    let saved = load_chat(&root).unwrap();
    let row = saved.last().expect("the auth row saved");
    assert_eq!(row.role, Role::System);
    assert_eq!(row.action, Some(Action::OpenTerminal));
    assert!(row.text.contains("signed in"), "{}", row.text);

    // resume runs the queued send
    bridge.resume_queue();
    let second = drain_turn_messages(&mut messages, "second").await;
    assert!(matches!(finished(&second), TurnOutcome::Success { .. }));
    drop(bridge);
    drop(guard);
}

/// A failure whose text merely embeds the digits 401 — token counts and
/// latencies — is NOT an auth failure: the error row carries the exit code
/// and no action, and the queue still pauses (any failure does).
#[tokio::test(flavor = "multi_thread")]
async fn auth_401_in_other_numbers_is_not_auth() {
    app::localize::select("en");
    let root = mk_project();
    let scenario = scenario_dir(
        "nums",
        &[
            "scenarios/auth-401-in-numbers.jsonl",
            "scenarios/pong.jsonl",
        ],
    );
    let dump = root.join("argv-dump.jsonl");

    let guard = ENV_LOCK.lock().await;
    set_env(&root, &scenario, None, &dump);
    let (bridge, mut messages) = Bridge::launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test(); // the send gate refuses without a ready python
    bridge.enqueue("first".into(), Vec::new()).unwrap();
    bridge.enqueue("second".into(), Vec::new()).unwrap();

    let events = drain_turn_messages(&mut messages, "first").await;
    match finished(&events) {
        TurnOutcome::Failed {
            auth, exit_code, ..
        } => {
            assert!(
                !auth,
                "1401/4.401 embed the digits but are not an auth status"
            );
            assert_eq!(*exit_code, Some(1));
        }
        other => panic!("a code-1 failure, got {other:?}"),
    }
    assert_silent(&mut messages, Duration::from_millis(1800)).await;

    let saved = load_chat(&root).unwrap();
    let row = saved.last().expect("the error row saved");
    assert_eq!(row.role, Role::System);
    assert_eq!(row.action, None, "no login action without an auth failure");
    assert_eq!(row.text, "Claude Code ended with an error (code 1).");

    bridge.resume_queue();
    let second = drain_turn_messages(&mut messages, "second").await;
    assert!(matches!(finished(&second), TurnOutcome::Success { .. }));
    drop(bridge);
    drop(guard);
}

/// Cancelling mid-tool ends the turn as a failure would: the running tool
/// row fails open and persists, the interrupted summary row lands, and the
/// queue pauses until resumed — cancel clears the running turn only.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_mid_turn_pauses_queue_fails_open() {
    let root = mk_project();
    let scenario = scenario_dir_mid_tool("cancel");
    let dump = root.join("argv-dump.jsonl");

    let guard = ENV_LOCK.lock().await;
    set_env(&root, &scenario, None, &dump);
    let (bridge, mut messages) = Bridge::launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test(); // the send gate refuses without a ready python
    bridge.enqueue("parked".into(), Vec::new()).unwrap();
    bridge.enqueue("second".into(), Vec::new()).unwrap();

    // wait until a tool row is open, then interrupt
    match recv(&mut messages, "TurnStarted").await {
        Message::TurnStarted { prompt, .. } => assert_eq!(prompt, "parked"),
        other => panic!("expected TurnStarted, got {other:?}"),
    }
    loop {
        match recv(&mut messages, "a tool row to open").await {
            Message::Event(EngineEvent::ToolStarted { .. }) => break,
            Message::Event(EngineEvent::TurnFinished(outcome)) => {
                panic!("turn ended before a tool row opened: {outcome:?}")
            }
            Message::Event(_) | Message::QueueChanged { .. } => continue,
            other => panic!("expected an engine event, got {other:?}"),
        }
    }
    bridge.cancel();
    // reaching this arm at all is the assertion: the cancel reached the
    // engine and ended the turn as Cancelled, not by running to completion
    loop {
        match recv(&mut messages, "the cancelled turn's terminal").await {
            Message::Event(EngineEvent::TurnFinished(TurnOutcome::Cancelled)) => break,
            Message::Event(_) | Message::QueueChanged { .. } => continue,
            other => panic!("expected events, got {other:?}"),
        }
    }

    assert_silent(&mut messages, Duration::from_millis(1800)).await;

    let saved = load_chat(&root).unwrap();
    let tool = saved
        .iter()
        .rev()
        .find(|row| row.tool.is_some())
        .expect("the open tool row persisted")
        .tool
        .as_ref()
        .unwrap();
    assert_eq!(
        tool.state,
        ToolState::Failed,
        "the interrupted tool failed open: {saved:?}"
    );
    assert_eq!(tool.preview, None);
    assert!(
        saved
            .iter()
            .any(|row| row.role == Role::Summary && row.text == "Stopped"),
        "the interrupted summary row persisted: {saved:?}"
    );

    bridge.resume_queue();
    let second = drain_turn_messages(&mut messages, "second").await;
    assert!(
        matches!(finished(&second), TurnOutcome::Success { .. }),
        "{second:?}"
    );
    drop(bridge);
    drop(guard);
}

/// A ~5k-event flood with a slow consumer cannot wedge cancel: the command
/// channel is absorbed between translations, so the interrupted terminal
/// arrives within a bounded poll while thousands of events are still in
/// flight (the drain law).
#[tokio::test(flavor = "multi_thread")]
async fn flood_does_not_wedge_cancel() {
    let root = mk_project();
    let scenario = fixture_fake("scenarios/flood.jsonl");
    let dump = root.join("argv-dump.jsonl");

    let guard = ENV_LOCK.lock().await;
    set_env(&root, &scenario, None, &dump);
    let (bridge, mut messages) = Bridge::launch(Settings::default(), root.clone());
    bridge.mark_python_ready_for_test(); // the send gate refuses without a ready python
    bridge.enqueue("flood".into(), Vec::new()).unwrap();
    match recv(&mut messages, "TurnStarted").await {
        Message::TurnStarted { prompt, .. } => assert_eq!(prompt, "flood"),
        other => panic!("expected TurnStarted, got {other:?}"),
    }
    match recv(&mut messages, "Started").await {
        Message::Event(EngineEvent::Started { .. }) => {}
        other => panic!("expected Started, got {other:?}"),
    }

    // the slow consumer: stop reading while the flood pours into the
    // unbounded forward channel, then interrupt
    tokio::time::sleep(Duration::from_millis(300)).await;
    bridge.cancel();
    let cancelled_at = Instant::now();
    // still not reading: the cancel must be absorbed and the SIGINT sent
    // with the consumer idle
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut events = 0usize;
    let terminal = loop {
        match tokio::time::timeout(Duration::from_secs(5), messages.next()).await {
            Ok(Some(Message::Event(event @ EngineEvent::TurnFinished(_)))) => break event,
            Ok(Some(Message::Event(_))) => events += 1,
            Ok(Some(other)) => panic!("expected events, got {other:?}"),
            Ok(None) => panic!("stream ended before the terminal, {events} events seen"),
            Err(_) => panic!("cancel wedged: no terminal within 5 s ({events} events seen)"),
        }
    };
    assert!(
        matches!(terminal, EngineEvent::TurnFinished(TurnOutcome::Cancelled)),
        "{terminal:?}"
    );
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(5),
        "the terminal landed within the bound"
    );
    assert!(events >= 500, "the flood actually flowed: {events} events");
    drop(bridge);
    drop(guard);
}
