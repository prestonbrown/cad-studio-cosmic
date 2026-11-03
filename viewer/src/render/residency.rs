//! GPU residency: what each staged part, overlay, and ghost part holds on
//! the device, keyed by the content identity that decides re-upload.

use std::hash::Hasher;

use wgpu::util::DeviceExt;

use super::{GridVertex, PartScene};

/// A part's GPU residency: its vertex buffer, draw count, and the content
/// identity the buffer currently holds — the transform is NOT part of it
/// (poses write the model uniform per frame). Built by `prepare_parts`
/// when a part's hash differs; untouched otherwise.
pub(super) struct ResidentPart {
    pub(super) content_hash: u64,
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) vertex_count: u32,
    /// The wireframe line list as a u32 index buffer into `vertex_buf`
    /// (2 indices per edge) and its index count.
    pub(super) edge_buf: wgpu::Buffer,
    pub(super) edge_count: u32,
    /// Group 1 for this part's draws: the model transform + appearance.
    pub(super) model_bg: wgpu::BindGroup,
    /// The model uniform behind `model_bg` — every prepare writes the
    /// frame's pose and appearance into it (80 B), the whole per-frame
    /// cost of moving or recoloring a part.
    pub(super) model_buf: wgpu::Buffer,
}

/// An overlay's GPU residency — the same content-identity contract as
/// [`ResidentPart`], over the unlit meshes staged by [`grid::geometry`]
/// and [`markers::geometry`].
pub(super) struct ResidentGrid {
    pub(super) content_hash: u64,
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) vertex_count: u32,
}

/// Content-hash-keyed overlay residency — the staging step both overlay
/// slots (`grid`, `marker`) share: (re)build the vertex buffer only when
/// the content identity differs from what the slot holds; `None` drops
/// the slot entirely. `label` names the buffer for debuggers.
pub(super) fn stage_overlay(
    slot: &mut Option<ResidentGrid>,
    device: &wgpu::Device,
    mesh: Option<(u64, &[GridVertex], &str)>,
) {
    if let Some((content_hash, vertices, label)) = mesh {
        if !slot
            .as_ref()
            .is_some_and(|r| r.content_hash == content_hash)
        {
            *slot = Some(ResidentGrid {
                content_hash,
                vertex_buf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("{label} vertices")),
                    contents: bytemuck::cast_slice(vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                vertex_count: vertices.len() as u32,
            });
        }
    } else {
        *slot = None;
    }
}

/// A ghost part's GPU residency — [`ResidentPart`] minus the model
/// uniform: the ghost draws through the camera-only layout (upstream
/// parents ghostEntity straight under `content`, so there is no per-part
/// pose to carry). The vertex buffer holds the ghost scene's staged
/// bytes; the edge index buffer is the line list this part draws.
pub(super) struct ResidentGhostPart {
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) edge_buf: wgpu::Buffer,
    pub(super) edge_count: u32,
}

/// Whole-scene identity of a ghost [`PartScene`]: the part count and every
/// part's content hash — the key that stands in for upstream's ghostID
/// swap (ModelViewer.swift:171-174 restages the ghost when ghostID or the
/// mesh changes). The per-part identities are already computed at staging
/// (pin included), so this is O(parts), not O(vertices).
pub(super) fn ghost_identity(scene: &PartScene) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_usize(scene.parts.len());
    for part in &scene.parts {
        h.write_u64(part.content_hash);
    }
    h.finish()
}
