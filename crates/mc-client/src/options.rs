//! Player options, saved as `key:value` lines like vanilla's `options.txt` (in `mcrs-options.txt` next to the game data).

use crate::audio::category;
use std::path::{Path, PathBuf};
use winit::keyboard::KeyCode;

/// Rebindable actions, in the order the Key Binds screen lists them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Sneak,
    Sprint,
    Inventory,
    Drop,
    Chat,
    Command,
    PlayerList,
    Fly,
    PickBlock,
}

impl Action {
    pub const ALL: [Action; 14] = [
        Action::Forward,
        Action::Back,
        Action::Left,
        Action::Right,
        Action::Jump,
        Action::Sneak,
        Action::Sprint,
        Action::Inventory,
        Action::Drop,
        Action::Chat,
        Action::Command,
        Action::PlayerList,
        Action::Fly,
        Action::PickBlock,
    ];

    /// Vanilla's option key (`key_key.forward`) and display name.
    pub fn info(self) -> (&'static str, &'static str, KeyCode) {
        match self {
            Action::Forward => ("key.forward", "Walk Forwards", KeyCode::KeyW),
            Action::Back => ("key.back", "Walk Backwards", KeyCode::KeyS),
            Action::Left => ("key.left", "Strafe Left", KeyCode::KeyA),
            Action::Right => ("key.right", "Strafe Right", KeyCode::KeyD),
            Action::Jump => ("key.jump", "Jump", KeyCode::Space),
            Action::Sneak => ("key.sneak", "Sneak", KeyCode::ShiftLeft),
            Action::Sprint => ("key.sprint", "Sprint", KeyCode::ControlLeft),
            Action::Inventory => ("key.inventory", "Open/Close Inventory", KeyCode::KeyE),
            Action::Drop => ("key.drop", "Drop Selected Item", KeyCode::KeyQ),
            Action::Chat => ("key.chat", "Open Chat", KeyCode::KeyT),
            Action::Command => ("key.command", "Open Command", KeyCode::Slash),
            Action::PlayerList => ("key.playerlist", "List Players", KeyCode::Tab),
            Action::Fly => ("key.mcrs.fly", "Toggle Flying", KeyCode::KeyF),
            Action::PickBlock => ("key.pickItem", "Pick Block (key)", KeyCode::KeyR),
        }
    }
}

/// Keys that can be bound, for reading names back from the options file.
const KEYS: &[KeyCode] = &[
    KeyCode::KeyA, KeyCode::KeyB, KeyCode::KeyC, KeyCode::KeyD, KeyCode::KeyE, KeyCode::KeyF, KeyCode::KeyG, KeyCode::KeyH,
    KeyCode::KeyI, KeyCode::KeyJ, KeyCode::KeyK, KeyCode::KeyL, KeyCode::KeyM, KeyCode::KeyN, KeyCode::KeyO, KeyCode::KeyP,
    KeyCode::KeyQ, KeyCode::KeyR, KeyCode::KeyS, KeyCode::KeyT, KeyCode::KeyU, KeyCode::KeyV, KeyCode::KeyW, KeyCode::KeyX,
    KeyCode::KeyY, KeyCode::KeyZ, KeyCode::Digit0, KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4,
    KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9, KeyCode::Space, KeyCode::ShiftLeft,
    KeyCode::ShiftRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight, KeyCode::Tab,
    KeyCode::CapsLock, KeyCode::Enter, KeyCode::Backspace, KeyCode::Slash, KeyCode::Backslash, KeyCode::Comma, KeyCode::Period,
    KeyCode::Semicolon, KeyCode::Quote, KeyCode::BracketLeft, KeyCode::BracketRight, KeyCode::Minus, KeyCode::Equal,
    KeyCode::Backquote, KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::Insert,
    KeyCode::Delete, KeyCode::Home, KeyCode::End, KeyCode::PageUp, KeyCode::PageDown, KeyCode::F6, KeyCode::F7, KeyCode::F8,
    KeyCode::F9, KeyCode::F10, KeyCode::F11, KeyCode::F12, KeyCode::Numpad0, KeyCode::Numpad1, KeyCode::Numpad2,
    KeyCode::Numpad3, KeyCode::Numpad4, KeyCode::Numpad5, KeyCode::Numpad6, KeyCode::Numpad7, KeyCode::Numpad8, KeyCode::Numpad9,
];

pub fn key_name(k: KeyCode) -> String {
    let s = format!("{k:?}");
    let s = s.strip_prefix("Key").or_else(|| s.strip_prefix("Digit")).unwrap_or(&s).to_string();
    match s.as_str() {
        "ShiftLeft" => "Left Shift".into(),
        "ShiftRight" => "Right Shift".into(),
        "ControlLeft" => "Left Control".into(),
        "ControlRight" => "Right Control".into(),
        "AltLeft" => "Left Alt".into(),
        "AltRight" => "Right Alt".into(),
        "ArrowUp" => "Up".into(),
        "ArrowDown" => "Down".into(),
        "ArrowLeft" => "Left".into(),
        "ArrowRight" => "Right".into(),
        _ => s,
    }
}

fn parse_key(s: &str) -> Option<KeyCode> {
    KEYS.iter().copied().find(|k| format!("{k:?}") == s)
}

#[derive(Clone, PartialEq)]
pub struct Options {
    pub fov: f32,
    pub render_distance: u8,
    /// 260 = unlimited.
    pub max_fps: u32,
    pub vsync: bool,
    /// 0 = auto.
    pub gui_scale: u32,
    /// Vanilla's "Brightness" (gamma) 0..1: Moody .. Bright.
    pub gamma: f32,
    pub view_bobbing: bool,
    pub fullscreen: bool,
    pub clouds: bool,
    /// 0..1, vanilla's mouse sensitivity (0.5 = 100%).
    pub sensitivity: f32,
    pub invert_mouse: bool,
    pub controller_sensitivity: f32,
    pub volumes: [f32; category::COUNT],
    pub chat_opacity: f32,
    pub chat_background: f32,
    pub chat_scale: f32,
    pub hide_splashes: bool,
    pub attack_indicator: bool,
    /// Built-in performance mod: skip sections hidden behind terrain, and entities off screen or hidden.
    pub cave_culling: bool,
    pub entity_culling: bool,
    pub pause_on_lost_focus: bool,
    pub fps_graph: bool,
    pub keys: Vec<KeyCode>,
    pub last_server: String,
    #[allow(dead_code)]
    path: PathBuf,
}

impl Options {
    pub fn defaults(path: PathBuf) -> Self {
        Self {
            fov: 70.0,
            render_distance: 8,
            max_fps: 260,
            vsync: true,
            gui_scale: 0,
            gamma: 0.5,
            view_bobbing: true,
            fullscreen: false,
            clouds: true,
            sensitivity: 0.5,
            invert_mouse: false,
            controller_sensitivity: 0.5,
            volumes: [1.0; category::COUNT],
            chat_opacity: 1.0,
            chat_background: 0.5,
            chat_scale: 1.0,
            hide_splashes: false,
            attack_indicator: true,
            cave_culling: true,
            entity_culling: true,
            pause_on_lost_focus: true,
            fps_graph: true,
            keys: Action::ALL.iter().map(|a| a.info().2).collect(),
            last_server: String::new(),
            path,
        }
    }

    pub fn load(data: &Path) -> Self {
        let path = data.join("mcrs-options.txt");
        let mut o = Self::defaults(path.clone());
        let Ok(text) = std::fs::read_to_string(&path) else { return o };
        for line in text.lines() {
            let Some((k, v)) = line.split_once(':') else { continue };
            let f = v.parse::<f32>().ok();
            let b = v == "true";
            match k {
                "fov" => o.fov = f.unwrap_or(70.0).clamp(30.0, 110.0),
                "renderDistance" => o.render_distance = v.parse().unwrap_or(8u8).clamp(2, 32),
                "maxFps" => o.max_fps = v.parse().unwrap_or(260u32).clamp(10, 260),
                "enableVsync" => o.vsync = b,
                "guiScale" => o.gui_scale = v.parse().unwrap_or(0),
                "gamma" => o.gamma = f.unwrap_or(0.5).clamp(0.0, 1.0),
                "bobView" => o.view_bobbing = b,
                "fullscreen" => o.fullscreen = b,
                "renderClouds" => o.clouds = v != "\"false\"" && v != "false",
                "mouseSensitivity" => o.sensitivity = f.unwrap_or(0.5).clamp(0.0, 1.0),
                "invertYMouse" => o.invert_mouse = b,
                "mcrs.controllerSensitivity" => o.controller_sensitivity = f.unwrap_or(0.5).clamp(0.0, 1.0),
                "chatOpacity" => o.chat_opacity = f.unwrap_or(1.0).clamp(0.0, 1.0),
                "textBackgroundOpacity" => o.chat_background = f.unwrap_or(0.5).clamp(0.0, 1.0),
                "chatScale" => o.chat_scale = f.unwrap_or(1.0).clamp(0.0, 1.0),
                "hideSplashTexts" => o.hide_splashes = b,
                "attackIndicator" => o.attack_indicator = v != "0",
                "lastServer" => o.last_server = v.to_string(),
                "mcrs.caveCulling" => o.cave_culling = b,
                "mcrs.entityCulling" => o.entity_culling = b,
                "pauseOnLostFocus" => o.pause_on_lost_focus = b,
                "mcrs.fpsGraph" => o.fps_graph = b,
                _ => {
                    if let Some(cat) = k.strip_prefix("soundCategory_") {
                        if let Some(i) = category::NAMES.iter().position(|n| *n == cat) {
                            o.volumes[i] = f.unwrap_or(1.0).clamp(0.0, 1.0);
                        }
                    } else if let Some(name) = k.strip_prefix("key_") {
                        if let (Some(i), Some(code)) = (Action::ALL.iter().position(|a| a.info().0 == name), parse_key(v)) {
                            o.keys[i] = code;
                        }
                    }
                }
            }
        }
        o
    }

    pub fn save(&self) {
        let mut s = String::new();
        let mut put = |k: &str, v: String| {
            s.push_str(k);
            s.push(':');
            s.push_str(&v);
            s.push('\n');
        };
        put("fov", self.fov.to_string());
        put("renderDistance", self.render_distance.to_string());
        put("maxFps", self.max_fps.to_string());
        put("enableVsync", self.vsync.to_string());
        put("guiScale", self.gui_scale.to_string());
        put("gamma", self.gamma.to_string());
        put("bobView", self.view_bobbing.to_string());
        put("fullscreen", self.fullscreen.to_string());
        put("renderClouds", self.clouds.to_string());
        put("mouseSensitivity", self.sensitivity.to_string());
        put("invertYMouse", self.invert_mouse.to_string());
        put("mcrs.controllerSensitivity", self.controller_sensitivity.to_string());
        put("chatOpacity", self.chat_opacity.to_string());
        put("textBackgroundOpacity", self.chat_background.to_string());
        put("chatScale", self.chat_scale.to_string());
        put("hideSplashTexts", self.hide_splashes.to_string());
        put("attackIndicator", (self.attack_indicator as u8).to_string());
        put("lastServer", self.last_server.clone());
        put("mcrs.caveCulling", self.cave_culling.to_string());
        put("mcrs.entityCulling", self.entity_culling.to_string());
        put("pauseOnLostFocus", self.pause_on_lost_focus.to_string());
        put("mcrs.fpsGraph", self.fps_graph.to_string());
        for (i, v) in self.volumes.iter().enumerate() {
            put(&format!("soundCategory_{}", category::NAMES[i]), v.to_string());
        }
        for (a, k) in Action::ALL.iter().zip(&self.keys) {
            put(&format!("key_{}", a.info().0), format!("{k:?}"));
        }
        let _ = std::fs::write(&self.path, s);
    }

    pub fn key(&self, a: Action) -> KeyCode {
        self.keys[Action::ALL.iter().position(|x| *x == a).unwrap_or(0)]
    }

    /// Degrees of turn per mouse count, vanilla's `(s * 0.6 + 0.2)^3 * 8 * 0.15`.
    pub fn mouse_factor(&self) -> f32 {
        let s = self.sensitivity * 0.6 + 0.2;
        s * s * s * 8.0 * 0.15
    }

    /// Text for the render distance slider and others that show a unit.
    pub fn fps_label(&self) -> String {
        if self.max_fps >= 260 { "Unlimited".into() } else { format!("{} fps", self.max_fps) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("mcrs-opts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut o = Options::load(&dir);
        o.fov = 90.0;
        o.keys[0] = KeyCode::ArrowUp;
        o.volumes[category::MUSIC] = 0.25;
        o.save();
        let back = Options::load(&dir);
        assert_eq!(back.fov, 90.0);
        assert_eq!(back.key(Action::Forward), KeyCode::ArrowUp);
        assert_eq!(back.volumes[category::MUSIC], 0.25);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_sensitivity_matches_vanilla() {
        // 0.5 sensitivity: (0.5) ^ 3 * 8 * 0.15 = 0.15 degrees per count.
        assert!((Options::defaults(PathBuf::new()).mouse_factor() - 0.15).abs() < 1e-6);
    }
}
