//! Packet framing over TCP: length prefix, optional zlib compression.
//! Encryption isn't implemented yet (only needed for online-mode servers).

use crate::buf::{Reader, write_varint};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::io::{BufReader, Error, ErrorKind, Read, Result, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

const MAX_PACKET: usize = 2 * 1024 * 1024;
const MAX_UNCOMPRESSED: usize = 8 * 1024 * 1024;

fn bad(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, msg.to_string())
}

/// Sending half; cheap to clone and safe to share between threads.
#[derive(Clone)]
pub struct Sender {
    stream: Arc<Mutex<TcpStream>>,
    threshold: Arc<AtomicI32>,
}

impl Sender {
    /// Closes the connection; the receiving side then fails, ending the session thread.
    pub fn shutdown(&self) {
        if let Ok(s) = self.stream.lock() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    pub fn send(&self, id: i32, body: &[u8]) -> Result<()> {
        let mut inner = Vec::with_capacity(body.len() + 5);
        write_varint(&mut inner, id);
        inner.extend_from_slice(body);

        let threshold = self.threshold.load(Relaxed);
        let payload = if threshold < 0 {
            inner
        } else if inner.len() < threshold as usize {
            let mut p = vec![0];
            p.extend_from_slice(&inner);
            p
        } else {
            let mut p = Vec::new();
            write_varint(&mut p, inner.len() as i32);
            let mut z = ZlibEncoder::new(p, Compression::default());
            z.write_all(&inner)?;
            z.finish()?
        };
        let mut frame = Vec::with_capacity(payload.len() + 5);
        write_varint(&mut frame, payload.len() as i32);
        frame.extend_from_slice(&payload);
        self.stream.lock().unwrap().write_all(&frame)
    }
}

/// Receiving half.
pub struct Receiver {
    reader: BufReader<TcpStream>,
    threshold: Arc<AtomicI32>,
}

impl Receiver {
    fn read_varint(&mut self) -> Result<i32> {
        let mut value = 0u32;
        for i in 0..5 {
            let mut b = [0u8];
            self.reader.read_exact(&mut b)?;
            value |= ((b[0] & 0x7f) as u32) << (7 * i);
            if b[0] & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(bad("frame length varint too long"))
    }

    /// Blocks for the next packet and returns `(id, body)`.
    pub fn recv(&mut self) -> Result<(i32, Vec<u8>)> {
        let len = usize::try_from(self.read_varint()?).map_err(|_| bad("negative frame length"))?;
        if len > MAX_PACKET {
            return Err(bad("frame too large"));
        }
        let mut frame = vec![0u8; len];
        self.reader.read_exact(&mut frame)?;

        let inner = if self.threshold.load(Relaxed) < 0 {
            frame
        } else {
            let mut r = Reader::new(&frame);
            let data_len = r.len()?;
            if data_len == 0 {
                r.remaining().to_vec()
            } else {
                if data_len > MAX_UNCOMPRESSED {
                    return Err(bad("decompressed packet too large"));
                }
                let mut out = Vec::with_capacity(data_len);
                ZlibDecoder::new(r.remaining()).take(data_len as u64).read_to_end(&mut out)?;
                if out.len() != data_len {
                    return Err(bad("decompressed size mismatch"));
                }
                out
            }
        };
        let mut r = Reader::new(&inner);
        let id = r.varint()?;
        Ok((id, r.remaining().to_vec()))
    }

    /// Applies the threshold from the login compression packet to both halves.
    pub fn set_compression(&self, threshold: i32) {
        self.threshold.store(threshold, Relaxed);
    }
}

pub fn connect(addr: &str) -> Result<(Sender, Receiver)> {
    let stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    let threshold = Arc::new(AtomicI32::new(-1));
    let sender = Sender { stream: Arc::new(Mutex::new(stream.try_clone()?)), threshold: threshold.clone() };
    let receiver = Receiver { reader: BufReader::with_capacity(64 * 1024, stream), threshold };
    Ok((sender, receiver))
}

/// Like [`connect`], but gives up after `timeout` (for both connecting and every later read).
/// `addr` may be a host name; port 25565 is used when none is given.
pub fn connect_timeout(addr: &str, timeout: std::time::Duration) -> Result<(Sender, Receiver)> {
    use std::net::ToSocketAddrs;
    let with_port = if addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) { addr.to_string() } else { format!("{addr}:25565") };
    let target = with_port.to_socket_addrs()?.next().ok_or_else(|| bad("unknown host"))?;
    let stream = TcpStream::connect_timeout(&target, timeout)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(timeout))?;
    let threshold = Arc::new(AtomicI32::new(-1));
    let sender = Sender { stream: Arc::new(Mutex::new(stream.try_clone()?)), threshold: threshold.clone() };
    let receiver = Receiver { reader: BufReader::with_capacity(64 * 1024, stream), threshold };
    Ok((sender, receiver))
}
