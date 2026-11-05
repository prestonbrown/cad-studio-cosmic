//! Part-keyed CPU scene: engine parts staged as scene-space vertices.
//!
//! MODEL (engine coords, Z-up) becomes SCENE (render coords, Y-up) through
//! `(x, z, -y)` — a proper rotation, applied once here at scene build —
//! then the whole scene is translated by the floor pin (ModelViewer.swift:
//! 167-168): `(-center.x, -min.y, -center.z)`, putting the model floor at
//! scene y=0 and centering x/z. The pin is recorded on the struct and
//! [`PartScene::to_scene`]/[`PartScene::to_model`] are its public crossing
//! — the grid, bed outline, and tripod and `camera::frame`'s
//! `(0, size.y/2, 0)` target are all stated against this pinned frame.
//!
//! Per-triangle colors stage here too: `TriangleColor::Some` is u8 sRGB
//! and crosses [`srgb_to_linear`] exactly once, at build; `None` paints
//! the neutral. Face normals stage with them — one per triangle on all 3
//! corners, flat shading, computed from the remapped corners by
//! [`face_normal`]. All of it lands in the staged vertex bytes, so the
//! upload-once content identity below covers color, neutral, and normal
//! changes alike.
//!
//! Each part carries a full-content hash over its staged vertices, so the
//! pipeline's upload-once check (`MeshPipeline::prepare_parts`) rewrites a
//! part's vertex buffer only when that staged content — including the pin
//! offset — changes; camera motion and per-part poses write uniforms,
//! never mesh bytes.

use std::hash::Hasher;

use engine::parsing::{Box3, Part, TriangleColor};

use super::edges::{unit_normal, wire_edges};
use super::{PartPose, Vertex};

/// Upstream's neutral face color for uncolored triangles, dark appearance
/// (ModelViewer.swift:401-403: `NSColor(red: 0.62, green: 0.70, blue:
/// 0.80)`), LINEAR: NSColor's components are sRGB-encoded and RealityKit
/// decodes material tints before lighting, so this is the sRGB decode of
/// upstream's literals — the same crossing every 3MF triangle color takes.
/// `from_parts` paints it onto uncolored triangles; it is a parameter so
/// the theme's neutral ([`neutral`]) can take its place.
pub const NEUTRAL: [f32; 3] = [0.3424, 0.448, 0.6038];

/// The light-appearance neutral (ModelViewer.swift:403: `NSColor(red:
/// 0.55, green: 0.64, blue: 0.75)`), linear like [`NEUTRAL`].
pub const NEUTRAL_LIGHT: [f32; 3] = [0.2633, 0.3672, 0.5225];

/// The neutral for an appearance (ModelViewer.swift:401-403).
pub fn neutral(dark: bool) -> [f32; 3] {
    if dark { NEUTRAL } else { NEUTRAL_LIGHT }
}

/// Exact piecewise sRGB EOTF (IEC 61966-2-2) on the f32 unit scale, where
/// 0.0 is black and 1.0 is white. The linear segment below 0.04045 is not
/// negotiable — a single 2.2 power reads the golden base's own blue R (31)
/// as 0.0097 instead of 0.0137, 29% dark, and drifts worst exactly where
/// dark colors live. The u8 form below delegates to this one, so every
/// caller (staged triangle colors, cap and marker constants) crosses the
/// same curve.
pub fn srgb_to_linear_f(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// u8 sRGB -> scene-linear f32: [`srgb_to_linear_f`] over `c / 255`.
pub fn srgb_to_linear(c: u8) -> f32 {
    srgb_to_linear_f(c as f32 / 255.0)
}

/// Exact inverse of [`srgb_to_linear_f`] (the sRGB OETF), clamped to the
/// unit range — the curve `to_target` in the color-writing shaders applies
/// when the pipeline's target format is not sRGB-tagged (see
/// [`crate::render::target_space_wgsl`]). The Rust twin of that WGSL block,
/// so tests pin one curve both sides run.
pub fn linear_to_srgb_f(l: f32) -> f32 {
    let e = if l <= 0.0031308 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    e.clamp(0.0, 1.0)
}

/// A triangle's u8 sRGB color -> linear; `None` paints the `neutral`.
fn stage_color(color: TriangleColor, neutral: [f32; 3]) -> [f32; 3] {
    color.map_or(neutral, |c| {
        [
            srgb_to_linear(c[0]),
            srgb_to_linear(c[1]),
            srgb_to_linear(c[2]),
        ]
    })
}

/// One engine part staged for upload: scene-space per-corner vertices, its
/// aabb, and the content identity the upload-once check compares. The
/// per-part transform is NOT here — poses arrive per frame through the
/// renderer's model uniforms (the GPU-first split), so this struct is pure
/// geometry and paint.
#[derive(Clone, Debug)]
pub struct GpuPart {
    pub name: String,
    /// One vertex per corner (unindexed, per-face).
    pub vertices: Vec<Vertex>,
    /// The part's wire edges as corner-index pairs into `vertices`
    /// ([`wire_edges`]: its CAD feature edges, or every triangle edge for
    /// a smooth closed body): the line list wireframe mode and the compare
    /// ghost draw. A 12-triangle cube stages its 12 edges; the face
    /// diagonals are tessellation, not features. Empty for a part with no
    /// non-degenerate triangle.
    pub edges: Vec<[u32; 2]>,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// Full-content identity over the STAGED vertices: a moved corner, a
    /// new color, or a shifted floor pin each change it as surely as an
    /// added corner, so residency tracks exactly what would be uploaded.
    pub content_hash: u64,
}

/// The whole model: per-part staging plus the union aabb used for framing
/// and the section cap quad. Built once per model change, never per frame.
#[derive(Clone, Debug)]
pub struct PartScene {
    pub parts: Vec<GpuPart>,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// The floor translation baked into every staged vertex — what
    /// [`PartScene::to_scene`] adds and [`PartScene::to_model`] removes.
    /// Recorded so MODEL-space callers can cross over, and so a second
    /// scene can share this one's frame ([`PartScene::from_parts_with_pin`],
    /// the comparison ghost's staging).
    pub pin: [f32; 3],
}

/// Content identity of one part: every staged vertex byte feeds the hash
/// (O(n), run once per `PartScene` build, after the remap and pin — so the
/// identity covers exactly the bytes residency would serve). Valid only
/// for comparing scenes built in the same process.
fn content_hash(vertices: &[Vertex]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_usize(vertices.len());
    h.write(bytemuck::cast_slice(vertices));
    h.finish()
}

/// MODEL Z-up -> SCENE Y-up, upstream's `sceneVector`: a proper rotation.
pub(crate) fn remap(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}

/// Flat-shading face normal of a scene-space triangle, upstream's
/// addTriangle rule (TriangleMesh.swift:21-23): normalize(cross(b−a, c−a)),
/// falling back to (0,0,1) for a zero-area triangle. A degenerate triangle
/// rasterizes nothing, so the fallback only keeps the attribute finite.
/// The edge pass shares the same normal ([`unit_normal`]) but skips a
/// zero-area triangle instead.
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    unit_normal(a, b, c).unwrap_or([0.0, 0.0, 1.0])
}

impl PartScene {
    /// Stage engine parts for rendering at their own floor pin (computed
    /// over the remapped union aabb): remap to scene space, paint each
    /// triangle's color (sRGB -> linear, or the neutral), pin the floor,
    /// hash each part's staged content. Parts keep engine order; slot `i`
    /// in the scene is slot `i` at residency and in the frame's poses.
    pub fn from_parts(parts: &[Part], neutral: [f32; 3]) -> Self {
        Self::from_parts_with_pin(parts, neutral, floor_pin(parts))
    }

    /// [`PartScene::from_parts`] staged against a caller-chosen `pin`
    /// instead of the computed one — how a second scene shares this one's
    /// frame: the comparison ghost stages the OLD model in the CURRENT
    /// scene's pinned frame (ModelViewer.swift:135, ghost under `content`),
    /// where its own pin would float it off the floor.
    pub fn from_parts_with_pin(parts: &[Part], neutral: [f32; 3], pin: [f32; 3]) -> Self {
        let mut staged = Vec::with_capacity(parts.len());
        for part in parts {
            assert_eq!(
                part.positions.len(),
                part.triangle_colors.len() * 3,
                "part {}: 3 corners per triangle, one color per triangle",
                part.name
            );
            // one color per triangle onto its 3 corners: `Some` is u8 sRGB
            // (3MF displaycolor) and crosses the exact EOTF here, at scene
            // build — the shader never sees an encoded value. The face
            // normal stages alongside it, one per triangle shared by all 3
            // corners (flat shading, upstream's per-face addTriangle
            // normals): computed from the REMAPPED corners, and the floor
            // pin below only translates — cross(b−a, c−a) sees differences
            // alone — so these are the pinned triangles' normals exactly.
            let vertices = part
                .positions
                .chunks_exact(3)
                .zip(&part.triangle_colors)
                .flat_map(|(corners, &color)| {
                    let color = stage_color(color, neutral);
                    let corners = [remap(corners[0]), remap(corners[1]), remap(corners[2])];
                    let normal = face_normal(corners[0], corners[1], corners[2]);
                    corners
                        .into_iter()
                        .map(move |pos| Vertex { pos, normal, color })
                })
                .collect::<Vec<_>>();
            let bounds = Box3::from_points(vertices.iter().map(|v| v.pos)).unwrap_or(Box3::ZERO);
            staged.push(GpuPart {
                name: part.name.clone(),
                vertices,
                edges: Vec::new(), // set after the pin below
                aabb_min: bounds.min,
                aabb_max: bounds.max,
                content_hash: 0, // set after the pin below
            });
        }
        // a part with no triangles stages a zero box but bounds nothing
        let scene_box = Box3::union_all(
            staged
                .iter()
                .filter(|p| !p.vertices.is_empty())
                .map(|p| Box3::new(p.aabb_min, p.aabb_max)),
        );
        // The pin is baked into the staged vertices — a later part growing
        // past the union aabb moves `from_parts`'s pin, and the content
        // hashes (computed after) pick that up so residency never serves a
        // part at a stale pin.
        for part in &mut staged {
            for v in &mut part.vertices {
                for a in 0..3 {
                    v.pos[a] += pin[a];
                }
            }
            for a in 0..3 {
                part.aabb_min[a] += pin[a];
                part.aabb_max[a] += pin[a];
            }
        }
        // an empty scene renders nothing; its box stays finite for the
        // camera math
        let scene_box = scene_box.map_or(Box3::ZERO, |b| b.translated(pin));
        for part in &mut staged {
            let positions: Vec<[f32; 3]> = part.vertices.iter().map(|v| v.pos).collect();
            let corners: Vec<u32> = (0..positions.len() as u32).collect();
            part.edges = wire_edges(&positions, &corners);
            part.content_hash = content_hash(&part.vertices);
        }
        Self {
            parts: staged,
            aabb_min: scene_box.min,
            aabb_max: scene_box.max,
            pin,
        }
    }

    /// The staged (pinned scene) bounds as a [`Box3`].
    pub fn aabb(&self) -> Box3 {
        Box3::new(self.aabb_min, self.aabb_max)
    }

    /// MODEL -> this scene's pinned SCENE: the remap plus the recorded
    /// floor pin — exactly the map the staged vertices crossed.
    pub fn to_scene(&self, model: [f32; 3]) -> [f32; 3] {
        let s = remap(model);
        [s[0] + self.pin[0], s[1] + self.pin[1], s[2] + self.pin[2]]
    }

    /// Pinned SCENE -> MODEL: the inverse of [`PartScene::to_scene`].
    pub fn to_model(&self, scene: [f32; 3]) -> [f32; 3] {
        let s = [
            scene[0] - self.pin[0],
            scene[1] - self.pin[1],
            scene[2] - self.pin[2],
        ];
        [s[0], -s[2], s[1]]
    }

    /// A MODEL box as this scene's pinned SCENE box: the corners cross
    /// [`PartScene::to_scene`] and re-fold, since the remap negates an axis.
    pub fn model_box_to_scene(&self, model: Box3) -> Box3 {
        Box3::from_points([self.to_scene(model.min), self.to_scene(model.max)]).expect("two points")
    }

    /// A pinned SCENE box as a MODEL box: [`PartScene::model_box_to_scene`]
    /// inverted.
    pub fn scene_box_to_model(&self, scene: Box3) -> Box3 {
        Box3::from_points([self.to_model(scene.min), self.to_model(scene.max)]).expect("two points")
    }

    /// The pose that moves a staged part by a MODEL-space `offset` (an
    /// arrangement push): the offset is a direction, so it takes the remap
    /// but not the pin.
    pub fn model_offset_pose(offset: [f32; 3]) -> PartPose {
        Some(glam::Mat4::from_translation(glam::Vec3::from_array(remap(offset))).to_cols_array_2d())
    }
}

/// The floor pin (ModelViewer.swift:167-168) over the remapped union aabb:
/// `(-center.x, -min.y, -center.z)` — the model floor at scene y=0, x/z
/// centered on the origin. Zero for an empty scene.
fn floor_pin(parts: &[Part]) -> [f32; 3] {
    let remapped = parts
        .iter()
        .flat_map(|p| p.positions.iter().map(|&q| remap(q)));
    let Some(b) = Box3::from_points(remapped) else {
        return [0.0; 3];
    };
    let c = b.center();
    [-c[0], -b.min[1], -c[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::parts;
    use crate::render::test_support::{golden_3mf_parts, golden_stl_parts};

    /// The box crossings agree with the point crossings, re-fold the
    /// negated axis, and invert each other.
    #[test]
    fn box_crossings_match_to_scene_and_roundtrip() {
        let part = Part {
            name: "p".into(),
            positions: vec![[-10.0, -6.0, 0.0], [10.0, 6.0, 20.0], [0.0, 0.0, 5.0]],
            triangle_colors: vec![None],
        };
        let scene = PartScene::from_parts(&[part], NEUTRAL);
        let model = Box3::new([-10.0, -6.0, 0.0], [10.0, 6.0, 20.0]);
        let s = scene.model_box_to_scene(model);
        // scene x = model x, scene y = model z, scene z = -model y; pinned
        // floor at y 0, x/z centered
        assert_eq!(s, Box3::new([-10.0, 0.0, -6.0], [10.0, 20.0, 6.0]));
        assert_eq!(s, scene.aabb(), "the staged box is the model box crossed");
        assert_eq!(scene.scene_box_to_model(s), model);
    }

    /// An arrangement offset moves a part by the remapped direction alone.
    #[test]
    fn model_offset_pose_remaps_without_the_pin() {
        let m =
            glam::Mat4::from_cols_array_2d(&PartScene::model_offset_pose([1.0, 2.0, 3.0]).unwrap());
        assert_eq!(
            m.transform_point3(glam::Vec3::ZERO),
            glam::vec3(1.0, 3.0, -2.0)
        );
        assert_eq!(
            m.transform_vector3(glam::Vec3::X),
            glam::Vec3::X,
            "pure translation"
        );
    }

    /// The neutrals are upstream's NSColor literals decoded to linear —
    /// staging the raw literals as linear renders them visibly washed out.
    #[test]
    fn neutrals_are_decoded_upstream_literals() {
        for (linear, literal) in [
            (NEUTRAL, [0.62, 0.70, 0.80]),
            (NEUTRAL_LIGHT, [0.55, 0.64, 0.75]),
        ] {
            for i in 0..3 {
                assert!(
                    (linear[i] - srgb_to_linear_f(literal[i])).abs() < 1e-3,
                    "{linear:?} vs decode of {literal:?}"
                );
            }
        }
        assert_eq!(neutral(true), NEUTRAL);
        assert_eq!(neutral(false), NEUTRAL_LIGHT);
    }

    /// The staged colors cross srgb_to_linear once at build; the shader
    /// curves them back through linear_to_srgb on a plain target — the
    /// pair must be exact inverses (to the f32 rounding of two powfs), and
    /// both must be the identity-fixed at the endpoints and the sRGB
    /// standard's own knee pair (0.04045 sRGB ↔ 0.0031308 linear).
    #[test]
    fn linear_to_srgb_inverts_the_staging_eotf() {
        let knee = 0.04045f32;
        assert!((srgb_to_linear_f(knee) - knee / 12.92).abs() < 1e-7);
        assert!((linear_to_srgb_f(knee / 12.92) - knee).abs() < 1e-6);
        assert_eq!(linear_to_srgb_f(0.0), 0.0);
        // 1.055 − 0.055 is one ulp shy of 1 in f32
        assert!((linear_to_srgb_f(1.0) - 1.0).abs() < 1e-6);
        // the sRGB OETF's own reference pair
        assert!((linear_to_srgb_f(0.5) - 0.7354).abs() < 1e-4);
        // roundtrip over the whole byte range: u8 -> linear -> u8 is exact
        for b in 0u8..=255 {
            let there = srgb_to_linear(b);
            let back = linear_to_srgb_f(there) * 255.0;
            assert!(
                (back - b as f32).abs() <= 0.5,
                "b {b}: linear {there:.6} -> {back:.3}"
            );
        }
        // out-of-range input clamps, never NaN
        assert_eq!(linear_to_srgb_f(1.2), 1.0);
        assert_eq!(linear_to_srgb_f(-0.1), 0.0);
    }

    #[test]
    fn from_parts_remaps_and_pins_floor_at_origin() {
        let part = engine::parsing::Part {
            name: "tri".into(),
            positions: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            triangle_colors: vec![None],
        };
        let scene = PartScene::from_parts(std::slice::from_ref(&part), NEUTRAL);
        // upstream sceneVector (x, z, -y) maps the MODEL triangle to
        // x[1,7] y[3,9] z[-8,-2]; the pin then translates by
        // (-center.x, -min.y, -center.z) = (-4, -3, +5): floor at scene
        // y=0, x/z centered (ModelViewer.swift:167-168)
        assert_eq!(scene.parts[0].vertices[0].pos, [-3.0, 0.0, 3.0]);
        assert_eq!(scene.parts[0].vertices[2].pos, [3.0, 6.0, -3.0]);
        // neutral painted, per-part aabb is the pinned triangle's
        assert_eq!(scene.parts[0].vertices[0].color, NEUTRAL);
        assert_eq!(scene.parts[0].aabb_min, [-3.0, 0.0, -3.0]);
        assert_eq!(scene.parts[0].aabb_max, [3.0, 6.0, 3.0]);
        // the scene aabb is the pinned union: y from 0, x/z symmetric
        assert_eq!(scene.aabb_min, [-3.0, 0.0, -3.0]);
        assert_eq!(scene.aabb_max, [3.0, 6.0, 3.0]);
    }

    #[test]
    fn golden_stl_scene_is_24_triangles_inside_its_aabb() {
        let scene = PartScene::from_parts(&golden_stl_parts(), NEUTRAL);
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].vertices.len(), 24 * 3);
        for v in &scene.parts[0].vertices {
            for a in 0..3 {
                assert!(v.pos[a] >= scene.aabb_min[a] - 1e-6);
                assert!(v.pos[a] <= scene.aabb_max[a] + 1e-6);
            }
        }
        for a in 0..3 {
            assert!(scene.aabb_max[a] > scene.aabb_min[a]);
        }
    }

    #[test]
    fn golden_3mf_scene_is_two_parts_of_twelve_triangles() {
        let scene = PartScene::from_parts(&golden_3mf_parts(), NEUTRAL);
        assert_eq!(scene.parts.len(), 2, "one GpuPart per engine part");
        for part in &scene.parts {
            assert_eq!(part.vertices.len(), 12 * 3);
            for a in 0..3 {
                assert!(part.aabb_max[a] > part.aabb_min[a]);
                assert!(part.aabb_min[a] >= scene.aabb_min[a] - 1e-6);
                assert!(part.aabb_max[a] <= scene.aabb_max[a] + 1e-6);
            }
        }
        // fixture: two boxes, base 60x60 z in [-4,4] and lid 40x40 z in
        // [12,22] — MODEL x/y/z spans 60/60/26 become SCENE x/y/z 60/26/60
        let span = |a: usize| scene.aabb_max[a] - scene.aabb_min[a];
        assert!((span(0) - 60.0).abs() < 1e-3, "scene x span {}", span(0));
        assert!((span(1) - 26.0).abs() < 1e-3, "scene y span {}", span(1));
        assert!((span(2) - 60.0).abs() < 1e-3, "scene z span {}", span(2));
        // post-pin the union aabb itself: MODEL floor (z=-4) at scene y=0,
        // x/z centered on the scene origin, spans unchanged
        let pinned = [
            (scene.aabb_min[0], scene.aabb_max[0], -30.0, 30.0),
            (scene.aabb_min[1], scene.aabb_max[1], 0.0, 26.0),
            (scene.aabb_min[2], scene.aabb_max[2], -30.0, 30.0),
        ];
        for (a, &(mn, mx, emn, emx)) in pinned.iter().enumerate() {
            assert!(
                (mn - emn).abs() < 1e-3 && (mx - emx).abs() < 1e-3,
                "pinned aabb[{a}] [{mn}, {mx}] vs [{emn}, {emx}]"
            );
        }
    }

    /// The sRGB EOTF is the exact IEC piecewise, not a single power: the
    /// linear segment below 0.04045 and the 2.4 segment above differ enough
    /// that a 2.2-only conversion is visibly wrong on dark channels.
    #[test]
    fn srgb_conversion_table() {
        let f = parts::srgb_to_linear;
        assert_eq!(f(0), 0.0);
        assert!((f(255) - 1.0).abs() < 1e-6);
        assert!((f(128) - 0.216).abs() < 5e-4, "128 -> {}", f(128));
        // the dark discriminator — the base part's own blue R channel: a
        // 2.2 power reads 0.0097 here, 29% low, and fails this assert
        assert!((f(31) - 0.0137).abs() < 2e-4, "31 -> {}", f(31));
        // the linear segment: 10/255 = 0.0392 stays below 0.04045, so
        // /12.92 — a power-only formula reads 0.0008, 3.8x low
        assert!((f(10) - 0.003035).abs() < 2e-5, "10 -> {}", f(10));
        // continuity across the segment boundary (10 linear, 11 power)
        assert!(f(10) < f(11) && f(11) < f(12));
    }

    /// Per-triangle colors stage per corner: `Some` goes through the sRGB
    /// EOTF onto all 3 corners, `None` paints the neutral. Pinned against
    /// the golden fixture's actual bytes so a parse regression cannot pass.
    #[test]
    fn colored_triangles_paint_linear_corners() {
        let parts = golden_3mf_parts();
        // fixture truth (model.3mf basematerials): base #1F78B5, lid #D62629
        assert_eq!(parts[0].name, "base");
        assert_eq!(parts[0].triangle_colors, vec![Some([31, 120, 181]); 12]);
        assert_eq!(parts[1].name, "lid");
        assert_eq!(parts[1].triangle_colors, vec![Some([214, 38, 41]); 12]);
        let scene = PartScene::from_parts(&parts, NEUTRAL);
        let linear = |c: [u8; 3]| {
            [
                parts::srgb_to_linear(c[0]),
                parts::srgb_to_linear(c[1]),
                parts::srgb_to_linear(c[2]),
            ]
        };
        let (blue, red) = (linear([31, 120, 181]), linear([214, 38, 41]));
        assert!(blue[0] < 0.02 && blue[2] > 0.45, "dark-R blue: {blue:?}");
        for v in &scene.parts[0].vertices {
            assert_eq!(v.color, blue, "base corner painted its triangle's color");
        }
        for v in &scene.parts[1].vertices {
            assert_eq!(v.color, red, "lid corner painted its triangle's color");
        }
        // mixed part: Some -> linear red exactly; None -> neutral
        let mixed = engine::parsing::Part {
            name: "mixed".into(),
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
            triangle_colors: vec![Some([255, 0, 0]), None],
        };
        let scene = PartScene::from_parts(std::slice::from_ref(&mixed), NEUTRAL);
        for v in &scene.parts[0].vertices[0..3] {
            assert_eq!(
                v.color,
                [1.0, 0.0, 0.0],
                "Some([255,0,0]) is exact linear red"
            );
        }
        for v in &scene.parts[0].vertices[3..6] {
            assert_eq!(v.color, NEUTRAL, "None paints the neutral");
        }
    }

    /// Every staged triangle is flat-shaded with its own face
    /// normal — all 3 corners carry normalize(cross(b−a, c−a)) of the
    /// STAGED (remapped, floor-pinned) positions, recomputed here
    /// independently of the staging code. A zero-area triangle falls back
    /// to (0,0,1) (upstream TriangleMesh.swift:21-23). The 1e-4 tolerance
    /// is float headroom only: a wrong normal (wrong corner order, MODEL
    /// instead of scene space, unnormalized) is O(1) off.
    #[test]
    fn normals_match_cross_product() {
        let scenes = [
            PartScene::from_parts(&golden_3mf_parts(), NEUTRAL),
            PartScene::from_parts(&golden_stl_parts(), NEUTRAL),
        ];
        for scene in &scenes {
            for part in &scene.parts {
                for tri in part.vertices.chunks_exact(3) {
                    let (a, b, c) = (
                        glam::Vec3::from_array(tri[0].pos),
                        glam::Vec3::from_array(tri[1].pos),
                        glam::Vec3::from_array(tri[2].pos),
                    );
                    let cross = (b - a).cross(c - a);
                    let len = cross.length();
                    assert!(len > 0.0, "fixture triangles are non-degenerate");
                    let n = (cross / len).to_array();
                    for v in tri {
                        for axis in 0..3 {
                            assert!(
                                (v.normal[axis] - n[axis]).abs() < 1e-4,
                                "corner normal {:?} != face normal {n:?}",
                                v.normal
                            );
                        }
                        let l = v.normal.iter().map(|x| x * x).sum::<f32>().sqrt();
                        assert!((l - 1.0).abs() < 1e-4, "unit length, got {l}");
                    }
                }
            }
        }
        // degenerate: a zero-area triangle stages the (0,0,1) fallback on
        // all 3 corners — the brief's literal constant, matching upstream's
        // addTriangle fallback shape (unobservable: nothing rasterizes)
        let part = engine::parsing::Part {
            name: "degenerate".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [2.0, 2.0, 2.0]],
            triangle_colors: vec![None],
        };
        let scene = PartScene::from_parts(std::slice::from_ref(&part), NEUTRAL);
        for v in &scene.parts[0].vertices {
            assert_eq!(v.normal, [0.0, 0.0, 1.0]);
        }
    }

    /// The f32 EOTF is the exact core the u8 form delegates to: every u8
    /// value through `srgb_to_linear_f(c / 255)` equals `srgb_to_linear(c)`,
    /// so downstream f32 callers (the cap color, the marker orange) sit on
    /// the same curve the staging paints with.
    #[test]
    fn srgb_to_linear_f_is_the_u8_forms_core() {
        for c in 0..=255u8 {
            assert_eq!(
                parts::srgb_to_linear_f(c as f32 / 255.0),
                parts::srgb_to_linear(c),
                "u8 {c}"
            );
        }
        // the segment boundary is <= on the linear side
        assert_eq!(parts::srgb_to_linear_f(0.04045), 0.04045 / 12.92);
        assert!(parts::srgb_to_linear_f(0.04046) > parts::srgb_to_linear_f(0.04045));
        // out-of-range inputs keep the formula's shape (callers clamp)
        assert_eq!(parts::srgb_to_linear_f(0.0), 0.0);
        assert!((parts::srgb_to_linear_f(1.0) - 1.0).abs() < 1e-6);
    }

    /// `to_scene` is exactly the staging map (remap + the struct's pin), and
    /// `to_model` inverts it: every MODEL corner lands on its staged byte,
    /// and the roundtrip returns the MODEL corner — the crossing
    /// picking and marker placement ride.
    #[test]
    fn to_scene_to_model_match_staging_and_roundtrip() {
        let parts = golden_3mf_parts();
        let scene = PartScene::from_parts(&parts, NEUTRAL);
        for (part, raw) in scene.parts.iter().zip(&parts) {
            assert_eq!(part.vertices.len(), raw.positions.len());
            for (v, p) in part.vertices.iter().zip(&raw.positions) {
                let s = scene.to_scene(*p);
                let m = scene.to_model(v.pos);
                for a in 0..3 {
                    assert!((s[a] - v.pos[a]).abs() < 1e-5, "to_scene != staged at {a}");
                    assert!((m[a] - p[a]).abs() < 1e-4, "roundtrip != MODEL at {a}");
                }
            }
        }
        // the recorded pin is the map's translation: to_scene of the origin
        // is the pin itself
        assert_eq!(scene.to_scene([0.0; 3]), scene.pin);
    }

    /// `from_parts_with_pin` stages against a caller-chosen frame: the
    /// recorded pin is the input, and the staged bytes sit at exactly
    /// (foreign pin − own pin) from `from_parts`'s — the comparison ghost's
    /// contract (the ghost stages the old model in the CURRENT scene's frame).
    #[test]
    fn from_parts_with_pin_honors_a_foreign_pin() {
        let parts = golden_3mf_parts();
        let own = PartScene::from_parts(&parts, NEUTRAL);
        let pin = [10.0, 20.0, 30.0];
        let foreign = PartScene::from_parts_with_pin(&parts, NEUTRAL, pin);
        assert_eq!(foreign.pin, pin);
        let shift = |a: usize| pin[a] - own.pin[a];
        for a in 0..3 {
            assert!(
                (foreign.aabb_min[a] - own.aabb_min[a] - shift(a)).abs() < 1e-4,
                "aabb shifted by the pin difference at {a}"
            );
        }
        for (f, o) in foreign.parts[0].vertices.iter().zip(&own.parts[0].vertices) {
            for a in 0..3 {
                assert!((f.pos[a] - o.pos[a] - shift(a)).abs() < 1e-4);
            }
        }
        // and the foreign scene's own crossing map lands on ITS staging
        let p = parts[0].positions[0];
        let s = foreign.to_scene(p);
        for a in 0..3 {
            assert!((s[a] - foreign.parts[0].vertices[0].pos[a]).abs() < 1e-5);
        }
    }
}
