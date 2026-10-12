# M3 viewer-parity facts

Sources: upstream macOS original at `/tmp/cad-studio-src/CADStudio/Viewer3D/*.swift`
(re-clone https://github.com/filippofinke/cad-studio if gone); our M0 spike
`viewer/src/render/{mod.rs,scene.rs,mesh.wgsl,cap.wgsl,blit.wgsl}` +
`viewer/examples/spike_window.rs`; engine `engine/src/parsing/{mod.rs,animation.rs}`.
Citations `Swift:line` mean the file in `/tmp/cad-studio-src/CADStudio/Viewer3D/`;
`repo:line` mean paths under `/Users/pbrown/code/cad-studio-cosmic/`.

Frames: **MODEL** = engine/upstream mesh coords, Z-up. **SCENE** = render coords,
Y-up. Remap `sceneVector(v) = (v.x, v.z, -v.y)` (ModelViewer.swift:493-495), inverse
`modelVector(v) = (v.x, -v.z, v.y)` (:497-499) — a proper rotation (det +1), so
normals and quats rotate through it unchanged in direction.

## 1. Camera parity

- FOV 35 deg, near 1, far 100_000 — absolute, not radius-relative (ModelViewer.swift:142-144). Spike: same (mod.rs:151,160).
- Orbit: `yaw -= dx * 0.008`; `pitch = clamp(pitch + dy * 0.008, -1.5607, +1.5607)` (ModelViewer.swift:95,301-305). dy sign: AppKit deltas, drag up increases pitch.
- Zoom: `distance = clamp(distance * factor, radius*0.1, radius*200)` (ModelViewer.swift:307-310).
- Scroll: `steps = hasPreciseDeltas ? deltaY/12 : deltaY`; `zoom(pow(0.9, steps))`; shift/option+scroll pans instead (CameraInputView.swift:63-71).
- Pinch (magnify): `zoom(1 / max(1 + magnification, 0.1))` (CameraInputView.swift:73-75).
- Pan: `unitsPerPoint = 2 * distance * tan(fov/2) / viewHeight`; `target += (-right*dx + up*dy) * unitsPerPoint` where right/up are the camera's scene axes (ModelViewer.swift:312-320). Right-drag and middle-drag pan; option/shift+drag pans (CameraInputView.swift:43-47,55-61).
- Frame: `radius = max(length(size)/2, 1)` — size is the SCENE size = MODEL diagonal; `target = (0, size.y/2, 0)`; `distance = radius / sin(fov/2) * 1.15` (ModelViewer.swift:328-337). Note: **diagonal/2**, not max-extent/2 — spike uses max-extent (mod.rs:146-150), different for long-thin parts.
- placeCamera: `dir = (cos(pitch)sin(yaw), sin(pitch), cos(pitch)cos(yaw))`; `look(at: target, from: target + dir*distance, up: (0,1,0))` (ModelViewer.swift:339-342). Spike eye formula identical (mod.rs:154-157) but center = aabb center; upstream pins MODEL floor to scene y=0 via `content.position = (-center.x, -minimum.z, -center.z)` (ModelViewer.swift:167-168) and targets half-height.
- Presets (ModelViewer.swift:19-33): iso (yaw pi/4, pitch 0.6), front (0, 0), top (0, 1.5607), right (pi/2, 0). Preset change bumps `cameraID` -> reframe (ModelViewerState.swift:58-61, ModelViewer.swift:322-326).
- Re-frame on mesh change: only when `|diag(new) - diag(prev)| / prev_diag > 0.5` (ModelViewerState.swift:127-135); also clears measurePoints (:128). Measured on the assembled `output.mesh`, not the arranged one (ModelPane.swift:64-68).
- Click vs drag: accumulate `|dx|+|dy|`; click only if total < 4 px (CameraInputView.swift:37,40-52).
- Lights: key directional 2600 from (60,120,90), fill 900 from (-90,40,-60), both scene coords (ModelViewer.swift:344-353). Spike approximates in-shader: `0.26*max(dot(n,l1),0) + 0.09*max(dot(n,l2),0) + 0.15`, clamp 0..1.2, l1/l2 normalized upstream positions (mesh.wgsl:37-43).
- Pick ray (for measure): `tangent = tan(fov*pi/360)`; `x = (2*px/w - 1)*tangent*aspect`; `y = (2*py/h - 1)*tangent` — AppKit y is bottom-up, **iced cursor y is top-down, flip required**; `dir = normalize(camera.convert((x,y,-1)))`; `origin = cameraPos - content.position`; intersect in MODEL space via `modelVector` (ModelViewer.swift:291-299). Picks against the displayed (sectioned) mesh (:80-83).

## 2. Grid + bed outline

- Rebuilt when mesh changes (ModelViewer.swift:164-170). `footprint = max(size.x, size.y)` MODEL; `half = max(100, (Int(footprint*0.75/10)+1)*10)` (ModelViewer.swift:427-428).
- Lines every 10 units from -half to +half; major when `Int(offset.rounded()) % 100 == 0`; widths major 0.5 / minor 0.25; alpha 0.10 minor / 0.28 major, white in dark mode / black in light (ModelViewer.swift:429-445). Lines are thin quads (2 tris) built by `quad(from:to:width:)` with side = cross(along, (0,1,0)) * width/2 (:458-462); unlit transparent, no culling (:464-473).
- Grid sits at scene y = -0.05 (just below MODEL floor which content pins at 0) (ModelViewer.swift:446). Grid is a child of root, not content (:139-140) — not affected by content offset.
- Axis tripod (ModelViewer.swift:449-455): corner `(-fpX/2 - margin, 0.05, +|fpZ|/2 + margin)` SCENE, `margin = max(max(sx,sy)*0.15, 3)`, `axisLength = max(max(sx,sy,sz)*0.3, 5)`; directions scene (1,0,0) red, (0,0,-1) green, (0,1,0) blue = MODEL +X/+Y/+Z. Boxes, `thickness = max(length/40, 0.6)` (:475-481).
- Bed rectangle (ModelViewer.swift:187-199): from manifest bed SIMD2 (x,y mm); rectangle at scene y = 0.06 **inside content** (0.06 above MODEL floor); corners (±x/2, 0.06, ±y/2); line width `max(x,y)/250`; color = control accent color. Only passed when effective layout == plate (ModelPane.swift:96).
- Grid toggle: `grid.isEnabled = showsGrid` (ModelViewer.swift:184); default on (ModelViewerState.swift:22).

## 3. Multi-part rendering

- Materials list = [neutral] + one per distinct color; face material index 0 = neutral, i+1 = colors[i] (TriangleMesh.swift:44-50, ModelViewer.swift:400-406). Uncolored triangles -> neutral.
- Neutral: dark mode (0.62, 0.70, 0.80), light (0.55, 0.64, 0.75) (ModelViewer.swift:401-403). Spike hardcodes the dark one (mod.rs:112).
- Surface material: metallic 0.15, roughness 0.55, back-face culling; wireframe mode = same materials with `triangleFillMode = .lines` + culling none (ModelViewer.swift:415-423). Wireframe is a material/pipeline mode, edges-only, both faces drawn.
- Interior shell: same mesh, unlit red `systemRed blended 15% black`, `faceCulling = .front` (draws back faces) — shows as the cut face where sectioning clipped the shell; disabled in wireframe (ModelViewer.swift:355-367,363-365).
- Playback transparency: static (non-moving) parts get `blending transparent(0.22)`, culling none (ModelViewer.swift:282-284,408-413); moving parts stay opaque.
- Collision material: PBR baseColor systemRed + emissive systemRed intensity 0.6, no culling (ModelViewer.swift:275-279); swapped per part when colliding set membership changes (:264-267).
- 12-swatch legend: first 12 distinct colors of the assembled mesh, 10 px circles, a11y label "N colori" = total count (ModelPane.swift:202-213).
- Hidden parts: hidden groups dropped from arranged mesh (PartArrangement.swift:69-91) or entity disabled in animation mode (ModelViewer.swift:221-223).
- Per-part colors originate from 3MF basematerials/colorgroup, per-triangle `pid/p1` overriding object `pid/pindex` (ThreeMFParser.swift:113-128); RGBA hex 6/8 (ModelViewer.swift n/a — ThreeMFParser.swift:202-212).

## 4. Explode / plate / arrangement

- Group name: strip suffix regex `[_\-\s]?\d+$` (optional `_`/`-`/space then trailing digits); empty result falls back to full name (PartArrangement.swift:35-38). A base is treated as a group only when >1 member shares it (:40-54); group color = first color of first member (:50).
- Arrangement cache keyed by `sceneID-layout-plateID-explode-hidden(sorted)-attached(sorted)` (ModelViewerState.swift:88-95).
- effectiveLayout: animating or compare -> assembled; exploded with <2 parts -> assembled (ModelViewerState.swift:78-86). Layout picker disabled while animating/comparing (PartsOverlay.swift:23); available layouts drop exploded when parts.count < 2 (:53-55).
- Exploded offsets (PartArrangement.swift:95-140):
  - Bounds per group over all parts with triangles (hidden groups included in collision logic, excluded from drawing).
  - Anchor = group with max bbox volume (:112). `overall` = union of boxes; `gap = max(length(overallMax-overallMin) * 0.06, 2)` (:116-117).
  - Others sorted ascending by center distance to anchor center (:120-122).
  - Attached groups: placed as-is, no push (:125-128).
  - `direction = dominantAxis(groupCenter - anchorCenter)`; z-priority: if `|z| >= |x| && |z| >= |y|` -> ±Z; else X if `|x| >= |y|`; else ±Y; degenerate (<0.01 all) -> +Z (:153-163). MODEL axes, Z = up.
  - `step = max(length(boxDiagonal) * 0.05, 0.5)`; march `travel += step` until no overlap with placed boxes; overlap test inflates the other box by `margin*0.5` on each side (:130-134,165-174).
  - `offset = direction * (travel + gap) * explodeAmount` (:135-137).
- Floor lift after explode: shift whole mesh by `(0,0, assembledMinZ - explodedMinZ)` MODEL so the exploded pile sits on the assembled floor (PartArrangement.swift:77-85).
- Explode slider 0...2, default 1.0 (PartsOverlay.swift:28, ModelViewerState.swift:46).
- Plate layout: agent-provided `plateParts` when present, else `flatLayout` fallback (PartArrangement.swift:86-91); UI explains the fallback (PartsOverlay.swift:37-42).
- flatLayout fallback (:176-203): gap 5; sort desc by MODEL size.y; `area = sum((sx+gap)*(sy+gap))`; `rowWidth = max(sqrt(area)*1.3, maxX)`; new row when `cursor.x > 0 && cursor.x + sx > rowWidth` -> `(0, y + rowDepth + gap)`; each part translated so its minimum sits at the cursor; final shift `(-maxX/2, -maxY/2, 0)` centers.
- Layout change persists to UserDefaults, clears measurePoints, bumps cameraID (reframe) (ModelViewerState.swift:39-45).
- Hide/attach/isolate act on group names; isolate = double-click row, toggles "all others hidden" (PartsOverlay.swift:145-148); hiddenParts expanded to member names for the animation path (ModelPane.swift:116-118).

## 5. Measure

- Toggle clears points (ModelViewerState.swift:51-53); disabled while animating (ModelPane.swift:132-133); Esc clears (:54-58); cleared on mesh change (ModelViewerState.swift:128) and mesh swap in scene (ModelViewer.swift:180-183).
- Points: 2 max — third click restarts (ModelViewerState.swift:67-72).
- Pick: CPU ray, Moller-Trumbore over every triangle of the displayed (sectioned, arranged) mesh, both-sided (|det| > 1e-9), nearest t > 0 (TriangleMesh.swift:84-106; ray construction see section 1).
- Markers (ModelViewer.swift:381-398): unlit orange spheres, `radius = max(modelRadius * 0.03, 0.4)`; with 2 points a connecting box, cross-section `radius*0.5`, length = distance, centered midpoint, oriented by quat from (0,0,1) to normalized delta — all SCENE coords (points mapped through sceneVector).
- Badge (ModelPane.swift:163-179): `total · dX · dY · dZ`, deltas = abs(component) of MODEL-space difference (measurePoints are MODEL coords — dZ is height); each formatted via display unit (`mm / unit`, unit fraction digits, unit symbol on every term). Prompts: "click a point" / "click second point".

## 6. Section

- State: axis x/y/z (default x), position 0.5 (ModelViewerState.swift:24-26); slider range 0.001...0.999 (ModelPane.swift:191).
- Plane: `value = min[axis] + (max[axis] - min[axis]) * position` on the **arranged** mesh aabb (ModelViewerState.swift:116-121).
- Keep side: `keepsBelow = axis != .y` — X and Z keep the below side, **Y keeps the above side** (ModelViewerState.swift:120, TriangleMesh.swift:59-82 sign trick).
- Clip = Sutherland-Hodgman per triangle, fan-triangulated, colors preserved (TriangleMesh.swift:59-82). Upstream clips CPU-side and re-renders; cut face shown by the red interior shell (section 3).
- Mutual exclusion: starting animation clears sectioning+measuring (ModelViewerState.swift:28-36); section toggle disabled while animating (ModelPane.swift:134-135).
- Spike equivalent: shader discard `v > section_pos` (keep-below on every axis — misses the Y-above rule) + stencil winding-parity cap quad, red (0.92, 0.15, 0.10), aabb-grown 10% (mesh.wgsl:31-36, cap.wgsl:17-20, mod.rs:62-88,328-356).

## 7. Playback wiring

- Timer at 1/60 s, real deltaTime (CACurrentMediaTime deltas) (ModelViewer.swift:233-246); pose applied only when player.time changed (:253-255); timer stopped on view disappear (:85-87) and when study/parts absent (:201-210).
- Player: `time`, `isPlaying`, `speed`, `loops`; `advance(by:duration:)`: `next = time + dt*speed`; `next < duration` -> time = next; loops -> `next % duration`; else clamp to duration and stop (MotionPlayer.swift:11-22). Toggle restarts from 0 when at end (:24-29).
- Speeds: fixed menu [0.1, 0.25, 0.5, 1, 2] (MotionControls.swift:7); loop toggle in same menu. Space = play/pause shortcut (:19).
- Scrub slider 0...max(duration, 0.001); starting a scrub pauses playback (does not auto-resume); readout "0.00 / D s", 2 fraction digits (MotionControls.swift:21-31).
- Poses: `entity.position = sceneVector(translate)`, `orientation = quat(sceneVector(rotate.imag), rotate.real)` per part; missing pose = identity (ModelViewer.swift:258-269). Parts are separate entities — animation moves transforms, never re-uploads geometry.
- Animation forces assembled layout (ModelViewerState.swift:78-81) and swaps the single-mesh path for per-part entities (ModelViewer.swift:201-226); hidden groups disabled, static parts translucent 0.22 (section 3).
- Controls row (MotionControls.swift:51-70): sticky phase label (bold), values `name value` with 1...3 significant digits, collision warning red icon + text `A <-> B 1.2 mm³` (volume 1...2 sig digits, joined ", ") (:81-89).
- Engine parity already landed: `state_at` (nearest-frame collisions, tie at fraction 0.5 goes later; label = last labeled frame <= t; values/poses interpolate with both-way fallback) (animation.rs:172-232); `duration` = last frame t (:158-160); `is_active` = >1 frame (:163-166).
- **Display order**: `StudyState.values` is a BTreeMap — iterating it alphabetizes; upstream order (first frame appearance, then sorted within frame) comes only from `value_names()` (animation.rs:134-139,168-170; upstream MotionStudy.swift:93-99,118).
- Motion controls exist only when parts.count > 1 (ModelPane.swift:13-15,136-137); study disappearing stops animation (:69-73).

## 8. Compare (M6) — viewer hooks to keep in M3

- Ghost = old-version mesh: unlit systemOrange, transparent 0.6, culling none, fill mode .lines (orange wireframe ghost) (ModelViewer.swift:369-379).
- Ghost passed as `ghost: TriangleMesh?` + `ghostID: UUID?`; rebuilt when ghostID or mesh changes (ModelViewer.swift:176-179); compare forces assembled layout (ModelViewerState.swift:79); UI note "orange = chosen version's shape" (ParametersOverlay.swift:100-105).

## 9. Spike -> product gap list

What the M0 spike already implements (function names):
- `orbit_camera(mesh, yaw, pitch, zoom)` — eye/target/fov 35/near/far/distance formula (mod.rs:140-168); `default_camera` = iso preset (mod.rs:134-136).
- Viewer-owned Depth24PlusStencil8 pass, LoadOp::Load compositing, `MeshPipeline::{new, with_msaa, prepare, render, draw_scene}` (mod.rs:188-803); iced adapter `SpikeScene`/`SpikePrimitive` (scene.rs).
- Section via shader discard + stencil-parity red cap quad (mesh.wgsl, cap.wgsl, mod.rs:62-88).
- MSAA 4x path with resolve + premultiplied blit (mod.rs:542-595,711-761), headless-tested.
- Windowed example: drag orbit (sensitivity 0.01, pitch clamp 1.4), scroll zoom 0.9^steps with pixel divisor 40, zoom clamp [0.05, 50] (spike_window.rs:58-74) — all three deviate from upstream (0.008 / 1.5607 / divisor 12 / radius-relative clamps).

Missing vs the blocks above (M3 must build): pan + target, distance clamps [0.1r,200r], pinch, presets + reframe-on->50%-diagonal-change, MODEL->SCENE remap + floor pinning (spike renders MODEL coords as-if SCENE), grid/bed/tripod, per-part meshes + materials + wireframe mode + transparency, explode/plate arrangement (CPU), measure (ray pick + markers + badge), playback (timer, player, pose transforms, controls), ghost hooks, real normals (spike uses dpdx/dpdy screen-space, mesh.wgsl:41), aspect from viewport (hardcoded 16:9, mod.rs:160).

Carry-forward obligations (docs/research/carry-forwards.md:39-57, restated): upload-once (`Arc<TriMesh>` identity or dirty flag; today scene.rs:48 clones per frame and mod.rs:646-659 re-uploads all vertices); aspect from viewport; orientation-sensitive blit test; wire `with_msaa` into the windowed example Storage (1x/4x toggle); real normals upload; cull-None fragment-cost note; bounded poll timeout if headless flakes; `image` dev-dep `default-features = false, features = ["png"]`; iced must stay pinned to the vendored fork inside `pop-os/libcosmic.git` (type identity).

## 10. Part type mapping (engine -> viewer upload)

Engine type (engine/src/parsing/mod.rs:11-18): `Part { name: String, positions: Vec<[f32;3]> (per-corner, unindexed, 3/tri, MODEL coords), triangle_colors: Vec<Option<[u8;3]>> (1/tri) }`, `aabb()` :22-29. No normals stored.

Upstream reference: normals computed per-face at addTriangle time, `normalize(cross(b-a, c-a))` fallback (0,0,1) (TriangleMesh.swift:21-23); positions+normals remapped through sceneVector at resource build (ModelViewer.swift:483-491); one entity per part (ModelViewer.swift:280-288).

Conversion M3 needs (sketch):

```rust
// viewer/src/render (new)
pub struct PartMesh {
    pub name: String,
    pub vertices: Vec<Vertex>,            // scene-space (x, z, -y); normal
                                           // per-face rotated by same remap
    pub aabb_min: [f32; 3],               // scene-space, for framing/grid
    pub aabb_max: [f32; 3],
    pub first_vertex: u32,                // draw range into shared buffer
}
pub fn part_to_mesh(part: &Part, neutral: [f32; 3]) -> PartMesh;
// neutral = theme neutral (section 3); color = t.map(|c| c/255).unwrap_or(neutral)
```

- Per-triangle: 3 corners -> 3 `Vertex { pos: scene, normal: scene(cross), color }`; color constant across the face (flat), matching upstream per-face materials.
- One GPU buffer per part (upstream = one entity per part) so playback poses are per-part uniform transforms, matching upstream entity moves; alternatively one shared buffer with `first_vertex` draw ranges — either satisfies "part-index attribute for overlays" (hover/isolate) without a per-vertex attribute, but an explicit `u32 part_index` vertex attribute is the direct route if overlays need fragment-level identification.
- Keep MODEL-space CPU geometry around: measure picking and badge deltas, explode offsets, section keep-sides, and the floor lift are all MODEL-space upstream; remap only at upload/display.
