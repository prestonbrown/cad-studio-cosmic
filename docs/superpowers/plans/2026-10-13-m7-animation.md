# M7 Animation Playback Implementation Plan (rev 2 — independent review applied: 2M/7m/5N folded; APPROVED_WITH_FINDINGS)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship animation playback end to end — the pane loads `output/animation.json` into the landed engine study model, the status bar grows the Animazione toggle (plus the ask-the-agent button when no study exists), MotionControls plays/pauses/scrubs/loops the study on an elapsed-driven 60 Hz clock, and every tick writes per-part pose matrices + motion appearance flags through the M6 pose-uniform path — moving parts opaque, static parts translucent at 0.22, colliding parts flat red — with collision/`motionError` issues reaching the header's ActivityViewer.

**Architecture:** Viewer-first, spec §4.3/§5: a new pure `viewer/src/playback.rs` (the spec's `playback.rs`) ports MotionPlayer and derives per-part `(PartPose, PartAppearance)` from `StudyState` — the MODEL→SCENE quaternion remap and its pin-pivot composition are the load-bearing math. The renderer gains one uniform extension (collision flag + alpha in the 80-byte Model binding) and two alpha-blended part pipelines (fill + hairline, depth-write off) so a static part's 0.22 translucency and a colliding part's flat red are per-frame uniform state, never vertex rewrites (§5's "per-part transform uniforms per frame"). The app gains the pane-side playback clock (`iced time::every`, elapsed-delta advance — no wall-sleep), the MotionControls widget (bottom capsule, replacing section controls while animating), the `isAnimating` semantics inside M6's `ViewerTools` (mutual exclusion with section/measure, assembled-forced layout), and the two upstream issue branches in `activity::issues`. Engine change is exactly two derived helpers on the landed study model (`collision_count`, `moving_parts`).

**Tech Stack:** No new dependencies. Rust edition 2024, wgpu 28 exact, libcosmic rev `dbc62f2d899cdbfbe43d170689ddc0290db663f4` (iced pinned through the same git source), glam, bytemuck; app uses existing cosmic widgets (button, slider, text, dropdown — new to this pane but already a libcosmic widget, no manifest change).

**Spec:** docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md AS AMENDED — §4.3 (`playback.rs` — "MotionPlayer state-at-time, collision flags"), §4.4 (3D pane overlay inventory: "motion controls (play/pause/scrub/speed/loop, phase label, values, collisions)"), §5 (GPU-first; "Animation playback: per-part transform uniforms per frame; no vertex rewrites"), §10 M7. Behavior source of truth: vendored Swift at fixtures/upstream-source/ — Viewer3D/{MotionPlayer,MotionControls,ModelViewer,ModelViewerState,ModelPane}.swift, MotionStudy.swift, ProjectOutput.swift, Project.swift (issues:123-127). Parity facts: .superpowers/sdd/m3-viewer-facts.md §7 (facts #86-99; cited "facts #N"). Landed engine model: engine/src/parsing/animation.rs (read FIRST — `StudyState`, `state_at`, `is_active`, `value_names`). Assumes the M6 tasks each task's Gates line names are merged; where this plan and a Swift file disagree, the Swift file wins and the discrepancy goes to the controller. Line numbers cite files as they stand at plan time — re-locate by symbol if merges shifted them.

## Global Constraints

- PIXEL-EXPECTATION LAW (from T2's implementation, binding on T3/T6): with
  upstream's faceCulling .none, a translucent closed mesh is NEVER
  single-fragment — every interior pixel composites ≥2 draw-order-dependent
  layers. Blend/color expectations must be built on single facets (or model
  the layered compositing explicitly), never on closed-translucent
  interiors.

- DOCUMENTED DIVERGENCE (controller-ruled 2026-10-13): the Space play/pause
  shortcut cannot be focus-scoped at the pinned iced rev (a global key
  listener can't see widget focus) — it applies app-wide, or is paired with
  an explicit control click; revisit on the next iced/libcosmic pin bump
  (same class as the pinch-gesture block).
- T8 ADDITIONALLY owns two granted M4-close minors (carry-forwards M6
  section): route PresentSetupSheet to the app layer (kill the self-loop;
  the loop sits at project.rs:443-444/:753 and app.rs already handles the
  forwarded message — no new files for this half) and move the parameters
  ⌥⌘I match arm app-wide (Commands.swift:69-73) — the pane-local arm
  (viewer_pane.rs's subscription, ~:345) must be REMOVED in the same edit:
  left in place, both subscriptions see ⌥⌘I and the visibility toggles
  twice, a silent no-op. T8 touches exactly three files: project.rs,
  activity.rs, viewer_pane.rs.

- Engine's GUI-free law holds: the ONLY engine change is two derived, allocation-light helpers on `MotionStudy` in engine/src/parsing/animation.rs (`collision_count`, `moving_parts`) — no parsing-behavior change, no watcher change (`ReloadKind::Motion` landed M1, watch.rs:129).
- GPU-first law (spec §5): playback state is uniforms only. A full playback sweep — pose matrices AND collision/translucency flags changing every tick — must keep `MeshPipeline`'s upload counters flat (the enforcement point: `upload_count_for_tests`). No staged vertex, color byte, or content hash may change because animation is on.
- Viewer purity: `viewer/src/playback.rs` imports no iced/cosmic types (spec §4.3); it consumes `engine::parsing::animation::{MotionStudy, StudyState, Collision}` and `crate::render::{PartAppearance, PartPose, PartScene}` (render types, not iced types).
- Colors cross the exact piecewise sRGB EOTF exactly once. New constants, test-pinned not hand-waved: `COLLISION_LINEAR` = linear of systemRed sRGB (255, 59, 48)/255 (the same systemRed ModelViewer.swift:363 blends for the cut face); translucency alpha = `0.22` (upstream `translucent`, ModelViewer.swift:408-413); collision parts render OPAQUE (upstream's collision material replaces the translucent one, ModelViewer.swift:282-287).
- User-visible strings use `fl!()` with keys from app/i18n/en/main.ftl (grep the slug, never invent). All M7 keys verified present at plan time: `animazione`, `riproduci-l-animazione-con-la-simulazione-fisica`, `pausa`, `riproduci`, `tempo-dell-animazione`, `velocit`, `velocit-di-riproduzione`, `ripeti`, `crea-animazione`, `chiedi-all-agente-di-simulare-e-animare-il-meccanismo`, `anima-il-movimento-di-questo-meccanismo-con-una-simulazione-fisica`, `l-animazione-ha-collisioni-tra-parti-in-lld-fotogrammi`, `animation-json-non-valido`. `fl!` emits literal `%lld`/`%@` pre-M8: substitute through the shared `localize::substitute_placeholders` (this plan's T6 lands it if M6 T9's legend-count substitution didn't already — search first, DRY) and record the interim substitution for M8's placeable sweep.
- Upstream quirks are parity, not bugs — port them and name them in the test: scrub pauses playback and never auto-resumes (MotionControls.swift:24-28); play restarts from 0 only when time sits at duration (MotionPlayer.swift:24-29); loop wrap uses remainder, clamp only when looping is off (:11-22); starting animation clears sectioning + measuring and forces assembled layout (ModelViewerState.swift:28-38, :78-81); the animation toggle does NOT re-frame the camera (isAnimating's didSet bumps no cameraID); measure/section toggles and the layout picker are disabled while animating (ModelPane.swift:133-135, PartsOverlay.swift:23); a study that CHANGES mid-playback keeps playing (only disappearance stops it, ModelPane.swift:69-73); a single-frame study is inert (upstream drops it at load, ProjectOutput.swift:130); unlabeled frames keep the last label and collisions come from the NEAREST frame (engine-tested at M1).
- Implementers run `cargo check -p <crate>` + the task's focused tests locally ONLY (`cargo test -p engine --test animation`, `cargo test -p viewer --lib`, `cargo test -p viewer --test render_headless <name>`, `cargo test -p app --test <file>`); thelio full-suite gates are controller-owned, single-invocation, never stacked, HEAD-verified after pull. GPU tests take the `render::gpu` shared context (never construct their own Instance).
- Edition 2024 (no bare `env::set_var`), libcosmic rev `dbc62f2d899cdbfbe43d170689ddc0290db663f4` + wgpu 28 exact, conventional commits, stage explicit paths only (never `git add -A`/`-u`/`.`, never `git rm`), commit messages simple double-quoted `-m` (no backticks), rustfmt-clean tree, one-module registration per task (each task registers only its own modules in `lib.rs`), parallel-dispatch cap ≤ 5 agents with staggered reviews. No manifest changes → no Cargo.lock churn, no new dependencies.

## Review Focus

Five failure modes the spec implies but ordinary task tests might miss — each pinned to its owning task's named test:

1. **The study vanishes or goes inert mid-playback** — the agent deletes or truncates animation.json while animating: playback must stop cleanly, poses revert to assembled identity, the tick subscription drops, and a later valid study re-arms (T6 `study_disappearing_mid_playback_stops_cleanly` + `study_swap_keeps_playing`).
2. **A hidden group reappears the moment animation starts** — playback poses are name-keyed and orthogonal to the arrangement's visibility; a port that rebuilds poses from the study alone un-hides the part (upstream keeps `isEnabled = !hidden`, ModelViewer.swift:221-223). Hidden wins over every pose, collision included (T3 `hidden_wins_over_pose_and_collision`, T6 `hidden_group_stays_hidden_during_playback`).
3. **The MODEL-space rotation quaternion is applied unremapped — or composed unpinned** — a MODEL rotation used directly in the Y-up scene tilts parts the wrong way; the imaginary part must cross through the same `(x, z, −y)` remap as translations (ModelViewer.swift:262-263). And because staged vertices carry the pin baked in, `Mat4::from_rotation_translation(q, t)` orbits the part by `q·pin − pin` — wrong whenever the pin is nonzero and the quat isn't identity; the composition must pivot on `scene.pin` (T3's Interfaces). T3 `model_rotation_maps_through_the_scene_remap`, strengthened to a full-matrix `transform_point3` assertion over the pinned scene.
4. **Translucent static parts lose their alpha through the MSAA resolve** — the M3/M6 grid-alpha class of bug, now on mesh parts: a 0.22 part at 4× must composite at 0.22, not snap to opaque or background (T2 `translucent_alpha_survives_the_msaa_resolve`).
5. **The clock teleports after a pause or a stalled main loop** — a stale `last_tick` makes the first post-resume delta span the whole pause. First tick after (re)start must baseline, not advance; deltas are real elapsed, unclamped in-loop (parity), but never fabricated from a stale anchor (T6 `first_tick_after_resume_baselines_instead_of_jumping`).

A sixth, carried explicitly: **values render alphabetically** — `StudyState.values` is a BTreeMap and iterates sorted, while upstream's row order is first-frame-appearance (`value_names()`); the controls must go through `value_names()` (facts, animation.rs:137-144). Pinned by T5 `values_render_in_first_appearance_order`.

---

### Task 1: engine — `MotionStudy::collision_count` + `moving_parts`

**Gates:** none (dispatches immediately; the engine is untouched by every in-flight M6 task).

**Files:**
- Modify: `engine/src/parsing/animation.rs` (two methods on `impl MotionStudy`, after `value_names` at :172-174)
- Test: `engine/tests/animation.rs` (extend; the golden fixture already carries a collision at t=1)

**Interfaces:**
- Consumes: the landed model — `Frame.collisions: Vec<Collision>` (:79), `Frame.parts: HashMap<String, Pose>` (:77), `MotionStudy.frames: Vec<Frame>` (:150).
- Produces (T6's pane and T8's issues call these; T3's tests use them for fixture-derived sets):
  - `pub fn collision_count(&self) -> usize` — upstream `collisionCount`: `frames.filter { !$0.collisions.isEmpty }.count` (MotionStudy.swift:87). Frames, not collisions: a frame with two collision pairs counts once.
  - `pub fn moving_parts(&self) -> std::collections::BTreeSet<String>` — upstream `movingParts`: `Set(frames.flatMap(\.parts.keys))` (MotionStudy.swift:81-83). BTreeSet for deterministic tests; a part named in ANY keyframe moves, even if its pose is identity throughout.

- [ ] **Step 1: Write the failing tests** — append to `engine/tests/animation.rs`:

```rust
#[test]
fn collision_count_counts_frames_not_pairs() {
    let study = MotionStudy::parse(
        br#"{"frames":[
            {"t":0},
            {"t":1,"collisions":[{"parts":["a","b"]},{"parts":["b","c"]}]},
            {"t":2,"collisions":[{"parts":["a","b"]}]}
        ]}"#,
    )
    .unwrap();
    // two colliding FRAMES; the t=1 frame's two pairs count once
    assert_eq!(study.collision_count(), 2);
    assert_eq!(MotionStudy::parse(br#"{"frames":[{"t":0},{"t":1}]}"#)
        .unwrap()
        .collision_count(), 0);
}

#[test]
fn moving_parts_is_the_union_of_keyed_names() {
    let study = MotionStudy::parse(
        br#"{"frames":[
            {"t":0,"parts":{"lid":{"translate":[0,0,0]}}},
            {"t":1,"parts":{"lid":{"translate":[0,0,2]},"cam":{"rotate":[0,0,0,1]}}}
        ]}"#,
    )
    .unwrap();
    let moving = study.moving_parts();
    assert!(moving.contains("lid") && moving.contains("cam"));
    assert_eq!(moving.len(), 2, "a part named in any keyframe moves");
    // the golden fixture: only the lid carries keyframes
    let golden = MotionStudy::parse(&std::fs::read(golden()).unwrap()).unwrap();
    assert_eq!(golden.moving_parts(), ["lid".to_string()].into_iter().collect());
}

#[test]
fn golden_study_reports_one_collision_frame() {
    let golden = MotionStudy::parse(&std::fs::read(golden()).unwrap()).unwrap();
    assert_eq!(golden.collision_count(), 1, "collision at t=1 only");
}
```

- [ ] **Step 2: RED** — `cargo test -p engine --test animation collision_count` → compile error (no such methods).
- [ ] **Step 3: Implement** — in `impl MotionStudy`:

```rust
    /// Upstream `collisionCount`: the number of FRAMES carrying any
    /// collision (MotionStudy.swift:87) — the header warning's count.
    pub fn collision_count(&self) -> usize {
        self.frames
            .iter()
            .filter(|f| !f.collisions.is_empty())
            .count()
    }

    /// Upstream `movingParts`: every part keyed by ANY keyframe
    /// (MotionStudy.swift:81-83). Only these render opaque during
    /// playback; every other part renders translucent at 0.22.
    pub fn moving_parts(&self) -> std::collections::BTreeSet<String> {
        self.frames
            .iter()
            .flat_map(|f| f.parts.keys().cloned())
            .collect()
    }
```

- [ ] **Step 4: GREEN** — `cargo test -p engine --test animation` (all of it — the M1 suite must stay green).
- [ ] **Step 5: Commit** — `git add engine/src/parsing/animation.rs engine/tests/animation.rs && git commit -m "feat(engine): motion study collision_count and moving_parts derived helpers"`

### Task 2: `viewer::render` — motion appearance: 80-byte Model uniform, flat-red collision, translucent blend pipelines

**Gates:** M6 T6 merged (ghost last touched render/mod.rs's pipeline set and draw order) AND M6 T7 merged (scene.rs `set_overlay_frame`/`bed` — same file this task extends).

**Files:**
- Modify: `viewer/src/render/mod.rs` (PartAppearance; Frame.appearances; Model uniform; COLLISION_LINEAR; mesh_wgsl_source markers; blend pipelines; prepare/draw partition), `viewer/src/render/mesh.wgsl` (Model struct + fs_main), `viewer/src/render/scene.rs` (`ModelScene.appearances` threaded into ModelPrimitive/Frame), `viewer/tests/render_headless.rs` (harness extension + tests)
- Test: mod.rs in-crate + headless render tests

**Interfaces:**
- Consumes: T1 (nothing — independent); the landed pose path (`PartPose` :124, `Frame` :133-145, `prepare_parts`' uniform write :1390-1406, `draw_scene` :1596-1639).
- Produces (T3's `playback::poses`, T6's pane):
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)] pub struct PartAppearance { pub translucent: bool, pub collision: bool }` — default `{ false, false }` draws the staged color, lit, opaque (byte-identical to today's frames).
  - `Frame` gains `pub appearances: &'a [PartAppearance]` — same length as `poses` (asserted beside the existing pose-length assert in `prepare_parts`). Every existing `Frame` construction site (tests, scene.rs, the M6 T10 pane seam) adds `appearances: &[]`-equivalent defaults; the in-crate test `frame(...)` helper grows a `defaults(scene)` line.
  - `pub const COLLISION_LINEAR: [f32; 3] = [1.0, 0.0437, 0.0295];` in `render::mod` — linear of systemRed sRGB `(1.0, 59/255, 48/255)` through the exact EOTF; test-pinned against `srgb_to_linear_f` (the CUT_FACE_LINEAR pattern, mod.rs:170).
  - `pub const TRANSLUCENT_ALPHA: f32 = 0.22;` (ModelViewer.swift:410).
  - `Model` uniform becomes 80 bytes: `{ transform: [[f32; 4]; 4], collision: f32, alpha: f32, _pad: [f32; 2] }`; `prepare_parts` writes `collision` from the frame's appearance and `alpha = translucent.then(TRANSLUCENT_ALPHA).unwrap_or(1.0)`. WGSL rounds uniform size to 16 — 64+4+4 pads to 80; the Rust struct must say so explicitly. `model_layout`'s visibility widens from `ShaderStages::VERTEX` (mod.rs:693) to `VERTEX_FRAGMENT` (the camera layout's precedent, mod.rs:670): `fs_main` now reads `model.collision`/`model.alpha`, and pipeline creation fails validation until the layout says so — named here so it isn't hunted at runtime.
  - Two new pipelines from `make_pipelines`: `mesh_blend` and `line_blend` — the mesh and line pipelines with `blend: Some(wgpu::BlendState::ALPHA_BLENDING)`, `depth_write_enabled: false`, `depth_compare: LessEqual` (the grid pipeline's depth stance, mod.rs:1045-1051: translucent parts must not occlude). The clones swap the stencil inc/dec winding-parity ops for `Keep`: the blend pipelines never participate in section parity (section ∩ animation = ∅ by mutual exclusion), and stating that invariant beats silently inheriting ops that rely on it. `line_vertex_count_for_tests` sums BOTH drawn sets (opaque + translucent) so translucent wireframe playback doesn't undercount — byte-identical to today's count on opaque-only frames. `set_msaa` rebuilds them with the rest (it calls `make_pipelines`).
  - `prepare_parts` partitions the drawn slots: `drawn` (opaque: every `Some`-pose slot whose appearance is `collision || !translucent`) and `drawn_translucent` (the rest); `draw_scene` draws the opaque set first (existing pipelines — a collision part colors flat red through the shader flag, still opaque: upstream's collision material REPLACES the translucent one), then the translucent set (`mesh_blend`/`line_blend` by the frame's wireframe mode), then cap, then overlays. `#[cfg(test)] pub(crate) fn drawn_part_count_for_tests` keeps returning the TOTAL (`drawn + drawn_translucent`) so M6's assertions hold.
  - `ModelScene.appearances: Arc<[PartAppearance]>` — opaque-initialized in `new` and `replace_scene`, carried through `draw()` → `ModelPrimitive` → `prepare`'s `Frame`.

- [ ] **Step 1: Write the failing tests** — in `viewer/src/render/mod.rs` tests:

```rust
    /// The WGSL uniform struct rounds to 16-byte boundaries: 64 + 4 + 4 is 72,
    /// which must be padded to 80 in BOTH the Rust repr and the WGSL struct —
    /// a mismatch fails bind-group validation at min_binding_size, loudly.
    #[test]
    fn model_uniform_is_80_bytes() {
        assert_eq!(
            std::mem::size_of::<Model>(),
            80,
            "transform 64 + collision 4 + alpha 4 + pad 8"
        );
    }

    /// COLLISION_LINEAR is systemRed through the exact EOTF (the same red
    /// ModelViewer.swift:363 blends 15% black for the cut face; the motion
    /// material uses it straight, ModelViewer.swift:275-279).
    #[test]
    fn collision_color_is_linear_system_red() {
        let srgb = [1.0_f32, 59.0 / 255.0, 48.0 / 255.0];
        for i in 0..3 {
            assert!(
                (COLLISION_LINEAR[i] - parts::srgb_to_linear_f(srgb[i])).abs() < 1e-3,
                "channel {i}: {} vs {}",
                COLLISION_LINEAR[i],
                parts::srgb_to_linear_f(srgb[i])
            );
        }
    }

    /// The whole motion surface is uniform state: a playback-shaped sweep —
    /// pose matrices AND collision/translency flags changing every step —
    /// uploads zero vertices and keeps both parts drawn.
    #[test]
    fn motion_sweep_moves_poses_and_flags_without_uploads() {
        let (device, queue) = gpu::device_queue();
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        let camera = default_camera(&scene, 4.0 / 3.0);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let id = glam::Mat4::IDENTITY.to_cols_array_2d();
        let mut uploads_after_first = None;
        for step in 0..24u32 {
            let poses: Vec<PartPose> = vec![
                Some(id),
                Some(
                    glam::Mat4::from_translation(glam::Vec3::new(
                        0.0,
                        step as f32 * 0.5,
                        0.0,
                    ))
                    .to_cols_array_2d(),
                ),
            ];
            let appearances = vec![
                PartAppearance {
                    collision: step % 2 == 0,
                    translucent: step % 3 == 0,
                },
                PartAppearance::default(),
            ];
            let f = Frame {
                appearances: &appearances,
                ..frame(&camera, &scene, &poses)
            };
            pipeline.prepare_parts(device, queue, (800, 600), &f);
            if step == 0 {
                uploads_after_first = Some(pipeline.upload_count_for_tests());
            }
            assert_eq!(pipeline.drawn_part_count_for_tests(), 2,
                "a translucent part is still drawn (step {step})");
        }
        assert_eq!(
            pipeline.upload_count_for_tests(),
            uploads_after_first.unwrap(),
            "24 playback frames: zero vertex uploads"
        );
    }
```

  And in `viewer/tests/render_headless.rs` (extend the harness: `render_frame_appearances` — the `render_frame_poses` shape plus an `appearances: &[PartAppearance]` parameter threaded into its `Frame`; existing call sites keep a defaults wrapper — plus two small helpers added BESIDE the existing ones, never twinned: `identity_poses(scene: &PartScene) -> Vec<PartPose>` (the in-crate test helper's shape, mod.rs — private there, absent from this file: `vec![Some(glam::Mat4::IDENTITY.to_cols_array_2d()); scene.parts.len()]`) and `encode_srgb(linear: f32) -> u8`, the EOTF's inverse with round-to-nearest, so blend expectations round-trip decode → lerp → encode exactly as fixed-function sRGB blending does):

```rust
/// A static part renders translucent at upstream's 0.22: an sRGB attachment
/// blends in LINEAR space (decode → blend → re-encode — the fixed-function
/// sRGB rule), so the stored byte is
/// `encode(0.22·decode(opaque_byte) + 0.78·decode(bg_byte))` — NOT raw-u8
/// arithmetic (opaque 200 over bg 89 stores 125, not 113: ~12 counts apart,
/// far beyond the ±2 tolerance).
#[test]
fn translucent_part_blends_at_upstream_opacity() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let cam = default_camera(&scene, 800.0 / 600.0);
    let poses = identity_poses(&scene);
    let plain = vec![PartAppearance::default(); scene.parts.len()];
    let px_opaque =
        render_frame_appearances(device, queue, &mut pipeline, &scene, &poses, &plain, 800, 600);
    let bg = background_reference(device, queue, &mut pipeline, &cam, 800, 600);
    let translucent = vec![
        PartAppearance {
            translucent: true,
            ..PartAppearance::default()
        },
        PartAppearance::default(),
    ];
    let px_blend =
        render_frame_appearances(device, queue, &mut pipeline, &scene, &poses, &translucent, 800, 600);
    // a base top-face point OUTSIDE the lid footprint (lid spans x/y ±20):
    // MODEL (26, 0, 4) is lid-free base top face
    let p = project_px(cam.view_proj, scene.to_scene([26.0, 0.0, 4.0]), 800.0, 600.0)
        .expect("top face projects inside the frame");
    let idx = (p.1 as usize * 800 + p.0 as usize) * 4;
    for c in 0..3 {
        // decode → lerp → encode in f32: the sRGB attachment blends linearized
        let expected = encode_srgb(
            0.22 * srgb_to_linear(px_opaque[idx + c]) + 0.78 * srgb_to_linear(bg[c]),
        );
        assert!(
            (px_blend[idx + c] as i32 - expected as i32).abs() <= 2,
            "channel {c}: {} vs linear-blended {expected}",
            px_blend[idx + c]
        );
    }
}

/// A colliding part renders FLAT red — unlit, the same value from every
/// normal (upstream's emissive collision material, ModelViewer.swift:275-279)
/// — and opaque: the collision flag replaces translucency.
#[test]
fn collision_paints_flat_red_on_every_normal() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let poses = identity_poses(&scene);
    let look = vec![
        PartAppearance {
            collision: true,
            translucent: true, // collision must win over translucency
        },
        PartAppearance::default(),
    ];
    let px =
        render_frame_appearances(device, queue, &mut pipeline, &scene, &poses, &look, 800, 600);
    let cam = default_camera(&scene, 800.0 / 600.0);
    // top face and +x side face: different normals, same flat color
    for model_point in [[26.0, 0.0, 4.0], [30.0, 8.0, -2.0]] {
        let p = project_px(cam.view_proj, scene.to_scene(model_point), 800.0, 600.0)
            .expect("face point projects inside the frame");
        let px4: [u8; 4] = px[(p.1 as usize * 800 + p.0 as usize) * 4..][..4]
            .try_into()
            .unwrap();
        let lin = decode_linear(&px4);
        for i in 0..3 {
            assert!(
                (lin[i] - viewer::render::COLLISION_LINEAR[i]).abs() < 0.05,
                "MODEL {model_point:?} channel {i}: {lin:?}"
            );
        }
    }
}

/// Carry-forward class (the M3/M6 grid-alpha bug, now on mesh parts): a 0.22
/// part through the 4x MSAA resolve must still composite at 0.22 — neither
/// snapped to the opaque color (alpha lost) nor to the background.
#[test]
fn translucent_alpha_survives_the_msaa_resolve() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let mut pipeline =
        MeshPipeline::with_msaa(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, 4);
    let poses = identity_poses(&scene);
    let plain = vec![PartAppearance::default(); scene.parts.len()];
    let px_opaque =
        render_frame_appearances(device, queue, &mut pipeline, &scene, &poses, &plain, 800, 600);
    let translucent = vec![
        PartAppearance {
            translucent: true,
            ..PartAppearance::default()
        },
        PartAppearance::default(),
    ];
    let px_blend =
        render_frame_appearances(device, queue, &mut pipeline, &scene, &poses, &translucent, 800, 600);
    let cam = default_camera(&scene, 800.0 / 600.0);
    let bg = background_reference(device, queue, &mut pipeline, &cam, 800, 600);
    let p = project_px(cam.view_proj, scene.to_scene([26.0, 0.0, 4.0]), 800.0, 600.0).unwrap();
    let idx = (p.1 as usize * 800 + p.0 as usize) * 4;
    let distance_from_opaque =
        (px_blend[idx] as i32 - px_opaque[idx] as i32).abs();
    let distance_from_bg = (px_blend[idx] as i32 - bg[0] as i32).abs();
    assert!(
        distance_from_opaque > 10 && distance_from_bg > 10,
        "blended pixel sits between opaque ({}) and background ({}), not on either",
        px_opaque[idx],
        bg[0]
    );
}
```

  (`default_camera`/`project_px`/`decode_linear`/`background_reference`/`srgb_to_linear` are the harness's existing helpers with exactly these shapes — `project_px` takes `cam.view_proj` (the matrix, not the camera), `background_reference(device, queue, pipeline, camera, w, h)` returns the ONE `[u8; 4]` corner pixel, and `golden_scene()` returns the `PartScene` itself, so `to_scene` is called on it directly. `identity_poses` and `encode_srgb` are the two helpers this task ADDS to the harness, beside the existing ones.)

- [ ] **Step 2: RED** — `cargo test -p viewer --lib model_uniform` → compile error (no `PartAppearance`); headless tests fail on the missing harness parameter.
- [ ] **Step 3: Implement** — mod.rs: `PartAppearance`, `COLLISION_LINEAR`, `TRANSLUCENT_ALPHA`, the 80-byte `Model`, `mesh_wgsl_source` gains `@COLLISION_R@/@COLLISION_G@/@COLLISION_B@` substitution from `COLLISION_LINEAR`; `model_layout`'s visibility widens VERTEX → `VERTEX_FRAGMENT` (fs_main reads the new fields; creation fails validation until it does); `make_pipelines` builds `mesh_blend`/`line_blend` (clone the mesh/line descriptors, swap blend + depth-write, stencil ops → `Keep`); `prepare_parts` writes the extended uniform and partitions `drawn`/`drawn_translucent`; `draw_scene` gains the translucent loop. mesh.wgsl:

```wgsl
struct Model {
    transform: mat4x4<f32>,
    collision: f32,
    alpha: f32,
    _pad: vec2<f32>,
};
const COLLISION_R: f32 = @COLLISION_R@;
const COLLISION_G: f32 = @COLLISION_G@;
const COLLISION_B: f32 = @COLLISION_B@;
```

  `fs_main` tail (after the section discard and the lit term):

```wgsl
    if (model.collision > 0.5) {
        // upstream's collision material: emissive systemRed — flat from
        // every normal, opaque even on a would-be-translucent part
        return vec4<f32>(vec3<f32>(COLLISION_R, COLLISION_G, COLLISION_B), 1.0);
    }
    return vec4<f32>(in.color * clamp(lit, 0.0, LIT_MAX), model.alpha);
```

  scene.rs: `appearances` field + opaque init + `ModelPrimitive.appearances` + the Frame field. Fix every `Frame { .. }` construction (mod.rs tests via the shared helper; M6's pane seam if merged state differs).
- [ ] **Step 4: GREEN** — `cargo test -p viewer` (lib + integration files; the M6 suite must stay green — the opaque default path is byte-identical) and `cargo check -p app`.
- [ ] **Step 5: Commit** — `git add viewer/src/render/mod.rs viewer/src/render/mesh.wgsl viewer/src/render/scene.rs viewer/tests/render_headless.rs && git commit -m "feat(viewer): motion appearance uniforms — flat-red collision and 0.22 translucent blend pipelines"`

### Task 3: `viewer::playback` — MotionPlayer + StudyState→poses/appearances (the spec's playback.rs)

**Gates:** T1 merged (`moving_parts` in tests) AND T2 merged (`PartAppearance`).

**Files:**
- Create: `viewer/src/playback.rs`
- Modify: `viewer/src/lib.rs` (register `pub mod playback;` with a module doc paragraph)
- Test: in-crate `#[cfg(test)] mod tests` at the end of `playback.rs`

**Interfaces:**
- Consumes: T2's `PartAppearance`; landed `PartPose`, `PartScene`; engine's `MotionStudy`, `StudyState`, `Pose`.
- Produces (T4 stores the player; T5 renders it; T6 drives it):
  - `pub const SPEEDS: [f64; 5] = [0.1, 0.25, 0.5, 1.0, 2.0];` (MotionControls.swift:7) and `pub const SCRUB_FLOOR: f64 = 0.001;` (the slider's lower bound `max(duration, 0.001)`, MotionControls.swift:23).
  - `#[derive(Clone, Copy, Debug, PartialEq)] pub struct MotionPlayer { pub time: f64, pub is_playing: bool, pub speed: f64, pub loops: bool }` with `Default` = `{ 0.0, false, 1.0, true }` (MotionPlayer.swift:6-9).
  - `impl MotionPlayer`:
    - `pub fn advance(&mut self, seconds: f64, duration: f64)` — MotionPlayer.swift:11-22 verbatim semantics: no-op unless playing and duration > 0; `next = time + seconds * speed`; `next < duration` → time = next; else loops → `next % duration`; else `time = duration; is_playing = false`. The `<` is strict: landing exactly on duration wraps to 0 when looping.
    - `pub fn toggle_playback(&mut self, duration: f64)` — :24-29: if `!is_playing && time >= duration` → `time = 0`; then flip `is_playing`.
  - `pub fn poses(scene: &PartScene, state: &StudyState, moving: &BTreeSet<String>, hidden: &BTreeSet<String>) -> (Vec<PartPose>, Vec<PartAppearance>)` — per part slot, in scene.parts order:
    - `hidden.contains(part.name)` → pose `None` (hidden wins over everything — upstream `isEnabled`, ModelViewer.swift:221-223).
    - else the study pose, defaulting to identity for parts the state doesn't name (:261): translation remapped MODEL→SCENE `(x, z, −y)`; rotation through the SAME remap on the quaternion's imaginary part — `Quat::from_xyzw(q.x, q.z, −q.y, q.w)` (upstream `simd_quatf(vector: SIMD4(sceneVector(rotate.imag), rotate.real))`, ModelViewer.swift:263; correct because a rotation about MODEL axis `a` is a rotation about the remapped scene axis `M·a` by the same angle) — composed THROUGH THE PIN: staged vertices carry `scene.pin` baked in (`from_parts`'s staging loop, parts.rs), while upstream rotates about the UN-pinned remapped MODEL origin — the part entities hang under `content`, whose position is the pin (ModelViewer.swift:168), and `pose()` sets position/orientation in that frame (:262-263), so upstream world = `pin + q·v_remapped + t`. Composing `Mat4::from_rotation_translation(quat, translate)` on the pinned vertices yields `q·v + q·pin + t` instead — the part orbits by `q·pin − pin`, wrong whenever the pin is nonzero and the quat isn't identity. The correct composition is `Mat4::from_rotation_translation(quat, translate + pin − quat·pin)` with `pin = glam::Vec3::from(scene.pin)` (equivalently `translate(pin)·rot(quat)·translate(−pin)·translate(t)`), which reduces to the naive formula when the quat is identity — all golden-fixture expectations (translation-only studies) stay valid, which is exactly why the pivot needs its own non-identity test.
    - appearance: `collision` = part named in any `state.collisions[].parts` (upstream `collidingParts`, MotionStudy.swift:70-72); `translucent = !collision && !moving.contains(part.name)` (:282-284 — non-moving parts ride the 0.22 material).

- [ ] **Step 1: Write the failing tests** — `viewer/src/playback.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn fixture(rel: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/golden")
            .join(rel)
    }

    fn golden_parts() -> Vec<engine::parsing::Part> {
        let bytes = std::fs::read(fixture("model.3mf")).unwrap();
        engine::parsing::threemf::parse_3mf(&bytes).unwrap()
    }

    fn golden_study() -> MotionStudy {
        MotionStudy::parse(&std::fs::read(fixture("animation.json")).unwrap()).unwrap()
    }

    #[test]
    fn advance_below_loops_and_clamps_like_upstream() {
        let mut p = MotionPlayer {
            is_playing: true,
            ..Default::default()
        };
        p.advance(0.5, 2.0);
        assert!((p.time - 0.5).abs() < 1e-9);
        // loop wrap uses remainder: 1.9 + 0.2 -> 0.1
        p.time = 1.9;
        p.advance(0.2, 2.0);
        assert!((p.time - 0.1).abs() < 1e-9, "wrapped: {}", p.time);
        // STRICT <: landing exactly on duration wraps to 0 while looping
        p.time = 1.5;
        p.advance(0.5, 2.0);
        assert!(p.time.abs() < 1e-9, "exact landing wraps: {}", p.time);
        // clamp when looping is off, and playback stops
        p.loops = false;
        p.time = 1.9;
        p.advance(0.2, 2.0);
        assert!((p.time - 2.0).abs() < 1e-9);
        assert!(!p.is_playing, "non-loop end stops playback");
        // paused advance is a no-op; so is duration 0
        p.is_playing = false;
        p.time = 0.25;
        p.advance(0.5, 2.0);
        assert!((p.time - 0.25).abs() < 1e-9);
        p.is_playing = true;
        p.advance(1.0, 0.0);
        assert!((p.time - 0.25).abs() < 1e-9, "zero-duration study never advances");
    }

    #[test]
    fn toggle_restarts_only_at_the_end() {
        let mut p = MotionPlayer {
            is_playing: false,
            time: 2.0,
            ..Default::default()
        };
        p.toggle_playback(2.0);
        assert!(p.is_playing && p.time == 0.0, "at duration: restart from 0");
        p.toggle_playback(2.0);
        assert!(!p.is_playing);
        p.time = 0.7;
        p.toggle_playback(2.0);
        assert!(p.is_playing && (p.time - 0.7).abs() < 1e-9, "mid-scrub resume keeps time");
    }

    /// The load-bearing remap AND pivot: a MODEL rotation about MODEL +Z by
    /// 90° moves MODEL (1,0,0) to (0,1,0); staged scene space maps those to
    /// (1,0,0) and (0,0,-1) — so the scene quaternion must rotate scene x
    /// toward scene -z (an unremapped quaternion rotates toward scene +y,
    /// the wrong floor). And because staged vertices carry the pin baked in,
    /// the FULL matrix must land a staged point at `pin + q·remap(v)` — an
    /// unpinned `from_rotation_translation(q, t)` orbits it by `q·pin − pin`
    /// (here: MODEL (1,0,0) lands at (0.5, 0, -0.5) instead of
    /// (-0.5, 0, -0.5), a full unit off).
    #[test]
    fn model_rotation_maps_through_the_scene_remap() {
        let part = engine::parsing::Part {
            name: "lid".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangle_colors: vec![None],
        };
        let scene = PartScene::from_parts(std::slice::from_ref(&part), crate::render::NEUTRAL);
        let mut state = StudyState::default();
        state.parts.insert(
            "lid".to_string(),
            engine::parsing::animation::Pose {
                translate: [0.0; 3],
                rotate: glam::Quat::from_axis_angle(
                    glam::Vec3::Z,
                    std::f32::consts::FRAC_PI_2,
                ),
            },
        );
        let (poses, _) = poses(&scene, &state, &BTreeSet::new(), &BTreeSet::new());
        let m = glam::Mat4::from_cols_array_2d(&poses[0].expect("visible"));
        // direction: the scene quaternion rotates scene x toward scene -z
        let rotated = m.transform_vector3(glam::Vec3::new(1.0, 0.0, 0.0));
        assert!(
            (rotated - glam::Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5,
            "MODEL +Z quarter-turn maps to scene -z: {rotated:?}"
        );
        // FULL matrix over the PINNED scene: every staged vertex (remap(v) +
        // pin) must land at pin + q·remap(v). transform_point3 keeps the
        // translation term, which is exactly where the pivot error lives —
        // transform_vector3 strips it. q_scene is derived independently: a
        // rotation about MODEL +Z by θ is a rotation about the remapped
        // axis, scene +y, by the same θ.
        let pin = glam::Vec3::from(scene.pin);
        let remap = |v: [f32; 3]| glam::Vec3::new(v[0], v[2], -v[1]);
        let q_scene = glam::Quat::from_axis_angle(glam::Vec3::Y, std::f32::consts::FRAC_PI_2);
        for v in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            let v_scene = remap(v);
            let landed = m.transform_point3(v_scene + pin);
            let expected = pin + q_scene * v_scene;
            assert!(
                (landed - expected).length() < 1e-5,
                "MODEL {v:?}: landed {landed:?} vs pin + q·remap(v) {expected:?}"
            );
        }
    }

    #[test]
    fn golden_poses_translucent_and_collision_states() {
        let scene = PartScene::from_parts(&golden_parts(), crate::render::NEUTRAL);
        let study = golden_study();
        let moving = study.moving_parts();
        // t = 0.5: lid lifted MODEL z 5 -> scene +y 5; base static -> translucent
        let (p, look) = poses(&scene, &study.state_at(0.5), &moving, &BTreeSet::new());
        assert_eq!(scene.parts[0].name, "base");
        assert_eq!(p[0], Some(glam::Mat4::IDENTITY.to_cols_array_2d()), "base holds identity");
        let lid = glam::Mat4::from_cols_array_2d(&p[1].expect("lid visible"));
        let translation = glam::Vec3::new(lid.x_axis.w, lid.y_axis.w, lid.z_axis.w);
        assert!(
            (translation - glam::Vec3::new(0.0, 5.0, 0.0)).length() < 1e-4,
            "MODEL z 5 crosses to scene +y 5: {translation:?}"
        );
        assert!(look[0].translucent && !look[0].collision, "static base translucent");
        assert!(!look[1].translucent && !look[1].collision, "moving lid opaque");
        // t = 1.0: the golden frame carries base<->lid at volume 0.5
        let (_, look) = poses(&scene, &study.state_at(1.0), &moving, &BTreeSet::new());
        assert!(look[0].collision && look[1].collision, "both members flagged");
        // a colliding STATIC part stays opaque: collision replaces translucency
        assert!(!look[0].translucent);
    }

    #[test]
    fn hidden_wins_over_pose_and_collision() {
        let scene = PartScene::from_parts(&golden_parts(), crate::render::NEUTRAL);
        let study = golden_study();
        let hidden = BTreeSet::from(["lid".to_string()]);
        let (p, _) = poses(
            &scene,
            &study.state_at(1.0),
            &study.moving_parts(),
            &hidden,
        );
        assert!(p[1].is_none(), "hidden member never draws (ModelViewer.swift:221-223)");
        assert!(p[0].is_some(), "the visible member still poses");
    }
}
```

- [ ] **Step 2: RED** — `cargo test -p viewer --lib playback` → compile error (module missing).
- [ ] **Step 3: Implement** — the module per the Interfaces block; `scene_quat` stays private. `moving`/`hidden` as `BTreeSet<String>` matches T1's return type.
- [ ] **Step 4: GREEN** — `cargo test -p viewer --lib` (full crate; M6 suite green).
- [ ] **Step 5: Commit** — `git add viewer/src/playback.rs viewer/src/lib.rs && git commit -m "feat(viewer): motion player and study-state pose derivation with scene-space quaternion remap"`

### Task 4: `app::viewer_tools` — the `isAnimating` semantics (ModelViewerState's didSet)

**Gates:** M6 T8 merged (`viewer_tools.rs` exists) AND T3 merged (`MotionPlayer`).

**Files:**
- Modify: `app/src/viewer_tools.rs`, `app/tests/viewer_tools_model.rs`
- Test: the app integration test file

**Interfaces:**
- Consumes: T3's `viewer::playback::MotionPlayer`; M6 T8's `ViewerTools` (fields incl. `sectioning`, `measuring`, `layout`; `effective_layout(parts_count, comparing)`).
- Produces (T6's pane, M6 T9's picker-disabled gate grows a term):
  - `ViewerTools` gains `pub animating: bool` (default `false`) and `pub player: MotionPlayer` (default).
  - `pub fn set_animating(&mut self, on: bool)` — ModelViewerState.swift:28-38's didSet, verbatim: `player.time = 0; player.is_playing = on; if on { sectioning = false; measuring = false }`. Toggling OFF does not clear sectioning/measuring (they were already cleared when it went on).
  - `effective_layout`'s body gains the animating term: `self.animating || comparing` → `PartsLayout::Assembled` (ModelViewerState.swift:79 — the term M6 T8 deliberately omitted; this task makes it load-bearing). Signature UNCHANGED (`(&self, parts_count: usize, comparing: bool)`).
  - `pub fn motion_available(&self, parts_count: usize, study_present: bool) -> bool` — upstream's controls gate: the Animazione toggle shows iff `study_present && parts_count > 1` (ModelPane.swift:13-15, :136).

- [ ] **Step 1: Write the failing tests** — extend `app/tests/viewer_tools_model.rs`:

```rust
#[test]
fn set_animating_ports_the_didset_semantics() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    t.sectioning = true;
    t.measuring = true;
    t.player.time = 1.25;
    t.set_animating(true);
    assert!(t.animating);
    assert_eq!(t.player.time, 0.0, "time resets on every toggle");
    assert!(t.player.is_playing);
    assert!(!t.sectioning && !t.measuring, "starting animation clears both");
    // toggling off keeps the (already cleared) tools and stops the player
    t.player.time = 0.5;
    t.set_animating(false);
    assert!(!t.animating && !t.player.is_playing);
    assert!(!t.sectioning && !t.measuring, "off does not re-enable them");
}

#[test]
fn animating_forces_assembled_layout() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    assert_eq!(t.effective_layout(2, false), PL::Exploded);
    t.set_animating(true);
    assert_eq!(t.effective_layout(2, false), PL::Assembled);
    assert_eq!(t.effective_layout(2, true), PL::Assembled);
    t.set_animating(false);
    assert_eq!(t.effective_layout(2, false), PL::Exploded, "layout intent survives");
    assert_eq!(t.layout, PL::Exploded);
}

#[test]
fn motion_gate_needs_study_and_two_parts() {
    let mut t = ViewerTools::new(PL::Exploded, false);
    assert!(!t.motion_available(2, false), "no study: the ask button shows instead");
    assert!(t.motion_available(2, true));
    assert!(!t.motion_available(1, true), "single-part model never animates (ModelPane.swift:13-15)");
}
```

- [ ] **Step 2: RED** — `cargo test -p app --test viewer_tools_model set_animating` → compile error.
- [ ] **Step 3: Implement** — per the Interfaces block; the module doc's M6-era note ("isAnimating deliberately omitted … becomes load-bearing at M7") is replaced by a present-tense sentence naming `set_animating`.
- [ ] **Step 4: GREEN** — `cargo test -p app --test viewer_tools_model` (whole file — M6's cases must stay green; the `effective_layout_forces_assembled_when_comparing` test may gain an animating assert line in the same edit).
- [ ] **Step 5: Commit** — `git add app/src/viewer_tools.rs app/tests/viewer_tools_model.rs && git commit -m "feat(app): viewer tools isAnimating semantics — mutual exclusion and assembled-forced layout"`

### Task 5: `app::motion_controls` — the MotionControls widget + Swift-format helpers

**Gates:** T3 merged (`MotionPlayer`, `SPEEDS`) AND T6 merged (the widget maps into the pane `Message` variants T6 lands) AND M6 T9 merged (the pane's overlay-mount pattern this widget joins; the fl!-in-test pattern).

**Files:**
- Create: `app/src/motion_controls.rs`, `app/tests/motion_controls_model.rs`
- Modify: `app/src/lib.rs` (register)
- Test: the integration file + pane in-crate construction test (in T7's file, not here — this task's view fn is exercised through its pure halves here)

**Interfaces:**
- Consumes: T3's `MotionPlayer`/`SPEEDS`/`SCRUB_FLOOR`; engine's `MotionStudy`/`Collision`; `viewer_pane::Message` (the widget maps into it).
- Produces (T7 mounts; T6's tests read the helpers):
  - `pub fn time_readout(time: f64, duration: f64) -> String` — `"{time:.2} / {duration:.2} s"` (MotionControls.swift:31, fractionLength(2), literal " s").
  - `pub fn format_significant(value: f64, min_digits: usize, max_digits: usize) -> String` — Swift's `.precision(.significantDigits(min...max))` for the two shapes rendered (values 1...3, volume 1...2): round to `max` significant digits (via the magnitude/decimals form), strip fractional trailing zeros down to `min` significant digits, group thousands with `,`. `0.0`/non-finite → `"0"`.
  - `pub fn speed_label(speed: f64) -> String` — `"{speed}×"` (Display-shortest: `0.1×`, `0.25×`, `0.5×`, `1×`, `2×`; MotionControls.swift:44).
  - `pub fn collision_text(collisions: &[Collision]) -> String` — MotionControls.swift:81-89: per collision `parts.join(" ↔ ")` plus, when volume is `Some`, `" {volume}"` through `format_significant(v, 1, 2)` + `" mm³"`; joined `", "`. The `↔`, `mm³`, and separators are literals (upstream hardcodes them).
  - `pub fn view_motion_controls<'a>(player: &MotionPlayer, study: &MotionStudy) -> cosmic::Element<'a, crate::viewer_pane::Message>` — MotionControls.swift:9-79: computes `study.state_at(player.time)` once; row 1 = play/pause icon button (`media-playback-start`/`media-playback-pause` — the in-tree icon precedent, chat_view.rs:824; description `fl!("riproduci")`/`fl!("pausa")` by state) → `cosmic::widget::slider(0.0..=(duration.max(SCRUB_FLOOR)), player.time, Message::Scrub)` (description `fl!("tempo-dell-animazione")`) → `time_readout` in `cosmic::font::mono()` size 12, dim class → speed `dropdown(SPEEDS-labels, selected index, |i| Message::SetSpeed(SPEEDS[i]))` (label `speed_label(player.speed)`; description `fl!("velocit")`, the upstream menu title) plus a loop toggle button (`fl!("ripeti")`, selected class when `player.loops`) — the upstream nests both in one menu (MotionControls.swift:35-49); the port shows dropdown + adjacent loop button, a documented micro-divergence (one menu's worth of popover machinery not worth a two-item menu). Row 2 shows only when the state has a label, values, or collisions (swift :51): the label (weight medium), each value `"{} {}"` name + `format_significant(value, 1, 3)` mono dim — ITERATED IN `study.value_names()` ORDER, reading `state.values` by key (the BTreeMap alphabetizes; upstream's row order is first-frame-appearance — facts, animation.rs:137-144) — and, when collisions are non-empty, a red `dialog-warning` icon + `collision_text(state.collisions)`. Container: padding `[8, 12]`, corner radius 10, `max_width(560)`; the whole thing bottom-center with 10 px outer padding (T7 mounts it where M6 T9 mounts section controls).

- [ ] **Step 1: Write the failing tests** — `app/tests/motion_controls_model.rs`:

```rust
use app::motion_controls::{collision_text, format_significant, speed_label, time_readout};
use engine::parsing::animation::Collision;

#[test]
fn time_readout_is_two_fraction_digits() {
    assert_eq!(time_readout(0.0, 2.0), "0.00 / 2.00 s");
    assert_eq!(time_readout(1.25, 2.0), "1.25 / 2.00 s");
}

#[test]
fn significant_digits_match_the_two_swift_shapes() {
    // values row (1...3): MotionControls.swift:58
    assert_eq!(format_significant(10.0, 1, 3), "10");
    assert_eq!(format_significant(0.5, 1, 3), "0.5");
    assert_eq!(format_significant(3.14159, 1, 3), "3.14");
    assert_eq!(format_significant(1234.5, 1, 3), "1,230", "grouped, 3 sig");
    assert_eq!(format_significant(0.0, 1, 3), "0");
    // volume (1...2): MotionControls.swift:86
    assert_eq!(format_significant(0.5, 1, 2), "0.5");
    assert_eq!(format_significant(12.345, 1, 2), "12");
    assert_eq!(format_significant(1.0, 2, 2), "1.0", "min binds: keep the .0");
}

#[test]
fn speed_labels_are_shortest_display() {
    for (speed, label) in [
        (0.1, "0.1×"),
        (0.25, "0.25×"),
        (0.5, "0.5×"),
        (1.0, "1×"),
        (2.0, "2×"),
    ] {
        assert_eq!(speed_label(speed), label);
    }
}

#[test]
fn collision_text_pairs_volumes_and_joins() {
    let one = Collision {
        parts: vec!["base".into(), "lid".into()],
        volume: Some(0.5),
    };
    assert_eq!(collision_text(&[one.clone()]), "base ↔ lid 0.5 mm³");
    let unmeasured = Collision {
        parts: vec!["cam".into(), "lid".into()],
        volume: None,
    };
    assert_eq!(
        collision_text(&[unmeasured.clone()]),
        "cam ↔ lid",
        "no volume: the pair alone"
    );
    assert_eq!(
        collision_text(&[one, unmeasured]),
        "base ↔ lid 0.5 mm³, cam ↔ lid"
    );
}

#[test]
fn values_render_in_first_appearance_order() {
    // a synthetic study whose total order is NOT alphabetical: within a
    // frame the engine iterates a sorted BTreeMap, then appends each
    // frame's new names — so Mid (frame 2; sorts before Zed) trails Zed
    let study = engine::parsing::animation::MotionStudy::parse(
        br#"{"frames":[
            {"t":0,"values":{"Zed":1.0,"Alpha":2.0}},
            {"t":1,"values":{"Mid":3.0,"Alpha":2.5}}
        ]}"#,
    )
    .unwrap();
    assert_eq!(
        study.value_names(),
        ["Alpha", "Zed", "Mid"],
        "engine order: sorted within a frame, first-appearance across frames"
    );
    // at t=0.5 every name is interpolated into the state: value_names order
    // (Alpha, Zed, Mid) and BTreeMap order (Alpha, Mid, Zed) differ exactly
    // here — at t=0 Mid is absent and the two orders coincide
    let state = study.state_at(0.5);
    let rendered: Vec<&str> = study
        .value_names()
        .iter()
        .filter_map(|n| state.values.get(*n).map(|_| *n))
        .collect();
    assert_eq!(rendered, ["Alpha", "Zed", "Mid"],
        "the controls must iterate value_names(), never the BTreeMap");
}
```

- [ ] **Step 2: RED** — `cargo test -p app --test motion_controls_model` → compile error (module missing).
- [ ] **Step 3: Implement** — the pure helpers plus `view_motion_controls` per the Interfaces block. `format_significant` shape:

```rust
pub fn format_significant(value: f64, min_digits: usize, max_digits: usize) -> String {
    if value == 0.0 || !value.is_finite() {
        return "0".to_string();
    }
    // round at the decade level first when the integer digits exceed max,
    // then re-derive the magnitude (a carry can grow it: 999.5 -> 1000)
    let magnitude = value.abs().log10().floor() as i32 + 1; // integer digits
    let round_to = 10f64.powi((magnitude - max_digits as i32).max(0));
    let value = (value / round_to).round() * round_to;
    let magnitude = value.abs().log10().floor() as i32 + 1;
    let decimals = (max_digits as i32 - magnitude).clamp(0, 18) as usize;
    let mut digits = format!("{value:.decimals$}");
    let significant = |s: &str| {
        s.trim_start_matches(['-', '0'])
            .chars()
            .filter(|c| c.is_ascii_digit())
            .count()
    };
    while digits.contains('.') && digits.ends_with('0') && significant(&digits) > min_digits {
        digits.pop();
    }
    if digits.ends_with('.') {
        digits.pop();
    }
    group_thousands(&digits)
}
```

  with `fn group_thousands(s: &str) -> String` inserting `,` every three integer digits (sign-aware; `1230` → `1,230`). When the integer digits exceed `max`, round FIRST at the decade level and re-derive the magnitude (a carry can grow it):

```rust
    let round_to = 10f64.powi((magnitude - max_digits as i32).max(0));
    let value = (value / round_to).round() * round_to;
    let magnitude = value.abs().log10().floor() as i32 + 1; // recompute post-carry
```

  (`1234.5` at 1..3 → round to `1230` → `"1,230"`; without the decade rounding, `decimals = 0` would print all four digits.) The view maps into the pane `Message` variants T6 lands (`TogglePlayback`, `Scrub`, `SetSpeed`, `ToggleLoops`) — the gate above makes them exist.

- [ ] **Step 4: GREEN** — `cargo test -p app --test motion_controls_model`; `cargo check -p app`.
- [ ] **Step 5: Commit** — `git add app/src/motion_controls.rs app/src/lib.rs app/tests/motion_controls_model.rs && git commit -m "feat(app): motion controls widget and Swift-parity number formatting"`

### Task 6: pane model — study loading, the playback clock, pose application

**Gates:** T1, T2, T3, T4 merged (T2 because this task writes and resets `scene.appearances` — its Interfaces consume a T2 product, so the gate names it); M6 T10 merged (`apply_arrangement`, `model_parts`, the pane test helper) AND M6 T11 merged (marker clear sites + the Escape arm this task's mutual exclusion rides).

**Files:**
- Modify: `app/src/viewer_pane.rs`, `app/src/localize.rs` (the interim placeable substitution, if M6 T9 did not land one)
- Test: pane in-crate tests (the M6 T10-established helper)

**Interfaces:**
- Consumes: T1 (`collision_count` — via T8's accessor, computed here too), T3 (`MotionPlayer`, `playback::poses`), T4 (`set_animating`, `motion_available`), T2 (`scene.appearances`), M6 T10 (`apply_arrangement`, `hidden`-by-group, `model_parts`), the engine watcher's `ReloadKind::Motion`.
- Produces (T7's view, T8's window):
  - New `Message` variants: `ToggleAnimation`, `TogglePlayback`, `Scrub(f64)`, `SetSpeed(f64)`, `ToggleLoops`, `Tick(std::time::Instant)`, `RequestMotionStudy`.
  - `ViewerPane` gains `pub study: Option<engine::parsing::animation::MotionStudy>`, `pub motion_error: Option<String>`, `moving: BTreeSet<String>`, `last_tick: Option<Instant>`, `posed_time: Option<f64>`.
  - `pub const TICK: Duration = Duration::from_secs_f64(1.0 / 60.0);` — upstream's timer interval (ModelViewer.swift:236); the elapsed math makes the interval a smoothness knob, not a correctness one.
  - `fn reload_motion(&mut self)` — ProjectOutput.swift:122-136: read `output/animation.json`; missing → `(study: None, error: None)`; parse error → `(None, Some(substituted fl!("animation-json-non-valido") + the engine's error text))`; parsed and `is_active()` → store + refresh `moving = study.moving_parts()`, clear the error; parsed single-frame → `(None, None)` (upstream's `frames.count > 1` drop). If the study ended up `None` while `tools.animating` → `set_animating(false)` + `apply_poses()` (ModelPane.swift:69-73).
  - `Reload(ReloadKind::Motion)` arm becomes `self.reload_motion(); if self.tools.animating { self.apply_poses(); }` (a CHANGED study keeps playing — upstream has no stop on change, only on disappearance).
  - `fn apply_poses(&mut self)` — THE one pose writer during playback: calls `apply_arrangement()` (M6 T10's; `effective_layout` is assembled while animating, so it writes identity/hidden-only poses and keeps the grid stamp frozen), then, when `tools.animating` and the study is present: `let (poses, appearances) = playback::poses(&self.scene.scene, &study.state_at(self.tools.player.time), &self.moving, &self.hidden_members());` → `self.scene.poses = poses.into(); self.scene.appearances = appearances.into();` — otherwise resets `scene.appearances` to opaque defaults. `hidden_members()` = M6 T10's group→member expansion over `tools.hidden` (upstream `hiddenMembers`, ModelPane.swift:116-118); if T10 named it differently, use that.
  - Update arms (all parity-pinned):
    - `ToggleAnimation` → `tools.set_animating(!tools.animating); last_tick = None; apply_poses();` (no re-frame — upstream's didSet bumps no cameraID).
    - `TogglePlayback` → gated on `tools.animating && study.is_some()`: `tools.player.toggle_playback(study.duration()); last_tick = None;` (the subscription's presence follows `is_playing`).
    - `Scrub(t)` → gated on animating: `player.time = t.min(study.duration()); player.is_playing = false; apply_poses();` (scrub pauses, never auto-resumes; the clamp keeps a dragged thumb inside the study; drag-start is not observable in iced's slider — pausing on every scrub tick reaches the same state, a documented micro-divergence for keyboard-driven scrub).
    - `SetSpeed(s)` / `ToggleLoops` → plain field writes (no re-pose — the next advance uses them).
    - `Tick(now)` → `if !animating || !player.is_playing || study inactive { last_tick = None; return; }`; `let Some(last) = last_tick.replace(now) else { return; }` (FIRST tick baselines); `player.advance(now.duration_since(last).as_secs_f64(), duration);` then `if posed_time != Some(player.time) { posed_time = Some(player.time); apply_poses(); }` (upstream's posedTime gate, ModelViewer.swift:253-255).
    - `RequestMotionStudy` → no-op here; the SHELL intercepts (T8).
  - `reload_outputs()`'s scene-swap branch gains `if self.tools.animating { self.apply_poses(); }` after `replace_scene` (a new model re-poses immediately; markers already cleared by T11's `mesh_changed`) AND the parts-drop stop: `if self.model_parts.len() <= 1 { self.tools.set_animating(false); self.apply_poses(); }` — upstream's `motionStudy` is `parts.count > 1`-gated (ModelPane.swift:13-15) and its disappearance onChange (:69-73) stops animating when a model swap drops to a single part while animation.json persists; without this the study stays `Some`, `wants_tick` keeps returning true, and a lone static part even renders translucent (not in `moving`).
  - `subscription()` gains, batched with the relay and keyboard: `(self.wants_tick()).then(|| cosmic::iced::time::every(TICK).map(Message::Tick))` where `pub fn wants_tick(&self) -> bool { self.tools.animating && self.tools.player.is_playing && self.study.as_ref().is_some_and(|s| s.is_active()) }` (upstream's timer idles while paused — the posedTime gate makes the paused timer a no-op, so gating the subscription on playing is behavior-identical and doesn't wake the runtime 60×/s for nothing). The existing `keyboard::listen` filter gains: `KeyPressed { key: Named(Space), modifiers }` with NO modifiers and `tools.animating` → `Some(Message::TogglePlayback)` (MotionControls.swift:19). DIVERGENCE, recorded in the module docs: SwiftUI button shortcuts do not fire while a text field has focus; iced's global listener cannot see focus, so Space typed into the chat draft toggles playback too — revisit at M8's command system.
  - `localize::substitute_placeholders(template: &str, values: &[&str]) -> String` (only if M6 T9 landed no equivalent — grep `app/src` for an existing `%lld` substitution first and reuse it): replaces the first `values.len()` occurrences of `%lld` or `%@`, leftmost first, leaving later markers.

- [ ] **Step 1: Write the failing tests** — pane in-crate (the M6 T10 helper's exact call shape; adjust to its landed signature):

```rust
    fn load_golden_animation(root: &std::path::Path) {
        // the pane test module's fixture helper (M6 T10's landed form)
        std::fs::copy(fixture("golden/animation.json"), root.join("output/animation.json"))
            .unwrap();
    }

    fn write_animation(root: &std::path::Path, json: &[u8]) {
        std::fs::write(root.join("output/animation.json"), json).unwrap();
    }

    #[test]
    fn motion_reload_parses_activates_and_reports_errors() {
        let (root, mut pane) = pane("motion-load", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        assert!(pane.study.is_some());
        assert!(pane.motion_error.is_none());
        assert_eq!(pane.moving.iter().next().map(String::as_str), Some("lid"));
        // single-frame study is inert (upstream drops it at load)
        write_animation(&root, br#"{"frames":[{"t":0}]}"#);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        assert!(pane.study.is_none() && pane.motion_error.is_none());
        // malformed: the study clears and the error surfaces, substituted
        write_animation(&root, b"{ not json");
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        assert!(pane.study.is_none());
        let error = pane.motion_error.as_deref().unwrap();
        assert!(error.starts_with("animation.json"), "substituted fl! text: {error}");
        assert!(!error.contains("%@"), "placeholder substituted");
    }

    #[test]
    fn study_disappearing_mid_playback_stops_cleanly() {
        let (root, mut pane) = pane("motion-stop", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleAnimation);
        assert!(pane.tools.animating && pane.tools.player.is_playing);
        assert!(pane.wants_tick());
        std::fs::remove_file(root.join("output/animation.json")).unwrap();
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        assert!(!pane.tools.animating, "disappearance stops playback");
        assert!(!pane.wants_tick());
        assert!(pane.scene.poses.iter().all(|p| p
            .map_or(false, |m| m == glam::Mat4::IDENTITY.to_cols_array_2d())),
            "poses revert to assembled");
        assert!(pane.scene.appearances.iter().all(|a| *a == Default::default()));
    }

    #[test]
    fn study_swap_keeps_playing() {
        let (root, mut pane) = pane("motion-swap", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleAnimation);
        // a DIFFERENT two-frame study (lid to MODEL z 20): keeps playing
        write_animation(&root, br#"{"frames":[
            {"t":0,"parts":{"lid":{"translate":[0,0,0]}}},
            {"t":1,"parts":{"lid":{"translate":[0,0,20]}}}]}"#);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        assert!(pane.tools.animating, "a changed study never stops playback");
        assert!(pane.study.as_ref().unwrap().duration() == 1.0);
    }

    #[test]
    fn parts_drop_to_one_stops_playback() {
        // study_swap's sibling: the STUDY persists but the MODEL drops to a
        // single part — upstream re-gates motionStudy on parts.count > 1 and
        // its disappearance stops animating (ModelPane.swift:13-15, :69-73)
        let (root, mut pane) = pane("motion-drop", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleAnimation);
        assert!(pane.tools.animating && pane.wants_tick());
        // swap to the single-part STL output (load_model_parts' fallback)
        std::fs::remove_file(root.join("output/model.3mf")).unwrap();
        std::fs::copy(fixture("golden/model.stl"), root.join("output/model.stl")).unwrap();
        let _ = pane.update(Message::Reload(ReloadKind::Mesh));
        assert!(!pane.tools.animating, "a <=1-part model stops playback");
        assert!(!pane.wants_tick());
        assert!(
            pane.scene.appearances.iter().all(|a| *a == Default::default()),
            "appearances reset with the poses"
        );
    }

    #[test]
    fn tick_advances_and_poses_the_study() {
        let (root, mut pane) = pane("motion-tick", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleAnimation);
        let t0 = std::time::Instant::now();
        let _ = pane.update(Message::Tick(t0)); // baseline
        assert_eq!(pane.tools.player.time, 0.0);
        let _ = pane.update(Message::Tick(t0 + Duration::from_millis(500)));
        assert!((pane.tools.player.time - 0.5).abs() < 0.05, "elapsed-driven: {}", pane.tools.player.time);
        // golden at t=0.5: lid at MODEL z 5 -> scene +y 5; base translucent
        let lid = pane.scene.poses[1].expect("lid posed");
        let m = glam::Mat4::from_cols_array_2d(&lid);
        assert!((m.y_axis.w - 5.0).abs() < 1e-3, "scene +y 5: {}", m.y_axis.w);
        assert!(pane.scene.appearances[0].translucent);
        // scrub to the colliding frame: pauses + flags both members
        let _ = pane.update(Message::Scrub(1.0));
        assert!(!pane.tools.player.is_playing, "scrub pauses, no auto-resume");
        assert!(pane.scene.appearances[0].collision
            && pane.scene.appearances[1].collision);
        assert!(!pane.wants_tick());
    }

    #[test]
    fn first_tick_after_resume_baselines_instead_of_jumping() {
        let (root, mut pane) = pane("motion-resume", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleAnimation);
        let t0 = std::time::Instant::now();
        let _ = pane.update(Message::Tick(t0));
        let _ = pane.update(Message::Tick(t0 + Duration::from_millis(250)));
        let mid = pane.tools.player.time;
        // pause across a LONG gap, then resume: the first tick must baseline
        let _ = pane.update(Message::TogglePlayback);
        let later = t0 + Duration::from_secs(30);
        let _ = pane.update(Message::TogglePlayback);
        let _ = pane.update(Message::Tick(later));
        assert!((pane.tools.player.time - mid).abs() < 1e-9,
            "no teleport across the pause: {} vs {mid}", pane.tools.player.time);
        let _ = pane.update(Message::Tick(later + Duration::from_millis(100)));
        assert!((pane.tools.player.time - mid - 0.1).abs() < 0.02);
    }

    #[test]
    fn non_loop_end_stops_and_replay_restarts() {
        let (root, mut pane) = pane("motion-end", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleLoops);
        let _ = pane.update(Message::ToggleAnimation);
        let t0 = std::time::Instant::now();
        let _ = pane.update(Message::Tick(t0));
        let _ = pane.update(Message::Tick(t0 + Duration::from_secs(3)));
        assert!((pane.tools.player.time - 2.0).abs() < 1e-9, "clamped at duration");
        assert!(!pane.tools.player.is_playing && !pane.wants_tick());
        let _ = pane.update(Message::TogglePlayback);
        assert_eq!(pane.tools.player.time, 0.0, "replay restarts from 0");
        assert!(pane.tools.player.is_playing);
    }

    #[test]
    fn hidden_group_stays_hidden_during_playback() {
        let (root, mut pane) = pane("motion-hidden", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleHiddenGroup("lid".into()));
        let _ = pane.update(Message::ToggleAnimation);
        let _ = pane.update(Message::Scrub(1.0));
        assert!(pane.scene.poses[1].is_none(),
            "hidden member never draws — even colliding and keyed by the study");
        assert!(pane.scene.poses[0].is_some());
    }

    #[test]
    fn scrub_clamps_past_duration() {
        let (root, mut pane) = pane("motion-clamp", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleAnimation);
        let _ = pane.update(Message::Scrub(999.0));
        assert!((pane.tools.player.time - 2.0).abs() < 1e-9);
    }
```

- [ ] **Step 2: RED** — `cargo test -p app --lib motion_reload` → compile errors (no `study` field / message variants).
- [ ] **Step 3: Implement** — per the Interfaces block. The pane's module docs gain the M7 paragraph (clock, hidden interplay, the Space-focus divergence).
- [ ] **Step 4: GREEN** — `cargo test -p app` (whole crate — M4/M5/M6 pane tests must stay green).
- [ ] **Step 5: Commit** — `git add app/src/viewer_pane.rs app/src/localize.rs && git commit -m "feat(app): pane motion model — study reload, elapsed playback clock, playback pose application"`

### Task 7: pane view — the status-bar animation slot, MotionControls mount, disabled gates

**Gates:** T5, T6 merged; M6 T9 merged (the status bar and overlay mounts this task extends).

**Files:**
- Modify: `app/src/viewer_pane.rs`
- Test: pane in-crate tests

**Interfaces:**
- Consumes: T5's `view_motion_controls`; T4's `motion_available`; the shell's `OverlayContext.busy` (parameters_view.rs:123).
- Produces (T8's window reads nothing new here; the user-facing surface):
  - `view_model_3d`'s overlay stack: the BOTTOM-center slot becomes upstream's either/or (ModelPane.swift:39-45) — `tools.animating && study present` mounts `view_motion_controls(&tools.player, study)` (mapped through the pane's Message); ELSE `tools.sectioning && model` mounts M6 T9's section controls (unchanged).
  - `view_status_bar(&self, busy: bool)` (thread `busy` from `view_model_3d`'s `OverlayContext`): the slot M6 T9 reserved between the section toggle and the legend (its in-code comment names M7 — replace the comment with the code): when `tools.motion_available(self.model_parts.len(), self.study.is_some())` → a selected-state toggle button (`Animazione` via `fl!("animazione")`, Suggested when animating — the preset_picker pattern, on_press `ToggleAnimation`, description `fl!("riproduci-l-animazione-con-la-simulazione-fisica")`); ELSE when `self.model_parts.len() > 1` → the create-animation icon button (`media-playback-start`, on_press `RequestMotionStudy` ONLY when `!busy` — an iced button without on_press is the disabled state; description `fl!("crea-animazione")`, help-class text `fl!("chiedi-all-agente-di-simulare-e-animare-il-meccanismo")`).
  - The measure and section toggle buttons gain `.disabled(tools.animating)` styling — build without `on_press` while animating (upstream ModelPane.swift:133-135). The parts panel's layout picker (M6 T9's `view_parts_panel`) disabled state grows `|| tools.animating` (PartsOverlay.swift:23).
  - The play/pause/speed/loop/scrub interactions arrive through T6's messages; this task only mounts.

- [ ] **Step 1: Write the failing tests** — pane in-crate:

```rust
    #[test]
    fn status_bar_shows_the_animation_toggle_only_with_a_study() {
        let (_root, pane) = pane("slot-study", &[], &["golden/model.3mf"]);
        // no animation.json: two parts -> the ask button, not the toggle
        assert!(!pane.tools.motion_available(2, pane.study.is_some()));
        // view construction must not panic in either state (pure build)
        let _ = pane.view_model_3d(None);
        let (root2, mut pane2) = pane("slot-study2", &[], &["golden/model.3mf"]);
        load_golden_animation(&root2);
        let _ = pane2.update(Message::Reload(ReloadKind::Motion));
        assert!(pane2.tools.motion_available(2, pane2.study.is_some()));
        let _ = pane2.update(Message::ToggleAnimation);
        let _ = pane2.view_model_3d(None); // animating: motion controls mounted
    }

    #[test]
    fn animating_mounts_motion_controls_over_section_controls() {
        let (root, mut pane) = pane("slot-mount", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleSection);
        let _ = pane.update(Message::ToggleAnimation);
        // upstream either/or: animating wins the bottom slot and sectioning
        // was cleared by set_animating — the pane builds cleanly either way
        assert!(!pane.tools.sectioning);
        let _ = pane.view_model_3d(None);
        let _ = pane.update(Message::ToggleAnimation);
        assert!(!pane.tools.animating);
        let _ = pane.view_model_3d(None);
    }

    #[test]
    fn section_and_measure_toggle_states_track_animating() {
        let (root, mut pane) = pane("slot-gates", &[], &["golden/model.3mf"]);
        load_golden_animation(&root);
        let _ = pane.update(Message::Reload(ReloadKind::Motion));
        let _ = pane.update(Message::ToggleAnimation);
        // the disabled state is a view-time function of tools.animating; the
        // model-side contract it renders: animating clears and holds them off
        assert!(!pane.tools.sectioning && !pane.tools.measuring);
        let _ = pane.update(Message::ToggleAnimation);
        assert!(!pane.tools.animating);
        let _ = pane.update(Message::ToggleSection); // re-enableable once off
        assert!(pane.tools.sectioning);
    }
```

- [ ] **Step 2: RED** — `cargo test -p app --lib status_bar_shows_the_animation_toggle` → compile error (`view_status_bar` arity).
- [ ] **Step 3: Implement** — per the Interfaces block; icons are `cosmic::widget::icon::from_name(...)` (`media-playback-pause` and `dialog-warning` have in-tree precedents — chat_view.rs's playback icons, project.rs's warning icon; `media-playback-start` has NO in-tree precedent, only pause/stop — but it is a standard freedesktop name, fine to use — the M6 text-button rule applied to glyphs with NO good icon name; these have standard ones).
- [ ] **Step 4: GREEN** — `cargo test -p app`.
- [ ] **Step 5: Commit** — `git add app/src/viewer_pane.rs && git commit -m "feat(app): status-bar animation slot, motion controls mount, animating disabled gates"`

### Task 8: window + issues — the ask-the-agent intercept, motion errors and collision warnings

**Gates:** T6 merged (pane accessors); M6 T12 merged (the window's pane-forwarding seams this task extends).

**Files:**
- Modify: `app/src/windows/project.rs`, `app/src/activity.rs`, `app/src/viewer_pane.rs` (the granted ⌥⌘I move: the app-wide arm lands AND the pane-local subscription arm is REMOVED in the same edit — left in place, both subscriptions see ⌥⌘I and the visibility toggles twice, a silent no-op)
- Test: `app/tests/activity_issues.rs` if it exists, else the in-crate `activity` tests (activity.rs's own `#[cfg(test)]`)

**Interfaces:**
- Consumes: T6's `Message::RequestMotionStudy`, `pane.motion_error`, `pane.study`; T1's `collision_count`.
- Produces: M7 closed on the issues surface —
  - `activity::issues(build_error: Option<&str>, missing_outputs: &[String], motion_error: Option<&str>, collision_frames: usize) -> Vec<Issue>` — the two upstream branches appended in order (Project.swift:123-127): motion error → one Warning carrying the already-substituted text; `collision_frames > 0` → one Warning `substitute_placeholders(fl!("l-animazione-ha-collisioni-tra-parti-in-lld-fotogrammi"), &[&collision_frames.to_string()])`.
  - `ViewerPane::pub fn motion_error(&self) -> Option<&str>` and `pub fn collision_frames(&self) -> usize` (the latter `self.study.as_ref().map_or(0, MotionStudy::collision_count)`).
  - `project.rs`'s `Message::Viewer(RequestMotionStudy)` intercept, beside the ParameterCommit one (:424-439): `if !self.is_busy() { let _ = self.bridge.enqueue(fl!("anima-il-movimento-di-questo-meccanismo-con-una-simulazione-fisica"), Vec::new()); }` — the button is disabled while busy anyway; the guard is the belt (upstream `.disabled(project.isBusy)`, ModelPane.swift:144).
  - Both `activity::issues` call sites (`view_activity` :992, `view_issue_list` :1027) pass the pane-derived pair; the stale doc lines naming M6 (activity.rs:9-10 "join at M6", project.rs:944 "is M6") are corrected to the M7 reality — manifest warnings remain M8's (the ActivityViewer full port).

- [ ] **Step 1: Write the failing tests** — extend activity's tests:

```rust
    #[test]
    fn motion_issues_render_both_upstream_branches() {
        let issues = crate::activity::issues(
            None,
            &[],
            Some("animation.json is not valid: expected a comma"),
            0,
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::Warning);
        assert_eq!(
            issues[0].message,
            "animation.json is not valid: expected a comma"
        );
        let issues = crate::activity::issues(None, &[], None, 2);
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].message.contains("2 frames"),
            "count substituted into the fl! text: {}",
            issues[0].message
        );
        assert!(!issues[0].message.contains("%lld"), "placeholder gone");
        // zero-collision and no-error studies stay silent
        assert!(crate::activity::issues(None, &[], None, 0).is_empty());
    }
```

- [ ] **Step 2: RED** — `cargo test -p app --lib motion_issues` → compile error (arity).
- [ ] **Step 3: Implement** — per the Interfaces block; fix the two view call sites, the existing `issues` tests in activity.rs's `#[cfg(test)]` (mechanical `None, 0` tail), and the two doc lines in the same edit.
- [ ] **Step 4: GREEN** — `cargo test -p app`.
- [ ] **Step 5: Commit** — `git add app/src/windows/project.rs app/src/activity.rs app/src/viewer_pane.rs && git commit -m "feat(app): motion issues in the header and the create-animation chat intercept"`

### Task 9: M7 wrap — module docs, carry-forwards, thelio gate, shots

**Gates:** T1-T8 all merged.

**Files:**
- Modify: `viewer/src/lib.rs` + `app/src/lib.rs` (module docs: playback.rs, motion_controls.rs), `docs/research/carry-forwards.md` (new M7 section recording the accepted divergences), `viewer/examples/` (one shots example — the established M0/M6 pattern)
- Test: no new tests — the full-suite gate

**Interfaces:**
- Consumes: everything above.
- Produces: M7 closed. Carry-forwards' new M7 section records: (1) the Space-focus divergence (T6's module doc has the detail; M8's command system owns the revisit); (2) keyboard-driven scrub pauses (drag-start is unobservable in iced's slider); (3) the speed menu's dropdown+button split vs upstream's nested menu (T5); (4) translucent parts composite in slot order (no sort among translucents, depth-write off) where upstream sorts back-to-front — cosmetic, and this task's shots are the verdict venue. Shots for Preston's eyeball verdict: the golden study at t=0.5 (lid raised, base translucent), at t=1.0 (both parts collision red, the warning row), and paused-scrubbed mid-frame — one example program or test-gated dump reusing `render_headless`'s harness, run on thelio.

- [ ] **Step 1: Module docs** — one paragraph per crate: viewer gains the playback paragraph (MotionPlayer, the remap law, appearance flags); app gains the motion paragraph (clock, mutual exclusion, issues).
- [ ] **Step 2: Carry-forwards** — append the M7 section with the four recorded divergences above.
- [ ] **Step 3: thelio gate** — controller runs `ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && cargo test --workspace --all-targets"'` once, HEAD-verified. Fix-forward tasks file under `.superpowers/sdd/` per house process if anything flakes.
- [ ] **Step 4: Shots for Preston** — offscreen renders of the three playback states, shown inline for the eyeball verdict (the M0/M6 carry-forward pattern).
- [ ] **Step 5: Commit** — `git add viewer/src/lib.rs app/src/lib.rs docs/research/carry-forwards.md && git commit -m "docs: M7 animation playback complete"`

---

**Execution note (dispatch waves, ≤5 concurrent, controller merges serially):** Wave 1: T1 (engine — immediate, no M6 dependency). T2 waits for M6 T6+T7 merges (same files), then dispatches. Wave 2: T3 after T1+T2. Wave 3: T4 after T3 + M6 T8. Wave 4: T6 after T2 + T4 + M6 T10+T11 (T2 is long merged by this wave in practice; T6's gate names it because its Interfaces write a T2 product). Wave 5: T5 ∥ T8 after T6 (disjoint files — parallel, staggered reviews; T5 also needs M6 T9). Wave 6: T7 after T5 + M6 T9; T9 last. Every task: implementer runs focused local tests only; thelio gates are the controller's, single-invocation, HEAD-verified. Gates name the M6 tasks each dependency actually consumes — re-verify each is merged at dispatch time, never assume "M6 is done".
