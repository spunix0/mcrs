//! Game client: window and input loop (winit, gilrs), menus, player physics, ties the other crates together.
//!
//! `mc-client [--data <dir>] [--name Player]` opens the title screen. Singleplayer worlds run on a hidden vanilla
//! server (`--java`, `--server-jar`, `--eula` from the launcher); multiplayer connects to any offline-mode server.
//! `--world <folder>` or `--connect host:port` skip the title screen. `--run "/give @s stone 64"` (repeatable)
//! sends a chat line or command after joining. `--no-noclip` disables the spectator fly toggle.
//! `--headless --connect host:port [--say "hi"] [--for 30] [--greet]` runs without a window; stdin lines are chat.

mod app;
mod audio;
mod containers;
mod creative;
mod debug;
mod entities;
mod game;
mod hand;
mod hud;
mod inventory;
mod models;
mod options;
mod particles;
mod physics;
mod screens;
mod servers;
mod session;
mod sky;
mod target;
mod ui;
mod worlds;

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

/// The launcher's settings file: player name and whether the EULA was accepted there.
fn launcher_settings(data: &std::path::Path) -> (Option<String>, bool) {
    let v: serde_json::Value = std::fs::read_to_string(data.join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    (v["username"].as_str().map(str::to_string), v["eula_accepted"].as_bool().unwrap_or(false))
}

fn run() -> Result<()> {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let data = arg(&args, "--data").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("mcrs").join("data"));
    let (saved_name, eula) = launcher_settings(&data);
    let name = arg(&args, "--name").or(saved_name).unwrap_or_else(|| "Player".into());
    if args.iter().any(|a| a == "--headless") {
        let addr = arg(&args, "--connect").unwrap_or_else(|| "127.0.0.1:25565".into());
        return headless(&args, &addr, &name, args.iter().any(|a| a == "--greet"));
    }

    let jar = data.join("versions").join("26.3").join("26.3.jar");
    let reports = data.join("reports");
    let models = Arc::new(mc_assets::BlockModels::load(&jar, &reports).context("loading game assets")?);
    println!("assets ready after {:?} ({} block states)", started.elapsed(), models.states.len());

    let setup = worlds::ServerSetup {
        java: arg(&args, "--java").map(PathBuf::from).unwrap_or_else(|| data.join("runtime").join("java-runtime-epsilon").join("bin").join("java.exe")),
        server_jar: arg(&args, "--server-jar").map(PathBuf::from).unwrap_or_else(|| data.join("versions").join("26.3").join("server.jar")),
        worlds: data.join("worlds"),
        eula: eula || args.iter().any(|a| a == "--eula"),
    };
    let _ = std::fs::create_dir_all(&setup.worlds);

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let noclip = !args.iter().any(|a| a == "--no-noclip");
    let mut app = app::App::new(started, models, setup, name, noclip);
    if args.iter().any(|a| a == "--no-vsync") {
        app.options.vsync = false;
    }
    if let Some(v) = arg(&args, "--view-distance").and_then(|s| s.parse::<u8>().ok()) {
        app.options.render_distance = v.clamp(2, 32);
    }
    app.join_commands = args.windows(2).filter(|w| w[0] == "--run").map(|w| w[1].clone()).collect();
    app.start_world = arg(&args, "--world");
    app.start_server = arg(&args, "--connect");
    app.audio = audio::Audio::new(&data, "26.3").map_err(|e| eprintln!("sound disabled: {e:#}")).ok();
    event_loop.run_app(&mut app)?;
    app.shutdown();
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
        Event::Teleport(p, _, _) => println!("position {:.2} {:.2} {:.2}", p[0], p[1], p[2]),
        Event::Chat(m) => println!("[chat] {m}"),
        Event::Health { health, .. } => println!("health {health:.1}"),
        Event::ChatTag(t) => {
            println!("[chat] {}", mc_protocol::text::plain(&t));
            if std::env::var_os("MCRS_DEBUG_CHAT").is_some() {
                println!("{t:?}");
            }
        }
        Event::Experience { level, total, .. } => println!("xp level {level} ({total} points)"),
        Event::Time { day } => println!("time {day:?}"),
        _ => {}
    })?;
    println!("disconnected: {reason}");
    println!("{} chunks received", chunks.load(Relaxed));
    Ok(())
}
