//! Drawing in vanilla's GUI pixels: sprites, nine-sliced widgets, text with `§` color codes, item icons (3D for
//! blocks), and the in-game HUD (hotbar, health, hunger, air, XP, crosshair, chat, titles).

use crate::inventory::{Inventory, Stack};
use glam::{Mat4, Vec3};
use mc_assets::BlockModels;
use mc_render::{Gui, GuiTexture};

pub const WHITE: [u8; 4] = [255, 255, 255, 255];
pub const GRAY: [u8; 4] = [160, 160, 160, 255];
pub const DARK: [u8; 4] = [64, 64, 64, 255];

/// Vanilla's 16 text colors (`§0` .. `§f`).
const PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], [0x00, 0x00, 0xaa], [0x00, 0xaa, 0x00], [0x00, 0xaa, 0xaa], [0xaa, 0x00, 0x00], [0xaa, 0x00, 0xaa],
    [0xff, 0xaa, 0x00], [0xaa, 0xaa, 0xaa], [0x55, 0x55, 0x55], [0x55, 0x55, 0xff], [0x55, 0xff, 0x55], [0x55, 0xff, 0xff],
    [0xff, 0x55, 0x55], [0xff, 0x55, 0xff], [0xff, 0xff, 0x55], [0xff, 0xff, 0xff],
];

/// Vanilla's automatic GUI scale: as large as fits 320x240 GUI pixels; `chosen` (1..) caps it.
pub fn gui_scale(width: u32, height: u32, chosen: u32) -> f32 {
    let auto = ((width / 320).min(height / 240)).max(1);
    (if chosen == 0 { auto } else { chosen.min(auto) }) as f32
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
    pub fn new(models: &'a BlockModels, width: u32, height: u32, chosen_scale: u32) -> Self {
        let scale = gui_scale(width, height, chosen_scale);
        Self { gui: Gui::default(), models, scale, width: width as f32 / scale, height: height as f32 / scale }
    }

    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        let s = self.scale;
        [x * s, y * s, (x + w) * s, (y + h) * s]
    }

    /// Clips later drawing to a rectangle in GUI pixels.
    pub fn clip(&mut self, r: Option<[f32; 4]>) {
        let s = self.scale;
        self.gui.set_clip(r.map(|[x, y, w, h]| [x * s, y * s, w * s, h * s]));
    }

    pub fn sprite(&mut self, name: &str, x: f32, y: f32) {
        let s = self.models.gui.sprite(name);
        self.sprite_rect(name, x, y, s.w, s.h, WHITE);
    }

    pub fn sprite_rect(&mut self, name: &str, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let s = self.models.gui.sprite(name);
        let r = self.rect(x, y, w, h);
        self.gui.quad(GuiTexture::Gui, r, s.uv, color);
    }

    /// The top-left `w` x `h` pixels of a sprite (progress bars).
    pub fn sprite_part(&mut self, name: &str, x: f32, y: f32, u: f32, v: f32, w: f32, h: f32) {
        let s = self.models.gui.sprite(name);
        if s.w <= 0.0 || w <= 0.0 || h <= 0.0 {
            return;
        }
        let [u0, v0, u1, v1] = s.uv;
        let (du, dv) = ((u1 - u0) / s.w, (v1 - v0) / s.h);
        let r = self.rect(x, y, w, h);
        self.gui.quad(GuiTexture::Gui, r, [u0 + du * u, v0 + dv * v, u0 + du * (u + w), v0 + dv * (v + h)], WHITE);
    }

    /// A nine-sliced sprite stretched to `w` x `h` with a `border`-pixel frame (buttons, text fields).
    pub fn nine_slice(&mut self, name: &str, x: f32, y: f32, w: f32, h: f32, border: f32) {
        let s = self.models.gui.sprite(name);
        let [u0, v0, u1, v1] = s.uv;
        let (du, dv) = ((u1 - u0) / s.w, (v1 - v0) / s.h);
        let xs = [(0.0, border, 0.0, border), (border, w - border, border, s.w - border), (w - border, w, s.w - border, s.w)];
        let ys = [(0.0, border, 0.0, border), (border, h - border, border, s.h - border), (h - border, h, s.h - border, s.h)];
        for (x0, x1, su0, su1) in xs {
            for (y0, y1, sv0, sv1) in ys {
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let r = self.rect(x + x0, y + y0, x1 - x0, y1 - y0);
                self.gui.quad(GuiTexture::Gui, r, [u0 + du * su0, v0 + dv * sv0, u0 + du * su1, v0 + dv * sv1], WHITE);
            }
        }
    }

    /// A texture repeated to fill an area (menu backgrounds).
    pub fn tiled(&mut self, name: &str, x: f32, y: f32, w: f32, h: f32, color: [u8; 4], offset: f32) {
        let s = self.models.gui.sprite(name);
        let (tw, th) = (s.w.max(1.0), s.h.max(1.0));
        let mut ty = y - (offset % th);
        while ty < y + h {
            let mut tx = x;
            while tx < x + w {
                let (cw, ch) = ((x + w - tx).min(tw), (y + h - ty).min(th));
                let top = (y - ty).max(0.0);
                if ch > top {
                    let [u0, v0, u1, v1] = s.uv;
                    let (du, dv) = ((u1 - u0) / tw, (v1 - v0) / th);
                    let r = self.rect(tx, ty + top, cw, ch - top);
                    self.gui.quad(GuiTexture::Gui, r, [u0, v0 + dv * top, u0 + du * cw, v0 + dv * ch], color);
                }
                tx += tw;
            }
            ty += th;
        }
    }

    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let uv = self.models.gui.sprite("white").uv;
        let r = self.rect(x, y, w, h);
        self.gui.quad(GuiTexture::Gui, r, uv, color);
    }

    /// Vertical gradient (vanilla's `fillGradient`).
    pub fn gradient(&mut self, x: f32, y: f32, w: f32, h: f32, top: [u8; 4], bottom: [u8; 4]) {
        let [u0, v0, u1, v1] = self.models.gui.sprite("white").uv;
        let (mu, mv) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
        let s = self.scale;
        let (x0, y0, x1, y1) = (x * s, y * s, (x + w) * s, (y + h) * s);
        self.gui.poly(GuiTexture::Gui, [[x0, y0], [x0, y1], [x1, y1], [x1, y0]], [[mu, mv]; 4], [top, bottom, bottom, top]);
    }

    pub fn text_width(&self, s: &str) -> f32 {
        self.models.gui.text_width(s)
    }

    /// Text with vanilla's 1-pixel drop shadow (shadow = color at 25% brightness). `§x` codes change the color.
    pub fn text(&mut self, s: &str, x: f32, y: f32, color: [u8; 4]) {
        self.text_ex(s, x, y, color, 1.0, true);
    }

    pub fn text_flat(&mut self, s: &str, x: f32, y: f32, color: [u8; 4]) {
        self.text_ex(s, x, y, color, 1.0, false);
    }

    pub fn text_centered(&mut self, s: &str, cx: f32, y: f32, color: [u8; 4]) {
        let w = self.text_width(s);
        self.text(s, (cx - w / 2.0).floor(), y, color);
    }

    /// Text at `scale` times the GUI scale.
    pub fn text_ex(&mut self, s: &str, x: f32, y: f32, color: [u8; 4], scale: f32, shadow: bool) {
        let passes: &[f32] = if shadow { &[1.0, 0.0] } else { &[0.0] };
        for &dx in passes {
            let mut cx = x + dx * scale;
            let mut c = color;
            let mut chars = s.chars();
            while let Some(ch) = chars.next() {
                if ch == '\u{a7}' {
                    match chars.next().and_then(|k| k.to_digit(16)) {
                        Some(i) => {
                            let p = PALETTE[i as usize];
                            c = [p[0], p[1], p[2], color[3]];
                        }
                        None => c = color,
                    }
                    continue;
                }
                let b = if ch.is_ascii() && !ch.is_control() { ch as u8 } else { b'?' };
                let col = if dx > 0.0 { [c[0] / 4, c[1] / 4, c[2] / 4, c[3]] } else { c };
                let uv = self.models.gui.glyph(b);
                let r = self.rect(cx, y + dx * scale, 8.0 * scale, 8.0 * scale);
                self.gui.quad(GuiTexture::Gui, r, uv, col);
                cx += self.models.gui.advance[b as usize] as f32 * scale;
            }
        }
    }

    /// Text turned by `angle` degrees around (`cx`, `cy`), centered there (the title screen splash).
    pub fn text_rotated(&mut self, s: &str, cx: f32, cy: f32, angle: f32, scale: f32, color: [u8; 4]) {
        let (sin, cos) = angle.to_radians().sin_cos();
        let w = self.text_width(s) * scale;
        let k = self.scale;
        let rot = |x: f32, y: f32| [(cx + x * cos - y * sin) * k, (cy + x * sin + y * cos) * k];
        for (dx, shadow) in [(1.0, true), (0.0, false)] {
            let mut x = -w / 2.0;
            let col = if shadow { [color[0] / 4, color[1] / 4, color[2] / 4, color[3]] } else { color };
            for ch in s.chars() {
                let b = if ch.is_ascii() && !ch.is_control() { ch as u8 } else { b'?' };
                let [u0, v0, u1, v1] = self.models.gui.glyph(b);
                let (x0, y0, x1, y1) = (x + dx * scale, -4.0 * scale + dx * scale, x + (8.0 + dx) * scale, 4.0 * scale + dx * scale);
                self.gui.poly(GuiTexture::Gui, [rot(x0, y0), rot(x0, y1), rot(x1, y1), rot(x1, y0)], [[u0, v0], [u0, v1], [u1, v1], [u1, v0]], [col; 4]);
                x += self.models.gui.advance[b as usize] as f32 * scale;
            }
        }
    }

    /// A 16x16 item at (x, y) with its count and durability bar, like `GuiGraphics.renderItem` + decorations.
    pub fn item(&mut self, stack: &Stack, x: f32, y: f32) {
        self.item_icon(stack.item, x, y);
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
            let w = self.text_width(&label);
            self.text(&label, x + 17.0 - w, y + 9.0, WHITE);
        }
    }

    /// The item's icon: block items as their 3D model with the `gui` display transform, others flat.
    pub fn item_icon(&mut self, item: u32, x: f32, y: f32) {
        let Some(info) = self.models.items.get(item as usize) else { return };
        if let (true, Some(quads)) = (info.block, &info.model) {
            let k = self.scale;
            let m = Mat4::from_translation(Vec3::new(x + 8.0, y + 8.0, 0.0))
                * Mat4::from_scale(Vec3::new(16.0, -16.0, 16.0))
                * crate::hand::display(&info.gui)
                * Mat4::from_translation(Vec3::splat(-0.5));
            // Painter's algorithm: drop faces turned away, draw the rest back to front.
            let mut faces: Vec<(f32, [[f32; 2]; 4], [[f32; 2]; 4], [u8; 4])> = Vec::new();
            for q in quads.iter() {
                let local = q.pos.map(Vec3::from);
                let n = m.transform_vector3((local[1] - local[0]).cross(local[3] - local[0]));
                if n.z <= 1e-4 {
                    continue;
                }
                let p = local.map(|v| m.transform_point3(v));
                let depth = p.iter().map(|v| v.z).sum::<f32>();
                let tint = if q.tinted { info.model_tint } else { info.tint.unwrap_or([255; 3]) };
                let s = q.shade;
                let c = [(tint[0] as f32 * s) as u8, (tint[1] as f32 * s) as u8, (tint[2] as f32 * s) as u8, 255];
                faces.push((depth, p.map(|v| [v.x * k, v.y * k]), q.uv, c));
            }
            faces.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, pos, uv, c) in faces {
                self.gui.poly(GuiTexture::Blocks, pos, uv, [c; 4]);
            }
            return;
        }
        if let Some(uv) = info.icon {
            let tint = info.tint.map_or(WHITE, |t| [t[0], t[1], t[2], 255]);
            let r = self.rect(x, y, 16.0, 16.0);
            self.gui.quad(GuiTexture::Blocks, r, uv, tint);
        }
    }

    /// Vanilla's tooltip box with a title line and more lines under it.
    pub fn tooltip(&mut self, lines: &[String], mx: f32, my: f32) {
        if lines.is_empty() {
            return;
        }
        let w = lines.iter().map(|l| self.text_width(l)).fold(0.0, f32::max);
        let h = if lines.len() == 1 { 8.0 } else { 10.0 + (lines.len() - 1) as f32 * 10.0 - 2.0 };
        let mut x = mx + 12.0;
        let mut y = my - 12.0;
        if x + w + 4.0 > self.width {
            x = mx - 16.0 - w;
        }
        y = y.clamp(4.0, (self.height - h - 4.0).max(4.0));
        let bg = [16, 0, 16, 240];
        self.fill(x - 3.0, y - 4.0, w + 6.0, 1.0, bg);
        self.fill(x - 3.0, y + h + 3.0, w + 6.0, 1.0, bg);
        self.fill(x - 3.0, y - 3.0, w + 6.0, h + 6.0, bg);
        self.fill(x - 4.0, y - 3.0, 1.0, h + 6.0, bg);
        self.fill(x + w + 3.0, y - 3.0, 1.0, h + 6.0, bg);
        self.gradient(x - 3.0, y - 2.0, 1.0, h + 4.0, [80, 0, 255, 80], [40, 0, 127, 80]);
        self.gradient(x + w + 2.0, y - 2.0, 1.0, h + 4.0, [80, 0, 255, 80], [40, 0, 127, 80]);
        self.fill(x - 3.0, y - 3.0, w + 6.0, 1.0, [80, 0, 255, 80]);
        self.fill(x - 3.0, y + h + 2.0, w + 6.0, 1.0, [40, 0, 127, 80]);
        let mut ly = y;
        for (i, l) in lines.iter().enumerate() {
            self.text(l, x, ly, WHITE);
            ly += if i == 0 { 12.0 } else { 10.0 };
        }
    }
}

/// An item's display name, as vanilla's language file has it.
pub fn item_name(models: &BlockModels, stack: &Stack) -> String {
    stack.name.clone().unwrap_or_else(|| {
        models.items.get(stack.item as usize).map_or("?".into(), |i| {
            let lang = &models.lang;
            lang.get(&format!("item.minecraft.{}", i.name)).or_else(|| lang.get(&format!("block.minecraft.{}", i.name))).cloned().unwrap_or_else(|| pretty(&i.name))
        })
    })
}

pub fn pretty(name: &str) -> String {
    name.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Roman numerals for enchantment levels.
pub fn roman(n: u32) -> String {
    match n {
        1 => "I".into(),
        2 => "II".into(),
        3 => "III".into(),
        4 => "IV".into(),
        5 => "V".into(),
        6 => "VI".into(),
        7 => "VII".into(),
        8 => "VIII".into(),
        9 => "IX".into(),
        10 => "X".into(),
        n => n.to_string(),
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

/// Everything about the player's state the status bars show.
pub struct Status {
    pub health: f32,
    pub food: i32,
    pub hardcore: bool,
    /// Air supply in ticks (300 = full), shown while underwater.
    pub air: i32,
    pub xp_progress: f32,
    pub xp_level: i32,
    /// Ticks since the last hurt, for the heart flash.
    pub blink: bool,
}

/// Hearts, hunger, air and the experience bar, as vanilla's `Gui.renderPlayerHealth` and `renderExperienceBar`.
pub fn status(p: &mut Painter, s: &Status) {
    let center = (p.width / 2.0).floor();
    let x0 = center - 91.0;
    let bar_y = p.height - 29.0;
    p.sprite("xp_background", x0, bar_y);
    let filled = (s.xp_progress.clamp(0.0, 1.0) * 183.0).floor();
    if filled > 0.0 {
        p.sprite_part("xp_progress", x0, bar_y, 0.0, 0.0, filled.min(182.0), 5.0);
    }
    if s.xp_level > 0 {
        let label = s.xp_level.to_string();
        let x = ((p.width - p.text_width(&label)) / 2.0).floor();
        let y = p.height - 35.0;
        // Vanilla's outlined green number.
        for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            p.text_flat(&label, x + dx, y + dy, [0, 0, 0, 255]);
        }
        p.text_flat(&label, x, y, [0x80, 0xff, 0x20, 255]);
    }

    let y = p.height - 39.0;
    let hp = s.health.ceil().max(0.0) as i32;
    let (full, half) = if s.hardcore { ("heart_hardcore_full", "heart_hardcore_half") } else { ("heart_full", "heart_half") };
    for i in 0..10 {
        let x = x0 + i as f32 * 8.0;
        // Low health makes the hearts jitter.
        let jitter = if hp <= 4 && (i * 7 + (s.health * 13.0) as i32) % 3 == 0 { -1.0 } else { 0.0 };
        p.sprite("heart_container", x, y + jitter);
        if !s.blink || i % 2 == 0 {
            if hp >= (i + 1) * 2 {
                p.sprite(full, x, y + jitter);
            } else if hp == i * 2 + 1 {
                p.sprite(half, x, y + jitter);
            }
        }
        let fx = center + 91.0 - 9.0 - i as f32 * 8.0;
        p.sprite("food_empty", fx, y);
        if s.food >= (i + 1) * 2 {
            p.sprite("food_full", fx, y);
        } else if s.food == i * 2 + 1 {
            p.sprite("food_half", fx, y);
        }
    }
    if s.air < 300 {
        let full = ((s.air.max(0) as f32 * 10.0 / 300.0).ceil()) as i32;
        let popping = ((s.air.max(0) as f32 * 10.0 / 300.0).ceil() as i32) - (s.air.max(0) * 10 / 300);
        for i in 0..full.min(10) {
            let fx = center + 91.0 - 9.0 - i as f32 * 8.0;
            p.sprite(if i == full - 1 && popping > 0 { "air_bursting" } else { "air" }, fx, y - 10.0);
        }
    }
}

/// The crosshair and, while the attack recharges, vanilla's attack indicator under it.
pub fn crosshair(p: &mut Painter, attack_strength: f32, show_indicator: bool) {
    let (cx, cy) = ((p.width / 2.0).floor(), (p.height / 2.0).floor());
    p.sprite("crosshair", cx - 7.0, cy - 7.0);
    if show_indicator && attack_strength < 1.0 {
        let (x, y) = (cx - 8.0, cy - 7.0 + 16.0);
        p.sprite("attack_background", x, y);
        let w = (attack_strength * 17.0).floor();
        p.sprite_part("attack_progress", x, y, 0.0, 0.0, w, 4.0);
    }
}

/// Recent chat (bottom-left, fading out after 10 seconds) and the input line while typing.
pub fn chat(p: &mut Painter, log: &[(String, std::time::Instant)], input: Option<&str>, opacity: f32, background: f32) {
    let typing = input.is_some();
    let mut y = p.height - 48.0;
    if let Some(line) = input {
        p.fill(2.0, p.height - 14.0, p.width - 4.0, 12.0, [0, 0, 0, (background * 255.0) as u8]);
        let blink = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() / 300) % 2 == 0;
        p.text(&format!("{line}{}", if blink { "_" } else { "" }), 4.0, p.height - 12.0, WHITE);
    }
    let shown = if typing { 20 } else { 10 };
    for (line, at) in log.iter().rev().take(shown) {
        let age = at.elapsed().as_secs_f32();
        if !typing && age > 10.0 {
            break;
        }
        // Fade over the last second like vanilla.
        let alpha = opacity * if typing { 1.0 } else { (10.0 - age).clamp(0.0, 1.0) };
        p.fill(2.0, y - 1.0, 320.0, 9.0, [0, 0, 0, (background * 255.0 * alpha) as u8]);
        p.text(line, 4.0, y, [255, 255, 255, (255.0 * alpha) as u8]);
        y -= 9.0;
    }
}

/// Title (4x) and subtitle (2x) in the middle of the screen, with vanilla's fade.
pub fn title(p: &mut Painter, title: &str, subtitle: &str, alpha: f32) {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
    if a < 8 {
        return;
    }
    let (cx, cy) = (p.width / 2.0, p.height / 2.0);
    let w = p.text_width(title) * 4.0;
    p.text_ex(title, cx - w / 2.0, cy - 10.0 * 4.0 + 4.0 * 4.0 - 30.0, [255, 255, 255, a], 4.0, true);
    let w = p.text_width(subtitle) * 2.0;
    p.text_ex(subtitle, cx - w / 2.0, cy + 5.0 * 2.0 - 10.0, [255, 255, 255, a], 2.0, true);
}

/// The action bar message above the hotbar.
pub fn action_bar(p: &mut Painter, text: &str, alpha: f32) {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
    if a >= 8 {
        p.text_centered(text, p.width / 2.0, p.height - 68.0, [255, 255, 255, a]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_matches_vanilla_auto() {
        assert_eq!(gui_scale(1600, 900, 0), 3.0);
        assert_eq!(gui_scale(1920, 1080, 0), 4.0);
        assert_eq!(gui_scale(640, 480, 0), 2.0);
        assert_eq!(gui_scale(300, 200, 0), 1.0);
        assert_eq!(gui_scale(1920, 1080, 2), 2.0);
        assert_eq!(gui_scale(640, 480, 4), 2.0);
        assert_eq!(pretty("diamond_pickaxe"), "Diamond Pickaxe");
        assert_eq!(roman(4), "IV");
    }
}
