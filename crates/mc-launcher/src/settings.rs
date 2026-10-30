use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub username: String,
    pub memory_mb: u32,
    pub java_override: String,
    /// The user ticked "I accept the Minecraft EULA" (needed to run the world server).
    pub eula_accepted: bool,
    /// Singleplayer world folder name under `worlds/`.
    pub world: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            username: "Player".into(),
            memory_mb: 4096,
            java_override: String::new(),
            eula_accepted: false,
            world: "world".into(),
        }
    }
}

impl Settings {
    pub fn load(root: &Path) -> Self {
        std::fs::read_to_string(root.join("settings.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, root: &Path) {
        let _ = std::fs::create_dir_all(root);
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(root.join("settings.json"), json);
        }
    }
}
