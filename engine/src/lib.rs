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
//! - [`python`] — venv install planning/check driver and (M5 T2) the model.py runner
//!
//! M4's app drives agent sessions only through that command/event pair,
//! and drains [`agent::EngineEvent`]s for the whole turn: the engine acts
//! on a cancel only between event translations, so a reader that stops
//! draining stalls the turn rather than cancelling it.

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
pub mod watch;
