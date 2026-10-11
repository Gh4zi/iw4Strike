//! AMD FSR 3 upscaling (`r_fsr`, Video > Advanced). With it on, the 3D camera renders the scene
//! at a lower internal resolution into an off-screen image, the frame is upscaled to the
//! window, and the HUD and menus (drawn by their own cameras) go over it at full size. Off, the
//! 3D camera draws straight to the window as it always has.
//!
//! This is the plumbing: the quality modes, the internal resolution and the present path. Until
//! FSR's own passes land, the upscale is the UI's bilinear stretch of the internal image.

use bevy::{
    asset::RenderAssetUsages,
    camera::RenderTarget,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
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

/// The off-screen image the 3D camera renders into while FSR is on, and what shows it.
#[derive(Resource, Default)]
struct FsrTarget {
    image: Option<Handle<Image>>,
    size: UVec2,
    present: Option<Entity>,
}

/// The full-window image under the HUD that shows the upscaled frame.
#[derive(Component)]
struct FsrPresent;

/// The UI draws nothing below this, so the frame sits under every HUD element and menu.
const PRESENT_Z: i32 = i32::MIN + 1;

pub(crate) fn register(app: &mut App) {
    app.init_resource::<FsrTarget>()
        .add_systems(Update, apply_fsr_target);
}

/// The camera already renders into our image.
fn is_ours(render_target: Option<&RenderTarget>, image: Option<&Handle<Image>>) -> bool {
    matches!(
        (render_target, image),
        (Some(RenderTarget::Image(target)), Some(handle)) if target.handle == *handle
    )
}

fn new_render_image(size: UVec2) -> Image {
    let mut image = Image::new_fill(
        Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::COPY_SRC
        | TextureUsages::RENDER_ATTACHMENT;
    image
}

/// Points the 3D camera at the internal-resolution image while FSR is on (sized from the
/// window and the mode), and back at the window when it is off.
#[allow(clippy::type_complexity)]
fn apply_fsr_target(
    mut commands: Commands,
    settings: Res<frame::GameSettings>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(Entity, &Camera, Option<&RenderTarget>), With<Camera3d>>,
    mut images: ResMut<Assets<Image>>,
    mut target: ResMut<FsrTarget>,
    mut presents: Query<(&mut ImageNode, &mut Visibility), With<FsrPresent>>,
) {
    let mode = FsrMode::from_setting(settings.fsr_mode);
    let Ok((camera_entity, camera, render_target)) = cameras.single() else {
        return;
    };
    if mode == FsrMode::Off {
        if is_ours(render_target, target.image.as_ref()) {
            commands
                .entity(camera_entity)
                .insert(RenderTarget::Window(WindowRef::Primary));
        }
        if let Some(present) = target.present.take() {
            commands.entity(present).despawn();
        }
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let display = window.physical_size();
    // A minimized window reports 0x0: keep the last image until it is back.
    if display.x < 64 || display.y < 64 {
        return;
    }
    let size = mode.render_size(display);
    if target.image.is_none() || target.size != size {
        let handle = images.add(new_render_image(size));
        target.image = Some(handle);
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
    let handle = target.image.clone().expect("render image set above");
    if !is_ours(render_target, target.image.as_ref()) {
        commands
            .entity(camera_entity)
            .insert(RenderTarget::Image(handle.clone().into()));
    }
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
            if node.image != handle {
                node.image = handle;
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
                    ImageNode::new(handle),
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
