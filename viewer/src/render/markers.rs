//! Measure markers (facts §2, ModelViewer.swift:381-398): the orange
//! world-space markers upstream's measurement plants — one sphere per
//! picked point, and for exactly two points a connecting box — all one
//! unlit systemOrange material. Pure staging over [`GridVertex`]: the
//! markers ride the GRID overlay pass (unlit, alpha-blended,
//! depth-tested, no depth write) after the grid itself, so there is no
//! pipeline of their own, and residency is content-hash keyed like every
//! other staged buffer — an unchanged measurement costs zero marker bytes
//! however the camera moves; a pick or a clear is what restages (the
//! GPU-first law's marker clause).
//!
//! The sphere is a documented tessellation stand-in for RealityKit's
//! `generateSphere`: 24 segments x 16 rings. A marker is ~3 % of the
//! model's framing radius ([`marker_radius`]), so tessellation detail is
//! invisible at marker scale.
//!
//! [`ORANGE`] is LINEAR systemOrange — sRGB (255, 149, 0) through the
//! exact EOTF every staged color crosses once — pinned by test against
//! `parts::srgb_to_linear_f`, not a hand-waved triple.

use std::hash::Hasher;

use super::GridVertex;

/// The markers' color: linear systemOrange, sRGB (255, 149, 0).
pub const ORANGE: [f32; 3] = [1.0, 0.3005, 0.0];

/// Sphere tessellation (the `generateSphere` stand-in): longitude
/// segments and latitude ring strips.
const SEGMENTS: usize = 24;
const RINGS: usize = 16;

/// Marker sphere radius (ModelViewer.swift:383): `max(radius · 0.03,
/// 0.4)` — 3 % of the framing radius, floored at 0.4 so a tiny model
/// still gets a visible dot.
pub fn marker_radius(framing_radius: f32) -> f32 {
    (framing_radius * 0.03).max(0.4)
}

/// The staged measure markers: overlay vertices plus the full-content
/// identity of those bytes — the same shape
/// [`crate::render::grid::GridMesh`] has, consumed the same way
/// (`Frame::markers` into `MeshPipeline`'s content-keyed residency).
#[derive(Clone, Debug)]
pub struct MarkerMesh {
    pub vertices: Vec<GridVertex>,
    pub content_hash: u64,
}

/// The measure-marker mesh for `points` in PINNED-SCENE coordinates (the
/// caller maps MODEL-space picks through `PartScene::to_scene`): one
/// sphere per point, plus — for exactly two distinct points — the
/// connecting box. Vertices are draw ordered spheres first, link last,
/// so the link's triangles sit at the tail of `vertices`.
pub fn geometry(points: &[[f32; 3]], radius: f32) -> MarkerMesh {
    let color = [ORANGE[0], ORANGE[1], ORANGE[2], 1.0];
    let mut vertices = Vec::new();
    for &p in points {
        push_sphere(p, radius, color, &mut vertices);
    }
    if let [p0, p1] = points {
        push_link(*p0, *p1, radius, color, &mut vertices);
    }
    let content_hash = content_hash(&vertices);
    MarkerMesh {
        vertices,
        content_hash,
    }
}

/// Content identity of the staged marker bytes — the residency contract
/// [`MarkerMesh`] shares with `grid::GridMesh`.
fn content_hash(vertices: &[GridVertex]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_usize(vertices.len());
    h.write(bytemuck::cast_slice(vertices));
    h.finish()
}

/// One UV sphere at `center`: `theta = PI·r/rings` down from the +Y pole,
/// `phi = 2PI·s/segments`, two triangles per (ring, segment) quad for
/// exactly `SEGMENTS·RINGS·2` triangles. The pole rings are the degenerate
/// case — every segment's vertex coincides at the pole — so the first and
/// last strips collapse into fans with one zero-area triangle each: the
/// sphere stays closed and the count stays uniform.
fn push_sphere(center: [f32; 3], radius: f32, color: [f32; 4], out: &mut Vec<GridVertex>) {
    let center = glam::Vec3::from_array(center);
    let point = |r: usize, s: usize| {
        let theta = std::f32::consts::PI * r as f32 / RINGS as f32;
        let phi = 2.0 * std::f32::consts::PI * (s % SEGMENTS) as f32 / SEGMENTS as f32;
        center
            + glam::Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            ) * radius
    };
    for r in 0..RINGS {
        for s in 0..SEGMENTS {
            let quad = [
                point(r, s),
                point(r, s + 1),
                point(r + 1, s + 1),
                point(r + 1, s),
            ];
            for tri in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                for p in tri {
                    out.push(GridVertex {
                        pos: p.to_array(),
                        color,
                    });
                }
            }
        }
    }
}

/// The connecting box between two marker points (ModelViewer.swift:391-
/// 397): RealityKit's `generateBox(size: (radius·0.5, radius·0.5, length))`
/// planted at the midpoint with local +Z aimed p0 -> p1 — `simd_quatf(from:
/// (0,0,1), to:)` parity through `Quat::from_rotation_arc`. A degenerate
/// (zero-length) pair draws nothing.
fn push_link(p0: [f32; 3], p1: [f32; 3], radius: f32, color: [f32; 4], out: &mut Vec<GridVertex>) {
    let (a, b) = (glam::Vec3::from_array(p0), glam::Vec3::from_array(p1));
    let length = (b - a).length();
    if length <= 0.0 {
        return;
    }
    let rot = glam::Quat::from_rotation_arc(glam::Vec3::Z, (b - a) / length);
    let mid = (a + b) / 2.0;
    let half = [radius * 0.25, radius * 0.25, length / 2.0];
    let corners: [[f32; 3]; 8] = std::array::from_fn(|mask| {
        let mut local = [0.0f32; 3];
        for axis in 0..3 {
            local[axis] = if mask & (1 << axis) != 0 {
                half[axis]
            } else {
                -half[axis]
            };
        }
        (mid + rot * glam::Vec3::from_array(local)).to_array()
    });
    super::grid::box_faces(&corners, color, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::parts::srgb_to_linear_f;

    #[test]
    fn radius_clamps_at_four_tenths() {
        assert!((marker_radius(10.0) - 0.4).abs() < 1e-6);
        assert!((marker_radius(1000.0) - 30.0).abs() < 1e-4);
    }

    /// The once-only EOTF crossing, pinned: every channel of ORANGE is
    /// systemOrange's sRGB byte through the exact curve the staging uses.
    #[test]
    fn orange_is_linear_system_orange() {
        assert!((ORANGE[0] - srgb_to_linear_f(1.0)).abs() < 1e-6);
        assert!(
            (ORANGE[1] - srgb_to_linear_f(149.0 / 255.0)).abs() < 1e-4,
            "149/255 -> {}",
            srgb_to_linear_f(149.0 / 255.0)
        );
        assert_eq!(ORANGE[2], 0.0);
    }

    #[test]
    fn one_point_one_sphere_two_points_add_the_link() {
        let one = geometry(&[[0.0, 5.0, 0.0]], 2.0);
        let sphere_tris = 16 * 24 * 2;
        assert_eq!(one.vertices.len(), sphere_tris * 3);
        // a sphere at EVERY point (upstream showMarkers loops the points),
        // so two points stage two spheres plus the 12-triangle link box
        let two = geometry(&[[0.0, 5.0, 0.0], [30.0, 5.0, 0.0]], 2.0);
        assert_eq!(
            two.vertices.len(),
            (sphere_tris * 2 + 12) * 3,
            "two spheres plus the 12-triangle box"
        );
        // every vertex orange, alpha 1
        assert!(
            two.vertices
                .iter()
                .all(|v| v.color == [ORANGE[0], ORANGE[1], ORANGE[2], 1.0])
        );
        // coincident points: spheres but no degenerate link
        let coincident = geometry(&[[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]], 2.0);
        assert_eq!(
            coincident.vertices.len(),
            sphere_tris * 2 * 3,
            "two spheres, no link"
        );
        // zero points: nothing staged
        assert!(geometry(&[], 2.0).vertices.is_empty());
    }

    /// Scale and translate, which the count test cannot see: every sphere
    /// vertex sits exactly `radius` from its point.
    #[test]
    fn sphere_vertices_lie_on_their_radius() {
        let g = geometry(&[[1.0, 2.0, 3.0]], 5.0);
        assert!(!g.vertices.is_empty());
        for v in &g.vertices {
            let d = glam::Vec3::from_array(v.pos) - glam::Vec3::new(1.0, 2.0, 3.0);
            assert!(
                (d.length() - 5.0).abs() < 1e-4,
                "vertex at distance {}",
                d.length()
            );
        }
    }

    #[test]
    fn link_box_spans_the_two_points() {
        let g = geometry(&[[0.0, 5.0, 0.0], [30.0, 5.0, 0.0]], 2.0);
        let sphere_len = 2 * 16 * 24 * 2 * 3;
        assert_eq!(
            g.vertices.len() - sphere_len,
            36,
            "after the spheres the link is one 12-triangle box"
        );
        let xs: Vec<f32> = g.vertices[sphere_len..].iter().map(|v| v.pos[0]).collect();
        assert!(xs.iter().cloned().fold(f32::MIN, f32::max) >= 29.0);
        assert!(xs.iter().cloned().fold(f32::MAX, f32::min) <= 1.0);
    }

    /// Orientation, which the axis-aligned span test cannot see: a
    /// diagonal pair's box must hug the diagonal segment — every link
    /// vertex projects inside the segment's span and sits no farther from
    /// its line than the cross-section's half-diagonal.
    #[test]
    fn link_box_follows_a_diagonal() {
        let (p0, p1) = ([0.0, 5.0, 0.0], [30.0, 35.0, 0.0]);
        let g = geometry(&[p0, p1], 2.0);
        let sphere_len = 2 * 16 * 24 * 2 * 3;
        let (a, b) = (glam::Vec3::from_array(p0), glam::Vec3::from_array(p1));
        let length = (b - a).length();
        let dir = (b - a) / length;
        let half_diag = 2.0 * 0.25 * std::f32::consts::SQRT_2;
        for v in &g.vertices[sphere_len..] {
            let p = glam::Vec3::from_array(v.pos);
            let t = (p - a).dot(dir);
            assert!(
                (0.0..=length).contains(&t),
                "vertex projects to {t}, outside the segment [0, {length}]"
            );
            let off = (p - (a + dir * t)).length();
            assert!(
                off <= half_diag + 1e-4,
                "vertex sits {off} from the segment's line, cross-section half-diagonal is {half_diag}"
            );
        }
    }

    /// The residency key: same input, same identity; a moved point or a
    /// resized marker restages.
    #[test]
    fn content_hash_tracks_the_staged_points() {
        let a = geometry(&[[0.0, 0.0, 0.0]], 1.0);
        let b = geometry(&[[0.0, 0.0, 0.0]], 1.0);
        let moved = geometry(&[[1.0, 0.0, 0.0]], 1.0);
        let bigger = geometry(&[[0.0, 0.0, 0.0]], 2.0);
        assert_eq!(a.content_hash, b.content_hash);
        assert_ne!(a.content_hash, moved.content_hash);
        assert_ne!(a.content_hash, bigger.content_hash);
    }
}
