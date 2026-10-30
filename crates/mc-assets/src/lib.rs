//! Loading the user's own game assets (jar + asset index): block-state and model JSON, texture atlas stitching, language files, sounds.
//!
//! [`BlockModels::load`] turns every block state into baked quads (unit-cube space, atlas UVs) the mesher can copy straight into vertex buffers.

mod atlas;
mod gui;
mod model;
mod tools;

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

pub use atlas::{Animation, Atlas};
pub use gui::{GuiAtlas, Panorama, Sprite};
pub use model::Transform;
pub use tools::{Mineable, Tool, ToolKind, tool_for_item};

/// Creative inventory tabs from vanilla's `CreativeModeTabs` (`tools/creative_tabs.pl`): tab key, kind, name.
pub const CREATIVE_TABS: &str = include_str!("../data/creative_tabs.tsv");
use tools::{ToolRules, shears_speed};

/// Face directions in vanilla order: down, up, north, south, west, east.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    pub const ALL: [Dir; 6] = [Dir::Down, Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East];

    pub fn offset(self) -> [i32; 3] {
        match self {
            Dir::Down => [0, -1, 0],
            Dir::Up => [0, 1, 0],
            Dir::North => [0, 0, -1],
            Dir::South => [0, 0, 1],
            Dir::West => [-1, 0, 0],
            Dir::East => [1, 0, 0],
        }
    }

    fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "down" | "bottom" => Dir::Down,
            "up" | "top" => Dir::Up,
            "north" => Dir::North,
            "south" => Dir::South,
            "west" => Dir::West,
            "east" => Dir::East,
            _ => return None,
        })
    }

    fn from_normal(n: [f32; 3]) -> Dir {
        let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
        if ay >= ax && ay >= az {
            if n[1] > 0.0 { Dir::Up } else { Dir::Down }
        } else if ax >= az {
            if n[0] > 0.0 { Dir::East } else { Dir::West }
        } else if n[2] > 0.0 {
            Dir::South
        } else {
            Dir::North
        }
    }

    /// Vanilla's fixed directional shading.
    pub fn shade(self) -> f32 {
        match self {
            Dir::Down => 0.5,
            Dir::Up => 1.0,
            Dir::North | Dir::South => 0.8,
            Dir::West | Dir::East => 0.6,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Quad {
    /// Corners in block space (0..1), counter-clockwise seen from outside.
    pub pos: [[f32; 3]; 4],
    /// Atlas UVs for each corner.
    pub uv: [[f32; 2]; 4],
    /// Skip this quad when the neighbor in this direction is an opaque full block.
    pub cull: Option<Dir>,
    pub shade: f32,
    pub tinted: bool,
    pub translucent: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fluid {
    Water,
    Lava,
}

#[derive(Clone)]
pub struct StateModel {
    pub quads: Arc<Vec<Quad>>,
    /// Hides neighbor faces that point at it.
    pub opaque: bool,
    pub fluid: Option<Fluid>,
    /// The `level` property of water and lava: 0 source, 1..7 flowing, 8+ falling.
    pub fluid_level: u8,
    pub tint: [u8; 3],
    /// Collision boxes in block units (`[min x, y, z, max x, y, z]`). Approximated from model elements, since
    /// vanilla defines shapes in code; see [`collision_for`].
    pub collision: Arc<Vec<[f32; 6]>>,
    /// Ground slipperiness (0.6 for most blocks).
    pub friction: f32,
    /// Light the block gives off, 0..15.
    pub light: u8,
    /// How much light it takes away passing through (vanilla `getLightBlock`): 15 opaque, 1 water and leaves.
    pub light_block: u8,
    /// Mining hardness (vanilla `destroyTime`); negative means unbreakable.
    pub hardness: f32,
    /// Drops only with the right tool; bare hands then mine 3.3x slower.
    pub needs_tool: bool,
    pub mineable: Mineable,
    /// Block name without `minecraft:` (shared by all states of the block).
    pub block: Arc<str>,
    /// Sound set used for breaking, placing, stepping (e.g. `stone`, `wood`, `grass`).
    pub sound: Arc<str>,
}

/// An entity type: registry name, hitbox and spawn category.
#[derive(Clone, Debug)]
pub struct EntityInfo {
    pub name: String,
    pub width: f32,
    pub height: f32,
    /// `monster`, `creature`, `misc`, ...
    pub category: String,
}

fn entity_infos(names: Vec<String>) -> Vec<EntityInfo> {
    let table: HashMap<&str, (f32, f32, &str)> = include_str!("../data/entity_sizes.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Some((*f.first()?, (f.get(1)?.parse().ok()?, f.get(2)?.parse().ok()?, *f.get(3)?)))
        })
        .collect();
    names
        .into_iter()
        .map(|name| {
            let (width, height, category) = table.get(name.as_str()).copied().unwrap_or((0.6, 1.8, "misc"));
            EntityInfo { name, width, height, category: category.to_string() }
        })
        .collect()
}

/// The sounds a block makes (vanilla `SoundType`).
#[derive(Clone, Debug)]
pub struct SoundSet {
    pub volume: f32,
    pub pitch: f32,
    pub break_: String,
    pub step: String,
    pub place: String,
    pub hit: String,
    pub fall: String,
}

/// `block -> sound type`, extracted from Blocks.java by `tools/block_sounds.pl`.
fn block_sound(block: &str) -> &'static str {
    static TABLE: std::sync::OnceLock<HashMap<&'static str, &'static str>> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        include_str!("../data/block_sounds.tsv").lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once('\t')).collect()
    });
    t.get(block).copied().unwrap_or("stone")
}

fn sound_types() -> HashMap<String, SoundSet> {
    include_str!("../data/sound_types.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let [name, volume, pitch, brk, step, place, hit, fall] = f.as_slice() else { return None };
            let own = |x: &str| x.to_string();
            Some((
                own(name),
                SoundSet {
                    volume: volume.parse().ok()?,
                    pitch: pitch.parse().ok()?,
                    break_: own(brk),
                    step: own(step),
                    place: own(place),
                    hit: own(hit),
                    fall: own(fall),
                },
            ))
        })
        .collect()
}

/// Light a block state gives off (vanilla `lightEmission`), from the rules `tools/light.pl` extracted from Blocks.java.
fn light_emission(table: &HashMap<&str, &str>, block: &str, props: &[(String, String)]) -> u8 {
    let prop = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let num = |k: &str| prop(k).and_then(|v| v.parse::<u8>().ok()).unwrap_or(0);
    let lit = prop("lit") == Some("true");
    // Blocks registered in loops the extractor can't follow.
    let rule = table.get(block).copied().or_else(|| {
        Some(if block.ends_with("_candle_cake") {
            "lit:3"
        } else if block.ends_with("_candle") {
            "candle"
        } else if block.contains("copper_lantern") {
            "15"
        } else if block.ends_with("copper_bulb") {
            if block.contains("oxidized") { "lit:4" } else if block.contains("weathered") { "lit:8" } else if block.contains("exposed") { "lit:12" } else { "lit:15" }
        } else {
            return None;
        })
    });
    match rule {
        None => 0,
        Some("candle") => if lit { 3 * num("candles") } else { 0 },
        Some("level") => num("level"),
        Some("pickle") => if prop("waterlogged") == Some("true") { 3 + 3 * num("pickles") } else { 0 },
        Some("anchor") => [0, 3, 7, 11, 15][num("charges").min(4) as usize],
        Some(r) => match r.split_once(':') {
            Some(("lit", n)) => if lit { n.parse().unwrap_or(0) } else { 0 },
            Some(("berries", n)) => if prop("berries") == Some("true") { n.parse().unwrap_or(0) } else { 0 },
            _ => r.parse().unwrap_or(0),
        },
    }
}

/// `block -> (hardness, needs correct tool)`, extracted from 26.3's Blocks.java by `tools/hardness.pl`.
fn hardness_table() -> HashMap<&'static str, (f32, bool)> {
    include_str!("../data/hardness.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some((f.next()?, (f.next()?.parse().ok()?, f.next()? == "1")))
        })
        .collect()
}

impl StateModel {
    /// Ticks to mine holding `tool` (vanilla `getDestroyProgress` / `Player.getDestroySpeed`),
    /// or `None` if it can't be mined. `efficiency` is the tool's Efficiency level (adds level squared + 1 to a
    /// speed above 1, like the mining efficiency attribute); potion effects aren't counted yet.
    pub fn break_ticks(&self, tool: Option<&Tool>, on_ground: bool, efficiency: u32) -> Option<u32> {
        if self.hardness < 0.0 {
            return None;
        }
        if self.hardness == 0.0 {
            return Some(0);
        }
        let (mut speed, correct) = match tool {
            Some(t) if t.kind == ToolKind::Shears => {
                let s = shears_speed(&self.block);
                (s, s > 1.0)
            }
            Some(t) if self.mineable.by == Some(t.kind) => (t.speed, t.tier.is_some_and(|i| self.mineable.correct_tiers & (1 << i) != 0)),
            _ => (1.0, false),
        };
        if speed > 1.0 && efficiency > 0 {
            speed += (efficiency * efficiency + 1) as f32;
        }
        if !on_ground {
            speed /= 5.0;
        }
        let divisor = if self.needs_tool && !correct { 100.0 } else { 30.0 };
        let per_tick = speed / self.hardness / divisor;
        // Progress >= 1 in one tick is an instant break.
        Some(if per_tick >= 1.0 { 0 } else { (1.0 / per_tick).ceil() as u32 })
    }
}

/// Blocks you can walk through even though their model has solid-looking boxes.
const NO_COLLISION: &[&str] = &[
    "torch", "sign", "banner", "button", "pressure_plate", "lever", "fire", "tripwire", "portal", "light", "structure_void", "powder_snow",
];

fn collision_for(block: &str, boxes: Vec<[f32; 6]>) -> Vec<[f32; 6]> {
    if NO_COLLISION.iter().any(|n| block.contains(n)) {
        return Vec::new();
    }
    // Fences and walls (posts and arms alike) collide as 1.5-block-tall columns so you can't jump them.
    let tall = block.ends_with("_fence") || block.ends_with("_wall");
    boxes
        .into_iter()
        .map(|mut b| {
            if tall {
                b[1] = 0.0;
                b[4] = 1.5;
            }
            b
        })
        .collect()
}

fn friction_for(block: &str) -> f32 {
    match block {
        "ice" | "packed_ice" | "frosted_ice" => 0.98,
        "blue_ice" => 0.989,
        "slime_block" => 0.8,
        _ => 0.6,
    }
}

/// An item as the client draws it.
#[derive(Clone)]
pub struct ItemInfo {
    pub name: String,
    /// Atlas UVs of its flat icon, if one was found.
    pub icon: Option<[f32; 4]>,
    /// Grass/foliage tint for icons whose texture is grayscale (leaves, grass, vines).
    pub tint: Option<[u8; 3]>,
    pub tool: Option<Tool>,
    /// Drawn as a 3D block model in hand and in inventories (block items) rather than a flat sprite.
    pub block: bool,
    /// 3D geometry in model space (0..1): the block model, or the icon extruded into a 1/16 thick slab.
    pub model: Option<Arc<Vec<Quad>>>,
    /// Grass/foliage color for tinted model faces.
    pub model_tint: [u8; 3],
    /// Display transforms for first-person right hand, inventory slots and dropped items.
    pub hand: Transform,
    pub gui: Transform,
    pub ground: Transform,
}

/// Turns a flat item sprite into vanilla's generated model: front and back faces plus a 1-pixel side wall
/// wherever an opaque pixel borders a transparent one.
fn extrude(atlas: &Atlas, rect: [f32; 4]) -> Vec<Quad> {
    let (w, h) = (atlas.width as usize, atlas.height as usize);
    let (x0, y0) = ((rect[0] * w as f32).round() as usize, (rect[1] * h as f32).round() as usize);
    let n = (((rect[2] - rect[0]) * w as f32).round() as usize).max(1);
    let opaque = |x: i32, y: i32| {
        x >= 0 && y >= 0 && (x as usize) < n && (y as usize) < n && atlas.rgba[((y0 + y as usize) * w + x0 + x as usize) * 4 + 3] > 0
    };
    let [u0, v0, u1, v1] = rect;
    let (z0, z1) = (7.5 / 16.0, 8.5 / 16.0);
    let quad = |pos: [[f32; 3]; 4], uv: [[f32; 2]; 4], shade: f32| Quad { pos, uv, cull: None, shade, tinted: false, translucent: false };
    let mut out = vec![
        quad([[0.0, 1.0, z1], [0.0, 0.0, z1], [1.0, 0.0, z1], [1.0, 1.0, z1]], [[u0, v0], [u0, v1], [u1, v1], [u1, v0]], 0.8),
        quad([[1.0, 1.0, z0], [1.0, 0.0, z0], [0.0, 0.0, z0], [0.0, 1.0, z0]], [[u1, v0], [u1, v1], [u0, v1], [u0, v0]], 0.8),
    ];
    let p = 1.0 / n as f32;
    let (du, dv) = ((u1 - u0) / n as f32, (v1 - v0) / n as f32);
    for y in 0..n as i32 {
        for x in 0..n as i32 {
            if !opaque(x, y) {
                continue;
            }
            // Sample just this pixel (inset a little so neighbors don't bleed in).
            let (pu, pv) = (u0 + du * (x as f32 + 0.1), v0 + dv * (y as f32 + 0.1));
            let (qu, qv) = (u0 + du * (x as f32 + 0.9), v0 + dv * (y as f32 + 0.9));
            let uv = [[pu, pv], [pu, qv], [qu, qv], [qu, pv]];
            let (mx0, mx1) = (x as f32 * p, (x + 1) as f32 * p);
            let (my0, my1) = (1.0 - (y + 1) as f32 * p, 1.0 - y as f32 * p);
            if !opaque(x, y - 1) {
                out.push(quad([[mx0, my1, z0], [mx0, my1, z1], [mx1, my1, z1], [mx1, my1, z0]], uv, 1.0));
            }
            if !opaque(x, y + 1) {
                out.push(quad([[mx0, my0, z1], [mx0, my0, z0], [mx1, my0, z0], [mx1, my0, z1]], uv, 0.5));
            }
            if !opaque(x - 1, y) {
                out.push(quad([[mx0, my1, z0], [mx0, my0, z0], [mx0, my0, z1], [mx0, my1, z1]], uv, 0.6));
            }
            if !opaque(x + 1, y) {
                out.push(quad([[mx1, my1, z1], [mx1, my0, z1], [mx1, my0, z0], [mx1, my1, z0]], uv, 0.6));
            }
        }
    }
    out
}

pub struct BlockModels {
    pub states: Vec<StateModel>,
    pub atlas: Atlas,
    pub water_uv: [f32; 4],
    pub lava_uv: [f32; 4],
    /// Flowing fluid textures (sides and sloped surfaces).
    pub water_flow_uv: [f32; 4],
    pub lava_flow_uv: [f32; 4],
    /// Block-breaking crack textures, stage 0 to 9.
    pub crack_uv: [[f32; 4]; 10],
    /// Particle animation frames: the explosion puff (16) and generic smoke (8).
    pub explosion_uv: Vec<[f32; 4]>,
    pub smoke_uv: Vec<[f32; 4]>,
    /// Indexed by item protocol ID.
    pub items: Vec<ItemInfo>,
    /// Data component type name (without `minecraft:`) to protocol ID.
    pub components: HashMap<String, u32>,
    pub gui: GuiAtlas,
    /// Sound type name (see [`StateModel::sound`]) to its sounds.
    pub sound_types: HashMap<String, SoundSet>,
    /// Sound event names (e.g. `entity.cow.ambient`) by protocol ID.
    pub sound_events: Vec<String>,
    /// Entity types by protocol ID.
    pub entities: Vec<EntityInfo>,
    /// English translations (`en_us.json`), e.g. `block.minecraft.stone` -> `Stone`.
    pub lang: HashMap<String, String>,
    /// Menu (container screen) types by protocol ID, e.g. `generic_9x3`.
    pub menus: Vec<String>,
}

/// `(registry name -> entries in protocol-ID order)` from the server's `reports/registries.json`.
fn load_registry(v: &Value, registry: &str) -> Vec<String> {
    let mut entries: Vec<(u64, String)> = v[registry]["entries"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, e)| Some((e["protocol_id"].as_u64()?, k.strip_prefix("minecraft:").unwrap_or(k).to_string())))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries.into_iter().map(|(_, n)| n).collect()
}

fn item_tint(item: &str) -> Option<[u8; 3]> {
    match item {
        "short_grass" | "tall_grass" | "fern" | "large_fern" | "sugar_cane" => Some(GRASS),
        "lily_pad" | "vine" | "birch_leaves" | "spruce_leaves" => Some(tint_for(item, &[])),
        i if i.ends_with("_leaves") && !matches!(i, "cherry_leaves" | "azalea_leaves" | "flowering_azalea_leaves" | "pale_oak_leaves") => {
            Some(FOLIAGE)
        }
        _ => None,
    }
}

/// Reads assets straight out of the client jar.
pub struct Jar {
    zip: zip::ZipArchive<std::io::Cursor<Vec<u8>>>,
}

impl Jar {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(Self { zip: zip::ZipArchive::new(std::io::Cursor::new(bytes))? })
    }

    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let mut f = self.zip.by_name(name).ok()?;
        let mut out = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut out).ok()?;
        Some(out)
    }

    pub fn json(&mut self, name: &str) -> Option<Value> {
        serde_json::from_slice(&self.read(name)?).ok()
    }
}

/// One entry from the server's `reports/blocks.json`.
struct StateInfo {
    block: String,
    props: Vec<(String, String)>,
}

fn load_states(blocks_json: &Path) -> Result<Vec<StateInfo>> {
    let v: Value = serde_json::from_slice(&fs::read(blocks_json).with_context(|| format!("reading {}", blocks_json.display()))?)?;
    let mut out: Vec<Option<StateInfo>> = Vec::new();
    for (name, block) in v.as_object().context("blocks.json is not an object")? {
        let short = name.strip_prefix("minecraft:").unwrap_or(name).to_string();
        for s in block["states"].as_array().context("missing states")? {
            let id = s["id"].as_u64().context("missing id")? as usize;
            let props = s["properties"]
                .as_object()
                .map(|p| p.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect())
                .unwrap_or_default();
            if out.len() <= id {
                out.resize_with(id + 1, || None);
            }
            out[id] = Some(StateInfo { block: short.clone(), props });
        }
    }
    Ok(out
        .into_iter()
        .map(|s| s.unwrap_or(StateInfo { block: "air".into(), props: vec![] }))
        .collect())
}

const GRASS: [u8; 3] = [0x91, 0xbd, 0x59];
const FOLIAGE: [u8; 3] = [0x77, 0xab, 0x2f];
const WATER: [u8; 3] = [0x3f, 0x76, 0xe4];

/// Plains-biome colors; real biome blending comes later.
/// Vanilla's `BlockColors`: only these blocks are tinted; every other tinted face stays white.
fn tint_for(block: &str, props: &[(String, String)]) -> [u8; 3] {
    let num = |k: &str| props.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.parse::<f32>().ok()).unwrap_or(0.0);
    match block {
        "grass_block" | "short_grass" | "tall_grass" | "fern" | "large_fern" | "potted_fern" | "bush" | "pink_petals" | "wildflowers" | "sugar_cane" => GRASS,
        "birch_leaves" => [0x80, 0xa7, 0x55],
        "spruce_leaves" => [0x61, 0x99, 0x61],
        "lily_pad" => [0x20, 0x80, 0x30],
        "oak_leaves" | "jungle_leaves" | "acacia_leaves" | "dark_oak_leaves" | "mangrove_leaves" | "vine" => FOLIAGE,
        "leaf_litter" => [0x5c, 0x3c, 0x32],
        "water" | "bubble_column" | "water_cauldron" => WATER,
        "attached_melon_stem" | "attached_pumpkin_stem" => [0xe0, 0xc7, 0x1c],
        "melon_stem" | "pumpkin_stem" => {
            let age = num("age");
            [(age * 32.0) as u8, (255.0 - age * 8.0) as u8, (age * 4.0) as u8]
        }
        "redstone_wire" => {
            // RedStoneWireBlock.getColorForPower.
            let f = num("power") / 15.0;
            let r = f * 0.6 + if f > 0.0 { 0.4 } else { 0.3 };
            let g = (f * f * 0.7 - 0.5).clamp(0.0, 1.0);
            let b = (f * f * 0.6 - 0.7).clamp(0.0, 1.0);
            [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
        }
        _ => [255; 3],
    }
}

impl BlockModels {
    /// `reports` is the server data generator's `reports` folder (`blocks.json`, `registries.json`).
    pub fn load(jar_path: &Path, reports: &Path) -> Result<Self> {
        let states = load_states(&reports.join("blocks.json"))?;
        let registries: Value = serde_json::from_slice(&fs::read(reports.join("registries.json")).context("reading registries.json")?)?;
        let item_names = load_registry(&registries, "minecraft:item");
        let sound_events = load_registry(&registries, "minecraft:sound_event");
        let entities = entity_infos(load_registry(&registries, "minecraft:entity_type"));
        let menus = load_registry(&registries, "minecraft:menu");
        let components = load_registry(&registries, "minecraft:data_component_type")
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, i as u32))
            .collect();
        let mut jar = Jar::open(jar_path)?;
        let mut baker = model::Baker::new();
        let mut blockstate_cache: HashMap<String, Option<Value>> = HashMap::new();

        // First pass: resolve which models each state uses (this also registers every texture).
        let mut resolved = Vec::with_capacity(states.len());
        for s in &states {
            let def = blockstate_cache
                .entry(s.block.clone())
                .or_insert_with(|| jar.json(&format!("assets/minecraft/blockstates/{}.json", s.block)))
                .clone();
            let parts = def.as_ref().map(|d| model::select(d, &s.props)).unwrap_or_default();
            let baked: Vec<_> = parts.iter().map(|p| baker.bake(&mut jar, p)).collect();
            resolved.push(baked);
        }
        let item_defs: Vec<Option<model::ItemModelDef>> = item_names.iter().map(|n| baker.item_model(&mut jar, n)).collect();
        let atlas = baker.stitch(&mut jar)?;
        let items = item_names
            .iter()
            .zip(item_defs)
            .map(|(name, def)| {
                let icon = def.as_ref().and_then(|d| d.icon.as_ref()).map(|t| atlas.rect(t));
                let model = def.as_ref().and_then(|d| match &d.elements_model {
                    Some(m) => {
                        let part = baker.bake(&mut jar, &model::VariantRef { model: m.clone(), x: 0, y: 0 });
                        Some(Arc::new(part.quads.iter().map(|q| q.finish(&atlas)).collect::<Vec<_>>()))
                    }
                    None => icon.map(|r| Arc::new(extrude(&atlas, r))),
                });
                let display = |k: &str| def.as_ref().and_then(|d| d.display.get(k).copied()).unwrap_or_default();
                ItemInfo {
                    name: name.clone(),
                    icon,
                    block: def.as_ref().is_some_and(|d| d.elements_model.is_some()),
                    model,
                    model_tint: tint_for(name, &[]),
                    hand: display("firstperson_righthand"),
                    gui: display("gui"),
                    ground: display("ground"),
                    tint: item_tint(name),
                    tool: tool_for_item(name),
                }
            })
            .collect();
        let gui = GuiAtlas::load(&mut jar, &asset_reader(jar_path))?;
        let lang: HashMap<String, String> = jar
            .json("assets/minecraft/lang/en_us.json")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let water_uv = atlas.rect("minecraft:block/water_still");
        let lava_uv = atlas.rect("minecraft:block/lava_still");
        let water_flow_uv = atlas.rect("minecraft:block/water_flow");
        let lava_flow_uv = atlas.rect("minecraft:block/lava_flow");
        let crack_uv = std::array::from_fn(|i| atlas.rect(&format!("minecraft:block/destroy_stage_{i}")));
        let explosion_uv = (0..16).map(|i| atlas.rect(&format!("minecraft:particle/explosion_{i}"))).collect();
        let smoke_uv = (0..8).map(|i| atlas.rect(&format!("minecraft:particle/generic_{i}"))).collect();

        let hardness = hardness_table();
        let light: HashMap<&str, &str> = include_str!("../data/light.tsv").lines().filter_map(|l| l.split_once('\t')).collect();
        let tools = ToolRules::load(&mut jar);
        let mut names: HashMap<String, Arc<str>> = HashMap::new();
        let mut sounds: HashMap<String, Arc<str>> = HashMap::new();
        let states = states
            .iter()
            .zip(resolved)
            .map(|(s, parts)| {
                let mut quads = Vec::new();
                let mut boxes = Vec::new();
                let mut opaque = false;
                for p in &parts {
                    // Pink petals and wildflowers: layer 0 (the flowers) is blank, layer 1 (the stems) grass.
                    let layered = matches!(s.block.as_str(), "pink_petals" | "wildflowers");
                    quads.extend(p.quads.iter().map(|q| Quad { tinted: q.tint.is_some() && (!layered || q.tint == Some(1)), ..q.finish(&atlas) }));
                    boxes.extend_from_slice(&p.boxes);
                    opaque |= p.full_cube && !p.textures.iter().any(|t| atlas.has_alpha(t));
                }
                let is_air = matches!(s.block.as_str(), "air" | "cave_air" | "void_air");
                let waterlogged = s.props.iter().any(|(k, v)| k == "waterlogged" && v == "true");
                let fluid = match s.block.as_str() {
                    "water" | "bubble_column" | "seagrass" | "tall_seagrass" | "kelp" | "kelp_plant" => Some(Fluid::Water),
                    "lava" => Some(Fluid::Lava),
                    _ if waterlogged => Some(Fluid::Water),
                    _ => None,
                };
                StateModel {
                    quads: Arc::new(if is_air { Vec::new() } else { quads }),
                    opaque: opaque && !is_air,
                    fluid,
                    fluid_level: s.props.iter().find(|(k, _)| k == "level").and_then(|(_, v)| v.parse().ok()).unwrap_or(0),
                    tint: tint_for(&s.block, &s.props),
                    collision: Arc::new(if is_air { Vec::new() } else { collision_for(&s.block, boxes) }),
                    friction: friction_for(&s.block),
                    light: light_emission(&light, &s.block, &s.props),
                    light_block: if opaque && !is_air || s.block == "tinted_glass" {
                        15
                    } else if fluid.is_some() || s.block.ends_with("_leaves") || matches!(s.block.as_str(), "ice" | "frosted_ice" | "slime_block" | "cobweb") {
                        1
                    } else {
                        0
                    },
                    hardness: hardness.get(s.block.as_str()).map_or(1.0, |h| h.0),
                    needs_tool: hardness.get(s.block.as_str()).is_some_and(|h| h.1),
                    mineable: tools.get(&s.block),
                    block: names.entry(s.block.clone()).or_insert_with(|| Arc::from(s.block.as_str())).clone(),
                    sound: sounds.entry(s.block.clone()).or_insert_with(|| Arc::from(block_sound(&s.block))).clone(),
                }
            })
            .collect();
        Ok(Self { states, atlas, water_uv, lava_uv, water_flow_uv, lava_flow_uv, crack_uv, explosion_uv, smoke_uv, items, components, gui, sound_types: sound_types(), sound_events, entities, lang, menus })
    }
}

/// Reads files from the downloaded asset index (`assets/indexes`, `assets/objects`) next to the version's jar
/// (`<data>/versions/<id>/<id>.jar`). Missing index means no assets.
fn asset_reader(jar_path: &Path) -> impl Fn(&str) -> Option<Vec<u8>> {
    let data = jar_path.parent().and_then(Path::parent).and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    let version = jar_path.with_extension("json");
    let objects: HashMap<String, String> = (|| {
        let v: Value = serde_json::from_slice(&fs::read(&version).ok()?).ok()?;
        let id = v["assetIndex"]["id"].as_str()?;
        let index: Value = serde_json::from_slice(&fs::read(data.join("assets/indexes").join(format!("{id}.json"))).ok()?).ok()?;
        Some(index["objects"].as_object()?.iter().filter_map(|(k, o)| Some((k.clone(), o["hash"].as_str()?.to_string()))).collect())
    })()
    .unwrap_or_default();
    move |name: &str| {
        let hash = objects.get(name)?;
        fs::read(data.join("assets/objects").join(&hash[..2]).join(hash)).ok()
    }
}

#[cfg(test)]
mod audit {
    use super::*;

    /// Every item must have something to draw that isn't the missing-texture checker. Uses the installed game
    /// (skipped when it isn't there); run with `--ignored --nocapture` to see the list.
    #[test]
    #[ignore = "62 block-entity items (chests, shulker boxes, heads, banners, shield, decorated pot, conduit, copper golem statues) have no model yet"]
    fn every_item_has_its_textures() {
        let data = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("mcrs").join("data");
        let jar = data.join("versions/26.3/26.3.jar");
        if !jar.exists() {
            return;
        }
        let m = BlockModels::load(&jar, &data.join("reports")).unwrap();
        let bad = m.atlas.rect("minecraft:missingno");
        let in_bad = |uv: [f32; 2]| uv[0] >= bad[0] && uv[0] <= bad[2] && uv[1] >= bad[1] && uv[1] <= bad[3];
        let mut problems = Vec::new();
        for item in &m.items {
            if item.name == "air" {
                continue;
            }
            let checker = item.model.as_ref().is_some_and(|q| q.iter().any(|q| q.uv.iter().all(|uv| in_bad(*uv))));
            let icon_bad = item.icon.is_some_and(|r| in_bad([r[0], r[1]]));
            if (item.icon.is_none() && item.model.is_none()) || checker || icon_bad {
                problems.push(format!("{} (icon {}, model {}{})", item.name, item.icon.is_some(), item.model.is_some(), if checker || icon_bad { ", checker" } else { "" }));
            }
        }
        println!("unreadable textures: {:?}", m.atlas.missing);
        println!("{} items with missing textures:\n{}", problems.len(), problems.join("\n"));
        assert!(problems.is_empty() && m.atlas.missing.is_empty());
    }
}
