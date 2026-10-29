//! `cargo run -p mc-assets --example stats` — bakes all block states from the installed 26.3 jar and prints a summary.

use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let data = PathBuf::from(std::env::var("USERPROFILE")?).join("mcrs").join("data");
    let t = Instant::now();
    let m = mc_assets::BlockModels::load(&data.join("versions/26.3/26.3.jar"), &data.join("reports"))?;
    println!("baked {} states in {:?}; atlas {}x{} with {} textures", m.states.len(), t.elapsed(), m.atlas.width, m.atlas.height, m.atlas.len());
    let blocks: serde_json::Value = serde_json::from_slice(&std::fs::read(data.join("reports/blocks.json"))?)?;
    let first = |name: &str| blocks[format!("minecraft:{name}")]["states"][0]["id"].as_u64().unwrap() as usize;
    for name in ["air", "stone", "grass_block", "oak_slab", "oak_stairs", "oak_fence", "short_grass", "torch", "water", "ice"] {
        let s = &m.states[first(name)];
        println!("  {name:12} quads={:2} opaque={:5} fluid={:?} friction={} collision={:?}", s.quads.len(), s.opaque, s.fluid, s.friction, s.collision);
    }
    let with_icon = m.items.iter().filter(|i| i.icon.is_some()).count();
    println!("items: {} ({} with icons), components: {}, gui atlas {}x{}", m.items.len(), with_icon, m.components.len(), m.gui.width, m.gui.height);
    for name in ["stone", "diamond_pickaxe", "oak_leaves", "torch", "air"] {
        let i = m.items.iter().find(|i| i.name == name).unwrap();
        println!("  {name:16} icon={:?} tint={:?}", i.icon.is_some(), i.tint);
    }
    println!("  damage component id = {:?}, \"10\" is {} px wide", m.components.get("damage"), m.gui.text_width("10"));
    Ok(())
}
