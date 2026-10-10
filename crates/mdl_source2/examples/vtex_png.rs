//! `vtex_png <pak01_dir.vpk> <texture.vtex_c> <max size> <out prefix>`: writes each channel of a
//! BC4/BC5 texture's first mip no wider than `max size` as a greyscale PNG
//! (`<prefix>_r.png`, `<prefix>_g.png`), to see how CS2 packs its material maps.

use mdl_source2::texture::{self, Format};

/// One BC4 block (8 bytes) into 16 values, row by row.
fn bc4_block(block: &[u8]) -> [u8; 16] {
    let (a0, a1) = (u16::from(block[0]), u16::from(block[1]));
    let mut palette = [0u16; 8];
    palette[0] = a0;
    palette[1] = a1;
    if a0 > a1 {
        for i in 1..7u16 {
            palette[usize::from(i) + 1] = ((7 - i) * a0 + i * a1) / 7;
        }
    } else {
        for i in 1..5u16 {
            palette[usize::from(i) + 1] = ((5 - i) * a0 + i * a1) / 5;
        }
        palette[6] = 0;
        palette[7] = 255;
    }
    let bits = block[2..8]
        .iter()
        .enumerate()
        .fold(0u64, |acc, (i, b)| acc | (u64::from(*b) << (8 * i)));
    core::array::from_fn(|i| palette[((bits >> (3 * i)) & 7) as usize] as u8)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, pak, path, max, prefix] = args.as_slice() else {
        eprintln!("usage: vtex_png <pak01_dir.vpk> <texture.vtex_c> <max size> <out prefix>");
        std::process::exit(2);
    };
    let max: u32 = max.parse().expect("max size");
    let vpk = mdl_source::Vpk::open(std::path::Path::new(pak)).expect("open pak");
    let bytes = vpk.read(path).expect("texture in the pak");
    let tex = texture::load(&bytes).expect("decode texture");
    let channels = match tex.format {
        Format::Bc4 => 1,
        Format::Bc5 => 2,
        other => panic!("{other:?}: only BC4 and BC5"),
    };
    let (mut w, mut h, mut level) = (tex.width, tex.height, 0usize);
    while w > max && level + 1 < tex.mips.len() {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        level += 1;
    }
    let data = &tex.mips[level];
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    let block_bytes = 8 * channels;
    for (channel, name) in ["r", "g"].iter().enumerate().take(channels) {
        let mut pixels = vec![0u8; (w * h) as usize];
        for by in 0..bh {
            for bx in 0..bw {
                let at = (by * bw + bx) * block_bytes + channel * 8;
                let Some(block) = data.get(at..at + 8) else {
                    continue;
                };
                for (i, v) in bc4_block(block).into_iter().enumerate() {
                    let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                    if x < w as usize && y < h as usize {
                        pixels[y * w as usize + x] = v;
                    }
                }
            }
        }
        let file = std::fs::File::create(format!("{prefix}_{name}.png")).expect("create png");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("png header")
            .write_image_data(&pixels)
            .expect("png data");
    }
    println!("{w}x{h} mip {level} {:?}", tex.format);
}
