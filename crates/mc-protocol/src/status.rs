//! Server list ping: the status JSON (MOTD, players, version) and round-trip time, as the multiplayer screen shows them.

use crate::buf::{Reader, Writer};
use crate::conn::connect_timeout;
use crate::ids::{handshake, status};
use std::io::Result;
use std::time::{Duration, Instant};

pub struct Status {
    /// The raw status JSON (`description`, `players`, `version`, `favicon`).
    pub json: String,
    pub ping: Duration,
}

/// Asks a server for its status. `addr` is `host[:port]`.
pub fn ping(addr: &str, timeout: Duration) -> Result<Status> {
    let (host, port) = match addr.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h, p.parse().unwrap_or(25565)),
        _ => (addr, 25565),
    };
    let (tx, mut rx) = connect_timeout(addr, timeout)?;
    tx.send(handshake::INTENTION, &Writer::new().varint(crate::PROTOCOL_VERSION).string(host).u16(port).varint(1).buf)?;
    tx.send(status::c2s::STATUS_REQUEST, &[])?;
    let (_, body) = rx.recv()?;
    let json = Reader::new(&body).string()?;
    let sent = Instant::now();
    tx.send(status::c2s::PING_REQUEST, &Writer::new().i64(0).buf)?;
    let _ = rx.recv()?;
    Ok(Status { json, ping: sent.elapsed() })
}
