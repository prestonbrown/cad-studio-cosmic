//! Part-keyed CPU scene: engine parts staged as scene-space vertices.
//!
//! MODEL (engine coords, Z-up) becomes SCENE (render coords, Y-up) through
//! `(x, z, -y)` — a proper rotation, applied once here at scene build.
//! Each part carries a full-content hash over positions+colors, so the
//! pipeline's upload-once check (`MeshPipeline::prepare_parts`) rewrites a
//! part's vertex buffer only when its real content changes; camera motion
//! writes uniforms, never mesh bytes.

use std::hash::Hasher;

use engine::parsing::Part;

use super::Vertex;

/// The app's fixed neutral face color (upstream-contract §5). `from_parts`
/// paints it onto the mesh; it is a parameter so a theme-supplied neutral
/// can take its place.
pub const NEUTRAL: [f32; 3] = [0.62, 0.70, 0.80];

/// One engine part staged for upload: scene-space per-corner vertices, its
/// aabb, and the content identity the upload-once check compares.
#[derive(Clone, Debug)]
pub struct GpuPart {
    pub name: String,
    /// One vertex per corner (unindexed, per-face).
    pub vertices: Vec<Vertex>,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// Full-content identity over positions+colors: a moved corner changes
    /// it as surely as an added one, so residency tracks content, not
    /// counts or endpoints.
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

/// Content identity of one part: every position byte and every triangle
/// color feeds the hash (O(n), run once per `PartScene` build). Valid only
/// for comparing scenes built in the same process.
fn content_hash(part: &Part) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_usize(part.positions.len());
    h.write(bytemuck::cast_slice(&part.positions));
    for color in &part.triangle_colors {
        match color {
            None => h.write_u8(0),
            Some(rgb) => {
                h.write_u8(1);
                h.write(rgb);
            }
        }
    }
    h.finish()
}

/// MODEL Z-up -> SCENE Y-up, upstream's `sceneVector`: a proper rotation.
fn remap(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}

impl PartScene {
    /// Stage engine parts for rendering: remap to scene space, paint the
    /// neutral, hash each part's content. Parts keep engine order; slot `i`
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
            // mesh.wgsl derives normals in screen space and never reads the
            // normal attribute today
            let vertices = part
                .positions
                .iter()
                .map(|&p| Vertex {
                    pos: remap(p),
                    normal: [0.0; 3],
                    color: neutral,
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
                content_hash: content_hash(part),
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
            // camera math
            (scene_min, scene_max) = ([0.0; 3], [0.0; 3]);
        }
        Self {
            parts: staged,
            aabb_min: scene_min,
            aabb_max: scene_max,
        }
    }
}
