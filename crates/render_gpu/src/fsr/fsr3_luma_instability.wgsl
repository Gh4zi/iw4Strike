// FSR 3.1 luma instability (ffx_fsr3upscaler_luma_instability.h): keeps each pixel's last four
// lumas and flags pixels that flicker back to an older value, so the accumulation can trust
// the history there.

@group(0) @binding(1) var dilated_reactive_masks: texture_2d<f32>;
@group(0) @binding(2) var dilated_motion: texture_2d<f32>;
@group(0) @binding(3) var prev_luma_history: texture_2d<f32>;
@group(0) @binding(4) var current_luma: texture_2d<f32>;
@group(0) @binding(5) var linear_clamp: sampler;
@group(0) @binding(6) var luma_history: texture_storage_2d<rgba16float, write>;
@group(0) @binding(7) var luma_instability: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pos = vec2<i32>(id.xy);
    if !is_on_screen(pos, render_size()) {
        return;
    }
    var history = vec4<f32>(0.0);
    var instability = 0.0;
    let motion = textureLoad(dilated_motion, pos, 0).xy;
    let uv = (vec2<f32>(pos) + 0.5) / render_size_f();
    let uv_current = uv + fsr.jitter / render_size_f();
    let uv_previous = uv + fsr.prev_jitter / vec2<f32>(fsr.prev_render_size);
    let reprojected = uv_previous + motion;
    let resource_size = vec2<i32>(textureDimensions(current_luma));
    if is_uv_inside(reprojected) {
        let masks_uv = clamp_uv(uv_current, render_size(), resource_size);
        let masks = textureSampleLevel(dilated_reactive_masks, linear_clamp, masks_uv, 0.0);
        let reactive = saturate(masks[REACTIVE]);
        let disocclusion = saturate(masks[DISOCCLUSION]);
        let shading = saturate(masks[SHADING_CHANGE]);
        let accumulated = saturate(masks[ACCUMULATION]);
        if accumulated > 0.9 {
            let luma = textureSampleLevel(current_luma, linear_clamp, masks_uv, 0.0).x;
            let history_uv = clamp_uv(
                reprojected,
                fsr.prev_render_size,
                vec2<i32>(textureDimensions(prev_luma_history)),
            );
            history = textureSampleLevel(prev_luma_history, linear_clamp, history_uv, 0.0);

            // Is this frame's luma closer to an older one than to last frame's, on the same side?
            let diff0 = luma - history[0];
            let similarity0 = min_divided_by_max(luma, history[0], 1.0);
            var max_similarity = similarity0;
            var flicker = 0.0;
            if similarity0 < 1.0 {
                for (var i = 1; i <= 3; i++) {
                    let diff1 = luma - history[i];
                    let similarity1 = min_divided_by_max(luma, history[i], 0.0);
                    if sign(diff0) == sign(diff1) {
                        max_similarity = max(max_similarity, similarity1);
                    }
                }
                flicker = f32(max_similarity > similarity0);
            }
            history = vec4<f32>(luma, history.x, history.y, history.z);
            instability = flicker * f32(history.w != 0.0);
            let velocity_weight = 1.0 - saturate(get_4k_velocity(motion) / 20.0);
            instability *= velocity_weight * (1.0 - disocclusion) * (1.0 - reactive)
                * (1.0 - shading);
        }
    }
    textureStore(luma_history, pos, history);
    textureStore(luma_instability, pos, vec4<f32>(instability, 0.0, 0.0, 0.0));
}
