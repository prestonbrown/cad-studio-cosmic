//! Parity camera math, transcribed from ModelViewer.swift (facts §1).
//! Pure — no GPU types — so the formulas are testable headless. The pose
//! is one [`Orbit`] value; [`crate::camera_input::CameraInput`] owns the
//! live one and the adapter draws whatever it is handed.

use super::{Box3, PartScene};

/// Vertical FOV (ModelViewer.swift:144) with near and far planes
/// (:142-143) — absolute, never radius-relative.
pub const FOV: f32 = 35.0_f32.to_radians();
pub const NEAR: f32 = 1.0;
pub const FAR: f32 = 100_000.0;
/// Upstream's exact pitch clamp (ModelViewer.swift:95), π/2 minus
/// ~0.01 — not a value this crate invents.
pub const MAX_PITCH: f32 = 1.5607;
/// Drag-to-orbit sensitivity, radians per point (ModelViewer.swift:302-303).
pub const ORBIT_SENSITIVITY: f32 = 0.008;

/// Upstream's named views (ModelViewer.swift:4-33).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Iso,
    Front,
    Top,
    Right,
}

impl Preset {
    pub fn yaw_pitch(self) -> (f32, f32) {
        match self {
            Preset::Iso => (std::f32::consts::FRAC_PI_4, 0.6),
            Preset::Front => (0.0, 0.0),
            // top looks straight down at the same maximumPitch constant
            Preset::Top => (0.0, MAX_PITCH),
            Preset::Right => (std::f32::consts::FRAC_PI_2, 0.0),
        }
    }
}

/// placeCamera's eye offset (ModelViewer.swift:339-342): the unit
/// direction from the look-at target toward an orbit camera at
/// `(yaw, pitch)` — yaw about scene +Y from +Z toward +X, pitch up
/// from the floor plane. The one definition every pose derivation
/// ([`Orbit`]'s view, pan, and pick ray; `lights::to_eye`) builds on.
pub fn eye_dir(yaw: f32, pitch: f32) -> glam::Vec3 {
    glam::Vec3::new(
        pitch.cos() * yaw.sin(),
        pitch.sin(),
        pitch.cos() * yaw.cos(),
    )
}

/// Framing radius: half the aabb DIAGONAL, floored at 1
/// (ModelViewer.swift:330) — a long-thin part frames by its diagonal,
/// not its longest extent.
pub fn framing_radius(aabb: Box3) -> f32 {
    (aabb.diagonal() / 2.0).max(1.0)
}

/// frame() (ModelViewer.swift:328-337), aspect-free by construction:
/// the target is half the scene height above the pinned floor, the
/// distance fits the radius through the FOV with 15% headroom. The
/// x/z zeros assume the PartScene's floor pin already centered the
/// model horizontally.
pub fn frame(aabb: Box3) -> ([f32; 3], f32) {
    let target = [0.0, aabb.size()[1] / 2.0, 0.0];
    let distance = framing_radius(aabb) / (FOV / 2.0).sin() * 1.15;
    (target, distance)
}

/// pan's units-per-point (ModelViewer.swift:312-320): the view-plane
/// height covered by one point at `distance`.
pub fn pan_units_per_point(distance: f32, view_height: f32) -> f32 {
    2.0 * distance * (FOV / 2.0).tan() / view_height
}

/// An orbit camera pose: `yaw` about scene +Y from +Z toward +X, `pitch`
/// up from the floor plane (kept within ±[`MAX_PITCH`]), `distance` from
/// the look-at `target`. The one pose value the adapter, the input state
/// machine, and the pick ray share — the formulas below read it by name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
}

impl Orbit {
    /// `preset`'s angles at [`frame`]'s target and distance for `aabb`
    /// (upstream's preset re-frame, ModelViewer.swift:322-337).
    pub fn framed(aabb: Box3, preset: Preset) -> Orbit {
        let (target, distance) = frame(aabb);
        let (yaw, pitch) = preset.yaw_pitch();
        Orbit {
            yaw,
            pitch,
            distance,
            target,
        }
    }

    /// The eye position: `target + eye_dir(yaw, pitch)·distance`.
    pub fn eye(&self) -> glam::Vec3 {
        glam::Vec3::from_array(self.target) + eye_dir(self.yaw, self.pitch) * self.distance
    }

    /// placeCamera (ModelViewer.swift:339-342): look at `target` from
    /// [`Orbit::eye`] with Y up, projected at `aspect` — the viewport's
    /// width/height, required, so the projection always matches the window
    /// it renders into.
    pub fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        let view = glam::camera::rh::view::look_at_mat4(
            self.eye(),
            glam::Vec3::from_array(self.target),
            glam::Vec3::Y,
        );
        // directx = WebGPU NDC (Z in [0,1], Y-up)
        let proj = glam::camera::rh::proj::directx::perspective(FOV, aspect, NEAR, FAR);
        (proj * view).to_cols_array_2d()
    }

    /// [`Orbit::view_proj`] as the frame's camera.
    pub fn camera(&self, aspect: f32) -> Camera {
        Camera {
            view_proj: self.view_proj(aspect),
        }
    }

    /// The camera's right and up axes in scene space — the look-at basis
    /// [`Orbit::view_proj`] builds (right degenerates only at ±90° pitch,
    /// which [`MAX_PITCH`] keeps unreachable).
    fn basis(&self) -> (glam::Vec3, glam::Vec3) {
        let back = eye_dir(self.yaw, self.pitch);
        let right = glam::Vec3::Y.cross(back).normalize_or_zero();
        (right, back.cross(right))
    }

    /// orbit(by:) (ModelViewer.swift:301-305): drag deltas in points to
    /// yaw/pitch — yaw decreases with dx, pitch increases with dy, both
    /// scaled [`ORBIT_SENSITIVITY`], pitch clamped to ±[`MAX_PITCH`].
    /// `dx`/`dy` are AppKit-convention deltas (dy bottom-up).
    pub fn dragged(self, dx: f32, dy: f32) -> Orbit {
        Orbit {
            yaw: self.yaw - dx * ORBIT_SENSITIVITY,
            pitch: (self.pitch + dy * ORBIT_SENSITIVITY).clamp(-MAX_PITCH, MAX_PITCH),
            ..self
        }
    }

    /// zoom (ModelViewer.swift:307-310): distance × `factor` clamped to
    /// [0.1·radius, 200·radius].
    pub fn zoomed(self, factor: f32, radius: f32) -> Orbit {
        Orbit {
            distance: (self.distance * factor).clamp(radius * 0.1, radius * 200.0),
            ..self
        }
    }

    /// pan(by:viewHeight:) (ModelViewer.swift:312-320): the target shifted
    /// by `(-right·dx + up·dy)·units_per_point` over the camera's own
    /// axes. `dx`/`dy` are AppKit-convention deltas (dy bottom-up); a
    /// non-positive view height leaves the pose alone, as upstream's
    /// guard does.
    pub fn panned(self, view_height: f32, dx: f32, dy: f32) -> Orbit {
        if view_height <= 0.0 {
            return self;
        }
        let (right, up) = self.basis();
        let t = glam::Vec3::from_array(self.target)
            + (right * -dx + up * dy) * pan_units_per_point(self.distance, view_height);
        Orbit {
            target: t.to_array(),
            ..self
        }
    }

    /// pick(at:in:on:)'s ray half (ModelViewer.swift:291-299, facts #29):
    /// the scene-space (origin, direction) a click at `cursor` casts for a
    /// `view`-sized viewport. Upstream unprojects through RealityKit, which
    /// in pure math is `normalize(x·r + y·u + f)` over the look-at basis.
    /// `cursor` is iced's top-down point and crosses to AppKit's bottom-up
    /// `h − cy` BEFORE the NDC mapping (facts #29) or every vertical click
    /// aims mirrored; `x` scales by the view's aspect and both by the
    /// half-FOV tangent, exactly upstream's unprojection terms.
    pub fn pick_ray(&self, cursor: (f32, f32), view: (f32, f32)) -> ([f32; 3], [f32; 3]) {
        let (right, up) = self.basis();
        let forward = -eye_dir(self.yaw, self.pitch);
        let tangent = (FOV / 2.0).tan();
        let (w, h) = view;
        let x = (2.0 * cursor.0 / w - 1.0) * tangent * (w / h);
        // iced's cursor is top-down; AppKit's point is bottom-up (facts #29)
        let y = (2.0 * (h - cursor.1) / h - 1.0) * tangent;
        let dir = (right * x + up * y + forward).normalize_or_zero();
        (self.eye().to_array(), dir.to_array())
    }
}

/// The frame's view-projection (column-major, glam `to_cols_array_2d`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub view_proj: [[f32; 4]; 4],
}

/// The camera for `preset` framing `scene` (upstream's preset-change
/// reframe, ModelViewer.swift:322-337).
pub fn preset_camera(scene: &PartScene, preset: Preset, aspect: f32) -> Camera {
    Orbit::framed(scene.aabb(), preset).camera(aspect)
}

/// Default iso camera framing `scene` (upstream presets: yaw π/4, pitch 0.6).
pub fn default_camera(scene: &PartScene, aspect: f32) -> Camera {
    preset_camera(scene, Preset::Iso, aspect)
}

#[cfg(test)]
mod tests {
    use crate::render::test_support::golden_3mf_parts;
    use crate::render::{Box3, NEUTRAL, PartScene};
    use crate::render::{camera, lights};
    use camera::Orbit;

    fn pose(yaw: f32, pitch: f32, distance: f32, target: [f32; 3]) -> Orbit {
        Orbit {
            yaw,
            pitch,
            distance,
            target,
        }
    }

    #[test]
    fn frame_radius_is_diagonal_half() {
        // the long-thin regression: max-extent/2 would frame at 5.0, the
        // diagonal at √102/2 ≈ 5.05 (facts #1: ModelViewer.swift:328-337)
        let (min, max) = ([0.0, 0.0, 0.0], [10.0, 1.0, 1.0]);
        let r = f32::sqrt(102.0) / 2.0;
        assert!((camera::framing_radius(Box3::new(min, max)) - r).abs() < 1e-5);
        let (target, distance) = camera::frame(Box3::new(min, max));
        // aspect-free: half the height; x/z zero assumes the pinned scene
        assert_eq!(target, [0.0, 0.5, 0.0]);
        let expected = r / f32::sin(17.5f32.to_radians()) * 1.15;
        assert!(
            (distance - expected).abs() < 1e-3,
            "distance {distance} vs r/sin(17.5°)·1.15 = {expected}"
        );
        // the radius floors at 1 so a degenerate model still frames
        assert_eq!(camera::framing_radius(Box3::new([0.0; 3], [0.0; 3])), 1.0);
    }

    #[test]
    fn zoom_clamps_and_presets() {
        // zoom: distance × factor clamped to [0.1r, 200r] (facts #19)
        let at50 = pose(0.0, 0.0, 50.0, [0.0; 3]);
        assert_eq!(at50.zoomed(0.5, 10.0).distance, 25.0);
        assert_eq!(at50.zoomed(0.001, 10.0).distance, 1.0, "floor 0.1r");
        assert_eq!(at50.zoomed(100.0, 10.0).distance, 2000.0, "ceiling 200r");
        // pitch clamp is upstream's exact constant, not π/2 − ε (facts #18)
        assert_eq!(camera::MAX_PITCH, 1.5607);
        assert!(camera::MAX_PITCH < std::f32::consts::FRAC_PI_2);
        // drag: yaw -= dx·0.008, pitch += dy·0.008, clamped at ±MAX_PITCH
        let dragged = pose(0.3, 0.2, 50.0, [1.0, 2.0, 3.0]).dragged(100.0, 25.0);
        assert!((dragged.yaw + 0.5).abs() < 1e-6, "yaw {}", dragged.yaw);
        assert!(
            (dragged.pitch - 0.4).abs() < 1e-6,
            "pitch {}",
            dragged.pitch
        );
        assert_eq!((dragged.distance, dragged.target), (50.0, [1.0, 2.0, 3.0]));
        assert_eq!(
            pose(0.0, 1.5, 1.0, [0.0; 3]).dragged(0.0, 1000.0).pitch,
            1.5607
        );
        assert_eq!(
            pose(0.0, -1.5, 1.0, [0.0; 3]).dragged(0.0, -1000.0).pitch,
            -1.5607
        );
        // presets (facts #25): top reuses the maximumPitch constant
        assert_eq!(
            camera::Preset::Iso.yaw_pitch(),
            (std::f32::consts::FRAC_PI_4, 0.6)
        );
        assert_eq!(camera::Preset::Front.yaw_pitch(), (0.0, 0.0));
        assert_eq!(camera::Preset::Top.yaw_pitch(), (0.0, camera::MAX_PITCH));
        assert_eq!(
            camera::Preset::Right.yaw_pitch(),
            (std::f32::consts::FRAC_PI_2, 0.0)
        );
        // pan: world units per view point at `distance` (facts #22)
        let upp = camera::pan_units_per_point(100.0, 500.0);
        let expected = 2.0 * 100.0 * f32::tan(17.5f32.to_radians()) / 500.0;
        assert!((upp - expected).abs() < 1e-6, "{upp} vs {expected}");
    }

    /// pan (facts #22): the target moves by (-right·dx + up·dy)·units/pt
    /// over the camera's own scene axes — verified against the look-at
    /// basis glam builds for the same pose (rows 0/1 of the view matrix's
    /// rotation block are the camera's right/up in world space). Dragging
    /// right (dx > 0) moves the target LEFT, so the model follows the
    /// cursor; a non-positive view height leaves the target alone, as the
    /// upstream guard does.
    #[test]
    fn pan_matches_lookat_basis() {
        let target = [3.0, -4.0, 5.0];
        let (yaw, pitch, distance) = (0.7f32, 0.3f32, 120.0f32);
        let (dx, dy, view_height) = (25.0f32, -10.0f32, 500.0f32);
        let panned = pose(yaw, pitch, distance, target)
            .panned(view_height, dx, dy)
            .target;
        let at = glam::Vec3::from_array(target);
        let back = camera::eye_dir(yaw, pitch);
        let view = glam::camera::rh::view::look_at_mat4(at + back * distance, at, glam::Vec3::Y);
        // col-major glam: matrix rows 0/1 are the camera right/up axes
        let right = glam::Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x);
        let up = glam::Vec3::new(view.x_axis.y, view.y_axis.y, view.z_axis.y);
        let upp = camera::pan_units_per_point(distance, view_height);
        let expected = at + (right * -dx + up * dy) * upp;
        for a in 0..3 {
            assert!(
                (panned[a] - expected[a]).abs() < 1e-4,
                "panned {panned:?} vs look-at basis {expected:?}"
            );
        }
        // the model follows the cursor: dx > 0 shifts the target against
        // the camera's right axis
        assert!(glam::Vec3::from_array(panned).dot(right) < at.dot(right));
        assert_eq!(
            pose(yaw, pitch, distance, target)
                .panned(0.0, dx, dy)
                .target,
            target,
            "zero view height must not pan"
        );
    }

    /// placeCamera's eye offset on its defining poses: yaw 0 looks from
    /// +Z, yaw π/2 from +X, pitch π/2 from straight above; and every pose
    /// is a unit vector, so `distance` scales it to the true eye distance.
    #[test]
    fn eye_dir_is_place_cameras_offset() {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
        let near = |a: glam::Vec3, b: [f32; 3]| (a - glam::Vec3::from_array(b)).length() < 1e-6;
        assert!(near(camera::eye_dir(0.0, 0.0), [0.0, 0.0, 1.0]));
        assert!(near(camera::eye_dir(FRAC_PI_2, 0.0), [1.0, 0.0, 0.0]));
        assert!(near(camera::eye_dir(0.0, FRAC_PI_2), [0.0, 1.0, 0.0]));
        let (c, s) = (0.6f32.cos(), 0.6f32.sin());
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(near(camera::eye_dir(FRAC_PI_4, 0.6), [c * h, s, c * h]));
        for &(yaw, pitch) in &[(0.7f32, 0.3f32), (-2.5, camera::MAX_PITCH), (3.0, -1.2)] {
            assert!((camera::eye_dir(yaw, pitch).length() - 1.0).abs() < 1e-6);
        }
        assert_eq!(
            lights::to_eye(0.7, 0.3),
            camera::eye_dir(0.7, 0.3).to_array(),
            "the shading's view axis is the camera's eye offset"
        );
    }

    /// The ported orbit test: the view_proj is exactly the Swift
    /// placeCamera formula (facts #1: ModelViewer.swift:339-342), the
    /// orbit target sits at clip-space (0, 0) for every pose, the aspect
    /// argument is load-bearing, and extreme poses stay finite.
    #[test]
    fn orbit_view_proj_matches_formula() {
        // arbitrary off-center target: the formula must not assume pinning
        let target = [3.0, -4.0, 5.0];
        let aspect = 800.0 / 600.0;
        for &(yaw, pitch, distance) in &[
            (0.7f32, 0.3f32, 120.0f32),
            (0.0, 0.0, 1.0),
            (-2.5, camera::MAX_PITCH, 90_000.0),
            (std::f32::consts::PI, -camera::MAX_PITCH, 250.0),
        ] {
            let vp = pose(yaw, pitch, distance, target).view_proj(aspect);
            let dir = camera::eye_dir(yaw, pitch);
            let at = glam::Vec3::from_array(target);
            let view = glam::camera::rh::view::look_at_mat4(at + dir * distance, at, glam::Vec3::Y);
            let proj = glam::camera::rh::proj::directx::perspective(
                35f32.to_radians(),
                aspect,
                1.0,
                100_000.0,
            );
            let expected = (proj * view).to_cols_array_2d();
            for (row, erow) in vp.iter().zip(expected.iter()) {
                for (a, b) in row.iter().zip(erow.iter()) {
                    assert!((a - b).abs() < 1e-4, "{vp:?} vs {expected:?}");
                }
            }
            // the target is the look-at point: it projects to (0, 0)
            let clip = glam::Mat4::from_cols_array_2d(&vp)
                * glam::Vec4::new(target[0], target[1], target[2], 1.0);
            assert!(clip.x.abs() < 1e-3 && clip.y.abs() < 1e-3, "{clip:?}");
            for row in vp {
                for x in row {
                    assert!(x.is_finite());
                }
            }
            // aspect is load-bearing: a 16:9 projection differs from 4:3 —
            // the hardcode this task kills would render both identically
            assert_ne!(vp, pose(yaw, pitch, distance, target).view_proj(16.0 / 9.0));
        }
    }

    /// The pick ray aims where the clicked pixel looks: center exactly along
    /// the view axis at the target, the screen edges tilting with the
    /// camera's own right/up axes. The y-flip (facts #29) is the load-bearing
    /// convention here — iced's cursor is top-down, upstream's AppKit point
    /// is bottom-up, so `h − cy` must happen BEFORE the NDC mapping or every
    /// vertical drag picks mirrored.
    #[test]
    fn pick_ray_center_points_at_the_target() {
        // yaw 0, pitch 0: eye = target + dist*(0,0,1); center pixel aims at f
        let front = pose(0.0, 0.0, 100.0, [0.0, 5.0, 0.0]);
        let (o, d) = front.pick_ray((400.0, 300.0), (800.0, 600.0));
        assert!(
            (o[0] - 0.0).abs() < 1e-4 && (o[1] - 5.0).abs() < 1e-4 && (o[2] - 100.0).abs() < 1e-4
        );
        let dot = glam::Vec3::from_array(d).dot(glam::Vec3::new(0.0, 0.0, -1.0));
        assert!(dot > 0.999, "center ray along the view axis: {d:?}");
        // right edge (x=799) tilts toward +x; BOTTOM-left corner tilts -x -y
        // (iced y is top-down: cursor y 599 flips to AppKit y 1 = bottom,
        // which aims DOWN in any bottom-up convention)
        let (_, d2) = front.pick_ray((799.0, 300.0), (800.0, 600.0));
        assert!(d2[0] > 0.1, "right edge aims right: {d2:?}");
        let (_, d3) = front.pick_ray((0.0, 599.0), (800.0, 600.0));
        assert!(
            d3[0] < 0.0 && d3[1] < 0.0,
            "bottom-left aims left and DOWN (iced bottom = AppKit bottom after the flip): {d3:?}"
        );
    }

    #[test]
    fn framed_iso_model_fills_the_pane() {
        // The pane-fill contract of frame() (ModelViewer.swift:328-337):
        // the framed distance puts the scene's bounding sphere at
        // sin(half-FOV)/1.15 of the camera — 86.6 % of the half-view — and
        // the golden fixture's box (scene 60×26×60, radius √7876/2) fills
        // ~two thirds of the viewport height at the iso preset, at ANY
        // aspect (the projection's y extent is aspect-free). If the 1.15
        // headroom, the half-diagonal radius, or the target rule changes,
        // these ratios move and the fill visibly changes with them.
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        let iso = Orbit::framed(scene.aabb(), camera::Preset::Iso);
        let distance = iso.distance;
        let radius = camera::framing_radius(scene.aabb());
        let half_fov = camera::FOV / 2.0;
        let angular_fill = (radius / distance).asin() / half_fov;
        assert!(
            (angular_fill - 0.866).abs() < 0.005,
            "sphere fills {angular_fill:.3} of the half-view, not 86.6 %"
        );
        let corners = |a: f32| {
            let view_proj = glam::Mat4::from_cols_array_2d(&iso.view_proj(a));
            let mut span = (f32::MAX, f32::MIN);
            for i in 0..8 {
                let p = [
                    [scene.aabb_min[0], scene.aabb_max[0]][i & 1],
                    [scene.aabb_min[1], scene.aabb_max[1]][i >> 1 & 1],
                    [scene.aabb_min[2], scene.aabb_max[2]][i >> 2 & 1],
                    1.0,
                ];
                let clip = view_proj * glam::Vec4::from_array(p);
                let ndc_y = clip.y / clip.w;
                span.0 = span.0.min(ndc_y);
                span.1 = span.1.max(ndc_y);
            }
            (span.1 - span.0) / 2.0 // fraction of the viewport height
        };
        let (square, wide) = (corners(1.0), corners(2.16));
        for (aspect, fill) in [(1.0, square), (2.16, wide)] {
            assert!(
                (0.60..=0.75).contains(&fill),
                "aspect {aspect}: the framed box fills {fill:.2} of the viewport height"
            );
        }
        assert!(
            (square - wide).abs() < 0.005,
            "the y fill is aspect-free: {square:.4} vs {wide:.4}"
        );
    }
}
