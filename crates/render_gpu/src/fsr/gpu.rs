//! FSR's render-world side. After post-processing, the world's motion vectors come from the
//! scene depth and the camera, the internal image is upscaled into the full-size frame, and
//! `FsrOutput` points the passes after it (CS viewmodel, scope, in-world HUD) at that frame.

use std::num::NonZeroU64;

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::extract_resource::ExtractResourcePlugin;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_depth_2d, texture_storage_2d, uniform_buffer_sized,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor,
    BufferUsages, CachedComputePipelineId, ComputePassDescriptor, ComputePipelineDescriptor,
    Extent3d, FilterMode, LoadOp, Operations, PipelineCache, RenderPassColorAttachment, Sampler,
    SamplerBindingType, SamplerDescriptor, ShaderStages, StorageTextureAccess, StoreOp, Texture,
    TextureDescriptor, TextureDimension, TextureFormat, TextureId, TextureSampleType,
    TextureUsages, TextureView, TextureViewDescriptor,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use super::FsrFrame;
use crate::drawsurf::scene_depth::SceneDepthTexture;

const MOTION_SHADER: &str = "embedded://render_gpu/fsr/motion.wgsl";
const BLIT_SHADER: &str = "embedded://render_gpu/fsr/blit.wgsl";
const MOTION_PARAMS: u64 = 64 * 3 + 16;
const BLIT_PARAMS: u64 = 32;
const MOTION_FORMAT: TextureFormat = TextureFormat::Rg32Float;
const OUTPUT_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
const GROUP: u32 = 8;

/// FSR's full-size frame while FSR is on: the passes after the upscale draw on it instead of
/// the view's own (internal-resolution) target.
#[derive(Resource, Default)]
pub(crate) struct FsrOutput(pub Option<FsrOutputTarget>);

pub(crate) struct FsrOutputTarget {
    pub texture: Texture,
    pub view: TextureView,
}

impl FsrOutputTarget {
    pub fn format(&self) -> TextureFormat {
        self.texture.format()
    }

    pub fn size(&self) -> Extent3d {
        self.texture.size()
    }

    /// Drawn over, never cleared: the upscaled world is already in it.
    pub fn color_attachment(&self) -> RenderPassColorAttachment<'_> {
        RenderPassColorAttachment {
            view: &self.view,
            resolve_target: None,
            ops: Operations {
                load: LoadOp::Load,
                store: StoreOp::Store,
            },
            depth_slice: None,
        }
    }
}

/// The passes after FSR order themselves after this.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FsrSet;

#[derive(Resource)]
struct FsrPipelines {
    motion_layout: BindGroupLayoutDescriptor,
    motion: CachedComputePipelineId,
    motion_params: Buffer,
    blit_layout: BindGroupLayoutDescriptor,
    blit: CachedComputePipelineId,
    blit_params: Buffer,
    linear: Sampler,
}

/// What FSR keeps from frame to frame.
#[derive(Resource, Default)]
pub(crate) struct FsrHistory {
    motion: Option<(UVec2, Texture, TextureView)>,
    output_view: Option<(TextureId, TextureView)>,
    /// Last frame's unjittered `clip_from_world`.
    previous: Option<Mat4>,
}

pub(super) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "motion.wgsl");
    bevy::asset::embedded_asset!(app, "blit.wgsl");
    app.add_plugins(ExtractResourcePlugin::<FsrFrame>::default());
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<FsrOutput>()
        .init_resource::<FsrHistory>()
        .add_systems(RenderStartup, init_pipelines)
        .add_systems(Render, prepare_fsr.in_set(RenderSystems::PrepareResources))
        .add_systems(
            Core3d,
            draw_fsr
                .in_set(Core3dSystems::PostProcess)
                .in_set(FsrSet)
                .after(crate::drawsurf::PostFxSet)
                .before(crate::drawsurf::CsViewmodelSet),
        );
}

fn init_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
) {
    let motion_layout = BindGroupLayoutDescriptor::new(
        "fsr_motion_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer_sized(false, NonZeroU64::new(MOTION_PARAMS)),
                texture_depth_2d(),
                texture_storage_2d(MOTION_FORMAT, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    let blit_layout = BindGroupLayoutDescriptor::new(
        "fsr_blit_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer_sized(false, NonZeroU64::new(BLIT_PARAMS)),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_storage_2d(OUTPUT_FORMAT, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    let compute = |label: &'static str, layout: &BindGroupLayoutDescriptor, path: &'static str| {
        let shader: Handle<Shader> = asset_server.load(path);
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            immediate_size: 0,
            shader,
            shader_defs: Vec::new(),
            entry_point: Some("main".into()),
            zero_initialize_workgroup_memory: false,
        })
    };
    let uniform = |label: &'static str, size: u64| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    commands.insert_resource(FsrPipelines {
        motion: compute("fsr_motion", &motion_layout, MOTION_SHADER),
        motion_params: uniform("fsr_motion_params", MOTION_PARAMS),
        motion_layout,
        blit: compute("fsr_blit", &blit_layout, BLIT_SHADER),
        blit_params: uniform("fsr_blit_params", BLIT_PARAMS),
        blit_layout,
        linear: device.create_sampler(&SamplerDescriptor {
            label: Some("fsr_linear"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
    });
}

/// Points `FsrOutput` at this frame's full-size image and sizes FSR's own textures.
pub(crate) fn prepare_fsr(
    frame: Res<FsrFrame>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    mut output: ResMut<FsrOutput>,
    mut history: ResMut<FsrHistory>,
) {
    output.0 = None;
    if !frame.active {
        *history = FsrHistory::default();
        return;
    }
    let Some(image) = frame.output.as_ref().and_then(|handle| images.get(handle)) else {
        return;
    };
    // The image's own view is sRGB (for the UI); FSR and the passes after it want the plain one.
    let texture = image.texture.clone();
    let view = match history.output_view.take() {
        Some((id, view)) if id == texture.id() => view,
        _ => texture.create_view(&TextureViewDescriptor {
            label: Some("fsr_output"),
            format: Some(OUTPUT_FORMAT),
            ..default()
        }),
    };
    history.output_view = Some((texture.id(), view.clone()));
    output.0 = Some(FsrOutputTarget { texture, view });
    let size = frame.render_size;
    if history
        .motion
        .as_ref()
        .is_none_or(|(current, ..)| *current != size)
    {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("fsr_motion_vectors"),
            size: Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: MOTION_FORMAT,
            usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        history.motion = Some((size, texture, view));
    }
}

fn fsr_debug_motion() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("IW4L_FSR_DEBUG").is_ok_and(|value| value == "motion"))
}

fn push_mat4(bytes: &mut Vec<u8>, matrix: Mat4) {
    for v in matrix.to_cols_array() {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
}

fn push_vec4(bytes: &mut Vec<u8>, v: [f32; 4]) {
    for v in v {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_fsr(
    view: ViewQuery<(&ViewTarget, &SceneDepthTexture)>,
    output: Res<FsrOutput>,
    pipelines: Option<Res<FsrPipelines>>,
    published: Option<Res<crate::drawsurf::PublishedRenderFrame>>,
    mut history: ResMut<FsrHistory>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut context: RenderContext,
) {
    let (target, depth) = view.into_inner();
    let (Some(out), Some(pipelines)) = (output.0.as_ref(), pipelines) else {
        return;
    };
    let (Some(motion_pipeline), Some(blit_pipeline)) = (
        cache.get_compute_pipeline(pipelines.motion),
        cache.get_compute_pipeline(pipelines.blit),
    ) else {
        return;
    };
    let Some((_, _, motion_view)) = history.motion.clone() else {
        return;
    };
    let colour = target.main_texture().size();
    let render = UVec2::new(colour.width, colour.height);
    let display = out.size();

    // Motion vectors, from this frame's and last frame's camera (none on the first frame).
    let exec = published.as_deref().map(|frame| &frame.exec_frame);
    let matrices = exec.and_then(|exec| {
        Some((
            exec.clip_from_world?,
            exec.unjittered_clip_from_world?,
            exec.view_origin,
        ))
    });
    let depth_matches = depth.texture.size() == colour && depth.texture.sample_count() == 1;
    let motion_size = history
        .motion
        .as_ref()
        .map_or(UVec2::ZERO, |(size, ..)| *size);
    let run_motion = depth_matches && motion_size == render;
    if let Some((jittered, unjittered, eye)) = matrices.filter(|_| run_motion) {
        let to_eye = Mat4::from_translation(eye);
        let previous = history.previous.unwrap_or(unjittered);
        history.previous = Some(unjittered);
        let mut bytes = Vec::with_capacity(MOTION_PARAMS as usize);
        push_mat4(&mut bytes, (jittered * to_eye).inverse());
        push_mat4(&mut bytes, unjittered * to_eye);
        push_mat4(&mut bytes, previous * to_eye);
        let size = render.as_vec2();
        push_vec4(&mut bytes, [size.x, size.y, 1.0 / size.x, 1.0 / size.y]);
        queue.write_buffer(&pipelines.motion_params, 0, &bytes);
        let bind = device.create_bind_group(
            "fsr_motion",
            &cache.get_bind_group_layout(&pipelines.motion_layout),
            &BindGroupEntries::sequential((
                pipelines.motion_params.as_entire_buffer_binding(),
                depth.view(),
                &motion_view,
            )),
        );
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("fsr_motion"),
                timestamp_writes: None,
            });
        pass.set_pipeline(motion_pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(render.x.div_ceil(GROUP), render.y.div_ceil(GROUP), 1);
    } else {
        history.previous = None;
    }

    // The upscale into the full-size frame.
    let mut bytes = Vec::with_capacity(BLIT_PARAMS as usize);
    let (w, h) = (display.width as f32, display.height as f32);
    push_vec4(&mut bytes, [w, h, 1.0 / w, 1.0 / h]);
    let debug = if fsr_debug_motion() && run_motion {
        1.0
    } else {
        0.0
    };
    push_vec4(&mut bytes, [render.x as f32, render.y as f32, debug, 0.0]);
    queue.write_buffer(&pipelines.blit_params, 0, &bytes);
    let bind = device.create_bind_group(
        "fsr_blit",
        &cache.get_bind_group_layout(&pipelines.blit_layout),
        &BindGroupEntries::sequential((
            pipelines.blit_params.as_entire_buffer_binding(),
            target.main_texture_view(),
            &pipelines.linear,
            &motion_view,
            &out.view,
        )),
    );
    let mut pass = context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("fsr_upscale"),
            timestamp_writes: None,
        });
    pass.set_pipeline(blit_pipeline);
    pass.set_bind_group(0, &bind, &[]);
    pass.dispatch_workgroups(
        display.width.div_ceil(GROUP),
        display.height.div_ceil(GROUP),
        1,
    );
}
