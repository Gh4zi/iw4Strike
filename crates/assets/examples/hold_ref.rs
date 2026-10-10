//! `hold_ref <out_dir> [cstrike_dir]`: stick figures of the Counter-Strike games' own skeletons through the
//! knife and grenade moves third-person stances are read from (CS2, CS:S, CS 1.6, whichever are
//! installed; CS 1.6 from `cstrike_dir` when CS:S is there too), one PNG per game: a row per move, its frames across, each seen from the front and
//! from the right. The right arm is red, the left blue, the weapon green.

use glam::Vec3;

const CELL: usize = 220;
const SCALE: f32 = 2.4;
const FRACTIONS: [f32; 5] = [0.0, 0.2, 0.4, 0.6, 0.8];

/// A posed skeleton: each bone's model-space position, its parent and its name.
type Posed = Vec<(Vec3, Option<usize>, String)>;

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: hold_ref <out_dir>"));
    std::fs::create_dir_all(&out).expect("out dir");
    if let Some(rows) = cs2() {
        draw(&rows, &out.join("ref_cs2.png"));
    }
    if let Some(rows) = css() {
        draw(&rows, &out.join("ref_css.png"));
    }
    if let Some(rows) = goldsrc(std::env::args().nth(2)) {
        draw(&rows, &out.join("ref_cs16.png"));
    }
}

fn cs2() -> Option<Vec<Vec<Posed>>> {
    let vpk = mdl_source::Vpk::open(&asset_transport::find_cs2_pak()?).ok()?;
    let read = |path: &str| vpk.read(&format!("{path}_c"));
    let clips = [
        "knife/_default_knife/idle_knife",
        "knife/_default_knife/frontswing_knife",
        "knife/_default_knife/frontswing_b_knife",
        "knife/_default_knife/frontstab_knife",
        "grenade/_default_grenade/idle_grenade",
        "grenade/_default_grenade/throw_overhand_grenade",
    ];
    Some(
        clips
            .iter()
            .map(|clip| {
                let path = format!("animation/anims/world/{clip}.vnmclip");
                let bytes = read(&format!("{path}+non_additive.vnmclip"))
                    .or_else(|| read(&path))
                    .expect("clip");
                let clip = mdl_source2::anim::load_clip(&bytes).expect("decode clip");
                let skeleton =
                    mdl_source2::anim::load_skeleton(&read(&clip.skeleton).expect("skeleton"))
                        .expect("decode skeleton");
                FRACTIONS
                    .iter()
                    .map(|f| {
                        let world = mdl_source2::pose::world_matrices(
                            &skeleton,
                            &clip.sample(clip.duration * f),
                            mdl_source2::pose::Mat3x4::IDENTITY,
                        );
                        world
                            .iter()
                            .enumerate()
                            .map(|(i, m)| {
                                let m = m.0;
                                (
                                    Vec3::new(m[3], m[7], m[11]),
                                    skeleton.parents[i],
                                    skeleton.bones[i].clone(),
                                )
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect(),
    )
}

fn css() -> Option<Vec<Vec<Posed>>> {
    let vpk = mdl_source::Vpk::open(&asset_transport::find_css_pak()?).ok()?;
    let model =
        mdl_source::StudioModel::parse(&vpk.read("models/player/cs_player_shared.mdl")?, &[], &[])
            .ok()?;
    let moves: [(&str, Option<&str>); 6] = [
        ("Idle_Upper_KNIFE", None),
        ("Idle_Upper_KNIFE", Some("Idle_Shoot_KNIFE")),
        ("Idle_Upper_KNIFE", Some("Idle_Shoot_KNIFE")),
        ("Idle_Upper_KNIFE", Some("Idle_Shoot_KNIFE")),
        ("Idle_Upper_GREN", None),
        ("Idle_Upper_GREN", Some("Idle_Shoot_GREN1")),
    ];
    Some(
        moves
            .iter()
            .map(|&(pose, layer)| {
                let seconds = layer.map_or(0.0, |label| {
                    let seq = model
                        .sequences
                        .iter()
                        .find(|s| s.label == label)
                        .expect(label);
                    seq.num_frames.saturating_sub(1) as f32 / seq.fps
                });
                FRACTIONS
                    .iter()
                    .map(|f| {
                        let locals =
                            assets::cs_hold_stance::css_pose(&model, pose, layer, seconds * f)
                                .expect("pose");
                        model
                            .world_from_locals(&locals)
                            .iter()
                            .zip(&model.bones)
                            .map(|(m, b)| {
                                (
                                    Vec3::new(m[0][3], m[1][3], m[2][3]),
                                    b.parent,
                                    b.name.clone(),
                                )
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect(),
    )
}

fn goldsrc(dir: Option<String>) -> Option<Vec<Vec<Posed>>> {
    let player = "models/player/gign/gign.mdl";
    let path = match dir {
        Some(dir) => std::path::Path::new(&dir).join(player),
        None => asset_transport::find_goldsrc()?.file(player)?,
    };
    let model = mdl_goldsrc::StudioModel::parse(&std::fs::read(path).ok()?).ok()?;
    let moves = [
        "ref_aim_knife",
        "ref_shoot_knife",
        "ref_shoot_knife",
        "ref_shoot_knife",
        "ref_aim_grenade",
        "ref_shoot_grenade",
    ];
    let mut world = Vec::new();
    Some(
        moves
            .iter()
            .map(|label| {
                let seq = model.sequence_index(label).expect(label);
                let seconds = model.sequences[seq].duration();
                FRACTIONS
                    .iter()
                    .map(|f| {
                        model.pose(seq, seconds * f, &mut world);
                        world
                            .iter()
                            .zip(&model.bones)
                            .map(|(m, b)| {
                                (
                                    Vec3::new(m[0][3], m[1][3], m[2][3]),
                                    b.parent,
                                    b.name.clone(),
                                )
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect(),
    )
}

fn colour(name: &str) -> Option<[u8; 3]> {
    let n = name.to_ascii_lowercase();
    if n.contains("finger")
        && !n.contains("middle_0")
        && !n.ends_with("finger2")
        && !n.ends_with("finger1")
    {
        return None;
    }
    if n.contains("wpn") || n.contains("weapon") {
        return Some([60, 230, 60]);
    }
    let left = name.ends_with("_L") || name.contains("_L_") || name.contains(" L ");
    let right = name.ends_with("_R") || name.contains("_R_") || name.contains(" R ");
    let arm = n.contains("arm") || n.contains("hand") || n.contains("finger") || n.contains("clav");
    Some(match (arm, left, right) {
        (true, true, _) => [80, 140, 255],
        (true, _, true) => [255, 70, 70],
        _ => [190, 190, 190],
    })
}

fn draw(rows: &[Vec<Posed>], path: &std::path::Path) {
    let cols = FRACTIONS.len() * 2;
    let (w, h) = (CELL * cols, CELL * rows.len());
    let mut img = vec![30u8; w * h * 3];
    // Each skeleton centred on its pelvis-ish root: the mean of its bones.
    for (r, row) in rows.iter().enumerate() {
        for (c, posed) in row.iter().enumerate() {
            let centre = posed.iter().map(|b| b.0).sum::<Vec3>() / posed.len().max(1) as f32;
            for (view, right_axis) in [(0usize, Vec3::Y), (1, Vec3::X)] {
                let (ox, oy) = ((c * 2 + view) * CELL + CELL / 2, r * CELL + CELL / 2);
                let project = |p: Vec3| {
                    let p = p - centre;
                    (
                        ox as f32 + p.dot(right_axis) * SCALE,
                        oy as f32 - p.z * SCALE,
                    )
                };
                for (pos, parent, name) in posed {
                    let Some(parent) = parent else { continue };
                    let Some(rgb) = colour(name) else { continue };
                    // Weapon bones hang off the root or the spine: drawn from their own origin.
                    let from = if rgb == [60, 230, 60] {
                        *pos
                    } else {
                        posed[*parent].0
                    };
                    let (a, b) = (project(from), project(*pos));
                    line(&mut img, w, h, a, b, rgb);
                    if rgb == [60, 230, 60] {
                        let (x, y) = project(*pos);
                        line(&mut img, w, h, (x - 2.0, y), (x + 2.0, y), rgb);
                    }
                }
                // The weapon's butt to tip, when the skeleton says.
                let find = |n: &str| posed.iter().find(|b| b.2 == n).map(|b| b.0);
                if let (Some(butt), Some(tip)) = (find("wpnEnd"), find("wpnTip")) {
                    line(&mut img, w, h, project(butt), project(tip), [255, 255, 0]);
                }
            }
            // Cell borders.
            for y in r * CELL..(r + 1) * CELL {
                let x = c * 2 * CELL;
                img[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[90, 90, 90]);
            }
        }
    }
    let file = std::fs::File::create(path).expect("png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut wr| wr.write_image_data(&img))
        .expect("write png");
}

fn line(img: &mut [u8], w: usize, h: usize, a: (f32, f32), b: (f32, f32), rgb: [u8; 3]) {
    let steps = ((b.0 - a.0).abs().max((b.1 - a.1).abs()) as usize).max(1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1)] {
            let (x, y) = (x as isize + dx, y as isize + dy);
            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                let at = (y as usize * w + x as usize) * 3;
                img[at..at + 3].copy_from_slice(&rgb);
            }
        }
    }
}
