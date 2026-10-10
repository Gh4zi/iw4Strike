// Counter-Strike viewmodels (GoldSrc studio models) drawn in view space over the world.
// View space is GoldSrc's: x forward, y left, z up. Each vertex blends up to four bones (GoldSrc
// and Source models use three, CS2's four).

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
    // The view's axes in the world (forward, left, up).
    axis_forward: vec4<f32>,
    axis_left: vec4<f32>,
    axis_up: vec4<f32>,
    // x = 1 when the map's reflection probe is bound.
    reflection: vec4<f32>,
    // Three rows per bone (row-major 3x4).
    bones: array<vec4<f32>, 384>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var skin_tex: texture_2d<f32>;
@group(0) @binding(2) var skin_samp: sampler;
// CS2's normal (hemi-octahedral, red and green), roughness/metalness and occlusion maps.
@group(0) @binding(3) var normal_tex: texture_2d<f32>;
@group(0) @binding(4) var metal_tex: texture_2d<f32>;
@group(0) @binding(5) var ao_tex: texture_2d<f32>;
// The map's reflection probe for where the gun is (a cube map, looked up by world direction).
@group(0) @binding(6) var probe_tex: texture_cube<f32>;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bones: u32,
    @location(4) weights: vec4<f32>,
    @location(5) tangent: vec4<f32>,
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    // w: the bitangent's sign.
    @location(2) tangent: vec4<f32>,
    @location(3) view_pos: vec3<f32>,
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
    let b3 = (in.bones >> 24u) & 0xffu;
    var w = in.weights;
    let total = w.x + w.y + w.z + w.w;
    if (total <= 0.0) {
        w = vec4<f32>(1.0, 0.0, 0.0, 0.0);
    } else {
        w = w / total;
    }
    let p = bone_point(b0, in.position) * w.x
        + bone_point(b1, in.position) * w.y
        + bone_point(b2, in.position) * w.z
        + bone_point(b3, in.position) * w.w;
    // Reverse-Z with an infinite far plane: depth = near / forward distance.
    out.clip = vec4<f32>(-p.y * params.proj.x, p.z * params.proj.y, params.proj.z, p.x);
    out.uv = in.uv;
    out.normal = bone_dir(b0, in.normal) * w.x
        + bone_dir(b1, in.normal) * w.y
        + bone_dir(b2, in.normal) * w.z
        + bone_dir(b3, in.normal) * w.w;
    let tangent = bone_dir(b0, in.tangent.xyz) * w.x
        + bone_dir(b1, in.tangent.xyz) * w.y
        + bone_dir(b2, in.tangent.xyz) * w.z
        + bone_dir(b3, in.tangent.xyz) * w.w;
    out.tangent = vec4<f32>(tangent, in.tangent.w);
    out.view_pos = p;
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

// GoldSrc-style wrapped lambert: light reaches a little past the terminator.
fn lit(in: VsOut, texel: vec4<f32>) -> vec4<f32> {
    let n = normalize(in.normal);
    let wrap = clamp((dot(n, params.light_dir.xyz) + 0.5) / 1.5, 0.0, 1.0);
    let sun = max(dot(n, params.sun_dir.xyz), 0.0);
    let shade = params.ambient.rgb + params.shade.rgb * wrap + params.sun.rgb * sun;
    return vec4<f32>(encode(goldsrc_texture(texel.rgb) * shade), 1.0);
}

@fragment
fn fs_viewmodel(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(skin_tex, skin_samp, in.uv);
    if (texel.a < 0.5) {
        discard;
    }
    return lit(in, texel);
}

// CS2's colour textures keep masks in alpha, so nothing is cut out.
@fragment
fn fs_viewmodel_opaque(in: VsOut) -> @location(0) vec4<f32> {
    return lit(in, textureSample(skin_tex, skin_samp, in.uv));
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

const PI: f32 = 3.14159265;

// A tangent-space normal from CS2's hemi-octahedral normal map (red and green); green points
// against the bitangent.
fn hemi_oct_normal(rg: vec2<f32>) -> vec3<f32> {
    let e = vec2<f32>(rg.x + rg.y - 1.003922, rg.x - rg.y);
    let n = normalize(vec3<f32>(e, 1.0 - abs(e.x) - abs(e.y)));
    return vec3<f32>(n.x, -n.y, n.z);
}

// Light a surface reflects from its surroundings, averaged over its roughness: Karis' analytic
// fit of the split-sum environment BRDF ("Physically Based Shading on Mobile", 2014).
fn env_brdf(f0: vec3<f32>, roughness: f32, nv: f32) -> vec3<f32> {
    let r = roughness * vec4<f32>(-1.0, -0.0275, -0.572, 0.022)
        + vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

// A light's highlight: GGX distribution, Schlick-Smith visibility and Schlick's Fresnel.
fn highlight(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, roughness: f32, f0: vec3<f32>) -> vec3<f32> {
    let h = normalize(v + l);
    let nl = max(dot(n, l), 0.0);
    let nv = max(dot(n, v), 1e-4);
    let nh = max(dot(n, h), 0.0);
    let vh = max(dot(v, h), 0.0);
    let a = max(roughness * roughness, 0.002);
    let a2 = a * a;
    let d_den = nh * nh * (a2 - 1.0) + 1.0;
    let d = a2 / (PI * d_den * d_den);
    let k = a * 0.5;
    let vis = 0.25 / ((nl * (1.0 - k) + k) * (nv * (1.0 - k) + k));
    let f = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - vh, 5.0);
    return d * vis * f * nl;
}

// What a surface facing `r` (view space) reflects: the map's reflection probe, blurrier the
// rougher it is, or without one a sky brighter than the ground. MW2's probes keep the world's z
// mirrored (their faces are left-handed, the world right-handed): up is their -z.
fn surroundings(r: vec3<f32>, roughness: f32) -> vec3<f32> {
    let world = params.axis_forward.xyz * r.x + params.axis_left.xyz * r.y + params.axis_up.xyz * r.z;
    let lod = roughness * f32(textureNumLevels(probe_tex) - 1u);
    let raw = textureSampleLevel(probe_tex, skin_samp, vec3<f32>(world.xy, -world.z), lod).rgb;
    let probe = mix(vec3<f32>(dot(raw, vec3<f32>(0.2126, 0.7152, 0.0722))), raw, PROBE_SATURATION);
    let up = vec3<f32>(params.axis_forward.z, params.axis_left.z, params.axis_up.z);
    let sky = params.ambient.rgb * 1.3 + params.shade.rgb * 0.6 + params.sun.rgb * 0.2;
    let ground = params.ambient.rgb * 0.4;
    let spread = roughness * roughness;
    let gradient = mix(ground, sky, smoothstep(-0.3 - spread, 0.4 + spread, dot(r, up)));
    return select(gradient, probe * PROBE_SCALE, params.reflection.x > 0.5);
}

// The probe's light against the gun's, and how much of its colour is kept: MW2 captured its
// probes far more saturated than the map renders (a desert's sand turned steel gold).
const PROBE_SCALE: f32 = 0.7;
const PROBE_SATURATION: f32 = 0.4;
// How strongly surfaces reflect their surroundings.
const REFLECTION: f32 = 0.8;

// CS2's materials: the colour lit as the other models are (now with its occlusion), plus what
// the surface reflects. Metal takes its colour from reflections alone, and the sun leaves
// highlights.
fn pbr(in: VsOut, character: bool) -> vec4<f32> {
    let base = textureSample(skin_tex, skin_samp, in.uv);
    let normal_map = textureSample(normal_tex, skin_samp, in.uv);
    let metal_map = textureSample(metal_tex, skin_samp, in.uv);
    let ao = textureSample(ao_tex, skin_samp, in.uv).r;
    let roughness = clamp(select(metal_map.r, normal_map.b, character), 0.05, 1.0);
    let metalness = metal_map.g;

    let ng = normalize(in.normal);
    var t = in.tangent.xyz - ng * dot(ng, in.tangent.xyz);
    t = select(vec3<f32>(0.0, 0.0, 1.0), normalize(t), dot(t, t) > 1e-8);
    let b = select(1.0, sign(in.tangent.w), in.tangent.w != 0.0) * cross(ng, t);
    let ts = hemi_oct_normal(normal_map.rg);
    let n = normalize(t * ts.x + b * ts.y + ng * ts.z);
    let v = normalize(-in.view_pos);
    let nv = max(dot(n, v), 1e-4);

    let diffuse_colour = base.rgb * (1.0 - metalness);
    let f0 = mix(vec3<f32>(0.04), base.rgb, metalness);
    let wrap = clamp((dot(n, params.light_dir.xyz) + 0.5) / 1.5, 0.0, 1.0);
    let sun_nl = max(dot(n, params.sun_dir.xyz), 0.0);
    let diffuse = diffuse_colour
        * (params.ambient.rgb * ao + params.shade.rgb * wrap * mix(1.0, ao, 0.5) + params.sun.rgb * sun_nl);

    let env = surroundings(reflect(-v, n), roughness);
    let specular = env * env_brdf(f0, roughness, nv) * ao * REFLECTION
        + highlight(n, v, params.sun_dir.xyz, roughness, f0) * params.sun.rgb;
    return vec4<f32>(encode(diffuse + specular), 1.0);
}

@fragment
fn fs_viewmodel_pbr_weapon(in: VsOut) -> @location(0) vec4<f32> {
    return pbr(in, false);
}

@fragment
fn fs_viewmodel_pbr_character(in: VsOut) -> @location(0) vec4<f32> {
    return pbr(in, true);
}

@fragment
fn fs_viewmodel_additive(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(skin_tex, skin_samp, in.uv);
    return vec4<f32>(encode(texel.rgb), 1.0);
}
