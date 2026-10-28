//! Game client: window and input loop (winit, gilrs), prediction and player physics, ties the other crates together.
//!
//! For now it runs headless: `mc-client --connect 127.0.0.1:25565 --name Player [--say "hi"] [--for 30]`.
//! Lines typed on stdin are sent as chat (or as commands when they start with `/`).

mod session;

use anyhow::Result;
use session::Stats;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

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
    let args: Vec<String> = std::env::args().collect();
    let addr = arg(&args, "--connect").unwrap_or_else(|| "127.0.0.1:25565".into());
    let name = arg(&args, "--name").unwrap_or_else(|| "Player".into());
    let say = arg(&args, "--say");
    let duration = arg(&args, "--for").and_then(|s| s.parse::<u64>().ok());

    println!("connecting to {addr} as {name} (protocol {})", mc_protocol::PROTOCOL_VERSION);
    let (tx, rx) = mc_protocol::connect(&addr)?;
    let stats = Arc::new(Stats::default());

    {
        let (tx, stats) = (tx.clone(), stats.clone());
        std::thread::spawn(move || {
            while !stats.in_world.load(Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
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
        let stats = stats.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            println!("done after {secs}s: {} chunks received", stats.chunks.load(Relaxed));
            std::process::exit(0);
        });
    }

    session::run(&addr, &name, stats.clone(), tx, rx)?;
    println!("{} chunks received", stats.chunks.load(Relaxed));
    Ok(())
}
