//! Mesh/part parsing shared by STL and 3MF (contract §3).

pub mod stl;

/// Uncolored triangles render in the app's neutral color; engine only says
/// "no color", the viewer picks RGB (contract §3).
pub type TriangleColor = Option<[u8; 3]>;

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    /// Per-corner positions, unindexed: 3 entries per triangle.
    pub positions: Vec<[f32; 3]>,
    /// One entry per triangle.
    pub triangle_colors: Vec<TriangleColor>,
}

impl Part {
    pub fn triangle_count(&self) -> usize { self.triangle_colors.len() }
    pub fn aabb(&self) -> ([f32; 3], [f32; 3]) {
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        for p in &self.positions {
            for a in 0..3 { mn[a] = mn[a].min(p[a]); mx[a] = mx[a].max(p[a]); }
        }
        (mn, mx)
    }
}
