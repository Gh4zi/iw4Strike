//! Counter-Strike sniper scope: while the main world's [`CsScopeFrame`] is active, one fullscreen
//! triangle draws the scope ring, its black surround, crosshair lines and lens grime over the
//! zoomed view. It runs after the CS viewmodel and before the 2D HUD, so the HUD stays readable
//! over the scope as in CS.

use std::num::NonZeroU64;

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer_sized};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendState, Buffer,
    BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, FragmentState, MultisampleState,
    PipelineCache, PrimitiveState, RenderPipelineDescriptor, SamplerBindingType, ShaderStages,
    SpecializedRenderPipeline, SpecializedRenderPipelines, TextureFormat, TextureSampleType,
    VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use bevy::render::{Extract, ExtractSchedule, RenderApp, RenderStartup};
use bevy::shader::Shader;

const SHADER_PATH: &str = "embedded://render_gpu/drawsurf/cs_scope.wgsl";
const PARAMS_SIZE: u64 = 32;
/// The ring stops this fraction of the screen height short of the top and bottom.
const INSET: f32 = 1.0 / 16.0;
/// Line width: one pixel of a 480-line screen.
const VIRTUAL_HEIGHT: f32 = 480.0;

/// What the main world wants: the scope while zoomed, with its arc (a quarter of the ring,
/// clear inside) and optional lens grime.
#[derive(Resource, Clone, Debug, Default)]
pub struct CsScopeFrame {
    pub active: bool,
    pub arc: Option<Handle<Image>>,
    pub lens: Option<Handle<Image>>,
    /// How strongly the grime shows.
    pub lens_alpha: f32,
}

/// The scope pass; the 2D HUD draws after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CsScopeSet;

#[derive(Resource, Default)]
struct ExtractedCsScope(CsScopeFrame);

#[derive(Resource)]
struct CsScopePipeline {
    shader: Handle<Shader>,
    layout: BindGroupLayoutDescriptor,
    params: Buffer,
}

impl SpecializedRenderPipeline for CsScopePipeline {
    type Key = TextureFormat;

    fn specialize(&self, target: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("cs_scope".into()),
            layout: vec![self.layout.clone()],
            immediate_size: 0,
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some("vs_scope".into()),
                buffers: Vec::new(),
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some("fs_scope".into()),
                targets: vec![Some(ColorTargetState {
                    format: target,
                    blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            zero_initialize_workgroup_memory: false,
        }
    }
}

fn init_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    device: Res<RenderDevice>,
) {
    let params = device.create_buffer(&BufferDescriptor {
        label: Some("cs_scope_params"),
        size: PARAMS_SIZE,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    commands.insert_resource(CsScopePipeline {
        shader: asset_server.load(SHADER_PATH),
        layout: BindGroupLayoutDescriptor::new(
            "cs_scope_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    uniform_buffer_sized(false, NonZeroU64::new(PARAMS_SIZE)),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                ),
            ),
        ),
        params,
    });
}

fn extract_cs_scope(
    mut extracted: ResMut<ExtractedCsScope>,
    frame: Extract<Option<Res<CsScopeFrame>>>,
) {
    extracted.0 = frame.as_deref().cloned().unwrap_or_default();
}

#[allow(clippy::too_many_arguments)]
fn draw_cs_scope(
    view: ViewQuery<&ViewTarget>,
    extracted: Res<ExtractedCsScope>,
    pipeline: Option<Res<CsScopePipeline>>,
    mut specialized: ResMut<SpecializedRenderPipelines<CsScopePipeline>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    images: Res<RenderAssets<GpuImage>>,
    mut context: RenderContext,
) {
    let target = view.into_inner();
    let frame = &extracted.0;
    let Some(pipeline) = pipeline.filter(|_| frame.active) else {
        return;
    };
    let Some(arc) = frame.arc.as_ref().and_then(|handle| images.get(handle)) else {
        return;
    };
    let lens = frame.lens.as_ref().and_then(|handle| images.get(handle));
    let size = target.main_texture().size();
    if size.width == 0 || size.height == 0 {
        return;
    }
    let id = specialized.specialize(&cache, &pipeline, target.main_texture_format());
    let Some(gpu_pipeline) = cache.get_render_pipeline(id) else {
        return;
    };
    let (w, h) = (size.width as f32, size.height as f32);
    let radius = (h * 0.5 - (h * INSET).round()).max(1.0);
    let half_line = ((h / VIRTUAL_HEIGHT).round().max(1.0) * 0.5).max(0.5);
    let lens_alpha = if lens.is_some() {
        frame.lens_alpha
    } else {
        0.0
    };
    let mut bytes = Vec::with_capacity(PARAMS_SIZE as usize);
    for v in [
        (w * 0.5).round(),
        (h * 0.5).round(),
        radius,
        half_line,
        lens_alpha,
        0.0,
        0.0,
        0.0,
    ] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    queue.write_buffer(&pipeline.params, 0, &bytes);
    let lens = lens.unwrap_or(arc);
    let layout = cache.get_bind_group_layout(&pipeline.layout);
    let bind = device.create_bind_group(
        "cs_scope",
        &layout,
        &BindGroupEntries::sequential((
            pipeline.params.as_entire_buffer_binding(),
            &arc.texture_view,
            &lens.texture_view,
            &arc.sampler,
        )),
    );
    let attachments = [Some(target.get_unsampled_color_attachment())];
    let mut pass =
        context.begin_tracked_render_pass(bevy::render::render_resource::RenderPassDescriptor {
            label: Some("cs_scope_pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_render_pipeline(gpu_pipeline);
    pass.set_bind_group(0, &bind, &[]);
    pass.draw(0..3, 0..1);
}

pub(super) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "cs_scope.wgsl");
    app.init_resource::<CsScopeFrame>();
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<ExtractedCsScope>()
        .init_resource::<SpecializedRenderPipelines<CsScopePipeline>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(ExtractSchedule, extract_cs_scope)
        .add_systems(
            Core3d,
            draw_cs_scope
                .in_set(Core3dSystems::PostProcess)
                .in_set(CsScopeSet)
                .after(super::postfx::PostFxSet)
                .after(super::cs_viewmodel::CsViewmodelSet),
        );
}
