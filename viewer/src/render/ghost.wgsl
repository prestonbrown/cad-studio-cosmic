struct Camera {
    view_proj: mat4x4<f32>,
    section_axis: u32,
    section_pos: f32,
    section_enable: f32,
};
@group(0) @binding(0) var<uniform> camera: Camera;

// The compare ghost (ModelViewer.swift:371-378): upstream stages the old
// version's mesh as an UnlitMaterial systemOrange wireframe at 0.6
// transparent blending — here an unlit LINE pass over the ghost scene's
// staged feature-edge list (render/edges.rs). The vertex buffer is the staged part vertex (stride
// 36) with only the position read: the ghost carries no lighting, no
// color attribute, and no per-part transform (upstream parents
// ghostEntity straight under `content`, the current scene's frame — see
// `from_parts_with_pin`). The @GHOST_@ markers are substituted from the
// Rust-side constants (see `ghost_wgsl_source` in mod.rs); a surviving
// marker is invalid WGSL and fails compilation loudly.

@TARGET_SPACE@

@vertex
fn vs_ghost(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return camera.view_proj * vec4<f32>(pos, 1.0);
}

@fragment
fn fs_ghost() -> @location(0) vec4<f32> {
    return vec4<f32>(to_target(vec3<f32>(@GHOST_R@, @GHOST_G@, @GHOST_B@)), @GHOST_ALPHA@);
}
