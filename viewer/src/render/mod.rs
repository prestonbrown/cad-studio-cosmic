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
//! camera moves. Camera and section-cap uniforms legitimately write per
//! frame.
//!
//! msaa: `MeshPipeline::new` stays single-sample (the default, byte-identical
//! frames); `MeshPipeline::with_msaa` (count 4) renders into an owned
//! multisample color + depth attachment, resolves into an intermediate
//! single-sample texture, and blits that over the target with premultiplied
//! alpha — iced's content underneath survives, same Load-and-composite
//! contract as the single-sample path.
//!
//! Cross-sections: the mesh pass writes winding parity into the stencil
//! (front +1 / back -1 per rasterized fragment, wrap ops, section-discarded
//! fragments excluded), and a red cap quad drawn right after fills exactly
//! the cut face (stencil != 0) when `Camera::section_enable` is set.

pub mod parts;
pub mod scene;

pub use parts::{GpuPart, PartScene, NEUTRAL, srgb_to_linear};
pub use scene::{SpikePrimitive, SpikeScene};

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

/// Per-part model transform (group 1): identity while parts render where
/// they sit; explode offsets and playback poses will write real matrices
/// through the same binding.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Model {
    transform: [[f32; 4]; 4],
}

impl Model {
    const IDENTITY: Model = Model {
        transform: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CapVertex {
    pos: [f32; 3],
}

/// World-space quad on the section plane: the scene aabb cross-section at
/// `camera.section_pos`, grown 10% on the two off-axis directions so the cap
/// always covers the cut face (the stencil test clips it back to the solid).
/// A 4-vertex triangle strip.
fn cap_quad(scene: &PartScene, camera: &Camera) -> [CapVertex; 4] {
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
        let center = (scene.aabb_min[a] + scene.aabb_max[a]) / 2.0;
        let half = (scene.aabb_max[a] - scene.aabb_min[a]) / 2.0 * 1.1;
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
    pub fn orbiting(
        distance: f32,
        yaw: f32,
        pitch: f32,
        target: [f32; 3],
        aspect: f32,
    ) -> Camera {
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

    /// orbit(by:) (ModelViewer.swift:301-305): drag deltas in points to
    /// yaw/pitch — yaw decreases with dx, pitch increases with dy, both
    /// scaled 0.008, pitch clamped to ±MAX_PITCH.
    pub fn orbit_drag(yaw: f32, pitch: f32, dx: f32, dy: f32) -> (f32, f32) {
        (
            yaw - dx * ORBIT_SENSITIVITY,
            (pitch + dy * ORBIT_SENSITIVITY).clamp(-MAX_PITCH, MAX_PITCH),
        )
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
/// identity the buffer currently holds. Built by `prepare_parts` when a
/// part's hash differs; untouched otherwise.
struct ResidentPart {
    content_hash: u64,
    vertex_buf: wgpu::Buffer,
    vertex_count: u32,
    /// Group 1 for this part's draws: the model transform.
    model_bg: wgpu::BindGroup,
}

pub struct MeshPipeline {
    pipeline: wgpu::RenderPipeline,
    cap_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    model_layout: wgpu::BindGroupLayout,
    /// Depth24PlusStencil8, viewer-owned; resized in
    /// [`MeshPipeline::prepare_parts`].
    depth: wgpu::TextureView,
    depth_tex: wgpu::Texture,
    uniform_buf: wgpu::Buffer,
    residents: Vec<ResidentPart>,
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
    /// Vertex uploads since creation — one per part residency; camera and
    /// cap uniforms write per frame and never count. Read only by the
    /// `*_count_for_tests` accessors.
    #[cfg_attr(not(test), allow(dead_code))]
    upload_count: u32,
    /// Vertex-buffer allocations since creation (one per residency). Read
    /// only by the `*_count_for_tests` accessors.
    #[cfg_attr(not(test), allow(dead_code))]
    alloc_count: u32,
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
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mesh.wgsl").into()),
        });
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
        // draw — assemblies of any size render correctly
        let model_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: Some("part model layout"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[&layout, &model_layout],
            immediate_size: 0,
            label: None,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
            layout: Some(&pipeline_layout),
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
        // Camera-only layout: the cap has no per-part transform.
        let cap_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[&layout],
            immediate_size: 0,
            label: None,
        });
        let cap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cap.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cap.wgsl").into()),
        });
        let cap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("section cap"),
            layout: Some(&cap_pipeline_layout),
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
            primitive: wgpu::PrimitiveState {
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
            pipeline,
            cap_pipeline,
            bind_group,
            model_layout,
            depth,
            depth_tex,
            uniform_buf,
            residents: Vec::new(),
            cap_buf,
            draw_cap: false,
            size: (1, 1),
            sample_count,
            format: target_format,
            blit,
            msaa: None,
            upload_count: 0,
            alloc_count: 0,
        }
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
    /// render-target `size`, and the upload-once mesh path — a part's
    /// vertices are (re)written only when its content identity differs
    /// from what is resident, so an unchanged scene costs zero mesh
    /// uploads however the camera moves. Part transforms are identity
    /// and written once per residency.
    pub fn prepare_parts(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        camera: &Camera,
        scene: &PartScene,
    ) {
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
        // init-write are what the counters track — model uniforms are not
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
            let model_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("part {label} model")),
                contents: bytemuck::bytes_of(&Model::IDENTITY),
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
                model_bg,
            };
            if i < self.residents.len() {
                self.residents[i] = resident;
            } else {
                self.residents.push(resident);
            }
        }
        self.draw_cap = camera.section_enable > 0.5;
        if self.draw_cap {
            queue.write_buffer(&self.cap_buf, 0, bytemuck::cast_slice(&cap_quad(scene, camera)));
        }
    }

    /// Vertex uploads since pipeline creation (test visibility): counts
    /// vertex uploads only — camera and cap uniforms legitimately write
    /// every frame and never count.
    #[cfg(test)]
    pub(crate) fn upload_count_for_tests(&self) -> u32 {
        self.upload_count
    }

    /// Vertex-buffer allocations since pipeline creation (test visibility).
    #[cfg(test)]
    pub(crate) fn alloc_count_for_tests(&self) -> u32 {
        self.alloc_count
    }

    /// Renders the parts — and, when the camera sections (`section_enable` >
    /// 0.5), the red cap quad over the cut face — into `target` (the iced
    /// surface view) inside a viewer-owned pass. Color attachment:
    /// `LoadOp::Load` (iced content underneath stays); depth and stencil:
    /// cleared. `viewport` is `[x, y, width, height]` in physical pixels.
    ///
    /// At `sample_count > 1` the mesh pass instead renders into the owned
    /// multisample attachment (cleared transparent — coverage lives in
    /// alpha) and resolves into the intermediate; a second pass then blits
    /// the intermediate over `target` with premultiplied alpha, keeping
    /// iced's content underneath. The msaa path maps the intermediate 1:1,
    /// so it expects `viewport` to match the size last given to
    /// `prepare_parts` (true for the headless harness; the spike app keeps
    /// count 1).
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: [f32; 4],
    ) {
        if self.residents.is_empty() {
            return; // nothing prepared yet
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
            set_vp(&mut blit_pass);
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

    /// Part draws — one bind group + vertex buffer + draw per resident
    /// part — and, when the camera sections, the cap quad right after —
    /// into the pass's color attachment (msaa or single-sample alike: the
    /// cap inherits the pass attachment and its pipeline matches its
    /// sample count).
    fn draw_scene(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        for resident in &self.residents {
            pass.set_bind_group(1, &resident.model_bg, &[]);
            pass.set_vertex_buffer(0, resident.vertex_buf.slice(..));
            pass.draw(0..resident.vertex_count, 0..1);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use engine::parsing::stl::parse_stl;
    use engine::parsing::threemf::parse_3mf;

    fn fixture(rel: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden").join(rel)
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

    async fn device() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: None,
                ..Default::default()
            })
            .await
            .expect("surfaceless wgpu adapter (Metal locally, Vulkan on thelio)");
        adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap()
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
        assert_eq!(
            camera::Preset::Top.yaw_pitch(),
            (0.0, camera::MAX_PITCH)
        );
        assert_eq!(
            camera::Preset::Right.yaw_pitch(),
            (std::f32::consts::FRAC_PI_2, 0.0)
        );
        // pan: world units per view point at `distance` (facts #22)
        let upp = camera::pan_units_per_point(100.0, 500.0);
        let expected = 2.0 * 100.0 * f32::tan(17.5f32.to_radians()) / 500.0;
        assert!((upp - expected).abs() < 1e-6, "{upp} vs {expected}");
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

    #[test]
    fn upload_once_second_frame_no_reupload() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (device, queue) = rt.block_on(device());
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        let camera = default_camera(&scene, 4.0 / 3.0);
        let mut pipeline =
            MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(&device, &queue, (800, 600), &camera, &scene);
        let (u1, a1) = (
            pipeline.upload_count_for_tests(),
            pipeline.alloc_count_for_tests(),
        );
        assert_eq!((u1, a1), (2, 2), "one upload+alloc per part on residency");
        // second frame, same scene, MOVED camera: mesh bytes must not move
        let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
        let moved = Camera::orbiting(distance * 2.0, 0.9, 0.4, target, 4.0 / 3.0);
        pipeline.prepare_parts(&device, &queue, (800, 600), &moved, &scene);
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
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (device, queue) = rt.block_on(device());
        let mut parts = golden_3mf_parts();
        let scene1 = PartScene::from_parts(&parts, NEUTRAL);
        let camera = default_camera(&scene1, 4.0 / 3.0);
        let mut pipeline =
            MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(&device, &queue, (800, 600), &camera, &scene1);
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
        pipeline.prepare_parts(&device, &queue, (800, 600), &camera, &scene2);
        assert_eq!(
            pipeline.upload_count_for_tests(),
            u1 + 1,
            "grown part reuploads; the untouched part does not"
        );
        assert_eq!(pipeline.alloc_count_for_tests(), a1 + 1);
    }

    #[test]
    fn middle_vertex_mutation_reuploads() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (device, queue) = rt.block_on(device());
        let mut parts = golden_3mf_parts();
        let scene1 = PartScene::from_parts(&parts, NEUTRAL);
        let camera = default_camera(&scene1, 4.0 / 3.0);
        let mut pipeline =
            MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(&device, &queue, (800, 600), &camera, &scene1);
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
        pipeline.prepare_parts(&device, &queue, (800, 600), &camera, &scene2);
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
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (device, queue) = rt.block_on(device());
        // the STL is the all-None fixture: every triangle paints neutral,
        // so the neutral is load-bearing in the staged bytes
        let parts = golden_stl_parts();
        assert!(parts[0].triangle_colors.iter().all(|c| c.is_none()));
        let scene1 = PartScene::from_parts(&parts, NEUTRAL);
        let camera = default_camera(&scene1, 4.0 / 3.0);
        let mut pipeline =
            MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(&device, &queue, (800, 600), &camera, &scene1);
        let u1 = pipeline.upload_count_for_tests();
        // facts #43's light-mode neutral: same geometry, only the paint
        // changes — a skipped upload would serve the dark-mode colors
        let scene2 = PartScene::from_parts(&parts, [0.55, 0.64, 0.75]);
        pipeline.prepare_parts(&device, &queue, (800, 600), &camera, &scene2);
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
            [parts::srgb_to_linear(c[0]), parts::srgb_to_linear(c[1]), parts::srgb_to_linear(c[2])]
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
            assert_eq!(v.color, [1.0, 0.0, 0.0], "Some([255,0,0]) is exact linear red");
        }
        for v in &scene.parts[0].vertices[3..6] {
            assert_eq!(v.color, NEUTRAL, "None paints the neutral");
        }
    }

    /// The floor pin is baked into the staged vertices, so content identity
    /// must track them: a part growing past the union aabb moves the pin,
    /// and every part's scene-space content changes with it — revert the
    /// identity to raw MODEL positions and this test catches parts left
    /// resident at the stale pin.
    #[test]
    fn pin_shift_reuploads_every_part() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (device, queue) = rt.block_on(device());
        let mut parts = golden_3mf_parts();
        let scene1 = PartScene::from_parts(&parts, NEUTRAL);
        let camera = default_camera(&scene1, 4.0 / 3.0);
        let mut pipeline =
            MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.prepare_parts(&device, &queue, (800, 600), &camera, &scene1);
        let u1 = pipeline.upload_count_for_tests();
        // grow past MODEL x=30: union widens, the pin's x-center shifts
        for corner in [[40.0, 0.0, 16.0], [41.0, 0.0, 16.0], [40.0, 1.0, 16.0]] {
            parts[1].positions.push(corner);
        }
        parts[1].triangle_colors.push(None);
        let scene2 = PartScene::from_parts(&parts, NEUTRAL);
        pipeline.prepare_parts(&device, &queue, (800, 600), &camera, &scene2);
        assert_eq!(
            pipeline.upload_count_for_tests(),
            u1 + 2,
            "a moved pin re-renders every part at the new offset"
        );
    }
}
