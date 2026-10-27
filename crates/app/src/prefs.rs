//! Small persistent preferences (recording folder, last-used capture options).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub recordings_dir: PathBuf,
    pub fps: u32,
    pub max_long_side: u32,
    pub system_audio: bool,
    pub microphone: bool,
    pub microphone_id: Option<String>,
    pub microphone_name: String,
    /// "display", "window", "area" or "camera" (a window or area pick does not survive a restart).
    pub mode: String,
    /// 9:16: the area picker keeps that shape, and a camera-only video is cut to it.
    pub vertical: bool,
    pub camera: bool,
    pub camera_id: Option<String>,
    pub camera_label: String,
    pub export_dir: Option<PathBuf>,
    /// Notify-only update check at startup (default on).
    pub update_check: Option<bool>,
    pub dismissed_update: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        let base = dirs::video_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
        Self {
            recordings_dir: base.join("ScreenCraft"),
            fps: 30,
            max_long_side: 2560,
            system_audio: true,
            microphone: false,
            microphone_id: None,
            microphone_name: String::new(),
            mode: "display".into(),
            vertical: false,
            camera: false,
            camera_id: None,
            camera_label: String::new(),
            export_dir: None,
            update_check: None,
            dismissed_update: None,
        }
    }
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ScreenCraft").join("settings.json"))
}

impl Prefs {
    pub fn load() -> Self {
        let mut p: Self = path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // Handy for tests and demos: point the library somewhere else without touching settings.
        if let Some(dir) = std::env::var_os("SCREENCRAFT_RECORDINGS_DIR") {
            p.recordings_dir = dir.into();
        }
        p
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_vec_pretty(self) {
                let _ = std::fs::write(p, json);
            }
        }
    }
}
