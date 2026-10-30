//! Sound playback from the game's own assets: `sounds.json` events, Ogg files via the asset index, simple
//! distance falloff and stereo panning, and background music.

use anyhow::{Context, Result};
use rodio::Source;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Sound categories in vanilla's `SoundSource` order (the server sends this index with each sound).
pub mod category {
    pub const MASTER: usize = 0;
    pub const MUSIC: usize = 1;
    pub const BLOCKS: usize = 4;
    pub const HOSTILE: usize = 5;
    pub const NEUTRAL: usize = 6;
    pub const PLAYERS: usize = 7;
    pub const AMBIENT: usize = 8;
    pub const UI: usize = 10;
    pub const COUNT: usize = 11;
    pub const NAMES: [&str; COUNT] = ["master", "music", "record", "weather", "block", "hostile", "neutral", "player", "ambient", "voice", "ui"];
}

#[derive(Clone)]
struct Variant {
    name: String,
    volume: f32,
    pitch: f32,
    weight: u32,
    /// Refers to another event instead of a file.
    event: bool,
}

pub struct Audio {
    sink: rodio::MixerDeviceSink,
    /// Asset path (`minecraft/sounds/dig/stone1.ogg`) to its file in `assets/objects`.
    files: HashMap<String, PathBuf>,
    events: HashMap<String, Vec<Variant>>,
    cache: HashMap<PathBuf, std::sync::Arc<Decoded>>,
    rng: u64,
    listener: [f64; 3],
    /// Unit vector to the listener's right.
    right: [f64; 3],
    music: Option<rodio::Player>,
    /// Which music event is playing (`music.menu`, `music.game`).
    music_kind: &'static str,
    music_gain: f32,
    next_music: Instant,
    /// A looping ambience (underwater) and its event.
    ambient: Option<(&'static str, rodio::Player)>,
    /// Volume per category, 0..1 (from the options).
    pub volumes: [f32; category::COUNT],
}

fn object_path(objects: &Path, hash: &str) -> PathBuf {
    objects.join(&hash[..2]).join(hash)
}

impl Audio {
    /// `data` is the launcher data folder (versions/, assets/).
    pub fn new(data: &Path, version: &str) -> Result<Self> {
        let version_json: Value = serde_json::from_slice(&std::fs::read(data.join("versions").join(version).join(format!("{version}.json")))?)?;
        let index_id = version_json["assetIndex"]["id"].as_str().context("no asset index")?;
        let index: Value = serde_json::from_slice(&std::fs::read(data.join("assets/indexes").join(format!("{index_id}.json")))?)?;
        let objects = data.join("assets/objects");
        let files: HashMap<String, PathBuf> = index["objects"]
            .as_object()
            .context("bad asset index")?
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), object_path(&objects, v["hash"].as_str()?))))
            .collect();
        let sounds: Value = serde_json::from_slice(&std::fs::read(files.get("minecraft/sounds.json").context("sounds.json missing")?)?)?;
        let mut events = HashMap::new();
        for (name, def) in sounds.as_object().context("bad sounds.json")? {
            let variants = def["sounds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| match s {
                    Value::String(n) => Some(Variant { name: n.clone(), volume: 1.0, pitch: 1.0, weight: 1, event: false }),
                    Value::Object(o) => Some(Variant {
                        name: o.get("name")?.as_str()?.to_string(),
                        volume: o.get("volume").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                        pitch: o.get("pitch").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                        weight: o.get("weight").and_then(Value::as_u64).unwrap_or(1) as u32,
                        event: o.get("type").and_then(Value::as_str) == Some("event"),
                    }),
                    _ => None,
                })
                .collect::<Vec<_>>();
            events.insert(name.clone(), variants);
        }
        let sink = rodio::DeviceSinkBuilder::open_default_sink().context("no audio output device")?;
        Ok(Self {
            sink,
            files,
            events,
            cache: HashMap::new(),
            rng: 0x9e37_79b9_7f4a_7c15 ^ Instant::now().elapsed().as_nanos() as u64,
            listener: [0.0; 3],
            right: [1.0, 0.0, 0.0],
            music: None,
            music_kind: "",
            music_gain: 1.0,
            next_music: Instant::now(),
            ambient: None,
            volumes: [1.0; category::COUNT],
        })
    }

    fn random(&mut self) -> u32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }

    pub fn set_listener(&mut self, eye: [f64; 3], yaw_degrees: f32) {
        self.listener = eye;
        let y = (yaw_degrees as f64).to_radians();
        // Minecraft yaw 0 faces +Z; the right ear is toward -X.
        self.right = [-y.cos(), 0.0, -y.sin()];
    }

    /// Picks a concrete file for an event, following `"type": "event"` references.
    fn pick(&mut self, event: &str, depth: u32) -> Option<(String, f32, f32)> {
        let name = event.strip_prefix("minecraft:").unwrap_or(event);
        let variants = self.events.get(name)?.clone();
        let total: u32 = variants.iter().map(|v| v.weight.max(1)).sum();
        if total == 0 {
            return None;
        }
        let mut roll = self.random() % total;
        let v = variants.iter().find(|v| {
            let w = v.weight.max(1);
            if roll < w {
                true
            } else {
                roll -= w;
                false
            }
        })?;
        if v.event {
            if depth > 4 {
                return None;
            }
            let (f, vol, pitch) = self.pick(&v.name, depth + 1)?;
            return Some((f, vol * v.volume, pitch * v.pitch));
        }
        Some((v.name.clone(), v.volume, v.pitch))
    }

    /// A sound file decoded once and kept (footsteps and digging repeat a lot).
    fn decoded(&mut self, file: &str) -> Option<std::sync::Arc<Decoded>> {
        let path = file.strip_prefix("minecraft:").unwrap_or(file);
        let disk = self.files.get(&format!("minecraft/sounds/{path}.ogg"))?.clone();
        if let Some(d) = self.cache.get(&disk) {
            return Some(d.clone());
        }
        let d = std::sync::Arc::new(decode(std::fs::File::open(&disk).ok()?)?);
        self.cache.insert(disk, d.clone());
        Some(d)
    }

    /// Plays a sound event at a world position (or at the listener when `None`), like `SoundManager.play`.
    /// `category` is a [`category`] index; its volume and the master volume scale the sound.
    pub fn play(&mut self, event: &str, at: Option<[f64; 3]>, volume: f32, pitch: f32, category: usize) {
        let mix = self.volumes[category::MASTER] * self.volumes.get(category).copied().unwrap_or(1.0);
        if mix <= 0.0 {
            if std::env::var_os("MCRS_DEBUG_SOUND").is_some() {
                eprintln!("sound {event} muted (category {category})");
            }
            return;
        }
        let Some((file, v_vol, v_pitch)) = self.pick(event, 0) else { return };
        let volume = (volume * v_vol).clamp(0.0, 1.0) * mix;
        let pitch = (pitch * v_pitch).clamp(0.5, 2.0);
        // Linear falloff to 16 blocks, scaled by volumes over 1 (vanilla's attenuation distance).
        let (gain, pan) = match at {
            None => (volume, 0.0),
            Some(p) => {
                let d = [p[0] - self.listener[0], p[1] - self.listener[1], p[2] - self.listener[2]];
                let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let range = 16.0 * volume.max(1.0) as f64;
                if dist >= range {
                    return;
                }
                let pan = if dist > 0.01 { (d[0] * self.right[0] + d[2] * self.right[2]) / dist } else { 0.0 };
                (volume.min(1.0) * (1.0 - dist / range) as f32, pan as f32)
            }
        };
        if std::env::var_os("MCRS_DEBUG_SOUND").is_some() {
            eprintln!("sound {event} file {file} gain {gain:.2} pan {pan:.2}");
        }
        let Some(decoded) = self.decoded(&file) else { return };
        self.sink.mixer().add(effect(&decoded, pitch, gain, pan));
    }

    /// Keeps a looping ambience (`ambient.underwater.loop`) playing, or stops it with `None`.
    pub fn tick_ambient(&mut self, event: Option<&'static str>) {
        if self.ambient.as_ref().map(|a| a.0) != event {
            self.ambient = None;
        }
        let Some(event) = event else { return };
        if self.ambient.as_ref().is_some_and(|a| !a.1.empty()) {
            return;
        }
        let mix = self.volumes[category::MASTER] * self.volumes[category::AMBIENT];
        let Some((file, vol, pitch)) = self.pick(event, 0) else { return };
        let Some(d) = self.decoded(&file) else { return };
        let player = self.ambient.take().map_or_else(|| rodio::Player::connect_new(self.sink.mixer()), |a| a.1);
        player.append(effect(&d, pitch, vol * mix, 0.0));
        self.ambient = Some((event, player));
    }

    /// Keeps the music volume in line with the options.
    pub fn update_music_volume(&mut self) {
        let v = self.volumes[category::MASTER] * self.volumes[category::MUSIC] * self.music_gain;
        if let Some(m) = &self.music {
            m.set_volume(v);
        }
    }

    /// Starts a new background track of `kind` (`music.menu` or `music.game`) when the last one finished and the
    /// pause is over. Switching kind (entering or leaving a world) stops the current track.
    pub fn tick_music(&mut self, kind: &'static str) {
        if kind != self.music_kind {
            self.music = None;
            self.music_kind = kind;
            // The menu starts its music right away; in game vanilla waits a while before the first track.
            self.next_music = Instant::now() + if kind == "music.menu" { Duration::from_secs(1) } else { Duration::from_secs(30) };
        }
        if self.music.as_ref().is_some_and(|m| !m.empty()) || Instant::now() < self.next_music {
            return;
        }
        if self.music.take().is_some() {
            // Vanilla pauses minutes between tracks.
            self.next_music = Instant::now() + Duration::from_secs(120 + (self.random() % 240) as u64);
            return;
        }
        let Some((file, vol, _)) = self.pick(kind, 0) else { return };
        let path = file.strip_prefix("minecraft:").unwrap_or(&file);
        let Some(disk) = self.files.get(&format!("minecraft/sounds/{path}.ogg")).cloned() else { return };
        let Ok(f) = std::fs::File::open(disk) else { return };
        let Ok(decoder) = rodio::Decoder::try_from(f) else { return };
        let player = rodio::Player::connect_new(self.sink.mixer());
        self.music_gain = vol;
        player.append(decoder);
        self.music = Some(player);
        self.update_music_volume();
    }
}

/// Decoded samples of one sound file.
pub struct Decoded {
    channels: rodio::ChannelCount,
    rate: rodio::SampleRate,
    samples: Vec<f32>,
}

/// Decodes a whole file. Effects play from these buffers: rodio 0.23's Vorbis decoder goes silent when the mixer
/// resamples it (mono 44.1 kHz into the device format), but a sample buffer converts fine.
fn decode(r: impl std::io::Read + std::io::Seek + Send + Sync + 'static) -> Option<Decoded> {
    let d = rodio::Decoder::new(std::io::BufReader::new(r)).ok()?;
    let (channels, rate) = (d.channels(), d.sample_rate());
    Some(Decoded { channels, rate, samples: d.collect() })
}

/// A sound effect ready for the mixer: pitched, scaled and panned.
fn effect(d: &Decoded, pitch: f32, gain: f32, pan: f32) -> impl Source + Send + 'static {
    let buf = rodio::buffer::SamplesBuffer::new(d.channels, d.rate, d.samples.clone());
    // Pan with constant total loudness: ears offset left/right of a unit-distance emitter.
    rodio::source::Spatial::new(buf.speed(pitch).amplify(gain), [pan, 0.0, 1.0], [-0.5, 0.0, 0.0], [0.5, 0.0, 0.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real block sound from the installed assets must come out of the mixer audible (it used to be silent).
    #[test]
    fn effects_are_audible() {
        let data = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("mcrs").join("data");
        let Ok(index) = std::fs::read_to_string(data.join("assets/indexes/34.json")) else { return };
        let index: Value = serde_json::from_str(&index).unwrap();
        let hash = index["objects"]["minecraft/sounds/dig/stone1.ogg"]["hash"].as_str().unwrap().to_string();
        let d = decode(std::fs::File::open(object_path(&data.join("assets/objects"), &hash)).unwrap()).unwrap();
        // Through a device-style mixer (48 kHz stereo), as play() sends it.
        let (mixer, out) = rodio::mixer::mixer(std::num::NonZero::new(2).unwrap(), std::num::NonZero::new(48_000).unwrap());
        mixer.add(effect(&d, 0.8, 0.5, 0.3));
        let peak = out.take(40_000).fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.01, "peak {peak}");
    }
}

