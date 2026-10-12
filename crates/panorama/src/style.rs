//! A panel's computed style: the declarations that apply to it, in cascade order, read into the
//! properties Panorama's layout and drawing use.

use bevy::color::{Color, Srgba};

/// A size or offset.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Len {
    /// `fit-children` (or nothing said): as big as the content.
    #[default]
    Fit,
    Px(f32),
    Percent(f32),
    /// `fill-parent-flow( weight )`: what the parent's flow leaves, shared by weight.
    Fill(f32),
    /// `height-percentage( p% )` as a width, `width-percentage( p% )` as a height: this share
    /// of the panel's other side.
    OfOther(f32),
}

/// How a panel lays out its children (`flow-children`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Flow {
    /// Stacked on each other, each placed by its own alignment and offsets.
    #[default]
    None,
    Right,
    Down,
    RightWrap,
    DownWrap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

/// A linear gradient: its direction (the CSS angle: 0 up, clockwise, radians) and stops.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub angle: f32,
    pub stops: Vec<(f32, Color)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Color(Color),
    Gradient(Gradient),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub width: Len,
    pub height: Len,
    pub min_width: Option<Len>,
    pub min_height: Option<Len>,
    pub max_width: Option<Len>,
    pub max_height: Option<Len>,
    pub flow: Flow,
    pub halign: Option<Align>,
    pub valign: Option<Align>,
    pub x: Len,
    pub y: Len,
    /// Left, top, right, bottom.
    pub margin: [Len; 4],
    pub padding: [Len; 4],
    pub background: Option<Paint>,
    pub background_image: Option<String>,
    pub border_width: [f32; 4],
    pub border_color: Option<Color>,
    pub border_radius: Len,
    pub visible: bool,
    pub opacity: f32,
    pub wash: Option<Color>,
    pub z_index: Option<i32>,
    pub clip: bool,
    pub translate: (f32, f32),
    pub scale: (f32, f32),
    pub rotate: f32,
    /// `transform-origin`, as shares of the panel's width and height (the middle by default).
    pub origin: (f32, f32),
    // Text (inherited).
    pub color: Option<Color>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub font_weight: Option<String>,
    pub text_align: Option<Align>,
    pub uppercase: Option<bool>,
    pub text_shadow: Option<(f32, f32, Color)>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            width: Len::Fit,
            height: Len::Fit,
            min_width: None,
            min_height: None,
            max_width: None,
            max_height: None,
            flow: Flow::None,
            halign: None,
            valign: None,
            x: Len::Px(0.0),
            y: Len::Px(0.0),
            margin: [Len::Px(0.0); 4],
            padding: [Len::Px(0.0); 4],
            background: None,
            background_image: None,
            border_width: [0.0; 4],
            border_color: None,
            border_radius: Len::Px(0.0),
            visible: true,
            opacity: 1.0,
            wash: None,
            z_index: None,
            clip: false,
            translate: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotate: 0.0,
            origin: (0.5, 0.5),
            color: None,
            font_family: None,
            font_size: None,
            font_weight: None,
            text_align: None,
            uppercase: None,
            text_shadow: None,
        }
    }
}

/// The arguments inside the outermost `name( ... )`, split on top-level commas.
fn call_args<'a>(value: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let rest = value.trim();
    let lower = rest.to_ascii_lowercase();
    if !lower.starts_with(name) {
        return None;
    }
    let open = rest.find('(')?;
    let close = rest.rfind(')')?;
    let inner = &rest[open + 1..close];
    let mut args = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                args.push(inner[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    args.push(inner[start..].trim());
    Some(args)
}

/// A length: `12px`, `50%`, `fit-children`, `fill-parent-flow( 1 )`, a bare number (px).
#[must_use]
pub fn parse_len(value: &str) -> Option<Len> {
    let v = value.trim().to_ascii_lowercase();
    if v == "fit-children" || v == "auto" {
        return Some(Len::Fit);
    }
    if let Some(args) = call_args(&v, "fill-parent-flow") {
        return Some(Len::Fill(args.first()?.parse().unwrap_or(1.0)));
    }
    if let Some(args) = call_args(&v, "width-percentage")
        .or_else(|| call_args(&v, "height-percentage"))
    {
        let p: f32 = args.first()?.trim_end_matches('%').trim().parse().ok()?;
        return Some(Len::OfOther(p / 100.0));
    }
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse().ok().map(Len::Percent);
    }
    v.trim_end_matches("px").trim().parse().ok().map(Len::Px)
}

fn parse_px(value: &str) -> Option<f32> {
    match parse_len(value)? {
        Len::Px(px) => Some(px),
        _ => None,
    }
}

/// A colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`, a few names.
#[must_use]
pub fn parse_color(value: &str) -> Option<Color> {
    let v = value.trim();
    if let Some(hex) = v.strip_prefix('#') {
        let digit = |i: usize, n: usize| u8::from_str_radix(hex.get(i..i + n)?, 16).ok();
        let [r, g, b, a] = match hex.len() {
            3 | 4 => {
                let one = |i: usize| digit(i, 1).map(|d| d * 17);
                [one(0)?, one(1)?, one(2)?, if hex.len() == 4 { one(3)? } else { 255 }]
            }
            6 | 8 => [
                digit(0, 2)?,
                digit(2, 2)?,
                digit(4, 2)?,
                if hex.len() == 8 { digit(6, 2)? } else { 255 },
            ],
            _ => return None,
        };
        return Some(Color::srgba_u8(r, g, b, a));
    }
    let lower = v.to_ascii_lowercase();
    if let Some(args) = call_args(&lower, "rgba").or_else(|| call_args(&lower, "rgb")) {
        let n = |i: usize| args.get(i).and_then(|a| a.trim().parse::<f32>().ok());
        let a = if args.len() > 3 { n(3)? } else { 1.0 };
        return Some(Color::srgba(n(0)? / 255.0, n(1)? / 255.0, n(2)? / 255.0, a));
    }
    Some(match lower.as_str() {
        "white" => Color::WHITE,
        "black" => Color::BLACK,
        "transparent" | "none" => Color::NONE,
        "red" => Color::srgb(1.0, 0.0, 0.0),
        "green" => Color::srgb(0.0, 0.5, 0.0),
        "blue" => Color::srgb(0.0, 0.0, 1.0),
        "yellow" => Color::srgb(1.0, 1.0, 0.0),
        "gray" | "grey" => Color::srgb(0.5, 0.5, 0.5),
        _ => return None,
    })
}

/// `gradient( linear, x1 y1, x2 y2, from( c ), color-stop( t, c ), to( c ) )`.
fn parse_gradient(value: &str) -> Option<Gradient> {
    let args = call_args(value, "gradient")?;
    if !args.first()?.eq_ignore_ascii_case("linear") {
        return None;
    }
    let point = |s: &str| -> Option<(f32, f32)> {
        let mut it = s.split_whitespace().map(|p| p.trim_end_matches('%').parse::<f32>().ok());
        Some((it.next()??, it.next()??))
    };
    let (x1, y1) = point(args.get(1)?)?;
    let (x2, y2) = point(args.get(2)?)?;
    // CSS angles: 0 points up, clockwise; panel y grows down.
    let angle = (x2 - x1).atan2(-(y2 - y1));
    let mut stops = Vec::new();
    for arg in &args[3..] {
        if let Some(inner) = call_args(arg, "from") {
            stops.push((0.0, parse_color(inner.first()?)?));
        } else if let Some(inner) = call_args(arg, "to") {
            stops.push((1.0, parse_color(inner.first()?)?));
        } else if let Some(inner) = call_args(arg, "color-stop") {
            stops.push((inner.first()?.parse().ok()?, parse_color(inner.get(1)?)?));
        }
    }
    (stops.len() >= 2).then_some(Gradient { angle, stops })
}

fn parse_paint(value: &str) -> Option<Paint> {
    parse_gradient(value)
        .map(Paint::Gradient)
        .or_else(|| parse_color(value).map(Paint::Color))
}

fn parse_align(value: &str) -> Option<Align> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "left" | "top" => Align::Start,
        "center" | "middle" => Align::Center,
        "right" | "bottom" => Align::End,
        _ => return None,
    })
}

/// 1 to 4 lengths as left, top, right, bottom (CSS order: top, right, bottom, left).
fn parse_box(value: &str) -> Option<[Len; 4]> {
    let parts: Vec<Len> = value.split_whitespace().filter_map(parse_len).collect();
    let [top, right, bottom, left] = match parts.as_slice() {
        [a] => [*a; 4],
        [v, h] => [*v, *h, *v, *h],
        [t, h, b] => [*t, *h, *b, *h],
        [t, r, b, l, ..] => [*t, *r, *b, *l],
        [] => return None,
    };
    Some([left, top, right, bottom])
}

/// The url in `url("...")`.
fn parse_url(value: &str) -> Option<String> {
    let start = value.find("url(")? + 4;
    let end = value[start..].find(')')? + start;
    Some(value[start..end].trim().trim_matches(['"', '\'']).to_owned())
}

/// `transform: translateX( 4px ) scale3d( 1.1, 1.1, 1 ) rotateZ( 45deg ) …`.
fn parse_transform(value: &str, style: &mut Style) {
    let mut rest = value.trim();
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().to_ascii_lowercase();
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let args: Vec<&str> = rest[open + 1..open + close].split(',').map(str::trim).collect();
        let px = |i: usize| args.get(i).and_then(|a| parse_px(a)).unwrap_or(0.0);
        let num = |i: usize| args.get(i).and_then(|a| a.parse::<f32>().ok());
        match name.as_str() {
            "translatex" => style.translate.0 = px(0),
            "translatey" => style.translate.1 = px(0),
            "translate3d" | "translate" => style.translate = (px(0), px(1)),
            "scale3d" | "scale" => {
                let x = num(0).unwrap_or(1.0);
                style.scale = (x, num(1).unwrap_or(x));
            }
            "rotatez" | "rotate" | "rotate3d" => {
                let deg = args
                    .last()
                    .and_then(|a| a.trim_end_matches("deg").trim().parse::<f32>().ok())
                    .unwrap_or(0.0);
                style.rotate = deg.to_radians();
            }
            _ => {}
        }
        rest = &rest[open + close + 1..];
    }
}

impl Style {
    /// Applies one declaration.
    pub fn apply(&mut self, property: &str, value: &str) {
        let v = value.trim().trim_end_matches("!important").trim();
        match property {
            "width" => self.width = parse_len(v).unwrap_or(self.width),
            "height" => self.height = parse_len(v).unwrap_or(self.height),
            "min-width" => self.min_width = parse_len(v),
            "min-height" => self.min_height = parse_len(v),
            "max-width" => self.max_width = parse_len(v),
            "max-height" => self.max_height = parse_len(v),
            "flow-children" => {
                self.flow = match v.to_ascii_lowercase().as_str() {
                    "right" | "left" => Flow::Right,
                    "down" | "up" => Flow::Down,
                    "right-wrap" | "left-wrap" => Flow::RightWrap,
                    "down-wrap" | "up-wrap" => Flow::DownWrap,
                    _ => Flow::None,
                };
            }
            "horizontal-align" => self.halign = parse_align(v),
            "vertical-align" => self.valign = parse_align(v),
            "x" => self.x = parse_len(v).unwrap_or(self.x),
            "y" => self.y = parse_len(v).unwrap_or(self.y),
            "position" => {
                let mut it = v.split_whitespace().filter_map(parse_len);
                if let (Some(x), Some(y)) = (it.next(), it.next()) {
                    self.x = x;
                    self.y = y;
                }
            }
            "margin" => self.margin = parse_box(v).unwrap_or(self.margin),
            "padding" => self.padding = parse_box(v).unwrap_or(self.padding),
            "margin-left" | "margin-top" | "margin-right" | "margin-bottom" => {
                let i = ["left", "top", "right", "bottom"]
                    .iter()
                    .position(|s| property.ends_with(s))
                    .unwrap_or(0);
                self.margin[i] = parse_len(v).unwrap_or(self.margin[i]);
            }
            "padding-left" | "padding-top" | "padding-right" | "padding-bottom" => {
                let i = ["left", "top", "right", "bottom"]
                    .iter()
                    .position(|s| property.ends_with(s))
                    .unwrap_or(0);
                self.padding[i] = parse_len(v).unwrap_or(self.padding[i]);
            }
            "background-color" => {
                // Several layers: the last opaque-capable one draws (gradients over colours).
                self.background = v
                    .split(", gradient")
                    .next()
                    .and_then(parse_paint)
                    .or_else(|| parse_paint(v));
            }
            "background-image" => {
                if v.eq_ignore_ascii_case("none") {
                    self.background_image = None;
                } else if let Some(url) = parse_url(v) {
                    self.background_image = Some(url);
                } else if let Some(gradient) = parse_gradient(v) {
                    self.background = Some(Paint::Gradient(gradient));
                }
            }
            "border" | "border-left" | "border-top" | "border-right" | "border-bottom" => {
                let mut width = None;
                for part in v.split_whitespace() {
                    if let Some(px) = parse_px(part).filter(|_| part.ends_with("px")) {
                        width = Some(px);
                    } else if let Some(color) = parse_color(part) {
                        self.border_color = Some(color);
                    }
                }
                let width = width.unwrap_or(0.0);
                match property {
                    "border-left" => self.border_width[0] = width,
                    "border-top" => self.border_width[1] = width,
                    "border-right" => self.border_width[2] = width,
                    "border-bottom" => self.border_width[3] = width,
                    _ => self.border_width = [width; 4],
                }
            }
            "border-color" => self.border_color = parse_color(v),
            "border-width" => {
                if let Some(px) = parse_px(v) {
                    self.border_width = [px; 4];
                }
            }
            "border-radius" => {
                self.border_radius = v
                    .split_whitespace()
                    .next()
                    .and_then(parse_len)
                    .filter(|len| matches!(len, Len::Px(_) | Len::Percent(_)))
                    .unwrap_or(Len::Px(0.0));
            }
            "visibility" => self.visible = !v.eq_ignore_ascii_case("collapse"),
            "opacity" => self.opacity = v.parse().unwrap_or(self.opacity),
            "wash-color" | "wash-color-fast" => self.wash = parse_color(v),
            "z-index" => self.z_index = v.parse::<f32>().ok().map(|z| z as i32),
            "overflow" => self.clip = v.contains("clip") || v.contains("scroll"),
            "transform" => parse_transform(v, self),
            "transform-origin" => {
                let mut it = v
                    .split_whitespace()
                    .filter_map(|p| p.trim_end_matches('%').parse::<f32>().ok());
                if let (Some(x), Some(y)) = (it.next(), it.next()) {
                    self.origin = (x / 100.0, y / 100.0);
                }
            }
            "pre-transform-scale2d" => {
                let mut it = v.split(',').filter_map(|s| s.trim().parse::<f32>().ok());
                if let Some(x) = it.next() {
                    self.scale = (x, it.next().unwrap_or(x));
                }
            }
            "color" => self.color = parse_color(v).or(self.color),
            "font-family" => {
                self.font_family = v
                    .split(',')
                    .next()
                    .map(|f| f.trim().trim_matches(['"', '\'']).to_owned());
            }
            "font-size" => self.font_size = parse_px(v).or(self.font_size),
            "font-weight" => self.font_weight = Some(v.to_ascii_lowercase()),
            "text-align" => self.text_align = parse_align(v),
            "text-transform" => self.uppercase = Some(v.eq_ignore_ascii_case("uppercase")),
            "text-shadow" => {
                let px: Vec<f32> = v.split_whitespace().filter_map(parse_px).collect();
                let color = v.split_whitespace().filter_map(parse_color).last();
                self.text_shadow = match (px.as_slice(), color) {
                    ([x, y, ..], Some(c)) => Some((*x, *y, c)),
                    _ => None,
                };
            }
            _ => {}
        }
    }

    /// Text properties from the parent where this panel says nothing.
    pub fn inherit(&mut self, parent: &Style) {
        self.color = self.color.or(parent.color);
        if self.font_family.is_none() {
            self.font_family.clone_from(&parent.font_family);
        }
        self.font_size = self.font_size.or(parent.font_size);
        if self.font_weight.is_none() {
            self.font_weight.clone_from(&parent.font_weight);
        }
        self.text_align = self.text_align.or(parent.text_align);
        self.uppercase = self.uppercase.or(parent.uppercase);
        self.text_shadow = self.text_shadow.or(parent.text_shadow);
    }
}

/// `color` with its alpha scaled by `opacity` and tinted by `wash`.
#[must_use]
pub fn tint(color: Color, wash: Option<Color>, opacity: f32) -> Color {
    let c: Srgba = color.into();
    let w: Srgba = wash.map_or(Srgba::WHITE, Into::into);
    Color::srgba(c.red * w.red, c.green * w.green, c.blue * w.blue, c.alpha * w.alpha * opacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lengths_colours_gradients_and_boxes() {
        assert_eq!(parse_len("12px"), Some(Len::Px(12.0)));
        assert_eq!(parse_len("48%"), Some(Len::Percent(48.0)));
        assert_eq!(parse_len("fit-children"), Some(Len::Fit));
        assert_eq!(parse_len("fill-parent-flow( 1.0 )"), Some(Len::Fill(1.0)));
        assert_eq!(parse_len("height-percentage( 86.72% )"), Some(Len::OfOther(0.8672)));
        assert_eq!(parse_color("#66666699"), Some(Color::srgba_u8(0x66, 0x66, 0x66, 0x99)));
        assert_eq!(parse_color("#fff"), Some(Color::srgba_u8(255, 255, 255, 255)));
        let g = parse_gradient(
            "gradient( linear, 0% 0%, 100% 0%, from( #00000000 ), color-stop( 0.1, #000000BB ), to( #000000CC ) )",
        )
        .expect("gradient");
        assert!((g.angle - core::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(g.stops.len(), 3);
        let mut s = Style::default();
        s.apply("margin", "1px 2px 3px 4px");
        assert_eq!(s.margin, [Len::Px(4.0), Len::Px(1.0), Len::Px(2.0), Len::Px(3.0)]);
        s.apply("transform", "translateX( 5px ) scale3d( 1.2, 1.2, 1 )");
        assert_eq!((s.translate.0, s.scale.0), (5.0, 1.2));
    }
}
