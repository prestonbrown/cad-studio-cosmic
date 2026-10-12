//! CAD Studio domain engine.
//!
//! Library-only: no GUI dependencies (spec §4.2). Modules:
//! - [`agent`] — agent executable discovery and spawn environment
//! - [`settings`] — app settings (upstream UserDefaults key names, XDG path)
//! - [`project`] — manifest decoding, output requirements, project metadata/chat/layouts, recents
//! - [`parsing`] — STL/3MF mesh parts and animation studies
//! - [`watch`] — output/ directory reload watching
//!
//! The `EngineCommand`/`EngineEvent` message surface (spec §4.2) arrives with M2's
//! agent-session work.

pub mod agent;
pub mod parsing;
pub mod project;
pub mod settings;
pub mod watch;
