struct VertexInput {
    @location(0) position: vec2f,
    @location(1) color: vec3f,
}

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) color: vec3f,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4f(in.position, 0.0, 1.0);
    out.color = in.color;
    return out;
}

// Decode an sRGB color to linear. The surface is sRGB, so the GPU re-encodes
// this on write; emitting linear keeps blending gamma-correct while the displayed
// color stays the same.
fn srgb_to_linear(c: vec3f) -> vec3f {
    let cutoff = c <= vec3f(0.04045);
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3f(2.4));
    return select(high, low, cutoff);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    return vec4f(srgb_to_linear(in.color), 1.0);
}
