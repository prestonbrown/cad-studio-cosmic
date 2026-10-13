//! CAD Studio app crate: the libcosmic shell around the engine and viewer,
//! plus the pure, GUI-free logic modules that shell maps onto widgets. Every
//! behavior module (camera_input, chat, panes, parameters, schematic) stays
//! free of cosmic/iced types so it is unit-testable without a window; widget
//! files are thin mapping layers and register here as their tasks land.

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
pub mod viewer_pane;
pub mod windows;

// The About and Settings windows are M8 (spec §4.4 packaging/Sys row);
// until then `fl!` strings and this crate's catalog ride along unwired.
