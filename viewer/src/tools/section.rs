//! Upstream's section tool, mapped onto the mesh shader's GPU discard
//! (facts §6, #84-87).
//!
//! ModelViewerState.swift:116-121 computes the plane in MODEL space —
//! `value = min[axis] + (max[axis] - min[axis]) * position` over the
//! arranged mesh's aabb — and clips CPU-side with `keepsBelow = axis != .y`
//! (TriangleMesh.swift:59-66: X and Z keep `v <= value`, MODEL Y keeps
//! `v >= value`). The mesh shader already implements keep-below on a scene
//! axis (mesh.wgsl's discard), so the whole upstream rule closes into the
//! MODEL→scene mapping [`scene_plane`] returns: X and Z keep below at the
//! plain value on their scene axes (scene x = model x, scene y = model z),
//! and MODEL-above is scene-below at the NEGATED value on scene z (scene
//! z = -model y under the `(x, z, -y)` remap) — no shader flag exists or is
//! needed, and the boundary lands as upstream's does (`v > pos` discards
//! strictly, so the plane itself is kept).
//!
//! The discard tests the fragment's world position alone — never winding,
//! face orientation, or the stencil parity the cap quad uses — so a
//! flipped-STL part sections identically (carry-forward M6 line 1).
//!
//! The returned plane is stated in RAW-remap scene coordinates. Staged
//! vertices carry the scene's floor pin, so the caller feeding the Camera
//! uniform must translate the plane by the pin along its scene axis — a
//! point on the plane mapped through `PartScene::to_scene` carries exactly
//! that offset; the headless tests pin this against an identity-pin scene.

/// The slider's upstream range (ModelPane.swift:191): 0.001..0.999, never
/// touching either end, so the cut always leaves geometry on the kept side.
pub const SLIDER_MIN: f32 = 0.001;
pub const SLIDER_MAX: f32 = 0.999;

/// The section axes (ModelViewerState.swift:3-15): `title` is the
/// segmented control's label; the MODEL-axis index is the match below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionAxis {
    X,
    Y,
    Z,
}

impl SectionAxis {
    pub fn title(self) -> &'static str {
        match self {
            SectionAxis::X => "X",
            SectionAxis::Y => "Y",
            SectionAxis::Z => "Z",
        }
    }

    /// The MODEL-axis array index upstream interpolates with
    /// (SectionAxis.rawValue).
    fn index(self) -> usize {
        match self {
            SectionAxis::X => 0,
            SectionAxis::Y => 1,
            SectionAxis::Z => 2,
        }
    }
}

/// Upstream's axis list (SectionAxis.allCases) for the picker.
pub const ALL: [SectionAxis; 3] = [SectionAxis::X, SectionAxis::Y, SectionAxis::Z];

/// The MODEL-space plane value (ModelViewerState.swift:119): `position`
/// (the slider, 0..1) interpolated along the axis of the effective —
/// arranged, hidden-filtered — MODEL aabb.
pub fn plane_value(axis: SectionAxis, position: f32, model_aabb: ([f32; 3], [f32; 3])) -> f32 {
    let a = axis.index();
    model_aabb.0[a] + (model_aabb.1[a] - model_aabb.0[a]) * position
}

/// The plane as the mesh shader's discard consumes it (mesh.wgsl,
/// unchanged by this tool): a scene-space axis and a keep-below position.
/// X and Z pass their MODEL values through on their scene axes; Y negates
/// onto scene z so keep-MODEL-above becomes the shader's keep-scene-below —
/// upstream's `keepsBelow = axis != .y` (ModelViewerState.swift:120) in
/// full.
pub fn scene_plane(
    axis: SectionAxis,
    position: f32,
    model_aabb: ([f32; 3], [f32; 3]),
) -> (u32, f32) {
    let v = plane_value(axis, position, model_aabb);
    match axis {
        SectionAxis::X => (0, v),
        SectionAxis::Z => (1, v),
        SectionAxis::Y => (2, -v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AABB: ([f32; 3], [f32; 3]) = ([-10.0, -6.0, 0.0], [10.0, 6.0, 20.0]);

    #[test]
    fn plane_value_interpolates_the_model_aabb() {
        assert!((plane_value(SectionAxis::X, 0.5, AABB)).abs() < 1e-6);
        assert!((plane_value(SectionAxis::Z, 0.25, AABB) - 5.0).abs() < 1e-6);
        assert!((plane_value(SectionAxis::Y, 0.75, AABB) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn scene_plane_maps_all_three_model_axes() {
        // X and Z keep below at the plain value on their scene axes;
        // Y keeps MODEL-above = scene-below at the NEGATED value on scene z.
        assert_eq!(scene_plane(SectionAxis::X, 0.5, AABB), (0, 0.0));
        assert_eq!(scene_plane(SectionAxis::Z, 0.25, AABB), (1, 5.0));
        assert!((scene_plane(SectionAxis::Y, 0.75, AABB).1 - (-3.0)).abs() < 1e-6);
        assert_eq!(scene_plane(SectionAxis::Y, 0.75, AABB).0, 2);
    }

    #[test]
    fn cut_face_color_is_blended_system_red_through_the_eotf() {
        let srgb = [0.85_f32, 0.85 * 59.0 / 255.0, 0.85 * 48.0 / 255.0];
        for i in 0..3 {
            assert!(
                (crate::render::CUT_FACE_LINEAR[i]
                    - crate::render::parts::srgb_to_linear_f(srgb[i]))
                .abs()
                    < 1e-3,
                "channel {i}: {} vs {}",
                crate::render::CUT_FACE_LINEAR[i],
                crate::render::parts::srgb_to_linear_f(srgb[i])
            );
        }
    }

    /// The upstream bits the pane renders (ModelViewerState.swift:3-15).
    #[test]
    fn titles_and_slider_bounds_match_upstream() {
        assert_eq!(SectionAxis::X.title(), "X");
        assert_eq!(SectionAxis::Y.title(), "Y");
        assert_eq!(SectionAxis::Z.title(), "Z");
        assert_eq!(ALL, [SectionAxis::X, SectionAxis::Y, SectionAxis::Z]);
        assert_eq!(SLIDER_MIN, 0.001);
        assert_eq!(SLIDER_MAX, 0.999);
        // the slider never reaches either end: position stays strictly
        // inside, so the cut always keeps geometry
        assert!(SLIDER_MIN > 0.0 && SLIDER_MAX < 1.0);
    }
}
