//! The render pipelines: one [`PipelineSpec`] per pipeline, built by
//! [`build_pipeline`] at the frame's format and sample count.

use super::shaders::{cap_wgsl_source, ghost_wgsl_source, grid_wgsl_source, mesh_wgsl_source};
use super::{CapVertex, GridVertex, Vertex};

/// One stencil face: `compare`, with `fail` kept and `op` applied on both
/// depth-pass and depth-fail.
pub(super) fn stencil_face(
    compare: wgpu::CompareFunction,
    op: wgpu::StencilOperation,
) -> wgpu::StencilFaceState {
    wgpu::StencilFaceState {
        compare,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: op,
        pass_op: op,
    }
}

/// The face every pass that leaves the stencil alone uses.
pub(super) const STENCIL_INERT: wgpu::StencilFaceState = wgpu::StencilFaceState {
    compare: wgpu::CompareFunction::Always,
    fail_op: wgpu::StencilOperation::Keep,
    depth_fail_op: wgpu::StencilOperation::Keep,
    pass_op: wgpu::StencilOperation::Keep,
};

/// The viewer's Depth24PlusStencil8 state with full read/write masks.
pub(super) fn depth_stencil(
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
    front: wgpu::StencilFaceState,
    back: wgpu::StencilFaceState,
) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth24PlusStencil8,
        depth_write_enabled: depth_write,
        depth_compare,
        stencil: wgpu::StencilState {
            front,
            back,
            read_mask: 0xFF,
            write_mask: 0xFF,
        },
        bias: Default::default(),
    }
}

/// The depth/stencil contract shared by every pass that must not occlude —
/// the grid/markers overlay pipeline, the compare ghost, and the
/// translucent part pipelines: depth-TESTED so the opaque mesh orders
/// these draws correctly, depth-write OFF so they occlude nothing after
/// them, stencil inert (the mesh pass's winding parity stays untouched for
/// whoever reads it next).
pub(super) fn overlay_depth_stencil() -> wgpu::DepthStencilState {
    depth_stencil(
        false,
        wgpu::CompareFunction::LessEqual,
        STENCIL_INERT,
        STENCIL_INERT,
    )
}

/// The staged part vertex as the part pipelines read it: position,
/// normal, color.
pub(super) const PART_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];

/// A position-only read of a vertex buffer (the cap quad, the ghost's
/// part vertices).
pub(super) const POSITION_ATTR: [wgpu::VertexAttribute; 1] =
    wgpu::vertex_attr_array![0 => Float32x3];

/// The overlay vertex: position, straight-alpha color.
pub(super) const OVERLAY_ATTRS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];

/// What differs between the viewer's render pipelines; everything else —
/// one vertex buffer, one color target at the frame's format, no culling,
/// the frame's sample count — is shared by [`build_pipeline`].
pub(super) struct PipelineSpec<'a> {
    pub(super) label: &'a str,
    pub(super) layout: &'a wgpu::PipelineLayout,
    pub(super) module: &'a wgpu::ShaderModule,
    pub(super) vs: &'a str,
    pub(super) fs: &'a str,
    pub(super) stride: usize,
    pub(super) attributes: &'a [wgpu::VertexAttribute],
    pub(super) topology: wgpu::PrimitiveTopology,
    pub(super) blend: Option<wgpu::BlendState>,
    /// Which faces are culled; `None` rasterizes both windings.
    pub(super) cull_mode: Option<wgpu::Face>,
    pub(super) depth_stencil: wgpu::DepthStencilState,
}

pub(super) fn build_pipeline(
    device: &wgpu::Device,
    spec: PipelineSpec<'_>,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(spec.label),
        layout: Some(spec.layout),
        vertex: wgpu::VertexState {
            module: spec.module,
            entry_point: Some(spec.vs),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: spec.stride as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: spec.attributes,
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: spec.module,
            entry_point: Some(spec.fs),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: spec.blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: spec.topology,
            cull_mode: spec.cull_mode,
            ..Default::default()
        },
        depth_stencil: Some(spec.depth_stencil),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    })
}

/// The sample-count-dependent render pipelines, rebuilt as a set by
/// [`build`] at creation and on every
/// [`Options::msaa`] flip in `prepare_parts`.
pub(super) struct Pipelines {
    /// Filled parts, writing the section cap's winding parity.
    pub(super) mesh: wgpu::RenderPipeline,
    /// The section cap quad, drawn where the parity is nonzero.
    pub(super) cap: wgpu::RenderPipeline,
    /// Wireframe hairlines (facts #44): the mesh shader at LineList,
    /// drawn instead of `mesh` in wireframe mode.
    pub(super) line: wgpu::RenderPipeline,
    /// The mesh/line pipelines' alpha-blended twins for the translucent
    /// part set (blend on, the overlay depth stance), drawn after the
    /// opaque set, before the cap.
    pub(super) mesh_blend: wgpu::RenderPipeline,
    pub(super) line_blend: wgpu::RenderPipeline,
    /// Unlit grid/bed/tripod and measure-marker overlay (facts #34/#37).
    pub(super) grid: wgpu::RenderPipeline,
    /// The compare ghost's unlit orange wireframe (ModelViewer.swift:
    /// 371-378) over the ghost scene's edge lists.
    pub(super) ghost: wgpu::RenderPipeline,
}

/// Blit stage for the msaa > 1 path: a fullscreen-textured-quad pipeline
/// that composites the resolved intermediate over the destination.
pub(super) struct Blit {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) sampler: wgpu::Sampler,
    pub(super) layout: wgpu::BindGroupLayout,
}

/// The seven render pipelines at `sample_count`, built against the SAME
/// pipeline layout objects the resident bind groups reference — a
/// rebuilt pipeline stays bind-compatible without re-staging anything.
pub(super) fn build(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    sample_count: u32,
    part_layout: &wgpu::PipelineLayout,
    camera_layout: &wgpu::PipelineLayout,
) -> Pipelines {
    use wgpu::CompareFunction::{Always, Less, LessEqual, NotEqual};
    use wgpu::PrimitiveTopology::{LineList, TriangleList, TriangleStrip};
    use wgpu::StencilOperation::{DecrementWrap, IncrementWrap};
    let module = |label: &str, source: String| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        })
    };
    let build = |spec| build_pipeline(device, spec, target_format, sample_count);
    let mesh_shader = module("mesh.wgsl", mesh_wgsl_source(target_format));
    // The four part pipelines share the mesh shader and the part vertex,
    // and differ in topology, blend, and depth/stencil.
    let part = |label, topology, blend, cull_mode, depth_stencil| {
        build(PipelineSpec {
            label,
            layout: part_layout,
            module: &mesh_shader,
            vs: "vs_main",
            fs: "fs_main",
            stride: std::mem::size_of::<Vertex>(),
            attributes: &PART_ATTRS,
            topology,
            blend,
            cull_mode,
            depth_stencil,
        })
    };
    // Filled parts. No culling: back faces must rasterize or their
    // decrement never happens and the winding parity breaks. Winding
    // parity for the section cap: every rasterized fragment flips the
    // stencil (front +1, back -1), including depth-occluded ones, so
    // after the pass stencil != 0 exactly where a ray from the camera
    // ends inside the sectioned solid — the cut face the cap quad
    // fills. Wrap (not Clamp) keeps 0-1 = 255 in the parity; fragments
    // discarded by the section plane never reach the stencil op, which
    // is what clips the count at the cut.
    let mesh = part(
        "mesh",
        TriangleList,
        None,
        None,
        depth_stencil(
            true,
            Less,
            stencil_face(Always, IncrementWrap),
            stencil_face(Always, DecrementWrap),
        ),
    );
    // Wireframe hairlines (facts #44): the SAME mesh shader, material,
    // and opacity as fill — only the topology changes (LineList;
    // wgpu/WebGPU fix line width at 1, so no wide lines and no tint).
    // Depth-tested like the fill so hidden edges stay hidden. Stencil
    // inert: winding parity is a filled-rasterization trick, and the
    // cap it feeds is disabled in wireframe anyway.
    let line = part(
        "wireframe lines",
        LineList,
        None,
        None,
        depth_stencil(true, Less, STENCIL_INERT, STENCIL_INERT),
    );
    // The translucent part set (motion playback, ModelViewer.swift:410):
    // the mesh and line materials with PREMULTIPLIED blending (fs_main's
    // glass branch writes lights::glass over model.alpha's coverage) and
    // the overlay depth stance — depth-TESTED so the opaque set orders
    // correctly, never WRITING depth so a translucent part occludes
    // nothing after it. Back faces are culled: upstream's playback
    // captures read one glass layer over the viewport (glass + 0.78 of
    // what lies behind, the same 0.78 in light and dark), never the far
    // faces composited under the near ones. The blend pipelines never
    // participate in section winding parity: section ∩ animation is empty
    // by mutual exclusion (the pane's isAnimating excludes sectioning).
    let glass = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
    let mesh_blend = part(
        "mesh blend",
        TriangleList,
        glass,
        Some(wgpu::Face::Back),
        overlay_depth_stencil(),
    );
    let line_blend = part(
        "wireframe lines blend",
        LineList,
        glass,
        None,
        overlay_depth_stencil(),
    );
    let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
    // The cap quad fills the cut face where the mesh pass left winding
    // parity. Its stencil test is read-only (all ops Keep) so the mesh's
    // inc/dec stays in the attachment; depth is read-only too (the quad
    // must not occlude anything drawn after it), and it wins LessEqual
    // only where the surviving mesh lies beyond the section plane.
    // Camera-only (camera_layout): the cap has no per-part transform.
    // The quad is 4 Z-ordered vertices drawn 0..4 (see `cap_quad`), so
    // the topology is a STRIP: a TriangleList would draw only the first
    // 3 of the 4 vertices, half the quad.
    let cap_shader = module("cap.wgsl", cap_wgsl_source(target_format));
    let not_zero = stencil_face(NotEqual, wgpu::StencilOperation::Keep);
    let cap = build(PipelineSpec {
        label: "section cap",
        layout: camera_layout,
        module: &cap_shader,
        vs: "vs_cap",
        fs: "fs_cap",
        stride: std::mem::size_of::<CapVertex>(),
        attributes: &POSITION_ATTR,
        topology: TriangleStrip,
        blend: None,
        cull_mode: None,
        depth_stencil: depth_stencil(false, LessEqual, not_zero, not_zero),
    });
    // Grid / bed / tripod / markers overlay (facts #34/#37): unlit
    // vertex colors, alpha-blended. Depth-tested so the model occludes
    // the overlay, but no depth write — an overlay must not occlude.
    let grid_shader = module("grid.wgsl", grid_wgsl_source(target_format));
    let grid = build(PipelineSpec {
        label: "grid overlay",
        layout: camera_layout,
        module: &grid_shader,
        vs: "vs_grid",
        fs: "fs_grid",
        stride: std::mem::size_of::<GridVertex>(),
        attributes: &OVERLAY_ATTRS,
        topology: TriangleList,
        blend,
        cull_mode: None,
        depth_stencil: overlay_depth_stencil(),
    });
    // The compare ghost (ModelViewer.swift:371-378): unlit systemOrange
    // wireframe at 0.6 — LINE topology like the wireframe pass but its
    // OWN unlit shader and blend (upstream's ghost is a different
    // material than the shaded wireframe: UnlitMaterial + transparent
    // blending). Camera-only (camera_layout): upstream parents
    // ghostEntity straight under `content`, so the ghost carries no
    // per-part pose. It reads only the position of the staged part
    // vertex. Depth ON / write OFF, the overlay contract.
    let ghost_shader = module("ghost.wgsl", ghost_wgsl_source(target_format));
    let ghost = build(PipelineSpec {
        label: "compare ghost",
        layout: camera_layout,
        module: &ghost_shader,
        vs: "vs_ghost",
        fs: "fs_ghost",
        stride: std::mem::size_of::<Vertex>(),
        attributes: &POSITION_ATTR,
        topology: LineList,
        blend,
        cull_mode: None,
        depth_stencil: overlay_depth_stencil(),
    });
    Pipelines {
        mesh,
        cap,
        line,
        mesh_blend,
        line_blend,
        grid,
        ghost,
    }
}

/// Blit pipeline for the msaa path: fullscreen triangle sampling the
/// resolved intermediate, drawn over the destination (`LoadOp::Load` +
/// premultiplied alpha, single-sample).
pub(super) fn blit(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Blit {
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
