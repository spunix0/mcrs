//! Turns a chunk column into vertex/index lists: copies each block's baked quads, skips faces hidden by opaque
//! neighbors (including across chunk borders), and bakes smooth light levels and ambient occlusion into the vertices.
//! The shader turns light levels into brightness, so day and night don't need a remesh.

use bytemuck::{Pod, Zeroable};
use mc_assets::{BlockModels, Dir, Fluid, Quad};
use mc_world::Chunk;
use std::sync::Arc;

/// Overlay, entity and hand geometry: color is final (already lit).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

/// Chunk geometry: color is tint * face shade * AO, `light` is (sky, block) level * 17 (0..255).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ChunkVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    pub light: [u8; 4],
}

#[derive(Default)]
pub struct Geometry {
    pub vertices: Vec<ChunkVertex>,
    pub indices: Vec<u32>,
}

/// A vertex color and its light levels.
type Lit = ([u8; 4], [u8; 4]);

impl Geometry {
    fn quad(&mut self, pos: [[f32; 3]; 4], uv: [[f32; 2]; 4], lit: [Lit; 4]) {
        let base = self.vertices.len() as u32;
        for i in 0..4 {
            self.vertices.push(ChunkVertex { pos: pos[i], uv: uv[i], color: lit[i].0, light: lit[i].1 });
        }
        // Split along the brighter diagonal so AO gradients don't look anisotropic.
        let b = |i: usize| {
            let (c, l) = lit[i];
            (c[0] as u32 + c[1] as u32 + c[2] as u32) * (64 + l[0].max(l[1]) as u32)
        };
        if b(0) + b(2) >= b(1) + b(3) {
            self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            self.indices.extend_from_slice(&[base, base + 1, base + 3, base + 1, base + 2, base + 3]);
        }
    }
}

#[derive(Default)]
pub struct SectionMesh {
    pub opaque: Geometry,
    pub translucent: Geometry,
    /// Which faces see each other through non-opaque blocks: bit `a * 6 + b` (see [`crate::cull`]).
    pub vis: u64,
}

/// Fresh meshes for some sections of a column.
pub struct ChunkMesh {
    pub x: i32,
    pub z: i32,
    pub min_section: i32,
    /// Section count of the column.
    pub height: usize,
    /// (section index from the bottom, mesh).
    pub sections: Vec<(usize, SectionMesh)>,
}

/// A chunk plus its 8 neighbors (any of which may not be loaded yet).
pub struct Area {
    pub center: Arc<Chunk>,
    /// Indexed `(dz + 1) * 3 + (dx + 1)`; the center slot is ignored.
    pub around: [Option<Arc<Chunk>>; 9],
    /// Sections to mesh, bit i = section i from the bottom.
    pub mask: u64,
}

impl Area {
    #[inline]
    fn chunk(&self, x: i32, z: i32) -> Option<(&Chunk, usize, usize)> {
        let (cx, cz) = (x.div_euclid(16), z.div_euclid(16));
        let (lx, lz) = (x.rem_euclid(16) as usize, z.rem_euclid(16) as usize);
        if cx == 0 && cz == 0 {
            return Some((&self.center, lx, lz));
        }
        if !(-1..=1).contains(&cx) || !(-1..=1).contains(&cz) {
            return None;
        }
        self.around[((cz + 1) * 3 + (cx + 1)) as usize].as_deref().map(|c| (c, lx, lz))
    }

    /// Block state at center-chunk-relative coordinates; `None` if that neighbor isn't loaded.
    #[inline]
    fn block(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        self.chunk(x, z).map(|(c, lx, lz)| c.get(lx, y, lz))
    }

    #[inline]
    fn light(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        self.chunk(x, z).map_or((15, 0), |(c, lx, lz)| c.light(lx, y, lz))
    }
}

const WATER_TINT: [u8; 3] = [0x3f, 0x76, 0xe4];
const AO: [f32; 4] = [0.5, 0.67, 0.83, 1.0];
/// Vanilla 26.3's lightmap (`lightmap.fsh`) for light levels 0..15 (fractional from smooth lighting): ambient plus
/// tinted sky light plus warm block light, clamped, then bent by the Brightness option. The chunk shader has the same.
pub fn lightmap(sky: f32, block: f32, env: &crate::Environment) -> [f32; 3] {
    let level = |l: f32| (l / 15.0).clamp(0.0, 1.0);
    let bright = |l: f32| l / (4.0 - 3.0 * l);
    let (sl, bl) = (level(sky), level(block));
    let (sb, bb) = (bright(sl) * env.daylight, bright(bl) * 1.4);
    let mix_block = 0.9 * (2.0 * bl - 1.0) * (2.0 * bl - 1.0);
    let c: [f32; 3] = std::array::from_fn(|i| {
        let block_color = env.block_tint[i] + (1.0 - env.block_tint[i]) * mix_block;
        (env.ambient[i] + env.sky_light[i] * sb + block_color * bb).clamp(0.0, 1.0)
    });
    let m = c[0].max(c[1]).max(c[2]).max(1e-4);
    let inv = 1.0 - m;
    let scaled = 1.0 - inv * inv * inv * inv;
    std::array::from_fn(|i| c[i] + (c[i] * scaled / m - c[i]) * env.gamma)
}

/// [`lightmap`] as one brightness for entities and the hand.
pub fn brightness(sky: f32, block: f32, env: &crate::Environment) -> f32 {
    let c = lightmap(sky, block, env);
    (c[0] + c[1] + c[2]) / 3.0
}

fn lit(tint: [u8; 3], shade: f32, sky: f32, block: f32, ao: f32, alpha: u8) -> Lit {
    let mut out = [0u8; 4];
    for i in 0..3 {
        out[i] = (tint[i] as f32 * shade * ao).min(255.0) as u8;
    }
    out[3] = alpha;
    let level = |l: f32| (l.clamp(0.0, 15.0) * 17.0).round() as u8;
    (out, [level(sky), level(block), 0, 0])
}

fn axis(d: Dir) -> usize {
    match d {
        Dir::West | Dir::East => 0,
        Dir::Down | Dir::Up => 1,
        Dir::North | Dir::South => 2,
    }
}

/// Meshes the sections of `area.mask`. Every one of them gets an entry, so emptied sections replace old geometry.
pub fn mesh_chunk(area: &Area, models: &BlockModels) -> ChunkMesh {
    let chunk = &*area.center;
    let (ox, oz) = (chunk.x as f32 * 16.0, chunk.z as f32 * 16.0);
    let mut out = ChunkMesh { x: chunk.x, z: chunk.z, min_section: chunk.min_section, height: chunk.sections.len(), sections: Vec::new() };
    let opaque = |x: i32, y: i32, z: i32| area.block(x, y, z).is_some_and(|s| models.states[s as usize].opaque);

    for (si, section) in chunk.sections.iter().enumerate() {
        if si >= 64 || area.mask & (1 << si) == 0 {
            continue;
        }
        let mut mesh = SectionMesh { vis: crate::cull::visibility(section, models), ..Default::default() };
        if section.is_air() {
            out.sections.push((si, mesh));
            continue;
        }
        let base_y = (chunk.min_section + si as i32) * 16;
        for ly in 0..16 {
            let y = base_y + ly as i32;
            for z in 0..16i32 {
                for x in 0..16i32 {
                    let state = section.get(x as usize, ly, z as usize);
                    if state == 0 {
                        continue;
                    }
                    let Some(model) = models.states.get(state as usize) else { continue };
                    let (wx, wy, wz) = (ox + x as f32, y as f32, oz + z as f32);

                    for q in model.quads.iter() {
                        if let Some(dir) = q.cull {
                            let [dx, dy, dz] = dir.offset();
                            if opaque(x + dx, y + dy, z + dz) {
                                continue;
                            }
                            // Glass, ice, slime: faces between two of the same block are hidden (vanilla's
                            // `skipRendering`); leaves keep them, like fancy graphics.
                            let full = model.collision.len() == 1 && model.collision[0] == [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
                            if full && area.block(x + dx, y + dy, z + dz) == Some(state) && !model.block.ends_with("_leaves") {
                                continue;
                            }
                        }
                        let tint = if q.tinted { model.tint } else { [255, 255, 255] };
                        let colors = match q.cull {
                            Some(dir) => smooth(area, &opaque, q, dir, [x, y, z], tint),
                            None => {
                                let (sky, block) = area.light(x, y, z);
                                [lit(tint, q.shade, sky as f32, block as f32, 1.0, 255); 4]
                            }
                        };
                        let pos = q.pos.map(|p| [p[0] + wx, p[1] + wy, p[2] + wz]);
                        let target = if q.translucent { &mut mesh.translucent } else { &mut mesh.opaque };
                        target.quad(pos, q.uv, colors);
                    }

                    if let Some(fluid) = model.fluid {
                        fluid_faces(&mut mesh, area, models, fluid, [x, y, z], [wx, wy, wz]);
                    }
                }
            }
        }
        out.sections.push((si, mesh));
    }
    out
}

/// Per-vertex light and AO for a quad lying on a block face, sampled from the blocks in front of it.
fn smooth(area: &Area, opaque: &impl Fn(i32, i32, i32) -> bool, q: &Quad, dir: Dir, b: [i32; 3], tint: [u8; 3]) -> [Lit; 4] {
    let n = dir.offset();
    let front = [b[0] + n[0], b[1] + n[1], b[2] + n[2]];
    let a = axis(dir);
    let (u, v) = ((a + 1) % 3, (a + 2) % 3);
    let mut out = [([0u8; 4], [0u8; 4]); 4];
    for (i, p) in q.pos.iter().enumerate() {
        let su = if p[u] > 0.5 { 1 } else { -1 };
        let sv = if p[v] > 0.5 { 1 } else { -1 };
        let at = |du: i32, dv: i32| {
            let mut c = front;
            c[u] += du;
            c[v] += dv;
            c
        };
        let (s1, s2, corner) = (at(su, 0), at(0, sv), at(su, sv));
        let (o1, o2) = (opaque(s1[0], s1[1], s1[2]), opaque(s2[0], s2[1], s2[2]));
        let oc = (o1 && o2) || opaque(corner[0], corner[1], corner[2]);
        let ao = if o1 && o2 { 0 } else { 3 - o1 as usize - o2 as usize - oc as usize };

        let (mut sky, mut block, mut n) = (0.0f32, 0.0f32, 0.0f32);
        let mut sample = |c: [i32; 3], blocked: bool| {
            if !blocked {
                let (s, bl) = area.light(c[0], c[1], c[2]);
                sky += s as f32;
                block += bl as f32;
                n += 1.0;
            }
        };
        sample(front, false);
        sample(s1, o1);
        sample(s2, o2);
        sample(corner, oc);
        out[i] = lit(tint, q.shade, sky / n, block / n, AO[ao], 255);
    }
    out
}

fn fluid_faces(mesh: &mut SectionMesh, area: &Area, models: &BlockModels, fluid: Fluid, [x, y, z]: [i32; 3], [wx, wy, wz]: [f32; 3]) {
    let same = |s: u16| models.states[s as usize].fluid == Some(fluid);
    let is_fluid = |x: i32, y: i32, z: i32| area.block(x, y, z).is_some_and(same);
    // FluidState.getOwnHeight: amount / 9, where a source or falling fluid has amount 8 and flowing 8 - level.
    let own = |x: i32, y: i32, z: i32| -> Option<f32> {
        let s = area.block(x, y, z).filter(|s| same(*s))?;
        let level = models.states[s as usize].fluid_level;
        Some(if level == 0 || level >= 8 { 8.0 / 9.0 } else { (8 - level) as f32 / 9.0 })
    };
    // LiquidBlockRenderer.calculateAverageHeight over the four blocks around a corner (full blocks weigh 10).
    let corner = |cx: i32, cz: i32| -> f32 {
        let (mut total, mut weight) = (0.0f32, 0.0f32);
        for (bx, bz) in [(cx - 1, cz - 1), (cx, cz - 1), (cx - 1, cz), (cx, cz)] {
            if is_fluid(bx, y + 1, bz) {
                return 1.0;
            }
            match own(bx, y, bz) {
                Some(h) if h >= 0.8 => (total, weight) = (total + h * 10.0, weight + 10.0),
                Some(h) => (total, weight) = (total + h, weight + 1.0),
                None => {
                    let solid = area.block(bx, y, bz).is_none_or(|s| models.states[s as usize].opaque);
                    if !solid {
                        weight += 1.0;
                    }
                }
            }
        }
        if weight > 0.0 { total / weight } else { 0.0 }
    };
    let covered = is_fluid(x, y + 1, z);
    // Corner heights: (x0,z0), (x0,z1), (x1,z1), (x1,z0).
    let h = if covered { [1.0; 4] } else { [corner(x, z), corner(x, z + 1), corner(x + 1, z + 1), corner(x + 1, z)] };
    let (still, flow, tint, alpha) = match fluid {
        Fluid::Water => (models.water_uv, models.water_flow_uv, WATER_TINT, 200),
        Fluid::Lava => (models.lava_uv, models.lava_flow_uv, [255, 255, 255], 255),
    };
    let tex = |r: [f32; 4], u: f32, v: f32| [r[0] + (r[2] - r[0]) * u, r[1] + (r[3] - r[1]) * v];
    // Flow direction from the height differences to the neighbors (FlowingFluid.getFlow, simplified).
    let here = own(x, y, z).unwrap_or(8.0 / 9.0);
    let mut flow_dir = [0.0f32; 2];
    for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let diff = match own(x + dx, y, z + dz) {
            Some(n) => here - n,
            None if area.block(x + dx, y, z + dz).is_some_and(|s| !models.states[s as usize].opaque) => here,
            None => 0.0,
        };
        flow_dir = [flow_dir[0] + dx as f32 * diff, flow_dir[1] + dz as f32 * diff];
    }
    for dir in Dir::ALL {
        let [dx, dy, dz] = dir.offset();
        let hidden = match area.block(x + dx, y + dy, z + dz) {
            // Unloaded neighbor: assume more fluid so borders don't flash faces before it arrives.
            None => true,
            Some(n) => same(n) || models.states[n as usize].opaque,
        };
        if hidden {
            continue;
        }
        let (x0, y0, z0, x1, z1) = (wx, wy, wz, wx + 1.0, wz + 1.0);
        let (pos, uv) = match dir {
            Dir::Up => {
                let pos = [[x0, y0 + h[0], z0], [x0, y0 + h[1], z1], [x1, y0 + h[2], z1], [x1, y0 + h[3], z0]];
                let uv = if flow_dir == [0.0, 0.0] {
                    [tex(still, 0.0, 0.0), tex(still, 0.0, 1.0), tex(still, 1.0, 1.0), tex(still, 1.0, 0.0)]
                } else {
                    // The flowing texture turned to point along the flow, as LiquidBlockRenderer does.
                    let angle = flow_dir[1].atan2(flow_dir[0]) - std::f32::consts::FRAC_PI_2;
                    let (s, c) = (angle.sin() * 0.25, angle.cos() * 0.25);
                    [
                        tex(flow, 0.5 + (-c - s), 0.5 + (-c + s)),
                        tex(flow, 0.5 + (-c + s), 0.5 + (c + s)),
                        tex(flow, 0.5 + (c + s), 0.5 + (c - s)),
                        tex(flow, 0.5 + (c - s), 0.5 + (-c - s)),
                    ]
                };
                (pos, uv)
            }
            Dir::Down => ([[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]], [tex(still, 0.0, 0.0), tex(still, 0.0, 1.0), tex(still, 1.0, 1.0), tex(still, 1.0, 0.0)]),
            side => {
                // Sides: the flowing texture's left half, cut at each top corner's height.
                let (a, b, ha, hb) = match side {
                    Dir::North => ([x1, z0], [x0, z0], h[3], h[0]),
                    Dir::South => ([x0, z1], [x1, z1], h[1], h[2]),
                    Dir::West => ([x0, z0], [x0, z1], h[0], h[1]),
                    _ => ([x1, z1], [x1, z0], h[2], h[3]),
                };
                let pos = [[a[0], y0 + ha, a[1]], [a[0], y0, a[1]], [b[0], y0, b[1]], [b[0], y0 + hb, b[1]]];
                let uv = [tex(flow, 0.0, (1.0 - ha) * 0.5), tex(flow, 0.0, 0.5), tex(flow, 0.5, 0.5), tex(flow, 0.5, (1.0 - hb) * 0.5)];
                (pos, uv)
            }
        };
        let (sky, block) = area.light(x + dx, y + dy, z + dz);
        let c = lit(tint, dir.shade(), sky as f32, block.max(if fluid == Fluid::Lava { 15 } else { 0 }) as f32, 1.0, alpha);
        match fluid {
            Fluid::Water => {
                mesh.translucent.quad(pos, uv, [c; 4]);
                // The surface seen from underwater: vanilla adds the top face again, facing down.
                if dir == Dir::Up {
                    let (mut p, mut t) = (pos, uv);
                    p.reverse();
                    t.reverse();
                    mesh.translucent.quad(p, t, [c; 4]);
                }
            }
            Fluid::Lava => mesh.opaque.quad(pos, uv, [c; 4]),
        }
    }
}
