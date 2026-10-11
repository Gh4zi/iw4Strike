// FSR 3.1 accumulation (ffx_fsr3upscaler_accumulate.h, _upsample.h, _reproject.h, _sample.h):
// for each output pixel, last frame's upscaled colour reprojected with a Lanczos filter, this
// frame's samples gathered with a jitter-aware Lanczos kernel, the history clamped to a
// variance box of this frame's neighbourhood, and the two blended by how much history the
// pixel has earned. Writes the new history (with its lock) and, without RCAS, the output.

@group(0) @binding(1) var dilated_reactive_masks: texture_2d<f32>;
@group(0) @binding(2) var dilated_motion: texture_2d<f32>;
@group(0) @binding(3) var prev_upscaled: texture_2d<f32>;
@group(0) @binding(4) var farthest_depth_mip1: texture_2d<f32>;
@group(0) @binding(5) var luma_instability: texture_2d<f32>;
@group(0) @binding(6) var input_colour: texture_2d<f32>;
@group(0) @binding(7) var linear_clamp: sampler;
@group(0) @binding(8) var upscaled: texture_storage_2d<rgba16float, write>;
@group(0) @binding(9) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(10) var<storage, read_write> new_locks: array<f32>;

struct Params {
    hr_pos: vec2<i32>,
    hr_uv: vec2<f32>,
    lr_uv_jittered: vec2<f32>,
    lr_uv_hw: vec2<f32>,
    motion: vec2<f32>,
    reprojected_hr_uv: vec2<f32>,
    velocity_4k: f32,
    disocclusion: f32,
    reactive: f32,
    shading_change: f32,
    accumulation: f32,
    luma_instability: f32,
    farthest_depth_in_meters: f32,
    is_existing_sample: bool,
    is_new_sample: bool,
}

struct Box {
    center: vec3<f32>,
    vec: vec3<f32>,
    aabb_min: vec3<f32>,
    aabb_max: vec3<f32>,
    center_weight: f32,
}

struct Data {
    clipping: Box,
    upsampled_color: vec3<f32>,
    upsampled_weight: f32,
    history_color: vec3<f32>,
    history_weight: f32,
    lock: f32,
    lock_contribution: f32,
}

fn lock_index(pos: vec2<i32>) -> u32 {
    return u32(pos.y) * u32(fsr.upscale_size.x) + u32(pos.x);
}

// ---- Lanczos (ffx_fsr3upscaler_sample.h)

const PI: f32 = 3.141592653589793;

fn lanczos2(x_in: f32) -> f32 {
    let x = min(abs(x_in), 2.0);
    if abs(x) < EPSILON {
        return 1.0;
    }
    return (sin(PI * x) / (PI * x)) * (sin(0.5 * PI * x) / (0.5 * PI * x));
}

fn lanczos2_approx_sq(x2_in: f32) -> f32 {
    let x2 = min(x2_in, 4.0);
    let a = (2.0 / 5.0) * x2 - 1.0;
    let b = (1.0 / 4.0) * x2 - 1.0;
    return ((25.0 / 16.0) * a * a - (25.0 / 16.0 - 1.0)) * (b * b);
}

fn lanczos2_row(c0: vec4<f32>, c1: vec4<f32>, c2: vec4<f32>, c3: vec4<f32>, t: f32) -> vec4<f32> {
    let w0 = lanczos2(-1.0 - t);
    let w1 = lanczos2(-0.0 - t);
    let w2 = lanczos2(1.0 - t);
    let w3 = lanczos2(2.0 - t);
    return (w0 * c0 + w1 * c1 + w2 * c2 + w3 * c3) / (w0 + w1 + w2 + w3);
}

// Clamp by offset direction (the sample itself is already in range).
fn clamp_coord(pos: vec2<i32>, offset: vec2<i32>, size: vec2<i32>) -> vec2<i32> {
    return clamp(pos + offset, vec2<i32>(1), size - 2);
}

fn load_history(pos: vec2<i32>, size: vec2<i32>, offset: vec2<i32>) -> vec4<f32> {
    return textureLoad(prev_upscaled, clamp_coord(pos, offset, size), 0);
}

// Last frame's upscaled colour at a UV: 4x4 Lanczos, deringed by the four nearest texels.
fn history_sample(uv: vec2<f32>, size: vec2<i32>) -> vec4<f32> {
    var px = uv * vec2<f32>(size) - 0.5;
    let frac = fract(px);
    px = clamp(px, vec2<f32>(0.0), vec2<f32>(size - 1));
    let p = vec2<i32>(floor(px));
    var rows: array<vec4<f32>, 4>;
    for (var y = 0; y < 4; y++) {
        rows[y] = lanczos2_row(
            load_history(p, size, vec2<i32>(-1, y - 1)),
            load_history(p, size, vec2<i32>(0, y - 1)),
            load_history(p, size, vec2<i32>(1, y - 1)),
            load_history(p, size, vec2<i32>(2, y - 1)),
            frac.x,
        );
    }
    var colour = lanczos2_row(rows[0], rows[1], rows[2], rows[3], frac.y);
    let c11 = load_history(p, size, vec2<i32>(0, 0));
    let c21 = load_history(p, size, vec2<i32>(1, 0));
    let c12 = load_history(p, size, vec2<i32>(0, 1));
    let c22 = load_history(p, size, vec2<i32>(1, 1));
    let low = min(min(c11, c21), min(c12, c22));
    let high = max(max(c11, c21), max(c12, c22));
    return clamp(colour, low, high);
}

// ---- Rectification box (ffx_fsr3upscaler_common.h)

fn box_add_sample(box_in: Box, initial: bool, colour: vec3<f32>, weight: f32) -> Box {
    var b = box_in;
    if initial {
        b.aabb_min = colour;
        b.aabb_max = colour;
        let weighted = colour * weight;
        b.center = weighted;
        b.vec = colour * weighted;
        b.center_weight = weight;
    } else {
        b.aabb_min = min(b.aabb_min, colour);
        b.aabb_max = max(b.aabb_max, colour);
        let weighted = colour * weight;
        b.center += weighted;
        b.vec += colour * weighted;
        b.center_weight += weight;
    }
    return b;
}

fn box_compute_variance(box_in: Box) -> Box {
    var b = box_in;
    b.center_weight = select(1.0, b.center_weight, abs(b.center_weight) > FP32_MIN);
    b.center /= b.center_weight;
    b.vec /= b.center_weight;
    b.vec = sqrt(abs(b.vec - b.center * b.center));
    return b;
}

// ---- Passes

fn load_prepared_colour(pos: vec2<i32>) -> vec3<f32> {
    let rgb = max(vec3<f32>(0.0), srgb_to_linear(textureLoad(input_colour, pos, 0).rgb));
    return rgb_to_ycocg(rgb);
}

fn init_params(hr_pos: vec2<i32>) -> Params {
    var params: Params;
    params.hr_pos = hr_pos;
    let hr_uv = (vec2<f32>(hr_pos) + 0.5) / vec2<f32>(fsr.upscale_size);
    params.hr_uv = hr_uv;
    params.lr_uv_jittered = hr_uv + fsr.jitter / render_size_f();
    let resource_size = vec2<i32>(textureDimensions(dilated_reactive_masks));
    params.lr_uv_hw = clamp_uv(params.lr_uv_jittered, render_size(), resource_size);
    // Low-resolution motion vectors: the dilated one under this output pixel.
    let motion_pos = clamp(vec2<i32>(hr_uv * render_size_f()), vec2<i32>(0), render_size() - 1);
    params.motion = textureLoad(dilated_motion, motion_pos, 0).xy;
    params.velocity_4k = get_4k_velocity(params.motion);
    params.reprojected_hr_uv = hr_uv + params.motion;
    params.is_existing_sample = is_uv_inside(params.reprojected_hr_uv);
    let instability_uv = clamp_uv(hr_uv, render_size(), resource_size);
    params.luma_instability =
        textureSampleLevel(luma_instability, linear_clamp, instability_uv, 0.0).x;
    let farthest_uv = clamp_uv(
        params.lr_uv_jittered,
        render_size() / 2,
        vec2<i32>(textureDimensions(farthest_depth_mip1)),
    );
    params.farthest_depth_in_meters =
        textureSampleLevel(farthest_depth_mip1, linear_clamp, farthest_uv, 0.0).x;
    params.is_new_sample = !params.is_existing_sample || fsr.frame_index == 0.0;
    let masks = textureSampleLevel(dilated_reactive_masks, linear_clamp, params.lr_uv_hw, 0.0);
    params.reactive = saturate(masks[REACTIVE]);
    params.disocclusion = saturate(masks[DISOCCLUSION]);
    params.shading_change = saturate(masks[SHADING_CHANGE]);
    params.accumulation = saturate(masks[ACCUMULATION]);
    params.accumulation *= f32(round(params.accumulation * 100.0) > 1.0);
    return params;
}

fn reproject_history_colour(params: Params, data: ptr<function, Data>) {
    let history = history_sample(params.reprojected_hr_uv, fsr.prev_upscale_size);
    (*data).history_color = rgb_to_ycocg(history.rgb);
    (*data).lock = history.w;
}

fn update_lock_status(params: Params, data: ptr<function, Data>) {
    var lock = (*data).lock * f32(!params.is_new_sample);
    let decrease_factor = max(saturate(params.shading_change), max(params.reactive, params.disocclusion));
    lock = max(0.0, lock - decrease_factor * LOCK_MAX);
    // This frame's lock contribution.
    (*data).lock_contribution = saturate(saturate(lock - LOCK_THRESHOLD) * (LOCK_MAX - LOCK_THRESHOLD));
    let new_lock = new_locks[lock_index(params.hr_pos)] * (1.0 - max(0.0, params.reactive));
    lock = max(0.0, min(lock + new_lock, LOCK_MAX));
    // Preparing for next frame.
    let lifetime_decrease = (0.1 / fsr.jitter_sequence_length) * (1.0 - decrease_factor);
    lock = max(0.0, lock - lifetime_decrease);
    // Kill locks expected to leave the screen next frame.
    let next_uv = params.hr_uv - params.motion;
    lock *= f32(is_uv_inside(next_uv));
    (*data).lock = lock;
}

fn compute_base_accumulation_weight(params: Params, data: ptr<function, Data>) {
    var base = params.accumulation;
    base = min(base, mix(base, 0.15, saturate(max(0.0, (params.velocity_4k * fsr.velocity_factor) / 0.5))));
    (*data).history_weight = base;
}

fn compute_upsampled_colour_and_weight(params: Params, data: ptr<function, Data>) {
    // A sliced 2-lobe Lanczos (the other slices accumulate over time).
    let dst_pos = vec2<f32>(params.hr_pos) + 0.5;
    let src_pos = dst_pos * fsr.downscale_factor;
    let src_input = vec2<i32>(floor(src_pos));
    // The un-jittered position of the sample at offset (0, 0).
    let src_unjittered = vec2<f32>(src_input) + 0.5 - fsr.jitter;
    let base_offset = src_unjittered - src_pos;
    let flip_col = src_unjittered.x > src_pos.x;
    let flip_row = src_unjittered.y > src_pos.y;
    let offset_tl = vec2<i32>(select(-1, -2, flip_col), select(-1, -2, flip_row));
    let is_initial = params.accumulation == 0.0;

    var samples: array<vec3<f32>, 9>;
    for (var row = 0; row < 3; row++) {
        for (var col = 0; col < 3; col++) {
            let col_row = vec2<i32>(select(col, 3 - col, flip_col), select(row, 3 - row, flip_row));
            let sample_pos = src_input + offset_tl + col_row;
            samples[row * 3 + col] = load_prepared_colour(clamp_load(sample_pos, vec2<i32>(0), render_size()));
        }
    }

    let kernel_bias_max = min(1.99, 1.0 + (1.0 / fsr.downscale_factor.x - 1.0));
    let kernel_bias_min = max(1.0, (1.0 + kernel_bias_max) * 0.3);
    let kernel_bias_weight = min(
        1.0 - params.disocclusion * 0.5,
        min(1.0 - params.shading_change, saturate((*data).history_weight * 5.0)),
    );
    let kernel_bias = mix(kernel_bias_min, kernel_bias_max, kernel_bias_weight);

    var upsampled = vec3<f32>(0.0);
    var upsampled_weight = 0.0;
    var clipping: Box;
    for (var row = 0; row < 3; row++) {
        for (var col = 0; col < 3; col++) {
            let col_row = vec2<i32>(select(col, 3 - col, flip_col), select(row, 3 - row, flip_row));
            let offset = vec2<f32>(offset_tl) + vec2<f32>(col_row);
            let sample_offset = base_offset + offset;
            let sample_pos = src_input + offset_tl + col_row;
            let on_screen = f32(is_on_screen(sample_pos, render_size()));
            let colour = samples[row * 3 + col];
            if !is_initial {
                let biased = sample_offset * kernel_bias;
                let weight = on_screen * lanczos2_approx_sq(dot(biased, biased));
                upsampled += colour * weight;
                upsampled_weight += weight;
            }
            // Rectification box.
            let box_weight = exp(-2.3 * dot(sample_offset, sample_offset)) * on_screen;
            clipping = box_add_sample(clipping, row == 0 && col == 0, colour, box_weight);
        }
    }
    clipping = box_compute_variance(clipping);
    upsampled_weight *= f32(upsampled_weight > EPSILON);
    if upsampled_weight > EPSILON {
        // Normalized for deringing (colours are compared).
        upsampled /= upsampled_weight;
        upsampled_weight *= AVERAGE_LANCZOS_WEIGHT_PER_FRAME;
        upsampled = clamp(upsampled, clipping.aabb_min, clipping.aabb_max);
    }
    if is_initial {
        upsampled = clipping.center;
        upsampled_weight = 1.0;
        (*data).history_weight = 0.0;
    }
    (*data).clipping = clipping;
    (*data).upsampled_color = upsampled;
    (*data).upsampled_weight = upsampled_weight;
}

fn rectify_history(params: Params, data: ptr<function, Data>) {
    let velocity_factor = saturate(params.velocity_4k / 20.0);
    let distance_factor = saturate(0.75 - params.farthest_depth_in_meters / 20.0);
    let accumulation_factor = 1.0 - params.accumulation;
    let reactive_factor = sqrt(params.reactive);
    let shading_factor = params.shading_change;
    let t = max(velocity_factor, max(distance_factor, max(accumulation_factor, max(reactive_factor, shading_factor))));
    let box_scale = mix(3.0, 1.0, t);
    let clipping = (*data).clipping;
    let scaled = clipping.vec * vec3<f32>(1.7, 1.0, 1.0) * box_scale;
    let clamped_scaled = max(scaled, vec3<f32>(1.193e-7));
    let transformed = ((*data).history_color - clipping.center) / clamped_scaled;
    if length(transformed) > 1.0 {
        let clamped = normalize(transformed) * scaled + clipping.center;
        // Keep some history where the lock or the flicker detection trusts it.
        let contribution = max(params.luma_instability, (*data).lock_contribution)
            * params.accumulation * (1.0 - params.disocclusion);
        (*data).history_color = mix(clamped, (*data).history_color, saturate(contribution));
    }
}

fn accumulate(data: ptr<function, Data>) {
    // Avoid invalid values when accumulation and upsampled weight are 0.
    var history_weight = (*data).history_weight * f32((*data).history_weight > FP16_MIN);
    history_weight = max(EPSILON, history_weight + (*data).upsampled_weight);
    (*data).history_weight = history_weight;
    let alpha = saturate((*data).upsampled_weight / history_weight);
    (*data).history_color = ycocg_to_rgb(mix((*data).history_color, (*data).upsampled_color, alpha));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let hr_pos = vec2<i32>(id.xy);
    if !is_on_screen(hr_pos, fsr.upscale_size) {
        return;
    }
    let params = init_params(hr_pos);
    var data: Data;
    data.upsampled_color = vec3<f32>(0.0);
    data.history_color = vec3<f32>(0.0);
    data.history_weight = 1.0;
    data.upsampled_weight = 0.0;
    data.lock = 0.0;
    data.lock_contribution = 0.0;
    if params.is_existing_sample && !params.is_new_sample {
        reproject_history_colour(params, &data);
    }
    update_lock_status(params, &data);
    compute_base_accumulation_weight(params, &data);
    compute_upsampled_colour_and_weight(params, &data);
    rectify_history(params, &data);
    accumulate(&data);
    let colour = max(data.history_color, vec3<f32>(0.0));
    textureStore(upscaled, hr_pos, vec4<f32>(colour, data.lock));
    if fsr.sharpen == 0.0 {
        textureStore(output, hr_pos, vec4<f32>(linear_to_srgb(colour), 1.0));
    }
    new_locks[lock_index(hr_pos)] = 0.0;
}
