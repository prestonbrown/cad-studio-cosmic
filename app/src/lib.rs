//! CAD Studio app crate: the libcosmic shell around the engine and viewer,
//! plus the pure, GUI-free logic modules that shell maps onto widgets. Every
//! behavior module (camera_input, chat, panes, parameters, schematic) stays
//! free of cosmic/iced types so it is unit-testable without a window; widget
//! files are thin mapping layers and register here as their tasks land.
//!
//! Module map (M4 complete — the vertical slice
//! `app/tests/vertical_slice.rs` drives one prompt through all of it):
//!
//! - [`activity`] — the header's ActivityViewer model + issue list (pure).
//! - [`app`] — the `cosmic::Application`: windows, sheets, commands,
//!   startup reap + SIGTERM, portals, DBus reveal.
//! - [`bridge`] — turn driving over the engine session: the send queue,
//!   the drain loop, chat.json persistence.
//! - [`camera_input`] — the 3D camera state machine (orbit/pan/zoom/clamps).
//! - [`chat`] — the chat-row reducer over engine events (pure).
//! - [`chat_view`] — the chat pane widgets over [`chat`]'s state.
//! - [`localize`] — the fluent scaffold (catalog unwired until M8).
//! - [`markdown`] — inline markdown rendering for chat text.
//! - [`panes`] — pane-grid ↔ layout-tree conversion + resize clamping.
//! - [`parameters`] / [`parameters_view`] — the model-parameter panel the
//!   bridge syncs and the overlay the 3D pane floats (M5).
//! - [`recents`] — the recents store wrapping the engine's, via
//!   cosmic-config.
//! - [`schematic`] — the drawing-page model, raster cache, and pane.
//! - [`sheets`] — the first-run setup and agent-missing sheets.
//! - [`viewer_pane`] — the embedded 3D viewer: scene reload from `output/`,
//!   status bar, parameters overlay host.
//! - [`windows`] — the welcome and project windows.
//!
//! The About and Settings windows are M8 (spec §4.4 packaging/Sys row);
//! until then `fl!` strings and this crate's catalog ride along unwired.

pub mod activity;
pub mod app;
pub mod bridge;
pub mod camera_input;
pub mod chat;
pub mod chat_view;
pub mod localize;
pub mod markdown;
pub mod panes;
pub mod parameters;
pub mod parameters_view;
pub mod recents;
pub mod schematic;
pub mod sheets;
pub mod version_menu;
pub mod viewer_pane;
pub mod windows;
