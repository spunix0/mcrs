//! `cargo run -p mc-assets --example stats` — bakes all block states from the installed 26.3 jar and prints a summary.

use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let data = PathBuf::from(std::env::var("USERPROFILE")?).join("mcrs").join("data");
    let t = Instant::now();
    let m = mc_assets::BlockModels::load(&data.join("versions/26.3/26.3.jar"), &data.join("reports/blocks.json"))?;
    println!("baked {} states in {:?}; atlas {}x{} with {} textures", m.states.len(), t.elapsed(), m.atlas.width, m.atlas.height, m.atlas.len());
    for (name, id) in [("air", 0), ("stone", 1), ("grass_block", 9), ("oak_log y", 140), ("water", 86)] {
        let s = &m.states[id];
        println!("  {name:12} quads={:2} opaque={} fluid={:?}", s.quads.len(), s.opaque, s.fluid);
    }
    let empty = m.states.iter().filter(|s| s.quads.is_empty() && s.fluid.is_none()).count();
    println!("  states with no geometry (air, entity-rendered blocks): {empty}");
    Ok(())
}
