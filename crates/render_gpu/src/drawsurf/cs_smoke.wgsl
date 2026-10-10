// CS2's volumetric smoke (see cs_smoke.rs): every pixel marches its view ray through the smoke
// volumes up to the scene depth, front to back. The look follows the numbers of CS2's own smoke
// material (`materials/dev/smoke_volume.vmat`): white smoke lit 0.6 by the light around it and
// 0.35 by the sun, two octaves of billowing noise eating 0.8 into it, darker inside.

struct Volume {
    // Camera-relative corner, voxel side.
    lo: vec4<f32>,
    // Camera-relative far corner, atlas slot.
    hi: vec4<f32>,
    // Colour of the light around the smoke, grow.
    ambient: vec4<f32>,
    // Sun light, alpha.
    sun: vec4<f32>,
}

struct Hole {
    // Camera-relative start, radius.
    a: vec4<f32>,
    // Camera-relative end, strength.
    b: vec4<f32>,
}

struct Params {
    world_from_clip: mat4x4<f32>,
    // Width, height, top of the world's depth range, seconds.
    screen: vec4<f32>,
    // Volumes, holes, frame pixels across one march pixel (smoke_quality), march step.
    counts: vec4<f32>,
    sun_dir: vec4<f32>,
    origin: vec4<f32>,
    volumes: array<Volume, 12>,
    holes: array<Hole, 24>,
}

@group(0) @binding(0) var<uniform> params: Params;
#ifdef MULTISAMPLED
@group(0) @binding(1) var depth_tex: texture_depth_multisampled_2d;
#else
@group(0) @binding(1) var depth_tex: texture_depth_2d;
#endif
@group(0) @binding(2) var atlas: texture_3d<f32>;
@group(0) @binding(3) var noise_tex: texture_3d<f32>;
@group(0) @binding(4) var clamp_samp: sampler;
@group(0) @binding(5) var repeat_samp: sampler;

// Atlas slices (one per smoke).
const SLOTS: f32 = 12.0;
const MAX_STEPS: i32 = 160;
// Steps skipped at once through empty voxels.
const EMPTY_STEPS: f32 = 3.0;
// Light lost per unit through smoke of density 1.
const EXTINCTION: f32 = 0.35;
// Below this much light left the ray stops.
const OPAQUE: f32 = 0.02;

// CS2's smoke material.
const SMOKE_COLOR: vec3<f32> = vec3<f32>(1.0, 1.0, 1.0);
const DENSITY: f32 = 0.85;
const AMBIENT_STRENGTH: f32 = 0.6;
const DIRECT_STRENGTH: f32 = 0.35;
const RIM_LIGHT_STRENGTH: f32 = 0.25;
const NOISE_STRENGTH_LOW: f32 = 0.5;
const NOISE_STRENGTH_HIGH: f32 = 0.35;
const SMOKE_NOISE_STRENGTH: f32 = 0.8;
const NOISE_OFFSET: f32 = -0.95;
// How strongly the voxels hold the smoke against the noise eating it, and how soft a
// billow's edge is.
const BODY: f32 = 1.25;
const EDGE_SOFTNESS: f32 = 0.2;
// Shadows toward the light: worked out every this many steps and carried between.
const SHADOW_EVERY: i32 = 4;
// Light lost per unit toward the light: softer than the view's, so a billow's shade fades over it
// instead of flipping at its edge.
const SHADOW_EXTINCTION: f32 = 0.06;
// CS2: g_flNoiseNormalStrength and g_flNoiseNormalStrengthDirect. The billow slope is read this
// far toward the light and scaled into how squarely a billow faces it.
const NOISE_NORMAL_STRENGTH: f32 = 0.5;
const NOISE_NORMAL_STRENGTH_DIRECT: f32 = 0.5;
const NORMAL_STEP: f32 = 8.0;
const NORMAL_SLOPE: f32 = 4.0;

// World units one tile of the noise spans, for the large billows and the fine ones.
const NOISE_TILE_LOW: f32 = 180.0;
const NOISE_TILE_HIGH: f32 = 64.0;
// How far the smooth noise bends the smoke's shape off its voxels, end to end.
const WARP: f32 = 16.0;
// A fixed turn (about 40° around a slanted axis) for the fine octave.
const TURN: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(0.8519, 0.4645, -0.2416),
    vec3<f32>(-0.3533, 0.8519, 0.3866),
    vec3<f32>(0.3866, -0.2416, 0.8901),
);
// How much smooth noise is mixed into the fine billows.
const FINE_SMOOTH_SHARE: f32 = 0.35;
// Texels across the noise texture's finest level.
const NOISE_TEXELS: f32 = 48.0;
// The texels store the light around the smoke over this.
const AMBIENT_MAX: f32 = 1.5;

@vertex
fn vs_smoke(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// The main target stores display values; the smoke is lit in linear light.
fn encode(rgb: vec3<f32>) -> vec3<f32> {
    let c = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

// Entry and exit distance of a ray from the camera through a camera-relative box.
fn ray_box(dir: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec2<f32> {
    let safe = select(dir, vec3<f32>(1.0e-6), abs(dir) < vec3<f32>(1.0e-6));
    let inv = 1.0 / safe;
    let a = lo * inv;
    let b = hi * inv;
    let near = min(a, b);
    let far = max(a, b);
    return vec2<f32>(max(max(near.x, near.y), near.z), min(min(far.x, far.y), far.z));
}

// Noise fixed to the world and drifting up through it: a large octave of Perlin-Worley
// billows, a fine octave of Worley ones, and a smooth bend of the shape.
struct Noise {
    warp: vec3<f32>,
    billow: f32,
}

// `lod`: the mip levels (low, high octave) whose texels are as wide as a march step, so the ray
// never skips over detail finer than its steps (that shows as stripes).
fn drift_noise(world: vec3<f32>, seconds: f32, lod: vec2<f32>) -> Noise {
    let low = textureSampleLevel(
        noise_tex,
        repeat_samp,
        (world + vec3<f32>(1.5, 1.0, -6.0) * seconds) / NOISE_TILE_LOW,
        lod.x,
    );
    // The fine octave turned off the axes, so its cells never line up with the coarse ones.
    let high = textureSampleLevel(
        noise_tex,
        repeat_samp,
        (TURN * world + vec3<f32>(-2.0, 1.5, -9.0) * seconds) / NOISE_TILE_HIGH,
        lod.y,
    );
    var noise: Noise;
    noise.warp = (vec3<f32>(low.b, low.a, high.b) - 0.5) * WARP;
    // Worley cells alone are all one size (a golf ball); smooth noise mixed in varies them.
    noise.billow = NOISE_STRENGTH_LOW * low.r
        + NOISE_STRENGTH_HIGH * mix(high.g, high.a, FINE_SMOOTH_SHARE);
    return noise;
}

// The voxels' own density at camera-relative `p` (no noise, no growth): zero means no smoke
// within half a voxel, so the march can skip ahead; it also stands in for the smoke between a
// sample and the light.
fn voxel_density(p: vec3<f32>) -> f32 {
    var density = 0.0;
    let count = i32(params.counts.x);
    for (var i = 0; i < count; i++) {
        let v = params.volumes[i];
        if (any(p < v.lo.xyz) || any(p > v.hi.xyz)) {
            continue;
        }
        let local = (p - v.lo.xyz) / (v.hi.xyz - v.lo.xyz);
        density += textureSampleLevel(
            atlas,
            clamp_samp,
            vec3<f32>(local.xy, (local.z + v.hi.w) / SLOTS),
            0.0,
        ).r * v.sun.w;
    }
    return density;
}

// One smoke sample: density, the light reaching it from all around, and whether it sees the
// sun (both already weighted by density).
struct Sample {
    density: f32,
    ambient: vec3<f32>,
    sky: f32,
}

fn sample_smoke(p: vec3<f32>, noise: Noise) -> Sample {
    var s: Sample;
    s.density = 0.0;
    s.ambient = vec3<f32>(0.0);
    s.sky = 0.0;
    let q = p + noise.warp;
    let count = i32(params.counts.x);
    for (var i = 0; i < count; i++) {
        let v = params.volumes[i];
        if (any(q < v.lo.xyz) || any(q > v.hi.xyz)) {
            continue;
        }
        let local = (q - v.lo.xyz) / (v.hi.xyz - v.lo.xyz);
        let texel = textureSampleLevel(
            atlas,
            clamp_samp,
            vec3<f32>(local.xy, (local.z + v.hi.w) / SLOTS),
            0.0,
        );
        if (texel.r <= 0.0) {
            continue;
        }
        // Flood order and light are stored times density: dividing by the filtered density
        // gives the filled neighbours' own values at the edges.
        let arrival = texel.g / texel.r;
        let reached = 1.0 - smoothstep(v.ambient.w - 0.12, v.ambient.w, arrival);
        // The billows eat in where the smoke thins toward open air, and everywhere as it
        // clears.
        let shape = texel.r * reached * v.sun.w * BODY;
        let raw = shape + SMOKE_NOISE_STRENGTH * (noise.billow + NOISE_OFFSET);
        // Each billow ends in a crisp edge, as CS2's do.
        let density = smoothstep(0.0, EDGE_SOFTNESS, raw) * DENSITY;
        if (density <= 0.0) {
            continue;
        }
        s.density += density;
        s.ambient += v.ambient.rgb * (texel.a / texel.r * AMBIENT_MAX) * density;
        s.sky += texel.b / texel.r * density;
    }
    return s;
}

// How much of the smoke the bullet tunnels and blasts leave at `p` (1 none taken).
fn hole_keep(p: vec3<f32>, noise: f32) -> f32 {
    var keep = 1.0;
    let count = i32(params.counts.y);
    for (var i = 0; i < count; i++) {
        let h = params.holes[i];
        let ab = h.b.xyz - h.a.xyz;
        let t = clamp(dot(p - h.a.xyz, ab) / max(dot(ab, ab), 1.0e-4), 0.0, 1.0);
        let radius = h.a.w;
        let d = length(p - (h.a.xyz + ab * t)) + (noise - 0.4) * radius * 0.8;
        let open = 1.0 - smoothstep(radius * 0.5, radius, d);
        keep *= 1.0 - open * h.b.w;
    }
    return keep;
}

// The sun light of the first volume (every smoke shares the map's sun).
fn sun_light() -> vec3<f32> {
    return params.volumes[0].sun.rgb;
}


// Distance from the camera to the scene at a full-size pixel (0 in the viewmodel's depth band,
// far for the sky).
fn scene_distance(ndc: vec2<f32>, depth: f32) -> f32 {
    if (depth > params.screen.z) {
        return 0.0;
    }
    if (depth <= 0.0) {
        return 1.0e9;
    }
    let hit = params.world_from_clip * vec4<f32>(ndc, depth / params.screen.z, 1.0);
    return length(hit.xyz / hit.w);
}

struct MarchOut {
    // Premultiplied smoke, display values.
    @location(0) colour: vec4<f32>,
    // Where this pixel's ray met the scene, for the composite's depth test.
    @location(1) distance: f32,
}

// One march pixel covers a scale×scale block of the frame (1: full resolution); it marches up
// to the nearest of the block's depths, so smoke never shows on something in front of it.
@fragment
fn fs_smoke(@builtin(position) frag: vec4<f32>) -> MarchOut {
    let scale = max(i32(params.counts.z), 1);
    let block = vec2<i32>(floor(frag.xy)) * scale;
    let last = vec2<i32>(params.screen.xy) - 1;
    var depth = 0.0;
    for (var k = 0; k < scale * scale; k++) {
        let texel = min(block + vec2<i32>(k % scale, k / scale), last);
        depth = max(depth, textureLoad(depth_tex, texel, 0));
    }
    let centre = vec2<f32>(block) + f32(scale) * 0.5;
    let ndc = vec2<f32>(
        centre.x / params.screen.x * 2.0 - 1.0,
        1.0 - centre.y / params.screen.y * 2.0,
    );
    let scene = scene_distance(ndc, depth);
    var out: MarchOut;
    out.distance = scene;
    out.colour = vec4<f32>(0.0);
    // Reverse depth: the near plane is 1.
    let near = params.world_from_clip * vec4<f32>(ndc, 1.0, 1.0);
    let dir = normalize(near.xyz / near.w);

    var start = 1.0e9;
    var end = 0.0;
    let count = i32(params.counts.x);
    for (var i = 0; i < count; i++) {
        let span = ray_box(dir, params.volumes[i].lo.xyz, params.volumes[i].hi.xyz);
        if (span.y > max(span.x, 0.0)) {
            start = min(start, max(span.x, 0.0));
            end = max(end, span.y);
        }
    }
    end = min(end, scene);
    if (end <= start) {
        return out;
    }

    // The samples are staggered over each 2×2 block in a fixed order, hiding the step pattern;
    // the composite's 3×3 blur takes in all four staggers evenly, so it cancels out.
    let cell = vec2<u32>(floor(frag.xy)) & vec2<u32>(1u);
    let jitter = (f32(cell.x * 2u + (cell.x ^ cell.y)) + 0.5) * 0.25;
    let seconds = params.screen.w;
    // March step (CS2 steps 6 units, g_flRayStepLength; smoke_quality high takes 4).
    let step = max(params.counts.w, 1.0);
    let lod = max(
        log2(vec2<f32>(step) / (vec2<f32>(NOISE_TILE_LOW, NOISE_TILE_HIGH) / NOISE_TEXELS)),
        vec2<f32>(0.0),
    );
    let has_holes = params.counts.y > 0.0;
    let has_sun = dot(params.sun_dir.xyz, params.sun_dir.xyz) > 0.5;
    // Shadows look toward the sun, or up without one.
    let to_light = select(vec3<f32>(0.0, 0.0, 1.0), params.sun_dir.xyz, has_sun);
    let sun = sun_light();
    // Thin smoke lit from behind glows at its edges.
    let rim = RIM_LIGHT_STRENGTH * pow(max(dot(dir, to_light), 0.0), 4.0) * f32(has_sun);
    var transmit = 1.0;
    var light = vec3<f32>(0.0);
    var shadow = 1.0;
    var facing = 0.5;
    var light_due = 0;
    var t = start + jitter * step;
    for (var i = 0; i < MAX_STEPS && t < end; i++) {
        let p = dir * t;
        // Empty voxels need no noise: skip through them a few steps at a time (a whole number
        // of steps, so the stagger stays in phase).
        if (voxel_density(p) <= 0.0) {
            t += step * EMPTY_STEPS;
            continue;
        }
        t += step;
        let world = p + params.origin.xyz;
        let noise = drift_noise(world, seconds, lod);
        let s = sample_smoke(p, noise);
        if (s.density <= 0.0) {
            continue;
        }
        var density = s.density;
        if (has_holes) {
            density *= hole_keep(p, noise.billow);
        }
        if (i >= light_due) {
            // The smoke between here and the light (voxels only), and CS2's billow lighting:
            // the side of a billow facing the light (its noise falling toward it) is lit, the
            // side turned away is in shade. Both are carried over the next few steps.
            let near = voxel_density(p + to_light * 12.0);
            let far = voxel_density(p + to_light * 32.0);
            shadow = exp(-(near * 16.0 + far * 24.0) * DENSITY * SHADOW_EXTINCTION);
            let ahead = drift_noise(world + to_light * NORMAL_STEP, seconds, lod).billow;
            facing = clamp(0.5 + (noise.billow - ahead) * NORMAL_SLOPE, 0.0, 1.0);
            light_due = i + SHADOW_EVERY;
        }
        let ambient_shade = mix(1.0, 0.5 + facing, NOISE_NORMAL_STRENGTH) * mix(0.45, 1.0, shadow);
        let direct_shade = mix(1.0, facing, NOISE_NORMAL_STRENGTH_DIRECT) * mix(0.1, 1.0, shadow);
        let ambient = s.ambient / s.density;
        let sky = s.sky / s.density;
        let thin = 1.0 - clamp(density * 2.0, 0.0, 1.0);
        let lit = SMOKE_COLOR
            * (AMBIENT_STRENGTH * ambient * ambient_shade + DIRECT_STRENGTH * sun * sky * direct_shade)
            + sun * sky * rim * thin;
        let absorbed = 1.0 - exp(-density * EXTINCTION * step);
        light += transmit * absorbed * lit;
        transmit *= 1.0 - absorbed;
        if (transmit < OPAQUE) {
            transmit = 0.0;
            break;
        }
    }
    let alpha = 1.0 - transmit;
    if (alpha > 0.0) {
        out.colour = vec4<f32>(encode(light / alpha) * alpha, alpha);
    }
    return out;
}
