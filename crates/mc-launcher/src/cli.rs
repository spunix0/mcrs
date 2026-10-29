//! Headless mode: `mc-launcher --install <version> [--launch]` downloads (and optionally starts) a version without opening the window.

use crate::install::{self, Paths, Progress};
use crate::launch::{self, Account};
use crate::settings::Settings;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

/// Returns true if the arguments asked for headless mode (and it has been handled).
pub fn run(args: &[String]) -> bool {
    let Some(pos) = args.iter().position(|a| a == "--install") else { return false };
    let Some(id) = args.get(pos + 1) else {
        eprintln!("usage: mc-launcher --install <version> [--launch]");
        return true;
    };
    let paths = Paths::new();
    let progress = Arc::new(Progress::default());
    let manifest = match install::fetch_manifest() {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("warning: couldn't load version list: {e:#}");
            None
        }
    };

    let result = std::thread::scope(|s| {
        let worker = s.spawn(|| install::prepare(&paths, id, manifest.as_ref(), &progress));
        while !worker.is_finished() {
            std::thread::sleep(Duration::from_secs(5));
            let (bd, bt) = (progress.bytes_done.load(Relaxed), progress.bytes_total.load(Relaxed));
            println!(
                "[{}] {}/{} files, {:.0}/{:.0} MB",
                progress.stage.lock().unwrap(),
                progress.files_done.load(Relaxed),
                progress.files_total.load(Relaxed),
                bd as f64 / 1_048_576.0,
                bt as f64 / 1_048_576.0
            );
        }
        worker.join().unwrap()
    });

    let prepared = match result {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install failed: {e:#}");
            return true;
        }
    };
    println!("installed {id}; java at {}", prepared.java.display());

    if args.iter().any(|a| a == "--singleplayer") {
        // Only for scripted testing: the UI asks for EULA acceptance; here `--accept-eula` must be passed explicitly.
        let settings = Settings::load(&paths.root);
        let eula = args.iter().any(|a| a == "--accept-eula") || settings.eula_accepted;
        match crate::singleplayer::start(&paths, &prepared, &settings.username, &settings.world, eula, &progress) {
            Ok(mut running) => {
                println!("world running; Rust client pid {}", running.client.id());
                let _ = running.client.wait();
                println!("client closed; saving world");
                crate::singleplayer::stop(running);
            }
            Err(e) => eprintln!("singleplayer failed: {e:#}"),
        }
        return true;
    }
    if args.iter().any(|a| a == "--launch") {
        let settings = Settings::load(&paths.root);
        let account = Account::offline(&settings.username);
        match launch::launch(&paths, &prepared, &account, settings.memory_mb, None) {
            Ok(child) => println!("launched (pid {}); log: game/logs/mcrs-launch.log", child.id()),
            Err(e) => eprintln!("launch failed: {e:#}"),
        }
    }
    true
}
