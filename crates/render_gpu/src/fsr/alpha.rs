//! Makes FSR's frame opaque before the window shows it. The window shows it through a UI image,
//! which blends by alpha, and the HUD and menus leave alpha below 1 where they draw (menus mask
//! the map preview through it): without this the preview showed through to black, and
//! translucent overlays dimmed the picture. Without FSR the scene is copied to the window
//! unblended, so its alpha never shows.

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::render_resource::{
    ColorTargetState, ColorWrites, FragmentState, MultisampleState, PipelineCache, PrimitiveState,
    RenderPassDescriptor, RenderPipelineDescriptor, SpecializedRenderPipeline,
    SpecializedRenderPipelines, TextureFormat, VertexState,
};
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::view::ViewTarget;
use bevy::render::{RenderApp, RenderStartup};
use bevy::shader::Shader;

use super::FsrOutput;

#[derive(Resource)]
struct AlphaFillPipeline {
    shader: Handle<Shader>,
}

impl SpecializedRenderPipeline for AlphaFillPipeline {
    type Key = TextureFormat;

    fn specialize(&self, format: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("fsr_alpha_fill".into()),
            layout: Vec::new(),
            immediate_size: 0,
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some("vs".into()),
                buffers: Vec::new(),
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some("fs".into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALPHA,
                })],
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            zero_initialize_workgroup_memory: false,
        }
    }
}

#[derive(Resource)]
struct AlphaFillShader(Handle<Shader>);

pub(super) fn register(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    let shader = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(Shader::from_wgsl(
            include_str!("alpha_fill.wgsl"),
            "fsr/alpha_fill.wgsl",
        ));
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(AlphaFillShader(shader))
        .init_resource::<SpecializedRenderPipelines<AlphaFillPipeline>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(
            Core3d,
            fill_output_alpha
                .in_set(Core3dSystems::PostProcess)
                .after(crate::drawsurf::IwTessSet),
        );
}

fn init_pipeline(mut commands: Commands, shader: Res<AlphaFillShader>) {
    commands.insert_resource(AlphaFillPipeline {
        shader: shader.0.clone(),
    });
}

/// After the HUD, the last pass that draws on FSR's frame.
fn fill_output_alpha(
    _view: ViewQuery<&ViewTarget>,
    output: Res<FsrOutput>,
    pipeline: Option<Res<AlphaFillPipeline>>,
    mut specialized: ResMut<SpecializedRenderPipelines<AlphaFillPipeline>>,
    cache: Res<PipelineCache>,
    mut context: RenderContext,
) {
    let (Some(out), Some(pipeline)) = (output.0.as_ref(), pipeline) else {
        return;
    };
    let id = specialized.specialize(&cache, &pipeline, out.format());
    let Some(gpu_pipeline) = cache.get_render_pipeline(id) else {
        return;
    };
    let attachments = [Some(out.color_attachment())];
    let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("fsr_alpha_fill"),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_render_pipeline(gpu_pipeline);
    pass.draw(0..3, 0..1);
}
