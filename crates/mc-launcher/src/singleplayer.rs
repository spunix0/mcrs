//! One-click singleplayer with the Rust client: a hidden vanilla server runs the world on localhost (that's how
//! vanilla singleplayer works too, inside the Java client), and the Rust client connects to it.

use crate::install::{self, Paths, Prepared, Progress};
use crate::launch::Account;
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct Running {
    pub server: Child,
    pub server_input: Option<ChildStdin>,
    pub client: Child,
    pub world_dir: PathBuf,
}

/// The server's `--reports` data (block states, registries) the Rust client loads at startup.
fn ensure_reports(paths: &Paths, java: &Path, server_jar: &Path, progress: &Progress) -> Result<()> {
    let out = paths.root.join("reports");
    if out.join("blocks.json").exists() && out.join("registries.json").exists() {
        return Ok(());
    }
    progress.set_stage("Generating block data (first time only)");
    let work = paths.root.join("datagen");
    fs::create_dir_all(&work)?;
    let status = Command::new(java)
        .args(["-DbundlerMainClass=net.minecraft.data.Main", "-jar"])
        .arg(server_jar)
        .args(["--reports", "--output", "generated"])
        .current_dir(&work)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .context("running the data generator")?;
    if !status.success() {
        bail!("data generator failed ({status})");
    }
    fs::create_dir_all(&out)?;
    for f in ["blocks.json", "registries.json"] {
        fs::copy(work.join("generated/reports").join(f), out.join(f)).with_context(|| format!("copying {f}"))?;
    }
    let _ = fs::remove_dir_all(&work);
    Ok(())
}

fn dashed_uuid(simple: &str) -> String {
    let s = simple;
    format!("{}-{}-{}-{}-{}", &s[0..8], &s[8..12], &s[12..16], &s[16..20], &s[20..32])
}

/// Sets `key=value` lines in a properties file, keeping everything else.
fn set_properties(path: &Path, values: &[(&str, String)]) -> Result<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    for (key, value) in values {
        let line = format!("{key}={value}");
        match lines.iter_mut().find(|l| l.split_once('=').is_some_and(|(k, _)| k == *key)) {
            Some(l) => *l = line,
            None => lines.push(line),
        }
    }
    fs::write(path, lines.join("\n") + "\n")?;
    Ok(())
}

fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Path of the Rust client: next to the launcher executable.
pub fn client_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.parent()?.join("mc-client.exe");
    exe.exists().then_some(exe)
}

/// Installs what's missing, starts the world server, waits for it, then opens the Rust client.
pub fn start(paths: &Paths, prepared: &Prepared, name: &str, world: &str, eula_accepted: bool, progress: &Arc<Progress>) -> Result<Running> {
    if !eula_accepted {
        bail!("Accept the Minecraft EULA on the Play screen to run a singleplayer world.");
    }
    let client = client_exe().context("mc-client.exe not found next to the launcher; build it with `cargo build -p mc-client --release`")?;
    let id = &prepared.version.id;
    let server = prepared.version.downloads.server.as_ref().context("this version has no server download")?;
    let server_jar = paths.version_dir(id).join("server.jar");
    progress.set_stage("World engine (vanilla server)");
    install::fetch(&server.url, &server_jar, &server.sha1, server.size, progress)?;
    ensure_reports(paths, &prepared.java, &server_jar, progress)?;

    progress.set_stage("Starting the world");
    let world_dir = paths.root.join("worlds").join(world);
    fs::create_dir_all(&world_dir)?;
    // The user accepted the EULA in the launcher; the server needs it written down.
    fs::write(world_dir.join("eula.txt"), "eula=true\n")?;
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
            ("view-distance", "8".into()),
            ("motd", format!("{world} (mcrs singleplayer)")),
        ],
    )?;
    // Operator, so the client can switch to spectator for noclip flying.
    let uuid = dashed_uuid(&Account::offline(name).uuid);
    fs::write(world_dir.join("ops.json"), format!(r#"[{{"uuid":"{uuid}","name":"{name}","level":2,"bypassesPlayerLimit":false}}]"#))?;

    let log = world_dir.join("logs").join("latest.log");
    let _ = fs::remove_file(&log);
    let mut server = Command::new(&prepared.java)
        .args(["-Xmx2G", "-jar"])
        .arg(&server_jar)
        .arg("nogui")
        .current_dir(&world_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("starting the world server")?;
    let server_input = server.stdin.take();

    // A new world takes a while to generate; wait for the server's "Done (...)" line.
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if progress.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = server.kill();
            bail!("cancelled");
        }
        if let Some(status) = server.try_wait()? {
            bail!("the world server stopped ({status}); see {}", log.display());
        }
        if fs::read_to_string(&log).is_ok_and(|l| l.contains("Done (")) {
            break;
        }
        if Instant::now() > deadline {
            let _ = server.kill();
            bail!("the world server didn't start in time; see {}", log.display());
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    progress.set_stage("Opening the Rust client");
    let client_log = fs::File::create(paths.game().join("logs").join("mcrs-client.log")).or_else(|_| {
        fs::create_dir_all(paths.game().join("logs"))?;
        fs::File::create(paths.game().join("logs").join("mcrs-client.log"))
    })?;
    let client = Command::new(client)
        .args(["--connect", &format!("127.0.0.1:{port}"), "--name", name, "--data"])
        .arg(&paths.root)
        .stdout(Stdio::from(client_log.try_clone()?))
        .stderr(Stdio::from(client_log))
        .spawn()
        .context("starting the Rust client")?;
    Ok(Running { server, server_input, client, world_dir })
}

/// Saves and stops the world: vanilla's `stop` command, then a hard kill if it hangs.
pub fn stop(mut running: Running) {
    if let Some(mut input) = running.server_input.take() {
        let _ = input.write_all(b"stop\n");
        let _ = input.flush();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if running.server.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = running.server.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_uuid() {
        assert_eq!(dashed_uuid("dfd8ab4146ac3edb9cf4aa6cf4081d6a"), "dfd8ab41-46ac-3edb-9cf4-aa6cf4081d6a");
    }

    #[test]
    fn updates_properties_in_place() {
        let dir = std::env::temp_dir().join(format!("mcrs-props-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("server.properties");
        fs::write(&f, "motd=old\ndifficulty=easy\n").unwrap();
        set_properties(&f, &[("motd", "new".into()), ("server-port", "1234".into())]).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "motd=new\ndifficulty=easy\nserver-port=1234\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
