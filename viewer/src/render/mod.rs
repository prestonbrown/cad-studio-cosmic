//! Spike renderer: mesh via viewer-owned render pass with Depth24PlusStencil8.
//! iced's compositor render pass carries no depth/stencil attachment, so
//! everything the viewer draws happens in passes this module owns: the surface
//! color attachment is loaded (`LoadOp::Load`, iced content underneath stays),
//! depth and stencil are cleared every frame, and the depth texture is resized
//! with the viewport. Pure wgpu — no iced types — so the headless test can
//! reuse this module as-is; the iced adapter lives in [`crate::render::scene`].
//!
//! msaa is fixed at 1 here; the MSAA task passes 4 and owns the resolve path.
//!
//! Cross-sections: the mesh pass writes winding parity into the stencil
//! (front +1 / back -1 per rasterized fragment, wrap ops, section-discarded
//! fragments excluded), and a red cap quad drawn right after fills exactly
//! the cut face (stencil != 0) when `Camera::section_enable` is set.

pub mod scene;

pub use scene::{SpikePrimitive, SpikeScene};

use std::io::Read;

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

#[derive(Clone, Debug)]
pub struct TriMesh {
    pub vertices: Vec<Vertex>, // one per corner (unindexed, per-face)
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CapVertex {
    pos: [f32; 3],
}

/// World-space quad on the section plane: the mesh aabb cross-section at
/// `camera.section_pos`, grown 10% on the two off-axis directions so the cap
/// always covers the cut face (the stencil test clips it back to the solid).
/// A 4-vertex triangle strip.
fn cap_quad(mesh: &TriMesh, camera: &Camera) -> [CapVertex; 4] {
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
        let center = (mesh.aabb_min[a] + mesh.aabb_max[a]) / 2.0;
        let half = (mesh.aabb_max[a] - mesh.aabb_min[a]) / 2.0 * 1.1;
        (center - half, center + half)
    };
    let (u0, u1) = edge(u);
    let (v0, v1) = edge(v);
    [at(u0, v0), at(u1, v0), at(u0, v1), at(u1, v1)]
}

/// Binary STL. `bytes` must be well-formed (call [`load_fixture_stl`], which
/// length-checks, or check `84 + 50 * n == bytes.len()` first).
pub fn parse_binary_stl(bytes: &[u8]) -> TriMesh {
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    let mut vertices = Vec::with_capacity(n * 3);
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for i in 0..n {
        let off = 84 + i * 50;
        // 50-byte record: 12 floats (normal, then 3 corners) + u16 attribute;
        // float k sits at record offset 4k
        let f = |k: usize| f32::from_le_bytes(bytes[off + k * 4..off + 4 + k * 4].try_into().unwrap());
        let normal = [f(0), f(1), f(2)];
        for c in 0..3 {
            let p = [f(3 + c * 3), f(4 + c * 3), f(5 + c * 3)];
            for a in 0..3 {
                mn[a] = mn[a].min(p[a]);
                mx[a] = mx[a].max(p[a]);
            }
            vertices.push(Vertex {
                pos: p,
                normal,
                color: [0.62, 0.70, 0.80], // neutral color, upstream-contract §5
            });
        }
    }
    TriMesh {
        vertices,
        aabb_min: mn,
        aabb_max: mx,
    }
}

pub fn load_fixture_stl(path: &std::path::Path) -> TriMesh {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]);
    assert_eq!(bytes.len(), 84 + 50 * n as usize, "fixture must be binary STL");
    parse_binary_stl(&bytes)
}

/// Default iso camera framing `mesh` (upstream presets: yaw π/4, pitch 0.6).
pub fn default_camera(mesh: &TriMesh) -> Camera {
    orbit_camera(mesh, std::f32::consts::FRAC_PI_4, 0.6, 1.0)
}

/// Orbit camera around the mesh aabb center; `zoom` scales the framing
/// distance (1.0 = default framing). Sectioning off.
pub fn orbit_camera(mesh: &TriMesh, yaw: f32, pitch: f32, zoom: f32) -> Camera {
    let center = glam::Vec3::from_array([
        (mesh.aabb_min[0] + mesh.aabb_max[0]) / 2.0,
        (mesh.aabb_min[1] + mesh.aabb_max[1]) / 2.0,
        (mesh.aabb_min[2] + mesh.aabb_max[2]) / 2.0,
    ]);
    let radius = ((mesh.aabb_max[0] - mesh.aabb_min[0])
        .max(mesh.aabb_max[1] - mesh.aabb_min[1])
        .max(mesh.aabb_max[2] - mesh.aabb_min[2])
        / 2.0)
        .max(1.0);
    let fov = 35f32.to_radians();
    let distance = radius / (fov / 2.0).sin() * 1.15 * zoom;
    // pitch clamped by callers to keep cos(pitch) > 0 (up vector stays Y)
    let eye = center
        + glam::Vec3::new(pitch.cos() * yaw.sin(), pitch.sin(), pitch.cos() * yaw.cos())
            * distance;
    let view = glam::camera::rh::view::look_at_mat4(eye, center, glam::Vec3::Y);
    // directx = WebGPU NDC (Z in [0,1], Y-up); spike uses a fixed 16:9 aspect,
    // the real renderer derives it from the viewport
    let proj = glam::camera::rh::proj::directx::perspective(fov, 16.0 / 9.0, 1.0, 100_000.0);
    Camera {
        view_proj: (proj * view).to_cols_array_2d(),
        section_axis: 1,
        section_pos: f32::MAX,
        section_enable: 0.0,
        _pad: 0.0,
    }
}

pub struct MeshPipeline {
    pipeline: wgpu::RenderPipeline,
    cap_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    /// Depth24PlusStencil8, viewer-owned; resized in [`MeshPipeline::prepare`].
    pub depth: wgpu::TextureView,
    depth_tex: wgpu::Texture,
    uniform_buf: wgpu::Buffer,
    vertex_buf: Option<wgpu::Buffer>,
    vertex_capacity: u64,
    pub vertex_count: u32,
    cap_buf: wgpu::Buffer,
    draw_cap: bool,
    size: (u32, u32),
}

impl MeshPipeline {
    pub fn new(
        device: &wgpu::Device,
        _queue: &wgpu::Queue, // camera/mesh data arrives per frame in `prepare`
        target_format: wgpu::TextureFormat,
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
            bind_group_layouts: &[&layout],
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
                count: 1,
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
        let cap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cap.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cap.wgsl").into()),
        });
        let cap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("section cap"),
            layout: Some(&pipeline_layout),
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
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        // 4-vertex strip, zeroed at build; `prepare` writes the real quad
        // from the current mesh aabb + camera section params each frame
        let cap_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("section cap quad"),
            contents: bytemuck::cast_slice(&[CapVertex { pos: [0.0; 3] }; 4]),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let (depth_tex, depth) = Self::make_depth(device, (1, 1));
        Self {
            pipeline,
            cap_pipeline,
            bind_group,
            depth,
            depth_tex,
            uniform_buf,
            vertex_buf: None,
            vertex_capacity: 0,
            vertex_count: 0,
            cap_buf,
            draw_cap: false,
            size: (1, 1),
        }
    }

    fn make_depth(device: &wgpu::Device, size: (u32, u32)) -> (wgpu::Texture, wgpu::TextureView) {
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewer depth+stencil"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24PlusStencil8,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let v = t.create_view(&Default::default());
        (t, v)
    }

    /// Per-frame upload: camera uniform, mesh vertices, and depth-texture
    /// resize to the render-target `size`.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        camera: &Camera,
        mesh: &TriMesh,
    ) {
        if self.size != size {
            let (t, v) = Self::make_depth(device, size);
            self.depth_tex = t;
            self.depth = v;
            self.size = size;
        }
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(camera));
        let needed = (mesh.vertices.len() * std::mem::size_of::<Vertex>()) as u64;
        if needed > self.vertex_capacity {
            self.vertex_buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh vertices"),
                size: needed,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.vertex_capacity = needed;
        }
        if let Some(buf) = &self.vertex_buf {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&mesh.vertices));
        }
        self.vertex_count = mesh.vertices.len() as u32;
        self.draw_cap = camera.section_enable > 0.5;
        if self.draw_cap {
            queue.write_buffer(&self.cap_buf, 0, bytemuck::cast_slice(&cap_quad(mesh, camera)));
        }
    }

    /// Renders the mesh — and, when the camera sections (`section_enable` >
    /// 0.5), the red cap quad over the cut face — into `target` (the iced
    /// surface view) inside a viewer-owned pass. Color attachment:
    /// `LoadOp::Load` (iced content underneath stays); depth and stencil:
    /// cleared. `viewport` is `[x, y, width, height]` in physical pixels.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: [f32; 4],
    ) {
        let Some(vertex_buf) = &self.vertex_buf else {
            return; // nothing prepared yet
        };
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
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0),
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_viewport(
            viewport[0],
            viewport[1],
            viewport[2].max(1.0),
            viewport[3].max(1.0),
            0.0,
            1.0,
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, vertex_buf.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
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

    fn golden() -> TriMesh {
        load_fixture_stl(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/golden/model.stl"
        )))
    }

    #[test]
    fn golden_fixture_is_24_triangles_inside_its_aabb() {
        let mesh = golden();
        assert_eq!(mesh.vertices.len(), 24 * 3);
        for v in &mesh.vertices {
            for a in 0..3 {
                assert!(v.pos[a] >= mesh.aabb_min[a] - 1e-6);
                assert!(v.pos[a] <= mesh.aabb_max[a] + 1e-6);
            }
        }
        for a in 0..3 {
            assert!(mesh.aabb_max[a] > mesh.aabb_min[a]);
        }
    }

    #[test]
    fn cameras_are_finite() {
        let mesh = golden();
        for &(yaw, pitch, zoom) in &[
            (std::f32::consts::FRAC_PI_4, 0.6f32, 1.0f32),
            (0.0, 0.0, 0.05),
            (2.0 * std::f32::consts::PI, 1.4, 50.0),
        ] {
            let camera = orbit_camera(&mesh, yaw, pitch, zoom);
            for row in camera.view_proj {
                for x in row {
                    assert!(x.is_finite());
                }
            }
        }
        assert_eq!(default_camera(&mesh).section_enable, 0.0);
    }
}
