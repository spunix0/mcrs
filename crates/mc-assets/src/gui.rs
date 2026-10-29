//! GUI textures (hotbar, inventory background, ...) packed into one atlas, plus the ASCII bitmap font.

use crate::Jar;
use anyhow::{Context, Result};
use std::collections::HashMap;

/// A sprite in the GUI atlas: size in GUI pixels and its UV rectangle.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub w: f32,
    pub h: f32,
    pub uv: [f32; 4],
}

pub struct GuiAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    sprites: HashMap<&'static str, Sprite>,
    font_origin: [u32; 2],
    /// Advance in pixels for each byte value of the ASCII font (glyph width + 1 spacing).
    pub advance: [u8; 256],
}

/// `(name, path in the jar, crop to w x h from the top-left; 0 = whole image)`.
const SPRITES: &[(&str, &str, u32, u32)] = &[
    ("hotbar", "gui/sprites/hud/hotbar.png", 0, 0),
    ("hotbar_selection", "gui/sprites/hud/hotbar_selection.png", 0, 0),
    ("crosshair", "gui/sprites/hud/crosshair.png", 0, 0),
    ("inventory", "gui/container/inventory.png", 176, 166),
    ("font", "font/ascii.png", 0, 0),
];

const SIZE: u32 = 512;

impl GuiAtlas {
    pub fn load(jar: &mut Jar) -> Result<Self> {
        let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
        let mut sprites = HashMap::new();
        let (mut x, mut y, mut row_h) = (2u32, 2u32, 0u32);
        let uv = |x: u32, y: u32, w: u32, h: u32| [x as f32 / SIZE as f32, y as f32 / SIZE as f32, (x + w) as f32 / SIZE as f32, (y + h) as f32 / SIZE as f32];

        // A white pixel block for solid rectangles (backgrounds, slot highlights).
        for py in 0..4 {
            for px in 0..4 {
                rgba[((py * SIZE + px) * 4) as usize..][..4].copy_from_slice(&[255; 4]);
            }
        }
        sprites.insert("white", Sprite { w: 1.0, h: 1.0, uv: uv(1, 1, 1, 1) });

        let mut font_origin = [0, 0];
        let mut font: Option<image::RgbaImage> = None;
        for &(name, path, cw, ch) in SPRITES {
            let bytes = jar.read(&format!("assets/minecraft/textures/{path}")).with_context(|| format!("missing {path}"))?;
            let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?.to_rgba8();
            let (w, h) = if cw == 0 { (img.width(), img.height()) } else { (cw.min(img.width()), ch.min(img.height())) };
            if x + w + 2 > SIZE {
                (x, y, row_h) = (2, y + row_h + 2, 0);
            }
            anyhow::ensure!(y + h + 2 <= SIZE, "GUI atlas full at {name}");
            for py in 0..h {
                for px in 0..w {
                    let p = img.get_pixel(px, py).0;
                    rgba[(((y + py) * SIZE + x + px) * 4) as usize..][..4].copy_from_slice(&p);
                }
            }
            sprites.insert(name, Sprite { w: w as f32, h: h as f32, uv: uv(x, y, w, h) });
            if name == "font" {
                font_origin = [x, y];
                font = Some(img);
            }
            x += w + 2;
            row_h = row_h.max(h);
        }

        // Glyph widths like vanilla's bitmap provider: rightmost non-empty column + 1, plus 1 pixel spacing.
        let mut advance = [6u8; 256];
        if let Some(img) = font {
            let cell = img.width() / 16;
            for c in 0..256u32 {
                let (gx, gy) = ((c % 16) * cell, (c / 16) * cell);
                let width = (0..cell).rev().find(|&col| (0..cell).any(|row| img.get_pixel(gx + col, gy + row).0[3] > 0)).map_or(0, |col| col + 1);
                advance[c as usize] = (width * 8 / cell + 1) as u8;
            }
            advance[b' ' as usize] = 4;
        }
        Ok(Self { width: SIZE, height: SIZE, rgba, sprites, font_origin, advance })
    }

    pub fn sprite(&self, name: &str) -> Sprite {
        self.sprites.get(name).copied().unwrap_or(self.sprites["white"])
    }

    /// UV rectangle of an 8x8 font glyph.
    pub fn glyph(&self, c: u8) -> [f32; 4] {
        let f = self.sprites["font"];
        let cell = f.w / 16.0;
        let (gx, gy) = ((c % 16) as f32 * cell, (c / 16) as f32 * cell);
        let s = SIZE as f32;
        let (ox, oy) = (self.font_origin[0] as f32, self.font_origin[1] as f32);
        [(ox + gx) / s, (oy + gy) / s, (ox + gx + cell) / s, (oy + gy + cell) / s]
    }

    /// Width in GUI pixels of a string drawn with [`GuiAtlas::glyph`].
    pub fn text_width(&self, s: &str) -> f32 {
        s.bytes().map(|b| self.advance[b as usize] as f32).sum::<f32>() - 1.0
    }
}
