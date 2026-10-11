//! FSR's render-world side. After post-processing, the world's motion vectors come from the
//! scene depth and the camera, FSR 3.1's upscaler passes (AMD's shaders ported to WGSL, see
//! `fsr3_common.wgsl`) accumulate the jittered frames into the full-size frame, and
//! `FsrOutput` points the passes after it (CS viewmodel, scope, in-world HUD) at that frame.
//! `IW4L_FSR_DEBUG=motion` swaps the upscale for a bilinear stretch tinted with the motion.

use std::num::NonZeroU64;

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::camera::TemporalJitter;
use bevy::render::extract_resource::ExtractResourcePlugin;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer_read_only_sized, storage_buffer_sized, texture_2d, texture_depth_2d,
    texture_storage_2d, uniform_buffer_sized,
};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
    BindGroupLayoutEntryBuilder, Buffer, BufferDescriptor, BufferUsages, CachedComputePipelineId,
    ComputePass, ComputePassDescriptor, ComputePipelineDescriptor, Extent3d, FilterMode, LoadOp,
    Operations, PipelineCache, RenderPassColorAttachment, Sampler, SamplerBindingType,
    SamplerDescriptor, ShaderStages, StorageTextureAccess, StoreOp, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureId, TextureSampleType, TextureUsages, TextureView,
    TextureViewDescriptor,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use super::FsrFrame;
use crate::drawsurf::scene_depth::SceneDepthTexture;

const MOTION_PARAMS: u64 = 64 * 3 + 16;
const BLIT_PARAMS: u64 = 32;
/// `Fsr3` in fsr3_common.wgsl.
const FSR_PARAMS: u64 = 128;
const MOTION_FORMAT: TextureFormat = TextureFormat::Rg32Float;
const OUTPUT_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
const GROUP: u32 = 8;
/// The shading change pass reads the first three mips of its pyramid.
const SHADING_MIPS: u32 = 3;
/// CoD's units are inches; FSR's thresholds are in meters.
const METERS_PER_UNIT: f32 = 0.0254;
/// A camera move this long in one frame is a cut (spawn, teleport, killcam): history restarts.
const CUT_DISTANCE: f32 = 128.0;

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

/// FSR 3.1's passes, in dispatch order.
#[derive(Clone, Copy)]
enum Pass {
    PrepareInputs,
    FarthestDepth,
    ShadingPyramid,
    Downsample,
    ShadingChange,
    PrepareReactivity,
    LumaInstability,
    Accumulate,
    Rcas,
}

const PASSES: [(Pass, &str, &str); 9] = [
    (
        Pass::PrepareInputs,
        "fsr3_prepare_inputs",
        include_str!("fsr3_prepare_inputs.wgsl"),
    ),
    (
        Pass::FarthestDepth,
        "fsr3_farthest_depth",
        include_str!("fsr3_farthest_depth.wgsl"),
    ),
    (
        Pass::ShadingPyramid,
        "fsr3_shading_change_pyramid",
        include_str!("fsr3_shading_change_pyramid.wgsl"),
    ),
    (
        Pass::Downsample,
        "fsr3_downsample",
        include_str!("fsr3_downsample.wgsl"),
    ),
    (
        Pass::ShadingChange,
        "fsr3_shading_change",
        include_str!("fsr3_shading_change.wgsl"),
    ),
    (
        Pass::PrepareReactivity,
        "fsr3_prepare_reactivity",
        include_str!("fsr3_prepare_reactivity.wgsl"),
    ),
    (
        Pass::LumaInstability,
        "fsr3_luma_instability",
        include_str!("fsr3_luma_instability.wgsl"),
    ),
    (
        Pass::Accumulate,
        "fsr3_accumulate",
        include_str!("fsr3_accumulate.wgsl"),
    ),
    (Pass::Rcas, "fsr3_rcas", include_str!("fsr3_rcas.wgsl")),
];

#[derive(Resource)]
struct FsrShaders {
    motion: Handle<Shader>,
    blit: Handle<Shader>,
    passes: Vec<Handle<Shader>>,
}

#[derive(Resource)]
struct FsrPipelines {
    motion_layout: BindGroupLayoutDescriptor,
    motion: CachedComputePipelineId,
    motion_params: Buffer,
    blit_layout: BindGroupLayoutDescriptor,
    blit: CachedComputePipelineId,
    blit_params: Buffer,
    pass_layouts: Vec<BindGroupLayoutDescriptor>,
    passes: Vec<CachedComputePipelineId>,
    params: Buffer,
    linear: Sampler,
}

/// One of FSR's textures (the view keeps it alive).
struct Tex {
    view: TextureView,
}

/// FSR's own textures, sized for one render and display size.
struct FsrTextures {
    render: UVec2,
    display: UVec2,
    dilated_motion: Tex,
    dilated_depth: Tex,
    farthest_depth: Tex,
    /// This and last frame's luma (by parity).
    luma: [Tex; 2],
    luma_history: [Tex; 2],
    luma_instability: Tex,
    reactive_masks: Tex,
    accumulation: [Tex; 2],
    /// Last frame's depth pushed along the motion (atomic max of the float bits).
    reconstructed_depth: Buffer,
    farthest_depth_mip1: Tex,
    shading_pyramid: Texture,
    shading_pyramid_all: TextureView,
    shading_pyramid_mips: Vec<TextureView>,
    shading_change: Tex,
    /// The accumulated full-size frames (colour and lock), by parity.
    upscaled: [Tex; 2],
    new_locks: Buffer,
}

/// What FSR keeps from frame to frame.
#[derive(Resource, Default)]
pub(crate) struct FsrHistory {
    motion: Option<(UVec2, Tex)>,
    output_view: Option<(TextureId, TextureView)>,
    textures: Option<FsrTextures>,
    /// Last frame's unjittered `clip_from_world`, eye, jitter and sizes.
    previous: Option<Mat4>,
    previous_eye: Option<Vec3>,
    previous_jitter: Vec2,
    previous_render: UVec2,
    previous_display: UVec2,
    /// Frames accumulated since the last reset.
    frame_index: u32,
    parity: usize,
}

pub(super) fn register(app: &mut App) {
    app.add_plugins(ExtractResourcePlugin::<FsrFrame>::default());
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    let common = include_str!("fsr3_common.wgsl");
    let shaders = {
        let mut assets = app.world_mut().resource_mut::<Assets<Shader>>();
        FsrShaders {
            motion: assets.add(Shader::from_wgsl(
                include_str!("motion.wgsl"),
                "fsr/motion.wgsl",
            )),
            blit: assets.add(Shader::from_wgsl(
                include_str!("blit.wgsl"),
                "fsr/blit.wgsl",
            )),
            passes: PASSES
                .iter()
                .map(|(_, name, source)| {
                    assets.add(Shader::from_wgsl(
                        format!("{common}\n{source}"),
                        format!("fsr/{name}.wgsl"),
                    ))
                })
                .collect(),
        }
    };
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(shaders)
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

fn params_entry() -> BindGroupLayoutEntryBuilder {
    uniform_buffer_sized(false, NonZeroU64::new(FSR_PARAMS))
}

fn load_only() -> BindGroupLayoutEntryBuilder {
    texture_2d(TextureSampleType::Float { filterable: false })
}

fn filtered() -> BindGroupLayoutEntryBuilder {
    texture_2d(TextureSampleType::Float { filterable: true })
}

fn write(format: TextureFormat) -> BindGroupLayoutEntryBuilder {
    texture_storage_2d(format, StorageTextureAccess::WriteOnly)
}

fn linear() -> BindGroupLayoutEntryBuilder {
    sampler(SamplerBindingType::Filtering)
}

fn pass_layout(pass: Pass) -> BindGroupLayoutDescriptor {
    use TextureFormat::{R32Float, Rg32Float, Rgba8Unorm, Rgba16Float};
    let compute = ShaderStages::COMPUTE;
    match pass {
        Pass::PrepareInputs => BindGroupLayoutDescriptor::new(
            "fsr3_prepare_inputs",
            &BindGroupLayoutEntries::sequential(
                compute,
                (
                    params_entry(),
                    texture_depth_2d(),
                    load_only(),
                    load_only(),
                    write(Rg32Float),
                    write(R32Float),
                    storage_buffer_sized(false, None),
                    write(R32Float),
                    write(Rgba16Float),
                ),
            ),
        ),
        Pass::FarthestDepth => BindGroupLayoutDescriptor::new(
            "fsr3_farthest_depth",
            &BindGroupLayoutEntries::sequential(
                compute,
                (params_entry(), load_only(), write(Rgba16Float)),
            ),
        ),
        Pass::ShadingPyramid => BindGroupLayoutDescriptor::new(
            "fsr3_shading_change_pyramid",
            &BindGroupLayoutEntries::sequential(
                compute,
                (
                    params_entry(),
                    load_only(),
                    load_only(),
                    load_only(),
                    write(Rgba16Float),
                ),
            ),
        ),
        Pass::Downsample => BindGroupLayoutDescriptor::new(
            "fsr3_downsample",
            &BindGroupLayoutEntries::sequential(
                compute,
                (params_entry(), load_only(), write(Rgba16Float)),
            ),
        ),
        Pass::ShadingChange => BindGroupLayoutDescriptor::new(
            "fsr3_shading_change",
            &BindGroupLayoutEntries::sequential(
                compute,
                (params_entry(), filtered(), linear(), write(Rgba8Unorm)),
            ),
        ),
        Pass::PrepareReactivity => BindGroupLayoutDescriptor::new(
            "fsr3_prepare_reactivity",
            &BindGroupLayoutEntries::sequential(
                compute,
                (
                    params_entry(),
                    storage_buffer_read_only_sized(false, None),
                    load_only(),
                    load_only(),
                    filtered(),
                    filtered(),
                    load_only(),
                    linear(),
                    write(Rgba8Unorm),
                    storage_buffer_sized(false, None),
                    write(Rgba8Unorm),
                ),
            ),
        ),
        Pass::LumaInstability => BindGroupLayoutDescriptor::new(
            "fsr3_luma_instability",
            &BindGroupLayoutEntries::sequential(
                compute,
                (
                    params_entry(),
                    filtered(),
                    load_only(),
                    filtered(),
                    filtered(),
                    linear(),
                    write(Rgba16Float),
                    write(Rgba16Float),
                ),
            ),
        ),
        Pass::Accumulate => BindGroupLayoutDescriptor::new(
            "fsr3_accumulate",
            &BindGroupLayoutEntries::sequential(
                compute,
                (
                    params_entry(),
                    filtered(),
                    load_only(),
                    load_only(),
                    filtered(),
                    filtered(),
                    load_only(),
                    linear(),
                    write(Rgba16Float),
                    write(OUTPUT_FORMAT),
                    storage_buffer_sized(false, None),
                ),
            ),
        ),
        Pass::Rcas => BindGroupLayoutDescriptor::new(
            "fsr3_rcas",
            &BindGroupLayoutEntries::sequential(
                compute,
                (params_entry(), load_only(), write(OUTPUT_FORMAT)),
            ),
        ),
    }
}

fn init_pipelines(
    mut commands: Commands,
    shaders: Res<FsrShaders>,
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
                write(MOTION_FORMAT),
            ),
        ),
    );
    let blit_layout = BindGroupLayoutDescriptor::new(
        "fsr_blit_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer_sized(false, NonZeroU64::new(BLIT_PARAMS)),
                filtered(),
                linear(),
                load_only(),
                write(OUTPUT_FORMAT),
            ),
        ),
    );
    let compute = |label: String, layout: &BindGroupLayoutDescriptor, shader: &Handle<Shader>| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            immediate_size: 0,
            shader: shader.clone(),
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
    let pass_layouts: Vec<_> = PASSES.iter().map(|(pass, ..)| pass_layout(*pass)).collect();
    let passes = PASSES
        .iter()
        .zip(&pass_layouts)
        .zip(&shaders.passes)
        .map(|(((_, name, _), layout), shader)| compute((*name).to_owned(), layout, shader))
        .collect();
    commands.insert_resource(FsrPipelines {
        motion: compute("fsr_motion".to_owned(), &motion_layout, &shaders.motion),
        motion_params: uniform("fsr_motion_params", MOTION_PARAMS),
        motion_layout,
        blit: compute("fsr_blit".to_owned(), &blit_layout, &shaders.blit),
        blit_params: uniform("fsr_blit_params", BLIT_PARAMS),
        blit_layout,
        pass_layouts,
        passes,
        params: uniform("fsr3_params", FSR_PARAMS),
        linear: device.create_sampler(&SamplerDescriptor {
            label: Some("fsr_linear"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
    });
}

fn texture(device: &RenderDevice, label: &'static str, size: UVec2, format: TextureFormat) -> Tex {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(label),
        size: Extent3d {
            width: size.x.max(1),
            height: size.y.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    Tex {
        view: texture.create_view(&TextureViewDescriptor::default()),
    }
}

fn storage(device: &RenderDevice, label: &'static str, size: UVec2) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: u64::from(size.x.max(1)) * u64::from(size.y.max(1)) * 4,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl FsrTextures {
    /// New textures are zeroed, which is what FSR's first frame expects.
    fn new(device: &RenderDevice, render: UVec2, display: UVec2) -> Self {
        use TextureFormat::{R32Float, Rg32Float, Rgba8Unorm, Rgba16Float};
        let half = (render / 2).max(UVec2::ONE);
        let mips = SHADING_MIPS.min(32 - half.x.max(half.y).leading_zeros());
        let shading_pyramid = device.create_texture(&TextureDescriptor {
            label: Some("fsr3_shading_change_pyramid"),
            size: Extent3d {
                width: half.x,
                height: half.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: Rgba16Float,
            usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shading_pyramid_mips = (0..mips)
            .map(|mip| {
                shading_pyramid.create_view(&TextureViewDescriptor {
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    ..default()
                })
            })
            .collect();
        Self {
            render,
            display,
            dilated_motion: texture(device, "fsr3_dilated_motion", render, Rg32Float),
            dilated_depth: texture(device, "fsr3_dilated_depth", render, R32Float),
            farthest_depth: texture(device, "fsr3_farthest_depth", render, R32Float),
            luma: [
                texture(device, "fsr3_luma_1", render, Rgba16Float),
                texture(device, "fsr3_luma_2", render, Rgba16Float),
            ],
            luma_history: [
                texture(device, "fsr3_luma_history_1", render, Rgba16Float),
                texture(device, "fsr3_luma_history_2", render, Rgba16Float),
            ],
            luma_instability: texture(device, "fsr3_luma_instability", render, Rgba16Float),
            reactive_masks: texture(device, "fsr3_dilated_reactive_masks", render, Rgba8Unorm),
            accumulation: [
                texture(device, "fsr3_accumulation_1", render, Rgba8Unorm),
                texture(device, "fsr3_accumulation_2", render, Rgba8Unorm),
            ],
            reconstructed_depth: storage(device, "fsr3_reconstructed_previous_depth", render),
            farthest_depth_mip1: texture(device, "fsr3_farthest_depth_mip1", half, Rgba16Float),
            shading_pyramid_all: shading_pyramid.create_view(&TextureViewDescriptor::default()),
            shading_pyramid,
            shading_pyramid_mips,
            shading_change: texture(device, "fsr3_shading_change", half, Rgba8Unorm),
            upscaled: [
                texture(device, "fsr3_upscaled_1", display, Rgba16Float),
                texture(device, "fsr3_upscaled_2", display, Rgba16Float),
            ],
            new_locks: storage(device, "fsr3_new_locks", display),
        }
    }
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
    let target = image.texture.clone();
    let view = match history.output_view.take() {
        Some((id, view)) if id == target.id() => view,
        _ => target.create_view(&TextureViewDescriptor {
            label: Some("fsr_output"),
            format: Some(OUTPUT_FORMAT),
            ..default()
        }),
    };
    history.output_view = Some((target.id(), view.clone()));
    let display = UVec2::new(target.width(), target.height());
    output.0 = Some(FsrOutputTarget {
        texture: target,
        view,
    });
    let render = frame.render_size;
    if history
        .motion
        .as_ref()
        .is_none_or(|(current, _)| *current != render)
    {
        let motion = texture(&device, "fsr_motion_vectors", render, MOTION_FORMAT);
        history.motion = Some((render, motion));
    }
    if history
        .textures
        .as_ref()
        .is_none_or(|textures| textures.render != render || textures.display != display)
    {
        history.textures = Some(FsrTextures::new(&device, render, display));
        history.frame_index = 0;
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

fn push_f32s(bytes: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
}

fn push_i32s(bytes: &mut Vec<u8>, values: &[i32]) {
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
}

fn dispatch(pass: &mut ComputePass, bind: &BindGroup, size: UVec2) {
    pass.set_bind_group(0, bind, &[]);
    pass.dispatch_workgroups(size.x.div_ceil(GROUP), size.y.div_ceil(GROUP), 1);
}

/// This frame's camera, as FSR needs it.
struct FsrCamera {
    jittered: Mat4,
    unjittered: Mat4,
    eye: Vec3,
    near: f32,
    /// cot(vertical fov / 2).
    cot_half_fov_y: f32,
}

#[allow(clippy::too_many_arguments)]
fn draw_fsr(
    view: ViewQuery<(&ViewTarget, &SceneDepthTexture, Option<&TemporalJitter>)>,
    frame: Res<FsrFrame>,
    output: Res<FsrOutput>,
    pipelines: Option<Res<FsrPipelines>>,
    published: Option<Res<crate::drawsurf::PublishedRenderFrame>>,
    mut history: ResMut<FsrHistory>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut context: RenderContext,
) {
    let (target, depth, temporal) = view.into_inner();
    let (Some(out), Some(pipelines)) = (output.0.as_ref(), pipelines) else {
        return;
    };
    let Some(motion_view) = history
        .motion
        .as_ref()
        .map(|(_, motion)| motion.view.clone())
    else {
        return;
    };
    let colour = target.main_texture().size();
    let render = UVec2::new(colour.width, colour.height);
    let out_size = out.size();
    let display = UVec2::new(out_size.width, out_size.height);
    let blit = Blit {
        pipelines: &pipelines,
        cache: &cache,
        device: &device,
        queue: &queue,
        target,
        motion: &motion_view,
        out,
        render,
    };
    let camera = published.as_deref().and_then(|frame| {
        let exec = &frame.exec_frame;
        let clip_from_view = exec.clip_from_view?;
        Some(FsrCamera {
            jittered: exec.clip_from_world?,
            unjittered: exec.unjittered_clip_from_world?,
            eye: exec.view_origin,
            near: clip_from_view.w_axis.z,
            cot_half_fov_y: clip_from_view.y_axis.y,
        })
    });
    let sized = depth.texture.size() == colour
        && depth.texture.sample_count() == 1
        && history
            .motion
            .as_ref()
            .is_some_and(|(size, _)| *size == render)
        && history
            .textures
            .as_ref()
            .is_some_and(|textures| textures.render == render && textures.display == display);
    let (Some(camera), true, Some(motion_pipeline)) =
        (camera, sized, cache.get_compute_pipeline(pipelines.motion))
    else {
        // The camera is still switching to its internal image, or nothing is ready yet.
        history.previous = None;
        blit.draw(&mut context, false);
        return;
    };

    // A cut (or the first frame) starts the history over.
    if history
        .previous_eye
        .is_none_or(|eye| eye.distance(camera.eye) > CUT_DISTANCE)
    {
        history.previous = None;
        history.frame_index = 0;
    }
    let jitter = temporal.map_or(Vec2::ZERO, |temporal| -temporal.offset);

    // Motion vectors, from this frame's and last frame's camera (relative to the eye).
    let to_eye = Mat4::from_translation(camera.eye);
    let previous = history.previous.unwrap_or(camera.unjittered);
    let mut bytes = Vec::with_capacity(MOTION_PARAMS as usize);
    push_mat4(&mut bytes, (camera.jittered * to_eye).inverse());
    push_mat4(&mut bytes, camera.unjittered * to_eye);
    push_mat4(&mut bytes, previous * to_eye);
    let size = render.as_vec2();
    push_f32s(&mut bytes, &[size.x, size.y, 1.0 / size.x, 1.0 / size.y]);
    queue.write_buffer(&pipelines.motion_params, 0, &bytes);
    let motion_bind = device.create_bind_group(
        "fsr_motion",
        &cache.get_bind_group_layout(&pipelines.motion_layout),
        &BindGroupEntries::sequential((
            pipelines.motion_params.as_entire_buffer_binding(),
            depth.view(),
            &motion_view,
        )),
    );
    {
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("fsr_motion"),
                timestamp_writes: None,
            });
        pass.set_pipeline(motion_pipeline);
        dispatch(&mut pass, &motion_bind, render);
    }

    let fsr_pipelines: Option<Vec<_>> = pipelines
        .passes
        .iter()
        .map(|id| cache.get_compute_pipeline(*id))
        .collect();
    let (Some(fsr_pipelines), false) = (fsr_pipelines, fsr_debug_motion()) else {
        blit.draw(&mut context, fsr_debug_motion());
        remember(&mut history, &camera, jitter, render, display, false);
        return;
    };

    let first = history.frame_index == 0;
    let previous_render = if first {
        render
    } else {
        history.previous_render
    };
    let previous_display = if first {
        display
    } else {
        history.previous_display
    };
    let previous_jitter = if first {
        jitter
    } else {
        history.previous_jitter
    };
    let ratio = display.x as f32 / render.x as f32;
    let aspect = render.x as f32 / render.y as f32;
    let a = camera.cot_half_fov_y / aspect;
    let b = camera.cot_half_fov_y;
    let sharpen = frame.sharpness > 0.0;
    let mut bytes = Vec::with_capacity(FSR_PARAMS as usize);
    push_i32s(
        &mut bytes,
        &[
            render.x as i32,
            render.y as i32,
            previous_render.x as i32,
            previous_render.y as i32,
            display.x as i32,
            display.y as i32,
            previous_display.x as i32,
            previous_display.y as i32,
        ],
    );
    push_f32s(
        &mut bytes,
        &[
            // Device depth to view depth, for inverted infinite depth.
            -f32::EPSILON,
            camera.near,
            1.0 / a,
            1.0 / b,
            jitter.x,
            jitter.y,
            previous_jitter.x,
            previous_jitter.y,
            render.x as f32 / display.x as f32,
            render.y as f32 / display.y as f32,
            aspect / camera.cot_half_fov_y,
            ((8.0 * ratio * ratio) as i32).max(1) as f32,
            METERS_PER_UNIT,
            history.frame_index as f32,
            // Velocity factor, reactiveness and shading change scales (AMD's defaults).
            1.0,
            1.0,
            1.0,
            // Accumulation added per frame, minimum disocclusion accumulation.
            1.0 / 3.0,
            -1.0 / 3.0,
            // RCAS takes its strength in stops: sharpness 1 is 0 stops, 0 is 2.
            (-(2.0 - 2.0 * frame.sharpness.clamp(0.0, 1.0))).exp2(),
            if sharpen { 1.0 } else { 0.0 },
            0.0,
        ],
    );
    bytes.resize(FSR_PARAMS as usize, 0);
    queue.write_buffer(&pipelines.params, 0, &bytes);

    let textures = history.textures.as_ref().expect("checked above");
    let now = history.parity;
    let then = 1 - now;
    let layout = |pass: Pass| cache.get_bind_group_layout(&pipelines.pass_layouts[pass as usize]);
    let params = pipelines.params.as_entire_buffer_binding();
    let linear = &pipelines.linear;
    let half = (render / 2).max(UVec2::ONE);
    let binds = [
        device.create_bind_group(
            "fsr3_prepare_inputs",
            &layout(Pass::PrepareInputs),
            &BindGroupEntries::sequential((
                params.clone(),
                depth.view(),
                target.main_texture_view(),
                &motion_view,
                &textures.dilated_motion.view,
                &textures.dilated_depth.view,
                textures.reconstructed_depth.as_entire_buffer_binding(),
                &textures.farthest_depth.view,
                &textures.luma[now].view,
            )),
        ),
        device.create_bind_group(
            "fsr3_farthest_depth",
            &layout(Pass::FarthestDepth),
            &BindGroupEntries::sequential((
                params.clone(),
                &textures.farthest_depth.view,
                &textures.farthest_depth_mip1.view,
            )),
        ),
        device.create_bind_group(
            "fsr3_shading_change_pyramid",
            &layout(Pass::ShadingPyramid),
            &BindGroupEntries::sequential((
                params.clone(),
                &textures.luma[now].view,
                &textures.luma[then].view,
                &textures.dilated_motion.view,
                &textures.shading_pyramid_mips[0],
            )),
        ),
        device.create_bind_group(
            "fsr3_shading_change",
            &layout(Pass::ShadingChange),
            &BindGroupEntries::sequential((
                params.clone(),
                &textures.shading_pyramid_all,
                linear,
                &textures.shading_change.view,
            )),
        ),
        device.create_bind_group(
            "fsr3_prepare_reactivity",
            &layout(Pass::PrepareReactivity),
            &BindGroupEntries::sequential((
                params.clone(),
                textures.reconstructed_depth.as_entire_buffer_binding(),
                &textures.dilated_motion.view,
                &textures.dilated_depth.view,
                &textures.accumulation[then].view,
                &textures.shading_change.view,
                &textures.luma[now].view,
                linear,
                &textures.reactive_masks.view,
                textures.new_locks.as_entire_buffer_binding(),
                &textures.accumulation[now].view,
            )),
        ),
        device.create_bind_group(
            "fsr3_luma_instability",
            &layout(Pass::LumaInstability),
            &BindGroupEntries::sequential((
                params.clone(),
                &textures.reactive_masks.view,
                &textures.dilated_motion.view,
                &textures.luma_history[then].view,
                &textures.luma[now].view,
                linear,
                &textures.luma_history[now].view,
                &textures.luma_instability.view,
            )),
        ),
        device.create_bind_group(
            "fsr3_accumulate",
            &layout(Pass::Accumulate),
            &BindGroupEntries::sequential((
                params.clone(),
                &textures.reactive_masks.view,
                &textures.dilated_motion.view,
                &textures.upscaled[then].view,
                &textures.farthest_depth_mip1.view,
                &textures.luma_instability.view,
                target.main_texture_view(),
                linear,
                &textures.upscaled[now].view,
                &out.view,
                textures.new_locks.as_entire_buffer_binding(),
            )),
        ),
        device.create_bind_group(
            "fsr3_rcas",
            &layout(Pass::Rcas),
            &BindGroupEntries::sequential((
                params.clone(),
                &textures.upscaled[now].view,
                &out.view,
            )),
        ),
    ];
    let downsample: Vec<BindGroup> = textures
        .shading_pyramid_mips
        .windows(2)
        .map(|mips| {
            device.create_bind_group(
                "fsr3_downsample",
                &layout(Pass::Downsample),
                &BindGroupEntries::sequential((params.clone(), &mips[0], &mips[1])),
            )
        })
        .collect();
    let [
        prepare_inputs,
        farthest_depth,
        shading_pyramid,
        shading_change,
        prepare_reactivity,
        luma_instability,
        accumulate,
        rcas,
    ] = &binds;
    let pipeline = |pass: Pass| fsr_pipelines[pass as usize];
    let pyramid = textures.shading_pyramid.size();

    let encoder = context.command_encoder();
    encoder.clear_buffer(&textures.reconstructed_depth, 0, None);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("fsr3_upscale"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline(Pass::PrepareInputs));
        dispatch(&mut pass, prepare_inputs, render);
        pass.set_pipeline(pipeline(Pass::FarthestDepth));
        dispatch(&mut pass, farthest_depth, half);
        pass.set_pipeline(pipeline(Pass::ShadingPyramid));
        dispatch(&mut pass, shading_pyramid, half);
        pass.set_pipeline(pipeline(Pass::Downsample));
        for (mip, bind) in downsample.iter().enumerate() {
            let size = UVec2::new(pyramid.width, pyramid.height) >> (mip as u32 + 1);
            dispatch(&mut pass, bind, size.max(UVec2::ONE));
        }
        pass.set_pipeline(pipeline(Pass::ShadingChange));
        dispatch(
            &mut pass,
            shading_change,
            (render.as_vec2() * 0.5).as_uvec2(),
        );
        pass.set_pipeline(pipeline(Pass::PrepareReactivity));
        dispatch(&mut pass, prepare_reactivity, render);
        pass.set_pipeline(pipeline(Pass::LumaInstability));
        dispatch(&mut pass, luma_instability, render);
        pass.set_pipeline(pipeline(Pass::Accumulate));
        dispatch(&mut pass, accumulate, display);
        if sharpen {
            pass.set_pipeline(pipeline(Pass::Rcas));
            dispatch(&mut pass, rcas, display);
        }
    }
    remember(&mut history, &camera, jitter, render, display, true);
}

/// Carries this frame over as last frame.
fn remember(
    history: &mut FsrHistory,
    camera: &FsrCamera,
    jitter: Vec2,
    render: UVec2,
    display: UVec2,
    accumulated: bool,
) {
    history.previous = Some(camera.unjittered);
    history.previous_eye = Some(camera.eye);
    history.previous_jitter = jitter;
    history.previous_render = render;
    history.previous_display = display;
    if accumulated {
        history.frame_index = history.frame_index.saturating_add(1);
        history.parity = 1 - history.parity;
    } else {
        history.frame_index = 0;
    }
}

/// A bilinear stretch into the full-size frame: until FSR's pipelines are ready, and with
/// `IW4L_FSR_DEBUG=motion` (then tinted with the motion vectors).
struct Blit<'a> {
    pipelines: &'a FsrPipelines,
    cache: &'a PipelineCache,
    device: &'a RenderDevice,
    queue: &'a RenderQueue,
    target: &'a ViewTarget,
    motion: &'a TextureView,
    out: &'a FsrOutputTarget,
    render: UVec2,
}

impl Blit<'_> {
    fn draw(&self, context: &mut RenderContext, tint: bool) {
        let Some(pipeline) = self.cache.get_compute_pipeline(self.pipelines.blit) else {
            return;
        };
        let size = self.out.size();
        let (w, h) = (size.width as f32, size.height as f32);
        let mut bytes = Vec::with_capacity(BLIT_PARAMS as usize);
        push_f32s(&mut bytes, &[w, h, 1.0 / w, 1.0 / h]);
        let render = self.render.as_vec2();
        push_f32s(
            &mut bytes,
            &[render.x, render.y, if tint { 1.0 } else { 0.0 }, 0.0],
        );
        self.queue
            .write_buffer(&self.pipelines.blit_params, 0, &bytes);
        let bind = self.device.create_bind_group(
            "fsr_blit",
            &self
                .cache
                .get_bind_group_layout(&self.pipelines.blit_layout),
            &BindGroupEntries::sequential((
                self.pipelines.blit_params.as_entire_buffer_binding(),
                self.target.main_texture_view(),
                &self.pipelines.linear,
                self.motion,
                &self.out.view,
            )),
        );
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("fsr_blit"),
                timestamp_writes: None,
            });
        pass.set_pipeline(pipeline);
        dispatch(&mut pass, &bind, UVec2::new(size.width, size.height));
    }
}
