//! Session-lifecycle integration tests (task 8): the full upstream turn
//! dance through `Session::send` — busy retry then fresh restart,
//! missing-session fresh fallback, auth failure without retry, and
//! cancellation during the busy-retry wait. The fake agent runs in directory
//! mode (`CAD_FAKE_SCENARIO` points at a directory; the fake picks
//! `<n>.jsonl` per its counter file), so one `send` walks a scripted
//! sequence of attempts, and every invocation appends one argv-dump line —
//! spawn counts and per-run flags read those lines. `CAD_FAKE_STDERR` stays
//! constant across the directory's runs ON PURPOSE: only the dance's guards
//! (a resumed, uncancelled attempt that errored or produced no result) keep
//! the SUCCESSFUL later runs from being misclassified by the same stderr.

use std::path::{Path, PathBuf};
use std::time::Duration;

use engine::agent::claude::TurnRequest;
use engine::agent::session::{EngineEvent, Session, TurnOutcome};
use engine::project::store::{ProjectMetadata, load_metadata, save_metadata};
use engine::settings::Settings;

/// The fake reads its env at spawn time and the session snapshots the login
/// environment per attempt, so every [set env → send → TurnFinished] window
/// must be exclusive across the parallel test threads.
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const PONG_SID: &str = "94fcba3f-44f9-4003-b32d-c469499dc529";
const BUSY_SID: &str = "11111111-2222-3333-4444-555555555555";

/// AgentSession.swift:79-82's system row (English catalog value; the Swift
/// literals are the Italian source strings).
const BUSY_FRESH_MESSAGE: &str = "The previous session is still in use by another process: the conversation context started over, but the existing model.py is still the basis for the work.";
/// AgentSession.swift:89-91's system row.
const MISSING_MESSAGE: &str = "The previous session wasn't found: the conversation context has been reset, but the existing model.py remains the basis of the work.";

fn fixture_fake(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/fake-agent")
        .join(p)
}

fn system_template() -> String {
    std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/upstream/CADSystemPrompt.md"),
    )
    .unwrap()
}

fn mk_project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("cad-lifecycle-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    root
}

/// A directory-mode scenario: run n replays `sources[n-1]`.
fn scenario_dir(tag: &str, sources: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cad-scenario-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    for (n, source) in sources.iter().enumerate() {
        std::fs::copy(fixture_fake(source), dir.join(format!("{}.jsonl", n + 1))).unwrap();
    }
    dir
}

/// The argv of every invocation, in spawn order (one append-mode dump line
/// each — the fake's authoritative spawn record).
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

/// Point the CAD_* env at one scripted dance and run `send` to its
/// TurnFinished, returning every EngineEvent in order plus the argv dump.
async fn run_dance(
    root: &Path,
    scenario: &Path,
    stderr: Option<&Path>,
    preset_sid: Option<&str>,
    retry_delay: Duration,
) -> (Vec<EngineEvent>, PathBuf) {
    let guard = ENV_LOCK.lock().await;
    if let Some(sid) = preset_sid {
        let mut metadata = ProjectMetadata::new("test");
        metadata.session_id = Some(sid.into());
        save_metadata(root, &metadata).unwrap();
    }
    let dump = root.join("argv-dump.jsonl");
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var("CAD_FAKE_SCENARIO", scenario);
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::set_var("CAD_FAKE_ARGV_OUT", &dump);
        match stderr {
            Some(path) => std::env::set_var("CAD_FAKE_STDERR", path),
            None => std::env::remove_var("CAD_FAKE_STDERR"),
        }
    }
    let mut session = Session::new(Settings::default())
        .with_system_template(system_template())
        .with_retry_delay(retry_delay);
    let mut handle = session.send(
        root,
        &TurnRequest { prompt: "please respond".into(), attachments: Vec::new() },
    );
    let events = collect_until_finished(&mut handle).await;
    drop(guard);
    (events, dump)
}

/// Drain the handle's events up to and including TurnFinished, bounded by
/// the suite-wide 5 s margin (never a fixed sleep).
async fn collect_until_finished(
    handle: &mut engine::agent::session::TurnHandle,
) -> Vec<EngineEvent> {
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, handle.events.recv()).await {
            Ok(Some(event)) => {
                let finished = matches!(event, EngineEvent::TurnFinished(_));
                events.push(event);
                if finished {
                    return events;
                }
            }
            Ok(None) => panic!("events channel closed before TurnFinished: {events:?}"),
            Err(_) => panic!("timed out before TurnFinished: {events:?}"),
        }
    }
}

fn positions<F: Fn(&EngineEvent) -> bool>(events: &[EngineEvent], pred: F) -> Vec<usize> {
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| pred(event))
        .map(|(i, _)| i)
        .collect()
}

fn started_sids(events: &[EngineEvent]) -> Vec<Option<&str>> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::Started { session_id } => Some(session_id.as_deref()),
            _ => None,
        })
        .collect()
}

fn finished<'a>(events: &'a [EngineEvent]) -> &'a TurnOutcome {
    match events.last() {
        Some(EngineEvent::TurnFinished(outcome)) => outcome,
        other => panic!("last event is TurnFinished, got {other:?}"),
    }
}

/// `argv` contains `flag` followed immediately by `value`.
fn has_pair(argv: &[String], flag: &str, value: &str) -> bool {
    argv.windows(2).any(|pair| pair[0] == flag && pair[1] == value)
}

fn system_rows(events: &[EngineEvent], message: &str) -> Vec<usize> {
    positions(events, |event| {
        matches!(event, EngineEvent::System { text } if text == message)
    })
}

// ─── the dance ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn busy_retries_once_then_fresh() {
    let root = mk_project();
    let scenario = scenario_dir(
        "busy3",
        &[
            "scenarios/busy-then-success.jsonl",
            "scenarios/busy-then-success.jsonl",
            "scenarios/pong.jsonl",
        ],
    );
    let (events, dump) = run_dance(
        &root,
        &scenario,
        Some(&fixture_fake("scenarios/busy-then-success.stderr")),
        Some("seed-sid"),
        Duration::from_millis(50),
    )
    .await;

    // the busy stderr rides every run, yet only runs 1-2 (error results)
    // classify; run 3's success ends the dance
    assert_eq!(
        finished(&events),
        &TurnOutcome::Success { session_id: Some(PONG_SID.into()), text: "pong".into() }
    );
    assert_eq!(load_metadata(&root).unwrap().session_id.as_deref(), Some(PONG_SID));

    // exactly three spawns: resume, resume retry, fresh
    let argv = dump_argv(&dump);
    assert_eq!(argv.len(), 3, "busy → retry → fresh is three spawns");
    assert!(has_pair(&argv[0], "--resume", "seed-sid"), "run 1 resumes the stored id");
    assert!(has_pair(&argv[1], "--resume", BUSY_SID), "run 1's init updated the stored id");
    assert!(argv[0].iter().any(|a| a == "--fork-session"));
    assert!(argv[1].iter().any(|a| a == "--fork-session"));
    assert!(
        !argv[2].iter().any(|a| a == "--resume" || a == "--fork-session"),
        "the post-collision run is fresh"
    );

    // the busy explanation arrives after the second collision, before the
    // fresh run starts
    let system = system_rows(&events, BUSY_FRESH_MESSAGE);
    assert_eq!(system.len(), 1, "{events:?}");
    let started = positions(&events, |e| matches!(e, EngineEvent::Started { .. }));
    assert_eq!(started_sids(&events), vec![Some(BUSY_SID), Some(BUSY_SID), Some(PONG_SID)]);
    assert!(started[1] < system[0] && system[0] < started[2], "{events:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_retry_succeeds_second_time() {
    let root = mk_project();
    let scenario =
        scenario_dir("busy2", &["scenarios/busy-then-success.jsonl", "scenarios/pong.jsonl"]);
    let (events, dump) = run_dance(
        &root,
        &scenario,
        Some(&fixture_fake("scenarios/busy-then-success.stderr")),
        Some("seed-sid"),
        Duration::from_millis(50),
    )
    .await;

    assert_eq!(
        finished(&events),
        &TurnOutcome::Success { session_id: Some(PONG_SID.into()), text: "pong".into() }
    );
    // busy once, resume retry succeeds — two spawns, both resuming, no
    // system row (no fresh fallback happened)
    let argv = dump_argv(&dump);
    assert_eq!(argv.len(), 2);
    assert!(has_pair(&argv[0], "--resume", "seed-sid"));
    assert!(has_pair(&argv[1], "--resume", BUSY_SID));
    assert!(argv.iter().all(|run| run.iter().any(|a| a == "--fork-session")));
    assert!(
        events.iter().all(|e| !matches!(e, EngineEvent::System { .. })),
        "no fallback explanation: {events:?}"
    );
    assert_eq!(load_metadata(&root).unwrap().session_id.as_deref(), Some(PONG_SID));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_session_falls_back_fresh() {
    let root = mk_project();
    let scenario =
        scenario_dir("missing", &["scenarios/missing-session.jsonl", "scenarios/pong.jsonl"]);
    let (events, dump) = run_dance(
        &root,
        &scenario,
        Some(&fixture_fake("scenarios/missing-session.stderr")),
        Some("stale-sid"),
        Duration::from_millis(50),
    )
    .await;

    // the stale id is explained away and replaced by the fresh run's id
    assert_eq!(
        finished(&events),
        &TurnOutcome::Success { session_id: Some(PONG_SID.into()), text: "pong".into() }
    );
    let system = system_rows(&events, MISSING_MESSAGE);
    assert_eq!(system.len(), 1, "{events:?}");
    let started = positions(&events, |e| matches!(e, EngineEvent::Started { .. }));
    assert_eq!(started_sids(&events), vec![Some(PONG_SID)], "only the fresh run starts");
    assert!(system[0] < started[0], "the explanation precedes the fresh run: {events:?}");

    let argv = dump_argv(&dump);
    assert_eq!(argv.len(), 2);
    assert!(has_pair(&argv[0], "--resume", "stale-sid"));
    assert!(
        !argv[1].iter().any(|a| a == "--resume" || a == "--fork-session"),
        "the fallback run is fresh"
    );
    assert_eq!(load_metadata(&root).unwrap().session_id.as_deref(), Some(PONG_SID));
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_error_no_retry() {
    let root = mk_project();
    let scenario = scenario_dir("auth", &["scenarios/auth-error.jsonl"]);
    let (events, dump) = run_dance(
        &root,
        &scenario,
        Some(&fixture_fake("scenarios/auth-error.stderr")),
        Some("seed-sid"),
        Duration::from_millis(50),
    )
    .await;

    // auth is terminal even though a session id was available to resume
    match finished(&events) {
        TurnOutcome::Failed { auth, error } => {
            assert!(auth, "{error}");
            assert!(error.contains("Unauthorized"), "{error}");
        }
        other => panic!("{other:?}"),
    }
    let argv = dump_argv(&dump);
    assert_eq!(argv.len(), 1, "no retry, no fresh run: {argv:?}");
    assert!(
        events.iter().all(|e| !matches!(e, EngineEvent::System { .. })),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_turn_ends_cleanly() {
    let root = mk_project();
    let scenario = scenario_dir("cancel", &["scenarios/busy-then-success.jsonl"]);
    let dump = root.join("argv-dump.jsonl");

    let guard = ENV_LOCK.lock().await;
    let mut metadata = ProjectMetadata::new("test");
    metadata.session_id = Some("seed-sid".into());
    save_metadata(&root, &metadata).unwrap();
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var("CAD_FAKE_SCENARIO", &scenario);
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::set_var("CAD_FAKE_ARGV_OUT", &dump);
        std::env::set_var(
            "CAD_FAKE_STDERR",
            fixture_fake("scenarios/busy-then-success.stderr"),
        );
    }
    let mut session = Session::new(Settings::default())
        .with_system_template(system_template())
        // wide enough that the cancel below lands inside the retry wait,
        // not inside the busy attempt itself
        .with_retry_delay(Duration::from_secs(2));
    let mut handle = session.send(
        &root,
        &TurnRequest { prompt: "please respond".into(), attachments: Vec::new() },
    );

    // the busy attempt announces itself, then exits almost immediately;
    // the dance goes quiet for the 2 s retry wait
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, handle.events.recv()).await {
            Ok(Some(EngineEvent::Started { .. })) => break,
            Ok(other) => panic!("expected Started first: {other:?}"),
            Err(_) => panic!("Started never arrived"),
        }
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    handle.cancel();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut rest = Vec::new();
    loop {
        match tokio::time::timeout_at(deadline, handle.events.recv()).await {
            Ok(Some(event)) => {
                let finished = matches!(event, EngineEvent::TurnFinished(_));
                rest.push(event);
                if finished {
                    break;
                }
            }
            Ok(None) => panic!("channel closed before TurnFinished: {rest:?}"),
            Err(_) => panic!("cancel did not end the dance within 2 s: {rest:?}"),
        }
    }
    assert_eq!(rest.last(), Some(&EngineEvent::TurnFinished(TurnOutcome::Cancelled)));
    drop(guard);

    // the retry wait absorbed the cancel: no second spawn ever happened
    assert_eq!(dump_argv(&dump).len(), 1, "no spawn after a cancelled retry wait");
}
