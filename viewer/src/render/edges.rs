//! CAD feature edges: the line list wireframe mode and the compare ghost
//! draw.
//!
//! A triangle mesh carries two kinds of edges. Feature edges outline the
//! solid as its designer drew it: the rim of a hole, the corner of a box, an
//! open boundary. Tessellation edges exist only because a face was cut into
//! triangles: a planar face's diagonals, the seams between a cylinder's
//! facets. [`feature_edges`] keeps the first kind:
//!
//! - a boundary edge (one adjacent triangle), or a non-manifold edge (three
//!   or more), always;
//! - an edge between exactly two triangles when their face normals differ by
//!   more than the crease angle ([`CREASE_DEG`]).
//!
//! STL and 3MF meshes are often unindexed or split per face, so adjacency
//! comes from POSITION: corners within a weld tolerance (relative to the
//! mesh's bounding box diagonal) are one vertex. Without that weld every
//! edge of a triangle soup would read as a boundary.

use std::collections::HashMap;

/// The crease angle in degrees: two faces meeting at more than this read as
/// a feature edge; at or below it, as one smooth surface.
pub const CREASE_DEG: f32 = 30.0;

/// Weld tolerance as a fraction of the bounding box diagonal.
const WELD_REL: f64 = 1e-5;

/// Slack on the crease comparison (in cosine), so an angle equal to the
/// crease within f32 position noise stays smooth: about 1e-4 degrees at a
/// 30° crease.
const CREASE_SLACK: f64 = 1e-6;

/// The feature edges of a triangle list, as index pairs into `positions`.
///
/// `indices` holds three entries per triangle (a trailing partial triangle
/// is ignored). Each feature edge appears once, keyed by its welded
/// endpoints, and carries the corner indices of the first triangle that
/// names it; output order follows the triangles, so it is deterministic.
/// Degenerate triangles (two corners welded together, or zero area)
/// contribute no edges and no adjacency.
pub fn feature_edges(positions: &[[f32; 3]], indices: &[u32], crease_deg: f32) -> Vec<[u32; 2]> {
    let welded = weld(positions);
    let cos_crease = f64::from(crease_deg).to_radians().cos();
    let pos = |i: u32| positions[i as usize].map(f64::from);
    // welded edge -> (first corner pair, adjacent face normals); `order`
    // keeps first-appearance order
    let mut adjacency: HashMap<(u32, u32), (usize, Vec<[f64; 3]>)> = HashMap::new();
    let mut order: Vec<[u32; 2]> = Vec::new();
    for tri in indices.chunks_exact(3) {
        let w = [
            welded[tri[0] as usize],
            welded[tri[1] as usize],
            welded[tri[2] as usize],
        ];
        if w[0] == w[1] || w[1] == w[2] || w[2] == w[0] {
            continue;
        }
        let Some(normal) = unit_normal(pos(tri[0]), pos(tri[1]), pos(tri[2])) else {
            continue;
        };
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let key = (w[a].min(w[b]), w[a].max(w[b]));
            let entry = adjacency.entry(key).or_insert_with(|| {
                order.push([tri[a], tri[b]]);
                (order.len() - 1, Vec::new())
            });
            entry.1.push(normal);
        }
    }
    let mut keep = vec![false; order.len()];
    for (slot, normals) in adjacency.values() {
        keep[*slot] = match normals.as_slice() {
            [n0, n1] => dot(*n0, *n1) < cos_crease - CREASE_SLACK,
            _ => true,
        };
    }
    order
        .into_iter()
        .zip(keep)
        .filter_map(|(edge, keep)| keep.then_some(edge))
        .collect()
}

/// One welded vertex id per position: positions within the weld tolerance
/// of an earlier representative take its id. A hash grid with cells one
/// tolerance wide keeps the search to the 27 cells around each point.
fn weld(positions: &[[f32; 3]]) -> Vec<u32> {
    let points: Vec<[f64; 3]> = positions.iter().map(|p| p.map(f64::from)).collect();
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in &points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let diagonal = (0..3).map(|k| (hi[k] - lo[k]).powi(2)).sum::<f64>().sqrt();
    let tol = diagonal * WELD_REL;
    // a zero tolerance (empty or single-point mesh) welds exact matches
    // only; any cell size works for that
    let cell_size = if tol > 0.0 { tol } else { 1.0 };
    let cell = |p: [f64; 3]| p.map(|c| (c / cell_size).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut reps: Vec<[f64; 3]> = Vec::new();
    points
        .iter()
        .map(|&p| {
            let c = cell(p);
            let near = (-1..=1)
                .flat_map(|dx| (-1..=1).flat_map(move |dy| (-1..=1).map(move |dz| [dx, dy, dz])))
                .filter_map(|d| grid.get(&[c[0] + d[0], c[1] + d[1], c[2] + d[2]]))
                .flatten()
                .copied()
                .find(|&id| dist2(reps[id as usize], p) <= tol * tol);
            near.unwrap_or_else(|| {
                let id = reps.len() as u32;
                reps.push(p);
                grid.entry(c).or_default().push(id);
                id
            })
        })
        .collect()
}

/// The unit normal of triangle (a, b, c) by its winding, or `None` for a
/// zero-area triangle.
fn unit_normal(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Option<[f64; 3]> {
    let (u, v) = (sub(b, a), sub(c, a));
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = dot(n, n).sqrt();
    (len > 0.0).then(|| n.map(|x| x / len))
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unindexed soup's identity index list.
    fn soup(positions: &[[f32; 3]]) -> Vec<u32> {
        (0..positions.len() as u32).collect()
    }

    /// Unit cube corners, indexed `x + 2y + 4z`.
    fn cube_corners() -> Vec<[f32; 3]> {
        (0..8)
            .map(|i| [(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32])
            .collect()
    }

    /// The cube's 12 outward-wound triangles, two per face.
    fn cube_indices() -> Vec<u32> {
        // each face as a quad a,b,c,d counter-clockwise from outside
        let quads: [[u32; 4]; 6] = [
            [0, 2, 3, 1], // z = 0
            [4, 5, 7, 6], // z = 1
            [0, 1, 5, 4], // y = 0
            [2, 6, 7, 3], // y = 1
            [0, 4, 6, 2], // x = 0
            [1, 3, 7, 5], // x = 1
        ];
        quads
            .iter()
            .flat_map(|&[a, b, c, d]| [a, b, c, a, c, d])
            .collect()
    }

    /// The edge set as sorted position pairs, independent of which corner
    /// copy the function chose.
    fn edge_positions(positions: &[[f32; 3]], edges: &[[u32; 2]]) -> Vec<[[i64; 3]; 2]> {
        let key = |i: u32| positions[i as usize].map(|c| (c * 1000.0).round() as i64);
        let mut out: Vec<_> = edges
            .iter()
            .map(|&[a, b]| {
                let (pa, pb) = (key(a), key(b));
                if pa <= pb { [pa, pb] } else { [pb, pa] }
            })
            .collect();
        out.sort();
        out
    }

    fn axis_aligned(e: &[[i64; 3]; 2]) -> bool {
        (0..3).filter(|&k| e[0][k] != e[1][k]).count() == 1
    }

    #[test]
    fn indexed_cube_yields_its_12_edges_and_no_diagonals() {
        let positions = cube_corners();
        let edges = feature_edges(&positions, &cube_indices(), CREASE_DEG);
        assert_eq!(edges.len(), 12, "a cube has 12 edges: {edges:?}");
        let pos = edge_positions(&positions, &edges);
        assert!(pos.iter().all(axis_aligned), "no face diagonal: {pos:?}");
        pos.windows(2)
            .for_each(|w| assert_ne!(w[0], w[1], "each edge appears once"));
    }

    #[test]
    fn unindexed_cube_welds_to_the_same_12_edges() {
        let corners = cube_corners();
        let positions: Vec<[f32; 3]> = cube_indices()
            .iter()
            .map(|&i| corners[i as usize])
            .collect();
        assert_eq!(positions.len(), 36);
        let edges = feature_edges(&positions, &soup(&positions), CREASE_DEG);
        assert_eq!(edges.len(), 12, "per-face corners weld by position");
        let indexed = feature_edges(&corners, &cube_indices(), CREASE_DEG);
        assert_eq!(
            edge_positions(&positions, &edges),
            edge_positions(&corners, &indexed),
            "unindexed and indexed cubes draw the same lines"
        );
        for &[a, b] in &edges {
            assert!(
                a < 36 && b < 36 && a != b,
                "indices address the soup's corners"
            );
        }
    }

    #[test]
    fn weld_absorbs_float_noise_relative_to_the_model_size() {
        // a 200-unit cube whose per-face copies disagree in the 6th
        // significant digit, as an ASCII STL export leaves them
        let corners: Vec<[f32; 3]> = cube_corners()
            .iter()
            .map(|c| c.map(|v| v * 200.0))
            .collect();
        let positions: Vec<[f32; 3]> = cube_indices()
            .iter()
            .enumerate()
            .map(|(k, &i)| {
                let jitter = if k % 2 == 0 { 1e-4 } else { -1e-4 };
                corners[i as usize].map(|v| v + jitter)
            })
            .collect();
        let edges = feature_edges(&positions, &soup(&positions), CREASE_DEG);
        assert_eq!(edges.len(), 12);
    }

    #[test]
    fn flat_square_of_two_triangles_keeps_its_4_boundary_edges() {
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let edges = feature_edges(&positions, &[0, 1, 2, 0, 2, 3], CREASE_DEG);
        let pos = edge_positions(&positions, &edges);
        assert_eq!(pos.len(), 4, "4 sides, no diagonal: {pos:?}");
        assert!(pos.iter().all(axis_aligned));
    }

    /// A closed cylinder of `n` side facets: caps as center fans, sides as
    /// quads split into two triangles. Outward winding throughout.
    fn cylinder(n: usize) -> (Vec<[f32; 3]>, Vec<u32>) {
        let mut positions = Vec::new();
        for z in [0.0f32, 1.0] {
            for k in 0..n {
                let a = std::f32::consts::TAU * k as f32 / n as f32;
                positions.push([a.cos(), a.sin(), z]);
            }
        }
        let (bot_c, top_c) = (2 * n as u32, 2 * n as u32 + 1);
        positions.push([0.0, 0.0, 0.0]);
        positions.push([0.0, 0.0, 1.0]);
        let mut indices = Vec::new();
        for k in 0..n as u32 {
            let k1 = (k + 1) % n as u32;
            let (b0, b1, t0, t1) = (k, k1, n as u32 + k, n as u32 + k1);
            indices.extend([bot_c, b1, b0]); // bottom cap faces -z
            indices.extend([top_c, t0, t1]); // top cap faces +z
            indices.extend([b0, b1, t1, b0, t1, t0]); // side faces outward
        }
        (positions, indices)
    }

    #[test]
    fn faceted_cylinder_keeps_only_its_cap_circles() {
        // 24 facets: neighbouring side normals differ by 15 degrees
        let n = 24;
        let (positions, indices) = cylinder(n);
        let edges = feature_edges(&positions, &indices, CREASE_DEG);
        let pos = edge_positions(&positions, &edges);
        assert_eq!(pos.len(), 2 * n, "the two rim circles only: {pos:?}");
        // every surviving edge lies in a cap plane: no seam, no diagonal,
        // no fan spoke (a spoke would touch the center)
        for e in &pos {
            assert_eq!(e[0][2], e[1][2], "edge lies in a cap plane: {e:?}");
            assert!(
                e.iter().all(|p| p[0] != 0 || p[1] != 0),
                "no edge reaches a cap center: {e:?}"
            );
        }
    }

    #[test]
    fn coarse_cylinder_seams_are_features() {
        // 8 facets: 45 degrees between side normals, past the crease
        let n = 8;
        let (positions, indices) = cylinder(n);
        let edges = feature_edges(&positions, &indices, CREASE_DEG);
        assert_eq!(
            edges.len(),
            3 * n,
            "two rims plus one vertical seam per facet"
        );
    }

    /// Two triangles sharing the edge (0,0,0)-(1,0,0), the second folded
    /// about it so the face normals differ by `deg` degrees.
    fn hinge(deg: f64) -> Vec<[f32; 3]> {
        let t = deg.to_radians();
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.5, -1.0, 0.0],
            [0.5, t.cos() as f32, t.sin() as f32],
        ]
    }

    #[test]
    fn crease_threshold_is_exclusive() {
        let indices = [0, 1, 2, 1, 0, 3];
        let count = |deg: f64| feature_edges(&hinge(deg), &indices, CREASE_DEG).len();
        assert_eq!(count(0.0), 4, "flat: the 4 boundary edges only");
        assert_eq!(count(29.0), 4, "below the crease: smooth");
        assert_eq!(count(30.0), 4, "exactly at the crease: smooth");
        assert_eq!(count(31.0), 5, "past the crease: the hinge is a feature");
        assert_eq!(count(90.0), 5);
    }

    #[test]
    fn non_manifold_edges_are_features() {
        // three fins on one shared edge
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.5, 1.0, 0.0],
            [0.5, -1.0, 0.0],
            [0.5, 0.0, 1.0],
        ];
        let edges = feature_edges(&positions, &[0, 1, 2, 1, 0, 3, 0, 1, 4], CREASE_DEG);
        let pos = edge_positions(&positions, &edges);
        let shared = [[0, 0, 0], [1000, 0, 0]];
        assert!(pos.contains(&shared), "an edge with 3 faces draws: {pos:?}");
    }

    #[test]
    fn degenerate_triangles_add_nothing() {
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
        ];
        assert!(feature_edges(&positions, &[0, 1, 2, 0, 3, 1], CREASE_DEG).is_empty());
        assert!(feature_edges(&[], &[], CREASE_DEG).is_empty());
    }
}
