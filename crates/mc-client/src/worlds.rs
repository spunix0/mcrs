//! Singleplayer worlds: each lives in `worlds/<name>/` with its own hidden vanilla server (that's how vanilla
//! singleplayer works too, inside the Java client). This module lists, creates, edits and runs them.

use anyhow::{Context, Result, bail};
use std::io::{BufRead, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Where the world server's files are.
#[derive(Clone)]
pub struct ServerSetup {
    pub java: PathBuf,
    pub server_jar: PathBuf,
    pub worlds: PathBuf,
    /// The user accepted the Minecraft EULA in the launcher.
    pub eula: bool,
}

#[derive(Clone, Debug)]
pub struct WorldInfo {
    pub folder: String,
    pub name: String,
    /// Milliseconds since 1970; 0 if never played.
    pub last_played: i64,
    pub game_mode: i32,
    pub hardcore: bool,
    pub cheats: bool,
    pub version: String,
}

impl WorldInfo {
    pub fn mode_name(&self) -> &'static str {
        if self.hardcore {
            return "Hardcore Mode";
        }
        match self.game_mode {
            1 => "Creative Mode",
            2 => "Adventure Mode",
            3 => "Spectator Mode",
            _ => "Survival Mode",
        }
    }
}

/// Settings for a new world, as on vanilla's Create New World screen.
#[derive(Clone, Debug, PartialEq)]
pub struct NewWorld {
    pub name: String,
    pub seed: String,
    /// 0 survival, 1 creative, 2 hardcore.
    pub mode: usize,
    /// 0 peaceful .. 3 hard.
    pub difficulty: usize,
    pub cheats: bool,
    /// 0 default, 1 superflat, 2 large biomes, 3 amplified.
    pub world_type: usize,
    pub structures: bool,
}

impl Default for NewWorld {
    fn default() -> Self {
        Self { name: "New World".into(), seed: String::new(), mode: 0, difficulty: 2, cheats: true, world_type: 0, structures: true }
    }
}

pub const MODES: [&str; 3] = ["Survival", "Creative", "Hardcore"];
pub const DIFFICULTIES: [&str; 4] = ["Peaceful", "Easy", "Normal", "Hard"];
pub const WORLD_TYPES: [&str; 4] = ["Default", "Superflat", "Large Biomes", "Amplified"];

fn read_level_dat(path: &Path) -> Option<mc_nbt::Tag> {
    let mut gz = flate2::read::GzDecoder::new(std::fs::File::open(path).ok()?);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut gz, &mut bytes).ok()?;
    let (_, root) = mc_nbt::read_named(&mut bytes.as_slice()).ok()?;
    Some(root)
}

fn int(tag: Option<&mc_nbt::Tag>) -> i64 {
    match tag {
        Some(mc_nbt::Tag::Byte(v)) => *v as i64,
        Some(mc_nbt::Tag::Short(v)) => *v as i64,
        Some(mc_nbt::Tag::Int(v)) => *v as i64,
        Some(mc_nbt::Tag::Long(v)) => *v,
        _ => 0,
    }
}

/// Worlds in `dir`, most recently played first.
pub fn list(dir: &Path) -> Vec<WorldInfo> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if !path.join("server.properties").exists() {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().into_owned();
        let data = read_level_dat(&path.join("world").join("level.dat"));
        let d = data.as_ref().and_then(|t| t.get("Data"));
        let props = std::fs::read_to_string(path.join("server.properties")).unwrap_or_default();
        let prop = |k: &str| props.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).unwrap_or_default().to_string();
        let mode_prop = match prop("gamemode").as_str() {
            "creative" => 1,
            "adventure" => 2,
            "spectator" => 3,
            _ => 0,
        };
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
        out.push(WorldInfo {
            name: folder.clone(),
            last_played: d.map(|d| int(d.get("LastPlayed"))).filter(|t| *t > 0).unwrap_or(modified),
            game_mode: d.map_or(mode_prop, |d| int(d.get("GameType")) as i32),
            hardcore: d.map(|d| int(d.get("hardcore")) != 0).unwrap_or(prop("hardcore") == "true"),
            cheats: path.join("ops.json").exists(),
            version: d.and_then(|d| d.get("Version")).and_then(|v| v.get("Name")).and_then(mc_nbt::Tag::as_str).unwrap_or("26.3").to_string(),
            folder,
        });
    }
    out.sort_by(|a, b| b.last_played.cmp(&a.last_played));
    out
}

/// A folder name for a world name: unsafe characters replaced, `(1)` appended if it's taken.
pub fn folder_for(dir: &Path, name: &str) -> String {
    let clean: String = name.trim().chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    let base = if clean.trim_matches('.').is_empty() { "World".to_string() } else { clean };
    let mut candidate = base.clone();
    let mut n = 1;
    while dir.join(&candidate).exists() {
        candidate = format!("{base} ({n})");
        n += 1;
    }
    candidate
}

/// Sets `key=value` lines in a properties file, keeping everything else.
fn set_properties(path: &Path, values: &[(&str, String)]) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    for (key, value) in values {
        let line = format!("{key}={value}");
        match lines.iter_mut().find(|l| l.split_once('=').is_some_and(|(k, _)| k == *key)) {
            Some(l) => *l = line,
            None => lines.push(line),
        }
    }
    std::fs::write(path, lines.join("\n") + "\n")?;
    Ok(())
}

fn dashed_uuid(u: u128) -> String {
    let s = format!("{u:032x}");
    format!("{}-{}-{}-{}-{}", &s[0..8], &s[8..12], &s[12..16], &s[16..20], &s[20..32])
}

/// Gives (or takes) the player operator rights, which "Allow Commands" means for a server world.
pub fn set_cheats(world_dir: &Path, player: &str, on: bool) -> Result<()> {
    let ops = world_dir.join("ops.json");
    if on {
        let uuid = dashed_uuid(crate::session::offline_uuid(player));
        std::fs::write(ops, format!(r#"[{{"uuid":"{uuid}","name":"{player}","level":4,"bypassesPlayerLimit":false}}]"#))?;
    } else if ops.exists() {
        std::fs::remove_file(ops)?;
    }
    Ok(())
}

/// Writes a new world's settings; the server generates the terrain on first start.
pub fn create(dir: &Path, w: &NewWorld, player: &str) -> Result<String> {
    let folder = folder_for(dir, &w.name);
    let world_dir = dir.join(&folder);
    std::fs::create_dir_all(&world_dir)?;
    let level_type = ["minecraft:normal", "minecraft:flat", "minecraft:large_biomes", "minecraft:amplified"][w.world_type.min(3)];
    let mode = if w.mode == 1 { "creative" } else { "survival" };
    let difficulty = ["peaceful", "easy", "normal", "hard"][w.difficulty.min(3)];
    set_properties(
        &world_dir.join("server.properties"),
        &[
            ("gamemode", mode.into()),
            ("difficulty", if w.mode == 2 { "hard".into() } else { difficulty.into() }),
            ("hardcore", (w.mode == 2).to_string()),
            ("level-seed", w.seed.clone()),
            ("level-type", level_type.into()),
            ("generate-structures", w.structures.to_string()),
        ],
    )?;
    set_cheats(&world_dir, player, w.cheats && w.mode != 2)?;
    Ok(folder)
}

pub fn rename(dir: &Path, folder: &str, new_name: &str) -> Result<String> {
    let target = folder_for(dir, new_name);
    std::fs::rename(dir.join(folder), dir.join(&target))?;
    Ok(target)
}

/// Deletes a world's folder for good (the screen asks first, like vanilla).
pub fn delete(dir: &Path, folder: &str) -> Result<()> {
    std::fs::remove_dir_all(dir.join(folder))?;
    Ok(())
}

fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// A running world server.
pub struct LocalServer {
    child: Child,
    input: Option<ChildStdin>,
    pub port: u16,
    pub ready: Arc<AtomicBool>,
    /// Latest progress line from the server log ("Preparing spawn area: 42%").
    pub progress: Arc<Mutex<String>>,
    pub log: PathBuf,
}

impl LocalServer {
    pub fn start(setup: &ServerSetup, folder: &str) -> Result<Self> {
        if !setup.eula {
            bail!("Accept the Minecraft EULA in the launcher to play singleplayer worlds.");
        }
        if !setup.server_jar.exists() {
            bail!("The world engine (server.jar) isn't installed yet; press PLAY in the launcher once.");
        }
        let world_dir = setup.worlds.join(folder);
        std::fs::write(world_dir.join("eula.txt"), "eula=true\n")?;
        let port = free_port()?;
        set_properties(
            &world_dir.join("server.properties"),
            &[
                ("server-ip", "127.0.0.1".into()),
                ("server-port", port.to_string()),
                ("online-mode", "false".into()),
                ("white-list", "false".into()),
                ("allow-flight", "true".into()),
                ("spawn-protection", "0".into()),
                ("view-distance", "12".into()),
                ("motd", format!("{folder} (mcrs singleplayer)")),
            ],
        )?;
        let log = world_dir.join("logs").join("latest.log");
        let mut child = Command::new(&setup.java)
            .args(["-Xmx2G", "-jar"])
            .arg(&setup.server_jar)
            .arg("nogui")
            .current_dir(&world_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .context("starting the world server")?;
        let input = child.stdin.take();
        let ready = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new("Starting the world engine".to_string()));
        if let Some(out) = child.stdout.take() {
            let (ready, progress) = (ready.clone(), progress.clone());
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    if line.contains("Done (") {
                        ready.store(true, Relaxed);
                    } else if let Some(p) = line.split("]: ").nth(1) {
                        if p.contains('%') || p.starts_with("Preparing") || p.starts_with("Loading") {
                            *progress.lock().unwrap() = p.trim().to_string();
                        }
                    }
                }
            });
        }
        Ok(Self { child, input, port, ready, progress, log })
    }

    /// `Some(error)` if the server process exited.
    pub fn failed(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(format!("The world server stopped ({status}). See {}", self.log.display())),
            _ => None,
        }
    }

    pub fn command(&mut self, cmd: &str) {
        if let Some(i) = &mut self.input {
            let _ = writeln!(i, "{cmd}");
            let _ = i.flush();
        }
    }

    /// Saves and stops the world in the background; join the handle to wait for it.
    pub fn stop(mut self) -> std::thread::JoinHandle<()> {
        self.command("stop");
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = self.child.kill();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_lists_renames_and_deletes() {
        let dir = std::env::temp_dir().join(format!("mcrs-worlds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = NewWorld { name: "My: World".into(), mode: 1, ..Default::default() };
        let folder = create(&dir, &w, "Player").unwrap();
        assert_eq!(folder, "My_ World");
        assert_eq!(folder_for(&dir, "My: World"), "My_ World (1)");
        let listed = list(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].game_mode, 1);
        assert!(listed[0].cheats);
        let renamed = rename(&dir, &folder, "Other").unwrap();
        assert_eq!(list(&dir)[0].folder, renamed);
        delete(&dir, &renamed).unwrap();
        assert!(list(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
