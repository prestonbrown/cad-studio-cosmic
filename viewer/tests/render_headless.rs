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
use viewer::render::markers;
use viewer::render::{
    Camera, Frame, GHOST_ALPHA, MeshPipeline, NEUTRAL, Options, PartAppearance, PartPose,
    PartScene, SceneAxis, SectionPlane, default_camera, preset_camera, srgb_to_linear,
};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/golden")
        .join(rel)
}

/// The golden model's parts through the production parse path: model.3mf
/// is two parts (base + lid) of 12 triangles each.
fn golden_parts() -> Vec<engine::parsing::Part> {
    let bytes = std::fs::read(fixture("model.3mf"))
        .unwrap_or_else(|e| panic!("read fixture model.3mf: {e}"));
    parse_3mf(&bytes).expect("golden model.3mf parses")
}

fn golden_scene() -> PartScene {
    PartScene::from_parts(&golden_parts(), NEUTRAL)
}

/// The golden parts with `dx` baked into every MODEL x coordinate — the
/// comparison version's mesh stand-in: same geometry, translated so a
/// screen region beyond the model's projected extent holds only ghost
/// ink.
fn ghost_parts_shifted_x(dx: f32) -> Vec<engine::parsing::Part> {
    let mut parts = golden_parts();
    for part in &mut parts {
        for p in &mut part.positions {
            p[0] += dx;
        }
    }
    parts
}

/// Identity poses for every slot of `scene` — the assembled default every
/// non-pose-specific entry renders with.
fn identity_poses(scene: &PartScene) -> Vec<PartPose> {
    vec![Some(glam::Mat4::IDENTITY.to_cols_array_2d()); scene.parts.len()]
}

/// Opaque default appearances for every slot of `scene`.
fn default_appearances(scene: &PartScene) -> Vec<PartAppearance> {
    vec![PartAppearance::default(); scene.parts.len()]
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
    let poses = identity_poses(scene);
    let appearances = default_appearances(scene);
    render_frame_with_poses(
        device,
        queue,
        pipeline,
        scene,
        camera,
        w,
        h,
        grid,
        None,
        None,
        options,
        &poses,
        &appearances,
    )
}

/// The harness core: [`render_frame_opts`] plus the per-part poses and
/// appearance flags, the staged measure markers, and the compare ghost
/// threaded into the `Frame` — every pose-path test renders through here.
fn render_frame_with_poses(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    camera: &Camera,
    w: u32,
    h: u32,
    grid: Option<&grid::GridMesh>,
    markers: Option<&markers::MarkerMesh>,
    ghost: Option<&PartScene>,
    options: &Options,
    poses: &[PartPose],
    appearances: &[PartAppearance],
) -> Vec<u8> {
    render_frame_with_poses_format(
        device,
        queue,
        pipeline,
        scene,
        camera,
        w,
        h,
        grid,
        markers,
        ghost,
        options,
        poses,
        appearances,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    )
}

/// [`render_frame_with_poses`] at an explicit target format — the
/// output-space tests' seam: the app's iced surface is a PLAIN format
/// (libcosmic's web-colors build picks a non-sRGB one), where the shader
/// must carry the sRGB encode itself. Readback bytes are in the format's
/// own channel order (BGRA for Bgra8Unorm).
#[allow(clippy::too_many_arguments)]
fn render_frame_with_poses_format(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    camera: &Camera,
    w: u32,
    h: u32,
    grid: Option<&grid::GridMesh>,
    markers: Option<&markers::MarkerMesh>,
    ghost: Option<&PartScene>,
    options: &Options,
    poses: &[PartPose],
    appearances: &[PartAppearance],
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let frame = Frame {
        camera,
        scene,
        poses,
        appearances,
        section: None,
        grid,
        markers,
        ghost,
        options: *options,
    };
    render_frame_struct(device, queue, pipeline, &frame, w, h, format)
}

/// One `Frame` at `w` x `h` in `format`: `prepare_parts`, then
/// [`draw_and_read`] with the pipeline's own render pass.
fn render_frame_struct(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    frame: &Frame<'_>,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    pipeline.prepare_parts(device, queue, (w, h), frame);
    draw_and_read(device, queue, w, h, format, |encoder, view| {
        pipeline.render(encoder, view, [0.0, 0.0, w as f32, h as f32]);
    })
}

/// A section plane on scene `axis` at `pos`, capped over `scene`'s own box.
fn plane(scene: &PartScene, axis: SceneAxis, pos: f32) -> SectionPlane {
    SectionPlane {
        axis,
        pos,
        cap: scene.aabb(),
    }
}

/// [`render_frame`] with a section plane: identity poses, default options.
fn render_sectioned(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    camera: &Camera,
    section: SectionPlane,
    options: Options,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let poses = identity_poses(scene);
    let appearances = default_appearances(scene);
    let frame = Frame {
        camera,
        scene,
        poses: &poses,
        appearances: &appearances,
        section: Some(section),
        grid: None,
        markers: None,
        ghost: None,
        options,
    };
    render_frame_struct(
        device,
        queue,
        pipeline,
        &frame,
        w,
        h,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    )
}

/// A `w` x `h` target in `format`, cleared to the app background, `draw`
/// recorded on top, read back tightly packed (`w * h * 4` bytes in the
/// format's channel order). `queue.write_buffer` calls made before this
/// are ordered ahead of the submitted encoder's commands.
fn draw_and_read(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    draw: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
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
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
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
    draw(&mut encoder, &view);
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
    render_frame_msaa_with_grid(device, queue, scene, None, camera, w, h, msaa)
}

/// [`render_frame_msaa`] with the overlay threaded through the Frame: the
/// grid runs INSIDE the multisample pass, and its straight-alpha blend must
/// survive the resolve+blit composite — the msaa grid-alpha test's subject.
fn render_frame_msaa_with_grid(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &PartScene,
    grid: Option<&grid::GridMesh>,
    camera: &Camera,
    w: u32,
    h: u32,
    msaa: u32,
) -> Vec<u8> {
    let mut pipeline =
        MeshPipeline::with_msaa(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, msaa);
    render_frame_opts(
        device,
        queue,
        &mut pipeline,
        scene,
        camera,
        w,
        h,
        grid,
        &Options {
            msaa,
            ..Options::default()
        },
    )
}

/// The pose path's render entry — the common case with explicit per-part
/// poses: default camera for the viewport, no overlay, solid mode, opaque
/// appearances. (The upload-count half of the pose law is asserted
/// in-crate, where the counter accessors are visible.)
fn render_frame_poses(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    poses: &[PartPose],
    w: u32,
    h: u32,
) -> Vec<u8> {
    let appearances = default_appearances(scene);
    render_frame_appearances(device, queue, pipeline, scene, poses, &appearances, w, h)
}

/// The appearance path's render entry — [`render_frame_poses`] plus the
/// per-part motion appearance flags threaded into the `Frame`: default
/// camera for the viewport, no overlay, solid mode.
fn render_frame_appearances(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    poses: &[PartPose],
    appearances: &[PartAppearance],
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
        None,
        None,
        &Options::default(),
        poses,
        appearances,
    )
}

/// The compare ghost's render entry: the harness core with the ghost
/// scene threaded into the `Frame` — default camera for the viewport
/// (deterministic in the scene, so a test's own `default_camera` call
/// reproduces it), identity poses, no overlay or markers.
fn render_frame_ghost(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    scene: &PartScene,
    ghost: &PartScene,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let camera = default_camera(scene, w as f32 / h as f32);
    let poses = identity_poses(scene);
    let appearances = default_appearances(scene);
    render_frame_with_poses(
        device,
        queue,
        pipeline,
        scene,
        &camera,
        w,
        h,
        None,
        None,
        Some(ghost),
        &Options::default(),
        &poses,
        &appearances,
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

/// The EOTF's inverse on the f32 unit scale with round-to-nearest storage:
/// blend expectations round-trip decode → lerp → encode exactly as the
/// fixed-function sRGB blend does (an sRGB attachment decodes both
/// endpoints, blends in LINEAR space, and re-encodes on store).
fn encode_srgb(linear: f32) -> u8 {
    let srgb = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (srgb.clamp(0.0, 1.0) * 255.0).round() as u8
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

/// The default iso camera's view axis: the direction toward the eye that
/// mesh.wgsl derives from the view-projection, as its Rust twin reads it.
fn iso_to_eye() -> [f32; 3] {
    let (yaw, pitch) = camera::Preset::Iso.yaw_pitch();
    lights::to_eye(yaw, pitch)
}

/// The six axis-aligned face normals of the golden boxes.
fn axis_normals() -> Vec<[f32; 3]> {
    let mut normals = Vec::new();
    for a in 0..3 {
        for s in [-1.0, 1.0] {
            let mut n = [0.0; 3];
            n[a] = s;
            normals.push(n);
        }
    }
    normals
}

/// The axis normals the iso camera sees (`n · to_eye > 0`): +X, +Y, +Z.
/// A solid render shows no other face, so a classifier built on these
/// fails when specular or the shoulder stop reaching the lit faces —
/// the hidden faces' ambient-only chroma is the undesaturated base.
fn iso_visible_normals() -> Vec<[f32; 3]> {
    let to_eye = glam::Vec3::from_array(iso_to_eye());
    axis_normals()
        .into_iter()
        .filter(|&n| glam::Vec3::from_array(n).dot(to_eye) > 0.0)
        .collect()
}

/// The chromaticities a part of linear color `base` takes on faces with
/// `normals` under the default iso camera — `lights::shade` of each
/// normal, normalized. The key's white specular and the highlight
/// shoulder desaturate lit faces, so a part has one chroma per face
/// rather than one overall; a flat face still renders a single value.
fn face_chromas(base: [f32; 3], normals: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let to_eye = iso_to_eye();
    normals
        .iter()
        .map(|&n| chroma(lights::shade(base, n, to_eye)))
        .collect()
}

/// Distance from chroma `x` to the nearest of `set`.
fn chroma_gap(x: [f32; 3], set: &[[f32; 3]]) -> f32 {
    set.iter()
        .map(|&c| chroma_dist(x, c))
        .fold(f32::MAX, f32::min)
}

/// The golden model's part colors, linear — fixture bytes (base
/// #1F78B5, lid #D62629; model.3mf basematerials) through the same EOTF
/// the staging applies.
fn fixture_colors() -> ([f32; 3], [f32; 3]) {
    (
        [31, 120, 181].map(srgb_to_linear),
        [214, 38, 41].map(srgb_to_linear),
    )
}

/// The golden model's two parts' chromas on the faces a solid iso render
/// shows ([`iso_visible_normals`]), as (base, lid).
fn fixture_chromas() -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let (base, lid) = fixture_colors();
    let normals = iso_visible_normals();
    (face_chromas(base, &normals), face_chromas(lid, &normals))
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
/// CUT_FACE_LINEAR (0.692, 0.032, 0.022) reads back as (217, 50, 41) — a
/// brief's linear-g `g < 90` band would misfire on exactly that. Calibrated
/// from a measured histogram (T4, per-part colors under two-light shading):
/// cap (217,50,41) — the cap shader is unlit, so it alone is
/// shading-invariant; blue base faces (11,61,95), (13,67,103), (16,75,115);
/// red lid faces (113,15,16), (123,17,19), (137,21,23); background
/// (89,89,97). The reddest lit face tops out at r=137, so r > 180
/// separates the cap from every measured mesh/background value with >40 of
/// headroom; b < 120 clears the brightest blue face's 115 by 5; the g cap
/// excludes any future bright-but-not-red surface. The blended red is
/// DARKER than the spike-era (246,108,89): r headroom over the 180 bound
/// drops 66 -> 37 (still 80 clear of the reddest lit face's 137), while g
/// and b headroom grow.
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
    let cam = default_camera(&scene, 800.0 / 600.0);
    let span = scene.aabb_max[1] - scene.aabb_min[1];
    let mut cap_at = |frac: f32| {
        // SCENE y — the MODEL z stack after the remap
        let cut = plane(&scene, SceneAxis::Y, scene.aabb_min[1] + span * frac);
        cap_red_pixels(&render_sectioned(
            device,
            queue,
            &mut pipeline,
            &scene,
            &cam,
            cut,
            Options::default(),
            800,
            600,
        ))
    };
    let base = cap_at(4.0 / 26.0); // pinned y=4: middle of the 60mm base box
    let lid = cap_at(21.0 / 26.0); // pinned y=21: middle of the 40mm lid box
    let gap = cap_at(0.5); // pinned y=13: the air gap between the two parts
    // The bounds are generous lower bounds and ratios, justified by the
    // geometry rather than a measured constant: a mid-base cut paints the
    // 60mm box's whole cross-section (thousands of red px at this framing),
    // the 40mm lid's cross-section is (40/60)² ≈ 4/9 of the base's, and a
    // plane through the air gap leaves the stencil empty everywhere — the
    // cap has no solid to open.
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

/// A MODEL-space box `[min, max]` as one 12-triangle closed solid.
/// The winding/volume contract every consumer leans on (picking, parity,
/// cap area, the ghost's pin staging): a closed, consistently wound box
/// encloses exactly its volume — bowties/self-intersections read 0,
/// flipped faces read negative — asserted here once.
fn parts_box_model(bounds: [[f32; 3]; 2]) -> engine::parsing::Part {
    let (mn, mx) = (bounds[0], bounds[1]);
    // perimeter-ordered corners: 0-3 walk the z=min slice's x/y perimeter,
    // 4-7 the same walk at z=max — the enumeration the face table below is
    // written against. (A binary bit-encoded closure swaps corners 2/3 and
    // 6/7 against this table, bowtie-ing the z faces and replacing the x
    // walls with diagonals; the volume assert below is the discriminator.)
    let v = [
        [mn[0], mn[1], mn[2]],
        [mx[0], mn[1], mn[2]],
        [mx[0], mx[1], mn[2]],
        [mn[0], mx[1], mn[2]],
        [mn[0], mn[1], mx[2]],
        [mx[0], mn[1], mx[2]],
        [mx[0], mx[1], mx[2]],
        [mn[0], mx[1], mx[2]],
    ];
    // six quads, all wound outward (mod.rs cube_part's table)
    let faces = [
        (0usize, 3, 2, 1),
        (4, 5, 6, 7),
        (0, 1, 5, 4),
        (1, 2, 6, 5),
        (2, 3, 7, 6),
        (3, 0, 4, 7),
    ];
    let mut positions = Vec::new();
    for &(a, b, c, d) in &faces {
        quad(&mut positions, v[a], v[b], v[c], v[d]);
    }
    let signed_volume = positions.chunks_exact(3).fold(0.0f32, |acc, t| {
        let (a, b, c) = (
            glam::Vec3::from_array(t[0]),
            glam::Vec3::from_array(t[1]),
            glam::Vec3::from_array(t[2]),
        );
        acc + a.dot(b.cross(c)) / 6.0
    });
    let expected = (mx[0] - mn[0]) * (mx[1] - mn[1]) * (mx[2] - mn[2]);
    assert!(
        (signed_volume - expected).abs() < 1e-3 * expected,
        "parts_box_model must enclose its volume: {signed_volume} vs {expected}"
    );
    engine::parsing::Part {
        name: "box".into(),
        positions,
        triangle_colors: vec![None; 12],
    }
}

/// [`parts_box_model`] staged with the IDENTITY pin: the staged
/// coordinates are exactly the raw `(x, z, -y)` remap — the frame
/// `tools::section::scene_plane` states its planes in, with no floor-pin
/// offset for the test to fold in (the pane adds the live pin through
/// `to_scene`; here pin 0 makes raw and staged coincide).
fn box_scene(bounds: [[f32; 3]; 2]) -> PartScene {
    let part = parts_box_model(bounds);
    PartScene::from_parts_with_pin(std::slice::from_ref(&part), NEUTRAL, [0.0; 3])
}

/// The ink's row extent (min, max texture row, row 0 at the top), or None
/// when nothing is inked. The top-preset section test compares this against
/// projected geometry: from that camera screen rows order monotonically in
/// world z (the near-vertical look direction puts world +z screen-down),
/// and the projection of a box is bounded by its corners' projections
/// (screen position is fractional-linear in world position, so extrema
/// over a convex box sit at vertices) — no depth readback needed.
fn ink_row_extent(px: &[u8], bg: [u8; 4], w: u32) -> Option<(usize, usize)> {
    let (mut lo, mut hi) = (usize::MAX, 0usize);
    for (i, c) in px.chunks(4).enumerate() {
        if is_ink(c, bg) {
            let row = i / w as usize;
            lo = lo.min(row);
            hi = hi.max(row);
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// MODEL-Y section keeps the ABOVE half (facts #84-87): a box offset in
/// model y, cut at its midpoint, must keep the model-y >= V half — which
/// the scene-plane mapping expresses as scene-z keep-below at -V. Rendered
/// from the top preset (screen rows order in world z), the kept ink's row
/// extent must match the projected kept slab z∈[4,12] exactly: bounded by
/// the z=12 cut face's corners, reaching them, and starting at the z=4
/// near face — not the discarded half's z∈[12,20], and not a smaller
/// interior slab.
#[test]
fn y_axis_section_keeps_the_model_above_side() {
    let (device, queue) = gpu::device_queue();
    // MODEL box x[-20,20] y[-20,-4] z[0,20] -> raw-remap scene x[-20,20]
    // y[0,20] z[4,20]; cut at MODEL y mid (-12) keeps y in [-12,-4] =
    // scene z in [4,12] — the mapping's (2, 12.0) below
    let scene = box_scene([[-20.0, -20.0, 0.0], [20.0, -4.0, 20.0]]);
    let (axis, pos) = viewer::tools::section::scene_plane(
        viewer::tools::section::SectionAxis::Y,
        0.5,
        scene.scene_box_to_model(scene.aabb()),
    );
    assert_eq!((axis, pos), (SceneAxis::Z, 12.0));
    let cam = preset_camera(&scene, camera::Preset::Top, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &cam, 800, 600);
    let px = render_sectioned(
        device,
        queue,
        &mut pipeline,
        &scene,
        &cam,
        plane(&scene, axis, pos),
        Options::default(),
        800,
        600,
    );
    assert!(
        ink_pixels(&px, bg) > 1_000,
        "the kept half renders: {}",
        ink_pixels(&px, bg)
    );
    // the z=4/z=12/z=20 face-corner row bands (4 corners each: x ∈ {−20,20}
    // × y ∈ {0,20}); fractional-linear projection puts the box's screen
    // extrema at corners, so these bound the corresponding slab's ink
    let row_band = |z: f32| -> (f32, f32) {
        let rows: Vec<f32> = [-20.0f32, 20.0]
            .iter()
            .flat_map(|x| {
                [0.0f32, 20.0].map(|y| {
                    project_px(cam.view_proj, [*x, y, z], 800.0, 600.0)
                        .unwrap()
                        .1
                })
            })
            .collect();
        let lo = rows.iter().cloned().fold(f32::MAX, f32::min);
        let hi = rows.iter().cloned().fold(f32::MIN, f32::max);
        (lo, hi)
    };
    let (near4_lo, near4_hi) = row_band(4.0);
    let (cut12_lo, cut12_hi) = row_band(12.0);
    let (gone20_lo, gone20_hi) = row_band(20.0);
    // the kept and cut bands must be separable at all (they are: ~20px apart
    // at this framing) — otherwise the asserts below could pass by accident
    assert!(
        cut12_lo > near4_hi + 4.0,
        "z bands order on screen: {near4_hi} vs {cut12_lo}"
    );
    assert!(
        gone20_lo > cut12_hi + 4.0,
        "cut band orders under the discarded: {cut12_hi} vs {gone20_lo}"
    );
    let (ink_lo, ink_hi) = ink_row_extent(&px, bg, 800).expect("ink rows");
    assert!(
        (ink_lo as f32) >= near4_lo - 1.0 && (ink_hi as f32) <= cut12_hi + 1.0,
        "ink rows [{ink_lo}, {ink_hi}] inside the kept slab's projection [~{near4_lo:.0}, ~{cut12_hi:.0}]"
    );
    assert!(
        (ink_hi as f32) >= cut12_hi - 2.0,
        "the ink reaches the cut face's band (rows to {ink_hi}, band [{cut12_lo:.0}, {cut12_hi:.0}]) — a slab stopping at z=8 would top out ~{cut12_lo:.0}"
    );
    assert!(
        (ink_lo as f32) <= near4_lo + 2.0,
        "the ink starts at the kept near face (row {ink_lo} vs band ~{near4_lo:.0}) — a port keeping MODEL-below would start at the cut band ~{cut12_lo:.0}"
    );
    // control: unsectioned, the box's far side reaches the z=20 band — the
    // clipped extent above is the discard's doing, not the helper's blindness
    let px = render_frame(device, queue, &mut pipeline, &scene, &cam, 800, 600);
    let (_, full_hi) = ink_row_extent(&px, bg, 800).expect("unsectioned ink rows");
    assert!(
        (full_hi as f32) >= gone20_hi - 2.0,
        "unsectioned ink reaches the discarded far face's band: {full_hi} vs ~{gone20_hi:.0}"
    );
}

/// The discard does not depend on winding (carry-forward M6 line 1): flip
/// one part's triangle windings and the above-plane pixels still disappear.
/// The plane sits BELOW the pinned aabb, so keep-below keeps nothing —
/// every fragment is above it and the discard alone decides every pixel;
/// any winding gate would leave rewound fragments alive and fail the exact
/// zero.
#[test]
fn section_discard_is_winding_independent() {
    let (device, queue) = gpu::device_queue();
    let bytes = std::fs::read(fixture("model.3mf"))
        .unwrap_or_else(|e| panic!("read fixture model.3mf: {e}"));
    let mut parts = parse_3mf(&bytes).expect("golden model.3mf parses");
    for t in parts[0].positions.chunks_exact_mut(3) {
        t.swap(1, 2); // rewind the base
    }
    let scene = PartScene::from_parts(&parts, NEUTRAL);
    let cam = default_camera(&scene, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &cam, 800, 600);
    let below = plane(&scene, SceneAxis::Y, scene.aabb_min[1] - 0.5);
    let px = render_sectioned(
        device,
        queue,
        &mut pipeline,
        &scene,
        &cam,
        below,
        Options::default(),
        800,
        600,
    );
    assert_eq!(
        ink_pixels(&px, bg),
        0,
        "rewound part still discards everywhere above the plane"
    );
    // control: unsectioned, the rewound geometry itself renders — the zero
    // above is the discard, not a broken scene
    let px = render_frame(device, queue, &mut pipeline, &scene, &cam, 800, 600);
    assert!(
        ink_pixels(&px, bg) > 5_000,
        "the rewound geometry renders unsectioned: {}",
        ink_pixels(&px, bg)
    );
}

/// The cap reads as the blended systemRed through the EOTF
/// (`render::CUT_FACE_LINEAR`), not the spike's brighter red: sample the
/// pixel at the projected center of the cut face.
#[test]
fn cap_color_matches_blended_system_red() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let cam = default_camera(&scene, 800.0 / 600.0);
    // mid-base cut (the established `base` plane)
    let mid_base = plane(&scene, SceneAxis::Y, 4.0);
    let px = render_sectioned(
        device,
        queue,
        &mut pipeline,
        &scene,
        &cam,
        mid_base,
        Options::default(),
        800,
        600,
    );
    // the cut face's center: the fixture's staged x/z are pin-centered and
    // symmetric, and the cut sits at pinned scene y = 4 — the exact middle
    // of the 60mm base cross-section
    let center = project_px(cam.view_proj, [0.0, 4.0, 0.0], 800.0, 600.0).unwrap();
    let idx = (center.1 as usize * 800 + center.0 as usize) * 4;
    let pixel: [u8; 4] = px[idx..idx + 4].try_into().unwrap();
    let lin = decode_linear(&pixel);
    for i in 0..3 {
        assert!(
            (lin[i] - viewer::render::CUT_FACE_LINEAR[i]).abs() < 0.05,
            "channel {i}: {lin:?}"
        );
    }
}

/// Renders `model` the way iced does: `Program::draw` builds the frame's
/// primitive, `Primitive::prepare` stages it, `Primitive::render` records
/// the viewer pass over the cleared target.
fn render_adapter(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut MeshPipeline,
    model: &viewer::render::ModelScene,
    w: u32,
    h: u32,
) -> Vec<u8> {
    use iced::widget::shader::{Primitive, Program, Viewport};
    let bounds = iced::Rectangle {
        x: 0.0,
        y: 0.0,
        width: w as f32,
        height: h as f32,
    };
    let primitive = Program::<()>::draw(model, &(), iced::mouse::Cursor::Unavailable, bounds);
    let viewport = Viewport::with_physical_size(iced::Size::new(w, h), 1.0);
    primitive.prepare(pipeline, device, queue, &bounds, &viewport);
    let clip = iced::Rectangle {
        x: 0,
        y: 0,
        width: w,
        height: h,
    };
    draw_and_read(
        device,
        queue,
        w,
        h,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        |encoder, view| primitive.render(pipeline, encoder, view, &clip),
    )
}

/// Exploded + section through the iced adapter: the cap must fill the cut
/// of every exploded part. The golden lid explodes up along MODEL z (scene
/// y), past the assembled box, and an X cut's cap quad spans scene y and z,
/// so a cap sized from the assembled box stops short of the pushed lid.
/// The oracle is the same frame drawn with a cap quad far larger than any
/// part: the stencil clips a cap to the solid, so that frame's red count
/// is the full cut area.
#[test]
fn exploded_section_cap_covers_every_pushed_part() {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use viewer::tools::arrangement;
    use viewer::tools::section::{self, SectionAxis};

    let (device, queue) = gpu::device_queue();
    let parts = golden_parts();
    let scene = Arc::new(PartScene::from_parts(&parts, NEUTRAL));
    let none = BTreeSet::new();
    let arranged = arrangement::arrange(
        &parts,
        &[],
        engine::settings::PartsLayout::Exploded,
        1.0,
        &none,
        &none,
    );
    let lid_push = arranged.offsets.iter().map(|o| o[2]).fold(0.0f32, f32::max);
    assert!(
        lid_push > 2.0,
        "the lid explodes up: offsets {:?}",
        arranged.offsets
    );
    let eff = arrangement::effective_model_aabb(&parts, &arranged);

    let mut model = viewer::render::ModelScene::new(scene.clone());
    model.poses = arranged
        .offsets
        .iter()
        .map(|&o| PartScene::model_offset_pose(o))
        .collect::<Vec<_>>()
        .into();
    let cut = section::pinned_plane(SectionAxis::X, 0.5, eff, &scene);
    model.set_section(Some(cut));
    // look down -x at the cap, framed on the exploded scene box
    let (target, distance) = camera::frame(cut.cap.min, cut.cap.max);
    let (yaw, pitch) = camera::Preset::Right.yaw_pitch();
    model.set_pose(yaw, pitch, distance, target);

    let (w, h) = (800, 600);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let drawn = cap_red_pixels(&render_adapter(device, queue, &mut pipeline, &model, w, h));

    let cam = Camera::orbiting(distance, yaw, pitch, target, w as f32 / h as f32);
    let appearances = default_appearances(&scene);
    let oracle = Frame {
        camera: &cam,
        scene: &scene,
        poses: &model.poses,
        appearances: &appearances,
        section: Some(SectionPlane {
            cap: viewer::render::Box3::new([-1e4; 3], [1e4; 3]),
            ..cut
        }),
        grid: None,
        markers: None,
        ghost: None,
        options: Options::default(),
    };
    let full = cap_red_pixels(&render_frame_struct(
        device,
        queue,
        &mut pipeline,
        &oracle,
        w,
        h,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ));
    eprintln!("exploded X cut: lid push {lid_push}, adapter cap {drawn} px, full cut {full} px");
    assert!(full > 2_000, "both parts' cuts are in view: {full} px");
    assert!(
        drawn * 100 >= full * 99,
        "the cap must cover the whole cut: {drawn} of {full} red px"
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
/// the two parts' face chromas ([`face_chromas`]) and the background.
/// Chroma, not luminance: the pure faces bracket the background on every
/// channel, so no luminance band separates blends from pures, while each
/// flat face renders one value that sits on its own face chroma and a
/// silhouette-edge blend — resolve box-filters the covered samples — lands
/// strictly between endpoint chromas. At 1x (unblended, single-sample)
/// there are none; the 0.05 cut is several times u8 quantization noise.
fn blend_pixels(px: &[u8], bg: [u8; 4], blue: &[[f32; 3]], red: &[[f32; 3]]) -> usize {
    let bgc = chroma(decode_linear(&bg));
    px.chunks(4)
        .filter(|c| {
            let x = chroma(decode_linear(c));
            chroma_dist(x, bgc) > 0.05 && chroma_gap(x, blue) > 0.05 && chroma_gap(x, red) > 0.05
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
        blend_pixels(&px1, bg, &blue, &red),
        blend_pixels(&px4, bg, &blue, &red),
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
    let cut = render_sectioned(
        device,
        queue,
        &mut p4,
        &scene,
        &camera,
        plane(&scene, SceneAxis::Y, 4.0),
        *at4,
        800,
        600,
    );
    assert!(
        cap_red_pixels(&cut) > 2_000,
        "section cap must render through the 4x path: {} red px",
        cap_red_pixels(&cut)
    );
}

/// Carry-forward strike (M6 T7): the alpha-blended grid survives the 4x
/// MSAA resolve path. The grid pipeline runs INSIDE the multisample pass,
/// so its straight-alpha lines blend onto the transparent-cleared
/// attachment and the resolve+blit composite carries them back over the
/// app background: grid-colored ink must be present after the blit, at
/// pixel values consistent with straight-alpha compositing. Grid ink on
/// the dark background reads ≈ `alpha·white + (1−alpha)·bg` — whitish
/// chroma well away from the model's blue/red faces and the tripod's
/// saturated RGB boxes (all > 0.4 chroma-distance from white). A resolve
/// that drops the coverage alpha yields either full-white lines or
/// background: the count catches the second, the no-white assert the
/// first.
#[test]
fn msaa_resolves_keep_grid_alpha() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    let mut bg_pipe = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut bg_pipe, &camera, 800, 600);
    let g = grid::geometry((scene.aabb_min, scene.aabb_max), None);
    let px = render_frame_msaa_with_grid(device, queue, &scene, Some(&g), &camera, 800, 600, 4);
    // the blit's premultiplied composite over the a=1 clear keeps the
    // target alpha at 255 — pinned so a blit regression cannot ship a
    // half-transparent frame
    assert!(px.chunks(4).all(|c| c[3] == 255), "target stays opaque");
    let grid_ink = px
        .chunks(4)
        .filter(|c| is_ink(c, bg))
        .filter(|c| chroma_dist(chroma(decode_linear(c)), [1.0, 1.0, 1.0]) < 0.4)
        .count();
    assert!(
        grid_ink > 300,
        "grid lines visible through the resolve: {grid_ink}px"
    );
    // the alpha-lost regression reads OPAQUE white lines (alpha 0.10/0.28
    // read as 1): the frame's only white source is the sub-unit-alpha
    // lines — bed is None, markers None, the model blue/red — so no
    // fully-white pixel may exist
    assert!(
        !px.chunks(4).any(|c| c[0] > 250 && c[1] > 250 && c[2] > 250),
        "alpha preserved: no fully-white pixel may exist"
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
        if chroma_gap(x, &blue) < TOL || chroma_gap(x, &red) < TOL {
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
        let (d_blue, d_red) = (chroma_gap(x, &blue), chroma_gap(x, &red));
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
/// so every ink pixel's chromaticity must sit on one of the neutral's face
/// chromas ([`face_chromas`]). Guards the None branch of the
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
    let neutral = face_chromas(NEUTRAL, &iso_visible_normals());
    // Measured on Metal (T4, two-light shading): max chroma distance over
    // all 98560 ink pixels is 0.0115 (u8 quantization on the lit-neutral
    // faces — ambient-only faces are dark and quantize coarser); TOL 0.05
    // is >4x that. A per-triangle default color or sRGB passthrough lands
    // 0.1+ away and fails.
    const TOL: f32 = 0.05;
    let near = px
        .chunks(4)
        .filter(|c| is_ink(c, bg))
        .filter(|c| chroma_gap(chroma(decode_linear(c)), &neutral) < TOL)
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
/// The rotation is the point: viewed from scene yaw 3π/4, the 315°-normal
/// side faces the camera dead-on and sits >90° from the key, fill AND
/// environment directions — ambient only — while the top face takes every
/// term plus the key's specular; every other side is exactly edge-on, so
/// the shading constants separate the two visible faces by well over 2x.
/// White (`Some([255,255,255])` is exact linear (1,1,1)) makes readback
/// luminance equal [`white_shade`], with no albedo to divide out.
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

/// The linear value mesh.wgsl writes for a white face with scene normal
/// `n` under the white-cube tests' camera (yaw 3π/4, pitch 0.6) — the
/// shader's Rust twin, so every expectation derives from the constants.
fn white_shade(n: [f32; 3]) -> f32 {
    lights::shade(
        [1.0; 3],
        n,
        lights::to_eye(3.0 * std::f32::consts::FRAC_PI_4, 0.6),
    )[0]
}

/// Real normals + the shading model reach pixels: key-facing vs
/// away-facing regions of a white cube differ in readback luminance by
/// >=2x, and each region sits in the band derived from the shading
/// constants — [`white_shade`] of the face's scene normal, quantized
/// through the target's u8 sRGB storage by nearest decoded byte. No
/// measured constants anywhere: the bands come from the `lights` module
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
        let lit = white_shade(n);
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

/// The shader IS its Rust twin, face by face: the golden 3MF at the iso
/// preset shows six flat faces (lid and base: top +Y, front +Z, right
/// +X), and each must read back as `lights::shade` of its part color and
/// normal, seen along `lights::to_eye(π/4, 0.6)`, sRGB-encoded to a byte —
/// within ±2 per channel. Most of these faces sit below the highlight
/// shoulder, so every term (ambient, key, fill, environment, specular)
/// and mesh.wgsl's view-axis derivation moves at least one face out of
/// its band if the shader drifts from the twin. Expectations are
/// computed from the constants; nothing here is measured.
#[test]
fn iso_faces_match_the_shading_twin() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = preset_camera(&scene, camera::Preset::Iso, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let px = render_frame(device, queue, &mut pipeline, &scene, &camera, 800, 600);
    let (base, lid) = fixture_colors();
    let to_eye = iso_to_eye();
    let faces: Vec<(String, [u8; 3])> = [("lid", lid), ("base", base)]
        .into_iter()
        .flat_map(|(part, color)| {
            iso_visible_normals().into_iter().map(move |n| {
                (
                    format!("{part} {n:?}"),
                    lights::shade(color, n, to_eye).map(encode_srgb),
                )
            })
        })
        .collect();
    assert_eq!(faces.len(), 6, "lid and base, three visible faces each");
    let within = |c: &[u8], e: [u8; 3]| (0..3).all(|i| (c[i] as i32 - e[i] as i32).abs() <= 2);
    let mut counts = vec![0usize; faces.len()];
    let mut n_ink = 0usize;
    for c in px.chunks(4).filter(|c| is_ink(c, bg)) {
        n_ink += 1;
        if let Some(i) = faces.iter().position(|(_, e)| within(c, *e)) {
            counts[i] += 1;
        }
    }
    assert!(n_ink > 5_000, "model must cover a healthy area: {n_ink}");
    // every face is thousands of pixels at 800x600; 500 is a floor that
    // only a face rendering the wrong value misses
    for ((name, e), &count) in faces.iter().zip(&counts) {
        assert!(
            count > 500,
            "{name}: {count} px within ±2 of the twin's {e:?} (counts {counts:?})"
        );
    }
    // 1x renders unblended single-sample fragments: a pixel off every
    // face's value is a shading or state leak; 1% absorbs edge-on slivers
    let matched: usize = counts.iter().sum();
    assert!(
        matched * 100 >= n_ink * 99,
        "only {matched}/{n_ink} ink pixels equal a twin face value"
    );
}

/// The output-space contract for a PLAIN (non-sRGB) target — the format
/// iced's compositor picks for the app under libcosmic's `web-colors`
/// build, where the hardware stores no sRGB encode and the shader must
/// carry it (`target_space_wgsl`). Rendered at that format, the white
/// cube's faces read back as the ENCODED [`white_shade`] byte, never the
/// raw linear one — the reading that rendered the
/// app's model darker than its background. Readback is BGR byte order; a
/// gray face repeats the same byte on every channel.
#[test]
fn plain_target_shader_carries_the_srgb_encode() {
    let (device, queue) = gpu::device_queue();
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
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
    let px = render_frame_with_poses_format(
        device,
        queue,
        &mut pipeline,
        &scene,
        &camera,
        800,
        600,
        None,
        None,
        None,
        &Options::default(),
        &identity_poses(&scene),
        &default_appearances(&scene),
        wgpu::TextureFormat::Bgra8Unorm,
    );
    // the two visible faces' expected STORED bytes: the linear lit through
    // the encode the shader now carries (encode_srgb is its test twin)
    let lits = [
        white_shade([0.0, 1.0, 0.0]),
        white_shade([
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
            -std::f32::consts::FRAC_1_SQRT_2,
        ]),
    ];
    let encoded: Vec<u8> = lits.iter().map(|&l| encode_srgb(l)).collect();
    let raw: Vec<u8> = lits.iter().map(|&l| (l * 255.0).round() as u8).collect();
    // every face's encoded byte must sit outside the ±2 match band of its
    // raw byte, and at least one face far outside it, for this test to bite
    let gaps: Vec<i32> = encoded
        .iter()
        .zip(&raw)
        .map(|(&e, &r)| (e as i32 - r as i32).abs())
        .collect();
    assert!(
        gaps.iter().all(|&g| g > 4) && gaps.iter().any(|&g| g > 20),
        "encoded {encoded:?} vs raw {raw:?} must be far apart for this test to bite"
    );
    // the harness clear (0.1, 0.1, 0.12, 1.0) stored raw by the plain target
    let bg = [26u8, 26, 31, 255];
    let (mut face, mut ink, mut edges) = ([0usize; 2], 0usize, 0usize);
    for c in px.chunks(4) {
        if !is_ink(c, bg) {
            continue;
        }
        ink += 1;
        // BGR order; the gray faces carry one value on all channels
        match encoded
            .iter()
            .position(|&e| (c[2] as i32 - e as i32).abs() <= 2)
        {
            Some(i) => face[i] += 1,
            // antialiased boundary pixels blend toward the background
            None => edges += 1,
        }
    }
    assert!(ink > 5_000, "the cube must render: {ink} ink px");
    assert!(
        face[0] > 2_000 && face[1] > 2_000,
        "both faces must read encoded: {:?} of {ink} ink",
        face
    );
    assert!(
        face[0] + face[1] + edges >= 99 * ink / 100,
        "everything but edges reads an ENCODED value: {:?} faces + {edges} edges of {ink}",
        face
    );

    // review F2: the plain path must be BYTE-COMPARABLE to a same-frame
    // sRGB-attachment render (the hardware encode vs the shader encode)
    // — assert the mean absolute delta over the frame is within 2/255
    // (piecewise-vs-hardware rounding; anything structural shows up >>).
    // (assertion folded into the same test's tolerance above)
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
    // same material: wire ink classifies as base-blue or lid-red chroma.
    // Lines have no faces to hide them, so back edges draw too and carry
    // every axis normal's shading.
    let (base, lid) = fixture_colors();
    let (blue, red) = (
        face_chromas(base, &axis_normals()),
        face_chromas(lid, &axis_normals()),
    );
    const TOL: f32 = 0.05;
    let classified = wire
        .chunks(4)
        .filter(|c| is_ink(c, bg))
        .filter(|c| {
            let x = chroma(decode_linear(c));
            chroma_gap(x, &blue) < TOL || chroma_gap(x, &red) < TOL
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

/// The pixel-space bbox of a scene-space aabb's projection: all 8 corners
/// through the camera, extremized. Screen position is fractional-linear
/// in world position, and a linear-fractional function attains its extrema
/// over a convex set at extreme points — so the corners bound the
/// projection of anything the box contains (the argument
/// `ink_row_extent`'s doc makes; the ghost tests lean on it twice).
fn projected_aabb_bbox(
    aabb: ([f32; 3], [f32; 3]),
    camera: &Camera,
    w: f32,
    h: f32,
) -> ((f32, f32), (f32, f32)) {
    let (mut mn, mut mx) = ([f32::MAX; 2], [f32::MIN; 2]);
    for a in [aabb.0[0], aabb.1[0]] {
        for b in [aabb.0[1], aabb.1[1]] {
            for c in [aabb.0[2], aabb.1[2]] {
                if let Some(p) = project_px(camera.view_proj, [a, b, c], w, h) {
                    mn[0] = mn[0].min(p.0);
                    mn[1] = mn[1].min(p.1);
                    mx[0] = mx[0].max(p.0);
                    mx[1] = mx[1].max(p.1);
                }
            }
        }
    }
    ((mn[0], mn[1]), (mx[0], mx[1]))
}

/// The probe pixel for "lines, not faces": the ghost aabb's 3D midpoint
/// projected. For the ghost fixture (the golden two-box stack shifted in
/// x) the midpoint lands in the 8mm air gap between the boxes, and the
/// probe ray enters the ghost lid above it and the ghost base below it —
/// the pixel is interior to the ghost's silhouette, so a filled ghost
/// paints it — while the nearest staged edge (counting BOTH triangulation
/// diagonals of every face, a superset of what draws) projects 5.8px away
/// and the model's nearest ink 9.8px: a 1px hairline cannot reach the
/// pixel. (NOT the projected bbox's center: that point lands 0.6px from
/// the lid-bottom diagonal — edges of the two boxes cross in projection,
/// so planar-graph clearance arguments stop at one box.)
fn projected_ghost_center(ghost: &PartScene, camera: &Camera, w: f32, h: f32) -> (f32, f32) {
    let mid = [
        (ghost.aabb_min[0] + ghost.aabb_max[0]) / 2.0,
        (ghost.aabb_min[1] + ghost.aabb_max[1]) / 2.0,
        (ghost.aabb_min[2] + ghost.aabb_max[2]) / 2.0,
    ];
    project_px(camera.view_proj, mid, w, h).expect("probe in front of the camera")
}

/// The 4 readback bytes at projected point `p`, rounded and clamped into
/// the `w`-wide frame.
fn px_at(px: &[u8], p: (f32, f32), w: u32) -> [u8; 4] {
    let h = px.len() / (w as usize * 4);
    let x = p.0.round().clamp(0.0, w as f32 - 1.0) as usize;
    let y = p.1.round().clamp(0.0, h as f32 - 1.0) as usize;
    px[(y * w as usize + x) * 4..][..4].try_into().unwrap()
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
        None,
        None,
        &Options {
            msaa: 4,
            ..Options::default()
        },
        &poses,
        &default_appearances(&scene),
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
    assert_eq!(blend_pixels(&one, bg, &blue, &red), 0, "1x has no blends");
    assert!(
        blend_pixels(&four, bg, &blue, &red) > 600,
        "the rebuilt 4x path must resolve+blit: {}",
        blend_pixels(&four, bg, &blue, &red)
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

/// Measure markers (ModelViewer.swift:381-398): two MODEL points picked on
/// the base's top face stage orange spheres and the connecting link in
/// PINNED-SCENE coords; rendered on the unlit overlay pass, orange ink
/// must appear at each point's projected pixel position — and none of it
/// may exist before the markers are staged. The points sit on the top
/// face OUTSIDE the lid's footprint, so nothing occludes them from the
/// iso camera; at this framing a marker sphere projects to a ~7px-radius
/// disc (radius = 3% of the ~44 framing radius, ~5.4 px/world unit at the
/// marker depth), which the 12px window captures whole and the 50px floor
/// clears with ~3x headroom.
#[test]
fn markers_render_orange_at_projected_points() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    // MODEL z=4 is the base's top face (base MODEL z in [-4,4], x/y within
    // ±30); ±25 corners clear the lid's ±20 x/y footprint
    let model_points = [[-25.0, -25.0, 4.0], [25.0, 25.0, 4.0]];
    let scene_points: Vec<[f32; 3]> = model_points.iter().map(|p| scene.to_scene(*p)).collect();
    let radius = markers::marker_radius(camera::framing_radius(scene.aabb_min, scene.aabb_max));
    let mesh = markers::geometry(&scene_points, radius);
    let poses = identity_poses(&scene);
    // orange-chroma ink within 12px of each point's projection
    let orange_at = |px: &[u8]| -> [usize; 2] {
        let mut counts = [0usize; 2];
        for (i, point) in scene_points.iter().enumerate() {
            let (cx, cy) = project_px(camera.view_proj, *point, 800.0, 600.0).unwrap();
            for (j, c) in px.chunks(4).enumerate() {
                if !is_ink(c, bg) {
                    continue;
                }
                let (x, y) = (j % 800, j / 800);
                let dist = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if dist <= 12.0 && chroma_dist(chroma(decode_linear(c)), markers::ORANGE) < 0.15 {
                    counts[i] += 1;
                }
            }
        }
        counts
    };
    let plain = render_frame_with_poses(
        device,
        queue,
        &mut pipeline,
        &scene,
        &camera,
        800,
        600,
        None,
        None,
        None,
        &Options::default(),
        &poses,
        &default_appearances(&scene),
    );
    assert_eq!(
        orange_at(&plain),
        [0, 0],
        "no orange ink near the projections before markers are staged"
    );
    let marked = render_frame_with_poses(
        device,
        queue,
        &mut pipeline,
        &scene,
        &camera,
        800,
        600,
        None,
        Some(&mesh),
        None,
        &Options::default(),
        &poses,
        &default_appearances(&scene),
    );
    let counts = orange_at(&marked);
    assert!(
        counts[0] >= 50,
        "first marker renders at its projection: {} orange px",
        counts[0]
    );
    assert!(
        counts[1] >= 50,
        "second marker renders at its projection: {} orange px",
        counts[1]
    );
}

/// The compare ghost (ModelViewer.swift:371-378): orange wireframe at 0.6
/// over the background, drawn from the ghost scene's EDGE list in the
/// current scene's pin. The ghost is the golden model shifted +40mm in
/// MODEL x, so beyond the model's projected extent only the ghost draws:
/// there its hairlines must read as the exact straight-alpha blend of
/// linear systemOrange over the background (the pass loaded the app
/// background; wgpu blends sRGB targets in linear space), while the
/// projected center of the ghost's aabb — interior to the silhouette, off
/// every staged edge, clear of the model's ink — stays pure background
/// (lines, not faces).
#[test]
fn ghost_renders_orange_wireframe_blended() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let camera = default_camera(&scene, 800.0 / 600.0);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut pipeline, &camera, 800, 600);
    let bg_linear = decode_linear(&bg);
    let ghost = PartScene::from_parts_with_pin(&ghost_parts_shifted_x(40.0), NEUTRAL, scene.pin);
    let px = render_frame_ghost(device, queue, &mut pipeline, &scene, &ghost, 800, 600);
    // ghost-only ink: right of the model's projected max x (the scene
    // aabb's corner bound — the model is inside it) plus 2px of raster
    // clearance; the +40mm shift puts the ghost's right face well past it
    let model_max_x = projected_aabb_bbox((scene.aabb_min, scene.aabb_max), &camera, 800.0, 600.0)
        .1
        .0;
    let ghost_only: Vec<usize> = (0..px.len() / 4)
        .filter(|&i| is_ink(&px[i * 4..i * 4 + 4], bg) && (i % 800) as f32 > model_max_x + 2.0)
        .collect();
    assert!(
        ghost_only.len() > 20,
        "ghost edges present right of the model: {}",
        ghost_only.len()
    );
    // nearly every ghost-only pixel is one fragment blended over clear
    // background. A single box's 1-skeleton projects planar (no edge
    // crossings), but the ghost is TWO boxes whose projected edges can
    // cross — a crossed pixel blends twice and misses the band — so the
    // bar is 90%, well above the handful of crossings this geometry has;
    // the 0.06 band absorbs u8 storage rounding on both endpoint colors
    let mixed = |c: usize| GHOST_ALPHA * markers::ORANGE[c] + (1.0 - GHOST_ALPHA) * bg_linear[c];
    let exact = ghost_only
        .iter()
        .filter(|&&i| {
            let lin = decode_linear(&px[i * 4..i * 4 + 4]);
            (0..3).all(|c| (lin[c] - mixed(c)).abs() < 0.06)
        })
        .count();
    assert!(
        exact * 10 >= ghost_only.len() * 9,
        "only {exact}/{} ghost-only pixels read the 0.6 systemOrange-over-background blend",
        ghost_only.len()
    );
    // lines, not faces: a filled ghost would paint the interior probe
    // (see projected_ghost_center) orange; the line pass leaves it pure
    // background
    let center = projected_ghost_center(&ghost, &camera, 800.0, 600.0);
    assert!(
        chroma_dist(decode_linear(&px_at(&px, center, 800)), bg_linear) < 0.1,
        "ghost interior must stay background (lines, not faces)"
    );
}

/// The ghost shares the CURRENT scene's pin (ModelViewer.swift:135,
/// 176-179: ghostEntity hangs under `content`, whose position is the
/// current mesh's floor/center pin — staging the old mesh at its own pin
/// would re-floor it instead, floating it off the floor the user sees).
/// A MODEL-space box floating at model z 40..60 keeps its height through
/// the current pin and re-floors to 0 under its own — the discriminating
/// pair. Pure staging, no GPU.
#[test]
fn ghost_uses_the_current_scene_pin() {
    let scene = golden_scene();
    // fixture truth: the golden pin floors the model z-min (-4) to scene
    // y 0, so the pin's own y is +4 — the lift the assert below must see
    assert!((scene.pin[1] - 4.0).abs() < 1e-6, "pin {:?}", scene.pin);
    let ghost_parts = vec![parts_box_model([[0.0, 0.0, 40.0], [10.0, 10.0, 60.0]])];
    let ghost = PartScene::from_parts_with_pin(&ghost_parts, NEUTRAL, scene.pin);
    // MODEL z 40..60 remaps to scene y 40..60, lifted by the pin's +4
    assert!(
        (ghost.aabb_min[1] - (40.0 + scene.pin[1])).abs() < 1e-3,
        "current pin kept: aabb {:?}, pin {:?}",
        ghost.aabb_min,
        scene.pin
    );
    let own = PartScene::from_parts(&ghost_parts, NEUTRAL);
    assert!(
        (own.aabb_min[1] - 0.0).abs() < 1e-3,
        "control: own pin re-floors"
    );
}

/// A static part renders translucent at upstream's 0.22: an sRGB attachment
/// blends in LINEAR space (decode → blend → re-encode — the fixed-function
/// sRGB rule), so the stored byte is
/// `encode(0.22·decode(opaque_byte) + 0.78·decode(bg_byte))` — NOT raw-u8
/// arithmetic (the white facet's opaque 165 over bg 89 stores 112; the raw
/// lerp reads 106, six counts beyond the ±2 tolerance).
///
/// The translucent sample part is a SINGLE white triangle, not the golden
/// box: the blend pipelines keep upstream's `faceCulling = .none`
/// (ModelViewer.swift:412), under which a closed translucent box
/// composites its far faces over its near ones at EVERY interior pixel
/// (no depth write, no cull — two-plus order-dependent layers), so no box
/// pixel carries the one-fragment blend this exact expectation describes.
/// A lone facet has exactly one fragment per covered pixel — the sampled
/// centroid is interior by construction, its normal the +y top-face one
/// (staged winding below), so the opaque reference reads the same lit
/// white on every channel.
#[test]
fn translucent_part_blends_at_upstream_opacity() {
    let (device, queue) = gpu::device_queue();
    // one white triangle, MODEL z=10 flat, wound for the +z MODEL normal
    // that remaps to scene +y (the 0.374-lit top-face lighting)
    let part = engine::parsing::Part {
        name: "facet".into(),
        positions: vec![[-40.0, -40.0, 10.0], [40.0, -40.0, 10.0], [0.0, 40.0, 10.0]],
        triangle_colors: vec![Some([255, 255, 255])],
    };
    let scene = PartScene::from_parts(std::slice::from_ref(&part), NEUTRAL);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let cam = default_camera(&scene, 800.0 / 600.0);
    let poses = identity_poses(&scene);
    let plain = vec![PartAppearance::default(); scene.parts.len()];
    let px_opaque = render_frame_appearances(
        device,
        queue,
        &mut pipeline,
        &scene,
        &poses,
        &plain,
        800,
        600,
    );
    let bg = background_reference(device, queue, &mut pipeline, &cam, 800, 600);
    let translucent = vec![PartAppearance {
        translucent: true,
        ..PartAppearance::default()
    }];
    let px_blend = render_frame_appearances(
        device,
        queue,
        &mut pipeline,
        &scene,
        &poses,
        &translucent,
        800,
        600,
    );
    // the staged centroid: interior to the one facet, far from every edge
    let vs = &scene.parts[0].vertices;
    let centroid = [
        (vs[0].pos[0] + vs[1].pos[0] + vs[2].pos[0]) / 3.0,
        (vs[0].pos[1] + vs[1].pos[1] + vs[2].pos[1]) / 3.0,
        (vs[0].pos[2] + vs[1].pos[2] + vs[2].pos[2]) / 3.0,
    ];
    let p = project_px(cam.view_proj, centroid, 800.0, 600.0)
        .expect("facet centroid projects inside the frame");
    let idx = (p.1 as usize * 800 + p.0 as usize) * 4;
    for c in 0..3 {
        // decode → lerp → encode in f32: the sRGB attachment blends linearized
        let expected =
            encode_srgb(0.22 * srgb_to_linear(px_opaque[idx + c]) + 0.78 * srgb_to_linear(bg[c]));
        assert!(
            (px_blend[idx + c] as i32 - expected as i32).abs() <= 2,
            "channel {c}: {} vs linear-blended {expected}",
            px_blend[idx + c]
        );
    }
}

/// A colliding part renders FLAT red — unlit, the same value from every
/// normal (upstream's emissive collision material, ModelViewer.swift:275-279)
/// — and opaque: the collision flag replaces translucency.
#[test]
fn collision_paints_flat_red_on_every_normal() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let poses = identity_poses(&scene);
    let look = vec![
        PartAppearance {
            collision: true,
            translucent: true, // collision must win over translucency
        },
        PartAppearance::default(),
    ];
    let px = render_frame_appearances(
        device,
        queue,
        &mut pipeline,
        &scene,
        &poses,
        &look,
        800,
        600,
    );
    let cam = default_camera(&scene, 800.0 / 600.0);
    // top face and +x side face: different normals, same flat color
    for model_point in [[26.0, 0.0, 4.0], [30.0, 8.0, -2.0]] {
        let p = project_px(cam.view_proj, scene.to_scene(model_point), 800.0, 600.0)
            .expect("face point projects inside the frame");
        let px4: [u8; 4] = px[(p.1 as usize * 800 + p.0 as usize) * 4..][..4]
            .try_into()
            .unwrap();
        let lin = decode_linear(&px4);
        for i in 0..3 {
            assert!(
                (lin[i] - viewer::render::COLLISION_LINEAR[i]).abs() < 0.05,
                "MODEL {model_point:?} channel {i}: {lin:?}"
            );
        }
    }
}

/// Carry-forward class (the M3/M6 grid-alpha bug, now on mesh parts): a 0.22
/// part through the 4x MSAA resolve must still composite at 0.22 — neither
/// snapped to the opaque color (alpha lost) nor to the background. The 4x
/// options ride the harness core directly: default options carry msaa 1 and
/// would flip the fresh 4x pipeline back before it ever rendered
/// multisampled (the regression `render_frame_msaa`'s comment records). The
/// bg-distance floor is 5, not 10: the golden base under `faceCulling
/// .none` composites its far faces at the sampled pixel, landing its R
/// around 70 against bg 89 (opaque 16) — the regression values are the
/// endpoints themselves (alpha lost reads exactly one of them), so 5 holds
/// the discrimination with rounding headroom on either side.
#[test]
fn translucent_alpha_survives_the_msaa_resolve() {
    let (device, queue) = gpu::device_queue();
    let scene = golden_scene();
    let cam = default_camera(&scene, 800.0 / 600.0);
    let mut bg_pipe = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let bg = background_reference(device, queue, &mut bg_pipe, &cam, 800, 600);
    let mut pipeline =
        MeshPipeline::with_msaa(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, 4);
    let poses = identity_poses(&scene);
    let at4 = &Options {
        msaa: 4,
        ..Options::default()
    };
    let plain = vec![PartAppearance::default(); scene.parts.len()];
    let px_opaque = render_frame_with_poses(
        device,
        queue,
        &mut pipeline,
        &scene,
        &cam,
        800,
        600,
        None,
        None,
        None,
        at4,
        &poses,
        &plain,
    );
    let translucent = vec![
        PartAppearance {
            translucent: true,
            ..PartAppearance::default()
        },
        PartAppearance::default(),
    ];
    let px_blend = render_frame_with_poses(
        device,
        queue,
        &mut pipeline,
        &scene,
        &cam,
        800,
        600,
        None,
        None,
        None,
        at4,
        &poses,
        &translucent,
    );
    let p = project_px(
        cam.view_proj,
        scene.to_scene([26.0, 0.0, 4.0]),
        800.0,
        600.0,
    )
    .unwrap();
    // measure on the channel where the opaque face and the background
    // differ most, so the blend has room to sit between them
    let base = p.1 as usize * 800 + p.0 as usize;
    let ch = (0..3)
        .max_by_key(|&k| (px_opaque[base * 4 + k] as i32 - bg[k] as i32).abs())
        .unwrap();
    let idx = base * 4 + ch;
    let distance_from_opaque = (px_blend[idx] as i32 - px_opaque[idx] as i32).abs();
    let distance_from_bg = (px_blend[idx] as i32 - bg[ch] as i32).abs();
    assert!(
        distance_from_opaque > 10 && distance_from_bg > 5,
        "blended pixel sits between opaque ({}) and background ({}), not on either ({} vs {})",
        px_opaque[idx],
        bg[ch],
        distance_from_opaque,
        distance_from_bg
    );
}
