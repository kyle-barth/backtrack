//! User settings, persisted as JSON in %APPDATA%\Backtrack\settings.json.

use crate::util;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub clip_dirs: Vec<String>,
    pub export_dir: String,
    pub default_preset: String,
    pub custom_mb: f64,
    pub auto_copy: bool,
    pub quick_len: f64,
    /// Read-only, filled in at runtime with the resolved ffmpeg.exe.
    pub ffmpeg_path: Option<String>,
    pub recorder: RecorderSettings,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Hotkeys {
    pub save: String,
    pub toggle: String,
    pub mute_mic: String,
    pub mark: String,
    pub open: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys {
            save: "Alt+S".into(),
            toggle: "Alt+F9".into(),
            mute_mic: "Alt+F8".into(),
            mark: "Alt+F7".into(),
            open: "Ctrl+Alt+B".into(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RecorderSettings {
    pub enabled: bool,
    pub buffer_secs: u32,
    /// Short side of the recording (720 / 1080 / 1440); 0 = native.
    pub resolution: u32,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub encoder: String,
    pub monitor: u32,
    pub clips_dir: String,
    pub hotkeys: Hotkeys,
    pub confirm: bool,
    pub chat_app: String,
    pub mic_device: String,
    pub mic_enabled: bool,
    pub start_with_windows: bool,
}

impl Default for RecorderSettings {
    fn default() -> Self {
        let videos = dirs::video_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Videos")))
            .unwrap_or_else(std::env::temp_dir);
        RecorderSettings {
            enabled: true,
            buffer_secs: 60,
            resolution: 1080,
            fps: 60,
            bitrate_mbps: 25,
            encoder: "auto".into(),
            monitor: 0,
            clips_dir: videos.join("Backtrack").join("Clips").to_string_lossy().into_owned(),
            hotkeys: Hotkeys::default(),
            confirm: true,
            chat_app: "Discord".into(),
            mic_device: String::new(),
            mic_enabled: true,
            start_with_windows: false,
        }
    }
}

impl RecorderSettings {
    /// Settings that require restarting the capture pipeline when changed.
    pub fn pipeline_key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.buffer_secs, self.resolution, self.fps, self.bitrate_mbps, self.encoder, self.monitor,
            self.chat_app, self.mic_device, self.mic_enabled
        )
    }

    fn normalize(&mut self) {
        let d = RecorderSettings::default();
        if ![15, 30, 60, 120, 300].contains(&self.buffer_secs) {
            self.buffer_secs = self.buffer_secs.clamp(10, 600);
        }
        if ![0, 720, 1080, 1440, 2160].contains(&self.resolution) {
            self.resolution = d.resolution;
        }
        if ![30, 60, 120, 144].contains(&self.fps) {
            self.fps = d.fps;
        }
        self.bitrate_mbps = self.bitrate_mbps.clamp(4, 100);
        if !["auto", "amf", "nvenc", "x264"].contains(&self.encoder.as_str()) {
            self.encoder = "auto".into();
        }
        if self.clips_dir.trim().is_empty() {
            self.clips_dir = d.clips_dir;
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        let videos = dirs::video_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Videos")))
            .unwrap_or_else(std::env::temp_dir);
        let gg = videos.join("SteelSeries Moments");
        Settings {
            clip_dirs: if gg.is_dir() { vec![gg.to_string_lossy().into_owned()] } else { vec![] },
            export_dir: videos.join("Backtrack").join("Exports").to_string_lossy().into_owned(),
            default_preset: "d10".into(),
            custom_mb: 25.0,
            auto_copy: true,
            quick_len: 20.0,
            ffmpeg_path: None,
            recorder: RecorderSettings::default(),
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        let path = util::data_dir().join("settings.json");
        let mut s: Settings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.normalize();
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let path = util::data_dir().join("settings.json");
        let mut copy = self.clone();
        copy.ffmpeg_path = None;
        let text = serde_json::to_string_pretty(&copy).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("Couldn't save settings: {e}"))
    }

    pub fn normalize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.clip_dirs.retain(|d| !d.trim().is_empty() && seen.insert(d.to_lowercase()));
        if self.export_dir.trim().is_empty() {
            self.export_dir = Settings::default().export_dir;
        }
        if !["d10", "d50", "d500", "orig", "custom"].contains(&self.default_preset.as_str()) {
            self.default_preset = "d10".into();
        }
        if !(self.custom_mb > 0.5) {
            self.custom_mb = 25.0;
        }
        if !(self.quick_len >= 3.0) {
            self.quick_len = 20.0;
        }
        self.recorder.normalize();
        // Saved clips must show up in the library.
        let clips = self.recorder.clips_dir.clone();
        if !self.clip_dirs.iter().any(|d| d.eq_ignore_ascii_case(&clips)) {
            self.clip_dirs.push(clips);
        }
    }
}
