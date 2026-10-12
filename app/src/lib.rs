//! CAD Studio app crate: the libcosmic shell around the engine and viewer,
//! plus the pure, GUI-free logic modules that shell maps onto widgets. Every
//! behavior module (`chat`, `panes`, later `camera_input`/`schematic`) stays
//! free of cosmic/iced types so it is unit-testable without a window; widget
//! files are thin mapping layers and register here as their tasks land.

pub mod chat;
pub mod panes;
