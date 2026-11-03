struct Camera {
    view_proj: mat4x4<f32>,
    section_axis: u32,
    section_pos: f32,
    section_enable: f32,
};
@group(0) @binding(0) var<uniform> camera: Camera;

// per-part model transform + motion appearance: one bind group per part
// draw (identity today; explode offsets, playback poses, and the
// collision/translucency flags all go through this same binding — motion
// is uniform state, never a re-stage)
struct Model {
    transform: mat4x4<f32>,
    collision: f32,
    alpha: f32,
    // uniform structs round up to 16-byte boundaries: 64 + 4 + 4 = 72 pads
    // to 80, matching the Rust repr exactly (bind-group validation checks
    // min_binding_size)
    _pad: vec2<f32>,
};
@group(1) @binding(0) var<uniform> model: Model;

// Parity shading (ModelViewer.swift:344-353, 415-423): the `lights`
// module's fitted model of upstream's PhysicallyBasedMaterial under its key
// and fill lights plus RealityView's environment —
// shoulder(D(n)·base + S(n, v)). The @NAME@ markers are substituted from
// the Rust-side `lights` module at pipeline creation: one definition
// shared by the shader and its Rust twin `lights::shade` (a surviving
// marker is invalid WGSL and fails compilation loudly). The COLLISION_@
// markers follow the same contract from COLLISION_LINEAR.
const KEY_POS: vec3<f32> = @KEY_POS@;
const KEY_COEFF: f32 = @KEY_COEFF@;
const FILL_POS: vec3<f32> = @FILL_POS@;
const FILL_COEFF: f32 = @FILL_COEFF@;
const ENV_DIR: vec3<f32> = @ENV_DIR@;
const ENV_COEFF: f32 = @ENV_COEFF@;
const AMBIENT: f32 = @AMBIENT@;
const SPECULAR: f32 = @SPECULAR@;
const SHININESS: f32 = @SHININESS@;
const SHOULDER_START: f32 = @SHOULDER_START@;
const COLLISION_R: f32 = @COLLISION_R@;
const COLLISION_G: f32 = @COLLISION_G@;
const COLLISION_B: f32 = @COLLISION_B@;
@TARGET_SPACE@

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

// The direction toward the camera: a perspective view-projection's w row
// is the camera's forward axis (clip w = view depth), so the column-major
// matrix's w components hold it without a separate uniform.
fn to_eye() -> vec3<f32> {
    let vp = camera.view_proj;
    return -normalize(vec3<f32>(vp[0].w, vp[1].w, vp[2].w));
}

// lights::shoulder: identity below SHOULDER_START, then a roll-off
// toward 1.
fn shoulder(c: f32) -> f32 {
    let d = 1.0 - SHOULDER_START;
    return select(max(c, 0.0), 1.0 - d * d / (c + d - SHOULDER_START), c >= SHOULDER_START);
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
    let key = normalize(KEY_POS);
    let n_key = max(dot(n, key), 0.0);
    let diffuse = AMBIENT
            + KEY_COEFF * n_key
            + FILL_COEFF * max(dot(n, normalize(FILL_POS)), 0.0)
            + ENV_COEFF * max(dot(n, normalize(ENV_DIR)), 0.0);
    let h = normalize(key + to_eye());
    let spec = SPECULAR * pow(max(dot(n, h), 1e-6), SHININESS) * n_key;
    if (model.collision > 0.5) {
        // upstream's collision material: emissive systemRed — flat from
        // every normal, opaque even on a would-be-translucent part
        return vec4<f32>(to_target(vec3<f32>(COLLISION_R, COLLISION_G, COLLISION_B)), 1.0);
    }
    let lit = in.color * diffuse + vec3<f32>(spec);
    return vec4<f32>(to_target(vec3<f32>(shoulder(lit.r), shoulder(lit.g), shoulder(lit.b))), model.alpha);
}
