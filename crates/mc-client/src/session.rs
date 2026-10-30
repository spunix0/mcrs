//! Protocol session: handshake → login → configuration → play. Answers what the server needs to keep us connected
//! and reports everything else as [`Event`]s.

use anyhow::{Result, bail};
use mc_protocol::ids::{config, handshake, login, play};
use mc_protocol::{PROTOCOL_VERSION, Reader, Sender, Writer, text};
use crate::inventory::{Components, Stack, read_stack};
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
    /// Full player inventory (container 0, menu slot order) with its state ID.
    Inventory(i32, Vec<Option<Stack>>),
    /// One menu slot of container 0.
    InventorySlot(i32, usize, Option<Stack>),
    /// Contents of an open container (not the inventory).
    Container { id: i32, state: i32, items: Vec<Option<Stack>> },
    ContainerSlot { id: i32, state: i32, slot: usize, item: Option<Stack> },
    /// A container screen opened: menu type (protocol ID) and title.
    OpenScreen { id: i32, kind: u32, title: mc_nbt::Tag },
    CloseScreen,
    /// A container property (furnace fuel and progress).
    ContainerData { id: i32, prop: i16, value: i16 },
    /// The item held on the mouse cursor in a container screen.
    Cursor(Option<Stack>),
    /// We joined: our entity ID, hardcore, game mode, dimension name, whether to show the death screen.
    Joined { entity: i32, hardcore: bool, game_mode: u8, dimension: String, death_screen: bool },
    /// Respawned or changed dimension: chunks must be dropped.
    Respawned { game_mode: u8, dimension: String },
    GameMode(u8),
    Experience { progress: f32, level: i32, total: i32 },
    /// World age and the day clock (ticks, rate) if one was sent.
    Time { day: Option<(i64, f32)> },
    EntityData { id: i32, data: Vec<crate::entities::Data>, air: Option<i32> },
    /// An entity was hurt (red flash; camera shake if it's us).
    Hurt(i32),
    /// An entity swung its arm.
    Swing(i32),
    /// We died, with the death message.
    Died(mc_nbt::Tag),
    Title(mc_nbt::Tag),
    Subtitle(mc_nbt::Tag),
    ActionBar(mc_nbt::Tag),
    /// Title fade in, stay, fade out (ticks).
    TitleTimes(i32, i32, i32),
    ClearTitles,
    /// Players added to the tab list (UUID, name), and latency updates.
    PlayersAdded(Vec<(u128, String)>),
    PlayerLatency(Vec<(u128, i32)>),
    PlayersRemoved(Vec<u128>),
    TabList(mc_nbt::Tag, mc_nbt::Tag),
    /// Enchantment registry names in ID order (to find Efficiency).
    Enchantments(Vec<String>),
    Difficulty(u8),
    /// Rain strength 0..1.
    Rain(f32),
    /// One slot in the inventory's own order (`set_player_inventory`).
    PlayerSlot(usize, Option<Stack>),
    HeldSlot(u8),
    /// A sound from the server; `at` is `None` for sounds attached to entities we don't track yet.
    Sound { event: SoundRef, at: Option<[f64; 3]>, entity: Option<i32>, volume: f32, pitch: f32, category: usize },
    /// World events such as 2001 (block broken by someone else, `data` = block state).
    LevelEvent { kind: i32, pos: [i32; 3], data: i32 },
    EntityAdd { id: i32, kind: u32, pos: [f64; 3], yaw: f32, pitch: f32, head_yaw: f32, data: i32 },
    /// Relative moves in 1/4096 blocks; several steps since 26.x.
    EntityMove { id: i32, steps: Vec<[i16; 3]>, rot: Option<[f32; 2]> },
    EntityRot { id: i32, rot: [f32; 2] },
    /// Absolute position (sync or teleport).
    EntityPos { id: i32, pos: [f64; 3], rot: [f32; 2] },
    EntityHead { id: i32, yaw: f32 },
    EntityRemove(Vec<i32>),
    /// Server moved us: feet position and yaw/pitch.
    Teleport([f64; 3], [f32; 2], i32),
    Chat(String),
    Health { health: f32, food: i32 },
    /// A chat line as a text component (translated by the client).
    ChatTag(mc_nbt::Tag),
}

/// A sound event either by registry ID or spelled out (custom sounds).
pub enum SoundRef {
    Id(u32),
    Name(String),
}

/// `Holder<SoundEvent>`: 0 means an inline event (name + optional fixed range), otherwise registry ID + 1.
fn sound_ref(r: &mut Reader) -> Result<SoundRef> {
    let h = r.varint()?;
    if h == 0 {
        let name = r.string()?;
        if r.bool()? {
            r.f32()?;
        }
        Ok(SoundRef::Name(name))
    } else {
        Ok(SoundRef::Id(h as u32 - 1))
    }
}

/// `<name> message`, as vanilla's `chat.type.text` renders it.
fn chat_line(name: mc_nbt::Tag, message: mc_nbt::Tag) -> mc_nbt::Tag {
    use mc_nbt::Tag;
    Tag::Compound(vec![("translate".into(), Tag::String("chat.type.text".into())), ("with".into(), Tag::List(vec![name, message]))])
}

/// `ChatType.Bound`: chat type (registry ID + 1; inline types aren't supported), sender name, optional target.
fn chat_bound(r: &mut Reader) -> Result<Option<(mc_nbt::Tag, Option<mc_nbt::Tag>)>> {
    if r.varint()? == 0 {
        return Ok(None);
    }
    let name = r.nbt()?.unwrap_or(mc_nbt::Tag::String(String::new()));
    let target = if r.bool()? { r.nbt()? } else { None };
    Ok(Some((name, target)))
}

/// `ClientboundPlayerChatPacket` -> a `<name> message` line. Signatures are read past, not checked.
fn player_chat(r: &mut Reader) -> Result<Option<mc_nbt::Tag>> {
    let _global_index = r.varint()?;
    let _sender = r.uuid()?;
    let _index = r.varint()?;
    if r.bool()? {
        r.bytes(256)?;
    }
    let content = r.string()?;
    let (_timestamp, _salt) = (r.i64()?, r.i64()?);
    for _ in 0..r.len()? {
        // Last-seen entries: 0 = full signature follows, otherwise a cache index + 1.
        if r.varint()? == 0 {
            r.bytes(256)?;
        }
    }
    let unsigned = if r.bool()? { r.nbt()? } else { None };
    // Filter mask: 0 pass-through, 1 fully filtered, 2 partial (bitset follows).
    let filter = r.varint()?;
    if filter == 2 {
        let n = r.len()?;
        r.bytes(n)?;
    }
    if filter == 1 {
        return Ok(None);
    }
    let message = unsigned.unwrap_or(mc_nbt::Tag::String(content));
    Ok(chat_bound(r)?.map(|(name, _)| chat_line(name, message)))
}

/// Angles are sent as 1/256 of a turn.
fn angle(b: u8) -> f32 {
    b as i8 as f32 * 360.0 / 256.0
}

/// `LpVec3`: a compact velocity. Only skipped here; entities don't predict motion yet.
fn skip_lp_vec3(r: &mut Reader) -> Result<()> {
    let lowest = r.u8()?;
    if lowest == 0 {
        return Ok(());
    }
    r.u8()?;
    r.i32()?;
    if lowest & 4 != 0 {
        r.varint()?;
    }
    Ok(())
}

/// `VecDelta`: one (dx, dy, dz) in shorts, or `steps` of (ticks, dx, dy, dz).
fn vec_delta(r: &mut Reader, steps: usize) -> Result<Vec<[i16; 3]>> {
    if steps == 0 {
        return Ok(vec![[r.i16()?, r.i16()?, r.i16()?]]);
    }
    (0..steps)
        .map(|_| {
            r.varint()?;
            Ok([r.i16()?, r.i16()?, r.i16()?])
        })
        .collect()
}

fn vec3(r: &mut Reader) -> Result<[f64; 3]> {
    Ok([r.f64()?, r.f64()?, r.f64()?])
}

/// `SynchedEntityData` entries: index, serializer ID, value; 0xff ends the list. Only the entries we use are kept.
/// Serializers we can't skip (particles, profiles) end the read early, which only loses later indices.
fn entity_data(r: &mut Reader, comps: Option<&Components>) -> Result<(Vec<crate::entities::Data>, Option<i32>)> {
    use crate::entities::Data;
    let mut out = Vec::new();
    let mut air = None;
    loop {
        let index = r.u8()?;
        if index == 0xff {
            break;
        }
        match r.varint()? {
            0 => {
                let v = r.u8()?;
                // Sheep wool: color in the low bits, 0x10 = sheared (index 18 after the ageable fields).
                match index {
                    0 => out.push(Data::Flags(v)),
                    18 => out.push(Data::Wool(v)),
                    _ => {}
                }
            }
            1 => {
                let v = r.varint()?;
                match index {
                    1 => air = Some(v),
                    16 => out.push(Data::Size(v)),
                    _ => {}
                }
            }
            2 => {
                r.varlong()?;
            }
            3 => {
                r.f32()?;
            }
            4 => {
                r.string()?;
            }
            5 => {
                r.nbt()?;
            }
            6 => {
                if r.bool()? {
                    r.nbt()?;
                }
            }
            7 => {
                let Some(c) = comps else { break };
                match read_stack(r, c) {
                    Ok(s) => {
                        if index == 8 {
                            out.push(Data::Item(s));
                        }
                    }
                    Err((s, _)) => {
                        if index == 8 {
                            out.push(Data::Item(s));
                        }
                        break;
                    }
                }
            }
            8 => {
                let b = r.bool()?;
                if index == 16 {
                    out.push(Data::Baby(b));
                }
            }
            9 | 39 => {
                r.bytes(12)?;
            }
            10 => {
                r.i64()?;
            }
            11 => {
                if r.bool()? {
                    r.i64()?;
                }
            }
            13 => {
                if r.bool()? {
                    r.uuid()?;
                }
            }
            12 | 14 | 15 | 19 | 20 | 21..=32 | 35..=38 | 42 | 43 => {
                r.varint()?;
            }
            18 => {
                r.varint()?;
                r.varint()?;
                r.varint()?;
            }
            33 => {
                if r.bool()? {
                    r.string()?;
                    r.i64()?;
                }
            }
            40 => {
                r.bytes(16)?;
            }
            _ => break,
        }
    }
    Ok((out, air))
}

/// `CommonPlayerSpawnInfo` -> (game mode, dimension name).
fn spawn_info(r: &mut Reader) -> Result<(u8, String)> {
    let _dimension_type = r.varint()?;
    let dimension = r.string()?;
    let _seed = r.i64()?;
    let mode = r.varint()? as u8;
    Ok((mode, dimension))
}

/// `ClientboundPlayerInfoUpdatePacket`: a 1-byte action set, then per entry the fields of each action in order.
fn player_info(r: &mut Reader, emit: &mut impl FnMut(Event)) -> Result<()> {
    let actions = r.u8()?;
    let (mut added, mut latency) = (Vec::new(), Vec::new());
    for _ in 0..r.len()? {
        let uuid = r.uuid()?;
        for bit in 0..8 {
            if actions & (1 << bit) == 0 {
                continue;
            }
            match bit {
                0 => {
                    let name = r.string()?;
                    for _ in 0..r.len()? {
                        r.string()?;
                        r.string()?;
                        if r.bool()? {
                            r.string()?;
                        }
                    }
                    added.push((uuid, name));
                }
                1 => {
                    if r.bool()? {
                        r.uuid()?;
                        r.i64()?;
                        let n = r.len()?;
                        r.bytes(n)?;
                        let n = r.len()?;
                        r.bytes(n)?;
                    }
                }
                2 | 6 => {
                    r.varint()?;
                }
                3 | 7 => {
                    r.bool()?;
                }
                4 => latency.push((uuid, r.varint()?)),
                _ => {
                    if r.bool()? {
                        r.nbt()?;
                    }
                }
            }
        }
    }
    if !added.is_empty() {
        emit(Event::PlayersAdded(added));
    }
    if !latency.is_empty() {
        emit(Event::PlayerLatency(latency));
    }
    Ok(())
}

/// Confirms a server teleport; since 26.x it echoes the resulting position and rotation.
pub fn send_accept_teleport(tx: &Sender, id: i32, pos: [f64; 3], rot: [f32; 2]) -> Result<()> {
    let [x, y, z] = pos;
    tx.send(play::c2s::ACCEPT_TELEPORTATION, &Writer::new().varint(id).f64(x).f64(y).f64(z).f32(rot[0]).f32(rot[1]).buf)?;
    Ok(())
}

/// Creative mode: sets an inventory menu slot (or drops the stack with slot -1). Damage and enchantments are
/// written back so moving an item doesn't strip them; other components are dropped.
pub fn send_creative_slot(tx: &Sender, comps: &std::collections::HashMap<String, u32>, slot: i16, stack: Option<&Stack>) -> Result<()> {
    let mut w = Writer::new();
    w.u16(slot as u16);
    match stack {
        None => {
            w.varint(0);
        }
        Some(s) => {
            w.varint(s.count as i32).varint(s.item as i32);
            let damage = s.damage.and_then(|d| Some((*comps.get("damage")?, d)));
            let ench = (!s.enchantments.is_empty()).then(|| comps.get("enchantments").copied()).flatten();
            w.varint(damage.is_some() as i32 + ench.is_some() as i32).varint(0);
            if let Some((id, d)) = damage {
                w.varint(id as i32).varint(d as i32);
            }
            if let Some(id) = ench {
                w.varint(id as i32).varint(s.enchantments.len() as i32);
                for (e, level) in &s.enchantments {
                    w.varint(*e as i32).varint(*level as i32);
                }
            }
        }
    }
    tx.send(play::c2s::SET_CREATIVE_MODE_SLOT, &w.buf)?;
    Ok(())
}

pub fn send_respawn(tx: &Sender) -> Result<()> {
    tx.send(play::c2s::CLIENT_COMMAND, &Writer::new().varint(0).buf)?;
    Ok(())
}

/// The arm swing other players see, and vanilla's "punch" for swings that hit nothing.
pub fn send_punch(tx: &Sender) -> Result<()> {
    tx.send(play::c2s::PUNCH, &[])?;
    Ok(())
}

/// Sprint start/stop (`player_command` actions 1 and 2): the server needs it for sprint-jumping and knockback.
pub fn send_sprint(tx: &Sender, entity: i32, start: bool) -> Result<()> {
    tx.send(play::c2s::PLAYER_COMMAND, &Writer::new().varint(entity).varint(if start { 1 } else { 2 }).varint(0).buf)?;
    Ok(())
}

/// Drops the held item: `player_action` 5 (one) or 4 (whole stack); 26.x put CHANGE_DESTROY_DIRECTION at 1.
pub fn send_drop(tx: &Sender, whole_stack: bool) -> Result<()> {
    let action = if whole_stack { 4 } else { 5 };
    tx.send(play::c2s::PLAYER_ACTION, &Writer::new().varint(action).i64(0).u8(0).varint(0).buf)?;
    Ok(())
}

/// Middle click in creative: the server puts that block in our hand.
pub fn send_pick_block(tx: &Sender, pos: [i32; 3]) -> Result<()> {
    tx.send(play::c2s::PICK_ITEM_FROM_BLOCK, &Writer::new().i64(pack_block_pos(pos)).bool(false).buf)?;
    Ok(())
}

/// View distance and the other client settings, sent again when the options change.
pub fn send_client_information(tx: &Sender, view_distance: u8) -> Result<()> {
    tx.send(play::c2s::CLIENT_INFORMATION, &client_information(view_distance).buf)?;
    Ok(())
}

fn client_information(view_distance: u8) -> Writer {
    let mut w = Writer::new();
    w.string("en_us")
        .u8(view_distance)
        .varint(0) // chat: full
        .bool(true) // chat colors
        .u8(0x7f) // all skin layers
        .varint(1) // main hand: right
        .bool(false) // text filtering
        .bool(true) // show in server list
        .varint(0); // particles: all
    w
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
fn pack_block_pos([x, y, z]: [i32; 3]) -> i64 {
    ((x as i64 & 0x3ff_ffff) << 38) | ((z as i64 & 0x3ff_ffff) << 12) | (y as i64 & 0xfff)
}

/// Mining actions of `player_action`.
#[derive(Clone, Copy)]
pub enum Dig {
    Start = 0,
    Abort = 2,
    Stop = 3,
}

/// `sequence` lets the server acknowledge the change so the client could roll back predictions.
pub fn send_dig(tx: &Sender, action: Dig, pos: [i32; 3], face: u8, sequence: i32) -> Result<()> {
    tx.send(play::c2s::PLAYER_ACTION, &Writer::new().varint(action as i32).i64(pack_block_pos(pos)).u8(face).varint(sequence).buf)?;
    Ok(())
}

/// Right-click on a block face with the main hand (place a block, open a door, ...).
pub fn send_use_on(tx: &Sender, pos: [i32; 3], face: u8, point: [f64; 3], sequence: i32) -> Result<()> {
    let mut w = Writer::new();
    w.varint(0) // main hand
        .i64(pack_block_pos(pos))
        .varint(face as i32)
        .f32((point[0] - pos[0] as f64) as f32)
        .f32((point[1] - pos[1] as f64) as f32)
        .f32((point[2] - pos[2] as f64) as f32)
        .bool(false) // inside the block
        .bool(false) // world border
        .varint(sequence);
    tx.send(play::c2s::USE_ITEM_ON, &w.buf)?;
    Ok(())
}

/// Right-click in the air with the main hand (eat, throw, ...).
pub fn send_use(tx: &Sender, sequence: i32, rot: [f32; 2]) -> Result<()> {
    tx.send(play::c2s::USE_ITEM, &Writer::new().varint(0).varint(sequence).f32(rot[0]).f32(rot[1]).buf)?;
    Ok(())
}

pub fn send_hotbar_slot(tx: &Sender, slot: u8) -> Result<()> {
    tx.send(play::c2s::SET_CARRIED_ITEM, &Writer::new().u16(slot as u16).buf)?;
    Ok(())
}

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
    /// Needed to read item stacks; without it inventory packets are ignored.
    pub components: Option<&'a Components>,
}

/// Reads one stack; an unknown component keeps the stack but means the rest of the packet can't be trusted.
fn stack(r: &mut Reader, comps: &Components, ok: &mut bool) -> Option<Stack> {
    if !*ok {
        return None;
    }
    match read_stack(r, comps) {
        Ok(s) => s,
        Err((s, e)) => {
            eprintln!("item with unsupported component {}; skipping the rest of this inventory packet", e.0);
            *ok = false;
            s
        }
    }
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
    let mut min_section = MIN_SECTION;
    let mut overworld_clock: Option<i32> = None;
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
                tx.send(config::c2s::CLIENT_INFORMATION, &client_information(opts.view_distance).buf)?;
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
                if registry == "minecraft:world_clock" {
                    // SET_TIME carries several clocks (overworld, the End) in hash order; remember which ID is ours.
                    for i in 0..entries {
                        if r.string()? == "minecraft:overworld" {
                            overworld_clock = Some(i as i32);
                        }
                        if r.bool()? {
                            r.nbt()?;
                        }
                    }
                }
                if registry == "minecraft:enchantment" {
                    // Entries: name, then optional data (absent for known packs). The order is the ID order.
                    let mut names = Vec::with_capacity(entries);
                    for _ in 0..entries {
                        names.push(r.string()?);
                        if r.bool()? {
                            r.nbt()?;
                        }
                    }
                    emit(Event::Enchantments(names));
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
                // With a window, the game loop accepts when it applies the move, so no stale position can follow
                // the accept (the server rejected those as "moved too quickly").
                if !opts.drives_movement {
                    send_accept_teleport(&tx, teleport, pos, rot)?;
                    send_tick(&tx, pos, rot, 0)?;
                }
                emit(Event::Teleport(pos, rot, teleport));
            }
            (State::Play, play::s2c::LEVEL_CHUNK_WITH_LIGHT) => match mc_world::decode_chunk(&body, min_section, reg) {
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
            (State::Play, play::s2c::CONTAINER_SET_CONTENT) => {
                if let Some(comps) = opts.components {
                    let container = r.varint()?;
                    let state = r.varint()?;
                    let n = r.len()?;
                    let mut ok = true;
                    let items: Vec<_> = (0..n).map(|_| stack(&mut r, comps, &mut ok)).collect();
                    let carried = stack(&mut r, comps, &mut ok);
                    if container == 0 {
                        emit(Event::Inventory(state, items));
                    } else {
                        emit(Event::Container { id: container, state, items });
                    }
                    if ok {
                        emit(Event::Cursor(carried));
                    }
                }
            }
            (State::Play, play::s2c::CONTAINER_SET_SLOT) => {
                if let Some(comps) = opts.components {
                    let (container, state, slot) = (r.varint()?, r.varint()?, r.i16()?);
                    let mut ok = true;
                    let item = stack(&mut r, comps, &mut ok);
                    if container == 0 && slot >= 0 {
                        emit(Event::InventorySlot(state, slot as usize, item));
                    } else if slot >= 0 {
                        emit(Event::ContainerSlot { id: container, state, slot: slot as usize, item });
                    }
                }
            }
            (State::Play, play::s2c::SET_PLAYER_INVENTORY) => {
                if let Some(comps) = opts.components {
                    let index = r.varint()?;
                    let mut ok = true;
                    let item = stack(&mut r, comps, &mut ok);
                    emit(Event::PlayerSlot(index.max(0) as usize, item));
                }
            }
            (State::Play, play::s2c::ADD_ENTITY) => {
                let id = r.varint()?;
                let _uuid = r.uuid()?;
                let kind = r.varint()? as u32;
                let pos = vec3(&mut r)?;
                skip_lp_vec3(&mut r)?;
                let (pitch, yaw, head_yaw) = (angle(r.u8()?), angle(r.u8()?), angle(r.u8()?));
                let data = r.varint().unwrap_or(0);
                emit(Event::EntityAdd { id, kind, pos, yaw, pitch, head_yaw, data });
            }
            (State::Play, packet @ (play::s2c::MOVE_ENTITY_POS | play::s2c::MOVE_ENTITY_POS_ROT)) => {
                let entity = r.varint()?;
                // Low bit = on ground, the rest = number of steps (0 = a single plain delta).
                let properties = r.varint()?;
                let steps = vec_delta(&mut r, (properties >> 1).max(0) as usize)?;
                let rot = if packet == play::s2c::MOVE_ENTITY_POS_ROT { Some([angle(r.u8()?), angle(r.u8()?)]) } else { None };
                emit(Event::EntityMove { id: entity, steps, rot });
            }
            (State::Play, play::s2c::MOVE_ENTITY_ROT) => {
                let id = r.varint()?;
                let _on_ground = r.bool()?;
                let rot = [angle(r.u8()?), angle(r.u8()?)];
                emit(Event::EntityRot { id, rot });
            }
            (State::Play, play::s2c::ENTITY_POSITION_SYNC) => {
                let id = r.varint()?;
                // PositionPath: 0 = one position, 1 = list of (position, tick offset); we keep the end point.
                let pos = if r.varint()? == 0 {
                    vec3(&mut r)?
                } else {
                    let mut last = [0.0; 3];
                    for _ in 0..r.len()? {
                        last = vec3(&mut r)?;
                        r.varint()?;
                    }
                    last
                };
                let rot = [r.f32()?, r.f32()?];
                emit(Event::EntityPos { id, pos, rot });
            }
            (State::Play, play::s2c::TELEPORT_ENTITY) => {
                let id = r.varint()?;
                let pos = vec3(&mut r)?;
                let _delta = vec3(&mut r)?;
                let rot = [r.f32()?, r.f32()?];
                emit(Event::EntityPos { id, pos, rot });
            }
            (State::Play, play::s2c::ROTATE_HEAD) => {
                let id = r.varint()?;
                emit(Event::EntityHead { id, yaw: angle(r.u8()?) });
            }
            (State::Play, play::s2c::REMOVE_ENTITIES) => {
                let n = r.len()?;
                let ids = (0..n).map(|_| r.varint()).collect::<std::io::Result<Vec<_>>>()?;
                emit(Event::EntityRemove(ids));
            }
            (State::Play, play::s2c::SOUND) => {
                let event = sound_ref(&mut r)?;
                let category = r.varint()? as usize;
                let at = [r.i32()? as f64 / 8.0, r.i32()? as f64 / 8.0, r.i32()? as f64 / 8.0];
                emit(Event::Sound { event, at: Some(at), entity: None, volume: r.f32()?, pitch: r.f32()?, category });
            }
            (State::Play, play::s2c::SOUND_ENTITY) => {
                let event = sound_ref(&mut r)?;
                let (category, entity) = (r.varint()? as usize, r.varint()?);
                emit(Event::Sound { event, at: None, entity: Some(entity), volume: r.f32()?, pitch: r.f32()?, category });
            }
            (State::Play, play::s2c::LEVEL_EVENT) => {
                let kind = r.i32()?;
                let pos = mc_world::unpack_block_pos(r.i64()?);
                emit(Event::LevelEvent { kind, pos, data: r.i32()? });
            }
            (State::Play, play::s2c::SET_HELD_SLOT) => emit(Event::HeldSlot(r.varint()?.clamp(0, 8) as u8)),
            (State::Play, play::s2c::FORGET_LEVEL_CHUNK) => {
                let packed = r.i64()?;
                emit(Event::Forget(packed as i32, (packed >> 32) as i32));
            }
            (State::Play, play::s2c::CHUNK_BATCH_FINISHED) => {
                // Chunks per tick we can take; meshing runs on worker threads, so ask for plenty.
                tx.send(play::c2s::CHUNK_BATCH_RECEIVED, &Writer::new().f32(64.0).buf)?;
            }
            (State::Play, play::s2c::GAME_EVENT) => {
                let (event, param) = (r.u8()?, r.f32()?);
                match event {
                    // Start/stop raining, rain strength.
                    1 => emit(Event::Rain(1.0)),
                    2 => emit(Event::Rain(0.0)),
                    7 => emit(Event::Rain(param)),
                    3 => emit(Event::GameMode(param as u8)),
                    13 if !loaded => {
                        loaded = true;
                        tx.send(play::c2s::PLAYER_LOADED, &[])?;
                        emit(Event::InWorld);
                    }
                    _ => {}
                }
            }
            (State::Play, play::s2c::LOGIN) => {
                let entity = r.i32()?;
                let hardcore = r.bool()?;
                for _ in 0..r.len()? {
                    r.string()?;
                }
                let (_max_players, _view, _sim) = (r.varint()?, r.varint()?, r.varint()?);
                let (_reduced_debug, death_screen, _limited_crafting) = (r.bool()?, r.bool()?, r.bool()?);
                let (game_mode, dimension) = spawn_info(&mut r)?;
                min_section = crate::sky::Dimension::from_name(&dimension).min_section();
                emit(Event::Joined { entity, hardcore, game_mode, dimension, death_screen });
            }
            (State::Play, play::s2c::RESPAWN) => {
                let (game_mode, dimension) = spawn_info(&mut r)?;
                min_section = crate::sky::Dimension::from_name(&dimension).min_section();
                emit(Event::Respawned { game_mode, dimension });
            }
            (State::Play, play::s2c::SET_EXPERIENCE) => {
                emit(Event::Experience { progress: r.f32()?, level: r.varint()?, total: r.varint()? });
            }
            (State::Play, play::s2c::SET_TIME) => {
                let _game_time = r.i64()?;
                // Clock updates: (clock holder, total ticks, partial tick, rate); the overworld has one clock.
                let mut day = None;
                for _ in 0..r.len()? {
                    let clock = r.varint()?;
                    let ticks = r.varlong()?;
                    let (_partial, rate) = (r.f32()?, r.f32()?);
                    if overworld_clock.is_none_or(|c| c == clock) {
                        day = Some((ticks, rate));
                    }
                }
                emit(Event::Time { day });
            }
            (State::Play, play::s2c::SET_ENTITY_DATA) => {
                let id = r.varint()?;
                if let Ok((data, air)) = entity_data(&mut r, opts.components) {
                    emit(Event::EntityData { id, data, air });
                }
            }
            (State::Play, play::s2c::SET_CURSOR_ITEM) => {
                if let Some(comps) = opts.components {
                    let mut ok = true;
                    emit(Event::Cursor(stack(&mut r, comps, &mut ok)));
                }
            }
            (State::Play, play::s2c::OPEN_SCREEN) => {
                let (id, kind) = (r.varint()?, r.varint()? as u32);
                let title = r.nbt()?.unwrap_or(mc_nbt::Tag::String(String::new()));
                emit(Event::OpenScreen { id, kind, title });
            }
            (State::Play, play::s2c::CONTAINER_CLOSE) => emit(Event::CloseScreen),
            (State::Play, play::s2c::CONTAINER_SET_DATA) => {
                emit(Event::ContainerData { id: r.varint()?, prop: r.i16()?, value: r.i16()? });
            }
            (State::Play, play::s2c::DAMAGE_EVENT) => emit(Event::Hurt(r.varint()?)),
            (State::Play, play::s2c::ANIMATE) => {
                let id = r.varint()?;
                // 0 = swing main hand, 3 = swing off hand.
                if matches!(r.u8()?, 0 | 3) {
                    emit(Event::Swing(id));
                }
            }
            (State::Play, play::s2c::PLAYER_COMBAT_KILL) => {
                let _player = r.varint()?;
                emit(Event::Died(r.nbt()?.unwrap_or(mc_nbt::Tag::String(String::new()))));
            }
            (State::Play, play::s2c::SET_TITLE_TEXT) => emit(Event::Title(r.nbt()?.unwrap_or(mc_nbt::Tag::String(String::new())))),
            (State::Play, play::s2c::SET_SUBTITLE_TEXT) => emit(Event::Subtitle(r.nbt()?.unwrap_or(mc_nbt::Tag::String(String::new())))),
            (State::Play, play::s2c::SET_ACTION_BAR_TEXT) => emit(Event::ActionBar(r.nbt()?.unwrap_or(mc_nbt::Tag::String(String::new())))),
            (State::Play, play::s2c::SET_TITLES_ANIMATION) => emit(Event::TitleTimes(r.i32()?, r.i32()?, r.i32()?)),
            (State::Play, play::s2c::CLEAR_TITLES) => emit(Event::ClearTitles),
            (State::Play, play::s2c::PLAYER_INFO_UPDATE) => {
                let _ = player_info(&mut r, &mut emit);
            }
            (State::Play, play::s2c::PLAYER_INFO_REMOVE) => {
                let n = r.len()?;
                let ids = (0..n).map(|_| r.uuid()).collect::<std::io::Result<Vec<_>>>()?;
                emit(Event::PlayersRemoved(ids));
            }
            (State::Play, play::s2c::TAB_LIST) => {
                let header = r.nbt()?.unwrap_or(mc_nbt::Tag::String(String::new()));
                let footer = r.nbt()?.unwrap_or(mc_nbt::Tag::String(String::new()));
                emit(Event::TabList(header, footer));
            }
            (State::Play, play::s2c::CHANGE_DIFFICULTY) => emit(Event::Difficulty(r.u8()?)),
            (State::Play, play::s2c::SET_HEALTH) => {
                let health = r.f32()?;
                emit(Event::Health { health, food: r.varint()? });
            }
            (State::Play, play::s2c::SYSTEM_CHAT) => {
                let tag = r.nbt().ok().flatten();
                let overlay = r.bool().unwrap_or(false);
                let msg = tag.as_ref().map(text::plain).unwrap_or_default();
                if opts.greet {
                    if let Some(who) = msg.strip_prefix("multiplayer.player.joined [").and_then(|s| s.strip_suffix(']')) {
                        if who != name {
                            send_chat(&tx, &format!("Hi {who}! I'm {name}, a Minecraft client written in Rust."))?;
                        }
                    }
                }
                // Overlay messages go above the hotbar in vanilla; we show them in chat for now.
                let _ = overlay;
                match tag {
                    Some(tag) => emit(Event::ChatTag(tag)),
                    None => emit(Event::Chat(msg)),
                }
            }
            (State::Play, play::s2c::PLAYER_CHAT) => {
                if let Some(tag) = player_chat(&mut r).ok().flatten() {
                    emit(Event::ChatTag(tag));
                }
            }
            (State::Play, play::s2c::DISGUISED_CHAT) => {
                let message = r.nbt()?.unwrap_or(mc_nbt::Tag::String(String::new()));
                if let Some((name, _)) = chat_bound(&mut r).ok().flatten() {
                    emit(Event::ChatTag(chat_line(name, message)));
                }
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
    fn block_pos_round_trips() {
        for p in [[0, 64, 0], [-5, -60, 12], [30_000_000, 319, -30_000_000]] {
            assert_eq!(mc_world::unpack_block_pos(pack_block_pos(p)), p);
        }
    }

    #[test]
    fn offline_uuid_matches_java() {
        // UUID.nameUUIDFromBytes("OfflinePlayer:Notch")
        assert_eq!(format!("{:032x}", offline_uuid("Notch")), "b50ad385829d3141a2167e7d7539ba7f");
    }
}
