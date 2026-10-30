//! The creative inventory, as vanilla's `CreativeModeInventoryScreen`: item tabs above and below the panel, a
//! scrolling 9x5 grid, search, and the inventory tab with armor, offhand and the delete slot. The held stack lives on
//! the client; every changed inventory slot is sent with `set_creative_mode_slot`, like vanilla.

use crate::hud::{self, Painter};
use crate::inventory::{Inventory, Stack};
use crate::ui::Input;
use mc_assets::BlockModels;
use std::collections::HashMap;
use winit::keyboard::KeyCode;

const W: f32 = 195.0;
const H: f32 = 136.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Items,
    Search,
    Inventory,
}

pub struct Tab {
    pub title: String,
    pub icon: u32,
    pub top: bool,
    pub column: u8,
    pub kind: Kind,
    pub items: Vec<u32>,
}

/// Vanilla's tab order: (key, icon, top row?, column).
const TABS: [(&str, &str, bool, u8); 12] = [
    ("buildingBlocks", "bricks", true, 0),
    ("coloredBlocks", "cyan_wool", true, 1),
    ("natural", "grass_block", true, 2),
    ("functional", "oak_sign", true, 3),
    ("redstone", "redstone", true, 4),
    ("search", "compass", true, 6),
    ("tools", "diamond_pickaxe", false, 0),
    ("combat", "netherite_sword", false, 1),
    ("foodAndDrink", "golden_apple", false, 2),
    ("ingredients", "iron_ingot", false, 3),
    ("spawnEggs", "creeper_spawn_egg", false, 4),
    ("inventory", "chest", false, 6),
];

/// `gameplayColorOrder`.
const COLORS: [&str; 16] = ["white", "light_gray", "gray", "black", "brown", "red", "orange", "yellow", "lime", "green", "cyan", "light_blue", "blue", "purple", "magenta", "pink"];

/// Builds the tabs from the extracted `CreativeModeTabs` lists, resolving color and copper families by name.
pub fn tabs(models: &BlockModels) -> Vec<Tab> {
    let ids: HashMap<&str, u32> = models.items.iter().enumerate().map(|(i, it)| (it.name.as_str(), i as u32)).collect();
    let mut lists: HashMap<&str, Vec<u32>> = HashMap::new();
    for line in mc_assets::CREATIVE_TABS.lines() {
        let mut f = line.split('\t');
        let (Some(tab), Some(kind), Some(name)) = (f.next(), f.next(), f.next()) else { continue };
        let list = lists.entry(tab).or_default();
        let lower = name.to_ascii_lowercase();
        let base = lower.strip_prefix("dyed_").unwrap_or(&lower);
        match kind {
            "item" => list.extend(ids.get(lower.as_str())),
            "pick" => {
                // FAMILY:method, e.g. WOOL:white -> white_wool, LIGHTNING_ROD:waxed -> waxed_lightning_rod.
                if let Some((fam, m)) = lower.split_once(':') {
                    list.extend(ids.get(format!("{m}_{}", fam.strip_prefix("dyed_").unwrap_or(fam)).as_str()));
                }
            }
            "colored" => list.extend(COLORS.iter().filter_map(|c| ids.get(format!("{c}_{base}").as_str()))),
            _ => {
                // Every item of a family: the plain one, then all named `<something>_<family>`.
                let (prefix, fam) = lower.strip_prefix("waxed_").map_or(("", base), |f| ("waxed_", f));
                let mut found: Vec<(&str, u32)> = ids.iter().filter(|(n, _)| n.starts_with(prefix) && (n.ends_with(&format!("_{fam}")) || **n == fam)).map(|(n, i)| (*n, *i)).collect();
                found.sort_by_key(|(_, i)| *i);
                list.extend(found.into_iter().map(|(_, i)| i));
            }
        }
    }
    let all: Vec<u32> = (1..models.items.len() as u32).collect();
    TABS.iter()
        .map(|&(key, icon, top, column)| {
            let kind = match key {
                "search" => Kind::Search,
                "inventory" => Kind::Inventory,
                _ => Kind::Items,
            };
            let mut items = if kind == Kind::Search { all.clone() } else { lists.remove(key).unwrap_or_default() };
            let mut seen = std::collections::HashSet::new();
            items.retain(|i| seen.insert(*i));
            let title = models.lang.get(&format!("itemGroup.{key}")).cloned().unwrap_or_else(|| hud::pretty(key));
            Tab { title, icon: ids.get(icon).copied().unwrap_or(0), top, column, kind, items }
        })
        .collect()
}

/// Max stack size from the item's name (the real value is an item component the server doesn't send).
/// shortcut: name rules, not the item registry's `max_stack_size`; reading the jar's item components would be exact.
pub fn max_stack(name: &str) -> u32 {
    const ONE: [&str; 33] = [
        "_sword", "_pickaxe", "_axe", "_shovel", "_hoe", "_helmet", "_chestplate", "_leggings", "_boots", "bow", "trident", "shield", "elytra",
        "totem_of_undying", "_boat", "_raft", "minecart", "saddle", "shears", "flint_and_steel", "fishing_rod", "_on_a_stick", "potion",
        "_book", "_stew", "_soup", "spyglass", "brush", "mace", "_horse_armor", "shulker_box", "bundle", "_bed",
    ];
    const SIXTEEN: [&str; 8] = ["ender_pearl", "snowball", "egg", "bucket", "_sign", "banner", "honey_bottle", "armor_stand"];
    if name.ends_with("_spawn_egg") {
        return 64;
    }
    if name.starts_with("music_disc") || name == "cake" || name == "goat_horn" || name == "debug_stick" || name == "knowledge_book" {
        return 1;
    }
    if name.ends_with("_bucket") || ONE.iter().any(|s| name.ends_with(s)) {
        return 1;
    }
    if SIXTEEN.iter().any(|s| name.ends_with(s)) {
        return 16;
    }
    64
}

/// What the screen asks the game to send.
pub enum Out {
    /// `set_creative_mode_slot` for a menu slot of the inventory (0..45).
    SetSlot(i16, Option<Stack>),
    /// Drop a stack into the world (`set_creative_mode_slot` slot -1).
    Drop(Stack),
}

#[derive(Default)]
pub struct Screen {
    pub tab: usize,
    pub scroll: f32,
    pub search: String,
    pub carried: Option<Stack>,
    pub dragging: bool,
}

fn plain(item: u32, count: u32) -> Stack {
    Stack { item, count, damage: None, max_damage: None, name: None, enchanted: false, enchantments: Vec::new() }
}

fn name_of(models: &BlockModels, item: u32) -> String {
    hud::item_name(models, &plain(item, 1))
}

/// Where each inventory menu slot sits on the inventory tab.
fn inventory_tab_pos(i: usize) -> Option<(f32, f32)> {
    Some(match i {
        5..=8 => {
            let p = i - 5;
            (54.0 + (p / 2) as f32 * 54.0, 6.0 + (p % 2) as f32 * 27.0)
        }
        45 => (35.0, 20.0),
        9..=35 => (9.0 + ((i - 9) % 9) as f32 * 18.0, 54.0 + ((i - 9) / 9) as f32 * 18.0),
        36..=44 => (9.0 + (i - 36) as f32 * 18.0, 112.0),
        _ => return None,
    })
}

fn inside(m: (f32, f32), x: f32, y: f32, w: f32, h: f32) -> bool {
    m.0 >= x && m.0 < x + w && m.1 >= y && m.1 < y + h
}

impl Screen {
    /// Draws the screen and handles this frame's clicks; `inv` is updated right away.
    pub fn frame(&mut self, p: &mut Painter, tabs: &[Tab], inv: &mut Inventory, input: &Input, max: impl Fn(u32) -> u32) -> Vec<Out> {
        let mut out = Vec::new();
        p.gradient(0.0, 0.0, p.width, p.height, [16, 16, 16, 192], [16, 16, 16, 208]);
        let (x0, y0) = (((p.width - W) / 2.0).floor(), ((p.height - H) / 2.0).floor());
        let m = input.mouse;
        let tab = &tabs[self.tab.min(tabs.len() - 1)];

        // Search: type to filter by display name.
        let items: Vec<u32> = if tab.kind == Kind::Search {
            if !input.typed.is_empty() {
                self.search.extend(input.typed.chars().filter(|c| !c.is_control()));
                self.scroll = 0.0;
            }
            if input.pressed(KeyCode::Backspace) {
                self.search.pop();
                self.scroll = 0.0;
            }
            let q = self.search.to_lowercase();
            tab.items.iter().copied().filter(|i| q.is_empty() || name_of(p.models, *i).to_lowercase().contains(&q)).collect()
        } else {
            tab.items.clone()
        };
        let rows = items.len().div_ceil(9);
        let max_rows = rows.saturating_sub(5);

        // Unselected tabs behind the panel, the selected one in front.
        let tab_rect = |t: &Tab| {
            let x = if t.column == 6 { W - 27.0 + 1.0 } else { 27.0 * t.column as f32 };
            (x0 + x, if t.top { y0 - 28.0 } else { y0 + H - 4.0 })
        };
        let draw_tab = |p: &mut Painter, t: &Tab, selected: bool| {
            let (x, y) = tab_rect(t);
            let sprite = format!("ctab_{}_{}_{}", if t.top { "top" } else { "bottom" }, if selected { "selected" } else { "unselected" }, t.column + 1);
            p.sprite_rect(&sprite, x, y, 26.0, 32.0, hud::WHITE);
            p.item_icon(t.icon, x + 5.0, y + 8.0 + if t.top { 1.0 } else { -1.0 });
        };
        for (i, t) in tabs.iter().enumerate() {
            if i != self.tab {
                draw_tab(p, t, false);
            }
        }
        let bg = match tab.kind {
            Kind::Items => "creative_items",
            Kind::Search => "creative_search",
            Kind::Inventory => "creative_inventory",
        };
        p.sprite(bg, x0, y0);
        draw_tab(p, tab, true);
        if tab.kind == Kind::Search {
            let blink = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() / 300) % 2 == 0;
            p.text(&format!("{}{}", self.search, if blink { "_" } else { "" }), x0 + 82.0, y0 + 6.0, hud::WHITE);
        } else {
            p.text_flat(&tab.title, x0 + 8.0, y0 + 6.0, hud::DARK);
        }

        let mut hovered_item: Option<u32> = None;
        let mut hovered_slot: Option<usize> = None;
        if tab.kind == Kind::Inventory {
            for i in 0..inv.slots.len() {
                let Some((sx, sy)) = inventory_tab_pos(i) else { continue };
                let (x, y) = (x0 + sx, y0 + sy);
                if let Some(s) = &inv.slots[i] {
                    p.item(s, x, y);
                }
                if inside(m, x - 1.0, y - 1.0, 18.0, 18.0) {
                    p.fill(x, y, 16.0, 16.0, [255, 255, 255, 128]);
                    hovered_slot = Some(i);
                }
            }
        } else {
            // Scrollbar: wheel or drag.
            if inside(m, x0, y0, W, H) && input.scroll != 0.0 && max_rows > 0 {
                self.scroll = (self.scroll - input.scroll / max_rows as f32).clamp(0.0, 1.0);
            }
            let (sx, sy) = (x0 + 175.0, y0 + 18.0);
            if input.clicked && inside(m, sx, sy, 12.0, 112.0) {
                self.dragging = true;
            }
            if !input.held {
                self.dragging = false;
            }
            if self.dragging && max_rows > 0 {
                self.scroll = ((m.1 - sy - 7.5) / (112.0 - 15.0)).clamp(0.0, 1.0);
            }
            let thumb = if max_rows > 0 { "creative_scroller" } else { "creative_scroller_disabled" };
            p.sprite(thumb, sx, sy + (112.0 - 15.0) * self.scroll);
            let first = (self.scroll * max_rows as f32).round() as usize * 9;
            for n in 0..45 {
                let Some(&item) = items.get(first + n) else { break };
                let (x, y) = (x0 + 9.0 + (n % 9) as f32 * 18.0, y0 + 18.0 + (n / 9) as f32 * 18.0);
                p.item_icon(item, x, y);
                if inside(m, x - 1.0, y - 1.0, 18.0, 18.0) {
                    p.fill(x, y, 16.0, 16.0, [255, 255, 255, 128]);
                    hovered_item = Some(item);
                }
            }
            for c in 0..9 {
                let (x, y) = (x0 + 9.0 + c as f32 * 18.0, y0 + 112.0);
                if let Some(s) = &inv.slots[36 + c] {
                    p.item(s, x, y);
                }
                if inside(m, x - 1.0, y - 1.0, 18.0, 18.0) {
                    p.fill(x, y, 16.0, 16.0, [255, 255, 255, 128]);
                    hovered_slot = Some(36 + c);
                }
            }
        }
        let on_destroy = tab.kind == Kind::Inventory && inside(m, x0 + 172.0, y0 + 111.0, 18.0, 18.0);

        // Clicks.
        if input.clicked || input.right_clicked {
            let right = input.right_clicked;
            if let Some(i) = tabs.iter().position(|t| {
                let (x, y) = tab_rect(t);
                inside(m, x, y, 26.0, 32.0)
            }) {
                if i != self.tab {
                    self.tab = i;
                    self.scroll = 0.0;
                }
            } else if let Some(item) = hovered_item {
                let full = max(item);
                match &mut self.carried {
                    Some(c) if c.item == item && c.name.is_none() && c.enchantments.is_empty() => {
                        if right {
                            c.count -= 1;
                            if c.count == 0 {
                                self.carried = None;
                            }
                        } else {
                            c.count = if input.shift { full } else { (c.count + 1).min(full) };
                        }
                    }
                    None => self.carried = Some(plain(item, if input.shift { full } else { 1 })),
                    Some(_) if !right => self.carried = None,
                    Some(c) => {
                        c.count -= 1;
                        if c.count == 0 {
                            self.carried = None;
                        }
                    }
                }
            } else if let Some(i) = hovered_slot {
                let slot = &mut inv.slots[i];
                let full = |s: &Stack| max(s.item);
                match (self.carried.take(), slot.take()) {
                    (None, Some(mut s)) if right => {
                        let half = s.count.div_ceil(2);
                        let mut taken = s.clone();
                        taken.count = half;
                        s.count -= half;
                        *slot = (s.count > 0).then_some(s);
                        self.carried = Some(taken);
                    }
                    (None, s) => self.carried = s,
                    (Some(mut c), None) if right => {
                        let mut one = c.clone();
                        one.count = 1;
                        c.count -= 1;
                        *slot = Some(one);
                        self.carried = (c.count > 0).then_some(c);
                    }
                    (Some(c), None) => *slot = Some(c),
                    (Some(mut c), Some(mut s)) if c.item == s.item && c.enchantments == s.enchantments && c.damage == s.damage && c.name == s.name => {
                        let room = full(&s).saturating_sub(s.count);
                        let moved = if right { 1.min(room) } else { c.count.min(room) };
                        s.count += moved;
                        c.count -= moved;
                        *slot = Some(s);
                        self.carried = (c.count > 0).then_some(c);
                    }
                    (Some(c), Some(s)) => {
                        *slot = Some(c);
                        self.carried = Some(s);
                    }
                }
                out.push(Out::SetSlot(i as i16, inv.slots[i].clone()));
            } else if on_destroy {
                if input.shift {
                    // Shift-click on the delete slot clears the whole inventory.
                    for i in 5..inv.slots.len() {
                        if inv.slots[i].take().is_some() {
                            out.push(Out::SetSlot(i as i16, None));
                        }
                    }
                }
                self.carried = None;
            } else if !inside(m, x0, y0, W, H) {
                if let Some(mut c) = self.carried.take() {
                    if right && c.count > 1 {
                        let mut one = c.clone();
                        one.count = 1;
                        c.count -= 1;
                        out.push(Out::Drop(one));
                        self.carried = Some(c);
                    } else {
                        out.push(Out::Drop(c));
                    }
                }
            }
        }
        // Number keys put a full stack of the hovered grid item in that hotbar slot.
        if let Some(item) = hovered_item {
            let digits = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9];
            for (n, d) in digits.iter().enumerate() {
                if input.pressed(*d) && tab.kind != Kind::Search {
                    let s = plain(item, max(item));
                    inv.slots[36 + n] = Some(s.clone());
                    out.push(Out::SetSlot(36 + n as i16, Some(s)));
                }
            }
        }

        match &self.carried {
            Some(c) => p.item(c, m.0 - 8.0, m.1 - 8.0),
            None => {
                if let Some(item) = hovered_item {
                    p.tooltip(&[name_of(p.models, item)], m.0, m.1);
                } else if let Some(s) = hovered_slot.and_then(|i| inv.slots[i].as_ref()) {
                    p.tooltip(&[hud::item_name(p.models, s)], m.0, m.1);
                } else if let Some(t) = tabs.iter().find(|t| {
                    let (x, y) = tab_rect(t);
                    inside(m, x + 3.0, y + 3.0, 21.0, 27.0)
                }) {
                    p.tooltip(&[t.title.clone()], m.0, m.1);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_sizes_follow_item_kinds() {
        assert_eq!(max_stack("diamond_pickaxe"), 1);
        assert_eq!(max_stack("water_bucket"), 1);
        assert_eq!(max_stack("bucket"), 16);
        assert_eq!(max_stack("oak_sign"), 16);
        assert_eq!(max_stack("stone"), 64);
        assert_eq!(max_stack("red_bed"), 1);
        assert_eq!(max_stack("creeper_spawn_egg"), 64);
        assert_eq!(max_stack("egg"), 16);
    }

    #[test]
    fn tab_data_has_every_tab() {
        for key in ["buildingBlocks", "coloredBlocks", "natural", "functional", "redstone", "tools", "combat", "foodAndDrink", "ingredients", "spawnEggs"] {
            assert!(mc_assets::CREATIVE_TABS.lines().any(|l| l.starts_with(&format!("{key}\t"))), "{key}");
        }
    }
}
