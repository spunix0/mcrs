//! Builds the Java command line for a prepared version and starts it.

use crate::install::{Paths, Prepared, library_jobs_paths};
use crate::meta::{Arg, OneOrMany, rules_allow};
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone)]
pub struct Account {
    pub name: String,
    pub uuid: String,
    pub token: String,
}

impl Account {
    /// Offline account with the same UUID the vanilla launcher derives for a name.
    pub fn offline(name: &str) -> Self {
        let digest = md5::compute(format!("OfflinePlayer:{name}"));
        let uuid = uuid::Builder::from_md5_bytes(digest.0).into_uuid();
        Self { name: name.to_string(), uuid: uuid.simple().to_string(), token: "0".into() }
    }
}

pub fn valid_name(name: &str) -> bool {
    (3..=16).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn expand(arg: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = arg.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("${{{k}}}"), v);
    }
    out
}

fn flatten(args: &[Arg]) -> Vec<String> {
    let mut out = Vec::new();
    for a in args {
        match a {
            Arg::Plain(s) => out.push(s.clone()),
            Arg::Ruled { rules, value } if rules_allow(&Some(rules.clone())) => match value {
                OneOrMany::One(s) => out.push(s.clone()),
                OneOrMany::Many(v) => out.extend(v.iter().cloned()),
            },
            Arg::Ruled { .. } => {}
        }
    }
    out
}

pub fn launch(paths: &Paths, prepared: &Prepared, account: &Account, memory_mb: u32, java_override: Option<&str>) -> Result<Child> {
    let v = &prepared.version;
    let game_dir = paths.game();
    let natives = paths.natives(&v.id);
    fs::create_dir_all(game_dir.join("logs"))?;
    fs::create_dir_all(&natives)?;

    let mut classpath: Vec<String> = library_jobs_paths(paths, v)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    classpath.push(paths.client_jar(&v.id).to_string_lossy().into_owned());

    let log_cfg = v
        .logging
        .as_ref()
        .and_then(|l| l.client.as_ref())
        .map(|c| (c.argument.clone(), paths.assets().join("log_configs").join(&c.file.id)));

    let vars: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", account.name.clone()),
        ("version_name", v.id.clone()),
        ("game_directory", game_dir.to_string_lossy().into_owned()),
        ("assets_root", paths.assets().to_string_lossy().into_owned()),
        ("assets_index_name", v.asset_index.id.clone()),
        ("auth_uuid", account.uuid.clone()),
        ("auth_access_token", account.token.clone()),
        ("clientid", String::new()),
        ("auth_xuid", String::new()),
        ("version_type", v.kind.clone()),
        ("natives_directory", natives.to_string_lossy().into_owned()),
        ("launcher_name", "mcrs".into()),
        ("launcher_version", env!("CARGO_PKG_VERSION").into()),
        ("classpath", classpath.join(";")),
    ]);

    let mut args: Vec<String> = vec![format!("-Xmx{memory_mb}M"), format!("-Xms{}M", (memory_mb / 4).max(512))];
    args.extend(flatten(&v.arguments.jvm).iter().map(|a| expand(a, &vars)));
    if let Some((template, path)) = log_cfg {
        args.push(template.replace("${path}", &path.to_string_lossy()));
    }
    args.push(v.main_class.clone());
    args.extend(flatten(&v.arguments.game).iter().map(|a| expand(a, &vars)));

    let java = java_override
        .filter(|s| !s.trim().is_empty())
        .map(Into::into)
        .unwrap_or_else(|| prepared.java.clone());
    let log = fs::File::create(game_dir.join("logs").join("mcrs-launch.log"))?;
    let child = Command::new(java)
        .args(args)
        .current_dir(&game_dir)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    Ok(child)
}
