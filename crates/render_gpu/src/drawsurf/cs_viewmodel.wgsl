// Counter-Strike viewmodels (GoldSrc studio models) drawn in view space over the world.
// View space is GoldSrc's: x forward, y left, z up. Each vertex blends up to three bones.

struct Params {
    // x = 1 / tan(half horizontal fov), y = 1 / tan(half vertical fov), z = near plane.
    proj: vec4<f32>,
    // xyz = direction toward the light (view space), w = texture gamma exponent (GoldSrc 0.8,
    // Source 1).
    light_dir: vec4<f32>,
    // Ambient and directional light colours (linear), rgb.
    ambient: vec4<f32>,
    shade: vec4<f32>,
    // Direction toward the sun (view space) and its colour, zero in shadow.
    sun_dir: vec4<f32>,
    sun: vec4<f32>,
    // Three rows per bone (row-major 3x4).
    bones: array<vec4<f32>, 384>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var skin_tex: texture_2d<f32>;
@group(0) @binding(2) var skin_samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bones: u32,
    @location(4) weights: vec4<f32>,
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
}

fn bone_point(bone: u32, p: vec3<f32>) -> vec3<f32> {
    let r0 = params.bones[bone * 3u];
    let r1 = params.bones[bone * 3u + 1u];
    let r2 = params.bones[bone * 3u + 2u];
    return vec3<f32>(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w);
}

fn bone_dir(bone: u32, d: vec3<f32>) -> vec3<f32> {
    let r0 = params.bones[bone * 3u];
    let r1 = params.bones[bone * 3u + 1u];
    let r2 = params.bones[bone * 3u + 2u];
    return vec3<f32>(dot(r0.xyz, d), dot(r1.xyz, d), dot(r2.xyz, d));
}

@vertex
fn vs_viewmodel(in: VsIn) -> VsOut {
    var out: VsOut;
    let b0 = in.bones & 0xffu;
    let b1 = (in.bones >> 8u) & 0xffu;
    let b2 = (in.bones >> 16u) & 0xffu;
    var w = in.weights.xyz;
    let total = w.x + w.y + w.z;
    if (total <= 0.0) {
        w = vec3<f32>(1.0, 0.0, 0.0);
    } else {
        w = w / total;
    }
    let p = bone_point(b0, in.position) * w.x
        + bone_point(b1, in.position) * w.y
        + bone_point(b2, in.position) * w.z;
    // Reverse-Z with an infinite far plane: depth = near / forward distance.
    out.clip = vec4<f32>(-p.y * params.proj.x, p.z * params.proj.y, params.proj.z, p.x);
    out.uv = in.uv;
    out.normal = bone_dir(b0, in.normal) * w.x
        + bone_dir(b1, in.normal) * w.y
        + bone_dir(b2, in.normal) * w.z;
    return out;
}

// GoldSrc brightens model textures with its texture gamma (texgamma 2.0 over gamma 2.5): a 0.8
// power on the colour. Source textures are used as they are (exponent 1).
fn goldsrc_texture(rgb: vec3<f32>) -> vec3<f32> {
    return pow(rgb, vec3<f32>(params.light_dir.w));
}

// Textures sample as sRGB (linear light) and are lit in linear light, but the main target stores
// display values (MW2's gamma-space colour buffer), so the result is encoded back to sRGB.
fn encode(rgb: vec3<f32>) -> vec3<f32> {
    let c = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_viewmodel(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(skin_tex, skin_samp, in.uv);
    if (texel.a < 0.5) {
        discard;
    }
    // GoldSrc-style wrapped lambert: light reaches a little past the terminator.
    let n = normalize(in.normal);
    let wrap = clamp((dot(n, params.light_dir.xyz) + 0.5) / 1.5, 0.0, 1.0);
    let sun = max(dot(n, params.sun_dir.xyz), 0.0);
    let shade = params.ambient.rgb + params.shade.rgb * wrap + params.sun.rgb * sun;
    return vec4<f32>(encode(goldsrc_texture(texel.rgb) * shade), 1.0);
}

@fragment
fn fs_viewmodel_fullbright(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(skin_tex, skin_samp, in.uv);
    if (texel.a < 0.5) {
        discard;
    }
    return vec4<f32>(encode(goldsrc_texture(texel.rgb)), 1.0);
}

// Muzzle flash: the sprite times its tint (carried in the normal), added over the gun.
@fragment
fn fs_viewmodel_flash(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(skin_tex, skin_samp, in.uv);
    return vec4<f32>(encode(texel.rgb * texel.a * in.normal), 1.0);
}

@fragment
fn fs_viewmodel_additive(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(skin_tex, skin_samp, in.uv);
    return vec4<f32>(encode(texel.rgb), 1.0);
}
