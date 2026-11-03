//! Session-turn integration tests (task 7): one fake-agent turn per test,
//! driven end to end through `Session::run_turn` — spawn, event translation,
//! tee logging, metadata persistence, cancellation. The fake agent is
//! fixtures/fake-agent/fake_claude.py (reached through the `CAD_TEST_CLAUDE`
//! seam, never the real locator, so no `--version` probe runs against it),
//! scenarios are the probe corpus, and each spawn's pid registry is pointed
//! inside the temp project so the shared default registry is never touched.

use std::path::{Path, PathBuf};
use std::time::Duration;

use engine::agent::claude::TurnRequest;
use engine::agent::session::{
    EngineEvent, Session, TurnOutcome, is_authentication_error, is_busy_session, is_missing_session,
};
use engine::agent::tool_summary::{ToolAction, ToolTitle};
use engine::project::store::{ProjectMetadata, load_metadata, save_metadata};
use engine::settings::Settings;

mod common;
use common::*;

/// The fake reads its env at spawn time and `Session::run_turn` snapshots the
/// login environment per turn, so every [set env → run_turn → turn finished]
/// window must be exclusive across the parallel test threads.
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const PONG_SID: &str = "94fcba3f-44f9-4003-b32d-c469499dc529";
const FORK_SID: &str = "283df95a-9a62-4620-9a04-73d1e3209968";
const WRITE_ID: &str = "toolu_013uZKwucpApefxH7dUCMw1L";
const READ_ID: &str = "toolu_01PiSki6C6xfd2exZn7nabus";

/// A temp project with the real on-disk layout: empty output/, no model.py.
fn mk_project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("cad-session-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    root
}

/// Point the CAD_* env at one fake invocation and run a whole turn to its
/// TurnFinished, returning every EngineEvent in order.
async fn run_fake(root: &Path, scenario: &Path, settings: &Settings) -> Vec<EngineEvent> {
    run_fake_with(root, scenario, settings, None).await
}

/// [`run_fake`] with an optional argv-dump file for the fake to append to.
async fn run_fake_with(
    root: &Path,
    scenario: &Path,
    settings: &Settings,
    argv_out: Option<&Path>,
) -> Vec<EngineEvent> {
    let guard = ENV_LOCK.lock().await;
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var("CAD_FAKE_SCENARIO", scenario);
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        match argv_out {
            Some(path) => std::env::set_var("CAD_FAKE_ARGV_OUT", path),
            None => std::env::remove_var("CAD_FAKE_ARGV_OUT"),
        }
        std::env::remove_var("CAD_FAKE_STDERR");
    }
    let mut session = Session::new(settings.clone()).with_system_template(system_template());
    let mut handle = session.run_turn(
        root,
        &TurnRequest {
            prompt: "please respond".into(),
            references: Vec::new(),
            images: Vec::new(),
        },
    );
    let events = collect_until_finished(&mut handle).await;
    drop(guard);
    events
}

/// Positions (indexes into `events`) of the events matching `pred`.
fn positions<F: Fn(&EngineEvent) -> bool>(events: &[EngineEvent], pred: F) -> Vec<usize> {
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| pred(event))
        .map(|(i, _)| i)
        .collect()
}

fn started_sid(event: &EngineEvent) -> Option<&str> {
    match event {
        EngineEvent::Started { session_id } => session_id.as_deref(),
        _ => None,
    }
}

fn finished<'a>(events: &'a [EngineEvent]) -> &'a TurnOutcome {
    match events.last() {
        Some(EngineEvent::TurnFinished(outcome)) => outcome,
        other => panic!("last event is TurnFinished, got {other:?}"),
    }
}

fn tool_finished<'a>(events: &'a [EngineEvent], id: &str) -> &'a EngineEvent {
    events
        .iter()
        .find(|event| matches!(event, EngineEvent::ToolFinished { id: got, .. } if got == id))
        .unwrap_or_else(|| panic!("no ToolFinished for {id}: {events:?}"))
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `argv` contains `flag` followed immediately by `value`.
fn has_pair(argv: &[String], flag: &str, value: &str) -> bool {
    argv.windows(2)
        .any(|pair| pair[0] == flag && pair[1] == value)
}

// ─── the seven-turn suite ───────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn pong_turn_emits_full_event_sequence() {
    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("scenarios/pong.jsonl"),
        &Settings::default(),
    )
    .await;

    // Started first, carrying the init session id
    assert_eq!(started_sid(&events[0]), Some(PONG_SID), "{events:?}");
    // thinking estimate flows through the thinking_tokens system events
    assert!(
        events.contains(&EngineEvent::Thinking {
            estimated_tokens: 50
        }),
        "{events:?}"
    );
    // streamed text accumulates, then the assistant block confirms it
    let streaming = positions(
        &events,
        |e| matches!(e, EngineEvent::TextStreaming { text } if text == "pong"),
    );
    let confirmed = positions(
        &events,
        |e| matches!(e, EngineEvent::TextConfirmed { text } if text == "pong"),
    );
    assert_eq!(streaming.len(), 1, "{events:?}");
    assert_eq!(confirmed.len(), 1, "{events:?}");
    assert!(streaming[0] < confirmed[0], "{events:?}");
    // no tool rows in a text-only turn; the rate_limit_event is consumed silently
    assert!(
        events.iter().all(|e| !matches!(
            e,
            EngineEvent::ToolStarted { .. } | EngineEvent::ToolFinished { .. }
        )),
        "{events:?}"
    );
    // Success carries the confirmed text (the hasText rule: the result text is
    // only used when no assistant text was confirmed this turn)
    assert_eq!(
        finished(&events),
        &TurnOutcome::Success {
            session_id: Some(PONG_SID.into()),
            text: "pong".into()
        }
    );
    // the session id from init is persisted immediately
    assert_eq!(
        load_metadata(&root).unwrap().session_id.as_deref(),
        Some(PONG_SID)
    );
    // one tee log with every raw stdout line: the verbatim corpus is 19 lines
    let logs_dir = root.join(".cadstudio/logs");
    let mut logs: Vec<PathBuf> = std::fs::read_dir(&logs_dir)
        .unwrap_or_else(|e| panic!("logs dir missing: {e}"))
        .flatten()
        .map(|entry| entry.path())
        .collect();
    logs.sort();
    assert_eq!(logs.len(), 1, "{logs:?}");
    let log = std::fs::read_to_string(&logs[0]).unwrap();
    assert_eq!(log.lines().count(), 19, "{log}");
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_turn_tracks_tool_rows() {
    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("scenarios/tool-use.jsonl"),
        &Settings::default(),
    )
    .await;

    // both tools open keyed by their tool_use ids, in stream order
    let started = positions(&events, |e| matches!(e, EngineEvent::ToolStarted { .. }));
    assert_eq!(started.len(), 2, "{events:?}");
    match (&events[started[0]], &events[started[1]]) {
        (
            EngineEvent::ToolStarted {
                id: a,
                name: na,
                title: ta,
            },
            EngineEvent::ToolStarted {
                id: b,
                name: nb,
                title: tb,
            },
        ) => {
            assert_eq!(a, WRITE_ID);
            assert_eq!(b, READ_ID);
            assert_eq!(na, "Write", "the tool_use block's name rides along");
            assert_eq!(nb, "Read");
            assert_eq!(
                ta,
                &ToolTitle::from(ToolAction::WriteFile),
                "live title at content_block_start"
            );
            assert_eq!(tb, &ToolTitle::from(ToolAction::ReadFile));
        }
        other => panic!("{other:?}"),
    }
    // both rows complete with their outputs, none in error
    match tool_finished(&events, WRITE_ID) {
        EngineEvent::ToolFinished {
            output, is_error, ..
        } => {
            assert!(output.contains("File created successfully"), "{output}");
            assert!(!is_error);
        }
        other => panic!("{other:?}"),
    }
    match tool_finished(&events, READ_ID) {
        EngineEvent::ToolFinished {
            output, is_error, ..
        } => {
            assert!(output.contains("hi"), "{output}");
            assert!(!is_error);
        }
        other => panic!("{other:?}"),
    }
    // the interleaved mid-stream tool_result completes the Write row while
    // the Read row is still streaming its input
    let write_done = positions(
        &events,
        |e| matches!(e, EngineEvent::ToolFinished { id, .. } if id == WRITE_ID),
    )[0];
    let read_inputs = positions(
        &events,
        |e| matches!(e, EngineEvent::ToolInput { id, .. } if id == READ_ID),
    );
    assert!(!read_inputs.is_empty(), "{events:?}");
    assert!(
        read_inputs[0] < write_done && *read_inputs.last().unwrap() > write_done,
        "Write finishes between Read's first and last input delta: {events:?}"
    );
    // the turn succeeds and the outcome text is the confirmed assistant text
    match finished(&events) {
        TurnOutcome::Success { text, .. } => assert!(text.contains("I created"), "{text}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn error_result_fails_turn() {
    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("scenarios/error-result.jsonl"),
        &Settings::default(),
    )
    .await;

    // the synthetic error assistant block is a TextConfirmed like any block
    assert!(
        events.iter().any(
            |e| matches!(e, EngineEvent::TextConfirmed { text } if text.contains("selected model"))
        ),
        "{events:?}"
    );
    match finished(&events) {
        TurnOutcome::Failed {
            error,
            auth,
            exit_code,
        } => {
            assert!(!auth);
            assert_eq!(
                exit_code,
                &Some(0),
                "the child exited clean; the result failed"
            );
            assert!(error.contains("no-such-model"), "{error}");
            assert!(
                error.contains("404"),
                "the api error status rides along: {error}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_failure_marks_row_failed() {
    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("scenarios/tool-failure.jsonl"),
        &Settings::default(),
    )
    .await;

    match tool_finished(&events, "toolu_fake_0001") {
        EngineEvent::ToolFinished {
            output, is_error, ..
        } => {
            assert!(output.contains("Traceback"), "{output}");
            assert!(is_error);
        }
        other => panic!("{other:?}"),
    }
    // a failed tool does not fail the turn when the result line says success
    assert!(
        matches!(finished(&events), TurnOutcome::Success { .. }),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_events_do_not_break_turn() {
    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("scenarios/unknown-events.jsonl"),
        &Settings::default(),
    )
    .await;

    // this scenario has no init line, so no Started fires at all; the session
    // id is still learned from the result line
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, EngineEvent::Started { .. })),
        "{events:?}"
    );
    // no assistant text this turn, so the outcome text is the result text
    assert_eq!(
        finished(&events),
        &TurnOutcome::Success {
            session_id: Some("00000000-0000-0000-0000-00000000a006".into()),
            text: "skipped the unknowns".into(),
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn argv_dump_asserts_flags() {
    let root = mk_project();
    // a pre-existing session id makes this a resume turn
    let mut metadata = ProjectMetadata::new("test");
    metadata.session_id = Some("seed-sid".into());
    save_metadata(&root, &metadata).unwrap();

    let dump = root.join("argv-dump.jsonl");
    let events = run_fake_with(
        &root,
        &fixture_fake("scenarios/pong.jsonl"),
        &Settings::default(),
        Some(&dump),
    )
    .await;
    assert!(
        matches!(finished(&events), TurnOutcome::Success { .. }),
        "{events:?}"
    );

    let lines: Vec<String> = std::fs::read_to_string(&dump)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(lines.len(), 1, "one dump line per invocation: {lines:?}");
    let invocation: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let argv = invocation["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<String>>();
    // python's getcwd resolves symlinks (/var → /private/var on macOS), so
    // compare canonicalized paths
    let canonical_root = std::fs::canonicalize(&root).unwrap();
    assert_eq!(
        Path::new(invocation["cwd"].as_str().unwrap()),
        canonical_root,
        "cwd is the project root"
    );
    assert_eq!(argv[0], "-p");
    assert!(argv[1].starts_with("please respond"), "{}", argv[1]);
    assert!(has_pair(&argv, "--output-format", "stream-json"));
    assert!(argv.iter().any(|a| a == "--include-partial-messages"));
    assert!(has_pair(&argv, "--permission-mode", "acceptEdits"));
    // the rendered system prompt: every placeholder substituted for this
    // project (the directory name lands in {{PROJECT_NAME}}), nothing left
    let prompt_pos = argv
        .iter()
        .position(|a| a == "--append-system-prompt")
        .expect("append-system-prompt flag");
    let rendered = &argv[prompt_pos + 1];
    let project_name = root.file_name().unwrap().to_str().unwrap();
    assert!(
        rendered.contains(project_name),
        "project name substituted: {rendered}"
    );
    assert!(!rendered.contains("{{"), "no placeholder left: {rendered}");
    assert!(!rendered.is_empty());
    assert!(argv.iter().any(|a| a == "--allowedTools"));
    // isolation flags per the default settings
    assert!(argv.iter().any(|a| a == "--strict-mcp-config"));
    assert!(has_pair(&argv, "--setting-sources", "project,local"));
    // resume + fork because metadata carried a session id
    assert!(has_pair(&argv, "--resume", "seed-sid"));
    assert!(argv.iter().any(|a| a == "--fork-session"));

    // with isolation and streaming off, none of those flags survive
    let mut off = Settings::default();
    off.isolates_claude = false;
    off.streaming_output = false;
    std::fs::remove_file(&dump).unwrap();
    let events = run_fake_with(
        &root,
        &fixture_fake("scenarios/pong.jsonl"),
        &off,
        Some(&dump),
    )
    .await;
    assert!(matches!(finished(&events), TurnOutcome::Success { .. }));
    let plain: Vec<String> = std::fs::read_to_string(&dump)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(plain.len(), 1);
    let argv: Vec<String> = serde_json::from_str::<serde_json::Value>(&plain[0]).unwrap()["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(has_pair(&argv, "--output-format", "json"));
    assert!(!argv.iter().any(|a| a == "--strict-mcp-config"));
    assert!(!argv.iter().any(|a| a == "--setting-sources"));
    assert!(!argv.iter().any(|a| a == "--include-partial-messages"));
    // the stored session id still resumes
    assert!(has_pair(&argv, "--resume", PONG_SID));
}

#[tokio::test(flavor = "multi_thread")]
async fn forked_session_replaces_metadata_id() {
    let root = mk_project();
    let first = run_fake(
        &root,
        &fixture_fake("scenarios/pong.jsonl"),
        &Settings::default(),
    )
    .await;
    assert_eq!(
        load_metadata(&root).unwrap().session_id.as_deref(),
        Some(PONG_SID)
    );
    let _ = first;

    // the second turn resumes (metadata has a session id) and the forked
    // init announces a NEW session id, which replaces the stored one
    let second = run_fake(
        &root,
        &fixture_fake("05-resume-fork-isolated.jsonl"),
        &Settings::default(),
    )
    .await;
    assert_eq!(started_sid(&second[0]), Some(FORK_SID), "{second:?}");
    let stored = load_metadata(&root).unwrap().session_id;
    assert_eq!(stored.as_deref(), Some(FORK_SID));
    assert_ne!(stored.as_deref(), Some(PONG_SID));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_mid_stream() {
    let root = mk_project();
    let guard = ENV_LOCK.lock().await;
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var(
            "CAD_FAKE_SCENARIO",
            fixture_fake("scenarios/cancel-mid-stream.jsonl"),
        );
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::remove_var("CAD_FAKE_ARGV_OUT");
        std::env::remove_var("CAD_FAKE_STDERR");
    }
    let mut session = Session::new(Settings::default()).with_system_template(system_template());
    let mut handle = session.run_turn(
        &root,
        &TurnRequest {
            prompt: "slow please".into(),
            references: Vec::new(),
            images: Vec::new(),
        },
    );

    // the init line arrives before the scenario's 5 s sleep, so Started is
    // observable while the process is mid-stream
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut before_cancel = Vec::new();
    loop {
        match tokio::time::timeout_at(deadline, handle.events.recv()).await {
            Ok(Some(event)) if !matches!(event, EngineEvent::Started { .. }) => {
                before_cancel.push(event)
            }
            Ok(Some(event)) => {
                before_cancel.push(event);
                break;
            }
            other => panic!("Started never arrived: {other:?}, {before_cancel:?}"),
        }
    }
    assert_eq!(
        started_sid(&before_cancel.last().unwrap()),
        Some("00000000-0000-0000-0000-00000000a002")
    );

    handle.cancel();
    // the interrupted turn ends as Cancelled well inside the fake's sleep
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
            Err(_) => panic!("cancel did not end the turn within 2 s: {rest:?}"),
        }
    }
    assert_eq!(
        rest.last(),
        Some(&EngineEvent::TurnFinished(TurnOutcome::Cancelled))
    );
    // the post-sleep text never streamed
    assert!(
        !rest
            .iter()
            .any(|e| matches!(e, EngineEvent::TextStreaming { .. })),
        "{rest:?}"
    );
    drop(guard);

    // the child is really gone
    let pid = handle.pid().expect("pid published after spawn");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while alive(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "child {pid} survived cancellation"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The app-wide venv interpreter the session names everywhere: the
/// restricted-bash allowlist entry and the `{{PYTHON}}` substitution (the
/// rendered system prompt) both carry python::env's canonical venv —
/// `~/.local/share/cad-studio/venv/bin/python` on Linux, the venv M5
/// creates there — and the child env carries ModelBuilder.toolEnvironment's
/// pair over the login env. A spaced canonical path (macOS data dir) is
/// quoted inside the entry and the prompt lines.
#[tokio::test(flavor = "multi_thread")]
async fn restricted_allowlist_and_child_env_name_the_app_venv() {
    let root = mk_project();
    let settings = Settings {
        restricted_bash: true,
        ..Default::default()
    };
    let dump = root.join("argv-dump.jsonl");
    let events = run_fake_with(
        &root,
        &fixture_fake("scenarios/pong.jsonl"),
        &settings,
        Some(&dump),
    )
    .await;
    assert!(
        matches!(finished(&events), TurnOutcome::Success { .. }),
        "{events:?}"
    );

    let line = std::fs::read_to_string(&dump)
        .unwrap()
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("one dump line")
        .to_string();
    let invocation: serde_json::Value = serde_json::from_str(&line).unwrap();
    let argv = invocation["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<String>>();

    let venv = engine::python::env::interpreter_path(&engine::python::env::venv_dir())
        .to_string_lossy()
        .into_owned();
    // the entry quotes a spaced path (claude.rs quoted_python), so the
    // claim is "names the canonical venv", not byte-equality with it
    assert!(
        argv.iter()
            .any(|a| a.starts_with("Bash(") && a.ends_with(":*)") && a.contains(&venv)),
        "restricted entry names the app-wide venv: {argv:?}"
    );
    let prompt_pos = argv
        .iter()
        .position(|a| a == "--append-system-prompt")
        .expect("append-system-prompt flag");
    assert!(
        argv[prompt_pos + 1].contains(&venv),
        "{{PYTHON}} renders the app-wide venv: {}",
        argv[prompt_pos + 1]
    );
    assert_eq!(invocation["env"]["MPLBACKEND"], "Agg", "{line}");
    assert_eq!(invocation["env"]["PYTHONUNBUFFERED"], "1", "{line}");
}

/// The session-level half of the versions rule: prompt_ctx feeds the shared
/// versions cell into the python line and the {{ENVIRONMENT}} bullets, so
/// the rendered system prompt a real turn hands the agent (the
/// `--append-system-prompt` argv) carries the fallback until a verification
/// lands, the verified line after.
#[tokio::test(flavor = "multi_thread")]
async fn rendered_prompt_carries_versions_line() {
    let root = mk_project();
    let settings = Settings::default();
    let dump = root.join("argv-dump.jsonl");

    engine::python::env::set_shared_versions(None);
    let events = run_fake_with(
        &root,
        &fixture_fake("scenarios/pong.jsonl"),
        &settings,
        Some(&dump),
    )
    .await;
    assert!(
        matches!(finished(&events), TurnOutcome::Success { .. }),
        "{events:?}"
    );
    let prompt = append_system_prompt(&dump);
    assert!(
        prompt.contains("- Python packages: build123d, matplotlib, numpy"),
        "fallback line in rendered prompt: {prompt}"
    );

    engine::python::env::set_shared_versions(Some("Python 3.12.4, build123d 0.8.1".into()));
    let events = run_fake_with(
        &root,
        &fixture_fake("scenarios/pong.jsonl"),
        &settings,
        Some(&dump),
    )
    .await;
    assert!(
        matches!(finished(&events), TurnOutcome::Success { .. }),
        "{events:?}"
    );
    let prompt = append_system_prompt(&dump);
    assert!(
        prompt.contains("- Python packages: Python 3.12.4, build123d 0.8.1"),
        "verified line in rendered prompt: {prompt}"
    );
    engine::python::env::set_shared_versions(None);
}

/// The `--append-system-prompt` value of the LAST invocation in an argv dump.
fn append_system_prompt(dump: &Path) -> String {
    let line = std::fs::read_to_string(dump)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .expect("one dump line")
        .to_string();
    let invocation: serde_json::Value = serde_json::from_str(&line).unwrap();
    let argv = invocation["argv"].as_array().unwrap();
    let pos = argv
        .iter()
        .position(|v| v.as_str() == Some("--append-system-prompt"))
        .expect("append-system-prompt flag");
    argv[pos + 1].as_str().unwrap().to_string()
}

// ─── classification surface for the lifecycle dance (task 8) ────────────────

#[test]
fn classification_helpers_are_case_insensitive() {
    assert!(is_busy_session(
        "Session X ALREADY HAS AN ACTIVE WRITER today"
    ));
    assert!(!is_busy_session("a quiet turn"));
    for missing in [
        "No conversation found with session ID: 1",
        "-->> NO ROLLOUT FOUND <<--",
        "Session Not Found",
        "thread not found.",
    ] {
        assert!(is_missing_session(missing), "{missing}");
    }
    assert!(!is_missing_session("a present session"));
    for auth in [
        "Invalid API key",
        "Please run /login",
        "Not logged in",
        "authentication required",
        "OAuth token expired",
        "Unauthorized",
        "codex login needed",
        "returned 401",
    ] {
        assert!(is_authentication_error(auth), "{auth}");
    }
    assert!(!is_authentication_error("model not found 404"));
}

// ─── shell parity extensions (M4 task 0) ─────────────────────────────────────

/// A thinking block opens a boundary pair at its `content_block_start` and
/// closes it at the matching `content_block_stop` (upstream
/// `startBlock`/`stopBlock`, AgentSession.swift:147-174): the open interval
/// carries the token estimates and ends before the message's text block
/// starts streaming. A `redacted_thinking` block opens the same pair; a
/// stream with no stop for it leaves the boundary open at turn end.
#[tokio::test(flavor = "multi_thread")]
async fn thinking_boundaries_emitted() {
    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("02-minimal-session.jsonl"),
        &Settings::default(),
    )
    .await;

    let started = positions(&events, |e| matches!(e, EngineEvent::ThinkingStarted));
    let finished_at = positions(&events, |e| matches!(e, EngineEvent::ThinkingFinished));
    assert_eq!(
        started.len(),
        1,
        "the thinking block opens one pair: {events:?}"
    );
    assert_eq!(finished_at.len(), 1, "{events:?}");
    let first_text = positions(&events, |e| matches!(e, EngineEvent::TextStreaming { .. }));
    assert!(
        started[0] < finished_at[0] && finished_at[0] < first_text[0],
        "start → stop, closed before the text block streams: {events:?}"
    );
    assert!(
        events[started[0]..finished_at[0]]
            .iter()
            .any(|e| matches!(e, EngineEvent::Thinking { .. })),
        "the estimates ride inside the open interval: {events:?}"
    );

    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("scenarios/redacted-thinking.jsonl"),
        &Settings::default(),
    )
    .await;
    assert_eq!(
        positions(&events, |e| matches!(e, EngineEvent::ThinkingStarted)).len(),
        1,
        "redacted_thinking opens the same pair: {events:?}"
    );
    assert_eq!(
        positions(&events, |e| matches!(e, EngineEvent::ThinkingFinished)).len(),
        0,
        "no content_block_stop in this scenario: {events:?}"
    );
    assert!(
        matches!(finished(&events), TurnOutcome::Success { .. }),
        "{events:?}"
    );
}

/// Both tool rows carry the tool_use block's name alongside the title — at
/// the stream announce and at the assistant snapshot that opens an
/// unannounced row.
#[tokio::test(flavor = "multi_thread")]
async fn tool_started_carries_name() {
    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("scenarios/tool-use.jsonl"),
        &Settings::default(),
    )
    .await;

    let started = positions(&events, |e| matches!(e, EngineEvent::ToolStarted { .. }));
    assert_eq!(started.len(), 2, "{events:?}");
    for (at, want) in [(started[0], "Write"), (started[1], "Read")] {
        match &events[at] {
            EngineEvent::ToolStarted { name, .. } => assert_eq!(name, want, "{events:?}"),
            other => panic!("{other:?}"),
        }
    }

    // the second emit site: a row the stream never announced opens at an
    // assistant snapshot, name included
    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("scenarios/tool-failure.jsonl"),
        &Settings::default(),
    )
    .await;
    match events
        .iter()
        .find(|e| matches!(e, EngineEvent::ToolStarted { .. }))
    {
        Some(EngineEvent::ToolStarted { id, name, .. }) => {
            assert_eq!(id, "toolu_fake_0001");
            assert_eq!(name, "Bash");
        }
        other => panic!("{other:?}"),
    }
}

/// A nonzero child exit rides the Failed outcome (upstream's "(code N)"
/// failure row), next to the result text that explains it.
#[tokio::test(flavor = "multi_thread")]
async fn failed_carries_exit_code() {
    let root = mk_project();
    let events = run_fake(
        &root,
        &fixture_fake("scenarios/exit-code-failure.jsonl"),
        &Settings::default(),
    )
    .await;
    match finished(&events) {
        TurnOutcome::Failed {
            error,
            auth,
            exit_code,
        } => {
            assert!(!auth, "{error}");
            assert_eq!(exit_code, &Some(3), "{error}");
            assert!(error.contains("the agent fell over"), "{error}");
        }
        other => panic!("{other:?}"),
    }
}

/// The bare "401" needle matches only as a whole word: token counts and
/// latencies that embed the digits are not an auth status (the other needles
/// stay plain case-insensitive substrings).
#[test]
fn auth_401_word_boundary_engine() {
    assert!(!is_authentication_error("1401 tokens used"));
    assert!(!is_authentication_error("duration 4.401s"));
    assert!(!is_authentication_error("failed with 4013"));
    assert!(is_authentication_error("api error status 401"));
    assert!(is_authentication_error("returned (401)"));
}

/// Runs `turns` sequential turns on ONE session against the fake at
/// `fake`, calling `between(i)` before turn `i` — the mid-session seam:
/// the executable is resolved per turn, so what `between` does to the
/// file decides the next turn.
async fn run_turns_on_one_session(
    root: &Path,
    fake: &Path,
    turns: usize,
    mut between: impl FnMut(usize),
) -> Vec<Vec<EngineEvent>> {
    let guard = ENV_LOCK.lock().await;
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fake);
        std::env::set_var("CAD_FAKE_SCENARIO", fixture_fake("scenarios/pong.jsonl"));
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::remove_var("CAD_FAKE_ARGV_OUT");
        std::env::remove_var("CAD_FAKE_STDERR");
    }
    let mut session = Session::new(Settings::default()).with_system_template(system_template());
    let mut all = Vec::new();
    for i in 0..turns {
        between(i);
        let mut handle = session.run_turn(
            root,
            &TurnRequest {
                prompt: "please respond".into(),
                references: Vec::new(),
                images: Vec::new(),
            },
        );
        all.push(collect_until_finished(&mut handle).await);
    }
    drop(guard);
    all
}

/// An executable that vanishes between turns ends the next turn with the
/// typed AgentMissing marker before anything spawns (upstream's per-turn
/// `locator.resolve()` guard, AgentSession.swift:40-50) — no Started, no
/// free-text failure for the app to show in English.
#[tokio::test(flavor = "multi_thread")]
async fn agent_vanishing_mid_session_ends_turn_agent_missing() {
    let root = mk_project();
    let fake = root.join("fake_claude.py");
    std::fs::copy(fixture_fake("fake_claude.py"), &fake).unwrap();
    let turns = run_turns_on_one_session(&root, &fake, 2, |i| {
        if i == 1 {
            std::fs::remove_file(&fake).unwrap();
        }
    })
    .await;
    assert!(
        matches!(finished(&turns[0]), TurnOutcome::Success { .. }),
        "{:?}",
        turns[0]
    );
    assert_eq!(
        turns[1],
        vec![EngineEvent::TurnFinished(TurnOutcome::AgentMissing)],
        "nothing but the marker: the turn never spawned"
    );
}

/// An executable that resolves but cannot be exec'd ends StartFailed with
/// the OS error as its detail (upstream's catch, AgentSession.swift:98-100),
/// not a generic Failed.
#[tokio::test(flavor = "multi_thread")]
async fn unspawnable_agent_ends_turn_start_failed() {
    use std::os::unix::fs::PermissionsExt;
    let root = mk_project();
    let fake = root.join("broken-agent");
    std::fs::write(&fake, "#!/nonexistent/interpreter\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let turns = run_turns_on_one_session(&root, &fake, 1, |_| {}).await;
    match finished(&turns[0]) {
        TurnOutcome::StartFailed { error } => assert!(!error.is_empty()),
        other => panic!("expected StartFailed, got {other:?}"),
    }
}
