//! The window: input (keyboard, mouse, Xbox controller), the menu stack, and the world session when one is open.
//! Starts on the title screen; singleplayer worlds run on a hidden local server, multiplayer connects directly.

use crate::audio::{Audio, category};
use crate::debug;
use crate::game::{Controls, Game};
use crate::hud::Painter;
use crate::options::{Action as Key, Options};
use crate::screens::{self, Action, Ctx, Screen};
use crate::servers::ServerList;
use crate::session::{self, Event};
use crate::ui::{self, Focus, Input, Nav};
use crate::worlds::{self, LocalServer, ServerSetup};
use gilrs::{Axis, Button, Gilrs};
use glam::{Mat4, Vec3};
use mc_assets::BlockModels;
use mc_render::{Environment, Overlay, Renderer};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

const STICK_DEADZONE: f32 = 0.15;
/// Degrees per second at full right-stick deflection (at 100% sensitivity).
const PAD_LOOK_SPEED: f32 = 220.0;
/// GUI pixels per second the inventory cursor moves at full stick.
const PAD_CURSOR_SPEED: f32 = 260.0;

/// Gamepad state for one frame (Xbox layout).
#[derive(Default, Clone, Copy)]
struct Pad {
    left: [f32; 2],
    right: [f32; 2],
    a: bool,
    b: bool,
    x: bool,
    y: bool,
    lb: bool,
    rb: bool,
    lt: bool,
    rt: bool,
    l3: bool,
    r3: bool,
    start: bool,
    up: bool,
    down: bool,
    left_d: bool,
    right_d: bool,
}

/// A connection being opened in the background.
struct Pending {
    target: String,
    result: Receiver<anyhow::Result<mc_protocol::Sender>>,
    events: Receiver<Event>,
    ended: Arc<Mutex<Option<String>>>,
}

pub struct App {
    started: Instant,
    models: Arc<BlockModels>,
    setup: ServerSetup,
    name: String,
    noclip: bool,
    pub options: Options,
    saved_options: Options,
    servers: ServerList,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    gilrs: Option<Gilrs>,
    prev_pad: Pad,
    nav_repeat: Option<(Nav, Instant)>,
    keys: HashSet<KeyCode>,
    pressed: Vec<KeyCode>,
    typed: String,
    mouse_px: (f32, f32),
    mouse_left: bool,
    mouse_right: bool,
    clicked: bool,
    right_clicked: bool,
    last_click: Option<Instant>,
    double_click: bool,
    scroll: f32,
    attack_pressed: bool,
    pad_mode: bool,
    pad_armed: bool,
    focused: bool,
    grabbed: bool,
    screens: Vec<Screen>,
    focus: Focus,
    game: Option<Game>,
    pending: Option<Pending>,
    server: Option<LocalServer>,
    saving: Vec<std::thread::JoinHandle<()>>,
    /// The world we're starting and whether it's the local one.
    singleplayer: bool,
    paused_server: bool,
    pub audio: Option<Audio>,
    pub join_commands: Vec<String>,
    /// Open this right away instead of the title screen.
    pub start_world: Option<String>,
    pub start_server: Option<String>,
    show_debug: bool,
    f3_combo: bool,
    hide_hud: bool,
    last_frame: Instant,
    frames: u32,
    /// The last 120 frame times in milliseconds, for the FPS graph.
    frame_times: std::collections::VecDeque<f32>,
    fps_since: Instant,
    fps: f32,
    first_frame: bool,
    quit: bool,
}

fn data_dir_of(setup: &ServerSetup) -> PathBuf {
    setup.worlds.parent().map(PathBuf::from).unwrap_or_default()
}

impl App {
    pub fn new(started: Instant, models: Arc<BlockModels>, setup: ServerSetup, name: String, noclip: bool) -> Self {
        let data = data_dir_of(&setup);
        let options = Options::load(&data);
        let servers = ServerList::load(&data.join("game"));
        let title = screens::title(&models.gui.splashes);
        Self {
            started,
            models,
            setup,
            name,
            noclip,
            saved_options: options.clone(),
            options,
            servers,
            window: None,
            renderer: None,
            gilrs: Gilrs::new()
                .map_err(|e| eprintln!("gamepad support unavailable: {e}"))
                .ok()
                .inspect(|g| g.gamepads().for_each(|(_, p)| println!("controller found: {}", p.name()))),
            prev_pad: Pad::default(),
            nav_repeat: None,
            keys: HashSet::new(),
            pressed: Vec::new(),
            typed: String::new(),
            mouse_px: (0.0, 0.0),
            mouse_left: false,
            mouse_right: false,
            clicked: false,
            right_clicked: false,
            last_click: None,
            double_click: false,
            scroll: 0.0,
            attack_pressed: false,
            pad_mode: false,
            pad_armed: false,
            focused: true,
            grabbed: false,
            screens: vec![title],
            focus: Focus::default(),
            game: None,
            pending: None,
            server: None,
            saving: Vec::new(),
            singleplayer: false,
            paused_server: false,
            audio: None,
            join_commands: Vec::new(),
            start_world: None,
            start_server: None,
            show_debug: false,
            f3_combo: false,
            hide_hud: false,
            last_frame: Instant::now(),
            frames: 0,
            frame_times: std::collections::VecDeque::new(),
            fps_since: Instant::now(),
            fps: 0.0,
            first_frame: false,
            quit: false,
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

    fn scale(&self) -> f32 {
        let (w, h) = self.window_size();
        crate::hud::gui_scale(w, h, self.options.gui_scale)
    }

    fn window_size(&self) -> (u32, u32) {
        self.window.as_ref().map_or((1280, 720), |w| (w.inner_size().width, w.inner_size().height))
    }

    /// Opens a connection in the background; the game starts when it succeeds.
    fn connect(&mut self, target: String) {
        let (result_tx, result) = channel();
        let (event_tx, events) = channel();
        let ended = Arc::new(Mutex::new(None));
        let (name, view_distance) = (self.name.clone(), self.options.render_distance);
        let components = mc_inventory_components(&self.models);
        let block_states = self.models.states.len();
        let addr = target.clone();
        let ended_t = ended.clone();
        std::thread::spawn(move || {
            let with_port = if addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) { addr.clone() } else { format!("{addr}:25565") };
            let (tx, rx) = match mc_protocol::connect(&with_port) {
                Ok(c) => c,
                Err(e) => {
                    let _ = result_tx.send(Err(anyhow::anyhow!("{e}")));
                    return;
                }
            };
            let _ = result_tx.send(Ok(tx.clone()));
            let opts = session::Options {
                addr: &with_port,
                name: &name,
                greet: false,
                block_states: Some(block_states),
                drives_movement: true,
                view_distance,
                components: Some(&components),
            };
            let reason = match session::run(&opts, tx, rx, |e| {
                let _ = event_tx.send(e);
            }) {
                Ok(reason) => reason,
                Err(e) => format!("{e:#}"),
            };
            println!("disconnected: {reason}");
            *ended_t.lock().unwrap() = Some(reason);
        });
        self.pending = Some(Pending { target: target.clone(), result, events, ended });
        self.screens.push(Screen::Connecting { target });
    }

    fn play_world(&mut self, folder: String) {
        match LocalServer::start(&self.setup, &folder) {
            Ok(s) => {
                self.server = Some(s);
                self.singleplayer = true;
                self.screens.push(Screen::Loading { title: "Loading world".into() });
            }
            Err(e) => {
                if let Some(Screen::SelectWorld { error, .. }) = self.screens.last_mut() {
                    *error = Some(format!("{e:#}"));
                } else {
                    self.screens.push(Screen::Message { title: "Couldn't start the world".into(), text: format!("{e:#}"), back_to_servers: false });
                }
            }
        }
    }

    /// Leaves the world or server and goes back to the title screen (saving a singleplayer world first).
    fn disconnect(&mut self, message: Option<(String, String)>) {
        if let Some(g) = self.game.take() {
            g.tx.shutdown();
        }
        if let Some(p) = self.pending.take() {
            drop(p);
        }
        if let Some(s) = self.server.take() {
            self.saving.push(s.stop());
        }
        if let Some(r) = &mut self.renderer {
            r.clear_chunks();
        }
        let was_multiplayer = !self.singleplayer;
        self.singleplayer = false;
        self.paused_server = false;
        self.set_grab(false);
        self.screens = vec![screens::title(&self.models.gui.splashes)];
        if was_multiplayer && message.is_some() {
            self.screens.push(Screen::Multiplayer { selected: None, scroll: ui::Scroll::default() });
        }
        if let Some((title, text)) = message {
            self.screens.push(Screen::Message { title, text, back_to_servers: was_multiplayer });
        }
        if !self.saving.is_empty() {
            self.screens.push(Screen::Loading { title: "Saving world".into() });
        }
    }

    /// Applies changed options (vsync, fullscreen, view distance) and saves them.
    fn apply_options(&mut self) {
        if self.options == self.saved_options {
            return;
        }
        if self.options.vsync != self.saved_options.vsync {
            if let Some(r) = &mut self.renderer {
                r.set_vsync(self.options.vsync);
            }
        }
        if self.options.fullscreen != self.saved_options.fullscreen {
            if let Some(w) = &self.window {
                w.set_fullscreen(self.options.fullscreen.then_some(Fullscreen::Borderless(None)));
            }
        }
        if self.options.render_distance != self.saved_options.render_distance {
            if let Some(g) = &self.game {
                let _ = session::send_client_information(&g.tx, self.options.render_distance);
            }
        }
        if let Some(a) = &mut self.audio {
            a.volumes = self.options.volumes;
            a.update_music_volume();
        }
        self.options.save();
        self.saved_options = self.options.clone();
    }

    /// Polls the background connection, the local server and the session end.
    fn poll_world(&mut self) {
        if let Some(s) = &mut self.server {
            if let Some(err) = s.failed() {
                self.disconnect(Some(("Couldn't start the world".into(), err)));
                return;
            }
            if self.pending.is_none() && self.game.is_none() && s.ready.load(std::sync::atomic::Ordering::Relaxed) {
                let port = s.port;
                self.connect(format!("127.0.0.1:{port}"));
                // Keep the loading screen rather than "Connecting to the server..." for a local world.
                if let Some(Screen::Connecting { .. }) = self.screens.last() {
                    self.screens.pop();
                }
            }
        }
        if let Some(p) = &self.pending {
            match p.result.try_recv() {
                Ok(Ok(tx)) => {
                    let p = self.pending.take().unwrap();
                    let mut g = Game::new(self.models.clone(), tx, p.events, p.ended, self.noclip);
                    g.join_commands = std::mem::take(&mut self.join_commands);
                    self.game = Some(g);
                    if !self.singleplayer {
                        self.options.last_server = p.target.clone();
                    }
                }
                Ok(Err(e)) => {
                    let target = p.target.clone();
                    self.pending = None;
                    self.disconnect(Some(("Failed to connect to the server".into(), format!("{target}: {e}"))));
                }
                Err(_) => {}
            }
        }
        let ended = self.game.as_ref().and_then(|g| g.ended.lock().unwrap().clone());
        if let Some(reason) = ended {
            let title = if self.singleplayer { "Disconnected" } else { "Connection Lost" };
            self.disconnect(Some((title.into(), reason)));
            return;
        }
        if let Some(g) = &self.game {
            // Loading finished: close the connecting/loading screens and capture the mouse.
            if g.ready() && matches!(self.screens.last(), Some(Screen::Loading { .. } | Screen::Connecting { .. })) {
                self.screens.clear();
                self.focus = Focus::default();
                self.set_grab(true);
            }
        }
        self.saving.retain(|h| !h.is_finished());
        if self.saving.is_empty() && matches!(self.screens.last(), Some(Screen::Loading { title }) if title == "Saving world") {
            self.screens.pop();
        }
    }

    fn status_text(&self) -> String {
        if let Some(g) = &self.game {
            return g.status.clone();
        }
        if self.pending.is_some() {
            return "Connecting...".into();
        }
        if let Some(s) = &self.server {
            return s.progress.lock().unwrap().clone();
        }
        if !self.saving.is_empty() {
            return "Saving the world".into();
        }
        String::new()
    }

    /// First connected gamepad's sticks and buttons; all neutral without one or while unfocused.
    fn pad(&mut self) -> Pad {
        let Some(g) = &mut self.gilrs else { return Pad::default() };
        while let Some(ev) = g.next_event() {
            match ev.event {
                gilrs::EventType::Connected => println!("controller connected: {}", g.gamepad(ev.id).name()),
                gilrs::EventType::Disconnected => println!("controller disconnected"),
                _ => {}
            }
        }
        if !self.focused {
            self.pad_armed = false;
            return Pad::default();
        }
        let Some((_, pad)) = g.gamepads().find(|(_, p)| p.is_connected()) else { return Pad::default() };
        let stick = |x: Axis, y: Axis| {
            let v = [pad.value(x), pad.value(y)];
            let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
            if len < STICK_DEADZONE {
                return [0.0; 2];
            }
            let scaled = ((len - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).min(1.0);
            [v[0] / len * scaled, v[1] / len * scaled]
        };
        let trigger = |b: Button| pad.is_pressed(b) || pad.button_data(b).is_some_and(|d| d.value() > 0.3);
        // After startup or a focus change, wait until every button reads released: stale or bouncing button
        // state at that moment looked like a double-tapped A and switched players to spectator on join.
        if !self.pad_armed {
            self.pad_armed = !pad.is_pressed(Button::South) && !pad.is_pressed(Button::Start) && !trigger(Button::RightTrigger2) && !trigger(Button::LeftTrigger2);
            return Pad::default();
        }
        Pad {
            left: stick(Axis::LeftStickX, Axis::LeftStickY),
            right: stick(Axis::RightStickX, Axis::RightStickY),
            a: pad.is_pressed(Button::South),
            b: pad.is_pressed(Button::East),
            x: pad.is_pressed(Button::West),
            y: pad.is_pressed(Button::North),
            lb: pad.is_pressed(Button::LeftTrigger),
            rb: pad.is_pressed(Button::RightTrigger),
            lt: trigger(Button::LeftTrigger2),
            rt: trigger(Button::RightTrigger2),
            l3: pad.is_pressed(Button::LeftThumb),
            r3: pad.is_pressed(Button::RightThumb),
            start: pad.is_pressed(Button::Start),
            up: pad.is_pressed(Button::DPadUp),
            down: pad.is_pressed(Button::DPadDown),
            left_d: pad.is_pressed(Button::DPadLeft),
            right_d: pad.is_pressed(Button::DPadRight),
        }
    }

    /// Menu navigation from the D-pad or left stick, repeating while held.
    fn pad_nav(&mut self, pad: &Pad) -> Option<Nav> {
        let dir = if pad.up || pad.left[1] > 0.6 {
            Some(Nav::Up)
        } else if pad.down || pad.left[1] < -0.6 {
            Some(Nav::Down)
        } else if pad.left_d || pad.left[0] < -0.6 {
            Some(Nav::Left)
        } else if pad.right_d || pad.left[0] > 0.6 {
            Some(Nav::Right)
        } else {
            None
        };
        match (dir, self.nav_repeat) {
            (None, _) => {
                self.nav_repeat = None;
                None
            }
            (Some(d), Some((prev, at))) if prev == d => {
                if at.elapsed() > Duration::from_millis(120) {
                    self.nav_repeat = Some((d, Instant::now()));
                    Some(d)
                } else {
                    None
                }
            }
            (Some(d), _) => {
                // First press: a longer pause before repeating.
                self.nav_repeat = Some((d, Instant::now() + Duration::from_millis(230)));
                Some(d)
            }
        }
    }

    fn held(&self, a: Key) -> bool {
        self.keys.contains(&self.options.key(a))
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let pad = self.pad();
        let prev = std::mem::replace(&mut self.prev_pad, pad);
        let edge = |now: bool, before: bool| now && !before;
        let any_pad = edge(pad.a, prev.a) || edge(pad.b, prev.b) || edge(pad.start, prev.start) || pad.left != [0.0; 2] || pad.right != [0.0; 2] || pad.up || pad.down;
        if any_pad {
            self.pad_mode = true;
        }
        self.poll_world();
        if let (Some(g), Some(r)) = (&mut self.game, &mut self.renderer) {
            g.drain(Some(r));
            r.animate(&self.models.atlas, (self.started.elapsed().as_millis() / 50) as u64);
        }

        // Death and container screens come from the server.
        let death = self.game.as_ref().map(|g| (g.dead.clone(), g.xp.2, g.hardcore));
        if let Some((dead, score, hardcore)) = death {
            let showing = matches!(self.screens.last(), Some(Screen::Death { .. }));
            match dead {
                Some(message) if !showing => {
                    self.screens.push(Screen::Death { message, score, hardcore, since: Instant::now() });
                    self.set_grab(false);
                }
                None if showing => {
                    self.screens.pop();
                    self.set_grab(true);
                }
                _ => {}
            }
        }

        let (w, h) = self.window_size();
        let scale = self.scale();
        let in_world = self.game.as_ref().is_some_and(|g| g.in_world && g.ready());
        let container_open = self.game.as_ref().is_some_and(Game::screen_open) && self.screens.is_empty();

        // Controller: in the inventory the left stick moves a cursor; in menus it moves focus.
        if container_open && self.pad_mode {
            self.mouse_px.0 = (self.mouse_px.0 + pad.left[0] * PAD_CURSOR_SPEED * scale * dt).clamp(0.0, w as f32);
            self.mouse_px.1 = (self.mouse_px.1 - pad.left[1] * PAD_CURSOR_SPEED * scale * dt).clamp(0.0, h as f32);
        }
        let nav = if !self.screens.is_empty() { self.pad_nav(&pad) } else { None };
        // B lets go of a grabbed slider instead of closing the screen.
        let release = self.focus.grabbed.is_some() && edge(pad.b, prev.b);
        if release || self.screens.is_empty() {
            self.focus.grabbed = None;
        }
        let slide_dir = (pad.left[0] + (pad.right_d as i32 - pad.left_d as i32) as f32).clamp(-1.0, 1.0);
        let mut input = Input {
            mouse: (self.mouse_px.0 / scale, self.mouse_px.1 / scale),
            clicked: self.clicked || (container_open && edge(pad.a, prev.a)),
            right_clicked: self.right_clicked || (container_open && edge(pad.x, prev.x)),
            held: self.mouse_left,
            double_click: self.double_click,
            // The right stick scrolls inventories (the creative grid) at up to 6 rows a second.
            scroll: self.scroll + if container_open { pad.right[1] * dt * 6.0 } else { 0.0 },
            typed: std::mem::take(&mut self.typed),
            keys: std::mem::take(&mut self.pressed),
            shift: self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight),
            ctrl: self.keys.contains(&KeyCode::ControlLeft) || self.keys.contains(&KeyCode::ControlRight),
            nav,
            accept: !self.screens.is_empty() && edge(pad.a, prev.a),
            back: !self.screens.is_empty() && edge(pad.b, prev.b) && !release,
            // A full stick crosses a slider in two seconds.
            slide: slide_dir * dt * 0.5,
            pad: self.pad_mode && !container_open,
        };
        if self.clicked {
            self.pad_mode = false;
            input.pad = false;
        }
        (self.clicked, self.right_clicked, self.double_click, self.scroll) = (false, false, false, 0.0);

        // In-world controls (only with no menu open).
        let menus = !self.screens.is_empty();
        if let Some(g) = &mut self.game {
            if !menus && g.ready() {
                // Discrete controller actions.
                if !g.screen_open() && g.chat_input.is_none() {
                    if edge(pad.a, prev.a) {
                        g.jump_pressed();
                    }
                    if edge(pad.rb, prev.rb) {
                        g.select_slot(g.slot + 1);
                    }
                    if edge(pad.lb, prev.lb) {
                        g.select_slot(g.slot + 8);
                    }
                    if edge(pad.rt, prev.rt) {
                        self.attack_pressed = true;
                    }
                    if edge(pad.b, prev.b) || edge(pad.right_d, prev.right_d) {
                        g.drop_item(false);
                    }
                }
                if edge(pad.y, prev.y) {
                    g.toggle_inventory();
                    if g.screen_open() {
                        self.mouse_px = (w as f32 / 2.0, h as f32 / 2.0);
                    }
                }
                if edge(pad.b, prev.b) && g.screen_open() {
                    g.close_screens();
                }
                let ctrl_look = 0.5 + self.options.controller_sensitivity;
                let curve = |v: f32| v * v.abs();
                if !g.screen_open() {
                    g.cam.yaw += curve(pad.right[0]) * PAD_LOOK_SPEED * ctrl_look * dt;
                    g.cam.pitch = (g.cam.pitch - curve(pad.right[1]) * PAD_LOOK_SPEED * 0.75 * ctrl_look * dt).clamp(-89.9, 89.9);
                }
                let typing = g.chat_input.is_some();
                let held = |k: Key| !typing && self.keys.contains(&self.options.key(k));
                let axis = |pos: Key, neg: Key, stick: f32| ((held(pos) as i32 - held(neg) as i32) as f32 + stick).clamp(-1.0, 1.0);
                let pad_move = if g.screen_open() { [0.0; 2] } else { pad.left };
                let controls = Controls {
                    forward: axis(Key::Forward, Key::Back, pad_move[1]),
                    strafe: axis(Key::Left, Key::Right, -pad_move[0]),
                    jump: held(Key::Jump) || (pad.a && !g.screen_open()),
                    sneak: held(Key::Sneak) || pad.r3,
                    sprint: held(Key::Sprint) || pad.l3,
                    mine: (self.mouse_left && self.grabbed) || pad.rt,
                    use_item: (self.mouse_right && self.grabbed) || pad.lt,
                    attack_pressed: std::mem::take(&mut self.attack_pressed),
                };
                g.update(dt, &controls);
                g.update_target();
            } else if in_world {
                // A menu is open over the world: keep ticking (the server doesn't pause), no input.
                g.update(dt, &Controls::default());
            }
            if edge(pad.start, prev.start) && !menus && g.ready() {
                self.screens.push(Screen::Pause);
                self.set_grab(false);
            }
        }

        // Audio.
        if let Some(a) = &mut self.audio {
            let underwater = self.game.as_ref().is_some_and(|g| g.in_world && g.eyes_in_water);
            a.tick_ambient(underwater.then_some("ambient.underwater.loop"));
            if let Some(g) = &mut self.game {
                a.set_listener(g.cam.eye, g.cam.yaw);
                // Vanilla's sound engine has a limited channel pool; a chain of explosions mustn't start hundreds at once.
                let mut started: HashMap<String, u32> = HashMap::new();
                for (ev, at, vol, pitch, cat) in g.sounds.drain(..) {
                    let n = started.entry(ev.clone()).or_insert(0);
                    *n += 1;
                    if *n <= 8 {
                        a.play(&ev, at, vol, pitch, cat);
                    }
                }
            }
            let clicks = ui::CLICKS.with(|c| c.replace(0));
            for _ in 0..clicks {
                a.play("ui.button.click", None, 0.25, 1.0, category::UI);
            }
            let kind = match &self.game {
                Some(g) if g.in_world && g.game_mode == 1 => "music.creative",
                Some(g) if g.in_world => "music.game",
                _ => "music.menu",
            };
            a.tick_music(kind);
        } else {
            ui::CLICKS.with(|c| c.set(0));
        }

        // Draw.
        let mut painter = Painter::new(&self.models, w, h, self.options.gui_scale);
        let (aspect, gpu) = self.renderer.as_ref().map_or((1.0, String::new()), |r| (r.aspect(), r.adapter_name.clone()));
        let mut overlay = Overlay::default();
        let mut view_proj = Mat4::IDENTITY;
        let mut eye = Vec3::ZERO;
        let mut env = Environment::default();
        let show_tab = self.held(Key::PlayerList) && self.screens.is_empty();
        if in_world {
            let g = self.game.as_mut().unwrap();
            let (vp, e, hand) = g.camera(aspect, &self.options);
            (view_proj, eye) = (vp, e);
            env = g.environment(&self.options);
            let r = self.renderer.as_ref();
            overlay = g.overlay(&self.options, hand, &|min, max| r.is_none_or(|r| r.entity_visible(&vp, min, max)));
            if !self.hide_hud {
                g.hud(&mut painter, &self.options, show_tab);
            } else {
                overlay.hand_item.clear();
                overlay.hand_skin.clear();
            }
            if self.show_debug && !self.hide_hud {
                let server = if self.singleplayer { "Integrated server (vanilla, local)".to_string() } else { format!("Server: {}", self.options.last_server) };
                let info = g.debug_info(self.fps, &self.options, (w, h), &gpu, &server);
                debug::draw(&mut painter, &info);
            }
            if self.options.fps_graph && !self.hide_hud {
                // Under F3's right column when it's open, else in the top right corner.
                let y = if self.show_debug { painter.height - 62.0 } else { 2.0 };
                crate::hud::fps_graph(&mut painter, &self.frame_times, self.fps, y);
            }
            if container_open {
                g.container_screen(&mut painter, &input);
                // The OS cursor stays hidden while the controller moves this one.
                if self.pad_mode {
                    crate::hud::pad_cursor(&mut painter, input.mouse);
                }
            }
        }

        // The menu on top of the stack.
        let mut action = Action::None;
        if !self.screens.is_empty() {
            let status = self.status_text();
            let time = self.started.elapsed().as_secs_f32();
            let worlds_dir = self.setup.worlds.clone();
            let mut ctx = Ctx {
                options: &mut self.options,
                servers: &mut self.servers,
                worlds_dir: &worlds_dir,
                player: &self.name,
                in_world,
                singleplayer: self.singleplayer,
                difficulty: self.game.as_ref().map_or(2, |g| g.difficulty),
                status: &status,
                time,
            };
            let mut ui = ui::Ui::new(&mut painter, &input, &mut self.focus);
            let top = self.screens.last_mut().unwrap();
            action = screens::run(top, &mut ui, &mut ctx);
            ui.finish();
        }
        let gui = painter.gui;
        self.handle(action, event_loop);
        self.apply_options();

        if let Some(r) = &mut self.renderer {
            (r.cave_culling, r.entity_culling) = (self.options.cave_culling, self.options.entity_culling);
            let fog_end = self.options.render_distance as f32 * 16.0;
            let stats = r.render(view_proj, eye, fog_end, &env, &overlay, &gui, in_world);
            if let Some(g) = &mut self.game {
                g.stats = (stats.drawn, stats.loaded, stats.triangles, stats.gpu_bytes);
            }
            if !self.first_frame {
                self.first_frame = true;
                println!("first frame after {:?} ({})", self.started.elapsed(), r.adapter_name);
            }
        }
        self.frames += 1;
        if self.frame_times.len() >= 120 {
            self.frame_times.pop_front();
        }
        self.frame_times.push_back(dt * 1000.0);
        if self.fps_since.elapsed() >= Duration::from_secs(1) {
            self.fps = self.frames as f32 / self.fps_since.elapsed().as_secs_f32();
            self.frames = 0;
            if let Some(g) = self.game.as_ref().filter(|_| std::env::var_os("MCRS_DEBUG_STATS").is_some()) {
                let (drawn, loaded, tris, _) = g.stats;
                let i = g.debug_info(self.fps, &self.options, (w, h), "", "");
                println!("fps {:.0}, sections {drawn}/{loaded}, {tris} triangles, cave culling {}, light {} sky {} block", self.fps, self.options.cave_culling, i.sky_light, i.block_light);
            }
            self.fps_since = Instant::now();
            if let Some(win) = &self.window {
                let mode = if self.game.is_none() { "" } else if self.singleplayer { " - Singleplayer" } else { " - Multiplayer (3rd-party Server)" };
                win.set_title(&format!("Minecraft 26.3 \u{b7} Rust Edition{mode}"));
            }
        }
    }

    fn handle(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        match action {
            Action::None => {}
            Action::Push(s) => {
                self.screens.push(s);
                self.focus = Focus::default();
            }
            Action::Pop => {
                self.screens.pop();
                self.focus = Focus::default();
                if self.screens.is_empty() {
                    self.resume();
                }
            }
            Action::Replace(s) => {
                self.screens.pop();
                self.screens.push(s);
                self.focus = Focus::default();
            }
            Action::Title => {
                self.screens = vec![screens::title(&self.models.gui.splashes)];
                self.focus = Focus::default();
            }
            Action::Quit => {
                self.quit = true;
                event_loop.exit();
            }
            Action::PlayWorld(folder) => self.play_world(folder),
            Action::CreateWorld(nw) => match worlds::create(&self.setup.worlds, &nw, &self.name) {
                Ok(folder) => {
                    self.screens.pop();
                    self.play_world(folder);
                }
                Err(e) => self.screens.push(Screen::Message { title: "Couldn't create the world".into(), text: format!("{e:#}"), back_to_servers: false }),
            },
            Action::Connect(addr) => {
                self.singleplayer = false;
                self.options.last_server = addr.clone();
                self.connect(addr);
            }
            Action::Disconnect => self.disconnect(None),
            Action::Respawn => {
                if let Some(g) = &mut self.game {
                    g.respawn();
                }
            }
            Action::OptionsChanged => {}
            Action::Command(cmd) => {
                if let Some(g) = &self.game {
                    let _ = session::send_chat(&g.tx, &cmd);
                }
            }
        }
        // A singleplayer world freezes while its pause menu is open, like vanilla's integrated server.
        let pause = self.singleplayer && self.game.is_some() && matches!(self.screens.first(), Some(Screen::Pause));
        if pause != self.paused_server {
            if let Some(s) = &mut self.server {
                s.command(if pause { "tick freeze" } else { "tick unfreeze" });
            }
            self.paused_server = pause;
        }
    }

    fn resume(&mut self) {
        if self.game.as_ref().is_some_and(|g| g.ready()) {
            self.set_grab(true);
        }
    }

    /// Keys while in the world with no menu open.
    fn game_key(&mut self, code: KeyCode, repeat: bool) {
        let Some(g) = &mut self.game else { return };
        if repeat {
            return;
        }
        if self.keys.contains(&KeyCode::F3) && code != KeyCode::F3 {
            self.f3_combo = true;
            match code {
                KeyCode::KeyB => g.show_hitboxes = !g.show_hitboxes,
                KeyCode::KeyG => g.show_chunk_borders = !g.show_chunk_borders,
                KeyCode::KeyP => self.options.pause_on_lost_focus = !self.options.pause_on_lost_focus,
                _ => {}
            }
            return;
        }
        if g.screen_open() {
            if code == KeyCode::Escape || (code == self.options.key(Key::Inventory) && !g.typing_in_screen()) {
                g.close_screens();
                self.set_grab(true);
            }
            return;
        }
        let k = |a: Key| self.options.key(a);
        match code {
            KeyCode::Escape => {
                self.screens.push(Screen::Pause);
                self.focus = Focus::default();
                self.set_grab(false);
            }
            KeyCode::F1 => self.hide_hud = !self.hide_hud,
            c if c == k(Key::Inventory) => {
                g.toggle_inventory();
                self.set_grab(false);
            }
            c if c == k(Key::Chat) || c == k(Key::Command) => {
                g.chat_input = Some(if c == k(Key::Command) { "/".into() } else { String::new() });
                self.keys.clear();
                self.set_grab(false);
            }
            c if c == k(Key::Drop) => {
                let whole = self.keys.contains(&KeyCode::ControlLeft);
                g.drop_item(whole);
            }
            c if c == k(Key::Fly) => g.toggle_mode(),
            c if c == k(Key::Jump) => g.jump_pressed(),
            c if c == k(Key::PickBlock) => g.pick_block(),
            _ => {
                let digits = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9];
                if let Some(i) = digits.iter().position(|d| *d == code) {
                    g.select_slot(i as u8);
                }
            }
        }
    }

    /// Typing into the chat box.
    fn chat_key(&mut self, code: KeyCode, text: Option<&str>) {
        let Some(g) = &mut self.game else { return };
        match code {
            KeyCode::Escape => {
                g.chat_input = None;
                self.set_grab(true);
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                let line = g.chat_input.take().unwrap_or_default();
                if !line.trim().is_empty() {
                    let _ = session::send_chat(&g.tx, line.trim());
                }
                self.set_grab(true);
            }
            KeyCode::Backspace => {
                g.chat_input.as_mut().map(String::pop);
            }
            _ => {
                if let (Some(text), Some(input)) = (text, &mut g.chat_input) {
                    // Vanilla caps chat at 256 characters.
                    input.extend(text.chars().filter(|c| !c.is_control()).take(256usize.saturating_sub(input.len())));
                }
            }
        }
    }

    /// Waits for worlds being saved (on exit).
    pub fn shutdown(&mut self) {
        self.disconnect(None);
        for h in self.saving.drain(..) {
            let _ = h.join();
        }
    }
}

/// The item-stack component table the session needs (built once per connection).
fn mc_inventory_components(models: &BlockModels) -> crate::inventory::Components {
    crate::inventory::Components::new(&models.components)
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Minecraft 26.3 \u{b7} Rust Edition").with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        if self.options.fullscreen {
            window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let size = window.inner_size();
        match Renderer::new(window.clone(), size.width, size.height, &self.models.atlas, &self.models.gui, self.options.vsync) {
            Ok(r) => self.renderer = Some(r),
            Err(e) => {
                eprintln!("renderer failed: {e:#}");
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        if let Some(a) = &mut self.audio {
            a.volumes = self.options.volumes;
        }
        if let Some(world) = self.start_world.take() {
            self.play_world(world);
        } else if let Some(addr) = self.start_server.take() {
            self.connect(addr);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.quit = true;
                event_loop.exit();
            }
            WindowEvent::Resized(s) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(s.width, s.height);
                }
            }
            WindowEvent::Focused(false) => {
                self.focused = false;
                self.keys.clear();
                self.mouse_left = false;
                self.mouse_right = false;
                self.set_grab(false);
                // Vanilla opens the pause menu when the window loses focus (unless F3+P turned that off).
                if self.options.pause_on_lost_focus && self.screens.is_empty() && self.game.as_ref().is_some_and(|g| g.ready() && !g.screen_open() && g.chat_input.is_none()) {
                    self.screens.push(Screen::Pause);
                }
            }
            WindowEvent::Focused(true) => self.focused = true,
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_px = (position.x as f32, position.y as f32);
                self.pad_mode = false;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                let in_game = self.screens.is_empty() && self.game.as_ref().is_some_and(|g| g.ready() && !g.screen_open() && g.chat_input.is_none());
                match button {
                    MouseButton::Left => {
                        self.mouse_left = down;
                        if down {
                            self.clicked = true;
                            self.double_click = self.last_click.is_some_and(|t| t.elapsed() < Duration::from_millis(250));
                            self.last_click = Some(Instant::now());
                            if in_game {
                                if !self.grabbed {
                                    self.set_grab(true);
                                } else {
                                    self.attack_pressed = true;
                                }
                            }
                        }
                    }
                    MouseButton::Right => {
                        self.mouse_right = down;
                        if down {
                            self.right_clicked = true;
                        }
                    }
                    MouseButton::Middle if down && in_game => {
                        if let Some(g) = &mut self.game {
                            g.pick_block();
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                let in_game = self.screens.is_empty() && self.grabbed && self.game.as_ref().is_some_and(|g| !g.screen_open());
                match &mut self.game {
                    // Scrolling down moves right along the hotbar, like vanilla.
                    Some(g) if in_game => {
                        if dy < 0.0 {
                            g.select_slot(g.slot + 1);
                        } else if dy > 0.0 {
                            g.select_slot(g.slot + 8);
                        }
                    }
                    _ => self.scroll += dy,
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                if event.state == ElementState::Released {
                    self.keys.remove(&code);
                    if code == KeyCode::F3 {
                        if !self.f3_combo && self.screens.is_empty() {
                            self.show_debug = !self.show_debug;
                        }
                        self.f3_combo = false;
                    }
                    return;
                }
                self.pad_mode = false;
                if code == KeyCode::F11 && !event.repeat {
                    self.options.fullscreen = !self.options.fullscreen;
                }
                let chatting = self.screens.is_empty() && self.game.as_ref().is_some_and(|g| g.chat_input.is_some());
                if chatting {
                    let text = event.text.as_ref().map(|t| t.to_string());
                    self.chat_key(code, text.as_deref());
                    return;
                }
                if !self.screens.is_empty() || self.game.as_ref().is_some_and(Game::screen_open) {
                    // Menus and container screens read this frame's keys and typed text.
                    self.pressed.push(code);
                    if let Some(t) = &event.text {
                        self.typed.push_str(t);
                    }
                }
                if self.screens.is_empty() {
                    self.game_key(code, event.repeat);
                }
                self.keys.insert(code);
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.grabbed && self.screens.is_empty() {
                if let Some(g) = &mut self.game {
                    if g.screen_open() || g.chat_input.is_some() {
                        return;
                    }
                    let f = self.options.mouse_factor();
                    let inv = if self.options.invert_mouse { -1.0 } else { 1.0 };
                    g.cam.yaw += dx as f32 * f;
                    g.cam.pitch = (g.cam.pitch + dy as f32 * f * inv).clamp(-89.9, 89.9);
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit {
            return;
        }
        // Frame cap without vsync: wait until the next frame is due.
        if !self.options.vsync && self.options.max_fps < 260 {
            let next = self.last_frame + Duration::from_secs_f32(1.0 / self.options.max_fps as f32);
            if Instant::now() < next {
                event_loop.set_control_flow(ControlFlow::WaitUntil(next));
                return;
            }
        }
        event_loop.set_control_flow(ControlFlow::Poll);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}
