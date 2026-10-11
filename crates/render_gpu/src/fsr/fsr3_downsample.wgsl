// One step of FSR's shading change pyramid (SPD's reduction): each texel the average of the
// 2x2 texels under it in the mip above.

@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var destination: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pos = vec2<i32>(id.xy);
    if !is_on_screen(pos, vec2<i32>(textureDimensions(destination))) {
        return;
    }
    let size = vec2<i32>(textureDimensions(source));
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        sum += textureLoad(source, clamp_load(pos * 2, bilinear_offset(i), size), 0);
    }
    textureStore(destination, pos, sum * 0.25);
}
