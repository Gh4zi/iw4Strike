//! CS2's volumetric smoke: each smoke is a voxel volume the client filled through the map from
//! where the grenade popped (render_frontend's `cs_smoke`). One fullscreen pass marches every
//! pixel's view ray through the volumes up to the scene depth, with drifting noise on the
//! edges and the holes bullets and HE blasts leave. It runs after the world and its sun effects
//! and before post-processing, so the scope, the CS viewmodel and the HUD stay on top.

use std::num::NonZeroU64;
use std::sync::Arc;

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_3d, texture_depth_2d, texture_depth_2d_multisampled,
    uniform_buffer_sized,
};
use bevy::render::render_resource::{
    AddressMode, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendState,
    Buffer, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, Extent3d, FilterMode,
    FragmentState, LoadOp, MultisampleState, Operations, Origin3d, PipelineCache, PrimitiveState,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler,
    SamplerBindingType, SamplerDescriptor, ShaderStages, SpecializedRenderPipeline,
    SpecializedRenderPipelines, StoreOp, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType,
    TextureUsages, TextureView, TextureViewDescriptor, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Extract, ExtractSchedule, RenderApp, RenderStartup};
use bevy::render::render_resource::MipmapFilterMode;
use bevy::shader::Shader;

use super::scene_depth::SceneDepthTexture;

const SHADER_PATH: &str = "embedded://render_gpu/drawsurf/cs_smoke.wgsl";
const COMPOSITE_SHADER_PATH: &str = "embedded://render_gpu/drawsurf/cs_smoke_composite.wgsl";

/// Voxels per smoke volume along x, y and z.
pub const CS_SMOKE_DIM: [u32; 3] = [32, 32, 24];
/// How many smokes can be drawn at once; each has its own slice of the voxel atlas.
pub const CS_SMOKE_SLOTS: usize = 12;
/// How many bullet or blast holes the pass carves at once.
pub const CS_SMOKE_HOLES: usize = 24;
/// Side of the tiling noise texture the edges drift through, and its coarsest Worley cells
/// across one tile.
const NOISE_DIM: u32 = 48;
const NOISE_CELLS: i64 = 3;

/// One smoke: its voxels and where they sit, how far it has grown and how much is left.
#[derive(Clone, Debug)]
pub struct CsSmokeVolume {
    /// Stable for the smoke's life: the voxels upload again only when another id takes the slot.
    pub id: u64,
    /// Atlas slice, below [`CS_SMOKE_SLOTS`].
    pub slot: u32,
    /// [`CS_SMOKE_DIM`] voxels, x fastest, four bytes each: density, the order the smoke reached
    /// the voxel in (0 first, 1 last), the sunlight that reaches it and the light from all
    /// around; the last three multiplied by density, so they filter right at the edges.
    pub texels: Arc<Vec<u8>>,
    /// World corner of voxel (0, 0, 0) and the side of one voxel.
    pub min: [f32; 3],
    pub voxel: f32,
    /// Colour of the light from all around (luminance 1; the voxels say how bright), and the
    /// sun's linear light on the smoke.
    pub ambient: [f32; 3],
    pub sun: [f32; 3],
    /// How far the smoke has spread, in the order of its voxels (0 just popped, above 1 full).
    pub grow: f32,
    /// How much of the smoke is left (1 until it starts to clear).
    pub alpha: f32,
}

/// A tunnel a bullet left through the smoke (`a` to `b`), or a blast (`a` == `b`).
#[derive(Clone, Copy, Debug)]
pub struct CsSmokeHole {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub radius: f32,
    /// 1 fully open, 0 closed again.
    pub strength: f32,
}

/// What the main world wants drawn this frame.
#[derive(Resource, Clone, Debug, Default)]
pub struct CsSmokeFrame {
    pub volumes: Vec<CsSmokeVolume>,
    pub holes: Vec<CsSmokeHole>,
    /// Seconds the noise has drifted.
    pub seconds: f32,
    /// Unit direction toward the sun (zero for none).
    pub sun_dir: [f32; 3],
    /// How finely the smoke is drawn (`smoke_quality`).
    pub quality: frame::settings::SmokeQuality,
}

/// Frame pixels across one march pixel, and the march step in units, for a `smoke_quality`:
/// high marches every pixel in 4-unit steps (sharp billows and edges), medium half the
/// resolution (a quarter of the pixels), low half the resolution in CS2's own 6-unit steps.
fn march_for(quality: frame::settings::SmokeQuality) -> (u32, f32) {
    use frame::settings::SmokeQuality;
    match quality {
        SmokeQuality::High => (1, 4.0),
        SmokeQuality::Medium => (2, 4.0),
        SmokeQuality::Low => (2, 6.0),
    }
}

#[derive(Resource, Default)]
struct ExtractedCsSmoke(CsSmokeFrame);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct VolumeGpu {
    /// Camera-relative corner, voxel side.
    lo: [f32; 4],
    /// Camera-relative far corner, atlas slot.
    hi: [f32; 4],
    /// Ambient light, grow.
    ambient: [f32; 4],
    /// Sun light, alpha.
    sun: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct HoleGpu {
    /// Camera-relative start, radius.
    a: [f32; 4],
    /// Camera-relative end, strength.
    b: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ParamsGpu {
    /// Camera-relative world position from clip space.
    world_from_clip: [f32; 16],
    /// Width, height, top of the world's depth range, seconds.
    screen: [f32; 4],
    /// Volumes, holes, frame pixels across one march pixel, march step.
    counts: [f32; 4],
    sun_dir: [f32; 4],
    /// View origin, for noise fixed to the world.
    origin: [f32; 4],
    volumes: [VolumeGpu; CS_SMOKE_SLOTS],
    holes: [HoleGpu; CS_SMOKE_HOLES],
}

const PARAMS_SIZE: u64 = std::mem::size_of::<ParamsGpu>() as u64;

/// The smoke is marched (at the resolution `smoke_quality` asks), then laid over the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SmokePass {
    March,
    Composite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CsSmokeKey {
    pass: SmokePass,
    target: TextureFormat,
    multisampled: bool,
}

/// The march's half-resolution smoke (premultiplied) and the scene distance each of its pixels
/// stopped at.
const HALF_COLOUR_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const HALF_DISTANCE_FORMAT: TextureFormat = TextureFormat::R32Float;

struct HalfTargets {
    size: (u32, u32),
    _colour: Texture,
    colour: TextureView,
    _distance: Texture,
    distance: TextureView,
}

#[derive(Resource)]
struct CsSmokeGpu {
    shader: Handle<Shader>,
    composite_shader: Handle<Shader>,
    march_single: BindGroupLayoutDescriptor,
    march_msaa: BindGroupLayoutDescriptor,
    composite_single: BindGroupLayoutDescriptor,
    composite_msaa: BindGroupLayoutDescriptor,
    params: Buffer,
    atlas: Texture,
    atlas_view: TextureView,
    _noise: Texture,
    noise_view: TextureView,
    clamp: Sampler,
    repeat: Sampler,
    half: Option<HalfTargets>,
    /// Which smoke each atlas slice holds.
    uploaded: [Option<u64>; CS_SMOKE_SLOTS],
}

impl SpecializedRenderPipeline for CsSmokeGpu {
    type Key = CsSmokeKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let defs: Vec<_> = if key.multisampled {
            vec!["MULTISAMPLED".into()]
        } else {
            Vec::new()
        };
        let target = |format, blend| {
            Some(ColorTargetState {
                format,
                blend,
                write_mask: ColorWrites::ALL,
            })
        };
        let (label, shader, layout, vertex, fragment, targets) = match key.pass {
            SmokePass::March => (
                "cs_smoke_march",
                &self.shader,
                if key.multisampled {
                    &self.march_msaa
                } else {
                    &self.march_single
                },
                "vs_smoke",
                "fs_smoke",
                vec![
                    target(HALF_COLOUR_FORMAT, None),
                    target(HALF_DISTANCE_FORMAT, None),
                ],
            ),
            SmokePass::Composite => (
                "cs_smoke_composite",
                &self.composite_shader,
                if key.multisampled {
                    &self.composite_msaa
                } else {
                    &self.composite_single
                },
                "vs_composite",
                "fs_composite",
                vec![target(
                    key.target,
                    Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                )],
            ),
        };
        RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            immediate_size: 0,
            vertex: VertexState {
                shader: shader.clone(),
                shader_defs: defs.clone(),
                entry_point: Some(vertex.into()),
                buffers: Vec::new(),
            },
            fragment: Some(FragmentState {
                shader: shader.clone(),
                shader_defs: defs,
                entry_point: Some(fragment.into()),
                targets,
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            zero_initialize_workgroup_memory: false,
        }
    }
}

fn scene_depth(multisampled: bool) -> bevy::render::render_resource::BindGroupLayoutEntryBuilder {
    if multisampled {
        texture_depth_2d_multisampled()
    } else {
        texture_depth_2d()
    }
}

fn march_layout(label: &'static str, multisampled: bool) -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        label,
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer_sized(false, NonZeroU64::new(PARAMS_SIZE)),
                scene_depth(multisampled),
                texture_3d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    )
}

fn composite_layout(label: &'static str, multisampled: bool) -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        label,
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer_sized(false, NonZeroU64::new(PARAMS_SIZE)),
                scene_depth(multisampled),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
        ),
    )
}

fn half_targets(device: &RenderDevice, size: (u32, u32)) -> HalfTargets {
    let make = |label, format| {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some(label),
            size: Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        (texture, view)
    };
    let (colour_texture, colour) = make("cs_smoke_half_colour", HALF_COLOUR_FORMAT);
    let (distance_texture, distance) = make("cs_smoke_half_distance", HALF_DISTANCE_FORMAT);
    HalfTargets {
        size,
        _colour: colour_texture,
        colour,
        _distance: distance_texture,
        distance,
    }
}

fn texture_3d_rgba(
    device: &RenderDevice,
    label: &'static str,
    size: Extent3d,
    mip_level_count: u32,
) -> Texture {
    device.create_texture(&TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count,
        sample_count: 1,
        dimension: TextureDimension::D3,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// A lattice point's hash, the lattice wrapping every `period` points.
fn lattice_hash(p: [i64; 3], period: i64, seed: u32) -> u32 {
    let w = p.map(|v| v.rem_euclid(period) as u32);
    let mut h = seed.wrapping_mul(0x9E37_79B9)
        ^ w[0].wrapping_mul(0x8DA6_B343)
        ^ w[1].wrapping_mul(0xD816_3841)
        ^ w[2].wrapping_mul(0xCB1A_B31F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Tiling gradient (Perlin) noise at `p` in lattice units, in about [0, 1].
fn perlin(p: [f32; 3], period: i64, seed: u32) -> f32 {
    const GRADIENTS: [[f32; 3]; 12] = [
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
        [1.0, -1.0, 0.0],
        [-1.0, -1.0, 0.0],
        [1.0, 0.0, 1.0],
        [-1.0, 0.0, 1.0],
        [1.0, 0.0, -1.0],
        [-1.0, 0.0, -1.0],
        [0.0, 1.0, 1.0],
        [0.0, -1.0, 1.0],
        [0.0, 1.0, -1.0],
        [0.0, -1.0, -1.0],
    ];
    let base = p.map(f32::floor);
    let f: [f32; 3] = core::array::from_fn(|k| p[k] - base[k]);
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let u = f.map(fade);
    let mut value = 0.0;
    for corner in 0..8 {
        let o = [corner & 1, (corner >> 1) & 1, corner >> 2];
        let lattice = core::array::from_fn(|k| base[k] as i64 + o[k] as i64);
        let g = GRADIENTS[(lattice_hash(lattice, period, seed) % 12) as usize];
        let dot: f32 = (0..3).map(|k| g[k] * (f[k] - o[k] as f32)).sum();
        let weight: f32 = (0..3)
            .map(|k| if o[k] == 1 { u[k] } else { 1.0 - u[k] })
            .product();
        value += dot * weight;
    }
    0.5 + 0.5 * value
}

/// Tiling Worley noise, inverted: 1 on a cell's feature point, 0 a cell's width away.
fn worley(p: [f32; 3], period: i64, seed: u32) -> f32 {
    let base = p.map(|v| v.floor() as i64);
    let mut nearest = f32::MAX;
    for neighbour in 0..27_i64 {
        let cell: [i64; 3] = [
            base[0] + neighbour % 3 - 1,
            base[1] + (neighbour / 3) % 3 - 1,
            base[2] + neighbour / 9 - 1,
        ];
        let d2: f32 = (0..3)
            .map(|k| {
                let feature = cell[k] as f32 + unit(lattice_hash(cell, period, seed + k as u32));
                (feature - p[k]).powi(2)
            })
            .sum();
        nearest = nearest.min(d2);
    }
    1.0 - nearest.sqrt().min(1.0)
}

/// Four channels of tiling noise the smoke is shaped and lit with, the same every run:
/// Perlin-Worley (Perlin noise pushed into Worley's round cells, the cauliflower billows of a
/// cumulus or CS2's smoke), Worley for the finer billows, and two smooth Perlin channels that
/// bend the smoke's shape.
fn noise_texels() -> Vec<u8> {
    let n = NOISE_DIM as usize;
    let octaves = |p: [f32; 3], noise: fn([f32; 3], i64, u32) -> f32, seed: u32, cells: i64| {
        let weights = [0.625, 0.25, 0.125];
        (0..3)
            .map(|o| {
                let scale = (cells << o) as f32;
                weights[o] * noise(p.map(|v| v * scale), cells << o, seed + o as u32 * 7)
            })
            .sum::<f32>()
    };
    let mut channels: [Vec<f32>; 4] = core::array::from_fn(|_| Vec::with_capacity(n * n * n));
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let p = [x, y, z].map(|v| (v as f32 + 0.5) / n as f32);
                let cloud = octaves(p, perlin, 1, NOISE_CELLS);
                let cells = octaves(p, worley, 20, NOISE_CELLS);
                // Perlin remapped from [cells - 1, 1]: the low Perlin values fall away
                // between the cells.
                let billow = ((cloud - (cells - 1.0)) / (2.0 - cells)).clamp(0.0, 1.0);
                channels[0].push(billow);
                channels[1].push(octaves(p, worley, 40, NOISE_CELLS));
                channels[2].push(perlin(p.map(|v| v * 3.0), 3, 60));
                channels[3].push(perlin(p.map(|v| v * 3.0), 3, 70));
            }
        }
    }
    let mut texels = vec![0u8; n * n * n * 4];
    for (channel, values) in channels.iter().enumerate() {
        let (lo, hi) = values
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        let span = (hi - lo).max(1e-6);
        for (i, v) in values.iter().enumerate() {
            texels[i * 4 + channel] = (((v - lo) / span) * 255.0).round() as u8;
        }
    }
    texels
}

/// The noise and its mip chain down to 3³, each level the 2×2×2 average of the one above, so a
/// march step reads noise no finer than itself.
fn noise_mips(top: Vec<u8>) -> Vec<(u32, Vec<u8>)> {
    let mut mips = vec![(NOISE_DIM, top)];
    while let Some((dim, texels)) = mips.last().filter(|(dim, _)| dim % 2 == 0 && *dim > 3) {
        let (n, half) = (*dim as usize, *dim as usize / 2);
        let mut next = vec![0u8; half * half * half * 4];
        for z in 0..half {
            for y in 0..half {
                for x in 0..half {
                    for channel in 0..4 {
                        let sum: u32 = (0..8)
                            .map(|corner| {
                                let (dx, dy, dz) = (corner & 1, (corner >> 1) & 1, corner >> 2);
                                let at = ((z * 2 + dz) * n + y * 2 + dy) * n + x * 2 + dx;
                                u32::from(texels[at * 4 + channel])
                            })
                            .sum();
                        next[((z * half + y) * half + x) * 4 + channel] = (sum / 8) as u8;
                    }
                }
            }
        }
        mips.push((half as u32, next));
    }
    mips
}

fn init_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let params = device.create_buffer(&BufferDescriptor {
        label: Some("cs_smoke_params"),
        size: PARAMS_SIZE,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let [w, h, d] = CS_SMOKE_DIM;
    let atlas = texture_3d_rgba(
        &device,
        "cs_smoke_atlas",
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: d * CS_SMOKE_SLOTS as u32,
        },
        1,
    );
    let mips = noise_mips(noise_texels());
    let noise = texture_3d_rgba(
        &device,
        "cs_smoke_noise",
        Extent3d {
            width: NOISE_DIM,
            height: NOISE_DIM,
            depth_or_array_layers: NOISE_DIM,
        },
        mips.len() as u32,
    );
    for (level, (dim, texels)) in mips.iter().enumerate() {
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &noise,
                mip_level: level as u32,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            texels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(dim * 4),
                rows_per_image: Some(*dim),
            },
            Extent3d {
                width: *dim,
                height: *dim,
                depth_or_array_layers: *dim,
            },
        );
    }
    let linear = |label, address| {
        device.create_sampler(&SamplerDescriptor {
            label: Some(label),
            address_mode_u: address,
            address_mode_v: address,
            address_mode_w: address,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..default()
        })
    };
    commands.insert_resource(CsSmokeGpu {
        shader: asset_server.load(SHADER_PATH),
        composite_shader: asset_server.load(COMPOSITE_SHADER_PATH),
        march_single: march_layout("cs_smoke_march", false),
        march_msaa: march_layout("cs_smoke_march_msaa", true),
        composite_single: composite_layout("cs_smoke_composite", false),
        composite_msaa: composite_layout("cs_smoke_composite_msaa", true),
        params,
        half: None,
        atlas_view: atlas.create_view(&TextureViewDescriptor::default()),
        atlas,
        noise_view: noise.create_view(&TextureViewDescriptor::default()),
        _noise: noise,
        clamp: linear("cs_smoke_clamp", AddressMode::ClampToEdge),
        repeat: linear("cs_smoke_repeat", AddressMode::Repeat),
        uploaded: [None; CS_SMOKE_SLOTS],
    });
}

fn extract_cs_smoke(mut extracted: ResMut<ExtractedCsSmoke>, frame: Extract<Res<CsSmokeFrame>>) {
    extracted.0.clone_from(&frame);
}

/// The pixels (x, y, width, height) the volumes' boxes cover on screen, a few pixels wider for
/// the composite's blur: the whole screen when the camera is in or right by a box, none when
/// every box is out of view.
fn screen_rect(
    volumes: &[VolumeGpu],
    clip_from_relative: Mat4,
    (width, height): (u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    const PAD: f32 = 4.0;
    // Corners closer than this to the eye plane would project wildly: draw everything.
    const NEAR_W: f32 = 8.0;
    let full = Some((0, 0, width, height));
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for volume in volumes {
        for corner in 0..8 {
            let pick = |k: usize, bit: usize| {
                if corner & bit == 0 {
                    volume.lo[k]
                } else {
                    volume.hi[k]
                }
            };
            let clip = clip_from_relative * Vec4::new(pick(0, 1), pick(1, 2), pick(2, 4), 1.0);
            if clip.w < NEAR_W {
                return full;
            }
            let ndc = Vec2::new(clip.x, clip.y) / clip.w;
            let pixel = Vec2::new(
                (ndc.x + 1.0) * 0.5 * width as f32,
                (1.0 - ndc.y) * 0.5 * height as f32,
            );
            lo = lo.min(pixel);
            hi = hi.max(pixel);
        }
    }
    let lo = (lo - PAD).max(Vec2::ZERO);
    let hi = (hi + PAD).min(Vec2::new(width as f32, height as f32));
    (hi.x > lo.x && hi.y > lo.y).then(|| {
        let (x, y) = (lo.x.floor() as u32, lo.y.floor() as u32);
        (x, y, (hi.x.ceil() as u32 - x).max(1), (hi.y.ceil() as u32 - y).max(1))
    })
}

fn relative(point: [f32; 3], origin: Vec3) -> [f32; 3] {
    (Vec3::from_array(point) - origin).to_array()
}

#[allow(clippy::too_many_arguments)]
fn draw_cs_smoke(
    view: ViewQuery<(&ViewTarget, &ExtractedView, &SceneDepthTexture)>,
    extracted: Res<ExtractedCsSmoke>,
    frame: Res<super::PublishedRenderFrame>,
    gpu: Option<ResMut<CsSmokeGpu>>,
    mut specialized: ResMut<SpecializedRenderPipelines<CsSmokeGpu>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut context: RenderContext,
) {
    let smoke = &extracted.0;
    let Some(mut gpu) = gpu.filter(|_| !smoke.volumes.is_empty()) else {
        return;
    };
    let exec = &frame.exec_frame;
    let Some(clip_from_world) = exec.clip_from_world else {
        return;
    };
    let (target, extracted_view, depth) = view.into_inner();
    let size = target.main_texture().size();
    if size.width == 0 || size.height == 0 || extracted_view.viewport.z == 0 {
        return;
    }
    let origin = exec.view_origin;
    let world_from_clip = (clip_from_world * Mat4::from_translation(origin)).inverse();
    if !world_from_clip.is_finite() {
        return;
    }
    let multisampled = depth.texture.sample_count() > 1;
    let mut pipeline = |pass| {
        let key = CsSmokeKey {
            pass,
            target: target.main_texture_format(),
            multisampled,
        };
        specialized.specialize(&cache, &gpu, key)
    };
    let (march_id, composite_id) = (pipeline(SmokePass::March), pipeline(SmokePass::Composite));
    let (Some(march), Some(composite)) = (
        cache.get_render_pipeline(march_id),
        cache.get_render_pipeline(composite_id),
    ) else {
        return;
    };
    let (scale, step) = march_for(smoke.quality);
    let half_size = (size.width.div_ceil(scale), size.height.div_ceil(scale));
    if gpu.half.as_ref().is_none_or(|half| half.size != half_size) {
        gpu.half = Some(half_targets(&device, half_size));
    }

    let [w, h, d] = CS_SMOKE_DIM;
    let mut volumes = [VolumeGpu::default(); CS_SMOKE_SLOTS];
    let mut count = 0;
    for volume in &smoke.volumes {
        let slot = volume.slot as usize;
        if slot >= CS_SMOKE_SLOTS || volume.texels.len() != (w * h * d * 4) as usize {
            continue;
        }
        if gpu.uploaded[slot] != Some(volume.id) {
            queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &gpu.atlas,
                    mip_level: 0,
                    origin: Origin3d {
                        x: 0,
                        y: 0,
                        z: slot as u32 * d,
                    },
                    aspect: TextureAspect::All,
                },
                &volume.texels,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: d,
                },
            );
            gpu.uploaded[slot] = Some(volume.id);
        }
        let lo = relative(volume.min, origin);
        let extent = [w, h, d].map(|n| n as f32 * volume.voxel);
        volumes[count] = VolumeGpu {
            lo: [lo[0], lo[1], lo[2], volume.voxel],
            hi: [
                lo[0] + extent[0],
                lo[1] + extent[1],
                lo[2] + extent[2],
                slot as f32,
            ],
            ambient: [
                volume.ambient[0],
                volume.ambient[1],
                volume.ambient[2],
                volume.grow,
            ],
            sun: [volume.sun[0], volume.sun[1], volume.sun[2], volume.alpha],
        };
        count += 1;
        if count == CS_SMOKE_SLOTS {
            break;
        }
    }
    if count == 0 {
        return;
    }
    // Both passes only cover the smokes' boxes on screen; none in view, nothing to draw.
    let rel_clip_from_world = clip_from_world * Mat4::from_translation(origin);
    let Some(rect) = screen_rect(&volumes[..count], rel_clip_from_world, (size.width, size.height))
    else {
        return;
    };
    let mut holes = [HoleGpu::default(); CS_SMOKE_HOLES];
    let hole_count = smoke.holes.len().min(CS_SMOKE_HOLES);
    for (slot, hole) in holes.iter_mut().zip(&smoke.holes) {
        let a = relative(hole.a, origin);
        let b = relative(hole.b, origin);
        *slot = HoleGpu {
            a: [a[0], a[1], a[2], hole.radius],
            b: [b[0], b[1], b[2], hole.strength],
        };
    }
    let params = ParamsGpu {
        world_from_clip: world_from_clip.to_cols_array(),
        screen: [
            size.width as f32,
            size.height as f32,
            1.0 - render_backend::DEPTH_RANGE_BAND,
            smoke.seconds,
        ],
        counts: [count as f32, hole_count as f32, scale as f32, step],
        sun_dir: [smoke.sun_dir[0], smoke.sun_dir[1], smoke.sun_dir[2], 0.0],
        origin: [origin.x, origin.y, origin.z, 0.0],
        volumes,
        holes,
    };
    queue.write_buffer(&gpu.params, 0, bytemuck::bytes_of(&params));
    let Some(half) = gpu.half.as_ref() else {
        return;
    };
    let (march_layout, composite_layout) = if multisampled {
        (&gpu.march_msaa, &gpu.composite_msaa)
    } else {
        (&gpu.march_single, &gpu.composite_single)
    };
    let march_bind = device.create_bind_group(
        "cs_smoke_march",
        &cache.get_bind_group_layout(march_layout),
        &BindGroupEntries::sequential((
            gpu.params.as_entire_buffer_binding(),
            depth.view(),
            &gpu.atlas_view,
            &gpu.noise_view,
            &gpu.clamp,
            &gpu.repeat,
        )),
    );
    let composite_bind = device.create_bind_group(
        "cs_smoke_composite",
        &cache.get_bind_group_layout(composite_layout),
        &BindGroupEntries::sequential((
            gpu.params.as_entire_buffer_binding(),
            depth.view(),
            &half.colour,
            &half.distance,
        )),
    );
    let cleared = |view| {
        Some(RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: Operations {
                load: LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 0.0).into()),
                store: StoreOp::Store,
            },
            depth_slice: None,
        })
    };
    {
        let attachments = [cleared(&half.colour), cleared(&half.distance)];
        let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("cs_smoke_march"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_scissor_rect(rect.0, rect.1, rect.2, rect.3);
        pass.set_render_pipeline(march);
        pass.set_bind_group(0, &march_bind, &[]);
        pass.draw(0..3, 0..1);
    }
    let attachments = [Some(target.get_unsampled_color_attachment())];
    let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("cs_smoke_composite"),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_scissor_rect(rect.0, rect.1, rect.2, rect.3);
    pass.set_render_pipeline(composite);
    pass.set_bind_group(0, &composite_bind, &[]);
    pass.draw(0..3, 0..1);
}

pub(super) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "cs_smoke.wgsl");
    bevy::asset::embedded_asset!(app, "cs_smoke_composite.wgsl");
    app.init_resource::<CsSmokeFrame>();
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<ExtractedCsSmoke>()
        .init_resource::<SpecializedRenderPipelines<CsSmokeGpu>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(ExtractSchedule, extract_cs_smoke)
        .add_systems(
            Core3d,
            draw_cs_smoke
                .in_set(Core3dSystems::PostProcess)
                .after(super::sun_effects::SunEffectsSet)
                .before(super::postfx::PostFxSet),
        );
}
