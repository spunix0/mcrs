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

pub use atlas::Atlas;
pub use gui::{GuiAtlas, Sprite};
pub use tools::{Mineable, Tool, ToolKind, tool_for_item};
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
    pub tint: [u8; 3],
    /// Collision boxes in block units (`[min x, y, z, max x, y, z]`). Approximated from model elements, since
    /// vanilla defines shapes in code; see [`collision_for`].
    pub collision: Arc<Vec<[f32; 6]>>,
    /// Ground slipperiness (0.6 for most blocks).
    pub friction: f32,
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
    /// or `None` if it can't be mined. Efficiency and potion effects aren't counted yet.
    pub fn break_ticks(&self, tool: Option<&Tool>, on_ground: bool) -> Option<u32> {
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
    /// Drawn as a small cube in hand (block items) rather than a flat sprite.
    pub block: bool,
}

pub struct BlockModels {
    pub states: Vec<StateModel>,
    pub atlas: Atlas,
    pub water_uv: [f32; 4],
    pub lava_uv: [f32; 4],
    /// Block-breaking crack textures, stage 0 to 9.
    pub crack_uv: [[f32; 4]; 10],
    /// Indexed by item protocol ID.
    pub items: Vec<ItemInfo>,
    /// Data component type name (without `minecraft:`) to protocol ID.
    pub components: HashMap<String, u32>,
    pub gui: GuiAtlas,
    /// Sound type name (see [`StateModel::sound`]) to its sounds.
    pub sound_types: HashMap<String, SoundSet>,
    /// Sound event names (e.g. `entity.cow.ambient`) by protocol ID.
    pub sound_events: Vec<String>,
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
        "lily_pad" | "vine" | "birch_leaves" | "spruce_leaves" => Some(tint_for(item)),
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
fn tint_for(block: &str) -> [u8; 3] {
    match block {
        "birch_leaves" => [0x80, 0xa7, 0x55],
        "spruce_leaves" => [0x61, 0x99, 0x61],
        "lily_pad" => [0x20, 0x80, 0x30],
        "water" | "bubble_column" => WATER,
        b if b.ends_with("leaves") || b == "vine" => FOLIAGE,
        _ => GRASS,
    }
}

impl BlockModels {
    /// `reports` is the server data generator's `reports` folder (`blocks.json`, `registries.json`).
    pub fn load(jar_path: &Path, reports: &Path) -> Result<Self> {
        let states = load_states(&reports.join("blocks.json"))?;
        let registries: Value = serde_json::from_slice(&fs::read(reports.join("registries.json")).context("reading registries.json")?)?;
        let item_names = load_registry(&registries, "minecraft:item");
        let sound_events = load_registry(&registries, "minecraft:sound_event");
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
        let icon_textures: Vec<Option<(String, bool)>> = item_names.iter().map(|n| baker.item_icon(&mut jar, n)).collect();
        let atlas = baker.stitch(&mut jar)?;
        let items = item_names
            .iter()
            .zip(icon_textures)
            .map(|(name, tex)| ItemInfo {
                name: name.clone(),
                icon: tex.as_ref().map(|t| atlas.rect(&t.0)),
                block: tex.is_some_and(|t| t.1),
                tint: item_tint(name),
                tool: tool_for_item(name),
            })
            .collect();
        let gui = GuiAtlas::load(&mut jar)?;
        let water_uv = atlas.rect("minecraft:block/water_still");
        let lava_uv = atlas.rect("minecraft:block/lava_still");
        let crack_uv = std::array::from_fn(|i| atlas.rect(&format!("minecraft:block/destroy_stage_{i}")));

        let hardness = hardness_table();
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
                    quads.extend(p.quads.iter().map(|q| q.finish(&atlas)));
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
                    tint: tint_for(&s.block),
                    collision: Arc::new(if is_air { Vec::new() } else { collision_for(&s.block, boxes) }),
                    friction: friction_for(&s.block),
                    hardness: hardness.get(s.block.as_str()).map_or(1.0, |h| h.0),
                    needs_tool: hardness.get(s.block.as_str()).is_some_and(|h| h.1),
                    mineable: tools.get(&s.block),
                    block: names.entry(s.block.clone()).or_insert_with(|| Arc::from(s.block.as_str())).clone(),
                    sound: sounds.entry(s.block.clone()).or_insert_with(|| Arc::from(block_sound(&s.block))).clone(),
                }
            })
            .collect();
        Ok(Self { states, atlas, water_uv, lava_uv, crack_uv, items, components, gui, sound_types: sound_types(), sound_events })
    }
}
