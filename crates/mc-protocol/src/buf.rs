//! Primitive encodings used by packet bodies: VarInt, strings, big-endian numbers, UUIDs, network NBT.

use std::io::{Error, ErrorKind, Result};

fn eof() -> Error {
    Error::new(ErrorKind::UnexpectedEof, "packet: truncated")
}

fn bad(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, msg.to_string())
}

/// Cursor over a received packet body.
pub struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    pub fn remaining(&self) -> &'a [u8] {
        self.buf
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.buf.len() < n {
            return Err(eof());
        }
        let (head, rest) = self.buf.split_at(n);
        self.buf = rest;
        Ok(head)
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }

    pub fn u8(&mut self) -> Result<u8> { Ok(self.arr::<1>()?[0]) }
    pub fn bool(&mut self) -> Result<bool> { Ok(self.u8()? != 0) }
    pub fn i16(&mut self) -> Result<i16> { Ok(i16::from_be_bytes(self.arr()?)) }
    pub fn u16(&mut self) -> Result<u16> { Ok(u16::from_be_bytes(self.arr()?)) }
    pub fn i32(&mut self) -> Result<i32> { Ok(i32::from_be_bytes(self.arr()?)) }
    pub fn i64(&mut self) -> Result<i64> { Ok(i64::from_be_bytes(self.arr()?)) }
    pub fn f32(&mut self) -> Result<f32> { Ok(f32::from_be_bytes(self.arr()?)) }
    pub fn f64(&mut self) -> Result<f64> { Ok(f64::from_be_bytes(self.arr()?)) }
    pub fn uuid(&mut self) -> Result<u128> { Ok(u128::from_be_bytes(self.arr()?)) }

    pub fn varint(&mut self) -> Result<i32> {
        let mut value = 0u32;
        for i in 0..5 {
            let b = self.u8()?;
            value |= ((b & 0x7f) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(bad("varint too long"))
    }

    pub fn varlong(&mut self) -> Result<i64> {
        let mut value = 0u64;
        for i in 0..10 {
            let b = self.u8()?;
            value |= ((b & 0x7f) as u64) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as i64);
            }
        }
        Err(bad("varlong too long"))
    }

    pub fn len(&mut self) -> Result<usize> {
        usize::try_from(self.varint()?).map_err(|_| bad("negative length"))
    }

    pub fn string(&mut self) -> Result<String> {
        let n = self.len()?;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }

    pub fn nbt(&mut self) -> Result<Option<mc_nbt::Tag>> {
        let mut slice = self.buf;
        let tag = mc_nbt::read_network(&mut slice)?;
        self.buf = slice;
        Ok(tag)
    }
}

/// Builder for an outgoing packet body.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, v: u8) -> &mut Self { self.buf.push(v); self }
    pub fn bool(&mut self, v: bool) -> &mut Self { self.u8(v as u8) }
    pub fn u16(&mut self, v: u16) -> &mut Self { self.buf.extend_from_slice(&v.to_be_bytes()); self }
    pub fn i32(&mut self, v: i32) -> &mut Self { self.buf.extend_from_slice(&v.to_be_bytes()); self }
    pub fn i64(&mut self, v: i64) -> &mut Self { self.buf.extend_from_slice(&v.to_be_bytes()); self }
    pub fn f32(&mut self, v: f32) -> &mut Self { self.buf.extend_from_slice(&v.to_be_bytes()); self }
    pub fn f64(&mut self, v: f64) -> &mut Self { self.buf.extend_from_slice(&v.to_be_bytes()); self }
    pub fn uuid(&mut self, v: u128) -> &mut Self { self.buf.extend_from_slice(&v.to_be_bytes()); self }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self { self.buf.extend_from_slice(v); self }

    pub fn varint(&mut self, v: i32) -> &mut Self {
        write_varint(&mut self.buf, v);
        self
    }

    pub fn string(&mut self, s: &str) -> &mut Self {
        self.varint(s.len() as i32);
        self.buf.extend_from_slice(s.as_bytes());
        self
    }
}

pub fn write_varint(out: &mut Vec<u8>, v: i32) {
    let mut v = v as u32;
    loop {
        if v & !0x7f == 0 {
            out.push(v as u8);
            return;
        }
        out.push((v & 0x7f | 0x80) as u8);
        v >>= 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_known_values() {
        for (v, bytes) in [
            (0, vec![0x00]),
            (1, vec![0x01]),
            (127, vec![0x7f]),
            (128, vec![0x80, 0x01]),
            (25565, vec![0xdd, 0xc7, 0x01]),
            (2147483647, vec![0xff, 0xff, 0xff, 0xff, 0x07]),
            (-1, vec![0xff, 0xff, 0xff, 0xff, 0x0f]),
        ] {
            let mut out = Vec::new();
            write_varint(&mut out, v);
            assert_eq!(out, bytes, "encode {v}");
            assert_eq!(Reader::new(&bytes).varint().unwrap(), v, "decode {v}");
        }
    }

    #[test]
    fn string_round_trip() {
        let mut w = Writer::new();
        w.string("héllo").i64(-5);
        let mut r = Reader::new(&w.buf);
        assert_eq!(r.string().unwrap(), "héllo");
        assert_eq!(r.i64().unwrap(), -5);
    }
}
