//! Client-side world state: chunk sections, block-state palettes, light data and block queries used by the renderer and physics.

use std::io::{Error, ErrorKind, Result};

pub const SECTION_VOLUME: usize = 4096;

/// Block states of one 16x16x16 section, indexed `(y << 8) | (z << 4) | x`.
#[derive(Clone)]
pub enum Section {
    Uniform(u16),
    Mixed(Box<[u16; SECTION_VOLUME]>),
}

impl Section {
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> u16 {
        match self {
            Section::Uniform(s) => *s,
            Section::Mixed(d) => d[(y << 8) | (z << 4) | x],
        }
    }

    pub fn is_air(&self) -> bool {
        matches!(self, Section::Uniform(0))
    }
}

/// A chunk column as sent by the server.
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    /// Section index of `sections[0]` (world Y = min_section * 16).
    pub min_section: i32,
    pub sections: Vec<Section>,
}

impl Chunk {
    /// Block state at world-relative coordinates inside this column (`x`, `z` in 0..16). Out of range Y is air.
    pub fn get(&self, x: usize, y: i32, z: usize) -> u16 {
        let s = (y >> 4) - self.min_section;
        match self.sections.get(s as usize) {
            Some(sec) if s >= 0 => sec.get(x, (y & 15) as usize, z),
            _ => 0,
        }
    }
}

fn bad(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, msg.to_string())
}

/// Sizes of the global palettes; needed because direct-palette entries use ceil(log2(size)) bits.
#[derive(Clone, Copy)]
pub struct Registries {
    pub block_states: usize,
    pub biomes: usize,
}

fn ceil_log2(n: usize) -> u32 {
    usize::BITS - n.saturating_sub(1).leading_zeros()
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Error::new(ErrorKind::UnexpectedEof, "chunk: truncated"));
        }
        let (h, t) = self.0.split_at(n);
        self.0 = t;
        Ok(h)
    }
    fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
    fn i16(&mut self) -> Result<i16> { Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap())) }
    fn i32(&mut self) -> Result<i32> { Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap())) }
    fn i64(&mut self) -> Result<i64> { Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap())) }
    fn varint(&mut self) -> Result<i32> {
        let mut v = 0u32;
        for i in 0..5 {
            let b = self.u8()?;
            v |= ((b & 0x7f) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v as i32);
            }
        }
        Err(bad("varint too long"))
    }
    fn len(&mut self) -> Result<usize> {
        usize::try_from(self.varint()?).map_err(|_| bad("negative length"))
    }
}

enum Palette {
    Single(u32),
    List(Vec<u32>),
    Global,
}

/// Reads one paletted container (blocks: 4 bits per axis, biomes: 2) and returns its values.
fn read_container(c: &mut Cursor, entries: usize, is_blocks: bool, global_size: usize) -> Result<Option<Vec<u32>>> {
    let bits = c.u8()? as u32;
    // Mirrors Strategy.getConfigurationForBitCount: what's on the wire vs. what the storage uses.
    let (palette, storage_bits) = match (is_blocks, bits) {
        (_, 0) => (Palette::Single(c.varint()? as u32), 0),
        (true, 1..=4) => (Palette::List(read_list(c)?), 4),
        (true, 5..=8) => (Palette::List(read_list(c)?), bits),
        (false, 1..=3) => (Palette::List(read_list(c)?), bits),
        _ => (Palette::Global, ceil_log2(global_size)),
    };
    let longs = if storage_bits == 0 {
        0
    } else {
        let per_long = 64 / storage_bits as usize;
        entries.div_ceil(per_long)
    };
    let mut raw = Vec::with_capacity(longs);
    for _ in 0..longs {
        raw.push(c.i64()? as u64);
    }
    if let Palette::Single(v) = palette {
        return Ok(Some(vec![v]));
    }
    let per_long = 64 / storage_bits as usize;
    let mask = (1u64 << storage_bits) - 1;
    let mut out = Vec::with_capacity(entries);
    for i in 0..entries {
        let word = raw[i / per_long];
        let idx = ((word >> ((i % per_long) as u32 * storage_bits)) & mask) as usize;
        out.push(match &palette {
            Palette::List(p) => *p.get(idx).ok_or_else(|| bad("palette index out of range"))?,
            _ => idx as u32,
        });
    }
    Ok(Some(out))
}

fn read_list(c: &mut Cursor) -> Result<Vec<u32>> {
    let n = c.len()?;
    (0..n).map(|_| c.varint().map(|v| v as u32)).collect()
}

/// Decodes the body of `level_chunk_with_light` (protocol 777) into block states. Light and block entities are skipped.
pub fn decode_chunk(body: &[u8], min_section: i32, reg: Registries) -> Result<Chunk> {
    let mut c = Cursor(body);
    let x = c.i32()?;
    let z = c.i32()?;
    // Heightmaps: map of type id -> long array.
    for _ in 0..c.len()? {
        c.varint()?;
        let n = c.len()?;
        c.take(n * 8)?;
    }
    let n = c.len()?;
    let mut data = Cursor(c.take(n)?);
    let mut sections = Vec::new();
    while !data.0.is_empty() {
        let _non_air = data.i16()?;
        let _fluids = data.i16()?;
        let blocks = read_container(&mut data, SECTION_VOLUME, true, reg.block_states)?.unwrap();
        read_container(&mut data, 64, false, reg.biomes)?;
        sections.push(if blocks.len() == 1 {
            Section::Uniform(blocks[0] as u16)
        } else {
            let mut arr = Box::new([0u16; SECTION_VOLUME]);
            for (dst, src) in arr.iter_mut().zip(&blocks) {
                *dst = *src as u16;
            }
            Section::Mixed(arr)
        });
    }
    Ok(Chunk { x, z, min_section, sections })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(out: &mut Vec<u8>, v: i32) {
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

    #[test]
    fn decodes_single_and_linear_sections() {
        let mut sec = Vec::new();
        // Section 1: all stone (single value 1), biome single 0.
        sec.extend_from_slice(&4096i16.to_be_bytes());
        sec.extend_from_slice(&0i16.to_be_bytes());
        sec.push(0);
        varint(&mut sec, 1);
        sec.push(0);
        varint(&mut sec, 0);
        // Section 2: 2-entry palette [0, 9] sent as 1 bit -> stored at 4 bits; entry 1 is the second value.
        sec.extend_from_slice(&1i16.to_be_bytes());
        sec.extend_from_slice(&0i16.to_be_bytes());
        sec.push(1);
        varint(&mut sec, 2);
        varint(&mut sec, 0);
        varint(&mut sec, 9);
        for i in 0..256u64 {
            let word: u64 = if i == 0 { 1 << 4 } else { 0 }; // entry 1 (x=1) -> palette index 1 = 9
            sec.extend_from_slice(&(word as i64).to_be_bytes());
        }
        sec.push(0);
        varint(&mut sec, 0);

        let mut body = Vec::new();
        body.extend_from_slice(&3i32.to_be_bytes());
        body.extend_from_slice(&(-2i32).to_be_bytes());
        varint(&mut body, 0); // no heightmaps
        varint(&mut body, sec.len() as i32);
        body.extend_from_slice(&sec);
        varint(&mut body, 0); // block entities (ignored)

        let chunk = decode_chunk(&body, -4, Registries { block_states: 35723, biomes: 65 }).unwrap();
        assert_eq!((chunk.x, chunk.z, chunk.sections.len()), (3, -2, 2));
        assert_eq!(chunk.get(5, -64, 5), 1);
        assert_eq!(chunk.get(1, -48, 0), 9);
        assert_eq!(chunk.get(0, -48, 0), 0);
        assert_eq!(chunk.get(0, 500, 0), 0);
    }

    #[test]
    fn ceil_log2_matches_mth() {
        assert_eq!(ceil_log2(35723), 16);
        assert_eq!(ceil_log2(65), 7);
        assert_eq!(ceil_log2(64), 6);
    }
}
