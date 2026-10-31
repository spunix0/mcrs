//! GUI textures (HUD, widgets, container backgrounds, mob skins) packed into one atlas, the ASCII bitmap font,
//! and the title screen panorama.

use crate::Jar;
use anyhow::Result;
use std::collections::HashMap;

/// A sprite in the GUI atlas: size in GUI pixels and its UV rectangle.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub w: f32,
    pub h: f32,
    pub uv: [f32; 4],
}

/// The six panorama faces side by side (`panorama_0` .. `panorama_5`), with mip levels for the blurred variant.
pub struct Panorama {
    pub width: u32,
    pub height: u32,
    /// Base level first.
    pub levels: Vec<Vec<u8>>,
}

pub struct GuiAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    sprites: HashMap<&'static str, Sprite>,
    font_origin: [u32; 2],
    /// Advance in pixels for each byte value of the ASCII font (glyph width + 1 spacing).
    pub advance: [u8; 256],
    pub panorama: Option<Panorama>,
    /// Title screen splash texts.
    pub splashes: Vec<String>,
}

/// `(name, path under textures/, crop to w x h from the top-left; 0 = whole image)`.
const SPRITES: &[(&str, &str, u32, u32)] = &[
    ("hotbar", "gui/sprites/hud/hotbar.png", 0, 0),
    ("hotbar_selection", "gui/sprites/hud/hotbar_selection.png", 0, 0),
    ("crosshair", "gui/sprites/hud/crosshair.png", 0, 0),
    ("attack_background", "gui/sprites/hud/crosshair_attack_indicator_background.png", 0, 0),
    ("attack_progress", "gui/sprites/hud/crosshair_attack_indicator_progress.png", 0, 0),
    ("font", "font/ascii.png", 0, 0),
    ("heart_container", "gui/sprites/hud/heart/container.png", 0, 0),
    ("heart_full", "gui/sprites/hud/heart/full.png", 0, 0),
    ("heart_half", "gui/sprites/hud/heart/half.png", 0, 0),
    ("heart_hardcore_full", "gui/sprites/hud/heart/hardcore_full.png", 0, 0),
    ("heart_hardcore_half", "gui/sprites/hud/heart/hardcore_half.png", 0, 0),
    ("heart_container_blinking", "gui/sprites/hud/heart/container_blinking.png", 0, 0),
    ("heart_container_hardcore", "gui/sprites/hud/heart/container_hardcore.png", 0, 0),
    ("heart_container_hardcore_blinking", "gui/sprites/hud/heart/container_hardcore_blinking.png", 0, 0),
    ("heart_full_blinking", "gui/sprites/hud/heart/full_blinking.png", 0, 0),
    ("heart_half_blinking", "gui/sprites/hud/heart/half_blinking.png", 0, 0),
    ("heart_hardcore_full_blinking", "gui/sprites/hud/heart/hardcore_full_blinking.png", 0, 0),
    ("heart_hardcore_half_blinking", "gui/sprites/hud/heart/hardcore_half_blinking.png", 0, 0),
    ("food_empty", "gui/sprites/hud/food_empty.png", 0, 0),
    ("food_full", "gui/sprites/hud/food_full.png", 0, 0),
    ("food_half", "gui/sprites/hud/food_half.png", 0, 0),
    ("armor_empty", "gui/sprites/hud/armor_empty.png", 0, 0),
    ("armor_full", "gui/sprites/hud/armor_full.png", 0, 0),
    ("armor_half", "gui/sprites/hud/armor_half.png", 0, 0),
    ("air", "gui/sprites/hud/air.png", 0, 0),
    ("air_bursting", "gui/sprites/hud/air_bursting.png", 0, 0),
    ("xp_background", "gui/sprites/hud/experience_bar_background.png", 0, 0),
    ("xp_progress", "gui/sprites/hud/experience_bar_progress.png", 0, 0),
    // Widgets (nine-sliced: buttons have a 3 px border, sliders and text fields 1 px).
    ("button", "gui/sprites/widget/button.png", 0, 0),
    ("button_highlighted", "gui/sprites/widget/button_highlighted.png", 0, 0),
    ("button_disabled", "gui/sprites/widget/button_disabled.png", 0, 0),
    ("slider", "gui/sprites/widget/slider.png", 0, 0),
    ("slider_highlighted", "gui/sprites/widget/slider_highlighted.png", 0, 0),
    ("slider_handle", "gui/sprites/widget/slider_handle.png", 0, 0),
    ("slider_handle_highlighted", "gui/sprites/widget/slider_handle_highlighted.png", 0, 0),
    ("text_field", "gui/sprites/widget/text_field.png", 0, 0),
    ("text_field_highlighted", "gui/sprites/widget/text_field_highlighted.png", 0, 0),
    ("checkbox", "gui/sprites/widget/checkbox.png", 0, 0),
    ("checkbox_highlighted", "gui/sprites/widget/checkbox_highlighted.png", 0, 0),
    ("checkbox_selected", "gui/sprites/widget/checkbox_selected.png", 0, 0),
    ("checkbox_selected_highlighted", "gui/sprites/widget/checkbox_selected_highlighted.png", 0, 0),
    ("scroller", "gui/sprites/widget/scroller.png", 0, 0),
    ("scroller_background", "gui/sprites/widget/scroller_background.png", 0, 0),
    ("tab", "gui/sprites/widget/tab.png", 0, 0),
    ("tab_selected", "gui/sprites/widget/tab_selected.png", 0, 0),
    ("tab_highlighted", "gui/sprites/widget/tab_highlighted.png", 0, 0),
    ("icon_language", "gui/sprites/icon/language.png", 0, 0),
    ("icon_accessibility", "gui/sprites/icon/accessibility.png", 0, 0),
    ("ping_1", "gui/sprites/icon/ping_1.png", 0, 0),
    ("ping_2", "gui/sprites/icon/ping_2.png", 0, 0),
    ("ping_3", "gui/sprites/icon/ping_3.png", 0, 0),
    ("ping_4", "gui/sprites/icon/ping_4.png", 0, 0),
    ("ping_5", "gui/sprites/icon/ping_5.png", 0, 0),
    ("ping_unknown", "gui/sprites/icon/ping_unknown.png", 0, 0),
    ("unreachable", "gui/sprites/server_list/unreachable.png", 0, 0),
    ("join", "gui/sprites/server_list/join.png", 0, 0),
    ("join_highlighted", "gui/sprites/server_list/join_highlighted.png", 0, 0),
    ("menu_background", "gui/menu_background.png", 0, 0),
    ("menu_list_background", "gui/menu_list_background.png", 0, 0),
    ("header_separator", "gui/header_separator.png", 0, 0),
    ("footer_separator", "gui/footer_separator.png", 0, 0),
    ("logo", "gui/title/minecraft.png", 0, 0),
    ("pack", "misc/unknown_pack.png", 0, 0),
    ("underwater", "misc/underwater.png", 0, 0),
    ("server_icon", "misc/unknown_server.png", 0, 0),
    // Container backgrounds (the 256x256 textures hold the panel in the top-left).
    ("inventory", "gui/container/inventory.png", 176, 166),
    ("generic_54", "gui/container/generic_54.png", 176, 222),
    ("crafting_table", "gui/container/crafting_table.png", 176, 166),
    ("furnace", "gui/container/furnace.png", 176, 166),
    ("blast_furnace", "gui/container/blast_furnace.png", 176, 166),
    ("smoker", "gui/container/smoker.png", 176, 166),
    ("hopper", "gui/container/hopper.png", 176, 133),
    ("dispenser", "gui/container/dispenser.png", 176, 166),
    ("shulker_box", "gui/container/shulker_box.png", 176, 166),
    ("creative_items", "gui/container/creative_inventory/tab_items.png", 195, 136),
    ("creative_search", "gui/container/creative_inventory/tab_item_search.png", 195, 136),
    ("creative_inventory", "gui/container/creative_inventory/tab_inventory.png", 195, 136),
    ("creative_scroller", "gui/sprites/container/creative_inventory/scroller.png", 0, 0),
    ("creative_scroller_disabled", "gui/sprites/container/creative_inventory/scroller_disabled.png", 0, 0),
    ("ctab_top_selected_1", "gui/sprites/container/creative_inventory/tab_top_selected_1.png", 0, 0),
    ("ctab_top_selected_2", "gui/sprites/container/creative_inventory/tab_top_selected_2.png", 0, 0),
    ("ctab_top_selected_3", "gui/sprites/container/creative_inventory/tab_top_selected_3.png", 0, 0),
    ("ctab_top_selected_4", "gui/sprites/container/creative_inventory/tab_top_selected_4.png", 0, 0),
    ("ctab_top_selected_5", "gui/sprites/container/creative_inventory/tab_top_selected_5.png", 0, 0),
    ("ctab_top_selected_6", "gui/sprites/container/creative_inventory/tab_top_selected_6.png", 0, 0),
    ("ctab_top_selected_7", "gui/sprites/container/creative_inventory/tab_top_selected_7.png", 0, 0),
    ("ctab_top_unselected_1", "gui/sprites/container/creative_inventory/tab_top_unselected_1.png", 0, 0),
    ("ctab_top_unselected_2", "gui/sprites/container/creative_inventory/tab_top_unselected_2.png", 0, 0),
    ("ctab_top_unselected_3", "gui/sprites/container/creative_inventory/tab_top_unselected_3.png", 0, 0),
    ("ctab_top_unselected_4", "gui/sprites/container/creative_inventory/tab_top_unselected_4.png", 0, 0),
    ("ctab_top_unselected_5", "gui/sprites/container/creative_inventory/tab_top_unselected_5.png", 0, 0),
    ("ctab_top_unselected_6", "gui/sprites/container/creative_inventory/tab_top_unselected_6.png", 0, 0),
    ("ctab_top_unselected_7", "gui/sprites/container/creative_inventory/tab_top_unselected_7.png", 0, 0),
    ("ctab_bottom_selected_1", "gui/sprites/container/creative_inventory/tab_bottom_selected_1.png", 0, 0),
    ("ctab_bottom_selected_2", "gui/sprites/container/creative_inventory/tab_bottom_selected_2.png", 0, 0),
    ("ctab_bottom_selected_3", "gui/sprites/container/creative_inventory/tab_bottom_selected_3.png", 0, 0),
    ("ctab_bottom_selected_4", "gui/sprites/container/creative_inventory/tab_bottom_selected_4.png", 0, 0),
    ("ctab_bottom_selected_5", "gui/sprites/container/creative_inventory/tab_bottom_selected_5.png", 0, 0),
    ("ctab_bottom_selected_6", "gui/sprites/container/creative_inventory/tab_bottom_selected_6.png", 0, 0),
    ("ctab_bottom_selected_7", "gui/sprites/container/creative_inventory/tab_bottom_selected_7.png", 0, 0),
    ("ctab_bottom_unselected_1", "gui/sprites/container/creative_inventory/tab_bottom_unselected_1.png", 0, 0),
    ("ctab_bottom_unselected_2", "gui/sprites/container/creative_inventory/tab_bottom_unselected_2.png", 0, 0),
    ("ctab_bottom_unselected_3", "gui/sprites/container/creative_inventory/tab_bottom_unselected_3.png", 0, 0),
    ("ctab_bottom_unselected_4", "gui/sprites/container/creative_inventory/tab_bottom_unselected_4.png", 0, 0),
    ("ctab_bottom_unselected_5", "gui/sprites/container/creative_inventory/tab_bottom_unselected_5.png", 0, 0),
    ("ctab_bottom_unselected_6", "gui/sprites/container/creative_inventory/tab_bottom_unselected_6.png", 0, 0),
    ("ctab_bottom_unselected_7", "gui/sprites/container/creative_inventory/tab_bottom_unselected_7.png", 0, 0),
    ("lit_progress", "gui/sprites/container/furnace/lit_progress.png", 0, 0),
    ("burn_progress", "gui/sprites/container/furnace/burn_progress.png", 0, 0),
    // Sky.
    ("sun", "environment/celestial/sun.png", 0, 0),
    ("moon_full_moon", "environment/celestial/moon/full_moon.png", 0, 0),
    ("moon_waning_gibbous", "environment/celestial/moon/waning_gibbous.png", 0, 0),
    ("moon_third_quarter", "environment/celestial/moon/third_quarter.png", 0, 0),
    ("moon_waning_crescent", "environment/celestial/moon/waning_crescent.png", 0, 0),
    ("moon_new_moon", "environment/celestial/moon/new_moon.png", 0, 0),
    ("moon_waxing_crescent", "environment/celestial/moon/waxing_crescent.png", 0, 0),
    ("moon_first_quarter", "environment/celestial/moon/first_quarter.png", 0, 0),
    ("moon_waxing_gibbous", "environment/celestial/moon/waxing_gibbous.png", 0, 0),
    // Mob skins.
    ("steve", "entity/player/wide/steve.png", 0, 0),
    ("zombie", "entity/zombie/zombie.png", 0, 0),
    ("husk", "entity/zombie/husk.png", 0, 0),
    ("drowned", "entity/zombie/drowned.png", 0, 0),
    ("skeleton", "entity/skeleton/skeleton.png", 0, 0),
    ("stray", "entity/skeleton/stray.png", 0, 0),
    ("wither_skeleton", "entity/skeleton/wither_skeleton.png", 0, 0),
    ("pig", "entity/pig/pig_temperate.png", 0, 0),
    ("cow", "entity/cow/cow_temperate.png", 0, 0),
    ("mooshroom", "entity/cow/mooshroom_red.png", 0, 0),
    ("sheep", "entity/sheep/sheep.png", 0, 0),
    ("sheep_wool", "entity/sheep/sheep_wool.png", 0, 0),
    ("chicken", "entity/chicken/chicken_temperate.png", 0, 0),
    ("creeper", "entity/creeper/creeper.png", 0, 0),
    ("spider", "entity/spider/spider.png", 0, 0),
    ("cave_spider", "entity/spider/cave_spider.png", 0, 0),
    ("enderman", "entity/enderman/enderman.png", 0, 0),
    ("villager", "entity/villager/villager.png", 0, 0),
    ("wandering_trader", "entity/wandering_trader/wandering_trader.png", 0, 0),
    ("witch", "entity/witch/witch.png", 0, 0),
    ("wolf", "entity/wolf/wolf.png", 0, 0),
    ("squid", "entity/squid/squid.png", 0, 0),
    ("glow_squid", "entity/squid/glow_squid.png", 0, 0),
    ("slime", "entity/slime/slime.png", 0, 0),
    ("iron_golem", "entity/iron_golem/iron_golem.png", 0, 0),
    ("experience_orb", "entity/experience/experience_orb.png", 0, 0),
];

const SIZE: u32 = 2048;

/// Halves an RGBA image (2x2 box filter).
fn half(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let px = |dx: u32, dy: u32| src[((((y * 2 + dy).min(h - 1)) * w + (x * 2 + dx).min(w - 1)) * 4 + c) as usize] as u32;
                out[((y * nw + x) * 4 + c) as usize] = ((px(0, 0) + px(1, 0) + px(0, 1) + px(1, 1)) / 4) as u8;
            }
        }
    }
    out
}

fn load_panorama(assets: &dyn Fn(&str) -> Option<Vec<u8>>) -> Option<Panorama> {
    let faces: Vec<image::RgbaImage> = (0..6)
        .map(|i| {
            // The jar only holds 1x1 placeholders; the real images come with the downloaded assets.
            let bytes = assets(&format!("minecraft/textures/gui/title/background/panorama_{i}.png"))?;
            Some(image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8())
        })
        .collect::<Option<_>>()?;
    // Square power-of-two faces keep every mip level aligned to face borders.
    let side = faces[0].width().min(1024);
    let (w, h) = (side * 6, side);
    let mut base = vec![0u8; (w * h * 4) as usize];
    for (i, f) in faces.iter().enumerate() {
        let f = image::imageops::resize(f, side, side, image::imageops::FilterType::Triangle);
        for y in 0..side {
            let row = &f.as_raw()[(y * side * 4) as usize..((y + 1) * side * 4) as usize];
            let at = ((y * w + i as u32 * side) * 4) as usize;
            base[at..at + row.len()].copy_from_slice(row);
        }
    }
    let mut levels = vec![base];
    let (mut lw, mut lh) = (w, h);
    while lh > 1 && levels.len() < 6 {
        let next = half(levels.last().unwrap(), lw, lh);
        levels.push(next);
        (lw, lh) = ((lw / 2).max(1), (lh / 2).max(1));
    }
    Some(Panorama { width: w, height: h, levels })
}

impl GuiAtlas {
    /// `assets` reads a file from the downloaded asset index by name (`minecraft/textures/...`).
    pub fn load(jar: &mut Jar, assets: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Self> {
        let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
        let mut sprites = HashMap::new();
        let (mut x, mut y, mut row_h) = (8u32, 2u32, 4u32);
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
            let Some(bytes) = jar.read(&format!("assets/minecraft/textures/{path}")) else {
                eprintln!("GUI texture missing: {path}");
                continue;
            };
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
            if name == "logo" {
                // LogoRenderer draws the top 44/64 of the texture at 256x44, whatever its resolution.
                let s = sprites.get_mut("logo").unwrap();
                s.uv[3] = s.uv[1] + (s.uv[3] - s.uv[1]) * 44.0 / 64.0;
                (s.w, s.h) = (256.0, 44.0);
            }
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
        let splashes = jar
            .read("assets/minecraft/texts/splashes.txt")
            .map(|b| String::from_utf8_lossy(&b).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
            .unwrap_or_default();
        let panorama = load_panorama(assets);
        Ok(Self { width: SIZE, height: SIZE, rgba, sprites, font_origin, advance, panorama, splashes })
    }

    pub fn sprite(&self, name: &str) -> Sprite {
        self.sprites.get(name).copied().unwrap_or(self.sprites["white"])
    }

    pub fn has(&self, name: &str) -> bool {
        self.sprites.contains_key(name)
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

    /// Width in GUI pixels of a string drawn with [`GuiAtlas::glyph`] (formatting codes take no space).
    pub fn text_width(&self, s: &str) -> f32 {
        let mut w = 0.0;
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\u{a7}' {
                chars.next();
                continue;
            }
            let b = if c.is_ascii() { c as u8 } else { b'?' };
            w += self.advance[b as usize] as f32;
        }
        (w - 1.0f32).max(0.0)
    }
}
