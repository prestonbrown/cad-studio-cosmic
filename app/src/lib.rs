//! CAD Studio app crate: the libcosmic shell around the engine and viewer,
//! plus the pure, GUI-free logic modules that shell maps onto widgets. Every
//! behavior module (camera_input, chat, panes, schematic) stays free of
//! cosmic/iced types so it is unit-testable without a window; widget files
//! are thin mapping layers and register here as their tasks land.

pub mod bridge;
pub mod camera_input;
pub mod chat;
pub mod chat_view;
pub mod markdown;
pub mod panes;
pub mod schematic;
pub mod viewer_pane;
