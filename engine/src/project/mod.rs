//! Project types (spec §4.2): the manifest decoder lives in [`manifest`].

pub mod manifest;
pub mod outputs;

pub use manifest::{BBox, Drawing, Manifest, PartRef};
pub use outputs::MissingCtx;
