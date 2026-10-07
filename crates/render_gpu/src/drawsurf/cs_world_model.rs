//! Counter-Strike weapons in the world: Counter-Strike: Source world models (`w_*.mdl`) in
//! players' hands, seen in third person.
//!
//! The main world publishes a [`CsWorldModelsFrame`]: each model (vertices uploaded once, rest
//! pose already baked into them, one draw per textured mesh) with its world placement and the
//! light where it is. They are drawn in world space with the view's own matrices, after the
//! world and against its depth, so walls and players in front hide them.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Arc;

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::mesh::VertexBufferLayout;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, uniform_buffer, uniform_buffer_sized,
};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer,
    BufferInitDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction,
    DepthStencilState, FragmentState, FrontFace, MultisampleState, PipelineCache, PrimitiveState,
    RenderPipelineDescriptor, SamplerBindingType, ShaderStages, SpecializedRenderPipeline,
    SpecializedRenderPipelines, StoreOp, TextureFormat, TextureSampleType, VertexAttribute,
    VertexFormat, VertexState, VertexStepMode,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::{
    ExtractedView, Msaa, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms,
};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use super::cs_viewmodel::{CsViewmodelModel, CsViewmodelShading, CsViewmodelVertex};
use super::depth_range::{GFX_DEPTH_RANGE_SCENE, reverse_z_viewport_depth};
use super::scene_depth::{SCENE_DEPTH_FORMAT, SceneDepthTexture};

const SHADER_PATH: &str = "embedded://render_gpu/drawsurf/cs_world_model.wgsl";
/// `Instance` in the shader: a 4x4 matrix and three vec4s.
const INSTANCE_SIZE: u64 = 64 + 3 * 16;

/// One CS model placed in the world this frame.
#[derive(Clone, Debug)]
pub struct CsWorldModelInstance {
    pub model: Arc<CsViewmodelModel>,
    /// Column-major world-from-model matrix (MW2 world, inches).
    pub world_from_model: [[f32; 4]; 4],
    /// Ambient light colour (linear).
    pub ambient: [f32; 3],
    /// Direction toward the sun (world) and its colour; zero when the model is in shadow.
    pub sun_dir: [f32; 3],
    pub sun: [f32; 3],
}

/// What the main world wants drawn this frame.
#[derive(Resource, Clone, Debug, Default)]
pub struct CsWorldModelsFrame {
    pub instances: Vec<CsWorldModelInstance>,
}

#[derive(Resource, Default)]
struct ExtractedCsWorldModels(CsWorldModelsFrame);

#[derive(Resource)]
struct CsWorldModelPipeline {
    shader: Handle<Shader>,
    view_layout: BindGroupLayoutDescriptor,
    mesh_layout: BindGroupLayoutDescriptor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CsWorldModelPipelineKey {
    target: TextureFormat,
    samples: u32,
    fullbright: bool,
}

/// Vertex buffers by model id, and this frame's per-instance uniforms.
#[derive(Resource, Default)]
struct CsWorldModelGpu {
    vertices: HashMap<u64, Buffer>,
    instances: Vec<Buffer>,
}

#[derive(Component)]
struct CsWorldModelViewBind(BindGroup);

impl SpecializedRenderPipeline for CsWorldModelPipeline {
    type Key = CsWorldModelPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("cs_world_model".into()),
            layout: vec![self.view_layout.clone(), self.mesh_layout.clone()],
            immediate_size: 0,
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some("vs_world".into()),
                buffers: vec![VertexBufferLayout {
                    array_stride: core::mem::size_of::<CsViewmodelVertex>() as u64,
                    step_mode: VertexStepMode::Vertex,
                    attributes: vec![
                        VertexAttribute {
                            format: VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        VertexAttribute {
                            format: VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        VertexAttribute {
                            format: VertexFormat::Float32x2,
                            offset: 24,
                            shader_location: 2,
                        },
                    ],
                }],
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some(
                    if key.fullbright {
                        "fs_world_fullbright"
                    } else {
                        "fs_world"
                    }
                    .into(),
                ),
                targets: vec![Some(ColorTargetState {
                    format: key.target,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState {
                front_face: FrontFace::Ccw,
                cull_mode: None,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: SCENE_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: MultisampleState {
                count: key.samples,
                ..default()
            },
            zero_initialize_workgroup_memory: false,
        }
    }
}

fn init_pipeline(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(CsWorldModelPipeline {
        shader: asset_server.load(SHADER_PATH),
        view_layout: BindGroupLayoutDescriptor::new(
            "cs_world_model_view",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (uniform_buffer::<ViewUniform>(true),),
            ),
        ),
        mesh_layout: BindGroupLayoutDescriptor::new(
            "cs_world_model_mesh",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    uniform_buffer_sized(false, NonZeroU64::new(INSTANCE_SIZE)),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                ),
            ),
        ),
    });
}

fn extract_cs_world_models(
    mut extracted: ResMut<ExtractedCsWorldModels>,
    frame: Extract<Option<Res<CsWorldModelsFrame>>>,
) {
    extracted.0 = frame.as_deref().cloned().unwrap_or_default();
}

fn prepare_cs_world_models(
    extracted: Res<ExtractedCsWorldModels>,
    mut gpu: ResMut<CsWorldModelGpu>,
    device: Res<RenderDevice>,
) {
    let gpu = &mut *gpu;
    gpu.instances.clear();
    // Upload models seen for the first time; keep the buffers of the ones still drawn.
    gpu.vertices.retain(|id, _| {
        extracted
            .0
            .instances
            .iter()
            .any(|instance| instance.model.id == *id)
    });
    for instance in &extracted.0.instances {
        let model = &instance.model;
        if !model.vertices.is_empty() && !gpu.vertices.contains_key(&model.id) {
            let buffer = device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("cs_world_model_vertices"),
                contents: bytemuck::cast_slice(&model.vertices),
                usage: BufferUsages::VERTEX,
            });
            gpu.vertices.insert(model.id, buffer);
        }
        let mut bytes = Vec::with_capacity(INSTANCE_SIZE as usize);
        for column in instance.world_from_model {
            for v in column {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        for colour in [instance.ambient, instance.sun_dir, instance.sun] {
            for v in [colour[0], colour[1], colour[2], 0.0] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        gpu.instances
            .push(device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("cs_world_model_instance"),
                contents: &bytes,
                usage: BufferUsages::UNIFORM,
            }));
    }
}

fn prepare_views(
    mut commands: Commands,
    pipeline: Option<Res<CsWorldModelPipeline>>,
    extracted: Res<ExtractedCsWorldModels>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    uniforms: Res<ViewUniforms>,
    views: Query<Entity, With<ExtractedView>>,
) {
    if extracted.0.instances.is_empty() {
        return;
    }
    let (Some(pipeline), Some(binding)) = (pipeline, uniforms.uniforms.binding()) else {
        return;
    };
    for entity in &views {
        let bind = device.create_bind_group(
            "cs_world_model_view",
            &cache.get_bind_group_layout(&pipeline.view_layout),
            &BindGroupEntries::single(binding.clone()),
        );
        commands.entity(entity).insert(CsWorldModelViewBind(bind));
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_cs_world_models(
    view: ViewQuery<(
        &ViewTarget,
        &SceneDepthTexture,
        &ExtractedView,
        &ViewUniformOffset,
        &Msaa,
        Option<&CsWorldModelViewBind>,
    )>,
    extracted: Res<ExtractedCsWorldModels>,
    pipeline: Option<Res<CsWorldModelPipeline>>,
    gpu: Res<CsWorldModelGpu>,
    mut specialized: ResMut<SpecializedRenderPipelines<CsWorldModelPipeline>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    images: Res<RenderAssets<GpuImage>>,
    mut context: RenderContext,
) {
    let (target, depth, extracted_view, view_offset, msaa, view_bind) = view.into_inner();
    let (Some(pipeline), Some(view_bind)) = (pipeline, view_bind) else {
        return;
    };
    if extracted.0.instances.is_empty() || gpu.instances.len() != extracted.0.instances.len() {
        return;
    }
    let layout = cache.get_bind_group_layout(&pipeline.mesh_layout);
    let mut prepared = Vec::new();
    for (instance, uniform) in extracted.0.instances.iter().zip(&gpu.instances) {
        let Some(vertices) = gpu.vertices.get(&instance.model.id) else {
            continue;
        };
        for draw in &instance.model.draws {
            let Some(image) = images.get(&draw.image) else {
                continue;
            };
            let id = specialized.specialize(
                &cache,
                &pipeline,
                CsWorldModelPipelineKey {
                    target: extracted_view.target_format,
                    samples: msaa.samples(),
                    fullbright: draw.shading != CsViewmodelShading::Lit,
                },
            );
            if cache.get_render_pipeline(id).is_none() {
                continue;
            }
            let bind = device.create_bind_group(
                "cs_world_model_mesh",
                &layout,
                &BindGroupEntries::sequential((
                    uniform.as_entire_buffer_binding(),
                    &image.texture_view,
                    &image.sampler,
                )),
            );
            prepared.push((id, bind, vertices, draw.first_vertex, draw.vertex_count));
        }
    }
    if prepared.is_empty() {
        return;
    }
    let attachments = [Some(target.get_color_attachment())];
    let mut pass =
        context.begin_tracked_render_pass(bevy::render::render_resource::RenderPassDescriptor {
            label: Some("cs_world_model_pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    // The world's own slice of the depth range (the viewmodel has the rest).
    let viewport = extracted_view.viewport;
    let (min_depth, max_depth) = reverse_z_viewport_depth(GFX_DEPTH_RANGE_SCENE);
    pass.set_viewport(
        viewport.x as f32,
        viewport.y as f32,
        viewport.z as f32,
        viewport.w as f32,
        min_depth,
        max_depth,
    );
    pass.set_bind_group(0, &view_bind.0, &[view_offset.offset]);
    for (id, bind, vertices, first, count) in &prepared {
        let Some(gpu_pipeline) = cache.get_render_pipeline(*id) else {
            continue;
        };
        pass.set_render_pipeline(gpu_pipeline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.draw(*first..*first + *count, 0..1);
    }
}

pub(super) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "cs_world_model.wgsl");
    app.init_resource::<CsWorldModelsFrame>();
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<ExtractedCsWorldModels>()
        .init_resource::<CsWorldModelGpu>()
        .init_resource::<SpecializedRenderPipelines<CsWorldModelPipeline>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(ExtractSchedule, extract_cs_world_models)
        .add_systems(
            Render,
            (
                prepare_cs_world_models.in_set(RenderSystems::PrepareResources),
                prepare_views.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        .add_systems(
            Core3d,
            draw_cs_world_models
                .in_set(Core3dSystems::MainPass)
                .after(super::draw::ExactColourDrawSet),
        );
}
