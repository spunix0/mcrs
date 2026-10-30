//! Client-side light updates. The server only sends light for chunks at the edge of the view distance
//! (`ChunkHolder.broadcastChanges`); everywhere else the client relights after block changes itself, like vanilla's
//! client-side `LevelLightEngine`. Both layers use the usual flood fill: take away the light the change cut off,
//! then spread light again from what is left and from the new block.

use crate::Chunk;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

pub type World = HashMap<(i32, i32), Arc<Chunk>>;

/// Down first: sky light at 15 keeps its level going straight down.
const DIRS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

struct Layer<'a, E, B> {
    world: &'a mut World,
    sky: bool,
    emission: &'a E,
    light_block: &'a B,
    /// Sections (x, y, z) whose light changed.
    touched: &'a mut HashSet<[i32; 3]>,
}

impl<E: Fn(u16) -> u8, B: Fn(u16) -> u8> Layer<'_, E, B> {
    fn chunk(&self, p: [i32; 3]) -> Option<&Chunk> {
        self.world.get(&(p[0].div_euclid(16), p[2].div_euclid(16))).map(|c| &**c)
    }

    fn get(&self, p: [i32; 3]) -> Option<u8> {
        let c = self.chunk(p)?;
        let s = (p[1] >> 4) - c.min_section + 1;
        if s < 0 || s as usize >= c.light_sections() {
            return None;
        }
        let (sky, block) = c.light(p[0].rem_euclid(16) as usize, p[1], p[2].rem_euclid(16) as usize);
        Some(if self.sky { sky } else { block })
    }

    fn set(&mut self, p: [i32; 3], v: u8) {
        let key = (p[0].div_euclid(16), p[2].div_euclid(16));
        if let Some(c) = self.world.get_mut(&key) {
            if Arc::make_mut(c).set_light(p[0].rem_euclid(16) as usize, p[1], p[2].rem_euclid(16) as usize, self.sky, v) {
                self.touched.insert([key.0, p[1] >> 4, key.1]);
            }
        }
    }

    fn state(&self, p: [i32; 3]) -> u16 {
        self.chunk(p).map_or(0, |c| c.get(p[0].rem_euclid(16) as usize, p[1], p[2].rem_euclid(16) as usize))
    }

    /// Nothing above `p` (or at it) takes away sky light.
    fn open_to_sky(&self, p: [i32; 3]) -> bool {
        let Some(c) = self.chunk(p) else { return false };
        let top = (c.min_section + c.sections.len() as i32) * 16;
        (p[1]..top).all(|y| (self.light_block)(self.state([p[0], y, p[2]])) == 0)
    }

    /// The light `p` makes itself: its emission, or 15 from the sky.
    fn source(&self, p: [i32; 3], level: u8) -> u8 {
        if self.sky {
            if level == 15 && self.open_to_sky(p) { 15 } else { 0 }
        } else {
            (self.emission)(self.state(p))
        }
    }

    fn run(&mut self, changed: &[[i32; 3]]) {
        let mut dec: VecDeque<([i32; 3], u8)> = VecDeque::new();
        let mut inc: VecDeque<[i32; 3]> = VecDeque::new();
        let add = |p: [i32; 3], d: &[i32; 3]| [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
        for &p in changed {
            let Some(old) = self.get(p) else { continue };
            if old > 0 {
                self.set(p, 0);
                dec.push_back((p, old));
            }
            let src = self.source(p, 15);
            if src > 0 {
                self.set(p, src);
                inc.push_back(p);
            }
            for d in &DIRS {
                let n = add(p, d);
                if self.get(n).is_some_and(|l| l > 0) {
                    inc.push_back(n);
                }
            }
        }
        while let Some((p, level)) = dec.pop_front() {
            for (i, d) in DIRS.iter().enumerate() {
                let n = add(p, d);
                let Some(nl) = self.get(n) else { continue };
                if nl == 0 {
                    continue;
                }
                let sky_column = self.sky && i == 0 && level == 15 && nl == 15;
                if nl < level || sky_column {
                    self.set(n, 0);
                    dec.push_back((n, nl));
                    let src = self.source(n, nl);
                    if src > 0 {
                        self.set(n, src);
                        inc.push_back(n);
                    }
                } else {
                    inc.push_back(n);
                }
            }
        }
        while let Some(p) = inc.pop_front() {
            let Some(l) = self.get(p) else { continue };
            if l <= 1 {
                continue;
            }
            for (i, d) in DIRS.iter().enumerate() {
                let n = add(p, d);
                let Some(nl) = self.get(n) else { continue };
                let lb = (self.light_block)(self.state(n));
                if lb >= 15 {
                    continue;
                }
                let next = if self.sky && i == 0 && l == 15 && lb == 0 { 15 } else { l.saturating_sub(lb.max(1)) };
                if next > nl {
                    self.set(n, next);
                    inc.push_back(n);
                }
            }
        }
    }
}

/// Relights both layers around blocks that changed. Returns the sections (x, y, z) whose light changed.
pub fn relight(world: &mut World, changed: &[[i32; 3]], emission: impl Fn(u16) -> u8, light_block: impl Fn(u16) -> u8) -> HashSet<[i32; 3]> {
    let mut touched = HashSet::new();
    for sky in [false, true] {
        Layer { world: &mut *world, sky, emission: &emission, light_block: &light_block, touched: &mut touched }.run(changed);
    }
    touched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Light, Section};

    // State 1 is glowstone (emits 15, opaque), 2 is stone.
    fn emission(s: u16) -> u8 {
        if s == 1 { 15 } else { 0 }
    }
    fn light_block(s: u16) -> u8 {
        if s == 0 { 0 } else { 15 }
    }

    fn world() -> World {
        let mut w = World::new();
        for x in -1..=1 {
            for z in -1..=1 {
                let light = Light { sky: vec![None; 6], block: vec![None; 6] };
                w.insert((x, z), Arc::new(Chunk { x, z, min_section: 0, sections: vec![Section::Uniform(0); 4], light }));
            }
        }
        w
    }

    fn set(w: &mut World, p: [i32; 3], s: u16) {
        Arc::make_mut(w.get_mut(&(p[0].div_euclid(16), p[2].div_euclid(16))).unwrap()).set(p[0].rem_euclid(16) as usize, p[1], p[2].rem_euclid(16) as usize, s);
    }

    fn light(w: &World, p: [i32; 3]) -> (u8, u8) {
        w[&(p[0].div_euclid(16), p[2].div_euclid(16))].light(p[0].rem_euclid(16) as usize, p[1], p[2].rem_euclid(16) as usize)
    }

    #[test]
    fn placing_and_breaking_a_light() {
        let mut w = world();
        set(&mut w, [15, 20, 8], 1);
        relight(&mut w, &[[15, 20, 8]], emission, light_block);
        assert_eq!(light(&w, [15, 20, 8]).1, 15);
        assert_eq!(light(&w, [16, 20, 8]).1, 14, "crosses into the next chunk");
        assert_eq!(light(&w, [15, 23, 8]).1, 12);
        set(&mut w, [15, 20, 8], 0);
        relight(&mut w, &[[15, 20, 8]], emission, light_block);
        assert_eq!(light(&w, [16, 20, 8]).1, 0);
        assert_eq!(light(&w, [15, 23, 8]).1, 0);
    }

    #[test]
    fn a_roof_shades_and_breaking_it_lets_sky_back() {
        let mut w = world();
        let roof: Vec<_> = (0..16).flat_map(|x| (0..16).map(move |z| [x, 40, z])).collect();
        for p in &roof {
            set(&mut w, *p, 2);
        }
        relight(&mut w, &roof, emission, light_block);
        assert_eq!(light(&w, [8, 39, 8]).0, 7, "8 blocks in from the open edge");
        assert_eq!(light(&w, [0, 39, 0]).0, 14);
        set(&mut w, [8, 40, 8], 0);
        relight(&mut w, &[[8, 40, 8]], emission, light_block);
        assert_eq!(light(&w, [8, 39, 8]).0, 15);
        assert_eq!(light(&w, [8, 10, 8]).0, 15, "straight down without fading");
    }
}
