//! CAD Studio domain engine.
//!
//! Library-only: no GUI dependencies (spec §4.2). Modules:
//! - [`agent`] — the agent session stack: executable discovery
//!   ([`agent::locator`]), stream-event decoding ([`agent::stream_event`]),
//!   tool-use row summaries ([`agent::tool_summary`]), argv and prompt
//!   assembly ([`agent::claude`], [`agent::codex`]), and the turn state
//!   machine plus busy/missing/auth lifecycle dance ([`agent::session`]);
//!   the app drives it through `Session::send` and the returned
//!   `TurnHandle` (see [`agent`] for the drain/pacing contract)
//! - [`settings`] — app settings (upstream UserDefaults key names, XDG path)
//! - [`project`] — the project folder layout, manifest decoding, output
//!   requirements, project metadata/chat/layouts, versions, recents
//! - [`parsing`] — STL/3MF mesh parts and animation studies
//! - [`watch`] — output/ directory reload watching
//! - [`processes`] — streamed child spawn, SIGINT→SIGTERM escalation, pid
//!   registry, the tool environment
//! - [`python`] — venv install planning/check driver and the model.py runner
//! - [`storage`] — the atomic file writer and the per-user app directories

/// A fieldless enum plus its `ALL` slice, every variant in declaration
/// order, derived from the one declaration so a new variant cannot be
/// left out of exhaustive checks.
macro_rules! enum_with_all {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $( $(#[$variant_meta])* $variant ),*
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),*];
        }
    };
}

pub mod agent;
pub mod parsing;
pub mod processes;
pub mod project;
pub mod python;
pub mod settings;
pub mod storage;
pub mod watch;
