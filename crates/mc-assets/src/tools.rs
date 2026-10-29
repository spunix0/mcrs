//! Mining tool rules from the jar's block tags (`mineable/*`, `incorrect_for_*_tool`) and 26.3's tool tiers.

use crate::Jar;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
    Hoe,
    Shears,
}

/// Tool tiers in `ToolMaterial` order with their mining speeds.
const TIERS: [(&str, &str, f32); 7] = [
    ("wooden", "wooden", 2.0),
    ("stone", "stone", 4.0),
    ("copper", "copper", 5.0),
    ("iron", "iron", 6.0),
    ("diamond", "diamond", 8.0),
    ("golden", "gold", 12.0),
    ("netherite", "netherite", 9.0),
];

#[derive(Clone, Copy, Debug)]
pub struct Tool {
    pub kind: ToolKind,
    /// Index into the tier table; `None` for shears.
    pub tier: Option<usize>,
    pub speed: f32,
}

/// What the item's name says about it as a tool, e.g. `diamond_pickaxe`.
pub fn tool_for_item(item: &str) -> Option<Tool> {
    if item == "shears" {
        return Some(Tool { kind: ToolKind::Shears, tier: None, speed: 1.0 });
    }
    let (material, kind) = item.rsplit_once('_')?;
    let kind = match kind {
        "pickaxe" => ToolKind::Pickaxe,
        "axe" => ToolKind::Axe,
        "shovel" => ToolKind::Shovel,
        "hoe" => ToolKind::Hoe,
        _ => return None,
    };
    let tier = TIERS.iter().position(|t| t.0 == material)?;
    Some(Tool { kind, tier: Some(tier), speed: TIERS[tier].2 })
}

/// Per-block mining facts derived from tags.
#[derive(Clone, Copy, Default, Debug)]
pub struct Mineable {
    pub by: Option<ToolKind>,
    /// Bit i set = tier i counts as the correct tool (drops items, mines at full speed).
    pub correct_tiers: u8,
}

/// Resolves a block tag, following `#minecraft:other_tag` references.
fn tag(jar: &mut Jar, name: &str, depth: usize, out: &mut HashSet<String>) {
    if depth > 8 {
        return;
    }
    let Some(v) = jar.json(&format!("data/minecraft/tags/block/{name}.json")) else { return };
    for entry in v["values"].as_array().into_iter().flatten() {
        let id = entry.as_str().or_else(|| entry["id"].as_str()).unwrap_or_default();
        let id = id.strip_prefix("minecraft:").unwrap_or(id);
        if let Some(inner) = id.strip_prefix('#') {
            tag(jar, inner.strip_prefix("minecraft:").unwrap_or(inner), depth + 1, out);
        } else {
            out.insert(id.to_string());
        }
    }
}

pub struct ToolRules {
    mineable: HashMap<String, Mineable>,
}

impl ToolRules {
    pub fn load(jar: &mut Jar) -> Self {
        let mut mineable: HashMap<String, Mineable> = HashMap::new();
        for (kind, name) in [(ToolKind::Pickaxe, "pickaxe"), (ToolKind::Axe, "axe"), (ToolKind::Shovel, "shovel"), (ToolKind::Hoe, "hoe")] {
            let mut blocks = HashSet::new();
            tag(jar, &format!("mineable/{name}"), 0, &mut blocks);
            for b in blocks {
                mineable.entry(b).or_default().by = Some(kind);
            }
        }
        let mut incorrect = Vec::new();
        for (_, tag_name, _) in TIERS {
            let mut blocks = HashSet::new();
            tag(jar, &format!("incorrect_for_{tag_name}_tool"), 0, &mut blocks);
            incorrect.push(blocks);
        }
        for (block, m) in mineable.iter_mut() {
            m.correct_tiers = (0..TIERS.len()).filter(|&i| !incorrect[i].contains(block)).fold(0, |acc, i| acc | 1 << i);
        }
        Self { mineable }
    }

    pub fn get(&self, block: &str) -> Mineable {
        self.mineable.get(block).copied().unwrap_or_default()
    }
}

/// Shears are fast on a few blocks that aren't in any `mineable` tag.
pub fn shears_speed(block: &str) -> f32 {
    if block.ends_with("leaves") || block == "cobweb" {
        15.0
    } else if block.ends_with("wool") {
        5.0
    } else if block == "vine" || block == "glow_lichen" {
        2.0
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_items() {
        let t = tool_for_item("diamond_pickaxe").unwrap();
        assert_eq!((t.kind, t.tier, t.speed), (ToolKind::Pickaxe, Some(4), 8.0));
        assert_eq!(tool_for_item("golden_axe").unwrap().speed, 12.0);
        assert!(tool_for_item("stone").is_none());
        assert!(tool_for_item("diamond_sword").is_none());
        assert_eq!(tool_for_item("shears").unwrap().kind, ToolKind::Shears);
    }
}
