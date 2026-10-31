//! Particles, after vanilla's particle classes: block pieces (`TerrainParticle`: the burst when a block breaks, chips
//! while mining), explosion puffs and smoke, and every type the server sends (`level_particles`) or blocks give off
//! (`animateTick`), each with its provider's motion, lifetime, size and sprites. Simulated at 20 Hz and drawn as
//! camera-facing quads from the block atlas.

use glam::Vec3;
use mc_render::Vertex;

struct Particle {
    pos: Vec3,
    prev: Vec3,
    vel: Vec3,
    age: u32,
    life: u32,
    size: f32,
    /// Atlas UVs when it shows a fixed picture (a piece of a block or item texture, or one random sprite).
    uv: [f32; 4],
    /// Plays these frames over its life instead (`SpriteSet.get(age, lifetime)`).
    frames: Option<u16>,
    color: [f32; 3],
    /// Fades to this color over its life (dust color transition).
    color_to: Option<[f32; 3]>,
    on_ground: bool,
    /// Velocity lost to gravity each tick (vanilla's `gravity * 0.04`); negative rises.
    gravity: f32,
    friction: f32,
    /// Ignores the light where it is (flames, lava, glowing particles).
    bright: bool,
    /// Stops on blocks instead of passing through.
    collide: bool,
    /// Shrinks toward the end of its life (flames, lava, crits).
    shrink: bool,
}

impl Particle {
    fn new(pos: Vec3, vel: Vec3, life: u32, size: f32) -> Self {
        Self {
            pos,
            prev: pos,
            vel,
            age: 0,
            life: life.max(1),
            size,
            uv: [0.0; 4],
            frames: None,
            color: [1.0; 3],
            color_to: None,
            on_ground: false,
            gravity: 0.0,
            friction: 0.98,
            bright: false,
            collide: true,
            shrink: false,
        }
    }
}

/// Extra data some particle types carry.
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    None,
    /// A block state (block, falling dust, dust pillar, ...).
    Block(u16),
    /// A color, and a scale or power.
    Color([f32; 3], f32),
    /// Dust that changes color.
    Transition([f32; 3], [f32; 3], f32),
    /// An item's id (item breaking).
    Item(u32),
}

/// How many particles get made: vanilla's Particles option.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum Amount {
    #[default]
    All,
    Decreased,
    Minimal,
}

const LIMIT: usize = 8000;

#[derive(Default)]
pub struct Particles {
    list: Vec<Particle>,
    rng: u64,
    /// Sprites of every particle type by protocol ID, and the type names.
    pub sprites: Vec<Vec<[f32; 4]>>,
    pub names: Vec<String>,
    /// `HugeExplosionSeedParticle`s: center and ticks left.
    emitters: Vec<(Vec3, u32)>,
    pub amount: Amount,
}

impl Particles {
    fn random(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.rng >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Approximately normal (sum of uniforms), like `nextGaussian`.
    pub fn gaussian(&mut self) -> f32 {
        (0..4).map(|_| self.random()).sum::<f32>() - 2.0
    }

    fn kind(&self, name: &str) -> Option<u16> {
        self.names.iter().position(|n| n == name).map(|i| i as u16)
    }

    /// A random 1/4-size piece of `rect` (a block face's texture).
    fn piece(&mut self, rect: [f32; 4]) -> [f32; 4] {
        let (uo, vo) = (self.random() * 3.0, self.random() * 3.0);
        let (w, h) = ((rect[2] - rect[0]) / 16.0, (rect[3] - rect[1]) / 16.0);
        let (u0, v0) = (rect[0] + w * uo * 4.0, rect[1] + h * vo * 4.0);
        [u0, v0, u0 + w * 4.0, v0 + h * 4.0]
    }

    /// `Particle(level, x, y, z, xa, ya, za)`: random spread around the given velocity, then normalized.
    fn base_velocity(&mut self, vel: Vec3) -> Vec3 {
        let mut v = vel + Vec3::new(self.random() * 2.0 - 1.0, self.random() * 2.0 - 1.0, self.random() * 2.0 - 1.0) * 0.4;
        let speed = (self.random() + self.random() + 1.0) * 0.15;
        v = v / v.length().max(1e-4) * speed * 0.4;
        v.y += 0.1;
        v
    }

    /// `8 / (random * 0.8 + 0.2)`, the usual lifetime.
    fn life8(&mut self) -> f32 {
        8.0 / (self.random() * 0.8 + 0.2)
    }

    /// `TerrainParticle`: a piece of a block's texture.
    fn spawn(&mut self, pos: Vec3, vel: Vec3, rect: [f32; 4], tint: [u8; 3], power: f32, scale: f32) {
        let mut v = self.base_velocity(vel) * power;
        if power != 1.0 {
            v.y = (v.y - 0.1 * power) + 0.1;
        }
        let life = (4.0 / (self.random() * 0.9 + 0.1)) as u32;
        let size = 0.1 * (self.random() * 0.5 + 0.5) * scale;
        let mut p = Particle::new(pos, v, life, size);
        p.uv = self.piece(rect);
        p.color = tint.map(|c| c as f32 / 255.0 * 0.6);
        p.gravity = 0.04;
        self.add(p);
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

    /// An explosion (ClientPacketListener.handleExplosion): a big one is `explosion_emitter`, which puffs for 8
    /// ticks, a small one a single `explosion` puff; then smoke drifts up from around it.
    pub fn explosion(&mut self, center: Vec3, radius: f32) {
        if radius >= 2.0 {
            self.emitters.push((center, 8));
        } else {
            self.puff(center, 0.0);
        }
        for _ in 0..(radius * 6.0) as usize {
            let off = Vec3::new(self.random() - 0.5, self.random() - 0.5, self.random() - 0.5) * radius * 2.0;
            let vel = Vec3::new(self.random() - 0.5, self.random() * 0.5, self.random() - 0.5) * 0.1;
            self.emit("poof", center + off, vel, &Data::None, |_| None);
        }
    }

    /// `HugeExplosionParticle`: a gray puff that plays the explosion sheet, `age` 0..1 of its emitter's life.
    fn puff(&mut self, pos: Vec3, age: f32) {
        let Some(k) = self.kind("explosion") else { return };
        let gray = self.random() * 0.6 + 0.4;
        let life = 6 + (self.random() * 4.0) as u32;
        let mut p = Particle::new(pos, Vec3::ZERO, life, 2.0 * (1.0 - age * 0.5));
        p.frames = Some(k);
        p.color = [gray; 3];
        p.bright = true;
        p.collide = false;
        self.add(p);
    }

    fn add(&mut self, p: Particle) {
        if self.list.len() < LIMIT {
            self.list.push(p);
        }
    }

    /// `ClientPacketListener.handleParticleEvent`: `count` particles spread by `dist` (gaussian, or uniform for the
    /// alternative modes), or with `count` 0 one particle moving `dist * speed`.
    #[allow(clippy::too_many_arguments)]
    pub fn packet(&mut self, kind: u32, data: &Data, pos: Vec3, dist: Vec3, speed: Vec3, count: i32, mode: i32, always: bool, look: &dyn Fn(&Data) -> Option<([f32; 4], [u8; 3])>) {
        let Some(name) = self.names.get(kind as usize).cloned() else { return };
        if count == 0 {
            self.emit_limited(&name, pos, dist * speed, data, look, always);
            return;
        }
        for _ in 0..count.min(2000) {
            let (off, vel) = if mode != 0 {
                let off = Vec3::new(self.random(), self.random(), self.random()) * dist;
                let vel = if mode == 2 { speed * Vec3::new(self.random(), self.random(), self.random()) } else { speed };
                (off, vel)
            } else {
                let off = Vec3::new(self.gaussian(), self.gaussian(), self.gaussian()) * dist;
                (off, Vec3::new(self.gaussian(), self.gaussian(), self.gaussian()) * speed)
            };
            self.emit_limited(&name, pos + off, vel, data, look, always);
        }
    }

    /// A particle a block makes on its own (no block or item texture), subject to the Particles option.
    pub fn ambient(&mut self, name: &str, pos: Vec3, vel: Vec3, data: &Data) {
        self.emit_limited(name, pos, vel, data, &|_| None, false);
    }

    /// The Particles option: Decreased drops a third of them, Minimal all but the ones that must show.
    fn emit_limited(&mut self, name: &str, pos: Vec3, vel: Vec3, data: &Data, look: &dyn Fn(&Data) -> Option<([f32; 4], [u8; 3])>, always: bool) {
        let drop = match self.amount {
            Amount::All => false,
            Amount::Decreased => self.random() < 1.0 / 3.0,
            Amount::Minimal => true,
        };
        if !drop || always {
            self.emit(name, pos, vel, data, look);
        }
    }

    /// One particle of type `name`, built like that type's vanilla provider.
    pub fn emit(&mut self, name: &str, pos: Vec3, vel: Vec3, data: &Data, look: impl Fn(&Data) -> Option<([f32; 4], [u8; 3])>) {
        let Some(kind) = self.kind(name) else { return };
        let sprites = self.sprites.get(kind as usize).map_or(0, Vec::len);
        let (r1, r2, r3) = (self.random(), self.random(), self.random());
        let gray = |v: f32| [v; 3];
        let mut p = Particle::new(pos, vel, 20, 0.1 * (r1 * 0.5 + 0.5) * 2.0);
        // Animated by default when the type has several sprites, else its one sprite.
        if sprites > 1 {
            p.frames = Some(kind);
        } else if let Some(s) = self.sprites.get(kind as usize).and_then(|s| s.first()) {
            p.uv = *s;
        }
        let pick_one = |p: &mut Particle, me: &mut Self| {
            // `pickSprite`: one random frame for the whole life.
            let n = me.sprites[kind as usize].len();
            if n > 0 {
                let i = (me.random() * n as f32) as usize % n;
                p.uv = me.sprites[kind as usize][i];
                p.frames = None;
            }
        };
        match name {
            "explosion" => return self.puff(pos, 0.0),
            "explosion_emitter" => return self.emitters.push((pos, 8)),
            "block" | "block_marker" | "dust_pillar" | "block_crumble" | "falling_dust" => {
                let Some((rect, tint)) = look(data) else { return };
                if name == "falling_dust" {
                    // FallingDustParticle: drifts down slowly in the block's color.
                    p.vel = Vec3::ZERO;
                    p.life = (32.0 / (r1 * 0.8 + 0.2)) as u32;
                    p.uv = self.piece(rect);
                    p.color = tint.map(|c| c as f32 / 255.0);
                    p.gravity = 0.04 * 0.1;
                    p.size *= 0.75;
                } else if name == "block_marker" {
                    // BlockMarker: the block's own texture, still, for 80 ticks.
                    p.uv = rect;
                    p.vel = Vec3::ZERO;
                    p.life = 80;
                    p.size = 0.5;
                    p.collide = false;
                } else {
                    return self.spawn(pos, vel, rect, tint, if name == "dust_pillar" { 0.8 } else { 1.0 }, 1.0);
                }
            }
            "item" | "item_slime" | "item_snowball" | "item_cobweb" => {
                // BreakingItemParticle: a piece of the item's texture, falling.
                let rect = match data {
                    Data::Item(_) => look(data).map(|b| b.0),
                    _ => self.sprites.get(kind as usize).and_then(|s| s.first().copied()),
                };
                let Some(rect) = rect else { return };
                p.vel = self.base_velocity(vel) * 0.1 + vel;
                p.uv = self.piece(rect);
                p.gravity = 0.04;
                p.size *= 0.5;
                p.life = (4.0 / (r2 * 0.9 + 0.1)) as u32;
            }
            "smoke" | "large_smoke" | "white_smoke" | "campfire_cosy_smoke" | "campfire_signal_smoke" | "squid_ink" | "glow_squid_ink" => {
                // BaseAshSmokeParticle / CampfireSmokeParticle.
                let scale = match name {
                    "large_smoke" => 2.5,
                    "campfire_cosy_smoke" | "campfire_signal_smoke" => 3.0,
                    _ => 1.0,
                };
                p.vel = self.base_velocity(Vec3::ZERO) * 0.1 + vel;
                p.color = match name {
                    "white_smoke" | "campfire_cosy_smoke" | "campfire_signal_smoke" => gray(0.7 + r2 * 0.3),
                    "squid_ink" => gray(0.0),
                    "glow_squid_ink" => [0.2, 0.8, 0.8],
                    _ => gray(r2 * 0.3),
                };
                p.size *= 0.75 * scale;
                p.life = (self.life8() * scale.min(2.5)) as u32;
                p.gravity = -0.004;
                p.friction = 0.96;
                if name.starts_with("campfire") {
                    // Rises for a long time and fades.
                    p.life = if name == "campfire_signal_smoke" { 280 } else { 80 } + (r3 * 50.0) as u32;
                    p.gravity = -0.0001;
                    p.collide = false;
                    pick_one(&mut p, self);
                }
            }
            "flame" | "soul_fire_flame" | "small_flame" | "copper_fire_flame" | "electric_spark" => {
                // FlameParticle (a RisingParticle): barely moves on its own, glows, shrinks.
                p.vel = self.base_velocity(Vec3::ZERO) * 0.01 + vel;
                p.life = (self.life8() + 4.0) as u32;
                p.friction = 0.96;
                p.bright = true;
                p.shrink = true;
                p.collide = false;
                if name == "small_flame" {
                    p.size *= 0.5;
                }
                pick_one(&mut p, self);
            }
            "lava" => {
                // LavaParticle: pops up and falls, glowing.
                p.vel = Vec3::new(vel.x * 0.8 + (r1 - 0.5) * 0.1, r2 * 0.4 + 0.05, vel.z * 0.8 + (r3 - 0.5) * 0.1);
                p.size *= self.random() * 2.0 + 0.2;
                p.life = (16.0 / (self.random() * 0.8 + 0.2)) as u32;
                p.gravity = 0.75 * 0.04;
                p.friction = 0.999;
                p.bright = true;
                p.shrink = true;
            }
            "dust" | "dust_color_transition" => {
                // DustParticleBase: slow, colored, sized by the scale.
                let (from, to, scale) = match data {
                    Data::Color(c, s) => (*c, None, *s),
                    Data::Transition(a, b, s) => (*a, Some(*b), *s),
                    _ => ([1.0, 0.0, 0.0], None, 1.0),
                };
                let shade = r2 * 0.4 + 0.6;
                p.vel = self.base_velocity(Vec3::ZERO) * 0.1 + vel;
                p.color = from.map(|c| c * shade);
                p.color_to = to.map(|t| t.map(|c| c * shade));
                p.size *= 0.75 * scale;
                p.life = ((self.life8() * scale) as u32).max(1);
                p.friction = 0.96;
            }
            "effect" | "instant_effect" | "entity_effect" | "witch" | "raid_omen" | "trial_omen" => {
                // SpellParticle: drifts up, colored by the effect.
                p.vel = self.base_velocity(vel);
                p.vel.y *= 0.2;
                if vel.x == 0.0 && vel.z == 0.0 {
                    p.vel.x *= 0.1;
                    p.vel.z *= 0.1;
                }
                p.size *= 0.75;
                p.life = self.life8() as u32;
                p.friction = 0.96;
                p.gravity = -0.004;
                if let Data::Color(c, _) = data {
                    p.color = *c;
                } else if name == "witch" {
                    p.color = [0.75 + r3 * 0.25, 0.0, 0.75 + r3 * 0.25];
                }
            }
            "crit" | "enchanted_hit" | "damage_indicator" => {
                // CritParticle: bursts out, slows fast, falls a little.
                p.vel = self.base_velocity(Vec3::ZERO) * 0.1 + vel * 0.4;
                let c = r2 * 0.3 + 0.6;
                p.color = match name {
                    "enchanted_hit" => [c * 0.3, c * 0.8, c],
                    "damage_indicator" => [0.6 + r2 * 0.2, 0.1, 0.1],
                    _ => gray(c),
                };
                if name == "damage_indicator" {
                    p.vel.y += 0.1;
                }
                p.size *= 0.75;
                p.life = (6.0 / (r3 * 0.8 + 0.6)) as u32;
                p.friction = 0.7;
                p.gravity = 0.5 * 0.04;
                p.shrink = true;
            }
            "heart" | "angry_villager" | "note" => {
                // HeartParticle / NoteParticle: float up, a bit bigger.
                p.vel = Vec3::new(vel.x * 0.01, vel.y * 0.01 + 0.1, vel.z * 0.01);
                p.size *= 1.5;
                p.life = 16;
                p.friction = 0.86;
                p.collide = false;
                if name == "note" {
                    // NoteParticle: the color wheel position comes in the x speed.
                    let hue = vel.x;
                    let wave = |o: f32| ((hue + o) * std::f32::consts::TAU).sin() * 0.65 + 0.35;
                    p.color = [wave(0.0).max(0.0), wave(1.0 / 3.0).max(0.0), wave(2.0 / 3.0).max(0.0)];
                    p.vel = Vec3::new(0.0, 0.2, 0.0);
                }
            }
            "happy_villager" | "composter" | "egg_crack" | "mycelium" | "spore_blossom_air" | "white_ash" | "ash" | "crimson_spore" | "warped_spore" | "underwater" | "dolphin" | "fishing" => {
                // SuspendedTownParticle / SuspendedParticle / WhiteAshParticle: hang in the air and drift.
                p.vel = vel + Vec3::new(r1 - 0.5, r2 - 0.5, r3 - 0.5) * 0.02;
                p.life = (20.0 / (self.random() * 0.8 + 0.2)) as u32;
                p.friction = 0.96;
                p.collide = false;
                if name == "white_ash" || name == "ash" {
                    p.gravity = 0.0025;
                    p.color = if name == "ash" { gray(0.3) } else { gray(0.9) };
                }
                pick_one(&mut p, self);
            }
            "poof" | "cloud" | "sneeze" | "gust" | "small_gust" | "gust_emitter_large" | "gust_emitter_small" => {
                // ExplodeParticle (the white puffs a dying mob leaves), cloud, sneeze.
                p.vel = vel + Vec3::new(r1 * 2.0 - 1.0, r2 * 2.0 - 1.0, r3 * 2.0 - 1.0) * 0.05;
                let c = self.random() * 0.3 + 0.7;
                p.color = if name == "sneeze" { [0.6, 0.8, 0.4].map(|v| v * c) } else { gray(c) };
                p.size *= 1.5 + if name == "cloud" { 1.0 } else { 0.0 };
                p.life = (16.0 / (self.random() * 0.8 + 0.2)) as u32 + 2;
                p.friction = 0.9;
                p.gravity = -0.004;
            }
            "bubble" | "bubble_column_up" | "current_down" => {
                // BubbleParticle: wobbles up (current_down sinks).
                p.vel = vel * 0.2 + Vec3::new(r1 * 2.0 - 1.0, r2 * 2.0 - 1.0, r3 * 2.0 - 1.0) * 0.02;
                p.size *= r1 * 0.6 + 0.2;
                p.life = self.life8() as u32;
                p.gravity = if name == "current_down" { 0.002 } else { -0.002 };
                p.friction = 0.85;
            }
            "bubble_pop" => {
                p.life = 4;
                p.vel = Vec3::ZERO;
            }
            "splash" | "rain" | "falling_water" => {
                // WaterDropParticle / SplashParticle: flicks up and falls.
                p.vel = if name == "splash" && vel != Vec3::ZERO { vel } else { Vec3::new(vel.x * 0.3 + (r1 - 0.5) * 0.1, r2 * 0.2 + 0.1, vel.z * 0.3 + (r3 - 0.5) * 0.1) };
                p.gravity = 0.06;
                p.life = self.life8() as u32;
                pick_one(&mut p, self);
            }
            n if n.starts_with("dripping_") || n.starts_with("falling_") || n.starts_with("landing_") => {
                // DripParticle: hangs, falls, splats; tinted by the liquid.
                p.vel = Vec3::ZERO;
                p.color = if n.contains("lava") {
                    [1.0, 0.35, 0.1]
                } else if n.contains("honey") {
                    [0.58, 0.47, 0.06]
                } else if n.contains("obsidian_tear") {
                    [0.51, 0.03, 0.89]
                } else if n.contains("nectar") || n.contains("spore_blossom") {
                    [0.9, 0.6, 0.9]
                } else {
                    [0.2, 0.3, 1.0]
                };
                p.bright = n.contains("lava") || n.contains("obsidian");
                if n.starts_with("dripping_") {
                    p.gravity = 0.0006;
                    p.life = 40;
                } else if n.starts_with("falling_") {
                    p.gravity = 0.04 * 0.6;
                    p.life = (64.0 / (r1 * 0.8 + 0.2)) as u32;
                } else {
                    p.life = (16.0 / (r1 * 0.8 + 0.2)) as u32;
                }
                p.size *= 0.5;
            }
            "portal" | "reverse_portal" | "enchant" | "nautilus" | "vault_connection" | "ominous_spawning" => {
                // PortalParticle / FlyTowardsPositionParticle: swirls in.
                p.vel = vel;
                p.size *= 0.6 + r1 * 0.4;
                p.life = (r2 * 10.0) as u32 + 40;
                p.friction = 0.9;
                p.collide = false;
                p.bright = name != "enchant";
                let c = r3 * 0.6 + 0.4;
                p.color = match name {
                    "portal" | "reverse_portal" => [c * 0.9, c * 0.3, c],
                    _ => gray(c),
                };
                pick_one(&mut p, self);
            }
            "end_rod" | "firework" | "totem_of_undying" | "glow" | "electric_spark_glow" | "wax_on" | "wax_off" | "scrape" | "trial_spawner_detection" | "trial_spawner_detection_ominous" | "copper_fire" | "soul" => {
                // SimpleAnimatedParticle family: glowing sparks that slow and fall a little.
                p.vel = vel;
                p.life = 48 + (r1 * 12.0) as u32;
                p.friction = 0.91;
                p.gravity = if name == "totem_of_undying" { 1.25 * 0.04 } else { 0.0125 * 0.04 };
                p.bright = true;
                p.shrink = true;
                if name == "totem_of_undying" {
                    p.color = if r2 < 0.5 { [0.6 + r3 * 0.2, 0.6 + r3 * 0.3, r3 * 0.2] } else { [0.1 + r3 * 0.2, 0.4 + r3 * 0.3, r3 * 0.2] };
                    p.life = 60 + (r1 * 12.0) as u32;
                }
            }
            "sweep_attack" => {
                // AttackSweepParticle: a still swipe, 4 ticks.
                p.vel = Vec3::ZERO;
                p.size = 1.0 - vel.x * 0.5;
                p.life = 4;
                p.collide = false;
                p.bright = true;
                p.color = gray(r1 * 0.6 + 0.4);
            }
            "sonic_boom" | "flash" | "shriek" | "sculk_charge" | "sculk_charge_pop" | "sculk_soul" | "vibration" | "trail" | "infested" | "elder_guardian" => {
                p.vel = vel;
                p.life = 20;
                p.size *= if name == "sonic_boom" { 10.0 } else if name == "flash" { 20.0 } else { 1.0 };
                p.bright = true;
                p.collide = false;
                if let Data::Color(c, _) = data {
                    p.color = *c;
                }
                if name == "elder_guardian" {
                    // Drawn on screen by vanilla; here it just doesn't show.
                    return;
                }
            }
            "cherry_leaves" | "pale_oak_leaves" | "tinted_leaves" | "falling_leaves" => {
                // FallingLeavesParticle: tumbles down slowly.
                p.vel = Vec3::new(r1 - 0.5, 0.0, r3 - 0.5) * 0.05;
                p.gravity = 0.04 * 0.07;
                p.friction = 1.0;
                p.life = 300;
                if let Data::Color(c, _) = data {
                    p.color = *c;
                }
                pick_one(&mut p, self);
            }
            _ => {
                // Anything else: its sprites, carried by the given velocity.
                if let Data::Color(c, _) = data {
                    p.color = *c;
                }
            }
        }
        self.add(p);
    }

    /// One tick: gravity, drag and a simple stop on solid blocks.
    pub fn tick(&mut self, solid: impl Fn(i32, i32, i32) -> bool) {
        for i in 0..self.emitters.len() {
            let (c, left) = self.emitters[i];
            for _ in 0..6 {
                let off = Vec3::new(self.random() - self.random(), self.random() - self.random(), self.random() - self.random()) * 4.0;
                self.puff(c + off, (8 - left) as f32 / 8.0);
            }
            self.emitters[i].1 -= 1;
        }
        self.emitters.retain(|e| e.1 > 0);
        self.list.retain_mut(|p| {
            p.prev = p.pos;
            p.age += 1;
            if p.age >= p.life {
                return false;
            }
            p.vel.y -= p.gravity;
            let next = p.pos + p.vel;
            let below = |v: Vec3| solid(v.x.floor() as i32, v.y.floor() as i32, v.z.floor() as i32);
            if p.collide && below(next) {
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
            p.vel *= p.friction;
            if p.on_ground {
                p.vel.x *= 0.7;
                p.vel.z *= 0.7;
            }
            true
        });
    }

    pub fn clear(&mut self) {
        self.list.clear();
        self.emitters.clear();
    }

    /// Camera-facing quads, interpolated `t` (0..1) between ticks.
    pub fn mesh(&self, right: Vec3, up: Vec3, t: f32, light: impl Fn(Vec3) -> f32) -> Vec<Vertex> {
        let mut out = Vec::with_capacity(self.list.len() * 6);
        for p in &self.list {
            let c = p.prev.lerp(p.pos, t);
            let age = (p.age as f32 + t) / p.life as f32;
            let b = if p.bright { 1.0 } else { light(c) };
            let color = match p.color_to {
                Some(to) => [0, 1, 2].map(|i| p.color[i] + (to[i] - p.color[i]) * age),
                None => p.color,
            };
            let col = [(color[0] * b * 255.0) as u8, (color[1] * b * 255.0) as u8, (color[2] * b * 255.0) as u8, 255];
            let size = if p.shrink { p.size * (1.0 - age * age * 0.5) } else { p.size };
            let (r, u) = (right * size, up * size);
            // SpriteSet.get(age, lifetime): the frame for how far through its life it is.
            let [u0, v0, u1, v1] = match p.frames.and_then(|k| self.sprites.get(k as usize)) {
                Some(frames) if !frames.is_empty() => frames[(p.age as usize * (frames.len() - 1)) / p.life.max(1) as usize],
                _ => p.uv,
            };
            let v = |pos: Vec3, uv: [f32; 2]| Vertex { pos: pos.into(), uv, color: col };
            let q = [v(c - r + u, [u0, v0]), v(c - r - u, [u0, v1]), v(c + r - u, [u1, v1]), v(c + r + u, [u1, v0])];
            out.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        out
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Particles {
        let names = ["flame", "smoke", "dust", "block", "heart", "mystery"].map(String::from).to_vec();
        Particles { sprites: vec![vec![[0.0, 0.0, 0.1, 0.1]]; names.len()], names, ..Default::default() }
    }

    #[test]
    fn every_kind_spawns_and_dies() {
        let mut p = engine();
        let block = |_: &Data| Some(([0.0, 0.0, 0.1, 0.1], [255; 3]));
        p.packet(0, &Data::None, Vec3::ZERO, Vec3::splat(0.5), Vec3::splat(0.01), 10, 0, false, &block);
        p.packet(2, &Data::Color([1.0, 0.0, 0.0], 1.0), Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, 0, 0, false, &block);
        p.packet(3, &Data::Block(1), Vec3::ZERO, Vec3::ONE, Vec3::ZERO, 5, 0, false, &block);
        p.packet(5, &Data::None, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, 3, 1, false, &block);
        assert_eq!(p.len(), 10 + 1 + 5 + 3);
        for _ in 0..400 {
            p.tick(|_, _, _| false);
        }
        assert_eq!(p.len(), 0, "every particle has a finite life");
    }

    #[test]
    fn minimal_keeps_only_forced_particles() {
        let mut p = engine();
        p.amount = Amount::Minimal;
        let block = |_: &Data| None;
        p.packet(4, &Data::None, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, 20, 0, false, &block);
        assert_eq!(p.len(), 0);
        p.packet(4, &Data::None, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, 20, 0, true, &block);
        assert_eq!(p.len(), 20);
    }
}
