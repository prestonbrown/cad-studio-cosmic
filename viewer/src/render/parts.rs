//! Part-keyed CPU scene: engine parts staged as scene-space vertices.
//!
//! MODEL (engine coords, Z-up) becomes SCENE (render coords, Y-up) through
//! `(x, z, -y)` — a proper rotation, applied once here at scene build —
//! then the whole scene is translated by the floor pin (ModelViewer.swift:
//! 167-168): `(-center.x, -min.y, -center.z)`, putting the model floor at
//! scene y=0 and centering x/z. The pin is recorded on the struct and
//! [`PartScene::to_scene`]/[`PartScene::to_model`] are its public crossing
//! — the grid, bed outline, and tripod (Task 5) and `camera::frame`'s
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

use engine::parsing::{Part, TriangleColor};

use super::Vertex;

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
    /// Unique undirected edges as corner-index pairs into `vertices` —
    /// the wireframe pass's line list (facts #44): every triangle edge of
    /// the part, shared ones collapsed to one line by position-keyed
    /// dedup, so a quad-faced cube stages its 12 polygon edges plus the 6
    /// face diagonals = 18.
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

/// Unique undirected edges of an unindexed triangle soup, as corner-index
/// pairs (facts #44) — the wireframe pass's line list. Upstream's `.lines`
/// fill mode rasterizes every triangle edge, so a triangulated face shows
/// its diagonals too; dedup here collapses only the SHARED edges an
/// unindexed soup repeats per incident triangle (36 per-triangle pairs for
/// a 12-triangle cube → its 12 polygon edges + 6 diagonals = 18). Dedup
/// keys on POSITION bit patterns, not indices, for exactly that reason.
fn undirected_edges(vertices: &[Vertex]) -> Vec<[u32; 2]> {
    let bits = |p: [f32; 3]| p.map(f32::to_bits);
    // first-appearance order keeps the output deterministic run to run
    // (HashMap iteration is not)
    let mut seen = std::collections::HashSet::new();
    let mut edges = Vec::new();
    for (t, tri) in vertices.chunks_exact(3).enumerate() {
        let base = (3 * t) as u32;
        for &(a, b) in &[(0usize, 1), (1, 2), (2, 0)] {
            let (pa, pb) = (bits(tri[a].pos), bits(tri[b].pos));
            // order the endpoints so (a,b) and (b,a) are one pair
            let key = if (pa, pb) <= (pb, pa) {
                (pa, pb)
            } else {
                (pb, pa)
            };
            if seen.insert(key) {
                edges.push([base + a as u32, base + b as u32]);
            }
        }
    }
    edges
}

/// Flat-shading face normal of a scene-space triangle, upstream's
/// addTriangle rule (TriangleMesh.swift:21-23): normalize(cross(b−a, c−a)),
/// falling back to (0,0,1) for a zero-area triangle. A degenerate triangle
/// rasterizes nothing, so the fallback only keeps the attribute finite.
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let (a, b, c) = (
        glam::Vec3::from_array(a),
        glam::Vec3::from_array(b),
        glam::Vec3::from_array(c),
    );
    let cross = (b - a).cross(c - a);
    let len = cross.length();
    if len > 0.0 {
        (cross / len).to_array()
    } else {
        [0.0, 0.0, 1.0]
    }
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
            let mut mn = [f32::MAX; 3];
            let mut mx = [f32::MIN; 3];
            for v in &vertices {
                for a in 0..3 {
                    mn[a] = mn[a].min(v.pos[a]);
                    mx[a] = mx[a].max(v.pos[a]);
                }
            }
            staged.push(GpuPart {
                name: part.name.clone(),
                vertices,
                edges: Vec::new(), // set after the pin below
                aabb_min: mn,
                aabb_max: mx,
                content_hash: 0, // set after the pin below
            });
        }
        let (mut scene_min, mut scene_max) = ([f32::MAX; 3], [f32::MIN; 3]);
        for part in &staged {
            for a in 0..3 {
                scene_min[a] = scene_min[a].min(part.aabb_min[a]);
                scene_max[a] = scene_max[a].max(part.aabb_max[a]);
            }
        }
        if staged.is_empty() {
            // an empty scene renders nothing; keep the aabb finite for the
            // camera math (the pin of a degenerate box is zero anyway)
            return Self {
                parts: staged,
                aabb_min: [0.0; 3],
                aabb_max: [0.0; 3],
                pin,
            };
        }
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
        for a in 0..3 {
            scene_min[a] += pin[a];
            scene_max[a] += pin[a];
        }
        for part in &mut staged {
            part.edges = undirected_edges(&part.vertices);
            part.content_hash = content_hash(&part.vertices);
        }
        Self {
            parts: staged,
            aabb_min: scene_min,
            aabb_max: scene_max,
            pin,
        }
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
}

/// The floor pin (ModelViewer.swift:167-168) over the remapped union aabb:
/// `(-center.x, -min.y, -center.z)` — the model floor at scene y=0, x/z
/// centered on the origin. Zero for an empty scene.
fn floor_pin(parts: &[Part]) -> [f32; 3] {
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut any = false;
    for part in parts {
        for &p in &part.positions {
            let p = remap(p);
            any = true;
            for a in 0..3 {
                mn[a] = mn[a].min(p[a]);
                mx[a] = mx[a].max(p[a]);
            }
        }
    }
    if !any {
        return [0.0; 3];
    }
    let mut pin = [0.0f32; 3];
    for a in 0..3 {
        pin[a] = -(mn[a] + mx[a]) / 2.0;
    }
    pin[1] = -mn[1];
    pin
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
