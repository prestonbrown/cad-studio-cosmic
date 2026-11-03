//! The measure tool's hit test: a CPU ray cast against the staged scene,
//! click-driven — never per frame (spec §5). [`first_hit`] is the pure
//! math half; the app owns the seam (spawn it off-thread on a click and
//! bring the answer back), which is why it takes plain arrays and an
//! `&PartScene`, no GPU types.
//!
//! Upstream picks against one merged, sectioned, arranged `TriangleMesh`
//! rebuilt on every layout/section change (ModelViewer.swift:291-299,
//! TriangleMesh.swift:84-106). This port keeps parts staged once and walks
//! every visible part's triangles in the part's own staged space — the
//! pose's inverse carries the ray in, Möller–Trumbore two-sided
//! (`|det| > 1e-9`) with the nearest `t > 0` winning; poses are rigid, so
//! the renormalized staged direction keeps `t` an exact world distance and
//! the comparison across parts is fair. The section keep-below test stands
//! in for upstream's CPU clip: a hit the plane would discard is SKIPPED,
//! not returned, so the nearest surviving hit wins — the same ray against
//! the clipped mesh, minus cap-surface hits upstream's clipped mesh also
//! lacks (facts #76). The winner maps back through the inverse pose and
//! [`PartScene::to_model`] to MODEL coords, what the measure points and
//! the badge deltas consume.

use crate::render::{PartPose, PartScene, SectionPlane};

/// Möller–Trumbore against one triangle: the ray's `t` at the intersection,
/// or `None` when it misses. Two-sided — `|det| <= 1e-9` rejects the
/// parallel ray and the degenerate triangle alike, front or back face
/// (facts #76); `u`/`v` outside the triangle reject; `t <= 0` (behind the
/// origin) rejects.
fn moller_trumbore(
    v0: [f32; 3],
    v1: [f32; 3],
    v2: [f32; 3],
    origin: glam::Vec3,
    dir: glam::Vec3,
) -> Option<f32> {
    let (v0, v1, v2) = (
        glam::Vec3::from_array(v0),
        glam::Vec3::from_array(v1),
        glam::Vec3::from_array(v2),
    );
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    if det.abs() <= 1e-9 {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - v0;
    let u = s.dot(h) * inv_det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = dir.dot(q) * inv_det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = edge2.dot(q) * inv_det;
    (t > 0.0).then_some(t)
}

/// The nearest ray/triangle hit across every part `poses` places visibly,
/// as a MODEL-space point — the measure tool's pick. `origin`/`dir` are
/// scene-space (the output of [`crate::render::camera::pick_ray`]);
/// `section` is the shader's discard plane as the camera uniform carries
/// it (a scene axis and keep-below position in the PINNED scene frame),
/// and a hit beyond it is skipped so the nearest SURVIVING hit wins.
pub fn first_hit(
    scene: &PartScene,
    poses: &[PartPose],
    section: Option<&SectionPlane>,
    origin: [f32; 3],
    dir: [f32; 3],
) -> Option<[f32; 3]> {
    assert_eq!(
        poses.len(),
        scene.parts.len(),
        "one pose per part: {} poses for {} parts",
        poses.len(),
        scene.parts.len()
    );
    let origin = glam::Vec3::from_array(origin);
    let dir = glam::Vec3::from_array(dir).normalize_or_zero();
    // (staged-space t, staged hit) of the best surviving candidate
    let mut best: Option<(f32, glam::Vec3)> = None;
    for (part, pose) in scene.parts.iter().zip(poses) {
        let Some(m) = pose else { continue };
        let world = glam::Mat4::from_cols_array_2d(m);
        let to_staged = world.inverse();
        let o = to_staged.transform_point3(origin);
        let d = to_staged.transform_vector3(dir).normalize_or_zero();
        for tri in part.vertices.chunks_exact(3) {
            let Some(t) = moller_trumbore(tri[0].pos, tri[1].pos, tri[2].pos, o, d) else {
                continue;
            };
            if best.is_some_and(|(best_t, _)| t >= best_t) {
                continue;
            }
            let staged = o + d * t;
            // the section test runs on the WORLD point — the shader
            // discards on the fragment's posed position
            let hit = world.transform_point3(staged);
            if let Some(plane) = section {
                if hit[plane.axis.index()] > plane.pos {
                    continue;
                }
            }
            best = Some((t, staged));
        }
    }
    best.map(|(_, staged)| scene.to_model(staged.to_array()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{NEUTRAL, PartScene};
    use glam::{Mat4, Vec3};

    fn ground_tri_scene() -> PartScene {
        // one MODEL triangle z=0 spanning (-10,-10)..(10,10), neutral color
        let part = engine::parsing::Part {
            name: "t".into(),
            positions: vec![[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [0.0, 10.0, 0.0]],
            triangle_colors: vec![None],
        };
        PartScene::from_parts(&[part], NEUTRAL)
    }

    #[test]
    fn ray_hits_reports_model_coords_round_trip() {
        let scene = ground_tri_scene();
        let poses = vec![Some(Mat4::IDENTITY.to_cols_array_2d())];
        // straight down the MODEL z axis at the centroid: the MODEL centroid
        // (0, -10/3, 0) stages at scene (0, 0, 10/3), so the scene ray -y
        // must pass through THAT point, not the scene origin
        let origin = [0.0, 50.0, 10.0f32 / 3.0];
        let hit = first_hit(&scene, &poses, None, origin, [0.0, -1.0, 0.0]).unwrap();
        // MODEL centroid of the triangle, z=0: scene_point maps back exactly
        let model = [0.0, -10.0f32 / 3.0, 0.0];
        for i in 0..3 {
            assert!((hit[i] - model[i]).abs() < 1e-4, "hit {hit:?} vs {model:?}");
        }
    }

    #[test]
    fn two_sided_and_nearest() {
        let scene = ground_tri_scene();
        let poses = vec![Some(Mat4::IDENTITY.to_cols_array_2d())];
        // two-sided: the same triangle picked from below (its back face)
        let below = first_hit(&scene, &poses, None, [0.0, -50.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(below.is_some(), "back face is pickable (|det| > 1e-9)");
        // nearest: two identical parts, one lifted 10mm through its pose —
        // a ray from above must hit the lifted (smaller t) one first
        let parts = vec![ground_tri_part(), ground_tri_part()];
        let scene2 = PartScene::from_parts(&parts, NEUTRAL);
        let poses2 = vec![
            Some(Mat4::from_translation(Vec3::new(0.0, 10.0, 0.0)).to_cols_array_2d()),
            Some(Mat4::IDENTITY.to_cols_array_2d()),
        ];
        let hit = first_hit(&scene2, &poses2, None, [0.0, 50.0, 0.0], [0.0, -1.0, 0.0]);
        // both staged at the same place; the lifted part's hit un-maps to the
        // SAME model point — assert the ray reports a hit whose t is the near
        // one by checking a ray that ONLY the far part can answer when the
        // near one is hidden:
        assert!(hit.is_some());
        let far_only = {
            let mut p = poses2.clone();
            p[0] = None; // hide the lifted part
            first_hit(&scene2, &p, None, [0.0, 50.0, 0.0], [0.0, -1.0, 0.0])
        };
        assert!(far_only.is_some(), "hidden filter + fallback hit both work");
        // nearest, observably: an oblique ray separates the two hits — the
        // lifted part's staged crossing lands at x = 0.1·40 (40mm down the
        // slope), the far part's at x = 0.1·50. A far-first bug returns 5.0.
        let oblique = Vec3::new(0.1, -1.0, 0.0).normalize().to_array();
        let won = first_hit(&scene2, &poses2, None, [0.0, 50.0, 10.0 / 3.0], oblique).unwrap();
        assert!((won[0] - 4.0).abs() < 1e-3, "nearest hit wins: {won:?}");
    }

    fn ground_tri_part() -> engine::parsing::Part {
        engine::parsing::Part {
            name: "t".into(),
            positions: vec![[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [0.0, 10.0, 0.0]],
            triangle_colors: vec![None],
        }
    }

    fn keep_below_y(pos: f32) -> SectionPlane {
        SectionPlane {
            axis: crate::render::SceneAxis::Y,
            pos,
            cap: engine::parsing::Box3::ZERO,
        }
    }

    #[test]
    fn section_filter_removes_discarded_hits() {
        let scene = ground_tri_scene();
        let poses = vec![Some(Mat4::IDENTITY.to_cols_array_2d())];
        // staged scene y in [0, epsilon]; section keep-below at y = -1 discards all
        assert!(
            first_hit(
                &scene,
                &poses,
                Some(&keep_below_y(-1.0)),
                [0.0, 50.0, 0.0],
                [0.0, -1.0, 0.0]
            )
            .is_none()
        );
        assert!(
            first_hit(
                &scene,
                &poses,
                Some(&keep_below_y(5.0)),
                [0.0, 50.0, 0.0],
                [0.0, -1.0, 0.0]
            )
            .is_some()
        );
    }

    #[test]
    fn hidden_parts_and_misses() {
        let scene = ground_tri_scene();
        assert!(first_hit(&scene, &[None], None, [0.0, 50.0, 0.0], [0.0, -1.0, 0.0]).is_none());
        assert!(
            first_hit(
                &scene,
                &[Some(Mat4::IDENTITY.to_cols_array_2d())],
                None,
                [0.0, 50.0, 0.0],
                [1.0, 0.0, 0.0]
            )
            .is_none(),
            "parallel ray misses"
        );
    }
}
