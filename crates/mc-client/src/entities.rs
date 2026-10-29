//! Entities the server tells us about: position tracking with interpolation, and simple models.
//! Humanoids (players, zombies, skeletons) use their real skin and vanilla's part layout with a walk cycle;
//! everything else is a box of the entity's real hitbox size until proper models exist.

use crate::physics::Aabb;
use glam::Vec3;
use mc_assets::{BlockModels, Sprite};
use mc_render::Vertex;
use std::collections::HashMap;
use std::time::Instant;

pub struct Entity {
    pub kind: u32,
    pos: [f64; 3],
    prev: [f64; 3],
    moved_at: Instant,
    yaw: f32,
    head_yaw: f32,
    pitch: f32,
    /// Horizontal distance travelled, drives the walk animation.
    walked: f32,
    /// Smoothed walking speed 0..1 (vanilla's `walkAnimation` amount).
    stride: f32,
}

#[derive(Default)]
pub struct Entities {
    pub map: HashMap<i32, Entity>,
}

impl Entity {
    /// Position interpolated across the 50 ms between server updates.
    pub fn render_pos(&self, now: Instant) -> [f64; 3] {
        let t = (now.duration_since(self.moved_at).as_secs_f64() / 0.05).clamp(0.0, 1.0);
        [0, 1, 2].map(|i| self.prev[i] + (self.pos[i] - self.prev[i]) * t)
    }
}

impl Entities {
    pub fn add(&mut self, id: i32, kind: u32, pos: [f64; 3], yaw: f32, pitch: f32, head_yaw: f32) {
        let e = Entity { kind, pos, prev: pos, moved_at: Instant::now(), yaw, head_yaw, pitch, walked: 0.0, stride: 0.0 };
        self.map.insert(id, e);
    }

    fn set_pos(&mut self, id: i32, pos: [f64; 3]) {
        let Some(e) = self.map.get_mut(&id) else { return };
        let now = Instant::now();
        e.prev = e.render_pos(now);
        let (dx, dz) = (pos[0] - e.pos[0], pos[2] - e.pos[2]);
        let step = (dx * dx + dz * dz).sqrt() as f32;
        e.walked += step;
        e.stride = e.stride * 0.6 + (step * 4.0).min(1.0) * 0.4;
        e.pos = pos;
        e.moved_at = now;
    }

    pub fn moved(&mut self, id: i32, steps: &[[i16; 3]], rot: Option<[f32; 2]>) {
        let Some(e) = self.map.get(&id) else { return };
        let mut p = e.pos;
        for s in steps {
            for i in 0..3 {
                p[i] += s[i] as f64 / 4096.0;
            }
        }
        self.set_pos(id, p);
        if let (Some([yaw, pitch]), Some(e)) = (rot, self.map.get_mut(&id)) {
            e.yaw = yaw;
            e.pitch = pitch;
        }
    }

    pub fn teleported(&mut self, id: i32, pos: [f64; 3], rot: [f32; 2]) {
        self.set_pos(id, pos);
        if let Some(e) = self.map.get_mut(&id) {
            (e.yaw, e.pitch) = (rot[0], rot[1]);
        }
    }

    pub fn rotated(&mut self, id: i32, rot: [f32; 2]) {
        if let Some(e) = self.map.get_mut(&id) {
            (e.yaw, e.pitch) = (rot[0], rot[1]);
        }
    }

    pub fn head(&mut self, id: i32, yaw: f32) {
        if let Some(e) = self.map.get_mut(&id) {
            e.head_yaw = yaw;
        }
    }

    /// Hitboxes for targeting, at their interpolated positions.
    pub fn hitboxes<'a>(&'a self, models: &'a BlockModels, now: Instant) -> impl Iterator<Item = (i32, Aabb)> + 'a {
        self.map.iter().filter_map(move |(id, e)| {
            let info = models.entities.get(e.kind as usize)?;
            let [x, y, z] = e.render_pos(now);
            let h = info.width as f64 / 2.0;
            Some((*id, Aabb { min: [x - h, y, z - h], max: [x + h, y + info.height as f64, z + h] }))
        })
    }
}

/// Skin for humanoid entity types, by registry name.
fn humanoid_skin(name: &str) -> Option<(&'static str, bool)> {
    // (sprite, thin skeleton limbs)
    Some(match name {
        "player" => ("steve", false),
        "zombie" | "zombie_villager" | "giant" => ("zombie", false),
        "husk" => ("husk", false),
        "drowned" => ("drowned", false),
        "skeleton" | "bogged" => ("skeleton", true),
        "stray" => ("stray", true),
        "wither_skeleton" => ("wither_skeleton", true),
        _ => return None,
    })
}

/// Placeholder colors for entities without a model yet.
fn box_color(name: &str, category: &str) -> [u8; 3] {
    if name == "item" || name == "experience_orb" {
        return [230, 200, 60];
    }
    let h = name.bytes().fold(2166136261u32, |a, b| (a ^ b as u32).wrapping_mul(16777619));
    let jitter = |s: u32| ((h >> s) & 0x3f) as u8;
    match category {
        "monster" => [140 + jitter(0), 40 + jitter(6) / 2, 40 + jitter(12) / 2],
        "creature" | "ambient" | "water_creature" | "water_ambient" | "underground_water_creature" | "axolotls" => {
            [120 + jitter(0), 100 + jitter(6), 70 + jitter(12)]
        }
        _ => [110 + jitter(0), 110 + jitter(6), 120 + jitter(12)],
    }
}

struct Frame {
    origin: Vec3,
    yaw: f32,
    scale: f32,
}

impl Frame {
    /// Model pixels (x right, y up, z forward when facing yaw) to world.
    fn world(&self, p: Vec3) -> [f32; 3] {
        let (s, c) = (-self.yaw.to_radians()).sin_cos();
        let p = p * self.scale;
        // Minecraft yaw 0 faces +Z; rotating by -yaw about Y turns model forward (+Z) into the facing direction.
        (self.origin + Vec3::new(p.x * c + p.z * s, p.y, -p.x * s + p.z * c)).into()
    }
}

fn rot_x(p: Vec3, pivot: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    let d = p - pivot;
    pivot + Vec3::new(d.x, d.y * c - d.z * s, d.y * s + d.z * c)
}

fn rot_y(p: Vec3, pivot: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    let d = p - pivot;
    pivot + Vec3::new(d.x * c + d.z * s, d.y, -d.x * s + d.z * c)
}

/// One model part: a box `size` at `from` (pixels), skin UV origin `uv`, rotated about `pivot`.
#[allow(clippy::too_many_arguments)]
fn cuboid(out: &mut Vec<Vertex>, f: &Frame, skin: &Sprite, tex: (f32, f32), from: Vec3, size: Vec3, uv: (f32, f32), pivot: Vec3, pitch: f32, yaw: f32, light: f32) {
    let (w, h, d) = (size.x, size.y, size.z);
    let (u, v) = uv;
    let [su0, sv0, su1, sv1] = skin.uv;
    let px = |x: f32, y: f32| [su0 + (su1 - su0) * x / tex.0, sv0 + (sv1 - sv0) * y / tex.1];
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| [px(x0, y0), px(x0, y1), px(x1, y1), px(x1, y0)];
    let t = |p: Vec3| f.world(rot_y(rot_x(p, pivot, pitch), pivot, yaw));
    let (x0, y0, z0) = (from.x, from.y, from.z);
    let (x1, y1, z1) = (x0 + w, y0 + h, z0 + d);
    let c = |shade: f32| {
        let v = (255.0 * light * shade) as u8;
        [v, v, v, 255]
    };
    // Corners top-left, bottom-left, bottom-right, top-right as seen from outside; UV regions of the standard box layout.
    let faces: [([Vec3; 4], [[f32; 2]; 4], f32); 6] = [
        ([Vec3::new(x0, y1, z1), Vec3::new(x0, y0, z1), Vec3::new(x1, y0, z1), Vec3::new(x1, y1, z1)], rect(u + d, v + d, u + d + w, v + d + h), 0.8),
        ([Vec3::new(x1, y1, z0), Vec3::new(x1, y0, z0), Vec3::new(x0, y0, z0), Vec3::new(x0, y1, z0)], rect(u + 2. * d + w, v + d, u + 2. * (d + w), v + d + h), 0.8),
        ([Vec3::new(x0, y1, z0), Vec3::new(x0, y0, z0), Vec3::new(x0, y0, z1), Vec3::new(x0, y1, z1)], rect(u, v + d, u + d, v + d + h), 0.6),
        ([Vec3::new(x1, y1, z1), Vec3::new(x1, y0, z1), Vec3::new(x1, y0, z0), Vec3::new(x1, y1, z0)], rect(u + d + w, v + d, u + 2. * d + w, v + d + h), 0.6),
        ([Vec3::new(x0, y1, z0), Vec3::new(x0, y1, z1), Vec3::new(x1, y1, z1), Vec3::new(x1, y1, z0)], rect(u + d, v, u + d + w, v + d), 1.0),
        ([Vec3::new(x0, y0, z1), Vec3::new(x0, y0, z0), Vec3::new(x1, y0, z0), Vec3::new(x1, y0, z1)], rect(u + d + w, v, u + d + 2. * w, v + d), 0.5),
    ];
    for (p, uv, shade) in faces {
        let col = c(shade);
        let vert = |i: usize| Vertex { pos: t(p[i]), uv: uv[i], color: col };
        out.extend_from_slice(&[vert(0), vert(1), vert(2), vert(0), vert(2), vert(3)]);
    }
}

/// Builds all entity geometry (textured from the GUI atlas) for this frame.
pub fn mesh(entities: &Entities, models: &BlockModels, now: Instant, light_at: impl Fn([f64; 3]) -> f32) -> Vec<Vertex> {
    let mut out = Vec::new();
    for e in entities.map.values() {
        let Some(info) = models.entities.get(e.kind as usize) else { continue };
        let [x, y, z] = e.render_pos(now);
        let light = light_at([x, y + 0.5, z]);
        let origin = Vec3::new(x as f32, y as f32, z as f32);
        if let Some((skin_name, thin)) = humanoid_skin(&info.name) {
            let skin = models.gui.sprite(skin_name);
            let tex = (64.0, if skin.h < 64.0 { 32.0 } else { 64.0 });
            let modern = tex.1 == 64.0;
            let f = Frame { origin, yaw: e.yaw, scale: info.height / 32.0 };
            let swing = (e.walked * 0.6662 * 16.0 / 9.0).cos() * 1.2 * e.stride;
            let head_yaw = (e.head_yaw - e.yaw).to_radians();
            let pitch = e.pitch.to_radians();
            let zombie_arms = matches!(skin_name, "zombie" | "husk" | "drowned");
            let arm_pitch = |side: f32| if zombie_arms { -1.5 } else { swing * side };
            let lw = if thin { 2.0 } else { 4.0 };
            let off = (4.0 - lw) / 2.0;
            cuboid(&mut out, &f, &skin, tex, Vec3::new(-4., 24., -4.), Vec3::splat(8.), (0., 0.), Vec3::new(0., 24., 0.), pitch, -head_yaw, light);
            cuboid(&mut out, &f, &skin, tex, Vec3::new(-4., 12., -2.), Vec3::new(8., 12., 4.), (16., 16.), Vec3::ZERO, 0., 0., light);
            let limb = Vec3::new(lw, 12., lw);
            // Right side (-x) uses the classic UVs; 64x64 skins have separate left-limb UVs.
            cuboid(&mut out, &f, &skin, tex, Vec3::new(-4. - lw, 12., -lw / 2.), limb, (40., 16.), Vec3::new(-4. - lw / 2., 22., 0.), arm_pitch(1.0), 0., light);
            let left_arm_uv = if modern && !thin { (32., 48.) } else { (40., 16.) };
            cuboid(&mut out, &f, &skin, tex, Vec3::new(4., 12., -lw / 2.), limb, left_arm_uv, Vec3::new(4. + lw / 2., 22., 0.), arm_pitch(-1.0), 0., light);
            cuboid(&mut out, &f, &skin, tex, Vec3::new(-4. + off, 0., -lw / 2.), limb, (0., 16.), Vec3::new(-2., 12., 0.), -swing, 0., light);
            let left_leg_uv = if modern && !thin { (16., 48.) } else { (0., 16.) };
            cuboid(&mut out, &f, &skin, tex, Vec3::new(off, 0., -lw / 2.), limb, left_leg_uv, Vec3::new(2., 12., 0.), swing, 0., light);
        } else {
            let white = models.gui.sprite("white");
            let [r, g, b] = box_color(&info.name, &info.category);
            let s = 16.0;
            let f = Frame { origin, yaw: e.yaw, scale: 1.0 / s };
            let (w, h) = (info.width * s, info.height * s);
            let start = out.len();
            cuboid(&mut out, &f, &white, (1.0, 1.0), Vec3::new(-w / 2., 0., -w / 2.), Vec3::new(w, h, w), (0., 0.), Vec3::ZERO, 0., 0., light);
            for v in &mut out[start..] {
                v.uv = [(white.uv[0] + white.uv[2]) / 2.0, (white.uv[1] + white.uv[3]) / 2.0];
                v.color = [(r as f32 * v.color[0] as f32 / 255.0) as u8, (g as f32 * v.color[1] as f32 / 255.0) as u8, (b as f32 * v.color[2] as f32 / 255.0) as u8, 255];
            }
        }
    }
    out
}
