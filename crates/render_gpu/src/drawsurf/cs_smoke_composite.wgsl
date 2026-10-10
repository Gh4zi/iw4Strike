// CS2 smoke, second pass (see cs_smoke.rs): lays the marched smoke over the frame. Each pixel
// blends the 3×3 march samples around it, weighted by how close their depth is to its own, so
// the 2×2 stagger of the march's samples evens out while the smoke stays cut sharply against
// whatever stands in front of it.

struct Params {
    world_from_clip: mat4x4<f32>,
    // Width, height, top of the world's depth range, seconds.
    screen: vec4<f32>,
    // Volumes, holes, frame pixels across one march pixel (smoke_quality), march step.
    counts: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
#ifdef MULTISAMPLED
@group(0) @binding(1) var depth_tex: texture_depth_multisampled_2d;
#else
@group(0) @binding(1) var depth_tex: texture_depth_2d;
#endif
@group(0) @binding(2) var smoke_colour: texture_2d<f32>;
@group(0) @binding(3) var smoke_distance: texture_2d<f32>;

// How fast a sample's weight falls as its depth parts from the pixel's (relative difference).
const DEPTH_FALLOFF: f32 = 10.0;

@vertex
fn vs_composite(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn scene_distance(pixel: vec2<f32>, depth: f32) -> f32 {
    if (depth > params.screen.z) {
        return 0.0;
    }
    if (depth <= 0.0) {
        return 1.0e9;
    }
    let ndc = vec2<f32>(
        pixel.x / params.screen.x * 2.0 - 1.0,
        1.0 - pixel.y / params.screen.y * 2.0,
    );
    let hit = params.world_from_clip * vec4<f32>(ndc, depth / params.screen.z, 1.0);
    return length(hit.xyz / hit.w);
}

@fragment
fn fs_composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let half_size = vec2<i32>(textureDimensions(smoke_colour));
    let centre = vec2<i32>(floor(frag.xy)) / max(i32(params.counts.z), 1);
    // No smoke here or beside: nothing to blend (most of the frame).
    var near = textureLoad(smoke_colour, centre, 0).a;
    for (var k = 0; k < 4; k++) {
        let side = vec2<i32>(select(-1, 1, (k & 1) == 1), 0);
        let offset = select(side, side.yx, k >= 2);
        near += textureLoad(smoke_colour, clamp(centre + offset, vec2<i32>(0), half_size - 1), 0).a;
    }
    if (near <= 0.0) {
        return vec4<f32>(0.0);
    }
    let scene = scene_distance(frag.xy, textureLoad(depth_tex, vec2<i32>(floor(frag.xy)), 0));
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    var nearest = vec4<f32>(0.0);
    var nearest_gap = 1.0e9;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let tap = clamp(centre + vec2<i32>(dx, dy), vec2<i32>(0), half_size - 1);
            let colour = textureLoad(smoke_colour, tap, 0);
            let distance = textureLoad(smoke_distance, tap, 0).r;
            let gap = abs(distance - scene) / max(min(distance, scene), 1.0);
            let w = f32((2 - abs(dx)) * (2 - abs(dy))) * exp(-gap * DEPTH_FALLOFF);
            sum += colour * w;
            weight += w;
            if (gap < nearest_gap) {
                nearest_gap = gap;
                nearest = colour;
            }
        }
    }
    if (weight < 1.0e-3) {
        return nearest;
    }
    return sum / weight;
}
