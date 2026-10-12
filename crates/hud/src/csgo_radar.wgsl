// The CS:GO radar's map: the overview sampled in a circle around the player, turned with their
// view (see csgo_radar.rs). Outside the overview is black; the rim is smoothed over a pixel.

#import bevy_ui::ui_vertex_output::UiVertexOutput

struct RadarParams {
    // xy: the player's position; z: world units across the radar.
    at: vec4<f32>,
    // The view's forward (xy) and right (zw) on the map.
    axes: vec4<f32>,
    // The overview's east (xy) and south (zw) directions in the world.
    map_axes: vec4<f32>,
    // The overview's upper left corner (xy) and its size in world units (zw).
    map: vec4<f32>,
}

@group(1) @binding(0) var<uniform> radar: RadarParams;
@group(1) @binding(1) var map_tex: texture_2d<f32>;
@group(1) @binding(2) var map_samp: sampler;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // From the radar's middle, -0.5..0.5 across, down the screen positive.
    let d = in.uv - vec2<f32>(0.5);
    let px = d * in.size;
    let edge = clamp(in.size.x * 0.5 - length(px), 0.0, 1.0);
    let world = radar.at.xy + (d.x * radar.axes.zw - d.y * radar.axes.xy) * radar.at.z;
    let delta = world - radar.map.xy;
    let uv = vec2<f32>(dot(radar.map_axes.xy, delta) / radar.map.z, dot(radar.map_axes.zw, delta) / radar.map.w);
    var colour = textureSample(map_tex, map_samp, uv).rgb;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        colour = vec3<f32>(0.0);
    }
    return vec4<f32>(colour, edge);
}
