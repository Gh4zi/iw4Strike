//! Where Panorama's files come from, all read at runtime from the CS:GO install: the layouts,
//! styles and scripts packed in `panorama/code.pbin` (a zip of stored files behind a short
//! header), the art in `pak01` (`materials/panorama/images`), the Stratum2 fonts
//! (`panorama/fonts/*.vfont`) and the English strings (`resource/csgo_english.txt`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::asset::{Assets, Handle, RenderAssetUsages};
use bevy::image::{CompressedImageFormats, Image, ImageSampler, ImageType};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::Font;

/// The packed files of `code.pbin`, by lowercase path (`panorama/layout/hud/hud.xml`).
fn read_code_pack(path: &Path) -> Option<HashMap<String, Vec<u8>>> {
    let data = std::fs::read(path).ok()?;
    let mut files = HashMap::new();
    let mut i = 0;
    while i + 30 <= data.len() {
        if data[i..i + 4] != [0x50, 0x4b, 3, 4] {
            i += 1;
            continue;
        }
        let u16_at = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        let method = u16_at(i + 8);
        let size = u32_at(i + 18) as usize;
        let name_len = usize::from(u16_at(i + 26));
        let extra_len = usize::from(u16_at(i + 28));
        let start = i + 30 + name_len + extra_len;
        let Some(name) = data.get(i + 30..i + 30 + name_len) else {
            break;
        };
        let name = String::from_utf8_lossy(name).replace('\\', "/").to_ascii_lowercase();
        if method == 0
            && let Some(bytes) = data.get(start..start + size)
            && !name.ends_with('/')
        {
            files.insert(name, bytes.to_vec());
        }
        i = start + size;
    }
    (!files.is_empty()).then_some(files)
}

/// UTF-16 (with a byte order mark) or UTF-8 text.
fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)).into_owned()
    }
}

/// A localisation file's strings: every line's first two quoted strings, key and value (escaped
/// quotes and newlines unescaped).
fn read_tokens(text: &str) -> HashMap<String, String> {
    let mut tokens = HashMap::new();
    for line in text.lines() {
        let mut quoted = Vec::with_capacity(2);
        let mut chars = line.chars();
        while quoted.len() < 2 {
            if !chars.by_ref().any(|c| c == '"') {
                break;
            }
            let mut s = String::new();
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    '\\' => match chars.next() {
                        Some('n') => s.push('\n'),
                        Some(other) => s.push(other),
                        None => {}
                    },
                    '"' => {
                        closed = true;
                        break;
                    }
                    c => s.push(c),
                }
            }
            if !closed {
                break;
            }
            quoted.push(s);
        }
        if let [key, value] = quoted.as_slice() {
            tokens.entry(key.to_ascii_lowercase()).or_insert_with(|| value.clone());
        }
    }
    tokens
}

/// CS:GO's `.vfont` (`VFONT1`): an sfnt font XORed with a key rolled from a salt stored at its
/// end. Decoded in memory only. Follows CS-Craft (github.com/FrosttysBots/CS-Craft, Apache-2.0).
#[must_use]
pub fn decode_vfont(mut bytes: Vec<u8>) -> Option<Vec<u8>> {
    let trailer = bytes.len().checked_sub(7)?;
    if &bytes[trailer + 1..] != b"VFONT1" {
        return None;
    }
    let salt_len = usize::from(bytes[trailer]);
    if salt_len == 0 {
        return None;
    }
    let end = (trailer + 1).checked_sub(salt_len)?;
    let mut key = 0xa7u8;
    for &salt in &bytes[end..trailer] {
        key ^= salt.wrapping_add(0xa7);
    }
    for b in &mut bytes[..end] {
        let encoded = *b;
        *b ^= key;
        key = encoded.wrapping_add(0xa7);
    }
    bytes.truncate(end);
    (bytes.starts_with(&[0, 1, 0, 0]) || bytes.starts_with(b"OTTO")).then_some(bytes)
}

/// The Stratum2 faces, by the names their `.vfont` files carry.
#[derive(Clone, Debug, Default)]
pub struct Fonts {
    pub regular: Handle<Font>,
    pub medium: Handle<Font>,
    pub light: Handle<Font>,
    pub bold: Handle<Font>,
    pub regular_mono: Handle<Font>,
    pub bold_mono: Handle<Font>,
}

impl Fonts {
    /// The face for a `font-family` and `font-weight` (Stratum2 for any family).
    #[must_use]
    pub fn face(&self, family: Option<&str>, weight: Option<&str>) -> Handle<Font> {
        let family = family.unwrap_or("stratum2").to_ascii_lowercase();
        let weight = weight.unwrap_or("normal");
        let bold = family.contains("bold")
            || matches!(weight, "bold" | "black" | "heavy" | "bolder")
            || weight.parse::<u32>().is_ok_and(|w| w >= 600);
        // The monodigit faces hold digits only (Panorama falls back to plain Stratum2 for the
        // rest), so a monodigit family draws with the plain face.
        if bold {
            self.bold.clone()
        } else if family.contains("light") || matches!(weight, "light" | "thin") {
            self.light.clone()
        } else if family.contains("medium") || matches!(weight, "medium" | "semi-bold") {
            self.medium.clone()
        } else {
            self.regular.clone()
        }
    }
}

/// A CS:GO install's Panorama files.
pub struct Files {
    code: HashMap<String, Vec<u8>>,
    pak: Arc<mdl_source::Vpk>,
    /// Art read ahead by `prefetch_images`, by pak path.
    prefetched: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    dir: PathBuf,
    tokens: HashMap<String, String>,
    /// Rasterised art by path and pixel size; `None` when it could not be read.
    images: HashMap<(String, u32, u32), Option<(Handle<Image>, [u32; 2])>>,
}

impl Files {
    /// The install's Panorama files; `None` without CS:GO or its packed UI.
    #[must_use]
    pub fn open() -> Option<Self> {
        let pak = asset_transport::find_csgo_pak()?;
        let dir = pak.parent()?.to_path_buf();
        let code = read_code_pack(&dir.join("panorama").join("code.pbin"))?;
        let pak = Arc::new(mdl_source::Vpk::open(&pak).ok()?);
        let tokens = std::fs::read(dir.join("resource").join("csgo_english.txt"))
            .map(|bytes| read_tokens(&decode_text(&bytes)))
            .unwrap_or_default();
        diag::info!(
            World,
            "panorama: {} packed files, {} strings from {}",
            code.len(),
            tokens.len(),
            dir.display()
        );
        Some(Self {
            code,
            pak,
            prefetched: Arc::default(),
            dir,
            tokens,
            images: HashMap::new(),
        })
    }

    /// A packed file's text by its `file://{resources}/…` or plain path.
    #[must_use]
    pub fn text(&self, path: &str) -> Option<String> {
        let path = path
            .replace("file://{resources}/", "panorama/")
            .replace('\\', "/")
            .to_ascii_lowercase();
        self.code.get(&path).map(|b| decode_text(b))
    }

    /// The English string of `#token` (without the `#`).
    #[must_use]
    pub fn token(&self, token: &str) -> Option<&str> {
        self.tokens.get(&token.to_ascii_lowercase()).map(String::as_str)
    }

    /// Reads the Stratum2 faces; `None` without them.
    pub fn fonts(&self, fonts: &mut Assets<Font>) -> Option<Fonts> {
        let dir = self.dir.join("panorama").join("fonts");
        let mut face = |name: &str| -> Option<Handle<Font>> {
            let bytes = std::fs::read(dir.join(format!("{name}.vfont"))).ok()?;
            let font = Font::from_bytes(decode_vfont(bytes)?);
            Some(fonts.add(font))
        };
        let regular = face("stratum2regular")?;
        let bold = face("stratum2bold")?;
        Some(Fonts {
            medium: face("stratum2medium").unwrap_or_else(|| regular.clone()),
            light: face("stratum2light").unwrap_or_else(|| regular.clone()),
            regular_mono: face("stratum2regular_monodigit").unwrap_or_else(|| regular.clone()),
            bold_mono: face("stratum2bold_monodigit").unwrap_or_else(|| bold.clone()),
            regular,
            bold,
        })
    }

    /// The pak path of an image source (`file://{images}/icons/ui/health.svg`).
    fn image_path(src: &str) -> String {
        let s = src.trim();
        let s = s
            .strip_prefix("file://{images}/")
            .map(|rest| format!("materials/panorama/images/{rest}"))
            .or_else(|| s.strip_prefix("s2r://").map(str::to_owned))
            .unwrap_or_else(|| s.to_owned());
        s.replace('\\', "/").to_ascii_lowercase()
    }

    /// Reads ahead, on a thread of its own, the art a match may show for the first time: every
    /// image the HUD and scoreboard layouts, styles and scripts name, every equipment icon (kill
    /// feed, buy menu) and the `extra` sources the game sets itself. A first read from pak01 is an
    /// open and a seek (5-25 ms on a hard drive), and it landed on the frame that first showed
    /// the image.
    pub fn prefetch_images(&self, extra: &[&str]) {
        let mut paths: Vec<String> = extra.iter().map(|src| Self::image_path(src)).collect();
        for (name, bytes) in &self.code {
            if !(name.contains("/hud/") || name.contains("scoreboard")) {
                continue;
            }
            let text = decode_text(bytes);
            for (at, _) in text.match_indices("file://{images}/") {
                let rest = &text[at..];
                let end = rest
                    .find(|c: char| matches!(c, '"' | '\'' | ')' | ';' | ',') || c.is_whitespace())
                    .unwrap_or(rest.len());
                paths.push(Self::image_path(&rest[..end]));
            }
        }
        paths.extend(
            self.pak
                .paths()
                .filter(|path| path.starts_with("materials/panorama/images/icons/equipment/"))
                .map(str::to_owned),
        );
        // `read_image` takes a `.png` or `.svg` for a source the layout names `.vtf`.
        let alternates: Vec<String> = paths
            .iter()
            .filter_map(|path| path.rsplit_once('.'))
            .flat_map(|(stem, _)| [format!("{stem}.png"), format!("{stem}.svg")])
            .collect();
        paths.extend(alternates);
        paths.sort_unstable();
        paths.dedup();
        let pak = Arc::clone(&self.pak);
        let store = Arc::clone(&self.prefetched);
        let spawned = std::thread::Builder::new()
            .name("panorama-prefetch".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let read = pak.read_sorted(&paths);
                let bytes: usize = read.iter().map(|(_, b)| b.len()).sum();
                let n = read.len();
                store
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .extend(read);
                diag::info!(
                    World,
                    "panorama: read ahead {n} images ({} KiB) in {:.0} ms",
                    bytes / 1024,
                    started.elapsed().as_secs_f64() * 1000.0
                );
            });
        if let Err(error) = spawned {
            diag::warn!(World, "panorama: no read-ahead thread: {error}");
        }
    }

    /// The image `src` as a texture, an SVG drawn at `size` pixels (or its own size when 0); its
    /// pixel size. `None` when it is not in the pak or cannot be read.
    pub fn image(
        &mut self,
        src: &str,
        size: [u32; 2],
        images: &mut Assets<Image>,
    ) -> Option<(Handle<Image>, [u32; 2])> {
        let path = Self::image_path(src);
        let svg = path.ends_with(".svg");
        let key = (path.clone(), if svg { size[0] } else { 0 }, if svg { size[1] } else { 0 });
        if let Some(found) = self.images.get(&key) {
            return found.clone();
        }
        let started = std::time::Instant::now();
        let found = self.read_image(&path, size, images);
        if found.is_none() {
            diag::debug!(World, "panorama: no image {path}");
        }
        // Read from the pak and drawn on the frame that first shows it.
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if ms > 2.0 {
            diag::info!(World, "panorama: image {path} {size:?} took {ms:.1} ms");
        }
        self.images.insert(key, found.clone());
        found
    }

    fn read_image(
        &self,
        path: &str,
        size: [u32; 2],
        images: &mut Assets<Image>,
    ) -> Option<(Handle<Image>, [u32; 2])> {
        // Panorama asks for `.vtf`/`.png` interchangeably; try what the pak has.
        let stem = path.rsplit_once('.').map_or(path, |(s, _)| s);
        let candidates = [
            path.to_owned(),
            format!("{stem}.png"),
            format!("{stem}.svg"),
        ];
        let mut prefetched = self.prefetched.lock().unwrap_or_else(|poison| poison.into_inner());
        // A raster decodes once (`image` caches it whatever the size); an SVG may be drawn again
        // at another size.
        let ahead = candidates.iter().find_map(|p| {
            if p.ends_with(".svg") {
                prefetched.get(p).map(|b| (p.clone(), b.clone()))
            } else {
                prefetched.remove(p).map(|b| (p.clone(), b))
            }
        });
        drop(prefetched);
        let (path, bytes) = match ahead {
            Some(found) => found,
            None => candidates
                .iter()
                .find_map(|p| self.pak.read(p).map(|b| (p.clone(), b)))?,
        };
        if path.ends_with(".svg") {
            return Some(raster_svg(&bytes, size, images)?);
        }
        let image = Image::from_buffer(
            &bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::linear(),
            RenderAssetUsages::default(),
        )
        .ok()?;
        let px = [image.width(), image.height()];
        Some((images.add(image), px))
    }
}

/// Draws an SVG at `size` pixels (its own size when 0; one side 0 keeps its aspect).
fn raster_svg(
    bytes: &[u8],
    size: [u32; 2],
    images: &mut Assets<Image>,
) -> Option<(Handle<Image>, [u32; 2])> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let own = tree.size();
    let (w, h) = match size {
        [0, 0] => (own.width().ceil() as u32, own.height().ceil() as u32),
        [w, 0] => (w, ((w as f32) * own.height() / own.width()).ceil() as u32),
        [0, h] => (((h as f32) * own.width() / own.height()).ceil() as u32, h),
        [w, h] => (w, h),
    };
    let (w, h) = (w.clamp(1, 4096), h.clamp(1, 4096));
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    let transform = resvg::tiny_skia::Transform::from_scale(
        w as f32 / own.width(),
        h as f32 / own.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // tiny-skia keeps premultiplied alpha; textures want it straight.
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
            }
        }
    }
    let image = Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    Some((images.add(image), [w, h]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_salted_font_and_rejects_bad_trailers() {
        let font = b"\x00\x01\x00\x00rest".to_vec();
        let salts = [3u8, 9];
        let mut key = 0xa7u8;
        for &s in &salts {
            key ^= s.wrapping_add(0xa7);
        }
        let mut encoded = Vec::new();
        for &b in &font {
            let e = b ^ key;
            encoded.push(e);
            key = e.wrapping_add(0xa7);
        }
        encoded.extend_from_slice(&salts);
        encoded.push(salts.len() as u8 + 1);
        encoded.extend_from_slice(b"VFONT1");
        assert_eq!(decode_vfont(encoded), Some(font));
        assert_eq!(decode_vfont(b"short".to_vec()), None);
    }

    #[test]
    fn reads_strings_with_escapes() {
        let tokens = read_tokens(concat!(
            "\"lang\"\n{\n\t\"Tokens\"\n\t{\n",
            "\t\t\"Hud_Money\"\t\"${d:r:hud-money-amount}\"\n",
            "\t\t\"Q\" \"say \\\"hi\\\"\"\n",
        ));
        assert_eq!(tokens.get("hud_money").map(String::as_str), Some("${d:r:hud-money-amount}"));
        assert_eq!(tokens.get("q").map(String::as_str), Some("say \"hi\""));
    }

    #[test]
    fn maps_image_sources_to_the_pak() {
        assert_eq!(
            Files::image_path("file://{images}/icons/ui/Health.svg"),
            "materials/panorama/images/icons/ui/health.svg"
        );
    }
}
