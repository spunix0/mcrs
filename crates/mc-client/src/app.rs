//! The window: fly camera, chunk meshing on worker threads, rendering, and movement packets back to the server.

use crate::entities::{self, Entities};
use crate::hand;
use crate::hud::{self, Painter};
use crate::inventory::Inventory;
use crate::physics::{self, Aabb, Body, Input};
use crate::audio::Audio;
use crate::session::{self, Dig, Event, SoundRef};
use crate::target::{self, Hit};
use gilrs::{Axis, Button, Gilrs};
use glam::{Mat4, Vec3};
use mc_assets::BlockModels;
use mc_render::{Area, ChunkMesh, LineVertex, Overlay, Renderer};
use mc_world::Chunk;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

const EYE_HEIGHT: f64 = 1.62;
const SNEAK_EYE_HEIGHT: f64 = 1.27;
/// One game tick.
const TICK: f32 = 0.05;
const STICK_DEADZONE: f32 = 0.15;
/// Degrees per second at full right-stick deflection.
const PAD_LOOK_SPEED: f32 = 220.0;

/// Gamepad state for one frame, already mapped to actions (Xbox layout):
/// left stick move, right stick look, A jump (double-tap: fly), R3 sneak, L3 sprint,
/// RT mine, LT use/place, LB/RB hotbar, Y inventory, B back.
#[derive(Default, Clone, Copy)]
struct Pad {
    left: [f32; 2],
    right: [f32; 2],
    jump: bool,
    sneak: bool,
    sprint: bool,
    mine: bool,
    use_item: bool,
    prev_slot: bool,
    next_slot: bool,
    inventory: bool,
    back: bool,
}

/// A block being mined in survival: the server checks how long we took, so we count ticks like vanilla.
struct Mining {
    pos: [i32; 3],
    face: u8,
    ticks: u32,
    needed: u32,
}

/// Vanilla waits this many ticks after breaking a block before starting the next.
const MINE_COOLDOWN: u8 = 5;
/// Vanilla repeats "use" every 4 ticks while the button is held.
const USE_COOLDOWN: u8 = 4;
/// Two jump presses this close together toggle flying, like creative mode.
const DOUBLE_TAP: Duration = Duration::from_millis(300);
/// Vanilla survival entity reach.
const ENTITY_REACH: f64 = 3.0;
/// Ticks between attacks (about a full-strength fist swing).
const ATTACK_COOLDOWN: u8 = 10;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    /// Survival with vanilla physics.
    Walk,
    /// Spectator noclip camera.
    Fly,
}

/// Block collision, friction and water lookups for the physics, from the loaded chunks.
struct WorldView<'a> {
    chunks: &'a HashMap<(i32, i32), Arc<Chunk>>,
    models: &'a BlockModels,
}

impl WorldView<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<&mc_assets::StateModel> {
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
}

struct Camera {
    eye: [f64; 3],
    yaw: f32,
    pitch: f32,
}

impl Camera {
    /// Minecraft convention: yaw 0 faces +Z (south), positive pitch looks down.
    fn look(&self) -> Vec3 {
        let (y, p) = (self.yaw.to_radians(), self.pitch.to_radians());
        Vec3::new(-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos())
    }
}

pub struct App {
    started: Instant,
    models: Arc<BlockModels>,
    vsync: bool,
    noclip: bool,
    view_distance: u8,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    net: Receiver<Event>,
    tx: mc_protocol::Sender,
    jobs: Sender<(u64, Area)>,
    meshes: Receiver<(u64, ChunkMesh)>,
    world: HashMap<(i32, i32), Arc<Chunk>>,
    /// Bumped on every change; a mesh is only uploaded if it was built from the latest version.
    versions: HashMap<(i32, i32), u64>,
    dirty: HashSet<(i32, i32)>,
    next_version: u64,
    cam: Camera,
    keys: HashSet<KeyCode>,
    /// `None` if the gamepad backend couldn't start; the keyboard still works.
    gilrs: Option<Gilrs>,
    grabbed: bool,
    in_world: bool,
    focused: bool,
    mode: Mode,
    body: Body,
    prev_feet: [f64; 3],
    tick_time: f32,
    prev_pad: Pad,
    last_jump_press: Option<Instant>,
    mouse_mine: bool,
    mouse_use: bool,
    slot: u8,
    sequence: i32,
    mining: Option<Mining>,
    mine_cooldown: u8,
    use_cooldown: u8,
    target: Option<Hit>,
    entity_target: Option<i32>,
    entities: Entities,
    attack_cooldown: u8,
    inventory_open: bool,
    inventory: Inventory,
    health: f32,
    food: i32,
    /// Recent chat lines and when they arrived.
    chat_log: Vec<(String, Instant)>,
    /// The line being typed, while the chat box is open.
    chat_input: Option<String>,
    /// Chat lines or commands to send once in the world (`--run`).
    pub join_commands: Vec<String>,
    /// `None` if there's no audio device or the sound assets are missing.
    pub audio: Option<Audio>,
    /// Where a block should appear after our last right-click, to play its place sound when the server confirms it.
    pending_place: Option<([i32; 3], Instant)>,
    /// Vanilla's `moveDist` / `nextStep` for footstep sounds.
    walked: f64,
    next_step: f64,
    /// Arm swing progress 0..1 (vanilla swings take 6 ticks).
    swing: f32,
    /// Cursor position in window pixels (for menus).
    mouse_pos: (f32, f32),
    last_frame: Instant,
    frames: u32,
    fps_since: Instant,
    first_frame: bool,
    first_chunk: bool,
    status: String,
}

impl App {
    pub fn new(started: Instant, models: Arc<BlockModels>, net: Receiver<Event>, tx: mc_protocol::Sender, vsync: bool, noclip: bool, view_distance: u8) -> Self {
        let (jobs, job_rx) = channel::<(u64, Area)>();
        let (mesh_tx, meshes) = channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(2).max(2));
        for _ in 0..workers {
            let (job_rx, mesh_tx, models) = (job_rx.clone(), mesh_tx.clone(), models.clone());
            std::thread::spawn(move || {
                loop {
                    let Ok((version, area)) = job_rx.lock().unwrap().recv() else { return };
                    if mesh_tx.send((version, mc_render::mesh_chunk(&area, &models))).is_err() {
                        return;
                    }
                }
            });
        }
        Self {
            started,
            models,
            vsync,
            noclip,
            view_distance,
            window: None,
            renderer: None,
            net,
            tx,
            jobs,
            meshes,
            world: HashMap::new(),
            versions: HashMap::new(),
            dirty: HashSet::new(),
            next_version: 0,
            cam: Camera { eye: [0.0, 80.0, 0.0], yaw: 0.0, pitch: 20.0 },
            keys: HashSet::new(),
            gilrs: Gilrs::new()
                .map_err(|e| eprintln!("gamepad support unavailable: {e}"))
                .ok()
                .inspect(|g| g.gamepads().for_each(|(_, p)| println!("controller found: {}", p.name()))),
            grabbed: false,
            in_world: false,
            focused: true,
            mode: Mode::Walk,
            body: Body::new([0.0, 80.0, 0.0]),
            prev_feet: [0.0, 80.0, 0.0],
            tick_time: 0.0,
            prev_pad: Pad::default(),
            last_jump_press: None,
            mouse_mine: false,
            mouse_use: false,
            slot: 0,
            sequence: 0,
            mining: None,
            mine_cooldown: 0,
            use_cooldown: 0,
            target: None,
            entity_target: None,
            entities: Entities::default(),
            attack_cooldown: 0,
            inventory_open: false,
            inventory: Inventory::default(),
            health: 20.0,
            food: 20,
            chat_log: Vec::new(),
            chat_input: None,
            join_commands: Vec::new(),
            audio: None,
            pending_place: None,
            walked: 0.0,
            next_step: 1.0,
            swing: 0.0,
            mouse_pos: (0.0, 0.0),
            last_frame: Instant::now(),
            frames: 0,
            fps_since: Instant::now(),
            first_frame: false,
            first_chunk: false,
            status: "connecting".into(),
        }
    }

    fn set_grab(&mut self, grab: bool) {
        let Some(w) = &self.window else { return };
        if grab {
            let ok = w.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined)).is_ok();
            w.set_cursor_visible(!ok);
            self.grabbed = ok;
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            w.set_cursor_visible(true);
            self.grabbed = false;
        }
    }

    fn mark_around(&mut self, x: i32, z: i32) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                self.dirty.insert((x + dx, z + dz));
            }
        }
    }

    /// Sends every dirty, loaded chunk to the meshers along with its neighbors.
    fn dispatch(&mut self) {
        for key in std::mem::take(&mut self.dirty) {
            let Some(center) = self.world.get(&key).cloned() else { continue };
            let mut around: [Option<Arc<Chunk>>; 9] = Default::default();
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx != 0 || dz != 0 {
                        around[((dz + 1) * 3 + (dx + 1)) as usize] = self.world.get(&(key.0 + dx, key.1 + dz)).cloned();
                    }
                }
            }
            self.next_version += 1;
            self.versions.insert(key, self.next_version);
            let _ = self.jobs.send((self.next_version, Area { center, around }));
        }
    }

    fn drain(&mut self) {
        while let Ok(ev) = self.net.try_recv() {
            match ev {
                Event::LoggedIn(name) => self.status = format!("logged in as {name}"),
                Event::InWorld => {
                    self.in_world = true;
                    self.status = "in world".into();
                    self.request_gamemode();
                    for cmd in std::mem::take(&mut self.join_commands) {
                        let _ = session::send_chat(&self.tx, &cmd);
                    }
                }
                Event::Chunk(c) => {
                    let key = (c.x, c.z);
                    self.world.insert(key, Arc::new(c));
                    // Neighbors can now cull their border faces and smooth their border light.
                    self.mark_around(key.0, key.1);
                }
                Event::Forget(x, z) => {
                    self.world.remove(&(x, z));
                    self.versions.remove(&(x, z));
                    if let Some(r) = &mut self.renderer {
                        r.remove(x, z);
                    }
                }
                Event::Blocks(changes) => {
                    for ([x, y, z], state) in changes {
                        let (cx, cz) = (x.div_euclid(16), z.div_euclid(16));
                        let Some(chunk) = self.world.get_mut(&(cx, cz)) else { continue };
                        let (lx, lz) = (x.rem_euclid(16), z.rem_euclid(16));
                        Arc::make_mut(chunk).set(lx as usize, y, lz as usize, state);
                        let placed = self.pending_place.is_some_and(|(p, t)| p == [x, y, z] && t.elapsed() < Duration::from_secs(1));
                        if placed && state != 0 {
                            self.pending_place = None;
                            self.block_sound([x, y, z], Some(state), |s| &s.place, |v| (v + 1.0) / 2.0, 0.8);
                        }
                        // A block affects meshes of every chunk whose 1-block border it sits in.
                        let xs = [-1, 0, 1].into_iter().filter(|d| *d == 0 || (*d == -1 && lx == 0) || (*d == 1 && lx == 15));
                        for dx in xs {
                            for dz in [-1, 0, 1] {
                                if dz == 0 || (dz == -1 && lz == 0) || (dz == 1 && lz == 15) {
                                    self.dirty.insert((cx + dx, cz + dz));
                                }
                            }
                        }
                    }
                }
                Event::Light(x, z, light) => {
                    if let Some(chunk) = self.world.get_mut(&(x, z)) {
                        let sections = chunk.light_sections();
                        Arc::make_mut(chunk).light.apply(light, sections);
                        self.mark_around(x, z);
                    }
                }
                Event::Teleport(feet, rot) => {
                    self.cam.eye = [feet[0], feet[1] + EYE_HEIGHT, feet[2]];
                    self.body = Body::new(feet);
                    self.prev_feet = feet;
                    self.cam.yaw = rot[0];
                    self.cam.pitch = rot[1];
                }
                Event::Chat(msg) => {
                    println!("[chat] {msg}");
                    self.chat_log.push((msg, Instant::now()));
                }
                Event::Sound { event, at, entity, volume, pitch } => {
                    let at = at.or_else(|| entity.and_then(|id| self.entities.map.get(&id)).map(|e| e.render_pos(Instant::now())));
                    let name = match event {
                        SoundRef::Id(id) => self.models.sound_events.get(id as usize).cloned(),
                        SoundRef::Name(n) => Some(n),
                    };
                    if let (Some(name), Some(a)) = (name, &mut self.audio) {
                        a.play(&name, at, volume, pitch);
                    }
                }
                // 2001: someone else broke a block (data = its state).
                Event::LevelEvent { kind: 2001, pos, data } => {
                    self.block_sound(pos, Some(data as u16), |s| &s.break_, |v| (v + 1.0) / 2.0, 0.8);
                }
                Event::LevelEvent { .. } => {}
                Event::EntityAdd { id, kind, pos, yaw, pitch, head_yaw } => self.entities.add(id, kind, pos, yaw, pitch, head_yaw),
                Event::EntityMove { id, steps, rot } => self.entities.moved(id, &steps, rot),
                Event::EntityRot { id, rot } => self.entities.rotated(id, rot),
                Event::EntityPos { id, pos, rot } => self.entities.teleported(id, pos, rot),
                Event::EntityHead { id, yaw } => self.entities.head(id, yaw),
                Event::EntityRemove(ids) => {
                    for id in ids {
                        self.entities.map.remove(&id);
                    }
                }
                Event::Health { health, food } => (self.health, self.food) = (health, food),
                Event::ChatTag(tag) => {
                    let lang = &self.models.lang;
                    let line = mc_protocol::text::render(&tag, &|k| lang.get(k).cloned());
                    println!("[chat] {line}");
                    self.chat_log.push((line, Instant::now()));
                }
                Event::Inventory(items) => {
                    for (slot, item) in items.into_iter().enumerate().take(self.inventory.slots.len()) {
                        self.inventory.slots[slot] = item;
                    }
                }
                Event::InventorySlot(slot, item) => {
                    if let Some(s) = self.inventory.slots.get_mut(slot) {
                        *s = item;
                    }
                }
                Event::PlayerSlot(index, item) => self.inventory.set_player_slot(index, item),
                Event::HeldSlot(slot) => self.slot = slot,
            }
        }
        self.dispatch();
        if let Some(r) = &mut self.renderer {
            while let Ok((version, mesh)) = self.meshes.try_recv() {
                if self.versions.get(&(mesh.x, mesh.z)) == Some(&version) {
                    r.upload(&mesh);
                    if !self.first_chunk {
                        self.first_chunk = true;
                        println!("first chunk on screen after {:?}", self.started.elapsed());
                    }
                }
            }
        }
    }

    /// First connected gamepad's sticks and buttons; all neutral without one.
    fn pad(&mut self) -> Pad {
        let Some(g) = &mut self.gilrs else { return Pad::default() };
        while let Some(ev) = g.next_event() {
            match ev.event {
                gilrs::EventType::Connected => println!("controller connected: {}", g.gamepad(ev.id).name()),
                gilrs::EventType::Disconnected => println!("controller disconnected"),
                _ => {}
            }
        }
        // Gamepads are global; a window in the background shouldn't move.
        if !self.focused {
            return Pad::default();
        }
        let Some((_, pad)) = g.gamepads().find(|(_, p)| p.is_connected()) else { return Pad::default() };
        let stick = |x: Axis, y: Axis| {
            let v = [pad.value(x), pad.value(y)];
            let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
            if len < STICK_DEADZONE {
                return [0.0; 2];
            }
            // Rescale so movement starts from zero at the deadzone edge.
            let scaled = ((len - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).min(1.0);
            [v[0] / len * scaled, v[1] / len * scaled]
        };
        // Triggers are analog (WGI reports them as axes); count anything past a light pull.
        let trigger = |b: Button| pad.is_pressed(b) || pad.button_data(b).is_some_and(|d| d.value() > 0.3);
        Pad {
            left: stick(Axis::LeftStickX, Axis::LeftStickY),
            right: stick(Axis::RightStickX, Axis::RightStickY),
            jump: pad.is_pressed(Button::South),
            sneak: pad.is_pressed(Button::RightThumb),
            sprint: pad.is_pressed(Button::LeftThumb),
            mine: trigger(Button::RightTrigger2),
            use_item: trigger(Button::LeftTrigger2),
            prev_slot: pad.is_pressed(Button::LeftTrigger),
            next_slot: pad.is_pressed(Button::RightTrigger),
            inventory: pad.is_pressed(Button::North),
            back: pad.is_pressed(Button::East),
        }
    }

    /// Switches walking (survival, physics) and flying (spectator, noclip).
    fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Walk => Mode::Fly,
            Mode::Fly => {
                self.body = Body::new(self.feet());
                self.prev_feet = self.body.pos;
                Mode::Walk
            }
        };
        self.request_gamemode();
    }

    fn request_gamemode(&self) {
        // Only spectators may pass through blocks; needs operator rights (else the server just says no).
        if self.noclip && self.in_world {
            let cmd = if self.mode == Mode::Fly { "/gamemode spectator" } else { "/gamemode survival" };
            let _ = session::send_chat(&self.tx, cmd);
        }
    }

    fn feet(&self) -> [f64; 3] {
        let [x, y, z] = self.cam.eye;
        [x, y - EYE_HEIGHT, z]
    }

    /// Jump was just pressed: a second press within [`DOUBLE_TAP`] toggles flying.
    fn jump_pressed(&mut self) {
        match self.last_jump_press {
            Some(t) if t.elapsed() < DOUBLE_TAP => {
                self.last_jump_press = None;
                self.toggle_mode();
            }
            _ => self.last_jump_press = Some(Instant::now()),
        }
    }

    fn select_slot(&mut self, slot: u8) {
        self.slot = slot % 9;
        let _ = session::send_hotbar_slot(&self.tx, self.slot);
    }

    /// Plays one of a block's sounds (break/hit/place/step) at its center, with vanilla's volume and pitch for that use.
    fn block_sound(&mut self, pos: [i32; 3], state: Option<u16>, which: fn(&mc_assets::SoundSet) -> &str, volume: fn(f32) -> f32, pitch: f32) {
        let world = WorldView { chunks: &self.world, models: &self.models };
        let state = match state {
            Some(s) => self.models.states.get(s as usize),
            None => world.state(pos[0], pos[1], pos[2]),
        };
        let Some(set) = state.and_then(|s| self.models.sound_types.get(&*s.sound)).cloned() else { return };
        let at = [pos[0] as f64 + 0.5, pos[1] as f64 + 0.5, pos[2] as f64 + 0.5];
        if let Some(a) = &mut self.audio {
            a.play(which(&set), Some(at), volume(set.volume), set.pitch * pitch);
        }
    }

    fn next_sequence(&mut self) -> i32 {
        self.sequence += 1;
        self.sequence
    }

    /// Opens or closes the inventory screen. Open frees the mouse; closing tells the server, like vanilla.
    fn toggle_inventory(&mut self) {
        self.inventory_open = !self.inventory_open;
        if self.inventory_open {
            self.set_grab(false);
        } else {
            let _ = self.tx.send(mc_protocol::ids::play::c2s::CONTAINER_CLOSE, &mc_protocol::Writer::new().varint(0).buf);
            self.set_grab(true);
        }
    }

    fn close_chat(&mut self) {
        self.chat_input = None;
        self.set_grab(true);
    }

    fn back(&mut self) {
        if self.inventory_open {
            self.toggle_inventory();
        }
    }

    /// One tick of mining and using, run with the physics. Mirrors vanilla's `continueDestroyBlock` and `startUseItem`.
    fn interact_tick(&mut self, mine: bool, use_item: bool) {
        self.mine_cooldown = self.mine_cooldown.saturating_sub(1);
        self.use_cooldown = self.use_cooldown.saturating_sub(1);
        let target = self.target.as_ref().map(|t| (t.pos, t.face, t.point));

        self.attack_cooldown = self.attack_cooldown.saturating_sub(1);
        if let (true, Some(id)) = (mine, self.entity_target) {
            // Hitting a mob or player instead of mining.
            if self.attack_cooldown == 0 {
                let _ = self.tx.send(mc_protocol::ids::play::c2s::ATTACK, &mc_protocol::Writer::new().varint(id).buf);
                self.attack_cooldown = ATTACK_COOLDOWN;
            }
            if let Some(m) = self.mining.take() {
                let seq = self.next_sequence();
                let _ = session::send_dig(&self.tx, Dig::Abort, m.pos, m.face, seq);
            }
            return;
        }
        if !mine {
            if let Some(m) = self.mining.take() {
                let seq = self.next_sequence();
                let _ = session::send_dig(&self.tx, Dig::Abort, m.pos, m.face, seq);
            }
        } else if self.mine_cooldown == 0 {
            // (position, broke it?) — sounds are played after the match to keep the borrow of `mining` short.
            let mut sound: Option<([i32; 3], bool)> = None;
            match (target, &mut self.mining) {
                (Some((pos, _, _)), Some(m)) if m.pos == pos => {
                    m.ticks += 1;
                    if m.ticks >= m.needed {
                        let (pos, face) = (m.pos, m.face);
                        self.mining = None;
                        let seq = self.next_sequence();
                        let _ = session::send_dig(&self.tx, Dig::Stop, pos, face, seq);
                        self.mine_cooldown = MINE_COOLDOWN;
                        sound = Some((pos, true));
                    } else if m.ticks % 4 == 0 {
                        sound = Some((pos, false));
                    }
                }
                (Some((pos, face, _)), _) => {
                    if let Some(m) = self.mining.take() {
                        let seq = self.next_sequence();
                        let _ = session::send_dig(&self.tx, Dig::Abort, m.pos, m.face, seq);
                    }
                    let world = WorldView { chunks: &self.world, models: &self.models };
                    let tool = self.inventory.hotbar(self.slot as usize).and_then(|s| self.models.items.get(s.item as usize)).and_then(|i| i.tool);
                    let needed = world.state(pos[0], pos[1], pos[2]).and_then(|s| s.break_ticks(tool.as_ref(), self.body.on_ground));
                    if let Some(needed) = needed {
                        let seq = self.next_sequence();
                        let _ = session::send_dig(&self.tx, Dig::Start, pos, face, seq);
                        if needed <= 1 {
                            // Breaks instantly (grass, flowers, torches).
                            self.mine_cooldown = MINE_COOLDOWN;
                            sound = Some((pos, true));
                        } else {
                            self.mining = Some(Mining { pos, face, ticks: 1, needed });
                            sound = Some((pos, false));
                        }
                    }
                }
                (None, _) => {
                    if let Some(m) = self.mining.take() {
                        let seq = self.next_sequence();
                        let _ = session::send_dig(&self.tx, Dig::Abort, m.pos, m.face, seq);
                    }
                }
            }
            // The server leaves the breaker out of its break broadcast, so the client plays these itself (vanilla too).
            match sound {
                Some((pos, true)) => self.block_sound(pos, None, |s| &s.break_, |v| (v + 1.0) / 2.0, 0.8),
                Some((pos, false)) => self.block_sound(pos, None, |s| &s.hit, |v| (v + 1.0) / 8.0, 0.5),
                None => {}
            }
        }

        if use_item && self.use_cooldown == 0 {
            let seq = self.next_sequence();
            let _ = match target {
                Some((pos, face, point)) => {
                    let o = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]][face as usize % 6];
                    self.pending_place = Some(([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]], Instant::now()));
                    session::send_use_on(&self.tx, pos, face, point, seq)
                }
                None => session::send_use(&self.tx, seq, [self.cam.yaw, self.cam.pitch]),
            };
            self.use_cooldown = USE_COOLDOWN;
        }
    }

    /// Updates the crosshair target from the current camera.
    fn update_target(&mut self) {
        if self.mode == Mode::Fly || !self.in_world {
            self.target = None;
            return;
        }
        let look = self.cam.look();
        let world = WorldView { chunks: &self.world, models: &self.models };
        self.target = target::raycast(self.cam.eye, [look.x as f64, look.y as f64, look.z as f64], target::REACH, |x, y, z| {
            let Some(s) = world.state(x, y, z) else { return Vec::new() };
            let (fx, fy, fz) = (x as f64, y as f64, z as f64);
            let to_world = |b: &[f32; 6]| Aabb {
                min: [fx + b[0] as f64, fy + b[1] as f64, fz + b[2] as f64],
                max: [fx + b[3] as f64, fy + b[4] as f64, fz + b[5] as f64],
            };
            if !s.collision.is_empty() {
                s.collision.iter().map(to_world).collect()
            } else if !s.quads.is_empty() {
                // Plants and other walk-through blocks are still targetable.
                vec![to_world(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0])]
            } else {
                Vec::new()
            }
        });
        // An entity in front of the targeted block (and within attack reach) takes the crosshair.
        let eye = self.cam.eye;
        let dir = [look.x as f64, look.y as f64, look.z as f64];
        let block_dist = self.target.as_ref().map_or(f64::MAX, |t| {
            let d = [t.point[0] - eye[0], t.point[1] - eye[1], t.point[2] - eye[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        });
        self.entity_target = self
            .entities
            .hitboxes(&self.models, Instant::now())
            .filter_map(|(id, b)| target::hit_box(eye, dir, &b).map(|(t, _)| (id, t)))
            .filter(|&(_, t)| t <= ENTITY_REACH && t < block_dist)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id);
    }

    fn light_at(&self, p: [f64; 3]) -> f32 {
        let (x, y, z) = (p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
        self.world.get(&(x.div_euclid(16), z.div_euclid(16))).map_or(1.0, |c| {
            let (s, b) = c.light(x.rem_euclid(16) as usize, y, z.rem_euclid(16) as usize);
            0.25 + 0.75 * s.max(b) as f32 / 15.0
        })
    }

    fn overlay(&self, height: u32) -> Overlay {
        let mut o = Overlay::default();
        o.entities = entities::mesh(&self.entities, &self.models, Instant::now(), |p| self.light_at(p));
        if let Some(t) = &self.target {
            let c = [0, 0, 0, 110];
            for b in &t.boxes {
                let (lo, hi) = (b.min.map(|v| v as f32 - 0.002), b.max.map(|v| v as f32 + 0.002));
                let p = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
                for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
                    o.world.push(LineVertex { pos: p(a), color: c });
                    o.world.push(LineVertex { pos: p(b), color: c });
                }
            }
        }
        // Cracks on the block being mined, one of vanilla's 10 destroy stages by progress.
        if let (Some(m), Some(t)) = (&self.mining, &self.target) {
            if m.pos == t.pos {
                let stage = (m.ticks * 10 / m.needed.max(1)).min(9) as usize;
                let [u0, v0, u1, v1] = self.models.crack_uv[stage];
                for b in &t.boxes {
                    let (lo, hi) = (b.min.map(|v| v as f32 - 0.003), b.max.map(|v| v as f32 + 0.003));
                    for axis in 0..3 {
                        for side in [lo[axis], hi[axis]] {
                            let (a, c) = ((axis + 1) % 3, (axis + 2) % 3);
                            let corner = |i: f32, j: f32| {
                                let mut p = [0.0; 3];
                                p[axis] = side;
                                p[a] = if i == 0.0 { lo[a] } else { hi[a] };
                                p[c] = if j == 0.0 { lo[c] } else { hi[c] };
                                mc_render::Vertex { pos: p, uv: [u0 + (u1 - u0) * i, v0 + (v1 - v0) * j], color: [255, 255, 255, 255] }
                            };
                            let q = [corner(0.0, 0.0), corner(0.0, 1.0), corner(1.0, 1.0), corner(1.0, 0.0)];
                            o.quads.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
                        }
                    }
                }
            }
        }
        if self.mode == Mode::Walk {
            // Crosshair: two lines half a block in front of the eye, sized in pixels.
            let look = self.cam.look();
            let right = look.cross(Vec3::Y).normalize();
            let up = right.cross(look);
            let eye = Vec3::new(self.cam.eye[0] as f32, self.cam.eye[1] as f32, self.cam.eye[2] as f32);
            let (d, arm_px) = (0.5, 9.0);
            let arm = 2.0 * d * (35f32.to_radians()).tan() / height.max(1) as f32 * arm_px;
            let center = eye + look * d;
            let c = [255, 255, 255, 220];
            for (a, b) in [(center - right * arm, center + right * arm), (center - up * arm, center + up * arm)] {
                o.top.push(LineVertex { pos: a.into(), color: c });
                o.top.push(LineVertex { pos: b.into(), color: c });
            }

            let [ex, ey, ez] = self.cam.eye;
            let (cx, cz) = ((ex.floor() as i32).div_euclid(16), (ez.floor() as i32).div_euclid(16));
            let light = self.world.get(&(cx, cz)).map_or(1.0, |c| {
                let (s, b) = c.light((ex.floor() as i32).rem_euclid(16) as usize, ey.floor() as i32, (ez.floor() as i32).rem_euclid(16) as usize);
                0.25 + 0.75 * s.max(b) as f32 / 15.0
            });
            let pose = hand::Pose { eye, look, swing: self.swing, walked: self.walked as f32, light };
            let held = self.inventory.hotbar(self.slot as usize).and_then(|s| self.models.items.get(s.item as usize));
            match held {
                Some(item) if item.icon.is_some() => hand::item(&mut o.hand_item, &pose, item),
                _ => hand::arm(&mut o.hand_skin, &pose, &self.models.gui.sprite("steve")),
            }
        }
        o
    }

    fn update(&mut self, dt: f32) {
        let pad = self.pad();
        let prev = std::mem::replace(&mut self.prev_pad, pad);
        let pressed = |now: bool, before: bool| now && !before;
        if pressed(pad.jump, prev.jump) && !self.inventory_open {
            self.jump_pressed();
        }
        if pressed(pad.next_slot, prev.next_slot) {
            self.select_slot(self.slot + 1);
        }
        if pressed(pad.prev_slot, prev.prev_slot) {
            self.select_slot(self.slot + 8);
        }
        if pressed(pad.inventory, prev.inventory) {
            self.toggle_inventory();
        }
        if pressed(pad.back, prev.back) {
            self.back();
        }
        // A menu takes all input; the world keeps simulating (you still fall) but you don't move or act.
        let pad = if self.inventory_open { Pad::default() } else { pad };
        let (left, right_stick, pad_sprint, rise, lower) = (pad.left, pad.right, pad.sprint, pad.jump, pad.sneak);
        let in_world_input = !self.inventory_open;
        let (mine, use_item) = (in_world_input && (self.mouse_mine || pad.mine), in_world_input && (self.mouse_use || pad.use_item));
        // Squared response gives fine control near the center and full speed at the edge.
        let curve = |v: f32| v * v.abs();
        self.cam.yaw += curve(right_stick[0]) * PAD_LOOK_SPEED * dt;
        self.cam.pitch = (self.cam.pitch - curve(right_stick[1]) * PAD_LOOK_SPEED * 0.75 * dt).clamp(-89.9, 89.9);

        let held = |k| in_world_input && self.keys.contains(&k);
        let sprint = held(KeyCode::ControlLeft) || pad_sprint;
        let axis = |pos: KeyCode, neg: KeyCode, stick: f32| ((held(pos) as i32 - held(neg) as i32) as f32 + stick).clamp(-1.0, 1.0);
        let input = Input {
            forward: axis(KeyCode::KeyW, KeyCode::KeyS, left[1]),
            strafe: axis(KeyCode::KeyA, KeyCode::KeyD, -left[0]),
            jump: held(KeyCode::Space) || rise,
            sprint,
            sneak: held(KeyCode::ShiftLeft) || lower,
        };

        // Keep swinging while mining or using; finish the current swing when released.
        let acting = (mine || use_item) && (self.target.is_some() || self.entity_target.is_some());
        if acting || self.swing > 0.0 {
            self.swing += dt / 0.3;
            if self.swing >= 1.0 {
                self.swing = if acting { self.swing - 1.0 } else { 0.0 };
            }
        }
        if self.mode == Mode::Walk {
            self.walk(&input, mine, use_item, dt);
        } else {
            self.fly(dt, sprint, left, rise, lower);
        }
    }

    /// Fixed 20 Hz physics ticks, each followed by one movement packet, with the camera interpolated in between.
    fn walk(&mut self, input: &Input, mine: bool, use_item: bool, dt: f32) {
        if !self.in_world {
            return;
        }
        self.tick_time = (self.tick_time + dt).min(0.25);
        while self.tick_time >= TICK {
            self.tick_time -= TICK;
            self.interact_tick(mine, use_item);
            self.prev_feet = self.body.pos;
            let world = WorldView { chunks: &self.world, models: &self.models };
            self.body.tick(input, self.cam.yaw, &world);
            // Footsteps like vanilla: walked distance * 0.6, one step sound per whole unit, from the block underfoot.
            let (dx, dz) = (self.body.pos[0] - self.prev_feet[0], self.body.pos[2] - self.prev_feet[2]);
            if self.body.on_ground && !input.sneak {
                self.walked += (dx * dx + dz * dz).sqrt() * 0.6;
                if self.walked > self.next_step {
                    self.next_step = self.walked.floor() + 1.0;
                    let [x, y, z] = self.body.pos;
                    let below = [x.floor() as i32, (y - 0.2).floor() as i32, z.floor() as i32];
                    self.block_sound(below, None, |s| &s.step, |v| v * 0.15, 1.0);
                }
            }
            let flags = self.body.on_ground as u8 | (self.body.horizontal_collision as u8) << 1;
            let _ = session::send_tick(&self.tx, self.body.pos, [self.cam.yaw, self.cam.pitch], flags);
        }
        let t = (self.tick_time / TICK) as f64;
        let eye = if input.sneak { SNEAK_EYE_HEIGHT } else { EYE_HEIGHT };
        for i in 0..3 {
            self.cam.eye[i] = self.prev_feet[i] + (self.body.pos[i] - self.prev_feet[i]) * t;
        }
        self.cam.eye[1] += eye;
    }

    fn fly(&mut self, dt: f32, sprint: bool, left: [f32; 2], rise: bool, lower: bool) {
        let held = |k| self.keys.contains(&k);
        let speed = if sprint { 30.0 } else { 10.0 } * dt as f64;
        let y = self.cam.yaw.to_radians() as f64;
        let forward = [-y.sin(), 0.0, y.cos()];
        let right = [-y.cos(), 0.0, -y.sin()];
        let mut mv = [0.0f64; 3];
        let mut add = |v: [f64; 3], s: f64| (0..3).for_each(|i| mv[i] += v[i] * s);
        if held(KeyCode::KeyW) { add(forward, 1.0) }
        if held(KeyCode::KeyS) { add(forward, -1.0) }
        if held(KeyCode::KeyD) { add(right, 1.0) }
        if held(KeyCode::KeyA) { add(right, -1.0) }
        if held(KeyCode::Space) || rise { add([0.0, 1.0, 0.0], 1.0) }
        if held(KeyCode::ShiftLeft) || lower { add([0.0, -1.0, 0.0], 1.0) }
        add(forward, left[1] as f64);
        add(right, left[0] as f64);
        // Keys give full speed, the stick gives up to full speed; never faster than either.
        let len = (mv[0] * mv[0] + mv[1] * mv[1] + mv[2] * mv[2]).sqrt();
        if len > 0.0 {
            let scale = speed / len.max(1.0);
            for i in 0..3 {
                self.cam.eye[i] += mv[i] * scale;
            }
        }
        self.tick_time = (self.tick_time + dt).min(0.25);
        while self.in_world && self.tick_time >= TICK {
            self.tick_time -= TICK;
            let _ = session::send_tick(&self.tx, self.feet(), [self.cam.yaw, self.cam.pitch], 0);
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.drain();
        self.update(dt);
        self.update_target();
        if let Some(a) = &mut self.audio {
            a.set_listener(self.cam.eye, self.cam.yaw);
            if self.in_world {
                a.tick_music();
            }
        }
        let mut overlay = self.overlay(self.renderer.as_ref().map_or(720, Renderer::height));
        let (w, h) = self.window.as_ref().map_or((1280, 720), |w| (w.inner_size().width, w.inner_size().height));
        let mut painter = Painter::new(&self.models, w, h);
        if self.in_world {
            hud::hotbar(&mut painter, &self.inventory, self.slot);
            if self.mode == Mode::Walk {
                hud::status(&mut painter, self.health, self.food);
            }
            hud::chat(&mut painter, &self.chat_log, self.chat_input.as_deref());
        }
        if self.inventory_open {
            overlay.top.clear();
            hud::inventory_screen(&mut painter, &self.inventory, self.mouse_pos);
        }
        let gui = painter.gui;
        let Some(r) = &mut self.renderer else { return };
        let eye = Vec3::new(self.cam.eye[0] as f32, self.cam.eye[1] as f32, self.cam.eye[2] as f32);
        let view = glam::camera::rh::view::look_to_mat4(eye, self.cam.look(), Vec3::Y);
        let fog_end = self.view_distance as f32 * 16.0;
        // DirectX convention = wgpu's: Y up in clip space, depth 0..1. Far plane covers the fog distance plus world height.
        let proj: Mat4 = glam::camera::rh::proj::directx::perspective(70f32.to_radians(), r.aspect(), 0.05, fog_end * 1.5 + 400.0);
        let stats = r.render(proj * view, eye, fog_end, &overlay, &gui);
        if !self.first_frame {
            self.first_frame = true;
            println!("first frame after {:?} ({})", self.started.elapsed(), r.adapter_name);
        }
        self.frames += 1;
        if self.fps_since.elapsed() >= Duration::from_secs(1) {
            let fps = self.frames as f32 / self.fps_since.elapsed().as_secs_f32();
            self.frames = 0;
            self.fps_since = Instant::now();
            if let Some(w) = &self.window {
                let [x, y, z] = self.cam.eye;
                w.set_title(&format!(
                    "mcrs · Rust Edition  |  {fps:.0} fps  |  {}/{} chunks  |  {:.1}M tris  |  {} MB GPU  |  {x:.0} {y:.0} {z:.0}  |  {:?}  |  slot {}  |  {}",
                    stats.drawn,
                    stats.loaded,
                    stats.triangles as f64 / 1e6,
                    stats.gpu_bytes / 1_048_576,
                    self.mode,
                    self.slot + 1,
                    self.status
                ));
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("mcrs · Rust Edition")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let size = window.inner_size();
        match Renderer::new(window.clone(), size.width, size.height, &self.models.atlas, &self.models.gui, self.vsync) {
            Ok(r) => self.renderer = Some(r),
            Err(e) => {
                eprintln!("renderer failed: {e:#}");
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(s.width, s.height);
                }
            }
            WindowEvent::Focused(false) => {
                self.focused = false;
                self.keys.clear();
                self.set_grab(false);
            }
            WindowEvent::Focused(true) => self.focused = true,
            WindowEvent::CursorMoved { position, .. } => self.mouse_pos = (position.x as f32, position.y as f32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } if !self.grabbed && !self.inventory_open => {
                self.set_grab(true)
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                match button {
                    MouseButton::Left => self.mouse_mine = down,
                    MouseButton::Right => self.mouse_use = down,
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.grabbed => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                // Scrolling down moves right along the hotbar, like vanilla.
                if dy < 0.0 {
                    self.select_slot(self.slot + 1);
                } else if dy > 0.0 {
                    self.select_slot(self.slot + 8);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if self.chat_input.is_some() => {
                if event.state != ElementState::Pressed {
                    return;
                }
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) => self.close_chat(),
                    PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter) => {
                        let line = self.chat_input.take().unwrap_or_default();
                        if !line.trim().is_empty() {
                            let _ = session::send_chat(&self.tx, line.trim());
                        }
                        self.close_chat();
                    }
                    PhysicalKey::Code(KeyCode::Backspace) => {
                        self.chat_input.as_mut().map(String::pop);
                    }
                    _ => {
                        if let (Some(text), Some(input)) = (&event.text, &mut self.chat_input) {
                            // Vanilla caps chat at 256 characters.
                            input.extend(text.chars().filter(|c| !c.is_control()).take(256usize.saturating_sub(input.len())));
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            if !event.repeat && !self.inventory_open && matches!(code, KeyCode::KeyT | KeyCode::Slash) {
                                self.chat_input = Some(if code == KeyCode::Slash { "/".into() } else { String::new() });
                                self.keys.clear();
                                self.set_grab(false);
                                return;
                            }
                            if !event.repeat {
                                match code {
                                    KeyCode::Escape => {
                                        self.back();
                                        self.set_grab(false);
                                    }
                                    KeyCode::KeyF => self.toggle_mode(),
                                    KeyCode::Space => self.jump_pressed(),
                                    KeyCode::KeyE => self.toggle_inventory(),
                                    _ => {}
                                }
                                let digits = [
                                    KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5,
                                    KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9,
                                ];
                                if let Some(i) = digits.iter().position(|d| *d == code) {
                                    self.select_slot(i as u8);
                                }
                            }
                            self.keys.insert(code);
                        }
                        ElementState::Released => {
                            self.keys.remove(&code);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => self.frame(),
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.grabbed {
                self.cam.yaw += dx as f32 * 0.15;
                self.cam.pitch = (self.cam.pitch + dy as f32 * 0.15).clamp(-89.9, 89.9);
            }
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}
