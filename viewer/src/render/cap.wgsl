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
    // built on the CPU from the mesh aabb (see `cap_quad` in mod.rs) — project
    // it with the same camera uniform the mesh uses.
    return camera.view_proj * vec4<f32>(corner, 1.0);
}

@fragment
fn fs_cap() -> @location(0) vec4<f32> {
    return vec4<f32>(0.92, 0.15, 0.10, 1.0); // upstream red cut face
}
