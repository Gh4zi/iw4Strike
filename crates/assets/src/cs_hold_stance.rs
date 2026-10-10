//! Reads Counter-Strike 2's third-person knife and grenade stances (`idle_knife`, `idle_grenade`
//! on CS2's character skeleton) from the player's CS2 install and hands them to
//! [`xmodel_runtime::install_hold_stances`], once per process, off the main thread. Without CS2
//! nothing is installed and bodies keep MW2's own poses.

use std::sync::Once;

use glam::Vec3;
use xmodel_runtime::{ArmJoints, HoldStance};

const KNIFE_IDLE: &str = "animation/anims/world/knife/_default_knife/idle_knife.vnmclip";
const GRENADE_IDLE: &str = "animation/anims/world/grenade/_default_grenade/idle_grenade.vnmclip";

/// CS2 character joints: lower spine, neck, clavicles, and per arm (left, right) the upper arm,
/// forearm, hand, middle finger and thumb.
const TORSO: [&str; 4] = ["spine_0", "neck_0", "clavicle_L", "clavicle_R"];
const ARMS: [[&str; 5]; 2] = [
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
];

/// Start reading the stances (does nothing after the first call).
pub fn install() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("cs hold stances".into())
            .spawn(|| {
                let Some(pak) = asset_transport::find_cs2_pak() else {
                    return;
                };
                let vpk = match mdl_source::Vpk::open(&pak) {
                    Ok(vpk) => vpk,
                    Err(error) => {
                        diag::warn!(World, "cs hold stances: CS2 pack: {error}");
                        return;
                    }
                };
                let knife = stance(&vpk, KNIFE_IDLE);
                let grenade = stance(&vpk, GRENADE_IDLE).or(knife);
                diag::info!(
                    World,
                    "cs hold stances: knife {}, grenade {} (from CS2)",
                    if knife.is_some() { "read" } else { "missing" },
                    if grenade.is_some() { "read" } else { "missing" }
                );
                xmodel_runtime::install_hold_stances(knife, grenade);
            });
        if let Err(error) = spawned {
            diag::warn!(World, "cs hold stances: no thread: {error}");
        }
    });
}

/// The stance of a CS2 character clip's first frame. Additive clips have a full-pose twin
/// (`…vnmclip+non_additive.vnmclip`), which is read when it exists.
fn stance(vpk: &mdl_source::Vpk, clip: &str) -> Option<HoldStance> {
    let read = |path: &str| vpk.read(&format!("{path}_c"));
    let bytes = read(&format!("{clip}+non_additive.vnmclip")).or_else(|| read(clip))?;
    let clip = mdl_source2::anim::load_clip(&bytes).ok()?;
    let skeleton = mdl_source2::anim::load_skeleton(&read(&clip.skeleton)?).ok()?;
    let world = mdl_source2::pose::world_matrices(
        &skeleton,
        &clip.sample(0.0),
        mdl_source2::pose::Mat3x4::IDENTITY,
    );
    let joint = |name: &str| -> Option<Vec3> {
        let m = world.get(skeleton.bone(name)?)?.0;
        Some(Vec3::new(m[3], m[7], m[11]))
    };
    let arm = |names: &[&str; 5]| -> Option<ArmJoints> {
        Some([
            joint(names[0])?,
            joint(names[1])?,
            joint(names[2])?,
            joint(names[3])?,
            joint(names[4])?,
        ])
    };
    HoldStance::from_joints(
        joint(TORSO[0])?,
        joint(TORSO[1])?,
        [joint(TORSO[2])?, joint(TORSO[3])?],
        [arm(&ARMS[0])?, arm(&ARMS[1])?],
    )
}
