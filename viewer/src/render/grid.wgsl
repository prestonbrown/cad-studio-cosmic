struct Camera {
    view_proj: mat4x4<f32>,
    section_axis: u32,
    section_pos: f32,
    section_enable: f32,
};
@group(0) @binding(0) var<uniform> camera: Camera;

// Unlit overlay (facts #34): grid lines, bed outline, and tripod boxes
// carry their own linear rgba — the fragment shader is a passthrough. The
// camera uniform matches mesh.wgsl's block, so one bind group serves every
// pipeline in the pass.
struct Vin {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};
struct Vout {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_grid(in: Vin) -> Vout {
    var out: Vout;
    out.clip = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_grid(in: Vout) -> @location(0) vec4<f32> {
    return in.color;
}
