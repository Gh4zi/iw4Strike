// A plain bilinear stretch of the internal image into the full-size frame: what shows until
// FSR's own passes are ready. `IW4L_FSR_DEBUG=motion` tints it with the motion vectors.

struct Params {
    // Output width, height and their reciprocals.
    output: vec4<f32>,
    // Render width and height; z = 1 to show the motion vectors.
    render: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var colour: texture_2d<f32>;
@group(0) @binding(2) var colour_sampler: sampler;
@group(0) @binding(3) var motion: texture_2d<f32>;
@group(0) @binding(4) var output: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= vec2<u32>(params.output.xy)) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) * params.output.zw;
    var rgb = textureSampleLevel(colour, colour_sampler, uv, 0.0).rgb;
    if params.render.z > 0.5 {
        let texel = min(vec2<i32>(uv * params.render.xy), vec2<i32>(params.render.xy) - 1);
        // Pixels moved this frame, red for x and green for y.
        let moved = abs(textureLoad(motion, texel, 0).xy * params.render.xy);
        rgb = rgb * 0.35 + vec3<f32>(clamp(moved * 0.1, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
    }
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(rgb, 1.0));
}
