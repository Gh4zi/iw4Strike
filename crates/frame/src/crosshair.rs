//! The CS crosshair settings (`cl_crosshair*`), and CS:GO / CS2 crosshair share codes read into
//! them, so a player's crosshair from either game carries over.
//!
//! Sizes are CS's: units of a 640x480 screen, scaled with the screen's height as CS:GO's `YRES`
//! scales them. CS2's newer codes hold whole pixels at the screen height they were made at and
//! are scaled by it. Splits, follow-recoil and the scope dot have no counterpart and are dropped.

/// The CS crosshair (`cl_crosshair*`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crosshair {
    /// The gap follows movement and shots (CS 1.6's dynamic crosshair). False holds it still, as
    /// CS:GO's static styles (`cl_crosshairstyle` 1 and 4) do.
    pub dynamic: bool,
    /// `cl_crosshairsize`: bar length.
    pub size: f32,
    /// `cl_crosshairgap`: added to the gap. A static crosshair's bars start [`Self::STATIC_GAP`]
    /// from the centre at 0; a dynamic one's at the held weapon's gap.
    pub gap: f32,
    /// `cl_crosshairthickness`; at least a pixel.
    pub thickness: f32,
    /// `cl_crosshaircolor_r/g/b` and `cl_crosshairalpha`.
    pub color: [u8; 4],
    /// `cl_crosshairdot`: a square of the bars' thickness at the centre.
    pub dot: bool,
    /// `cl_crosshair_t`: no top bar.
    pub t_style: bool,
    /// `cl_crosshair_drawoutline`: a black outline.
    pub outline: bool,
    /// `cl_crosshair_outlinethickness`, in screen pixels.
    pub outline_thickness: f32,
}

impl Default for Crosshair {
    /// CS 1.6's: green, dynamic, bars 5 long and 1 thick.
    fn default() -> Self {
        Self {
            dynamic: true,
            size: 5.0,
            gap: 0.0,
            thickness: 1.0,
            color: [50, 250, 50, 255],
            dot: false,
            t_style: false,
            outline: false,
            outline_thickness: 1.0,
        }
    }
}

impl Crosshair {
    /// Where a static crosshair's bars start with `cl_crosshairgap 0`.
    pub const STATIC_GAP: f32 = 4.0;
    /// CS:GO's `cl_crosshaircolor` presets 0-4: red, green, yellow, blue, cyan. 5 is custom.
    pub const PRESETS: [[u8; 3]; 5] = [
        [250, 50, 50],
        [50, 250, 50],
        [250, 250, 50],
        [50, 50, 250],
        [50, 250, 250],
    ];
    /// `cl_crosshaircolor` 5: the colour is `cl_crosshaircolor_r/g/b`.
    pub const CUSTOM_COLOR: u8 = 5;

    pub fn sanitize(&mut self) {
        let finite = |v: f32, lo: f32, hi: f32, default: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                default
            }
        };
        self.size = finite(self.size, 0.0, 20.0, 5.0);
        self.gap = finite(self.gap, -10.0, 20.0, 0.0);
        self.thickness = finite(self.thickness, 0.0, 10.0, 1.0);
        self.outline_thickness = finite(self.outline_thickness, 0.0, 5.0, 1.0);
    }

    /// The `cl_crosshaircolor` preset the colour is, else [`Self::CUSTOM_COLOR`].
    pub fn color_preset(&self) -> u8 {
        let rgb = [self.color[0], self.color[1], self.color[2]];
        Self::PRESETS
            .iter()
            .position(|preset| *preset == rgb)
            .map_or(Self::CUSTOM_COLOR, |index| index as u8)
    }

    /// `cl_crosshaircolor`: a preset sets the colour; [`Self::CUSTOM_COLOR`] keeps it.
    pub fn set_color_preset(&mut self, preset: u8) {
        if let Some(rgb) = Self::PRESETS.get(usize::from(preset)) {
            self.color[..3].copy_from_slice(rgb);
        }
    }

    /// `cl_crosshairstyle`, as CS:GO numbers it: 5 (legacy, dynamic) or 4 (classic static).
    pub fn style(&self) -> u8 {
        if self.dynamic { 5 } else { 4 }
    }

    /// `cl_crosshairstyle`: CS:GO's static styles 1 and 4 hold still, every other moves.
    pub fn set_style(&mut self, style: u8) {
        self.dynamic = !matches!(style, 1 | 4);
    }

    /// A `cl_crosshair*` variable's value as the console prints it and settings.cfg saves it.
    pub fn cvar(&self, name: &str) -> Option<String> {
        let flag = |on: bool| u8::from(on).to_string();
        Some(match name {
            "cl_crosshairstyle" => self.style().to_string(),
            "cl_crosshairsize" => number(self.size),
            "cl_crosshairgap" => number(self.gap),
            "cl_crosshairthickness" => number(self.thickness),
            "cl_crosshaircolor" => self.color_preset().to_string(),
            "cl_crosshaircolor_r" => self.color[0].to_string(),
            "cl_crosshaircolor_g" => self.color[1].to_string(),
            "cl_crosshaircolor_b" => self.color[2].to_string(),
            "cl_crosshairalpha" => self.color[3].to_string(),
            "cl_crosshairdot" => flag(self.dot),
            "cl_crosshair_t" => flag(self.t_style),
            "cl_crosshair_drawoutline" => flag(self.outline),
            "cl_crosshair_outlinethickness" => number(self.outline_thickness),
            _ => return None,
        })
    }

    /// Sets a `cl_crosshair*` variable from its text, as CS:GO reads it.
    pub fn set_cvar(&mut self, name: &str, value: &str) -> Result<(), String> {
        let value = value.trim();
        let bad = || format!("{name}: `{value}` is not a value it takes");
        let float = || {
            value
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(bad)
        };
        let byte = || {
            value
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .map(|v| v.round().clamp(0.0, 255.0) as u8)
                .ok_or_else(bad)
        };
        let flag = || match value {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(bad()),
        };
        match name {
            "cl_crosshairstyle" => self.set_style(byte()?),
            "cl_crosshairsize" => self.size = float()?,
            "cl_crosshairgap" => self.gap = float()?,
            "cl_crosshairthickness" => self.thickness = float()?,
            "cl_crosshaircolor" => match byte()? {
                preset @ 0..=Self::CUSTOM_COLOR => self.set_color_preset(preset),
                _ => return Err(bad()),
            },
            "cl_crosshaircolor_r" => self.color[0] = byte()?,
            "cl_crosshaircolor_g" => self.color[1] = byte()?,
            "cl_crosshaircolor_b" => self.color[2] = byte()?,
            "cl_crosshairalpha" => self.color[3] = byte()?,
            "cl_crosshairdot" => self.dot = flag()?,
            "cl_crosshair_t" => self.t_style = flag()?,
            "cl_crosshair_drawoutline" => self.outline = flag()?,
            "cl_crosshair_outlinethickness" => self.outline_thickness = float()?,
            _ => return Err(format!("{name} is not a crosshair setting")),
        }
        self.sanitize();
        Ok(())
    }
}

/// The `cl_crosshair*` variables [`Crosshair::cvar`] and [`Crosshair::set_cvar`] take, with what
/// each holds.
pub const CVARS: [(&str, &str); 13] = [
    (
        "cl_crosshairstyle",
        "4 static, 5 dynamic (CS:GO's 1 and 4 hold still, 0, 2 and 3 move)",
    ),
    ("cl_crosshairsize", "bar length, 0-20"),
    ("cl_crosshairgap", "added to the gap, -10 to 20"),
    ("cl_crosshairthickness", "bar thickness, 0-10"),
    (
        "cl_crosshaircolor",
        "0 red, 1 green, 2 yellow, 3 blue, 4 cyan, 5 cl_crosshaircolor_r/g/b",
    ),
    ("cl_crosshaircolor_r", "red, 0-255"),
    ("cl_crosshaircolor_g", "green, 0-255"),
    ("cl_crosshaircolor_b", "blue, 0-255"),
    ("cl_crosshairalpha", "opacity, 0-255"),
    ("cl_crosshairdot", "0|1, a dot at the centre"),
    ("cl_crosshair_t", "0|1, no top bar"),
    ("cl_crosshair_drawoutline", "0|1, a black outline"),
    (
        "cl_crosshair_outlinethickness",
        "outline width in pixels, 0-5",
    ),
];

/// A setting's number without trailing zeros: `2`, `0.5`, `3.125`.
fn number(value: f32) -> String {
    let text = format!("{value:.3}");
    match text.trim_end_matches('0').trim_end_matches('.') {
        "-0" => "0".to_owned(),
        text => text.to_owned(),
    }
}

/// The share code alphabet: base 57, no `I`, `g`, `l`, `0` or `1`.
const ALPHABET: &[u8; 57] = b"ABCDEFGHJKLMNOPQRSTUVWXYZabcdefhijkmnopqrstuvwxyz23456789";

/// Reads a crosshair share code: `CSGO-xxxxx-xxxxx-xxxxx-xxxxx-xxxxx` (CS:GO, and CS2 before
/// 1.41.8.8) or CS2's later 44-character `CS...` code.
pub fn decode_share_code(code: &str) -> Result<Crosshair, String> {
    let code = code.trim();
    let invalid = || "not a CS:GO or CS2 crosshair code".to_owned();
    if let Some(body) = strip_prefix_ignore_case(code, "CSGO") {
        let body: String = body.chars().filter(|c| *c != '-').collect();
        if body.len() != 25 {
            return Err(invalid());
        }
        let bytes = base57_bytes::<18>(&body).ok_or_else(invalid)?;
        check_sum(&bytes)?;
        return match bytes[1] {
            1 => Ok(legacy_v1(&bytes)),
            3 => Ok(legacy_pixels(&bytes, false)),
            4 => Ok(legacy_pixels(&bytes, true)),
            version => Err(format!("crosshair code version {version} is not supported")),
        };
    }
    if let Some(body) = strip_prefix_ignore_case(code, "CS")
        && body.len() == 44
    {
        let bytes = base57_bytes::<32>(body).ok_or_else(invalid)?;
        check_sum(&bytes)?;
        return match bytes[1] {
            1 => Ok(cs2_v1(&bytes)),
            version => Err(format!(
                "CS2 crosshair code version {version} is not supported"
            )),
        };
    }
    Err(invalid())
}

fn strip_prefix_ignore_case<'a>(code: &'a str, prefix: &str) -> Option<&'a str> {
    let head = code.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &code[prefix.len()..])
}

/// The code's characters as a base-57 number, least significant character first, written out
/// as `N` big-endian bytes.
fn base57_bytes<const N: usize>(chars: &str) -> Option<[u8; N]> {
    let mut bytes = [0u8; N];
    for c in chars.bytes().rev() {
        let mut carry = ALPHABET.iter().position(|a| *a == c)? as u32;
        for byte in bytes.iter_mut().rev() {
            let value = u32::from(*byte) * 57 + carry;
            *byte = value as u8;
            carry = value >> 8;
        }
        if carry != 0 {
            return None;
        }
    }
    Some(bytes)
}

/// Byte 0 is the sum of the others.
fn check_sum(bytes: &[u8]) -> Result<(), String> {
    let sum = bytes[1..]
        .iter()
        .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    if sum == bytes[0] {
        Ok(())
    } else {
        Err("a character in the code is wrong (checksum)".to_owned())
    }
}

/// CS:GO's layout: CS:GO units, tenths.
fn legacy_v1(b: &[u8]) -> Crosshair {
    let flags = b[13] >> 4;
    let preset = b[10] & 7;
    let rgb = Crosshair::PRESETS
        .get(usize::from(preset))
        .copied()
        .unwrap_or([b[4], b[5], b[6]]);
    let alpha = if flags & 4 != 0 { b[7] } else { 255 };
    let mut crosshair = Crosshair {
        dynamic: true,
        size: f32::from(b[14]) / 10.0,
        gap: f32::from(b[2] as i8) / 10.0,
        thickness: f32::from(b[12]) / 10.0,
        color: [rgb[0], rgb[1], rgb[2], alpha],
        dot: flags & 1 != 0,
        t_style: flags & 8 != 0,
        outline: b[10] & 8 != 0,
        outline_thickness: f32::from(b[3]) / 2.0,
    };
    crosshair.set_style((b[13] & 0xf) >> 1);
    crosshair
}

/// CS2 1.41.8.2 and .3: whole pixels at the screen height in bytes 14-15. Version 4 replaced the
/// outline flag with an outline mode (bits 28-29 of the field in bytes 10-13).
fn legacy_pixels(b: &[u8], outline_mode: bool) -> Crosshair {
    let bits = u32::from_le_bytes([b[10], b[11], b[12], b[13]]);
    let outline = if outline_mode {
        (bits >> 28) & 3 != 0
    } else {
        b[2] & 0x20 != 0
    };
    from_pixels(
        b[2] & 0xf,
        [b[3], b[4], b[5], b[6]],
        f32::from(b[7]),
        f32::from(b[8]),
        ((bits >> 23) & 0x1f) as f32,
        b[2] & 0x40 != 0,
        b[2] & 0x80 != 0,
        outline,
        u16::from_le_bytes([b[14], b[15]]),
    )
}

/// CS2 1.41.8.8's 32-byte code: whole pixels, a signed gap.
fn cs2_v1(b: &[u8]) -> Crosshair {
    let gap = i16::from_le_bytes([b[14], b[15]]).clamp(-3840, 3840);
    from_pixels(
        b[4] & 0x1f,
        [b[5], b[6], b[7], b[8]],
        f32::from(gap),
        f32::from(b[16]),
        f32::from(b[13] & 0x3f),
        b[4] & 0x40 != 0,
        b[4] & 0x80 != 0,
        b[13] >> 6 != 0,
        u16::from_le_bytes([b[2], b[3]]).max(240),
    )
}

#[allow(clippy::too_many_arguments)]
fn from_pixels(
    style: u8,
    color: [u8; 4],
    gap: f32,
    length: f32,
    thickness: f32,
    dot: bool,
    t_style: bool,
    outline: bool,
    screen_height: u16,
) -> Crosshair {
    // A code without a screen height was made before CS2 recorded one: 1080p.
    let height = if screen_height == 0 {
        1080.0
    } else {
        f32::from(screen_height)
    };
    let unit = 480.0 / height;
    let mut crosshair = Crosshair {
        dynamic: true,
        size: length * unit,
        gap: gap * unit - Crosshair::STATIC_GAP,
        thickness: thickness * unit,
        color,
        dot,
        t_style,
        outline,
        outline_thickness: 1.0,
    };
    crosshair.set_style(style);
    crosshair
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_csgo_code() {
        let crosshair =
            decode_share_code("CSGO-OXQqc-pVx3o-kSTSj-Qcd24-eWfOQ").expect("valid code");
        assert!(!crosshair.dynamic);
        assert_eq!(crosshair.size, 2.0);
        assert_eq!(crosshair.gap, -3.0);
        assert_eq!(crosshair.thickness, 0.0);
        assert_eq!(crosshair.color, [50, 250, 250, 255]);
        assert!(!crosshair.dot && !crosshair.t_style && !crosshair.outline);
    }

    #[test]
    fn reads_a_cs2_code() {
        let crosshair = decode_share_code("CSvbPubOq37zTGqtsPTP5QTrp5CB4xFXiKRLfzJsm49ZRe")
            .expect("valid code");
        assert!(crosshair.dynamic && crosshair.dot);
        assert_eq!(crosshair.color, [255, 0, 0, 255]);
        // 5 px long, 1 px thick and 0 px apart at 768 px high.
        assert_eq!(crosshair.size, 3.125);
        assert_eq!(crosshair.thickness, 0.625);
        assert_eq!(crosshair.gap, -Crosshair::STATIC_GAP);
    }

    #[test]
    fn rejects_a_wrong_character() {
        assert!(decode_share_code("CSGO-OXQqc-pVx3o-kSTSj-Qcd24-eWfOA").is_err());
        assert!(decode_share_code("CSGO-OXQqc-pVx3o").is_err());
        assert!(decode_share_code("not a code").is_err());
    }

    #[test]
    fn variables_round_trip() {
        let mut crosshair = Crosshair::default();
        crosshair
            .set_cvar("cl_crosshaircolor", "5")
            .expect("custom");
        crosshair
            .set_cvar("cl_crosshaircolor_r", "12")
            .expect("red");
        crosshair.set_cvar("cl_crosshairgap", "-2.5").expect("gap");
        crosshair.set_cvar("cl_crosshairstyle", "4").expect("style");
        crosshair.set_cvar("cl_crosshairdot", "1").expect("dot");
        let mut loaded = Crosshair::default();
        for (name, _) in CVARS {
            let value = crosshair.cvar(name).expect("known variable");
            loaded
                .set_cvar(name, &value)
                .expect("saved value reads back");
        }
        assert_eq!(loaded, crosshair);
        assert!(crosshair.set_cvar("cl_crosshairdot", "2").is_err());
    }
}
