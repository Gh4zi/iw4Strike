//! Counter-Strike weapons in players' hands, seen in third person, and lying dropped: the CS
//! game's own models instead of the MW2 twin the CS weapon rides on. Counter-Strike: Source's
//! world model (`w_rif_ak47.mdl`) either way; without CS:S, Condition Zero's or CS 1.6's
//! (`p_ak47.mdl` in a hand, `w_ak47.mdl` dropped, CZ's when it has one).
//!
//! The remote body kit hides the MW2 gun's parts (its tags stay, so muzzle flashes still have a
//! place), and posing publishes each body's weapon hand (`tag_weapon_right`) as a
//! [`CsHeldWeaponTag`]. A CS:S gun hangs off `ValveBiped.weapon_bone` (x left, y up, z toward the
//! muzzle); MW2's tag has the gun's x forward, y left, z up, so the model's rest pose is baked
//! into the tag's frame once when it loads. The knife and grenades hang off the right hand; a
//! CS:S player model's in-hand weapon bone (`weapon_bone_RHand`) carries them over. A GoldSrc
//! `p_` model is the gun in a player model's idle pose, held level: see
//! [`build_goldsrc_world_model`]. A dropped weapon (`item` skips its MW2 twin) lies on its widest
//! side where the item is.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use assets::PreparedWeapons;
use bevy::math::{Affine3A, Vec3A};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use render_gpu::{
    CsViewmodelDraw, CsViewmodelModel, CsViewmodelShading, CsViewmodelVertex, CsWorldModelInstance,
    CsWorldModelsFrame,
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

/// The CS weapon `weapon` (an MW2 weapon index) is: its viewmodel's GoldSrc and CS:S names.
fn view_models(
    weapons: &asset_game::WeaponRegistry,
    weapon: u32,
) -> Option<(&'static str, &'static str)> {
    use weapon_iw4::cs;
    let index = cs::cs_weapon_index_for(&weapons.script_name_of(weapon))?;
    Some(if let Some(grenade) = cs::cs_grenade(index) {
        (grenade.view_model, grenade.css_view_model)
    } else if cs::is_knife(index) {
        (cs::CS_KNIFE.view_model, cs::CS_KNIFE.css_view_model)
    } else if cs::is_c4(index) {
        (cs::CS_C4.view_model, cs::CS_C4.css_view_model)
    } else {
        let gun = cs::cs_weapon(index)?;
        (gun.view_model, gun.css_view_model)
    })
}

/// Where a CS model is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    /// In a hand: baked into MW2's weapon tag frame.
    Hand,
    /// Standing on the floor in its own frame (the bomb).
    Floor,
    /// Dropped: lying on its widest side, centred on the origin.
    Flat,
}

/// The model `weapon` shows as: CS:S's `w_` twin of its viewmodel, in a hand or dropped;
/// GoldSrc's `p_` in a hand and `w_` dropped.
fn model_name(
    weapons: &asset_game::WeaponRegistry,
    weapon: u32,
    placement: Placement,
) -> Option<String> {
    let (goldsrc, css) = view_models(weapons, weapon)?;
    if css_pack().is_some() {
        return Some(format!("w_{}", css.strip_prefix("v_")?));
    }
    let stem = goldsrc.strip_prefix("v_")?;
    Some(if placement == Placement::Hand {
        format!("p_{stem}")
    } else {
        format!("w_{stem}")
    })
}

/// The GoldSrc folders (Condition Zero, then CS 1.6), looked up once; `None` with CS:S found.
fn goldsrc_dirs() -> Option<&'static asset_transport::GoldSrcDirs> {
    static DIRS: OnceLock<Option<asset_transport::GoldSrcDirs>> = OnceLock::new();
    DIRS.get_or_init(asset_transport::find_goldsrc).as_ref()
}

/// Where the model `name` is read from: CS:S's pack, or the GoldSrc folders.
fn model_path(name: &str) -> String {
    if css_pack().is_some() {
        format!("models/weapons/{name}.mdl")
    } else {
        format!("models/{name}.mdl")
    }
}

/// Whether the CS game in use has the model `name` (each name looked up once).
fn has_model(name: &str) -> bool {
    static FOUND: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let mut found = FOUND
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(&known) = found.get(name) {
        return known;
    }
    let path = model_path(name);
    let known = match (css_pack(), goldsrc_dirs()) {
        (Some(pack), _) => pack.contains(&path),
        (None, Some(dirs)) => dirs.file(&path).is_some(),
        (None, None) => false,
    };
    found.insert(name.to_owned(), known);
    known
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

/// The bomb's model, dropped or `planted`: CS:S's `w_c4` and `w_c4_planted`; GoldSrc's backpack
/// and `w_c4`.
fn bomb_model(planted: bool) -> &'static str {
    match (css_pack().is_some(), planted) {
        (true, false) => "w_c4",
        (true, true) => "w_c4_planted",
        (false, false) => "w_backpack",
        (false, true) => "w_c4",
    }
}

/// Whether a script model of `model` is the bomb, drawn as the CS game's own instead.
pub(crate) fn replaces_bomb_model(model: &str) -> bool {
    movement_iw4::rules::CS_RULES
        && model.eq_ignore_ascii_case(MW2_BOMB_MODEL)
        && has_model(bomb_model(false))
        && has_model(bomb_model(true))
}

/// Whether `weapon` is drawn as a CS model in third person (so its MW2 twin hides).
pub fn shows_cs_world_model(weapons: &asset_game::WeaponRegistry, weapon: u32) -> bool {
    weapon != 0 && model_name(weapons, weapon, Placement::Hand).is_some_and(|name| has_model(&name))
}

/// Whether `weapon` lying dropped is drawn as a CS model (so `item` skips its MW2 twin).
pub(crate) fn shows_cs_dropped_model(weapons: &asset_game::WeaponRegistry, weapon: u32) -> bool {
    weapon != 0 && model_name(weapons, weapon, Placement::Flat).is_some_and(|name| has_model(&name))
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

/// Puts a model built in its own frame where `placement` wants it: the bomb standing on the
/// floor; a dropped weapon on its widest side (its thinnest extent upright), centred on the
/// origin with its underside on the floor. A model for a hand stays in the tag frame.
fn settle(vertices: &mut [CsViewmodelVertex], placement: Placement) {
    let bounds =
        |vertices: &[CsViewmodelVertex]| bounds_of(vertices.iter().map(|v| Vec3::from(v.position)));
    if placement == Placement::Hand {
        return;
    }
    if placement == Placement::Flat
        && let Some((lo, hi)) = bounds(vertices)
    {
        let size = hi - lo;
        // A cyclic swap of the axes (a rotation) brings the thinnest one up.
        let turn = |v: [f32; 3]| {
            if size.x <= size.y.min(size.z) {
                [v[1], v[2], v[0]]
            } else if size.y <= size.z {
                [v[2], v[0], v[1]]
            } else {
                v
            }
        };
        for v in vertices.iter_mut() {
            v.position = turn(v.position);
            v.normal = turn(v.normal);
        }
    }
    let Some((lo, hi)) = bounds(vertices) else {
        return;
    };
    let centre = (lo + hi) * 0.5;
    let shift = if placement == Placement::Flat {
        Vec3::new(centre.x, centre.y, lo.z)
    } else {
        Vec3::new(0.0, 0.0, lo.z)
    };
    for v in vertices.iter_mut() {
        v.position = (Vec3::from(v.position) - shift).to_array();
    }
}

/// Builds a CS:S world model read from `models/weapons/<name>.mdl`, its rest pose baked into
/// MW2's weapon tag frame, and where its muzzle ends up; or, on the floor or dropped, in its own
/// frame settled there.
fn build_world_model(
    pack: &mdl_source::Vpk,
    loaded: mdl_source::LoadedModel,
    placement: Placement,
    images: &mut Assets<Image>,
) -> Result<(CsViewmodelModel, Option<Vec3>), String> {
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
    let to_tag = if placement != Placement::Hand {
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
                tangent: [0.0; 4],
            }
        })
        .collect();
    settle(&mut vertices, placement);
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
                maps: None,
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

/// A held weapon's barrel and up directions, from its shape alone (a `p_` model's idle pose holds
/// it at any angle): the barrel is its long axis, pointing away from the hand (a gun's muzzle
/// reaches further from the hand than its stock); up is the longer of the other two (a gun is
/// taller than it is wide), toward the side reaching less far from the muzzle (the grip and
/// magazine hang below the bore).
fn held_axes(points: &[Vec3], hand: Vec3) -> Option<(Vec3, Vec3)> {
    let (lo, hi) = bounds_of(points.iter().copied())?;
    let mean = points.iter().sum::<Vec3>() / points.len() as f32;
    let outer = |v: Vec3| Mat3::from_cols(v * v.x, v * v.y, v * v.z);
    let spread = points
        .iter()
        .fold(Mat3::ZERO, |sum, p| sum + outer(*p - mean));
    // The covariance's main directions, by power iteration from the bounds' longest sides.
    let principal = |m: Mat3, seed: Vec3| (0..64).fold(seed, |v, _| (m * v).normalize_or(v));
    let size = hi - lo;
    let longest = if size.x >= size.y.max(size.z) {
        Vec3::X
    } else if size.y >= size.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let along = principal(spread, longest);
    let rest = spread - outer(along) * along.dot(spread * along);
    let seed = [Vec3::X, Vec3::Y, Vec3::Z]
        .map(|axis| axis - along * axis.dot(along))
        .into_iter()
        .max_by(|a, b| a.length().total_cmp(&b.length()))?;
    let across = principal(rest, seed.normalize());
    let across = (across - along * across.dot(along)).normalize_or(along.any_orthonormal_vector());
    let extent = |axis: Vec3| {
        points
            .iter()
            .map(|p| p.dot(axis))
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| {
                (lo.min(x), hi.max(x))
            })
    };
    let (back, front) = extent(along);
    let at = hand.dot(along);
    let forward = if front - at >= at - back {
        along
    } else {
        -along
    };
    let (_, tip) = extent(forward);
    let muzzle: Vec<f32> = points
        .iter()
        .filter(|p| p.dot(forward) > tip - 1.5)
        .map(|p| p.dot(across))
        .collect();
    let bore = muzzle.iter().sum::<f32>() / muzzle.len().max(1) as f32;
    let (below, above) = extent(across);
    let up = if above - bore > bore - below {
        -across
    } else {
        across
    };
    Some((forward, up))
}

/// Model names of CS weapons without a muzzle (the knife, grenades, the bomb).
const NO_MUZZLE: [&str; 4] = ["knife", "grenade", "flashbang", "c4"];

/// Builds a GoldSrc model (Condition Zero's or CS 1.6's `p_` or `w_`) at its first frame. A `p_`
/// model is the weapon in a player model's idle pose, held level at the right hand (`Bip01 R
/// Hand`), the rig and the facing differing from model to model: its long horizontal axis is
/// the barrel, pointing away from the hand (a gun's muzzle reaches further from the hand than
/// its stock), and up is up. The hand goes on the tag, and a gun's muzzle is the middle of its
/// far end. A `w_` model already lies on the floor.
fn build_goldsrc_world_model(
    name: &str,
    studio: &mdl_goldsrc::StudioModel,
    placement: Placement,
    images: &mut Assets<Image>,
) -> Result<(CsViewmodelModel, Option<Vec3>), String> {
    let mut pose = Vec::new();
    studio.pose(0, 0.0, &mut pose);
    let bone = |index: u8| {
        pose.get(usize::from(index))
            .unwrap_or(&mdl_goldsrc::IDENTITY)
    };
    let mut vertices: Vec<CsViewmodelVertex> = studio
        .vertices
        .iter()
        .map(|v| {
            let normal = affine(bone(v.normal_bone)).transform_vector3(Vec3::from(v.normal));
            CsViewmodelVertex {
                position: mdl_goldsrc::transform_point(bone(v.bone), v.position),
                normal: normal.normalize_or_zero().to_array(),
                uv: v.uv,
                bones: [0; 4],
                weights: [255, 0, 0, 0],
                tangent: [0.0; 4],
            }
        })
        .collect();
    let positions = |vertices: &[CsViewmodelVertex]| {
        vertices
            .iter()
            .map(|v| Vec3::from(v.position))
            .collect::<Vec<_>>()
    };
    let mut muzzle = None;
    if placement == Placement::Hand {
        let hand = studio
            .bones
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case("Bip01 R Hand"))
            .and_then(|index| pose.get(index))
            .ok_or("no right hand bone")?;
        let hand = Vec3::new(hand[0][3], hand[1][3], hand[2][3]);
        let (forward, up) = held_axes(&positions(&vertices), hand).ok_or("no vertices")?;
        // Columns are the tag's axes (forward, left, up) in the model; transposed, model to tag.
        let turn = Mat3::from_cols(forward, up.cross(forward), up).transpose();
        for v in &mut vertices {
            v.position = (turn * (Vec3::from(v.position) - hand)).to_array();
            v.normal = (turn * Vec3::from(v.normal)).to_array();
        }
        let gun = !NO_MUZZLE.iter().any(|kind| name.contains(kind));
        let tag_points = positions(&vertices);
        if gun && let Some((_, hi)) = bounds_of(tag_points.iter().copied()) {
            let tip: Vec<Vec3> = tag_points
                .into_iter()
                .filter(|p| p.x > hi.x - 1.5)
                .collect();
            let middle = tip.iter().sum::<Vec3>() / tip.len().max(1) as f32;
            muzzle = Some(Vec3::new(hi.x, middle.y, middle.z));
        }
    }
    settle(&mut vertices, placement);
    let images: Vec<Handle<Image>> = studio
        .textures
        .iter()
        .map(|t| images.add(image(t.width, t.height, t.rgba.clone())))
        .collect();
    let draws = studio
        .meshes
        .iter()
        .filter_map(|mesh| {
            let flags = studio.textures.get(mesh.texture)?.flags;
            (flags & mdl_goldsrc::TEXTURE_ADDITIVE == 0).then_some(())?;
            Some(CsViewmodelDraw {
                image: images.get(mesh.texture)?.clone(),
                first_vertex: mesh.first_vertex as u32,
                vertex_count: mesh.vertex_count as u32,
                maps: None,
                shading: if flags & mdl_goldsrc::TEXTURE_FULLBRIGHT != 0 {
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

/// A world model read off the main thread, before its textures go into `Assets`.
enum Decoded {
    Source(mdl_source::LoadedModel),
    GoldSrc(mdl_goldsrc::StudioModel),
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
    /// Models being read off the main thread, by the same key.
    decoding: HashMap<String, Task<Result<Decoded, String>>>,
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
    /// The world model `name`. One not read yet is read off the main thread (reading it there
    /// stopped the frame for a few milliseconds a model) and is `None` meanwhile.
    fn get(
        &mut self,
        name: &str,
        placement: Placement,
        images: &mut Assets<Image>,
    ) -> Option<WorldModel> {
        let key = match placement {
            Placement::Hand => name.to_owned(),
            Placement::Floor => format!("{name} (floor)"),
            Placement::Flat => format!("{name} (dropped)"),
        };
        if let Some(model) = self.models.get(&key) {
            return model.clone();
        }
        let task = match self.decoding.remove(&key) {
            Some(task) if task.is_finished() => task,
            Some(task) => {
                self.decoding.insert(key, task);
                return None;
            }
            None => {
                let path = model_path(name);
                let task = if let Some(pack) = css_pack() {
                    AsyncComputeTaskPool::get().spawn(async move {
                        // The knife's and grenades' hand frame reads a CS:S player model once.
                        let _ = hand_to_weapon_bone(pack);
                        mdl_source::load_model(pack, &path).map(Decoded::Source)
                    })
                } else {
                    let dirs = goldsrc_dirs()?;
                    AsyncComputeTaskPool::get().spawn(async move {
                        let bytes = dirs.read(&path).ok_or_else(|| format!("no {path}"))?;
                        mdl_goldsrc::StudioModel::parse(&bytes)
                            .map(Decoded::GoldSrc)
                            .map_err(|e| e.to_string())
                    })
                };
                self.decoding.insert(key, task);
                return None;
            }
        };
        let built = block_on(task).and_then(|decoded| match decoded {
            Decoded::Source(loaded) => {
                let pack = css_pack().ok_or_else(|| "no CS:S pack".to_owned())?;
                build_world_model(pack, loaded, placement, images)
            }
            Decoded::GoldSrc(studio) => build_goldsrc_world_model(name, &studio, placement, images),
        });
        let model = match built {
            Ok((model, muzzle)) => {
                diag::info!(
                    World,
                    "cs world model {key}: {} triangles",
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
                diag::warn!(World, "cs world model {key}: {error}");
                None
            }
        };
        self.models.insert(key, model.clone());
        model
    }
}

/// Places a CS model in every posed hand holding a CS weapon, on every dropped CS weapon, and
/// where the bomb lies. The light comes later (`render_frontend`'s CS lighting reads the map's light grid
/// at each one).
#[allow(clippy::too_many_arguments)]
pub fn update_cs_world_models(
    tags: Res<CsHeldWeaponTags>,
    weapons: Option<Res<PreparedWeapons>>,
    presented: Res<net::PresentedSnapshot>,
    cg_clock: Option<Res<net::FrameClock>>,
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
        let Some(model) = models.get(bomb_model(planted), Placement::Floor, &mut images) else {
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
        let Some(name) = model_name(&weapons.0, tag.weapon, Placement::Hand) else {
            continue;
        };
        let Some(model) = models.get(&name, Placement::Hand, &mut images) else {
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
    // Dropped CS weapons, lying where the item is, turned to its yaw.
    let Some(snapshot) = presented.snapshot() else {
        return;
    };
    let at_time = super::item::item_time(cg_clock.as_deref(), snapshot);
    for es in &snapshot.meta.entities {
        if es.e_type != entity_iw4::ET_ITEM || es.e_flags & super::item::EF_NODRAW != 0 {
            continue;
        }
        let Some(name) = u32::try_from(es.index)
            .ok()
            .filter(|weapon| shows_cs_dropped_model(&weapons.0, *weapon))
            .and_then(|weapon| model_name(&weapons.0, weapon, Placement::Flat))
        else {
            continue;
        };
        let Some(model) = models.get(&name, Placement::Flat, &mut images) else {
            continue;
        };
        let (origin, angles) = super::item::item_place(es, at_time);
        let world_from_model = Mat4::from_rotation_translation(
            Quat::from_rotation_z(angles[1].to_radians()),
            Vec3::from(origin),
        );
        frame.instances.push(CsWorldModelInstance {
            model: model.gpu,
            world_from_model: world_from_model.to_cols_array_2d(),
            ambient: [0.6; 3],
            sun_dir: [0.0; 3],
            sun: [0.0; 3],
        });
    }
}
