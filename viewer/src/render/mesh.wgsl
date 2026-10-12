struct Camera {
    view_proj: mat4x4<f32>,
    section_axis: u32,
    section_pos: f32,
    section_enable: f32,
};
@group(0) @binding(0) var<uniform> camera: Camera;

// per-part model transform: one bind group per part draw (identity today;
// explode offsets and playback poses go through this same binding)
struct Model {
    transform: mat4x4<f32>,
};
@group(1) @binding(0) var<uniform> model: Model;

struct Vin {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};
struct Vout {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) world: vec3<f32>,
};

@vertex
fn vs_main(in: Vin) -> Vout {
    var out: Vout;
    let world = model.transform * vec4<f32>(in.pos, 1.0);
    out.clip = camera.view_proj * world;
    out.color = in.color;
    out.world = world.xyz;
    return out;
}

@fragment
fn fs_main(in: Vout) -> @location(0) vec4<f32> {
    if (camera.section_enable > 0.5) {
        var v = in.world.y;
        if (camera.section_axis == 0u) { v = in.world.x; }
        if (camera.section_axis == 2u) { v = in.world.z; }
        if (v > camera.section_pos) { discard; }
    }
    // upstream lights (upstream-contract §5): key 2600 @ (60,120,90), fill 900 @ (-90,40,-60)
    let l1 = normalize(vec3<f32>(60.0, 120.0, 90.0));
    let l2 = normalize(vec3<f32>(-90.0, 40.0, -60.0));
    // spike: screen-space normal (dpdx/dpdy); M3 uploads real mesh normals
    let n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    let lit = 0.26 * max(dot(n, l1), 0.0) + 0.09 * max(dot(n, l2), 0.0) + 0.15;
    return vec4<f32>(in.color * clamp(lit, 0.0, 1.2), 1.0);
}
