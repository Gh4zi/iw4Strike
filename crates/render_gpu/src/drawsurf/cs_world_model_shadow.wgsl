// Counter-Strike world models (CS:GO bodies and the guns they hold) into the sun's shadow map:
// each partition's light clip transform, and as colour the depth IW4's casters store there (the
// sun's projection is orthographic, so its clip z), which the receivers compare against.

struct Shadow {
    clip_from_world: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> shadow: Shadow;

struct Instance {
    world_from_model: mat4x4<f32>,
    ambient: vec4<f32>,
    sun_dir: vec4<f32>,
    sun: vec4<f32>,
    // x = 1 when the vertices are skinned by `bones`, y = the first bone of its palette there.
    flags: vec4<f32>,
}

@group(1) @binding(0) var<uniform> inst: Instance;
@group(1) @binding(1) var skin_tex: texture_2d<f32>;
@group(1) @binding(2) var skin_samp: sampler;
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
    @location(1) depth: f32,
}

fn bone_point(bone: u32, p: vec3<f32>) -> vec3<f32> {
    let r0 = bones[bone * 3u];
    let r1 = bones[bone * 3u + 1u];
    let r2 = bones[bone * 3u + 2u];
    return vec3<f32>(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w);
}

@vertex
fn vs_shadow(in: VsIn) -> VsOut {
    var out: VsOut;
    var position = in.position;
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
    }
    let world = inst.world_from_model * vec4<f32>(position, 1.0);
    out.clip = shadow.clip_from_world * world;
    out.depth = out.clip.z / out.clip.w;
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_shadow(in: VsOut) -> @location(0) vec4<f32> {
    // Alpha-tested holes (straps, a gun's cut-outs) cast none, as they show none.
    if (textureSample(skin_tex, skin_samp, in.uv).a < 0.5) {
        discard;
    }
    return vec4<f32>(in.depth, 0.0, 0.0, 1.0);
}
