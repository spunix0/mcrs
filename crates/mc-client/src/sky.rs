//! Time of day and the sky, following vanilla: celestial angle, sky and fog colors, sunrise glow, how dark sky light
//! gets at night, and the sun, moon and stars as additive quads.

use glam::Vec3;
use mc_assets::GuiAtlas;
use mc_render::{Environment, Vertex};

/// Plains biome colors (real biome blending would need the biome palette).
const SKY: [f32; 3] = [0x78 as f32 / 255.0, 0xa7 as f32 / 255.0, 1.0];
const FOG: [f32; 3] = [0xc0 as f32 / 255.0, 0xd8 as f32 / 255.0, 1.0];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
}

impl Dimension {
    /// The folder its saves live in (`dimensions/minecraft/<this>`).
    pub fn folder(self) -> &'static str {
        match self {
            Dimension::Overworld => "overworld",
            Dimension::Nether => "the_nether",
            Dimension::End => "the_end",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name.strip_prefix("minecraft:").unwrap_or(name) {
            "the_nether" => Self::Nether,
            "the_end" => Self::End,
            _ => Self::Overworld,
        }
    }

    /// Lowest section index (the overworld goes down to Y -64, the others start at 0).
    pub fn min_section(self) -> i32 {
        if self == Self::Overworld { -4 } else { 0 }
    }
}

pub struct Sky {
    /// Day time in ticks (0 = sunrise + 1000, 6000 = noon), advanced locally between server updates.
    pub day_time: f64,
    /// Ticks per tick the clock runs at (0 when the daylight cycle is off).
    pub rate: f32,
    pub dimension: Dimension,
    stars: Vec<(Vec3, f32, f32)>,
}

/// `fract(t / 24000 - 0.25)` bent so days are a bit longer than nights, as `DimensionType.timeOfDay`.
pub fn time_of_day(day_time: f64) -> f32 {
    let f = (day_time / 24000.0 - 0.25).rem_euclid(1.0);
    let bent = 0.5 - (f * std::f64::consts::PI).cos() / 2.0;
    ((f * 2.0 + bent) / 3.0) as f32
}

impl Sky {
    pub fn new() -> Self {
        // Vanilla's star field: 1500 random points on a sphere (fixed seed), each a small rotated square.
        let mut seed = 10842u64;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut stars = Vec::new();
        while stars.len() < 1500 {
            let v = Vec3::new(rand() * 2.0 - 1.0, rand() * 2.0 - 1.0, rand() * 2.0 - 1.0);
            let size = 0.15 + rand() * 0.1;
            let l = v.length_squared();
            if l > 0.01 && l < 1.0 {
                stars.push((v.normalize(), size, rand() * std::f32::consts::TAU));
            }
        }
        Self { day_time: 6000.0, rate: 1.0, dimension: Dimension::Overworld, stars }
    }

    pub fn tick(&mut self) {
        self.day_time += self.rate as f64;
    }

    fn angle(&self) -> f32 {
        time_of_day(self.day_time)
    }

    /// Direction to the sun: rises in the east (+X), sets in the west.
    pub fn sun_dir(&self) -> Vec3 {
        let a = self.angle() * std::f32::consts::TAU;
        Vec3::new(-a.sin(), a.cos(), 0.0)
    }

    /// Sky light multiplier: vanilla's `getSkyDarken`, 0.2 at night to 1 at day.
    /// Ticks into the current day (0..24000), the `minecraft:day` timeline position.
    fn tod(&self) -> f32 {
        self.day_time.rem_euclid(24000.0) as f32
    }

    /// Sky light multiplier (26.3's `visual/sky_light_factor` track): 1 by day, 0.24 at night, 0 outside the overworld.
    pub fn daylight(&self) -> f32 {
        if self.dimension != Dimension::Overworld {
            return 0.0;
        }
        track(self.tod(), &[(730.0, [1.0]), (11270.0, [1.0]), (13140.0, [0.24]), (22860.0, [0.24])])[0]
    }

    pub fn environment(&self, gamma: f32) -> Environment {
        let rgb = |c: u32| [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0];
        let block_tint = rgb(0xffd88c);
        match self.dimension {
            Dimension::Nether => {
                let fog = [0.2, 0.03, 0.03];
                return Environment { sky: fog, fog, daylight: 0.0, sun: [0.0, 1.0, 0.0], sunrise: [0.0; 4], gamma, sky_light: rgb(0x7a7aff), ambient: rgb(0x302821), block_tint, fog_range: None };
            }
            Dimension::End => {
                let fog = [0.06, 0.04, 0.08];
                return Environment { sky: [0.09, 0.06, 0.11], fog, daylight: 0.0, sun: [0.0, 1.0, 0.0], sunrise: [0.0; 4], gamma, sky_light: rgb(0xac5fcd), ambient: rgb(0x3f473f), block_tint, fog_range: None };
            }
            Dimension::Overworld => {}
        }
        let t = self.tod();
        // The `minecraft:day` timeline tracks multiply the biome's sky and fog colors.
        let sky_mul = track(t, &[(133.0, [1.0; 3]), (11867.0, [1.0; 3]), (13670.0, [0.0; 3]), (22330.0, [0.0; 3])]);
        let fog_mul = track(t, &[(133.0, [1.0; 3]), (11867.0, [1.0; 3]), (13670.0, rgb(0x0f0f16)), (22330.0, rgb(0x0f0f16))]);
        let sky_light = track(t, &[(730.0, [1.0; 3]), (11270.0, [1.0; 3]), (13140.0, rgb(0x7a7aff)), (22860.0, rgb(0x7a7aff))]);
        let sky = [0, 1, 2].map(|i| SKY[i] * sky_mul[i]);
        let fog = [0, 1, 2].map(|i| FOG[i] * fog_mul[i]);
        let s = track(t, &SUNRISE.map(|(k, c)| (k, [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, (c >> 24) as f32 / 255.0])));
        Environment { sky, fog, daylight: self.daylight(), sun: self.sun_dir().into(), sunrise: s, gamma, sky_light, ambient: rgb(0x0a0a0a), block_tint, fog_range: None }
    }

    /// `visual/star_brightness`.
    fn stars(&self) -> f32 {
        const K: [(f32, [f32; 1]); 12] = [
            (92.0, [0.037]), (627.0, [0.0]), (11373.0, [0.0]), (11732.0, [0.016]), (11959.0, [0.044]), (12399.0, [0.143]),
            (12729.0, [0.258]), (13228.0, [0.5]), (22772.0, [0.5]), (23032.0, [0.364]), (23356.0, [0.225]), (23758.0, [0.101]),
        ];
        track(self.tod(), &K)[0]
    }

    /// Sun, moon and stars around the camera (only in the overworld).
    pub fn celestial(&self, eye: Vec3, gui: &GuiAtlas, rain: f32) -> Vec<Vertex> {
        let mut out = Vec::new();
        if self.dimension != Dimension::Overworld {
            return out;
        }
        let a = self.angle() * std::f32::consts::TAU;
        let sun = self.sun_dir();
        // The quad's axes: along the sun's path, and north-south.
        let side = Vec3::Z;
        let along = Vec3::new(-a.cos(), -a.sin(), 0.0);
        let vis = 1.0 - rain;
        let mut quad = |center: Vec3, half: f32, uv: [f32; 4], color: [u8; 4], flip: bool| {
            let [u0, v0, u1, v1] = uv;
            let (u0, u1) = if flip { (u1, u0) } else { (u0, u1) };
            let c = eye + center * 100.0;
            let p = |s: f32, t: f32| (c + side * s * half + along * t * half).into();
            let v = |pos, uv| Vertex { pos, uv, color };
            let q = [v(p(-1.0, -1.0), [u0, v0]), v(p(-1.0, 1.0), [u0, v1]), v(p(1.0, 1.0), [u1, v1]), v(p(1.0, -1.0), [u1, v0])];
            out.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        };
        let alpha = (vis * 255.0) as u8;
        quad(sun, 30.0, gui.sprite("sun").uv, [255, 255, 255, alpha], false);
        const PHASES: [&str; 8] = ["full_moon", "waning_gibbous", "third_quarter", "waning_crescent", "new_moon", "waxing_crescent", "first_quarter", "waxing_gibbous"];
        let phase = ((self.day_time / 24000.0).floor() as i64).rem_euclid(8) as usize;
        quad(-sun, 20.0, gui.sprite(&format!("moon_{}", PHASES[phase])).uv, [255, 255, 255, alpha], true);

        // Stars fade in at dusk; they turn with the sky.
        let star = self.stars() * vis;
        if star > 0.01 {
            let white = gui.sprite("white").uv;
            let mid = [(white[0] + white[2]) / 2.0, (white[1] + white[3]) / 2.0];
            let level = (star * 255.0) as u8;
            let (s, co) = a.sin_cos();
            for &(dir, size, spin) in &self.stars {
                // Rotate the field like the sun: about the Z axis by the celestial angle.
                let d = Vec3::new(dir.x * co - dir.y * s, dir.x * s + dir.y * co, dir.z);
                let up = if d.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
                let u = d.cross(up).normalize();
                let w = d.cross(u);
                let (ss, sc) = spin.sin_cos();
                let (u, w) = (u * sc + w * ss, w * sc - u * ss);
                let c = eye + d * 100.0;
                let v = |p: Vec3| Vertex { pos: p.into(), uv: mid, color: [level, level, level, 255] };
                let q = [v(c - u * size - w * size), v(c - u * size + w * size), v(c + u * size + w * size), v(c + u * size - w * size)];
                out.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_is_bright_and_midnight_dark() {
        let mut s = Sky::new();
        s.day_time = 6000.0;
        assert!((s.daylight() - 1.0).abs() < 1e-3);
        assert!(s.sun_dir().y > 0.99);
        s.day_time = 18000.0;
        assert!((s.daylight() - 0.24).abs() < 1e-3);
        // Halfway down the dusk ramp (11270 -> 13140).
        s.day_time = 12205.0;
        assert!((s.daylight() - 0.62).abs() < 1e-3);
        s.day_time = 18000.0;
        assert!(s.sun_dir().y < -0.99);
        // Sunrise is in the east.
        s.day_time = 0.0;
        assert!(s.sun_dir().x > 0.9);
    }
}

/// `visual/sunrise_sunset_color` keyframes (ticks, ARGB).
const SUNRISE: [(f32, u32); 32] = [
    (71.0, 0x5fefa333), (310.0, 0x29f5ba33), (565.0, 0x06fbd433), (730.0, 0x00ffe533), (11270.0, 0x00ffe533), (11397.0, 0x04fcd833),
    (11522.0, 0x0ff9cb33), (11690.0, 0x29f5ba33), (11929.0, 0x5fefa333), (12243.0, 0xb1e78733), (12358.0, 0xcce47e33), (12512.0, 0xe9e07233),
    (12613.0, 0xf6dd6b33), (12732.0, 0xfeda6333), (12841.0, 0xfed75c33), (13035.0, 0xecd25133), (13252.0, 0xc1cc4733), (13775.0, 0x36be3733),
    (13888.0, 0x1fbb3533), (14039.0, 0x09b73333), (14192.0, 0x00b33333), (21807.0, 0x00b23333), (21961.0, 0x09b73333), (22112.0, 0x1fbb3533),
    (22225.0, 0x36be3733), (22748.0, 0xc1cc4733), (22965.0, 0xecd25133), (23159.0, 0xfed75c33), (23272.0, 0xfeda6333), (23488.0, 0xe9e07233),
    (23642.0, 0xcce47e33), (23757.0, 0xb1e78733),
];

/// Linear keyframe interpolation over a 24000-tick day, wrapping from the last key to the first (timeline tracks).
fn track<const N: usize>(t: f32, keys: &[(f32, [f32; N])]) -> [f32; N] {
    let n = keys.len();
    let i = keys.iter().position(|k| k.0 > t).unwrap_or(n);
    let (a, b) = if i == 0 || i == n { (keys[n - 1], keys[0]) } else { (keys[i - 1], keys[i]) };
    let span = (b.0 - a.0).rem_euclid(24000.0).max(1.0);
    let f = ((t - a.0).rem_euclid(24000.0) / span).clamp(0.0, 1.0);
    std::array::from_fn(|j| a.1[j] + (b.1[j] - a.1[j]) * f)
}
