//! The agent session stack, bottom to top: [`locator`] discovers the agent
//! executable and snapshots the login environment it spawns in;
//! [`stream_event`] decodes the agent's stream-json stdout lines;
//! [`tool_summary`] titles the tool-use rows those lines announce; [`claude`]
//! assembles the claude runner's argv, system prompt, and turn requests;
//! [`codex`] the codex runner's argv plus its translator onto the same
//! [`StreamEvent`] pipeline; [`attachments`] classifies attachment paths
//! (the image allowlist both runners and the app's thumbnail widget share);
//! and [`session`] runs single turns and the busy/missing/auth lifecycle
//! dance around them, over [`crate::processes`]' streamed child spawn and
//! SIGINT→SIGTERM interrupt.
//!
//! The surface the app drives that stack through is the
//! [`EngineCommand`]/[`EngineEvent`] pair: the app sends commands (run a
//! turn, cancel, shut down) and drains the launched turn's [`EngineEvent`]
//! stream until its terminal `TurnFinished`. The events carry the
//! chat-transcript facts upstream builds its rows from — thinking block
//! boundaries, tool ids with names and titles, and a failing child's exit
//! code — so the app maps them onto its transcript without re-parsing the
//! stream. The pairing is also a pacing
//! contract — the engine polls for a cancel only between event
//! translations, so an app that stops draining stalls the turn instead of
//! cancelling it (carried in `docs/research/carry-forwards.md`, M4).

pub mod attachments;
pub mod claude;
pub mod codex;
pub mod locator;
pub mod session;
pub mod stream_event;
pub mod tool_summary;

use std::path::PathBuf;

use crate::agent::claude::TurnRequest;
use crate::agent::session::{Session, TurnHandle};
use crate::processes::reap_orphans;

pub use crate::agent::session::{EngineEvent, SystemRow};

/// One command to the agent engine — the app→engine half of the surface
/// spec §4.2 draws; the engine answers on the launched turn's event
/// stream. Thin by design (M2's ruling: the queue, the actor loop, and UI
/// policy all stay app-side): every variant routes directly onto
/// [`Session`] or [`TurnHandle`].
#[derive(Debug, Clone, PartialEq)]
pub enum EngineCommand {
    /// Run one turn of a project's conversation through the full
    /// lifecycle dance ([`Session::send`]) — busy/missing fallbacks
    /// included. The launched turn becomes the running one, replacing
    /// (and dropping the handle of) any turn before it; an overlapping
    /// send is the app's call to prevent, not the engine's.
    Send {
        /// The project the turn runs in: tee logs, metadata, resume state.
        root: PathBuf,
        request: TurnRequest,
    },
    /// Interrupt the running turn ([`TurnHandle::cancel`]): SIGINT now,
    /// SIGTERM after 3 s. A no-op when no turn is running.
    Cancel,
    /// Interrupt the running turn and reap the process registry's crash
    /// orphans ([`reap_orphans`]) — the app is going away.
    Shutdown,
}

impl EngineCommand {
    /// Apply one command to the state it drives: the session plus its
    /// running turn. `Send` installs the new turn's handle (dropping the
    /// previous handle never cancels its turn — the spawner drains its
    /// pipes); `Cancel`/`Shutdown` interrupt the running turn in place.
    pub fn apply(self, session: &mut Session, running: &mut Option<TurnHandle>) {
        match self {
            Self::Send { root, request } => {
                *running = Some(session.send(&root, &request));
            }
            Self::Cancel => {
                if let Some(handle) = running.as_ref() {
                    handle.cancel();
                }
            }
            Self::Shutdown => {
                if let Some(handle) = running.as_ref() {
                    handle.cancel();
                }
                reap_orphans();
            }
        }
    }
}
