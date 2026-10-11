// FSR's motion vectors, from the scene depth and the camera: where each pixel's point was on
// screen last frame, as a UV offset (previous minus current). Only the camera moves the world
// here; what moves on its own is left to FSR's own disocclusion and shading-change checks.

struct Params {
    // All relative to this frame's eye, to keep precision far from the map origin.
    // This frame's jittered projection, inverted: depth-buffer pixel to point.
    rel_from_clip: mat4x4<f32>,
    // This frame's and last frame's projections without the jitter.
    clip_from_rel: mat4x4<f32>,
    prev_clip_from_rel: mat4x4<f32>,
    // Render width, height and their reciprocals.
    size: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var motion: texture_storage_2d<rg32float, write>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(params.size.xy);
    if any(id.xy >= size) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) * params.size.zw;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    // Reverse-Z with an infinite far plane: the sky (depth 0) is a direction, w = 0.
    let depth = textureLoad(depth_tex, vec2<i32>(id.xy), 0);
    let point = params.rel_from_clip * vec4<f32>(ndc, depth, 1.0);
    let now = params.clip_from_rel * point;
    let then = params.prev_clip_from_rel * point;
    var offset = vec2<f32>(0.0);
    if now.w > 1e-7 && then.w > 1e-7 {
        offset = (then.xy / then.w - now.xy / now.w) * vec2<f32>(0.5, -0.5);
    }
    textureStore(motion, vec2<i32>(id.xy), vec4<f32>(offset, 0.0, 0.0));
}
