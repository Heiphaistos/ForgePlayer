use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum AspectMode {
    #[default]
    Fit,
    Fill,
    Stretch,
}

impl AspectMode {
    pub fn next(&self) -> Self {
        match self { Self::Fit => Self::Fill, Self::Fill => Self::Stretch, Self::Stretch => Self::Fit }
    }
    pub fn label(&self) -> &'static str {
        match self { Self::Fit => "Fit", Self::Fill => "Fill", Self::Stretch => "Stretch" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum LoopMode {
    #[default]
    Off,
    One,
    All,
}

impl LoopMode {
    pub fn next(&self) -> Self {
        match self { Self::Off => Self::One, Self::One => Self::All, Self::All => Self::Off }
    }
    pub fn label(&self) -> &'static str {
        match self { Self::Off => "Off", Self::One => "×1", Self::All => "All" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub window_width:        u32,
    pub window_height:       u32,
    pub volume:              f32,
    pub hw_accel:            String,
    pub subtitle_lang:       String,
    pub tonemap_mode:        u32,
    pub max_luminance:       f32,
    pub subtitle_service_port: u16,
    pub media_indexer_port:    u16,
    pub recent_files:        Vec<String>,
    pub media_library:       Vec<String>,
    #[serde(default)]
    pub aspect_mode:         AspectMode,
    #[serde(default)]
    pub loop_mode:           LoopMode,
    #[serde(default = "default_speed")]
    pub playback_speed:      f32,
}

fn default_speed() -> f32 { 1.0 }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            window_width:          1280,
            window_height:         720,
            volume:                1.0,
            hw_accel:              "auto".into(),
            subtitle_lang:         "fr".into(),
            tonemap_mode:          1,
            max_luminance:         1000.0,
            subtitle_service_port: 18080,
            media_indexer_port:    18081,
            recent_files:          Vec::new(),
            media_library:         Vec::new(),
            aspect_mode:           AspectMode::Fit,
            loop_mode:             LoopMode::Off,
            playback_speed:        1.0,
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("OmniPlayer")
            .join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn add_recent(&mut self, file: &str) {
        self.recent_files.retain(|f| f != file);
        self.recent_files.insert(0, file.to_string());
        self.recent_files.truncate(20);
    }
}
