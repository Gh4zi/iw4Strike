//! Counter-Strike viewmodels: GoldSrc studio models drawn over the world in view space.
//!
//! The main world publishes a [`CsViewmodelFrame`]: the model (vertices uploaded once, one draw
//! per textured mesh) and this frame's bone matrices. Vertices are skinned on the GPU (each rides
//! one bone), projected with CS 1.6's viewmodel field of view, and depth-tested against a depth
//! buffer of their own, so the gun never clips into walls. The pass runs after post-processing
//! and before the 2D HUD.

use std::num::NonZeroU64;
use std::sync::Arc;

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::mesh::VertexBufferLayout;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_cube, uniform_buffer_sized,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent,
    BlendFactor, BlendOperation, BlendState, Buffer, BufferDescriptor, BufferInitDescriptor,
    BufferUsages, ColorTargetState, ColorWrites, CompareFunction, DepthStencilState, Extent3d,
    FragmentState, FrontFace, LoadOp, MultisampleState, Operations, PipelineCache, PrimitiveState,
    RenderPassDepthStencilAttachment, RenderPipelineDescriptor, SamplerBindingType, ShaderStages,
    SpecializedRenderPipeline, SpecializedRenderPipelines, StoreOp, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
    TextureViewDescriptor, TextureViewDimension, VertexAttribute, VertexFormat, VertexState,
    VertexStepMode,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

const SHADER_PATH: &str = "embedded://render_gpu/drawsurf/cs_viewmodel.wgsl";
/// Bones a frame can carry (GoldSrc's `MAXSTUDIOBONES`).
pub const CS_VIEWMODEL_MAX_BONES: usize = 128;
const PARAMS_HEADER: u64 = 160;
const PARAMS_SIZE: u64 = PARAMS_HEADER + CS_VIEWMODEL_MAX_BONES as u64 * 48;
const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;
/// Near plane in view-space units (GoldSrc units).
const NEAR: f32 = 1.0;

/// One model vertex in bind space, skinned by up to four bones (`bones` bytes) with `weights`
/// (255 = 1.0; GoldSrc and Source models leave the fourth at 0). `tangent` (w: the bitangent's
/// sign) orients CS2's normal maps; the others leave it zero.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CsViewmodelVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub bones: [u8; 4],
    pub weights: [u8; 4],
    pub tangent: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CsViewmodelShading {
    Lit,
    /// Lit, with the texture's alpha ignored: CS2's colour textures keep masks there, not cutouts.
    Opaque,
    Fullbright,
    Additive,
    /// Muzzle flash sprites: additive, texture times the vertex normal (used as a tint).
    Flash,
    /// CS2's guns: colour, normal, roughness and metalness, and occlusion maps
    /// ([`CsViewmodelDraw::maps`]), lit with reflections; roughness is the metal map's red.
    PbrWeapon,
    /// CS2's arms: as [`Self::PbrWeapon`], with roughness in the normal map's blue.
    PbrCharacter,
}

impl CsViewmodelShading {
    /// Lit by the map's light (not drawn at full brightness or added).
    #[must_use]
    pub fn is_lit(self) -> bool {
        matches!(
            self,
            Self::Lit | Self::Opaque | Self::PbrWeapon | Self::PbrCharacter
        )
    }
}

/// A CS2 material's maps besides its colour.
#[derive(Clone, Debug)]
pub struct CsViewmodelMaps {
    pub normal: Handle<Image>,
    /// Roughness (red) and metalness (green).
    pub metal: Handle<Image>,
    pub ambient_occlusion: Handle<Image>,
}

/// Muzzle flash quads for one texture, already in view space: each vertex rides the identity
/// bone [`CsViewmodelFrame::bones`] ends with, and its normal carries the tint.
#[derive(Clone, Debug)]
pub struct CsViewmodelFlash {
    pub image: Handle<Image>,
    pub vertices: Vec<CsViewmodelVertex>,
}

#[derive(Clone, Debug)]
pub struct CsViewmodelDraw {
    pub image: Handle<Image>,
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub shading: CsViewmodelShading,
    /// The maps of a [`CsViewmodelShading::PbrWeapon`] or `PbrCharacter` draw.
    pub maps: Option<CsViewmodelMaps>,
}

/// A model's GPU-side description, built once when the model loads.
#[derive(Debug)]
pub struct CsViewmodelModel {
    /// Unique per loaded model; the vertex buffer is rebuilt when it changes.
    pub id: u64,
    pub vertices: Vec<CsViewmodelVertex>,
    pub draws: Vec<CsViewmodelDraw>,
}

/// What the main world wants drawn this frame.
#[derive(Resource, Clone, Debug, Default)]
pub struct CsViewmodelFrame {
    pub model: Option<Arc<CsViewmodelModel>>,
    /// Row-major 3x4 view-space transform per bone (GoldSrc axes: x forward, y left, z up).
    pub bones: Vec<[[f32; 4]; 3]>,
    /// Tangent of half the vertical field of view the model is drawn with.
    pub tan_half_fov_y: f32,
    /// Direction toward the light in view space; ambient and directional light colours (linear).
    pub light_dir: [f32; 3],
    pub ambient: [f32; 3],
    pub shade: [f32; 3],
    /// Direction toward the sun in view space and its colour (zero when the gun is in shadow).
    pub sun_dir: [f32; 3],
    pub sun: [f32; 3],
    /// The view's axes in the world (forward, left, up): CS2's metal reflects the map's reflection
    /// probe by world direction, and a brighter sky above than ground below without one.
    pub view_axes: [[f32; 3]; 3],
    /// The map's reflection probe (a cube map) for where the gun is, which CS2's metal reflects.
    pub reflection: Option<Handle<Image>>,
    /// Exponent applied to texture colour: GoldSrc's texture gamma 0.8, 1 for Source.
    pub texture_gamma: f32,
    /// Muzzle flash sprites drawn over the gun this frame.
    pub flashes: Vec<CsViewmodelFlash>,
}

/// The viewmodel pass; the 2D HUD draws after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CsViewmodelSet;

#[derive(Resource, Default)]
struct ExtractedCsViewmodel(CsViewmodelFrame);

#[derive(Resource)]
struct CsViewmodelPipeline {
    shader: Handle<Shader>,
    layout: BindGroupLayoutDescriptor,
    params: Buffer,
    /// Bound in place of a reflection probe when the map gives none.
    no_reflection: TextureView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CsViewmodelPipelineKey {
    target: TextureFormat,
    shading: CsViewmodelShading,
}

#[derive(Resource, Default)]
struct CsViewmodelGpu {
    model_id: Option<u64>,
    vertices: Option<Buffer>,
    /// This frame's muzzle flash quads, one range per [`CsViewmodelFlash`].
    flash_vertices: Option<Buffer>,
    flash_ranges: Vec<(u32, u32)>,
    depth: Option<(Extent3d, TextureView)>,
    ready: bool,
}

impl SpecializedRenderPipeline for CsViewmodelPipeline {
    type Key = CsViewmodelPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let additive = matches!(
            key.shading,
            CsViewmodelShading::Additive | CsViewmodelShading::Flash
        );
        let fragment = match key.shading {
            CsViewmodelShading::Lit => "fs_viewmodel",
            CsViewmodelShading::Opaque => "fs_viewmodel_opaque",
            CsViewmodelShading::Fullbright => "fs_viewmodel_fullbright",
            CsViewmodelShading::Additive => "fs_viewmodel_additive",
            CsViewmodelShading::Flash => "fs_viewmodel_flash",
            CsViewmodelShading::PbrWeapon => "fs_viewmodel_pbr_weapon",
            CsViewmodelShading::PbrCharacter => "fs_viewmodel_pbr_character",
        };
        let add = BlendComponent {
            src_factor: BlendFactor::One,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        };
        RenderPipelineDescriptor {
            label: Some("cs_viewmodel".into()),
            layout: vec![self.layout.clone()],
            immediate_size: 0,
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some("vs_viewmodel".into()),
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
                        VertexAttribute {
                            format: VertexFormat::Uint32,
                            offset: 32,
                            shader_location: 3,
                        },
                        VertexAttribute {
                            format: VertexFormat::Unorm8x4,
                            offset: 36,
                            shader_location: 4,
                        },
                        VertexAttribute {
                            format: VertexFormat::Float32x4,
                            offset: 40,
                            shader_location: 5,
                        },
                    ],
                }],
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some(fragment.into()),
                targets: vec![Some(ColorTargetState {
                    format: key.target,
                    blend: additive.then_some(BlendState {
                        color: add,
                        alpha: add,
                    }),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState {
                front_face: FrontFace::Ccw,
                cull_mode: None,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(!additive),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
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
        label: Some("cs_viewmodel_params"),
        size: PARAMS_SIZE,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let no_reflection = device
        .create_texture(&TextureDescriptor {
            label: Some("cs_viewmodel_no_reflection"),
            size: Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube),
            ..default()
        });
    commands.insert_resource(CsViewmodelPipeline {
        no_reflection,
        shader: asset_server.load(SHADER_PATH),
        layout: BindGroupLayoutDescriptor::new(
            "cs_viewmodel_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    uniform_buffer_sized(false, NonZeroU64::new(PARAMS_SIZE)),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    // CS2's normal, roughness and metalness, and occlusion maps (the colour
                    // texture stands in for draws without them).
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    // The map's reflection probe.
                    texture_cube(TextureSampleType::Float { filterable: true }),
                ),
            ),
        ),
        params,
    });
}

fn extract_cs_viewmodel(
    mut extracted: ResMut<ExtractedCsViewmodel>,
    frame: Extract<Option<Res<CsViewmodelFrame>>>,
) {
    extracted.0 = frame.as_deref().cloned().unwrap_or_default();
}

fn prepare_cs_viewmodel(
    extracted: Res<ExtractedCsViewmodel>,
    pipeline: Option<Res<CsViewmodelPipeline>>,
    mut gpu: ResMut<CsViewmodelGpu>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    gpu.ready = false;
    let (Some(model), Some(pipeline)) = (extracted.0.model.as_ref(), pipeline) else {
        return;
    };
    let frame = &extracted.0;
    if frame.bones.is_empty() || frame.bones.len() > CS_VIEWMODEL_MAX_BONES {
        return;
    }
    if gpu.model_id != Some(model.id) {
        gpu.vertices = (!model.vertices.is_empty()).then(|| {
            device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("cs_viewmodel_vertices"),
                contents: bytemuck::cast_slice(&model.vertices),
                usage: BufferUsages::VERTEX,
            })
        });
        gpu.model_id = Some(model.id);
    }
    if gpu.vertices.is_none() {
        return;
    }
    gpu.flash_ranges.clear();
    let mut flash = Vec::new();
    for draw in &frame.flashes {
        gpu.flash_ranges
            .push((flash.len() as u32, draw.vertices.len() as u32));
        flash.extend_from_slice(&draw.vertices);
    }
    gpu.flash_vertices = (!flash.is_empty()).then(|| {
        device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("cs_viewmodel_flash"),
            contents: bytemuck::cast_slice(&flash),
            usage: BufferUsages::VERTEX,
        })
    });
    // proj.x (horizontal) is filled in at draw time from the target's aspect; store fy here.
    let fy = 1.0 / frame.tan_half_fov_y.max(0.01);
    let mut bytes = Vec::with_capacity(PARAMS_SIZE as usize);
    for v in [fy, fy, NEAR, 0.0] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    for v in [
        frame.light_dir[0],
        frame.light_dir[1],
        frame.light_dir[2],
        frame.texture_gamma,
    ] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let axes = if frame.view_axes == [[0.0; 3]; 3] {
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    } else {
        frame.view_axes
    };
    for colour in [frame.ambient, frame.shade, frame.sun_dir, frame.sun] {
        for v in [colour[0], colour[1], colour[2], 0.0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    // The axes, then whether a reflection probe is bound.
    let reflection = [f32::from(u8::from(frame.reflection.is_some())), 0.0, 0.0];
    for colour in [axes[0], axes[1], axes[2], reflection] {
        for v in [colour[0], colour[1], colour[2], 0.0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    for bone in &frame.bones {
        for row in bone {
            for v in row {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    queue.write_buffer(&pipeline.params, 0, &bytes);
    gpu.ready = true;
}

fn draw_cs_viewmodel(
    view: ViewQuery<&ViewTarget>,
    extracted: Res<ExtractedCsViewmodel>,
    pipeline: Option<Res<CsViewmodelPipeline>>,
    mut gpu: ResMut<CsViewmodelGpu>,
    mut specialized: ResMut<SpecializedRenderPipelines<CsViewmodelPipeline>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    images: Res<RenderAssets<GpuImage>>,
    mut context: RenderContext,
) {
    let target = view.into_inner();
    let (Some(pipeline), Some(model)) = (pipeline, extracted.0.model.as_ref()) else {
        return;
    };
    if !gpu.ready {
        return;
    }
    let Some(vertices) = gpu.vertices.clone() else {
        return;
    };
    let size = target.main_texture().size();
    if size.width == 0 || size.height == 0 {
        return;
    }
    // Horizontal projection from this view's aspect, vertical field of view held (Hor+).
    let fy = 1.0 / extracted.0.tan_half_fov_y.max(0.01);
    let fx = fy * size.height as f32 / size.width as f32;
    queue.write_buffer(&pipeline.params, 0, &fx.to_le_bytes());

    if gpu.depth.as_ref().is_none_or(|(extent, _)| *extent != size) {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("cs_viewmodel_depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        gpu.depth = Some((size, texture.create_view(&TextureViewDescriptor::default())));
    }
    let Some((_, depth_view)) = gpu.depth.as_ref() else {
        return;
    };

    let format = target.main_texture_format();
    let layout = cache.get_bind_group_layout(&pipeline.layout);
    let reflection = extracted
        .0
        .reflection
        .as_ref()
        .and_then(|probe| images.get(probe))
        .map_or(&pipeline.no_reflection, |probe| &probe.texture_view);
    let mut prepared = Vec::with_capacity(model.draws.len());
    for draw in &model.draws {
        let Some(image) = images.get(&draw.image) else {
            continue;
        };
        // A CS2 draw waits for all its maps.
        let maps = match &draw.maps {
            Some(maps) => {
                let (Some(normal), Some(metal), Some(ao)) = (
                    images.get(&maps.normal),
                    images.get(&maps.metal),
                    images.get(&maps.ambient_occlusion),
                ) else {
                    continue;
                };
                [&normal.texture_view, &metal.texture_view, &ao.texture_view]
            }
            None => [&image.texture_view; 3],
        };
        let id = specialized.specialize(
            &cache,
            &pipeline,
            CsViewmodelPipelineKey {
                target: format,
                shading: draw.shading,
            },
        );
        if cache.get_render_pipeline(id).is_none() {
            continue;
        }
        let bind = device.create_bind_group(
            "cs_viewmodel_mesh",
            &layout,
            &BindGroupEntries::sequential((
                pipeline.params.as_entire_buffer_binding(),
                &image.texture_view,
                &image.sampler,
                maps[0],
                maps[1],
                maps[2],
                reflection,
            )),
        );
        prepared.push((id, bind, draw));
    }
    if prepared.is_empty() {
        return;
    }
    // Opaque and masked meshes first, additive ones over them.
    prepared.sort_by_key(|(_, _, draw)| draw.shading == CsViewmodelShading::Additive);

    let flash_vertices = gpu.flash_vertices.clone();
    let flash_pipeline = flash_vertices.is_some().then(|| {
        specialized.specialize(
            &cache,
            &pipeline,
            CsViewmodelPipelineKey {
                target: format,
                shading: CsViewmodelShading::Flash,
            },
        )
    });
    let flash_binds: Vec<_> = extracted
        .0
        .flashes
        .iter()
        .zip(&gpu.flash_ranges)
        .filter_map(|(flash, &(start, count))| {
            let image = images.get(&flash.image)?;
            let bind = device.create_bind_group(
                "cs_viewmodel_flash",
                &layout,
                &BindGroupEntries::sequential((
                    pipeline.params.as_entire_buffer_binding(),
                    &image.texture_view,
                    &image.sampler,
                    &image.texture_view,
                    &image.texture_view,
                    &image.texture_view,
                    reflection,
                )),
            );
            Some((bind, start, count))
        })
        .collect();

    let attachments = [Some(target.get_unsampled_color_attachment())];
    let mut pass =
        context.begin_tracked_render_pass(bevy::render::render_resource::RenderPassDescriptor {
            label: Some("cs_viewmodel_pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(Operations {
                    load: LoadOp::Clear(0.0),
                    store: StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_vertex_buffer(0, vertices.slice(..));
    for (id, bind, draw) in &prepared {
        let Some(gpu_pipeline) = cache.get_render_pipeline(*id) else {
            continue;
        };
        pass.set_render_pipeline(gpu_pipeline);
        pass.set_bind_group(0, bind, &[]);
        let start = draw.first_vertex;
        pass.draw(start..start + draw.vertex_count, 0..1);
    }

    // Muzzle flash last: added over the gun, hidden where the gun is in front of it.
    if let (Some(flash_vertices), Some(flash_pipeline)) =
        (flash_vertices.as_ref(), flash_pipeline.and_then(|id| cache.get_render_pipeline(id)))
    {
        pass.set_render_pipeline(flash_pipeline);
        pass.set_vertex_buffer(0, flash_vertices.slice(..));
        for (bind, start, count) in &flash_binds {
            pass.set_bind_group(0, bind, &[]);
            pass.draw(*start..*start + *count, 0..1);
        }
    }
}

pub(super) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "cs_viewmodel.wgsl");
    app.init_resource::<CsViewmodelFrame>();
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<ExtractedCsViewmodel>()
        .init_resource::<CsViewmodelGpu>()
        .init_resource::<SpecializedRenderPipelines<CsViewmodelPipeline>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(ExtractSchedule, extract_cs_viewmodel)
        .add_systems(
            Render,
            prepare_cs_viewmodel.in_set(RenderSystems::PrepareResources),
        )
        .add_systems(
            Core3d,
            draw_cs_viewmodel
                .in_set(Core3dSystems::PostProcess)
                .in_set(CsViewmodelSet)
                .after(super::postfx::PostFxSet),
        );
}
