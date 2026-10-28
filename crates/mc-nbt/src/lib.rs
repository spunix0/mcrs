//! NBT (named binary tag) reading and writing: the format behind chunks, level.dat and many packets.
//!
//! Network NBT (since 1.20.2) is a type byte followed by the payload, with no root name.
//! Strings are Java "modified UTF-8"; they're decoded as UTF-8 here, which matches for everything but
//! NUL and characters outside the BMP.

use std::io::{Error, ErrorKind, Result};

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<Tag>),
    Compound(Vec<(String, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

const MAX_DEPTH: usize = 512;

impl Tag {
    pub fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(_) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Tag> {
        match self {
            Tag::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Tag::List(v) => Some(v),
            _ => None,
        }
    }
}

fn bad(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, msg.to_string())
}

fn take<'a>(buf: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if buf.len() < n {
        return Err(Error::new(ErrorKind::UnexpectedEof, "nbt: truncated"));
    }
    let (head, rest) = buf.split_at(n);
    *buf = rest;
    Ok(head)
}

fn arr<const N: usize>(buf: &mut &[u8]) -> Result<[u8; N]> {
    Ok(take(buf, N)?.try_into().unwrap())
}

fn read_len(buf: &mut &[u8]) -> Result<usize> {
    let n = i32::from_be_bytes(arr(buf)?);
    usize::try_from(n).map_err(|_| bad("nbt: negative length"))
}

fn read_string(buf: &mut &[u8]) -> Result<String> {
    let n = u16::from_be_bytes(arr(buf)?) as usize;
    Ok(String::from_utf8_lossy(take(buf, n)?).into_owned())
}

fn read_payload(buf: &mut &[u8], id: u8, depth: usize) -> Result<Tag> {
    if depth > MAX_DEPTH {
        return Err(bad("nbt: too deeply nested"));
    }
    Ok(match id {
        1 => Tag::Byte(i8::from_be_bytes(arr(buf)?)),
        2 => Tag::Short(i16::from_be_bytes(arr(buf)?)),
        3 => Tag::Int(i32::from_be_bytes(arr(buf)?)),
        4 => Tag::Long(i64::from_be_bytes(arr(buf)?)),
        5 => Tag::Float(f32::from_be_bytes(arr(buf)?)),
        6 => Tag::Double(f64::from_be_bytes(arr(buf)?)),
        7 => {
            let n = read_len(buf)?;
            Tag::ByteArray(take(buf, n)?.iter().map(|&b| b as i8).collect())
        }
        8 => Tag::String(read_string(buf)?),
        9 => {
            let elem = arr::<1>(buf)?[0];
            let n = read_len(buf)?;
            if elem == 0 && n > 0 {
                return Err(bad("nbt: non-empty list of End"));
            }
            let mut v = Vec::with_capacity(n.min(buf.len()));
            for _ in 0..n {
                v.push(read_payload(buf, elem, depth + 1)?);
            }
            Tag::List(v)
        }
        10 => {
            let mut entries = Vec::new();
            loop {
                let t = arr::<1>(buf)?[0];
                if t == 0 {
                    break;
                }
                let name = read_string(buf)?;
                entries.push((name, read_payload(buf, t, depth + 1)?));
            }
            Tag::Compound(entries)
        }
        11 => {
            let n = read_len(buf)?;
            let bytes = take(buf, n.checked_mul(4).ok_or_else(|| bad("nbt: array too large"))?)?;
            Tag::IntArray(bytes.chunks_exact(4).map(|c| i32::from_be_bytes(c.try_into().unwrap())).collect())
        }
        12 => {
            let n = read_len(buf)?;
            let bytes = take(buf, n.checked_mul(8).ok_or_else(|| bad("nbt: array too large"))?)?;
            Tag::LongArray(bytes.chunks_exact(8).map(|c| i64::from_be_bytes(c.try_into().unwrap())).collect())
        }
        _ => return Err(bad("nbt: unknown tag type")),
    })
}

/// Reads network NBT (unnamed root). Returns `None` for an End tag, which the protocol uses for "no value".
pub fn read_network(buf: &mut &[u8]) -> Result<Option<Tag>> {
    let id = arr::<1>(buf)?[0];
    if id == 0 {
        return Ok(None);
    }
    read_payload(buf, id, 0).map(Some)
}

/// Reads file NBT (named root compound), returning the root name and tag.
pub fn read_named(buf: &mut &[u8]) -> Result<(String, Tag)> {
    let id = arr::<1>(buf)?[0];
    if id != 10 {
        return Err(bad("nbt: root is not a compound"));
    }
    let name = read_string(buf)?;
    Ok((name, read_payload(buf, id, 0)?))
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(u16::MAX as usize);
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&bytes[..n]);
}

fn write_payload(out: &mut Vec<u8>, tag: &Tag) {
    match tag {
        Tag::Byte(v) => out.push(*v as u8),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::ByteArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.extend(v.iter().map(|&b| b as u8));
        }
        Tag::String(s) => write_string(out, s),
        Tag::List(v) => {
            out.push(v.first().map_or(0, Tag::id));
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for t in v {
                write_payload(out, t);
            }
        }
        Tag::Compound(entries) => {
            for (k, t) in entries {
                out.push(t.id());
                write_string(out, k);
                write_payload(out, t);
            }
            out.push(0);
        }
        Tag::IntArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        Tag::LongArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
    }
}

/// Writes network NBT (unnamed root).
pub fn write_network(out: &mut Vec<u8>, tag: &Tag) {
    out.push(tag.id());
    write_payload(out, tag);
}

/// Writes file NBT (named root).
pub fn write_named(out: &mut Vec<u8>, name: &str, tag: &Tag) {
    out.push(tag.id());
    write_string(out, name);
    write_payload(out, tag);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_round_trip() {
        let tag = Tag::Compound(vec![
            ("text".into(), Tag::String("hello".into())),
            ("bold".into(), Tag::Byte(1)),
            ("extra".into(), Tag::List(vec![Tag::Int(1), Tag::Int(2)])),
            ("longs".into(), Tag::LongArray(vec![-1, 7])),
        ]);
        let mut buf = Vec::new();
        write_network(&mut buf, &tag);
        let mut slice = buf.as_slice();
        assert_eq!(read_network(&mut slice).unwrap(), Some(tag));
        assert!(slice.is_empty());
    }

    #[test]
    fn named_round_trip() {
        let tag = Tag::Compound(vec![("x".into(), Tag::Double(1.5))]);
        let mut buf = Vec::new();
        write_named(&mut buf, "root", &tag);
        assert_eq!(read_named(&mut buf.as_slice()).unwrap(), ("root".into(), tag));
    }

    #[test]
    fn end_is_none_and_truncation_errors() {
        assert_eq!(read_network(&mut [0u8].as_slice()).unwrap(), None);
        assert!(read_network(&mut [10u8, 8, 0].as_slice()).is_err());
    }
}
