//! Viewer-side tool math: the pure, MODEL-space computations behind the
//! viewer's interactive controls. [`arrangement`] is the PartsLayout engine
//! — part grouping, explode offsets, and the plate flat layout — stated as
//! transforms per part slot that the renderer consumes as per-part data,
//! never vertex rewrites (spec §9.6 records the divergence from upstream's
//! rebuilt TriangleMesh).

pub mod arrangement;
