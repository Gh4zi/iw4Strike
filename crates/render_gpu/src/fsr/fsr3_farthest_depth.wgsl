// FSR 3.1 luma pyramid (ffx_fsr3upscaler_luma_pyramid.h), the part the upscaler reads: the
// farthest depth averaged over 2x2 pixels (SPD's first mip). The scene luma and auto-exposure
// it also builds only feed frame interpolation, which is not used here.

@group(0) @binding(1) var farthest_depth: texture_2d<f32>;
@group(0) @binding(2) var farthest_depth_mip1: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pos = vec2<i32>(id.xy);
    if !is_on_screen(pos, vec2<i32>(textureDimensions(farthest_depth_mip1))) {
        return;
    }
    var sum = 0.0;
    for (var i = 0; i < 4; i++) {
        let source = clamp_load(pos * 2, bilinear_offset(i), render_size());
        sum += textureLoad(farthest_depth, source, 0).x;
    }
    textureStore(farthest_depth_mip1, pos, vec4<f32>(sum * 0.25, 0.0, 0.0, 0.0));
}
