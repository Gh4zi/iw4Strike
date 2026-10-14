use bevy::prelude::*;

pub struct RenderGpuPlugin;

impl Plugin for RenderGpuPlugin {
    fn build(&self, app: &mut App) {
        match crate::SamplerTable::captured_2026_08_11() {
            Ok(table) => {
                let adapted = table.host_adapted_rows();
                if !adapted.is_empty() {
                    diag::warn!(
                        World,
                        "drawsurf sampler host adaptation: {} of 24 table rows lift mip filter to linear for anisotropy (rows={adapted:?})",
                        adapted.len()
                    );
                }
                app.insert_resource(table);
            }
            Err(cause) => diag::warn!(World, "drawsurf sampler profile: cause={cause:?}"),
        }
        app.add_systems(Update, apply_texture_filter);
        crate::drawsurf::register_drawsurf_render(app);
        crate::fsr::register(app);
        app.init_resource::<crate::GpuSubmitReady>()
            .init_resource::<crate::ColourWorkingSet>();
        let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) else {
            return;
        };
        render_app
            .init_resource::<crate::GpuSubmitReady>()
            .init_resource::<crate::ColourWorkingSet>()
            .init_resource::<crate::WorldPipelineWarmup>();
    }
}

/// `r_texture_filter`: rebuilds MW2's sampler table (as its `r_texFilterAnisoMin/Max` and
/// `r_texFilterMipMode` would) when the setting changes. Automatic keeps the retail table; the
/// others force every filtered texture to one filter, as a driver override does. The render
/// world sees the new table on its next extract and rebuilds its samplers.
fn apply_texture_filter(
    settings: Res<frame::GameSettings>,
    table: Option<ResMut<crate::SamplerTable>>,
    mut applied: Local<Option<u8>>,
) {
    let Some(mut table) = table else {
        return;
    };
    if *applied == Some(settings.texture_filter) {
        return;
    }
    *applied = Some(settings.texture_filter);
    let filter = frame::settings::TextureFilter::from_setting(settings.texture_filter);
    let mut inputs = crate::RETAIL_SAMPLER_PROFILE_2026_08_11;
    if let Some(anisotropy) = filter.forced_anisotropy() {
        inputs.max_anisotropy = anisotropy;
        inputs.min_anisotropy = anisotropy;
        // 1 forces trilinear, 2 bilinear (MW2's `r_texFilterMipMode`).
        inputs.mip_mode = if filter == frame::settings::TextureFilter::Bilinear {
            2
        } else {
            1
        };
    }
    match crate::SamplerTable::produce(inputs) {
        Ok(next) => {
            if *table != next {
                *table = next;
            }
            diag::info!(World, "texture filter: {filter:?}");
        }
        Err(cause) => diag::warn!(World, "texture filter {filter:?}: cause={cause:?}"),
    }
}
