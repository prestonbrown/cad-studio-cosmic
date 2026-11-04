//! Parity shading for upstream's PhysicallyBasedMaterial under its light
//! rig (ModelViewer.swift:344-353, 415-423). Upstream adds two directional
//! lights — key 2600 looking at the origin from (60,120,90), fill 900 from
//! (-90,40,-60), both SCENE space — on top of RealityView's default
//! image-based environment, then shades metallic 0.15 / roughness 0.55.
//! The IBL is not in the source, so the model here is fitted to the
//! upstream capture's per-face colors (both box parts, three visible faces
//! each, sampled after converting the capture from its display profile to
//! sRGB) and reproduces them within ~7 sRGB units:
//!
//! `rgb = shoulder(D(n)·base + S(n, v))` with
//! - `D(n) = AMBIENT + KEY·max(n·key, 0) + FILL·max(n·fill, 0) + ENV·max(n·env, 0)`
//!   — key and fill keep upstream's 2600:900 intensity ratio, `env` is the
//!   environment's dominant direction (up and toward scene +Z);
//! - `S(n, v) = SPECULAR·max(n·h, 0)^SHININESS·max(n·key, 0)`, a white
//!   Blinn-Phong lobe on the key light, `h = normalize(key + v)` with `v`
//!   the direction toward the camera (the view axis, uniform per frame,
//!   so a flat face shades flat);
//! - `shoulder` is the per-channel highlight roll-off RealityKit's output
//!   shows above 0.76 linear (the Khronos PBR Neutral curve's compression).
//!
//! The constants live here, once: [`MeshPipeline`] substitutes them into
//! mesh.wgsl at creation, and [`lights::shade`] is the Rust twin the
//! headless shading tests derive their expectations from.

/// Key light position, scene space (ModelViewer.swift:346-348).
pub const KEY_POS: [f32; 3] = [60.0, 120.0, 90.0];
/// Key light intensity (ModelViewer.swift:347).
pub const KEY_INTENSITY: f32 = 2600.0;
/// Fill light position, scene space (ModelViewer.swift:350-352).
pub const FILL_POS: [f32; 3] = [-90.0, 40.0, -60.0];
/// Fill light intensity (ModelViewer.swift:351).
pub const FILL_INTENSITY: f32 = 900.0;
/// Environment light direction, scene space (fitted).
pub const ENV_DIR: [f32; 3] = [0.0, 0.69, 0.73];
/// Environment light diffuse coefficient (fitted).
pub const ENV_COEFF: f32 = 0.43;
/// Flat ambient term (fitted).
pub const AMBIENT: f32 = 0.216;
/// Key-light Blinn-Phong strength and exponent (fitted).
pub const SPECULAR: f32 = 3.53;
pub const SHININESS: f32 = 8.4;
/// Linear value above which the highlight shoulder compresses.
pub const SHOULDER_START: f32 = 0.76;
/// Directional intensity to diffuse coefficient (fitted on the key:
/// 2600 -> 0.89, so the fill's 900 -> 0.308).
const INTENSITY_SCALE: f32 = 0.89 / 2600.0;

/// The key light's diffuse coefficient.
pub fn key_coeff() -> f32 {
    KEY_INTENSITY * INTENSITY_SCALE
}

/// The fill light's diffuse coefficient.
pub fn fill_coeff() -> f32 {
    FILL_INTENSITY * INTENSITY_SCALE
}

/// The diffuse light `D(n)` for a scene-space normal. A zero vector
/// degrades to ambient.
pub fn diffuse(normal: [f32; 3]) -> f32 {
    let n = glam::Vec3::from_array(normal).normalize_or_zero();
    let key = glam::Vec3::from_array(KEY_POS).normalize();
    let fill = glam::Vec3::from_array(FILL_POS).normalize();
    let env = glam::Vec3::from_array(ENV_DIR).normalize();
    AMBIENT
        + key_coeff() * n.dot(key).max(0.0)
        + fill_coeff() * n.dot(fill).max(0.0)
        + ENV_COEFF * n.dot(env).max(0.0)
}

/// The white specular term `S(n, v)`, `to_eye` the direction toward
/// the camera.
pub fn specular(normal: [f32; 3], to_eye: [f32; 3]) -> f32 {
    let n = glam::Vec3::from_array(normal).normalize_or_zero();
    let key = glam::Vec3::from_array(KEY_POS).normalize();
    let h = (key + glam::Vec3::from_array(to_eye).normalize_or_zero()).normalize_or_zero();
    SPECULAR * n.dot(h).max(0.0).powf(SHININESS) * n.dot(key).max(0.0)
}

/// The highlight shoulder on one linear channel.
pub fn shoulder(c: f32) -> f32 {
    if c < SHOULDER_START {
        c.max(0.0)
    } else {
        let d = 1.0 - SHOULDER_START;
        1.0 - d * d / (c + d - SHOULDER_START)
    }
}

/// The linear rgb mesh.wgsl writes for a linear `base` color with
/// scene normal `normal`, seen from `to_eye`.
pub fn shade(base: [f32; 3], normal: [f32; 3], to_eye: [f32; 3]) -> [f32; 3] {
    let d = diffuse(normal);
    let s = specular(normal, to_eye);
    base.map(|c| shoulder(d * c + s))
}

/// The direction toward an orbit camera at `(yaw, pitch)` —
/// placeCamera's eye offset (ModelViewer.swift:339-342), which is the
/// view axis mesh.wgsl reads back out of the view-projection matrix.
pub fn to_eye(yaw: f32, pitch: f32) -> [f32; 3] {
    super::camera::eye_dir(yaw, pitch).to_array()
}

#[cfg(test)]
mod tests {
    use crate::render::lights;
    use crate::render::parts;

    /// The shading model reproduces upstream's box: every visible face of
    /// both golden parts (3MF colors #D62629 lid, #1F78B5 base) at the iso
    /// preset, against shots/upstream/project-model.png converted from its
    /// display profile to sRGB. Each reference is the value the face
    /// presents as a whole: the polygon mean for the tops, and the flat
    /// plateau that covers most of each side (a side's mean is pulled up
    /// by a reflection band along the base's front edge). Five faces must
    /// land within 7 units per channel. The base top is an in-face IBL
    /// gradient a flat-shaded face cannot follow — left corner 86,158,212,
    /// right corner 64,147,205, mean 74,152,208 — and must land inside
    /// that measured span, within 3.
    #[test]
    fn shade_matches_the_upstream_iso_faces() {
        let lin = |c: [u8; 3]| c.map(parts::srgb_to_linear);
        let (lid, base) = (lin([214, 38, 41]), lin([31, 120, 181]));
        let to_eye = lights::to_eye(std::f32::consts::FRAC_PI_4, 0.6);
        let (top, left, right) = ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        let srgb = |color: [f32; 3], normal: [f32; 3]| {
            lights::shade(color, normal, to_eye)
                .map(|c| (parts::linear_to_srgb_f(c) * 255.0).round() as i32)
        };
        for (color, normal, upstream) in [
            (lid, top, [238, 94, 94]),
            (lid, left, [217, 53, 54]),
            (lid, right, [161, 32, 34]),
            (base, left, [48, 130, 187]),
            (base, right, [33, 93, 138]),
        ] {
            let shaded = srgb(color, normal);
            for i in 0..3 {
                assert!(
                    (shaded[i] - upstream[i]).abs() <= 7,
                    "normal {normal:?}: {shaded:?} vs upstream {upstream:?}"
                );
            }
        }
        let (lo, hi) = ([64, 147, 205], [86, 158, 212]);
        let shaded = srgb(base, top);
        for i in 0..3 {
            assert!(
                shaded[i] >= lo[i] - 3 && shaded[i] <= hi[i] + 3,
                "base top {shaded:?} outside upstream's gradient {lo:?}..{hi:?}"
            );
        }
    }
}
