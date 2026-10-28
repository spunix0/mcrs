//! Turns a chunk column into vertex/index lists: copies each block's baked quads, skips faces hidden by opaque
//! neighbors (including across chunk borders), and bakes smooth lighting and ambient occlusion into vertex colors.

use bytemuck::{Pod, Zeroable};
use mc_assets::{BlockModels, Dir, Fluid, Quad};
use mc_world::Chunk;
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

#[derive(Default)]
pub struct Geometry {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Geometry {
    fn quad(&mut self, pos: [[f32; 3]; 4], uv: [[f32; 2]; 4], color: [[u8; 4]; 4]) {
        let base = self.vertices.len() as u32;
        for i in 0..4 {
            self.vertices.push(Vertex { pos: pos[i], uv: uv[i], color: color[i] });
        }
        // Split along the brighter diagonal so AO gradients don't look anisotropic.
        let b = |i: usize| color[i][0] as u32 + color[i][1] as u32 + color[i][2] as u32;
        if b(0) + b(2) >= b(1) + b(3) {
            self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            self.indices.extend_from_slice(&[base, base + 1, base + 3, base + 1, base + 2, base + 3]);
        }
    }
}

pub struct ChunkMesh {
    pub x: i32,
    pub z: i32,
    pub min_y: f32,
    pub max_y: f32,
    pub opaque: Geometry,
    pub translucent: Geometry,
}

/// A chunk plus its 8 neighbors (any of which may not be loaded yet).
pub struct Area {
    pub center: Arc<Chunk>,
    /// Indexed `(dz + 1) * 3 + (dx + 1)`; the center slot is ignored.
    pub around: [Option<Arc<Chunk>>; 9],
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

const WATER_TOP: f32 = 14.0 / 16.0;
const WATER_TINT: [u8; 3] = [0x3f, 0x76, 0xe4];
const AO: [f32; 4] = [0.5, 0.67, 0.83, 1.0];
/// Torch light is warmer than daylight.
const BLOCK_LIGHT: [f32; 3] = [1.0, 0.88, 0.72];

/// Light level (0..15, may be fractional from averaging) to brightness, close to vanilla at 50% gamma.
fn curve(level: f32) -> f32 {
    let b = (level / 15.0).clamp(0.0, 1.0);
    let dark = b / (4.0 - 3.0 * b);
    let bright = 1.0 - (1.0 - dark).powi(4);
    0.03 + 0.97 * (dark + bright) * 0.5
}

fn lit(tint: [u8; 3], shade: f32, sky: f32, block: f32, ao: f32, alpha: u8) -> [u8; 4] {
    let (s, b) = (curve(sky), curve(block));
    let mut out = [0u8; 4];
    for i in 0..3 {
        let l = s.max(b * BLOCK_LIGHT[i]);
        out[i] = (tint[i] as f32 * shade * l * ao).min(255.0) as u8;
    }
    out[3] = alpha;
    out
}

fn axis(d: Dir) -> usize {
    match d {
        Dir::West | Dir::East => 0,
        Dir::Down | Dir::Up => 1,
        Dir::North | Dir::South => 2,
    }
}

pub fn mesh_chunk(area: &Area, models: &BlockModels) -> ChunkMesh {
    let chunk = &*area.center;
    let (ox, oz) = (chunk.x as f32 * 16.0, chunk.z as f32 * 16.0);
    let mut mesh = ChunkMesh { x: chunk.x, z: chunk.z, min_y: f32::MAX, max_y: f32::MIN, opaque: Geometry::default(), translucent: Geometry::default() };
    let opaque = |x: i32, y: i32, z: i32| area.block(x, y, z).is_some_and(|s| models.states[s as usize].opaque);

    for (si, section) in chunk.sections.iter().enumerate() {
        if section.is_air() {
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
                    let mut emitted = false;

                    for q in model.quads.iter() {
                        if let Some(dir) = q.cull {
                            let [dx, dy, dz] = dir.offset();
                            if opaque(x + dx, y + dy, z + dz) {
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
                        emitted = true;
                    }

                    if let Some(fluid) = model.fluid {
                        emitted |= fluid_faces(&mut mesh, area, models, fluid, [x, y, z], [wx, wy, wz]);
                    }
                    if emitted {
                        mesh.min_y = mesh.min_y.min(wy);
                        mesh.max_y = mesh.max_y.max(wy + 1.0);
                    }
                }
            }
        }
    }
    mesh
}

/// Per-vertex light and AO for a quad lying on a block face, sampled from the blocks in front of it.
fn smooth(area: &Area, opaque: &impl Fn(i32, i32, i32) -> bool, q: &Quad, dir: Dir, b: [i32; 3], tint: [u8; 3]) -> [[u8; 4]; 4] {
    let n = dir.offset();
    let front = [b[0] + n[0], b[1] + n[1], b[2] + n[2]];
    let a = axis(dir);
    let (u, v) = ((a + 1) % 3, (a + 2) % 3);
    let mut out = [[0u8; 4]; 4];
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

fn fluid_faces(mesh: &mut ChunkMesh, area: &Area, models: &BlockModels, fluid: Fluid, [x, y, z]: [i32; 3], [wx, wy, wz]: [f32; 3]) -> bool {
    let same = |s: u16| models.states[s as usize].fluid == Some(fluid);
    let covered = area.block(x, y + 1, z).is_some_and(same);
    let top = if covered { 1.0 } else { WATER_TOP };
    let (rect, tint, alpha) = match fluid {
        Fluid::Water => (models.water_uv, WATER_TINT, 200),
        Fluid::Lava => (models.lava_uv, [255, 255, 255], 255),
    };
    let [u0, v0, u1, v1] = rect;
    let mut any = false;
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
        let (a, b) = ([wx, wy, wz], [wx + 1.0, wy + top, wz + 1.0]);
        let pos = match dir {
            Dir::Up => [[a[0], b[1], a[2]], [a[0], b[1], b[2]], [b[0], b[1], b[2]], [b[0], b[1], a[2]]],
            Dir::Down => [[a[0], a[1], b[2]], [a[0], a[1], a[2]], [b[0], a[1], a[2]], [b[0], a[1], b[2]]],
            Dir::North => [[b[0], b[1], a[2]], [b[0], a[1], a[2]], [a[0], a[1], a[2]], [a[0], b[1], a[2]]],
            Dir::South => [[a[0], b[1], b[2]], [a[0], a[1], b[2]], [b[0], a[1], b[2]], [b[0], b[1], b[2]]],
            Dir::West => [[a[0], b[1], a[2]], [a[0], a[1], a[2]], [a[0], a[1], b[2]], [a[0], b[1], b[2]]],
            Dir::East => [[b[0], b[1], b[2]], [b[0], a[1], b[2]], [b[0], a[1], a[2]], [b[0], b[1], a[2]]],
        };
        let uv = [[u0, v0], [u0, v1], [u1, v1], [u1, v0]];
        let (sky, block) = area.light(x + dx, y + dy, z + dz);
        let c = lit(tint, dir.shade(), sky as f32, block.max(if fluid == Fluid::Lava { 15 } else { 0 }) as f32, 1.0, alpha);
        match fluid {
            Fluid::Water => mesh.translucent.quad(pos, uv, [c; 4]),
            Fluid::Lava => mesh.opaque.quad(pos, uv, [c; 4]),
        }
        any = true;
    }
    any
}
