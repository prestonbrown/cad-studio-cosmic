# MSAA decision (M0 spike, task 9)

Chosen: **4x** via `MeshPipeline::with_msaa` — viewer-owned 4-sample color +
Depth24PlusStencil8, resolve into a single-sample intermediate, then a
premultiplied-alpha blit pass over the iced target (`LoadOp::Load`; the
resolve output's rgb is already coverage-weighted, so `One/OneMinusSrcAlpha`
composites it exactly — plain SrcAlpha blending darkens edges). The blit
pattern is kept: validates on both Vulkan (thelio) and Metal (Mac), iced's
underlay survives, frames stay byte-stable, and the section cap runs at
count 4 in the same pass. Perf feel: no measurable test-time cost (headless
runs are readback-bound); 4x is universal-attachment-supported where 8x is
not. Proof: `render_headless::msaa_smooths_a_diagonal_edge` — 4x produces
~1.4k coverage-blend edge pixels where 1x produces 0. Default stays 1
(`MeshPipeline::new`); the app opts in when the M3 camera/quality settings
land.
