//! Reads a Counter-Strike game's third-person knife and grenade stances and moves (holding, the
//! slashes, the stab, the throw) from the player's own install and hands them to
//! [`xmodel_runtime::install_hold_clips`], once per process, off the main thread. The game is
//! picked as the viewmodels pick theirs: Counter-Strike 2 (its `idle_knife`, `frontswing_knife`,
//! … on its character skeleton) when it's on, else Counter-Strike: Source (`cs_player_shared`'s
//! `Idle_Upper_KNIFE`, `Idle_Shoot_KNIFE`, …), else Condition Zero or CS 1.6 (a player model's
//! `ref_aim_knife`, `ref_shoot_knife`, …). Without any, bodies keep MW2's own poses.

use std::sync::Once;

use glam::Vec3;
use xmodel_runtime::{ArmJoints, HoldAction, HoldClip, HoldStance};

/// Frames a move is read at.
const FRAMES_PER_SECOND: f32 = 30.0;

/// A source game's joints: the torso's lower spine, neck and clavicles (left, right); per arm
/// (left, right) the upper arm, forearm, hand, middle finger and thumb; and the held weapon's butt
/// and tip when its skeleton has them.
struct Joints {
    torso: [&'static str; 4],
    arms: [[&'static str; 5]; 2],
    weapon: Option<[&'static str; 2]>,
}

const CS2_JOINTS: Joints = Joints {
    torso: ["spine_0", "neck_0", "clavicle_L", "clavicle_R"],
    arms: [
        [
            "arm_upper_L",
            "arm_lower_L",
            "hand_L",
            "finger_middle_0_L",
            "finger_thumb_0_L",
        ],
        [
            "arm_upper_R",
            "arm_lower_R",
            "hand_R",
            "finger_middle_0_R",
            "finger_thumb_0_R",
        ],
    ],
    weapon: Some(["wpnEnd", "wpnTip"]),
};

const CSS_JOINTS: Joints = Joints {
    torso: [
        "ValveBiped.Bip01_Spine",
        "ValveBiped.Bip01_Neck1",
        "ValveBiped.Bip01_L_Clavicle",
        "ValveBiped.Bip01_R_Clavicle",
    ],
    arms: [
        [
            "ValveBiped.Bip01_L_UpperArm",
            "ValveBiped.Bip01_L_Forearm",
            "ValveBiped.Bip01_L_Hand",
            "ValveBiped.Bip01_L_Finger2",
            "ValveBiped.Bip01_L_Finger0",
        ],
        [
            "ValveBiped.Bip01_R_UpperArm",
            "ValveBiped.Bip01_R_Forearm",
            "ValveBiped.Bip01_R_Hand",
            "ValveBiped.Bip01_R_Finger2",
            "ValveBiped.Bip01_R_Finger0",
        ],
    ],
    weapon: None,
};

const GOLDSRC_JOINTS: Joints = Joints {
    torso: [
        "Bip01 Spine",
        "Bip01 Neck",
        "Bip01 L Clavicle",
        "Bip01 R Clavicle",
    ],
    arms: [
        [
            "Bip01 L UpperArm",
            "Bip01 L Forearm",
            "Bip01 L Hand",
            "Bip01 L Finger1",
            "Bip01 L Finger0",
        ],
        [
            "Bip01 R UpperArm",
            "Bip01 R Forearm",
            "Bip01 R Hand",
            "Bip01 R Finger1",
            "Bip01 R Finger0",
        ],
    ],
    weapon: None,
};

/// CS2's world clip for each [`HoldAction`], in its order.
const CS2_CLIPS: [&str; HoldAction::COUNT] = [
    "animation/anims/world/knife/_default_knife/idle_knife.vnmclip",
    "animation/anims/world/knife/_default_knife/frontswing_knife.vnmclip",
    "animation/anims/world/knife/_default_knife/frontswing_b_knife.vnmclip",
    "animation/anims/world/knife/_default_knife/frontstab_knife.vnmclip",
    "animation/anims/world/grenade/_default_grenade/idle_grenade.vnmclip",
    "animation/anims/world/grenade/_default_grenade/throw_overhand_grenade.vnmclip",
];

/// CS:S's sequences for each [`HoldAction`]: the upper-body pose and, for a move, the sequence
/// played over it (a delta one is added onto the pose).
const CSS_SEQUENCES: [(&str, Option<&str>); HoldAction::COUNT] = [
    ("Idle_Upper_KNIFE", None),
    ("Idle_Upper_KNIFE", Some("Idle_Shoot_KNIFE")),
    ("Idle_Upper_KNIFE", Some("Idle_Shoot_KNIFE")),
    ("Idle_Upper_KNIFE", Some("Idle_Shoot_KNIFE")),
    ("Idle_Upper_GREN", None),
    ("Idle_Upper_GREN", Some("Idle_Shoot_GREN1")),
];

/// CS 1.6's (and Condition Zero's) player model sequences for each [`HoldAction`].
const GOLDSRC_SEQUENCES: [&str; HoldAction::COUNT] = [
    "ref_aim_knife",
    "ref_shoot_knife",
    "ref_shoot_knife",
    "ref_shoot_knife",
    "ref_aim_grenade",
    "ref_shoot_grenade",
];

/// A player model CS 1.6 and Condition Zero both ship.
const GOLDSRC_PLAYER: &str = "models/player/gign/gign.mdl";
/// Their world knife, which rides on the player's skeleton (it carries a copy down to the right
/// hand), and that hand.
const GOLDSRC_KNIFE: &str = "models/p_knife.mdl";
const GOLDSRC_HAND: &str = "Bip01 R Hand";

/// Start reading the stances (does nothing after the first call).
pub fn install() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("cs hold stances".into())
            .spawn(|| {
                let Some((game, clips)) = read_cs2()
                    .map(|clips| ("CS2", clips))
                    .or_else(|| read_css().map(|clips| ("CS:S", clips)))
                    .or_else(read_goldsrc)
                else {
                    return;
                };
                diag::info!(
                    World,
                    "cs hold stances: {} of {} moves read (from {game})",
                    clips.iter().flatten().count(),
                    clips.len()
                );
                for (i, clip) in clips.iter().enumerate() {
                    let Some(clip) = clip else { continue };
                    let yaws: Vec<String> = clip
                        .frames
                        .iter()
                        .step_by((clip.frames.len() / 8).max(1))
                        .map(|f| format!("{:.0}", f.yaw.to_degrees()))
                        .collect();
                    diag::debug!(
                        World,
                        "cs hold stances: move {i} torso yaw {}",
                        yaws.join(" ")
                    );
                }
                xmodel_runtime::install_hold_clips(clips);
            });
        if let Err(error) = spawned {
            diag::warn!(World, "cs hold stances: no thread: {error}");
        }
    });
}

/// Every move's clip, when the holding stances at least were read.
fn complete(
    clips: [Option<HoldClip>; HoldAction::COUNT],
) -> Option<[Option<HoldClip>; HoldAction::COUNT]> {
    let idle = |action: HoldAction| clips[action.index()].is_some();
    (idle(HoldAction::KnifeIdle) && idle(HoldAction::GrenadeIdle)).then_some(clips)
}

/// A clip from `seconds` long, read at [`FRAMES_PER_SECOND`].
fn sample_clip(
    seconds: f32,
    mut stance: impl FnMut(f32) -> Option<HoldStance>,
) -> Option<HoldClip> {
    let count = ((seconds * FRAMES_PER_SECOND).round() as usize).max(1) + 1;
    let frames = (0..count)
        .map(|i| stance(seconds * i as f32 / (count - 1) as f32))
        .collect::<Option<Vec<_>>>()?;
    Some(HoldClip { frames })
}

/// CS:S's standing lower-body sequence, under the upper-body poses.
const CSS_LOWER_BODY: &str = "Idle_lower";

/// CS:S's world knife, and the hand bone it rides on (its bones merge onto the player's).
const CSS_KNIFE: &str = "models/weapons/w_knife_t.mdl";
const CSS_HAND: &str = "ValveBiped.Bip01_R_Hand";

/// The stance of a posed skeleton, its joints found by name.
fn stance_from(joints: &Joints, joint: impl Fn(&str) -> Option<Vec3>) -> Option<HoldStance> {
    let weapon = joints
        .weapon
        .and_then(|[butt, tip]| joint(butt).zip(joint(tip)));
    stance_with_weapon(joints, weapon, joint)
}

/// The stance of a posed skeleton, with where its weapon points (butt, tip) found by the caller.
fn stance_with_weapon(
    joints: &Joints,
    weapon: Option<(Vec3, Vec3)>,
    joint: impl Fn(&str) -> Option<Vec3>,
) -> Option<HoldStance> {
    let arm = |names: &[&str; 5]| -> Option<ArmJoints> {
        Some([
            joint(names[0])?,
            joint(names[1])?,
            joint(names[2])?,
            joint(names[3])?,
            joint(names[4])?,
        ])
    };
    let torso = [
        joint(joints.torso[0])?,
        joint(joints.torso[1])?,
        joint(joints.torso[2])?,
        joint(joints.torso[3])?,
    ];
    HoldStance::from_joints(
        torso,
        [arm(&joints.arms[0])?, arm(&joints.arms[1])?],
        weapon,
    )
}

fn read_cs2() -> Option<[Option<HoldClip>; HoldAction::COUNT]> {
    let pak = asset_transport::find_cs2_pak()?;
    let vpk = mdl_source::Vpk::open(&pak)
        .map_err(|error| diag::warn!(World, "cs hold stances: CS2 pack: {error}"))
        .ok()?;
    complete(CS2_CLIPS.map(|path| cs2_clip(&vpk, path)))
}

/// A CS2 character clip. Additive clips have a full-pose twin (`…vnmclip+non_additive.vnmclip`),
/// which is read when it exists.
fn cs2_clip(vpk: &mdl_source::Vpk, path: &str) -> Option<HoldClip> {
    let read = |path: &str| vpk.read(&format!("{path}_c"));
    let bytes = read(&format!("{path}+non_additive.vnmclip")).or_else(|| read(path))?;
    let clip = mdl_source2::anim::load_clip(&bytes).ok()?;
    let skeleton = mdl_source2::anim::load_skeleton(&read(&clip.skeleton)?).ok()?;
    sample_clip(clip.duration, |seconds| {
        let world = mdl_source2::pose::world_matrices(
            &skeleton,
            &clip.sample(seconds),
            mdl_source2::pose::Mat3x4::IDENTITY,
        );
        stance_from(&CS2_JOINTS, |name| {
            let m = world.get(skeleton.bone(name)?)?.0;
            Some(Vec3::new(m[3], m[7], m[11]))
        })
    })
}

fn read_css() -> Option<[Option<HoldClip>; HoldAction::COUNT]> {
    let pak = asset_transport::find_css_pak()?;
    let vpk = mdl_source::Vpk::open(&pak).ok()?;
    let model =
        mdl_source::StudioModel::parse(&vpk.read("models/player/cs_player_shared.mdl")?, &[], &[])
            .ok()?;
    let blade = css_blade_in_hand(&vpk);
    complete(CSS_SEQUENCES.map(|(pose, layer)| css_clip(&model, pose, layer, blade)))
}

/// Where CS:S's world knife points its blade (its model's +x, as the world models are laid out)
/// in the right hand's frame, which it rides on.
fn css_blade_in_hand(vpk: &mdl_source::Vpk) -> Option<Vec3> {
    let knife = mdl_source::StudioModel::parse(&vpk.read(CSS_KNIFE)?, &[], &[]).ok()?;
    let hand = knife.bones.iter().find(|b| b.name == CSS_HAND)?;
    let m = hand.pose_to_bone;
    Vec3::new(m[0][0], m[1][0], m[2][0]).try_normalize()
}

/// CS:S's upper-body `pose` with `layer` played over it, the knife's `blade` (in the right
/// hand's frame) giving where the weapon points.
fn css_clip(
    model: &mdl_source::StudioModel,
    pose: &str,
    layer: Option<&str>,
    blade: Option<Vec3>,
) -> Option<HoldClip> {
    let seconds = match layer {
        Some(label) => {
            let sequence = model.sequences.iter().find(|s| s.label == label)?;
            sequence.num_frames.saturating_sub(1) as f32 / sequence.fps.max(1.0)
        }
        None => 0.0,
    };
    sample_clip(seconds, |at| {
        let locals = css_pose(model, pose, layer, at)?;
        let world = model.world_from_locals(&locals);
        let joint = |name: &str| {
            let bone = model.bones.iter().position(|b| b.name == name)?;
            world.get(bone).copied()
        };
        let weapon = joint(CSS_HAND).zip(blade).map(|(m, blade)| {
            let origin = Vec3::new(m[0][3], m[1][3], m[2][3]);
            let row = |r: usize| Vec3::new(m[r][0], m[r][1], m[r][2]).dot(blade);
            (origin, origin + Vec3::new(row(0), row(1), row(2)))
        });
        stance_with_weapon(&CSS_JOINTS, weapon, |name| {
            joint(name).map(|m| Vec3::new(m[0][3], m[1][3], m[2][3]))
        })
    })
}

/// CS:S's standing body holding `pose` (an upper-body sequence) with `layer` (a move) `at`
/// seconds in, as its local bone transforms: the lower-body sequence (which turns the body to
/// face +x), then the upper-body pose and the move, each with the sequences it carries (aim,
/// hand position), each on the bones its weights give it.
#[must_use]
pub fn css_pose(
    model: &mdl_source::StudioModel,
    pose: &str,
    layer: Option<&str>,
    at: f32,
) -> Option<Vec<mdl_source::BoneFrame>> {
    let find = |label: &str| model.sequences.iter().position(|s| s.label == label);
    let mut locals = model
        .animations
        .get(*model.sequences.get(find(CSS_LOWER_BODY)?)?.blends.first()?)?
        .frame(0)
        .to_vec();
    css_play(model, &mut locals, find(pose)?, 0.0, 0);
    if let Some(layer) = layer {
        css_play(model, &mut locals, find(layer)?, at, 0);
    }
    Some(locals)
}

/// Lay sequence `index` `at` seconds in over `locals`, then the sequences it carries. A blend
/// grid (an aim matrix) plays its middle: looking straight ahead. `depth` counts how many
/// sequences carry this one.
fn css_play(
    model: &mdl_source::StudioModel,
    locals: &mut [mdl_source::BoneFrame],
    index: usize,
    at: f32,
    depth: usize,
) {
    let Some(sequence) = model.sequences.get(index) else {
        return;
    };
    let middle = sequence.blends.get(sequence.blends.len() / 2);
    if let Some(animation) = middle.and_then(|&a| model.animations.get(a)) {
        let frame =
            ((at * animation.fps).round() as usize).min(animation.num_frames.saturating_sub(1));
        css_lay_over(locals, animation, frame, &sequence.weights);
    }
    if depth < 2 {
        for &carried in &sequence.autolayers {
            css_play(model, locals, carried, 0.0, depth + 1);
        }
    }
}

/// Lay `animation`'s `frame` over `locals` by each bone's weight: blended toward its pose, or a
/// delta animation's offsets added on as far as the weight says.
fn css_lay_over(
    locals: &mut [mdl_source::BoneFrame],
    animation: &mdl_source::Animation,
    frame: usize,
    weights: &[f32],
) {
    use glam::{Quat, Vec3};
    for ((local, over), &weight) in locals.iter_mut().zip(animation.frame(frame)).zip(weights) {
        if weight <= 0.0 {
            continue;
        }
        let (rotation, position) = (Quat::from_array(local.rotation), Vec3::from(local.position));
        let (over_rotation, over_position) =
            (Quat::from_array(over.rotation), Vec3::from(over.position));
        let (rotation, position) = if animation.delta {
            (
                rotation * Quat::IDENTITY.slerp(over_rotation, weight),
                position + over_position * weight,
            )
        } else {
            (
                rotation.slerp(over_rotation, weight),
                position.lerp(over_position, weight),
            )
        };
        local.rotation = rotation.normalize().to_array();
        local.position = position.to_array();
    }
}

/// Condition Zero's player model when it's installed, else CS 1.6's; named for the log.
fn read_goldsrc() -> Option<(&'static str, [Option<HoldClip>; HoldAction::COUNT])> {
    let dirs = asset_transport::find_goldsrc()?;
    let blade = dirs
        .file(GOLDSRC_KNIFE)
        .and_then(|knife| goldsrc_blade_in_hand(&knife));
    let path = dirs.file(GOLDSRC_PLAYER)?;
    let game = if path
        .components()
        .any(|part| part.as_os_str().eq_ignore_ascii_case("czero"))
    {
        "CZ"
    } else {
        "CS 1.6"
    };
    let model = mdl_goldsrc::StudioModel::parse(&std::fs::read(path).ok()?).ok()?;
    Some((
        game,
        complete(GOLDSRC_SEQUENCES.map(|label| goldsrc_clip(&model, label, blade)))?,
    ))
}

/// Where the GoldSrc world knife at `path` points its blade in the right hand's frame: toward
/// its point farthest from the hand.
fn goldsrc_blade_in_hand(path: &std::path::Path) -> Option<Vec3> {
    let knife = mdl_goldsrc::StudioModel::parse(&std::fs::read(path).ok()?).ok()?;
    let mut world = Vec::new();
    knife.pose(0, 0.0, &mut world);
    let hand = world.get(knife.bones.iter().position(|b| b.name == GOLDSRC_HAND)?)?;
    let origin = Vec3::new(hand[0][3], hand[1][3], hand[2][3]);
    // Into the hand's frame: by the transpose of its rotation.
    let column = |c: usize| Vec3::new(hand[0][c], hand[1][c], hand[2][c]);
    knife
        .vertices
        .iter()
        .filter_map(|v| {
            let at = mdl_goldsrc::transform_point(world.get(usize::from(v.bone))?, v.position);
            let d = Vec3::from(at) - origin;
            Some(Vec3::new(
                column(0).dot(d),
                column(1).dot(d),
                column(2).dot(d),
            ))
        })
        .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))?
        .try_normalize()
}

/// A CS 1.6 player model sequence, the knife's `blade` (in the right hand's frame) giving where
/// the weapon points.
fn goldsrc_clip(
    model: &mdl_goldsrc::StudioModel,
    label: &str,
    blade: Option<Vec3>,
) -> Option<HoldClip> {
    let sequence = model.sequence_index(label)?;
    let seconds = model.sequences[sequence].duration();
    let mut world = Vec::new();
    sample_clip(seconds, |at| {
        model.pose(sequence, at, &mut world);
        let joint = |name: &str| world.get(model.bones.iter().position(|b| b.name == name)?);
        let weapon = joint(GOLDSRC_HAND).zip(blade).map(|(m, blade)| {
            let origin = Vec3::new(m[0][3], m[1][3], m[2][3]);
            let row = |r: usize| Vec3::new(m[r][0], m[r][1], m[r][2]).dot(blade);
            (origin, origin + Vec3::new(row(0), row(1), row(2)))
        });
        stance_with_weapon(&GOLDSRC_JOINTS, weapon, |name| {
            joint(name).map(|m| Vec3::new(m[0][3], m[1][3], m[2][3]))
        })
    })
}
