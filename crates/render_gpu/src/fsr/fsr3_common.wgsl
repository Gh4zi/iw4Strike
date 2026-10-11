// AMD FidelityFX Super Resolution 3.1 upscaler, ported to WGSL for iw4Strike from the
// FidelityFX SDK (ffx_fsr3upscaler_*.h, ffx_fsr1.h). Pasted ahead of each FSR pass.
//
// Copyright (C) 2026 Advanced Micro Devices, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files(the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and /or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions :
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.
//
// Differences from the SDK: no exposure input (exposure 1, no pre-exposure), no reactive or
// transparency-and-composition masks, motion vectors already in UV space at render
// resolution, inverted infinite depth, no frame-interpolation outputs, and formats WebGPU can
// write from compute (single-channel intermediates widened to RGBA). The input is the scene's
// gamma-encoded colour: it is decoded to linear on load and encoded again on output.

struct Fsr3 {
    render_size: vec2<i32>,
    prev_render_size: vec2<i32>,
    upscale_size: vec2<i32>,
    prev_upscale_size: vec2<i32>,
    device_to_view_depth: vec4<f32>,
    jitter: vec2<f32>,
    prev_jitter: vec2<f32>,
    downscale_factor: vec2<f32>,
    tan_half_fov: f32,
    jitter_sequence_length: f32,
    view_space_to_meters: f32,
    frame_index: f32,
    velocity_factor: f32,
    reactiveness_scale: f32,
    shading_change_scale: f32,
    accumulation_added_per_frame: f32,
    min_disocclusion_accumulation: f32,
    // RCAS strength as a linear factor (exp2 of minus the stops).
    rcas_sharpness: f32,
    // 1 when RCAS runs after the accumulation (which then leaves the output alone).
    sharpen: f32,
    _pad0: f32,
}

@group(0) @binding(0) var<uniform> fsr: Fsr3;

const FP16_MIN: f32 = 6.10e-05;
const FP16_MAX: f32 = 65504.0;
const EPSILON: f32 = 6.10e-05;
const FP32_MAX: f32 = 3.402823466e+38;
const FP32_MIN: f32 = 1.175494351e-38;
const RECONSTRUCTED_DEPTH_BILINEAR_WEIGHT_THRESHOLD: f32 = 6.10e-04;
const UPSAMPLE_LANCZOS_WEIGHT_SCALE: f32 = 1.0 / 16.0;
const AVERAGE_LANCZOS_WEIGHT_PER_FRAME: f32 = 0.74 / 16.0;
const LOCK_THRESHOLD: f32 = 1.0;
const LOCK_MAX: f32 = 2.0;
const REACTIVE: i32 = 0;
const DISOCCLUSION: i32 = 1;
const SHADING_CHANGE: i32 = 2;
const ACCUMULATION: i32 = 3;

fn render_size() -> vec2<i32> {
    return fsr.render_size;
}

fn render_size_f() -> vec2<f32> {
    return vec2<f32>(fsr.render_size);
}

fn shading_change_render_size() -> vec2<i32> {
    return vec2<i32>(vec2<f32>(fsr.render_size) * 0.5);
}

fn reconstructed_depth_mv_px_threshold(nearest_depth_in_meters: f32) -> f32 {
    return mix(0.25, 0.75, saturate(nearest_depth_in_meters / 100.0));
}

fn get_4k_velocity(motion: vec2<f32>) -> f32 {
    return length(motion * vec2<f32>(3840.0, 2160.0));
}

fn min_divided_by_max(v0: f32, v1: f32, on_zero: f32) -> f32 {
    let m = max(v0, v1);
    if m != 0.0 {
        return min(v0, v1) / m;
    }
    return on_zero;
}

fn ycocg_to_rgb(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

fn rgb_to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn rgb_to_luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn is_uv_inside(uv: vec2<f32>) -> bool {
    return uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0;
}

fn clamp_load(pos: vec2<i32>, offset: vec2<i32>, size: vec2<i32>) -> vec2<i32> {
    return clamp(pos + offset, vec2<i32>(0), size - 1);
}

fn clamp_uv(uv: vec2<f32>, texture_size: vec2<i32>, resource_size: vec2<i32>) -> vec2<f32> {
    let location = uv * vec2<f32>(texture_size);
    let clamped = max(vec2<f32>(0.5), min(location, vec2<f32>(texture_size) - 0.5));
    return clamped / vec2<f32>(resource_size);
}

fn is_on_screen(pos: vec2<i32>, size: vec2<i32>) -> bool {
    return all(vec2<u32>(pos) < vec2<u32>(size));
}

fn compute_hr_pos_from_lr_pos(lr: vec2<i32>) -> vec2<i32> {
    let jittered = vec2<f32>(lr) + 0.5 - fsr.jitter;
    let in_hr = (jittered / render_size_f()) * vec2<f32>(fsr.upscale_size);
    return vec2<i32>(floor(in_hr));
}

fn get_view_space_depth(device_depth: f32) -> f32 {
    return fsr.device_to_view_depth.y / (device_depth - fsr.device_to_view_depth.x);
}

fn get_view_space_depth_in_meters(device_depth: f32) -> f32 {
    return get_view_space_depth(device_depth) * fsr.view_space_to_meters;
}

struct Bilinear {
    base: vec2<i32>,
    weights: vec4<f32>,
}

// The four bilinear taps in the order of their weights: (0,0), (1,0), (0,1), (1,1).
fn bilinear_offset(i: i32) -> vec2<i32> {
    return vec2<i32>(i & 1, i >> 1);
}

fn get_bilinear_sampling_data(uv: vec2<f32>, size: vec2<i32>) -> Bilinear {
    let px = uv * vec2<f32>(size) - 0.5;
    let frac = fract(px);
    var data: Bilinear;
    data.base = vec2<i32>(floor(px));
    data.weights = vec4<f32>(
        (1.0 - frac.x) * (1.0 - frac.y),
        frac.x * (1.0 - frac.y),
        (1.0 - frac.x) * frac.y,
        frac.x * frac.y,
    );
    return data;
}

// The scene's colour is gamma-encoded (sRGB); FSR wants linear light.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    let low = x * 12.92;
    let high = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, x <= vec3<f32>(0.0031308));
}
