//! Counter-Strike sniper scope: while the local player's CS scope is zoomed
//! (`PlayerState::cs_zoom`), `render_gpu`'s scope pass draws the ring CS:S draws. Its arc and lens
//! grime textures are read from the CS:S pack at runtime; without CS:S installed the arc is a
//! generated circle and there is no grime.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use frame::ViewSubject;
use net::{LocalPresentClient, PresentedSnapshot};
use render_gpu::CsScopeFrame;

/// Side of the generated arc texture.
const ARC_SIZE: u32 = 256;
/// How strongly the lens grime shows through the glass.
const LENS_ALPHA: f32 = 0.5;

/// The scope textures, looked up once.
#[derive(Resource, Default)]
pub struct CsScopeImages {
    loaded: bool,
    arc: Option<Handle<Image>>,
    lens: Option<Handle<Image>>,
}

fn rgba_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// A quarter ring like CS:S's `sprites/scope_arc`: clear inside a circle centred on the top left
/// corner, black outside, with a soft edge.
fn generated_arc() -> Image {
    let size = ARC_SIZE as f32;
    let mut rgba = Vec::with_capacity((ARC_SIZE * ARC_SIZE * 4) as usize);
    for y in 0..ARC_SIZE {
        for x in 0..ARC_SIZE {
            let r = (x as f32 + 0.5).hypot(y as f32 + 0.5) / size;
            let alpha = ((r - 0.96) / 0.03).clamp(0.0, 1.0);
            rgba.extend_from_slice(&[0, 0, 0, (alpha * 255.0) as u8]);
        }
    }
    rgba_image(ARC_SIZE, ARC_SIZE, rgba)
}

fn load_images(images: &mut Assets<Image>) -> CsScopeImages {
    let pack = asset_transport::find_css_pak().and_then(|pak| mdl_source::Vpk::open(&pak).ok());
    let texture = |path: &str| {
        let bytes = pack.as_ref()?.read(path)?;
        let image = mdl_source::vtf::decode(&bytes).ok()?;
        Some(rgba_image(image.width, image.height, image.rgba))
    };
    let arc = texture("materials/sprites/scope_arc.vtf");
    let from_pack = arc.is_some();
    let arc = images.add(arc.unwrap_or_else(generated_arc));
    let lens = texture("materials/overlays/scope_lens.vtf").map(|image| images.add(image));
    diag::info!(
        World,
        "cs scope: {} arc, {}",
        if from_pack { "CS:S" } else { "generated" },
        if lens.is_some() {
            "lens grime"
        } else {
            "no lens grime"
        }
    );
    CsScopeImages {
        loaded: true,
        arc: Some(arc),
        lens,
    }
}

pub fn update_cs_scope(
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    view: Res<ViewSubject>,
    mut textures: ResMut<CsScopeImages>,
    mut images: ResMut<Assets<Image>>,
    mut frame: ResMut<CsScopeFrame>,
) {
    let zoomed = !view.in_killcam()
        && presented
            .player(local.0)
            .is_some_and(|ps| ps.cs_zoom != 0 && ps.pm_type < playerstate_iw4::PM_TYPE_DEAD);
    if !zoomed {
        if frame.active {
            frame.active = false;
        }
        return;
    }
    if !textures.loaded {
        *textures = load_images(&mut images);
    }
    *frame = CsScopeFrame {
        active: true,
        arc: textures.arc.clone(),
        lens: textures.lens.clone(),
        lens_alpha: LENS_ALPHA,
    };
}
