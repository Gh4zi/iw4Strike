// FSR 3.1 prepare reactivity (ffx_fsr3upscaler_prepare_reactivity.h): disocclusion against last
// frame's reconstructed depth, motion divergence, shading change, how many frames each pixel
// has accumulated, and new locks on thin features (written at display resolution).

@group(0) @binding(1) var<storage, read> reconstructed_prev_depth: array<u32>;
@group(0) @binding(2) var dilated_motion: texture_2d<f32>;
@group(0) @binding(3) var dilated_depth: texture_2d<f32>;
@group(0) @binding(4) var prev_accumulation: texture_2d<f32>;
@group(0) @binding(5) var shading_change: texture_2d<f32>;
@group(0) @binding(6) var current_luma: texture_2d<f32>;
@group(0) @binding(7) var linear_clamp: sampler;
@group(0) @binding(8) var dilated_reactive_masks: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(9) var<storage, read_write> new_locks: array<f32>;
@group(0) @binding(10) var accumulation: texture_storage_2d<rgba8unorm, write>;

fn load_reconstructed_prev_depth(pos: vec2<i32>) -> f32 {
    let index = u32(pos.y) * u32(fsr.render_size.x) + u32(pos.x);
    return bitcast<f32>(reconstructed_prev_depth[index]);
}

fn compute_disocclusions(uv: vec2<f32>, motion_in: vec2<f32>, depth_view_space: f32) -> f32 {
    let nearest_in_meters = min(depth_view_space * fsr.view_space_to_meters, FP16_MAX);
    let threshold = reconstructed_depth_mv_px_threshold(nearest_in_meters);
    let motion = motion_in * f32(get_4k_velocity(motion_in) > threshold);
    let bilinear = get_bilinear_sampling_data(uv + motion, render_size());
    var disocclusion = 0.0;
    var weight_sum = 0.0;
    var potential = true;
    for (var i = 0; i < 4 && potential; i++) {
        let sample_pos = clamp_load(bilinear.base, bilinear_offset(i), render_size());
        if is_on_screen(sample_pos, render_size()) {
            let weight = bilinear.weights[i];
            if weight > RECONSTRUCTED_DEPTH_BILINEAR_WEIGHT_THRESHOLD {
                let prev_view_space = get_view_space_depth(load_reconstructed_prev_depth(sample_pos));
                let difference = depth_view_space - prev_view_space;
                potential = potential && difference > FP32_MIN;
                if potential {
                    let half_viewport_width = length(render_size_f() * 0.5);
                    let depth_threshold = max(depth_view_space, prev_view_space);
                    let k_sep = 1.37e-05;
                    let required_separation = k_sep * half_viewport_width * depth_threshold;
                    disocclusion += saturate(required_separation / difference) * weight;
                    weight_sum += weight;
                }
            }
        }
    }
    if potential && weight_sum > 0.0 {
        return saturate(1.0 - disocclusion / weight_sum);
    }
    return 0.0;
}

fn compute_motion_divergence(uv: vec2<f32>, motion: vec2<f32>, current_depth: f32) -> f32 {
    let reprojected_pos = clamp(
        vec2<i32>((uv + motion) * render_size_f()),
        vec2<i32>(0),
        render_size() - 1,
    );
    let reprojected_depth = textureLoad(dilated_depth, reprojected_pos, 0).x;
    let reprojected_motion = textureLoad(dilated_motion, reprojected_pos, 0).xy;
    let reprojected_velocity = get_4k_velocity(reprojected_motion);
    let velocity = get_4k_velocity(motion);
    // Still pixels diverge from nothing (and would divide by zero below).
    if velocity <= 0.0 {
        return 0.0;
    }
    let nucleus_in_meters = get_view_space_depth_in_meters(reprojected_depth);
    let current_in_meters = get_view_space_depth_in_meters(current_depth);
    let distance_factor = min_divided_by_max(nucleus_in_meters, current_in_meters, 0.0);
    let velocity_factor = saturate(velocity / 10.0);
    return (1.0 - saturate(reprojected_velocity / velocity)) * distance_factor * velocity_factor;
}

fn compute_thin_feature_confidence(pos: vec2<i32>) -> f32 {
    //  1 2 3
    //  4 0 5
    //  6 7 8
    var offsets = array<vec2<i32>, 9>(
        vec2<i32>(0, 0),
        vec2<i32>(-1, -1),
        vec2<i32>(0, -1),
        vec2<i32>(1, -1),
        vec2<i32>(-1, 0),
        vec2<i32>(1, 0),
        vec2<i32>(-1, 1),
        vec2<i32>(0, 1),
        vec2<i32>(1, 1),
    );
    var samples: array<f32, 9>;
    var luma_min = FP32_MAX;
    var luma_max = FP32_MIN;
    for (var i = 0; i < 9; i++) {
        let sample_pos = clamp_load(pos, offsets[i], render_size());
        samples[i] = textureLoad(current_luma, sample_pos, 0).x;
        luma_min = min(luma_min, samples[i]);
        luma_max = max(luma_max, samples[i]);
    }
    let threshold = 0.9;
    var dissimilar_min = FP32_MAX;
    var dissimilar_max = 0.0;
    // The nucleus counts as similar.
    var pattern = 1u;
    for (var i = 1; i < 9; i++) {
        let difference = abs(samples[i] - samples[0]) / (luma_max - luma_min);
        if difference < threshold {
            pattern |= 1u << u32(i);
        } else {
            dissimilar_min = min(dissimilar_min, samples[i]);
            dissimilar_max = max(dissimilar_max, samples[i]);
        }
    }
    let is_ridge = samples[0] > dissimilar_max || samples[0] < dissimilar_min;
    if !is_ridge {
        return 0.0;
    }
    // Upper left, upper right, lower left, lower right (each with the nucleus).
    var rejection = array<u32, 4>(
        (1u << 1u) | (1u << 2u) | (1u << 4u) | 1u,
        (1u << 2u) | (1u << 3u) | (1u << 5u) | 1u,
        (1u << 4u) | (1u << 6u) | (1u << 7u) | 1u,
        (1u << 5u) | (1u << 7u) | (1u << 8u) | 1u,
    );
    for (var i = 0; i < 4; i++) {
        if (pattern & rejection[i]) == rejection[i] {
            return 0.0;
        }
    }
    return 1.0 - luma_min / luma_max;
}

fn update_accumulation(
    pos: vec2<i32>,
    uv: vec2<f32>,
    motion: vec2<f32>,
    disocclusion: f32,
    shading: f32,
) -> f32 {
    let reprojected = uv + motion;
    var accumulated = 0.0;
    // A reset starts every pixel over.
    if is_uv_inside(reprojected) && fsr.frame_index > 0.0 {
        let hw_uv = clamp_uv(
            reprojected,
            fsr.prev_render_size,
            vec2<i32>(textureDimensions(prev_accumulation)),
        );
        accumulated = saturate(textureSampleLevel(prev_accumulation, linear_clamp, hw_uv, 0.0).x);
    }
    accumulated = mix(accumulated, 0.0, shading);
    accumulated = mix(
        accumulated,
        min(fsr.min_disocclusion_accumulation, accumulated),
        disocclusion,
    );
    accumulated *= f32(round(accumulated * 100.0) > 1.0);
    // Stored for next frame, normalized for unorm.
    let to_store = saturate(accumulated + fsr.accumulation_added_per_frame);
    textureStore(accumulation, pos, vec4<f32>(to_store, 0.0, 0.0, 0.0));
    return accumulated;
}

fn compute_shading_change(uv: vec2<f32>) -> f32 {
    // Jitter applied again here, removed when the accumulation samples it.
    let size = shading_change_render_size();
    let change_uv = clamp_uv(
        uv - fsr.jitter / render_size_f(),
        size,
        vec2<i32>(textureDimensions(shading_change)),
    );
    let change = textureSampleLevel(shading_change, linear_clamp, change_uv, 0.0).x;
    return saturate(change * fsr.shading_change_scale);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pos = vec2<i32>(id.xy);
    if !is_on_screen(pos, render_size()) {
        return;
    }
    let uv = (vec2<f32>(pos) + 0.5) / render_size_f();
    let motion = textureLoad(dilated_motion, pos, 0).xy;
    let depth = textureLoad(dilated_depth, pos, 0).x;
    let disocclusion = compute_disocclusions(uv, motion, get_view_space_depth(depth));
    // No reactive mask from the game: the shading change alone.
    let shading = compute_shading_change(uv);
    let divergence = compute_motion_divergence(uv, motion, depth);
    let accumulated = update_accumulation(pos, uv, motion, disocclusion, shading);
    textureStore(
        dilated_reactive_masks,
        pos,
        vec4<f32>(divergence, disocclusion, shading, accumulated),
    );
    let lock_strength = compute_thin_feature_confidence(pos);
    if lock_strength > 1.0 / 100.0 {
        let hr = compute_hr_pos_from_lr_pos(pos);
        if is_on_screen(hr, fsr.upscale_size) {
            new_locks[u32(hr.y) * u32(fsr.upscale_size.x) + u32(hr.x)] = lock_strength;
        }
    }
}
