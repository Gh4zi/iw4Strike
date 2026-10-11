//! AMD FSR 3 upscaling (`r_fsr`, Video > Advanced). With it on, the 3D camera renders the world
//! at a lower internal resolution into an off-screen image and FSR upscales it into a full-size
//! frame. The viewmodel, the scope and the in-world HUD then draw on that frame at full size, and
//! a full-window image under the UI shows it. Off, the 3D camera draws straight to the window as
//! it always has.

mod alpha;
mod gpu;

pub(crate) use gpu::{FsrOutput, prepare_fsr};

use bevy::{
    asset::RenderAssetUsages,
    camera::{ImageRenderTarget, RenderTarget},
    prelude::*,
    render::{
        extract_resource::ExtractResource,
        render_resource::{
            Extent3d, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
        },
    },
    window::{PrimaryWindow, WindowRef},
};

/// FSR 3's quality modes, as `GameSettings::fsr_mode` numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsrMode {
    Off,
    NativeAa,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}

impl FsrMode {
    pub fn from_setting(value: u8) -> Self {
        match value {
            1 => Self::NativeAa,
            2 => Self::Quality,
            3 => Self::Balanced,
            4 => Self::Performance,
            5 => Self::UltraPerformance,
            _ => Self::Off,
        }
    }

    /// Display size over render size, per axis (AMD's ratios for each mode).
    pub fn upscale_ratio(self) -> f32 {
        match self {
            Self::Off | Self::NativeAa => 1.0,
            Self::Quality => 1.5,
            Self::Balanced => 1.7,
            Self::Performance => 2.0,
            Self::UltraPerformance => 3.0,
        }
    }

    /// The internal render size for a display size.
    pub fn render_size(self, display: UVec2) -> UVec2 {
        (display.as_vec2() / self.upscale_ratio())
            .round()
            .as_uvec2()
            .max(UVec2::ONE)
    }
}

/// What the render world needs to run FSR this frame.
#[derive(Resource, Clone, Default, ExtractResource)]
pub(crate) struct FsrFrame {
    pub active: bool,
    pub render_size: UVec2,
    /// The full-size frame FSR writes and the passes after it draw on.
    pub output: Option<Handle<Image>>,
    /// RCAS strength, 0 (off) to 1.
    pub sharpness: f32,
}

/// The images the 3D camera and FSR render into while FSR is on, and what shows them.
#[derive(Resource, Default)]
struct FsrTarget {
    image: Option<Handle<Image>>,
    size: UVec2,
    /// Render pixels per window logical pixel, so `world_to_viewport` (HUD waypoints, overhead
    /// names) still answers in window coordinates.
    scale_factor: f32,
    output: Option<Handle<Image>>,
    display: UVec2,
    present: Option<Entity>,
}

impl FsrTarget {
    fn image_target(&self) -> Option<ImageRenderTarget> {
        Some(ImageRenderTarget {
            handle: self.image.clone()?,
            scale_factor: self.scale_factor,
        })
    }
}

/// The full-window image under the HUD that shows the upscaled frame.
#[derive(Component)]
struct FsrPresent;

/// The UI draws nothing below this, so the frame sits under every HUD element and menu.
const PRESENT_Z: i32 = i32::MIN + 1;

pub(crate) fn register(app: &mut App) {
    app.init_resource::<FsrTarget>()
        .init_resource::<FsrFrame>()
        .add_systems(Update, apply_fsr_target);
    gpu::register(app);
    alpha::register(app);
}

/// The camera already renders into our image.
fn is_ours(render_target: Option<&RenderTarget>, image: Option<&Handle<Image>>) -> bool {
    matches!(
        (render_target, image),
        (Some(RenderTarget::Image(target)), Some(handle)) if target.handle == *handle
    )
}

fn new_image(size: UVec2, format: TextureFormat, usage: TextureUsages) -> Image {
    let mut image = Image::new_fill(
        Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        format,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = usage;
    image
}

/// Points the 3D camera at the internal-resolution image while FSR is on (sized from the
/// window and the mode), and back at the window when it is off.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn apply_fsr_target(
    mut commands: Commands,
    settings: Res<frame::GameSettings>,
    windows: Query<&Window, With<PrimaryWindow>>,
    // The world's camera only: the main menu's camera is a 3D camera too, but it also draws the
    // UI, so taking it sent the whole menu into FSR's image and left the window black.
    cameras: Query<
        (Entity, &Camera, Option<&RenderTarget>),
        (With<Camera3d>, Without<frame::UiCamera>),
    >,
    mut images: ResMut<Assets<Image>>,
    mut target: ResMut<FsrTarget>,
    mut frame: ResMut<FsrFrame>,
    mut presents: Query<(&mut ImageNode, &mut Visibility), With<FsrPresent>>,
) {
    let mode = FsrMode::from_setting(settings.fsr_mode);
    let Ok((camera_entity, camera, render_target)) = cameras.single() else {
        // No world (main menu): nothing to show, and a frame left from a match must not cover it.
        frame.active = false;
        if let Some((_, mut visibility)) = target
            .present
            .and_then(|present| presents.get_mut(present).ok())
            && *visibility != Visibility::Hidden
        {
            *visibility = Visibility::Hidden;
        }
        return;
    };
    if mode == FsrMode::Off {
        frame.active = false;
        if is_ours(render_target, target.image.as_ref()) {
            commands
                .entity(camera_entity)
                .insert(RenderTarget::Window(WindowRef::Primary));
        }
        if let Some(present) = target.present.take() {
            commands.entity(present).despawn();
        }
        target.image = None;
        target.output = None;
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let display = window.physical_size();
    // A minimized window reports 0x0: keep the last images until it is back.
    if display.x < 64 || display.y < 64 {
        return;
    }
    let size = mode.render_size(display);
    target.scale_factor = size.x as f32 / window.width().max(1.0);
    if target.image.is_none() || target.size != size {
        // The camera's own target: Bevy blits the finished view into it (FSR reads the view).
        let usage = TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::COPY_SRC
            | TextureUsages::RENDER_ATTACHMENT;
        target.image = Some(images.add(new_image(size, TextureFormat::Rgba8UnormSrgb, usage)));
        target.size = size;
        diag::info!(
            World,
            "fsr: rendering at {}x{} for a {}x{} display ({:?})",
            size.x,
            size.y,
            display.x,
            display.y,
            mode
        );
    }
    if target.output.is_none() || target.display != display {
        // The scene's own format: the passes after FSR draw on it with their usual pipelines.
        let usage = TextureUsages::TEXTURE_BINDING
            | TextureUsages::STORAGE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::COPY_SRC
            | TextureUsages::RENDER_ATTACHMENT;
        let mut output = new_image(display, TextureFormat::Rgba8Unorm, usage);
        // The scene keeps gamma-encoded bytes in a plain RGBA8 target, so the UI reads the frame
        // through an sRGB view (the render world draws on a plain one).
        output.texture_descriptor.view_formats = &[TextureFormat::Rgba8UnormSrgb];
        output.texture_view_descriptor = Some(TextureViewDescriptor {
            format: Some(TextureFormat::Rgba8UnormSrgb),
            // sRGB views cannot be storage: this one only samples.
            usage: Some(TextureUsages::TEXTURE_BINDING),
            ..default()
        });
        target.output = Some(images.add(output));
        target.display = display;
    }
    let wanted = target.image_target().expect("render image set above");
    if !matches!(render_target, Some(RenderTarget::Image(current)) if *current == wanted) {
        commands
            .entity(camera_entity)
            .insert(RenderTarget::Image(wanted));
    }
    let output = target.output.clone().expect("output image set above");
    *frame = FsrFrame {
        active: true,
        render_size: size,
        output: Some(output.clone()),
        sharpness: settings.fsr_sharpness,
    };
    let shown = if camera.is_active {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    match target
        .present
        .and_then(|present| presents.get_mut(present).ok())
    {
        Some((mut node, mut visibility)) => {
            if node.image != output {
                node.image = output;
            }
            if *visibility != shown {
                *visibility = shown;
            }
        }
        None if target.present.is_none() => {
            let present = commands
                .spawn((
                    FsrPresent,
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(0),
                        top: px(0),
                        width: percent(100),
                        height: percent(100),
                        ..default()
                    },
                    ImageNode::new(output),
                    GlobalZIndex(PRESENT_Z),
                    Pickable::IGNORE,
                    shown,
                ))
                .id();
            target.present = Some(present);
        }
        None => {}
    }
}
