// Counter-Strike sniper scope over the zoomed view: the scope arc texture (one quarter of the
// ring, clear inside the circle) is looked up by distance from the screen centre, so it covers
// all four quarters; black beyond the ring's square, thin black crosshair lines, and the lens
// grime faintly over the glass. Output is premultiplied alpha.

struct Params {
    // xy = screen centre (pixels), z = ring radius (pixels), w = half line width (pixels).
    ring: vec4<f32>,
    // x = lens grime strength (0 without a lens texture).
    lens: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var arc_tex: texture_2d<f32>;
@group(0) @binding(2) var lens_tex: texture_2d<f32>;
@group(0) @binding(3) var scope_samp: sampler;

@vertex
fn vs_scope(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// The main target stores display values; textures sample as linear light.
fn encode(rgb: vec3<f32>) -> vec3<f32> {
    let c = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_scope(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let offset = frag.xy - params.ring.xy;
    let radius = params.ring.z;
    let quarter = abs(offset) / radius;
    // Sampled before any branch: derivatives must stay uniform.
    let arc = textureSampleLevel(arc_tex, scope_samp, min(quarter, vec2<f32>(1.0)), 0.0).a;
    let lens = textureSampleLevel(lens_tex, scope_samp, (offset / radius) * 0.5 + 0.5, 0.0);
    if (quarter.x >= 1.0 || quarter.y >= 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    var alpha = lens.a * params.lens.x;
    var colour = encode(lens.rgb) * alpha;
    if (abs(offset.x) < params.ring.w || abs(offset.y) < params.ring.w) {
        colour = vec3<f32>(0.0);
        alpha = 1.0;
    }
    colour = colour * (1.0 - arc);
    alpha = arc + alpha * (1.0 - arc);
    return vec4<f32>(colour, alpha);
}
