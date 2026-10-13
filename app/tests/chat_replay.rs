//! Chat reducer replay: fake-agent scenarios driven end to end through
//! `Session::run_turn`, every [`EngineEvent`] folded into [`app::chat::
//! ChatState`] on the wall clock, the final transcript asserted. This is the
//! seam test the unit suite cannot be — the real event ORDER the engine
//! emits (thinking boundaries around text, interleaved tool results, the
//! outcome shapes) reaching the rows upstream's chat shows. The harness is
//! the engine tests' ENV_LOCK + bounded-poll pattern (engine/tests/
//! session_turn.rs): the fake reads its env at spawn time, so every
//! [set env → run_turn → TurnFinished] window is exclusive across this
//! binary's parallel tests.

use std::path::{Path, PathBuf};
use std::time::Duration;

use app::chat::ChatState;
use engine::agent::claude::TurnRequest;
use engine::agent::session::{EngineEvent, Session};
use engine::project::store::Role;
use engine::settings::Settings;

/// The fake reads its env at spawn time and the session snapshots the login
/// environment per turn, so every [set env → run_turn → TurnFinished] window
/// must be exclusive across the parallel test threads.
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn fixture_fake(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/fake-agent")
        .join(p)
}

/// A temp project with the on-disk layout the session expects.
fn mk_project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("cad-chat-replay-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    root
}

/// Point the CAD_* env at one fake invocation, run a whole turn to its
/// TurnFinished (bounded, never a fixed sleep), and fold every event into a
/// fresh ChatState on the wall clock. The system template stays empty: the
/// replay asserts transcript reduction, not argv.
async fn replay(root: &Path, scenario: &Path, stderr: Option<&Path>) -> ChatState {
    let guard = ENV_LOCK.lock().await;
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var("CAD_FAKE_SCENARIO", scenario);
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::remove_var("CAD_FAKE_ARGV_OUT");
        match stderr {
            Some(path) => std::env::set_var("CAD_FAKE_STDERR", path),
            None => std::env::remove_var("CAD_FAKE_STDERR"),
        }
    }
    let mut session = Session::new(Settings::default());
    let mut handle = session.run_turn(
        root,
        &TurnRequest {
            prompt: "please respond".into(),
            attachments: Vec::new(),
        },
    );
    let mut state = ChatState::new(Vec::new());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, handle.events.recv()).await {
            Ok(Some(event)) => {
                let finished = matches!(event, EngineEvent::TurnFinished(_));
                state.apply(&event);
                if finished {
                    drop(guard);
                    return state;
                }
            }
            Ok(None) => panic!("events channel closed before TurnFinished"),
            Err(_) => panic!("timed out before TurnFinished"),
        }
    }
}

/// The thinking row's duration text — the wall-clock seconds the fake's
/// instant replay collapses to, so only the shape is asserted.
fn thought_for_seconds(text: &str) -> i64 {
    let seconds = text
        .strip_prefix("Thought for ")
        .and_then(|rest| rest.strip_suffix(" s"))
        .unwrap_or_else(|| panic!("not a duration row: {text}"));
    seconds.parse().expect("integral seconds")
}

// ─── scenarios ───────────────────────────────────────────────────────────────

/// The full success shape: thinking row timed by its block boundaries, two
/// tool rows completed with titles finalized over their streamed input, and
/// the streamed-then-confirmed assistant row — exactly once.
#[tokio::test(flavor = "multi_thread")]
async fn tool_use_turn_builds_the_full_transcript() {
    let root = mk_project();
    let state = replay(&root, &fixture_fake("scenarios/tool-use.jsonl"), None).await;

    let rows: Vec<(Role, &str)> = state
        .messages
        .iter()
        .map(|m| (m.role, m.text.as_str()))
        .collect();
    assert_eq!(rows.len(), 4, "{:?}", state.messages);
    assert_eq!(rows[0].0, Role::Thinking);
    assert!(thought_for_seconds(rows[0].1) >= 1, "{}", rows[0].1);

    // the Write row: title finalized over the full streamed input, the live
    // preview gone at completion (block-stop parity), output and success
    // recorded, name preserved
    let write = state.messages[1].tool.as_ref().unwrap();
    assert_eq!(state.messages[1].text, "Preparing a helper file · 2 lines");
    assert_eq!(write.name, "Write");
    assert_eq!(write.preview, None);
    assert!(
        write
            .output
            .as_ref()
            .unwrap()
            .contains("File created successfully")
    );
    assert_eq!(write.state, engine::project::store::ToolState::Succeeded);

    // the Read row, completed while it streamed and after its result landed
    let read = state.messages[2].tool.as_ref().unwrap();
    assert_eq!(state.messages[2].text, "Reading a file");
    assert_eq!(read.name, "Read");
    assert_eq!(read.preview, None);
    assert!(read.output.as_ref().unwrap().contains("hi"));
    assert_eq!(read.state, engine::project::store::ToolState::Succeeded);

    // the assistant text: streamed snapshots SET one row, the confirm
    // rewrote it in place, the Success outcome did not duplicate it
    assert_eq!(rows[3].0, Role::Assistant);
    assert!(rows[3].1.starts_with("I created `/tmp/"), "{}", rows[3].1);
    assert!(state.turn_had_confirmed_text);
    assert!(state.unconfirmed.is_empty());
}

/// An auth-classified failure produces exactly the sign-in system row with
/// the open-terminal action.
#[tokio::test(flavor = "multi_thread")]
async fn auth_error_rows_open_terminal() {
    let root = mk_project();
    let state = replay(
        &root,
        &fixture_fake("scenarios/auth-error.jsonl"),
        Some(&fixture_fake("scenarios/auth-error.stderr")),
    )
    .await;

    assert_eq!(state.messages.len(), 1, "{:?}", state.messages);
    let row = &state.messages[0];
    assert_eq!(row.role, Role::System);
    assert_eq!(
        row.text,
        "Claude Code isn't signed in. Open Terminal and run `claude` to sign in."
    );
    assert_eq!(
        row.action,
        Some(engine::project::store::Action::OpenTerminal)
    );
    assert!(row.detail.as_deref().unwrap().contains("Unauthorized"));
}

/// A nonzero child exit names its code in the failure row.
#[tokio::test(flavor = "multi_thread")]
async fn exit_code_failure_names_code() {
    let root = mk_project();
    let state = replay(
        &root,
        &fixture_fake("scenarios/exit-code-failure.jsonl"),
        None,
    )
    .await;

    assert_eq!(state.messages.len(), 1, "{:?}", state.messages);
    let row = &state.messages[0];
    assert_eq!(row.role, Role::System);
    assert_eq!(row.text, "Claude Code ended with an error (code 3).");
    assert!(
        row.detail
            .as_deref()
            .unwrap()
            .contains("the agent fell over")
    );
    assert_eq!(row.action, None);
}

/// A clean-exit error result: the synthetic error text block confirms as an
/// assistant row and the turn fails with "(code 0)".
#[tokio::test(flavor = "multi_thread")]
async fn error_result_confirms_text_and_fails() {
    let root = mk_project();
    let state = replay(&root, &fixture_fake("scenarios/error-result.jsonl"), None).await;

    assert_eq!(state.messages.len(), 2, "{:?}", state.messages);
    assert_eq!(state.messages[0].role, Role::Assistant);
    assert!(
        state.messages[0]
            .text
            .starts_with("There's an issue with the selected model")
    );
    let row = &state.messages[1];
    assert_eq!(row.role, Role::System);
    assert_eq!(row.text, "Claude Code ended with an error (code 0).");
    let detail = row.detail.as_deref().unwrap();
    assert!(detail.contains("no-such-model"), "{detail}");
    assert!(detail.contains("api error status 404"), "{detail}");
}

/// A redacted-thinking block that never stops: the placeholder row survives
/// as-is (the turn end clears the pointer, not the text) and the no-text
/// success appends its fallback row.
#[tokio::test(flavor = "multi_thread")]
async fn redacted_thinking_row_stays_then_fallback_appends() {
    let root = mk_project();
    let state = replay(
        &root,
        &fixture_fake("scenarios/redacted-thinking.jsonl"),
        None,
    )
    .await;

    assert_eq!(state.messages.len(), 2, "{:?}", state.messages);
    assert_eq!(state.messages[0].role, Role::Thinking);
    assert_eq!(state.messages[0].text, "Claude Code is thinking…");
    assert_eq!(state.active_thinking_id, None);
    assert_eq!(state.messages[1].role, Role::Assistant);
    assert_eq!(state.messages[1].text, "redacted thinking skipped");
}

/// A failed tool row (snapshot-announced, so its engine `summarize` title
/// survives completion) does not fail the turn: the success fallback row
/// still lands.
#[tokio::test(flavor = "multi_thread")]
async fn tool_failure_row_fails_without_failing_turn() {
    let root = mk_project();
    let state = replay(&root, &fixture_fake("scenarios/tool-failure.jsonl"), None).await;

    assert_eq!(state.messages.len(), 2, "{:?}", state.messages);
    assert_eq!(state.messages[0].text, "Analyzing the model");
    let tool = state.messages[0].tool.as_ref().unwrap();
    assert_eq!(tool.state, engine::project::store::ToolState::Failed);
    assert!(tool.output.as_deref().unwrap().contains("Traceback"));
    assert_eq!(state.messages[1].role, Role::Assistant);
    assert_eq!(
        state.messages[1].text,
        "the tool failed but the turn completed"
    );
}

/// The minimal success turn: a thinking row (whose token estimates rode
/// inside the open interval, ending at the block stop's duration form) and
/// the streamed-then-confirmed assistant row, nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn pong_turn_reduces_to_thinking_and_text() {
    let root = mk_project();
    let state = replay(&root, &fixture_fake("scenarios/pong.jsonl"), None).await;

    assert_eq!(state.messages.len(), 2, "{:?}", state.messages);
    assert_eq!(state.messages[0].role, Role::Thinking);
    assert!(thought_for_seconds(&state.messages[0].text) >= 1);
    assert_eq!(state.messages[1].role, Role::Assistant);
    assert_eq!(state.messages[1].text, "pong");
    assert!(state.unconfirmed.is_empty());
    assert_eq!(state.streaming_id, Some(state.messages[1].id));
}
