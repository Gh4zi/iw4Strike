//! Source engine assets as Counter-Strike: Source ships them, read at runtime from the user's own
//! install: VPK archives, VTF textures, VMT materials, studio models and sound scripts.

pub mod kv;
pub mod studio;
pub mod vpk;
pub mod vtf;

pub use studio::{Animation, Attachment, BodyPart, BoneFrame, EVENT_CLIENT_SOUND, StudioModel};
pub use vpk::Vpk;

/// A model material resolved to its base texture.
#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub texture: Option<vtf::Image>,
    /// `$alphatest` or `$translucent`: alpha cuts the surface.
    pub alpha_test: bool,
    /// `$additive`.
    pub additive: bool,
    /// `$selfillum`: not lit.
    pub fullbright: bool,
}

/// A viewmodel and its materials, loaded from the pack.
#[derive(Clone, Debug)]
pub struct LoadedModel {
    pub model: StudioModel,
    pub materials: Vec<Material>,
}

fn flag(shader: &kv::Value, key: &str) -> bool {
    shader.text(key).is_some_and(|v| v.trim() != "0")
}

/// Resolve `name` through the model's material directories to a material and its base texture.
#[must_use]
pub fn load_material(vpk: &Vpk, dirs: &[String], name: &str) -> Material {
    let mut material = Material {
        name: name.to_owned(),
        texture: None,
        alpha_test: false,
        additive: false,
        fullbright: false,
    };
    let candidates = dirs
        .iter()
        .map(|dir| format!("materials/{}{name}.vmt", dir.replace('\\', "/")))
        .chain(std::iter::once(format!("materials/{name}.vmt")));
    for path in candidates {
        let Some(bytes) = vpk.read(&path) else {
            continue;
        };
        let root = kv::parse(&String::from_utf8_lossy(&bytes));
        let Some((_, shader)) = root.entries().next() else {
            continue;
        };
        material.alpha_test = flag(shader, "$alphatest") || flag(shader, "$translucent");
        material.additive = flag(shader, "$additive");
        material.fullbright = flag(shader, "$selfillum") && !flag(shader, "$basealphaenvmapmask");
        if let Some(base) = shader.text("$basetexture") {
            let tex = format!(
                "materials/{}.vtf",
                base.replace('\\', "/").trim_end_matches(".vtf")
            );
            material.texture = vpk.read(&tex).and_then(|b| vtf::decode(&b).ok());
        }
        break;
    }
    material
}

/// Load `path` (`models/weapons/v_rif_ak47.mdl`) with its `.vvd`, `.dx90.vtx` and materials.
pub fn load_model(vpk: &Vpk, path: &str) -> Result<LoadedModel, String> {
    let stem = path.trim_end_matches(".mdl");
    let read = |p: String| vpk.read(&p).ok_or_else(|| format!("{p} not in the pack"));
    let mdl = read(format!("{stem}.mdl"))?;
    let vvd = read(format!("{stem}.vvd"))?;
    let vtx = read(format!("{stem}.dx90.vtx")).or_else(|_| read(format!("{stem}.dx80.vtx")))?;
    let model = StudioModel::parse(&mdl, &vvd, &vtx)?;
    let materials = model
        .materials
        .iter()
        .map(|name| load_material(vpk, &model.material_dirs, name))
        .collect();
    Ok(LoadedModel { model, materials })
}

/// Sound script entries (`Weapon_AK47.Single`) → their wave paths under `sound/`.
#[derive(Clone, Debug, Default)]
pub struct SoundScripts {
    entries: std::collections::HashMap<String, Vec<String>>,
}

impl SoundScripts {
    /// Read every `scripts/game_sounds*.txt` in the pack.
    #[must_use]
    pub fn load(vpk: &Vpk) -> Self {
        let mut scripts = Self::default();
        let files: Vec<String> = vpk
            .paths()
            .filter(|p| p.starts_with("scripts/game_sounds") && p.ends_with(".txt"))
            .map(str::to_owned)
            .collect();
        for file in files {
            if let Some(bytes) = vpk.read(&file) {
                scripts.add(&String::from_utf8_lossy(&bytes));
            }
        }
        scripts
    }

    pub fn add(&mut self, text: &str) {
        for (name, entry) in kv::parse(text).entries() {
            let mut waves: Vec<String> = Vec::new();
            if let Some(w) = entry.text("wave") {
                waves.push(w.to_owned());
            }
            if let Some(rnd) = entry.block("rndwave") {
                waves.extend(rnd.entries().filter_map(|(_, v)| match v {
                    kv::Value::Text(t) => Some(t.clone()),
                    kv::Value::Block(_) => None,
                }));
            }
            let waves: Vec<String> = waves
                .into_iter()
                .map(|w| {
                    // Leading characters are mixing hints (`)`, `^`, `*`, `#`...).
                    w.trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
                        .replace('\\', "/")
                        .to_ascii_lowercase()
                })
                .filter(|w| !w.is_empty())
                .collect();
            if !waves.is_empty() {
                self.entries.insert(name.to_ascii_lowercase(), waves);
            }
        }
    }

    /// Wave paths (relative to `sound/`) of `entry`.
    #[must_use]
    pub fn waves(&self, entry: &str) -> &[String] {
        self.entries
            .get(&entry.to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loads the CS:S viewmodels from a local install when `IW4L_CSS` names its
    /// `cstrike_pak_dir.vpk`; skipped otherwise.
    #[test]
    fn retail_css_viewmodels_load_when_installed() {
        let Some(dir) = std::env::var_os("IW4L_CSS") else {
            return;
        };
        let vpk = Vpk::open(std::path::Path::new(&dir)).expect("open vpk");
        let scripts = SoundScripts::load(&vpk);
        println!("sound scripts: {} entries", scripts.names().count());
        for name in [
            "v_rif_ak47",
            "v_rif_m4a1",
            "v_snip_awp",
            "v_pist_deagle",
            "v_pist_usp",
            "v_pist_glock18",
            "v_knife_t",
            "v_eq_fraggrenade",
        ] {
            let loaded = load_model(&vpk, &format!("models/weapons/{name}.mdl"))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let m = &loaded.model;
            assert!(!m.meshes.is_empty(), "{name}");
            let mut pose = Vec::new();
            for (i, s) in m.sequences.iter().enumerate() {
                m.pose(i, s.duration() * 0.5, &mut pose);
                assert!(
                    pose.iter().flatten().flatten().all(|v| v.is_finite()),
                    "{name} {}",
                    s.label
                );
            }
            let seqs: Vec<_> = m
                .sequences
                .iter()
                .map(|s| format!("{}[{}]{}f@{}", s.label, s.activity, s.num_frames, s.fps))
                .collect();
            let mats: Vec<_> = loaded
                .materials
                .iter()
                .map(|mat| {
                    format!(
                        "{}={}",
                        mat.name,
                        mat.texture
                            .as_ref()
                            .map_or("MISSING".to_owned(), |t| format!(
                                "{}x{}",
                                t.width, t.height
                            ))
                    )
                })
                .collect();
            println!(
                "{name}: {} bones, {} tris, mats [{}]\n  seqs {}",
                m.bones.len(),
                m.vertices.len() / 3,
                mats.join(" "),
                seqs.join(" ")
            );
            for s in &m.sequences {
                for e in &s.events {
                    if e.event == EVENT_CLIENT_SOUND {
                        println!(
                            "  {} @{:.2} {} -> {:?}",
                            s.label,
                            e.cycle,
                            e.options,
                            scripts.waves(&e.options)
                        );
                    }
                }
            }
        }
    }
}
