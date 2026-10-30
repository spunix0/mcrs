//! The multiplayer server list: vanilla's `servers.dat` (uncompressed NBT) and background status pings.

use mc_nbt::Tag;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, PartialEq, Debug)]
pub struct Server {
    pub name: String,
    pub ip: String,
}

#[derive(Clone, Debug)]
pub enum Ping {
    Pinging,
    Ok {
        /// MOTD with `§` formatting codes.
        motd: String,
        online: i64,
        max: i64,
        version: String,
        protocol: i64,
        ms: u32,
    },
    Failed(String),
}

pub struct ServerList {
    path: PathBuf,
    pub servers: Vec<Server>,
    pub pings: Arc<Mutex<HashMap<String, Ping>>>,
}

impl ServerList {
    pub fn load(game_dir: &Path) -> Self {
        let path = game_dir.join("servers.dat");
        let mut servers = Vec::new();
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok((_, root)) = mc_nbt::read_named(&mut bytes.as_slice()) {
                for s in root.get("servers").and_then(Tag::as_list).unwrap_or(&[]) {
                    let get = |k: &str| s.get(k).and_then(Tag::as_str).unwrap_or_default().to_string();
                    servers.push(Server { name: get("name"), ip: get("ip") });
                }
            }
        }
        Self { path, servers, pings: Arc::default() }
    }

    pub fn save(&self) {
        let list = self
            .servers
            .iter()
            .map(|s| Tag::Compound(vec![("name".into(), Tag::String(s.name.clone())), ("ip".into(), Tag::String(s.ip.clone()))]))
            .collect();
        let mut out = Vec::new();
        mc_nbt::write_named(&mut out, "", &Tag::Compound(vec![("servers".into(), Tag::List(list))]));
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, out);
    }

    /// Pings every server again in the background.
    pub fn refresh(&self) {
        for s in &self.servers {
            ping_async(&s.ip, self.pings.clone());
        }
    }

    pub fn ping_of(&self, ip: &str) -> Option<Ping> {
        self.pings.lock().unwrap().get(ip).cloned()
    }
}

fn ping_async(ip: &str, pings: Arc<Mutex<HashMap<String, Ping>>>) {
    let ip = ip.to_string();
    pings.lock().unwrap().insert(ip.clone(), Ping::Pinging);
    std::thread::spawn(move || {
        let result = match mc_protocol::status::ping(&ip, Duration::from_secs(5)) {
            Ok(s) => parse_status(&s.json, s.ping.as_millis() as u32),
            Err(e) => Ping::Failed(match e.kind() {
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "Timed out".into(),
                std::io::ErrorKind::ConnectionRefused => "Can't connect to server".into(),
                _ => "Can't resolve hostname".into(),
            }),
        };
        pings.lock().unwrap().insert(ip, result);
    });
}

fn parse_status(json: &str, ms: u32) -> Ping {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return Ping::Failed("Bad response".into()) };
    Ping::Ok {
        motd: legacy_text(&v["description"]),
        online: v["players"]["online"].as_i64().unwrap_or(0),
        max: v["players"]["max"].as_i64().unwrap_or(0),
        version: v["version"]["name"].as_str().unwrap_or("?").to_string(),
        protocol: v["version"]["protocol"].as_i64().unwrap_or(0),
        ms,
    }
}

/// A JSON text component as a string with `§` color codes (the form the text renderer understands).
pub fn legacy_text(v: &Value) -> String {
    const COLORS: [(&str, char); 16] = [
        ("black", '0'), ("dark_blue", '1'), ("dark_green", '2'), ("dark_aqua", '3'), ("dark_red", '4'), ("dark_purple", '5'),
        ("gold", '6'), ("gray", '7'), ("dark_gray", '8'), ("blue", '9'), ("green", 'a'), ("aqua", 'b'), ("red", 'c'),
        ("light_purple", 'd'), ("yellow", 'e'), ("white", 'f'),
    ];
    fn walk(v: &Value, color: Option<char>, out: &mut String) {
        match v {
            Value::String(s) => {
                if let Some(c) = color {
                    out.push('\u{a7}');
                    out.push(c);
                }
                out.push_str(s);
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, color, out)),
            Value::Object(o) => {
                let color = o.get("color").and_then(Value::as_str).and_then(|c| COLORS.iter().find(|(n, _)| *n == c)).map(|(_, k)| *k).or(color);
                if let Some(t) = o.get("text").and_then(Value::as_str) {
                    walk(&Value::String(t.to_string()), color, out);
                }
                if let Some(e) = o.get("extra") {
                    walk(e, color, out);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    walk(v, None, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_loads_servers_dat() {
        let dir = std::env::temp_dir().join(format!("mcrs-servers-{}", std::process::id()));
        let mut list = ServerList::load(&dir);
        list.servers.push(Server { name: "Home".into(), ip: "192.168.1.5:25565".into() });
        list.save();
        let back = ServerList::load(&dir);
        assert_eq!(back.servers, list.servers);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn motd_components_become_color_codes() {
        let v: Value = serde_json::from_str(r#"{"text":"A ","extra":[{"text":"B","color":"red"}]}"#).unwrap();
        assert_eq!(legacy_text(&v), "A \u{a7}cB");
        assert_eq!(legacy_text(&Value::String("plain".into())), "plain");
    }
}
