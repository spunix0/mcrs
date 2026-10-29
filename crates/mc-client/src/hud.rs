//! Builds the HUD (hotbar) and the inventory screen as GUI quads, laid out in vanilla's GUI pixels.

use crate::inventory::{Inventory, Stack};
use mc_assets::BlockModels;
use mc_render::{Gui, GuiTexture};

const WHITE: [u8; 4] = [255, 255, 255, 255];

/// Vanilla's automatic GUI scale: as large as fits 320x240 GUI pixels.
pub fn gui_scale(width: u32, height: u32) -> f32 {
    ((width / 320).min(height / 240)).max(1) as f32
}

pub struct Painter<'a> {
    pub gui: Gui,
    pub models: &'a BlockModels,
    pub scale: f32,
    /// Window size in GUI pixels.
    pub width: f32,
    pub height: f32,
}

impl<'a> Painter<'a> {
    pub fn new(models: &'a BlockModels, width: u32, height: u32) -> Self {
        let scale = gui_scale(width, height);
        Self { gui: Gui::default(), models, scale, width: width as f32 / scale, height: height as f32 / scale }
    }

    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        let s = self.scale;
        [x * s, y * s, (x + w) * s, (y + h) * s]
    }

    pub fn sprite(&mut self, name: &str, x: f32, y: f32) {
        let s = self.models.gui.sprite(name);
        let r = self.rect(x, y, s.w, s.h);
        self.gui.quad(GuiTexture::Gui, r, s.uv, WHITE);
    }

    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let uv = self.models.gui.sprite("white").uv;
        let r = self.rect(x, y, w, h);
        self.gui.quad(GuiTexture::Gui, r, uv, color);
    }

    /// Draws ASCII text with vanilla's 1-pixel drop shadow (shadow = color at 25% brightness).
    pub fn text(&mut self, s: &str, x: f32, y: f32, color: [u8; 4]) {
        let shadow = [color[0] / 4, color[1] / 4, color[2] / 4, color[3]];
        for (dx, c) in [(1.0, shadow), (0.0, color)] {
            let mut cx = x + dx;
            for b in s.bytes() {
                let uv = self.models.gui.glyph(b);
                let r = self.rect(cx, y + dx, 8.0, 8.0);
                self.gui.quad(GuiTexture::Gui, r, uv, c);
                cx += self.models.gui.advance[b as usize] as f32;
            }
        }
    }

    /// A 16x16 item at (x, y) with its count and durability bar, like `GuiGraphics.renderItem` + decorations.
    pub fn item(&mut self, stack: &Stack, x: f32, y: f32) {
        let Some(info) = self.models.items.get(stack.item as usize) else { return };
        if let Some(uv) = info.icon {
            let tint = info.tint.map_or(WHITE, |t| [t[0], t[1], t[2], 255]);
            let r = self.rect(x, y, 16.0, 16.0);
            self.gui.quad(GuiTexture::Blocks, r, uv, tint);
        }
        if let (Some(damage), Some(max)) = (stack.damage, stack.max_damage) {
            if damage > 0 && max > 0 {
                let frac = 1.0 - damage as f32 / max as f32;
                let width = (13.0 * frac).round();
                // Hue from red (0) to green (1/3), as vanilla does.
                let (r, g) = if frac > 0.5 { ((2.0 - frac * 2.0) * 255.0, 255.0) } else { (255.0, frac * 2.0 * 255.0) };
                self.fill(x + 2.0, y + 13.0, 13.0, 2.0, [0, 0, 0, 255]);
                self.fill(x + 2.0, y + 13.0, width, 1.0, [r as u8, g as u8, 0, 255]);
            }
        }
        if stack.count > 1 {
            let label = stack.count.to_string();
            let w = self.models.gui.text_width(&label);
            self.text(&label, x + 17.0 - w, y + 9.0, WHITE);
        }
    }
}

pub fn hotbar(p: &mut Painter, inv: &Inventory, selected: u8) {
    let x = (p.width / 2.0 - 91.0).floor();
    let y = p.height - 22.0;
    p.sprite("hotbar", x, y);
    p.sprite("hotbar_selection", x - 1.0 + selected as f32 * 20.0, y - 1.0);
    for i in 0..9 {
        if let Some(s) = inv.hotbar(i) {
            p.item(s, x + 3.0 + i as f32 * 20.0, y + 3.0);
        }
    }
}

/// Hearts (left) and hunger (right) above the hotbar, as in vanilla's `Gui.renderPlayerHealth`.
pub fn status(p: &mut Painter, health: f32, food: i32) {
    let center = (p.width / 2.0).floor();
    let y = p.height - 39.0;
    let hp = health.ceil().max(0.0) as i32;
    for i in 0..10 {
        let x = center - 91.0 + i as f32 * 8.0;
        p.sprite("heart_container", x, y);
        if hp >= (i + 1) * 2 {
            p.sprite("heart_full", x, y);
        } else if hp == i * 2 + 1 {
            p.sprite("heart_half", x, y);
        }
        let fx = center + 91.0 - 9.0 - i as f32 * 8.0;
        p.sprite("food_empty", fx, y);
        if food >= (i + 1) * 2 {
            p.sprite("food_full", fx, y);
        } else if food == i * 2 + 1 {
            p.sprite("food_half", fx, y);
        }
    }
}

/// Only ASCII glyphs are in the bitmap font we load; anything else shows as `?`.
fn ascii(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii() && !c.is_control() { c } else { '?' }).collect()
}

/// Recent chat (bottom-left, fading out after 10 seconds) and the input line while typing.
pub fn chat(p: &mut Painter, log: &[(String, std::time::Instant)], input: Option<&str>) {
    let typing = input.is_some();
    let mut y = p.height - 48.0;
    if let Some(line) = input {
        p.fill(2.0, p.height - 14.0, p.width - 4.0, 12.0, [0, 0, 0, 128]);
        p.text(&format!("{}_", ascii(line)), 4.0, p.height - 12.0, WHITE);
    }
    let shown = if typing { 20 } else { 10 };
    for (line, at) in log.iter().rev().take(shown) {
        let age = at.elapsed().as_secs_f32();
        if !typing && age > 10.0 {
            break;
        }
        // Fade over the last second like vanilla.
        let alpha = if typing { 1.0 } else { (10.0 - age).clamp(0.0, 1.0) };
        p.fill(2.0, y - 1.0, 320.0, 9.0, [0, 0, 0, (128.0 * alpha) as u8]);
        p.text(&ascii(line), 4.0, y, [255, 255, 255, (255.0 * alpha) as u8]);
        y -= 9.0;
    }
}

/// Top-left corner of each menu slot of the player inventory screen, relative to the 176x166 background.
fn slot_pos(slot: usize) -> Option<(f32, f32)> {
    let s = slot as f32;
    Some(match slot {
        0 => (154.0, 28.0),
        1..=4 => (98.0 + ((slot - 1) % 2) as f32 * 18.0, 18.0 + ((slot - 1) / 2) as f32 * 18.0),
        5..=8 => (8.0, 8.0 + (s - 5.0) * 18.0),
        9..=35 => (8.0 + ((slot - 9) % 9) as f32 * 18.0, 84.0 + ((slot - 9) / 9) as f32 * 18.0),
        36..=44 => (8.0 + (s - 36.0) * 18.0, 142.0),
        45 => (77.0, 62.0),
        _ => return None,
    })
}

fn pretty(name: &str) -> String {
    name.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The player inventory screen; `mouse` is in window pixels. Returns the hovered menu slot.
pub fn inventory_screen(p: &mut Painter, inv: &Inventory, mouse: (f32, f32)) -> Option<usize> {
    p.fill(0.0, 0.0, p.width, p.height, [16, 16, 16, 176]);
    let (x0, y0) = (((p.width - 176.0) / 2.0).floor(), ((p.height - 166.0) / 2.0).floor());
    p.sprite("inventory", x0, y0);
    p.text("Crafting", x0 + 97.0, y0 + 6.0, [64, 64, 64, 255]);
    let (mx, my) = (mouse.0 / p.scale, mouse.1 / p.scale);
    let mut hovered = None;
    for slot in 0..inv.slots.len() {
        let Some((sx, sy)) = slot_pos(slot) else { continue };
        let (x, y) = (x0 + sx, y0 + sy);
        if let Some(s) = &inv.slots[slot] {
            p.item(s, x, y);
        }
        if mx >= x - 1.0 && mx < x + 17.0 && my >= y - 1.0 && my < y + 17.0 {
            p.fill(x, y, 16.0, 16.0, [255, 255, 255, 128]);
            hovered = Some(slot);
        }
    }
    if let Some(s) = hovered.and_then(|h| inv.slots[h].as_ref()) {
        let lang = &p.models.lang;
        let name = s.name.clone().unwrap_or_else(|| {
            p.models.items.get(s.item as usize).map_or("?".into(), |i| {
                // Vanilla names: item.minecraft.x or block.minecraft.x in en_us.json.
                lang.get(&format!("item.minecraft.{}", i.name)).or_else(|| lang.get(&format!("block.minecraft.{}", i.name))).cloned().unwrap_or_else(|| pretty(&i.name))
            })
        });
        let w = p.models.gui.text_width(&name);
        let (tx, ty) = (mx + 12.0, my - 12.0);
        // Vanilla tooltip: dark purple-black box with a lighter border.
        p.fill(tx - 3.0, ty - 3.0, w + 6.0, 14.0, [16, 0, 16, 240]);
        p.fill(tx - 2.0, ty - 2.0, w + 4.0, 1.0, [80, 0, 255, 80]);
        p.text(&ascii(&name), tx, ty, if s.enchanted { [85, 255, 255, 255] } else { WHITE });
    }
    hovered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_matches_vanilla_auto() {
        assert_eq!(gui_scale(1600, 900), 3.0);
        assert_eq!(gui_scale(1920, 1080), 4.0);
        assert_eq!(gui_scale(640, 480), 2.0);
        assert_eq!(gui_scale(300, 200), 1.0);
    }

    #[test]
    fn hotbar_and_offhand_slot_positions() {
        assert_eq!(slot_pos(36), Some((8.0, 142.0)));
        assert_eq!(slot_pos(44), Some((152.0, 142.0)));
        assert_eq!(slot_pos(9), Some((8.0, 84.0)));
        assert_eq!(slot_pos(45), Some((77.0, 62.0)));
        assert_eq!(pretty("diamond_pickaxe"), "Diamond Pickaxe");
    }
}
