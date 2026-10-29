//! Sound playback from the game's own assets: `sounds.json` events, Ogg files via the asset index, simple
//! distance falloff and stereo panning, and background music.

use anyhow::{Context, Result};
use rodio::Source;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MUSIC_VOLUME: f32 = 0.35;

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
    cache: HashMap<PathBuf, std::sync::Arc<[u8]>>,
    rng: u64,
    listener: [f64; 3],
    /// Unit vector to the listener's right.
    right: [f64; 3],
    music: Option<rodio::Player>,
    next_music: Instant,
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
            // Vanilla waits a while before the first track.
            next_music: Instant::now() + Duration::from_secs(20),
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

    fn bytes(&mut self, file: &str) -> Option<std::sync::Arc<[u8]>> {
        let path = file.strip_prefix("minecraft:").unwrap_or(file);
        let disk = self.files.get(&format!("minecraft/sounds/{path}.ogg"))?.clone();
        if let Some(b) = self.cache.get(&disk) {
            return Some(b.clone());
        }
        let b: std::sync::Arc<[u8]> = std::fs::read(&disk).ok()?.into();
        self.cache.insert(disk, b.clone());
        Some(b)
    }

    /// Plays a sound event at a world position (or at the listener when `None`), like `SoundManager.play`.
    pub fn play(&mut self, event: &str, at: Option<[f64; 3]>, volume: f32, pitch: f32) {
        let Some((file, v_vol, v_pitch)) = self.pick(event, 0) else { return };
        let volume = (volume * v_vol).clamp(0.0, 1.0);
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
        let Some(bytes) = self.bytes(&file) else { return };
        let Ok(decoder) = rodio::Decoder::try_from(std::io::Cursor::new(bytes)) else { return };
        // Pan with constant total loudness: ears offset left/right of a unit-distance emitter.
        let emitter = [pan, 0.0, 1.0];
        let source = rodio::source::Spatial::new(decoder.speed(pitch).amplify(gain), emitter, [-0.5, 0.0, 0.0], [0.5, 0.0, 0.0]);
        self.sink.mixer().add(source);
    }

    /// Starts a new background track when the last one finished and the pause is over.
    pub fn tick_music(&mut self) {
        if self.music.as_ref().is_some_and(|m| !m.empty()) || Instant::now() < self.next_music {
            return;
        }
        if self.music.take().is_some() {
            // Vanilla pauses minutes between tracks.
            self.next_music = Instant::now() + Duration::from_secs(120 + (self.random() % 240) as u64);
            return;
        }
        let Some((file, vol, _)) = self.pick("music.game", 0) else { return };
        let path = file.strip_prefix("minecraft:").unwrap_or(&file);
        let Some(disk) = self.files.get(&format!("minecraft/sounds/{path}.ogg")).cloned() else { return };
        let Ok(f) = std::fs::File::open(disk) else { return };
        let Ok(decoder) = rodio::Decoder::try_from(f) else { return };
        let player = rodio::Player::connect_new(self.sink.mixer());
        player.set_volume(MUSIC_VOLUME * vol);
        player.append(decoder);
        self.music = Some(player);
    }
}
