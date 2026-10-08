//! GoldSrc sprites (`.spr`, `IDSP` version 2): Half-Life and Counter-Strike 1.6 HUD sheets,
//! effects and crosshairs.
//!
//! A sprite is a 256-colour palette and one or more frames of palette indices. How a frame is
//! drawn depends on its texture format: additive sprites (every HUD sheet) add the palette
//! colour, so black is see-through; index-alpha sprites take the colour from the last palette
//! entry and the index as coverage; alpha-test sprites hide index 255. The layout follows the
//! public `spritegn.h` structure definitions; the code is our own.

use crate::MdlError;

/// The texture formats (`SPR_NORMAL` … `SPR_ALPHTEST`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexFormat {
    Normal,
    Additive,
    IndexAlpha,
    AlphaTest,
}

/// One frame decoded to RGBA (8 bits per channel, rows top to bottom).
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Sprite {
    pub format: TexFormat,
    /// Single frames in file order; frame groups (animated sprites) are skipped past.
    pub frames: Vec<Frame>,
}

const HEADER_LEN: usize = 40;

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8], MdlError> {
        let end = self.at.checked_add(len).ok_or(MdlError::Truncated(what))?;
        let bytes = self.data.get(self.at..end).ok_or(MdlError::Truncated(what))?;
        self.at = end;
        Ok(bytes)
    }

    fn i32(&mut self, what: &'static str) -> Result<i32, MdlError> {
        let bytes = self.take(4, what)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u16(&mut self, what: &'static str) -> Result<u16, MdlError> {
        let bytes = self.take(2, what)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
}

impl Sprite {
    /// Decodes a version 2 sprite. Every frame comes out in RGBA; additive frames keep their
    /// palette colour with full alpha (black stays black), so a caller drawing them additively
    /// — or turning brightness into coverage — gets what GoldSrc draws.
    pub fn parse(data: &[u8]) -> Result<Self, MdlError> {
        let mut r = Reader { data, at: 0 };
        let header = r.take(HEADER_LEN, "sprite header")?;
        if &header[0..4] != b"IDSP" {
            return Err(MdlError::Bad("sprite header (no IDSP)"));
        }
        let field = |at: usize| {
            i32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        let version = field(4);
        if version != 2 {
            return Err(MdlError::Version(version));
        }
        let format = match field(12) {
            0 => TexFormat::Normal,
            1 => TexFormat::Additive,
            2 => TexFormat::IndexAlpha,
            3 => TexFormat::AlphaTest,
            _ => return Err(MdlError::Bad("sprite texture format")),
        };
        let frame_count = usize::try_from(field(28)).map_err(|_| MdlError::Bad("frame count"))?;
        let colors = usize::from(r.u16("palette size")?);
        let palette = r.take(colors * 3, "palette")?;
        let color = |index: u8| {
            let at = usize::from(index) * 3;
            palette.get(at..at + 3).map_or([0, 0, 0], |c| [c[0], c[1], c[2]])
        };
        let mut frames = Vec::new();
        for _ in 0..frame_count {
            match r.i32("frame type")? {
                0 => frames.push(Self::frame(&mut r, format, &color, colors)?),
                _ => {
                    // A group: its count, one interval each, then its frames.
                    let count = usize::try_from(r.i32("group size")?)
                        .map_err(|_| MdlError::Bad("group size"))?;
                    r.take(count * 4, "group intervals")?;
                    for _ in 0..count {
                        Self::frame(&mut r, format, &color, colors)?;
                    }
                }
            }
        }
        Ok(Self { format, frames })
    }

    fn frame(
        r: &mut Reader,
        format: TexFormat,
        color: &impl Fn(u8) -> [u8; 3],
        colors: usize,
    ) -> Result<Frame, MdlError> {
        let _origin = (r.i32("frame origin")?, r.i32("frame origin")?);
        let width = u32::try_from(r.i32("frame width")?).map_err(|_| MdlError::Bad("width"))?;
        let height = u32::try_from(r.i32("frame height")?).map_err(|_| MdlError::Bad("height"))?;
        let pixels = r.take(width as usize * height as usize, "frame pixels")?;
        let last = color(u8::try_from(colors.saturating_sub(1)).unwrap_or(255));
        let mut rgba = Vec::with_capacity(pixels.len() * 4);
        for &index in pixels {
            let px = match format {
                TexFormat::IndexAlpha => [last[0], last[1], last[2], index],
                TexFormat::AlphaTest if index == 255 => [0, 0, 0, 0],
                _ => {
                    let [r, g, b] = color(index);
                    [r, g, b, 255]
                }
            };
            rgba.extend_from_slice(&px);
        }
        Ok(Frame {
            width,
            height,
            rgba,
        })
    }
}

/// One line of `sprites/hud.txt` or a `weapon_*.txt`: a named cell of a sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HudSprite {
    pub name: String,
    /// The screen width the cell is meant for (320 or 640).
    pub resolution: u32,
    /// The sheet, `sprites/<sheet>.spr`.
    pub sheet: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Reads a HUD sprite list (`hud.txt`, `weapon_*.txt`): a count line, then
/// `name resolution sheet x y width height` per line.
#[must_use]
pub fn parse_hud_list(text: &str) -> Vec<HudSprite> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?.to_owned();
            let resolution = fields.next()?.parse().ok()?;
            let sheet = fields.next()?.to_owned();
            let mut number = || fields.next()?.parse::<u32>().ok();
            Some(HudSprite {
                name,
                resolution,
                sheet,
                x: number()?,
                y: number()?,
                width: number()?,
                height: number()?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprite(format: i32, pixels: &[u8], width: i32, height: i32) -> Vec<u8> {
        let mut data = b"IDSP".to_vec();
        for value in [2, 2, format] {
            data.extend_from_slice(&i32::to_le_bytes(value));
        }
        data.extend_from_slice(&1.0f32.to_le_bytes());
        for value in [width, height, 1, 0, 1] {
            data.extend_from_slice(&i32::to_le_bytes(value));
        }
        data.extend_from_slice(&256u16.to_le_bytes());
        for index in 0..=255u8 {
            data.extend_from_slice(&[index, index / 2, 0]);
        }
        for value in [0, 0, 0, width, height] {
            data.extend_from_slice(&i32::to_le_bytes(value));
        }
        data.extend_from_slice(pixels);
        data
    }

    #[test]
    fn additive_frames_keep_the_palette_colour() {
        let parsed = Sprite::parse(&sprite(1, &[0, 200, 100, 255], 2, 2)).unwrap();
        assert_eq!(parsed.format, TexFormat::Additive);
        let frame = &parsed.frames[0];
        assert_eq!((frame.width, frame.height), (2, 2));
        assert_eq!(&frame.rgba[0..8], &[0, 0, 0, 255, 200, 100, 0, 255]);
    }

    #[test]
    fn alpha_test_hides_the_last_index() {
        let parsed = Sprite::parse(&sprite(3, &[255, 10], 2, 1)).unwrap();
        assert_eq!(&parsed.frames[0].rgba, &[0, 0, 0, 0, 10, 5, 0, 255]);
    }

    #[test]
    fn hud_lists_skip_the_count_and_read_each_cell() {
        let list = parse_hud_list(
            "3\nnumber_0\t\t640 640hud7\t0\t0\t20\t25\nd_aug\t\t640 640hud1 148\t240\t44\t16\n",
        );
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[1],
            HudSprite {
                name: "d_aug".into(),
                resolution: 640,
                sheet: "640hud1".into(),
                x: 148,
                y: 240,
                width: 44,
                height: 16,
            }
        );
    }
}
