// Counter-Strike world models (CS:S `w_*.mdl`) in players' hands, drawn in world space after the
// world against the scene depth. Vertices are already in the model's frame (rest pose baked),
// except a skinned model's (a CS:GO body): those blend up to four bones of its palette first.

#import bevy_render::view::View

@group(0) @binding(0) var<uniform> view: View;

struct Instance {
    world_from_model: mat4x4<f32>,
    // Ambient light colour (linear), rgb.
    ambient: vec4<f32>,
    // Direction toward the sun (world) and its colour, zero in shadow.
    sun_dir: vec4<f32>,
    sun: vec4<f32>,
    // x = 1 when the vertices are skinned by `bones`, y = the first bone of its palette there.
    flags: vec4<f32>,
}

@group(1) @binding(0) var<uniform> inst: Instance;
@group(1) @binding(1) var skin_tex: texture_2d<f32>;
@group(1) @binding(2) var skin_samp: sampler;
// Three rows per bone (row-major 3x4), model space: every instance's palette, one after another.
@group(1) @binding(3) var<storage, read> bones: array<vec4<f32>>;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bone_ids: u32,
    @location(4) weights: vec4<f32>,
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world: vec3<f32>,
}

// The fill light, over the ambient: the viewmodel's (3.2 of the grid's colour over its 2.4
// ambient), from the viewer's side and above, so a model in the shade reads as the gun in hand
// does rather than as a silhouette.
const FILL: f32 = 1.3333;

fn bone_point(bone: u32, p: vec3<f32>) -> vec3<f32> {
    let r0 = bones[bone * 3u];
    let r1 = bones[bone * 3u + 1u];
    let r2 = bones[bone * 3u + 2u];
    return vec3<f32>(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w);
}

fn bone_dir(bone: u32, d: vec3<f32>) -> vec3<f32> {
    let r0 = bones[bone * 3u];
    let r1 = bones[bone * 3u + 1u];
    let r2 = bones[bone * 3u + 2u];
    return vec3<f32>(dot(r0.xyz, d), dot(r1.xyz, d), dot(r2.xyz, d));
}

@vertex
fn vs_world(in: VsIn) -> VsOut {
    var out: VsOut;
    var position = in.position;
    var normal = in.normal;
    if (inst.flags.x > 0.5) {
        let first = u32(inst.flags.y);
        let ids = vec4<u32>(
            in.bone_ids & 0xffu,
            (in.bone_ids >> 8u) & 0xffu,
            (in.bone_ids >> 16u) & 0xffu,
            (in.bone_ids >> 24u) & 0xffu,
        ) + vec4<u32>(first);
        var w = in.weights;
        let total = w.x + w.y + w.z + w.w;
        if (total <= 0.0) {
            w = vec4<f32>(1.0, 0.0, 0.0, 0.0);
        } else {
            w = w / total;
        }
        position = bone_point(ids.x, in.position) * w.x
            + bone_point(ids.y, in.position) * w.y
            + bone_point(ids.z, in.position) * w.z
            + bone_point(ids.w, in.position) * w.w;
        normal = bone_dir(ids.x, in.normal) * w.x
            + bone_dir(ids.y, in.normal) * w.y
            + bone_dir(ids.z, in.normal) * w.z
            + bone_dir(ids.w, in.normal) * w.w;
    }
    let world = inst.world_from_model * vec4<f32>(position, 1.0);
    out.clip = view.clip_from_world * world;
    out.world = world.xyz;
    out.uv = in.uv;
    out.normal = (inst.world_from_model * vec4<f32>(normal, 0.0)).xyz;
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
    // The fill comes from the viewer's side and above (MW2's z is up), wrapped a little past the
    // terminator as the viewmodel's; the sun where it reaches.
    let to_eye = view.world_position - in.world;
    let side = normalize(vec3<f32>(to_eye.xy, 0.0) + vec3<f32>(0.0, 0.0, 1e-4));
    let fill_dir = normalize(side * 0.6 + vec3<f32>(0.0, 0.0, 0.75));
    let wrap = clamp((dot(n, fill_dir) + 0.5) / 1.5, 0.0, 1.0);
    let sun = max(dot(n, inst.sun_dir.xyz), 0.0);
    let shade = inst.ambient.rgb * (1.0 + FILL * wrap) + inst.sun.rgb * sun;
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
