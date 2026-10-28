//! Packs 16x16 block textures into one RGBA atlas. Animated textures use their first frame for now.

use crate::Jar;
use std::collections::HashMap;

const TILE: usize = 16;
const COLUMNS: usize = 64;

pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    rects: HashMap<String, [f32; 4]>,
    alpha: HashMap<String, (bool, bool)>,
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

impl Atlas {
    pub fn build(jar: &mut Jar, names: &[String]) -> anyhow::Result<Self> {
        let count = names.len() + 1;
        let rows = count.div_ceil(COLUMNS);
        let (width, height) = (COLUMNS * TILE, (rows * TILE).next_power_of_two());
        let mut rgba = vec![0u8; width * height * 4];
        let mut rects = HashMap::new();
        let mut alpha = HashMap::new();
        let inset = 1.0 / 256.0;

        let all = std::iter::once("minecraft:missingno".to_string()).chain(names.iter().cloned());
        for (i, name) in all.enumerate() {
            let px = load_tile(jar, &name).unwrap_or_else(missing);
            let (cx, cy) = ((i % COLUMNS) * TILE, (i / COLUMNS) * TILE);
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
        Ok(Self { width: width as u32, height: height as u32, rgba, rects, alpha })
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
