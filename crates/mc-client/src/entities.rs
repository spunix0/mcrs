//! Entities the server tells us about: position tracking with interpolation, synced data (dropped item stacks, baby
//! flags, wool), hurt flashes, and rendering with vanilla's models (`models.rs`). Types without a model yet are drawn
//! as boxes of their real hitbox size.

use crate::hand;
use crate::inventory::Stack;
use crate::models::{self, Anim, Draw, Mob};
use crate::physics::Aabb;
use glam::{Mat4, Vec3};
use mc_assets::BlockModels;
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
    /// Smoothed body yaw, turning toward where it walks/looks like vanilla's `yBodyRot`.
    body_yaw: f32,
    /// `walkAnimation`: distance-driven position and smoothed speed.
    walk_pos: f32,
    walk_speed: f32,
    spawned: Instant,
    /// Ticks of red hurt tint left.
    pub hurt: u8,
    /// Arm swing 0..1 while attacking.
    swing: f32,
    /// Synced data we use.
    pub item: Option<Stack>,
    pub baby: bool,
    pub wool: Option<u8>,
    pub size: i32,
    /// Block state for falling blocks (from the spawn packet).
    pub data: i32,
    /// Random phase for item bobbing.
    bob_offset: f32,
    /// Rendered position at the last tick, for the walk animation.
    last_tick: [f64; 3],
}

#[derive(Default)]
pub struct Entities {
    pub map: HashMap<i32, Entity>,
    /// Model definitions by entity type, built on first use.
    mobs: HashMap<u32, Option<Mob>>,
}

impl Entity {
    /// Position interpolated toward the last server update over 3 ticks, like vanilla's `lerpTo(..., 3)`
    /// (most mobs only get updates every 3 ticks, so a shorter lerp stutters).
    pub fn render_pos(&self, now: Instant) -> [f64; 3] {
        let t = (now.duration_since(self.moved_at).as_secs_f64() / 0.15).clamp(0.0, 1.0);
        [0, 1, 2].map(|i| self.prev[i] + (self.pos[i] - self.prev[i]) * t)
    }
}

/// Synced entity data entries we understand (see `session.rs`).
#[derive(Clone, Debug)]
pub enum Data {
    Item(Option<Stack>),
    Baby(bool),
    Wool(u8),
    Size(i32),
    /// Shared flags (index 0): 0x01 on fire, 0x02 sneaking, 0x20 invisible.
    Flags(u8),
}

fn wrap(a: f32) -> f32 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

impl Entities {
    pub fn add(&mut self, id: i32, kind: u32, pos: [f64; 3], yaw: f32, pitch: f32, head_yaw: f32, data: i32) {
        let e = Entity {
            kind,
            pos,
            prev: pos,
            moved_at: Instant::now(),
            yaw,
            head_yaw,
            pitch,
            body_yaw: yaw,
            walk_pos: 0.0,
            walk_speed: 0.0,
            spawned: Instant::now(),
            hurt: 0,
            swing: 0.0,
            item: None,
            baby: false,
            wool: None,
            size: 1,
            data,
            last_tick: pos,
            bob_offset: ((id as u32).wrapping_mul(2654435761) % 1000) as f32 / 1000.0 * std::f32::consts::TAU,
        };
        self.map.insert(id, e);
    }

    fn set_pos(&mut self, id: i32, pos: [f64; 3]) {
        let Some(e) = self.map.get_mut(&id) else { return };
        let now = Instant::now();
        e.prev = e.render_pos(now);
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

    pub fn set_data(&mut self, id: i32, data: Vec<Data>) {
        let Some(e) = self.map.get_mut(&id) else { return };
        for d in data {
            match d {
                Data::Item(s) => e.item = s,
                Data::Baby(b) => e.baby = b,
                Data::Wool(w) => e.wool = Some(w),
                Data::Size(s) => e.size = s.max(1),
                Data::Flags(_) => {}
            }
        }
    }

    pub fn hurt(&mut self, id: i32) {
        if let Some(e) = self.map.get_mut(&id) {
            e.hurt = 10;
        }
    }

    pub fn swing(&mut self, id: i32) {
        if let Some(e) = self.map.get_mut(&id) {
            e.swing = 0.001;
        }
    }

    /// 20 Hz: walk animation, body turning, hurt and swing timers (vanilla's `LivingEntity.tick` parts).
    pub fn tick(&mut self) {
        let now = Instant::now();
        for e in self.map.values_mut() {
            // How far it really moved this tick (the old code reused the last packet's step forever).
            let p = e.render_pos(now);
            let (dx, dz) = ((p[0] - e.last_tick[0]) as f32, (p[2] - e.last_tick[2]) as f32);
            e.last_tick = p;
            let dist = (dx * dx + dz * dz).sqrt();
            // WalkAnimationState.update: speed eases toward min(distance * 4, 1), position advances by speed.
            let target = (dist * 4.0).min(1.0);
            e.walk_speed += (target - e.walk_speed) * 0.4;
            e.walk_pos += e.walk_speed;
            // Body follows the head within 50 degrees, and turns to the walking direction.
            if dist > 0.01 {
                let walk_yaw = (-dx).atan2(dz).to_degrees();
                e.body_yaw += wrap(walk_yaw - e.body_yaw) * 0.3;
            }
            let d = wrap(e.head_yaw - e.body_yaw);
            if d.abs() > 50.0 {
                e.body_yaw += d - 50.0 * d.signum();
            }
            e.hurt = e.hurt.saturating_sub(1);
            if e.swing > 0.0 {
                e.swing += 1.0 / 6.0;
                if e.swing >= 1.0 {
                    e.swing = 0.0;
                }
            }
        }
    }

    /// Hitboxes for targeting, at their interpolated positions.
    pub fn hitboxes<'a>(&'a self, models: &'a BlockModels, now: Instant) -> impl Iterator<Item = (i32, Aabb)> + 'a {
        self.map.iter().filter_map(move |(id, e)| {
            let info = models.entities.get(e.kind as usize)?;
            if matches!(info.name.as_str(), "item" | "experience_orb" | "arrow" | "falling_block") {
                return None;
            }
            let [x, y, z] = e.render_pos(now);
            let s = if e.baby { 0.5 } else { 1.0 } * if info.name == "slime" || info.name == "magma_cube" { e.size as f64 } else { 1.0 };
            let h = info.width as f64 / 2.0 * s;
            Some((*id, Aabb { min: [x - h, y, z - h], max: [x + h, y + info.height as f64 * s, z + h] }))
        })
    }
}

/// Placeholder colors for entities without a model yet.
fn box_color(name: &str, category: &str) -> [u8; 3] {
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

/// Wool colors by dye index (`DyeColor.textureDiffuseColor`).
const DYE: [[u8; 3]; 16] = [
    [0xf9, 0xff, 0xfe], [0xf9, 0x80, 0x1d], [0xc7, 0x4e, 0xbd], [0x3a, 0xb3, 0xda], [0xfe, 0xd8, 0x3d], [0x80, 0xc7, 0x1f],
    [0xf3, 0x8b, 0xaa], [0x47, 0x4f, 0x52], [0x9d, 0x9d, 0x97], [0x16, 0x9c, 0x9c], [0x89, 0x32, 0xb8], [0x3c, 0x44, 0xaa],
    [0x83, 0x54, 0x32], [0x5e, 0x7c, 0x16], [0xb0, 0x2e, 0x26], [0x1d, 0x1d, 0x21],
];

/// Builds this frame's entity geometry: `skins` uses the GUI atlas, `items` the block atlas.
pub fn mesh(entities: &mut Entities, models: &BlockModels, now: Instant, light_at: impl Fn([f64; 3]) -> f32, skins: &mut Vec<Vertex>, items: &mut Vec<Vertex>, skip: Option<i32>) {
    let Entities { map, mobs } = entities;
    for (id, e) in map.iter() {
        if Some(*id) == skip {
            continue;
        }
        let Some(info) = models.entities.get(e.kind as usize) else { continue };
        let [x, y, z] = e.render_pos(now);
        let light = light_at([x, y + 0.5, z]);
        let origin = Vec3::new(x as f32, y as f32, z as f32);
        let age = now.duration_since(e.spawned).as_secs_f32() * 20.0;
        match info.name.as_str() {
            "item" => {
                if let Some(item) = e.item.as_ref().and_then(|s| models.items.get(s.item as usize).map(|i| (i, s.count))) {
                    hand::dropped(items, item.0, origin, age, e.bob_offset, item.1, light);
                }
                continue;
            }
            "falling_block" | "tnt" => {
                let state = if info.name == "tnt" { models.states.iter().position(|s| &*s.block == "tnt").unwrap_or(0) } else { e.data as usize };
                if let Some(s) = models.states.get(state) {
                    hand::block(items, &s.quads, origin, light, s.tint);
                }
                continue;
            }
            "experience_orb" => {
                // A billboard from the 4x4 orb sheet, color pulsing green-yellow like vanilla.
                let s = models.gui.sprite("experience_orb");
                let t = age / 2.0;
                let r = ((t.sin() + 1.0) * 0.5 * 255.0) as u8;
                let b = (((t + 4.1888).sin() + 1.0) * 0.1 * 255.0) as u8;
                let (cell_w, cell_h) = ((s.uv[2] - s.uv[0]) / 4.0, (s.uv[3] - s.uv[1]) / 4.0);
                let uv = [s.uv[0], s.uv[1], s.uv[0] + cell_w, s.uv[1] + cell_h];
                let c = origin + Vec3::new(0.0, 0.25, 0.0);
                let half = 0.15;
                let v = |p: Vec3, u: [f32; 2]| Vertex { pos: p.into(), uv: u, color: [r, 255, b, 255] };
                // Two crossed quads so it's visible from any side.
                for axis in [Vec3::X, Vec3::Z] {
                    let q = [
                        v(c - axis * half + Vec3::Y * half, [uv[0], uv[1]]),
                        v(c - axis * half - Vec3::Y * half, [uv[0], uv[3]]),
                        v(c + axis * half - Vec3::Y * half, [uv[2], uv[3]]),
                        v(c + axis * half + Vec3::Y * half, [uv[2], uv[1]]),
                    ];
                    skins.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
                }
                continue;
            }
            _ => {}
        }
        let mob = mobs.entry(e.kind).or_insert_with(|| models::for_entity(&info.name));
        let tint = if e.hurt > 0 { [1.0, 0.6, 0.6] } else { [1.0; 3] };
        match mob {
            Some(mob) => {
                let scale = mob.scale * if e.baby { 0.5 } else { 1.0 } * if mob.kind == models::Kind::Slime { e.size as f32 * 0.999 } else { 1.0 };
                let m: Mat4 = models::entity_matrix(origin, e.body_yaw, scale);
                let anim = Anim {
                    walk_pos: e.walk_pos,
                    walk_speed: e.walk_speed,
                    head_yaw: wrap(e.head_yaw - e.body_yaw).to_radians(),
                    head_pitch: e.pitch.to_radians(),
                    age,
                    attack: (e.swing * std::f32::consts::PI).sin(),
                };
                let kind = mob.kind;
                let sprite = models.gui.sprite(mob.skin);
                let mut d = Draw { out: skins, sprite, light, tint };
                mob.model.render(m, &|name, rot| models::animate(kind, anim, name, rot), &mut d);
                if kind == models::Kind::Sheep {
                    // Unsheared sheep (wool bit 0x10 clear) wear the dyed wool layer.
                    let wool = e.wool.unwrap_or(0);
                    if wool & 0x10 == 0 {
                        let c = DYE[(wool & 15) as usize];
                        let wool_tint = [c[0] as f32 / 255.0 * tint[0], c[1] as f32 / 255.0 * tint[1], c[2] as f32 / 255.0 * tint[2]];
                        let mut d = Draw { out: skins, sprite: models.gui.sprite("sheep_wool"), light, tint: wool_tint };
                        models::sheep_wool().render(m, &|name, rot| models::animate(kind, anim, name, rot), &mut d);
                    }
                }
            }
            None => {
                let white = models.gui.sprite("white");
                let [r, g, b] = box_color(&info.name, &info.category);
                let mid = [(white.uv[0] + white.uv[2]) / 2.0, (white.uv[1] + white.uv[3]) / 2.0];
                let (w, h) = (info.width / 2.0, info.height);
                let m = Mat4::from_translation(origin) * Mat4::from_rotation_y((-e.yaw).to_radians());
                let corner = |cx: f32, cy: f32, cz: f32| m.transform_point3(Vec3::new(cx * w, cy * h, cz * w));
                let faces: [([Vec3; 4], f32); 6] = [
                    ([corner(-1., 1., -1.), corner(-1., 1., 1.), corner(1., 1., 1.), corner(1., 1., -1.)], 1.0),
                    ([corner(-1., 0., 1.), corner(-1., 0., -1.), corner(1., 0., -1.), corner(1., 0., 1.)], 0.5),
                    ([corner(1., 1., -1.), corner(1., 0., -1.), corner(-1., 0., -1.), corner(-1., 1., -1.)], 0.8),
                    ([corner(-1., 1., 1.), corner(-1., 0., 1.), corner(1., 0., 1.), corner(1., 1., 1.)], 0.8),
                    ([corner(-1., 1., -1.), corner(-1., 0., -1.), corner(-1., 0., 1.), corner(-1., 1., 1.)], 0.6),
                    ([corner(1., 1., 1.), corner(1., 0., 1.), corner(1., 0., -1.), corner(1., 1., -1.)], 0.6),
                ];
                for (p, shade) in faces {
                    let f = light * shade;
                    let col = [(r as f32 * f * tint[1]) as u8, (g as f32 * f * tint[1]) as u8, (b as f32 * f * tint[2]) as u8, 255];
                    let v = |i: usize| Vertex { pos: p[i].into(), uv: mid, color: col };
                    skins.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
                }
            }
        }
    }
}
