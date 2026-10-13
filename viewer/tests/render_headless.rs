//! Headless render gate: drives the pure-wgpu core (`MeshPipeline`) surfaceless
//! — no iced, no window. The pipeline lifecycle mirrors iced's Storage: one
//! `MeshPipeline` per test, then per frame `prepare_parts` (camera uniform +
//! upload-once part residency, depth-texture realloc on size change) and
//! `render` into a viewer-owned target. Every later viewer test renders
//! through `render_frame`.

use std::path::PathBuf;

use engine::parsing::stl::parse_stl;
use engine::parsing::threemf::parse_3mf;
use viewer::render::camera;
use viewer::render::gpu;
use viewer::render::grid;
use viewer::render::lights;
use viewer::render::{
    Camera, Frame, MeshPipeline, NEUTRAL, Options, PartPose, PartScene, default_camera,
    srgb_to_linear,
};

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

/// One frame at `w` x `h`: `prepare_parts` the pipeline, clear the target to
/// the app background, render the part pass on top, read the pixels back
/// tightly packed (`w * h * 4` RGBA). `queue.write_buffer` calls from
/// `prepare_parts` are ordered before the submitted encoder's commands.
/// No overlay, solid mode — the common case; [`render_frame_opts`] carries
/// the grid and render options.
fn render_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    camera: &Camera,
    w: u32,
    h: u32,
) -> Vec<u8> {
    render_frame_opts(
        device,
        queue,
        pipeline,
        scene,
        camera,
        w,
        h,
        None,
        &Options::default(),
    )
}

/// [`render_frame`] with the overlay and render mode: `grid` (when `Some`)
/// is prepared and drawn after the parts, `options` picks e.g. wireframe.
/// Poses are identity — the pose-carrying entry is [`render_frame_poses`].
fn render_frame_opts(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    camera: &Camera,
    w: u32,
    h: u32,
    grid: Option<&grid::GridMesh>,
    options: &Options,
) -> Vec<u8> {
    let poses = vec![Some(glam::Mat4::IDENTITY.to_cols_array_2d()); scene.parts.len()];
    render_frame_with_poses(
        device, queue, pipeline, scene, camera, w, h, grid, options, &poses,
    )
}

/// The harness core: [`render_frame_opts`] plus the per-part poses threaded
/// into the `Frame` — every pose-path test renders through here.
fn render_frame_with_poses(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    camera: &Camera,
    w: u32,
    h: u32,
    grid: Option<&grid::GridMesh>,
    options: &Options,
    poses: &[PartPose],
) -> Vec<u8> {
    let frame = Frame {
        camera,
        scene,
        poses,
        effective_aabb: (scene.aabb_min, scene.aabb_max),
        grid,
        options: *options,
    };
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
    pipeline.prepare_parts(device, queue, (w, h), &frame);
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
/// different sample counts are independent. The options carry `msaa`
/// explicitly — `Options::msaa` owns the count from the first prepare on,
/// so default options would flip a fresh 4x pipeline back to 1x before it
/// ever rendered multisampled (exactly the regression that zeroed this
/// suite's blend counts).
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
    let poses = vec![Some(glam::Mat4::IDENTITY.to_cols_array_2d()); scene.parts.len()];
    render_frame_with_poses(
        device,
        queue,
        &mut pipeline,
        scene,
        camera,
        w,
        h,
        None,
        &Options {
            msaa,
            ..Options::default()
        },
        &poses,
    )
}

/// The pose path's render entry — the common case with explicit per-part
/// poses: default camera for the viewport, no overlay, solid mode. (The
/// upload-count half of the pose law is asserted in-crate, where the
/// counter accessors are visible.)
fn render_frame_poses(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    poses: &[PartPose],
    w: u32,
    h: u32,
) -> Vec<u8> {
    let camera = default_camera(scene, w as f32 / h as f32);
    render_frame_with_poses(
        device,
        queue,
        pipeline,
        scene,
        &camera,
        w,
        h,
        None,
        &Options::default(),
        poses,
    )
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

/// Is this pixel ink (differs from the app background by more than 30)?
fn is_ink(c: &[u8], bg: [u8; 4]) -> bool {
    (c[0] as i32 - bg[0] as i32).abs()
        + (c[1] as i32 - bg[1] as i32).abs()
        + (c[2] as i32 - bg[2] as i32).abs()
        > 30
}

/// Pixels that differ from the app background by more than 30 across RGB.
fn ink_pixels(px: &[u8], bg: [u8; 4]) -> usize {
    px.chunks(4).filter(|c| is_ink(c, bg)).count()
}

/// Linear decode of an sRGB-encoded readback pixel — the same EOTF the
/// staging applies, run on the stored byte. Readback is sRGB-encoded
/// (Rgba8UnormSrgb target), so classification happens in linear space.
fn decode_linear(c: &[u8]) -> [f32; 3] {
    [
        srgb_to_linear(c[0]),
        srgb_to_linear(c[1]),
        srgb_to_linear(c[2]),
    ]
}

/// Chromaticity: the linear color scaled so its max component is 1.
/// mesh.wgsl multiplies a per-face constant (`lit`) into the vertex color,
/// and that scalar divides out here — so every face of a part, whatever its
/// orientation, classifies against the same normalized expected color.
fn chroma(c: [f32; 3]) -> [f32; 3] {
    let m = c[0].max(c[1]).max(c[2]).max(1e-6);
    [c[0] / m, c[1] / m, c[2] / m]
}

fn chroma_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The golden model's two part chromas — fixture bytes (base #1F78B5,
/// lid #D62629; model.3mf basematerials) through the same EOTF the staging
/// applies, normalized so per-face lighting divides out.
fn fixture_chromas() -> ([f32; 3], [f32; 3]) {
    (
        chroma([31, 120, 181].map(srgb_to_linear)),
        chroma([214, 38, 41].map(srgb_to_linear)),
    )
}

#[test]
fn mesh_renders_with_ink_and_is_frame_stable() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    // one pipeline across both frames (as iced's Storage holds it for the
    // app's lifetime): per-frame depth/stencil state that fails to clear
    // leaks into the second frame — and the second frame re-uploads
    // nothing, so any geometry difference would surface here too
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let a = render_frame(device, queue, &mut pipeline, &scene, &camera, 800, 600);
    assert!(
        ink_pixels(&a, bg) > 5_000,
        "mesh should cover a healthy area, got {}",
        ink_pixels(&a, bg)
    );
    let b = render_frame(device, queue, &mut pipeline, &scene, &camera, 800, 600);
    assert_eq!(
        a, b,
        "second frame must be identical — uncleared stencil/depth leaks state (Review Focus 3)"
    );
}

/// Pixels in the section-cap red band. The target is Rgba8UnormSrgb, so
/// readback values are sRGB-ENCODED, not linear: the cap fragment's linear
/// (0.92, 0.15, 0.10) reads back as (246, 108, 89) — the brief's linear-g
/// `g < 90` band misfires on exactly that. Calibrated from a measured
/// histogram (T4, per-part colors under two-light shading): cap
/// (246,108,89) — the cap shader is unlit, so it alone is
/// shading-invariant; blue base faces (11,61,95), (13,67,103), (16,75,115);
/// red lid faces (113,15,16), (123,17,19), (137,21,23); background
/// (89,89,97). The reddest lit face tops out at r=137, so r > 180
/// separates the cap from every measured mesh/background value with >40 of
/// headroom; b < 120 clears the brightest blue face's 115 by 5; the g cap
/// excludes any future bright-but-not-red surface.
fn cap_red_pixels(px: &[u8]) -> usize {
    px.chunks(4)
        .filter(|c| c[0] > 180 && c[1] < 150 && c[2] < 120)
        .count()
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
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    // one pipeline across every cut (iced Storage lifetime): stencil state
    // must not leak between frames
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut cam = default_camera(&scene, 800.0 / 600.0);
    cam.section_enable = 1.0;
    cam.section_axis = 1; // SCENE y — the MODEL z stack after the remap
    let span = scene.aabb_max[1] - scene.aabb_min[1];
    let mut cap_at = |frac: f32| {
        cam.section_pos = scene.aabb_min[1] + span * frac;
        cap_red_pixels(&render_frame(
            device,
            queue,
            &mut pipeline,
            &scene,
            &cam,
            800,
            600,
        ))
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
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    // one pipeline through both sizes: prepare_parts() must reallocate the
    // depth texture when the viewport changes; each size gets its own
    // aspect-correct camera (both are 4:3, so the framing is identical and
    // only pixel density changes)
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let camera = default_camera(&scene, 800.0 / 600.0);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let small = render_frame(device, queue, &mut pipeline, &scene, &camera, 800, 600);
    let camera_big = default_camera(&scene, 1024.0 / 768.0);
    let big = render_frame(device, queue, &mut pipeline, &scene, &camera_big, 1024, 768);
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
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let ink_bbox_width = |pipeline: &mut MeshPipeline, w: u32, h: u32| -> usize {
        let camera = default_camera(&scene, w as f32 / h as f32);
        let bg = background_reference(device, queue, pipeline, &camera, w, h);
        let px = render_frame(device, queue, pipeline, &scene, &camera, w, h);
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
    // measured on Metal: identical ink counts (98560) and bbox widths
    // (468 px) at both sizes — ratio 1.000; the 5% band only absorbs
    // driver rounding. (T3 re-measure: per-part colors made the dark-blue
    // face ink-visible, lifting the count from the neutral-era 31834; the
    // bbox width is unchanged.) Under the 16:9 hardcode this same render
    // measures 1.606 (verified against this test during T2).
    assert!(
        (0.95..=1.05).contains(&ratio),
        "ink bbox width ratio {ratio:.3} must be ~1.0 — viewport aspect must drive the projection (16:9 hardcode reads ~1.6)"
    );
}

/// Coverage-blend pixels of the 4x resolve (the msaa test): pixels whose
/// chromaticity sits more than 0.05 from EVERY pure chroma of the frame —
/// the two part colors and the background. Chroma, not luminance: with
/// two-light shading (T4 re-measure) the six pure faces span readback
/// values (11,61,95)..(16,75,115) for the blue base and
/// (113,15,16)..(137,21,23) for the red lid beside the (89,89,97)
/// background — pure faces bracket the background on every channel, so no
/// luminance band separates blends from pures. Chroma is
/// lighting-invariant (the per-face `lit` scalar divides out), so every
/// pure pixel lands within 0.0074 of its part's chroma and a
/// silhouette-edge blend — resolve box-filters the covered samples — lands
/// strictly between endpoint chromas. Measured on Metal: exactly 0
/// in-band pixels at 1x (unblended, single-sample), 1153 at 4x — the same
/// count T3 measured, the blended edge set being geometric while shading
/// moved only the luminances; the 0.05 cut is ~7x the measured pure-pixel
/// noise.
fn blend_pixels(px: &[u8], bg: [u8; 4], blue: [f32; 3], red: [f32; 3]) -> usize {
    let bgc = chroma(decode_linear(&bg));
    px.chunks(4)
        .filter(|c| {
            let x = chroma(decode_linear(c));
            chroma_dist(x, bgc) > 0.05 && chroma_dist(x, blue) > 0.05 && chroma_dist(x, red) > 0.05
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
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    // the two part chromas and a measured background — same clear + format
    // at any sample count
    let (blue, red) = fixture_chromas();
    let mut bg_pipe = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut bg_pipe, &camera, 800, 600);
    let px1 = render_frame_msaa(device, queue, &scene, &camera, 800, 600, 1);
    let px4 = render_frame_msaa(device, queue, &scene, &camera, 800, 600, 4);
    let (e1, e4) = (
        blend_pixels(&px1, bg, blue, red),
        blend_pixels(&px4, bg, blue, red),
    );
    // at 1x the pipeline writes unblended single-sample fragments, so every
    // pixel IS a pure chroma — anything else is a blend/state leak
    assert_eq!(e1, 0, "1x frame must contain only pure-chroma pixels");
    // measured 1153 blend pixels at 4x on Metal — unchanged by two-light
    // shading (T4 re-measure: the blended edge set is geometric; T3's
    // luminance-band predecessor read 1199 at T2 until per-part colors
    // killed that band and the chroma metric replaced it); the floor
    // guards "real AA happened"
    assert!(e4 > 600, "4x blend count {e4} should be substantial");
    // one pipeline across two frames at 4x: uncleared stencil/depth would
    // leak into the second frame through the resolve path too. The options
    // must ask for 4x — with defaults the prepare would flip p4 to 1x and
    // the frame-stability check would stop exercising the resolve path.
    let mut p4 = MeshPipeline::with_msaa(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, 4);
    let at4 = &Options {
        msaa: 4,
        ..Options::default()
    };
    let a = render_frame_opts(device, queue, &mut p4, &scene, &camera, 800, 600, None, at4);
    let b = render_frame_opts(device, queue, &mut p4, &scene, &camera, 800, 600, None, at4);
    assert_eq!(a, b, "4x frames must be identical on one pipeline");
    // cap at 4x, mid-base cut (fixture geometry: base box MODEL z in
    // [-4,4]; the pin lifts the floor to y=0, so the base spans pinned
    // y in [0,8] and its middle is y=4)
    let mut cam = camera;
    cam.section_enable = 1.0;
    cam.section_axis = 1;
    cam.section_pos = 4.0;
    let cut = render_frame_msaa(device, queue, &scene, &cam, 800, 600, 4);
    assert!(
        cap_red_pixels(&cut) > 2_000,
        "section cap must render through the 4x path: {} red px",
        cap_red_pixels(&cut)
    );
}

/// Per-part colors reach pixels (Review Focus 4). The golden model paints
/// base #1F78B5 and lid #D62629; every ink pixel of a 1x frame (no blends —
/// the pipeline writes unblended, single-sample) must classify, by
/// chromaticity-normalized nearest of the two expected linear colors, as
/// one of them. The SPATIAL anchor catches what nearest-only cannot —
/// swapped assignments: the lid (red) is the upper box in the fixture's
/// z-stack (MODEL z 12..22 over base z -4..4; scene y after the remap), so
/// the upper third of the ink bbox must classify red and the lower third
/// blue.
#[test]
fn two_parts_two_colors() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let px = render_frame(device, queue, &mut pipeline, &scene, &camera, 800, 600);
    let n_ink = ink_pixels(&px, bg);
    assert!(n_ink > 5_000, "model must cover a healthy area: {n_ink}");
    let (blue, red) = fixture_chromas();
    // Measured on Metal (T4, two-light shading): a 1x frame has exactly 6
    // distinct ink values — three blue faces and three red faces, each
    // within 0.0074 of its part's chroma (u8 quantization noise), the
    // nearest WRONG color 1.42 away. TOL 0.05 holds ~7x noise headroom and
    // 28x wrong-color margin.
    const TOL: f32 = 0.05;
    let mut classified = 0usize;
    let (mut min_row, mut max_row) = (usize::MAX, 0usize);
    for (i, c) in px.chunks(4).enumerate() {
        if !is_ink(c, bg) {
            continue;
        }
        let row = i / 800;
        min_row = min_row.min(row);
        max_row = max_row.max(row);
        let x = chroma(decode_linear(c));
        if chroma_dist(x, blue) < TOL || chroma_dist(x, red) < TOL {
            classified += 1;
        }
    }
    assert!(
        classified as f64 >= 0.95 * n_ink as f64,
        "only {classified}/{n_ink} ink pixels classify as base-blue or lid-red"
    );
    // spatial anchor: thirds of the ink bbox's row span
    let span = max_row - min_row + 1;
    let (mut top_red, mut top_any) = (0usize, 0usize);
    let (mut bot_blue, mut bot_any) = (0usize, 0usize);
    for (i, c) in px.chunks(4).enumerate() {
        if !is_ink(c, bg) {
            continue;
        }
        let row = i / 800;
        let x = chroma(decode_linear(c));
        let (d_blue, d_red) = (chroma_dist(x, blue), chroma_dist(x, red));
        let near = |d: f32| d < TOL;
        if row - min_row < span / 3 {
            if near(d_blue) || near(d_red) {
                top_any += 1;
                if near(d_red) {
                    top_red += 1;
                }
            }
        } else if max_row - row < span / 3 {
            if near(d_blue) || near(d_red) {
                bot_any += 1;
                if near(d_blue) {
                    bot_blue += 1;
                }
            }
        }
    }
    assert!(
        top_any > 500 && top_red * 100 >= top_any * 90,
        "upper third of the ink bbox must be the red lid: {top_red}/{top_any}"
    );
    assert!(
        bot_any > 500 && bot_blue * 100 >= bot_any * 90,
        "lower third of the ink bbox must be the blue base: {bot_blue}/{bot_any}"
    );
    // measured: 100% on both (28182/28182 red upper, 19862/19862 blue
    // lower) — the 90% bars leave room for driver raster drift
}

/// Uncolored geometry paints the neutral: the golden STL carries no colors,
/// so every ink pixel's chromaticity must sit on the neutral's (the
/// per-face lighting scalar divides out). Guards the None branch of the
/// staging against a future default-color or passthrough regression.
#[test]
fn neutral_when_uncolored() {
    let (device, queue) = gpu::device_queue();
    let bytes = std::fs::read(fixture("model.stl"))
        .unwrap_or_else(|e| panic!("read fixture model.stl: {e}"));
    let parts = vec![parse_stl(&bytes).expect("golden model.stl parses")];
    assert!(
        parts[0].triangle_colors.iter().all(|c| c.is_none()),
        "fixture truth: the STL carries no colors"
    );
    let scene = PartScene::from_parts(&parts, NEUTRAL);
    let camera = default_camera(&scene, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let px = render_frame(device, queue, &mut pipeline, &scene, &camera, 800, 600);
    let n_ink = ink_pixels(&px, bg);
    assert!(n_ink > 5_000, "model must cover a healthy area: {n_ink}");
    let neutral = chroma(NEUTRAL);
    // Measured on Metal (T4, two-light shading): max chroma distance over
    // all 98560 ink pixels is 0.0115 (u8 quantization on the lit-neutral
    // faces — ambient-only faces are dark and quantize coarser); TOL 0.05
    // is >4x that. A per-triangle default color or sRGB passthrough lands
    // 0.1+ away and fails.
    const TOL: f32 = 0.05;
    let near = px
        .chunks(4)
        .filter(|c| is_ink(c, bg))
        .filter(|c| chroma_dist(chroma(decode_linear(c)), neutral) < TOL)
        .count();
    assert!(
        near as f64 >= 0.99 * n_ink as f64,
        "only {near}/{n_ink} ink pixels sit on the neutral chromaticity"
    );
}

/// Two triangles of a planar quad, both winding the same way (so one face
/// normal).
fn quad(positions: &mut Vec<[f32; 3]>, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
    positions.extend_from_slice(&[a, b, c, a, c, d]);
}

/// A white cube rotated 45° about MODEL Z (corners on the axes), so the
/// remap lands its side-face normals at scene azimuths 315°/45°/135°/225°.
/// The rotation is the point: with an axis-aligned cube no single camera
/// sees two faces the shading constants separate by 2x — the top face is
/// the brightest at 0.26·0.7428+0.09·0.3468+0.15 ≈ 0.374, and the dimmest
/// side face any above-horizon camera can see alongside it lights to
/// 0.1968, a 1.90x ratio. Viewed from scene yaw 3π/4, the 315°-normal
/// side faces the camera dead-on and sits >90° from BOTH light directions
/// — ambient only, lit = 0.15 — while the top face takes
/// key+fill+ambient; every other side is exactly edge-on. White
/// (`Some([255,255,255])` is exact linear (1,1,1)) makes readback
/// luminance equal the light `lights::lit` computes, with no albedo to
/// divide out.
fn white_diamond_cube() -> engine::parsing::Part {
    let h = 20.0;
    let ring = |z: f32| [[h, 0.0, z], [0.0, h, z], [-h, 0.0, z], [0.0, -h, z]];
    let (bot, top) = (ring(-h), ring(h));
    let mut positions = Vec::new();
    quad(&mut positions, bot[0], bot[1], bot[2], bot[3]); // floor
    quad(&mut positions, top[0], top[1], top[2], top[3]); // ceiling
    for i in 0..4 {
        let j = (i + 1) % 4;
        quad(&mut positions, bot[i], bot[j], top[j], top[i]); // sides
    }
    engine::parsing::Part {
        name: "cube".into(),
        positions,
        triangle_colors: vec![Some([255, 255, 255]); 12],
    }
}

/// Real normals + the two-light model reach pixels (Task 4): key-facing vs
/// away-facing regions of a white cube differ in readback luminance by
/// >=2x, and each region sits in the band derived from the shading
/// constants — `lights::lit` of the face's scene normal, quantized through
/// the target's u8 sRGB storage by nearest decoded byte. No measured
/// constants anywhere: the bands come from the 0.26/0.09/0.15 coefficients
/// and the cube's geometry.
#[test]
fn shaded_faces_differ() {
    let (device, queue) = gpu::device_queue();
    let scene = PartScene::from_parts(std::slice::from_ref(&white_diamond_cube()), NEUTRAL);
    let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
    // yaw 3π/4 + pitch 0.6: top and front visible, other sides edge-on
    let camera = Camera::orbiting(
        distance,
        3.0 * std::f32::consts::FRAC_PI_4,
        0.6,
        target,
        800.0 / 600.0,
    );
    // expected decoded luminance of a white face with scene normal n: the
    // linear lit, carried through the target's u8 sRGB quantization
    // (nearest byte by the same EOTF the readback decodes with) — computed,
    // not measured
    let expected = |n: [f32; 3]| {
        let lit = lights::lit(n);
        let mut best = 0u8;
        let mut best_d = f32::MAX;
        for b in 0u8..=255 {
            let d = (srgb_to_linear(b) - lit).abs();
            if d < best_d {
                best_d = d;
                best = b;
            }
        }
        srgb_to_linear(best)
    };
    let e_top = expected([0.0, 1.0, 0.0]);
    let e_front = expected([
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
        -std::f32::consts::FRAC_1_SQRT_2,
    ]);
    assert!(
        e_top >= 2.0 * e_front,
        "geometry must give the constants a >=2x pair: {e_top} vs {e_front}"
    );
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let px = render_frame(device, queue, &mut pipeline, &scene, &camera, 800, 600);
    // flat white faces render unblended at 1x, so every region pixel carries
    // the region's exact value; the 0.01 band absorbs +-1 byte of storage
    // rounding (Metal rounds to nearest, other backends may differ) and the
    // 1% unclassified allowance absorbs edge-on slivers
    let (mut n_top, mut n_front, mut n_ink) = (0usize, 0usize, 0usize);
    let (mut sum_top, mut sum_front) = (0f64, 0f64);
    for c in px.chunks(4) {
        if !is_ink(c, bg) {
            continue;
        }
        n_ink += 1;
        let l = decode_linear(c)[0] as f64; // white: any channel is luminance
        if (l - e_top as f64).abs() <= 0.01 {
            n_top += 1;
            sum_top += l;
        } else if (l - e_front as f64).abs() <= 0.01 {
            n_front += 1;
            sum_front += l;
        }
    }
    assert!(
        n_ink > 5_000,
        "cube must cover a healthy area: {n_ink} ink px"
    );
    assert!(
        n_top > 2_000 && n_front > 2_000,
        "both regions must be sizable: top {n_top}, front {n_front} of {n_ink}"
    );
    assert!(
        n_top + n_front >= 99 * n_ink / 100,
        "only the two lit regions should render: {n_top}+{n_front} of {n_ink}"
    );
    let (mean_top, mean_front) = (sum_top / n_top as f64, sum_front / n_front as f64);
    assert!(
        mean_top >= 2.0 * mean_front,
        "readback luminance {mean_top:.4} vs {mean_front:.4} = {:.3}x, must be >=2x",
        mean_top / mean_front
    );
}

/// Wireframe mode draws hairlines, not faces (facts #44): the same scene,
/// same camera, `Options { wireframe: true }` vs solid — the line pass
/// draws the 1px edge list where the fill pass painted whole triangles, so
/// the wireframe ink is a small fraction of the solid ink. And the lines
/// carry the SAME material: every wireframe ink pixel classifies (by
/// chroma, lighting divides out) as one of the two part colors — a tinted
/// or recolored wireframe fails the classification half.
#[test]
fn wireframe_renders_lines_not_faces() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let solid = render_frame(device, queue, &mut pipeline, &scene, &camera, 800, 600);
    let wire = render_frame_opts(
        device,
        queue,
        &mut pipeline,
        &scene,
        &camera,
        800,
        600,
        None,
        &Options {
            wireframe: true,
            ..Options::default()
        },
    );
    let (si, wi) = (ink_pixels(&solid, bg), ink_pixels(&wire, bg));
    assert!(si > 5_000, "solid mode must cover a healthy area: {si}");
    // measured on Metal: solid 98560 ink px, wireframe 3423 — that was the
    // edge list WITHOUT face diagonals; upstream parity (facts #44, .lines
    // draws every triangle edge) adds the 12 diagonals, which can only
    // grow wire ink. Even a 3x growth reads ratio 0.104, inside the band,
    // and the floor only rises — full-suite re-measure deferred to the
    // thelio merge gate (laptop test policy)
    assert!(wi > 1_000, "wireframe lines must be visible at all: {wi}");
    let ratio = wi as f64 / si as f64;
    assert!(
        (0.01..0.12).contains(&ratio),
        "wireframe/solid ink ratio {ratio:.3} must sit well below 1 — hairlines, not faces"
    );
    // same material: wire ink classifies as base-blue or lid-red chroma
    let (blue, red) = fixture_chromas();
    const TOL: f32 = 0.05;
    let classified = wire
        .chunks(4)
        .filter(|c| is_ink(c, bg))
        .filter(|c| {
            let x = chroma(decode_linear(c));
            chroma_dist(x, blue) < TOL || chroma_dist(x, red) < TOL
        })
        .count();
    // measured 3423/3423 (100%); the 90% bar absorbs driver raster drift
    assert!(
        classified as f64 >= 0.90 * wi as f64,
        "only {classified}/{wi} wireframe ink pixels carry a part color"
    );
}

/// Pixels within TOL of the bed accent's chromaticity — the overlay is
/// unlit and opaque, so an unoccluded bed pixel is the pure accent and
/// u8 storage noise is the only deviation.
fn accent_pixels(px: &[u8]) -> usize {
    let accent = chroma(grid::ACCENT);
    px.chunks(4)
        .filter(|c| chroma_dist(chroma(decode_linear(c)), accent) < 0.05)
        .count()
}

/// The bed rectangle (facts #37): with `bed = Some` the accent-colored
/// outline draws around the model's footprint; with `None` no accent pixel
/// exists. Border, not a filled plate: the outline covers ~perimeter x
/// line-width while a filled quad would cover the bed's whole area — an
/// order of magnitude apart — and the accent bbox stays centered on the
/// model it surrounds.
#[test]
fn bed_rectangle_present() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let aabb = (scene.aabb_min, scene.aabb_max);
    // 80mm bed around the fixture's 60mm footprint: wide enough to clear
    // the model, small enough to stay inside the framing
    let with_bed = render_frame_opts(
        device,
        queue,
        &mut pipeline,
        &scene,
        &camera,
        800,
        600,
        Some(&grid::geometry(aabb, Some([80.0, 80.0]))),
        &Options::default(),
    );
    let sans_bed = render_frame_opts(
        device,
        queue,
        &mut pipeline,
        &scene,
        &camera,
        800,
        600,
        Some(&grid::geometry(aabb, None)),
        &Options::default(),
    );
    assert_eq!(accent_pixels(&sans_bed), 0, "bed None must draw no accent");
    let n = accent_pixels(&with_bed);
    // measured on Metal: 1520 accent px for the 80mm outline (perimeter
    // x ~2px line width); a FILLED bed would read ~1.3e5 (bed area minus
    // the model base), so the 8_000 ceiling sits 5x above the outline and
    // 16x below a filled plate
    assert!(
        n > 500,
        "bed Some must draw a visible outline: {n} accent px"
    );
    assert!(
        n < 8_000,
        "the bed is an outline, not a filled plate: {n} accent px"
    );
    // centered on the model: the accent bbox midpoint sits within 10% of
    // the frame width of the solid ink bbox midpoint
    let bbox_mid = |px: &[u8], pred: &dyn Fn(&[u8]) -> bool| -> (f64, f64) {
        let (mut mn_x, mut mx_x, mut mn_y, mut mx_y) = (usize::MAX, 0usize, usize::MAX, 0usize);
        for (i, c) in px.chunks(4).enumerate() {
            if pred(c) {
                let (x, y) = (i % 800, i / 800);
                mn_x = mn_x.min(x);
                mx_x = mx_x.max(x);
                mn_y = mn_y.min(y);
                mx_y = mx_y.max(y);
            }
        }
        assert!(mn_x <= mx_x, "predicate matched no pixels");
        ((mn_x + mx_x) as f64 / 2.0, (mn_y + mx_y) as f64 / 2.0)
    };
    let is_accent = |c: &[u8]| chroma_dist(chroma(decode_linear(c)), chroma(grid::ACCENT)) < 0.05;
    let (bed_x, bed_y) = bbox_mid(&with_bed, &is_accent);
    let ink_of_bg = |c: &[u8]| is_ink(c, bg);
    let (ink_x, ink_y) = bbox_mid(&with_bed, &ink_of_bg);
    // horizontally centered on the model, and at its floor: screen y grows
    // downward, so the rectangle's mid sits below the ink mid (the stacked
    // lid pulls the model's mid up), but still under the model — measured
    // 123px below on Metal
    assert!(
        (bed_x - ink_x).abs() < 40.0,
        "accent bbox mid x {bed_x:.0} must center on the model's {ink_x:.0}"
    );
    assert!(
        (0.0..200.0).contains(&(bed_y - ink_y)),
        "accent bbox mid y {bed_y:.0} must sit at the model's floor, below mid {ink_y:.0}"
    );
}

// --- blit orientation: CPU-rasterized ground truth (pure; no GPU) ---

/// The golden model with its lid translated through the pose path (the
/// group-1 model binding the mesh shader multiplies in — a per-frame
/// uniform since the GPU-first split). Two offset boxes make an L:
/// asymmetric left-right AND top-bottom on screen, so a horizontal mirror,
/// a vertical flip, or a 180° all move ink the CPU grid did not expect.
fn l_shaped() -> (PartScene, Vec<PartPose>) {
    let scene = golden_scene();
    assert_eq!(scene.parts.len(), 2);
    assert_eq!(scene.parts[1].name, "lid");
    // SCENE +x by 60: the lid (staged x in [-20, 20]) moves to [40, 60],
    // clear of the base's footprint (x in [-30, 30]) — an overhang to
    // screen-right at the iso yaw, still inside the framing headroom
    let poses = vec![
        Some(glam::Mat4::IDENTITY.to_cols_array_2d()),
        Some(glam::Mat4::from_translation(glam::Vec3::new(60.0, 0.0, 0.0)).to_cols_array_2d()),
    ];
    (scene, poses)
}

/// Project a scene-space point through `vp` to pixel coordinates, row 0 at
/// the top — NDC y-up flipped to texture rows top-down, the same mapping
/// the GPU's viewport transform and the readback share (so agreement
/// between this and the readback is the orientation check itself).
/// `None` behind the near plane.
fn project_px(vp: [[f32; 4]; 4], p: [f32; 3], w: f32, h: f32) -> Option<(f32, f32)> {
    let clip = glam::Mat4::from_cols_array_2d(&vp) * glam::Vec4::new(p[0], p[1], p[2], 1.0);
    if clip.w <= f32::EPSILON {
        return None;
    }
    let (nx, ny) = (clip.x / clip.w, clip.y / clip.w);
    Some(((nx * 0.5 + 0.5) * w, (0.5 - ny * 0.5) * h))
}

/// Does triangle `t` overlap the axis-aligned rect [x0, x1] x [y0, y1]?
/// SAT over the rect's two axes and the triangle's three edge normals —
/// exact area overlap, so the grid marks a cell even when only a corner
/// of the triangle reaches into it.
fn tri_overlaps_rect(t: [(f32, f32); 3], x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hw, hh) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    for i in 0..3 {
        let (a, b) = (t[i], t[(i + 1) % 3]);
        let (nx, ny) = (-(b.1 - a.1), b.0 - a.0);
        let len = (nx * nx + ny * ny).sqrt();
        if len == 0.0 {
            continue; // degenerate edge projects to a point
        }
        let (nx, ny) = (nx / len, ny / len);
        let rect_r = hw * nx.abs() + hh * ny.abs();
        let rect_c = cx * nx + cy * ny;
        let (mut mn, mut mx) = (f32::MAX, f32::MIN);
        for p in t {
            let d = p.0 * nx + p.1 * ny;
            mn = mn.min(d);
            mx = mx.max(d);
        }
        if mx < rect_c - rect_r || mn > rect_c + rect_r {
            return false;
        }
    }
    !(t.iter().all(|p| p.0 < x0)
        || t.iter().all(|p| p.0 > x1)
        || t.iter().all(|p| p.1 < y0)
        || t.iter().all(|p| p.1 > y1))
}

/// Coarse occupancy grid of `scene` under `vp`: cell (x, y) is set when any
/// staged triangle — carried through its part's pose, exactly as the mesh
/// shader's group-1 multiply does — overlaps the cell's pixel rect. `None`
/// poses skip their part, as the draw path does. Returns the grid width
/// and the row-major cells. Pure CPU geometry: the same triangles the GPU
/// rasterizes, projected by the same view_proj, so agreement between this
/// grid and the readback cannot be a re-implementation accident.
fn occupancy_grid(
    scene: &PartScene,
    poses: &[PartPose],
    vp: [[f32; 4]; 4],
    w: u32,
    h: u32,
    cell: u32,
) -> (usize, Vec<bool>) {
    let (gw, gh) = ((w / cell) as usize, (h / cell) as usize);
    let mut grid = vec![false; gw * gh];
    for (part, pose) in scene.parts.iter().zip(poses) {
        let model = match pose {
            Some(m) => glam::Mat4::from_cols_array_2d(m),
            None => continue,
        };
        for tri in part.vertices.chunks_exact(3) {
            let projected: Option<Vec<(f32, f32)>> = tri
                .iter()
                .map(|v| {
                    let world = model * glam::Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0);
                    project_px(vp, [world.x, world.y, world.z], w as f32, h as f32)
                })
                .collect();
            // a triangle crossing the near plane has no sane 2D projection;
            // the fixture camera keeps everything in front
            let Some(pts) = projected else { continue };
            let t = [pts[0], pts[1], pts[2]];
            let xs = [t[0].0, t[1].0, t[2].0];
            let ys = [t[0].1, t[1].1, t[2].1];
            let c0x = (xs.iter().cloned().fold(f32::MAX, f32::min) / cell as f32).floor() as i32;
            let c1x = (xs.iter().cloned().fold(f32::MIN, f32::max) / cell as f32).floor() as i32;
            let c0y = (ys.iter().cloned().fold(f32::MAX, f32::min) / cell as f32).floor() as i32;
            let c1y = (ys.iter().cloned().fold(f32::MIN, f32::max) / cell as f32).floor() as i32;
            for cy in c0y.max(0)..=c1y.min(gh as i32 - 1) {
                for cx in c0x.max(0)..=c1x.min(gw as i32 - 1) {
                    let (x0, y0) = (cx as f32 * cell as f32, cy as f32 * cell as f32);
                    if tri_overlaps_rect(t, x0, y0, x0 + cell as f32, y0 + cell as f32) {
                        grid[cy as usize * gw + cx as usize] = true;
                    }
                }
            }
        }
    }
    (gw, grid)
}

/// The occupancy floor the orientation assertions share: the grid must
/// describe a real render, not a sliver.
fn occupied_cells(grid: &[bool]) -> usize {
    grid.iter().filter(|c| **c).count()
}

/// The fixture must actually discriminate orientations: the CPU grid of the
/// L agrees with itself, but a horizontal mirror, a vertical flip, or a
/// 180° rotation must each fall well below the 85% agreement bar
/// `orientation_pattern_matches_projection` enforces — otherwise that test
/// passes vacuously and a flipped blit ships. Pure CPU: runs without a GPU.
#[test]
fn orientation_grid_discriminates_mirrors() {
    let (scene, poses) = l_shaped();
    let camera = default_camera(&scene, 800.0 / 600.0);
    const CELL: u32 = 25;
    let (gw, grid) = occupancy_grid(&scene, &poses, camera.view_proj, 800, 600, CELL);
    let (gh, n) = (grid.len() / gw, occupied_cells(&grid));
    assert!(
        n >= gw * gh / 8,
        "minimum-occupancy floor: {n} of {} cells — the grid must describe a real render",
        gw * gh
    );
    let hflip: Vec<bool> = grid
        .chunks(gw)
        .flat_map(|row| row.iter().rev())
        .copied()
        .collect();
    let vflip: Vec<bool> = grid.chunks(gw).rev().flatten().copied().collect();
    let rot180: Vec<bool> = grid.iter().rev().copied().collect();
    for (name, m) in [("horizontal", hflip), ("vertical", vflip), ("180°", rot180)] {
        let agree = grid.iter().zip(&m).filter(|(a, b)| **a && **b).count();
        assert!(
            (agree as f64) < 0.80 * n as f64,
            "{name} mirror agrees {agree}/{n} — the L fixture stopped discriminating flips"
        );
    }
}

/// The M0-deferred orientation obligation: the readback's occupied-cell
/// pattern must match a CPU rasterization of the SAME triangles through
/// the SAME view_proj — a flipped projection, viewport, or readback row
/// order lands the L where the CPU grid says nothing is. Agreement is
/// measured over OCCUPIED cells (raw cell agreement is dominated by empty
/// cells and passes vacuously), with a minimum-occupancy floor and an
/// extra-ink bound so a render that invents coverage cannot sneak through
/// the occupied-side denominator. Checked at 1x and through the 4x
/// resolve+blit path — a UV flip in the blit would mirror only the
/// composited frame, so the 1x leg alone cannot catch it.
#[test]
fn orientation_pattern_matches_projection() {
    let (device, queue) = gpu::device_queue();
    let (scene, poses) = l_shaped();
    let camera = default_camera(&scene, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let px = render_frame_poses(device, queue, &mut pipeline, &scene, &poses, 800, 600);
    assert!(
        ink_pixels(&px, bg) > 5_000,
        "the L must cover a healthy area: {}",
        ink_pixels(&px, bg)
    );
    const CELL: u32 = 25;
    let (gw, cpu) = occupancy_grid(&scene, &poses, camera.view_proj, 800, 600, CELL);
    let gh = cpu.len() / gw;
    let n = occupied_cells(&cpu);
    assert!(
        n >= gw * gh / 8,
        "minimum-occupancy floor: the CPU grid must describe a real render ({n} cells)"
    );
    let assert_grid_agrees = |px: &[u8], label: &str| {
        let mut gpu = vec![false; cpu.len()];
        for (i, c) in px.chunks(4).enumerate() {
            if is_ink(c, bg) {
                let (x, y) = (i % 800, i / 800);
                gpu[y / CELL as usize * gw + x / CELL as usize] = true;
            }
        }
        let agree = cpu.iter().zip(&gpu).filter(|(c, g)| **c && **g).count();
        assert!(
            agree as f64 >= 0.85 * n as f64,
            "{label}: only {agree}/{n} CPU-occupied cells are ink in the readback — an orientation flip somewhere in the render path"
        );
        let gpu_n = occupied_cells(&gpu);
        assert!(
            gpu_n as f64 <= 1.25 * n as f64,
            "{label}: readback invents coverage: {gpu_n} GPU cells vs {n} CPU cells"
        );
    };
    assert_grid_agrees(&px, "1x");
    // and through the resolve+blit path, where a UV flip in the blit would
    // mirror the composited frame — the M0 obligation's actual subject
    let mut p4 = MeshPipeline::with_msaa(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, 4);
    let px4 = render_frame_with_poses(
        device,
        queue,
        &mut p4,
        &scene,
        &camera,
        800,
        600,
        None,
        &Options {
            msaa: 4,
            ..Options::default()
        },
        &poses,
    );
    assert!(
        ink_pixels(&px4, bg) > 5_000,
        "the L must cover a healthy area at 4x: {}",
        ink_pixels(&px4, bg)
    );
    assert_grid_agrees(&px4, "4x blit");
}

/// The pose path at render scope: translating one part 60mm along scene +x
/// must displace its projected ink. (The upload-count half of the law is
/// asserted in-crate, where the test accessor is visible.)
#[test]
fn pose_translation_displaces_ink() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let camera = default_camera(&scene, 800.0 / 600.0);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let id = glam::Mat4::IDENTITY.to_cols_array_2d();
    let poses: Vec<PartPose> = vec![Some(id); scene.parts.len()];
    let px = render_frame_poses(device, queue, &mut pipeline, &scene, &poses, 800, 600);
    let mut moved = poses.clone();
    moved[1] =
        Some(glam::Mat4::from_translation(glam::Vec3::new(60.0, 0.0, 0.0)).to_cols_array_2d());
    let px2 = render_frame_poses(device, queue, &mut pipeline, &scene, &moved, 800, 600);
    // centroid of ink: mean (x, y) over pixels differing from the background
    let centroid = |px: &[u8]| -> (f32, f32) {
        let (mut sx, mut sy, mut n) = (0u64, 0u64, 0u64);
        for (i, c) in px.chunks(4).enumerate() {
            if is_ink(c, bg) {
                sx += (i % 800) as u64;
                sy += (i / 800) as u64;
                n += 1;
            }
        }
        (sx as f32 / n as f32, sy as f32 / n as f32)
    };
    let (c1, c2) = (centroid(&px), centroid(&px2));
    assert!(c2.0 > c1.0 + 20.0, "lid ink moved right: {c1:?} -> {c2:?}");
    assert!(
        c2.0 < c1.0 + 200.0,
        "and the base stayed put (mixed centroid)"
    );
}

/// The example's msaa toggle is an internal rebuild: ONE MeshPipeline
/// flipped 1 -> 4 -> 1 through `Options::msaa` must behave like pipelines
/// built at those counts — the 4x frame shows coverage-blend pixels (the
/// resolve+blit path really ran), and the flipped-back 1x frame is
/// byte-identical to the pre-toggle baseline (the rebuild left no msaa
/// state behind).
#[test]
fn msaa_toggle_rebuilds_in_place() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    let (blue, red) = fixture_chromas();
    let mut bg_pipe = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut bg_pipe, &camera, 800, 600);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let one = render_frame_opts(
        device,
        queue,
        &mut pipeline,
        &scene,
        &camera,
        800,
        600,
        None,
        &Options::default(),
    );
    let four = render_frame_opts(
        device,
        queue,
        &mut pipeline,
        &scene,
        &camera,
        800,
        600,
        None,
        &Options {
            msaa: 4,
            ..Options::default()
        },
    );
    let back = render_frame_opts(
        device,
        queue,
        &mut pipeline,
        &scene,
        &camera,
        800,
        600,
        None,
        &Options::default(),
    );
    assert_eq!(blend_pixels(&one, bg, blue, red), 0, "1x has no blends");
    assert!(
        blend_pixels(&four, bg, blue, red) > 600,
        "the rebuilt 4x path must resolve+blit: {}",
        blend_pixels(&four, bg, blue, red)
    );
    assert!(
        ink_pixels(&four, bg) > 5_000,
        "the 4x rebuild must still render the model: {}",
        ink_pixels(&four, bg)
    );
    assert_eq!(
        one, back,
        "flipping back to 1x must restore the single-sample frame exactly"
    );
}
