// FSR 3.1 shading change pyramid (ffx_fsr3upscaler_shading_change_pyramid.h), first mip: for
// each pixel, the smallest signed luma difference between this frame's and last frame's
// (reprojected) 5-tap neighbourhoods, averaged over 2x2 pixels. Later mips are plain 2x2
// averages (fsr3_downsample).

@group(0) @binding(1) var current_luma: texture_2d<f32>;
@group(0) @binding(2) var previous_luma: texture_2d<f32>;
@group(0) @binding(3) var dilated_motion: texture_2d<f32>;
@group(0) @binding(4) var mip0: texture_storage_2d<rgba16float, write>;

const SET_SIZE: i32 = 5;

fn sample_offset(i: i32) -> vec2<i32> {
    switch i {
        case 1: {
            return vec2<i32>(-1, 0);
        }
        case 2: {
            return vec2<i32>(1, 0);
        }
        case 3: {
            return vec2<i32>(0, -1);
        }
        case 4: {
            return vec2<i32>(0, 1);
        }
        default: {
            return vec2<i32>(0, 0);
        }
    }
}

fn compare_swap(samples: ptr<function, array<f32, 5>>, i: i32, j: i32) {
    let low = min((*samples)[i], (*samples)[j]);
    (*samples)[j] = max((*samples)[i], (*samples)[j]);
    (*samples)[i] = low;
}

fn sort_set(samples: ptr<function, array<f32, 5>>) {
    compare_swap(samples, 0, 3);
    compare_swap(samples, 1, 4);
    compare_swap(samples, 0, 2);
    compare_swap(samples, 1, 3);
    compare_swap(samples, 0, 1);
    compare_swap(samples, 2, 4);
    compare_swap(samples, 1, 2);
    compare_swap(samples, 3, 4);
    compare_swap(samples, 2, 3);
}

fn compute_minimum_difference(set0_in: array<f32, 5>, set1_in: array<f32, 5>) -> f32 {
    var set0 = set0_in;
    var set1 = set1_in;
    var min_diff = FP16_MAX - 1.0;
    var a = 0;
    var b = 0;
    sort_set(&set0);
    sort_set(&set1);
    let top = min(set0[SET_SIZE - 1], set1[SET_SIZE - 1]);
    if top > FP32_MIN {
        for (var i = 0; i < SET_SIZE && min_diff < FP16_MAX; i++) {
            // The SDK can step one past the end here; stay on the last sample instead.
            let x = set0[min(a, SET_SIZE - 1)];
            let y = set1[min(b, SET_SIZE - 1)];
            var diff = x - y;
            if abs(diff) > FP16_MIN {
                diff = sign(diff) * (1.0 - min_divided_by_max(x, y, 0.0));
                if abs(diff) < abs(min_diff) {
                    min_diff = diff;
                }
                a += i32(x < y);
                b += i32(set0[min(a, SET_SIZE - 1)] >= y);
            } else {
                min_diff = FP16_MAX;
            }
        }
    }
    return min_diff * f32(min_diff < FP16_MAX - 1.0);
}

fn compute_diff(uv: vec2<f32>, motion: vec2<f32>) -> f32 {
    let size = render_size();
    var current: array<f32, 5>;
    let base = vec2<i32>(floor((uv + fsr.jitter / render_size_f()) * render_size_f()));
    for (var i = 0; i < SET_SIZE; i++) {
        let sample_pos = clamp_load(base, sample_offset(i), size);
        current[i] = max(textureLoad(current_luma, sample_pos, 0).x, EPSILON);
    }
    let prev_size = fsr.prev_render_size;
    let reprojected = uv + fsr.prev_jitter / vec2<f32>(prev_size) + motion;
    if !is_uv_inside(reprojected) {
        return 0.0;
    }
    var previous: array<f32, 5>;
    let prev_base = vec2<i32>(floor(reprojected * vec2<f32>(prev_size)));
    for (var i = 0; i < SET_SIZE; i++) {
        let sample_pos = clamp_load(prev_base, sample_offset(i), prev_size);
        previous[i] = max(textureLoad(previous_luma, sample_pos, 0).x, EPSILON);
    }
    return compute_minimum_difference(current, previous);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pos = vec2<i32>(id.xy);
    if !is_on_screen(pos, vec2<i32>(textureDimensions(mip0))) {
        return;
    }
    var sum = vec2<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let source = clamp_load(pos * 2, bilinear_offset(i), render_size());
        let motion = textureLoad(dilated_motion, source, 0).xy;
        let uv = (vec2<f32>(source) + 0.5) / render_size_f();
        let diff = compute_diff(uv, motion);
        sum += vec2<f32>(diff, select(0.0, sign(diff), diff != 0.0));
    }
    textureStore(mip0, pos, vec4<f32>(sum * 0.25, 1.0, 0.0));
}
