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
//! without touching a vertex, and motion playback rides the same binding:
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
//! contract as the single-sample path. The app pane sets [`Options::msaa`]
//! to 4 (the example's `m` key flips it between 1 and 4); a count that
//! differs from the resident one makes `prepare_parts` rebuild the render
//! pipelines and owned attachments in place (iced builds the pipeline at 1
//! through `Pipeline::new` and its Storage cannot hand out replacements),
//! while the wireframe toggle stays a pure draw-path switch.
//!
//! Cross-sections: the mesh pass writes winding parity into the stencil
//! (front +1 / back -1 per rasterized fragment, wrap ops, section-discarded
//! fragments excluded), and a red cap quad drawn right after fills exactly
//! the cut face (stencil != 0) when the frame carries a [`SectionPlane`].
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
//! The compare ghost (the render half of version comparison; the app
//! owns the comparison state): a second [`PartScene`] staged from the comparison version in
//! the CURRENT scene's pin (`PartScene::from_parts_with_pin`,
//! ModelViewer.swift:135 — ghostEntity hangs under `content`) draws as
//! an unlit systemOrange wireframe at [`GHOST_ALPHA`] through its own
//! LINE pipeline over the ghost scene's edge lists, between the cap and
//! the overlays — depth-tested so the mesh occludes it, never writing
//! depth so it occludes nothing after it.

pub mod camera;
pub mod gpu;
pub mod grid;
pub mod lights;
pub mod markers;
pub mod parts;
mod pipelines;
mod residency;
pub mod scene;
mod shaders;
mod targets;
#[cfg(test)]
mod test_support;

pub use camera::{Camera, default_camera, preset_camera};
pub use grid::{GridMesh, GridVertex, Palette};
pub use markers::{MarkerMesh, ORANGE};
pub use parts::{
    GpuPart, NEUTRAL, NEUTRAL_LIGHT, PartScene, neutral, srgb_to_linear, srgb_to_linear_f,
};
pub use scene::{ModelPrimitive, ModelScene, Themed};
pub use shaders::{COLLISION_LINEAR, CUT_FACE_LINEAR, GHOST_ALPHA};

pub use engine::parsing::Box3;
use wgpu::util::DeviceExt;

use pipelines::{Blit, Pipelines};
use residency::{ResidentGhostPart, ResidentGrid, ResidentPart, ghost_identity, stage_overlay};
use targets::MsaaTargets;

/// A scene-space axis (render coords, Y up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneAxis {
    X,
    Y,
    Z,
}

impl SceneAxis {
    pub fn index(self) -> usize {
        match self {
            SceneAxis::X => 0,
            SceneAxis::Y => 1,
            SceneAxis::Z => 2,
        }
    }
}

/// A section plane in a scene's PINNED frame: fragments with
/// `position[axis] > pos` are discarded, and the red cap fills the cut.
/// `cap` is the scene-space box of what is drawn (arranged offsets
/// included), which the cap quad must span — it travels with the plane so
/// the discard and the cap cannot be built from two different boxes.
/// [`crate::tools::section::pinned_plane`] builds one from the tool state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionPlane {
    pub axis: SceneAxis,
    pub pos: f32,
    pub cap: Box3,
}

/// The camera uniform (group 0) as the shaders read it: the frame's
/// view-projection plus its section plane, if any.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    section_axis: u32,
    section_pos: f32,
    section_enable: f32,
    /// WGSL uniform structs round their size up to 16 bytes: 64 + 4 + 4 + 4 = 76
    /// must be padded to 80 or bind-group validation fails on min_binding_size.
    _pad: f32,
}

impl CameraUniform {
    fn new(camera: &Camera, section: Option<&SectionPlane>) -> Self {
        CameraUniform {
            view_proj: camera.view_proj,
            section_axis: section.map_or(1, |s| s.axis.index() as u32),
            section_pos: section.map_or(f32::MAX, |s| s.pos),
            section_enable: if section.is_some() { 1.0 } else { 0.0 },
            _pad: 0.0,
        }
    }
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
/// length — asserted), the section plane, the overlays, and the render
/// mode. The struct is the renderer's whole per-frame input contract.
pub struct Frame<'a> {
    pub camera: &'a Camera,
    pub scene: &'a PartScene,
    pub poses: &'a [PartPose],
    pub appearances: &'a [PartAppearance],
    /// The section plane, when sectioning: the mesh discard and the cap.
    pub section: Option<SectionPlane>,
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

/// The translucent static-parts material's opacity
/// (ModelViewer.swift:410: `.transparent(opacity: 0.22)`).
pub const TRANSLUCENT_ALPHA: f32 = 0.22;

/// World-space quad on the section plane: the cross-section of the
/// plane's cap box at `plane.pos`, grown 10% on the two off-axis
/// directions so the cap always covers the cut face (the stencil test
/// clips it back to the solid). A 4-vertex triangle strip.
fn cap_quad(plane: &SectionPlane) -> [CapVertex; 4] {
    let axis = plane.axis.index();
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let at = |pu: f32, pv: f32| {
        let mut p = [0.0f32; 3];
        p[axis] = plane.pos;
        p[u] = pu;
        p[v] = pv;
        CapVertex { pos: p }
    };
    let (center, size) = (plane.cap.center(), plane.cap.size());
    let edge = |a: usize| {
        let half = size[a] / 2.0 * 1.1;
        (center[a] - half, center[a] + half)
    };
    let (u0, u1) = edge(u);
    let (v0, v1) = edge(v);
    [at(u0, v0), at(u1, v0), at(u0, v1), at(u1, v1)]
}

pub struct MeshPipeline {
    /// The render pipelines at the current sample count.
    pipelines: Pipelines,
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
            contents: bytemuck::bytes_of(&CameraUniform::new(
                &Camera {
                    view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
                },
                None,
            )),
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
        let pipelines = pipelines::build(
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
        let blit = (sample_count > 1).then(|| pipelines::blit(device, target_format));
        let (depth_tex, depth) = targets::depth(device, (1, 1), sample_count);
        Self {
            pipelines,
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

    /// The msaa toggle's in-place rebuild (iced's Storage owns one
    /// [`MeshPipeline`] per type and cannot hand out replacements):
    /// recreate every render pipeline and owned attachment at `count`,
    /// keeping the layouts, buffers, and part residency — the fresh
    /// pipelines are built against the kept layout objects, so the
    /// resident bind groups stay valid.
    fn set_msaa(&mut self, device: &wgpu::Device, count: u32) {
        self.pipelines = pipelines::build(
            device,
            self.format,
            count,
            &self.part_pass_layout,
            &self.camera_pass_layout,
        );
        self.blit = (count > 1).then(|| pipelines::blit(device, self.format));
        self.sample_count = count;
        self.rebuild_attachments(device, self.size);
    }

    /// Recreate the owned depth/stencil and (at count > 1) msaa
    /// attachments at `size` and the current sample count.
    fn rebuild_attachments(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        let (depth_tex, depth) = targets::depth(device, size, self.sample_count);
        self.depth_tex = depth_tex;
        self.depth = depth;
        self.msaa = self
            .blit
            .as_ref()
            .map(|blit| targets::msaa(device, blit, self.format, self.sample_count, size));
        self.size = size;
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
            self.rebuild_attachments(device, size);
        }
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&CameraUniform::new(camera, frame.section.as_ref())),
        );
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
        let cap = frame.section.filter(|_| !options.wireframe);
        self.draw_cap = cap.is_some();
        if let Some(plane) = &cap {
            queue.write_buffer(&self.cap_buf, 0, bytemuck::cast_slice(&cap_quad(plane)));
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
    /// shader flag), then, when the camera sections, the cap quad, then
    /// the grid, then the translucent set through the blend pipelines
    /// (same topology choice, alpha from the model uniform), then the
    /// compare ghost, and last the measure markers — into the pass's
    /// color attachment (msaa or single-sample alike: everything here
    /// inherits the pass attachment and matches its sample count).
    fn draw_scene(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.bind_group, &[]);
        if self.wireframe {
            // facts #44: same material and opacity as fill, hairline list
            pass.set_pipeline(&self.pipelines.line);
            for &i in &self.drawn {
                let resident = &self.residents[i];
                pass.set_bind_group(1, &resident.model_bg, &[]);
                pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                pass.set_index_buffer(resident.edge_buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..resident.edge_count, 0, 0..1);
            }
        } else {
            pass.set_pipeline(&self.pipelines.mesh);
            for &i in &self.drawn {
                let resident = &self.residents[i];
                pass.set_bind_group(1, &resident.model_bg, &[]);
                pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                pass.draw(0..resident.vertex_count, 0..1);
            }
        }
        if self.draw_cap {
            // Same pass, after the mesh: the stencil attachment already holds
            // the parity the mesh's inc/dec left, so the quad only survives
            // where the cut opened the solid.
            pass.set_pipeline(&self.pipelines.cap);
            pass.set_stencil_reference(0);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.cap_buf.slice(..));
            pass.draw(0..4, 0..1);
        }
        // The grid after the opaque parts and the cap (it writes no depth;
        // the parts occlude it through the depth test), before anything
        // translucent so that blends over it.
        if let Some(grid) = self.grid.as_ref() {
            pass.set_pipeline(&self.pipelines.grid);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, grid.vertex_buf.slice(..));
            pass.draw(0..grid.vertex_count, 0..1);
        }
        // The translucent set after the opaque parts and the grid: it
        // blends over both (the grid shows dimmed through a static part
        // during playback, as RealityKit sorts its transparent entities)
        // and must not occlude anything after it (no depth write) — a
        // collision part never lands here (opaque by partition), so the
        // flat-red branch never blends. Playback and sectioning exclude
        // each other, so the cap never meets this set.
        if !self.drawn_translucent.is_empty() {
            if self.wireframe {
                pass.set_pipeline(&self.pipelines.line_blend);
                for &i in &self.drawn_translucent {
                    let resident = &self.residents[i];
                    pass.set_bind_group(1, &resident.model_bg, &[]);
                    pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                    pass.set_index_buffer(resident.edge_buf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..resident.edge_count, 0, 0..1);
                }
            } else {
                pass.set_pipeline(&self.pipelines.mesh_blend);
                for &i in &self.drawn_translucent {
                    let resident = &self.residents[i];
                    pass.set_bind_group(1, &resident.model_bg, &[]);
                    pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                    pass.draw(0..resident.vertex_count, 0..1);
                }
            }
        }
        // The compare ghost after the grid, before the markers:
        // depth-tested against the opaque mesh (a ghost behind the model
        // hides) but never writing depth, so it cannot occlude the markers
        // after it.
        // Drawn in wireframe mode too — upstream's ghost is independent of
        // the wireframe toggle (facts #44/#45 disable the interior shell,
        // not the ghost).
        if !self.ghost_residents.is_empty() {
            pass.set_pipeline(&self.pipelines.ghost);
            pass.set_bind_group(0, &self.bind_group, &[]);
            for resident in &self.ghost_residents {
                pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
                pass.set_index_buffer(resident.edge_buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..resident.edge_count, 0, 0..1);
            }
        }
        // The measure markers last (they write no depth): unlit
        // vertex-colored, depth-tested against the parts so the model
        // occludes them.
        if let Some(marker) = self.marker.as_ref() {
            pass.set_pipeline(&self.pipelines.grid);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, marker.vertex_buf.slice(..));
            pass.draw(0..marker.vertex_count, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_support::{cube_part, golden_3mf_parts, golden_stl_parts};

    /// The default must be the single-sample count, not `u32::default()`
    /// (0): that differs from a fresh pipeline's resident 1 and would
    /// drive the first default-options prepare into an invalid 0-sample
    /// rebuild — the derive(Default) bug the manual impl replaced.
    #[test]
    fn options_default_is_single_sample() {
        assert_eq!(Options::default().msaa, 1);
        assert!(!Options::default().wireframe);
    }

    /// The Frame every upload-once test shares: default options, no
    /// overlay, no section.
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
            section: None,
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
        let framed = camera::Orbit::framed(scene.aabb(), camera::Preset::Iso);
        let moved = camera::Orbit {
            yaw: 0.9,
            pitch: 0.4,
            distance: framed.distance * 2.0,
            ..framed
        }
        .camera(4.0 / 3.0);
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

    /// The content hash covers the STAGED vertex
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
    /// model uniform alone — zero vertex uploads, never a restage of
    /// the whole part.
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

    /// WGSL uniform structs round to 16-byte boundaries, so the camera's 76
    /// and the model's 72 bytes pad to 80 in BOTH the Rust repr and the WGSL
    /// struct — a mismatch fails bind-group validation at
    /// min_binding_size, loudly.
    #[test]
    fn camera_uniform_is_80_bytes_and_carries_the_plane() {
        assert_eq!(std::mem::size_of::<CameraUniform>(), 80);
        let cam = Camera {
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
        };
        let off = CameraUniform::new(&cam, None);
        assert_eq!(off.section_enable, 0.0);
        let plane = SectionPlane {
            axis: SceneAxis::Z,
            pos: 2.5,
            cap: Box3::ZERO,
        };
        let on = CameraUniform::new(&cam, Some(&plane));
        assert_eq!(
            (on.section_axis, on.section_pos, on.section_enable),
            (2, 2.5, 1.0)
        );
    }

    /// The cap quad spans the plane's cap box, grown 10% on the two
    /// off-axis directions, at the plane position.
    #[test]
    fn cap_quad_spans_the_planes_box() {
        let plane = SectionPlane {
            axis: SceneAxis::X,
            pos: 1.0,
            cap: Box3::new([-5.0, 0.0, -2.0], [5.0, 20.0, 2.0]),
        };
        let q = cap_quad(&plane);
        assert!(q.iter().all(|v| v.pos[0] == 1.0));
        let ys: Vec<f32> = q.iter().map(|v| v.pos[1]).collect();
        let zs: Vec<f32> = q.iter().map(|v| v.pos[2]).collect();
        let span = |v: &[f32]| {
            (
                v.iter().copied().fold(f32::MAX, f32::min),
                v.iter().copied().fold(f32::MIN, f32::max),
            )
        };
        let (y0, y1) = span(&ys);
        let (z0, z1) = span(&zs);
        assert!(
            (y0 - -1.0).abs() < 1e-5 && (y1 - 21.0).abs() < 1e-5,
            "{y0} {y1}"
        );
        assert!(
            (z0 - -2.2).abs() < 1e-5 && (z1 - 2.2).abs() < 1e-5,
            "{z0} {z1}"
        );
    }

    #[test]
    fn model_uniform_is_80_bytes() {
        assert_eq!(
            std::mem::size_of::<Model>(),
            80,
            "transform 64 + collision 4 + alpha 4 + pad 8"
        );
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
        let framed = camera::Orbit::framed(scene.aabb(), camera::Preset::Iso);
        let moved = camera::Orbit {
            yaw: 0.9,
            pitch: 0.4,
            distance: framed.distance * 2.0,
            ..framed
        }
        .camera(4.0 / 3.0);
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
}
