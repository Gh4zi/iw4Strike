// CS:GO's aim down a rifle's scope: the copied frame blurred and multiplied by the vignette
// everywhere but the scope's lens, which shows the world as it was before the gun drew over it,
// all faded in with the aim, and the reticle dot at the lens's centre. Output is premultiplied
// alpha.

struct Params {
    // xy = lens centre (half heights from the middle, x right, y up), z = lens radius (half
    // heights), w = aim amount.
    lens: vec4<f32>,
    // x = width / height, yz = one pixel in uv, w = dot radius (share of the lens's).
    screen: vec4<f32>,
    // xyz = dot colour (display RGB), w = 1 with the vignette texture.
    dot: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var frame_tex: texture_2d<f32>;
@group(0) @binding(2) var scene_tex: texture_2d<f32>;
@group(0) @binding(3) var vignette_tex: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;

// Blur radius, as a share of the screen's half height.
const BLUR: f32 = 0.025;

// Two rings of taps around the centre (unit disc).
var<private> TAPS: array<vec2<f32>, 16> = array<vec2<f32>, 16>(
    vec2<f32>(0.5, 0.0), vec2<f32>(0.354, 0.354), vec2<f32>(0.0, 0.5), vec2<f32>(-0.354, 0.354),
    vec2<f32>(-0.5, 0.0), vec2<f32>(-0.354, -0.354), vec2<f32>(0.0, -0.5), vec2<f32>(0.354, -0.354),
    vec2<f32>(0.924, 0.383), vec2<f32>(0.383, 0.924), vec2<f32>(-0.383, 0.924), vec2<f32>(-0.924, 0.383),
    vec2<f32>(-0.924, -0.383), vec2<f32>(-0.383, -0.924), vec2<f32>(0.383, -0.924), vec2<f32>(0.924, -0.383),
);

@vertex
fn vs_aimsight(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
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
fn fs_aimsight(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = params.screen.yz;
    let aspect = params.screen.x;
    let uv = frag.xy * texel;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let amount = params.lens.w;
    let radius = params.lens.z;
    // Distance from the lens's centre in half heights, and a pixel and a half in the same units.
    let dist = length(vec2<f32>(ndc.x * aspect - params.lens.x, ndc.y - params.lens.y));
    let edge = 3.0 * texel.y;

    // The blur: the centre and two rings, weighted toward the middle.
    let spread = vec2<f32>(BLUR * 0.5 / aspect, BLUR * 0.5);
    var sum = textureSampleLevel(frame_tex, samp, uv, 0.0).rgb * 2.0;
    var weight = 2.0;
    for (var i = 0u; i < 16u; i = i + 1u) {
        let w = select(0.75, 1.5, i < 8u);
        sum = sum + textureSampleLevel(frame_tex, samp, uv + TAPS[i] * spread, 0.0).rgb * w;
        weight = weight + w;
    }
    let blurred = sum / weight;

    // CS:GO's vignette over the blur (`$detailblendmode` 8, multiply), else a soft falloff.
    let from_middle = length(vec2<f32>(ndc.x * aspect, ndc.y)) / aspect;
    var vignette = vec3<f32>(1.0 - smoothstep(0.3, 1.1, from_middle) * 0.6);
    if (params.dot.w > 0.5) {
        vignette = encode(textureSampleLevel(vignette_tex, samp, uv, 0.0).rgb);
    }

    let inside = 1.0 - smoothstep(radius - edge, radius, dist);
    let sharp = textureSampleLevel(scene_tex, samp, uv, 0.0).rgb;
    var alpha = amount;
    var colour = mix(blurred * vignette, sharp, inside) * alpha;

    // The reticle dot, once the gun is nearly at the scope.
    let dot_radius = max(radius * params.screen.w, 1.5 * edge / 3.0);
    let dot = (1.0 - smoothstep(dot_radius - edge * 0.5, dot_radius + edge * 0.5, dist))
        * smoothstep(0.8, 1.0, amount);
    colour = colour * (1.0 - dot) + params.dot.rgb * dot;
    alpha = alpha + dot * (1.0 - alpha);
    return vec4<f32>(colour, alpha);
}
