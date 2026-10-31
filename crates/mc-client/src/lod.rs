//! Distant terrain (LOD), the built-in "far view" mod: past the render distance, the world's surface drawn as
//! textured columns out to 1024 chunks. Background threads read the surfaces (a singleplayer world's region files,
//! or for servers the [`Cache`] of every chunk received) and build one mesh per region (512x512 blocks), coarser the
//! farther away; the LOD Bias makes it finer or coarser.

use mc_assets::LodFace;
use mc_render::{ChunkVertex, Geometry};
use mc_world::region::{self, NONE, Surface};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Job {
    key: (i32, i32),
    cell: usize,
    path: PathBuf,
}

/// A finished region mesh.
pub struct Built {
    pub key: (i32, i32),
    pub geometry: Geometry,
    /// Y range, for culling.
    pub y: (f32, f32),
}

pub struct Lod {
    dir: PathBuf,
    jobs: Sender<Job>,
    results: Receiver<Built>,
    /// Surface files on disk and when they last changed, rescanned now and then (the world grows and gets saved).
    regions: HashMap<(i32, i32), (std::time::SystemTime, PathBuf)>,
    scanned: Option<Instant>,
    /// Cell size and file time each region was last built from.
    built: HashMap<(i32, i32), (usize, std::time::SystemTime)>,
}

/// Blocks per cell for a region `d` chunks away: 4 near, doubling with distance up to 64; the LOD Bias (0 Lowest .. 4
/// Highest, 2 = Medium) doubles it for each step down and halves it for each step up.
pub fn cell_for(d: i32, quality: u8) -> usize {
    let base = match d {
        ..64 => 4,
        64..128 => 8,
        128..256 => 16,
        256..512 => 32,
        _ => 64,
    };
    match quality {
        0 => base * 4,
        1 => base * 2,
        3 => (base / 2).max(2),
        4 => (base / 4).max(1),
        _ => base,
    }
}

impl Lod {
    /// `dir` holds the surfaces: a dimension's `region` folder (`.mca`, singleplayer) or a server's cache (`.lod`).
    /// `faces` says how each block looks from afar.
    pub fn new(dir: PathBuf, faces: Arc<HashMap<String, LodFace>>) -> Self {
        let (jobs, job_rx) = channel::<Job>();
        let (done, results) = channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for _ in 0..2 {
            let (job_rx, done, faces) = (job_rx.clone(), done.clone(), faces.clone());
            std::thread::spawn(move || {
                loop {
                    let Ok(job) = job_rx.lock().unwrap().recv() else { return };
                    let built = match region::read_any(&job.path) {
                        Ok(s) => mesh(&s, job.key, job.cell, &faces),
                        Err(_) => Built { key: job.key, geometry: Geometry::default(), y: (0.0, 0.0) },
                    };
                    if done.send(built).is_err() {
                        return;
                    }
                }
            });
        }
        Self { dir, jobs, results, regions: HashMap::new(), scanned: None, built: HashMap::new() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Asks for the regions within `radius` chunks of `center` at the right detail (and again when their file
    /// changed), nearest first, and returns the regions to drop.
    pub fn update(&mut self, center: (i32, i32), radius: i32, quality: u8) -> Vec<(i32, i32)> {
        if self.scanned.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            self.scanned = Some(Instant::now());
            self.regions = std::fs::read_dir(&self.dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let rest = name.strip_prefix("r.")?;
                    let stem = rest.strip_suffix(".mca").or_else(|| rest.strip_suffix(".lod"))?;
                    let mut parts = stem.split('.');
                    let key = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
                    Some((key, (e.metadata().ok()?.modified().ok()?, e.path())))
                })
                .collect();
        }
        let distance = |(rx, rz): (i32, i32)| {
            let dx = center.0.clamp(rx * 32, rx * 32 + 31) - center.0;
            let dz = center.1.clamp(rz * 32, rz * 32 + 31) - center.1;
            dx.abs().max(dz.abs())
        };
        let mut wanted: Vec<((i32, i32), i32)> = self.regions.keys().map(|k| (*k, distance(*k))).filter(|(_, d)| *d <= radius).collect();
        wanted.sort_by_key(|w| w.1);
        for (key, d) in wanted {
            let cell = cell_for(d, quality);
            let (modified, path) = self.regions[&key].clone();
            if self.built.get(&key) != Some(&(cell, modified)) {
                self.built.insert(key, (cell, modified));
                let _ = self.jobs.send(Job { key, cell, path });
            }
        }
        let gone: Vec<(i32, i32)> = self.built.keys().copied().filter(|k| distance(*k) > radius || !self.regions.contains_key(k)).collect();
        for k in &gone {
            self.built.remove(k);
        }
        gone
    }

    /// Meshes finished since the last call.
    pub fn poll(&mut self) -> Vec<Built> {
        self.results.try_iter().filter(|b| self.built.contains_key(&b.key)).collect()
    }
}

/// What a server sends, kept for distant terrain (servers don't share their region files): each received chunk's
/// surface, saved as `r.<x>.<z>.lod` per region so places you've been stay visible past the view distance.
pub struct Cache {
    dir: PathBuf,
    regions: HashMap<(i32, i32), (Surface, bool)>,
    flushed: Instant,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, regions: HashMap::new(), flushed: Instant::now() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Notes a chunk's surface: per column the highest block that blocks movement, is a fluid, or is leaves (like the
    /// `MOTION_BLOCKING` heightmap region files have).
    pub fn record(&mut self, chunk: &mc_world::Chunk, models: &mc_assets::BlockModels) {
        let key = (chunk.x.div_euclid(32), chunk.z.div_euclid(32));
        let path = self.dir.join(format!("r.{}.{}.lod", key.0, key.1));
        let entry = self.regions.entry(key).or_insert_with(|| (Surface::load(&path).unwrap_or_else(|_| Surface::empty()), false));
        let (ox, oz) = (chunk.x.rem_euclid(32) as usize * 16, chunk.z.rem_euclid(32) as usize * 16);
        for z in 0..16 {
            for x in 0..16 {
                if let Some((y, block)) = surface_top(chunk, models, x, z) {
                    entry.0.set(ox + x, oz + z, y, block);
                }
            }
        }
        entry.1 = true;
    }

    /// Writes changed regions every 15 seconds (or now, with `force`), off the game thread.
    pub fn flush(&mut self, force: bool) {
        if !force && self.flushed.elapsed() < Duration::from_secs(15) {
            return;
        }
        self.flushed = Instant::now();
        let dir = self.dir.clone();
        let dirty: Vec<(PathBuf, Surface)> = self
            .regions
            .iter_mut()
            .filter(|(_, r)| r.1)
            .map(|(k, r)| {
                r.1 = false;
                (dir.join(format!("r.{}.{}.lod", k.0, k.1)), r.0.clone())
            })
            .collect();
        if dirty.is_empty() {
            return;
        }
        let save = move || {
            for (path, s) in dirty {
                let _ = s.save(&path);
            }
        };
        if force {
            save();
        } else {
            std::thread::spawn(save);
        }
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        self.flush(true);
    }
}

/// The highest block of a column that blocks movement, is a fluid, or is leaves.
fn surface_top<'a>(chunk: &mc_world::Chunk, models: &'a mc_assets::BlockModels, x: usize, z: usize) -> Option<(i16, &'a str)> {
    for (si, sec) in chunk.sections.iter().enumerate().rev() {
        if sec.is_air() {
            continue;
        }
        for ly in (0..16).rev() {
            let Some(m) = models.states.get(sec.get(x, ly, z) as usize) else { continue };
            if m.collision.is_empty() && m.fluid.is_none() && !m.leaves {
                continue;
            }
            return Some(((chunk.min_section + si as i32) as i16 * 16 + ly as i16, &m.block));
        }
    }
    None
}

/// One region's surface as columns: per cell, the highest column's top in its block's textures (repeated once per
/// block; the shader tiles them), with walls down to lower neighbors and a skirt at the region's edge.
pub fn mesh(s: &Surface, key: (i32, i32), cell: usize, faces: &HashMap<String, LodFace>) -> Built {
    let n = 512 / cell;
    let mut h = vec![NONE; n * n];
    let mut look = vec![None; n * n];
    for cz in 0..n {
        for cx in 0..n {
            let (mut best, mut at) = (NONE, 0);
            for z in 0..cell {
                for x in 0..cell {
                    let i = (cz * cell + z) * 512 + cx * cell + x;
                    if s.heights[i] > best {
                        (best, at) = (s.heights[i], i);
                    }
                }
            }
            h[cz * n + cx] = best;
            if best != NONE {
                look[cz * n + cx] = s.names.get(s.blocks[at] as usize).and_then(|b| faces.get(b)).or_else(|| faces.get("stone")).copied();
            }
        }
    }
    let mut g = Geometry::default();
    let (ox, oz) = (key.0 as f32 * 512.0, key.1 as f32 * 512.0);
    let (mut y_min, mut y_max) = (f32::MAX, f32::MIN);
    // `uv` is in blocks along the face; `light` carries full sky light and the atlas tile in its spare bytes.
    let quad = |g: &mut Geometry, p: [[f32; 3]; 4], uv: [[f32; 2]; 4], tile: u16, tint: [u8; 3], shade: f32| {
        let base = g.vertices.len() as u32;
        let color = [(tint[0] as f32 * shade) as u8, (tint[1] as f32 * shade) as u8, (tint[2] as f32 * shade) as u8, 255];
        let light = [255, 0, (tile & 255) as u8, (tile >> 8) as u8];
        for i in 0..4 {
            g.vertices.push(ChunkVertex { pos: p[i], uv: uv[i], color, light });
        }
        g.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    let c = cell as f32;
    let gray = LodFace { top: 0, side: 0, top_tint: [128; 3], side_tint: [128; 3] };
    for cz in 0..n {
        for cx in 0..n {
            let top = h[cz * n + cx];
            if top == NONE {
                continue;
            }
            let face = look[cz * n + cx].unwrap_or(gray);
            let y = top as f32 + 1.0;
            let (x0, z0) = (ox + cx as f32 * c, oz + cz as f32 * c);
            let (x1, z1) = (x0 + c, z0 + c);
            quad(&mut g, [[x0, y, z0], [x0, y, z1], [x1, y, z1], [x1, y, z0]], [[x0, z0], [x0, z1], [x1, z1], [x1, z0]], face.top, face.top_tint, 1.0);
            // Walls down to each lower neighbor; past the region's edge (or an ungenerated cell) a skirt.
            let neighbor = |dx: i32, dz: i32| -> f32 {
                let (nx, nz) = (cx as i32 + dx, cz as i32 + dz);
                let skirt = y - c * 2.0;
                if nx < 0 || nz < 0 || nx >= n as i32 || nz >= n as i32 {
                    return skirt;
                }
                match h[nz as usize * n + nx as usize] {
                    NONE => skirt,
                    v => v as f32 + 1.0,
                }
            };
            let walls = [
                (neighbor(0, -1), [x1, z0], [x0, z0], 0.8),
                (neighbor(0, 1), [x0, z1], [x1, z1], 0.8),
                (neighbor(-1, 0), [x0, z0], [x0, z1], 0.6),
                (neighbor(1, 0), [x1, z1], [x1, z0], 0.6),
            ];
            for (low, a, b, shade) in walls {
                if low < y {
                    // Along the wall: x for north/south walls, z for east/west; down the texture: -y.
                    let along = |p: [f32; 2]| if a[1] == b[1] { p[0] } else { p[1] };
                    let uv = [[along(a), -y], [along(a), -low], [along(b), -low], [along(b), -y]];
                    quad(&mut g, [[a[0], y, a[1]], [a[0], low, a[1]], [b[0], low, b[1]], [b[0], y, b[1]]], uv, face.side, face.side_tint, shade);
                    y_min = y_min.min(low);
                }
            }
            y_min = y_min.min(y);
            y_max = y_max.max(y);
        }
    }
    Built { key, geometry: g, y: (y_min, y_max) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_makes_a_wall_and_edges_get_skirts() {
        let mut s = Surface { heights: vec![NONE; 512 * 512], blocks: vec![0; 512 * 512], names: vec!["stone".into()] };
        // Two 4x4-cell columns side by side: height 70 and 64.
        for z in 0..4 {
            for x in 0..8 {
                s.heights[z * 512 + x] = if x < 4 { 70 } else { 64 };
            }
        }
        let faces = HashMap::from([("stone".to_string(), LodFace { top: 1, side: 2, top_tint: [255; 3], side_tint: [255; 3] })]);
        let b = mesh(&s, (0, 0), 4, &faces);
        // 2 tops + 1 step wall + skirts: first cell 3 edges, second 3 edges.
        assert_eq!(b.geometry.indices.len() / 6, 2 + 1 + 6);
        assert_eq!(b.y.1, 71.0);
        assert!(b.y.0 < 64.0);
    }

    #[test]
    fn farther_regions_get_coarser() {
        assert!(cell_for(10, 2) < cell_for(300, 2));
        assert_eq!(cell_for(1000, 2), 64);
        assert_eq!(cell_for(10, 3), 2);
        assert_eq!(cell_for(10, 0), 16);
    }
}
