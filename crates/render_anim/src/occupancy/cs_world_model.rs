//! Counter-Strike weapons in players' hands, seen in third person: Counter-Strike: Source world
//! models (`w_rif_ak47.mdl`) instead of the MW2 twin the CS weapon rides on.
//!
//! The remote body kit hides the MW2 gun's parts (its tags stay, so muzzle flashes still have a
//! place), and posing publishes each body's weapon hand (`tag_weapon_right`) as a
//! [`CsHeldWeaponTag`]. A CS:S gun hangs off `ValveBiped.weapon_bone` (x left, y up, z toward the
//! muzzle); MW2's tag has the gun's x forward, y left, z up, so the model's rest pose is baked
//! into the tag's frame once when it loads. The knife and grenades hang off the right hand; a
//! CS:S player model's in-hand weapon bone (`weapon_bone_RHand`) carries them over.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use assets::PreparedWeapons;
use bevy::math::{Affine3A, Vec3A};
use bevy::prelude::*;
use render_gpu::{
    CsViewmodelDraw, CsViewmodelModel, CsViewmodelShading, CsViewmodelVertex,
    CsWorldModelInstance, CsWorldModelsFrame,
};

/// A body's weapon hand this frame, and the weapon it holds.
#[derive(Clone, Copy, Debug)]
pub struct CsHeldWeaponTag {
    pub weapon: u32,
    pub world_from_tag: Mat4,
    /// The MW2 twin's muzzle (`tag_flash`, x along the barrel) in the tag's frame.
    pub muzzle: Option<Mat4>,
}

/// Every posed body's weapon hand, refilled each frame by remote body posing.
#[derive(Resource, Default)]
pub struct CsHeldWeaponTags(pub Vec<CsHeldWeaponTag>);

/// The CS:S world model `weapon` (an MW2 weapon index) shows as: its viewmodel's `w_` twin.
fn world_model_name(weapons: &asset_game::WeaponRegistry, weapon: u32) -> Option<String> {
    use weapon_iw4::cs;
    let index = cs::cs_weapon_index_for(&weapons.script_name_of(weapon))?;
    let view = if let Some(grenade) = cs::cs_grenade(index) {
        grenade.css_view_model
    } else if cs::is_knife(index) {
        cs::CS_KNIFE.css_view_model
    } else if cs::is_c4(index) {
        cs::CS_C4.css_view_model
    } else {
        cs::cs_weapon(index)?.css_view_model
    };
    Some(format!("w_{}", view.strip_prefix("v_")?))
}

/// The CS:S pack, opened once (`None` without CS:S).
fn css_pack() -> Option<&'static mdl_source::Vpk> {
    static PACK: OnceLock<Option<mdl_source::Vpk>> = OnceLock::new();
    PACK.get_or_init(|| {
        let pak = asset_transport::find_css_pak()?;
        mdl_source::Vpk::open(&pak).ok()
    })
    .as_ref()
}

/// Whether CS:S is installed (its models and sounds are the ones used).
pub(crate) fn css_installed() -> bool {
    css_pack().is_some()
}

/// MW2's bomb lying in the world (the bomb mode's carried-then-dropped or planted bomb).
const MW2_BOMB_MODEL: &str = "prop_suitcase_bomb";

/// Whether a script model of `model` is the bomb, drawn as CS:S's C4 instead (`w_c4` lying
/// dropped, `w_c4_planted` once planted).
pub(crate) fn replaces_bomb_model(model: &str) -> bool {
    movement_iw4::rules::CS_RULES
        && model.eq_ignore_ascii_case(MW2_BOMB_MODEL)
        && css_pack().is_some_and(|pack| pack.contains("models/weapons/w_c4_planted.mdl"))
}

/// Whether `weapon` is drawn as a CS:S world model in third person (so its MW2 twin hides).
pub fn shows_cs_world_model(weapons: &asset_game::WeaponRegistry, weapon: u32) -> bool {
    weapon != 0
        && world_model_name(weapons, weapon)
            .zip(css_pack())
            .is_some_and(|(name, pack)| pack.contains(&format!("models/weapons/{name}.mdl")))
}

fn affine(m: &mdl_goldsrc::Mat3x4) -> Affine3A {
    Affine3A::from_cols_array(&[
        m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2], m[0][3],
        m[1][3], m[2][3],
    ])
}

/// CS:S `weapon_bone` axes (x left, y up, z forward) to MW2's weapon tag (x forward, y left,
/// z up).
fn weapon_bone_to_tag() -> Affine3A {
    Affine3A::from_mat3(Mat3::from_cols(Vec3::Y, Vec3::Z, Vec3::X))
}

/// The right hand's frame in the in-hand weapon bone's (`weapon_bone_RHand` under
/// `weapon_bone`), read from a CS:S player model.
fn hand_to_weapon_bone(pack: &mdl_source::Vpk) -> Option<Affine3A> {
    static HAND: OnceLock<Option<Affine3A>> = OnceLock::new();
    *HAND.get_or_init(|| {
        let player = mdl_source::load_model(pack, "models/player/ct_urban.mdl").ok()?;
        let bone = |name: &str| {
            player
                .model
                .bones
                .iter()
                .find(|b| b.name.eq_ignore_ascii_case(name))
                .map(|b| affine(&b.pose_to_bone))
        };
        Some(bone("ValveBiped.weapon_bone")? * bone("ValveBiped.weapon_bone_RHand")?.inverse())
    })
}

fn image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// Loads `models/weapons/<name>.mdl` with its rest pose baked into MW2's weapon tag frame, and
/// where its muzzle ends up; or, `on_floor`, in its own frame standing on the floor (the bomb).
fn load_world_model(
    pack: &mdl_source::Vpk,
    name: &str,
    on_floor: bool,
    images: &mut Assets<Image>,
) -> Result<(CsViewmodelModel, Option<Vec3>), String> {
    let loaded = mdl_source::load_model(pack, &format!("models/weapons/{name}.mdl"))?;
    let studio = &loaded.model;
    let mut skin = Vec::new();
    studio.pose(0, 0.0, &mut skin);
    let skin: Vec<Affine3A> = skin.iter().map(affine).collect();
    let find = |bone: &str| {
        studio
            .bones
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(bone))
    };
    // Guns hang off their weapon bone; the knife and grenades off the right hand.
    let to_tag = if on_floor {
        Affine3A::IDENTITY
    } else {
        let (anchor, carry) = match find("ValveBiped.weapon_bone") {
            Some(bone) => (bone, Affine3A::IDENTITY),
            None => (
                find("ValveBiped.Bip01_R_Hand").ok_or("no weapon bone or right hand")?,
                hand_to_weapon_bone(pack).ok_or("no CS:S player model for the hand")?,
            ),
        };
        let anchor_frame = skin[anchor] * affine(&studio.bones[anchor].pose_to_bone).inverse();
        weapon_bone_to_tag() * carry * anchor_frame.inverse()
    };
    let to_tag_dir = to_tag.matrix3;
    let muzzle = studio
        .attachments
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case("muzzle_flash"))
        .or_else(|| studio.muzzle())
        .and_then(|attachment| {
        let at = skin.get(attachment.bone)? * affine(&attachment.in_bind);
        Some(Vec3::from(to_tag.transform_point3a(at.translation)))
    });

    let mut vertices: Vec<CsViewmodelVertex> = studio
        .vertices
        .iter()
        .map(|v| {
            let (mut position, mut normal) = (Vec3A::ZERO, Vec3A::ZERO);
            for (bone, weight) in v.bones.iter().zip(v.weights) {
                let Some(matrix) = skin.get(usize::from(*bone)).filter(|_| weight > 0.0) else {
                    continue;
                };
                position += matrix.transform_point3a(Vec3A::from(v.position)) * weight;
                normal += matrix.transform_vector3a(Vec3A::from(v.normal)) * weight;
            }
            let position = to_tag.transform_point3a(position);
            let normal = (to_tag_dir * normal).normalize_or_zero();
            CsViewmodelVertex {
                position: position.to_array(),
                normal: normal.to_array(),
                uv: v.uv,
                bones: [0; 4],
                weights: [255, 0, 0, 0],
            }
        })
        .collect();
    if on_floor {
        let floor = vertices
            .iter()
            .map(|v| v.position[2])
            .fold(f32::INFINITY, f32::min);
        for v in &mut vertices {
            v.position[2] -= floor;
        }
    }
    let textures: Vec<Option<Handle<Image>>> = loaded
        .materials
        .iter()
        .map(|m| {
            m.texture.as_ref().map(|t| {
                let mut rgba = t.rgba.clone();
                // Only alpha-tested materials cut holes; others keep masks in alpha.
                if !m.alpha_test {
                    for px in rgba.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                }
                images.add(image(t.width, t.height, rgba))
            })
        })
        .collect();
    let draws = studio
        .meshes
        .iter()
        // The first model of each body group (the plain gun).
        .filter(|mesh| mesh.body_model == 0)
        .filter_map(|mesh| {
            let material = loaded.materials.get(mesh.material)?;
            (!material.additive).then_some(())?;
            Some(CsViewmodelDraw {
                image: textures.get(mesh.material)?.clone()?,
                first_vertex: mesh.first_vertex as u32,
                vertex_count: mesh.vertex_count as u32,
                shading: if material.fullbright {
                    CsViewmodelShading::Fullbright
                } else {
                    CsViewmodelShading::Lit
                },
            })
        })
        .collect();
    Ok((
        CsViewmodelModel {
            id: super::cs_viewmodel::next_model_id().fetch_add(1, Ordering::Relaxed),
            vertices,
            draws,
        },
        muzzle,
    ))
}

/// A loaded world model and its bounds in the tag frame.
#[derive(Clone)]
struct WorldModel {
    gpu: Arc<CsViewmodelModel>,
    bounds: (Vec3, Vec3),
    /// The muzzle (`muzzle_flash` attachment) in the tag frame; guns only.
    muzzle: Option<Vec3>,
}

/// Loaded world models by name (`None` when one failed to load), and where each weapon's model
/// sits on the tag.
#[derive(Resource, Default)]
pub struct CsWorldModels {
    models: HashMap<String, Option<WorldModel>>,
    offsets: HashMap<u32, Mat4>,
}

fn bounds_of(points: impl Iterator<Item = Vec3>) -> Option<(Vec3, Vec3)> {
    points.fold(None, |acc, p| match acc {
        None => Some((p, p)),
        Some((lo, hi)) => Some((lo.min(p), hi.max(p))),
    })
}

/// The hand on a pistol, from its muzzle (in the muzzle's frame): measured on the CS:S USP once
/// its muzzle sits on MW2's (the top of the grip, 8.4 behind and 1.7 below the muzzle).
const PISTOL_GRIP_FROM_MUZZLE: Vec3 = Vec3::new(-8.4, 0.0, -1.7);

/// Where a CS model sits on the weapon tag. A gun goes muzzle on its MW2 twin's muzzle, barrel
/// along barrel (CS:S's weapon bone isn't at the grip as MW2's tag is, and the twin is the same
/// kind of gun, already held right). The knife and grenades have no muzzle: their handle (the
/// back quarter of the knife, the middle of a grenade) goes where a pistol's grip is.
fn tag_from_model(model: &WorldModel, mw2_muzzle: Option<Mat4>) -> Mat4 {
    let Some(mw2) = mw2_muzzle else {
        return Mat4::IDENTITY;
    };
    let grip = match model.muzzle {
        Some(muzzle) => return mw2 * Mat4::from_translation(-muzzle),
        None => {
            let (lo, hi) = model.bounds;
            let centre = (lo + hi) * 0.5;
            let long = hi.x - lo.x > 2.0 * (hi.z - lo.z).max(hi.y - lo.y);
            if long {
                Vec3::new(lo.x + (hi.x - lo.x) * 0.22, centre.y, centre.z)
            } else {
                centre
            }
        }
    };
    mw2 * Mat4::from_translation(PISTOL_GRIP_FROM_MUZZLE - grip)
}

impl CsWorldModels {
    fn get(&mut self, name: &str, on_floor: bool, images: &mut Assets<Image>) -> Option<WorldModel> {
        let key = if on_floor {
            format!("{name} (floor)")
        } else {
            name.to_owned()
        };
        if let Some(model) = self.models.get(&key) {
            return model.clone();
        }
        let model = css_pack().and_then(|pack| match load_world_model(pack, name, on_floor, images) {
            Ok((model, muzzle)) => {
                diag::info!(
                    World,
                    "cs world model {name}: {} triangles",
                    model.vertices.len() / 3
                );
                let bounds = bounds_of(model.vertices.iter().map(|v| Vec3::from(v.position)))
                    .unwrap_or_default();
                Some(WorldModel {
                    gpu: Arc::new(model),
                    bounds,
                    muzzle,
                })
            }
            Err(error) => {
                diag::warn!(World, "cs world model {name}: {error}");
                None
            }
        });
        self.models.insert(key, model.clone());
        model
    }
}

/// Places a CS:S world model in every posed hand holding a CS weapon, and the CS:S C4 where the
/// bomb lies. The light comes later (`render_frontend`'s CS lighting reads the map's light grid
/// at each one).
pub fn update_cs_world_models(
    tags: Res<CsHeldWeaponTags>,
    weapons: Option<Res<PreparedWeapons>>,
    presented: Res<net::PresentedSnapshot>,
    bombs: Query<(
        &render_scene::WorldScriptModelInstance,
        &Transform,
        &Visibility,
    )>,
    mut models: ResMut<CsWorldModels>,
    mut images: ResMut<Assets<Image>>,
    mut frame: ResMut<CsWorldModelsFrame>,
) {
    frame.instances.clear();
    let planted = presented
        .snapshot()
        .and_then(super::cs_bomb::bomb_state)
        .is_some_and(|state| state == "planted");
    for (owner, transform, visibility) in &bombs {
        if *visibility == Visibility::Hidden || !replaces_bomb_model(&owner.current_model.0) {
            continue;
        }
        let name = if planted { "w_c4_planted" } else { "w_c4" };
        let Some(model) = models.get(name, true, &mut images) else {
            continue;
        };
        frame.instances.push(CsWorldModelInstance {
            model: model.gpu,
            world_from_model: transform.to_matrix().to_cols_array_2d(),
            ambient: [0.6; 3],
            sun_dir: [0.0; 3],
            sun: [0.0; 3],
        });
    }
    let Some(weapons) = weapons else {
        return;
    };
    for tag in &tags.0 {
        let Some(name) = world_model_name(&weapons.0, tag.weapon) else {
            continue;
        };
        let Some(model) = models.get(&name, false, &mut images) else {
            continue;
        };
        let tag_from_model = *models
            .offsets
            .entry(tag.weapon)
            .or_insert_with(|| tag_from_model(&model, tag.muzzle));
        frame.instances.push(CsWorldModelInstance {
            model: model.gpu,
            world_from_model: (tag.world_from_tag * tag_from_model).to_cols_array_2d(),
            ambient: [0.6; 3],
            sun_dir: [0.0; 3],
            sun: [0.0; 3],
        });
    }
}
