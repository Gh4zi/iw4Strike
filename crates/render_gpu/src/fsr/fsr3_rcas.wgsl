// FSR's robust contrast-adaptive sharpening (RCAS, ffx_fsr1.h with denoise, as the FSR 3.1
// upscaler runs it): sharpens the accumulated frame into the output, limited so it never
// overshoots the local 5-tap neighbourhood.

@group(0) @binding(1) var upscaled: texture_2d<f32>;
@group(0) @binding(2) var output: texture_storage_2d<rgba8unorm, write>;

const RCAS_LIMIT: f32 = 0.25 - 1.0 / 16.0;

fn rcas_load(pos: vec2<i32>) -> vec3<f32> {
    return textureLoad(upscaled, clamp(pos, vec2<i32>(0), fsr.upscale_size - 1), 0).rgb;
}

fn max3(a: f32, b: f32, c: f32) -> f32 {
    return max(a, max(b, c));
}

fn min3(a: f32, b: f32, c: f32) -> f32 {
    return min(a, min(b, c));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pos = vec2<i32>(id.xy);
    if !is_on_screen(pos, fsr.upscale_size) {
        return;
    }
    //    b
    //  d e f
    //    h
    let b = rcas_load(pos + vec2<i32>(0, -1));
    let d = rcas_load(pos + vec2<i32>(-1, 0));
    let e = rcas_load(pos);
    let f = rcas_load(pos + vec2<i32>(1, 0));
    let h = rcas_load(pos + vec2<i32>(0, 1));
    // Luma times 2.
    let bl = b.b * 0.5 + (b.r * 0.5 + b.g);
    let dl = d.b * 0.5 + (d.r * 0.5 + d.g);
    let el = e.b * 0.5 + (e.r * 0.5 + e.g);
    let fl = f.b * 0.5 + (f.r * 0.5 + f.g);
    let hl = h.b * 0.5 + (h.r * 0.5 + h.g);
    // Noise detection.
    var nz = 0.25 * bl + 0.25 * dl + 0.25 * fl + 0.25 * hl - el;
    let range = max3(max3(bl, dl, el), fl, hl) - min3(min3(bl, dl, el), fl, hl);
    // Guarded divisions: black pixels would give NaN where HLSL's saturate hides it.
    nz = saturate(abs(nz) / max(range, 1e-6));
    nz = -0.5 * nz + 1.0;
    // Min and max of the ring.
    let mn4 = min(min(min(b, d), f), h);
    let mx4 = max(max(max(b, d), f), h);
    let peak = vec2<f32>(1.0, -4.0);
    // Limiters.
    let lower_limiter = saturate(el / max(min(min3(bl, dl, fl), hl), 1e-6));
    let hit_min = mn4 / max(4.0 * mx4, vec3<f32>(1e-6)) * lower_limiter;
    let hit_max = (peak.x - mx4) / min(4.0 * mn4 + peak.y, vec3<f32>(-1e-6));
    let lobe_rgb = max(-hit_min, hit_max);
    var lobe = max(-RCAS_LIMIT, min(max3(lobe_rgb.r, lobe_rgb.g, lobe_rgb.b), 0.0)) * fsr.rcas_sharpness;
    lobe *= nz;
    let rcp_l = 1.0 / (4.0 * lobe + 1.0);
    let colour = (lobe * b + lobe * d + lobe * h + lobe * f + e) * rcp_l;
    textureStore(output, pos, vec4<f32>(linear_to_srgb(colour), 1.0));
}
