//! Protocol session: handshake → login → configuration → play. Answers what the server needs to keep us connected
//! and reports everything else as [`Event`]s.

use anyhow::{Result, bail};
use mc_protocol::ids::{config, handshake, login, play};
use mc_protocol::{PROTOCOL_VERSION, Reader, Sender, Writer, text};
use mc_world::{Chunk, LightUpdate, Registries};
use std::time::{SystemTime, UNIX_EPOCH};

/// Overworld in 26.3 spans Y -64..320.
const MIN_SECTION: i32 = -4;
const DEFAULT_BLOCK_STATES: usize = 35723;

pub enum Event {
    LoggedIn(String),
    /// The server now expects movement packets.
    InWorld,
    Chunk(Chunk),
    Forget(i32, i32),
    /// Block changes as (world position, new state).
    Blocks(Vec<([i32; 3], u16)>),
    Light(i32, i32, LightUpdate),
    /// Server moved us: feet position and yaw/pitch.
    Teleport([f64; 3], [f32; 2]),
    Chat(String),
    Health(f32),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Login,
    Config,
    Play,
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

/// One client tick's movement. The server allows a single position per tick, and a tick ends with `client_tick_end`.
/// `flags`: bit 0 = on ground, bit 1 = horizontal collision.
pub fn send_tick(tx: &Sender, pos: [f64; 3], rot: [f32; 2], flags: u8) -> Result<()> {
    let [x, y, z] = pos;
    tx.send(play::c2s::MOVE_PLAYER_POS_ROT, &Writer::new().f64(x).f64(y).f64(z).f32(rot[0]).f32(rot[1]).u8(flags).buf)?;
    tx.send(play::c2s::CLIENT_TICK_END, &[])?;
    Ok(())
}

pub struct Options<'a> {
    pub addr: &'a str,
    pub name: &'a str,
    pub greet: bool,
    pub block_states: Option<usize>,
    /// Someone else (the render loop) sends movement every tick.
    pub drives_movement: bool,
    /// Chunk radius to ask the server for (it caps this at its own view-distance).
    pub view_distance: u8,
}

/// Runs until the server disconnects us; returns the disconnect reason.
pub fn run(opts: &Options, tx: Sender, mut rx: mc_protocol::Receiver, mut emit: impl FnMut(Event)) -> Result<String> {
    let (host, port) = opts.addr.rsplit_once(':').map_or((opts.addr, 25565), |(h, p)| (h, p.parse().unwrap_or(25565)));
    let name = opts.name;
    tx.send(handshake::INTENTION, &Writer::new().varint(PROTOCOL_VERSION).string(host).u16(port).varint(2).buf)?;
    tx.send(login::c2s::HELLO, &Writer::new().string(name).uuid(offline_uuid(name)).buf)?;

    let mut reg = Registries { block_states: opts.block_states.unwrap_or(DEFAULT_BLOCK_STATES), biomes: 64 };
    let mut state = State::Login;
    let mut pos = [0.0f64; 3];
    let mut rot = [0.0f32; 2];
    let mut loaded = false;
    let debug = std::env::var_os("MCRS_DEBUG").is_some();
    loop {
        let (id, body) = rx.recv()?;
        if debug {
            eprintln!("{state:?} packet {id} ({} bytes)", body.len());
        }
        let mut r = Reader::new(&body);
        match (state, id) {
            (State::Login, login::s2c::COMPRESSION) => rx.set_compression(r.varint()?),
            (State::Login, login::s2c::HELLO) => bail!("server is in online mode; only offline-mode servers work so far"),
            (State::Login, login::s2c::DISCONNECT) => bail!("kicked during login: {}", r.string()?),
            (State::Login, login::s2c::CUSTOM_QUERY) => {
                let transaction = r.varint()?;
                tx.send(login::c2s::CUSTOM_QUERY_ANSWER, &Writer::new().varint(transaction).bool(false).buf)?;
            }
            (State::Login, login::s2c::LOGIN_FINISHED) => {
                let _uuid = r.uuid()?;
                emit(Event::LoggedIn(r.string()?));
                tx.send(login::c2s::LOGIN_ACKNOWLEDGED, &[])?;
                state = State::Config;
                // ClientInformation: the server sends min(its view-distance, ours) chunks; the default is only 2.
                let mut w = Writer::new();
                w.string("en_us")
                    .u8(opts.view_distance)
                    .varint(0) // chat: full
                    .bool(true) // chat colors
                    .u8(0x7f) // all skin layers
                    .varint(1) // main hand: right
                    .bool(false) // text filtering
                    .bool(true) // show in server list
                    .varint(0); // particles: all
                tx.send(config::c2s::CLIENT_INFORMATION, &w.buf)?;
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
                let registry = r.string()?;
                let entries = r.len()?;
                if registry == "minecraft:worldgen/biome" {
                    reg.biomes = entries;
                }
            }
            (State::Config, config::s2c::KEEP_ALIVE) => tx.send(config::c2s::KEEP_ALIVE, &Writer::new().i64(r.i64()?).buf)?,
            (State::Config, config::s2c::PING) => tx.send(config::c2s::PONG, &Writer::new().i32(r.i32()?).buf)?,
            (State::Config, config::s2c::CODE_OF_CONDUCT) => {
                emit(Event::Chat(format!("server code of conduct: {}", r.string()?)));
                tx.send(config::c2s::ACCEPT_CODE_OF_CONDUCT, &[])?;
            }
            (State::Config, config::s2c::DISCONNECT) => bail!("kicked during configuration: {}", component(&mut r)),
            (State::Config, config::s2c::FINISH_CONFIGURATION) => {
                tx.send(config::c2s::FINISH_CONFIGURATION, &[])?;
                state = State::Play;
            }

            (State::Play, play::s2c::KEEP_ALIVE) => tx.send(play::c2s::KEEP_ALIVE, &Writer::new().i64(r.i64()?).buf)?,
            (State::Play, play::s2c::PING) => tx.send(play::c2s::PONG, &Writer::new().i32(r.i32()?).buf)?,
            (State::Play, play::s2c::PLAYER_POSITION) => {
                let teleport = r.varint()?;
                let p = [r.f64()?, r.f64()?, r.f64()?];
                let _delta = [r.f64()?, r.f64()?, r.f64()?];
                let rt = [r.f32()?, r.f32()?];
                let rel = r.i32()?;
                for i in 0..3 {
                    pos[i] = if rel & (1 << i) != 0 { pos[i] + p[i] } else { p[i] };
                }
                rot[0] = if rel & (1 << 3) != 0 { rot[0] + rt[0] } else { rt[0] };
                rot[1] = if rel & (1 << 4) != 0 { rot[1] + rt[1] } else { rt[1] };
                // Since 26.x the confirmation echoes the resulting position and rotation.
                let [x, y, z] = pos;
                tx.send(play::c2s::ACCEPT_TELEPORTATION, &Writer::new().varint(teleport).f64(x).f64(y).f64(z).f32(rot[0]).f32(rot[1]).buf)?;
                // With a window, the render loop owns movement packets; sending here too could put two in one tick.
                if !opts.drives_movement {
                    send_tick(&tx, pos, rot, 0)?;
                }
                emit(Event::Teleport(pos, rot));
            }
            (State::Play, play::s2c::LEVEL_CHUNK_WITH_LIGHT) => match mc_world::decode_chunk(&body, MIN_SECTION, reg) {
                Ok(chunk) => emit(Event::Chunk(chunk)),
                Err(e) => {
                    if debug {
                        let _ = std::fs::write("bad-chunk.bin", &body);
                    }
                    emit(Event::Chat(format!("couldn't decode a chunk: {e}")))
                }
            },
            (State::Play, play::s2c::BLOCK_UPDATE) => {
                let pos = mc_world::unpack_block_pos(r.i64()?);
                emit(Event::Blocks(vec![(pos, r.varint()? as u16)]));
            }
            (State::Play, play::s2c::SECTION_BLOCKS_UPDATE) => {
                let [sx, sy, sz] = mc_world::unpack_section_pos(r.i64()?);
                let n = r.len()?;
                let mut changes = Vec::with_capacity(n);
                for _ in 0..n {
                    // state << 12 | x << 8 | z << 4 | y
                    let v = r.varlong()?;
                    let (lx, lz, ly) = (((v >> 8) & 15) as i32, ((v >> 4) & 15) as i32, (v & 15) as i32);
                    changes.push(([sx * 16 + lx, sy * 16 + ly, sz * 16 + lz], (v >> 12) as u16));
                }
                emit(Event::Blocks(changes));
            }
            (State::Play, play::s2c::LIGHT_UPDATE) => match mc_world::decode_light_update(&body) {
                Ok((x, z, light)) => emit(Event::Light(x, z, light)),
                Err(e) => emit(Event::Chat(format!("couldn't decode a light update: {e}"))),
            },
            (State::Play, play::s2c::FORGET_LEVEL_CHUNK) => {
                let packed = r.i64()?;
                emit(Event::Forget(packed as i32, (packed >> 32) as i32));
            }
            (State::Play, play::s2c::CHUNK_BATCH_FINISHED) => {
                // Chunks per tick we can take; meshing runs on worker threads, so ask for plenty.
                tx.send(play::c2s::CHUNK_BATCH_RECEIVED, &Writer::new().f32(64.0).buf)?;
            }
            (State::Play, play::s2c::GAME_EVENT) => {
                if r.u8()? == 13 && !loaded {
                    loaded = true;
                    tx.send(play::c2s::PLAYER_LOADED, &[])?;
                    emit(Event::InWorld);
                }
            }
            (State::Play, play::s2c::SET_HEALTH) => {
                let health = r.f32()?;
                if health <= 0.0 {
                    // Dead players get no chunks until they respawn; press "Respawn" right away.
                    tx.send(play::c2s::CLIENT_COMMAND, &Writer::new().varint(0).buf)?;
                }
                emit(Event::Health(health));
            }
            (State::Play, play::s2c::SYSTEM_CHAT) => {
                let msg = component(&mut r);
                if opts.greet {
                    if let Some(who) = msg.strip_prefix("multiplayer.player.joined [").and_then(|s| s.strip_suffix(']')) {
                        if who != name {
                            send_chat(&tx, &format!("Hi {who}! I'm {name}, a Minecraft client written in Rust."))?;
                        }
                    }
                }
                emit(Event::Chat(msg));
            }
            (State::Play, play::s2c::DISCONNECT) => return Ok(component(&mut r)),
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
