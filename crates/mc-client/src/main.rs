//! Game client: window and input loop (winit, gilrs), prediction and player physics, ties the other crates together.
//!
//! `mc-client [--connect 127.0.0.1:25565] [--name Player] [--view-distance 8] [--no-vsync] [--no-noclip]` opens a window and renders the world.
//! Starts walking (survival); double-tap jump or F to fly (spectator noclip, needs operator rights on the server).
//! `--run "/give @s stone 64"` (repeatable) sends a chat line or command after joining.
//! `--headless [--say "hi"] [--for 30] [--greet]` runs without a window; stdin lines are sent as chat.

mod app;
mod entities;
mod hand;
mod audio;
mod hud;
mod inventory;
mod physics;
mod session;
mod target;

use anyhow::{Context, Result};
use session::{Event, Options};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::time::{Duration, Instant};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let addr = arg(&args, "--connect").unwrap_or_else(|| "127.0.0.1:25565".into());
    let name = arg(&args, "--name").unwrap_or_else(|| "Player".into());
    let greet = args.iter().any(|a| a == "--greet");
    if args.iter().any(|a| a == "--headless") {
        return headless(&args, &addr, &name, greet);
    }

    let data = arg(&args, "--data").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("mcrs").join("data")
    });
    let jar = data.join("versions").join("26.3").join("26.3.jar");
    let reports = data.join("reports");

    // Load assets and connect at the same time.
    let assets = std::thread::spawn(move || mc_assets::BlockModels::load(&jar, &reports));
    let (tx, rx) = mc_protocol::connect(&addr).with_context(|| format!("connecting to {addr}"))?;
    let models = Arc::new(assets.join().expect("asset thread panicked").context("loading block models")?);
    println!("assets ready after {:?} ({} block states)", started.elapsed(), models.states.len());

    let view_distance: u8 = arg(&args, "--view-distance").and_then(|s| s.parse().ok()).unwrap_or(8).clamp(2, 64);
    let (net_tx, net_rx) = std::sync::mpsc::channel();
    {
        let (tx, block_states) = (tx.clone(), models.states.len());
        let components = inventory::Components::new(&models.components);
        std::thread::spawn(move || {
            let opts = Options {
                addr: &addr,
                name: &name,
                greet,
                block_states: Some(block_states),
                drives_movement: true,
                view_distance,
                components: Some(&components),
            };
            match session::run(&opts, tx, rx, |e| {
                let _ = net_tx.send(e);
            }) {
                Ok(reason) => println!("disconnected: {reason}"),
                Err(e) => eprintln!("connection error: {e:#}"),
            }
        });
    }

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let vsync = !args.iter().any(|a| a == "--no-vsync");
    let noclip = !args.iter().any(|a| a == "--no-noclip");
    let mut app = app::App::new(started, models, net_rx, tx, vsync, noclip, view_distance);
    app.join_commands = args.windows(2).filter(|w| w[0] == "--run").map(|w| w[1].clone()).collect();
    app.audio = audio::Audio::new(&data, "26.3").map_err(|e| eprintln!("sound disabled: {e:#}")).ok();
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn headless(args: &[String], addr: &str, name: &str, greet: bool) -> Result<()> {
    let say = arg(args, "--say");
    let duration = arg(args, "--for").and_then(|s| s.parse::<u64>().ok());
    println!("connecting to {addr} as {name} (protocol {})", mc_protocol::PROTOCOL_VERSION);
    let (tx, rx) = mc_protocol::connect(addr)?;
    let chunks = Arc::new(AtomicU32::new(0));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();

    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            if ready_rx.recv().is_err() {
                return;
            }
            std::thread::sleep(Duration::from_secs(2));
            if let Some(msg) = say {
                let _ = session::send_chat(&tx, &msg);
            }
            for line in std::io::stdin().lines().map_while(Result::ok) {
                if !line.trim().is_empty() && session::send_chat(&tx, line.trim()).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(secs) = duration {
        let chunks = chunks.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            println!("done after {secs}s: {} chunks received", chunks.load(Relaxed));
            std::process::exit(0);
        });
    }

    let opts = Options { addr, name, greet, block_states: None, drives_movement: false, view_distance: 2, components: None };
    let reason = session::run(&opts, tx, rx, |e| match e {
        Event::LoggedIn(n) => println!("logged in as {n}"),
        Event::InWorld => {
            println!("in world");
            let _ = ready_tx.send(());
        }
        Event::Chunk(_) => {
            chunks.fetch_add(1, Relaxed);
        }
        Event::Teleport(p, _) => println!("position {:.2} {:.2} {:.2}", p[0], p[1], p[2]),
        Event::Chat(m) => println!("[chat] {m}"),
        Event::Health(h) => println!("health {h:.1}"),
        Event::Forget(..) | Event::Blocks(_) | Event::Light(..) => {}
        Event::Inventory(_) | Event::InventorySlot(..) | Event::PlayerSlot(..) | Event::HeldSlot(_) => {}
        Event::Sound { .. } | Event::LevelEvent { .. } => {}
        Event::EntityAdd { .. } | Event::EntityMove { .. } | Event::EntityRot { .. } | Event::EntityPos { .. } => {}
        Event::EntityHead { .. } | Event::EntityRemove(_) => {}
    })?;
    println!("disconnected: {reason}");
    println!("{} chunks received", chunks.load(Relaxed));
    Ok(())
}
