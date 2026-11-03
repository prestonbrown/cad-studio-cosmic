//! CAD Studio app crate: the libcosmic shell around the engine and viewer,
//! plus the pure, GUI-free logic modules that shell maps onto widgets. Every
//! behavior module (camera_input, chat, panes, parameters, schematic,
//! viewer_tools) stays
//! free of cosmic/iced types so it is unit-testable without a window; widget
//! files are thin mapping layers and register here as their tasks land.
//!
//! Module map (M4 complete — the vertical slice
//! `app/tests/vertical_slice.rs` drives one prompt through all of it; M5's
//! wrap slice `app/tests/m5_slice.rs` adds the python check, slider
//! rebuild, version restore, and compare picker on top; M6's model tests
//! `app/tests/viewer_tools_model.rs` + `app/tests/parts_overlay_model.rs`
//! pin the viewer-tool state and the overlay's badge/section text; M7's
//! `app/tests/motion_controls_model.rs` pins the playback capsule's
//! Swift-format shapes):
//!
//! - [`activity`] — the header's ActivityViewer model + issue list (pure);
//!   motion errors and collision warnings join the list (M7).
//! - [`app`] — the `cosmic::Application`: windows, sheets, commands,
//!   startup reap + SIGTERM, portals, DBus reveal.
//! - [`bridge`] — turn driving over the engine session: the send queue,
//!   the drain loop, chat.json persistence.
//! - [`camera_input`] — the 3D camera state machine (orbit/pan/zoom/clamps).
//! - [`chat`] — the chat-row reducer over engine events (pure).
//! - [`chat_view`] — the chat pane widgets over [`chat`]'s state.
//! - [`compare_pane`] — the compare-ghost half of [`viewer_pane`]: staging
//!   the chosen version's ghost mesh at the scene pin and re-staging on
//!   reload (an impl extension — same crate, separate file) (M6).
//! - [`localize`] — the fluent scaffold: catalogs + the `fl!` macro (the
//!   full placeable migration is M8's).
//! - [`markdown`] — inline markdown rendering for chat text.
//! - [`motion_controls`] — the MotionControls port: the study-playback
//!   capsule's Swift-parity formatting (time, values, speeds, collisions);
//!   the widget rows mount with the pane's playback messages (M7).
//! - [`panes`] — pane-grid ↔ layout-tree conversion + resize clamping.
//! - [`parts_overlay`] — the parts panel, measure badge, and section
//!   controls floating over the 3D canvas (M6).
//! - [`parameters`] / [`parameters_view`] — the model-parameter panel the
//!   bridge syncs and the overlay the 3D pane floats (M5).
//! - [`recents`] — the recents store wrapping the engine's, via
//!   cosmic-config.
//! - [`schematic`] — the drawing-page model, raster cache, and pane.
//! - [`sheets`] — the first-run setup, python setup (the PythonSetupSheet
//!   port and its EnvEvent-to-state reduction), and agent-missing sheets.
//! - [`viewer_pane`] — the embedded 3D viewer: scene reload from `output/`,
//!   status bar, parameters overlay host, and the M6 tool wiring
//!   (arrangement poses, plate scene, frozen grid, section). M7 adds the
//!   motion half: the playback clock (a `time::every(TICK)` subscription
//!   mounted only while animating AND playing), animating's mutual
//!   exclusion with measuring/sectioning, and the study reload whose
//!   error and collision counts feed the header's issues.
//! - [`viewer_tools`] — the ModelViewerState port: the 3D pane's tool
//!   state (grid/wireframe/measure/section/layout/explode/hidden) (M6).
//! - [`windows`] — the welcome, project, settings, and About windows.
//!
//! Every module reads its user-visible strings through `fl!`; the sheets'
//! print-preferences section and the whole settings/About window landed
//! with M8's settings parity. The remaining sheet and welcome literals
//! ride with the repo-wide swap.
pub mod activity;
pub mod app;
pub mod attachments;
pub mod bridge;
pub mod camera_input;
pub mod chat;
pub mod chat_view;
pub mod compare_pane;
pub mod drive;
pub mod engine_text;
pub mod localize;
pub mod markdown;
pub mod motion_controls;
pub mod panes;
pub mod parameters;
pub mod parameters_view;
pub mod parts_overlay;
pub mod recents;
pub mod schematic;
pub mod sheets;
pub mod typography;
pub mod version_menu;
pub mod viewer_pane;
pub mod viewer_tools;
pub mod windows;
