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
//! The surface the app drives that stack through is [`session::Session`]:
//! [`session::Session::send`] launches one turn's lifecycle dance and
//! returns a [`session::TurnHandle`], whose `events` the app drains until
//! the terminal `TurnFinished` and whose `cancel()` interrupts the turn
//! (SIGINT, then SIGTERM after 3 s). Reaping crash orphans at launch and
//! quit is [`crate::processes::reap_orphans`], called by the app directly.
//! The events carry the chat-transcript facts upstream builds its rows
//! from — thinking block boundaries, tool ids with names and titles, and a
//! failing child's exit code — so the app maps them onto its transcript
//! without re-parsing the stream.
//!
//! Draining is also a pacing contract: the engine polls for a cancel only
//! between event translations, so an app that stops draining stalls the
//! turn instead of cancelling it.

pub mod attachments;
pub mod claude;
pub mod codex;
pub mod locator;
pub mod session;
pub mod stream_event;
pub mod tool_summary;

pub use crate::agent::session::{EngineEvent, SystemRow};
