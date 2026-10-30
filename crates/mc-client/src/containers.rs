//! Container screens: the player inventory (with the player model looking at the mouse, like vanilla) and the
//! containers the server opens (chests, crafting table, furnaces, hopper, dispenser, shulker box). Draws the
//! background, slots, cursor item and tooltip, and reports which slot is under the mouse.

use crate::hud::{self, Painter};
use crate::inventory::{Inventory, Menu, MenuKind, Stack};
use crate::models;
use glam::{Mat4, Vec3};
use mc_render::{GuiTexture, Vertex};

/// Top-left corner of each menu slot of the player inventory screen, relative to the 176x166 background.
pub fn player_slot_pos(slot: usize) -> Option<(f32, f32)> {
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

pub enum Open<'a> {
    Player(&'a Inventory),
    Menu(&'a Menu),
}

/// The panel's top-left corner and size.
pub fn bounds(p: &Painter, open: &Open) -> (f32, f32, f32, f32) {
    let (w, h) = match open {
        Open::Player(_) => (176.0, 166.0),
        Open::Menu(m) => {
            let (_, w, h) = m.kind.background();
            (w, h)
        }
    };
    (((p.width - w) / 2.0).floor(), ((p.height - h) / 2.0).floor(), w, h)
}

/// Draws the screen; returns the hovered slot (menu slot number) and whether the mouse is outside the panel.
pub fn draw(p: &mut Painter, open: &Open, cursor: Option<&Stack>, mouse: (f32, f32), look: (f32, f32), enchant_names: &[String]) -> (Option<usize>, bool) {
    p.gradient(0.0, 0.0, p.width, p.height, [16, 16, 16, 192], [16, 16, 16, 208]);
    let (x0, y0, w, h) = bounds(p, open);
    let (mx, my) = mouse;
    let slots: Vec<((f32, f32), Option<&Stack>)> = match open {
        Open::Player(inv) => {
            p.sprite("inventory", x0, y0);
            p.text_flat("Crafting", x0 + 97.0, y0 + 6.0, hud::DARK);
            player_model(p, x0 + 26.0, y0 + 8.0, x0 + 75.0, y0 + 78.0, look);
            (0..inv.slots.len()).filter_map(|i| Some((player_slot_pos(i)?, inv.slots[i].as_ref()))).collect()
        }
        Open::Menu(m) => {
            let (bg, _, _) = m.kind.background();
            match m.kind {
                MenuKind::Chest(rows) | MenuKind::Other(rows) => {
                    // generic_54 holds six rows; shorter chests use its top part plus the inventory part.
                    let top = rows.clamp(1, 6) as f32 * 18.0 + 17.0;
                    p.sprite_part(bg, x0, y0, 0.0, 0.0, 176.0, top);
                    p.sprite_part(bg, x0, y0 + top, 0.0, 126.0, 176.0, 96.0);
                }
                _ => p.sprite(bg, x0, y0),
            }
            if let MenuKind::Furnace(_) = m.kind {
                let get = |k: i16| m.data.get(&k).copied().unwrap_or(0) as f32;
                let (lit, lit_total, cook, cook_total) = (get(0), get(1).max(1.0), get(2), get(3));
                if lit > 0.0 {
                    let fh = ((lit / lit_total) * 13.0).ceil() + 1.0;
                    p.sprite_part("lit_progress", x0 + 56.0, y0 + 36.0 + 14.0 - fh, 0.0, 14.0 - fh, 14.0, fh);
                }
                if cook_total > 0.0 && cook > 0.0 {
                    let bw = ((cook / cook_total) * 24.0).ceil();
                    p.sprite_part("burn_progress", x0 + 79.0, y0 + 34.0, 0.0, 0.0, bw, 16.0);
                }
            }
            p.text_flat(&m.title, x0 + 8.0, y0 + 6.0, hud::DARK);
            let inv_label_y = match m.kind {
                MenuKind::Chest(rows) | MenuKind::Other(rows) => 18.0 + rows.clamp(1, 6) as f32 * 18.0 + 3.0,
                MenuKind::Hopper => 40.0,
                _ => 72.0,
            };
            p.text_flat("Inventory", x0 + 8.0, y0 + inv_label_y, hud::DARK);
            let positions = m.kind.slots(m.container_slots());
            positions.into_iter().zip(m.slots.iter().map(Option::as_ref)).collect()
        }
    };
    let mut hovered = None;
    for (i, ((sx, sy), stack)) in slots.iter().enumerate() {
        let (x, y) = (x0 + sx, y0 + sy);
        if let Some(s) = stack {
            p.item(s, x, y);
        }
        if mx >= x - 1.0 && mx < x + 17.0 && my >= y - 1.0 && my < y + 17.0 {
            p.fill(x, y, 16.0, 16.0, [255, 255, 255, 128]);
            hovered = Some(match open {
                Open::Player(_) => player_index(&slots, i),
                Open::Menu(_) => i,
            });
        }
    }
    let outside = mx < x0 || my < y0 || mx >= x0 + w || my >= y0 + h;
    match cursor {
        Some(c) => p.item(c, mx - 8.0, my - 8.0),
        None => {
            let hovered_stack = hovered.and_then(|h| match open {
                Open::Player(inv) => inv.slots.get(h).and_then(Option::as_ref),
                Open::Menu(m) => m.slots.get(h).and_then(Option::as_ref),
            });
            if let Some(s) = hovered_stack {
                let mut lines = vec![hud::item_name(p.models, s)];
                if s.enchanted && s.name.is_some() {
                    lines[0] = format!("\u{a7}b{}", lines[0]);
                }
                for (id, level) in &s.enchantments {
                    if let Some(name) = enchant_names.get(*id as usize) {
                        let key = format!("enchantment.minecraft.{}", name.strip_prefix("minecraft:").unwrap_or(name));
                        let label = p.models.lang.get(&key).cloned().unwrap_or_else(|| hud::pretty(name));
                        lines.push(format!("\u{a7}7{label} {}", hud::roman(*level)));
                    }
                }
                if let (Some(d), Some(max)) = (s.damage, s.max_damage) {
                    if d > 0 {
                        lines.push(format!("\u{a7}7Durability: {} / {max}", max.saturating_sub(d)));
                    }
                }
                p.tooltip(&lines, mx, my);
            }
        }
    }
    (hovered, outside)
}

/// The player screen's slots come from `player_slot_pos` in menu order, skipping none; the index is the slot.
fn player_index(_slots: &[((f32, f32), Option<&Stack>)], i: usize) -> usize {
    i
}

/// Draws Steve in the box `(x0, y0)..(x1, y1)`, turned toward the mouse (`look` = mouse offset from the box center).
fn player_model(p: &mut Painter, x0: f32, y0: f32, x1: f32, y1: f32, look: (f32, f32)) {
    let (cx, bottom) = ((x0 + x1) / 2.0, y1 - 5.0);
    let size = 30.0;
    // InventoryScreen.extractEntityInInventoryFollowsMouse: body turns by atan(dx / 40) * 20, head by twice that.
    let (dx, dy) = ((cx - look.0) / 40.0, ((y0 + y1) / 2.0 - 20.0 - look.1) / 40.0);
    let body_yaw = dx.atan() * 20.0;
    let head_yaw = dx.atan() * 40.0 - body_yaw;
    let pitch = -dy.atan() * 20.0;
    let model = models::player();
    let skin = p.models.gui.sprite("steve");
    let mut out: Vec<Vertex> = Vec::new();
    let k = p.scale;
    // World space here is GUI space: x right, y down, z toward the viewer; one block = `size` GUI pixels.
    let m = Mat4::from_translation(Vec3::new(cx, bottom, 100.0))
        * Mat4::from_scale(Vec3::new(size, -size, size))
        * Mat4::from_rotation_y((180.0 + body_yaw).to_radians())
        * Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0))
        * Mat4::from_translation(Vec3::new(0.0, -1.501, 0.0));
    let mut d = models::Draw { out: &mut out, sprite: skin, light: 1.0, tint: [1.0; 3] };
    let (hy, hp) = (head_yaw.to_radians(), pitch.to_radians());
    model.render(m, &|name, rot| {
        if name == "head" {
            rot[0] = hp;
            rot[1] = hy;
        }
    }, &mut d);
    // Back to front; each quad is 6 vertices (0 1 2, 0 2 3).
    let mut quads: Vec<(f32, [Vertex; 4])> = out.chunks_exact(6).map(|q| (q[0].pos[2] + q[1].pos[2] + q[2].pos[2] + q[5].pos[2], [q[0], q[1], q[2], q[5]])).collect();
    quads.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, q) in quads {
        p.gui.poly(GuiTexture::Gui, q.map(|v| [v.pos[0] * k, v.pos[1] * k]), q.map(|v| v.uv), q.map(|v| v.color));
    }
}
