//! Parity grid, bed outline, and axis tripod (facts §2, ModelViewer.swift
//! rebuildGrid/showBed/axisEntity). All of it is UNLIT transparent overlay
//! geometry drawn by the grid pipeline after the mesh pass: no depth write
//! (facts #34's overlay contract), depth-TESTED so the model occludes it.
//!
//! wgpu has no wide lines, so lines render as thin quads in a triangle
//! pass — exactly upstream's `quad(from:to:width:)` construction
//! (ModelViewer.swift:458-462): side = cross(along, (0,1,0)) · width/2, six
//! vertices per line. The tripod is upstream's BOXES (`axisEntity`,
//! generateBox) expanded to 12 triangles each — a box always presents a
//! face to the camera, which a flat quad cannot promise (the vertical axis
//! quad is degenerate under cross(along, (0,1,0)) in the first place).
//!
//! The scene frame these coordinates assume is the pinned one `from_parts`
//! produces: model floor at scene y=0, x/z centered. The grid plane sits
//! just below the floor at y = −0.05, the bed just above it at y = 0.06
//! (both absolute, facts #34/#37); the tripod rises from y=0 at the
//! footprint corner.
//!
//! The bed outline is M6 scope pulled in early: the geometry primitive
//! lives here with the grid, while its plate-view integration — feeding
//! the real bed rectangle from project state instead of the `bed:
//! Option<[f64; 2]>` argument the adapter currently passes as `None` —
//! is M6's to wire.
//!
//! Colors are LINEAR rgb + straight alpha in [`GridVertex`] — the same
//! recorded assumption as [`crate::render::parts::NEUTRAL`]. The bed
//! accent is the fixed surfaceless test stand-in for upstream's
//! controlAccentColor (libcosmic themes are unavailable headless); grid
//! lines are dark-mode white (light-mode black arrives with theming, M4+).

use std::hash::Hasher;

/// The bed outline's fixed accent color (upstream: controlAccentColor —
/// unavailable surfaceless; this constant is the sanctioned stand-in).
pub const ACCENT: [f32; 3] = [0.29, 0.56, 0.91];

/// Grid lines sit this far below the pinned model floor (facts #34).
const GRID_Y: f32 = -0.05;
/// The bed rectangle sits this far above the floor (facts #37).
const BED_Y: f32 = 0.06;
/// Minor lines every 10 units (facts #34).
const STEP: f32 = 10.0;
/// Line widths in world units: majors 0.5, minors 0.25 (facts #34).
const MAJOR_WIDTH: f32 = 0.5;
const MINOR_WIDTH: f32 = 0.25;
/// Alphas: minors 0.10, majors 0.28 (facts #34).
const MINOR_ALPHA: f32 = 0.10;
const MAJOR_ALPHA: f32 = 0.28;
/// Dark-mode grid color (facts #34; light-mode black comes with theming).
const LINE_COLOR: [f32; 3] = [1.0, 1.0, 1.0];

/// One unlit overlay vertex: world position plus linear rgba.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GridVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

/// One 10mm grid step: the offset both axis lines share, and whether it
/// is a 100mm major (facts #34: `Int(offset.rounded()) % 100 == 0`).
#[derive(Clone, Copy, Debug)]
pub struct GridStep {
    pub offset: f32,
    pub major: bool,
}

/// The grid + bed + tripod overlay, built once per scene change (upstream
/// rebuilds it when the mesh changes, facts #32). `vertices` is draw
/// ordered: grid lines (step order), then the bed rectangle, then the
/// tripod boxes — the pass writes no depth, so painter's order is the only
/// ordering, and the opaque bed must cover the lines it overlaps.
#[derive(Clone, Debug)]
pub struct GridMesh {
    pub half_extent: f32,
    pub steps: Vec<GridStep>,
    pub vertices: Vec<GridVertex>,
    /// Full-content identity over the staged vertices — the same
    /// residency contract as [`crate::render::parts::GpuPart`].
    pub content_hash: u64,
}

/// Grid half-extent from the MODEL footprint (facts #33,
/// ModelViewer.swift:428): `max(100, (Int(footprint·0.75/10)+1)·10)` —
/// Int() truncates, i.e. floor-then-+1, NOT ceil: footprint 200 reads 160
/// where a ceil-based port reads 150.
pub fn half_extent(footprint: f32) -> f32 {
    let rings = (footprint * 0.75 / 10.0).floor() as i32 + 1;
    (rings * 10).max(100) as f32
}

/// One thin line as a 6-vertex quad pair — upstream's `quad(from:to:width:)`
/// verbatim (ModelViewer.swift:458-462): side = cross(along, (0,1,0)) ·
/// width/2, so a horizontal line's quad lies flat in its plane (edge-on
/// only at a grazing camera, exactly like upstream). A degenerate
/// (zero-length) line draws nothing.
fn line_quad(from: [f32; 3], to: [f32; 3], width: f32, color: [f32; 4], out: &mut Vec<GridVertex>) {
    let (from, to) = (glam::Vec3::from_array(from), glam::Vec3::from_array(to));
    let along = to - from;
    let len = along.length();
    if len <= 0.0 {
        return;
    }
    let side = (along / len).cross(glam::Vec3::Y) * (width / 2.0);
    let mut push = |p: glam::Vec3| {
        out.push(GridVertex {
            pos: p.to_array(),
            color,
        })
    };
    push(from - side);
    push(to - side);
    push(to + side);
    push(from - side);
    push(to + side);
    push(from + side);
}

/// A closed box's 12 triangles over its 8 corners — the face topology
/// every unlit box in the overlay family shares (the tripod axes here,
/// the measure link in [`crate::render::markers`]). `corners` are
/// mask-ordered (bit a set = + half-extent on axis a): the enumeration
/// this table is written against — a reordered enumeration bowties it,
/// which the headless suite's `box_scene` fixture pins with a
/// signed-volume assert over the same topology.
pub(crate) fn box_faces(corners: &[[f32; 3]; 8], color: [f32; 4], out: &mut Vec<GridVertex>) {
    let faces = [
        [0usize, 2, 6, 4],
        [1, 5, 7, 3],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 1, 3, 2],
        [4, 6, 7, 5],
    ];
    for &[a, b, c, d] in &faces {
        for &p in &[a, b, c, a, c, d] {
            out.push(GridVertex {
                pos: corners[p],
                color,
            });
        }
    }
}

/// An axis-aligned box as 12 triangles, every face `color` — upstream's
/// axisEntity box (facts #36): unlit and uncull, so a solid silhouette
/// from any angle.
fn box_triangles(center: [f32; 3], size: [f32; 3], color: [f32; 4], out: &mut Vec<GridVertex>) {
    let half = [size[0] / 2.0, size[1] / 2.0, size[2] / 2.0];
    let corners: [[f32; 3]; 8] = std::array::from_fn(|mask| {
        let mut p = center;
        for a in 0..3 {
            p[a] += if mask & (1 << a) != 0 {
                half[a]
            } else {
                -half[a]
            };
        }
        p
    });
    box_faces(&corners, color, out);
}

/// Build the overlay for the pinned scene `aabb` and an optional bed
/// `[x, y]` in mm (upstream's manifest SIMD2, facts #37). The footprint
/// driving the extents is the MODEL x/y span — scene x/z after the remap.
pub fn geometry(aabb: ([f32; 3], [f32; 3]), bed: Option<[f64; 2]>) -> GridMesh {
    let (min, max) = aabb;
    let mut size = [0.0f32; 3];
    for a in 0..3 {
        size[a] = max[a] - min[a];
    }
    let footprint = size[0].max(size[2]);
    let half = half_extent(footprint);

    let mut vertices = Vec::new();
    // Grid lines (facts #34): every 10mm step draws both axis directions;
    // integer step arithmetic keeps offsets and majors exact (Swift's
    // `Int(offset.rounded()) % 100` over float accumulation, same values).
    let n = (half / STEP) as i32;
    let mut steps = Vec::with_capacity((2 * n + 1) as usize);
    for i in -n..=n {
        let offset = i * 10;
        let major = offset % 100 == 0;
        steps.push(GridStep {
            offset: offset as f32,
            major,
        });
        let (width, alpha) = if major {
            (MAJOR_WIDTH, MAJOR_ALPHA)
        } else {
            (MINOR_WIDTH, MINOR_ALPHA)
        };
        let color = [LINE_COLOR[0], LINE_COLOR[1], LINE_COLOR[2], alpha];
        let (o, h) = (offset as f32, half);
        line_quad([o, GRID_Y, -h], [o, GRID_Y, h], width, color, &mut vertices);
        line_quad([-h, GRID_Y, o], [h, GRID_Y, o], width, color, &mut vertices);
    }

    // Bed rectangle (facts #37): the manifest bed as an accent outline at
    // y = 0.06, corners ±half-bed, line width max(x,y)/250.
    if let Some([x, y]) = bed {
        let (hx, hy) = (x as f32 / 2.0, y as f32 / 2.0);
        let width = x.max(y) as f32 / 250.0;
        let color = [ACCENT[0], ACCENT[1], ACCENT[2], 1.0];
        let corners = [
            [-hx, BED_Y, hy],
            [hx, BED_Y, hy],
            [hx, BED_Y, -hy],
            [-hx, BED_Y, -hy],
        ];
        for i in 0..4 {
            line_quad(
                corners[i],
                corners[(i + 1) % 4],
                width,
                color,
                &mut vertices,
            );
        }
    }

    // Axis tripod (facts #36): RGB boxes at the footprint corner + 15%
    // margin, each axis `max(all-spans·0.3, 5)` long. Swift's corner z is
    // `|sceneVector(size).z|/2 + margin` = half the MODEL y extent (a
    // ground-plane span; MODEL is Z-up) — the scene-z extent, size[2].
    let margin = (footprint * 0.15).max(3.0);
    let corner = [-size[0] / 2.0 - margin, 0.0, size[2] / 2.0 + margin];
    let axis_length = (size[0].max(size[1]).max(size[2]) * 0.3).max(5.0);
    let thickness = (axis_length / 40.0).max(0.6);
    // Apple systemRed/Green/Blue sRGB triples — the same u8-sRGB→linear
    // crossing every other color in this crate takes.
    for (dir, rgb) in [
        ([1.0f32, 0.0, 0.0], [255u8, 59, 48]), // +X red
        ([0.0, 0.0, -1.0], [52, 199, 89]),     // +Y green
        ([0.0, 1.0, 0.0], [0, 122, 255]),      // +Z blue
    ] {
        // box size = dir·length + (1−|dir|)·thickness per axis, centered
        // at corner + dir·length/2 (axisEntity, facts #36)
        let mut sz = [0.0f32; 3];
        let mut center = corner;
        for a in 0..3 {
            sz[a] = dir[a].abs() * axis_length + (1.0 - dir[a].abs()) * thickness;
            center[a] += dir[a] * axis_length / 2.0;
        }
        let color = [
            super::parts::srgb_to_linear(rgb[0]),
            super::parts::srgb_to_linear(rgb[1]),
            super::parts::srgb_to_linear(rgb[2]),
            1.0,
        ];
        box_triangles(center, sz, color, &mut vertices);
    }

    let content_hash = content_hash(&vertices, half);
    GridMesh {
        half_extent: half,
        steps,
        vertices,
        content_hash,
    }
}

/// Content identity of the staged overlay bytes (see [`GridMesh`]'s
/// residency note) — the grid twin of `parts::content_hash`.
fn content_hash(vertices: &[GridVertex], half_extent: f32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_usize(vertices.len());
    h.write(&half_extent.to_ne_bytes());
    h.write(bytemuck::cast_slice(vertices));
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    /// The extents formula and the major/minor step layout, pure: footprint
    /// 260 -> 200 and the DISCRIMINATING footprint 200 -> 160 (a ceil-based
    /// port reads 150), majors exactly at the 100 multiples inside ±half
    /// (±160 are minors), line quads on the y = −0.05 plane, and the bed.
    #[test]
    fn grid_extents_and_majors() {
        assert_eq!(half_extent(260.0), 200.0, "(floor(19.5)+1)·10");
        assert_eq!(
            half_extent(200.0),
            160.0,
            "floor-then-+1, NOT ceil — a ceil-based port reads 150 here"
        );
        assert_eq!(half_extent(140.0), 110.0, "(floor(10.5)+1)·10");
        assert_eq!(half_extent(80.0), 100.0, "the max(100, ·) floor");

        // scene aabb with x span 200, z span 180 -> MODEL footprint 200
        let aabb = ([-100.0, -10.0, -90.0], [100.0, 10.0, 90.0]);
        let g = geometry(aabb, None);
        assert_eq!(g.half_extent, 160.0);
        assert_eq!(g.steps.len(), 33, "10mm steps from -160 to +160 inclusive");
        assert_eq!(g.steps.first().unwrap().offset, -160.0);
        assert_eq!(g.steps.last().unwrap().offset, 160.0);
        let majors: Vec<f32> = g
            .steps
            .iter()
            .filter(|s| s.major)
            .map(|s| s.offset)
            .collect();
        assert_eq!(
            majors,
            vec![-100.0, 0.0, 100.0],
            "±160 are minors: 160 % 100 != 0"
        );
        // structure: 33 steps x 2 directions x 6-vertex quads, then the
        // tripod's 3 boxes x 12 triangles x 3 corners; no bed
        let n_lines = 33 * 2 * 6;
        assert_eq!(g.vertices.len(), n_lines + 3 * 36);
        // the grid plane: every line vertex sits at y = -0.05, below the
        // pinned floor (facts #34)
        for v in &g.vertices[..n_lines] {
            assert_eq!(v.pos[1], GRID_Y, "grid lines live on the y=-0.05 plane");
        }
        // one major quad, exactly (Swift quad() order): the offset-0 x-line
        // runs (0,-0.05,-160) -> (0,-0.05,160); along = (0,0,1), so
        // side = cross(along, (0,1,0)) · 0.25 = (-0.25, 0, 0)
        let major_color = [LINE_COLOR[0], LINE_COLOR[1], LINE_COLOR[2], MAJOR_ALPHA];
        let at = |x: f32, z: f32| GridVertex {
            pos: [x, GRID_Y, z],
            color: major_color,
        };
        let step = g.steps.iter().position(|s| s.offset == 0.0).unwrap();
        let x_line = &g.vertices[step * 12..step * 12 + 6];
        let expected = [
            at(0.25, -160.0),
            at(0.25, 160.0),
            at(-0.25, 160.0),
            at(0.25, -160.0),
            at(-0.25, 160.0),
            at(-0.25, -160.0),
        ];
        for (v, e) in x_line.iter().zip(expected) {
            assert!(close(v.pos, e.pos), "major quad {:?} vs {:?}", v.pos, e.pos);
            assert_eq!(v.color, e.color);
        }
        // a minor: offset 10, width 0.25 -> x ±0.125, alpha 0.10
        let minor = g.steps.iter().position(|s| s.offset == 10.0).unwrap();
        let v = &g.vertices[minor * 12];
        assert!(close(v.pos, [10.125, GRID_Y, -160.0]), "{:?}", v.pos);
        assert_eq!(
            v.color,
            [LINE_COLOR[0], LINE_COLOR[1], LINE_COLOR[2], MINOR_ALPHA]
        );

        // bed Some: accent rectangle at y = 0.06 after the lines, before the
        // tripod; bed None drew none of those
        let g = geometry(aabb, Some([80.0, 80.0]));
        let n_bed = 4 * 6;
        assert_eq!(g.vertices.len(), n_lines + n_bed + 3 * 36);
        for v in &g.vertices[n_lines..n_lines + n_bed] {
            assert_eq!(v.pos[1], BED_Y, "bed rectangle lives at y=0.06");
            assert_eq!(v.color, [ACCENT[0], ACCENT[1], ACCENT[2], 1.0]);
        }
        // bed line width max(x,y)/250 = 0.32: the first quad runs
        // (-40,0.06,40) -> (40,0.06,40), along (1,0,0),
        // side = (0,0,1)·0.16, so it starts at z = 40 - 0.16
        assert!(
            close(g.vertices[n_lines].pos, [-40.0, BED_Y, 40.0 - 0.16]),
            "{:?}",
            g.vertices[n_lines].pos
        );
    }
}
