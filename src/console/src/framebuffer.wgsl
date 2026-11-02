@group(0) @binding(0) var framebuffer: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Vertex(vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0), uv);
}

@fragment fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(framebuffer, linear_sampler, input.uv).rgb, 1.0);
}
