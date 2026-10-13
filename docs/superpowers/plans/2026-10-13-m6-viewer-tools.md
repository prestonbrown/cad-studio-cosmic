# M6 Viewer Tools Implementation Plan (rev 2 — independent review applied: 7M/8m/7N folded; APPROVED_WITH_FINDINGS)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the M3/M4 renderer into the CAD tool surface upstream's ModelPane is — measure with badges and markers, the GPU section with product state (axis/position UI, Y-keep-above parity, arranged-aabb plane, cap color parity), explode/plate/bed layouts driven by per-part GPU pose uniforms (no vertex rewrites), the parts overlay (groups, hide/link/isolate), the compare ghost mesh, and the completed status-bar controls with the 12-swatch legend.

**Architecture:** Viewer-first, spec §4.3/§5: a new pure `viewer/src/tools/` module family (`arrangement`, `section`, `picking`) holds the ModelViewerState/PartArrangement/TriangleMesh math as plain functions over `engine::parsing::Part`, testable headless with no iced types (iced stays confined to `render::scene`). The renderer gets one structural change — a `Frame` bundle replacing `prepare_parts`' growing parameter list — and the per-part transform moves OUT of content identity into per-frame model-uniform writes with draw-skip visibility, which is the uniform path M7's playback reuses (the GpuPart doc's reservation, parts.rs:76-81). Explode/plate offsets, section planes, and visibility are then uniforms and draw ranges: a slider drag writes 64 bytes per part, never a vertex. The app gains a pure `viewer_tools` state machine (ModelViewerState.swift's didSet semantics), the PartsOverlay/section-controls/measure-badge widgets, and the pane wiring that pushes arrangement poses, plate scenes, the bed outline, off-thread measure picks, and the compare ghost.

**Tech Stack:** No new dependencies. Rust edition 2024, wgpu 28 exact, libcosmic rev `dbc62f2d899cdbfbe43d170689ddc0290db663f4`, glam, bytemuck; app uses existing cosmic widgets (button, container, slider, scrollable, text). Engine is NOT modified — `engine::settings::{PartsLayout, Settings}` and `Manifest::bed` already landed (M1/M4).

**Spec:** docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md AS AMENDED — §5 rendering policy (GPU-first; §5's discard+stencil section design IS this plan's section tool), §4.3 viewer crate (`tools/`), §4.4 overlay inventory + status-bar line, §10 M6 plus the M5 scope binding's compare split (M6 owns ONLY the orange ghost-mesh render; the picker and `comparison_parameters` are M5 T5/T7). Behavior source of truth: vendored Swift at fixtures/upstream-source/ — Viewer3D/{ModelViewer,ModelViewerState,PartArrangement,PartsOverlay,ModelPane,CameraInputView}.swift, Shared/PaneStatusBar.swift, ProjectOutput.swift (plate/comparison loading). Parity facts: .superpowers/sdd/m3-viewer-facts.md (cited "facts #N"). Carry-forwards M6 section (docs/research/carry-forwards.md:146-154) is BINDING. Where this plan and a Swift file disagree, the Swift file wins and the discrepancy goes to the controller. Assumes M3, M4 T5, and the M4/M5 tasks each task's Gates line names are merged.

## Global Constraints

- Engine stays untouched (spec §4.2 GUI-free law holds trivially): no file under `engine/` is modified. Settings persistence reuses `engine::settings::{PartsLayout, Settings::{load,save}, settings_path()}` — keys `parts_layout` (default Exploded) and `parts_collapsed` (default false) landed in M1's parity pass (settings.rs:168-169,192-193). ONE writer rule: the app saves through the engine's `settings_path()` only (the pane's settings-path field DEFAULTS to it; tests inject temp paths — never the developer's real config, which parallel pane tests would race).
- GPU-first law (spec §5): explode offsets, section plane, part visibility, and measure markers never rewrite staged vertices. Explode/section/visibility changes write uniforms (64-byte model matrices, camera scalar) and draw ranges only; markers/ghost re-stage only on their own content change (event-driven: a click, a compare-version swap). The upload-once counters must stay flat across an explode slider sweep — `MeshPipeline::upload_count_for_tests` is the enforcement point.
- Viewer purity: `viewer/src/tools/*` imports no iced/cosmic types (spec §4.3 — iced types live only in `render::scene`). All parity math is pure f32 over `engine::parsing::Part` / `PartScene` and unit-testable without a GPU.
- Colors cross the exact piecewise sRGB EOTF exactly once (`render::parts::srgb_to_linear`), the crate's recorded LINEAR-space assumption for NEUTRAL/grid alphas included. New constants: cut face = `systemRed.blended(0.15, black)` blended in sRGB then converted (ModelViewer.swift:363); markers/ghost = systemOrange (255,149,0). Derived linear values are pinned by tests, not comments.
- Frame mapping: MODEL (engine, Z-up) → pinned SCENE (render, Y-up) via `(x, z, −y)` + the floor pin baked in `PartScene::from_parts`. Section/measure/explode math is MODEL-space upstream and stays MODEL-space here; only the plane mapping (`tools::section::scene_plane`), pose offsets, and marker placement cross to scene space.
- User-visible strings use `fl!()` with keys from app/i18n/en/main.ftl (the 336-key corpus — grep the slug, never invent). All M6 keys verified present: `parti`, `mostra-le-parti`, `nascondi-le-parti`, `vista-delle-parti`, `montato`, `esploso`, `piatto`, `distanza-tra-le-parti`, `tieni-unita` (%@), `tieni-unita-al-resto`, `separa` (%@), `separa-dal-resto`, `mostra`, `nascondi`, `l-agente-non-ha-ancora-preparato-il-piatto-di-stampa-le-parti-sono-affiancate-senza-ruotarle-chiedi-di-prepararlo-nella-chat`, `misura`, `misura-la-distanza-tra-due-punti`, `sezione`, `mostra-una-sezione-del-modello`, `griglia`, `mostra-la-griglia`, `wireframe`, `mostra-il-wireframe`, `fai-clic-su-un-punto-del-modello`, `fai-clic-sul-secondo-punto`, `posizione-della-sezione`, `lld-colori` (%lld), `iso`, `fronte`, `alto`, `destra`.
- Upstream quirks are parity, not bugs — port them and name them in the test: hiding every group falls back to showing the assembled model (ModelPane.swift:79-80); the section plane value is recomputed from the ARRANGED aabb (explode moves it, ModelViewerState.swift:116-121); Y keeps the ABOVE side (ModelViewerState.swift:120); layout change always re-frames at the current preset and clears measure points (ModelViewerState.swift:39-45); compare forces assembled layout (ModelViewerState.swift:78-86).
- Implementers run `cargo check -p <crate>` + the task's focused tests locally ONLY (`cargo test -p viewer --lib`, `cargo test -p viewer --test render_headless <name>`, `cargo test -p app --test <file>`); thelio full-suite gates are controller-owned, single-invocation, never stacked, HEAD-verified after pull. GPU tests take the `render::gpu` shared context (never construct their own Instance).
- Edition 2024 (no bare `env::set_var`), libcosmic rev `dbc62f2d899cdbfbe43d170689ddc0290db663f4` + wgpu 28 exact, conventional commits, stage explicit paths only (never `git add -A`/`-u`/`.`, never `git rm`), commit messages simple double-quoted `-m` (no backticks), rustfmt-clean tree, one-module registration per task (each task registers only its own modules in `lib.rs`), parallel-dispatch cap ≤ 5 agents with staggered reviews. No manifest changes → no Cargo.lock churn.

## Review Focus

Five failure modes the spec implies but ordinary task tests might miss — each pinned to its owning task's named test:

1. **Section Y keeps the ABOVE side** — the naive port keeps below on every axis (the M0 spike did; facts #87). X and Z keep `v ≤ plane`, MODEL Y keeps `v ≥ plane`, which under the remap is scene-z keep-below at the NEGATED plane value (T3 `scene_plane_maps_all_three_model_axes` + `y_axis_section_keeps_the_model_above_side`).
2. **An explode drag re-uploads vertices** — M3's residency re-staged on any transform change (parts.rs:76-81, `transform_change_reuploads`); M6's slider would make that a per-tick full re-upload, violating spec §5. A pose sweep must move ink with a flat upload counter, and hiding a part must skip its draw without touching residency (T2 `pose_change_moves_ink_without_uploads`, `hidden_pose_skips_draw_without_reupload`).
3. **The group-name regex mis-groups parts** — `[_\-\s]?\d+$` strips `bolt_2`→`bolt` and `m4`→`m` but a lone `bolt` must stay itself and only >1-member bases become groups; wrong grouping merges unrelated parts' explode offsets and hide-eyes (T1 `group_names_match_upstream_table`, `singleton_bases_stay_ungrouped`).
4. **Hiding every group blanks the canvas** — upstream's empty-arranged fallback shows the assembled model instead (ModelPane.swift:79-80); a port that filters draws without the fallback renders nothing (T10 `hiding_every_group_falls_back_to_assembled`).
5. **Compare turned on mid-explode leaves an exploded scene with a ghost** — upstream forces assembled layout whenever `compareVersion != nil` and disables the picker (ModelViewerState.swift:78-86, PartsOverlay.swift:23); the ghost must also share the CURRENT scene's floor pin, not its own (ModelViewer.swift:135 ghostEntity under content) (T8 `effective_layout_forces_assembled_when_comparing`, T12 `ghost_uses_the_current_scene_pin`).

A sixth, carried explicitly: **flipped-winding STL facets** (carry-forward: stray caps/holes) — the discard is winding-independent and must stay so; the stencil cap may show slivers exactly like upstream's interior shell (accepted, documented). Pinned by T3 `section_discard_is_winding_independent`.

---

### Task 1: `viewer::tools::arrangement` — grouping, explode offsets, plate flat-layout (pure)

**Gates:** none (M3 merged). No render or app changes — dispatches immediately, in parallel with T2/T3.

**Files:**
- Create: `viewer/src/tools/mod.rs`, `viewer/src/tools/arrangement.rs`
- Modify: `viewer/src/lib.rs` (register `pub mod tools;`), `viewer/src/render/parts.rs` (make `remap` `pub(crate)` — one word)
- Test: in-crate `#[cfg(test)] mod tests` at the end of `arrangement.rs` (house pattern: grid.rs)

**Interfaces:**
- Consumes: `engine::parsing::Part` (positions/triangle_colors/aabb), `engine::settings::PartsLayout` (Assembled/Exploded/Plate — reuse, do NOT twin), `render::parts::remap` (pub(crate) from this task).
- Produces (all later tasks — T10 app wiring, T2's offset math consumers):
  - `pub struct PartGroup { pub name: String, pub members: Vec<String>, pub color: Option<[u8; 3]> }` — color = first `Some` triangle color of the first member (PartArrangement.swift:50).
  - `pub fn group_name(part: &str) -> String` — strip `[_\-\s]?\d+$`; empty result falls back to the full name (:35-38).
  - `pub fn groups(parts: &[Part]) -> Vec<PartGroup>` — a base is a group only when >1 member shares it; groups in first-appearance order; `×N` counts derivable from `members.len()` (:40-54).
  - `pub struct Arranged { pub offsets: Vec<[f32; 3]>, pub visible: Vec<bool>, pub plate_scene: bool }` — per input part slot, MODEL-space.
  - `pub fn arrange(model_parts: &[Part], plate_parts: &[Part], layout: PartsLayout, explode: f32, hidden: &BTreeSet<String>, attached: &BTreeSet<String>) -> Arranged` — offsets/visibility per layout (below); `plate_scene` true iff layout is Plate AND `plate_parts` is non-empty (the caller swaps in the plate scene; hidden filtering still applies to it — visibility array is model-parts-shaped then, and the caller re-filters plate parts by group). CALLER CONTRACT in plate mode: the effective aabb feeding section/cap/grid derives from the PLATE scene's aabb mapped back to MODEL, NOT from `effective_model_aabb` over the model parts — upstream's arranged mesh IS the plate mesh (ModelViewerState.swift:116-121); see T10.
  - `pub fn effective_model_aabb(parts: &[Part], arranged: &Arranged) -> ([f32; 3], [f32; 3])` — union of per-part `aabb()` translated by its offset, over VISIBLE slots only (upstream sections/grids against the arranged mesh, which excludes hidden — ModelViewerState.swift:116-121); when no slot is visible it degrades to the all-parts union (the all-hidden fallback's input).
  - `pub fn legend_colors(parts: &[Part]) -> Vec<[u8; 3]>` — distinct triangle colors, first-appearance order (ModelPane.swift:202-213 renders `mesh.colors.prefix(12)`, count = total).
- Explode algorithm verbatim (PartArrangement.swift:95-140): bounds per GROUP over all parts with triangles (hidden groups INCLUDED — facts #58); anchor = **first** maximal bbox volume in first-appearance order (Swift `max(by:)` keeps the first maximum; Rust `max_by` keeps the last — use an explicit fold); `gap = max(length(overall diag) * 0.06, 2)`; others ascending by center distance to anchor center (Rust `sort_by` is stable — ties keep first-appearance order; Swift's sort is not guaranteed stable, a divergence only on exact distance ties, noted in the module docs); attached groups placed as-is; `direction = dominantAxis(groupCenter − anchorCenter)` with the z-priority ladder and the <0.01-all degenerate → +Z (:153-163); `step = max(length(boxDiagonal) * 0.05, 0.5)`; march `travel += step` while any placed box overlaps (margin inflated `margin*0.5` each side), cap travel < 10_000; `offset = direction * (travel + gap) * explode`. Final lift: `lift = (0, 0, assembledFloorMinZ − explodedMinZ)` MODEL (:77-85) added to EVERY offset (anchored parts included — the whole pile sits on the assembled floor); BOTH minZ inputs are over VISIBLE parts only (Swift's mesh is hidden-filtered at that point, PartArrangement.swift:74-85) — hidden parts' post-offset extents do not stretch the lift.
- Plate fallback `flatLayout` (:176-203): gap 5; sort descending by MODEL `size.y`; `row_width = max(sqrt(Σ(size.x+gap)*(size.y+gap)) * 1.3, max size.x)`; new row when `cursor.x > 0 && cursor.x + size.x > row_width`; per-part offset `(cursor.x − min.x, cursor.y − min.y, −min.z)` — the z component FLOORS every part onto the plate (a part floating at MODEL z 40..60 drops to 0..20; PartArrangement.swift:191 is `SIMD3(cursor.x, cursor.y, 0) − part.mesh.minimum`, not an xy-only subtraction); final uniform shift `(−maxX/2, −maxY/2, 0)` folded into every offset (so it stays a transform, not a re-stage).

- [ ] **Step 1: Write the failing tests** — append to `viewer/src/tools/arrangement.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use engine::settings::PartsLayout as PL;
    use std::collections::BTreeSet;

    fn part(name: &str, min: [f32; 3], max: [f32; 3]) -> engine::parsing::Part {
        // one big triangle covering the box corners is enough for aabb math;
        // a second keeps it non-degenerate
        let positions = vec![
            min, max, [min[0], max[1], max[2]],
            min, [max[0], min[1], min[2]], max,
        ];
        engine::parsing::Part {
            name: name.into(),
            positions,
            triangle_colors: vec![Some([1, 2, 3]); 2],
        }
    }

    #[test]
    fn group_names_match_upstream_table() {
        assert_eq!(group_name("bolt_1"), "bolt");
        assert_eq!(group_name("bolt-12"), "bolt");
        assert_eq!(group_name("screw 3"), "screw");
        assert_eq!(group_name("m4"), "m");       // no separator still strips
        assert_eq!(group_name("bolt"), "bolt");  // no trailing digits: intact
        assert_eq!(group_name("2026"), "2026");  // strip leaves EMPTY -> full name returns
    }

    #[test]
    fn singleton_bases_stay_ungrouped() {
        let parts = vec![part("bolt", [0.0; 3], [1.0; 3]), part("nut", [2.0; 3], [3.0; 3])];
        let gs = groups(&parts);
        assert_eq!(
            gs.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            ["bolt", "nut"],
            "a base with one member is not a group"
        );
        let parts = vec![part("bolt_1", [0.0; 3], [1.0; 3]), part("bolt_2", [2.0; 3], [3.0; 3])];
        let gs = groups(&parts);
        assert_eq!(gs.len(), 1);
        assert_eq!(gs[0].name, "bolt");
        assert_eq!(gs[0].members, ["bolt_1", "bolt_2"]);
        assert_eq!(gs[0].color, Some([1, 2, 3]));
    }

    #[test]
    fn explode_pushes_the_small_part_away_from_the_volume_anchor() {
        // base 10x10x10 at origin (volume 1000, anchor), lid 2x2x2 at (5,5,12):
        // lid center is +z of anchor center -> dominant axis +Z, explode 1.
        let parts = vec![
            part("base", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            part("lid", [5.0, 5.0, 12.0], [7.0, 7.0, 14.0]),
        ];
        let arranged = arrange(&parts, &[], PL::Exploded, 1.0, &BTreeSet::new(), &BTreeSet::new());
        assert_eq!(arranged.offsets.len(), 2);
        assert_eq!(arranged.offsets[0], [0.0, 0.0, 0.0], "anchor does not move");
        let off = arranged.offsets[1];
        assert!(off[2] > 0.0 && off[0] == 0.0 && off[1] == 0.0, "pushed along +Z: {off:?}");
        // travel marches until clear of the anchor box + gap: offset.z >= gap,
        // gap = max(length(overall diagonal) * 0.06, 2)
        let (mn, mx) = parts.iter().map(|p| p.aabb()).fold(
            ([f32::MAX; 3], [f32::MIN; 3]),
            |(mut mn, mut mx), (a, b)| {
                for i in 0..3 {
                    mn[i] = mn[i].min(a[i]);
                    mx[i] = mx[i].max(b[i]);
                }
                (mn, mx)
            },
        );
        let diag = (mx[0] - mn[0]).hypot(mx[1] - mn[1]).hypot(mx[2] - mn[2]);
        let gap = (diag * 0.06).max(2.0);
        assert!(off[2] >= gap - 1e-3, "offset at least the gap: {:?} >= {gap}", off[2]);
        // explode 2 doubles the push; explode 0 collapses to assembled (lift 0 here)
        let x2 = arrange(&parts, &[], PL::Exploded, 2.0, &BTreeSet::new(), &BTreeSet::new());
        assert!((x2.offsets[1][2] - 2.0 * off[2]).abs() < 1e-3);
        let x0 = arrange(&parts, &[], PL::Exploded, 0.0, &BTreeSet::new(), &BTreeSet::new());
        assert_eq!(x0.offsets[1], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn explode_floor_lift_repins_the_pile() {
        // anchor base z 0..10; small cube BELOW it z -20..-18 gets pushed
        // FURTHER down on explode — the lift then shifts the whole pile so
        // the exploded minimum sits at the assembled floor (-20).
        let parts = vec![
            part("base", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            part("nut", [4.0, 4.0, -20.0], [6.0, 6.0, -18.0]),
        ];
        let arranged = arrange(&parts, &[], PL::Exploded, 1.0, &BTreeSet::new(), &BTreeSet::new());
        let aabb = effective_model_aabb(&parts, &arranged);
        assert!((aabb.0[2] - (-20.0)).abs() < 1e-3,
            "exploded floor re-pinned to the assembled floor: {:?}", aabb);
        let assembled = arrange(&parts, &[], PL::Assembled, 1.0, &BTreeSet::new(), &BTreeSet::new());
        let a2 = effective_model_aabb(&parts, &assembled);
        assert!((a2.0[2] - (-20.0)).abs() < 1e-3);
        // hidden-lowest: a hidden part pushed further down must NOT stretch the
        // lift (minZ inputs are over VISIBLE parts only, PartArrangement.swift:74-85)
        let mut hidden = BTreeSet::new();
        hidden.insert("bolt_1".to_string());
        let parts_h = vec![
            part("base", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            part("nut", [4.0, 4.0, -20.0], [6.0, 6.0, -18.0]),
            part("bolt_1", [4.0, 4.0, -40.0], [6.0, 6.0, -38.0]),
        ];
        let h = arrange(&parts_h, &[], PL::Exploded, 1.0, &hidden, &BTreeSet::new());
        let haabb = effective_model_aabb(&parts_h, &h);
        assert!((haabb.0[2] - (-20.0)).abs() < 1e-3,
            "hidden lowest part ignored by the lift: {:?}", haabb.0);
    }

    #[test]
    fn attached_groups_stay_put_and_hidden_groups_filter_draws_only() {
        let parts = vec![
            part("base", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            part("lid", [5.0, 5.0, 12.0], [7.0, 7.0, 14.0]),
        ];
        let mut attached = BTreeSet::new();
        attached.insert("lid".to_string());
        let a = arrange(&parts, &[], PL::Exploded, 1.0, &BTreeSet::new(), &attached);
        assert_eq!(a.offsets[1], [0.0, 0.0, 0.0], "attached group is placed as-is");
        let mut hidden = BTreeSet::new();
        hidden.insert("lid".to_string());
        let h = arrange(&parts, &[], PL::Exploded, 1.0, &hidden, &BTreeSet::new());
        assert!(!h.visible[1] && h.visible[0], "hidden filters visibility");
        assert!(h.offsets[1] != [0.0, 0.0, 0.0], "hidden groups still get offsets (collision math)");
        let haabb = effective_model_aabb(&parts, &h);
        assert!((haabb.1[2] - 10.0).abs() < 1e-4, "effective aabb excludes the hidden lid");
    }

    #[test]
    fn flat_layout_rows_and_centers() {
        // three 10x4 slabs: row width from the area formula = max(sqrt(405)*1.3, 10)
        // = 26.16 -> two per row, third wraps -> 2 rows of depth 4 + gap 5
        let parts = vec![
            part("a", [0.0, 0.0, 0.0], [10.0, 4.0, 5.0]),
            part("b", [0.0, 0.0, 0.0], [10.0, 4.0, 5.0]),
            part("c", [0.0, 0.0, 0.0], [10.0, 4.0, 5.0]),
        ];
        let arranged = arrange(&parts, &[], PL::Plate, 1.0, &BTreeSet::new(), &BTreeSet::new());
        assert!(!arranged.plate_scene, "no plate.3mf -> flat fallback transforms");
        assert!(arranged.visible.iter().all(|v| *v));
        let aabb = effective_model_aabb(&parts, &arranged);
        let sx = aabb.1[0] - aabb.0[0];
        let sy = aabb.1[1] - aabb.0[1];
        assert!((sx - 25.0).abs() < 1e-3, "two per row (0..10 + gap + 15..25): {sx}");
        assert!((sy - 13.0).abs() < 1e-3, "two rows (4 + gap 5 + 4): {sy}");
        assert!((aabb.0[0] + aabb.1[0]).abs() < 1e-3, "x centered");
        assert!((aabb.0[1] + aabb.1[1]).abs() < 1e-3, "y centered");
        assert!((aabb.0[2] - 0.0).abs() < 1e-3, "z floored onto the plate (parts already at z=0)");
        // a floating part discriminates the z flooring: MODEL z 40..60 drops to 0..20
        let parts2 = vec![
            part("a", [0.0, 0.0, 0.0], [10.0, 4.0, 5.0]),
            part("f", [0.0, 0.0, 40.0], [10.0, 4.0, 60.0]),
        ];
        let arranged2 = arrange(&parts2, &[], PL::Plate, 1.0, &BTreeSet::new(), &BTreeSet::new());
        let aabb2 = effective_model_aabb(&parts2, &arranged2);
        assert!((aabb2.0[2] - 0.0).abs() < 1e-3, "floating part floored to the plate: {:?}", aabb2.0);
        assert!((aabb2.1[2] - 20.0).abs() < 1e-3, "its height above the plate is preserved");
    }

    #[test]
    fn agent_plate_parts_select_the_plate_scene() {
        let model = vec![part("a", [0.0; 3], [1.0; 3])];
        let plate = vec![part("a", [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        let arranged = arrange(&model, &plate, PL::Plate, 1.0, &BTreeSet::new(), &BTreeSet::new());
        assert!(arranged.plate_scene);
        let assembled = arrange(&model, &plate, PL::Assembled, 1.0, &BTreeSet::new(), &BTreeSet::new());
        assert!(!assembled.plate_scene);
        assert!(assembled.offsets.iter().all(|o| *o == [0.0, 0.0, 0.0]));
    }

    #[test]
    fn legend_colors_dedup_first_appearance() {
        let mut a = part("a", [0.0; 3], [1.0; 3]);
        a.triangle_colors = vec![Some([10, 20, 30]), None, Some([10, 20, 30])];
        let mut b = part("b", [2.0; 3], [3.0; 3]);
        b.triangle_colors = vec![Some([40, 50, 60])];
        assert_eq!(legend_colors(&[a, b]), [([10, 20, 30]), ([40, 50, 60])]);
    }
}
```

- [ ] **Step 2: RED** — `cargo test -p viewer --lib arrangement` → compile error (module missing).
- [ ] **Step 3: Implement** `tools/mod.rs` (module docs + `pub mod arrangement;` + `pub fn legend_colors` if preferred there — keep it in arrangement.rs beside grouping, one home) and `arrangement.rs` per the Interfaces block. `dominant_axis`, `overlaps`, `box_volume`, `center_dist` stay private. Reuse `parts::remap` only if needed for nothing here — arrangement is MODEL-space; the pub(crate) export is consumed by T5's markers and the pane.
- [ ] **Step 4: GREEN** — `cargo test -p viewer --lib` and `cargo test -p viewer --test render_headless` (unchanged, must stay green).
- [ ] **Step 5: Commit** — `git add viewer/src/tools/mod.rs viewer/src/tools/arrangement.rs viewer/src/lib.rs viewer/src/render/parts.rs && git commit -m "feat(viewer): pure part arrangement — grouping, explode offsets, plate flat layout"`

### Task 2: `viewer::render` — Frame bundle + per-part pose uniforms + visibility (GPU-first split)

**Gates:** none (M3 merged). Touches `app/src/viewer_pane.rs` for one compile fix only (the `GpuPart.transform` field disappears) — coordinate with the controller if M5 T5 is mid-flight in a worktree; the change is a two-line drop.

**Files:**
- Modify: `viewer/src/render/parts.rs` (drop `GpuPart.transform`; add `PartScene::pin` + `to_scene`/`to_model`; add `srgb_to_linear_f`), `viewer/src/render/mod.rs` (Frame, PartPose, prepare/draw), `viewer/src/render/scene.rs` (ModelScene carries poses; draw builds Frame), `viewer/tests/render_headless.rs` (harness builds Frame; new tests), `app/src/viewer_pane.rs` (drop the `a.transform` clause in `scenes_differ`, lines 532-541)
- Test: `viewer/src/render/mod.rs` tests + `viewer/tests/render_headless.rs`

**Interfaces:**
- Consumes: T1's nothing (independent); existing residency machinery.
- Produces (T3/T5/T6/T7 extend the struct; T10/T11 app wiring; M7 playback reuses the pose path):
  - `pub type PartPose = Option<[[f32; 4]; 4]>;` — column-major 4x4 (glam `to_cols_array_2d`), `None` = hidden (skip draw + aabb).
  - `pub struct Frame<'a> { pub camera: &'a Camera, pub scene: &'a PartScene, pub poses: &'a [PartPose], pub effective_aabb: ([f32; 3], [f32; 3]), pub grid: Option<&'a GridMesh>, pub options: Options }` — `poses.len()` must equal `scene.parts.len()` (assert).
  - `MeshPipeline::prepare_parts(&mut self, device, queue, size: (u32, u32), frame: &Frame)` — replaces the 8-arg form. Residency identity is `content_hash` ALONE; `ResidentPart` loses its `transform` field. Every prepare writes each visible part's model uniform: `queue.write_buffer(&model_buf, 0, bytemuck::bytes_of(&Model { transform: pose }))` (uniform writes are the sanctioned per-frame path; the upload counters track vertex data only, comment updated to say so).
  - `MeshPipeline::render` unchanged (draws from residency + last-prepared mode); `draw_scene` skips `None` poses.
  - `pub fn PartScene::to_scene(&self, model: [f32; 3]) -> [f32; 3]` (remap + pin) and `pub fn PartScene::to_model(&self, scene: [f32; 3]) -> [f32; 3]` (inverse) — the pin lives in the struct: `pub pin: [f32; 3]`, set by `from_parts`, honored by `pub fn from_parts_with_pin(parts: &[Part], neutral: [f32; 3], pin: [f32; 3]) -> PartScene` (from_parts computes its pin then delegates — T12's ghost passes the CURRENT scene's pin).
  - `pub fn srgb_to_linear_f(c: f32) -> f32` — the f32 form; `srgb_to_linear(c: u8)` delegates (`c as f32 / 255.0`).
  - In-crate test accessors unchanged (`upload_count_for_tests` etc.); add `#[cfg(test)] pub(crate) fn drawn_part_count_for_tests(&self) -> usize`.

- [ ] **Step 1: Write the failing tests** — in `viewer/src/render/mod.rs` tests (replace `transform_change_reuploads` outright — its premise is inverted by this task):

```rust
    #[test]
    fn pose_change_moves_ink_without_uploads() {
        let (device, queue) = gpu::device_queue();
        let parts = golden_3mf_parts();
        let scene = PartScene::from_parts(&parts, NEUTRAL);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let poses: Vec<PartPose> = vec![Some(glam::Mat4::IDENTITY.to_cols_array_2d()); scene.parts.len()];
        let frame = Frame {
            camera: &default_camera(&scene, 4.0 / 3.0),
            scene: &scene,
            poses: &poses,
            effective_aabb: (scene.aabb_min, scene.aabb_max),
            grid: None,
            options: Options::default(),
        };
        pipeline.prepare_parts(device, queue, (800, 600), &frame);
        let uploads = pipeline.upload_count_for_tests();
        // sweep the lid 40mm along +x through the POSE path alone
        let moved: Vec<PartPose> = poses
            .iter()
            .enumerate()
            .map(|(i, p)| {
                i.eq(&1).then(|| {
                    glam::Mat4::from_translation(glam::Vec3::new(40.0, 0.0, 0.0)).to_cols_array_2d()
                }).or(*p).unwrap()
            })
            .collect();
        let frame2 = Frame { poses: &moved, ../* same camera/scene/aabb/grid/options */ };
        pipeline.prepare_parts(device, queue, (800, 600), &frame2);
        assert_eq!(pipeline.upload_count_for_tests(), uploads, "pose-only change uploads zero vertices");
        assert_eq!(pipeline.drawn_part_count_for_tests(), 2);
    }

    #[test]
    fn hidden_pose_skips_draw_without_reupload() {
        let (device, queue) = gpu::device_queue();
        let parts = golden_3mf_parts();
        let scene = PartScene::from_parts(&parts, NEUTRAL);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let id = glam::Mat4::IDENTITY.to_cols_array_2d();
        let poses: Vec<PartPose> = vec![Some(id); scene.parts.len()];
        let frame = Frame {
            camera: &default_camera(&scene, 4.0 / 3.0),
            scene: &scene,
            poses: &poses,
            effective_aabb: (scene.aabb_min, scene.aabb_max),
            grid: None,
            options: Options::default(),
        };
        pipeline.prepare_parts(device, queue, (800, 600), &frame);
        let uploads = pipeline.upload_count_for_tests();
        let hidden: Vec<PartPose> = vec![None, Some(id)];
        let frame2 = Frame { poses: &hidden, ..same_other_fields };
        pipeline.prepare_parts(device, queue, (800, 600), &frame2);
        assert_eq!(pipeline.drawn_part_count_for_tests(), 1);
        assert_eq!(pipeline.upload_count_for_tests(), uploads, "hiding touches no residency");
    }
```

  (`..same_other_fields` is shorthand in THIS plan for re-binding the identical camera/scene/aabb/grid/options locals — the implementer writes the full struct literal; both tests share a small `fn frame<'a>(camera: &'a Camera, scene: &'a PartScene, poses: &'a [PartPose]) -> Frame<'a>` helper in the test module.)

  And in `viewer/tests/render_headless.rs` (extend the harness: `render_frame_opts` builds `let poses = vec![Some(IDENTITY); scene.parts.len()]` and a `Frame` internally — existing call sites and tests unchanged):

```rust
/// The pose path at render scope: translating one part 60mm along scene +x
/// must displace its projected ink. (The upload-count half of the law is
/// asserted in-crate, where the test accessor is visible.)
#[test]
fn pose_translation_displaces_ink() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference();
    let id = glam::Mat4::IDENTITY.to_cols_array_2d();
    let poses: Vec<PartPose> = vec![Some(id); scene.parts.len()];
    let px = render_frame_poses(device, queue, &mut pipeline, &scene, &poses, 800, 600);
    let mut moved = poses.clone();
    moved[1] = Some(glam::Mat4::from_translation(glam::Vec3::new(60.0, 0.0, 0.0)).to_cols_array_2d());
    let px2 = render_frame_poses(device, queue, &mut pipeline, &scene, &moved, 800, 600);
    // centroid of ink: mean (x, y) over pixels differing from the background
    let centroid = |px: &[u8]| -> (f32, f32) {
        let (mut sx, mut sy, mut n) = (0u64, 0u64, 0u64);
        for (i, c) in px.chunks(4).enumerate() {
            if is_ink(c, bg) {
                sx += (i % 800) as u64;
                sy += (i / 800) as u64;
                n += 1;
            }
        }
        (sx as f32 / n as f32, sy as f32 / n as f32)
    };
    let (c1, c2) = (centroid(&px), centroid(&px2));
    assert!(c2.0 > c1.0 + 20.0, "lid ink moved right: {c1:?} -> {c2:?}");
    assert!(c2.0 < c1.0 + 200.0, "and the base stayed put (mixed centroid)");
}
```

  (`render_frame_poses` — the harness extension: `render_frame_opts` plus a `poses` parameter threaded into its `Frame`.)

- [ ] **Step 2: RED** — `cargo test -p viewer --lib pose_change` + `cargo test -p viewer --test render_headless pose_translation` → fail (no `Frame`/`PartPose`).
- [ ] **Step 3: Implement** — the `Frame` struct in mod.rs; `prepare_parts` re-signed; `ResidentPart` drops `transform`; `draw_scene` reads a `visible_poses` snapshot stored at prepare (store `drawn: Vec<[[f32;4];4]>` — the Some poses in slot order — and iterate it at draw, binding each slot's model group). `scene.rs`: `ModelScene` gains `pub poses: Arc<[PartPose]>` (identity-initialized in `new`/`replace_scene`, resized to the scene length) and `draw` builds the `Frame` with `effective_aabb` defaulting to the scene aabb (T10 replaces it). `app/src/viewer_pane.rs`: delete the `|| a.transform != b.transform` clause in `scenes_differ`.
- [ ] **Step 4: GREEN** — `cargo test -p viewer` (lib + both integration files), `cargo test -p app` (compile + existing pane tests).
- [ ] **Step 5: Commit** — `git add viewer/src/render/parts.rs viewer/src/render/mod.rs viewer/src/render/scene.rs viewer/tests/render_headless.rs app/src/viewer_pane.rs && git commit -m "feat(viewer): per-part pose uniforms and draw-skip visibility — the GPU-first transform path"`

### Task 3: `viewer::tools::section` — MODEL-axis plane mapping + Y-keep-above + cap color parity

**Gates:** T2 merged (cap_quad reads `Frame.effective_aabb`).

**Files:**
- Create: `viewer/src/tools/section.rs`
- Modify: `viewer/src/tools/mod.rs` (register), `viewer/src/render/mod.rs` (`CUT_FACE_LINEAR` + `cap_wgsl_source()` substitution + `cap_quad` over `frame.effective_aabb`), `viewer/src/render/cap.wgsl` (markers), `viewer/tests/render_headless.rs`
- Test: in-crate section tests + headless render tests

**Interfaces:**
- Consumes: T2's `Frame.effective_aabb`.
- Produces (T10's pane computes this on every slider/explode change):
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum SectionAxis { X, Y, Z }` with `pub fn title(self) -> &'static str` ("X"/"Y"/"Z", ModelViewerState.swift:9-15) and `pub const ALL: [SectionAxis; 3]`.
  - `pub const SLIDER_MIN: f32 = 0.001; pub const SLIDER_MAX: f32 = 0.999;` (ModelPane.swift:191).
  - `pub fn plane_value(axis: SectionAxis, position: f32, model_aabb: ([f32; 3], [f32; 3])) -> f32` — `min[axis] + (max[axis] − min[axis]) * position` (ModelViewerState.swift:119).
  - `pub fn scene_plane(axis: SectionAxis, position: f32, model_aabb: ([f32; 3], [f32; 3])) -> (u32, f32)` — the SCENE-space (axis, pos) for the shader's keep-below `discard` (mesh.wgsl:58-63 stays UNCHANGED): X → `(0, V)`; Z → `(1, V)`; Y → `(2, −V)` because MODEL-above (`model_y ≥ V`) is scene-below at `scene_z = −V` under the `(x, z, −y)` remap. `keepsBelow = axis != Y` (ModelViewerState.swift:120) closes into the mapping — no shader flag exists or is needed.
  - `pub const CUT_FACE_LINEAR: [f32; 3] = [0.6920, 0.0321, 0.0220];` in `render::mod` — `systemRed.blended(withFraction: 0.15, of: .black)` = sRGB `(0.85, 0.85·59/255, 0.85·48/255)` through the exact EOTF (ModelViewer.swift:363). `cap_wgsl_source()` substitutes `@CAP_R@/@CAP_G@/@CAP_B@` from the Rust const (the mesh_wgsl_source pattern — a missed marker is invalid WGSL and fails loudly).
  - `cap_quad(frame: &Frame) -> [CapVertex; 4]` — same 10%-grown quad construction, but over `frame.effective_aabb` and the scene-mapped axis (the Camera uniform already carries scene axis/pos).

- [ ] **Step 1: Write the failing tests** — `viewer/src/tools/section.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const AABB: ([f32; 3], [f32; 3]) = ([-10.0, -6.0, 0.0], [10.0, 6.0, 20.0]);

    #[test]
    fn plane_value_interpolates_the_model_aabb() {
        assert!((plane_value(SectionAxis::X, 0.5, AABB)).abs() < 1e-6);
        assert!((plane_value(SectionAxis::Z, 0.25, AABB) - 5.0).abs() < 1e-6);
        assert!((plane_value(SectionAxis::Y, 0.75, AABB) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn scene_plane_maps_all_three_model_axes() {
        // X and Z keep below at the plain value on their scene axes;
        // Y keeps MODEL-above = scene-below at the NEGATED value on scene z.
        assert_eq!(scene_plane(SectionAxis::X, 0.5, AABB), (0, 0.0));
        assert_eq!(scene_plane(SectionAxis::Z, 0.25, AABB), (1, 5.0));
        assert!((scene_plane(SectionAxis::Y, 0.75, AABB).1 - (-3.0)).abs() < 1e-6);
        assert_eq!(scene_plane(SectionAxis::Y, 0.75, AABB).0, 2);
    }

    #[test]
    fn cut_face_color_is_blended_system_red_through_the_eotf() {
        let srgb = [0.85_f32, 0.85 * 59.0 / 255.0, 0.85 * 48.0 / 255.0];
        for i in 0..3 {
            assert!(
                (crate::render::CUT_FACE_LINEAR[i] - crate::render::parts::srgb_to_linear_f(srgb[i])).abs() < 1e-3,
                "channel {i}: {} vs {}",
                crate::render::CUT_FACE_LINEAR[i],
                crate::render::parts::srgb_to_linear_f(srgb[i])
            );
        }
    }
}
```

  And in `viewer/tests/render_headless.rs`:

```rust
/// MODEL-Y section keeps the ABOVE half (facts #84-87): a box offset in
/// model y, cut at its midpoint, must keep the model-y>=V half — which the
/// scene-plane mapping expresses as scene-z keep-below at -V. Viewed from
/// the iso camera, the kept ink must sit entirely on the projected side of
/// the kept half (checked against an analytically projected half-space).
#[test]
fn y_axis_section_keeps_the_model_above_side() {
    let (device, queue) = gpu::device_queue();
    // MODEL box x[-20,20] y[-20,-4] z[0,20] -> cut at y mid (-12) keeps
    // y in [-12,-4]; scene: x[-20,20] y[0,20] z[4,20], kept scene z in [4,12]
    let scene = box_scene([[-20.0, -20.0, 0.0], [20.0, -4.0, 20.0]]);
    let (axis, pos) = viewer::tools::section::scene_plane(
        viewer::tools::section::SectionAxis::Y, 0.5, scene_model_aabb(&scene));
    let mut cam = default_camera(&scene, 800.0 / 600.0);
    cam.section_enable = 1.0; cam.section_axis = axis; cam.section_pos = pos;
    let px = render_frame(device, queue, &mut pipeline, &scene, &cam, 800, 600);
    // every ink pixel projects back (view_proj inverse) to a world point with
    // world.z <= 12 + eps: the kept half. A keep-below-everywhere port would
    // keep scene z in [4,8] instead — distinguish by the projected footprint.
    let (footprint_lo, footprint_hi) = z_footprint_of_ink(&px, &cam, 800, 600);
    assert!(footprint_hi <= 12.0 + 1e-3, "kept half is model-above: z <= 12, got {footprint_hi}");
    assert!(footprint_hi > 10.0, "and it is the upper kept slab, not the below one");
}

/// The discard does not depend on winding: flip one part's triangle windings
/// and the above-plane pixels still disappear (carry-forward M6 line 1).
#[test]
fn section_discard_is_winding_independent() {
    let (device, queue) = gpu::device_queue();
    let mut parts = golden_3mf_parts();
    for t in parts[0].positions.chunks_exact_mut(3) { t.swap(1, 2); } // rewind the base
    let scene = PartScene::from_parts(&parts, NEUTRAL);
    // section at the top edge of the aabb: almost everything discards
    let mut cam = default_camera(&scene, 800.0 / 600.0);
    cam.section_enable = 1.0; cam.section_axis = 1;
    cam.section_pos = scene.aabb_max[1] - 0.5;
    let px = render_frame(device, queue, &mut pipeline, &scene, &cam, 800, 600);
    assert!(ink_pixels(&px, bg_ref()) < 200, "rewound part still discards");
}

/// The cap reads as the blended systemRed through the EOTF, not the spike's
/// brighter red: sample a pixel at the projected center of the cut face.
#[test]
fn cap_color_matches_blended_system_red() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut cam = default_camera(&scene, 800.0 / 600.0);
    cam.section_enable = 1.0;
    cam.section_axis = 1;
    cam.section_pos = 4.0; // mid-base cut (the established `base` plane)
    let px = render_frame(device, queue, &mut pipeline, &scene, &cam, 800, 600);
    // the cut face's center: project the MODEL box center of the base through
    // the camera, then step ONTO the plane (scene y = 4) at that x/z
    let center = project_px(cam.view_proj, [0.0, 4.0, 0.0], 800.0, 600.0).unwrap();
    let lin = decode_linear(&px[(center.1 as usize * 800 + center.0 as usize) * 4..][..4].try_into().unwrap());
    for i in 0..3 {
        assert!((lin[i] - viewer::render::CUT_FACE_LINEAR[i]).abs() < 0.05, "channel {i}: {lin:?}");
    }
}
```

  (Write `box_scene`, and `z_footprint_of_ink` concretely — render from `Preset::Top` where scene z maps to screen x, and compare the ink's screen-x footprint against the projected kept slab via `project_px` (render_headless.rs:951): the kept half spans scene z ∈ [4, 12], so the ink's screen-x extent must equal that slab's projection and NOT the below half's [4, 8].)

- [ ] **Step 2: RED** — module missing / cap color assert fails.
- [ ] **Step 3: Implement** — section.rs; CUT_FACE_LINEAR + cap_wgsl_source; cap_quad over frame. Update `cap.wgsl` to carry the `@CAP_@@` markers. If `cap_red_pixels` thresholds regress (they should not: blended red still encodes to r≈217 > 180, g≈50 < 150, b≈41 < 120), adjust the helper's comment, not its bounds.
- [ ] **Step 4: GREEN** — `cargo test -p viewer`.
- [ ] **Step 5: Commit** — `git add viewer/src/tools/mod.rs viewer/src/tools/section.rs viewer/src/render/mod.rs viewer/src/render/cap.wgsl viewer/tests/render_headless.rs && git commit -m "feat(viewer): section plane mapping with Y keep-above parity and blended-red cap"`

### Task 4: `camera::pick_ray` + `viewer::tools::picking` — CPU measure ray (pure)

**Gates:** T2 merged (uses `PartScene::pin`, `to_model`).

**Files:**
- Create: `viewer/src/tools/picking.rs`
- Modify: `viewer/src/tools/mod.rs` (register), `viewer/src/render/mod.rs` (camera module gains `pick_ray`)
- Test: in-crate both files

**Interfaces:**
- Consumes: T2's `PartScene::{pin, to_model}`; `Frame`'s pose semantics (`PartPose`).
- Produces (T11's click flow):
  - `pub fn camera::pick_ray(distance: f32, yaw: f32, pitch: f32, target: [f32; 3], aspect: f32, cursor: (f32, f32), view: (f32, f32)) -> ([f32; 3], [f32; 3])` — scene-space (origin, direction): eye = `target + dir(yaw,pitch)·distance` (the placeCamera formula); basis `f = normalize(target − eye)`, `r = normalize(cross(f, (0,1,0)))`, `u = cross(r, f)`; `tangent = tan(FOV/2)`; `x = (2·cx/w − 1)·tangent·aspect`; `y_appkit = h − cy` (iced's cursor is top-down, AppKit's bottom-up — facts #29); `y = (2·y_appkit/h − 1)·tangent`; `direction = normalize(x·r + y·u + f)` (upstream's `camera.convert((x, y, −1))`).
  - `pub fn picking::first_hit(scene: &PartScene, poses: &[PartPose], section: Option<(u32, f32)>, origin: [f32; 3], dir: [f32; 3]) -> Option<[f32; 3]>` — Möller–Trumbore per triangle, two-sided (`|det| > 1e-9`), nearest `t > 0`, over every part with `poses[i] = Some(m)` (hit point = `m.transform_point3(staged)`); hits failing the keep-below section test (`world[axis] > pos` → rejected) are skipped so the nearest SURVIVING hit wins (parity with picking the clipped mesh, minus cap-surface hits which upstream's clipped mesh also lacks — facts #76). Returns MODEL coords via the per-part inverse pose (`Mat4::from_cols_2d(m).inverse()`) then `to_model` (general for M7's rotated poses; today's poses are translations).

- [ ] **Step 1: Write the failing tests** — `viewer/src/tools/picking.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{NEUTRAL, PartScene};
    use glam::{Mat4, Vec3};

    fn ground_tri_scene() -> PartScene {
        // one MODEL triangle z=0 spanning (-10,-10)..(10,10), neutral color
        let part = engine::parsing::Part {
            name: "t".into(),
            positions: vec![
                [-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [0.0, 10.0, 0.0],
            ],
            triangle_colors: vec![None],
        };
        PartScene::from_parts(&[part], NEUTRAL)
    }

    #[test]
    fn ray_hits_reports_model_coords_round_trip() {
        let scene = ground_tri_scene();
        let poses = vec![Some(Mat4::IDENTITY.to_cols_array_2d())];
        // straight down the MODEL z axis at the centroid: scene ray -y
        let origin = [0.0, 50.0, 0.0]; // scene: above the staged triangle
        let hit = first_hit(&scene, &poses, None, origin, [0.0, -1.0, 0.0]).unwrap();
        // MODEL centroid of the triangle, z=0: scene_point maps back exactly
        let model = [0.0, -10.0f32 / 3.0, 0.0];
        for i in 0..3 {
            assert!((hit[i] - model[i]).abs() < 1e-4, "hit {hit:?} vs {model:?}");
        }
    }

    #[test]
    fn two_sided_and_nearest() {
        let scene = ground_tri_scene();
        let poses = vec![Some(Mat4::IDENTITY.to_cols_array_2d())];
        // two-sided: the same triangle picked from below (its back face)
        let below = first_hit(&scene, &poses, None, [0.0, -50.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(below.is_some(), "back face is pickable (|det| > 1e-9)");
        // nearest: two identical parts, one lifted 10mm through its pose —
        // a ray from above must hit the lifted (smaller t) one first
        let parts = vec![ground_tri_part(), ground_tri_part()];
        let scene2 = PartScene::from_parts(&parts, NEUTRAL);
        let poses2 = vec![
            Some(Mat4::from_translation(Vec3::new(0.0, 10.0, 0.0)).to_cols_array_2d()),
            Some(Mat4::IDENTITY.to_cols_array_2d()),
        ];
        let hit = first_hit(&scene2, &poses2, None, [0.0, 50.0, 0.0], [0.0, -1.0, 0.0]);
        // both staged at the same place; the lifted part's hit un-maps to the
        // SAME model point — assert the ray reports a hit whose t is the near
        // one by checking a ray that ONLY the far part can answer when the
        // near one is hidden:
        assert!(hit.is_some());
        let far_only = {
            let mut p = poses2.clone();
            p[0] = None; // hide the lifted part
            first_hit(&scene2, &p, None, [0.0, 50.0, 0.0], [0.0, -1.0, 0.0])
        };
        assert!(far_only.is_some(), "hidden filter + fallback hit both work");
    }

    fn ground_tri_part() -> engine::parsing::Part {
        engine::parsing::Part {
            name: "t".into(),
            positions: vec![[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [0.0, 10.0, 0.0]],
            triangle_colors: vec![None],
        }
    }

    #[test]
    fn section_filter_removes_discarded_hits() {
        let scene = ground_tri_scene();
        let poses = vec![Some(Mat4::IDENTITY.to_cols_array_2d())];
        // staged scene y in [0, epsilon]; section keep-below at y = -1 discards all
        assert!(first_hit(&scene, &poses, Some((1, -1.0)), [0.0, 50.0, 0.0], [0.0, -1.0, 0.0]).is_none());
        assert!(first_hit(&scene, &poses, Some((1, 5.0)), [0.0, 50.0, 0.0], [0.0, -1.0, 0.0]).is_some());
    }

    #[test]
    fn hidden_parts_and_misses() {
        let scene = ground_tri_scene();
        assert!(first_hit(&scene, &[None], None, [0.0, 50.0, 0.0], [0.0, -1.0, 0.0]).is_none());
        assert!(first_hit(&scene, &[Some(Mat4::IDENTITY.to_cols_array_2d())], None, [0.0, 50.0, 0.0], [1.0, 0.0, 0.0]).is_none(), "parallel ray misses");
    }
}
```

  And in `render/mod.rs`'s camera tests:

```rust
    #[test]
    fn pick_ray_center_points_at_the_target() {
        // yaw 0, pitch 0: eye = target + dist*(0,0,1); center pixel aims at f
        let (o, d) = camera::pick_ray(100.0, 0.0, 0.0, [0.0, 5.0, 0.0], 4.0 / 3.0, (400.0, 300.0), (800.0, 600.0));
        assert!((o[0] - 0.0).abs() < 1e-4 && (o[1] - 5.0).abs() < 1e-4 && (o[2] - 100.0).abs() < 1e-4);
        let dot = glam::Vec3::from_array(d).dot(glam::Vec3::new(0.0, 0.0, -1.0));
        assert!(dot > 0.999, "center ray along the view axis: {d:?}");
        // right edge (x=799) tilts toward +x; BOTTOM-left corner tilts -x -y
        // (iced y is top-down: cursor y 599 flips to AppKit y 1 = bottom,
        // which aims DOWN in any bottom-up convention)
        let (_, d2) = camera::pick_ray(100.0, 0.0, 0.0, [0.0, 5.0, 0.0], 4.0 / 3.0, (799.0, 300.0), (800.0, 600.0));
        assert!(d2[0] > 0.1, "right edge aims right: {d2:?}");
        let (_, d3) = camera::pick_ray(100.0, 0.0, 0.0, [0.0, 5.0, 0.0], 4.0 / 3.0, (0.0, 599.0), (800.0, 600.0));
        assert!(d3[0] < 0.0 && d3[1] < 0.0, "bottom-left aims left and DOWN (iced bottom = AppKit bottom after the flip): {d3:?}");
    }
```

- [ ] **Step 2: RED** → **Step 3: Implement** — `first_hit` loops parts and triangles; per part, precompute `Mat4::from_cols_2d` + `.inverse()` once, transform the ray origin (`transform_point3`) and direction (`transform_vector3` + renormalize) into staged space, run Möller–Trumbore, keep the nearest `t > 0` whose world point passes the section test, and map the winner back through the inverse pose + `to_model`.
- [ ] **Step 4: GREEN** — `cargo test -p viewer --lib`.
- [ ] **Step 5: Commit** — `git add viewer/src/tools/mod.rs viewer/src/tools/picking.rs viewer/src/render/mod.rs && git commit -m "feat(viewer): measure pick ray and two-sided nearest section-aware hit test"`

### Task 5: `viewer::render::markers` — orange measure markers

**Gates:** T2 merged (Frame extension).

**Files:**
- Create: `viewer/src/render/markers.rs`
- Modify: `viewer/src/render/mod.rs` (Frame gains `markers: Option<&MarkerMesh>`; draw after the grid), `viewer/src/render/scene.rs` (`ModelScene.markers: Option<Arc<MarkerMesh>>`), `viewer/tests/render_headless.rs`
- Test: in-crate markers tests + one headless render test

**Interfaces:**
- Consumes: T2's Frame; the grid pipeline (unlit, blended, depth-tested — markers ride it: no new pipeline).
- Produces (T11):
  - `pub const ORANGE: [f32; 3] = [1.0, 0.3005, 0.0];` — linear systemOrange (255,149,0) via `srgb_to_linear_f` (test-pinned, not hand-waved).
  - `pub fn marker_radius(framing_radius: f32) -> f32` — `max(framing_radius * 0.03, 0.4)` (ModelViewer.swift:383).
  - `pub struct MarkerMesh { pub vertices: Vec<GridVertex>, pub content_hash: u64 }` + `pub fn geometry(points: &[[f32; 3]], radius: f32) -> MarkerMesh` — points in PINNED-SCENE coords: per point a UV sphere (24 segments × 16 rings — documented tessellation stand-in for RealityKit's `generateSphere`; a marker is ~3% of the model radius, tessellation is invisible), plus when `points.len() == 2` and `dist > 0` a connecting box: cross-section `radius*0.5` square, length = distance, centered at the midpoint, oriented by `Quat::from_rotation_arc(Vec3::Z, normalize(p1 − p0))` (ModelViewer.swift:391-397). All vertices `[ORANGE, 1.0]`. Zero/one point draws spheres only; duplicate points skip the link (`length > 0` guard).
  - Residency mirrors `ResidentGrid` (content-hash keyed) — `MeshPipeline.marker: Option<ResidentGrid>`.

- [ ] **Step 1: Write the failing tests** — in `markers.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_clamps_at_four_tenths() {
        assert!((marker_radius(10.0) - 0.4).abs() < 1e-6);
        assert!((marker_radius(1000.0) - 30.0).abs() < 1e-4);
    }

    #[test]
    fn one_point_one_sphere_two_points_add_the_link() {
        let one = geometry(&[[0.0, 5.0, 0.0]], 2.0);
        let sphere_tris = 16 * 24 * 2;
        assert_eq!(one.vertices.len(), sphere_tris * 3);
        let two = geometry(&[[0.0, 5.0, 0.0], [30.0, 5.0, 0.0]], 2.0);
        assert_eq!(two.vertices.len(), (sphere_tris + 12) * 3, "plus the 12-triangle box");
        // every vertex orange, alpha 1
        assert!(two.vertices.iter().all(|v| v.color == [ORANGE[0], ORANGE[1], ORANGE[2], 1.0]));
        // coincident points: spheres but no degenerate link
        let coincident = geometry(&[[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]], 2.0);
        assert_eq!(coincident.vertices.len(), sphere_tris * 2 * 3, "two spheres, no link");
    }

    #[test]
    fn link_box_spans_the_two_points() {
        let g = geometry(&[[0.0, 5.0, 0.0], [30.0, 5.0, 0.0]], 2.0);
        let xs: Vec<f32> = g.vertices[sphere_len..].iter().map(|v| v.pos[0]).collect();
        assert!(xs.iter().cloned().fold(f32::MIN, f32::max) >= 29.0);
        assert!(xs.iter().cloned().fold(f32::MAX, f32::min) <= 1.0);
    }
}
```

  And headless (`render_headless.rs`): `markers_render_orange_at_projected_points` — golden scene, two MODEL points picked on the base's top face (their `to_scene` projections run through `project_px` with the frame's camera); assert ≥50 ink pixels within 12px of EACH projected point with chroma within 0.15 of linear ORANGE (the `chroma_dist` helper exists), and none before markers were set.

- [ ] **Step 2: RED** → **Step 3: Implement** — sphere generator: per ring r in 1..=rings-1, per segment s: two triangles between ring r and r+1 (pole caps as fans); unit sphere scaled by radius, translated to the point. `theta = PI * r / rings`, `phi = 2PI * s / segments`, standard UV mapping.
- [ ] **Step 4: GREEN** — `cargo test -p viewer`.
- [ ] **Step 5: Commit** — `git add viewer/src/render/markers.rs viewer/src/render/mod.rs viewer/src/render/scene.rs viewer/tests/render_headless.rs && git commit -m "feat(viewer): orange measure markers — spheres and connecting box on the overlay pass"`

### Task 6: `viewer::render` — the compare ghost line pass

**Gates:** T2 merged.

**Files:**
- Create: `viewer/src/render/ghost.wgsl`
- Modify: `viewer/src/render/mod.rs` (`ghost.wgsl` pipeline + `Frame.ghost: Option<&PartScene>` + `ghost_residents`), `viewer/src/render/scene.rs` (`ModelScene.ghost: Option<Arc<PartScene>>`), `viewer/tests/render_headless.rs`
- Test: headless render tests

**Interfaces:**
- Consumes: T2's `from_parts_with_pin`; the edge lists `GpuPart.edges` (already staged).
- Produces (T12):
  - `Frame.ghost: Option<&'a PartScene>` — a PartScene staged from the comparison model WITH THE CURRENT SCENE'S PIN (`from_parts_with_pin(&ghost_parts, NEUTRAL, current_scene.pin)`) — upstream's ghost entity hangs under `content`, so the old mesh renders in the CURRENT floor/center frame (ModelViewer.swift:135,176-179); staging it with its own pin would float it — test-pinned.
  - Ghost residency: `Vec<ResidentPart>`-shaped (vertex + edge buffers, content-hash keyed, whole-scene rebuild on identity change — event-driven, a compare-version swap).
  - `ghost.wgsl`: vertex `camera.view_proj * vec4(pos, 1)`; fragment `vec4(ORANGE, 0.6)` — unlit systemOrange, straight alpha 0.6 (ModelViewer.swift:374-378), LINE topology, camera-only bind group (the `camera_pass_layout` the cap/grid use), depth-test ON / depth-write OFF, the same blend as the grid pipeline. Drawn after the cap, before the grid: opaque mesh → cap → ghost → overlay.
  - `pub const GHOST_ALPHA: f32 = 0.6;` (markers::ORANGE reused for the rgb).

- [ ] **Step 1: Write the failing tests** — `viewer/tests/render_headless.rs`:

```rust
/// The ghost: orange wireframe at 0.6 over the background, drawn from the
/// comparison scene's EDGE list, sharing the current scene's pin.
#[test]
fn ghost_renders_orange_wireframe_blended() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let bg = background_reference();
    let bg_linear = decode_linear(&bg);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    // a ghost offset in MODEL x by +40mm: its right edge lands where only
    // the ghost draws; its interior stays background (lines, not faces)
    let ghost = PartScene::from_parts_with_pin(&ghost_parts_shifted_x(40.0), NEUTRAL, scene.pin);
    let px = render_frame_ghost(device, queue, &mut pipeline, &scene, &ghost, 800, 600);
    // locate ghost-only ink: pixels right of the model's projected max x
    // that differ from the background
    let ghost_px: Vec<usize> = (0..px.len() / 4)
        .filter(|&i| is_ink(&px[i * 4..i * 4 + 4], bg) && (i % 800) > 500)
        .collect();
    assert!(ghost_px.len() > 20, "ghost edges present right of the model");
    let i = ghost_px[0];
    let lin = decode_linear(&px[i * 4..i * 4 + 4]);
    for c in 0..3 {
        let mixed = 0.6 * viewer::render::markers::ORANGE[c] + 0.4 * bg_linear[c];
        assert!((lin[c] - mixed).abs() < 0.06, "channel {c}: {lin:?} vs {mixed}");
    }
    // lines, not faces: a pixel inside the ghost's projected silhouette but
    // off every edge reads pure background (find one: the geometric center
    // of the ghost's projected aabb, which the shifted box's edges avoid)
    let center = projected_ghost_center(&scene, &ghost, &cam, 800, 600);
    assert!(chroma_dist(decode_linear(&px_at(&px, center)), bg_linear) < 0.1, "no filled faces");
}

#[test]
fn ghost_uses_the_current_scene_pin() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let ghost_parts = parts_box_model([[0.0, 0.0, 40.0], [10.0, 10.0, 60.0]]); // floats high
    // CURRENT pin (assembled floor at scene y=0): staged aabb min y = 40.
    // Its OWN pin would re-floor it to 0 — this is the discriminating assert.
    let ghost = PartScene::from_parts_with_pin(&ghost_parts, NEUTRAL, scene.pin);
    assert!((ghost.aabb_min[1] - 40.0).abs() < 1e-3, "current pin kept: {:?}", ghost.aabb_min);
    let own = PartScene::from_parts(&ghost_parts, NEUTRAL);
    assert!((own.aabb_min[1] - 0.0).abs() < 1e-3, "control: own pin re-floors");
}

#[test]
fn ghost_reuploads_only_on_content_change() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let ghost = PartScene::from_parts_with_pin(&ghost_parts_shifted_x(40.0), NEUTRAL, scene.pin);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    render_frame_ghost(device, queue, &mut pipeline, &scene, &ghost, 800, 600);
    let u1 = pipeline.ghost_upload_count_for_tests();
    render_frame_ghost(device, queue, &mut pipeline, &scene, &ghost, 800, 600);
    assert_eq!(pipeline.ghost_upload_count_for_tests(), u1, "same ghost content: zero re-uploads");
    let other = PartScene::from_parts_with_pin(&ghost_parts_shifted_x(80.0), NEUTRAL, scene.pin);
    render_frame_ghost(device, queue, &mut pipeline, &scene, &other, 800, 600);
    assert!(pipeline.ghost_upload_count_for_tests() > u1, "new ghost content restages");
}
```

- [ ] **Step 2: RED** → **Step 3: Implement** (write `ghost_parts_shifted_x`, `parts_box_model` — golden parts with a translation baked, or a box Part built like `cube_part` — plus `render_frame_ghost` (the harness's Frame with `ghost: Some(&ghost)`), `px_at`, and `projected_ghost_center` via `project_px` concretely; expose `#[cfg(test)] pub(crate) fn ghost_upload_count_for_tests` on MeshPipeline — integration tests are a separate crate, so the accessor must be `pub` under `#[cfg(test)]`... integration tests build the lib WITHOUT `cfg(test)`: mirror the existing suite's answer — the current accessors are `pub(crate)` + `#[cfg(test)]` and only the IN-CRATE tests use them. So put the ghost upload-count assertions in-crate (mod.rs tests) and keep the integration file to pixel asserts. Adjust: move `ghost_reuploads_only_on_content_change` into mod.rs's test module using `golden_3mf_parts`.)
- [ ] **Step 4: GREEN** — `cargo test -p viewer`.
- [ ] **Step 5: Commit** — `git add viewer/src/render/ghost.wgsl viewer/src/render/mod.rs viewer/src/render/scene.rs viewer/tests/render_headless.rs && git commit -m "feat(viewer): compare ghost — unlit orange wireframe pass sharing the scene pin"`

### Task 7: grid bed wiring + tripod corner pin + MSAA-through-resolve grid alpha

**Gates:** T2 merged (scene.rs Frame plumbing).

**Files:**
- Modify: `viewer/src/render/grid.rs` (tripod y at +0.05; test), `viewer/src/render/scene.rs` (`ModelScene.bed` + `set_overlay_frame`), `viewer/tests/render_headless.rs` (msaa+grid test)
- Test: grid.rs in-crate + headless

**Interfaces:**
- Consumes: T1's `effective_model_aabb` (the pane passes it in — grid geometry's aabb argument already exists).
- Produces (T10):
  - `ModelScene::set_overlay_frame(&mut self, aabb: ([f32; 3], [f32; 3]), bed: Option<[f64; 2]>)` — stores both; rebuilds `grid::geometry(aabb, bed)` when either changed (upstream rebuilds the grid when sceneID — mesh+layout+plateID — changes, NOT on explode/hidden changes; the pane calls this exactly on those triggers — T10 keeps a frozen `grid_aabb` and stamps the call — reproducing the quirk that the grid size freezes at layout-switch-time explode).
  - `ModelScene.bed: Option<[f64; 2]>` — kept across `replace_scene` (the new grid derives from the stored bed).
  - Tripod corner y becomes `0.05` (ModelViewer.swift:451 — `SIMD3(-fpX/2 - margin, 0.05, +|fpZ|/2 + margin)`; grid.rs:221 currently pins 0.0) with `const TRIPOD_Y: f32 = 0.05;`.
  - Strikes carry-forward lines: "No test pins the tripod corner's z placement" and "msaa 4× composited over the alpha-blended grid is untested through the resolve path" (docs/research/carry-forwards.md:151-154 — the T13 wrap strikes them formally; this task lands the tests).

- [ ] **Step 1: Write the failing tests** — in `grid.rs` tests:

```rust
    /// The tripod corner (facts #36, ModelViewer.swift:449-455): x at half
    /// the MODEL x span + margin, z at half the MODEL y span (a GROUND-plane
    /// span — the remap puts model y on scene z) + margin, y at 0.05.
    #[test]
    fn tripod_corner_pinned_at_model_y_half_extent() {
        // scene aabb: x span 40, y span 20 (model height), z span 60 (model y)
        let g = geometry(([-20.0, 0.0, -30.0], [20.0, 20.0, 30.0]), None);
        let margin = (60.0f32 * 0.15).max(3.0); // footprint = max(40, 60) = 60
        let corner = [-20.0 - margin, TRIPOD_Y, 30.0 + margin];
        // the +X (red) tripod box's center sits at corner + x*len/2 with the
        // right size; find it among the box vertices
        let len = (60.0f32 * 0.3).max(5.0);
        let expected_center = [corner[0] + len / 2.0, TRIPOD_Y, corner[2]];
        assert!(g.vertices[n_lines_of(&g)..].iter().any(|v| {
            (v.pos[0] - expected_center[0]).abs() < 1e-3
                && (v.pos[1] - expected_center[1]).abs() < 1e-3
                && (v.pos[2] - expected_center[2]).abs() < 1e-3
        }), "tripod grows from the pinned corner {corner:?}");
        // the axis boxes have thickness max(len/40, 0.6) centered at TRIPOD_Y:
        // they legitimately dip to TRIPOD_Y - thickness
        let thickness = (len / 40.0f32).max(0.6);
        assert!(g.vertices.iter().skip(n_lines_of(&g)).all(|v| v.pos[1] >= TRIPOD_Y - thickness - 1e-6));
    }
```

  And in `render_headless.rs`:

```rust
/// Carry-forward strike: the alpha-blended grid survives the 4x MSAA
/// resolve path — grid-colored ink must be present after resolve+blit,
/// at pixel values consistent with straight-alpha compositing.
#[test]
fn msaa_resolves_keep_grid_alpha() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let g = grid::geometry((scene.aabb_min, scene.aabb_max), None);
    let px = render_frame_msaa_with_grid(/* 4x pipeline, grid prepared */, &scene, &g, 800, 600);
    let grid_ink = px.chunks(4).filter(|c| is_ink(c, bg) && chroma_dist(decode_linear(c), [1.0, 1.0, 1.0]) < 0.4 && c[3] == 255).count();
    assert!(grid_ink > 300, "grid lines visible through the resolve: {grid_ink}px");
    // an alpha-lost regression (opaque white lines) reads chroma 0 from white
    // but saturates neighboring pixels — also assert NO fully-white pixel exists
    assert!(!px.chunks(4).any(|c| c[0] > 250 && c[1] > 250 && c[2] > 250), "alpha preserved");
}
```

  (Write `render_frame_msaa_with_grid` concretely — `render_frame_msaa` with the grid threaded through its Frame. Grid ink on the dark background reads ≈ `alpha·white + (1−alpha)·bg` — a resolve that drops coverage alpha yields either full white or background; both asserts catch it.)

- [ ] **Step 2: RED** → **Step 3: Implement**.
- [ ] **Step 4: GREEN** — `cargo test -p viewer`.
- [ ] **Step 5: Commit** — `git add viewer/src/render/grid.rs viewer/src/render/scene.rs viewer/tests/render_headless.rs && git commit -m "feat(viewer): bed-frame overlay wiring, pinned tripod corner, msaa grid alpha test"`

### Task 8: `app::viewer_tools` — the ModelViewerState port (pure)

**Gates:** M4 T5 merged (landed) AND T3 merged — `ViewerTools` carries `section_axis: SectionAxis`, produced by T3 (`viewer::tools::section`); dispatching T8 before T3 merges does not compile.

**Files:**
- Create: `app/src/viewer_tools.rs`
- Modify: `app/src/lib.rs` (register)
- Test: `app/tests/viewer_tools_model.rs`

**Interfaces:**
- Consumes: `engine::settings::PartsLayout`, `viewer::tools::section::SectionAxis`, T1's `groups` (only at call sites — this struct is part-name keyed).
- Produces (T9/T10/T11/T12):
  - `pub struct ViewerTools { pub shows_grid: bool, pub wireframe: bool, pub measuring: bool, pub sectioning: bool, pub section_axis: SectionAxis, pub section_position: f32, pub layout: PartsLayout, pub explode_amount: f32, pub hidden: BTreeSet<String>, pub attached: BTreeSet<String>, pub measure_points: Vec<[f32; 3]>, pub collapsed: bool }` — defaults per ModelViewerState.swift:21-47: grid true, wireframe false, sectioning false, axis X, position 0.5, layout from settings (Exploded default), explode 1.0, sets empty, collapsed from settings.
  - `impl ViewerTools`: `pub fn new(layout: PartsLayout, collapsed: bool) -> Self`; `pub fn set_layout(&mut self, l: PartsLayout)` (caller re-frames — didSet clears measure_points, :42); `pub fn toggle_measuring(&mut self)` (change clears points, :51-53); `pub fn set_sectioning(&mut self, on: bool)`; `pub fn add_measure_point(&mut self, p: [f32; 3])` (2-max, third restarts, :67-72); `pub fn clear_measurement(&mut self)`; `pub fn mesh_changed(&mut self)` (clears points, :128); `pub fn toggle_hidden(&mut self, group: &str)`; `pub fn toggle_attached(&mut self, group: &str)`; `pub fn isolate(&mut self, group: &str, all: &[String])` (others-hidden toggles off, PartsOverlay.swift:145-148); `pub fn effective_layout(&self, parts_count: usize, comparing: bool) -> PartsLayout` (:78-86 — comparing → Assembled; Exploded with <2 parts → Assembled; upstream also forces Assembled while `isAnimating` (:79) — deliberately omitted here, correct for M6, becomes load-bearing at M7); `pub fn available_layouts(parts_count: usize) -> Vec<PartsLayout>` (PartsOverlay.swift:53-55 — >1 all, else [Assembled, Plate]).
  - `pub fn is_double_click(prev: Option<(std::time::Instant, String)>, now: std::time::Instant, group: &str) -> bool` — same group within `pub const DOUBLE_CLICK_WINDOW: std::time::Duration = Duration::from_millis(500);` (documented stand-in for SwiftUI's `onTapGesture(count:2)` system window).
  - `pub const EXPLODE_RANGE: std::ops::RangeInclusive<f32> = 0.0..=2.0;`

- [ ] **Step 1: Write the failing tests** — `app/tests/viewer_tools_model.rs`:

```rust
use app::viewer_tools::{DOUBLE_CLICK_WINDOW, ViewerTools, is_double_click};
use engine::settings::PartsLayout as PL;
use std::time::{Duration, Instant};

#[test]
fn measure_points_cap_at_two_third_restarts() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    t.toggle_measuring();
    assert!(t.measure_points.is_empty(), "toggles clear");
    t.add_measure_point([0.0; 3]);
    t.add_measure_point([1.0, 2.0, 3.0]);
    assert_eq!(t.measure_points.len(), 2);
    t.add_measure_point([9.0; 3]);
    assert_eq!(t.measure_points, [[9.0, 9.0, 9.0]], "third click restarts");
}

#[test]
fn state_changes_clear_measurement() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    t.toggle_measuring();
    t.add_measure_point([1.0; 3]);
    t.set_layout(PL::Plate);
    assert!(t.measure_points.is_empty(), "layout change clears");
    t.add_measure_point([1.0; 3]);
    t.mesh_changed();
    assert!(t.measure_points.is_empty(), "mesh change clears");
    t.add_measure_point([1.0; 3]);
    t.toggle_measuring();
    assert!(t.measure_points.is_empty(), "toggle-off clears");
    t.toggle_measuring();
    t.clear_measurement();
    assert!(t.measure_points.is_empty(), "Esc clears");
}

#[test]
fn effective_layout_forces_assembled_when_comparing() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    assert_eq!(t.effective_layout(2, false), PL::Exploded);
    assert_eq!(t.effective_layout(2, true), PL::Assembled);
    assert_eq!(t.effective_layout(1, false), PL::Assembled, "exploded needs 2 parts");
    assert_eq!(t.effective_layout(1, false), t.effective_layout(1, false));
    assert_eq!(ViewerTools::available_layouts(2), [PL::Assembled, PL::Exploded, PL::Plate]);
    assert_eq!(ViewerTools::available_layouts(1), [PL::Assembled, PL::Plate]);
}

#[test]
fn isolate_toggles_others_hidden() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    let all = ["a".to_string(), "b".into(), "c".into()];
    t.isolate("a", &all);
    assert_eq!(t.hidden, ["b", "c"].map(String::from).into_iter().collect());
    t.isolate("a", &all);
    assert!(t.hidden.is_empty(), "second double-click un-isolates");
}

#[test]
fn double_click_window() {
    let now = Instant::now();
    assert!(is_double_click(Some((now - DOUBLE_CLICK_WINDOW + Duration::from_millis(50), "a".into())), now, "a"));
    assert!(!is_double_click(Some((now - Duration::from_secs(2), "a".into())), now, "a"));
    assert!(!is_double_click(Some((now, "a".into())), now, "b"), "different rows");
    assert!(!is_double_click(None, now, "a"));
}
```

- [ ] **Step 2: RED** → **Step 3: Implement**.
- [ ] **Step 4: GREEN** — `cargo test -p app --test viewer_tools_model`.
- [ ] **Step 5: Commit** — `git add app/src/viewer_tools.rs app/src/lib.rs app/tests/viewer_tools_model.rs && git commit -m "feat(app): viewer tools state machine — ModelViewerState parity"`

### Task 9: `app::parts_overlay` + measure badge + section controls + completed status bar (widgets)

**Gates:** T8 merged; M4 T7 merged (fl! scaffold + app shell).

**Files:**
- Create: `app/src/parts_overlay.rs`
- Modify: `app/src/viewer_pane.rs` (mount overlays in `view_model_3d`; complete the status-bar controls; legend), `app/src/lib.rs` (register)
- Test: `app/tests/parts_overlay_model.rs` (badge/legend pure helpers) + pane in-crate construction tests

**Interfaces:**
- Consumes: T8's ViewerTools, T1's `PartGroup`/`legend_colors`, M5 T5's ParametersOverlay mount position (top-trailing — stays).
- Produces:
  - `pub fn view_parts_panel(tools: &ViewerTools, groups: &[PartGroup], effective: PartsLayout, plate_ready: bool, comparing: bool) -> cosmic::Element<'_, crate::viewer_pane::Message>` — PartsOverlay.swift:12-51: header button (`fl!("parti")`, group count badge when >1, chevron text) toggling `collapsed`; layout picker as buttons (`fl!("montato")`/`fl!("esploso")`/`fl!("piatto")`, selected=Suggested — the preset_picker pattern), DISABLED when `comparing` (PartsOverlay.swift:23; the pane passes its `comparing` flag); explode slider row (`cosmic::widget::slider(*EXPLODE_RANGE, tools.explode_amount, Message::SetExplode)`, a11y text `fl!("distanza-tra-le-parti")`) only when effective == Exploded; the scrollable group list (max height 220) only when `groups.len() > 1` — per row: color dot (group color or secondary), name, `×N` when members >1, attach button (`fl!("tieni-unita-al-resto")`/`fl!("separa-dal-resto")`) only in exploded layout, show/hide button (`fl!("mostra")`/`fl!("nascondi")`); the plate note (`fl!("l-agente-non-ha-ancora-preparato-il-piatto-di-stampa-le-parti-sono-affiancate-senza-ruotarle-chiedi-di-prepararlo-nella-chat")`) when effective == Plate && !plate_ready. Panel width 250, padding 10, corner radius 10. Icons are TEXT buttons, not SF-Symbol lookups — documented divergence (function parity over glyph parity; no icon-name roulette).
  - `pub fn view_measure_badge(points: &[[f32; 3]], unit: MeasurementUnit) -> cosmic::Element<'_, crate::viewer_pane::Message>` + `pub fn badge_text(points: &[[f32; 3]], unit: MeasurementUnit) -> String` — 2 points: `"{total} · ΔX {dx} · ΔY {dy} · ΔZ {dz}"` with total = `simd_length(abs-free delta)`… exactly: `delta = abs(p1 - p0)` per MODEL component, each term through `viewer_pane::format_length(_, unit, true)` (ModelPane.swift:163-179 — symbol on EVERY term); 0 points → `fl!("fai-clic-su-un-punto-del-modello")`, 1 → `fl!("fai-clic-sul-secondo-punto")`. Capsule container, monospace (`cosmic::iced::font::Family::Monospace`).
  - `pub fn view_section_controls(tools: &ViewerTools) -> cosmic::Element<'_, crate::viewer_pane::Message>` — ModelPane.swift:181-200: X/Y/Z axis buttons (`SectionAxis::title`), slider `SLIDER_MIN..=SLIDER_MAX` width 220 a11y `fl!("posizione-della-sezione")`, capsule bottom-center.
  - Status bar (viewer_pane): after the preset picker add icon-less toggle buttons in upstream order — grid (`fl!("griglia")`, help `fl!("mostra-la-griglia")`), wireframe (`fl!("wireframe")`/`fl!("mostra-il-wireframe")`), measure (`fl!("misura")`/`fl!("misura-la-distanza-tra-due-punti")`), section (`fl!("sezione")`/`fl!("mostra-una-sezione-del-modello")`) — selected = Suggested class; then `view_legend(&legend_colors)` — first 12 colors as 10px dots, container a11y `fl!("lld-colori", lld = total as i64)` — `fl!` emits the literal `%lld` pre-M8: substitute the count onto the fl! output so the label reads "N colors" today, and record the interim substitution for M8's placeable sweep. The msaa toggle (port-specific, no upstream counterpart) sits end-of-row AFTER the legend. The animation slot between section and legend is M7 — noted in a comment. `info` drops M4's minimal " · N warnings" suffix — upstream's info is the size string only (ModelPane.swift:215-219; issues live in the header ActivityViewer); keeping it would have to be recorded as a documented divergence, so drop it.
  - Overlay mounting in `view_model_3d` (ModelPane.swift:17-49): measure badge top-center when measuring && model present; parts panel top-leading at padding (10, 32) when `parts_count > 1 || plate_parts non-empty`; section controls bottom-center when sectioning && model present. Keep M5's parameters overlay top-trailing untouched.

- [ ] **Step 1: Write the failing tests** — `app/tests/parts_overlay_model.rs`:

```rust
use app::viewer_pane::format_length;
use app::parts_overlay::badge_text;
use engine::settings::MeasurementUnit as U;

#[test]
fn badge_texts_all_three_states() {
    let unit = U::Millimeters;
    assert_eq!(badge_text(&[], unit), "Click a point on the model");
    assert_eq!(badge_text(&[[0.0; 3]], unit), "Click the second point");
    // 3-4-5 diagonal: total 5.0, dX 3, dY 4 (MODEL), dZ 0 — symbol on EVERY term
    assert_eq!(
        badge_text(&[[0.0, 0.0, 0.0], [3.0, -4.0, 0.0]], unit),
        "5.0 mm · ΔX 3.0 mm · ΔY 4.0 mm · ΔZ 0.0 mm"
    );
    assert_eq!(
        badge_text(&[[0.0, 0.0, 0.0], [25.4, 0.0, 0.0]], U::Inches),
        "1.00 in · ΔX 1.00 in · ΔY 0.00 in · ΔZ 0.00 in"
    );
}
```

  (First `fl!` use in a test — no fl! exists in landed app code: if `fl!` needs locale initialization outside a cosmic app, initialize it in the test's setup or assert through the same lookup helper the pane uses.)

  Plus pane in-crate: `view_builds_with_overlays_in_every_state` (measuring/sectioning/exploded/plate-note states construct Elements — pure, no GPU) and `legend_view_wraps_at_twelve` (13 distinct colors → 12 dots; a11y count 13 — assert via the pure `legend_colors` input the view consumes).

- [ ] **Step 2: RED** → **Step 3: Implement** (Δ glyph is a literal in the format string, matching upstream's badge exactly).
- [ ] **Step 4: GREEN** — `cargo test -p app`.
- [ ] **Step 5: Commit** — `git add app/src/parts_overlay.rs app/src/viewer_pane.rs app/src/lib.rs app/tests/parts_overlay_model.rs && git commit -m "feat(app): parts overlay, measure badge, section controls, completed status bar"`

### Task 10: pane wiring — layouts, explode, hidden, plate scene, bed, section state

**Gates:** T1, T2, T3, T7, T8 merged (M5 T5 merged — viewer_pane co-edit).

**Files:**
- Modify: `app/src/viewer_pane.rs`
- Test: pane in-crate tests

**Interfaces:**
- Consumes: T1 `arrange`/`effective_model_aabb`/`groups`, T2 `PartPose`/`to_scene`, T3 `scene_plane`/`plane_value`, T7 `set_overlay_frame`, T8 ViewerTools.
- Produces (T11/T12 build on the extended pane):
  - New `Message` variants: `ToggleGrid`, `ToggleWireframe`, `ToggleMeasure`, `ToggleSection`, `SetSectionAxis(SectionAxis)`, `SetSectionPosition(f32)`, `SetLayout(PartsLayout)`, `SetExplode(f32)`, `ToggleHiddenGroup(String)`, `ToggleAttachedGroup(String)`, `RowClicked(String)`, `TogglePartsCollapsed`.
  - `ViewerPane` gains: `tools: ViewerTools`, `model_parts: Vec<Part>` (the parse source, kept for grouping/re-arrange), `plate_parts: Vec<Part>` + `plate_loaded: bool`, `comparing: bool`, `settings_path: std::path::PathBuf` (constructor param defaulting to `engine::settings::settings_path()` — the ONE-writer rule holds in production; tests inject a temp path so parallel pane tests never load-modify-save the developer's real config), and the grid-freeze state below (`grid_aabb`, `grid_bed`, `grid_stamp`, `frame_aabb`).
  - `fn apply_arrangement(&mut self)` — THE pose push, called after every layout/explode/hidden/attached/scene change: `let arranged = arrangement::arrange(&self.model_parts, &self.plate_parts, self.tools.effective_layout(n, self.comparing), self.tools.explode_amount, &self.tools.hidden, &self.tools.attached);`
    - `plate_scene` → swap `scene` to `PartScene::from_parts(&visible plate parts, NEUTRAL)` (hidden-filtered by group) when not already the plate scene; else ensure the model scene is active.
    - visibility quirk: when `!arranged.visible.iter().any(|v| *v)` (all hidden), poses are identity + all visible (ModelPane.swift:79-80) AND eff recomputes from the zero-offset all-parts union — the fallback draws the assembled model, so the section plane must not keep the exploded aabb.
    - `poses[i] = visible.then(translation(remap_scene_offset(offsets[i])))` — MODEL offset `(ox,oy,oz)` → scene `(ox, oz, −oy)` via `render::parts` remap semantics; write as `glam::Mat4::from_translation(...).to_cols_array_2d()`.
    - `let eff = if arranged.plate_scene { self.plate_scene_aabb_as_model() } else { arrangement::effective_model_aabb(&self.model_parts, &arranged) }` stored as `self.effective_aabb` (model) — when `plate_scene` is set the DRAWN geometry is the plate scene, so eff derives from the plate scene's staged aabb mapped back to MODEL (upstream's arranged mesh IS the plate mesh, ModelViewerState.swift:116-121; T1's `Arranged` docs state the contract). The SECTION/CAP consume eff live on every tick; `scene_aabb_of` permutes MODEL→scene extents (the `model_size` inverse: scene x = model x span, scene y = model z span, scene z = model y span, positioned from the pin). The GRID freezes instead (upstream rebuildGrid fires on sceneID = mesh+layout+plateID, NOT on explode/hidden — ModelViewer.swift:165-170 via ModelPane.swift:83): keep `grid_aabb` + `grid_bed` recomputed ONLY on scene swap / layout change / plate-identity change (compare a `grid_stamp` of (scene Arc identity, effective layout, plate_loaded)); when the stamp changes call `scene.set_overlay_frame(scene_aabb_of(grid_aabb), grid_bed)` where `grid_bed = (effective layout == Plate).then_some(manifest.bed.flatten())` — an explode/hidden sweep must leave the grid's content_hash and build half-extents unchanged (test below). Also store `self.frame_aabb` = the scene-space aabb of what is drawn (the plate scene's staged aabb in plate mode, else `scene_aabb_of(eff)`) — the re-frame input below.
    - recompute the section uniform: `tools.sectioning` → `let (axis, pos) = section::scene_plane(tools.section_axis, tools.section_position, eff)` — then add the LIVE scene pin before feeding the camera (`to_scene` on a plane point, per T3's groundwork note: scene_plane returns the RAW-remap value, correct as-is only for pin-symmetric scenes; the golden fixture is, real models are not) — into `scene.camera`'s section fields (via a new `ModelScene::set_section(Option<(u32, f32)>)` — None disables).
    - `self.scene.poses = poses.into();`
  - `fn set_layout(&mut self, l: PartsLayout)` — `tools.set_layout(l)`, persist `Settings` (load-modify-save through `self.settings_path`, preserving every other key), `apply_arrangement()`, then ALWAYS re-frame at the current preset — against the CURRENT arranged aabb, not the input's stale tracked one (`CameraInput::preset` re-frames against the aabb `mesh_changed` last pushed, which is the assembled scene — Plate/Exploded views would render mis-framed; upstream's cameraID bump re-frames against the ARRANGED mesh unconditionally, ModelViewer.swift:77 → :328-337): push `self.frame_aabb` into the input's framing reference as a FORCED update (a `CameraInput` seam that sets the tracked aabb outright — NOT the >50 % `mesh_changed` gate), then `self.input.preset(self.preset); self.sync_pose();` (ModelViewerState.swift:42-43). The plate scene swap inside `apply_arrangement` pushes the same forced update.
  - `ReloadKind::Plate` handling (Plate currently routes through `reload_outputs` at viewer_pane.rs:261 — what is MISSING is plate.3mf parsing): parse `output/plate.3mf` into `plate_parts`, set `plate_loaded`, `apply_arrangement()`.
  - Explode/hidden/attached/section messages: mutate `tools`, `apply_arrangement()` (+ `sync_pose()` never needed — camera untouched).
  - `RowClicked(group)`: double-click detect via `is_double_click` with a stored `last_row_click` → `tools.isolate(group, &groups(model_parts).names)` + `apply_arrangement()`.
  - Layout/explode persistence: `tools.layout` and `tools.collapsed` load in `ViewerPane::new` (settings), saved on change (same load-modify-save, always through `self.settings_path` — tests inject a temp path there and assert on it, never the real global file).
  - `pub fn groups(&self) -> Vec<PartGroup>` — T1 `groups(&self.model_parts)` (T9's panel consumes).

- [ ] **Step 1: Write the failing tests** — pane in-crate (house pattern: temp project + fixture copies):

```rust
    #[test]
    fn layout_change_reframes_at_preset_and_rebuilds_grid() {
        let (root, mut pane) = pane("layout", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::Preset(Preset::Front));
        // orbit away from the preset
        let _ = pane.update(Message::Moved(Point::new(300.0, 300.0)));
        pane.input.press(false);
        let _ = pane.update(Message::Moved(Point::new(340.0, 300.0)));
        let (yaw_before, pitch_before) = (pane.input.yaw, pane.input.pitch);
        assert!(yaw_before.abs() > 1e-3);
        let dist_before = pane.input.distance;
        let _ = pane.update(Message::SetLayout(PartsLayout::Plate));
        assert!((pane.input.yaw - 0.0).abs() < 1e-4, "front preset yaw restored");
        assert!((pane.input.pitch - 0.0).abs() < 1e-4);
        assert!(pane.input.distance > dist_before,
            "re-frame used the CURRENT arranged aabb: the plate fallback lays parts side by side (wider than assembled), so the camera pulls BACK");
        assert_eq!(pane.tools.layout, PartsLayout::Plate);
        // persisted — through the pane's injected TEMP settings path, never the
        // developer's real global file (parallel pane tests would race it)
        let s = engine::settings::Settings::load(&pane.settings_path).unwrap_or_default();
        assert_eq!(s.parts_layout, PartsLayout::Plate, "ONE writer: the pane's settings path");
    }

    #[test]
    fn explode_sweep_leaves_the_grid_frozen() {
        let (root, mut pane) = pane("grid-freeze", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::SetLayout(PartsLayout::Exploded));
        let identity = pane.scene.grid_identity_for_tests();
        let _ = pane.update(Message::SetExplode(2.0));
        let _ = pane.update(Message::ToggleHiddenGroup("lid".into()));
        assert_eq!(pane.scene.grid_identity_for_tests(), identity,
            "grid freezes at the layout-switch-time aabb (upstream rebuilds only on sceneID, not explode/hidden)");
    }

    #[test]
    fn explode_updates_poses_without_a_scene_swap() {
        let (root, mut pane) = pane("explode", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::SetLayout(PartsLayout::Exploded));
        let scene_ptr = Arc::as_ptr(&pane.scene.scene);
        let pose_at_one = pane.scene.poses[1];
        let mag = |p: viewer::render::PartPose| p.map_or(0.0, |m| glam::Vec3::new(m[0][3], m[1][3], m[2][3]).length());
        let _ = pane.update(Message::SetExplode(2.0));
        assert_eq!(Arc::as_ptr(&pane.scene.scene), scene_ptr, "no scene rebuild");
        assert!((mag(pane.scene.poses[1]) - 2.0 * mag(pose_at_one)).abs() < 1e-3,
            "offset scales with explode: {} vs {}", mag(pane.scene.poses[1]), 2.0 * mag(pose_at_one));
        let _ = pane.update(Message::SetExplode(0.5));
        assert!(mag(pane.scene.poses[1]) < mag(pose_at_one), "monotone in explode");
        let _ = pane.update(Message::SetExplode(0.0));
        assert_eq!(mag(pane.scene.poses[1]), 0.0, "explode 0 = assembled positions (+lift)");
    }

    #[test]
    fn hiding_every_group_falls_back_to_assembled() {
        let (root, mut pane) = pane("hide-all", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::SetLayout(PartsLayout::Exploded));
        let _ = pane.update(Message::ToggleHiddenGroup("base".into()));
        let _ = pane.update(Message::ToggleHiddenGroup("lid".into()));
        assert!(pane.scene.poses.iter().all(|p| p.is_some()), "quirk: all-hidden shows assembled");
        // and hiding ONE group removes exactly it from the draw set
        let _ = pane.update(Message::ToggleHiddenGroup("lid".into()));
        assert!(pane.scene.poses.iter().enumerate().all(|(i, p)| {
            pane.scene.scene.parts[i].name == "base" || p.is_none()
        }));
    }

    #[test]
    fn plate_reload_stages_the_plate_scene_and_bed() {
        let (root, mut pane) = pane("plate", &[], &["golden/model.3mf"]);
        std::fs::copy(fixture("golden/plate.3mf"), root.join("output/plate.3mf")).unwrap();
        // manifest with a bed
        std::fs::write(root.join("output/manifest.json"), manifest_with_bed(220.0, 220.0)).unwrap();
        let _ = pane.update(Message::Reload(ReloadKind::Plate));
        let _ = pane.update(Message::Reload(ReloadKind::Manifest));
        let _ = pane.update(Message::SetLayout(PartsLayout::Plate));
        assert!(pane.plate_loaded);
        assert_eq!(pane.scene.bed, Some([220.0, 220.0]), "bed outline in plate view");
        let _ = pane.update(Message::SetLayout(PartsLayout::Assembled));
        assert_eq!(pane.scene.bed, None, "bed only in plate view");
    }

    #[test]
    fn section_slider_maps_through_the_arranged_aabb() {
        let (root, mut pane) = pane("section", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::ToggleSection);
        let _ = pane.update(Message::SetSectionAxis(SectionAxis::Z));
        let _ = pane.update(Message::SetSectionPosition(0.5));
        let (axis, pos) = pane.scene.section_for_tests();
        assert_eq!(axis, 1, "MODEL Z -> scene y");
        // golden: model z in [-4, 22]; arranged-assembled aabb mid = 9
        assert!((pos - 9.0).abs() < 1.0, "plane at the arranged mid: {pos}");
        // exploding moves the aabb hence the plane for the SAME t (upstream
        // quirk): the lid pushes +z while the lift holds min z at -4, so the
        // mid rises
        let _ = pane.update(Message::SetLayout(PartsLayout::Exploded));
        let (_, pos2) = pane.scene.section_for_tests();
        assert!(pos2 > pos, "same t, moved plane: {pos} -> {pos2}");
    }

    #[test]
    fn y_axis_section_maps_negated() {
        let (root, mut pane) = pane("section-y", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::ToggleSection);
        let _ = pane.update(Message::SetSectionAxis(SectionAxis::Y));
        let _ = pane.update(Message::SetSectionPosition(0.5));
        let (axis, _) = pane.scene.section_for_tests();
        assert_eq!(axis, 2, "MODEL Y -> scene z");
    }
```

  (`section_for_tests` — a `#[cfg(test)]` accessor on ModelScene returning the last-set section tuple; `grid_identity_for_tests` — likewise, returning the grid's content_hash plus the half-extents of the aabb it was built from; `manifest_with_bed` writes a minimal manifest JSON with `"bed": [220.0, 220.0]`.)

- [ ] **Step 2: RED** → **Step 3: Implement**.
- [ ] **Step 4: GREEN** — `cargo test -p app` + `cargo check -p viewer`.
- [ ] **Step 5: Commit** — `git add app/src/viewer_pane.rs && git commit -m "feat(app): layout/explode/hidden/plate/bed/section wiring through pose uniforms"`

### Task 11: pane wiring — measure click flow, off-thread pick, markers, Esc

**Gates:** T4, T5, T9, T10 merged.

**Files:**
- Modify: `app/src/viewer_pane.rs`
- Test: pane in-crate tests

**Interfaces:**
- Consumes: T4 `camera::pick_ray` + `picking::first_hit`, T5 `markers::{geometry, marker_radius}`, T2 `camera::framing_radius`, T9's badge.
- Produces:
  - `Message::Picked(Option<[f32; 3]>)` + `PaneEvent::Picked(Option<[f32; 3]>)` through the existing relay channel.
  - Click flow (viewer_pane.rs `Message::Released`): `let was_click = self.input.release();` — the pane funnels right/middle release into `Message::Released` too (viewer_pane.rs:363-365), so record the press kind at `Pressed` (`last_press_primary: bool` from the event's button) and gate on it — when `was_click && last_press_primary && self.tools.measuring && !self.pick_pending` (upstream picks on left `mouseUp` only, CameraInputView.swift:50-53): clone `Arc<PartScene>`, the poses `Vec`, the current section tuple, and the ray inputs (`pick_ray(self.input.distance, self.input.yaw, self.input.pitch, self.input.target, aspect, (last_cursor.x, last_cursor.y), (w, h))` — aspect and size from `last_size`); spawn a named std thread `viewer-pick` (spec §5: event-driven, off-thread — never the cosmic executor) running `first_hit`; on completion send `PaneEvent::Picked(hit)`. `pick_pending` set at spawn, cleared on `Picked` (a click during a pending pick is dropped — single in-flight, documented).
  - `Message::Picked(Some(p))` → `self.tools.add_measure_point(p)`; rebuild markers: `let radius = markers::marker_radius(camera::framing_radius(scene.aabb...))` — the assembled aabb; upstream's `modelRadius` is the last-FRAMED (arranged-at-reframe) size, so marker size carries bounded cosmetic drift — ACCEPTED and recorded here (swap in the framing aabb captured at layout time only if the eyeball verdict objects); `self.scene.markers = (!points.is_empty()).then(|| Arc::new(markers::geometry(&points.map(|m| scene.scene.to_scene(m)).collect::<Vec<_>>(), radius)))`.
  - `Message::Escape` (new subscription arm in `subscription()`: `keyboard::listen()` filter `KeyPressed { key: KeyCode::Escape, .. }`) → `if self.tools.measuring { self.tools.clear_measurement(); /* markers = None */ }` (ModelPane.swift:54-58 — ignored otherwise).
  - Measure toggle disabled wiring: while measuring, cursor interaction stays Grab; badge shows per T9.
  - Markers clear alongside every `tools.measure_points` clear site (mesh reload → `tools.mesh_changed()` + markers rebuild; layout change already clears via `set_layout`).

- [ ] **Step 1: Write the failing tests** — pane in-crate:

```rust
    #[test]
    fn click_while_measuring_adds_a_model_space_point() {
        let (root, mut pane) = pane("pick", &[], &["golden/model.3mf"]);
        pane.input.set_view_height(600.0);
        let _ = pane.update(Message::ToggleMeasure);
        // aim at the canvas center: the golden base's top face is under the
        // iso camera's center ray; drive press + tiny move + release
        let _ = pane.update(Message::Moved(Point::new(400.0, 300.0)));
        let _ = pane.update(Message::Pressed);
        let _ = pane.update(Message::Moved(Point::new(401.0, 300.0))); // < 4px: still a click
        let _ = pane.update(Message::Released);
        // the pick ran on a thread and reports through the relay: drain its
        // message into update until Picked lands (bounded, house pattern)
        pane.pump_pending_pick(Duration::from_secs(2), |pane| pane.tools.measure_points.len() == 1);
        let p = pane.tools.measure_points[0];
        // golden base spans MODEL z -4..4 (lid 12..22): the center ray with
        // the section OFF hits the lid top at z=22 or the base — assert on-model
        assert!(p[2] > -5.0 && p[2] < 23.0, "hit the model region: {p:?}");
        assert!(pane.scene.markers.is_some(), "markers staged");
        // second click completes the pair; badge text is the delta form
        let _ = pane.update(Message::Moved(Point::new(430.0, 300.0)));
        let _ = pane.update(Message::Pressed);
        let _ = pane.update(Message::Released);
        pane.pump_pending_pick(Duration::from_secs(2), |pane| pane.tools.measure_points.len() == 2);
        assert!(badge_text(&pane.tools.measure_points, pane.unit).contains("ΔX"));
    }

    #[test]
    fn drag_is_not_a_click() {
        let (root, mut pane) = pane("drag", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::ToggleMeasure);
        let _ = pane.update(Message::Moved(Point::new(100.0, 100.0)));
        let _ = pane.update(Message::Pressed);
        let _ = pane.update(Message::Moved(Point::new(200.0, 200.0)));
        let _ = pane.update(Message::Released);
        assert!(pane.tools.measure_points.is_empty(), "drag never picks");
        // a stationary non-primary release is not a pick either: mirror a right
        // press through the recorded press kind (upstream: left mouseUp only)
        pane.last_press_primary = false; // test seam
        let _ = pane.update(Message::Moved(Point::new(400.0, 300.0)));
        let _ = pane.update(Message::Pressed);
        let _ = pane.update(Message::Released);
        assert!(pane.tools.measure_points.is_empty(), "non-primary release never picks");
    }

    #[test]
    fn escape_clears_only_when_measuring() {
        let (root, mut pane) = pane("esc", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::ToggleMeasure);
        pane.tools.measure_points = vec![[1.0; 3], [2.0; 3]];
        pane.rebuild_markers(); // test seam, or set via the same private path
        let _ = pane.update(Message::Escape);
        assert!(pane.tools.measure_points.is_empty());
        assert!(pane.scene.markers.is_none());
        // Esc with measure off: no panic, no state change
        let _ = pane.update(Message::Escape);
    }
```

  (`pump_pending_pick` — a `#[cfg(test)]` pane helper: tests never start the subscription, so the relay's receiver still sits in `OutputRelay::events`; the helper takes it (`take_events`), `try_recv`-drains with 10 ms sleeps and a deadline — translating each `PaneEvent` through the same match `relay_stream` uses into `pane.update(...)` — and restores it via a test-only `OutputRelay::restore_events` (add alongside `take_events`). Panics on timeout. `Message::Pressed` at the `Moved` cursor requires the pane to track `last_cursor: Point` — implement in Step 3. The pick thread's `PaneEvent::Picked` travels the SAME relay as Reload/Resized — extend `relay_stream`'s match with the fourth arm.)

- [ ] **Step 2: RED** → **Step 3: Implement**.
- [ ] **Step 4: GREEN** — `cargo test -p app` (threaded test uses a bounded wait; if CI-style flakes appear the bounded-poll carry-forward note applies — keep 2 s).
- [ ] **Step 5: Commit** — `git add app/src/viewer_pane.rs && git commit -m "feat(app): measure click flow — off-thread pick, orange markers, badge, escape"`

### Task 12: pane wiring — the compare ghost (gates M5 T7)

**Gates:** T6 merged AND M5 T7 merged (Bridge `compare_with`/`compare_version` exist).

**Files:**
- Modify: `app/src/viewer_pane.rs`, `app/src/windows/project.rs` (or wherever the pane meets Bridge state — the M4 T7/M5 T7 project window; forward compare changes), `app/src/panes.rs` only if the pane is reached through it
- Test: pane in-crate tests

**Interfaces:**
- Consumes: T6's ghost Frame pass, M5 T7's `Bridge::compare_with(Option<i64>)` + `compare_version`.
- Produces:
  - `pub fn ViewerPane::set_compare(&mut self, version: Option<(i64, std::path::PathBuf)>)` — `None` clears (`scene.ghost = None; self.comparing = false; apply_arrangement()`); `Some((n, version_dir))` parses `version_dir/output/model.3mf` (STL fallback — reuse `load_model_parts(&version_dir.join("output"))`), stages `PartScene::from_parts_with_pin(&parts, NEUTRAL, self.scene.scene.pin)` into `scene.ghost`, sets `comparing = true`, and `apply_arrangement()` (which now sees `comparing` → effective layout Assembled, poses identity — the T10 path) + layout picker disabled state.
  - The window's compare-message handler calls `pane.set_compare(...)` with the version's directory (`versions/<n>/` — the same root M5 T3's version store writes).
  - `comparing` also gates the T9 picker-disabled param and keeps the plate/explode controls inert (upstream PartsOverlay.swift:23).
  - Note: upstream does NOT clear compare on mesh reload or restore (ProjectOutput.loadComparison persists until the picker says None) — the ghost scene stays until `set_compare(None)`; a mesh reload does NOT restage the ghost (its identity is the version, not the current output). The ghost's pin, however, tracks the CURRENT scene pin — `set_compare` re-stages if the model scene swapped since (store `ghost_version: Option<(i64, PathBuf)>` and re-derive in `reload_outputs` when the scene `Arc` identity changed).

- [ ] **Step 1: Write the failing tests** — pane in-crate:

```rust
    #[test]
    fn ghost_uses_the_current_scene_pin() {
        let (root, mut pane) = pane("ghost", &[], &["golden/model.3mf"]);
        // a "version" whose model floats at MODEL z 40..60 (above the current
        // scene's z -4..22): staged with the CURRENT pin (floor scene y 0),
        // its staged aabb min y is 40 — its OWN pin would read 0
        let vdir = root.join(".cadstudio/versions/1/output");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(vdir.join("model.stl"), stl_of_box([[0.0, 0.0, 40.0], [10.0, 10.0, 60.0]])).unwrap();
        pane.set_compare(Some((1, root.join(".cadstudio/versions/1"))));
        let ghost = pane.scene.ghost.as_ref().unwrap();
        assert!((ghost.aabb_min[1] - 40.0).abs() < 1e-3, "current pin kept: {:?}", ghost.aabb_min);
        assert!(pane.comparing);
    }

    #[test]
    fn compare_forces_assembled_and_clears_on_none() {
        let (root, mut pane) = pane("ghost2", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::SetLayout(PartsLayout::Exploded));
        let _ = pane.update(Message::SetExplode(2.0));
        let identity = || glam::Mat4::IDENTITY.to_cols_array_2d();
        let off_identity = |p: &viewer::render::PartPose| p.map_or(false, |m| m != identity());
        assert!(pane.scene.poses.iter().any(off_identity), "precondition: exploded poses");
        let vdir = write_version_1(&root); // helper: versions/1/output/model.stl
        pane.set_compare(Some((1, vdir)));
        assert!(pane.scene.poses.iter().all(|p| !off_identity(p)),
            "compare forces assembled (ModelViewerState.swift:78-86)");
        assert!(pane.scene.ghost.is_some());
        pane.set_compare(None);
        assert!(pane.scene.ghost.is_none());
        assert!(!pane.comparing);
        // layout picker state intact: still Exploded, poses re-applied
        assert!(pane.scene.poses.iter().any(off_identity));
    }
```

  (`stl_of_box` — a minimal binary-STL writer for one box, or copy `golden/model.stl` and accept its bounds in the assertion; prefer the explicit writer, ~20 lines, assert the pin math.)

- [ ] **Step 2: RED** → **Step 3: Implement** (window forwarding: one match arm where M5 T7's compare message lands — the pane is reachable there per the M4 T7 window assembly).
- [ ] **Step 4: GREEN** — `cargo test -p app`.
- [ ] **Step 5: Commit** — `git add app/src/viewer_pane.rs app/src/windows/project.rs && git commit -m "feat(app): compare ghost — orange wireframe of the chosen version, assembled-forced"`

### Task 13: M6 wrap — carry-forward strikes, thelio gate, shots

**Gates:** all of T1-T12 merged.

**Files:**
- Modify: `docs/research/carry-forwards.md` (strike the M6 section's three lines), `viewer/src/lib.rs` + `app/src/lib.rs` (module docs: the tools/ family, the pose-uniform path M7 reuses, the ghost/markers passes), `docs/superpowers/plans/2026-10-12-m4-app-shell.md` progress if the controller tracks it there (controller-owned — skip if not)
- Test: no new tests — the full-suite gate

**Interfaces:**
- Consumes: everything above.
- Produces: M6 closed — status-bar parity re-derivation recorded (see the report: upstream ModelPane has NO volume; info stays `X × Y × Z unit` with M4's " · N warnings" suffix DROPPED per T9; the legend is a controls-row element, not an info element; the animation slot is M7's). Controller follow-up from review m5: re-point M4 T7's dangling ActivityViewer-full-port note at M8 — the spec's §10 M6 line does not name it and this plan deliberately contains none of it (upstream delta: issue popovers/IssueList, name+chevron prefix — Shared/ActivityViewer.swift, 112 lines); no M6 task grows.

- [ ] **Step 1: Strike carry-forwards** — `docs/research/carry-forwards.md` M6 section: mark landed (with strikethrough + "(landed M6)") the winding line (pinned by `section_discard_is_winding_independent` — the stray-caps half is upstream-inherited, note that explicitly), the tripod-corner line, and the msaa-grid line.
- [ ] **Step 2: lib.rs docs** — one paragraph per crate describing the M6 surface (tools/ pure math; Frame + pose uniforms; markers/ghost passes; viewer_tools state; overlays).
- [ ] **Step 3: thelio gate** — controller runs `ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && cargo test --workspace --all-targets"'` once, HEAD-verified. Fix-forward tasks file under `.superpowers/sdd/` per house process if anything flakes.
- [ ] **Step 4: Shots for Preston** — offscreen renders (the established `render_headless` pattern generalized: one script-style example or test that dumps PNGs — `viewer/examples/` run on thelio): assembled vs exploded vs plate+bed of `golden/model.3mf` + `golden/plate.3mf`; section X/Y/Z at 0.5 with caps; measure badge markers on two points; the compare ghost over the current model. Show them inline to Preston for the eyeball verdict (the M0 carry-forward pattern).
- [ ] **Step 5: Commit** — `git add docs/research/carry-forwards.md viewer/src/lib.rs app/src/lib.rs && git commit -m "docs: M6 viewer tools complete"`

---

**Execution note (dispatch waves, ≤5 concurrent, controller merges serially):** Wave 1 (immediately, disjoint files): T1 ∥ T2. Wave 2: T3 ∥ T4 (both need only T2), with T8 dispatched the moment T3 merges — its ViewerTools carries T3's `SectionAxis` (the wave's T3 ∥ T4 ∥ T8 stays ≤5 concurrent). Then T5 ∥ T7 (both touch mod.rs/scene.rs in different corners — stagger reviews; expect small textual conflicts in scene.rs/render_headless.rs at the controller's serial merges, no semantic overlap), T6 after T5 (same files). Remaining app track: T9 after T8 + M4 T7; T10 after its viewer gates + T8; T11 after T10; T12 after T6 + M5 T7; T13 last. Every task: implementer runs focused local tests only; thelio gates are the controller's, single-invocation, HEAD-verified.
