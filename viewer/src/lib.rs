//! 3D viewer crate. wgpu 28 (exact major match with libcosmic's iced fork).
//! iced types are allowed only in the renderer adapter module (spec §4.3).
//!
//! One part-keyed renderer carries every mode (spec §5, GPU-first):
//! [`render::PartScene`] stages engine parts as scene-space vertices —
//! Z-up→Y-up remap, floor pin, per-triangle sRGB→linear colors, flat face
//! normals — and each part carries a full-content hash over those staged
//! bytes, so [`render::MeshPipeline`] uploads a part's mesh only when its
//! content changed; the camera writes uniforms, never buffers. The parity
//! math is pure and headless-testable: [`render::camera`] transcribes
//! ModelViewer.swift's orbit/zoom/pan formulas, diagonal/2 framing,
//! presets, and clamps, and [`render::lights`] holds the two-light
//! shading constants the mesh shader runs and the tests twin.
//! [`tools::arrangement`] is the PartsLayout math — grouping, explode
//! offsets, plate flat layout — as pure MODEL-space transforms
//! (PartArrangement.swift), consumed by the renderer as per-part data.
//! The unlit
//! overlay — parity grid, bed outline, axis tripod — is [`render::grid`],
//! stated against the same pinned frame.
//!
//! [`playback`] is the motion core: [`playback::MotionPlayer`] holds the
//! upstream playback state machine (advance with remainder loop-wrap,
//! clamp-and-stop without looping, restart-from-0 only at the duration),
//! and [`playback::poses`] derives one [`render::PartPose`] and one
//! [`render::PartAppearance`] per scene part from a study state — the
//! MODEL→SCENE remap on translations and quaternion imaginaries alike,
//! composed through the scene pin, hidden parts never drawing. Pure
//! state in, uniform-ready slots out; the animation clock and pane
//! wiring live in the app.
//!
//! [`render::scene`] is the iced adapter (a `shader::Program`) that drives
//! the pipeline from widget land; the windowed example and the headless
//! render tests exercise the identical renderer, and [`render::gpu`] is
//! the suite-global device context those tests share.

pub mod playback;
pub mod render;
pub mod tools;
