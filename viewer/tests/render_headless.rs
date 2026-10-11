//! Headless render gate: drives the pure-wgpu core (`MeshPipeline`) surfaceless
//! — no iced, no window. The pipeline lifecycle mirrors iced's Storage: one
//! `MeshPipeline` per test, then per frame `prepare` (camera + mesh upload,
//! depth-texture realloc on size change) and `render` into a viewer-owned
//! target. Every later viewer test renders through `render_frame`.

use std::path::PathBuf;

use viewer::render::{Camera, MeshPipeline, TriMesh, default_camera, load_fixture_stl};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/golden")
        .join(rel)
}

async fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: None,
            ..Default::default()
        })
        .await
        .expect("surfaceless wgpu adapter (run on thelio; if none: check WGPU_BACKEND / Vulkan ICDs in this ssh session)");
    adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .unwrap()
}

/// One frame at `w` x `h`: `prepare` the pipeline, clear the target to the app
/// background, render the mesh pass on top, read the pixels back tightly
/// packed (`w * h * 4` RGBA). `queue.write_buffer` calls from `prepare` are
/// ordered before the submitted encoder's commands.
fn render_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    mesh: &TriMesh,
    camera: &Camera,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("headless target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    pipeline.prepare(device, queue, (w, h), camera, mesh);
    let mut encoder = device.create_command_encoder(&Default::default());
    // clear to the app background first, then viewer pass loads on top
    // (mirrors iced underlay)
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.1,
                        g: 0.1,
                        b: 0.12,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        drop(pass);
    }
    pipeline.render(&mut encoder, &view, [0.0, 0.0, w as f32, h as f32]);
    // buffer copies require bytes_per_row 256-aligned; 800 * 4 = 3200 is not,
    // so pad the rows and strip the padding on readback
    let stride = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("headless readback"),
        size: (stride * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let padded = slice.get_mapped_range().to_vec();
    readback.unmap();
    padded
        .chunks(stride as usize)
        .take(h as usize)
        .flat_map(|row| row[..w as usize * 4].iter().copied())
        .collect()
}

/// The app background, measured: render a frame with an empty mesh (the
/// pipeline's nothing-prepared path draws nothing) and take a corner pixel.
/// The 0.1/0.1/0.12 clear lands in the Rgba8UnormSrgb target sRGB-encoded —
/// measured (89, 89, 97), not the raw (26, 26, 31) — so the reference is
/// measured rather than hardcoded: against a wrong constant every background
/// pixel counts as ink and the ink threshold passes with nothing drawn.
fn background_reference(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    camera: &Camera,
    w: u32,
    h: u32,
) -> [u8; 4] {
    let empty = TriMesh {
        vertices: Vec::new(),
        aabb_min: [0.0; 3],
        aabb_max: [0.0; 3],
    };
    let frame = render_frame(device, queue, pipeline, &empty, camera, w, h);
    frame[0..4].try_into().unwrap()
}

/// Pixels that differ from the app background by more than 30 across RGB.
fn ink_pixels(px: &[u8], bg: [u8; 4]) -> usize {
    px.chunks(4)
        .filter(|c| {
            (c[0] as i32 - bg[0] as i32).abs()
                + (c[1] as i32 - bg[1] as i32).abs()
                + (c[2] as i32 - bg[2] as i32).abs()
                > 30
        })
        .count()
}

#[test]
fn mesh_renders_with_ink_and_is_frame_stable() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, queue) = rt.block_on(device());
    let mesh = load_fixture_stl(&fixture("model.stl"));
    let camera = default_camera(&mesh);
    // one pipeline across both frames (as iced's Storage holds it for the
    // app's lifetime): per-frame depth/stencil state that fails to clear
    // leaks into the second frame
    let mut pipeline = MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(&device, &queue, &mut pipeline, &camera, 800, 600);
    let a = render_frame(&device, &queue, &mut pipeline, &mesh, &camera, 800, 600);
    assert!(
        ink_pixels(&a, bg) > 5_000,
        "mesh should cover a healthy area, got {}",
        ink_pixels(&a, bg)
    );
    let b = render_frame(&device, &queue, &mut pipeline, &mesh, &camera, 800, 600);
    assert_eq!(
        a, b,
        "second frame must be identical — uncleared stencil/depth leaks state (Review Focus 3)"
    );
}

#[test]
fn viewport_resize_reallocates_depth_and_renders() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, queue) = rt.block_on(device());
    let mesh = load_fixture_stl(&fixture("model.stl"));
    let camera = default_camera(&mesh);
    // one pipeline through both sizes: prepare() must reallocate the depth
    // texture when the viewport changes
    let mut pipeline = MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(&device, &queue, &mut pipeline, &camera, 800, 600);
    let small = render_frame(&device, &queue, &mut pipeline, &mesh, &camera, 800, 600);
    let big = render_frame(&device, &queue, &mut pipeline, &mesh, &camera, 1024, 768);
    assert!(ink_pixels(&small, bg) > 5_000);
    let growth = ink_pixels(&big, bg) as f64 / ink_pixels(&small, bg) as f64;
    assert!(
        (1.2..1.9).contains(&growth),
        "1024x768 vs 800x600 ink ratio {growth:.2} should track area ratio ~1.64 (Review Focus 4)"
    );
}
