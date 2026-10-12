//! The CS:GO radar's map, drawn on the GPU: the map's overview texture sampled in a circle around
//! the player and turned with their view, as a UI material on the radar's inner panel. Each frame
//! only its few numbers change.

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::ui_render::prelude::{UiMaterial, UiMaterialPlugin};

const SHADER_PATH: &str = "embedded://hud/csgo_radar.wgsl";

/// Where the radar looks: in world units, MW2's compass bounds.
#[derive(ShaderType, Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RadarParams {
    /// xy: the player's position; z: world units across the radar.
    pub at: Vec4,
    /// The view's forward (xy) and right (zw) on the map.
    pub axes: Vec4,
    /// The overview's east (xy) and south (zw) directions in the world.
    pub map_axes: Vec4,
    /// The overview's upper left corner (xy) and its size in world units (zw).
    pub map: Vec4,
}

impl RadarParams {
    pub(crate) fn new(
        bounds: &hud_iw4::CompassMapBounds,
        at: [f32; 2],
        yaw: f32,
        range: f32,
    ) -> Self {
        let (sin, cos) = yaw.to_radians().sin_cos();
        Self {
            at: Vec4::new(at[0], at[1], range, 0.0),
            axes: Vec4::new(cos, sin, sin, -cos),
            map_axes: Vec4::new(
                bounds.north[1],
                -bounds.north[0],
                -bounds.north[0],
                -bounds.north[1],
            ),
            map: Vec4::new(
                bounds.upper_left[0],
                bounds.upper_left[1],
                bounds.world_size[0],
                bounds.world_size[1],
            ),
        }
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub(crate) struct RadarMaterial {
    #[uniform(0)]
    pub params: RadarParams,
    #[texture(1)]
    #[sampler(2)]
    pub map: Handle<Image>,
}

impl UiMaterial for RadarMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }
}

pub(crate) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "csgo_radar.wgsl");
    app.add_plugins(UiMaterialPlugin::<RadarMaterial>::default());
}

/// The overview `rgba` (`width` × `height`) as a texture the radar samples smoothly.
pub(crate) fn map_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    let mut image = Image::new(
        bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::linear();
    image
}
