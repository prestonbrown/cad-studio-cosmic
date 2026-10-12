//! Headless render gate: drives the pure-wgpu core (`MeshPipeline`) surfaceless
//! — no iced, no window. The pipeline lifecycle mirrors iced's Storage: one
//! `MeshPipeline` per test, then per frame `prepare_parts` (camera uniform +
//! upload-once part residency, depth-texture realloc on size change) and
//! `render` into a viewer-owned target. Every later viewer test renders
//! through `render_frame`.

use std::path::PathBuf;

use engine::parsing::threemf::parse_3mf;
use viewer::render::{Camera, MeshPipeline, PartScene, NEUTRAL, default_camera};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/golden")
        .join(rel)
}

/// The golden model through the production parse path: model.3mf is two
/// parts (base + lid) of 12 triangles each.
fn golden_scene() -> PartScene {
    let bytes = std::fs::read(fixture("model.3mf"))
        .unwrap_or_else(|e| panic!("read fixture model.3mf: {e}"));
    let parts = parse_3mf(&bytes).expect("golden model.3mf parses");
    PartScene::from_parts(&parts, NEUTRAL)
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

/// One frame at `w` x `h`: `prepare_parts` the pipeline, clear the target to
/// the app background, render the part pass on top, read the pixels back
/// tightly packed (`w * h * 4` RGBA). `queue.write_buffer` calls from
/// `prepare_parts` are ordered before the submitted encoder's commands.
fn render_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
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
    pipeline.prepare_parts(device, queue, (w, h), camera, scene);
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

/// `render_frame` with the pipeline built at `msaa` samples per pixel
/// (`MeshPipeline::with_msaa`): a fresh pipeline per call, so frames at
/// different sample counts are independent.
fn render_frame_msaa(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &PartScene,
    camera: &Camera,
    w: u32,
    h: u32,
    msaa: u32,
) -> Vec<u8> {
    let mut pipeline =
        MeshPipeline::with_msaa(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, msaa);
    render_frame(device, queue, &mut pipeline, scene, camera, w, h)
}

/// The app background, measured: render a frame with an empty scene (the
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
    let empty = PartScene::from_parts(&[], NEUTRAL);
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
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    // one pipeline across both frames (as iced's Storage holds it for the
    // app's lifetime): per-frame depth/stencil state that fails to clear
    // leaks into the second frame — and the second frame re-uploads
    // nothing, so any geometry difference would surface here too
    let mut pipeline = MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(&device, &queue, &mut pipeline, &camera, 800, 600);
    let a = render_frame(&device, &queue, &mut pipeline, &scene, &camera, 800, 600);
    assert!(
        ink_pixels(&a, bg) > 5_000,
        "mesh should cover a healthy area, got {}",
        ink_pixels(&a, bg)
    );
    let b = render_frame(&device, &queue, &mut pipeline, &scene, &camera, 800, 600);
    assert_eq!(
        a, b,
        "second frame must be identical — uncleared stencil/depth leaks state (Review Focus 3)"
    );
}

/// Pixels in the section-cap red band. The target is Rgba8UnormSrgb, so
/// readback values are sRGB-ENCODED, not linear: the cap fragment's linear
/// (0.92, 0.15, 0.10) reads back as (246, 108, 89) — the brief's linear-g
/// `g < 90` band misfires on exactly that. Calibrated from a measured
/// histogram: cap (246,108,89); mesh faces (98,104,111), (103,109,116),
/// (86,91,97); background (89,89,97). r > 180 alone separates the cap from
/// every measured mesh/background value with >70 of headroom; the g/b caps
/// also exclude any future bright-but-not-red surface.
fn cap_red_pixels(px: &[u8]) -> usize {
    px.chunks(4).filter(|c| c[0] > 180 && c[1] < 150 && c[2] < 120).count()
}

/// Section cap: cutting the fixture along the model's Z stack must draw the
/// red cap face where the mesh pass left winding parity, and only there.
/// The fixture is two disjoint boxes — base 60x60 MODEL z∈[-4,4], lid 40x40
/// MODEL z∈[12,22] — with 8mm of air between, so "mid-plane" means the
/// middle of a box and "toward the edge" means the smaller lid and the
/// empty gap above the base. MODEL z maps to SCENE y (the remap in
/// `from_parts`), so the cut axis in scene space is 1; the floor pin then
/// puts the pinned aabb at y∈[0,26] with base y∈[0,8], lid y∈[16,26], gap
/// (8,16).
#[test]
fn section_cap_appears_and_shrinks_toward_the_edge() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, queue) = rt.block_on(device());
    let scene = golden_scene();
    // one pipeline across every cut (iced Storage lifetime): stencil state
    // must not leak between frames
    let mut pipeline = MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut cam = default_camera(&scene, 800.0 / 600.0);
    cam.section_enable = 1.0;
    cam.section_axis = 1; // SCENE y — the MODEL z stack after the remap
    let span = scene.aabb_max[1] - scene.aabb_min[1];
    let mut cap_at = |frac: f32| {
        cam.section_pos = scene.aabb_min[1] + span * frac;
        cap_red_pixels(&render_frame(&device, &queue, &mut pipeline, &scene, &cam, 800, 600))
    };
    let base = cap_at(4.0 / 26.0); // pinned y=4: middle of the 60mm base box
    let lid = cap_at(21.0 / 26.0); // pinned y=21: middle of the 40mm lid box
    let gap = cap_at(0.5); // pinned y=13: the air gap between the two parts
    // re-measured after T2 (pin + diagonal framing, camera at distance
    // ~170 vs ~115 before): base-mid 26450 red px, lid-mid 12374, gap 0
    assert!(
        base > 2_000,
        "cap must be visible at mid-plane, got {base} red px"
    );
    assert!(
        lid < base * 3 / 4,
        "cap must shrink on the smaller lid cross-section: base {base} vs lid {lid}"
    );
    assert!(
        gap < base / 100,
        "plane through the gap must paint no cap: {gap} red px"
    );
}

#[test]
fn viewport_resize_reallocates_depth_and_renders() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, queue) = rt.block_on(device());
    let scene = golden_scene();
    // one pipeline through both sizes: prepare_parts() must reallocate the
    // depth texture when the viewport changes; each size gets its own
    // aspect-correct camera (both are 4:3, so the framing is identical and
    // only pixel density changes)
    let mut pipeline = MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let camera = default_camera(&scene, 800.0 / 600.0);
    let bg = background_reference(&device, &queue, &mut pipeline, &camera, 800, 600);
    let small = render_frame(&device, &queue, &mut pipeline, &scene, &camera, 800, 600);
    let camera_big = default_camera(&scene, 1024.0 / 768.0);
    let big = render_frame(&device, &queue, &mut pipeline, &scene, &camera_big, 1024, 768);
    assert!(ink_pixels(&small, bg) > 5_000);
    let growth = ink_pixels(&big, bg) as f64 / ink_pixels(&small, bg) as f64;
    assert!(
        (1.2..1.9).contains(&growth),
        "1024x768 vs 800x600 ink ratio {growth:.2} should track area ratio ~1.64 (Review Focus 4)"
    );
}

/// Aspect comes from the viewport (Review Focus 3, the discriminator): the
/// same model, framed identically (framing is aspect-free), rendered at
/// 800×600 (4:3) and 1280×600 (≈2.13:1) — equal heights. With a vertical
/// FOV a point's pixel x offset scales as h / (2·tan(fov/2))·aspect_factor
/// — with the aspect correctly taken from the viewport, the ink bbox WIDTH
/// in pixels is identical at both sizes. Pixel HEIGHT is aspect-blind and
/// would pass vacuously; under the 16:9 hardcode this task kills, the
/// width ratio reads ~1.6.
#[test]
fn aspect_from_viewport() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, queue) = rt.block_on(device());
    let scene = golden_scene();
    let mut pipeline = MeshPipeline::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let ink_bbox_width = |pipeline: &mut MeshPipeline, w: u32, h: u32| -> usize {
        let camera = default_camera(&scene, w as f32 / h as f32);
        let bg = background_reference(&device, &queue, pipeline, &camera, w, h);
        let px = render_frame(&device, &queue, pipeline, &scene, &camera, w, h);
        assert!(
            ink_pixels(&px, bg) > 5_000,
            "model must be on screen at {w}x{h}"
        );
        let mut min_x = usize::MAX;
        let mut max_x = 0usize;
        for (i, c) in px.chunks(4).enumerate() {
            let ink = (c[0] as i32 - bg[0] as i32).abs()
                + (c[1] as i32 - bg[1] as i32).abs()
                + (c[2] as i32 - bg[2] as i32).abs()
                > 30;
            if ink {
                min_x = min_x.min(i % w as usize);
                max_x = max_x.max(i % w as usize);
            }
        }
        assert!(min_x <= max_x, "ink bbox must be non-empty");
        max_x - min_x + 1
    };
    let narrow = ink_bbox_width(&mut pipeline, 800, 600);
    let wide = ink_bbox_width(&mut pipeline, 1280, 600);
    let ratio = wide as f64 / narrow as f64;
    // measured on Metal: identical ink counts (31834) and bbox widths
    // (468 px) at both sizes — ratio 1.000; the 5% band only absorbs
    // driver rounding. Under the 16:9 hardcode this same render measures
    // 1.606 (verified against this test during T2).
    assert!(
        (0.95..=1.05).contains(&ratio),
        "ink bbox width ratio {ratio:.3} must be ~1.0 — viewport aspect must drive the projection (16:9 hardcode reads ~1.6)"
    );
}

/// Luminance band isolating anti-aliased edge pixels. Readback is
/// sRGB-encoded (task-8 measurements): background (89,89,97) lum 91; mesh
/// faces (98,104,111) lum 104, (103,109,116) lum 109, (86,91,97) lum 91.
/// The dark face sits at background luminance, so the band tracks the
/// high-contrast silhouette against the two lit faces. Measured 4x coverage
/// blends (resolve box-filters 4 samples): 1/4..1/2 coverage lands at lum
/// 95..101 — in band; 3/4 blends land at lum 104..105, colliding with the
/// pure mid-face luminance, so the band excludes them on purpose; dark-face
/// blends stay under lum 92. Band 93..=103 excludes every pure color with
/// >=2 lum headroom; at 1x (no AA) it counts exactly 0 pixels.
fn intermediate_edge_pixels(px: &[u8]) -> usize {
    px.chunks(4)
        .filter(|c| {
            let lum = (c[0] as u32 + c[1] as u32 + c[2] as u32) / 3;
            (93..=103).contains(&lum)
        })
        .count()
}

/// 4x MSAA smooths the box silhouettes: the default iso camera puts
/// high-contrast diagonal edges mid-screen, and resolving 4 samples per
/// pixel yields coverage-blend pixels along them where 1x snaps each pixel
/// to pure background or pure face color. Also proves the section-cap
/// pipeline runs at count 4 inside the same multisample pass — wgpu
/// validation rejects a pipeline whose multisample state mismatches the
/// attachment, so drawing it at all is the check — and that per-frame
/// depth/stencil clears hold through the resolve+blit path.
#[test]
fn msaa_smooths_a_diagonal_edge() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, queue) = rt.block_on(device());
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    let px1 = render_frame_msaa(&device, &queue, &scene, &camera, 800, 600, 1);
    let px4 = render_frame_msaa(&device, &queue, &scene, &camera, 800, 600, 4);
    let (e1, e4) = (intermediate_edge_pixels(&px1), intermediate_edge_pixels(&px4));
    assert!(
        e4 as f64 > e1 as f64 * 1.5,
        "4x MSAA should produce >=1.5x the intermediate-valued edge pixels: {e4} vs {e1}"
    );
    // measured 1199 in-band pixels at 4x on Metal with the parity framing
    // (T2 re-measurement: the diagonal-radius framing pulled the camera to
    // distance ~170, shrinking the silhouette ~0.68x vs the old 1551); the
    // floor guards "real AA happened", the ratio above guards "at least
    // 1.5x better than 1x"
    assert!(e4 > 600, "4x blend count {e4} should be substantial");
    // one pipeline across two frames at 4x: uncleared stencil/depth would
    // leak into the second frame through the resolve path too
    let mut p4 =
        MeshPipeline::with_msaa(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, 4);
    let a = render_frame(&device, &queue, &mut p4, &scene, &camera, 800, 600);
    let b = render_frame(&device, &queue, &mut p4, &scene, &camera, 800, 600);
    assert_eq!(a, b, "4x frames must be identical on one pipeline");
    // cap at 4x, mid-base cut (fixture geometry: base box MODEL z in
    // [-4,4]; the pin lifts the floor to y=0, so the base spans pinned
    // y in [0,8] and its middle is y=4)
    let mut cam = camera;
    cam.section_enable = 1.0;
    cam.section_axis = 1;
    cam.section_pos = 4.0;
    let cut = render_frame_msaa(&device, &queue, &scene, &cam, 800, 600, 4);
    assert!(
        cap_red_pixels(&cut) > 2_000,
        "section cap must render through the 4x path: {} red px",
        cap_red_pixels(&cut)
    );
}
