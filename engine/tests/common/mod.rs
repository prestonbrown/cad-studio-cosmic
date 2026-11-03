//! Helpers shared by the engine's integration test files. Each test file is
//! its own crate and uses a subset, so unused items are expected.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use engine::agent::session::{EngineEvent, TurnHandle};

/// A file under fixtures/fake-agent (the scripted stand-in agent).
pub fn fixture_fake(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/fake-agent")
        .join(p)
}

/// A file under fixtures/upstream (vendored upstream sources and prompts).
pub fn fixture_upstream(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/upstream")
        .join(p)
}

/// The upstream system prompt template.
pub fn system_template() -> String {
    std::fs::read_to_string(fixture_upstream("CADSystemPrompt.md")).unwrap()
}

/// Drain a turn's events through its terminal `TurnFinished`, panicking on
/// a closed channel or after 5 s.
pub async fn collect_until_finished(handle: &mut TurnHandle) -> Vec<EngineEvent> {
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
