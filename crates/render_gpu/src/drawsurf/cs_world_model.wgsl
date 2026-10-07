// Counter-Strike world models (CS:S `w_*.mdl`) in players' hands, drawn in world space after the
// world against the scene depth. Vertices are already in the model's frame (rest pose baked).

#import bevy_render::view::View

@group(0) @binding(0) var<uniform> view: View;

struct Instance {
    world_from_model: mat4x4<f32>,
    // Ambient light colour (linear), rgb.
    ambient: vec4<f32>,
    // Direction toward the sun (world) and its colour, zero in shadow.
    sun_dir: vec4<f32>,
    sun: vec4<f32>,
}

@group(1) @binding(0) var<uniform> inst: Instance;
@group(1) @binding(1) var skin_tex: texture_2d<f32>;
@group(1) @binding(2) var skin_samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
}

@vertex
fn vs_world(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = inst.world_from_model * vec4<f32>(in.position, 1.0);
    out.clip = view.clip_from_world * world;
    out.uv = in.uv;
    out.normal = (inst.world_from_model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

// Lit in linear light; the colour buffer holds display (sRGB) values, as for the viewmodel.
fn encode(rgb: vec3<f32>) -> vec3<f32> {
    let c = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_world(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(skin_tex, skin_samp, in.uv);
    if (texel.a < 0.5) {
        discard;
    }
    let n = normalize(in.normal);
    // The sky lights the top a little more than the bottom, the sun where it reaches.
    let sky = 0.75 + 0.25 * n.z;
    let sun = max(dot(n, inst.sun_dir.xyz), 0.0);
    let shade = inst.ambient.rgb * sky + inst.sun.rgb * sun;
    return vec4<f32>(encode(texel.rgb * shade), 1.0);
}

@fragment
fn fs_world_fullbright(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(skin_tex, skin_samp, in.uv);
    if (texel.a < 0.5) {
        discard;
    }
    return vec4<f32>(encode(texel.rgb), 1.0);
}
