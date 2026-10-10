//! Counter-Strike 2 assets for the CS models, read at runtime from CS2's own pack
//! (`game/csgo/pak01_dir.vpk`) through `mdl_source2`: a gun's first-person model with CS2's
//! default arms, its clips and its materials' textures (colour, normal, roughness and metalness,
//! occlusion), decoded off the main thread and turned into GPU images (block compressed as CS2
//! ships them).

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use mdl_source2::texture::{Format, Texture};
use mdl_source2::viewmodel::Viewmodel;
use weapon_iw4::cs::Cs2View;

/// CS2's guns and arms ship 4096-pixel colour textures; a viewmodel shows no more than this.
const MAX_COLOUR_SIZE: u32 = 2048;
/// The normal, roughness/metalness and occlusion maps, which carry less detail the eye catches.
const MAX_MAP_SIZE: u32 = 1024;

/// A CS2 viewmodel read and decoded off the main thread: its models and clips, and each material
/// its draws use.
pub(crate) struct Cs2Decoded {
    pub viewmodel: Viewmodel,
    pub materials: HashMap<String, Cs2Material>,
}

/// A texture and the path it was read from (which names it in the image cache).
pub(crate) struct Cs2Texture {
    pub path: String,
    pub texture: Texture,
}

/// How a CS2 material is shaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cs2Shading {
    /// `csgo_weapon.vfx`: roughness in the metal map's red.
    Weapon,
    /// `csgo_character.vfx` (the arms): roughness in the normal map's blue.
    Character,
    /// Colour only: a material with other shaders or without its maps.
    Plain,
}

/// What a CS2 material draws with here: its colour texture, whether that texture's alpha cuts
/// holes (only when the material asks for alpha testing; otherwise alpha holds a mask), and the
/// maps it is lit with.
pub(crate) struct Cs2Material {
    pub colour: Cs2Texture,
    pub alpha_test: bool,
    pub shading: Cs2Shading,
    pub normal: Option<Cs2Texture>,
    /// Roughness or retro-reflectivity (red) and metalness (green).
    pub metal: Option<Cs2Texture>,
    pub ambient_occlusion: Option<Cs2Texture>,
}

fn read(vpk: &mdl_source::Vpk, path: &str) -> Option<Vec<u8>> {
    let compiled = if path.ends_with("_c") {
        path.to_owned()
    } else {
        format!("{path}_c")
    };
    vpk.read(&compiled)
}

fn texture(vpk: &mdl_source::Vpk, path: &str, max_size: u32) -> Option<Cs2Texture> {
    let texture = mdl_source2::texture::load_max(&read(vpk, path)?, max_size).ok()?;
    (!texture.mips.is_empty()).then(|| Cs2Texture {
        path: path.to_owned(),
        texture,
    })
}

/// A material's textures and how it is shaded.
pub(crate) fn material(vpk: &mdl_source::Vpk, name: &str) -> Option<Cs2Material> {
    let material = mdl_source2::material::load(&read(vpk, name)?).ok()?;
    let colour = texture(vpk, material.texture("g_tColor")?, MAX_COLOUR_SIZE)?;
    let map = |param: &str| {
        material
            .texture(param)
            .and_then(|path| texture(vpk, path, MAX_MAP_SIZE))
    };
    let (normal, metal) = (map("g_tNormal"), map("g_tMetalness"));
    let shading = match material.shader.as_str() {
        _ if normal.is_none() || metal.is_none() => Cs2Shading::Plain,
        "csgo_weapon.vfx" => Cs2Shading::Weapon,
        "csgo_character.vfx" => Cs2Shading::Character,
        _ => Cs2Shading::Plain,
    };
    Some(Cs2Material {
        colour,
        alpha_test: material.int("F_ALPHA_TEST").unwrap_or(0) != 0
            || material.int("F_TRANSLUCENT").unwrap_or(0) != 0,
        shading,
        normal,
        metal,
        ambient_occlusion: map("g_tAmbientOcclusion"),
    })
}

/// Read a gun's CS2 viewmodel: the default arms, the gun, the clips of its graph and every
/// material drawn.
pub(crate) fn decode_viewmodel(vpk: &mdl_source::Vpk, view: Cs2View) -> Result<Cs2Decoded, String> {
    let viewmodel = mdl_source2::viewmodel::load(vpk, view.model, view.graph)
        .map_err(|e| format!("{}: {e}", view.model))?;
    let mut materials = HashMap::new();
    for model in [&viewmodel.arms, &viewmodel.weapon] {
        for draw in model.meshes.iter().flat_map(|m| &m.draws) {
            if materials.contains_key(&draw.material) {
                continue;
            }
            if let Some(found) = material(vpk, &draw.material) {
                materials.insert(draw.material.clone(), found);
            }
        }
    }
    Ok(Cs2Decoded {
        viewmodel,
        materials,
    })
}

/// The GPU image of a CS2 texture, every mip it keeps, in its own compressed format: colour
/// read as sRGB, the other maps as plain numbers.
pub(crate) fn image(texture: &Texture, srgb: bool) -> Image {
    let format = match (texture.format, srgb) {
        (Format::Bc1, true) => TextureFormat::Bc1RgbaUnormSrgb,
        (Format::Bc1, false) => TextureFormat::Bc1RgbaUnorm,
        (Format::Bc3, true) => TextureFormat::Bc3RgbaUnormSrgb,
        (Format::Bc3, false) => TextureFormat::Bc3RgbaUnorm,
        (Format::Bc4, _) => TextureFormat::Bc4RUnorm,
        (Format::Bc5, _) => TextureFormat::Bc5RgUnorm,
        (Format::Bc6h, _) => TextureFormat::Bc6hRgbUfloat,
        (Format::Bc7, true) => TextureFormat::Bc7RgbaUnormSrgb,
        (Format::Bc7, false) => TextureFormat::Bc7RgbaUnorm,
        (Format::Rgba8, true) => TextureFormat::Rgba8UnormSrgb,
        (Format::Rgba8, false) => TextureFormat::Rgba8Unorm,
        (Format::Bgra8, true) => TextureFormat::Bgra8UnormSrgb,
        (Format::Bgra8, false) => TextureFormat::Bgra8Unorm,
        (Format::R8, _) => TextureFormat::R8Unorm,
        (Format::Rgba16F, _) => TextureFormat::Rgba16Float,
    };
    let mut image = Image::new_uninit(
        Extent3d {
            width: texture.width,
            height: texture.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = texture.mips.len() as u32;
    image.data = Some(texture.mips.concat());
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    image
}

/// CS2 images already on the GPU, by texture path: the arms (and anything else guns share) are
/// uploaded once, not with every gun.
#[derive(Default)]
pub(crate) struct Cs2Images(HashMap<String, Handle<Image>>);

impl Cs2Images {
    pub fn get(
        &mut self,
        texture: &Cs2Texture,
        srgb: bool,
        images: &mut Assets<Image>,
    ) -> Handle<Image> {
        self.0
            .entry(texture.path.clone())
            .or_insert_with(|| images.add(image(&texture.texture, srgb)))
            .clone()
    }

    /// Full occlusion (white), for a material without an occlusion map.
    pub fn no_occlusion(&mut self, images: &mut Assets<Image>) -> Handle<Image> {
        self.0
            .entry(String::new())
            .or_insert_with(|| {
                let mut white = Image::new_fill(
                    Extent3d {
                        width: 4,
                        height: 4,
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    &[255],
                    TextureFormat::R8Unorm,
                    RenderAssetUsages::RENDER_WORLD,
                );
                white.sampler = ImageSampler::linear();
                images.add(white)
            })
            .clone()
    }
}
