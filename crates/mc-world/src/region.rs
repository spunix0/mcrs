//! Reads saved worlds' region files (`r.<x>.<z>.mca`, 32x32 chunks each) for distant terrain: per column, the top
//! block (from the `MOTION_BLOCKING` heightmap, so water and leaves count) and its height.

use mc_nbt::Tag;
use std::io::Read;
use std::path::Path;

/// The surface of one region: for each of its 512x512 columns (`z * 512 + x`) the top block's Y, or `i16::MIN`
/// where the chunk isn't generated, and an index into `names`.
#[derive(Clone)]
pub struct Surface {
    pub heights: Vec<i16>,
    pub blocks: Vec<u16>,
    /// Block names (no namespace) the columns point into.
    pub names: Vec<String>,
}

pub const NONE: i16 = i16::MIN;

/// Reads `path`; missing or unreadable chunks stay empty.
pub fn read_surface(path: &Path) -> std::io::Result<Surface> {
    let file = std::fs::read(path)?;
    let mut s = Surface::empty();
    if file.len() < 8192 {
        return Ok(s);
    }
    for i in 0..1024 {
        let loc = u32::from_be_bytes([0, file[i * 4], file[i * 4 + 1], file[i * 4 + 2]]) as usize * 4096;
        if loc == 0 || loc + 5 > file.len() {
            continue;
        }
        let len = u32::from_be_bytes(file[loc..loc + 4].try_into().unwrap()) as usize;
        let Some(body) = file.get(loc + 5..loc + 4 + len) else { continue };
        let mut nbt = Vec::new();
        let ok = match file[loc + 4] {
            1 => flate2::read::GzDecoder::new(body).read_to_end(&mut nbt).is_ok(),
            2 => flate2::read::ZlibDecoder::new(body).read_to_end(&mut nbt).is_ok(),
            3 => {
                nbt.extend_from_slice(body);
                true
            }
            // shortcut: LZ4 (type 4) and external (128+) chunks are skipped; add when a world uses them.
            _ => false,
        };
        let Ok((_, chunk)) = (if ok { mc_nbt::read_named(&mut nbt.as_slice()) } else { continue }) else { continue };
        column_tops(&chunk, (i % 32) * 16, (i / 32) * 16, &mut s);
    }
    Ok(s)
}

impl Surface {
    pub fn empty() -> Self {
        Self { heights: vec![NONE; 512 * 512], blocks: vec![0; 512 * 512], names: Vec::new() }
    }

    /// Records one column (region-relative `x`, `z`): its top block's Y and name.
    pub fn set(&mut self, x: usize, z: usize, y: i16, name: &str) {
        let id = match self.names.iter().position(|n| n == name) {
            Some(i) => i,
            None => {
                self.names.push(name.to_string());
                self.names.len() - 1
            }
        };
        self.heights[z * 512 + x] = y;
        self.blocks[z * 512 + x] = id as u16;
    }

    /// The client's own cache format (`.lod`): zlib of the names, then every height and block index.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut raw = Vec::with_capacity(512 * 512 * 4 + 1024);
        raw.extend_from_slice(&(self.names.len() as u16).to_le_bytes());
        for n in &self.names {
            raw.push(n.len().min(255) as u8);
            raw.extend_from_slice(&n.as_bytes()[..n.len().min(255)]);
        }
        for (h, b) in self.heights.iter().zip(&self.blocks) {
            raw.extend_from_slice(&h.to_le_bytes());
            raw.extend_from_slice(&b.to_le_bytes());
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        z.write_all(&raw)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, z.finish()?)
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(std::fs::File::open(path)?).read_to_end(&mut raw)?;
        let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "bad lod cache file");
        let mut s = Self::empty();
        let mut at = 2;
        let count = u16::from_le_bytes(raw.get(0..2).ok_or_else(bad)?.try_into().unwrap());
        for _ in 0..count {
            let len = *raw.get(at).ok_or_else(bad)? as usize;
            s.names.push(String::from_utf8_lossy(raw.get(at + 1..at + 1 + len).ok_or_else(bad)?).into_owned());
            at += 1 + len;
        }
        let cols = raw.get(at..at + 512 * 512 * 4).ok_or_else(bad)?;
        for (i, c) in cols.chunks_exact(4).enumerate() {
            s.heights[i] = i16::from_le_bytes([c[0], c[1]]);
            s.blocks[i] = u16::from_le_bytes([c[2], c[3]]);
        }
        Ok(s)
    }
}

/// A region file (`.mca`) or a client cache file (`.lod`).
pub fn read_any(path: &Path) -> std::io::Result<Surface> {
    if path.extension().is_some_and(|e| e == "lod") { Surface::load(path) } else { read_surface(path) }
}

fn longs(t: Option<&Tag>) -> &[i64] {
    match t {
        Some(Tag::LongArray(v)) => v,
        _ => &[],
    }
}

fn int(t: Option<&Tag>) -> Option<i64> {
    match t? {
        Tag::Byte(v) => Some(*v as i64),
        Tag::Short(v) => Some(*v as i64),
        Tag::Int(v) => Some(*v as i64),
        Tag::Long(v) => Some(*v),
        _ => None,
    }
}

/// Packed values that don't span longs (since 1.16): `bits` each, `64 / bits` per long.
fn packed(data: &[i64], bits: usize, i: usize) -> u64 {
    let per = 64 / bits;
    data.get(i / per).map_or(0, |l| (*l as u64 >> ((i % per) * bits)) & ((1 << bits) - 1))
}

fn column_tops(chunk: &Tag, ox: usize, oz: usize, s: &mut Surface) {
    if chunk.get("Status").and_then(Tag::as_str).is_some_and(|st| !st.ends_with("full")) {
        return;
    }
    let min_y = int(chunk.get("yPos")).unwrap_or(-4) * 16;
    let heights = longs(chunk.get("Heightmaps").and_then(|h| h.get("MOTION_BLOCKING")));
    let Some(sections) = chunk.get("sections").and_then(Tag::as_list) else { return };
    if heights.is_empty() {
        return;
    }
    for z in 0..16 {
        for x in 0..16 {
            // The heightmap holds the first free Y above the column, counted from the world's bottom.
            let top = packed(heights, 9, z * 16 + x) as i64 + min_y - 1;
            if top < min_y {
                continue;
            }
            let Some(sec) = sections.iter().find(|t| int(t.get("Y")) == Some(top.div_euclid(16))) else { continue };
            let Some(states) = sec.get("block_states") else { continue };
            let Some(palette) = states.get("palette").and_then(Tag::as_list) else { continue };
            let data = longs(states.get("data"));
            let idx = if palette.len() <= 1 || data.is_empty() {
                0
            } else {
                let bits = (usize::BITS - (palette.len() - 1).leading_zeros()).max(4) as usize;
                let ly = top.rem_euclid(16) as usize;
                packed(data, bits, (ly << 8) | (z << 4) | x) as usize
            };
            // 26.x writes a block without properties as a bare name; in a list mixed with compounds that's `{"": name}`.
            let entry = palette.get(idx);
            let Some(name) = entry.and_then(|p| p.get("Name").or_else(|| p.get("")).and_then(Tag::as_str).or_else(|| p.as_str())) else { continue };
            let name = name.strip_prefix("minecraft:").unwrap_or(name);
            let id = match s.names.iter().position(|n| n == name) {
                Some(i) => i,
                None => {
                    s.names.push(name.to_string());
                    s.names.len() - 1
                }
            };
            let col = (oz + z) * 512 + ox + x;
            s.heights[col] = top as i16;
            s.blocks[col] = id as u16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_files_round_trip() {
        let mut s = Surface::empty();
        s.set(3, 7, 70, "grass_block");
        s.set(511, 511, -20, "deepslate");
        let path = std::env::temp_dir().join(format!("mcrs-lod-{}.lod", std::process::id()));
        s.save(&path).unwrap();
        let back = read_any(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.heights[7 * 512 + 3], 70);
        assert_eq!(back.names[back.blocks[511 * 512 + 511] as usize], "deepslate");
        assert_eq!(back.heights[0], NONE);
    }

    #[test]
    fn packed_values_stay_within_longs() {
        // 9-bit heights, 7 to a long: the 8th value starts the next long.
        let data = [(5i64) | (300 << 9) | (7 << 54), 42];
        assert_eq!(packed(&data, 9, 0), 5);
        assert_eq!(packed(&data, 9, 1), 300);
        assert_eq!(packed(&data, 9, 6), 7);
        assert_eq!(packed(&data, 9, 7), 42);
    }

    /// Reads a region of the installed singleplayer world, if there is one, and checks the surface is plausible.
    #[test]
    fn reads_a_saved_region() {
        let dir = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("mcrs/data/worlds/New World/world/dimensions/minecraft/overworld/region");
        let Ok(files) = std::fs::read_dir(&dir) else { return };
        let mut total = 0;
        for file in files.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "mca")) {
            let s = read_surface(&file.path()).unwrap();
            let known: Vec<i16> = s.heights.iter().copied().filter(|h| *h != NONE).collect();
            println!("{}: {} columns, {} block kinds: {:?}", file.path().display(), known.len(), s.names.len(), &s.names[..s.names.len().min(10)]);
            assert!(known.iter().all(|h| (-64..320).contains(h)));
            total += known.len();
        }
        assert!(total > 0, "no generated terrain found");
    }
}
