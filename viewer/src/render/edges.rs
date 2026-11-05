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

/// The crease angle in degrees: two faces meeting at more than this read as
/// a feature edge; at or below it, as one smooth surface.
pub const CREASE_DEG: f32 = 30.0;

/// Weld tolerance as a fraction of the bounding box diagonal.
const WELD_REL: f64 = 1e-5;

/// Weld tolerance floor as a fraction of the largest coordinate magnitude:
/// four f32 ulps, so a small part far from the origin, whose float noise
/// outgrows its own diagonal's share, still welds.
const WELD_ULPS: f64 = 4.0 * f32::EPSILON as f64;

/// Weld grid cell size in tolerances. A point probes a neighbouring cell
/// only on an axis where it lies within one tolerance of the cell wall, so
/// wider cells keep more points to their own cell, at the cost of more
/// representatives per cell to scan.
const CELL_TOLS: f64 = 4.0;

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
    edge_records(positions, indices, crease_deg)
        .into_iter()
        .filter(EdgeRecord::is_feature)
        .map(|r| r.corners)
        .collect()
}

/// The edges wireframe mode and the compare ghost draw: the feature edges
/// ([`feature_edges`] at [`CREASE_DEG`]), or every welded triangle edge
/// once when there are none. A closed smooth body (sphere, torus) has no
/// boundary and no crease; without the fallback it would draw nothing.
pub fn wire_edges(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[u32; 2]> {
    let records = edge_records(positions, indices, CREASE_DEG);
    let features: Vec<[u32; 2]> = records
        .iter()
        .filter(|r| r.is_feature())
        .map(|r| r.corners)
        .collect();
    if features.is_empty() {
        records.into_iter().map(|r| r.corners).collect()
    } else {
        features
    }
}

/// One welded edge: the corner pair of the first triangle that names it,
/// and whether it is a feature (boundary, non-manifold, or a crease).
struct EdgeRecord {
    corners: [u32; 2],
    feature: bool,
}

impl EdgeRecord {
    fn is_feature(&self) -> bool {
        self.feature
    }
}

/// Every welded edge of the non-degenerate triangles, in first-appearance
/// order. Half-edge `h` runs from corner `h % 3` of triangle `h / 3` to the
/// next corner; half-edges bucket by their lower welded endpoint (a
/// counting sort), so one edge's half-edges meet in one short bucket and
/// no hash map is needed.
fn edge_records(positions: &[[f32; 3]], indices: &[u32], crease_deg: f32) -> Vec<EdgeRecord> {
    let (welded, vertex_count) = weld(positions);
    let cos_crease = f64::from(crease_deg).to_radians().cos() - CREASE_SLACK;
    let indices = &indices[..indices.len() / 3 * 3];
    let id = |corner: usize| welded[indices[corner] as usize];
    // per triangle: its unit normal, `None` when degenerate
    let normals: Vec<Option<[f32; 3]>> = indices
        .chunks_exact(3)
        .enumerate()
        .map(|(t, tri)| {
            let w = [id(3 * t), id(3 * t + 1), id(3 * t + 2)];
            if w[0] == w[1] || w[1] == w[2] || w[2] == w[0] {
                return None;
            }
            let corner = |k: usize| positions[tri[k] as usize];
            unit_normal(corner(0), corner(1), corner(2))
        })
        .collect();
    let ends = |h: usize| {
        let (a, b) = (id(h), id(h - h % 3 + (h + 1) % 3));
        (a.min(b), a.max(b))
    };
    let live = || (0..indices.len()).filter(|&h| normals[h / 3].is_some());
    let mut start = vec![0u32; vertex_count + 1];
    for h in live() {
        start[ends(h).0 as usize + 1] += 1;
    }
    for v in 0..vertex_count {
        start[v + 1] += start[v];
    }
    let mut cursor = start.clone();
    // (upper endpoint, half-edge); filled in half-edge order, so each
    // bucket's half-edges ascend
    let mut bucket = vec![(0u32, 0u32); start[vertex_count] as usize];
    for h in live() {
        let (lo, hi) = ends(h);
        bucket[cursor[lo as usize] as usize] = (hi, h as u32);
        cursor[lo as usize] += 1;
    }
    // the first half-edge of each edge: 1 for a smooth edge, 2 for a
    // feature
    let mut first = vec![0u8; indices.len()];
    for v in 0..vertex_count {
        let b = &mut bucket[start[v] as usize..start[v + 1] as usize];
        b.sort_unstable();
        for group in b.chunk_by(|x, y| x.0 == y.0) {
            let feature = match group {
                [(_, h0), (_, h1)] => {
                    let normal = |h: u32| normals[h as usize / 3].expect("live half-edge");
                    dot(normal(*h0), normal(*h1)) < cos_crease
                }
                _ => true,
            };
            first[group[0].1 as usize] = 1 + u8::from(feature);
        }
    }
    first
        .iter()
        .enumerate()
        .filter(|&(_, &f)| f != 0)
        .map(|(h, &f)| EdgeRecord {
            corners: [indices[h], indices[h - h % 3 + (h + 1) % 3]],
            feature: f == 2,
        })
        .collect()
}

/// One welded vertex id per position, and the number of ids. The weld
/// tolerance is the larger of [`WELD_REL`] x the bbox diagonal and
/// [`WELD_ULPS`] x the largest coordinate magnitude.
///
/// Positions bucket into a grid of [`CELL_TOLS`]-wide cells, sorted by cell
/// (three stable counting-sort passes, no hashing) and swept in that
/// order: a position within the tolerance of a representative already
/// placed, in its own cell or a neighbouring one, takes its id; otherwise
/// it becomes a new representative. Of any two positions within the
/// tolerance, the later one in the sweep probes the earlier one's cell, so
/// every such pair welds unless an earlier representative claimed one of
/// them first.
fn weld(positions: &[[f32; 3]]) -> (Vec<u32>, usize) {
    if positions.is_empty() {
        return (Vec::new(), 0);
    }
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let span = |k: usize| f64::from(hi[k]) - f64::from(lo[k]);
    let diagonal = (0..3).map(|k| span(k).powi(2)).sum::<f64>().sqrt();
    let reach = (0..3)
        .map(|k| f64::from(lo[k].abs().max(hi[k].abs())))
        .fold(0.0, f64::max);
    let tol = (diagonal * WELD_REL).max(reach * WELD_ULPS);
    // every point at the origin: a zero tolerance welds exact matches
    // only, and any cell size works for that
    let cell_size = if tol > 0.0 { tol * CELL_TOLS } else { 1.0 };
    // cell coordinate along axis k (-1 or past the last cell for a probe
    // one tolerance outside the box)
    let per_cell = cell_size.recip();
    let cell_at = |c: f64, k: usize| ((c - f64::from(lo[k])) * per_cell).floor() as i64;
    // span / cell_size <= 1 / (WELD_REL x CELL_TOLS): 25k cells a side at
    // most
    let dims: [i64; 3] = std::array::from_fn(|k| cell_at(f64::from(hi[k]), k).max(0) + 1);
    let cells: Vec<[u32; 3]> = positions
        .iter()
        .map(|p| std::array::from_fn(|k| cell_at(f64::from(p[k]), k).clamp(0, dims[k] - 1) as u32))
        .collect();
    // positions ordered by cell (x, then y, then z), input order within a
    // cell: least significant axis first, each pass stable
    let mut order: Vec<u32> = (0..positions.len() as u32).collect();
    let mut sorted = vec![0u32; positions.len()];
    for k in (0..3).rev() {
        let mut start = vec![0usize; dims[k] as usize + 1];
        for &i in &order {
            start[cells[i as usize][k] as usize + 1] += 1;
        }
        for c in 0..dims[k] as usize {
            start[c + 1] += start[c];
        }
        for &i in &order {
            let slot = &mut start[cells[i as usize][k] as usize];
            sorted[*slot] = i;
            *slot += 1;
        }
        std::mem::swap(&mut order, &mut sorted);
    }
    let tol2 = tol * tol;
    let mut ids = vec![0u32; positions.len()];
    let mut reps: Vec<[f32; 3]> = Vec::new();
    // one run per occupied cell, in sweep order: (cell, first rep); a
    // cell's reps run from its first rep to the next run's
    let mut runs: Vec<([u32; 3], u32)> = Vec::new();
    let within = |reps: &[[f32; 3]], from: u32, to: usize, p: [f32; 3]| {
        (from as usize..to)
            .find(|&r| dist2(reps[r], p) <= tol2)
            .map(|r| r as u32)
    };
    for &i in &order {
        let (cell, p) = (cells[i as usize], positions[i as usize]);
        if runs.last().is_none_or(|run| run.0 != cell) {
            runs.push((cell, reps.len() as u32));
        }
        let own = runs.len() - 1;
        // the earlier cells the tolerance ball around `p` reaches
        let neighbour = |runs: &[([u32; 3], u32)], reps: &[[f32; 3]]| {
            let axis = |k: usize| {
                let c = f64::from(p[k]);
                cell_at(c - tol, k).max(0)..=cell_at(c + tol, k).min(dims[k] - 1)
            };
            let (rx, ry, rz) = (axis(0), axis(1), axis(2));
            rx.flat_map(|x| {
                let rz = rz.clone();
                ry.clone()
                    .flat_map(move |y| rz.clone().map(move |z| [x as u32, y as u32, z as u32]))
            })
            .filter(|&c| c != cell)
            .find_map(|c| {
                let r = runs[..own].binary_search_by_key(&c, |run| run.0).ok()?;
                within(reps, runs[r].1, runs[r + 1].1 as usize, p)
            })
        };
        ids[i as usize] = within(&reps, runs[own].1, reps.len(), p)
            .or_else(|| neighbour(&runs, &reps))
            .unwrap_or_else(|| {
                reps.push(p);
                reps.len() as u32 - 1
            });
    }
    (ids, reps.len())
}

/// The unit normal of triangle (a, b, c) by its winding, or `None` for a
/// zero-area triangle. Computed in f64 so a thin sliver keeps a usable
/// direction; the scene's shading normal ([`super::parts`]) uses it too.
pub(super) fn unit_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<[f32; 3]> {
    let (a, b, c) = (a.map(f64::from), b.map(f64::from), c.map(f64::from));
    let (u, v) = (sub(b, a), sub(c, a));
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    (len > 0.0).then(|| n.map(|x| (x / len) as f32))
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f64 {
    (0..3).map(|k| f64::from(a[k]) * f64::from(b[k])).sum()
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f64 {
    (0..3)
        .map(|k| (f64::from(a[k]) - f64::from(b[k])).powi(2))
        .sum()
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

    /// Square (0..1)^2 split on its diagonal, the second triangle's copy of
    /// the diagonal's corners moved `offset` along x: within the weld
    /// tolerance the diagonal is shared (4 edges), past it each triangle
    /// keeps its own diagonal as a boundary (6). The 100-unit far corner
    /// sets the tolerance to 1e-5 x 100 = 1e-3.
    fn split_square(offset: f32) -> usize {
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [offset, 0.0, 0.0],
            [1.0 + offset, 1.0, 0.0],
            [100.0, 0.0, 0.0],
        ];
        feature_edges(&positions, &[0, 1, 2, 4, 5, 3], CREASE_DEG).len()
    }

    #[test]
    fn corners_just_past_the_weld_tolerance_stay_distinct() {
        assert_eq!(split_square(0.0), 4, "exact copies weld");
        assert_eq!(
            split_square(0.5e-3),
            4,
            "half a tolerance apart: one vertex"
        );
        assert_eq!(
            split_square(1.5e-3),
            6,
            "1.5 tolerances apart: two vertices"
        );
        let (w, _) = weld(&[
            [0.0; 3],
            [100.0, 0.0, 0.0],
            [50.0, 0.0, 0.0],
            [50.0015, 0.0, 0.0],
        ]);
        assert_ne!(w[2], w[3], "a real feature just past tol is not merged");
    }

    #[test]
    fn weld_absorbs_float_noise_far_from_the_origin() {
        // a unit cube 10 km out: one f32 ulp there (~1e-3) is a hundred
        // times the diagonal's share (1.7e-5), so only the magnitude term
        // welds the per-face copies
        let corners: Vec<[f32; 3]> = cube_corners()
            .iter()
            .map(|c| c.map(|v| v + 10_000.0))
            .collect();
        let positions: Vec<[f32; 3]> = cube_indices()
            .iter()
            .enumerate()
            .map(|(k, &i)| {
                corners[i as usize].map(|v| {
                    let bits = v.to_bits();
                    f32::from_bits(if k % 2 == 0 { bits + 1 } else { bits - 1 })
                })
            })
            .collect();
        let edges = feature_edges(&positions, &soup(&positions), CREASE_DEG);
        assert_eq!(edges.len(), 12);
    }

    /// A closed UV sphere: `lon` x `lat` facets, quads split in two, the
    /// pole rows as fans onto the pole points. Outward winding.
    fn uv_sphere(lon: u32, lat: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
        use std::f32::consts::{PI, TAU};
        let mut positions = vec![[0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
        for i in 1..lat {
            let t = PI * i as f32 / lat as f32;
            for j in 0..lon {
                let f = TAU * j as f32 / lon as f32;
                positions.push([t.sin() * f.cos(), t.sin() * f.sin(), t.cos()]);
            }
        }
        let ring = |i: u32, j: u32| 2 + (i - 1) * lon + j % lon;
        let mut indices = Vec::new();
        for j in 0..lon {
            indices.extend([0, ring(1, j), ring(1, j + 1)]);
            indices.extend([1, ring(lat - 1, j + 1), ring(lat - 1, j)]);
            for i in 1..lat - 1 {
                let (a, b, c, d) = (
                    ring(i, j),
                    ring(i + 1, j),
                    ring(i + 1, j + 1),
                    ring(i, j + 1),
                );
                indices.extend([a, b, c, a, c, d]);
            }
        }
        (positions, indices)
    }

    #[test]
    fn smooth_closed_body_wires_every_triangle_edge() {
        let (positions, indices) = uv_sphere(24, 12);
        assert!(
            feature_edges(&positions, &indices, CREASE_DEG).is_empty(),
            "fixture truth: a 15° sphere has no boundary and no crease"
        );
        let triangles = indices.len() / 3;
        let edges = wire_edges(&positions, &indices);
        assert_eq!(
            edges.len(),
            triangles * 3 / 2,
            "a closed mesh: 3/2 edges per triangle, each once"
        );
    }

    #[test]
    fn wire_edges_are_the_feature_edges_when_there_are_any() {
        let (positions, indices) = (cube_corners(), cube_indices());
        assert_eq!(
            wire_edges(&positions, &indices),
            feature_edges(&positions, &indices, CREASE_DEG)
        );
        let degenerate = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        assert!(
            wire_edges(&degenerate, &[0, 1, 2]).is_empty(),
            "nothing to fall back to"
        );
    }

    /// The edge pass at a million triangles, as the scene build runs it
    /// (an unindexed soup, 3 corners per triangle): a 708 x 708 grid
    /// heightfield of gentle waves. Run on the build host in release:
    /// `cargo test -p viewer --release --lib -- --ignored edge_pass_timing --nocapture`
    #[test]
    #[ignore]
    fn edge_pass_timing_at_a_million_triangles() {
        let n = 708u32;
        let height = |x: f32, y: f32| 0.05 * (6.0 * x).sin() * (6.0 * y).cos();
        let grid = |i: u32, j: u32| {
            let (x, y) = (i as f32 / n as f32, j as f32 / n as f32);
            [x, y, height(x, y)]
        };
        let mut positions = Vec::with_capacity((6 * n * n) as usize);
        for i in 0..n {
            for j in 0..n {
                let (a, b, c, d) = (
                    grid(i, j),
                    grid(i + 1, j),
                    grid(i + 1, j + 1),
                    grid(i, j + 1),
                );
                positions.extend([a, b, c, a, c, d]);
            }
        }
        let triangles = positions.len() / 3;
        let corners = soup(&positions);
        let start = std::time::Instant::now();
        let (_, vertices) = weld(&positions);
        println!(
            "weld: {vertices} vertices, {:.1} ms",
            start.elapsed().as_secs_f64() * 1e3
        );
        let start = std::time::Instant::now();
        let edges = wire_edges(&positions, &corners);
        let elapsed = start.elapsed();
        println!(
            "edge pass: {triangles} triangles, {} edges, {:.1} ms",
            edges.len(),
            elapsed.as_secs_f64() * 1e3
        );
        assert_eq!(edges.len(), 4 * n as usize, "the grid's boundary only");
        assert!(
            elapsed.as_millis() < 300,
            "edge pass over budget: {elapsed:?}"
        );
    }
}
