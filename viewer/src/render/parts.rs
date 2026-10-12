//! Part-keyed CPU scene: engine parts staged as scene-space vertices.
//!
//! MODEL (engine coords, Z-up) becomes SCENE (render coords, Y-up) through
//! `(x, z, -y)` — a proper rotation, applied once here at scene build —
//! then the whole scene is translated by the floor pin (ModelViewer.swift:
//! 167-168): `(-center.x, -min.y, -center.z)`, putting the model floor at
//! scene y=0 and centering x/z. The grid, bed outline, and tripod (Task 5)
//! and `camera::frame`'s `(0, size.y/2, 0)` target are all stated against
//! this pinned frame.
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
//! offset — changes; camera motion writes uniforms, never mesh bytes.

use std::hash::Hasher;

use engine::parsing::{Part, TriangleColor};

use super::Vertex;

/// The app's fixed neutral face color (upstream-contract §5): dark mode's
/// (0.62, 0.70, 0.80) (ModelViewer.swift:401-403). The facts give values,
/// not a color space; LINEAR is the recorded assumption — it is staged as
/// the vertex color straight into the shader's lit multiply, like every
/// EOTF-converted triangle color beside it. `from_parts` paints it onto
/// uncolored triangles; it is a parameter so a theme-supplied neutral can
/// take its place.
pub const NEUTRAL: [f32; 3] = [0.62, 0.70, 0.80];

/// Exact piecewise sRGB EOTF (IEC 61966-2-2): u8 sRGB -> scene-linear f32.
/// The linear segment below 0.04045 is not negotiable — a single 2.2 power
/// reads the golden base's own blue R (31) as 0.0097 instead of 0.0137,
/// 29% dark, and drifts worst exactly where dark colors live.
pub fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
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
/// aabb, and the content identity the upload-once check compares.
#[derive(Clone, Debug)]
pub struct GpuPart {
    pub name: String,
    /// One vertex per corner (unindexed, per-face).
    pub vertices: Vec<Vertex>,
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
fn remap(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
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
    /// Stage engine parts for rendering: remap to scene space, paint each
    /// triangle's color (sRGB -> linear, or the neutral), pin the floor,
    /// hash each part's staged content. Parts keep engine order; slot `i`
    /// in the scene is slot `i` at residency.
    pub fn from_parts(parts: &[Part], neutral: [f32; 3]) -> Self {
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
            };
        }
        // Floor pin (ModelViewer.swift:167-168): translate the remapped
        // scene so the model floor sits at y=0 and x/z center on the
        // origin. Baked into the staged vertices — a later part growing
        // past the union aabb moves this offset, and the content hashes
        // (computed after) pick that up so residency never serves a part
        // at a stale pin.
        let mut pin = [0.0f32; 3];
        for a in 0..3 {
            pin[a] = -(scene_min[a] + scene_max[a]) / 2.0;
        }
        pin[1] = -scene_min[1];
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
            part.content_hash = content_hash(&part.vertices);
        }
        Self {
            parts: staged,
            aabb_min: scene_min,
            aabb_max: scene_max,
        }
    }
}
