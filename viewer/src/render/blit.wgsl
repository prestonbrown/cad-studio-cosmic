// Blit for the msaa resolve path: draw the resolved intermediate over the
// destination with premultiplied alpha (the resolve output's rgb is already
// weighted by sample coverage — uncovered samples were transparent black —
// so One/OneMinusSrcAlpha composites coverage exactly; SrcAlpha would apply
// the coverage weighting twice and darken edge pixels).
@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex: texture_2d<f32>;

struct Vout {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) vi: u32) -> Vout {
    // fullscreen triangle: (-1,-1), (3,-1), (-1,3) in clip space
    let corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let xy = corners[vi];
    var out: Vout;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    // NDC is y-up, texture rows run top-down: flip v
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
    return out;
}

@fragment
fn fs_blit(in: Vout) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
