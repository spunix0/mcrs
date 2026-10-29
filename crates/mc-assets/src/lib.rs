//! Loading the user's own game assets (jar + asset index): block-state and model JSON, texture atlas stitching, language files, sounds.
//!
//! [`BlockModels::load`] turns every block state into baked quads (unit-cube space, atlas UVs) the mesher can copy straight into vertex buffers.

mod atlas;
mod model;

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

pub use atlas::Atlas;

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
    /// Ticks to mine with an empty hand (vanilla `getDestroyProgress`), or `None` if it can't be mined.
    pub fn break_ticks(&self, on_ground: bool) -> Option<u32> {
        if self.hardness < 0.0 {
            return None;
        }
        if self.hardness == 0.0 {
            return Some(0);
        }
        let speed = if on_ground { 1.0 } else { 0.2 };
        let per_tick = speed / self.hardness / if self.needs_tool { 100.0 } else { 30.0 };
        Some((1.0 / per_tick).ceil() as u32)
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

pub struct BlockModels {
    pub states: Vec<StateModel>,
    pub atlas: Atlas,
    pub water_uv: [f32; 4],
    pub lava_uv: [f32; 4],
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
    pub fn load(jar_path: &Path, blocks_json: &Path) -> Result<Self> {
        let states = load_states(blocks_json)?;
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
        let atlas = baker.stitch(&mut jar)?;
        let water_uv = atlas.rect("minecraft:block/water_still");
        let lava_uv = atlas.rect("minecraft:block/lava_still");

        let hardness = hardness_table();
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
                }
            })
            .collect();
        Ok(Self { states, atlas, water_uv, lava_uv })
    }
}
