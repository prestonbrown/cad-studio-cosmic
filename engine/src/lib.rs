//! CAD Studio domain engine.
//!
//! Library-only: no GUI dependencies (spec §4.2). Modules:
//! - [`agent`] — agent executable discovery and spawn environment
//! - [`settings`] — app settings (upstream UserDefaults key names, XDG path)
//! - [`project`] — manifest decoding, output requirements, project metadata/chat/layouts, recents
//! - [`parsing`] — STL/3MF mesh parts and animation studies
//! - [`agent`] — agent-side modules: tool-use summaries (more arrive with M2's
//!   session work)
//! - [`watch`] — output/ directory reload watching
//! - [`processes`] — streamed child spawn, SIGINT→SIGTERM escalation, pid registry
//!
//! The `EngineCommand`/`EngineEvent` message surface (spec §4.2) arrives with M2's
//! agent-session work.

pub mod agent;
pub mod parsing;
pub mod processes;
pub mod project;
pub mod settings;
pub mod watch;
