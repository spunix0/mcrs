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
        eprintln!("usage: mc-launcher --install <version> [--launch | --singleplayer [--world <folder>] [--accept-eula]]");
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
        let world = args.iter().position(|a| a == "--world").and_then(|i| args.get(i + 1)).cloned();
        match crate::singleplayer::launch_client(&paths, &prepared, &settings.username, eula, world.as_deref(), &progress) {
            Ok(mut client) => {
                println!("Rust client running (pid {})", client.id());
                let _ = client.wait();
            }
            Err(e) => eprintln!("couldn't start the Rust client: {e:#}"),
        }
        return true;
    }
    if args.iter().any(|a| a == "--launch") {
        let settings = Settings::load(&paths.root);
        let account = Account::offline(&settings.username);
        // `--join host:port` drops straight into a server (vanilla's quick play).
        let join: Vec<String> = args.iter().position(|a| a == "--join").and_then(|i| args.get(i + 1)).map(|a| vec!["--quickPlayMultiplayer".into(), a.clone()]).unwrap_or_default();
        match launch::launch(&paths, &prepared, &account, settings.memory_mb, None, &join) {
            Ok(child) => println!("launched (pid {}); log: game/logs/mcrs-launch.log", child.id()),
            Err(e) => eprintln!("launch failed: {e:#}"),
        }
    }
    true
}
