//! Codex-runner integration tests (M8 Task 3): one fake-agent turn per
//! test, driven end to end through `Session::run_turn` — engine dispatch,
//! the codex argv, per-line translation onto the SAME StreamEvent pipeline
//! claude uses, metadata persistence, and the mid-conversation engine flip
//! (Ruling 1: the engine is re-read per turn; the stored session id is
//! engine-blind). The fake codex is fixtures/fake-agent/fake_codex.py
//! (reached through the `CAD_TEST_CLAUDE` seam, never the locator, so no
//! `--version` probe runs against it); scenarios are recorded codex-JSON
//! lines.

use std::path::{Path, PathBuf};
use std::time::Duration;

use engine::agent::claude::TurnRequest;
use engine::agent::session::{EngineEvent, Session, TurnOutcome};
use engine::agent::tool_summary::{ToolAction, ToolTitle};
use engine::project::store::load_metadata;
use engine::settings::Settings;

/// The fake reads its env at spawn time and `Session::run_turn` snapshots
/// the login environment per turn, so every [set env → run_turn → turn
/// finished] window must be exclusive across the parallel test threads.
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const PONG_SID: &str = "94fcba3f-44f9-4003-b32d-c469499dc529";

fn fixture_fake(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/fake-agent")
        .join(p)
}

fn fixture_upstream(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/upstream")
        .join(p)
}

/// A temp project with the real on-disk layout: empty output/, no model.py.
fn mk_project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("cad-codex-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    root
}

fn system_template() -> String {
    std::fs::read_to_string(fixture_upstream("CADSystemPrompt.md")).unwrap()
}

fn codex_request() -> TurnRequest {
    TurnRequest {
        prompt: "please respond".into(),
        references: Vec::new(),
        images: Vec::new(),
    }
}

/// Point the CAD_* env at one fake invocation (fake_codex.py unless
/// overridden), then run a whole turn to its TurnFinished, returning every
/// EngineEvent in order. `settings_file` attaches the per-turn re-read seam
/// (None = the session's startup snapshot decides the engine).
#[allow(clippy::too_many_arguments)]
async fn run_turn(
    root: &Path,
    scenario: &Path,
    settings: &Settings,
    settings_file: Option<&Path>,
    argv_out: Option<&Path>,
    fake: Option<&Path>,
    request: &TurnRequest,
) -> Vec<EngineEvent> {
    let guard = ENV_LOCK.lock().await;
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var(
            "CAD_TEST_CLAUDE",
            fake.unwrap_or(&fixture_fake("fake_codex.py")),
        );
        std::env::set_var("CAD_FAKE_SCENARIO", scenario);
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        match argv_out {
            Some(path) => std::env::set_var("CAD_FAKE_ARGV_OUT", path),
            None => std::env::remove_var("CAD_FAKE_ARGV_OUT"),
        }
        std::env::remove_var("CAD_FAKE_STDERR");
    }
    let mut session = Session::new(settings.clone()).with_system_template(system_template());
    if let Some(file) = settings_file {
        session = session.with_settings_file(file.to_path_buf());
    }
    let mut handle = session.run_turn(root, request);
    let events = collect_until_finished(&mut handle).await;
    drop(guard);
    events
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

fn tool<'a>(events: &'a [EngineEvent], id: &str) -> &'a EngineEvent {
    events
        .iter()
        .find(|event| matches!(event, EngineEvent::ToolStarted { id: got, .. } if got == id))
        .unwrap_or_else(|| panic!("no ToolStarted for {id}: {events:?}"))
}

/// The argv of dump line `n` (0-based) of an argv dump file.
fn argv_of(dump: &Path, n: usize) -> Vec<String> {
    let lines: Vec<String> = std::fs::read_to_string(dump)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    let invocation: serde_json::Value =
        serde_json::from_str(lines.get(n).unwrap_or_else(|| panic!("dump line {n}: {lines:?}")))
            .unwrap();
    invocation["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// `argv` contains `flag` followed immediately by `value`.
fn has_pair(argv: &[String], flag: &str, value: &str) -> bool {
    argv.windows(2).any(|pair| pair[0] == flag && pair[1] == value)
}

fn codex_settings() -> Settings {
    Settings {
        agent_engine: "codex".into(),
        ..Default::default()
    }
}

// ─── the full codex turn ─────────────────────────────────────────────────────

/// One scripted codex turn: thread started → thinking pair → Bash tool row
/// (command unwrapped) → file_change Write pair → agent message →
/// completed. Every row the claude pipeline produces has its codex twin.
#[tokio::test(flavor = "multi_thread")]
async fn codex_turn_translates_full_sequence() {
    let root = mk_project();
    let events = run_turn(
        &root,
        &fixture_fake("scenarios/codex-turn.jsonl"),
        &codex_settings(),
        None,
        None,
        None,
        &codex_request(),
    )
    .await;

    // thread.started is the Started event; the thread id persists as the
    // engine-blind session id
    assert_eq!(started_sid(&events[0]), Some("th_codex_1"), "{events:?}");
    assert_eq!(
        load_metadata(&root).unwrap().session_id.as_deref(),
        Some("th_codex_1")
    );

    // reasoning opens and closes a thinking pair
    let thinking = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::ThinkingStarted | EngineEvent::ThinkingFinished))
        .count();
    assert_eq!(thinking, 2, "one start/stop pair: {events:?}");

    // the command_execution row: Bash, title from the unwrapped command
    match tool(&events, "cmd_1") {
        EngineEvent::ToolStarted {
            name, title, ..
        } => {
            assert_eq!(name, "Bash");
            assert_eq!(
                title,
                &ToolTitle::from(ToolAction::BuildModel),
                "unwrapped `python model.py`"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        events.iter().find(|e| matches!(e, EngineEvent::ToolFinished { id, .. } if id == "cmd_1")),
        Some(EngineEvent::ToolFinished { output, is_error, .. })
            if output == "built" && !is_error
    ));

    // the file_change synthesized pair: a Write row for model.py, closed
    // immediately, not failed
    match tool(&events, "fc_1-0") {
        EngineEvent::ToolStarted { name, .. } => assert_eq!(name, "Write"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        events.iter().find(|e| matches!(e, EngineEvent::ToolFinished { id, .. } if id == "fc_1-0")),
        Some(EngineEvent::ToolFinished { is_error: false, .. })
    ));

    // the agent message confirms as assistant text and becomes the
    // outcome's text (the hasText rule)
    assert!(events.contains(&EngineEvent::TextConfirmed {
        text: "I created model.py.".into()
    }));
    assert_eq!(
        finished(&events),
        &TurnOutcome::Success {
            session_id: Some("th_codex_1".into()),
            text: "I created model.py.".into(),
        }
    );

    // the tee log carries every raw codex line (8 in the scenario)
    let logs_dir = root.join(".cadstudio/logs");
    let log: Vec<String> = std::fs::read_dir(&logs_dir)
        .expect("logs dir")
        .flatten()
        .flat_map(|entry| std::fs::read_to_string(entry.path()))
        .collect();
    assert_eq!(log.len(), 1, "one log per turn");
    assert_eq!(log[0].lines().count(), 8, "{}", log[0]);
}

/// The codex argv through a real spawn (CodexRunner.swift:19-40): the exec
/// block with `-C <root>`, both `-c` overrides with the developer
/// instructions TOML-escaped, the isolation flag, `resume` while a session
/// id is stored, `-i` per IMAGE attachment only, and the prompt last.
#[tokio::test(flavor = "multi_thread")]
async fn codex_argv_carries_upstream_flags() {
    let root = mk_project();
    // a stored session id makes this a resume turn
    let mut metadata = engine::project::store::ProjectMetadata::new("test");
    metadata.session_id = Some("th_prev".into());
    engine::project::store::save_metadata(&root, &metadata).unwrap();

    let dump = root.join("argv-dump.jsonl");
    let request = TurnRequest {
        prompt: "please respond".into(),
        references: Vec::new(),
        images: vec![
            PathBuf::from("/references/1-shot.png"),
            PathBuf::from("/references/2-notes.txt"),
        ],
    };
    let events = run_turn(
        &root,
        &fixture_fake("scenarios/codex-turn.jsonl"),
        &codex_settings(),
        None,
        Some(&dump),
        None,
        &request,
    )
    .await;
    assert!(
        matches!(finished(&events), TurnOutcome::Success { .. }),
        "{events:?}"
    );

    let argv = argv_of(&dump, 0);
    assert_eq!(argv[0], "exec");
    assert!(argv.iter().any(|a| a == "--json"));
    assert!(argv.iter().any(|a| a == "--skip-git-repo-check"));
    assert!(has_pair(&argv, "--sandbox", "workspace-write"));
    // -C names the project root as the session resolved it (the raw path;
    // the fake's own cwd would be the symlink-resolved twin)
    let c_at = argv
        .iter()
        .position(|a| a == "-C")
        .expect("-C flag in argv");
    assert_eq!(Path::new(&argv[c_at + 1]), root, "{argv:?}");
    assert!(
        argv.iter().any(|a| a == "approval_policy=\"never\""),
        "the approval override carries its quotes: {argv:?}"
    );
    let dev_at = argv
        .iter()
        .position(|a| a.starts_with("developer_instructions="))
        .expect("developer_instructions override");
    let value = &argv[dev_at]["developer_instructions=".len()..];
    assert!(
        value.starts_with('"') && value.ends_with('"'),
        "TOML basic string: {value:?}"
    );
    assert!(
        !value.contains('\n') && value.contains("\\n"),
        "the multi-line system prompt rides escaped: {value:?}"
    );
    assert!(
        argv.iter().any(|a| a == "--ignore-user-config"),
        "default isolation"
    );
    assert!(has_pair(&argv, "resume", "th_prev"), "{argv:?}");
    // only the image rides -i; the txt attachment does not
    assert!(has_pair(&argv, "-i", "/references/1-shot.png"));
    assert!(!argv.windows(2).any(|w| w[1].contains("notes.txt")), "{argv:?}");
    // the prompt is the LAST argument
    assert_eq!(argv.last().map(String::as_str), Some("please respond"));
    // and none of claude's flags leak into a codex turn
    for claude_only in [
        "-p",
        "--append-system-prompt",
        "--permission-mode",
        "--allowedTools",
        "--fork-session",
    ] {
        assert!(!argv.contains(&claude_only.to_string()), "{argv:?}");
    }
}

/// Ruling 1 / plan Review Focus 2: flipping the engine setting between
/// turns switches the runner mid-conversation — turn 2 spawns the CODEX
/// argv while resuming the session id turn 1's CLAUDE run stored (the sid
/// is engine-blind), and the codex lines translate through the codex
/// translator, not the claude one.
#[tokio::test(flavor = "multi_thread")]
async fn engine_flip_mid_conversation_resumes_engine_blind() {
    let root = mk_project();
    let settings_file = root.join("settings.json");
    Settings::default().save(&settings_file).unwrap();
    let dump = root.join("argv-dump.jsonl");

    // turn 1: claude (pong) — the session re-reads the settings file per
    // turn, so the file's engine decides
    let first = run_turn(
        &root,
        &fixture_fake("scenarios/pong.jsonl"),
        &Settings::default(),
        Some(&settings_file),
        Some(&dump),
        Some(&fixture_fake("fake_claude.py")),
        &codex_request(),
    )
    .await;
    assert_eq!(started_sid(&first[0]), Some(PONG_SID), "{first:?}");
    assert!(matches!(finished(&first), TurnOutcome::Success { .. }));

    // flip the setting on disk mid-conversation: run_turn builds a
    // fresh Session per turn (what persists is root/metadata + the
    // settings file), so the flip e2e proves the RE-READ is decisive
    let mut codex_settings = Settings::default();
    codex_settings.agent_engine = "codex".into();
    codex_settings.save(&settings_file).unwrap();

    // turn 2: codex, resuming the stored (claude-written) session id
    let second = run_turn(
        &root,
        &fixture_fake("scenarios/codex-turn.jsonl"),
        &Settings::default(),
        Some(&settings_file),
        Some(&dump),
        None,
        &codex_request(),
    )
    .await;
    assert_eq!(
        started_sid(&second[0]),
        Some("th_codex_1"),
        "codex lines translated: {second:?}"
    );
    assert!(matches!(
        finished(&second),
        TurnOutcome::Success { session_id: Some(sid), .. } if sid == "th_codex_1"
    ));
    assert_eq!(
        load_metadata(&root).unwrap().session_id.as_deref(),
        Some("th_codex_1"),
        "the codex thread id replaces the stored one"
    );

    // the two spawns: claude argv first (fresh — no sid stored yet), codex
    // argv second, resuming the sid turn 1 stored (engine-blind)
    let claude_argv = argv_of(&dump, 0);
    assert!(claude_argv[0] == "-p", "turn 1 is a claude spawn: {claude_argv:?}");
    assert!(!claude_argv.contains(&"--resume".to_string()));
    let codex_argv = argv_of(&dump, 1);
    assert_eq!(codex_argv[0], "exec", "turn 2 is a codex spawn: {codex_argv:?}");
    assert!(
        has_pair(&codex_argv, "resume", PONG_SID),
        "the codex run resumes the claude-stored sid: {codex_argv:?}"
    );
    assert!(!codex_argv.contains(&"--fork-session".to_string()));
}

/// turn.failed after a top-level error line finishes the turn errored with
/// the STASHED message (CodexRunner.swift:86-91's fallback chain), the
/// outcome carrying the child's clean exit.
#[tokio::test(flavor = "multi_thread")]
async fn codex_turn_failed_reports_stashed_error_message() {
    let root = mk_project();
    let events = run_turn(
        &root,
        &fixture_fake("scenarios/codex-failed.jsonl"),
        &codex_settings(),
        None,
        None,
        None,
        &codex_request(),
    )
    .await;
    assert_eq!(started_sid(&events[0]), Some("th_codex_2"), "{events:?}");
    match finished(&events) {
        TurnOutcome::Failed {
            error,
            auth,
            exit_code,
        } => {
            assert_eq!(error, "experimental websocket", "{error}");
            assert!(!auth);
            assert_eq!(exit_code, &Some(0), "the child exited clean");
        }
        other => panic!("{other:?}"),
    }
}
