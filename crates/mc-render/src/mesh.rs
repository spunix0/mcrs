//! Turns a chunk column into vertex/index lists: copies each block's baked quads, skipping faces hidden by opaque neighbors.

use bytemuck::{Pod, Zeroable};
use mc_assets::{BlockModels, Dir, Fluid};
use mc_world::Chunk;

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
    fn quad(&mut self, pos: [[f32; 3]; 4], uv: [[f32; 2]; 4], color: [u8; 4]) {
        let base = self.vertices.len() as u32;
        for i in 0..4 {
            self.vertices.push(Vertex { pos: pos[i], uv: uv[i], color });
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
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

fn color(tint: Option<[u8; 3]>, shade: f32, alpha: u8) -> [u8; 4] {
    let [r, g, b] = tint.unwrap_or([255, 255, 255]);
    let s = |c: u8| (c as f32 * shade) as u8;
    [s(r), s(g), s(b), alpha]
}

const WATER_TOP: f32 = 14.0 / 16.0;
const WATER_TINT: [u8; 3] = [0x3f, 0x76, 0xe4];

pub fn mesh_chunk(chunk: &Chunk, models: &BlockModels) -> ChunkMesh {
    let (ox, oz) = (chunk.x as f32 * 16.0, chunk.z as f32 * 16.0);
    let mut mesh = ChunkMesh { x: chunk.x, z: chunk.z, min_y: f32::MAX, max_y: f32::MIN, opaque: Geometry::default(), translucent: Geometry::default() };
    // Neighbors outside this column are unknown: `None`.
    let at = |x: i32, y: i32, z: i32| -> Option<u16> {
        if (0..16).contains(&x) && (0..16).contains(&z) { Some(chunk.get(x as usize, y, z as usize)) } else { None }
    };

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
                            if at(x + dx, y + dy, z + dz).is_some_and(|n| models.states[n as usize].opaque) {
                                continue;
                            }
                        }
                        let pos = q.pos.map(|p| [p[0] + wx, p[1] + wy, p[2] + wz]);
                        let tint = q.tinted.then_some(model.tint);
                        let target = if q.translucent { &mut mesh.translucent } else { &mut mesh.opaque };
                        target.quad(pos, q.uv, color(tint, q.shade, 255));
                        emitted = true;
                    }

                    if let Some(fluid) = model.fluid {
                        emitted |= fluid_faces(&mut mesh, models, fluid, &at, [x, y, z], [wx, wy, wz]);
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

fn fluid_faces(
    mesh: &mut ChunkMesh,
    models: &BlockModels,
    fluid: Fluid,
    at: &impl Fn(i32, i32, i32) -> Option<u16>,
    [x, y, z]: [i32; 3],
    [wx, wy, wz]: [f32; 3],
) -> bool {
    let same = |s: u16| models.states[s as usize].fluid == Some(fluid);
    let covered = at(x, y + 1, z).is_some_and(same);
    let top = if covered { 1.0 } else { WATER_TOP };
    let (rect, tint, alpha) = match fluid {
        Fluid::Water => (models.water_uv, Some(WATER_TINT), 200),
        Fluid::Lava => (models.lava_uv, None, 255),
    };
    let [u0, v0, u1, v1] = rect;
    let mut any = false;
    for dir in Dir::ALL {
        let [dx, dy, dz] = dir.offset();
        let hidden = match at(x + dx, y + dy, z + dz) {
            // Unknown neighbor chunk: assume more fluid so chunk borders don't show seams.
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
        let c = color(tint, dir.shade(), alpha);
        match fluid {
            Fluid::Water => mesh.translucent.quad(pos, uv, c),
            Fluid::Lava => mesh.opaque.quad(pos, uv, c),
        }
        any = true;
    }
    any
}
