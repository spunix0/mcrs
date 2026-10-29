//! First-person hand: Steve's right arm when empty-handed, otherwise the held item (flat sprite or small cube).
//! Built in camera space (x right, y up, z forward) and turned into world-space triangles for the hand pass.

use glam::Vec3;
use mc_assets::{ItemInfo, Sprite};
use mc_render::Vertex;

pub struct Pose {
    pub eye: Vec3,
    pub look: Vec3,
    /// 0..1 through an arm swing (mining, using).
    pub swing: f32,
    /// Walk distance, for view bobbing.
    pub walked: f32,
    /// 0..1 brightness at the player.
    pub light: f32,
}

struct Basis {
    origin: Vec3,
    r: Vec3,
    u: Vec3,
    f: Vec3,
}

impl Basis {
    fn new(p: &Pose) -> Self {
        let f = p.look.normalize();
        let r = f.cross(Vec3::Y).normalize();
        let u = r.cross(f);
        Self { origin: p.eye, r, u, f }
    }

    fn world(&self, c: Vec3) -> [f32; 3] {
        (self.origin + self.r * c.x + self.u * c.y + self.f * c.z).into()
    }
}

fn shade(light: f32, face: f32) -> [u8; 4] {
    let v = (255.0 * light * face).clamp(0.0, 255.0) as u8;
    [v, v, v, 255]
}

fn quad(out: &mut Vec<Vertex>, b: &Basis, corners: [Vec3; 4], uv: [[f32; 2]; 4], color: [u8; 4]) {
    let v = |i: usize| Vertex { pos: b.world(corners[i]), uv: uv[i], color };
    out.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
}

/// Swing and walk bob, applied to the hand position.
fn offset(p: &Pose) -> Vec3 {
    let s = (p.swing * std::f32::consts::PI).sin();
    let bob = (p.walked * std::f32::consts::PI).sin();
    Vec3::new(-0.12 * s + 0.015 * bob, -0.1 * s + 0.02 * bob.abs(), 0.06 * s)
}

/// Steve's right arm (64x64 skin, wide model): 4x4x12 pixels at skin (40, 16).
pub fn arm(out: &mut Vec<Vertex>, p: &Pose, skin: &Sprite) {
    let b = Basis::new(p);
    let along = Vec3::new(-0.28, 0.42, 1.0).normalize();
    let tip = Vec3::new(0.38, -0.33, 0.62) + offset(p);
    let shoulder = tip - along * 0.75;
    let w1 = along.cross(Vec3::Y).normalize() * 0.125;
    let w2 = w1.cross(along).normalize() * 0.125;
    let [u0, v0, u1, v1] = skin.uv;
    let px = |x: f32, y: f32| [u0 + (u1 - u0) * x / 64.0, v0 + (v1 - v0) * y / 64.0];
    // A 4x12 strip of the skin: u from x0, v 20 (shoulder) to 32 (hand).
    let side = |x0: f32| [px(x0, 20.0), px(x0, 32.0), px(x0 + 4.0, 32.0), px(x0 + 4.0, 20.0)];
    for (a, c, x0, f) in [(w1, w2, 40.0, 0.6), (w2, -w1, 44.0, 0.9), (-w1, -w2, 48.0, 0.6), (-w2, w1, 52.0, 0.75)] {
        // Face pointing along `a`, spanning toward `c`.
        let corners = [shoulder + a - c, tip + a - c, tip + a + c, shoulder + a + c];
        quad(out, &b, corners, side(x0), shade(p.light, f));
    }
    let cap = [px(48.0, 16.0), px(48.0, 20.0), px(52.0, 20.0), px(52.0, 16.0)];
    quad(out, &b, [tip - w1 - w2, tip - w1 + w2, tip + w1 + w2, tip + w1 - w2], cap, shade(p.light, 1.0));
}

/// The held item: a flat, tilted sprite, or a cube for block items.
pub fn item(out: &mut Vec<Vertex>, p: &Pose, item: &ItemInfo) {
    let Some([u0, v0, u1, v1]) = item.icon else { return };
    let b = Basis::new(p);
    let tint = item.tint.unwrap_or([255, 255, 255]);
    let color = |f: f32| {
        let c = shade(p.light, f);
        [(c[0] as u32 * tint[0] as u32 / 255) as u8, (c[1] as u32 * tint[1] as u32 / 255) as u8, (c[2] as u32 * tint[2] as u32 / 255) as u8, 255]
    };
    let uv = [[u0, v0], [u0, v1], [u1, v1], [u1, v0]];
    if item.block {
        let c = Vec3::new(0.36, -0.3, 0.7) + offset(p);
        let s = 0.13;
        // Turned 45 degrees around Y and tipped slightly toward the camera, like vanilla's held blocks.
        let (sy, cy) = 45f32.to_radians().sin_cos();
        let (sx, cx) = 20f32.to_radians().sin_cos();
        let rot = |v: Vec3| {
            let v = Vec3::new(v.x * cy + v.z * sy, v.y, -v.x * sy + v.z * cy);
            Vec3::new(v.x, v.y * cx - v.z * sx, v.y * sx + v.z * cx)
        };
        let corner = |x: f32, y: f32, z: f32| c + rot(Vec3::new(x, y, z) * s);
        let faces: [([Vec3; 4], f32); 6] = [
            ([corner(-1., 1., -1.), corner(-1., 1., 1.), corner(1., 1., 1.), corner(1., 1., -1.)], 1.0),
            ([corner(-1., -1., 1.), corner(-1., -1., -1.), corner(1., -1., -1.), corner(1., -1., 1.)], 0.5),
            ([corner(1., 1., -1.), corner(1., -1., -1.), corner(-1., -1., -1.), corner(-1., 1., -1.)], 0.8),
            ([corner(-1., 1., 1.), corner(-1., -1., 1.), corner(1., -1., 1.), corner(1., 1., 1.)], 0.8),
            ([corner(-1., 1., -1.), corner(-1., -1., -1.), corner(-1., -1., 1.), corner(-1., 1., 1.)], 0.6),
            ([corner(1., 1., 1.), corner(1., -1., 1.), corner(1., -1., -1.), corner(1., 1., -1.)], 0.6),
        ];
        for (corners, f) in faces {
            quad(out, &b, corners, uv, color(f));
        }
    } else {
        let c = Vec3::new(0.34, -0.26, 0.66) + offset(p);
        let x = Vec3::new(1.0, 0.0, -0.35).normalize() * 0.2;
        let y = Vec3::new(0.25, 1.0, 0.25).normalize() * 0.2;
        quad(out, &b, [c - x + y, c - x - y, c + x - y, c + x + y], uv, color(1.0));
    }
}
