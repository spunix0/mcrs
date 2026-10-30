//! Block particles like vanilla's `TerrainParticle`: the 4x4x4 burst when a block breaks and the small chips while
//! mining. Simulated at 20 Hz and drawn as camera-facing quads from the block atlas.

use glam::Vec3;
use mc_render::Vertex;

struct Particle {
    pos: Vec3,
    prev: Vec3,
    vel: Vec3,
    age: u32,
    life: u32,
    size: f32,
    /// Atlas UVs of the 4x4-pixel piece of texture this particle shows.
    uv: [f32; 4],
    tint: [u8; 3],
    on_ground: bool,
}

#[derive(Default)]
pub struct Particles {
    list: Vec<Particle>,
    rng: u64,
}

impl Particles {
    fn random(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.rng >> 40) as f32) / (1u64 << 24) as f32
    }

    /// A random 1/4-size piece of `rect` (a block face's texture).
    fn piece(&mut self, rect: [f32; 4]) -> [f32; 4] {
        let (uo, vo) = (self.random() * 3.0, self.random() * 3.0);
        let (w, h) = ((rect[2] - rect[0]) / 16.0, (rect[3] - rect[1]) / 16.0);
        let (u0, v0) = (rect[0] + w * uo * 4.0, rect[1] + h * vo * 4.0);
        [u0, v0, u0 + w * 4.0, v0 + h * 4.0]
    }

    /// `Particle(level, x, y, z, xa, ya, za)`: random spread around the given velocity, then normalized.
    fn spawn(&mut self, pos: Vec3, vel: Vec3, rect: [f32; 4], tint: [u8; 3], power: f32, scale: f32) {
        let mut v = vel + Vec3::new(self.random() * 2.0 - 1.0, self.random() * 2.0 - 1.0, self.random() * 2.0 - 1.0) * 0.4;
        let speed = (self.random() + self.random() + 1.0) * 0.15;
        let len = v.length().max(1e-4);
        v = v / len * speed * 0.4;
        v.y += 0.1;
        v *= power;
        if power != 1.0 {
            v.y = (v.y - 0.1) * power + 0.1;
        }
        let life = (4.0 / (self.random() * 0.9 + 0.1)) as u32;
        let size = 0.1 * (self.random() * 0.5 + 0.5) * 2.0 / 2.0 * scale;
        let uv = self.piece(rect);
        if self.list.len() < 4000 {
            self.list.push(Particle { pos, prev: pos, vel: v, age: 0, life, size, uv, tint, on_ground: false });
        }
    }

    /// `ParticleEngine.destroy`: 64 particles spread over the block.
    pub fn destroy(&mut self, pos: [i32; 3], rect: [f32; 4], tint: [u8; 3]) {
        let base = Vec3::new(pos[0] as f32, pos[1] as f32, pos[2] as f32);
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    let d = Vec3::new((x as f32 + 0.5) / 4.0, (y as f32 + 0.5) / 4.0, (z as f32 + 0.5) / 4.0);
                    self.spawn(base + d, d - Vec3::splat(0.5), rect, tint, 1.0, 1.0);
                }
            }
        }
    }

    /// `ParticleEngine.crack`: one chip on the face being mined.
    pub fn crack(&mut self, pos: [i32; 3], face: u8, rect: [f32; 4], tint: [u8; 3]) {
        let b = Vec3::new(pos[0] as f32, pos[1] as f32, pos[2] as f32);
        let mut p = b + Vec3::new(self.random(), self.random(), self.random()) * 0.8 + Vec3::splat(0.1);
        let o = 0.1;
        match face {
            0 => p.y = b.y - o,
            1 => p.y = b.y + 1.0 + o,
            2 => p.z = b.z - o,
            3 => p.z = b.z + 1.0 + o,
            4 => p.x = b.x - o,
            _ => p.x = b.x + 1.0 + o,
        }
        self.spawn(p, Vec3::ZERO, rect, tint, 0.2, 0.6);
    }

    /// One tick: gravity, drag and a simple stop on solid blocks.
    pub fn tick(&mut self, solid: impl Fn(i32, i32, i32) -> bool) {
        self.list.retain_mut(|p| {
            p.prev = p.pos;
            p.age += 1;
            if p.age >= p.life {
                return false;
            }
            p.vel.y -= 0.04;
            let next = p.pos + p.vel;
            let below = |v: Vec3| solid(v.x.floor() as i32, v.y.floor() as i32, v.z.floor() as i32);
            if below(next) {
                // Landed (or hit a wall): stop moving into the block.
                if !below(Vec3::new(next.x, p.pos.y, next.z)) {
                    p.pos.x = next.x;
                    p.pos.z = next.z;
                }
                p.on_ground = p.vel.y < 0.0;
                p.vel.y = 0.0;
            } else {
                p.pos = next;
                p.on_ground = false;
            }
            p.vel *= 0.98;
            if p.on_ground {
                p.vel.x *= 0.7;
                p.vel.z *= 0.7;
            }
            true
        });
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// Camera-facing quads, interpolated `t` (0..1) between ticks.
    pub fn mesh(&self, right: Vec3, up: Vec3, t: f32, light: impl Fn(Vec3) -> f32) -> Vec<Vertex> {
        let mut out = Vec::with_capacity(self.list.len() * 6);
        for p in &self.list {
            let c = p.prev.lerp(p.pos, t);
            let b = light(c) * 0.6 + 0.0;
            let col = [(p.tint[0] as f32 * b) as u8, (p.tint[1] as f32 * b) as u8, (p.tint[2] as f32 * b) as u8, 255];
            let (r, u) = (right * p.size, up * p.size);
            let [u0, v0, u1, v1] = p.uv;
            let v = |pos: Vec3, uv: [f32; 2]| Vertex { pos: pos.into(), uv, color: col };
            let q = [v(c - r + u, [u0, v0]), v(c - r - u, [u0, v1]), v(c + r - u, [u1, v1]), v(c + r + u, [u1, v0])];
            out.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        out
    }
}

impl Particles {
    pub fn len(&self) -> usize {
        self.list.len()
    }
}
