// FSR 3.1 prepare inputs (ffx_fsr3upscaler_prepare_inputs.h): the nearest depth and its motion
// vector around each pixel, last frame's depth reconstructed by pushing this frame's along the
// motion, the farthest depth in meters and the pixel's luma.

@group(0) @binding(1) var input_depth: texture_depth_2d;
@group(0) @binding(2) var input_colour: texture_2d<f32>;
@group(0) @binding(3) var input_motion: texture_2d<f32>;
@group(0) @binding(4) var dilated_motion: texture_storage_2d<rg32float, write>;
@group(0) @binding(5) var dilated_depth: texture_storage_2d<r32float, write>;
@group(0) @binding(6) var<storage, read_write> reconstructed_prev_depth: array<atomic<u32>>;
@group(0) @binding(7) var farthest_depth: texture_storage_2d<r32float, write>;
@group(0) @binding(8) var current_luma: texture_storage_2d<rgba16float, write>;

fn load_input_depth(pos: vec2<i32>) -> f32 {
    return textureLoad(input_depth, clamp(pos, vec2<i32>(0), render_size() - 1), 0);
}

fn store_reconstructed_depth(pos: vec2<i32>, depth: f32) {
    // Inverted depth: the nearest is the largest, and positive floats order like their bits.
    let index = u32(pos.y) * u32(fsr.render_size.x) + u32(pos.x);
    atomicMax(&reconstructed_prev_depth[index], bitcast<u32>(depth));
}

fn reconstruct_prev_depth(pos: vec2<i32>, depth: f32, motion_in: vec2<f32>) {
    let nearest_in_meters = min(get_view_space_depth_in_meters(depth), FP16_MAX);
    let threshold = reconstructed_depth_mv_px_threshold(nearest_in_meters);
    // Discard small motion.
    let motion = motion_in * f32(get_4k_velocity(motion_in) > threshold);
    let uv = (vec2<f32>(pos) + 0.5) / render_size_f();
    let bilinear = get_bilinear_sampling_data(uv + motion, render_size());
    // Push to every pixel the bilinear reprojection would read from.
    for (var i = 0; i < 4; i++) {
        if bilinear.weights[i] > RECONSTRUCTED_DEPTH_BILINEAR_WEIGHT_THRESHOLD {
            let store = bilinear.base + bilinear_offset(i);
            if is_on_screen(store, render_size()) {
                store_reconstructed_depth(store, depth);
            }
        }
    }
}

const DEPTH_OFFSETS = array<vec2<i32>, 9>(
    vec2<i32>(0, 0),
    vec2<i32>(1, 0),
    vec2<i32>(0, 1),
    vec2<i32>(0, -1),
    vec2<i32>(-1, 0),
    vec2<i32>(-1, 1),
    vec2<i32>(1, 1),
    vec2<i32>(-1, -1),
    vec2<i32>(1, -1),
);

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pos = vec2<i32>(id.xy);
    if !is_on_screen(pos, render_size()) {
        return;
    }
    // Nearest (largest, inverted) and farthest depth of the 3x3 neighbourhood.
    var nearest = load_input_depth(pos);
    var nearest_pos = pos;
    var farthest = nearest;
    var offsets = DEPTH_OFFSETS;
    for (var i = 1; i < 9; i++) {
        let sample_pos = pos + offsets[i];
        if is_on_screen(sample_pos, render_size()) {
            let depth = load_input_depth(sample_pos);
            if depth > nearest {
                farthest = min(farthest, depth);
                nearest_pos = sample_pos;
                nearest = depth;
            }
        }
    }
    // Motion vectors are at render resolution: take the nearest pixel's.
    let motion = textureLoad(input_motion, nearest_pos, 0).xy;
    reconstruct_prev_depth(pos, nearest, motion);
    textureStore(dilated_motion, pos, vec4<f32>(motion, 0.0, 0.0));
    textureStore(dilated_depth, pos, vec4<f32>(nearest, 0.0, 0.0, 0.0));
    let farthest_in_meters = min(get_view_space_depth_in_meters(farthest), FP16_MAX);
    textureStore(farthest_depth, pos, vec4<f32>(farthest_in_meters, 0.0, 0.0, 0.0));
    let rgb = max(vec3<f32>(0.0), srgb_to_linear(textureLoad(input_colour, pos, 0).rgb));
    textureStore(current_luma, pos, vec4<f32>(rgb_to_luma(rgb), 0.0, 0.0, 0.0));
}
