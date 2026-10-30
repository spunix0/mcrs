//! Starting the Rust client. Its title screen has singleplayer (worlds run on a hidden vanilla server, which is how
//! vanilla singleplayer works too, inside the Java client), multiplayer and options; the launcher only makes sure
//! the world engine (server.jar) and the block data it needs are installed, then opens it.

use crate::install::{self, Paths, Prepared, Progress};
use anyhow::{Context, Result, bail};
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

/// Path of the Rust client: next to the launcher executable.
pub fn client_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.parent()?.join("mc-client.exe");
    exe.exists().then_some(exe)
}

/// Installs the world engine if needed and opens the Rust client on its title screen (or straight into `world`).
pub fn launch_client(paths: &Paths, prepared: &Prepared, name: &str, eula_accepted: bool, world: Option<&str>, progress: &Arc<Progress>) -> Result<Child> {
    let client = client_exe().context("mc-client.exe not found next to the launcher; build it with `cargo build -p mc-client --release`")?;
    let id = &prepared.version.id;
    let server = prepared.version.downloads.server.as_ref().context("this version has no server download")?;
    let server_jar = paths.version_dir(id).join("server.jar");
    progress.set_stage("World engine (vanilla server)");
    install::fetch(&server.url, &server_jar, &server.sha1, server.size, progress)?;
    ensure_reports(paths, &prepared.java, &server_jar, progress)?;
    fs::create_dir_all(paths.root.join("worlds"))?;

    progress.set_stage("Opening the Rust client");
    let logs = paths.game().join("logs");
    fs::create_dir_all(&logs)?;
    let log = fs::File::create(logs.join("mcrs-client.log"))?;
    let mut cmd = Command::new(client);
    cmd.args(["--name", name, "--data"]).arg(&paths.root).arg("--java").arg(&prepared.java).arg("--server-jar").arg(&server_jar);
    if eula_accepted {
        cmd.arg("--eula");
    }
    if let Some(w) = world {
        cmd.args(["--world", w]);
    }
    cmd.stdout(Stdio::from(log.try_clone()?)).stderr(Stdio::from(log)).spawn().context("starting the Rust client")
}
