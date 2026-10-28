//! Headless protocol session: handshake → login → configuration → play, answering what the server needs to keep us connected.

use anyhow::{Result, bail};
use mc_protocol::ids::{config, handshake, login, play};
use mc_protocol::{PROTOCOL_VERSION, Reader, Sender, Writer, text};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
pub struct Stats {
    pub in_world: AtomicBool,
    pub chunks: AtomicU32,
    pub registries: AtomicU32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Login,
    Config,
    Play,
}

#[derive(Default)]
struct Player {
    pos: [f64; 3],
    rot: [f32; 2],
    loaded: bool,
}

/// UUID the vanilla server assigns an offline player with this name.
pub fn offline_uuid(name: &str) -> u128 {
    let mut b = md5::compute(format!("OfflinePlayer:{name}")).0;
    b[6] = (b[6] & 0x0f) | 0x30;
    b[8] = (b[8] & 0x3f) | 0x80;
    u128::from_be_bytes(b)
}

pub fn send_chat(tx: &Sender, line: &str) -> Result<()> {
    if let Some(cmd) = line.strip_prefix('/') {
        tx.send(play::c2s::CHAT_COMMAND, &Writer::new().string(cmd).buf)?;
        return Ok(());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    let mut w = Writer::new();
    w.string(line)
        .i64(now)
        .i64(0) // salt
        .bool(false) // no signature: offline profiles can't sign
        .varint(0) // last-seen offset
        .bytes(&[0, 0, 0]) // acknowledged bitset (20 bits)
        .u8(0); // checksum 0 = ignore
    tx.send(play::c2s::CHAT, &w.buf)?;
    Ok(())
}

pub fn run(addr: &str, name: &str, greet: bool, stats: Arc<Stats>, tx: Sender, mut rx: mc_protocol::Receiver) -> Result<()> {
    let (host, port) = addr.rsplit_once(':').map_or((addr, 25565), |(h, p)| (h, p.parse().unwrap_or(25565)));
    tx.send(handshake::INTENTION, &Writer::new().varint(PROTOCOL_VERSION).string(host).u16(port).varint(2).buf)?;
    tx.send(login::c2s::HELLO, &Writer::new().string(name).uuid(offline_uuid(name)).buf)?;

    let mut state = State::Login;
    let mut player = Player::default();
    loop {
        let (id, body) = rx.recv()?;
        let mut r = Reader::new(&body);
        match (state, id) {
            (State::Login, login::s2c::COMPRESSION) => {
                let threshold = r.varint()?;
                rx.set_compression(threshold);
                println!("compression threshold {threshold}");
            }
            (State::Login, login::s2c::HELLO) => bail!("server is in online mode; only offline-mode servers work so far"),
            (State::Login, login::s2c::DISCONNECT) => bail!("kicked during login: {}", r.string()?),
            (State::Login, login::s2c::CUSTOM_QUERY) => {
                let transaction = r.varint()?;
                tx.send(login::c2s::CUSTOM_QUERY_ANSWER, &Writer::new().varint(transaction).bool(false).buf)?;
            }
            (State::Login, login::s2c::LOGIN_FINISHED) => {
                let uuid = r.uuid()?;
                let name = r.string()?;
                println!("logged in as {name} ({uuid:032x})");
                tx.send(login::c2s::LOGIN_ACKNOWLEDGED, &[])?;
                state = State::Config;
            }

            (State::Config, config::s2c::SELECT_KNOWN_PACKS) => {
                // Echo the server's list back: we have the vanilla data, so it can skip sending it.
                let n = r.len()?;
                let mut w = Writer::new();
                w.varint(n as i32);
                for _ in 0..n {
                    let (ns, id, ver) = (r.string()?, r.string()?, r.string()?);
                    w.string(&ns).string(&id).string(&ver);
                }
                tx.send(config::c2s::SELECT_KNOWN_PACKS, &w.buf)?;
            }
            (State::Config, config::s2c::REGISTRY_DATA) => {
                stats.registries.fetch_add(1, Relaxed);
            }
            (State::Config, config::s2c::KEEP_ALIVE) => tx.send(config::c2s::KEEP_ALIVE, &Writer::new().i64(r.i64()?).buf)?,
            (State::Config, config::s2c::PING) => tx.send(config::c2s::PONG, &Writer::new().i32(r.i32()?).buf)?,
            (State::Config, config::s2c::CODE_OF_CONDUCT) => {
                println!("server code of conduct:\n{}", r.string()?);
                tx.send(config::c2s::ACCEPT_CODE_OF_CONDUCT, &[])?;
            }
            (State::Config, config::s2c::DISCONNECT) => bail!("kicked during configuration: {}", component(&mut r)),
            (State::Config, config::s2c::FINISH_CONFIGURATION) => {
                println!("configuration done ({} registries)", stats.registries.load(Relaxed));
                tx.send(config::c2s::FINISH_CONFIGURATION, &[])?;
                state = State::Play;
            }

            (State::Play, play::s2c::LOGIN) => println!("entered world (entity id {})", r.i32()?),
            (State::Play, play::s2c::KEEP_ALIVE) => tx.send(play::c2s::KEEP_ALIVE, &Writer::new().i64(r.i64()?).buf)?,
            (State::Play, play::s2c::PING) => tx.send(play::c2s::PONG, &Writer::new().i32(r.i32()?).buf)?,
            (State::Play, play::s2c::PLAYER_POSITION) => {
                let teleport = r.varint()?;
                let pos = [r.f64()?, r.f64()?, r.f64()?];
                let _delta = [r.f64()?, r.f64()?, r.f64()?];
                let rot = [r.f32()?, r.f32()?];
                let rel = r.i32()?;
                for i in 0..3 {
                    player.pos[i] = if rel & (1 << i) != 0 { player.pos[i] + pos[i] } else { pos[i] };
                }
                player.rot[0] = if rel & (1 << 3) != 0 { player.rot[0] + rot[0] } else { rot[0] };
                player.rot[1] = if rel & (1 << 4) != 0 { player.rot[1] + rot[1] } else { rot[1] };
                let [x, y, z] = player.pos;
                let [yaw, pitch] = player.rot;
                // Since 26.x the confirmation echoes the resulting position and rotation.
                tx.send(
                    play::c2s::ACCEPT_TELEPORTATION,
                    &Writer::new().varint(teleport).f64(x).f64(y).f64(z).f32(yaw).f32(pitch).buf,
                )?;
                tx.send(
                    play::c2s::MOVE_PLAYER_POS_ROT,
                    &Writer::new().f64(x).f64(y).f64(z).f32(player.rot[0]).f32(player.rot[1]).u8(0).buf,
                )?;
                println!("position {x:.2} {y:.2} {z:.2}");
            }
            (State::Play, play::s2c::LEVEL_CHUNK_WITH_LIGHT) => {
                stats.chunks.fetch_add(1, Relaxed);
            }
            (State::Play, play::s2c::CHUNK_BATCH_FINISHED) => {
                tx.send(play::c2s::CHUNK_BATCH_RECEIVED, &Writer::new().f32(25.0).buf)?;
            }
            (State::Play, play::s2c::GAME_EVENT) => {
                if r.u8()? == 13 && !player.loaded {
                    player.loaded = true;
                    tx.send(play::c2s::PLAYER_LOADED, &[])?;
                    stats.in_world.store(true, Relaxed);
                    println!("world loading, told server the player is loaded");
                }
            }
            (State::Play, play::s2c::SET_HEALTH) => println!("health {:.1}", r.f32()?),
            (State::Play, play::s2c::SYSTEM_CHAT) => {
                let msg = component(&mut r);
                println!("[chat] {msg}");
                if greet {
                    if let Some(who) = msg.strip_prefix("multiplayer.player.joined [").and_then(|s| s.strip_suffix(']')) {
                        if who != name {
                            send_chat(&tx, &format!("Hi {who}! I'm {name}, a Minecraft client written in Rust."))?;
                        }
                    }
                }
            }
            (State::Play, play::s2c::DISCONNECT) => {
                println!("disconnected by server: {}", component(&mut r));
                return Ok(());
            }
            (State::Play, play::s2c::START_CONFIGURATION) => {
                tx.send(play::c2s::CONFIGURATION_ACKNOWLEDGED, &[])?;
                state = State::Config;
            }
            _ => {}
        }
    }
}

fn component(r: &mut Reader) -> String {
    match r.nbt() {
        Ok(Some(tag)) => text::plain(&tag),
        Ok(None) => String::new(),
        Err(e) => format!("<unreadable text: {e}>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_java() {
        // UUID.nameUUIDFromBytes("OfflinePlayer:Notch")
        assert_eq!(format!("{:032x}", offline_uuid("Notch")), "b50ad385829d3141a2167e7d7539ba7f");
    }
}
