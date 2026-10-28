//! The window: fly camera, chunk meshing on worker threads, rendering, and movement packets back to the server.

use crate::session::{self, Event};
use glam::{Mat4, Vec3};
use mc_assets::BlockModels;
use mc_render::{ChunkMesh, Renderer};
use mc_world::Chunk;
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

const EYE_HEIGHT: f64 = 1.62;
const VIEW_DISTANCE_CHUNKS: f32 = 10.0;

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
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    net: Receiver<Event>,
    tx: mc_protocol::Sender,
    jobs: Sender<Arc<Chunk>>,
    meshes: Receiver<ChunkMesh>,
    loaded: HashSet<(i32, i32)>,
    cam: Camera,
    keys: HashSet<KeyCode>,
    grabbed: bool,
    in_world: bool,
    last_send: Instant,
    last_frame: Instant,
    frames: u32,
    fps_since: Instant,
    first_frame: bool,
    first_chunk: bool,
    status: String,
    received: u32,
    meshed: u32,
}

impl App {
    pub fn new(started: Instant, models: Arc<BlockModels>, net: Receiver<Event>, tx: mc_protocol::Sender, vsync: bool) -> Self {
        let (jobs, job_rx) = channel::<Arc<Chunk>>();
        let (mesh_tx, meshes) = channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(2).max(2));
        for _ in 0..workers {
            let (job_rx, mesh_tx, models) = (job_rx.clone(), mesh_tx.clone(), models.clone());
            std::thread::spawn(move || {
                loop {
                    let Ok(chunk) = job_rx.lock().unwrap().recv() else { return };
                    if mesh_tx.send(mc_render::mesh_chunk(&chunk, &models)).is_err() {
                        return;
                    }
                }
            });
        }
        Self {
            started,
            models,
            vsync,
            window: None,
            renderer: None,
            net,
            tx,
            jobs,
            meshes,
            loaded: HashSet::new(),
            cam: Camera { eye: [0.0, 80.0, 0.0], yaw: 0.0, pitch: 20.0 },
            keys: HashSet::new(),
            grabbed: false,
            in_world: false,
            last_send: Instant::now(),
            last_frame: Instant::now(),
            frames: 0,
            fps_since: Instant::now(),
            first_frame: false,
            first_chunk: false,
            status: "connecting".into(),
            received: 0,
            meshed: 0,
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

    fn drain(&mut self) {
        while let Ok(ev) = self.net.try_recv() {
            match ev {
                Event::LoggedIn(name) => self.status = format!("logged in as {name}"),
                Event::InWorld => {
                    self.in_world = true;
                    self.status = "in world".into();
                }
                Event::Chunk(c) => {
                    self.received += 1;
                    self.loaded.insert((c.x, c.z));
                    let _ = self.jobs.send(Arc::new(c));
                }
                Event::Forget(x, z) => {
                    self.loaded.remove(&(x, z));
                    if let Some(r) = &mut self.renderer {
                        r.remove(x, z);
                    }
                }
                Event::Teleport(feet, rot) => {
                    self.cam.eye = [feet[0], feet[1] + EYE_HEIGHT, feet[2]];
                    self.cam.yaw = rot[0];
                    self.cam.pitch = rot[1];
                }
                Event::Chat(msg) => println!("[chat] {msg}"),
                Event::Health(_) => {}
            }
        }
        if let Some(r) = &mut self.renderer {
            while let Ok(mesh) = self.meshes.try_recv() {
                self.meshed += 1;
                if self.loaded.contains(&(mesh.x, mesh.z)) {
                    r.upload(&mesh);
                    if !self.first_chunk {
                        self.first_chunk = true;
                        println!("first chunk on screen after {:?}", self.started.elapsed());
                    }
                }
            }
        }
    }

    fn update(&mut self, dt: f32) {
        let held = |k| self.keys.contains(&k);
        let speed = if held(KeyCode::ControlLeft) { 30.0 } else { 10.0 } * dt as f64;
        let y = self.cam.yaw.to_radians() as f64;
        let forward = [-y.sin(), 0.0, y.cos()];
        let right = [-y.cos(), 0.0, -y.sin()];
        let mut mv = [0.0f64; 3];
        let mut add = |v: [f64; 3], s: f64| (0..3).for_each(|i| mv[i] += v[i] * s);
        if held(KeyCode::KeyW) { add(forward, 1.0) }
        if held(KeyCode::KeyS) { add(forward, -1.0) }
        if held(KeyCode::KeyD) { add(right, 1.0) }
        if held(KeyCode::KeyA) { add(right, -1.0) }
        if held(KeyCode::Space) { add([0.0, 1.0, 0.0], 1.0) }
        if held(KeyCode::ShiftLeft) { add([0.0, -1.0, 0.0], 1.0) }
        let len = (mv[0] * mv[0] + mv[1] * mv[1] + mv[2] * mv[2]).sqrt();
        if len > 0.0 {
            for i in 0..3 {
                self.cam.eye[i] += mv[i] / len * speed;
            }
        }
        if self.in_world && self.last_send.elapsed() >= Duration::from_millis(50) {
            self.last_send = Instant::now();
            let [x, y, z] = self.cam.eye;
            let _ = session::send_tick(&self.tx, [x, y - EYE_HEIGHT, z], [self.cam.yaw, self.cam.pitch]);
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.drain();
        self.update(dt);
        let Some(r) = &mut self.renderer else { return };
        let eye = Vec3::new(self.cam.eye[0] as f32, self.cam.eye[1] as f32, self.cam.eye[2] as f32);
        let view = glam::camera::rh::view::look_to_mat4(eye, self.cam.look(), Vec3::Y);
        // DirectX convention = wgpu's: Y up in clip space, depth 0..1.
        let proj: Mat4 = glam::camera::rh::proj::directx::perspective(70f32.to_radians(), r.aspect(), 0.05, 1000.0);
        let stats = r.render(proj * view, eye, VIEW_DISTANCE_CHUNKS * 16.0);
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
                    "mcrs · Rust Edition  |  {fps:.0} fps  |  {}/{} chunks  |  {x:.0} {y:.0} {z:.0}  |  {}",
                    stats.drawn, stats.loaded, self.status
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
        match Renderer::new(window.clone(), size.width, size.height, &self.models.atlas, self.vsync) {
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
                self.keys.clear();
                self.set_grab(false);
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } if !self.grabbed => self.set_grab(true),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            if code == KeyCode::Escape {
                                self.set_grab(false);
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
