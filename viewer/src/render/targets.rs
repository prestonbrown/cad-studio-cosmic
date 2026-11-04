//! The viewer-owned attachments: the depth/stencil texture and the msaa
//! color + resolve pair, rebuilt on resize and sample-count change.

use super::pipelines::Blit;

/// Owned multisample targets, recreated in [`MeshPipeline::prepare_parts`]
/// only on size change (never per frame). `color` is the sample-`count`
/// attachment the mesh pass renders into; `resolve` is the single-sample
/// intermediate the pass resolves into and the blit samples.
pub(super) struct MsaaTargets {
    pub(super) color: wgpu::TextureView,
    pub(super) resolve: wgpu::TextureView,
    pub(super) bind_group: wgpu::BindGroup,
}

pub(super) fn msaa(
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

pub(super) fn depth(
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
