//! Viewer-side tool math: the pure, MODEL-space computations behind the
//! viewer's interactive controls. [`arrangement`] is the PartsLayout engine
//! — part grouping, explode offsets, and the plate flat layout — stated as
//! transforms per part slot that the renderer consumes as per-part data,
//! never vertex rewrites (spec §9.6 records the divergence from upstream's
//! rebuilt TriangleMesh). [`section`] maps the section slider onto the
//! mesh shader's GPU discard — the MODEL-space plane through the
//! MODEL→scene remap, MODEL-Y keep-above parity included.
//! [`picking`] is the measure tool's CPU hit test —
//! the click-driven ray against the staged, posed, sectioned scene (spec
//! §5: event-driven, never per-frame).

pub mod arrangement;
pub mod picking;
pub mod section;
