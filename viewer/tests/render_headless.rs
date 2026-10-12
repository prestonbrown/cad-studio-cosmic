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
use viewer::render::lights;
use viewer::render::{
    Camera, MeshPipeline, PartScene, NEUTRAL, default_camera, srgb_to_linear,
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
    [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2])]
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
        cap_red_pixels(&render_frame(device, queue, &mut pipeline, &scene, &cam, 800, 600))
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
    // leak into the second frame through the resolve path too
    let mut p4 =
        MeshPipeline::with_msaa(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, 4);
    let a = render_frame(device, queue, &mut p4, &scene, &camera, 800, 600);
    let b = render_frame(device, queue, &mut p4, &scene, &camera, 800, 600);
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

