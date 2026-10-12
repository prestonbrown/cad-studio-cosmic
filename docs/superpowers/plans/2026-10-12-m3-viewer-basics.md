# M3 Viewer Basics Implementation Plan (rev 2 — independent review applied, 18 findings)

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
3. **Aspect from viewport** — the ink-bbox WIDTH ratio at 800×600 vs 1280×600 is ~1.0 (pixel HEIGHT is aspect-independent under vertical-FOV and would pass vacuously; width is the discriminator — under the 16:9 hardcode it scales 1.6×) → Task 2.
4. **Part colors reach pixels** — two-part model renders two distinguishable regions (per-part color bands measured, not assumed) → Task 3.
5. **Wireframe is edges-only** — line-list render of a cube yields 12 edge lines' worth of ink, not 36 (triangles suppressed) → Task 5.

---

### Task 1: Upload-once + Part-keyed buffers (GPU-first core)

**Files:**
- Modify: `viewer/src/render/mod.rs` (major + its in-crate unit tests), `viewer/src/render/scene.rs`, `viewer/tests/render_headless.rs`, `viewer/examples/spike_window.rs` (TriMesh retirement breaks it — port to PartScene), `viewer/Cargo.toml` (adds `engine = { path = "../engine" }` AND absorbs the carry-forward: image dev-dep `default-features = false, features = ["png"]`)
- Create: `viewer/src/render/parts.rs`

**Interfaces:**
- Consumes: `engine::parsing::{Part, TriangleColor}` (engine gains nothing; viewer adds `engine = { workspace = true }`? NO — workspace has no engine entry; viewer adds `engine = { path = "../engine" }`). `viewer::render::TriMesh` RETIRES.
- Produces: `viewer::render::parts::{GpuPart, PartScene}` — `PartScene::from_parts(parts: &[engine::parsing::Part], neutral: [f32;3]) -> PartScene` (CPU-side: per-part vertex buffers data + aabb); `MeshPipeline::prepare_parts(&mut self, device, queue, size, camera, scene: &PartScene)` uploads a part's vertices ONLY when its content identity — a FULL-CONTENT hash (O(n), computed once at PartScene build) over positions+colors — differs from what's resident; the iced adapter holds `Arc<PartScene>` (the per-frame clone at scene.rs:48 is deleted); per-part transform: ONE bind group per part (no MAX_PARTS cap — assemblies exceed 64; per-part binds are the simplest correct option), identity transforms this milestone. The iced adapter and the headless harness drive the SAME API.

**Tests (write first, RED):**
- `upload_once_second_frame_no_reupload`: hand-rolled counters exposed crate-public (`upload_count_for_tests() -> u32` counting VERTEX/INDEX uploads only — camera uniforms legitimately write per frame — plus `alloc_count_for_tests() -> u32`): same PartScene twice → both counters 0 on frame 2.
- `new_scene_reuploads`: mutate one part (push a vertex) → upload_count +1 for that part only.
- `middle_vertex_mutation_reuploads`: move a middle vertex (count/endpoints unchanged) → upload_count +1 (pins the full-content hash against weak-identity stale geometry).
- existing render_headless tests ported from TriMesh to PartScene (ink, stability, resize, section, msaa — same assertions, new fixture path through engine's parse_3mf of the golden model.3mf: 2 parts × 12 triangles).

**Steps:** tests → RED → implement (identity = full-content hash; per-part bind groups, no cap; counters crate-public, vertex/index-only) → GREEN thelio → commit `feat(viewer): part-keyed upload-once buffers (GPU-first core)`.

### Task 2: Camera parity + viewport aspect

**Files:** Modify `viewer/src/render/mod.rs` (camera fns), `viewer/tests/render_headless.rs`.

**Interfaces:** `camera::orbit(distance, yaw, pitch, target, aspect) -> view_proj` (aspect REQUIRED — this task kills the 16:9 hardcode; fov 35°, near 1, far 100_000 absolute) and `camera::frame(aabb) -> (target, distance)` (aspect-FREE — the formula never uses it; facts #23): target = (0, size.y/2, 0), radius = diagonal/2 = ‖max−min‖/2 (facts #1), distance = r/sin(fov/2)·1.15. Zoom clamps [0.1r, 200r]; pan scale 2·d·tan(fov/2)/view_height; pitch clamp = upstream's exact constant 1.5607; presets iso (π/4, 0.6) / front (0,0) / top (0, 1.5607 — the same maximumPitch constant, not π/2−ε) / right (π/2, 0); re-frame on size change > 50% (adapter-side). Z-up remap (x, z, −y) PLUS the floor pinning offset at PartScene build: after remapping all parts, translate the whole scene by (−center.x, −sceneMin.y, −center.z) (ModelViewer.swift update(), facts #24) — model floor sits at scene y=0, x/z centered; grid sits at y = −0.05 BELOW that floor, bed at y = 0.06 above it, tripod at the footprint corner.

**Tests:** `frame_radius_is_diagonal_half` (10×1×1 aabb → r = √102/2; assert distance = r/sin(17.5°)·1.15 within 1e-3); `aspect_from_viewport` (ink-bbox WIDTH at 800×600 vs 1280×600 within 5% of equal — the discriminator; under the 16:9 hardcode width scales 1.6×); `zoom_clamps_and_presets` (pure math asserts); ported orbit test (drag deltas → view_proj changes match formulas).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): parity camera — diagonal framing, floor pinning, viewport aspect, presets, clamps`.
**Recalibration budget:** framing distance and the remap+pin shift every ink count and flip the section test's axis (model z becomes scene y: `section_axis = 2` → 1 in scene space; aabb asserts go scene-space post-pin) — re-measure the ported assertions, do not assume "same assertions".

### Task 3: Per-part colors + neutral

**Files:** Modify `viewer/src/render/parts.rs`, `mesh.wgsl`, tests.

**Interfaces:** Vertex gains `color: [f32;3]` (replacing the fixed neutral); `PartScene::from_parts` maps `TriangleColor::Some([r,g,b])` (u8 sRGB → linear via the EXACT piecewise sRGB EOTF — pure 2.2 deviates ~40% on dark colors like the base's own blue) per triangle to its 3 corners; `None` → neutral (0.62, 0.70, 0.80 — facts gives values not space; LINEAR is a recorded assumption). mesh.wgsl reads the vertex color; the uniform tint (explode/compare overlays later) stays separate.

**Tests:** `two_parts_two_colors` (golden model.3mf: headless readback classifies by nearest of the two expected linear colors; ≥95% of ink within tolerance of one, PLUS a spatial anchor — the UPPER third of the ink bbox classifies red (lid), the lower third blue (base) — catching swapped assignments the nearest-only test cannot); `neutral_when_uncolored` (golden model.stl single part → all ink near neutral); `srgb_conversion_table` (pure fn: 0 → 0.0, 128 → ~0.216, 255 → 1.0, AND the dark discriminator 31 → 0.0137 — the value where 2.2-gamma reads 0.0097 and would fail).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): per-part vertex colors with sRGB→linear transfer`.

### Task 4: Real normals + lights parity

**Files:** Modify `parts.rs`, `mesh.wgsl`, tests.

**Interfaces:** Vertex gains `normal: [f32;3]` computed at PartScene build (face normal per triangle, per-corner — flat shading matches upstream's mesh-material look); mesh.wgsl replaces the dpdx/dpdy shorthand with the attribute; lights per facts (key 2600 from (60,120,90), fill 900 from (−90,40,−60), scene-space after remap; ambient 0.15).

**Tests:** `shaded_faces_differ` (a cube: key-light-facing vs away-facing regions differ in readback luminance by ≥2× — derived from the 0.26/0.09/0.15 coefficients; compute the band in the test from those constants); `normals_match_cross_product` (pure: per-triangle normal == normalize(cross(b−a, c−a))).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): per-corner face normals + parity two-light shading`.
**Recalibration budget:** real normals + lights change every face luminance — the MSAA test's 93..=103 band (render_headless.rs) will likely need re-measurement with its headroom re-documented.

### Task 5: Wireframe edges + grid + bed

**Files:** Modify `render/` (+ new `grid.wgsl`), `viewer/examples/spike_window.rs`, tests.

**Interfaces:** `PartScene` gains `edges: Vec<[u32;2]>` per part (dedup undirected pairs; built once); `MeshPipeline` gains a 1px hairline line-list pass for WIREFRAME (upstream parity: same material and opacity as fill — only the fill-mode and culling change, facts #44; NO 60% tint, NO wide lines — wgpu/WebGPU fixes line width at 1) toggled by `Options { wireframe: bool }`; `grid::geometry(aabb, bed: Option<[f64;2]>) -> GridMesh` rendered as CAMERA-FACING QUADS in a triangle pass (wgpu has no line width — upstream draws quads, ModelViewer.swift quad(from:to:width:), facts #34): 10mm minor at 10% alpha/0.25 world-units wide; 100mm major at 28%/0.5; half-extent = max(100, (floor(footprint·0.075)+1)·10) — floor-then-+1, NOT ceil (Swift rebuildGrid, facts #33; footprint 200 → 160, where ceil wrongly gives 150); grid plane at scene y = −0.05 (below the pinned floor); axis tripod RGB quads at the footprint corner +15% margin; bed rectangle quads (accent = the fixed test color [0.29, 0.56, 0.91] surfaceless — libcosmic themes unavailable headless) at scene y = 0.06 when Some. No depth write (grid overlays).

**Tests:** `wireframe_edge_count` (cube part → exactly 12 edges; line pass draws 24 vertices); `wireframe_renders_lines_not_faces` (readback ink ≪ solid-mode ink for the same camera; ratio banded); `grid_extents_and_majors` (pure fn: footprint 260 → half-extent 200 via (floor(260·0.075)+1)·10; DISCRIMINATING case footprint 200 → 160 where ceil-based reads 150; assert major-line positions); `bed_rectangle_present` (bed Some → accent-colored pixels form a rectangle border at the footprint; None → absent).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): wireframe edges, parity grid, bed outline`.

### Task 6: with_msaa in the windowed example + orientation test

**Files:** Modify `viewer/examples/spike_window.rs`, `viewer/tests/render_headless.rs`.

**Interfaces:** the example wires an `m` keybind toggle; iced's Storage owns one Pipeline per type and cannot hand out replacements — the toggle is an INTERNAL rebuild (`MeshPipeline` recreates its render pipelines + owned msaa/resolve attachments when `Primitive::prepare` observes the flag flip; wireframe stays a draw-path switch, no rebuild). The example ALSO adopts Task 2's parity input formulas (facts #114's deviations die: orbit 0.008, pitch clamp 1.5607, scroll δ/12 with precise deltas, zoom clamps [0.1r, 200r]; pan formula implemented, pan WIRING deferred to M4 deliberately). Headless gains `blit_orientation` — render an L-shaped asymmetric part (fixture: two boxes offset — reuse golden but translate one part via its transform slot) at 1×; assert the readback's left-right asymmetry matches the CPU-computed projection (a column-occupied pattern check — catches UV flips, the M0 deferred obligation).

**Tests:** `orientation_pattern_matches_projection` (CPU-rasterize a coarse occupancy grid from the same view_proj; ≥85% agreement over OCCUPIED cells with a minimum-occupancy floor — raw cell agreement is dominated by empty cells); manual: the example toggle (deferred visual gate, controller screenshots).

**Steps:** tests → RED → implement → GREEN → commit `feat(viewer): msaa toggle in example + orientation-sensitive blit test`.

### Task 7: M3 wrap

**Files:** `viewer/src/lib.rs` docs, `docs/research/carry-forwards.md` (strike M3 items), README dev section unchanged.

**Steps:** module docs (parts/camera/grid surface); RECORD the spec §5 MSAA decision (owned count-4 msaa texture → resolve → premultiplied blit per the M0 proof — carried forward as the settled pattern); record the bed-outline PULL-IN from M6 (geometry landed early with the grid; M6 owns its plate-view integration, not the primitive); strike the landed M3 carry-forwards (upload-once, aspect, orientation test, with_msaa toggle, real normals, image png-only; LEAVE cull-None cost note + M6/M7 items); full `cargo test --workspace --all-targets` thelio zero warnings; commit `docs: M3 complete — viewer surface, carry-forward strikes + MSAA decision`.

---

**Execution note:** Tasks 1–2 sequential (same files); 3–4 sequential after 2; 5 after 3; **6 after 5** (both touch spike_window.rs + render_headless.rs); 7 last. Viewer tasks are EDIT-safe vs M2's engine tail (zero file overlap) but not BUILD-safe-in-tree (viewer compiles engine via the path dep) — worktree forks are fine at a green engine commit; merge after the M2 tail lands.
