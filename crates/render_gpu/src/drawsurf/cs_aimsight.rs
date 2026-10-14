//! CS:GO's aim down a rifle's scope (the AUG's and SG 553's `aimsight`): while the main world's
//! [`CsAimsightFrame`] has the gun at the scope, the world is copied before the CS viewmodel draws
//! and the whole frame after it. The frame is drawn back blurred and darkened toward the edges
//! (CS:GO's `scope_bluroverlay` with its vignette) everywhere but the scope's lens, which shows the
//! world sharp through the gun's eyepiece, with the reticle dot at its centre. It runs after the
//! CS viewmodel and before the 2D HUD, as CS:GO draws it over the gun.

use std::num::NonZeroU64;

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer_sized};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use bevy::render::{Extract, ExtractSchedule, RenderApp, RenderStartup};
use bevy::shader::Shader;

const SHADER_PATH: &str = "embedded://render_gpu/drawsurf/cs_aimsight.wgsl";
const PARAMS_SIZE: u64 = 48;

/// What the main world wants: how far the gun is into its scope, and where the scope's lens is.
#[derive(Resource, Clone, Debug, Default)]
pub struct CsAimsightFrame {
    /// 0 (not aiming: nothing drawn) to 1 (at the scope).
    pub amount: f32,
    /// The lens's centre on screen, in half heights of the screen from its middle (x right, y
    /// up).
    pub lens_center: [f32; 2],
    /// The lens's radius as a share of the screen's half height.
    pub lens_radius: f32,
    /// The reticle dot's colour (display RGB, 0-1) and its radius as a share of the lens's.
    pub dot_color: [f32; 3],
    pub dot_radius: f32,
    /// CS:GO's `dev/scope_vignette`, multiplied over the blur; a generated falloff without it.
    pub vignette: Option<Handle<Image>>,
}

/// The aim pass; the 2D HUD draws after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CsAimsightSet;

#[derive(Resource, Default)]
struct ExtractedCsAimsight(CsAimsightFrame);

/// A copy of the frame: the world before the viewmodel (the lens shows it) or the whole frame
/// after it (blurred around the lens).
struct FrameCopy {
    texture: Texture,
    view: TextureView,
    format: TextureFormat,
    size: Extent3d,
}

impl FrameCopy {
    /// `held` when it fits `size` and `format`, else a new copy that does.
    fn fitted<'a>(
        held: &'a mut Option<FrameCopy>,
        device: &RenderDevice,
        size: Extent3d,
        format: TextureFormat,
    ) -> &'a FrameCopy {
        if held
            .as_ref()
            .is_none_or(|copy| copy.format != format || copy.size != size)
        {
            let texture = device.create_texture(&TextureDescriptor {
                label: Some("cs_aimsight_copy"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage: TextureUsages::COPY_DST | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&TextureViewDescriptor::default());
            *held = Some(FrameCopy {
                texture,
                view,
                format,
                size,
            });
        }
        held.as_ref().expect("filled above")
    }
}

#[derive(Resource)]
struct CsAimsightPipeline {
    shader: Handle<Shader>,
    layout: BindGroupLayoutDescriptor,
    params: Buffer,
    sampler: Sampler,
    /// A white texel, standing in for the vignette when CS:GO's is not there.
    white: (Texture, TextureView),
}

#[derive(Resource, Default)]
struct CsAimsightCopy {
    scene: Option<FrameCopy>,
    frame: Option<FrameCopy>,
}

impl SpecializedRenderPipeline for CsAimsightPipeline {
    type Key = TextureFormat;

    fn specialize(&self, target: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("cs_aimsight".into()),
            layout: vec![self.layout.clone()],
            immediate_size: 0,
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some("vs_aimsight".into()),
                buffers: Vec::new(),
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: Vec::new(),
                entry_point: Some("fs_aimsight".into()),
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
    queue: Res<RenderQueue>,
) {
    let params = device.create_buffer(&BufferDescriptor {
        label: Some("cs_aimsight_params"),
        size: PARAMS_SIZE,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let white = device.create_texture(&TextureDescriptor {
        label: Some("cs_aimsight_white"),
        size: Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        white.as_image_copy(),
        &[255, 255, 255, 255],
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let white_view = white.create_view(&TextureViewDescriptor::default());
    commands.insert_resource(CsAimsightPipeline {
        shader: asset_server.load(SHADER_PATH),
        layout: BindGroupLayoutDescriptor::new(
            "cs_aimsight_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    uniform_buffer_sized(false, NonZeroU64::new(PARAMS_SIZE)),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                ),
            ),
        ),
        params,
        sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            ..default()
        }),
        white: (white, white_view),
    });
}

fn extract_cs_aimsight(
    mut extracted: ResMut<ExtractedCsAimsight>,
    frame: Extract<Option<Res<CsAimsightFrame>>>,
) {
    extracted.0 = frame.as_deref().cloned().unwrap_or_default();
}

/// The texture the CS passes draw on: FSR's full-size frame while FSR is on, else the view's.
fn drawn_on<'a>(target: &'a ViewTarget, fsr: &'a crate::fsr::FsrOutput) -> &'a Texture {
    fsr.0
        .as_ref()
        .map_or_else(|| target.main_texture(), |out| &out.texture)
}

/// The world before the CS viewmodel draws over it, for the lens.
fn copy_cs_aimsight_scene(
    view: ViewQuery<&ViewTarget>,
    fsr: Res<crate::fsr::FsrOutput>,
    extracted: Res<ExtractedCsAimsight>,
    mut copy: ResMut<CsAimsightCopy>,
    device: Res<RenderDevice>,
    mut context: RenderContext,
) {
    if extracted.0.amount <= 0.0 {
        copy.scene = None;
        return;
    }
    let texture = drawn_on(view.into_inner(), &fsr);
    let (size, format) = (texture.size(), texture.format());
    if size.width == 0 || size.height == 0 {
        return;
    }
    let held = FrameCopy::fitted(&mut copy.scene, &device, size, format);
    context
        .command_encoder()
        .copy_texture_to_texture(texture.as_image_copy(), held.texture.as_image_copy(), size);
}

#[allow(clippy::too_many_arguments)]
fn draw_cs_aimsight(
    view: ViewQuery<&ViewTarget>,
    fsr: Res<crate::fsr::FsrOutput>,
    extracted: Res<ExtractedCsAimsight>,
    pipeline: Option<Res<CsAimsightPipeline>>,
    mut copy: ResMut<CsAimsightCopy>,
    mut specialized: ResMut<SpecializedRenderPipelines<CsAimsightPipeline>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    images: Res<RenderAssets<GpuImage>>,
    mut context: RenderContext,
) {
    let target = view.into_inner();
    let frame = &extracted.0;
    let Some(pipeline) = pipeline.filter(|_| frame.amount > 0.0) else {
        copy.frame = None;
        return;
    };
    // While FSR is on, the blur draws on its full-size frame, after the upscale.
    let (texture, attachment) = match fsr.0.as_ref() {
        Some(out) => (&out.texture, out.color_attachment()),
        None => (target.main_texture(), target.get_unsampled_color_attachment()),
    };
    let (size, format) = (texture.size(), texture.format());
    if size.width == 0 || size.height == 0 {
        return;
    }
    let id = specialized.specialize(&cache, &pipeline, format);
    let Some(gpu_pipeline) = cache.get_render_pipeline(id) else {
        return;
    };
    // The world from before the viewmodel; the first frame of an aim has none yet.
    let Some(scene) = copy
        .scene
        .as_ref()
        .filter(|scene| scene.format == format && scene.size == size)
        .map(|scene| scene.view.clone())
    else {
        return;
    };
    let held = FrameCopy::fitted(&mut copy.frame, &device, size, format);
    context
        .command_encoder()
        .copy_texture_to_texture(texture.as_image_copy(), held.texture.as_image_copy(), size);
    let (w, h) = (size.width as f32, size.height as f32);
    let mut bytes = Vec::with_capacity(PARAMS_SIZE as usize);
    for v in [
        frame.lens_center[0],
        frame.lens_center[1],
        frame.lens_radius,
        frame.amount,
        w / h,
        1.0 / w,
        1.0 / h,
        frame.dot_radius,
        frame.dot_color[0],
        frame.dot_color[1],
        frame.dot_color[2],
        if frame.vignette.is_some() { 1.0 } else { 0.0 },
    ] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    queue.write_buffer(&pipeline.params, 0, &bytes);
    let vignette = frame
        .vignette
        .as_ref()
        .and_then(|handle| images.get(handle))
        .map_or(&pipeline.white.1, |image| &image.texture_view);
    let layout = cache.get_bind_group_layout(&pipeline.layout);
    let bind = device.create_bind_group(
        "cs_aimsight",
        &layout,
        &BindGroupEntries::sequential((
            pipeline.params.as_entire_buffer_binding(),
            &held.view,
            &scene,
            vignette,
            &pipeline.sampler,
        )),
    );
    let attachments = [Some(attachment)];
    let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("cs_aimsight_pass"),
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
    bevy::asset::embedded_asset!(app, "cs_aimsight.wgsl");
    app.init_resource::<CsAimsightFrame>();
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<ExtractedCsAimsight>()
        .init_resource::<CsAimsightCopy>()
        .init_resource::<SpecializedRenderPipelines<CsAimsightPipeline>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(ExtractSchedule, extract_cs_aimsight)
        .add_systems(
            Core3d,
            (
                // After FSR's upscale too: with FSR on, its frame is the one the gun draws on.
                copy_cs_aimsight_scene
                    .in_set(Core3dSystems::PostProcess)
                    .after(super::postfx::PostFxSet)
                    .after(crate::fsr::FsrSet)
                    .before(super::cs_viewmodel::CsViewmodelSet),
                draw_cs_aimsight
                    .in_set(Core3dSystems::PostProcess)
                    .in_set(CsAimsightSet)
                    .after(super::postfx::PostFxSet)
                    .after(super::cs_viewmodel::CsViewmodelSet)
                    .before(super::cs_scope::CsScopeSet),
            ),
        );
}
