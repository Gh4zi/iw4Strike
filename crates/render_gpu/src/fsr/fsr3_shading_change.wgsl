// FSR 3.1 shading change (ffx_fsr3upscaler_shading_change.h): how much the lighting changed
// under each half-resolution pixel, from the first three mips of the shading change pyramid.

@group(0) @binding(1) var spd_mips: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;
@group(0) @binding(3) var shading_change: texture_storage_2d<rgba8unorm, write>;

const MIP_LEVELS_TO_USE: i32 = 3;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pos = vec2<i32>(id.xy);
    let size = shading_change_render_size();
    if !is_on_screen(pos, size) {
        return;
    }
    let uv = (vec2<f32>(pos) + 0.5) / vec2<f32>(size);
    let jittered = uv + fsr.jitter / render_size_f();
    let mip_uv = clamp_uv(jittered, size, vec2<i32>(textureDimensions(spd_mips, 0)));
    var change = 0.0;
    for (var mip = 0; mip < MIP_LEVELS_TO_USE; mip++) {
        let sample = textureSampleLevel(spd_mips, linear_clamp, mip_uv, f32(mip)).xy;
        let value = abs(sample.x * sample.y);
        if value > 0.0 {
            change = max(change, value);
        }
    }
    textureStore(shading_change, pos, vec4<f32>(saturate(change), 0.0, 0.0, 0.0));
}
