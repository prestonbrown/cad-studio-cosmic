struct Camera {
    view_proj: mat4x4<f32>,
    section_axis: u32,
    section_pos: f32,
    section_enable: f32,
};
@group(0) @binding(0) var<uniform> camera: Camera;

@vertex
fn vs_cap(@location(0) corner: vec3<f32>) -> @builtin(position) vec4<f32> {
    // `corner` is one of the 4 world-space quad vertices on the section plane,
    // built on the CPU from the frame's effective aabb (see `cap_quad` in
    // mod.rs) — project it with the same camera uniform the mesh uses.
    return camera.view_proj * vec4<f32>(corner, 1.0);
}

// The cut-face color, substituted from the Rust-side CUT_FACE_LINEAR
// constant (see `cap_wgsl_source` in mod.rs) — upstream's
// systemRed.blended(withFraction: 0.15, of: .black) through the exact
// EOTF. The @CAP_@ markers are not valid WGSL: a missed substitution fails
// shader compilation loudly.
const CAP_COLOR: vec3<f32> = vec3<f32>(@CAP_R@, @CAP_G@, @CAP_B@);
@TARGET_SPACE@

@fragment
fn fs_cap() -> @location(0) vec4<f32> {
    // upstream's blended-red cut face — LINEAR, encoded for the target
    return vec4<f32>(to_target(CAP_COLOR), 1.0);
}
