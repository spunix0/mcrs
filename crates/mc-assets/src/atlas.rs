//! Packs 16x16 block textures into one RGBA atlas. Animated textures use their first frame for now.

use crate::Jar;
use std::collections::HashMap;

const TILE: usize = 16;
const COLUMNS: usize = 64;

pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Mip levels 1.. (each half the size of the previous), down to 1 pixel per tile.
    pub mips: Vec<Vec<u8>>,
    rects: HashMap<String, [f32; 4]>,
    alpha: HashMap<String, (bool, bool)>,
    pub animations: Vec<Animation>,
}

fn missing() -> Vec<u8> {
    let mut px = vec![0u8; TILE * TILE * 4];
    for y in 0..TILE {
        for x in 0..TILE {
            let i = (y * TILE + x) * 4;
            let magenta = (x < 8) != (y < 8);
            px[i..i + 4].copy_from_slice(if magenta { &[248, 0, 248, 255] } else { &[0, 0, 0, 255] });
        }
    }
    px
}

/// Decodes a PNG and returns the top-left square (first animation frame) scaled to 16x16.
fn load_tile(jar: &mut Jar, name: &str) -> Option<Vec<u8>> {
    let path = name.split_once(':').map_or(name, |(_, p)| p);
    let bytes = jar.read(&format!("assets/minecraft/textures/{path}.png"))?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let side = img.width().min(img.height()) as usize;
    if side == 0 {
        return None;
    }
    let mut px = vec![0u8; TILE * TILE * 4];
    for y in 0..TILE {
        for x in 0..TILE {
            let p = img.get_pixel((x * side / TILE) as u32, (y * side / TILE) as u32);
            px[(y * TILE + x) * 4..][..4].copy_from_slice(&p.0);
        }
    }
    Some(px)
}

/// Box-filters down to 1 pixel per tile. Colors are weighted by alpha so transparent pixels don't darken edges.
fn mipmaps(base: &[u8], width: usize, height: usize) -> Vec<Vec<u8>> {
    let mut levels = Vec::new();
    let (mut w, mut h, mut prev) = (width, height, base.to_vec());
    let mut tile = TILE;
    while tile > 1 {
        let (nw, nh) = (w / 2, h / 2);
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                let (mut rgb, mut a_sum) = ([0u32; 3], 0u32);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = &prev[((y * 2 + dy) * w + x * 2 + dx) * 4..][..4];
                    let a = p[3] as u32;
                    for c in 0..3 {
                        rgb[c] += p[c] as u32 * a;
                    }
                    a_sum += a;
                }
                let o = &mut next[(y * nw + x) * 4..][..4];
                for c in 0..3 {
                    o[c] = if a_sum > 0 { (rgb[c] / a_sum) as u8 } else { 0 };
                }
                o[3] = (a_sum / 4) as u8;
            }
        }
        levels.push(next.clone());
        (w, h, prev, tile) = (nw, nh, next, tile / 2);
    }
    levels
}

/// An animated texture (`.png.mcmeta` with `animation`): its tile in the atlas and its frames, each 16x16 RGBA
/// with its mip levels.
pub struct Animation {
    /// Top-left pixel of the tile in the atlas.
    pub x: u32,
    pub y: u32,
    /// Frame images: base level followed by each mip level.
    pub frames: Vec<Vec<Vec<u8>>>,
    /// (frame index, ticks it shows), in play order.
    pub sequence: Vec<(usize, u32)>,
    pub length: u32,
}

impl Animation {
    /// The frame showing at game tick `tick`.
    pub fn frame_at(&self, tick: u64) -> usize {
        let mut t = (tick % self.length.max(1) as u64) as u32;
        for &(f, time) in &self.sequence {
            if t < time {
                return f;
            }
            t -= time;
        }
        0
    }
}

/// Splits a vertical frame strip into 16x16 frames (with mips) and reads the frame order from the `.mcmeta`.
fn load_animation(jar: &mut Jar, name: &str) -> Option<(Vec<Vec<Vec<u8>>>, Vec<(usize, u32)>)> {
    let path = name.split_once(':').map_or(name, |(_, p)| p);
    let meta = jar.json(&format!("assets/minecraft/textures/{path}.png.mcmeta"))?;
    let anim = meta.get("animation")?;
    let bytes = jar.read(&format!("assets/minecraft/textures/{path}.png"))?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let side = img.width() as usize;
    let count = img.height() as usize / side.max(1);
    if count < 2 {
        return None;
    }
    let frames: Vec<Vec<Vec<u8>>> = (0..count)
        .map(|f| {
            let mut px = vec![0u8; TILE * TILE * 4];
            for y in 0..TILE {
                for x in 0..TILE {
                    let p = img.get_pixel((x * side / TILE) as u32, (f * side + y * side / TILE) as u32);
                    px[(y * TILE + x) * 4..][..4].copy_from_slice(&p.0);
                }
            }
            let mut levels = vec![px.clone()];
            levels.extend(mipmaps(&px, TILE, TILE));
            levels
        })
        .collect();
    let default_time = anim["frametime"].as_u64().unwrap_or(1).max(1) as u32;
    let sequence = match anim["frames"].as_array() {
        Some(list) => list
            .iter()
            .filter_map(|f| match f {
                serde_json::Value::Number(n) => Some((n.as_u64()? as usize, default_time)),
                o => Some((o["index"].as_u64()? as usize, o["time"].as_u64().map_or(default_time, |t| t.max(1) as u32))),
            })
            .filter(|(i, _)| *i < count)
            .collect(),
        None => (0..count).map(|i| (i, default_time)).collect(),
    };
    Some((frames, sequence))
}

impl Atlas {
    pub fn build(jar: &mut Jar, names: &[String]) -> anyhow::Result<Self> {
        let count = names.len() + 1;
        let rows = count.div_ceil(COLUMNS);
        let (width, height) = (COLUMNS * TILE, (rows * TILE).next_power_of_two());
        let mut rgba = vec![0u8; width * height * 4];
        let mut rects = HashMap::new();
        let mut alpha = HashMap::new();
        let inset = 1.0 / 256.0;
        let mut animations = Vec::new();

        let all = std::iter::once("minecraft:missingno".to_string()).chain(names.iter().cloned());
        for (i, name) in all.enumerate() {
            let px = load_tile(jar, &name).unwrap_or_else(missing);
            let (cx, cy) = ((i % COLUMNS) * TILE, (i / COLUMNS) * TILE);
            if let Some((frames, sequence)) = load_animation(jar, &name) {
                let length = sequence.iter().map(|s| s.1).sum();
                animations.push(Animation { x: cx as u32, y: cy as u32, frames, sequence, length });
            }
            for row in 0..TILE {
                let dst = ((cy + row) * width + cx) * 4;
                rgba[dst..dst + TILE * 4].copy_from_slice(&px[row * TILE * 4..(row + 1) * TILE * 4]);
            }
            let has_alpha = px.chunks_exact(4).any(|p| p[3] < 255);
            let translucent = px.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255);
            let (w, h) = (width as f32, height as f32);
            let ti = inset / w;
            let tj = inset / h;
            rects.insert(
                name.clone(),
                [cx as f32 / w + ti, cy as f32 / h + tj, (cx + TILE) as f32 / w - ti, (cy + TILE) as f32 / h - tj],
            );
            alpha.insert(name, (has_alpha, translucent));
        }
        let mips = mipmaps(&rgba, width, height);
        Ok(Self { width: width as u32, height: height as u32, rgba, mips, rects, alpha, animations })
    }

    pub fn rect(&self, name: &str) -> [f32; 4] {
        self.rects.get(name).or_else(|| self.rects.get("minecraft:missingno")).copied().unwrap_or([0.0; 4])
    }

    pub fn has_alpha(&self, name: &str) -> bool {
        self.alpha.get(name).is_some_and(|a| a.0)
    }

    pub fn is_translucent(&self, name: &str) -> bool {
        self.alpha.get(name).is_some_and(|a| a.1)
    }

    pub fn len(&self) -> usize {
        self.rects.len()
    }
}
