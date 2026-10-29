//! Part-keyed renderer: meshes via viewer-owned render pass with
//! Depth24PlusStencil8. iced's compositor render pass carries no
//! depth/stencil attachment, so everything the viewer draws happens in
//! passes this module owns: the surface color attachment is loaded
//! (`LoadOp::Load`, iced content underneath stays), depth and stencil are
//! cleared every frame, and the depth texture is resized with the
//! viewport. Pure wgpu — no iced types — so the headless test can reuse
//! this module as-is; the iced adapter lives in [`crate::render::scene`].
//!
//! GPU-first (spec §5): the mesh arrives as a [`parts::PartScene`] whose
//! parts carry full-content identities; [`MeshPipeline::prepare_parts`]
//! uploads a part's vertices only when that identity differs from what is
//! resident, so an unchanged scene costs zero mesh bytes however the
//! camera moves. What legitimately writes per frame: the camera uniform,
//! the section-cap quad, and each visible part's model uniform — the
//! [`Frame`] pose path moves and hides parts (draw-skip on `None`)
//! without touching a vertex, and M7's playback rides the same binding:
//! poses AND the collision/translucency appearance flags are uniform
//! state (80 B/part), never vertex re-staging.
//!
//! The camera is split the same way: [`camera`] holds the ModelViewer.swift
//! parity math — orbit/zoom/pan formulas, diagonal/2 framing with the floor
//! pin, view presets, pitch and zoom clamps — as pure functions (the
//! adapter owns the yaw/pitch/distance/target state and calls in), and
//! [`lights`] the two-light shading constants the mesh shader substitutes
//! and the headless tests twin.
//!
//! msaa is the settled spec §5 decision (iced's own-pass multisample
//! settings do not apply to a custom `render()` pass, so the mesh pass owns
//! its count and resolve target): `MeshPipeline::new` stays single-sample
//! (the default, byte-identical frames); `MeshPipeline::with_msaa`
//! (count 4) renders into an owned
//! multisample color + depth attachment, resolves into an intermediate
//! single-sample texture, and blits that over the target with premultiplied
//! alpha — iced's content underneath survives, same Load-and-composite
//! contract as the single-sample path. The app's `m`-key toggle flips
//! [`Options::msaa`] between 1 and 4; `prepare_parts` then rebuilds the
//! render pipelines and owned attachments in place (iced's Storage owns one
//! pipeline per type and cannot hand out replacements), while the wireframe
//! toggle stays a pure draw-path switch.
//!
//! Cross-sections: the mesh pass writes winding parity into the stencil
//! (front +1 / back -1 per rasterized fragment, wrap ops, section-discarded
//! fragments excluded), and a red cap quad drawn right after fills exactly
//! the cut face (stencil != 0) when `Camera::section_enable` is set.
//!
//! Modes and overlays (facts §2/#44): [`Options::wireframe`] swaps the
//! part pass to the 1px hairline edge list — same material, LineList
//! topology; the section cap sits out (upstream disables the interior
//! shell in wireframe, facts #45). After the parts, the unlit
//! alpha-blended overlays: the grid/bed/tripod ([`grid::geometry`])
//! first, then the measure markers ([`markers::geometry`]) — orange
//! spheres at the picked points plus the two-point link. Both are
//! depth-tested so the model occludes them, but neither writes depth.
//!
//! The compare ghost (M5's comparison split — this port owns the render
//! half): a second [`PartScene`] staged from the comparison version in
//! the CURRENT scene's pin (`PartScene::from_parts_with_pin`,
//! ModelViewer.swift:135 — ghostEntity hangs under `content`) draws as
//! an unlit systemOrange wireframe at [`GHOST_ALPHA`] through its own
//! LINE pipeline over the ghost scene's edge lists, between the cap and
//! the overlays — depth-tested so the mesh occludes it, never writing
//! depth so it occludes nothing after it.

pub mod gpu;
pub mod grid;
pub mod markers;
pub mod parts;
pub mod scene;

pub use grid::{GridMesh, GridVertex};
pub use markers::{MarkerMesh, ORANGE};
pub use parts::{GpuPart, NEUTRAL, PartScene, srgb_to_linear, srgb_to_linear_f};
pub use scene::{ModelPrimitive, ModelScene};

use std::hash::Hasher;

use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Camera {
    pub view_proj: [[f32; 4]; 4],
    pub section_axis: u32,
    pub section_pos: f32,
    pub section_enable: f32,
    /// WGSL uniform structs round their size up to 16 bytes: 64 + 4 + 4 + 4 = 76
    /// must be padded to 80 or bind-group validation fails on min_binding_size.
    pub _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

/// Frame-level render mode (upstream ModelViewerState's toggles).
/// `wireframe` swaps the part pass from filled triangles to the 1px
/// hairline edge list — same material and opacity as fill, only the
/// topology changes (facts #44: upstream is the same material with
/// `triangleFillMode = .lines`; wgpu/WebGPU fixes line width at 1, so no
/// tint and no wide lines). `msaa` picks the sample count (1 is the
/// default); unlike wireframe's pure draw-path switch, a count flip
/// REBUILDS the render pipelines and owned attachments in
/// [`MeshPipeline::prepare_parts`] — iced's Storage owns one pipeline per
/// type and cannot hand out replacements, so the app's toggle mutates in
/// place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub wireframe: bool,
    /// Samples per pixel (1 or 4); a change from what is resident
    /// triggers the in-place rebuild.
    pub msaa: u32,
}

impl Default for Options {
    fn default() -> Self {
        // msaa must default to the single-sample count, not u32::default():
        // 0 would differ from a fresh pipeline's resident 1 and drive the
        // rebuild into an invalid 0-sample attachment
        Options {
            wireframe: false,
            msaa: 1,
        }
    }
}

/// One part's pose for a frame: `Some` carries the column-major model
/// matrix (glam `to_cols_array_2d`) the part's group-1 uniform is written
/// with; `None` hides the part — its draw is skipped and it contributes to
/// no effective aabb. The explode/visibility/playback state of a part is
/// exactly this slot, written per frame, never staged into content.
pub type PartPose = Option<[[f32; 4]; 4]>;

/// One part's motion-appearance flags for a frame (upstream's playback
/// materials, ModelViewer.swift:263-266): `translucent` draws the part
/// alpha-blended at [`TRANSLUCENT_ALPHA`] (the static-parts material),
/// `collision` replaces the whole material with the flat-red emissive one
/// — opaque, unlit, winning over `translucent` exactly as upstream's
/// material-array swap does. The default `{ false, false }` draws the
/// staged color, lit, opaque — byte-identical to a pre-motion frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PartAppearance {
    pub translucent: bool,
    pub collision: bool,
}

/// Everything one frame's prepare needs, bundled: the camera, the staged
/// scene, one [`PartPose`] and one [`PartAppearance`] per part (both same
/// length — asserted), the aabb the section cap and framing should use
/// (defaults to the scene's own; the arrangement wiring replaces it with
/// the effective box), the overlay, and the render mode. The struct is the
/// renderer's whole per-frame input contract — later tasks extend it
/// rather than grow the parameter list again.
pub struct Frame<'a> {
    pub camera: &'a Camera,
    pub scene: &'a PartScene,
    pub poses: &'a [PartPose],
    pub appearances: &'a [PartAppearance],
    /// The aabb of what is effectively drawn (hidden parts excluded,
    /// arranged offsets included) in pinned scene space.
    pub effective_aabb: ([f32; 3], [f32; 3]),
    pub grid: Option<&'a GridMesh>,
    /// The staged measure markers, when a measurement is active — drawn
    /// after the grid on the same unlit overlay pass.
    pub markers: Option<&'a MarkerMesh>,
    /// The compare ghost, when a comparison is active — a PartScene staged
    /// from the comparison version WITH THIS SCENE'S PIN
    /// (`PartScene::from_parts_with_pin(.., scene.pin)`): drawn after the
    /// cap, before the overlays, as an unlit orange wireframe over its
    /// edge lists.
    pub ghost: Option<&'a PartScene>,
    pub options: Options,
}

/// Per-part model transform plus motion appearance (group 1), written from
/// the frame's pose and appearance on every prepare — the sanctioned
/// per-frame path. Identity while parts render where they sit; explode
/// offsets, playback poses, and the collision/translucency flags all write
/// through this same binding, never a vertex.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Model {
    transform: [[f32; 4]; 4],
    /// 1.0 when the frame's appearance flagged a collision — fs_main swaps
    /// in the flat-red material on it.
    collision: f32,
    /// [`TRANSLUCENT_ALPHA`] when the appearance flagged translucency, else
    /// 1.0 — fs_main's output alpha (opaque on collision parts regardless).
    alpha: f32,
    /// WGSL uniform structs round their size up to 16 bytes: 64 + 4 + 4 = 72
    /// must be padded to 80 or bind-group validation fails on
    /// min_binding_size.
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CapVertex {
    pos: [f32; 3],
}

/// The section cap's color: upstream's cut-face material —
/// `systemRed.blended(withFraction: 0.15, of: .black)` (ModelViewer.swift:
/// 363) = sRGB `(0.85, 0.85·59/255, 0.85·48/255)` through the exact EOTF —
/// LINEAR, the space the unlit cap shader writes, encoded for the target
/// by `to_target` (the same once-only crossing every staged triangle color
/// makes). The Rust constant is the one definition: `cap_wgsl_source`
/// substitutes it into cap.wgsl, and the cap-color test reads it back
/// against this value.
pub const CUT_FACE_LINEAR: [f32; 3] = [0.6920, 0.0321, 0.0220];

/// The motion collision material's color: systemRed straight through the
/// exact EOTF (the same red [`CUT_FACE_LINEAR`] blends 15% black for the
/// cut face; the collision material uses it unblended as BOTH baseColor
/// and emissiveColor, ModelViewer.swift:275-279 — hence flat, unlit).
/// LINEAR, the space the mesh shader writes, encoded for the target by
/// `to_target`. The Rust constant is the one definition:
/// `mesh_wgsl_source` substitutes it into mesh.wgsl, and the
/// collision-color test reads it back against this value.
pub const COLLISION_LINEAR: [f32; 3] = [1.0, 0.0437, 0.0295];

/// The translucent static-parts material's opacity
/// (ModelViewer.swift:410: `.transparent(opacity: 0.22)`).
pub const TRANSLUCENT_ALPHA: f32 = 0.22;

/// The compare ghost's straight alpha (ModelViewer.swift:375):
/// `blending = .transparent(opacity: 0.6)` over the unlit systemOrange
/// rgb ([`markers::ORANGE`]) — the value `ghost_wgsl_source` substitutes
/// into ghost.wgsl and the blend test reads pixels back against.
pub const GHOST_ALPHA: f32 = 0.6;

/// World-space quad on the section plane: the effective scene aabb
/// cross-section at `frame.camera.section_pos`, grown 10% on the two
/// off-axis directions so the cap always covers the cut face (the stencil
/// test clips it back to the solid). A 4-vertex triangle strip.
fn cap_quad(frame: &Frame<'_>) -> [CapVertex; 4] {
    let camera = frame.camera;
    // match the mesh shader: axis 0 -> x, 2 -> z, anything else -> y
    let axis = match camera.section_axis {
        0 | 1 | 2 => camera.section_axis as usize,
        _ => 1,
    };
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let at = |pu: f32, pv: f32| {
        let mut p = [0.0f32; 3];
        p[axis] = camera.section_pos;
        p[u] = pu;
        p[v] = pv;
        CapVertex { pos: p }
    };
    let edge = |a: usize| {
        let center = (frame.effective_aabb.0[a] + frame.effective_aabb.1[a]) / 2.0;
        let half = (frame.effective_aabb.1[a] - frame.effective_aabb.0[a]) / 2.0 * 1.1;
        (center - half, center + half)
    };
    let (u0, u1) = edge(u);
    let (v0, v1) = edge(v);
    [at(u0, v0), at(u1, v0), at(u0, v1), at(u1, v1)]
}

impl Camera {
    /// The orbit camera at `(distance, yaw, pitch)` around `target` for a
    /// `aspect`-wide viewport, sectioning off — the uniform form of
    /// [`camera::orbit`].
    pub fn orbiting(distance: f32, yaw: f32, pitch: f32, target: [f32; 3], aspect: f32) -> Camera {
        Camera {
            view_proj: camera::orbit(distance, yaw, pitch, target, aspect),
            section_axis: 1,
            section_pos: f32::MAX,
            section_enable: 0.0,
            _pad: 0.0,
        }
    }
}

/// Parity camera math, transcribed from ModelViewer.swift (facts §1).
/// Pure — no GPU types — so the formulas are testable headless; the
/// adapter owns the state (yaw/pitch/distance/target) and calls in.
pub mod camera {
    /// Vertical FOV (ModelViewer.swift:144) with near and far planes
    /// (:142-143) — absolute, never radius-relative.
    pub const FOV: f32 = 35.0_f32.to_radians();
    pub const NEAR: f32 = 1.0;
    pub const FAR: f32 = 100_000.0;
    /// Upstream's exact pitch clamp (ModelViewer.swift:95), π/2 minus
    /// ~0.01 — not a value this crate invents.
    pub const MAX_PITCH: f32 = 1.5607;
    /// Drag-to-orbit sensitivity, radians per point (ModelViewer.swift:302-303).
    pub const ORBIT_SENSITIVITY: f32 = 0.008;

    /// Upstream's named views (ModelViewer.swift:4-33).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Preset {
        Iso,
        Front,
        Top,
        Right,
    }

    impl Preset {
        pub fn yaw_pitch(self) -> (f32, f32) {
            match self {
                Preset::Iso => (std::f32::consts::FRAC_PI_4, 0.6),
                Preset::Front => (0.0, 0.0),
                // top looks straight down at the same maximumPitch constant
                Preset::Top => (0.0, MAX_PITCH),
                Preset::Right => (std::f32::consts::FRAC_PI_2, 0.0),
            }
        }
    }

    /// placeCamera (ModelViewer.swift:339-342): look at `target` from
    /// `target + dir(yaw, pitch)·distance` with Y up, projected at
    /// `aspect` — the viewport's width/height, required, so the projection
    /// always matches the window it renders into.
    pub fn orbit(
        distance: f32,
        yaw: f32,
        pitch: f32,
        target: [f32; 3],
        aspect: f32,
    ) -> [[f32; 4]; 4] {
        let at = glam::Vec3::from_array(target);
        let dir = glam::Vec3::new(
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        );
        let view = glam::camera::rh::view::look_at_mat4(at + dir * distance, at, glam::Vec3::Y);
        // directx = WebGPU NDC (Z in [0,1], Y-up)
        let proj = glam::camera::rh::proj::directx::perspective(FOV, aspect, NEAR, FAR);
        (proj * view).to_cols_array_2d()
    }

    /// Framing radius: half the aabb DIAGONAL, floored at 1
    /// (ModelViewer.swift:330) — a long-thin part frames by its diagonal,
    /// not its longest extent.
    pub fn framing_radius(aabb_min: [f32; 3], aabb_max: [f32; 3]) -> f32 {
        let size = glam::Vec3::from_array(aabb_max) - glam::Vec3::from_array(aabb_min);
        (size.length() / 2.0).max(1.0)
    }

    /// frame() (ModelViewer.swift:328-337), aspect-free by construction:
    /// the target is half the scene height above the pinned floor, the
    /// distance fits the radius through the FOV with 15% headroom. The
    /// x/z zeros assume the PartScene's floor pin already centered the
    /// model horizontally.
    pub fn frame(aabb_min: [f32; 3], aabb_max: [f32; 3]) -> ([f32; 3], f32) {
        let size = glam::Vec3::from_array(aabb_max) - glam::Vec3::from_array(aabb_min);
        let target = [0.0, size.y / 2.0, 0.0];
        let distance = framing_radius(aabb_min, aabb_max) / (FOV / 2.0).sin() * 1.15;
        (target, distance)
    }

    /// zoom (ModelViewer.swift:307-310): distance × factor clamped to
    /// [0.1·radius, 200·radius].
    pub fn zoomed(distance: f32, factor: f32, radius: f32) -> f32 {
        (distance * factor).clamp(radius * 0.1, radius * 200.0)
    }

    /// pan's units-per-point (ModelViewer.swift:312-320): the view-plane
    /// height covered by one point at `distance`. The target translation
    /// wiring is M4's; this is the parity formula.
    pub fn pan_units_per_point(distance: f32, view_height: f32) -> f32 {
        2.0 * distance * (FOV / 2.0).tan() / view_height
    }

    /// pan(by:viewHeight:) (ModelViewer.swift:312-320): the look-at target
    /// shifted by the drag in camera-plane world units — `target +=
    /// (-right·dx + up·dy)·units_per_point` over the camera's own scene
    /// axes, derived here from the yaw/pitch pose the same way
    /// [`orbit`]'s look-at builds them. `dx`/`dy` are AppKit-convention
    /// deltas (dy bottom-up; iced's top-down cursor y must be negated by
    /// the caller). The drag WIRING is M4's; this is the parity formula.
    pub fn panned(
        target: [f32; 3],
        yaw: f32,
        pitch: f32,
        distance: f32,
        view_height: f32,
        dx: f32,
        dy: f32,
    ) -> [f32; 3] {
        if view_height <= 0.0 {
            return target;
        }
        let back = glam::Vec3::new(
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        );
        // the look-at basis: camera +X/+Y in scene space, Y-up world
        let right = glam::Vec3::Y.cross(back).normalize_or_zero();
        let up = back.cross(right);
        let t = glam::Vec3::from_array(target)
            + (right * -dx + up * dy) * pan_units_per_point(distance, view_height);
        t.to_array()
    }

    /// orbit(by:) (ModelViewer.swift:301-305): drag deltas in points to
    /// yaw/pitch — yaw decreases with dx, pitch increases with dy, both
    /// scaled 0.008, pitch clamped to ±MAX_PITCH.
    pub fn orbit_drag(yaw: f32, pitch: f32, dx: f32, dy: f32) -> (f32, f32) {
        (
            yaw - dx * ORBIT_SENSITIVITY,
            (pitch + dy * ORBIT_SENSITIVITY).clamp(-MAX_PITCH, MAX_PITCH),
        )
    }

    /// pick(at:in:on:)'s ray half (ModelViewer.swift:291-299, facts #29):
    /// the scene-space (origin, direction) a click at `cursor` casts through
    /// the orbit camera at `(distance, yaw, pitch)` around `target`, for a
    /// `view`-sized viewport at `aspect`. Upstream unprojects through
    /// RealityKit — `camera.convert((x, y, −1), to: root)` — which in pure
    /// math is `normalize(x·r + y·u + f)` over the look-at basis
    /// [`orbit`]/[`panned`] build: eye from placeCamera's formula, `f`
    /// forward, `r`/`u` the camera's own right/up. `cursor` is iced's
    /// top-down point and must cross to AppKit's bottom-up `h − cy` BEFORE
    /// the NDC mapping (facts #29) or every vertical click aims mirrored;
    /// `x` scales by `aspect` and both by the half-FOV tangent, exactly
    /// upstream's unprojection terms. Pure — the click flow owns the
    /// off-thread seam this is cast from.
    pub fn pick_ray(
        distance: f32,
        yaw: f32,
        pitch: f32,
        target: [f32; 3],
        aspect: f32,
        cursor: (f32, f32),
        view: (f32, f32),
    ) -> ([f32; 3], [f32; 3]) {
        let at = glam::Vec3::from_array(target);
        // placeCamera's eye offset, a unit vector by construction
        let back = glam::Vec3::new(
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        );
        let eye = at + back * distance;
        // the look-at basis: f = normalize(target − eye) = −back; r and u as
        // `panned` derives them (r degenerate only at ±90° pitch, which
        // MAX_PITCH's clamp keeps unreachable)
        let f = -back;
        let r = glam::Vec3::Y.cross(back).normalize_or_zero();
        let u = back.cross(r);
        let tangent = (FOV / 2.0).tan();
        let (w, h) = view;
        let x = (2.0 * cursor.0 / w - 1.0) * tangent * aspect;
        // iced's cursor is top-down; AppKit's point is bottom-up (facts #29)
        let y = (2.0 * (h - cursor.1) / h - 1.0) * tangent;
        let dir = (r * x + u * y + f).normalize_or_zero();
        (eye.to_array(), dir.to_array())
    }
}

/// Two-light parity shading (facts §1, ModelViewer.swift:344-353):
/// upstream adds two directional lights — key intensity 2600 looking at
/// the origin from (60,120,90), fill 900 from (-90,40,-60), both SCENE
/// space, i.e. post-remap — and their PBR output is approximated in-shader
/// as `intensity/10⁴ · max(dot(n, dir), 0)` over the vertex normal plus a
/// 0.15 ambient, clamped to [0, 1.2]. The constants live here, once:
/// [`MeshPipeline`] substitutes them into mesh.wgsl at creation, so the
/// shader and the headless shading tests run the same numbers.
pub mod lights {
    /// Key light position, scene space (ModelViewer.swift:346-348).
    pub const KEY_POS: [f32; 3] = [60.0, 120.0, 90.0];
    /// Key light intensity (ModelViewer.swift:347).
    pub const KEY_INTENSITY: f32 = 2600.0;
    /// Fill light position, scene space (ModelViewer.swift:350-352).
    pub const FILL_POS: [f32; 3] = [-90.0, 40.0, -60.0];
    /// Fill light intensity (ModelViewer.swift:351).
    pub const FILL_INTENSITY: f32 = 900.0;
    /// Flat ambient term.
    pub const AMBIENT: f32 = 0.15;
    /// Clamp ceiling on the summed light (the recorded approximation).
    pub const LIT_MAX: f32 = 1.2;
    /// Directional intensity to diffuse coefficient: 2600 -> 0.26,
    /// 900 -> 0.09 — the in-shader stand-in for RealityKit's
    /// directional-light output.
    const INTENSITY_SCALE: f32 = 1.0 / 10_000.0;

    /// The key light's diffuse coefficient (2600 -> 0.26).
    pub fn key_coeff() -> f32 {
        KEY_INTENSITY * INTENSITY_SCALE
    }

    /// The fill light's diffuse coefficient (900 -> 0.09).
    pub fn fill_coeff() -> f32 {
        FILL_INTENSITY * INTENSITY_SCALE
    }

    /// The light mesh.wgsl applies to a scene-space normal — the Rust twin
    /// of the substituted WGSL block, so tests derive expected face
    /// luminances from the constants the shader actually runs. A unit
    /// normal is the intended input; a zero vector degrades to ambient.
    pub fn lit(normal: [f32; 3]) -> f32 {
        let n = glam::Vec3::from_array(normal).normalize_or_zero();
        let key = glam::Vec3::from_array(KEY_POS).normalize();
        let fill = glam::Vec3::from_array(FILL_POS).normalize();
        (key_coeff() * n.dot(key).max(0.0) + fill_coeff() * n.dot(fill).max(0.0) + AMBIENT)
            .clamp(0.0, LIT_MAX)
    }
}

/// The camera for `preset` framing `scene` (upstream's preset-change
/// reframe, ModelViewer.swift:322-337).
pub fn preset_camera(scene: &PartScene, preset: camera::Preset, aspect: f32) -> Camera {
    let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
    let (yaw, pitch) = preset.yaw_pitch();
    Camera::orbiting(distance, yaw, pitch, target, aspect)
}

/// Default iso camera framing `scene` (upstream presets: yaw π/4, pitch 0.6).
pub fn default_camera(scene: &PartScene, aspect: f32) -> Camera {
    preset_camera(scene, camera::Preset::Iso, aspect)
}

/// Blit stage for the msaa > 1 path: a fullscreen-textured-quad pipeline
/// that composites the resolved intermediate over the destination.
struct Blit {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
}

/// Owned multisample targets, recreated in [`MeshPipeline::prepare_parts`]
/// only on size change (never per frame). `color` is the sample-`count`
/// attachment the mesh pass renders into; `resolve` is the single-sample
/// intermediate the pass resolves into and the blit samples.
struct MsaaTargets {
    color: wgpu::TextureView,
    resolve: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

/// A part's GPU residency: its vertex buffer, draw count, and the content
/// identity the buffer currently holds — the transform is NOT part of it
/// (poses write the model uniform per frame). Built by `prepare_parts`
/// when a part's hash differs; untouched otherwise.
struct ResidentPart {
    content_hash: u64,
    vertex_buf: wgpu::Buffer,
    vertex_count: u32,
    /// The wireframe line list as a u32 index buffer into `vertex_buf`
    /// (2 indices per edge) and its index count.
    edge_buf: wgpu::Buffer,
    edge_count: u32,
    /// Group 1 for this part's draws: the model transform + appearance.
    model_bg: wgpu::BindGroup,
    /// The model uniform behind `model_bg` — every prepare writes the
    /// frame's pose and appearance into it (80 B), the whole per-frame
    /// cost of moving or recoloring a part.
    model_buf: wgpu::Buffer,
}

/// An overlay's GPU residency — the same content-identity contract as
/// [`ResidentPart`], over the unlit meshes staged by [`grid::geometry`]
/// and [`markers::geometry`].
struct ResidentGrid {
    content_hash: u64,
    vertex_buf: wgpu::Buffer,
    vertex_count: u32,
}

/// Content-hash-keyed overlay residency — the staging step both overlay
/// slots (`grid`, `marker`) share: (re)build the vertex buffer only when
/// the content identity differs from what the slot holds; `None` drops
/// the slot entirely. `label` names the buffer for debuggers.
fn stage_overlay(
    slot: &mut Option<ResidentGrid>,
    device: &wgpu::Device,
    mesh: Option<(u64, &[GridVertex], &str)>,
) {
    if let Some((content_hash, vertices, label)) = mesh {
        if !slot
            .as_ref()
            .is_some_and(|r| r.content_hash == content_hash)
        {
            *slot = Some(ResidentGrid {
                content_hash,
                vertex_buf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("{label} vertices")),
                    contents: bytemuck::cast_slice(vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                vertex_count: vertices.len() as u32,
            });
        }
    } else {
        *slot = None;
    }
}

/// A ghost part's GPU residency — [`ResidentPart`] minus the model
/// uniform: the ghost draws through the camera-only layout (upstream
/// parents ghostEntity straight under `content`, so there is no per-part
/// pose to carry). The vertex buffer holds the ghost scene's staged
/// bytes; the edge index buffer is the line list this part draws.
struct ResidentGhostPart {
    vertex_buf: wgpu::Buffer,
    edge_buf: wgpu::Buffer,
    edge_count: u32,
}

/// Whole-scene identity of a ghost [`PartScene`]: the part count and every
/// part's content hash — the key that stands in for upstream's ghostID
/// swap (ModelViewer.swift:171-174 restages the ghost when ghostID or the
/// mesh changes). The per-part identities are already computed at staging
/// (pin included), so this is O(parts), not O(vertices).
fn ghost_identity(scene: &PartScene) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_usize(scene.parts.len());
    for part in &scene.parts {
        h.write_u64(part.content_hash);
    }
    h.finish()
}

/// The depth/stencil contract shared by every pass that must not occlude —
/// the grid/markers overlay pipeline, the compare ghost, and the
/// translucent part pipelines: depth-TESTED so the opaque mesh orders
/// these draws correctly, depth-write OFF so they occlude nothing after
/// them, stencil inert (Always/Keep — the mesh pass's winding parity stays
/// untouched for whoever reads it next).
fn overlay_depth_stencil() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth24PlusStencil8,
        depth_write_enabled: false,
        depth_compare: wgpu::CompareFunction::LessEqual,
        stencil: wgpu::StencilState {
            front: wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Always,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Keep,
            },
            back: wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Always,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Keep,
            },
            read_mask: 0xFF,
            write_mask: 0xFF,
        },
        bias: Default::default(),
    }
}

pub struct MeshPipeline {
    pipeline: wgpu::RenderPipeline,
    cap_pipeline: wgpu::RenderPipeline,
    /// Wireframe hairlines (facts #44): the mesh shader at LineList
    /// topology, drawn instead of `pipeline` when `wireframe` is set.
    line_pipeline: wgpu::RenderPipeline,
    /// The translucent part set's pipelines (motion playback): the mesh
    /// and line materials with alpha blending and no depth write — drawn
    /// after the opaque set, before the cap.
    mesh_blend_pipeline: wgpu::RenderPipeline,
    line_blend_pipeline: wgpu::RenderPipeline,
    /// Unlit grid/bed/tripod overlay (facts #34/#37).
    grid_pipeline: wgpu::RenderPipeline,
    /// The compare ghost's unlit orange wireframe (ModelViewer.swift:
    /// 371-378): LINE topology over the ghost scene's edge lists.
    ghost_pipeline: wgpu::RenderPipeline,
    /// Part-draw pipeline layout (camera + per-part model groups): the mesh
    /// and line pipelines; rebuilt pipelines keep using this object so the
    /// resident bind groups stay compatible.
    part_pass_layout: wgpu::PipelineLayout,
    /// Camera-only pipeline layout: the cap and grid pipelines.
    camera_pass_layout: wgpu::PipelineLayout,
    bind_group: wgpu::BindGroup,
    model_layout: wgpu::BindGroupLayout,
    /// Depth24PlusStencil8, viewer-owned; resized in
    /// [`MeshPipeline::prepare_parts`].
    depth: wgpu::TextureView,
    depth_tex: wgpu::Texture,
    uniform_buf: wgpu::Buffer,
    residents: Vec<ResidentPart>,
    /// The slots the last prepared frame poses visibly (its `Some` poses,
    /// in slot order) — the draw path's visibility snapshot, taken at
    /// prepare because render has no pose slice to read. This list holds
    /// the OPAQUE slots (default or collision appearance); the translucent
    /// slots sit in `drawn_translucent`.
    drawn: Vec<usize>,
    /// The translucent (`!collision && translucent`) slots of the last
    /// prepared frame, in slot order — drawn after `drawn` through the
    /// blend pipelines.
    drawn_translucent: Vec<usize>,
    /// The resident overlay, when the last prepared frame carried one.
    grid: Option<ResidentGrid>,
    /// The resident measure markers, when the last prepared frame carried
    /// any — the same content-identity contract as `grid`.
    marker: Option<ResidentGrid>,
    /// The resident compare ghost, when the last prepared frame carried
    /// one — whole-scene keyed on [`ghost_identity`] (a compare-version
    /// swap rebuilds every buffer; anything else touches none).
    ghost_residents: Vec<ResidentGhostPart>,
    /// The whole-scene identity currently resident in `ghost_residents`
    /// (`None` = the slot is empty).
    ghost_identity: Option<u64>,
    cap_buf: wgpu::Buffer,
    draw_cap: bool,
    size: (u32, u32),
    /// 1 for [`MeshPipeline::new`]; both pipelines carry this sample count
    /// and every owned attachment matches it.
    sample_count: u32,
    format: wgpu::TextureFormat,
    /// Present iff `sample_count > 1`.
    blit: Option<Blit>,
    /// Owned msaa color/resolve textures; `None` at count 1 and until the
    /// first `prepare_parts` at count > 1.
    msaa: Option<MsaaTargets>,
    /// The frame mode last prepared (facts #44).
    wireframe: bool,
    /// Vertex uploads since creation — one per part residency. The
    /// per-frame uniform writes (camera, cap, per-part model) are the
    /// sanctioned path and never count: these counters track vertex/index
    /// data only. Read only by the `*_count_for_tests` accessors.
    #[cfg_attr(not(test), allow(dead_code))]
    upload_count: u32,
    /// Vertex-buffer allocations since creation (one per residency). Read
    /// only by the `*_count_for_tests` accessors.
    #[cfg_attr(not(test), allow(dead_code))]
    alloc_count: u32,
    /// Ghost vertex+edge uploads since creation — one per ghost part
    /// restage. Read only by `ghost_upload_count_for_tests`.
    #[cfg_attr(not(test), allow(dead_code))]
    ghost_upload_count: u32,
}

/// The output-space block substituted as `@TARGET_SPACE@` into every
/// color-writing shader: the viewer's colors are LINEAR (staged through
/// [`parts::srgb_to_linear`], lit in linear), and whether the pipeline must
/// encode them on the way out depends on the target — an sRGB-tagged
/// format stores the encode in hardware, a plain one (what iced's
/// compositor picks under libcosmic's `web-colors` build, where
/// `GAMMA_CORRECTION` is false and every iced primitive writes
/// display-space values itself) does not, so the shader carries the curve.
/// `SRGB_ENCODE` is 1.0 only in the second case, and the curve is the
/// exact inverse of [`parts::srgb_to_linear_f`] (its Rust twin
/// [`parts::linear_to_srgb_f`] is what the tests pin against).
fn target_space_wgsl(target_format: wgpu::TextureFormat) -> String {
    let flag = if target_format.is_srgb() { 0.0 } else { 1.0 };
    format!(
        "// Substituted from target_space_wgsl: a plain (non-sRGB) target\n\
         // stores no encode in hardware, so the shader carries the curve.\n\
         const SRGB_ENCODE: f32 = {flag:.1};\n\
         fn encode_target(c: f32) -> f32 {{\n\
         \x20   let e = select(1.055 * pow(c, 1.0 / 2.4) - 0.055, c * 12.92, c <= 0.0031308);\n\
         \x20   return clamp(e, 0.0, 1.0);\n\
         }}\n\
         fn to_target(rgb: vec3<f32>) -> vec3<f32> {{\n\
         \x20   if (SRGB_ENCODE < 0.5) {{ return rgb; }}\n\
         \x20   return vec3<f32>(encode_target(rgb.r), encode_target(rgb.g), encode_target(rgb.b));\n\
         }}"
    )
}

/// mesh.wgsl with the lighting constants substituted from [`lights`], the
/// collision color from [`COLLISION_LINEAR`], and the output-space block
/// for `target_format` — the one definition the shader and the shading
/// tests share. The @NAME@ markers are not valid WGSL, so a missed
/// substitution breaks shader compilation instead of silently rendering
/// numbers the tests do not assert against.
fn mesh_wgsl_source(target_format: wgpu::TextureFormat) -> String {
    let vec3 = |p: [f32; 3]| format!("vec3<f32>({}, {}, {})", p[0], p[1], p[2]);
    include_str!("mesh.wgsl")
        .replace("@KEY_POS@", &vec3(lights::KEY_POS))
        .replace("@KEY_COEFF@", &lights::key_coeff().to_string())
        .replace("@FILL_POS@", &vec3(lights::FILL_POS))
        .replace("@FILL_COEFF@", &lights::fill_coeff().to_string())
        .replace("@AMBIENT@", &lights::AMBIENT.to_string())
        .replace("@LIT_MAX@", &lights::LIT_MAX.to_string())
        .replace("@COLLISION_R@", &COLLISION_LINEAR[0].to_string())
        .replace("@COLLISION_G@", &COLLISION_LINEAR[1].to_string())
        .replace("@COLLISION_B@", &COLLISION_LINEAR[2].to_string())
        .replace("@TARGET_SPACE@", &target_space_wgsl(target_format))
}

/// cap.wgsl with the cut-face color substituted from [`CUT_FACE_LINEAR`]
/// and the output-space block for `target_format` — the mesh_wgsl_source
/// pattern: the @CAP_@ markers are not valid WGSL, so a missed
/// substitution breaks shader compilation instead of silently rendering a
/// color the tests do not assert against.
fn cap_wgsl_source(target_format: wgpu::TextureFormat) -> String {
    include_str!("cap.wgsl")
        .replace("@CAP_R@", &CUT_FACE_LINEAR[0].to_string())
        .replace("@CAP_G@", &CUT_FACE_LINEAR[1].to_string())
        .replace("@CAP_B@", &CUT_FACE_LINEAR[2].to_string())
        .replace("@TARGET_SPACE@", &target_space_wgsl(target_format))
}

/// grid.wgsl with the output-space block for `target_format` — the
/// mesh_wgsl_source pattern over the unlit overlay's passthrough fragment
/// (rgb encoded, alpha carried as staged).
fn grid_wgsl_source(target_format: wgpu::TextureFormat) -> String {
    include_str!("grid.wgsl").replace("@TARGET_SPACE@", &target_space_wgsl(target_format))
}

/// ghost.wgsl with the ghost color substituted from [`markers::ORANGE`],
/// [`GHOST_ALPHA`], and the output-space block for `target_format` — the
/// mesh_wgsl_source pattern: the @GHOST_@ markers are not valid WGSL, so a
/// missed substitution breaks shader compilation instead of silently
/// rendering a color the tests do not assert against.
fn ghost_wgsl_source(target_format: wgpu::TextureFormat) -> String {
    include_str!("ghost.wgsl")
        .replace("@GHOST_R@", &markers::ORANGE[0].to_string())
        .replace("@GHOST_G@", &markers::ORANGE[1].to_string())
        .replace("@GHOST_B@", &markers::ORANGE[2].to_string())
        .replace("@GHOST_ALPHA@", &GHOST_ALPHA.to_string())
        .replace("@TARGET_SPACE@", &target_space_wgsl(target_format))
}

/// The sample-count-dependent render pipelines, rebuilt as a set by
/// [`MeshPipeline::make_pipelines`] at creation and on every
/// [`Options::msaa`] flip in `prepare_parts`.
struct Pipelines {
    mesh: wgpu::RenderPipeline,
    cap: wgpu::RenderPipeline,
    line: wgpu::RenderPipeline,
    /// The mesh/line pipelines' alpha-blended twins for the translucent
    /// part set (blend on, the overlay depth stance).
    mesh_blend: wgpu::RenderPipeline,
    line_blend: wgpu::RenderPipeline,
    grid: wgpu::RenderPipeline,
    ghost: wgpu::RenderPipeline,
}

impl MeshPipeline {
    pub fn new(
        device: &wgpu::Device,
        _queue: &wgpu::Queue, // camera/mesh data arrives per frame in `prepare_parts`
        target_format: wgpu::TextureFormat,
    ) -> Self {
        Self::new_at_samples(device, target_format, 1)
    }

    /// `new` at a multisample count (4 is the tested default): pipelines,
    /// depth/stencil, and the owned color attachment all run at
    /// `sample_count`; `render` resolves into an intermediate and blits it
    /// over the destination with premultiplied alpha, preserving content
    /// already there. Count 1 ([`MeshPipeline::new`]) stays the app default.
    pub fn with_msaa(
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        Self::new_at_samples(device, target_format, sample_count)
    }

    fn new_at_samples(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&Camera {
                view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
                section_axis: 1,
                section_pos: f32::MAX,
                section_enable: 0.0,
                _pad: 0.0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
            label: None,
        });
        // one bind group per part: no MAX_PARTS-style cap, just a bind per
        // draw — assemblies of any size render correctly. VERTEX_FRAGMENT
        // like the camera layout: fs_main reads model.collision/model.alpha,
        // and pipeline creation fails validation until the layout says so
        let model_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: Some("part model layout"),
        });
        let part_pass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[&layout, &model_layout],
            immediate_size: 0,
            label: None,
        });
        let camera_pass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[&layout],
            immediate_size: 0,
            label: None,
        });
        let pipelines = Self::make_pipelines(
            device,
            target_format,
            sample_count,
            &part_pass_layout,
            &camera_pass_layout,
        );
        // 4-vertex strip, zeroed at build; `prepare_parts` writes the real
        // quad from the current scene aabb + camera section params each frame
        let cap_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("section cap quad"),
            contents: bytemuck::cast_slice(&[CapVertex { pos: [0.0; 3] }; 4]),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let blit = (sample_count > 1).then(|| Self::make_blit(device, target_format));
        let (depth_tex, depth) = Self::make_depth(device, (1, 1), sample_count);
        Self {
            pipeline: pipelines.mesh,
            cap_pipeline: pipelines.cap,
            line_pipeline: pipelines.line,
            mesh_blend_pipeline: pipelines.mesh_blend,
            line_blend_pipeline: pipelines.line_blend,
            grid_pipeline: pipelines.grid,
            ghost_pipeline: pipelines.ghost,
            part_pass_layout,
            camera_pass_layout,
            bind_group,
            model_layout,
            depth,
            depth_tex,
            uniform_buf,
            residents: Vec::new(),
            drawn: Vec::new(),
            drawn_translucent: Vec::new(),
            grid: None,
            marker: None,
            ghost_residents: Vec::new(),
            ghost_identity: None,
            cap_buf,
            draw_cap: false,
            size: (1, 1),
            sample_count,
            format: target_format,
            blit,
            msaa: None,
            wireframe: false,
            upload_count: 0,
            alloc_count: 0,
            ghost_upload_count: 0,
        }
    }

    /// The six render pipelines at `sample_count`, built against the SAME
    /// pipeline layout objects the resident bind groups reference — a
    /// rebuilt pipeline stays bind-compatible without re-staging anything.
    fn make_pipelines(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        sample_count: u32,
        part_layout: &wgpu::PipelineLayout,
        camera_layout: &wgpu::PipelineLayout,
    ) -> Pipelines {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh.wgsl"),
            source: wgpu::ShaderSource::Wgsl(mesh_wgsl_source(target_format).into()),
        });
        let vlayout = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 24,
                shader_location: 2,
            },
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
            layout: Some(part_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 36,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &vlayout,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                // No culling: back faces must rasterize or their decrement
                // never happens and the winding parity below breaks.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                // Winding parity for the section cap: every rasterized
                // fragment flips the stencil (front +1, back -1), including
                // depth-occluded ones, so after the pass stencil != 0 exactly
                // where a ray from the camera ends inside the sectioned
                // solid — the cut face the cap quad fills. Wrap (not Clamp)
                // keeps 0-1 = 255 in the parity; fragments discarded by the
                // section plane never reach the stencil op, which is what
                // clips the count at the cut.
                stencil: wgpu::StencilState {
                    front: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Always,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::IncrementWrap,
                        pass_op: wgpu::StencilOperation::IncrementWrap,
                    },
                    back: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Always,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::DecrementWrap,
                        pass_op: wgpu::StencilOperation::DecrementWrap,
                    },
                    read_mask: 0xFF,
                    write_mask: 0xFF,
                },
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        // The cap quad fills the cut face where the mesh pass left winding
        // parity. Its stencil test is read-only (all ops Keep) so the mesh's
        // inc/dec stays in the attachment; depth is read-only too (the quad
        // must not occlude anything drawn after it), and it wins LessEqual
        // only where the surviving mesh lies beyond the section plane.
        // Camera-only (camera_layout): the cap has no per-part transform.
        let cap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cap.wgsl"),
            source: wgpu::ShaderSource::Wgsl(cap_wgsl_source(target_format).into()),
        });
        let cap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("section cap"),
            layout: Some(camera_layout),
            vertex: wgpu::VertexState {
                module: &cap_shader,
                entry_point: Some("vs_cap"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CapVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &cap_shader,
                entry_point: Some("fs_cap"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            // The quad is 4 Z-ordered vertices drawn 0..4 (see `cap_quad`) —
            // a STRIP, so both halves rasterize. The topology must say so:
            // PrimitiveState's TriangleList default renders only the first
            // 3 of the 4 vertices, HALF the quad, with the cut-face center
            // landing on the unrendered diagonal (the cap-color test's
            // sample point; through M3 this half-quad was invisible — the
            // red-pixel tests counted area, not shape).
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState {
                    front: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::NotEqual,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Keep,
                    },
                    back: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::NotEqual,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Keep,
                    },
                    read_mask: 0xFF,
                    write_mask: 0xFF,
                },
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        // Wireframe hairlines (facts #44): the SAME mesh shader, material,
        // and opacity as fill — only the topology (LineList; wgpu/WebGPU
        // fix line width at 1, so no wide lines and no tint) and culling
        // change. Depth-tested like the fill so hidden edges stay hidden.
        // Stencil ops all Keep: winding parity is a filled-rasterization
        // trick, and the cap it feeds is disabled in wireframe anyway.
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wireframe lines"),
            layout: Some(part_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 36,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &vlayout,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState {
                    front: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Always,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Keep,
                    },
                    back: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Always,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Keep,
                    },
                    read_mask: 0xFF,
                    write_mask: 0xFF,
                },
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        // The translucent part set (motion playback, ModelViewer.swift:410):
        // the mesh material with ALPHA_BLENDING and the overlay depth stance
        // ([`overlay_depth_stencil`]) — depth-TESTED so the opaque set orders
        // correctly, never WRITING depth so a translucent part occludes
        // nothing after it. Stencil all Keep through that shared state: the
        // blend pipelines never participate in section winding parity —
        // section ∩ animation is empty by mutual exclusion (the pane's
        // isAnimating excludes sectioning) — stated here rather than silently
        // inheriting the mesh's inc/dec ops that rely on it. No culling,
        // like the mesh (upstream's translucent material sets faceCulling
        // .none too).
        let mesh_blend_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh blend"),
            layout: Some(part_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 36,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &vlayout,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(overlay_depth_stencil()),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        // The same swap on the wireframe hairlines: a translucent part in
        // wireframe mode draws its edge list through this pipeline — fs_main
        // applies model.alpha identically to fill.
        let line_blend_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wireframe lines blend"),
            layout: Some(part_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 36,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &vlayout,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(overlay_depth_stencil()),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        // Grid / bed / tripod overlay (facts #34/#37): unlit vertex colors,
        // alpha-blended, no culling. Depth-tested so the model occludes
        // the overlay, but NO depth write — an overlay must not occlude
        // ([`overlay_depth_stencil`]).
        let grid_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grid.wgsl"),
            source: wgpu::ShaderSource::Wgsl(grid_wgsl_source(target_format).into()),
        });
        let grid_vlayout = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 12,
                shader_location: 1,
            },
        ];
        let grid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grid overlay"),
            layout: Some(camera_layout),
            vertex: wgpu::VertexState {
                module: &grid_shader,
                entry_point: Some("vs_grid"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GridVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &grid_vlayout,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &grid_shader,
                entry_point: Some("fs_grid"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(overlay_depth_stencil()),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        // The compare ghost (ModelViewer.swift:371-378): unlit systemOrange
        // wireframe at 0.6 — LINE topology like the wireframe pass but its
        // OWN unlit shader and blend (upstream's ghost is a different
        // material than the shaded wireframe: UnlitMaterial + transparent
        // blending). Camera-only (camera_layout): upstream parents
        // ghostEntity straight under `content`, so the ghost carries no
        // per-part pose. Depth ON / write OFF — the grid overlay contract:
        // the mesh occludes the ghost, the ghost occludes nothing after it.
        let ghost_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ghost.wgsl"),
            source: wgpu::ShaderSource::Wgsl(ghost_wgsl_source(target_format).into()),
        });
        let ghost_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compare ghost"),
            layout: Some(camera_layout),
            vertex: wgpu::VertexState {
                module: &ghost_shader,
                entry_point: Some("vs_ghost"),
                buffers: &[wgpu::VertexBufferLayout {
                    // the staged part vertex (pos/normal/color, 36 B) with
                    // only the position attribute read
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &ghost_shader,
                entry_point: Some("fs_ghost"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(overlay_depth_stencil()),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        Pipelines {
            mesh: pipeline,
            cap: cap_pipeline,
            line: line_pipeline,
            mesh_blend: mesh_blend_pipeline,
            line_blend: line_blend_pipeline,
            grid: grid_pipeline,
            ghost: ghost_pipeline,
        }
    }

    /// The msaa toggle's in-place rebuild (iced's Storage owns one
    /// [`MeshPipeline`] per type and cannot hand out replacements):
    /// recreate every render pipeline and owned attachment at `count`,
    /// keeping the layouts, buffers, and part residency — the fresh
    /// pipelines are built against the kept layout objects, so the
    /// resident bind groups stay valid.
    fn set_msaa(&mut self, device: &wgpu::Device, count: u32) {
        let pipelines = Self::make_pipelines(
            device,
            self.format,
            count,
            &self.part_pass_layout,
            &self.camera_pass_layout,
        );
        self.pipeline = pipelines.mesh;
        self.cap_pipeline = pipelines.cap;
        self.line_pipeline = pipelines.line;
        self.mesh_blend_pipeline = pipelines.mesh_blend;
        self.line_blend_pipeline = pipelines.line_blend;
        self.grid_pipeline = pipelines.grid;
        self.ghost_pipeline = pipelines.ghost;
        self.blit = (count > 1).then(|| Self::make_blit(device, self.format));
        let (depth_tex, depth) = Self::make_depth(device, self.size, count);
        self.depth_tex = depth_tex;
        self.depth = depth;
        self.msaa = match &self.blit {
            Some(blit) => Some(Self::make_msaa_targets(
                device,
                blit,
                self.format,
                count,
                self.size,
            )),
            None => None,
        };
        self.sample_count = count;
    }

    /// Blit pipeline for the msaa path: fullscreen triangle sampling the
    /// resolved intermediate, drawn over the destination (`LoadOp::Load` +
    /// premultiplied alpha, single-sample).
    fn make_blit(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Blit {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
            label: Some("blit bindings layout"),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[&layout],
            immediate_size: 0,
            label: None,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("msaa blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_blit"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blit"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        // nearest: a 1:1 copy — the blit must not blur the resolve output
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Blit {
            pipeline,
            sampler,
            layout,
        }
    }

    fn make_msaa_targets(
        device: &wgpu::Device,
        blit: &Blit,
        format: wgpu::TextureFormat,
        sample_count: u32,
        size: (u32, u32),
    ) -> MsaaTargets {
        let extent = wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        };
        let color_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewer msaa color"),
            size: extent,
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let resolve_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewer msaa resolve"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color = color_tex.create_view(&Default::default());
        let resolve = resolve_tex.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &blit.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&blit.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&resolve),
                },
            ],
            label: Some("blit bindings"),
        });
        MsaaTargets {
            color,
            resolve,
            bind_group,
        }
    }

    fn make_depth(
        device: &wgpu::Device,
        size: (u32, u32),
        sample_count: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewer depth+stencil"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24PlusStencil8,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let v = t.create_view(&Default::default());
        (t, v)
    }

    /// Per-frame upload: camera uniform, depth-texture resize to the
    /// render-target `size`, each visible part's model uniform (the pose
    /// and appearance path — 80 B/part: transform, collision flag, alpha;
    /// a moved, hidden, translucent, or colliding part never touches a
    /// vertex), and the upload-once mesh path — a part's vertices are (re)written
    /// only when its content identity differs from what is resident, so an
    /// unchanged scene costs zero mesh uploads however the camera or the
    /// poses move. `frame.grid` (when `Some`) is the overlay
    /// [`grid::geometry`] built — its residency follows the same
    /// content-identity contract; `frame.ghost` follows it too,
    /// whole-scene keyed (see [`ghost_identity`]); `frame.options` picks
    /// the frame's mode, and an `options.msaa` flip triggers the in-place
    /// rebuild ([`Self::set_msaa`]) before anything else.
    pub fn prepare_parts(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        frame: &Frame<'_>,
    ) {
        let (camera, scene, grid, markers, ghost, options) = (
            frame.camera,
            frame.scene,
            frame.grid,
            frame.markers,
            frame.ghost,
            frame.options,
        );
        assert_eq!(
            frame.poses.len(),
            scene.parts.len(),
            "one pose per part: {} poses for {} parts",
            frame.poses.len(),
            scene.parts.len()
        );
        assert_eq!(
            frame.appearances.len(),
            scene.parts.len(),
            "one appearance per part: {} appearances for {} parts",
            frame.appearances.len(),
            scene.parts.len()
        );
        if options.msaa != self.sample_count {
            self.set_msaa(device, options.msaa);
        }
        if self.size != size {
            let (t, v) = Self::make_depth(device, size, self.sample_count);
            self.depth_tex = t;
            self.depth = v;
            if let Some(blit) = &self.blit {
                self.msaa = Some(Self::make_msaa_targets(
                    device,
                    blit,
                    self.format,
                    self.sample_count,
                    size,
                ));
            }
            self.size = size;
        }
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(camera));
        // upload-once: slot-keyed by part index. A resident part whose
        // content identity still matches is skipped whole; a changed or new
        // part gets a fresh exact-size vertex buffer (its allocation and
        // init-write are what the counters track — the model uniform a
        // residency also allocates is written per frame below and is not
        // vertex/index data). Parts beyond the scene's length are dropped
        // with their buffers.
        self.residents.truncate(scene.parts.len());
        for (i, part) in scene.parts.iter().enumerate() {
            if self
                .residents
                .get(i)
                .is_some_and(|r| r.content_hash == part.content_hash)
            {
                continue;
            }
            let label = &part.name;
            let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("part {label} vertices")),
                contents: bytemuck::cast_slice(&part.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            self.alloc_count += 1;
            self.upload_count += 1;
            // the edge list rides the same residency: it derives from the
            // staged vertices, so the hash that triggered this rebuild
            // covers it too
            let edge_indices: Vec<u32> = part.edges.iter().flat_map(|&[a, b]| [a, b]).collect();
            let edge_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("part {label} edges")),
                contents: bytemuck::cast_slice(&edge_indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let edge_count = edge_indices.len() as u32;
            // zeroed: the pose write below fills it before any draw can
            // read it
            let model_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("part {label} model")),
                contents: bytemuck::bytes_of(&Model {
                    transform: [[0.0; 4]; 4],
                    collision: 0.0,
                    alpha: 0.0,
                    _pad: [0.0; 2],
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let model_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.model_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: model_buf.as_entire_binding(),
                }],
                label: Some(&format!("part {label} model bindings")),
            });
            let resident = ResidentPart {
                content_hash: part.content_hash,
                vertex_buf,
                vertex_count: part.vertices.len() as u32,
                edge_buf,
                edge_count,
                model_bg,
                model_buf,
            };
            if i < self.residents.len() {
                self.residents[i] = resident;
            } else {
                self.residents.push(resident);
            }
        }
        // The pose+appearance path: every visible part's model uniform
        // written from the frame (transform, collision flag, alpha — the
        // whole motion surface as uniform state), and the visibility
        // snapshot split the draw path reads: opaque slots (default OR
        // collision — upstream's collision material REPLACES the
        // translucent one, ModelViewer.swift:263-266, so a colliding part
        // stays opaque) in `drawn`, the rest in `drawn_translucent`. A
        // hidden part's slot is absent from both.
        self.drawn.clear();
        self.drawn_translucent.clear();
        for (i, (pose, look)) in frame.poses.iter().zip(frame.appearances).enumerate() {
            if pose.is_none() {
                continue;
            }
            if look.translucent && !look.collision {
                self.drawn_translucent.push(i);
            } else {
                self.drawn.push(i);
            }
            queue.write_buffer(
                &self.residents[i].model_buf,
                0,
                bytemuck::bytes_of(&Model {
                    transform: pose.expect("drawn slots pose Some"),
                    collision: look.collision as u32 as f32,
                    alpha: if look.translucent {
                        TRANSLUCENT_ALPHA
                    } else {
                        1.0
                    },
                    _pad: [0.0; 2],
                }),
            );
        }
        // The cap is this port's cut-face stand-in, and upstream disables
        // the interior shell in wireframe (facts #45); line rasterization
        // breaks winding parity anyway
        self.draw_cap = camera.section_enable > 0.5 && !options.wireframe;
        if self.draw_cap {
            queue.write_buffer(&self.cap_buf, 0, bytemuck::cast_slice(&cap_quad(frame)));
        }
        self.wireframe = options.wireframe;
        // Overlay residency — the same content-identity contract as the
        // parts: an unchanged grid or marker set costs zero overlay bytes
        // however the camera moves; `None` drops the slot (the app's grid
        // toggle, the measure flow's clear)
        stage_overlay(
            &mut self.grid,
            device,
            grid.map(|g| (g.content_hash, &g.vertices[..], "grid overlay")),
        );
        stage_overlay(
            &mut self.marker,
            device,
            markers.map(|m| (m.content_hash, &m.vertices[..], "measure marker")),
        );
        // Ghost residency — the same content-identity contract, whole-scene
        // keyed: every ghost buffer rebuilds when the ghost scene's
        // identity changes (a compare-version swap, upstream's ghostID),
        // and nothing is touched while it holds — a moved camera or an
        // unchanged comparison never re-uploads ghost bytes.
        let ghost_id = ghost.map(ghost_identity);
        if ghost_id != self.ghost_identity {
            self.ghost_residents = ghost
                .map(|ghost| {
                    ghost
                        .parts
                        .iter()
                        .map(|part| {
                            let label = &part.name;
                            let vertex_buf =
                                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some(&format!("ghost {label} vertices")),
                                    contents: bytemuck::cast_slice(&part.vertices),
                                    usage: wgpu::BufferUsages::VERTEX,
                                });
                            let edge_indices: Vec<u32> =
                                part.edges.iter().flat_map(|&[a, b]| [a, b]).collect();
                            let edge_buf =
                                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some(&format!("ghost {label} edges")),
                                    contents: bytemuck::cast_slice(&edge_indices),
                                    usage: wgpu::BufferUsages::INDEX,
                                });
                            self.ghost_upload_count += 1;
                            ResidentGhostPart {
                                vertex_buf,
                                edge_buf,
                                edge_count: edge_indices.len() as u32,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.ghost_identity = ghost_id;
        }
    }

    /// Vertex uploads since pipeline creation (test visibility): counts
    /// vertex uploads only — the camera, cap, and per-part model uniforms
    /// legitimately write every frame and never count.
    
#[cfg(test)]
    pub(crate) fn upload_count_for_tests(&self) -> u32 {
        self.upload_count
    }

    /// Vertex-buffer allocations since pipeline creation (test visibility).
    #[cfg(test)]
    pub(crate) fn alloc_count_for_tests(&self) -> u32 {
        self.alloc_count
    }

    /// Ghost vertex+edge uploads since pipeline creation (test
    /// visibility): counts one per ghost part restage — an unchanged
    /// ghost never re-uploads however the camera moves.
    #[cfg(test)]
    pub(crate) fn ghost_upload_count_for_tests(&self) -> u32 {
        self.ghost_upload_count
    }

    /// Parts the last prepared frame poses visibly — the slots the draw
    /// path binds (opaque + translucent), hidden parts absent (test
    /// visibility).
    #[cfg(test)]
    pub(crate) fn drawn_part_count_for_tests(&self) -> usize {
        self.drawn.len() + self.drawn_translucent.len()
    }

    /// Vertices the wireframe line pass would draw this frame — every
    /// drawn part's edges (BOTH sets: opaque and translucent — a
    /// translucent part in wireframe playback still draws its hairlines)
    /// contribute their 2 corner indices (test visibility).
    #[cfg(test)]
    pub(crate) fn line_vertex_count_for_tests(&self) -> u32 {
        self.drawn
            .iter()
            .chain(&self.drawn_translucent)
            .map(|&i| self.residents[i].edge_count)
            .sum()
    }

    /// Renders the parts (filled, or the wireframe hairlines when the last
    /// prepared `Options` set it) — the red cap quad when the camera
    /// sections, then the compare ghost, then the unlit overlays (grid
    /// first, measure markers after it) — into `target` (the iced surface
    /// view) inside a viewer-owned pass. Color attachment:
    /// `LoadOp::Load` (iced content underneath stays); depth and stencil:
    /// cleared. `viewport` is `[x, y, width, height]` in physical pixels.
    ///
    /// At `sample_count > 1` the mesh pass instead renders into the owned
    /// multisample attachment (cleared transparent — coverage lives in
    /// alpha) and resolves into the intermediate; a second pass then blits
    /// the intermediate over `target` with premultiplied alpha, keeping
    /// iced's content underneath. The blit covers the WHOLE prepared size
    /// (`self.size`, the surface), not just `viewport`: the mesh pass
    /// placed the scene into the surface-sized intermediate at the
    /// viewport's offset, and the uncovered region composites as
    /// transparent — so a widget clip rect smaller than the surface (the
    /// app's canvas beside its control strip) maps 1:1 either way.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: [f32; 4],
    ) {
        if self.drawn.is_empty()
            && self.drawn_translucent.is_empty()
            && self.grid.is_none()
            && self.marker.is_none()
            && self.ghost_residents.is_empty()
        {
            return; // nothing visible was prepared
        }
        let set_vp = |pass: &mut wgpu::RenderPass| {
            pass.set_viewport(
                viewport[0],
                viewport[1],
                viewport[2].max(1.0),
                viewport[3].max(1.0),
                0.0,
                1.0,
            );
        };
        // one depth/stencil state for both paths: cleared every frame, so
        // winding parity never leaks across frames
        let depth_stencil = wgpu::RenderPassDepthStencilAttachment {
            view: &self.depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(0),
                store: wgpu::StoreOp::Store,
            }),
        };
        if let Some(msaa) = &self.msaa {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewer mesh pass (msaa)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa.color,
                    depth_slice: None,
                    resolve_target: Some(&msaa.resolve),
                    ops: wgpu::Operations {
                        // transparent, not the app background: the blit pass
                        // composites over the real target, which still holds it
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        // only the resolve output is needed; the multisample
                        // data itself can be discarded after the resolve
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(depth_stencil),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            set_vp(&mut pass);
            self.draw_scene(&mut pass);
            drop(pass);
            // Composite the resolved frame over whatever is already in
            // `target` — the same Load-and-composite contract as the
            // single-sample path, just through an intermediate.
            let blit = self
                .blit
                .as_ref()
                .expect("msaa targets exist only alongside a blit pipeline");
            let mut blit_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewer msaa blit pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // the blit spans the whole prepared size: the mesh pass placed
            // the viewport-offset scene inside the surface-sized
            // intermediate, and the uncovered region composites transparent
            blit_pass.set_viewport(0.0, 0.0, self.size.0 as f32, self.size.1 as f32, 0.0, 1.0);
            blit_pass.set_pipeline(&blit.pipeline);
            blit_pass.set_bind_group(0, &msaa.bind_group, &[]);
            blit_pass.draw(0..3, 0..1);
        } else {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewer mesh pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(depth_stencil),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            set_vp(&mut pass);
            self.draw_scene(&mut pass);
        }
    }

    /// Part draws — one bind group + vertex buffer + draw per DRAWN part
    /// (the last prepared frame's visible slots; hidden parts are absent):
    /// the opaque set first (filled triangles or the wireframe hairline
    /// list; a collision part among them colors flat red through the
    /// shader flag), then the translucent set through the blend pipelines
    /// (same topology choice, alpha from the model uniform) — then, when
    /// the camera sections, the cap quad, then the compare ghost, and last
    /// the unlit overlays (grid, then measure markers) — into the pass's
    /// color attachment (msaa or single-sample alike: everything here
    /// inherits the pass attachment and matches its sample count).
    fn draw_scene(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.bind_group, &[]);
        if self.wireframe {
            // facts #44: same material and opacity as fill, hairline list
            pass.set_pipeline(&self.line_pipeline);
            for &i in &self.drawn {
                let resident = &self.residents[i];
                pass.set_bind_group(1, &resident.model_bg, &[]);
                pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                pass.set_index_buffer(resident.edge_buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..resident.edge_count, 0, 0..1);
            }
        } else {
            pass.set_pipeline(&self.pipeline);
            for &i in &self.drawn {
                let resident = &self.residents[i];
                pass.set_bind_group(1, &resident.model_bg, &[]);
                pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                pass.draw(0..resident.vertex_count, 0..1);
            }
        }
        // The translucent set after every opaque part, before the cap: it
        // blends over what is already drawn and must not occlude anything
        // after it (no depth write) — a collision part never lands here
        // (opaque by partition), so the flat-red branch never blends.
        if !self.drawn_translucent.is_empty() {
            if self.wireframe {
                pass.set_pipeline(&self.line_blend_pipeline);
                for &i in &self.drawn_translucent {
                    let resident = &self.residents[i];
                    pass.set_bind_group(1, &resident.model_bg, &[]);
                    pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                    pass.set_index_buffer(resident.edge_buf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..resident.edge_count, 0, 0..1);
                }
            } else {
                pass.set_pipeline(&self.mesh_blend_pipeline);
                for &i in &self.drawn_translucent {
                    let resident = &self.residents[i];
                    pass.set_bind_group(1, &resident.model_bg, &[]);
                    pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                    pass.draw(0..resident.vertex_count, 0..1);
                }
            }
        }
        if self.draw_cap {
            // Same pass, after the mesh: the stencil attachment already holds
            // the parity the mesh's inc/dec left, so the quad only survives
            // where the cut opened the solid.
            pass.set_pipeline(&self.cap_pipeline);
            pass.set_stencil_reference(0);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.cap_buf.slice(..));
            pass.draw(0..4, 0..1);
        }
        // The compare ghost between the cap and the overlays: depth-tested
        // against the opaque mesh (a ghost behind the model hides) but never
        // writing depth, so it cannot occlude the grid or markers after it.
        // Drawn in wireframe mode too — upstream's ghost is independent of
        // the wireframe toggle (facts #44/#45 disable the interior shell,
        // not the ghost).
        if !self.ghost_residents.is_empty() {
            pass.set_pipeline(&self.ghost_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            for resident in &self.ghost_residents {
                pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                pass.set_index_buffer(resident.edge_buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..resident.edge_count, 0, 0..1);
            }
        }
        // Overlays last, painter-ordered (they write no depth): the grid,
        // then the measure markers after it — both unlit vertex-colored,
        // depth-tested against the parts so the model occludes them, but
        // never writing depth, so they never occlude what follows.
        for overlay in [self.grid.as_ref(), self.marker.as_ref()]
            .into_iter()
            .flatten()
        {
            pass.set_pipeline(&self.grid_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, overlay.vertex_buf.slice(..));
            pass.draw(0..overlay.vertex_count, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use engine::parsing::stl::parse_stl;
    use engine::parsing::threemf::parse_3mf;

    fn fixture(rel: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/golden")
            .join(rel)
    }

    fn golden_stl_parts() -> Vec<engine::parsing::Part> {
        let bytes = std::fs::read(fixture("model.stl"))
            .unwrap_or_else(|e| panic!("read fixture model.stl: {e}"));
        vec![parse_stl(&bytes).expect("golden model.stl parses")]
    }

    fn golden_3mf_parts() -> Vec<engine::parsing::Part> {
        let bytes = std::fs::read(fixture("model.3mf"))
            .unwrap_or_else(|e| panic!("read fixture model.3mf: {e}"));
        parse_3mf(&bytes).expect("golden model.3mf parses: 2 parts x 12 triangles")
    }

    #[test]
    fn from_parts_remaps_and_pins_floor_at_origin() {
        let part = engine::parsing::Part {
            name: "tri".into(),
            positions: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            triangle_colors: vec![None],
        };
        let scene = PartScene::from_parts(std::slice::from_ref(&part), NEUTRAL);
        // upstream sceneVector (x, z, -y) maps the MODEL triangle to
        // x[1,7] y[3,9] z[-8,-2]; the pin then translates by
        // (-center.x, -min.y, -center.z) = (-4, -3, +5): floor at scene
        // y=0, x/z centered (ModelViewer.swift:167-168)
        assert_eq!(scene.parts[0].vertices[0].pos, [-3.0, 0.0, 3.0]);
        assert_eq!(scene.parts[0].vertices[2].pos, [3.0, 6.0, -3.0]);
        // neutral painted, per-part aabb is the pinned triangle's
        assert_eq!(scene.parts[0].vertices[0].color, NEUTRAL);
        assert_eq!(scene.parts[0].aabb_min, [-3.0, 0.0, -3.0]);
        assert_eq!(scene.parts[0].aabb_max, [3.0, 6.0, 3.0]);
        // the scene aabb is the pinned union: y from 0, x/z symmetric
        assert_eq!(scene.aabb_min, [-3.0, 0.0, -3.0]);
        assert_eq!(scene.aabb_max, [3.0, 6.0, 3.0]);
    }

    #[test]
    fn golden_stl_scene_is_24_triangles_inside_its_aabb() {
        let scene = PartScene::from_parts(&golden_stl_parts(), NEUTRAL);
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].vertices.len(), 24 * 3);
        for v in &scene.parts[0].vertices {
            for a in 0..3 {
                assert!(v.pos[a] >= scene.aabb_min[a] - 1e-6);
                assert!(v.pos[a] <= scene.aabb_max[a] + 1e-6);
            }
        }
        for a in 0..3 {
            assert!(scene.aabb_max[a] > scene.aabb_min[a]);
        }
    }

    #[test]
    fn golden_3mf_scene_is_two_parts_of_twelve_triangles() {
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        assert_eq!(scene.parts.len(), 2, "one GpuPart per engine part");
        for part in &scene.parts {
            assert_eq!(part.vertices.len(), 12 * 3);
            for a in 0..3 {
                assert!(part.aabb_max[a] > part.aabb_min[a]);
                assert!(part.aabb_min[a] >= scene.aabb_min[a] - 1e-6);
                assert!(part.aabb_max[a] <= scene.aabb_max[a] + 1e-6);
            }
        }
        // fixture: two boxes, base 60x60 z in [-4,4] and lid 40x40 z in
        // [12,22] — MODEL x/y/z spans 60/60/26 become SCENE x/y/z 60/26/60
        let span = |a: usize| scene.aabb_max[a] - scene.aabb_min[a];
        assert!((span(0) - 60.0).abs() < 1e-3, "scene x span {}", span(0));
        assert!((span(1) - 26.0).abs() < 1e-3, "scene y span {}", span(1));
        assert!((span(2) - 60.0).abs() < 1e-3, "scene z span {}", span(2));
        // post-pin the union aabb itself: MODEL floor (z=-4) at scene y=0,
        // x/z centered on the scene origin, spans unchanged
        let pinned = [
            (scene.aabb_min[0], scene.aabb_max[0], -30.0, 30.0),
            (scene.aabb_min[1], scene.aabb_max[1], 0.0, 26.0),
            (scene.aabb_min[2], scene.aabb_max[2], -30.0, 30.0),
        ];
        for (a, &(mn, mx, emn, emx)) in pinned.iter().enumerate() {
            assert!(
                (mn - emn).abs() < 1e-3 && (mx - emx).abs() < 1e-3,
                "pinned aabb[{a}] [{mn}, {mx}] vs [{emn}, {emx}]"
            );
        }
    }

    #[test]
    fn frame_radius_is_diagonal_half() {
        // the long-thin regression: max-extent/2 would frame at 5.0, the
        // diagonal at √102/2 ≈ 5.05 (facts #1: ModelViewer.swift:328-337)
        let (min, max) = ([0.0, 0.0, 0.0], [10.0, 1.0, 1.0]);
        let r = f32::sqrt(102.0) / 2.0;
        assert!((camera::framing_radius(min, max) - r).abs() < 1e-5);
        let (target, distance) = camera::frame(min, max);
        // aspect-free: half the height; x/z zero assumes the pinned scene
        assert_eq!(target, [0.0, 0.5, 0.0]);
        let expected = r / f32::sin(17.5f32.to_radians()) * 1.15;
        assert!(
            (distance - expected).abs() < 1e-3,
            "distance {distance} vs r/sin(17.5°)·1.15 = {expected}"
        );
        // the radius floors at 1 so a degenerate model still frames
        assert_eq!(camera::framing_radius([0.0; 3], [0.0; 3]), 1.0);
    }

    #[test]
    fn zoom_clamps_and_presets() {
        // zoom: distance × factor clamped to [0.1r, 200r] (facts #19)
        assert_eq!(camera::zoomed(50.0, 0.5, 10.0), 25.0);
        assert_eq!(camera::zoomed(50.0, 0.001, 10.0), 1.0, "floor 0.1r");
        assert_eq!(camera::zoomed(50.0, 100.0, 10.0), 2000.0, "ceiling 200r");
        // pitch clamp is upstream's exact constant, not π/2 − ε (facts #18)
        assert_eq!(camera::MAX_PITCH, 1.5607);
        assert!(camera::MAX_PITCH < std::f32::consts::FRAC_PI_2);
        // drag: yaw -= dx·0.008, pitch += dy·0.008, clamped at ±MAX_PITCH
        let (yaw, pitch) = camera::orbit_drag(0.3, 0.2, 100.0, 25.0);
        assert!((yaw + 0.5).abs() < 1e-6, "yaw {yaw}");
        assert!((pitch - 0.4).abs() < 1e-6, "pitch {pitch}");
        assert_eq!(camera::orbit_drag(0.0, 1.5, 0.0, 1000.0).1, 1.5607);
        assert_eq!(camera::orbit_drag(0.0, -1.5, 0.0, -1000.0).1, -1.5607);
        // presets (facts #25): top reuses the maximumPitch constant
        assert_eq!(
            camera::Preset::Iso.yaw_pitch(),
            (std::f32::consts::FRAC_PI_4, 0.6)
        );
        assert_eq!(camera::Preset::Front.yaw_pitch(), (0.0, 0.0));
        assert_eq!(camera::Preset::Top.yaw_pitch(), (0.0, camera::MAX_PITCH));
        assert_eq!(
            camera::Preset::Right.yaw_pitch(),
            (std::f32::consts::FRAC_PI_2, 0.0)
        );
        // pan: world units per view point at `distance` (facts #22)
        let upp = camera::pan_units_per_point(100.0, 500.0);
        let expected = 2.0 * 100.0 * f32::tan(17.5f32.to_radians()) / 500.0;
        assert!((upp - expected).abs() < 1e-6, "{upp} vs {expected}");
    }

    /// The default must be the single-sample count, not `u32::default()`
    /// (0): that differs from a fresh pipeline's resident 1 and would
    /// drive the first default-options prepare into an invalid 0-sample
    /// rebuild — the derive(Default) bug the manual impl replaced.
    #[test]
    fn options_default_is_single_sample() {
        assert_eq!(Options::default().msaa, 1);
        assert!(!Options::default().wireframe);
    }

    /// pan (facts #22): the target moves by (-right·dx + up·dy)·units/pt
    /// over the camera's own scene axes — verified against the look-at
    /// basis glam builds for the same pose (rows 0/1 of the view matrix's
    /// rotation block are the camera's right/up in world space). Dragging
    /// right (dx > 0) moves the target LEFT, so the model follows the
    /// cursor; a non-positive view height leaves the target alone, as the
    /// upstream guard does.
    #[test]
    fn pan_matches_lookat_basis() {
        let target = [3.0, -4.0, 5.0];
        let (yaw, pitch, distance) = (0.7f32, 0.3f32, 120.0f32);
        let (dx, dy, view_height) = (25.0f32, -10.0f32, 500.0f32);
        let panned = camera::panned(target, yaw, pitch, distance, view_height, dx, dy);
        let at = glam::Vec3::from_array(target);
        let back = glam::Vec3::new(
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        );
        let view = glam::camera::rh::view::look_at_mat4(at + back * distance, at, glam::Vec3::Y);
        // col-major glam: matrix rows 0/1 are the camera right/up axes
        let right = glam::Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x);
        let up = glam::Vec3::new(view.x_axis.y, view.y_axis.y, view.z_axis.y);
        let upp = camera::pan_units_per_point(distance, view_height);
        let expected = at + (right * -dx + up * dy) * upp;
        for a in 0..3 {
            assert!(
                (panned[a] - expected[a]).abs() < 1e-4,
                "panned {panned:?} vs look-at basis {expected:?}"
            );
        }
        // the model follows the cursor: dx > 0 shifts the target against
        // the camera's right axis
        assert!(glam::Vec3::from_array(panned).dot(right) < at.dot(right));
        assert_eq!(
            camera::panned(target, yaw, pitch, distance, 0.0, dx, dy),
            target,
            "zero view height must not pan"
        );
    }

    /// The ported orbit test: the view_proj is exactly the Swift
    /// placeCamera formula (facts #1: ModelViewer.swift:339-342), the
    /// orbit target sits at clip-space (0, 0) for every pose, the aspect
    /// argument is load-bearing, and extreme poses stay finite.
    #[test]
    fn orbit_view_proj_matches_formula() {
        // arbitrary off-center target: the formula must not assume pinning
        let target = [3.0, -4.0, 5.0];
        let aspect = 800.0 / 600.0;
        for &(yaw, pitch, distance) in &[
            (0.7f32, 0.3f32, 120.0f32),
            (0.0, 0.0, 1.0),
            (-2.5, camera::MAX_PITCH, 90_000.0),
            (std::f32::consts::PI, -camera::MAX_PITCH, 250.0),
        ] {
            let vp = camera::orbit(distance, yaw, pitch, target, aspect);
            let dir = glam::Vec3::new(
                pitch.cos() * yaw.sin(),
                pitch.sin(),
                pitch.cos() * yaw.cos(),
            );
            let at = glam::Vec3::from_array(target);
            let view = glam::camera::rh::view::look_at_mat4(at + dir * distance, at, glam::Vec3::Y);
            let proj = glam::camera::rh::proj::directx::perspective(
                35f32.to_radians(),
                aspect,
                1.0,
                100_000.0,
            );
            let expected = (proj * view).to_cols_array_2d();
            for (row, erow) in vp.iter().zip(expected.iter()) {
                for (a, b) in row.iter().zip(erow.iter()) {
                    assert!((a - b).abs() < 1e-4, "{vp:?} vs {expected:?}");
                }
            }
            // the target is the look-at point: it projects to (0, 0)
            let clip = glam::Mat4::from_cols_array_2d(&vp)
                * glam::Vec4::new(target[0], target[1], target[2], 1.0);
            assert!(clip.x.abs() < 1e-3 && clip.y.abs() < 1e-3, "{clip:?}");
            for row in vp {
                for x in row {
                    assert!(x.is_finite());
                }
            }
            // aspect is load-bearing: a 16:9 projection differs from 4:3 —
            // the hardcode this task kills would render both identically
            assert_ne!(vp, camera::orbit(distance, yaw, pitch, target, 16.0 / 9.0));
        }
    }

    /// The Frame every upload-once test shares: default options, no overlay
    /// or markers, effective_aabb at the staged scene's own (T10 replaces
    /// that input).
    /// The pick ray aims where the clicked pixel looks: center exactly along
    /// the view axis at the target, the screen edges tilting with the
    /// camera's own right/up axes. The y-flip (facts #29) is the load-bearing
    /// convention here — iced's cursor is top-down, upstream's AppKit point
    /// is bottom-up, so `h − cy` must happen BEFORE the NDC mapping or every
    /// vertical drag picks mirrored.
    #[test]
    fn pick_ray_center_points_at_the_target() {
        // yaw 0, pitch 0: eye = target + dist*(0,0,1); center pixel aims at f
        let (o, d) = camera::pick_ray(
            100.0,
            0.0,
            0.0,
            [0.0, 5.0, 0.0],
            4.0 / 3.0,
            (400.0, 300.0),
            (800.0, 600.0),
        );
        assert!(
            (o[0] - 0.0).abs() < 1e-4 && (o[1] - 5.0).abs() < 1e-4 && (o[2] - 100.0).abs() < 1e-4
        );
        let dot = glam::Vec3::from_array(d).dot(glam::Vec3::new(0.0, 0.0, -1.0));
        assert!(dot > 0.999, "center ray along the view axis: {d:?}");
        // right edge (x=799) tilts toward +x; BOTTOM-left corner tilts -x -y
        // (iced y is top-down: cursor y 599 flips to AppKit y 1 = bottom,
        // which aims DOWN in any bottom-up convention)
        let (_, d2) = camera::pick_ray(
            100.0,
            0.0,
            0.0,
            [0.0, 5.0, 0.0],
            4.0 / 3.0,
            (799.0, 300.0),
            (800.0, 600.0),
        );
        assert!(d2[0] > 0.1, "right edge aims right: {d2:?}");
        let (_, d3) = camera::pick_ray(
            100.0,
            0.0,
            0.0,
            [0.0, 5.0, 0.0],
            4.0 / 3.0,
            (0.0, 599.0),
            (800.0, 600.0),
        );
        assert!(
            d3[0] < 0.0 && d3[1] < 0.0,
            "bottom-left aims left and DOWN (iced bottom = AppKit bottom after the flip): {d3:?}"
        );
    }

    /// The Frame every upload-once test shares: default options, no overlay,
    /// effective_aabb at the staged scene's own (T10 replaces that input).
    fn frame<'a>(
        camera: &'a Camera,
        scene: &'a PartScene,
        poses: &'a [PartPose],
        appearances: &'a [PartAppearance],
    ) -> Frame<'a> {
        frame_ghost(camera, scene, poses, appearances, None)
    }

    /// [`frame`] plus the compare ghost scene.
    fn frame_ghost<'a>(
        camera: &'a Camera,
        scene: &'a PartScene,
        poses: &'a [PartPose],
        appearances: &'a [PartAppearance],
        ghost: Option<&'a PartScene>,
    ) -> Frame<'a> {
        Frame {
            camera,
            scene,
            poses,
            appearances,
            effective_aabb: (scene.aabb_min, scene.aabb_max),
            grid: None,
            markers: None,
            ghost,
            options: Options::default(),
        }
    }

    /// Identity poses for every slot of `scene` — the assembled default.
    fn identity_poses(scene: &PartScene) -> Vec<PartPose> {
        vec![Some(glam::Mat4::IDENTITY.to_cols_array_2d()); scene.parts.len()]
    }

    /// Opaque default appearances for every slot of `scene`.
    fn defaults(scene: &PartScene) -> Vec<PartAppearance> {
        vec![PartAppearance::default(); scene.parts.len()]
    }

    #[test]
    fn upload_once_second_frame_no_reupload() {
        let (device, queue) = gpu::device_queue();
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        let camera = default_camera(&scene, 4.0 / 3.0);
        let poses = identity_poses(&scene);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene, &poses, &defaults(&scene)),
        );
        let (u1, a1) = (
            pipeline.upload_count_for_tests(),
            pipeline.alloc_count_for_tests(),
        );
        assert_eq!((u1, a1), (2, 2), "one upload+alloc per part on residency");
        // second frame, same scene, MOVED camera: mesh bytes must not move
        let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
        let moved = Camera::orbiting(distance * 2.0, 0.9, 0.4, target, 4.0 / 3.0);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&moved, &scene, &poses, &defaults(&scene)),
        );
        assert_eq!(
            pipeline.upload_count_for_tests(),
            u1,
            "unchanged scene must not reupload on frame 2"
        );
        assert_eq!(
            pipeline.alloc_count_for_tests(),
            a1,
            "unchanged scene must not reallocate on frame 2"
        );
    }

    #[test]
    fn new_scene_reuploads() {
        let (device, queue) = gpu::device_queue();
        let mut parts = golden_3mf_parts();
        let scene1 = PartScene::from_parts(&parts, NEUTRAL);
        let camera = default_camera(&scene1, 4.0 / 3.0);
        let poses = identity_poses(&scene1);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene1, &poses, &defaults(&scene1)),
        );
        let (u1, a1) = (
            pipeline.upload_count_for_tests(),
            pipeline.alloc_count_for_tests(),
        );
        // grow one part by a triangle, INSIDE the union aabb so the floor
        // pin does not move — isolating per-part identity (the pin-shift
        // case is pinned by pin_shift_reuploads_every_part below)
        for corner in [[10.0, 0.0, 16.0], [11.0, 0.0, 16.0], [10.0, 1.0, 16.0]] {
            parts[1].positions.push(corner);
        }
        parts[1].triangle_colors.push(None);
        let scene2 = PartScene::from_parts(&parts, NEUTRAL);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene2, &poses, &defaults(&scene2)),
        );
        assert_eq!(
            pipeline.upload_count_for_tests(),
            u1 + 1,
            "grown part reuploads; the untouched part does not"
        );
        assert_eq!(pipeline.alloc_count_for_tests(), a1 + 1);
    }

    #[test]
    fn middle_vertex_mutation_reuploads() {
        let (device, queue) = gpu::device_queue();
        let mut parts = golden_3mf_parts();
        let scene1 = PartScene::from_parts(&parts, NEUTRAL);
        let camera = default_camera(&scene1, 4.0 / 3.0);
        let poses = identity_poses(&scene1);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene1, &poses, &defaults(&scene1)),
        );
        let (u1, a1) = (
            pipeline.upload_count_for_tests(),
            pipeline.alloc_count_for_tests(),
        );
        // move a middle corner INWARD (fixture corner is aabb-edge
        // (30,30,4); +5 would move the union aabb and the pin with it):
        // same count, same first/last vertex — only full-content identity
        // can notice
        let n = parts[0].positions.len();
        parts[0].positions[n / 2][0] -= 5.0;
        let scene2 = PartScene::from_parts(&parts, NEUTRAL);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene2, &poses, &defaults(&scene2)),
        );
        assert_eq!(
            pipeline.upload_count_for_tests(),
            u1 + 1,
            "a moved middle vertex must reupload its part (full-content hash)"
        );
        assert_eq!(pipeline.alloc_count_for_tests(), a1 + 1);
    }

    /// T1-review carry, resolved: the content hash covers the STAGED vertex
    /// bytes and `from_parts` paints `neutral` into the color field, so two
    /// scenes differing only in neutral already differ in staged content —
    /// no separate hash input is needed. This test is the proof, and the
    /// pin against a future hash that stops covering color bytes.
    #[test]
    fn neutral_change_reuploads() {
        let (device, queue) = gpu::device_queue();
        // the STL is the all-None fixture: every triangle paints neutral,
        // so the neutral is load-bearing in the staged bytes
        let parts = golden_stl_parts();
        assert!(parts[0].triangle_colors.iter().all(|c| c.is_none()));
        let scene1 = PartScene::from_parts(&parts, NEUTRAL);
        let camera = default_camera(&scene1, 4.0 / 3.0);
        let poses = identity_poses(&scene1);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene1, &poses, &defaults(&scene1)),
        );
        let u1 = pipeline.upload_count_for_tests();
        // facts #43's light-mode neutral: same geometry, only the paint
        // changes — a skipped upload would serve the dark-mode colors
        let scene2 = PartScene::from_parts(&parts, [0.55, 0.64, 0.75]);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene2, &poses, &defaults(&scene2)),
        );
        assert_eq!(
            pipeline.upload_count_for_tests(),
            u1 + 1,
            "a neutral change must reupload — stale colors otherwise"
        );
    }

    /// The sRGB EOTF is the exact IEC piecewise, not a single power: the
    /// linear segment below 0.04045 and the 2.4 segment above differ enough
    /// that a 2.2-only conversion is visibly wrong on dark channels.
    #[test]
    fn srgb_conversion_table() {
        let f = parts::srgb_to_linear;
        assert_eq!(f(0), 0.0);
        assert!((f(255) - 1.0).abs() < 1e-6);
        assert!((f(128) - 0.216).abs() < 5e-4, "128 -> {}", f(128));
        // the dark discriminator — the base part's own blue R channel: a
        // 2.2 power reads 0.0097 here, 29% low, and fails this assert
        assert!((f(31) - 0.0137).abs() < 2e-4, "31 -> {}", f(31));
        // the linear segment: 10/255 = 0.0392 stays below 0.04045, so
        // /12.92 — a power-only formula reads 0.0008, 3.8x low
        assert!((f(10) - 0.003035).abs() < 2e-5, "10 -> {}", f(10));
        // continuity across the segment boundary (10 linear, 11 power)
        assert!(f(10) < f(11) && f(11) < f(12));
    }

    /// Per-triangle colors stage per corner: `Some` goes through the sRGB
    /// EOTF onto all 3 corners, `None` paints the neutral. Pinned against
    /// the golden fixture's actual bytes so a parse regression cannot pass.
    #[test]
    fn colored_triangles_paint_linear_corners() {
        let parts = golden_3mf_parts();
        // fixture truth (model.3mf basematerials): base #1F78B5, lid #D62629
        assert_eq!(parts[0].name, "base");
        assert_eq!(parts[0].triangle_colors, vec![Some([31, 120, 181]); 12]);
        assert_eq!(parts[1].name, "lid");
        assert_eq!(parts[1].triangle_colors, vec![Some([214, 38, 41]); 12]);
        let scene = PartScene::from_parts(&parts, NEUTRAL);
        let linear = |c: [u8; 3]| {
            [
                parts::srgb_to_linear(c[0]),
                parts::srgb_to_linear(c[1]),
                parts::srgb_to_linear(c[2]),
            ]
        };
        let (blue, red) = (linear([31, 120, 181]), linear([214, 38, 41]));
        assert!(blue[0] < 0.02 && blue[2] > 0.45, "dark-R blue: {blue:?}");
        for v in &scene.parts[0].vertices {
            assert_eq!(v.color, blue, "base corner painted its triangle's color");
        }
        for v in &scene.parts[1].vertices {
            assert_eq!(v.color, red, "lid corner painted its triangle's color");
        }
        // mixed part: Some -> linear red exactly; None -> neutral
        let mixed = engine::parsing::Part {
            name: "mixed".into(),
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
            triangle_colors: vec![Some([255, 0, 0]), None],
        };
        let scene = PartScene::from_parts(std::slice::from_ref(&mixed), NEUTRAL);
        for v in &scene.parts[0].vertices[0..3] {
            assert_eq!(
                v.color,
                [1.0, 0.0, 0.0],
                "Some([255,0,0]) is exact linear red"
            );
        }
        for v in &scene.parts[0].vertices[3..6] {
            assert_eq!(v.color, NEUTRAL, "None paints the neutral");
        }
    }

    /// Task 4: every staged triangle is flat-shaded with its own face
    /// normal — all 3 corners carry normalize(cross(b−a, c−a)) of the
    /// STAGED (remapped, floor-pinned) positions, recomputed here
    /// independently of the staging code. A zero-area triangle falls back
    /// to (0,0,1) (upstream TriangleMesh.swift:21-23). The 1e-4 tolerance
    /// is float headroom only: a wrong normal (wrong corner order, MODEL
    /// instead of scene space, unnormalized) is O(1) off.
    #[test]
    fn normals_match_cross_product() {
        let scenes = [
            PartScene::from_parts(&golden_3mf_parts(), NEUTRAL),
            PartScene::from_parts(&golden_stl_parts(), NEUTRAL),
        ];
        for scene in &scenes {
            for part in &scene.parts {
                for tri in part.vertices.chunks_exact(3) {
                    let (a, b, c) = (
                        glam::Vec3::from_array(tri[0].pos),
                        glam::Vec3::from_array(tri[1].pos),
                        glam::Vec3::from_array(tri[2].pos),
                    );
                    let cross = (b - a).cross(c - a);
                    let len = cross.length();
                    assert!(len > 0.0, "fixture triangles are non-degenerate");
                    let n = (cross / len).to_array();
                    for v in tri {
                        for axis in 0..3 {
                            assert!(
                                (v.normal[axis] - n[axis]).abs() < 1e-4,
                                "corner normal {:?} != face normal {n:?}",
                                v.normal
                            );
                        }
                        let l = v.normal.iter().map(|x| x * x).sum::<f32>().sqrt();
                        assert!((l - 1.0).abs() < 1e-4, "unit length, got {l}");
                    }
                }
            }
        }
        // degenerate: a zero-area triangle stages the (0,0,1) fallback on
        // all 3 corners — the brief's literal constant, matching upstream's
        // addTriangle fallback shape (unobservable: nothing rasterizes)
        let part = engine::parsing::Part {
            name: "degenerate".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [2.0, 2.0, 2.0]],
            triangle_colors: vec![None],
        };
        let scene = PartScene::from_parts(std::slice::from_ref(&part), NEUTRAL);
        for v in &scene.parts[0].vertices {
            assert_eq!(v.normal, [0.0, 0.0, 1.0]);
        }
    }

    /// The floor pin is baked into the staged vertices, so content identity
    /// must track them: a part growing past the union aabb moves the pin,
    /// and every part's scene-space content changes with it — revert the
    /// identity to raw MODEL positions and this test catches parts left
    /// resident at the stale pin.
    #[test]
    fn pin_shift_reuploads_every_part() {
        let (device, queue) = gpu::device_queue();
        let mut parts = golden_3mf_parts();
        let scene1 = PartScene::from_parts(&parts, NEUTRAL);
        let camera = default_camera(&scene1, 4.0 / 3.0);
        let poses = identity_poses(&scene1);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene1, &poses, &defaults(&scene1)),
        );
        let u1 = pipeline.upload_count_for_tests();
        // grow past MODEL x=30: union widens, the pin's x-center shifts
        for corner in [[40.0, 0.0, 16.0], [41.0, 0.0, 16.0], [40.0, 1.0, 16.0]] {
            parts[1].positions.push(corner);
        }
        parts[1].triangle_colors.push(None);
        let scene2 = PartScene::from_parts(&parts, NEUTRAL);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene2, &poses, &defaults(&scene2)),
        );
        assert_eq!(
            pipeline.upload_count_for_tests(),
            u1 + 2,
            "a moved pin re-renders every part at the new offset"
        );
    }

    /// The GPU-first transform law: a pose change moves ink through the
    /// model uniform alone — zero vertex uploads (the M3 behavior this
    /// task retires restaged the whole part).
    #[test]
    fn pose_change_moves_ink_without_uploads() {
        let (device, queue) = gpu::device_queue();
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        let camera = default_camera(&scene, 4.0 / 3.0);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let poses = identity_poses(&scene);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene, &poses, &defaults(&scene)),
        );
        let uploads = pipeline.upload_count_for_tests();
        // sweep the lid 40mm along +x through the POSE path alone
        let mut moved = poses.clone();
        moved[1] =
            Some(glam::Mat4::from_translation(glam::Vec3::new(40.0, 0.0, 0.0)).to_cols_array_2d());
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene, &moved, &defaults(&scene)),
        );
        assert_eq!(
            pipeline.upload_count_for_tests(),
            uploads,
            "pose-only change uploads zero vertices"
        );
        assert_eq!(pipeline.drawn_part_count_for_tests(), 2);
    }

    /// `None` hides a part at draw with no residency change — visibility is
    /// a pose, not a content edit.
    #[test]
    fn hidden_pose_skips_draw_without_reupload() {
        let (device, queue) = gpu::device_queue();
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        let camera = default_camera(&scene, 4.0 / 3.0);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let poses = identity_poses(&scene);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene, &poses, &defaults(&scene)),
        );
        let uploads = pipeline.upload_count_for_tests();
        let hidden: Vec<PartPose> = vec![None, Some(glam::Mat4::IDENTITY.to_cols_array_2d())];
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene, &hidden, &defaults(&scene)),
        );
        assert_eq!(pipeline.drawn_part_count_for_tests(), 1);
        assert_eq!(
            pipeline.upload_count_for_tests(),
            uploads,
            "hiding touches no residency"
        );
    }

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
        let looks = defaults(&scene);
        let mut uploads_after_first = None;
        for step in 0..24u32 {
            let poses: Vec<PartPose> = vec![
                Some(id),
                Some(
                    glam::Mat4::from_translation(glam::Vec3::new(0.0, step as f32 * 0.5, 0.0))
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
                ..frame(&camera, &scene, &poses, &looks)
            };
            pipeline.prepare_parts(device, queue, (800, 600), &f);
            if step == 0 {
                uploads_after_first = Some(pipeline.upload_count_for_tests());
            }
            assert_eq!(
                pipeline.drawn_part_count_for_tests(),
                2,
                "a translucent part is still drawn (step {step})"
            );
        }
        assert_eq!(
            pipeline.upload_count_for_tests(),
            uploads_after_first.unwrap(),
            "24 playback frames: zero vertex uploads"
        );
    }

    /// A 10mm axis-aligned cube as 6 quads -> 12 triangles, corners drawn
    /// from a shared 8-entry table so identical positions repeat across
    /// faces exactly like real unindexed engine parts.
    fn cube_part() -> engine::parsing::Part {
        let c = |x: f32, y: f32, z: f32| [10.0 * x, 10.0 * y, 10.0 * z];
        let v = [
            c(0.0, 0.0, 0.0),
            c(1.0, 0.0, 0.0),
            c(1.0, 1.0, 0.0),
            c(0.0, 1.0, 0.0),
            c(0.0, 0.0, 1.0),
            c(1.0, 0.0, 1.0),
            c(1.0, 1.0, 1.0),
            c(0.0, 1.0, 1.0),
        ];
        let faces = [
            (0usize, 3, 2, 1),
            (4, 5, 6, 7),
            (0, 1, 5, 4),
            (1, 2, 6, 5),
            (2, 3, 7, 6),
            (3, 0, 4, 7),
        ];
        let mut positions = Vec::new();
        for &(a, b, c2, d) in &faces {
            positions.extend_from_slice(&[v[a], v[b], v[c2], v[a], v[c2], v[d]]);
        }
        engine::parsing::Part {
            name: "cube".into(),
            positions,
            triangle_colors: vec![None; 12],
        }
    }

    /// Wireframe edges (facts #44): upstream's `.lines` fill mode
    /// rasterizes every triangle edge, so a cube triangulated as 6 quads
    /// shows its 12 polygon edges PLUS the 6 face diagonals = 18; dedup
    /// collapses only the shared edges the unindexed soup repeats
    /// (36 per-triangle corner pairs). The line pass draws that list, one
    /// hairline per edge: 18 edges x 2 endpoints = 36 vertices.
    #[test]
    fn wireframe_edge_count() {
        let scene = PartScene::from_parts(std::slice::from_ref(&cube_part()), NEUTRAL);
        assert_eq!(
            scene.parts[0].vertices.len(),
            36,
            "fixture truth: 12 triangles x 3 corners"
        );
        assert_eq!(
            scene.parts[0].edges.len(),
            18,
            "12 polygon edges + 6 face diagonals, deduped"
        );
        for &[a, b] in &scene.parts[0].edges {
            assert!(
                a != b && a < 36 && b < 36,
                "edge indices address staged corners"
            );
        }
        let (device, queue) = gpu::device_queue();
        let camera = default_camera(&scene, 1.0);
        let poses = identity_poses(&scene);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(
            device,
            queue,
            (64, 64),
            &Frame {
                options: Options {
                    wireframe: true,
                    ..Options::default()
                },
                ..frame(&camera, &scene, &poses, &defaults(&scene))
            },
        );
        assert_eq!(
            pipeline.line_vertex_count_for_tests(),
            36,
            "the line pass draws 18 edges x 2 endpoints"
        );
    }

    /// The f32 EOTF is the exact core the u8 form delegates to: every u8
    /// value through `srgb_to_linear_f(c / 255)` equals `srgb_to_linear(c)`,
    /// so downstream f32 callers (T3's cap color, T6's marker orange) sit on
    /// the same curve the staging paints with.
    #[test]
    fn srgb_to_linear_f_is_the_u8_forms_core() {
        for c in 0..=255u8 {
            assert_eq!(
                parts::srgb_to_linear_f(c as f32 / 255.0),
                parts::srgb_to_linear(c),
                "u8 {c}"
            );
        }
        // the segment boundary is <= on the linear side
        assert_eq!(parts::srgb_to_linear_f(0.04045), 0.04045 / 12.92);
        assert!(parts::srgb_to_linear_f(0.04046) > parts::srgb_to_linear_f(0.04045));
        // out-of-range inputs keep the formula's shape (callers clamp)
        assert_eq!(parts::srgb_to_linear_f(0.0), 0.0);
        assert!((parts::srgb_to_linear_f(1.0) - 1.0).abs() < 1e-6);
    }

    /// Ghost residency (the GPU-first law's ghost clause): the whole ghost
    /// scene restages only when its content identity changes — the same
    /// ghost under a moved camera uploads zero ghost bytes, a dropped ghost
    /// (compare turned off) uploads none, and a compare-version swap
    /// restages every part. Upstream keys this on ghostID
    /// (ModelViewer.swift:171-174); the port keys it on the ghost scene's
    /// whole-content identity.
    #[test]
    fn ghost_reuploads_only_on_content_change() {
        let (device, queue) = gpu::device_queue();
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        let camera = default_camera(&scene, 4.0 / 3.0);
        let poses = identity_poses(&scene);
        let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let shifted = |dx: f32| {
            let mut parts = golden_3mf_parts();
            for part in &mut parts {
                for p in &mut part.positions {
                    p[0] += dx;
                }
            }
            PartScene::from_parts_with_pin(&parts, NEUTRAL, scene.pin)
        };
        let ghost = shifted(40.0);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame_ghost(&camera, &scene, &poses, &defaults(&scene), Some(&ghost)),
        );
        let u1 = pipeline.ghost_upload_count_for_tests();
        assert_eq!(u1, 2, "one upload per ghost part on residency");
        // moved camera, same ghost: zero ghost bytes
        let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
        let moved = Camera::orbiting(distance * 2.0, 0.9, 0.4, target, 4.0 / 3.0);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame_ghost(&moved, &scene, &poses, &defaults(&scene), Some(&ghost)),
        );
        assert_eq!(
            pipeline.ghost_upload_count_for_tests(),
            u1,
            "an unchanged ghost must not reupload on a moved camera"
        );
        // compare off: the slot empties without restaging
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame(&camera, &scene, &poses, &defaults(&scene)),
        );
        assert_eq!(
            pipeline.ghost_upload_count_for_tests(),
            u1,
            "dropping the ghost uploads nothing"
        );
        // a compare-version swap restages wholesale
        let other = shifted(80.0);
        pipeline.prepare_parts(
            device,
            queue,
            (800, 600),
            &frame_ghost(&camera, &scene, &poses, &defaults(&scene), Some(&other)),
        );
        assert!(
            pipeline.ghost_upload_count_for_tests() > u1,
            "new ghost content restages"
        );
    }

    /// `to_scene` is exactly the staging map (remap + the struct's pin), and
    /// `to_model` inverts it: every MODEL corner lands on its staged byte,
    /// and the roundtrip returns the MODEL corner — the crossing T5's
    /// picking and T11's marker placement ride.
    #[test]
    fn to_scene_to_model_match_staging_and_roundtrip() {
        let parts = golden_3mf_parts();
        let scene = PartScene::from_parts(&parts, NEUTRAL);
        for (part, raw) in scene.parts.iter().zip(&parts) {
            assert_eq!(part.vertices.len(), raw.positions.len());
            for (v, p) in part.vertices.iter().zip(&raw.positions) {
                let s = scene.to_scene(*p);
                let m = scene.to_model(v.pos);
                for a in 0..3 {
                    assert!((s[a] - v.pos[a]).abs() < 1e-5, "to_scene != staged at {a}");
                    assert!((m[a] - p[a]).abs() < 1e-4, "roundtrip != MODEL at {a}");
                }
            }
        }
        // the recorded pin is the map's translation: to_scene of the origin
        // is the pin itself
        assert_eq!(scene.to_scene([0.0; 3]), scene.pin);
    }

    /// `from_parts_with_pin` stages against a caller-chosen frame: the
    /// recorded pin is the input, and the staged bytes sit at exactly
    /// (foreign pin − own pin) from `from_parts`'s — the comparison ghost's
    /// contract (T12 stages the old model in the CURRENT scene's frame).
    #[test]
    fn from_parts_with_pin_honors_a_foreign_pin() {
        let parts = golden_3mf_parts();
        let own = PartScene::from_parts(&parts, NEUTRAL);
        let pin = [10.0, 20.0, 30.0];
        let foreign = PartScene::from_parts_with_pin(&parts, NEUTRAL, pin);
        assert_eq!(foreign.pin, pin);
        let shift = |a: usize| pin[a] - own.pin[a];
        for a in 0..3 {
            assert!(
                (foreign.aabb_min[a] - own.aabb_min[a] - shift(a)).abs() < 1e-4,
                "aabb shifted by the pin difference at {a}"
            );
        }
        for (f, o) in foreign.parts[0].vertices.iter().zip(&own.parts[0].vertices) {
            for a in 0..3 {
                assert!((f.pos[a] - o.pos[a] - shift(a)).abs() < 1e-4);
            }
        }
        // and the foreign scene's own crossing map lands on ITS staging
        let p = parts[0].positions[0];
        let s = foreign.to_scene(p);
        for a in 0..3 {
            assert!((s[a] - foreign.parts[0].vertices[0].pos[a]).abs() < 1e-5);
        }
    }

    #[test]
    fn framed_iso_model_fills_the_pane() {
        // The pane-fill contract of frame() (ModelViewer.swift:328-337):
        // the framed distance puts the scene's bounding sphere at
        // sin(half-FOV)/1.15 of the camera — 86.6 % of the half-view — and
        // the golden fixture's box (scene 60×26×60, radius √7876/2) fills
        // ~two thirds of the viewport height at the iso preset, at ANY
        // aspect (the projection's y extent is aspect-free). If the 1.15
        // headroom, the half-diagonal radius, or the target rule changes,
        // these ratios move and the fill visibly changes with them.
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
        let radius = camera::framing_radius(scene.aabb_min, scene.aabb_max);
        let half_fov = camera::FOV / 2.0;
        let angular_fill = (radius / distance).asin() / half_fov;
        assert!(
            (angular_fill - 0.866).abs() < 0.005,
            "sphere fills {angular_fill:.3} of the half-view, not 86.6 %"
        );
        let (yaw, pitch) = camera::Preset::Iso.yaw_pitch();
        let corners = |a: f32| {
            let view_proj =
                glam::Mat4::from_cols_array_2d(&camera::orbit(distance, yaw, pitch, target, a));
            let mut span = (f32::MAX, f32::MIN);
            for i in 0..8 {
                let p = [
                    [scene.aabb_min[0], scene.aabb_max[0]][i & 1],
                    [scene.aabb_min[1], scene.aabb_max[1]][i >> 1 & 1],
                    [scene.aabb_min[2], scene.aabb_max[2]][i >> 2 & 1],
                    1.0,
                ];
                let clip = view_proj * glam::Vec4::from_array(p);
                let ndc_y = clip.y / clip.w;
                span.0 = span.0.min(ndc_y);
                span.1 = span.1.max(ndc_y);
            }
            (span.1 - span.0) / 2.0 // fraction of the viewport height
        };
        let (square, wide) = (corners(1.0), corners(2.16));
        for (aspect, fill) in [(1.0, square), (2.16, wide)] {
            assert!(
                (0.60..=0.75).contains(&fill),
                "aspect {aspect}: the framed box fills {fill:.2} of the viewport height"
            );
        }
        assert!(
            (square - wide).abs() < 0.005,
            "the y fill is aspect-free: {square:.4} vs {wide:.4}"
        );
    }

    /// The output-space contract: iced's compositor under libcosmic's
    /// `web-colors` build picks a PLAIN (non-sRGB) surface and every iced
    /// primitive writes display-space values itself, so the viewer's
    /// linear-light output must be encoded in-shader there — and must stay
    /// linear for an sRGB-tagged target (the headless tests' format,
    /// where the hardware owns the encode).
    #[test]
    fn shader_sources_encode_only_for_plain_targets() {
        let plain = mesh_wgsl_source(wgpu::TextureFormat::Bgra8Unorm);
        assert!(
            plain.contains("SRGB_ENCODE: f32 = 1.0"),
            "a plain target turns the in-shader encode on"
        );
        let srgb = mesh_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb);
        assert!(
            srgb.contains("SRGB_ENCODE: f32 = 0.0"),
            "an sRGB target keeps the hardware encode"
        );
        for (name, source) in [
            ("mesh", mesh_wgsl_source(wgpu::TextureFormat::Bgra8Unorm)),
            ("cap", cap_wgsl_source(wgpu::TextureFormat::Bgra8Unorm)),
            ("grid", grid_wgsl_source(wgpu::TextureFormat::Bgra8Unorm)),
            ("ghost", ghost_wgsl_source(wgpu::TextureFormat::Bgra8Unorm)),
            (
                "mesh/srgb",
                mesh_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb),
            ),
            (
                "cap/srgb",
                cap_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb),
            ),
            (
                "grid/srgb",
                grid_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb),
            ),
            (
                "ghost/srgb",
                ghost_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb),
            ),
        ] {
            // the real markers this shader must have substituted (WGSL's
            // own @group/@location decorators and the docs' @NAME@ example
            // are not markers); a genuine survivor is caught here AND by
            // the parse test, since a marker is invalid WGSL
            for marker in [
                "@KEY_POS@",
                "@KEY_COEFF@",
                "@FILL_POS@",
                "@FILL_COEFF@",
                "@AMBIENT@",
                "@LIT_MAX@",
                "@COLLISION_R@",
                "@COLLISION_G@",
                "@COLLISION_B@",
                "@CAP_R@",
                "@CAP_G@",
                "@CAP_B@",
                "@GHOST_R@",
                "@GHOST_G@",
                "@GHOST_B@",
                "@GHOST_ALPHA@",
                "@TARGET_SPACE@",
            ] {
                assert!(
                    !source.contains(marker),
                    "{name}: substitution marker {marker} survived"
                );
            }
            assert!(
                source.contains("to_target("),
                "{name}: a color-writing shader must route through to_target"
            );
        }
    }

    /// The substituted sources must be valid WGSL for BOTH target kinds —
    /// a marker that survives, a bad declaration order, or a malformed
    /// encode block is a parse error here, not a silent dark render.
    #[test]
    fn substituted_shaders_parse_as_wgsl() {
        let mut flags = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        );
        for format in [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ] {
            for source in [
                mesh_wgsl_source(format),
                cap_wgsl_source(format),
                grid_wgsl_source(format),
                ghost_wgsl_source(format),
            ] {
                let module = naga::front::wgsl::parse_str(&source)
                    .unwrap_or_else(|e| panic!("parse {format:?}: {e}"));
                flags
                    .validate(&module)
                    .unwrap_or_else(|e| panic!("validate {format:?}: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod target_space_tests {
    use super::*;

    /// The WGSL encode string's curve constants must match the Rust twin
    /// exactly (review F1: a typo in the string passes every pure test —
    /// only the GPU test would catch it, which runs on one machine).
    #[test]
    fn wgsl_encode_constants_match_the_rust_twin() {
        let src = target_space_wgsl(wgpu::TextureFormat::Bgra8Unorm);
        for constant in ["0.0031308", "12.92", "1.055", "2.4", "0.055"] {
            assert!(src.contains(constant), "the WGSL curve lost {constant}: {src}");
        }
        // and the sRGB path is the identity (no curve at all)
        let identity = target_space_wgsl(wgpu::TextureFormat::Bgra8UnormSrgb);
        assert!(identity.contains("SRGB_ENCODE: f32 = 0.0"));
        // the twin and the string implement the same function at the knee
        // and mid-range (the knee's both branches + a gamma-branch value)
        let knee = parts::linear_to_srgb_f(0.003);
        assert!((knee - 0.003 * 12.92).abs() < 1e-6);
        let mid = parts::linear_to_srgb_f(0.5);
        assert!((mid - (1.055 * 0.5f32.powf(1.0 / 2.4) - 0.055)).abs() < 1e-6);
    }
}
