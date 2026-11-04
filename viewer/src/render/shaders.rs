//! The WGSL the pipelines run: each shader source with its constants and
//! output-space block substituted from the Rust definitions the tests read.

use super::{lights, markers};

/// The section cap's color: upstream's cut-face material —
/// `systemRed.blended(withFraction: 0.15, of: .black)` (ModelViewer.swift:
/// 363) = sRGB `(0.85, 0.85·59/255, 0.85·48/255)` through the exact EOTF —
/// LINEAR, the space the unlit cap shader writes, encoded for the target
/// by `to_target` (the same once-only crossing every staged triangle color
/// makes). The Rust constant is the one definition: `cap_wgsl_source`
/// substitutes it into cap.wgsl, and the cap-color test reads it back
/// against this value.
pub const CUT_FACE_LINEAR: [f32; 3] = [0.6920, 0.0321, 0.0220];

/// The motion collision material's color: systemRed straight through the
/// exact EOTF (the same red [`CUT_FACE_LINEAR`] blends 15% black for the
/// cut face; the collision material uses it unblended as BOTH baseColor
/// and emissiveColor, ModelViewer.swift:275-279 — hence flat, unlit).
/// LINEAR, the space the mesh shader writes, encoded for the target by
/// `to_target`. The Rust constant is the one definition:
/// `mesh_wgsl_source` substitutes it into mesh.wgsl, and the
/// collision-color test reads it back against this value.
pub const COLLISION_LINEAR: [f32; 3] = [1.0, 0.0437, 0.0295];

/// The compare ghost's straight alpha (ModelViewer.swift:375):
/// `blending = .transparent(opacity: 0.6)` over the unlit systemOrange
/// rgb ([`markers::ORANGE`]) — the value `ghost_wgsl_source` substitutes
/// into ghost.wgsl and the blend test reads pixels back against.
pub const GHOST_ALPHA: f32 = 0.6;

/// The output-space block substituted as `@TARGET_SPACE@` into every
/// color-writing shader: the viewer's colors are LINEAR (staged through
/// [`parts::srgb_to_linear`], lit in linear), and whether the pipeline must
/// encode them on the way out depends on the target — an sRGB-tagged
/// format stores the encode in hardware, a plain one (what iced's
/// compositor picks under libcosmic's `web-colors` build, where
/// `GAMMA_CORRECTION` is false and every iced primitive writes
/// display-space values itself) does not, so the shader carries the curve.
/// `SRGB_ENCODE` is 1.0 only in the second case, and the curve is the
/// exact inverse of [`parts::srgb_to_linear_f`] (its Rust twin
/// [`parts::linear_to_srgb_f`] is what the tests pin against).
pub(super) fn target_space_wgsl(target_format: wgpu::TextureFormat) -> String {
    let flag = if target_format.is_srgb() { 0.0 } else { 1.0 };
    format!(
        "// Substituted from target_space_wgsl: a plain (non-sRGB) target\n\
         // stores no encode in hardware, so the shader carries the curve.\n\
         const SRGB_ENCODE: f32 = {flag:.1};\n\
         fn encode_target(c: f32) -> f32 {{\n\
         \x20   let e = select(1.055 * pow(c, 1.0 / 2.4) - 0.055, c * 12.92, c <= 0.0031308);\n\
         \x20   return clamp(e, 0.0, 1.0);\n\
         }}\n\
         fn to_target(rgb: vec3<f32>) -> vec3<f32> {{\n\
         \x20   if (SRGB_ENCODE < 0.5) {{ return rgb; }}\n\
         \x20   return vec3<f32>(encode_target(rgb.r), encode_target(rgb.g), encode_target(rgb.b));\n\
         }}"
    )
}

/// mesh.wgsl with the lighting constants substituted from [`lights`], the
/// collision color from [`COLLISION_LINEAR`], and the output-space block
/// for `target_format` — the one definition the shader and the shading
/// tests share. The @NAME@ markers are not valid WGSL, so a missed
/// substitution breaks shader compilation instead of silently rendering
/// numbers the tests do not assert against.
pub(super) fn mesh_wgsl_source(target_format: wgpu::TextureFormat) -> String {
    let vec3 = |p: [f32; 3]| format!("vec3<f32>({}, {}, {})", p[0], p[1], p[2]);
    include_str!("mesh.wgsl")
        .replace("@KEY_POS@", &vec3(lights::KEY_POS))
        .replace("@KEY_COEFF@", &lights::key_coeff().to_string())
        .replace("@FILL_POS@", &vec3(lights::FILL_POS))
        .replace("@FILL_COEFF@", &lights::fill_coeff().to_string())
        .replace("@ENV_DIR@", &vec3(lights::ENV_DIR))
        .replace("@ENV_COEFF@", &lights::ENV_COEFF.to_string())
        .replace("@AMBIENT@", &lights::AMBIENT.to_string())
        .replace("@SPECULAR@", &lights::SPECULAR.to_string())
        .replace("@SHININESS@", &lights::SHININESS.to_string())
        .replace("@SHOULDER_START@", &lights::SHOULDER_START.to_string())
        .replace("@COLLISION_R@", &COLLISION_LINEAR[0].to_string())
        .replace("@COLLISION_G@", &COLLISION_LINEAR[1].to_string())
        .replace("@COLLISION_B@", &COLLISION_LINEAR[2].to_string())
        .replace("@TARGET_SPACE@", &target_space_wgsl(target_format))
}

/// cap.wgsl with the cut-face color substituted from [`CUT_FACE_LINEAR`]
/// and the output-space block for `target_format` — the mesh_wgsl_source
/// pattern: the @CAP_@ markers are not valid WGSL, so a missed
/// substitution breaks shader compilation instead of silently rendering a
/// color the tests do not assert against.
pub(super) fn cap_wgsl_source(target_format: wgpu::TextureFormat) -> String {
    include_str!("cap.wgsl")
        .replace("@CAP_R@", &CUT_FACE_LINEAR[0].to_string())
        .replace("@CAP_G@", &CUT_FACE_LINEAR[1].to_string())
        .replace("@CAP_B@", &CUT_FACE_LINEAR[2].to_string())
        .replace("@TARGET_SPACE@", &target_space_wgsl(target_format))
}

/// grid.wgsl with the output-space block for `target_format` — the
/// mesh_wgsl_source pattern over the unlit overlay's passthrough fragment
/// (rgb encoded, alpha carried as staged).
pub(super) fn grid_wgsl_source(target_format: wgpu::TextureFormat) -> String {
    include_str!("grid.wgsl").replace("@TARGET_SPACE@", &target_space_wgsl(target_format))
}

/// ghost.wgsl with the ghost color substituted from [`markers::ORANGE`],
/// [`GHOST_ALPHA`], and the output-space block for `target_format` — the
/// mesh_wgsl_source pattern: the @GHOST_@ markers are not valid WGSL, so a
/// missed substitution breaks shader compilation instead of silently
/// rendering a color the tests do not assert against.
pub(super) fn ghost_wgsl_source(target_format: wgpu::TextureFormat) -> String {
    include_str!("ghost.wgsl")
        .replace("@GHOST_R@", &markers::ORANGE[0].to_string())
        .replace("@GHOST_G@", &markers::ORANGE[1].to_string())
        .replace("@GHOST_B@", &markers::ORANGE[2].to_string())
        .replace("@GHOST_ALPHA@", &GHOST_ALPHA.to_string())
        .replace("@TARGET_SPACE@", &target_space_wgsl(target_format))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::parts;

    /// COLLISION_LINEAR is systemRed through the exact EOTF (the same red
    /// ModelViewer.swift:363 blends 15% black for the cut face; the motion
    /// material uses it straight, ModelViewer.swift:275-279).
    #[test]
    fn collision_color_is_linear_system_red() {
        let srgb = [1.0_f32, 59.0 / 255.0, 48.0 / 255.0];
        for i in 0..3 {
            assert!(
                (COLLISION_LINEAR[i] - parts::srgb_to_linear_f(srgb[i])).abs() < 1e-3,
                "channel {i}: {} vs {}",
                COLLISION_LINEAR[i],
                parts::srgb_to_linear_f(srgb[i])
            );
        }
    }

    /// The output-space contract: iced's compositor under libcosmic's
    /// `web-colors` build picks a PLAIN (non-sRGB) surface and every iced
    /// primitive writes display-space values itself, so the viewer's
    /// linear-light output must be encoded in-shader there — and must stay
    /// linear for an sRGB-tagged target (the headless tests' format,
    /// where the hardware owns the encode).
    #[test]
    fn shader_sources_encode_only_for_plain_targets() {
        let plain = mesh_wgsl_source(wgpu::TextureFormat::Bgra8Unorm);
        assert!(
            plain.contains("SRGB_ENCODE: f32 = 1.0"),
            "a plain target turns the in-shader encode on"
        );
        let srgb = mesh_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb);
        assert!(
            srgb.contains("SRGB_ENCODE: f32 = 0.0"),
            "an sRGB target keeps the hardware encode"
        );
        for (name, source) in [
            ("mesh", mesh_wgsl_source(wgpu::TextureFormat::Bgra8Unorm)),
            ("cap", cap_wgsl_source(wgpu::TextureFormat::Bgra8Unorm)),
            ("grid", grid_wgsl_source(wgpu::TextureFormat::Bgra8Unorm)),
            ("ghost", ghost_wgsl_source(wgpu::TextureFormat::Bgra8Unorm)),
            (
                "mesh/srgb",
                mesh_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb),
            ),
            (
                "cap/srgb",
                cap_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb),
            ),
            (
                "grid/srgb",
                grid_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb),
            ),
            (
                "ghost/srgb",
                ghost_wgsl_source(wgpu::TextureFormat::Rgba8UnormSrgb),
            ),
        ] {
            // the real markers this shader must have substituted (WGSL's
            // own @group/@location decorators and the docs' @NAME@ example
            // are not markers); a genuine survivor is caught here AND by
            // the parse test, since a marker is invalid WGSL
            for marker in [
                "@KEY_POS@",
                "@KEY_COEFF@",
                "@FILL_POS@",
                "@FILL_COEFF@",
                "@ENV_DIR@",
                "@ENV_COEFF@",
                "@AMBIENT@",
                "@SPECULAR@",
                "@SHININESS@",
                "@SHOULDER_START@",
                "@COLLISION_R@",
                "@COLLISION_G@",
                "@COLLISION_B@",
                "@CAP_R@",
                "@CAP_G@",
                "@CAP_B@",
                "@GHOST_R@",
                "@GHOST_G@",
                "@GHOST_B@",
                "@GHOST_ALPHA@",
                "@TARGET_SPACE@",
            ] {
                assert!(
                    !source.contains(marker),
                    "{name}: substitution marker {marker} survived"
                );
            }
            assert!(
                source.contains("to_target("),
                "{name}: a color-writing shader must route through to_target"
            );
        }
    }

    /// The substituted sources must be valid WGSL for BOTH target kinds —
    /// a marker that survives, a bad declaration order, or a malformed
    /// encode block is a parse error here, not a silent dark render.
    #[test]
    fn substituted_shaders_parse_as_wgsl() {
        let mut flags = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        );
        for format in [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ] {
            for source in [
                mesh_wgsl_source(format),
                cap_wgsl_source(format),
                grid_wgsl_source(format),
                ghost_wgsl_source(format),
            ] {
                let module = naga::front::wgsl::parse_str(&source)
                    .unwrap_or_else(|e| panic!("parse {format:?}: {e}"));
                flags
                    .validate(&module)
                    .unwrap_or_else(|e| panic!("validate {format:?}: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod target_space_tests {
    use super::*;
    use crate::render::parts;

    /// The WGSL encode string's curve constants must match the Rust twin
    /// exactly (a typo in the string passes every pure test —
    /// only the GPU test would catch it, which runs on one machine).
    #[test]
    fn wgsl_encode_constants_match_the_rust_twin() {
        let src = target_space_wgsl(wgpu::TextureFormat::Bgra8Unorm);
        for constant in ["0.0031308", "12.92", "1.055", "2.4", "0.055"] {
            assert!(
                src.contains(constant),
                "the WGSL curve lost {constant}: {src}"
            );
        }
        // and the sRGB path is the identity (no curve at all)
        let identity = target_space_wgsl(wgpu::TextureFormat::Bgra8UnormSrgb);
        assert!(identity.contains("SRGB_ENCODE: f32 = 0.0"));
        // the twin and the string implement the same function at the knee
        // and mid-range (the knee's both branches + a gamma-branch value)
        let knee = parts::linear_to_srgb_f(0.003);
        assert!((knee - 0.003 * 12.92).abs() < 1e-6);
        let mid = parts::linear_to_srgb_f(0.5);
        assert!((mid - (1.055 * 0.5f32.powf(1.0 / 2.4) - 0.055)).abs() < 1e-6);
    }
}
