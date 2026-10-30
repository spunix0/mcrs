//! Cave culling, as vanilla's `VisGraph` and `SectionOcclusionGraph` (which Sodium also builds on): each section records
//! which of its faces see each other through non-opaque blocks, and a breadth-first search from the camera's section
//! only crosses faces that connect and never turns back toward the camera. Sections sealed off behind terrain, like
//! caves under the player's feet, are never reached and never drawn.

use glam::Vec3;
use mc_assets::BlockModels;
use mc_world::Section;

/// Face order: down, up, north (-z), south (+z), west (-x), east (+x). `f ^ 1` is the opposite face.
pub const FACES: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
/// Every face sees every other face (air, or a section not meshed yet).
pub const ALL: u64 = (1 << 36) - 1;

fn sees(vis: u64, a: usize, b: usize) -> bool {
    vis >> (a * 6 + b) & 1 != 0
}

/// Face-to-face visibility of a section: bit `a * 6 + b` is set when some open path joins faces a and b.
pub fn visibility(section: &Section, models: &BlockModels) -> u64 {
    let solid = |s: u16| models.states.get(s as usize).is_some_and(|m| m.opaque);
    if let Section::Uniform(s) = section {
        return if solid(*s) { 0 } else { ALL };
    }
    let mut closed = [false; 4096];
    let mut count = 0;
    for (i, c) in closed.iter_mut().enumerate() {
        *c = solid(section.get(i & 15, i >> 8, (i >> 4) & 15));
        count += *c as usize;
    }
    // VisGraph.resolve: with fewer than 256 opaque blocks everything counts as connected.
    if count < 256 {
        return ALL;
    }
    let mut vis = 0u64;
    let mut stack = Vec::new();
    for start in 0..4096 {
        let (x, y, z) = (start & 15, start >> 8, (start >> 4) & 15);
        let edge = x == 0 || x == 15 || y == 0 || y == 15 || z == 0 || z == 15;
        if closed[start] || !edge {
            continue;
        }
        let mut faces = 0u8;
        closed[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let p = [(i & 15) as i32, (i >> 8) as i32, ((i >> 4) & 15) as i32];
            for (f, d) in FACES.iter().enumerate() {
                let n = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
                if n.iter().any(|v| !(0..16).contains(v)) {
                    faces |= 1 << f;
                    continue;
                }
                let j = ((n[1] << 8) | (n[2] << 4) | n[0]) as usize;
                if !closed[j] {
                    closed[j] = true;
                    stack.push(j);
                }
            }
        }
        for a in 0..6 {
            for b in 0..6 {
                if faces & (1 << a) != 0 && faces & (1 << b) != 0 {
                    vis |= 1 << (a * 6 + b);
                }
            }
        }
    }
    vis
}

/// Sections reachable from the camera, in search order (near first), as (x, section y, z).
/// `graph(x, sy, z)` is a loaded section's visibility (`None` when its column isn't loaded); `in_view` is the frustum test.
pub fn search(camera: Vec3, radius: i32, sy_range: (i32, i32), graph: impl Fn(i32, i32, i32) -> Option<u64>, in_view: impl Fn([i32; 3]) -> bool) -> Vec<[i32; 3]> {
    let (lo, hi) = sy_range;
    if hi < lo {
        return Vec::new();
    }
    let section = |v: f32| (v.floor() as i32).div_euclid(16);
    let (cx, cz) = (section(camera.x), section(camera.z));
    let cy = section(camera.y).clamp(lo, hi);
    let (side, height) = ((2 * radius + 1) as usize, (hi - lo + 1) as usize);
    let index = |p: [i32; 3]| -> Option<usize> {
        let (dx, dz) = (p[0] - cx + radius, p[2] - cz + radius);
        let ok = (0..side as i32).contains(&dx) && (0..side as i32).contains(&dz) && (lo..=hi).contains(&p[1]);
        ok.then(|| (dz as usize * side + dx as usize) * height + (p[1] - lo) as usize)
    };
    // Per section: bits 0-5 the travel directions it was entered with, 6-11 every direction taken so far, 12 queued.
    let mut state = vec![0u16; side * side * height];
    let start = [cx, cy, cz];
    let mut out = Vec::new();
    if graph(cx, cy, cz).is_none() {
        return out;
    }
    state[index(start).unwrap()] = 1 << 12;
    let mut queue = std::collections::VecDeque::from([start]);
    while let Some(p) = queue.pop_front() {
        out.push(p);
        let s = state[index(p).unwrap()];
        let vis = graph(p[0], p[1], p[2]).unwrap_or(ALL);
        for (f, d) in FACES.iter().enumerate() {
            // Never step back toward the camera.
            if s >> 6 & (1 << (f ^ 1)) != 0 {
                continue;
            }
            // Leave through face f only if it joins a face this section was entered through.
            if p != start && !(0..6).any(|j| s & (1 << j) != 0 && sees(vis, j ^ 1, f)) {
                continue;
            }
            let n = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
            let Some(i) = index(n) else { continue };
            if state[i] & (1 << 12) != 0 {
                state[i] |= 1 << f;
                continue;
            }
            if graph(n[0], n[1], n[2]).is_none() || !in_view(n) {
                continue;
            }
            state[i] = 1 << 12 | 1 << f | (s & (0x3f << 6)) | 1 << (f + 6);
            queue.push_back(n);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walls_stop_the_search() {
        // A 3-section-tall world, 1 column wide; the middle section is sealed, so the bottom one stays hidden.
        let graph = |x: i32, sy: i32, z: i32| (x == 0 && z == 0).then_some(if sy == 1 { 0 } else { ALL });
        let found = search(Vec3::new(8.0, 40.0, 8.0), 1, (0, 2), graph, |_| true);
        assert!(found.contains(&[0, 2, 0]) && found.contains(&[0, 1, 0]));
        assert!(!found.contains(&[0, 0, 0]));
        // Open air everywhere: every section is found.
        let open = search(Vec3::new(8.0, 40.0, 8.0), 1, (0, 2), |x, _, z| (x == 0 && z == 0).then_some(ALL), |_| true);
        assert_eq!(open.len(), 3);
    }
}
