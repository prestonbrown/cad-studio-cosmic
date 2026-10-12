# M3 Viewer Basics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Productize the M0 viewer spike into the real 3D pane foundation — upload-once GPU buffers, per-part colored meshes, parity camera, grid/bed/wireframe, real normals, viewport aspect — all pinned by headless render tests, per spec §10 M3.

**Architecture:** The viewer crate keeps its split (pure-wgpu core in `render/`, iced adapter thin). Engine's `Part` becomes the mesh currency (per-corner positions + per-triangle Option colors); the M0 `TriMesh` is replaced by a part-aware scene. Every formula is parity-transcribed from the vendored Swift (facts authority: `.superpowers/sdd/m3-viewer-facts.md` — the M3 plan's ground truth alongside the Swift itself).

**Tech Stack:** wgpu 28 (exact), glam, bytemuck; existing headless harness in viewer/tests/render_headless.rs.

**Spec:** docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md (§4.3, §5 GPU-first, §10 M3). Carry-forwards: docs/research/carry-forwards.md M3 section. Facts: .superpowers/sdd/m3-viewer-facts.md (m3-viewer-research agent, 2026-10-12).

## Global Constraints

- GPU-first: meshes upload ONCE (identity-keyed); every dynamic (camera, explode-later, playback-later) is uniform/instance data. Per-frame CPU mesh work is a defect.
- wgpu 28 exact; iced pinned via libcosmic's git source (never pop-os/iced.git).
- engine stays GUI-free; viewer's iced surface stays in the adapter (scene.rs) only.
- Parity formulas verbatim per the facts file (esp.: framing radius = bbox DIAGONAL/2 — facts #1; section Y-axis asymmetry is M6, NOT this milestone).
- Tests: headless surfaceless on thelio; no network; timing via bounded poll loops (T1-M2 precedent), never fixed sleep + single drain.
- Conventional commits; explicit paths; NEVER `git rm`; thelio login shell `bash -lc`.

## Review Focus

1. **Upload-once actually once** — a second render with an unchanged mesh must not rewrite vertex buffers (pinned by a wgpu-observer test: buffer write count) → Task 1.
2. **Framing radius regression** — long-thin part (10×1×1) frames per diagonal/2, not extent/2 (pin the camera distance) → Task 2.
3. **Aspect from viewport** — 800×600 vs 1280×600 render the same object at the same pixel height (no 16:9 stretch) → Task 2.
4. **Part colors reach pixels** — two-part model renders two distinguishable regions (per-part color bands measured, not assumed) → Task 3.
5. **Wireframe is edges-only** — line-list render of a cube yields 12 edge lines' worth of ink, not 36 (triangles suppressed) → Task 5.

---

### Task 1: Upload-once + Part-keyed buffers (GPU-first core)

**Files:**
- Modify: `viewer/src/render/mod.rs` (major), `viewer/src/render/scene.rs`, `viewer/tests/render_headless.rs`
- Create: `viewer/src/render/parts.rs`

**Interfaces:**
- Consumes: `engine::parsing::{Part, TriangleColor}` (engine gains nothing; viewer adds `engine = { workspace = true }`? NO — workspace has no engine entry; viewer adds `engine = { path = "../engine" }`). `viewer::render::TriMesh` RETIRES.
- Produces: `viewer::render::parts::{GpuPart, PartScene}` — `PartScene::from_parts(parts: &[engine::parsing::Part], neutral: [f32;3]) -> PartScene` (CPU-side: per-part vertex buffers data + aabb); `MeshPipeline::prepare_parts(&mut self, device, queue, size, camera, scene: &PartScene)` uploads a part's vertices ONLY when its content identity (`Arc`-pointer or hash of positions len + first/last coords + colors len) differs from what's resident; per-part transform slot in the uniform (mat4 per part, identity this milestone); `draw` iterates parts with per-part bind offsets. The iced adapter and the headless harness drive the SAME API.

**Tests (write first, RED):**
- `upload_once_second_frame_no_reupload`: wgpu buffer-write observer — render the golden two-part scene twice with the same PartScene; assert the vertex buffer's `size` allocated once and a write-counter (expose `MeshPipeline::upload_count_for_tests() -> u32`) stays 0 on the second frame.
- `new_scene_reuploads`: mutate one part's identity (push a vertex) → upload_count increments exactly 1 (only that part).
- existing render_headless tests ported from TriMesh to PartScene (ink, stability, resize, section, msaa — same assertions, new fixture path through engine's parse_3mf of the golden model.3mf: 2 parts × 12 triangles).

**Steps:** tests → RED → implement (identity = hash of (ptr-or-len, first/last positions, colors len); per-part uniform array `[[f32;4];4]` × MAX_PARTS=64 with a dynamic offset or one bind per part — simplest correct wins; document the choice) → GREEN thelio → commit `feat(viewer): part-keyed upload-once buffers (GPU-first core)`.

### Task 2: Camera parity + viewport aspect

**Files:** Modify `viewer/src/render/mod.rs` (camera fns), `viewer/tests/render_headless.rs`.

**Interfaces:** `camera::{orbit(distance,yaw,pitch,target) -> view_proj, frame(aabb,aspect) -> (target,distance), presets::{iso,front,top,right}}` — pure fns, property-tested. Framing radius = diagonal/2 = (‖max−min‖)/2 (facts #1); distance = r/sin(fov/2)·1.15; zoom clamps [0.1r, 200r]; pan scale 2·d·tan(fov/2)/view_height; pitch clamp ±1.5607; presets iso (yaw π/4, pitch 0.6) / front (0,0) / top (0, π/2−ε per upstream's `max`) / right (π/2, 0); re-frame trigger: mesh size change > 50% (kept in the adapter). Aspect ALWAYS from the render size (kill the 16:9 hardcode). Z-up model → scene remap (x, z, −y) applied at PartScene build.

**Tests:** `frame_radius_is_diagonal_half` (10×1×1 aabb → r = √102/2; assert distance = r/sin(17.5°)·1.15 within 1e-3); `aspect_from_viewport` (same scene at 800×600 and 1280×600: object pixel-height within 5%); `zoom_clamps_and_presets` (pure math asserts); ported orbit test (drag deltas → view_proj changes match formulas).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): parity camera — diagonal framing, viewport aspect, presets, clamps`.

### Task 3: Per-part colors + neutral

**Files:** Modify `viewer/src/render/parts.rs`, `mesh.wgsl`, tests.

**Interfaces:** Vertex gains `color: [f32;3]` (replacing the fixed neutral); `PartScene::from_parts` maps `TriangleColor::Some([r,g,b])` (u8 sRGB → linear via the standard 2.2-ish transfer, documented) per triangle to its 3 corners; `None` → the neutral (0.62, 0.70, 0.80 linear). mesh.wgsl reads the vertex color; the uniform tint (explode/compare overlays later) stays separate.

**Tests:** `two_parts_two_colors` (golden model.3mf: blue base region + red lid region — headless readback classifies pixels by nearest of the two expected linear colors; ≥95% of ink pixels within tolerance of one); `neutral_when_uncolored` (golden model.stl single part → all ink near neutral); `srgb_conversion_table` (pure fn: 0, 128, 255 → 0.0, ~0.216, 1.0).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): per-part vertex colors with sRGB→linear transfer`.

### Task 4: Real normals + lights parity

**Files:** Modify `parts.rs`, `mesh.wgsl`, tests.

**Interfaces:** Vertex gains `normal: [f32;3]` computed at PartScene build (face normal per triangle, per-corner — flat shading matches upstream's mesh-material look); mesh.wgsl replaces the dpdx/dpdy shorthand with the attribute; lights per facts (key 2600 from (60,120,90), fill 900 from (−90,40,−60), scene-space after remap; ambient 0.15).

**Tests:** `shaded_faces_differ` (a cube: front-facing bright face vs grazing face measurably differ in readback luminance, ratio consistent with the light dirs); `normals_match_cross_product` (pure: per-triangle normal == normalize(cross(b−a, c−a))).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): per-corner face normals + parity two-light shading`.

### Task 5: Wireframe edges + grid + bed

**Files:** Modify `render/` (+ new `grid.wgsl`), `viewer/examples/spike_window.rs`, tests.

**Interfaces:** `PartScene` gains `edges: Vec<[u32;2]>` per part (dedup undirected pairs; built once); `MeshPipeline` gains a line-list pass (3-wide, no cull, the part color at 60% — facts: wireframe uses the fill color with no face culling) toggled by `Options { wireframe: bool }`; `grid::geometry(aabb, bed: Option<[f64;2]>) -> GridMesh` (10mm minor at 10% alpha/0.25 width; 100mm major at 28%/0.5 — parity per facts; half-extent = max(100, ceil(max(x,y footprint)·0.75/10)·10); axis tripod RGB at the footprint corner +15% margin; bed rectangle accent-colored at scene-y 0.06 when Some). Grid renders as its own line pass (no depth write — facts: grid overlays).

**Tests:** `wireframe_edge_count` (cube part → exactly 12 edges; line pass draws 24 vertices); `wireframe_renders_lines_not_faces` (readback ink ≪ solid-mode ink for the same camera; ratio banded); `grid_extents_and_majors` (pure fn: footprint 260 → half-extent 200? — per formula: ceil(260·0.75/10)·10 = 200; assert major-line positions); `bed_rectangle_present` (bed Some → accent-colored pixels form a rectangle border at the footprint; None → absent).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): wireframe edges, parity grid, bed outline`.

### Task 6: with_msaa in the windowed example + orientation test

**Files:** Modify `viewer/examples/spike_window.rs`, `viewer/tests/render_headless.rs`.

**Interfaces:** the example's Storage wires `MeshPipeline::with_msaa(4)` behind a `m` keybind toggle (rebuild pipeline on toggle); headless gains `blit_orientation` — render an L-shaped asymmetric part (fixture: two boxes offset — reuse golden but translate one part via its transform slot) at 1×; assert the readback's left-right asymmetry matches the CPU-computed projection (a column-occupied pattern check — catches UV flips, the M0 deferred obligation).

**Tests:** `orientation_pattern_matches_projection` (CPU-rasterize a coarse occupancy grid from the same view_proj; ≥85% cell agreement with the GPU readback downsampled the same way); manual: the example toggle (deferred visual gate, controller screenshots).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): msaa toggle in example + orientation-sensitive blit test`.

### Task 7: M3 wrap

**Files:** `viewer/src/lib.rs` docs, `docs/research/carry-forwards.md` (strike M3 items), README dev section unchanged.

**Steps:** module docs (parts/camera/grid surface); strike the landed M3 carry-forwards (upload-once, aspect, orientation test, with_msaa toggle, real normals; LEAVE cull-None cost note + M6 items); full `cargo test --workspace --all-targets` thelio zero warnings (expect 60-engine + M2's new + ~25 viewer); commit `docs: M3 complete — viewer surface, carry-forward strikes`.

---

**Execution note:** Tasks 1–2 sequential (same files); 3–4 sequential after 2; 5 after 3; 6 after 1; 7 last. Where the M2 tail still owns engine files, viewer tasks never touch engine — parallel-safe by construction.
