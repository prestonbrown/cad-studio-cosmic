//! CAD Studio domain engine.
//!
//! Library-only: no GUI dependencies (spec §4.2). Modules:
//! - [`agent`] — the agent session stack: executable discovery
//!   ([`agent::locator`]), stream-event decoding ([`agent::stream_event`]),
//!   tool-use row summaries ([`agent::tool_summary`]), argv and prompt
//!   assembly ([`agent::claude`]), and the turn state machine plus
//!   busy/missing/auth lifecycle dance ([`agent::session`]), driven through
//!   the [`agent::EngineCommand`]/[`agent::EngineEvent`] pair
//! - [`settings`] — app settings (upstream UserDefaults key names, XDG path)
//! - [`project`] — manifest decoding, output requirements, project metadata/chat/layouts, recents
//! - [`parsing`] — STL/3MF mesh parts and animation studies
//! - [`watch`] — output/ directory reload watching
//! - [`processes`] — streamed child spawn, SIGINT→SIGTERM escalation, pid registry
//!
//! M4's app drives agent sessions only through that command/event pair,
//! and drains [`agent::EngineEvent`]s for the whole turn: the engine acts
//! on a cancel only between event translations, so a reader that stops
//! draining stalls the turn rather than cancelling it.

pub mod agent;
pub mod parsing;
pub mod processes;
pub mod project;
pub mod settings;
pub mod watch;
