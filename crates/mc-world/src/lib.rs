//! Client-side world state: chunk sections, block-state palettes, light data and block queries used by the renderer and physics.

pub mod region;
pub mod relight;

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

    pub fn set(&mut self, x: usize, y: usize, z: usize, state: u16) {
        if let Section::Uniform(v) = *self {
            if v == state {
                return;
            }
            *self = Section::Mixed(Box::new([v; SECTION_VOLUME]));
        }
        if let Section::Mixed(d) = self {
            d[(y << 8) | (z << 4) | x] = state;
        }
    }
}

/// One light layer of a 16x16x16 section: 4096 nibbles, same index order as [`Section`].
pub type Nibbles = Box<[u8; 2048]>;

fn nibble(data: &[u8; 2048], x: usize, y: usize, z: usize) -> u8 {
    let i = (y << 8) | (z << 4) | x;
    (data[i >> 1] >> ((i & 1) * 4)) & 15
}

/// Sky and block light for a column. Light sections run one below and one above the block sections.
#[derive(Clone, Default)]
pub struct Light {
    pub sky: Vec<Option<Nibbles>>,
    pub block: Vec<Option<Nibbles>>,
}

/// A light payload from `level_chunk_with_light` or `light_update`.
pub struct LightUpdate {
    sky_mask: Vec<u8>,
    block_mask: Vec<u8>,
    empty_sky: Vec<u8>,
    empty_block: Vec<u8>,
    sky: Vec<Nibbles>,
    block: Vec<Nibbles>,
}

/// Bitsets are `BitSet.toByteArray()` since 26.x: little-endian bytes, bit i = byte i/8, bit i%8.
fn bit(mask: &[u8], i: usize) -> bool {
    mask.get(i / 8).is_some_and(|b| (b >> (i % 8)) & 1 != 0)
}

impl LightUpdate {
    /// Block sections whose meshes this update changes: light section i is block section i - 1, and smooth
    /// lighting reads one block past each face, so the sections above and below count too.
    pub fn block_sections(&self) -> u64 {
        let masks = [&self.sky_mask, &self.block_mask, &self.empty_sky, &self.empty_block];
        let light = (0..64).filter(|i| masks.iter().any(|m| bit(m, *i))).fold(0u64, |m, i| m | 1 << i);
        light | light >> 1 | light >> 2
    }
}

impl Light {
    fn apply_layer(layer: &mut Vec<Option<Nibbles>>, count: usize, mask: &[u8], empty: &[u8], data: Vec<Nibbles>) {
        layer.resize_with(count, || None);
        let mut data = data.into_iter();
        for i in 0..count {
            if bit(mask, i) {
                layer[i] = data.next();
            } else if bit(empty, i) {
                layer[i] = Some(Box::new([0; 2048]));
            }
        }
    }

    pub fn apply(&mut self, update: LightUpdate, light_sections: usize) {
        Self::apply_layer(&mut self.sky, light_sections, &update.sky_mask, &update.empty_sky, update.sky);
        Self::apply_layer(&mut self.block, light_sections, &update.block_mask, &update.empty_block, update.block);
    }
}

/// A chunk column as sent by the server.
#[derive(Clone)]
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    /// Section index of `sections[0]` (world Y = min_section * 16).
    pub min_section: i32,
    pub sections: Vec<Section>,
    pub light: Light,
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

    /// Sets a block; returns false if `y` is outside the column.
    pub fn set(&mut self, x: usize, y: i32, z: usize, state: u16) -> bool {
        let s = (y >> 4) - self.min_section;
        match self.sections.get_mut(s as usize) {
            Some(sec) if s >= 0 => {
                sec.set(x, (y & 15) as usize, z, state);
                true
            }
            _ => false,
        }
    }

    /// `(sky, block)` light at a position in this column. Sections without data count as full sky / no block light.
    pub fn light(&self, x: usize, y: i32, z: usize) -> (u8, u8) {
        let s = ((y >> 4) - self.min_section + 1) as usize;
        let (ly, sky, block) = ((y & 15) as usize, self.light.sky.get(s), self.light.block.get(s));
        let sky = match sky {
            Some(Some(d)) => nibble(d, x, ly, z),
            _ => 15,
        };
        let block = match block {
            Some(Some(d)) => nibble(d, x, ly, z),
            _ => 0,
        };
        (sky, block)
    }

    /// Sets one light level; returns false if `y` is outside the light sections. A section without data gets one
    /// (full sky or no block light) first.
    pub fn set_light(&mut self, x: usize, y: i32, z: usize, sky: bool, v: u8) -> bool {
        let s = (y >> 4) - self.min_section + 1;
        let count = self.light_sections();
        let layer = if sky { &mut self.light.sky } else { &mut self.light.block };
        if s < 0 || s as usize >= count {
            return false;
        }
        layer.resize_with(count, || None);
        let data = layer[s as usize].get_or_insert_with(|| Box::new([if sky { 0xff } else { 0 }; 2048]));
        let i = (((y & 15) as usize) << 8) | (z << 4) | x;
        let shift = (i & 1) * 4;
        data[i >> 1] = (data[i >> 1] & !(15 << shift)) | (v & 15) << shift;
        true
    }

    pub fn light_sections(&self) -> usize {
        self.sections.len() + 2
    }
}

/// Splits a packed `BlockPos` long (x:26, z:26, y:12 bits).
pub fn unpack_block_pos(v: i64) -> [i32; 3] {
    [(v >> 38) as i32, ((v << 52) >> 52) as i32, ((v << 26) >> 38) as i32]
}

/// Splits a packed `SectionPos` long (x:22, z:22, y:20 bits).
pub fn unpack_section_pos(v: i64) -> [i32; 3] {
    [(v >> 42) as i32, ((v << 44) >> 44) as i32, ((v << 22) >> 42) as i32]
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
    fn skip_nbt(&mut self) -> Result<()> {
        let mut s = self.0;
        mc_nbt::read_network(&mut s)?;
        self.0 = s;
        Ok(())
    }
    fn byte_array(&mut self) -> Result<Vec<u8>> {
        let n = self.len()?;
        Ok(self.take(n)?.to_vec())
    }
    fn nibble_arrays(&mut self) -> Result<Vec<Nibbles>> {
        let n = self.len()?;
        (0..n)
            .map(|_| {
                let len = self.len()?;
                let bytes: [u8; 2048] = self.take(len)?.try_into().map_err(|_| bad("light array is not 2048 bytes"))?;
                Ok(Box::new(bytes))
            })
            .collect()
    }
    fn light(&mut self) -> Result<LightUpdate> {
        Ok(LightUpdate {
            sky_mask: self.byte_array()?,
            block_mask: self.byte_array()?,
            empty_sky: self.byte_array()?,
            empty_block: self.byte_array()?,
            sky: self.nibble_arrays()?,
            block: self.nibble_arrays()?,
        })
    }
}

/// Decodes the body of `light_update`: chunk x, z and the light payload.
pub fn decode_light_update(body: &[u8]) -> Result<(i32, i32, LightUpdate)> {
    let mut c = Cursor(body);
    let x = c.varint()?;
    let z = c.varint()?;
    Ok((x, z, c.light()?))
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
    // Block entities: packed xz, y, type id, optional NBT. Not used yet, but light data follows them.
    for _ in 0..c.len()? {
        c.u8()?;
        c.i16()?;
        c.varint()?;
        c.skip_nbt()?;
    }
    let mut chunk = Chunk { x, z, min_section, sections, light: Light::default() };
    let light_sections = chunk.light_sections();
    chunk.light.apply(c.light()?, light_sections);
    Ok(chunk)
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
        varint(&mut body, 0); // no block entities
        // Light: sky mask has light section 1 (= block section 0); block light all empty except nothing sent.
        varint(&mut body, 1); // sky mask: 1 byte
        body.push(0b10);
        varint(&mut body, 0); // block mask
        varint(&mut body, 0); // empty sky
        varint(&mut body, 0); // empty block
        varint(&mut body, 1); // one sky array
        varint(&mut body, 2048);
        let mut sky = [0u8; 2048];
        sky[0] = 0x07; // (0,0,0) = 7, (1,0,0) = 0
        body.extend_from_slice(&sky);
        varint(&mut body, 0); // no block arrays

        let mut chunk = decode_chunk(&body, -4, Registries { block_states: 35723, biomes: 65 }).unwrap();
        assert_eq!((chunk.x, chunk.z, chunk.sections.len()), (3, -2, 2));
        assert_eq!(chunk.get(5, -64, 5), 1);
        assert_eq!(chunk.get(1, -48, 0), 9);
        assert_eq!(chunk.get(0, -48, 0), 0);
        assert_eq!(chunk.get(0, 500, 0), 0);
        assert_eq!(chunk.light(0, -64, 0), (7, 0));
        assert_eq!(chunk.light(1, -64, 0), (0, 0));
        assert_eq!(chunk.light(0, -48, 0), (15, 0)); // no data -> open sky
        assert!(chunk.set(2, -64, 2, 5));
        assert_eq!((chunk.get(2, -64, 2), chunk.get(3, -64, 2)), (5, 1));
    }

    #[test]
    fn unpacks_positions() {
        // BlockPos.asLong(-5, 70, 12) and SectionPos.asLong(-2, 4, 3)
        let b = ((-5i64 & 0x3ff_ffff) << 38) | (70 & 0xfff) | ((12 & 0x3ff_ffff) << 12);
        assert_eq!(unpack_block_pos(b), [-5, 70, 12]);
        let s = ((-2i64 & 0x3f_ffff) << 42) | (4 & 0xf_ffff) | ((3 & 0x3f_ffff) << 20);
        assert_eq!(unpack_section_pos(s), [-2, 4, 3]);
    }

    #[test]
    fn ceil_log2_matches_mth() {
        assert_eq!(ceil_log2(35723), 16);
        assert_eq!(ceil_log2(65), 7);
        assert_eq!(ceil_log2(64), 6);
    }
}
