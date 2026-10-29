//! The real-agent smoke (M2 task 9) — GATED, never run by the suite: it
//! spawns the REAL claude executable the locator discovers (no
//! `CAD_TEST_CLAUDE` override), spending one real turn of Preston's
//! authenticated account. Only Preston runs it, against a throwaway
//! project root passed in `CAD_SMOKE_ROOT` (a fresh mktemp directory with
//! an empty `output/`; never a real project):
//!
//! ```text
//! ssh thelio 'bash -lc "P=\$(mktemp -d /tmp/cad-smoke-XXXXXX); mkdir -p \$P/output; cd ~/Code/cad-studio-cosmic && CAD_SMOKE_ROOT=\$P cargo test -p engine --test real_smoke -- --ignored --nocapture; rm -rf \$P"'
//! ```

use std::path::PathBuf;
use std::time::Duration;

use engine::agent::EngineCommand;
use engine::agent::claude::TurnRequest;
use engine::agent::session::{EngineEvent, Session, TurnOutcome};
use engine::settings::Settings;

/// One one-word turn ("pong") through the command surface against the
/// real executable: every event prints for the eyeball verdict, and the
/// only assertion is that the dance terminates within the bound without a
/// `Failed` outcome.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "spawns the real authenticated claude; Preston gates the run"]
async fn real_agent_pong() {
    let root = PathBuf::from(
        std::env::var("CAD_SMOKE_ROOT").expect("CAD_SMOKE_ROOT must name the fresh smoke project"),
    );
    let mut session = Session::new(Settings::default());
    let mut running = None;
    EngineCommand::Send {
        root,
        request: TurnRequest {
            prompt: "pong".into(),
            references: Vec::new(),
            images: Vec::new(),
        },
    }
    .apply(&mut session, &mut running);
    let handle = running.as_mut().expect("Send installs the running turn");

    let mut finished = None;
    let deadline = Duration::from_secs(120);
    while let Some(event) = tokio::time::timeout(deadline, handle.events.recv())
        .await
        .expect("the real turn did not finish within 120 s")
    {
        println!("[smoke] {event:?}");
        if let EngineEvent::TurnFinished(outcome) = event {
            finished = Some(outcome);
            break;
        }
    }
    match finished.expect("the dance always ends with TurnFinished") {
        TurnOutcome::Failed { error, .. } => panic!("the real agent turn failed: {error}"),
        outcome => println!("[smoke] turn ended: {outcome:?}"),
    }
}
