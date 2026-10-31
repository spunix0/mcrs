//! One connection to a world: chunk store and meshing, the player (vanilla physics, mining, using, attacking),
//! entities, particles, the sky clock, inventory and containers, chat and titles, and the first-person view
//! (camera bob, hurt tilt, hand swing and equip, all on vanilla's 20 Hz timing).

use crate::audio::category;
use crate::containers;
use crate::entities::{self, Entities};
use crate::hand;
use crate::hud::{self, Painter};
use crate::inventory::{self, Click, Inventory, Menu, MenuKind, Stack};
use crate::models;
use crate::options::Options;
use crate::particles::{self, Particles};
use crate::physics::{self, Aabb, Body, Input};
use crate::session::{self, Dig, Event, SoundRef};
use crate::sky::{Dimension, Sky};
use crate::target::{self, Hit};
use glam::{Mat4, Vec3};
use mc_assets::BlockModels;
use mc_render::{Area, ChunkMesh, LineVertex, Overlay, Renderer};
use mc_world::Chunk;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "game_play.rs"]
mod play;

const EYE_HEIGHT: f64 = 1.62;
const SNEAK_EYE_HEIGHT: f64 = 1.27;
/// One game tick.
pub const TICK: f32 = 0.05;
/// Vanilla waits this many ticks after breaking a block before starting the next.
const MINE_COOLDOWN: u8 = 5;
/// Vanilla repeats "use" every 4 ticks while the button is held.
const USE_COOLDOWN: u8 = 4;
/// Two jump presses this close together toggle flying, like creative mode.
const DOUBLE_TAP: Duration = Duration::from_millis(300);
/// Vanilla survival entity reach.
const ENTITY_REACH: f64 = 3.0;
/// Default swing length in ticks (`SwingAnimation.DEFAULT`).
const SWING_TICKS: u32 = 6;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// Survival with vanilla physics.
    Walk,
    /// Spectator noclip camera.
    Fly,
}

/// A block being mined in survival: the server checks how long we took, so we count ticks like vanilla.
struct Mining {
    pos: [i32; 3],
    face: u8,
    ticks: u32,
    needed: u32,
}

/// Block collision, friction and water lookups for the physics, from the loaded chunks.
pub struct WorldView<'a> {
    chunks: &'a HashMap<(i32, i32), Arc<Chunk>>,
    models: &'a BlockModels,
}

impl WorldView<'_> {
    pub fn state(&self, x: i32, y: i32, z: i32) -> Option<&mc_assets::StateModel> {
        let c = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        self.models.states.get(c.get(x.rem_euclid(16) as usize, y, z.rem_euclid(16) as usize) as usize)
    }
}

impl physics::World for WorldView<'_> {
    fn boxes(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) {
        let (fx, fy, fz) = (x as f64, y as f64, z as f64);
        match self.state(x, y, z) {
            Some(s) => out.extend(s.collision.iter().map(|b| Aabb {
                min: [fx + b[0] as f64, fy + b[1] as f64, fz + b[2] as f64],
                max: [fx + b[3] as f64, fy + b[4] as f64, fz + b[5] as f64],
            })),
            // Not loaded yet: solid, so you don't fall out of the world while chunks stream in.
            None => out.push(Aabb { min: [fx, fy, fz], max: [fx + 1.0, fy + 1.0, fz + 1.0] }),
        }
    }

    fn friction(&self, x: i32, y: i32, z: i32) -> f32 {
        self.state(x, y, z).map_or(0.6, |s| s.friction)
    }

    fn water(&self, x: i32, y: i32, z: i32) -> bool {
        self.state(x, y, z).is_some_and(|s| s.fluid == Some(mc_assets::Fluid::Water))
    }

    fn lava(&self, x: i32, y: i32, z: i32) -> bool {
        self.state(x, y, z).is_some_and(|s| s.fluid == Some(mc_assets::Fluid::Lava))
    }
}

pub struct Camera {
    pub eye: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

impl Camera {
    /// Minecraft convention: yaw 0 faces +Z (south), positive pitch looks down.
    pub fn look(&self) -> Vec3 {
        let (y, p) = (self.yaw.to_radians(), self.pitch.to_radians());
        Vec3::new(-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos())
    }
}

/// `LivingEntity.SwingState`: a 6-tick swing that can restart once past halfway.
#[derive(Default)]
struct Swing {
    active: bool,
    ticks: u32,
    old: f32,
    anim: f32,
}

impl Swing {
    fn start(&mut self) -> bool {
        if self.active && self.ticks <= SWING_TICKS / 2 && self.ticks > 0 {
            return false;
        }
        if !self.active {
            self.old = 0.0;
            self.anim = 0.0;
        }
        self.active = true;
        self.ticks = 0;
        true
    }

    fn tick(&mut self) {
        self.old = self.anim;
        if self.active {
            self.anim = (self.ticks as f32 / SWING_TICKS as f32).min(1.0);
            self.ticks += 1;
            if self.ticks > SWING_TICKS + 1 {
                self.active = false;
                self.anim = 0.0;
            }
        } else {
            self.anim = 0.0;
        }
    }

    fn get(&self, a: f32) -> f32 {
        let v = if self.anim >= self.old { self.old + (self.anim - self.old) * a } else { self.old + (self.anim + 1.0 - self.old) * a };
        v.fract()
    }
}

/// Controls for one frame, already mapped from keys, mouse and the controller.
#[derive(Default, Clone, Copy)]
pub struct Controls {
    pub forward: f32,
    pub strafe: f32,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
    pub mine: bool,
    pub use_item: bool,
    /// The attack button went down this frame (entities are hit once per press, like vanilla).
    pub attack_pressed: bool,
}

pub struct Game {
    models: Arc<BlockModels>,
    pub tx: mc_protocol::Sender,
    net: Receiver<Event>,
    /// Set by the session thread when the connection ends.
    pub ended: Arc<Mutex<Option<String>>>,
    jobs: Sender<(u64, Area)>,
    meshes: Receiver<(u64, ChunkMesh)>,
    world: HashMap<(i32, i32), Arc<Chunk>>,
    /// Newest mesh job per column and the sections it covers (including those of jobs it superseded).
    versions: HashMap<(i32, i32), (u64, u64)>,
    /// Sections waiting to be remeshed, bit i = section i from the bottom.
    dirty: HashMap<(i32, i32), u64>,
    next_version: u64,
    pub cam: Camera,
    pub mode: Mode,
    body: Body,
    prev_feet: [f64; 3],
    tick_time: f32,
    last_jump_press: Option<Instant>,
    pub slot: u8,
    sequence: i32,
    mining: Option<Mining>,
    mine_cooldown: u8,
    use_cooldown: u8,
    pub target: Option<Hit>,
    pub entity_target: Option<i32>,
    pub entities: Entities,
    pub inventory: Inventory,
    pub inventory_open: bool,
    pub menu: Option<Menu>,
    pub cursor: Option<Stack>,
    pub health: f32,
    pub food: i32,
    pub xp: (f32, i32, i32),
    pub air: i32,
    pub hardcore: bool,
    pub game_mode: u8,
    /// Creative (or `/ability`) flight is allowed: double-tapping jump toggles it.
    may_fly: bool,
    /// Game mode to go back to after noclip flying.
    walk_mode: u8,
    pub entity_id: i32,
    death_screen: bool,
    /// Set when we die: the death message, until respawn.
    pub dead: Option<String>,
    pub chat_log: Vec<(String, Instant)>,
    pub chat_input: Option<String>,
    pub sky: Sky,
    rain: f32,
    particles: Particles,
    /// Sounds to play: (event, position, volume, pitch, category).
    pub sounds: Vec<(String, Option<[f64; 3]>, f32, f32, usize)>,
    swing: Swing,
    main_height: f32,
    o_main_height: f32,
    shown_item: Option<u32>,
    last_held: Option<u32>,
    x_bob: f32,
    y_bob: f32,
    x_bob_o: f32,
    y_bob_o: f32,
    attack_ticker: u32,
    swap_ticker: u32,
    bob: f32,
    bob_o: f32,
    walk_dist: f32,
    walk_dist_o: f32,
    /// Ticks of camera hurt tilt left (vanilla's `hurtTime`, 10 on a hit).
    hurt_time: f32,
    fov_mod: f32,
    fov_mod_o: f32,
    /// Vanilla's footstep counters.
    walked: f64,
    next_step: f64,
    pub title: (String, String, Option<Instant>, [i32; 3]),
    pub action_bar: Option<(String, Instant)>,
    pub players: HashMap<u128, (String, i32)>,
    pub tab: (String, String),
    pub enchantments: Vec<String>,
    pub difficulty: u8,
    pub in_world: bool,
    pub loaded_at: Option<Instant>,
    pub status: String,
    pub join_commands: Vec<String>,
    noclip: bool,
    sprinting: bool,
    pending_place: Option<([i32; 3], Instant)>,
    pub show_hitboxes: bool,
    pub show_chunk_borders: bool,
    pub stats: (usize, usize, u64, u64),
    /// Chunks the server unloaded, to drop from the GPU on the next upload pass.
    forget: Vec<(i32, i32)>,
    player_model: models::Model,
    attack_queued: bool,
    /// Burning (shows fire on screen).
    pub on_fire: bool,
    /// Ticks the eyes have been underwater (vanilla's water vision).
    water_time: f32,
    /// Last tick's water state, for the splash on entering and the underwater ambience on going under.
    touching_water: bool,
    pub eyes_in_water: bool,
    /// The air bubble that last popped (vanilla's `lastBubblePopSoundPlayed`).
    last_bubble: i32,
    /// Cutout Leaves is off; changing it remeshes everything.
    pub solid_leaves: bool,
    /// On servers, every chunk's surface kept for distant terrain.
    lod_cache: Option<crate::lod::Cache>,
    /// Hud heart state: game ticks, health last tick, the health shown in white (`displayHealth`) and since when,
    /// and the tick the blinking stops.
    hearts: (u64, i32, i32, Instant, u64),
    /// The item being used (eaten, drunk, drawn) and since when.
    using: Option<(u32, Instant)>,
    rng: u64,
    /// The creative inventory while open, and its tabs (built on first open).
    pub creative: Option<crate::creative::Screen>,
    creative_tabs: Vec<crate::creative::Tab>,
    creative_tab: usize,
}

/// Attack speed by held item (attacks per second), for the cooldown and the equip dip.
fn attack_speed(name: &str) -> f32 {
    let tier = |n: &str| ["wooden", "stone", "copper", "iron", "golden", "diamond", "netherite"].iter().position(|t| n.starts_with(t));
    if name.ends_with("_sword") {
        1.6
    } else if name.ends_with("_pickaxe") {
        1.2
    } else if name.ends_with("_shovel") {
        1.0
    } else if name.ends_with("_axe") {
        match tier(name) {
            Some(0) | Some(1) => 0.8,
            Some(2) | Some(3) => 0.9,
            _ => 1.0,
        }
    } else if name.ends_with("_hoe") {
        match tier(name) {
            Some(0) | Some(4) => 1.0,
            Some(1) => 2.0,
            Some(2) | Some(3) => 3.0,
            _ => 4.0,
        }
    } else if name == "trident" {
        1.1
    } else if name == "mace" {
        0.6
    } else {
        4.0
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Game {
    pub fn new(models: Arc<BlockModels>, tx: mc_protocol::Sender, net: Receiver<Event>, ended: Arc<Mutex<Option<String>>>, noclip: bool) -> Self {
        let (sprites, names) = (models.particle_sprites.clone(), models.particle_types.clone());
        let (jobs, job_rx) = channel::<(u64, Area)>();
        let (mesh_tx, meshes) = channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(2).max(2));
        for _ in 0..workers {
            let (job_rx, mesh_tx, models) = (job_rx.clone(), mesh_tx.clone(), models.clone());
            std::thread::spawn(move || {
                loop {
                    let Ok((version, area)) = job_rx.lock().unwrap().recv() else { return };
                    let mesh = mc_render::mesh_chunk(&area, &models);
                    if mesh_tx.send((version, mesh)).is_err() {
                        return;
                    }
                }
            });
        }
        Self {
            models,
            tx,
            net,
            ended,
            jobs,
            meshes,
            world: HashMap::new(),
            versions: HashMap::new(),
            dirty: HashMap::new(),
            next_version: 0,
            cam: Camera { eye: [0.0, 80.0, 0.0], yaw: 0.0, pitch: 20.0 },
            mode: Mode::Walk,
            body: Body::new([0.0, 80.0, 0.0]),
            prev_feet: [0.0, 80.0, 0.0],
            tick_time: 0.0,
            last_jump_press: None,
            slot: 0,
            sequence: 0,
            mining: None,
            mine_cooldown: 0,
            use_cooldown: 0,
            target: None,
            entity_target: None,
            entities: Entities::default(),
            inventory: Inventory::default(),
            inventory_open: false,
            menu: None,
            cursor: None,
            health: 20.0,
            food: 20,
            xp: (0.0, 0, 0),
            air: 300,
            hardcore: false,
            game_mode: 0,
            may_fly: false,
            walk_mode: 0,
            entity_id: -1,
            death_screen: true,
            dead: None,
            chat_log: Vec::new(),
            chat_input: None,
            sky: Sky::new(),
            rain: 0.0,
            particles: {
                let mut p = Particles::default();
                (p.sprites, p.names) = (sprites, names);
                p
            },
            sounds: Vec::new(),
            swing: Swing::default(),
            main_height: 0.0,
            o_main_height: 0.0,
            shown_item: None,
            last_held: None,
            x_bob: 0.0,
            y_bob: 0.0,
            x_bob_o: 0.0,
            y_bob_o: 0.0,
            attack_ticker: 100,
            swap_ticker: 100,
            bob: 0.0,
            bob_o: 0.0,
            walk_dist: 0.0,
            walk_dist_o: 0.0,
            hurt_time: 0.0,
            fov_mod: 1.0,
            fov_mod_o: 1.0,
            walked: 0.0,
            next_step: 1.0,
            title: (String::new(), String::new(), None, [10, 70, 20]),
            action_bar: None,
            players: HashMap::new(),
            tab: (String::new(), String::new()),
            enchantments: Vec::new(),
            difficulty: 2,
            in_world: false,
            loaded_at: None,
            status: "Logging in...".into(),
            join_commands: Vec::new(),
            noclip,
            sprinting: false,
            pending_place: None,
            show_hitboxes: false,
            show_chunk_borders: false,
            stats: (0, 0, 0, 0),
            forget: Vec::new(),
            player_model: models::player(),
            attack_queued: false,
            on_fire: false,
            water_time: 0.0,
            touching_water: false,
            eyes_in_water: false,
            last_bubble: 0,
            solid_leaves: false,
            lod_cache: None,
            hearts: (0, 20, 20, Instant::now(), 0),
            using: None,
            rng: 0x2545_f491_4f6c_dd1d ^ Instant::now().elapsed().as_nanos() as u64 ^ std::process::id() as u64,
            creative: None,
            creative_tabs: Vec::new(),
            creative_tab: 0,
        }
    }

    fn view(&self) -> WorldView<'_> {
        WorldView { chunks: &self.world, models: &self.models }
    }

    /// Every chunk around the player's spawn is in, so the loading screen can close.
    pub fn ready(&self) -> bool {
        self.in_world && self.loaded_at.is_some_and(|t| t.elapsed() > Duration::from_millis(400))
    }

    pub(super) fn mark(&mut self, x: i32, z: i32, sections: u64) {
        *self.dirty.entry((x, z)).or_insert(0) |= sections;
    }

    /// Marks `sections` of a column and its 8 neighbors, whose border faces and light depend on it.
    fn mark_around(&mut self, x: i32, z: i32, sections: u64) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                self.mark(x + dx, z + dz, sections);
            }
        }
    }

    pub fn lod_cache_dir(&self) -> Option<std::path::PathBuf> {
        self.lod_cache.as_ref().map(|c| c.dir().to_path_buf())
    }

    /// Keeps (or stops keeping) the surface of received chunks in `dir`, for distant terrain on servers.
    pub fn set_lod_cache(&mut self, dir: Option<std::path::PathBuf>) {
        if self.lod_cache.as_ref().map(|c| c.dir()) == dir.as_deref() {
            return;
        }
        self.lod_cache = dir.map(crate::lod::Cache::new);
        if let Some(cache) = &mut self.lod_cache {
            for chunk in self.world.values() {
                cache.record(chunk, &self.models);
            }
        }
    }

    /// Video settings that change how things are built: Cutout Leaves (remeshes every chunk) and the particle amount.
    pub fn graphics(&mut self, cutout_leaves: bool, particles: u8) {
        self.particles.amount = match particles {
            1 => particles::Amount::Decreased,
            2 => particles::Amount::Minimal,
            _ => particles::Amount::All,
        };
        if self.solid_leaves == cutout_leaves {
            self.solid_leaves = !cutout_leaves;
            let keys: Vec<(i32, i32)> = self.world.keys().copied().collect();
            for (x, z) in keys {
                self.mark(x, z, u64::MAX);
            }
        }
    }

    /// Sends every dirty, loaded chunk to the meshers along with its neighbors, nearest first.
    fn dispatch(&mut self) {
        let mut dirty: Vec<_> = std::mem::take(&mut self.dirty).into_iter().collect();
        let (px, pz) = ((self.cam.eye[0] / 16.0).floor() as i32, (self.cam.eye[2] / 16.0).floor() as i32);
        dirty.sort_by_key(|((x, z), _)| (x - px).abs().max((z - pz).abs()));
        for (key, mut mask) in dirty {
            let Some(center) = self.world.get(&key).cloned() else { continue };
            let mut around: [Option<Arc<Chunk>>; 9] = Default::default();
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx != 0 || dz != 0 {
                        around[((dz + 1) * 3 + (dx + 1)) as usize] = self.world.get(&(key.0 + dx, key.1 + dz)).cloned();
                    }
                }
            }
            // A job still in flight gets superseded, so this one must cover its sections too.
            if let Some((_, pending)) = self.versions.get(&key) {
                mask |= pending;
            }
            self.next_version += 1;
            self.versions.insert(key, (self.next_version, mask));
            let _ = self.jobs.send((self.next_version, Area { center, around, mask, solid_leaves: self.solid_leaves }));
        }
    }

    fn sound(&mut self, event: &str, at: Option<[f64; 3]>, volume: f32, pitch: f32, cat: usize) {
        self.sounds.push((event.to_string(), at, volume, pitch, cat));
    }

    /// 0..1, for sound pitch and chances (xorshift64*).
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn chat(&mut self, line: String) {
        println!("[chat] {line}");
        self.chat_log.push((line, Instant::now()));
        if self.chat_log.len() > 100 {
            self.chat_log.remove(0);
        }
    }

    fn text(&self, tag: &mc_nbt::Tag) -> String {
        let lang = &self.models.lang;
        mc_protocol::text::render(tag, &|k| lang.get(k).cloned())
    }

    /// Handles everything the server sent since the last frame, and uploads finished chunk meshes.
    pub fn drain(&mut self, renderer: Option<&mut Renderer>) {
        let mut clear_chunks = false;
        while let Ok(ev) = self.net.try_recv() {
            match ev {
                Event::LoggedIn(_) => self.status = "Joining world...".into(),
                Event::Joined { entity, hardcore, game_mode, dimension, death_screen } => {
                    self.entity_id = entity;
                    self.hardcore = hardcore;
                    self.set_game_mode(game_mode);
                    self.walk_mode = if game_mode == 3 { 0 } else { game_mode };
                    self.sky.dimension = Dimension::from_name(&dimension);
                    self.death_screen = death_screen;
                    self.status = "Loading terrain...".into();
                }
                Event::Respawned { game_mode, dimension } => {
                    let dim = Dimension::from_name(&dimension);
                    if dim != self.sky.dimension || self.dead.is_some() {
                        // New dimension (or back from death): the old chunks and entities are gone.
                        self.world.clear();
                        self.versions.clear();
                        self.entities.map.clear();
                        self.particles.clear();
                        clear_chunks = true;
                    }
                    self.sky.dimension = dim;
                    self.set_game_mode(game_mode);
                    self.dead = None;
                    self.health = 20.0;
                }
                Event::InWorld => {
                    self.in_world = true;
                    self.loaded_at = Some(Instant::now());
                    self.status = "In world".into();
                    self.request_gamemode();
                    for cmd in std::mem::take(&mut self.join_commands) {
                        let _ = session::send_chat(&self.tx, &cmd);
                    }
                }
                Event::Chunk(c) => {
                    let key = (c.x, c.z);
                    if let Some(cache) = &mut self.lod_cache {
                        cache.record(&c, &self.models);
                    }
                    self.world.insert(key, Arc::new(c));
                    // Neighbors can now cull their border faces and smooth their border light.
                    self.mark_around(key.0, key.1, u64::MAX);
                }
                Event::Forget(x, z) => {
                    self.world.remove(&(x, z));
                    self.versions.remove(&(x, z));
                    self.forget.push((x, z));
                }
                Event::Blocks(changes) => self.blocks_changed(changes),
                Event::Light(x, z, light) => {
                    if let Some(chunk) = self.world.get_mut(&(x, z)) {
                        let sections = chunk.light_sections();
                        let changed = light.block_sections();
                        Arc::make_mut(chunk).light.apply(light, sections);
                        self.mark_around(x, z, changed);
                    }
                }
                Event::Teleport(feet, rot, id) => {
                    let _ = session::send_accept_teleport(&self.tx, id, feet, rot);
                    self.cam.eye = [feet[0], feet[1] + EYE_HEIGHT, feet[2]];
                    let flying = self.body.flying;
                    self.body = Body::new(feet);
                    self.body.flying = flying;
                    self.prev_feet = feet;
                    self.cam.yaw = rot[0];
                    self.cam.pitch = rot[1];
                    if self.loaded_at.is_none() && self.in_world {
                        self.loaded_at = Some(Instant::now());
                    }
                }
                Event::Chat(msg) => self.chat(msg),
                Event::ChatTag(tag) => {
                    let line = self.text(&tag);
                    self.chat(line);
                }
                Event::Sound { event, at, entity, volume, pitch, category } => {
                    let at = at.or_else(|| entity.and_then(|id| self.entities.map.get(&id)).map(|e| e.render_pos(Instant::now())));
                    let name = match event {
                        SoundRef::Id(id) => self.models.sound_events.get(id as usize).cloned(),
                        SoundRef::Name(n) => Some(n),
                    };
                    if let Some(name) = name {
                        self.sound(&name, at, volume, pitch, category);
                    }
                }
                Event::Particles { kind, data, pos, dist, speed, count, mode, always } => {
                    let mut ps = std::mem::take(&mut self.particles);
                    {
                        let look = |d: &particles::Data| match d {
                            particles::Data::Block(s) => self.particle_texture(*s),
                            particles::Data::Item(i) => self.models.items.get(*i as usize).and_then(|it| it.icon).map(|r| (r, [255; 3])),
                            _ => None,
                        };
                        let at = Vec3::new(pos[0] as f32, pos[1] as f32, pos[2] as f32);
                        ps.packet(kind, &data, at, Vec3::from(dist), Vec3::from(speed), count, mode, always, &look);
                    }
                    self.particles = ps;
                }
                Event::Explode { center, radius, knockback, sound } => {
                    // ClientPacketListener.handleExplosion: the sound at volume 4, the particles, and our push.
                    let name = match sound {
                        Some(SoundRef::Id(id)) => self.models.sound_events.get(id as usize).cloned(),
                        Some(SoundRef::Name(n)) => Some(n),
                        None => None,
                    };
                    let pitch = (1.0 + (self.rand() - self.rand()) * 0.2) * 0.7;
                    self.sound(name.as_deref().unwrap_or("entity.generic.explode"), Some(center), 4.0, pitch, category::BLOCKS);
                    self.particles.explosion(Vec3::new(center[0] as f32, center[1] as f32, center[2] as f32), radius);
                    if let Some(k) = knockback {
                        for i in 0..3 {
                            self.body.vel[i] += k[i];
                        }
                    }
                }
                // 2001: someone else broke a block (data = its state).
                Event::LevelEvent { kind: 2001, pos, data } => {
                    self.block_sound(pos, Some(data as u16), |s| &s.break_, |v| (v + 1.0) / 2.0, 0.8);
                    self.break_particles(pos, data as u16);
                }
                Event::LevelEvent { .. } => {}
                Event::EntityAdd { id, kind, pos, vel, yaw, pitch, head_yaw, data } => {
                    let simulate = self.models.entities.get(kind as usize).is_some_and(|i| matches!(i.name.as_str(), "item" | "experience_orb"));
                    self.entities.add(id, kind, pos, vel, yaw, pitch, head_yaw, data, simulate);
                }
                Event::EntityMotion(id, vel) => self.entities.motion(id, vel),
                Event::EntityMove { id, steps, rot } => self.entities.moved(id, &steps, rot),
                Event::EntityRot { id, rot } => self.entities.rotated(id, rot),
                Event::EntityPos { id, pos, rot } => self.entities.teleported(id, pos, rot),
                Event::EntityHead { id, yaw } => self.entities.head(id, yaw),
                Event::EntityRemove(ids) => {
                    for id in ids {
                        self.entities.map.remove(&id);
                    }
                }
                Event::TakeItem(id) => {
                    // ClientPacketListener.handleTakeItemEntity.
                    let Some(e) = self.entities.map.get(&id) else { continue };
                    let at = Some(e.render_pos(Instant::now()));
                    let kind = self.models.entities.get(e.kind as usize).map(|i| i.name.clone());
                    let (a, b) = (self.rand(), self.rand());
                    match kind.as_deref() {
                        Some("experience_orb") => self.sound("entity.experience_orb.pickup", at, 0.1, (a - b) * 0.35 + 0.9, category::PLAYERS),
                        Some(_) => self.sound("entity.item.pickup", at, 0.2, ((a - b) * 0.7 + 1.0) * 2.0, category::PLAYERS),
                        None => {}
                    }
                }
                Event::EntityData { id, data, air } => {
                    if id == self.entity_id {
                        if let Some(a) = air {
                            self.air = a;
                        }
                        for d in &data {
                            if let crate::entities::Data::Flags(f) = d {
                                self.on_fire = f & 1 != 0;
                            }
                        }
                    }
                    self.entities.set_data(id, data);
                }
                Event::Health { health, food } => {
                    if health < self.health && health > 0.0 {
                        self.hurt_time = 10.0;
                    }
                    (self.health, self.food) = (health, food);
                    if health <= 0.0 && self.dead.is_none() {
                        if self.death_screen {
                            self.dead = Some(String::new());
                        } else {
                            let _ = session::send_respawn(&self.tx);
                        }
                    }
                }
                Event::Died(tag) => {
                    let msg = self.text(&tag);
                    if self.death_screen {
                        self.dead = Some(msg);
                    }
                    self.inventory_open = false;
                    self.menu = None;
                }
                Event::Experience { progress, level, total } => self.xp = (progress, level, total),
                Event::Time { day, .. } => {
                    if let Some((ticks, rate)) = day {
                        self.sky.day_time = ticks as f64;
                        self.sky.rate = rate;
                    }
                }
                Event::Rain(r) => self.rain = r,
                Event::GameMode(m) => self.set_game_mode(m),
                Event::Inventory(state, items) => {
                    self.inventory.state_id = state;
                    for (slot, item) in items.into_iter().enumerate().take(self.inventory.slots.len()) {
                        self.inventory.slots[slot] = item;
                    }
                }
                Event::InventorySlot(state, slot, item) => {
                    self.inventory.state_id = state;
                    if let Some(s) = self.inventory.slots.get_mut(slot) {
                        *s = item;
                    }
                }
                Event::PlayerSlot(index, item) => self.inventory.set_player_slot(index, item),
                Event::HeldSlot(slot) => self.slot = slot,
                Event::OpenScreen { id, kind, title } => {
                    let kind_name = self.models.menus.get(kind as usize).cloned().unwrap_or_default();
                    let mut menu_kind = MenuKind::from_name(&kind_name);
                    if let MenuKind::Other(_) = menu_kind {
                        menu_kind = MenuKind::Other(3);
                    }
                    self.menu = Some(Menu { id, kind: menu_kind, title: self.text(&title), slots: Vec::new(), state_id: 0, data: HashMap::new() });
                    self.inventory_open = false;
                }
                Event::Container { id, state, items } => {
                    if let Some(m) = self.menu.as_mut().filter(|m| m.id == id) {
                        m.state_id = state;
                        m.slots = items;
                        if let MenuKind::Other(_) = m.kind {
                            m.kind = MenuKind::Other(((m.container_slots() + 8) / 9).clamp(1, 6) as u8);
                        }
                    }
                }
                Event::ContainerSlot { id, state, slot, item } => {
                    if let Some(m) = self.menu.as_mut().filter(|m| m.id == id) {
                        m.state_id = state;
                        if let Some(s) = m.slots.get_mut(slot) {
                            *s = item;
                        }
                    }
                }
                Event::ContainerData { id, prop, value } => {
                    if let Some(m) = self.menu.as_mut().filter(|m| m.id == id) {
                        m.data.insert(prop, value);
                    }
                }
                Event::CloseScreen => {
                    self.menu = None;
                    self.cursor = None;
                }
                Event::Cursor(c) => self.cursor = c,
                Event::Hurt(id, kind) => {
                    self.entities.hurt(id);
                    // LivingEntity.handleDamageEvent plays the hurt sound on every client, ours included.
                    let pitch = (self.rand() - self.rand()) * 0.2 + 1.0;
                    if id == self.entity_id {
                        self.hurt_time = 10.0;
                        // Player.getHurtSound: from the damage type's effects (DamageTypes).
                        let sound = match kind.as_deref().map(|k| k.trim_start_matches("minecraft:")) {
                            Some("drown") => "entity.player.hurt_drown",
                            Some("in_fire" | "campfire" | "on_fire" | "lava" | "hot_floor" | "sulfur_cube_hot" | "unattributed_fireball") => "entity.player.hurt_on_fire",
                            Some("sweet_berry_bush") => "entity.player.hurt_sweet_berry_bush",
                            Some("freeze") => "entity.player.hurt_freeze",
                            _ => "entity.player.hurt",
                        };
                        self.sound(sound, Some(self.feet()), 1.0, pitch, category::PLAYERS);
                    } else if let Some(e) = self.entities.map.get(&id) {
                        let at = Some(e.render_pos(Instant::now()));
                        let baby = if e.baby { 0.5 } else { 0.0 };
                        if let Some(info) = self.models.entities.get(e.kind as usize) {
                            let event = format!("entity.{}.hurt", info.name);
                            let event = if self.models.sound_events.contains(&event) { event } else { "entity.generic.hurt".into() };
                            let cat = if info.category == "monster" { category::HOSTILE } else { category::NEUTRAL };
                            self.sound(&event, at, 1.0, pitch + baby, cat);
                        }
                    }
                }
                Event::Swing(id) => self.entities.swing(id),
                Event::Title(t) => {
                    self.title.0 = self.text(&t);
                    self.title.2 = Some(Instant::now());
                }
                Event::Subtitle(t) => self.title.1 = self.text(&t),
                Event::ActionBar(t) => self.action_bar = Some((self.text(&t), Instant::now())),
                Event::TitleTimes(a, b, c) => self.title.3 = [a, b, c],
                Event::ClearTitles => {
                    self.title.0.clear();
                    self.title.1.clear();
                    self.title.2 = None;
                }
                Event::PlayersAdded(list) => {
                    for (uuid, name) in list {
                        self.players.insert(uuid, (name, 0));
                    }
                }
                Event::PlayerLatency(list) => {
                    for (uuid, ms) in list {
                        if let Some(p) = self.players.get_mut(&uuid) {
                            p.1 = ms;
                        }
                    }
                }
                Event::PlayersRemoved(list) => {
                    for uuid in list {
                        self.players.remove(&uuid);
                    }
                }
                Event::TabList(h, f) => self.tab = (self.text(&h), self.text(&f)),
                Event::Enchantments(names) => self.enchantments = names,
                Event::Difficulty(d) => self.difficulty = d,
                Event::Abilities(flags) => {
                    self.may_fly = flags & 4 != 0;
                    self.body.flying = flags & 2 != 0;
                }
            }
        }
        self.dispatch();
        if let Some(r) = renderer {
            if clear_chunks {
                r.clear_chunks();
            }
            for (x, z) in std::mem::take(&mut self.forget) {
                r.remove(x, z);
            }
            // A few milliseconds of uploads per frame; the rest wait so a burst of new chunks doesn't stall a frame.
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(4) {
                let Ok((version, mesh)) = self.meshes.try_recv() else { break };
                let key = (mesh.x, mesh.z);
                if self.versions.get(&key).is_some_and(|v| v.0 == version) {
                    self.versions.remove(&key);
                    r.upload(&mesh);
                }
            }
        }
    }
}
