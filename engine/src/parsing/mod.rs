//! Mesh/part parsing shared by STL and 3MF (contract §3).

pub mod animation;
pub mod stl;
pub mod threemf;

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
    pub fn triangle_count(&self) -> usize {
        self.triangle_colors.len()
    }

    /// The part's MODEL-space bounds; `None` when it has no positions.
    pub fn aabb(&self) -> Option<Box3> {
        Box3::from_points(self.positions.iter().copied())
    }
}

/// An axis-aligned box, `min <= max` on every axis. There is no empty box:
/// a fold over nothing is `None` ([`Box3::from_points`],
/// [`Box3::union_all`]), and each caller states what an empty input means
/// at that `Option` — most take [`Box3::ZERO`], a finite box the camera
/// math can frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box3 {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Box3 {
    /// The finite stand-in for "nothing to bound".
    pub const ZERO: Box3 = Box3 {
        min: [0.0; 3],
        max: [0.0; 3],
    };

    pub fn new(min: [f32; 3], max: [f32; 3]) -> Box3 {
        Box3 { min, max }
    }

    /// The tightest box around `points`; `None` for no points.
    pub fn from_points(points: impl IntoIterator<Item = [f32; 3]>) -> Option<Box3> {
        points
            .into_iter()
            .map(|p| Box3 { min: p, max: p })
            .reduce(Box3::union)
    }

    /// The tightest box around `boxes`; `None` for no boxes.
    pub fn union_all(boxes: impl IntoIterator<Item = Box3>) -> Option<Box3> {
        boxes.into_iter().reduce(Box3::union)
    }

    pub fn union(self, other: Box3) -> Box3 {
        Box3 {
            min: std::array::from_fn(|a| self.min[a].min(other.min[a])),
            max: std::array::from_fn(|a| self.max[a].max(other.max[a])),
        }
    }

    pub fn translated(self, by: [f32; 3]) -> Box3 {
        Box3 {
            min: std::array::from_fn(|a| self.min[a] + by[a]),
            max: std::array::from_fn(|a| self.max[a] + by[a]),
        }
    }

    pub fn size(&self) -> [f32; 3] {
        std::array::from_fn(|a| self.max[a] - self.min[a])
    }

    pub fn center(&self) -> [f32; 3] {
        std::array::from_fn(|a| (self.min[a] + self.max[a]) / 2.0)
    }

    /// Length of the min-to-max diagonal.
    pub fn diagonal(&self) -> f32 {
        let s = self.size();
        (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt()
    }

    pub fn volume(&self) -> f32 {
        let s = self.size();
        s[0] * s[1] * s[2]
    }

    /// `(min, max)`, the tuple form older call sites carry.
    pub fn pair(self) -> ([f32; 3], [f32; 3]) {
        (self.min, self.max)
    }
}

impl From<([f32; 3], [f32; 3])> for Box3 {
    fn from((min, max): ([f32; 3], [f32; 3])) -> Box3 {
        Box3 { min, max }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_over_nothing_are_none() {
        assert_eq!(Box3::from_points([]), None);
        assert_eq!(Box3::union_all([]), None);
        let empty = Part {
            name: "empty".into(),
            positions: vec![],
            triangle_colors: vec![],
        };
        assert_eq!(empty.aabb(), None);
    }

    #[test]
    fn from_points_is_the_tight_box() {
        let b = Box3::from_points([[1.0, -2.0, 3.0], [-4.0, 5.0, 0.5], [0.0, 0.0, 9.0]]).unwrap();
        assert_eq!(b, Box3::new([-4.0, -2.0, 0.5], [1.0, 5.0, 9.0]));
        // one point is a degenerate box, not an empty one
        let p = Box3::from_points([[7.0, 8.0, 9.0]]).unwrap();
        assert_eq!(p.size(), [0.0; 3]);
        assert_eq!(p.center(), [7.0, 8.0, 9.0]);
    }

    #[test]
    fn measures_and_moves() {
        let a = Box3::new([0.0, 0.0, 0.0], [2.0, 3.0, 6.0]);
        assert_eq!(a.size(), [2.0, 3.0, 6.0]);
        assert_eq!(a.center(), [1.0, 1.5, 3.0]);
        assert_eq!(a.diagonal(), 7.0);
        assert_eq!(a.volume(), 36.0);
        assert_eq!(
            a.translated([1.0, -1.0, 2.0]),
            Box3::new([1.0, -1.0, 2.0], [3.0, 2.0, 8.0])
        );
        let b = Box3::new([-1.0, 1.0, 1.0], [1.0, 5.0, 2.0]);
        assert_eq!(a.union(b), Box3::new([-1.0, 0.0, 0.0], [2.0, 5.0, 6.0]));
        assert_eq!(Box3::union_all([a, b]), Some(a.union(b)));
        assert_eq!(Box3::from(a.pair()), a);
    }
}
