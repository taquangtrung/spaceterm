struct VertexInput {
    @location(0) position: vec2f,
    @location(1) uv: vec2f,
}

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4f(in.position, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

// The texture stores sRGB color; decode to linear so the sRGB surface re-encodes
// it unchanged and the overlay's alpha blends gamma-correctly. Alpha is linear.
fn srgb_to_linear(c: vec3f) -> vec3f {
    let cutoff = c <= vec3f(0.04045);
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3f(2.4));
    return select(high, low, cutoff);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let c = textureSample(tex, samp, in.uv);
    return vec4f(srgb_to_linear(c.rgb), c.a);
}
