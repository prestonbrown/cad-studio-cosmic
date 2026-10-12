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

// Two-light parity shading (facts §1, ModelViewer.swift:344-353): upstream
// lights are directional — key 2600 from (60,120,90), fill 900 from
// (-90,40,-60), both SCENE space — approximated in-shader as
// intensity/10⁴ diffuse coefficients over the vertex normal plus ambient,
// clamped. The @NAME@ markers are substituted from the Rust-side `lights`
// module at pipeline creation: one definition shared by the shader and the
// headless shading tests (a surviving marker is invalid WGSL and fails
// compilation loudly).
const KEY_POS: vec3<f32> = @KEY_POS@;
const KEY_COEFF: f32 = @KEY_COEFF@;
const FILL_POS: vec3<f32> = @FILL_POS@;
const FILL_COEFF: f32 = @FILL_COEFF@;
const AMBIENT: f32 = @AMBIENT@;
const LIT_MAX: f32 = @LIT_MAX@;

struct Vin {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};
struct Vout {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) world: vec3<f32>,
    @location(2) normal: vec3<f32>,
};

@vertex
fn vs_main(in: Vin) -> Vout {
    var out: Vout;
    let world = model.transform * vec4<f32>(in.pos, 1.0);
    out.clip = camera.view_proj * world;
    out.color = in.color;
    out.world = world.xyz;
    // w = 0 rotates the normal without translating it — correct for the
    // rigid part transforms explode/playback will send through `model`
    out.normal = (model.transform * vec4<f32>(in.normal, 0.0)).xyz;
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
    // staged normals are unit and `model` is identity today; normalize is
    // insurance for scaled future transforms
    let n = normalize(in.normal);
    let lit = KEY_COEFF * max(dot(n, normalize(KEY_POS)), 0.0)
            + FILL_COEFF * max(dot(n, normalize(FILL_POS)), 0.0)
            + AMBIENT;
    return vec4<f32>(in.color * clamp(lit, 0.0, LIT_MAX), 1.0);
}
