use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MIC_DEFAULT: &str = "default";
pub const LOOPBACK: &str = "__loopback__";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum ImgFormat {
    Png,
    Jpg,
}

impl ImgFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ImgFormat::Png => "png",
            ImgFormat::Jpg => "jpg",
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    // screenshots
    pub save_dir: PathBuf,
    pub format: ImgFormat,
    pub jpg_quality: u8,
    pub auto_save: bool,
    pub auto_copy: bool,
    pub delay_secs: u32,
    // video
    pub fps: u32,
    pub crf: u32,
    pub record_audio: bool,
    pub audio_source: String,
    pub ffmpeg_path: String,
    // editor
    pub font_path: String,
    pub color: [u8; 3],
    pub width: f32,
    // window behaviour
    pub start_in_background: bool,
    pub close_to_tray: bool,
    pub autostart: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let base = dirs::picture_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            save_dir: base.join("SnipXTool"),
            format: ImgFormat::Png,
            jpg_quality: 90,
            auto_save: true,
            auto_copy: true,
            delay_secs: 0,
            fps: 30,
            crf: 23,
            record_audio: true,
            audio_source: MIC_DEFAULT.to_string(),
            ffmpeg_path: ffmpeg_default(),
            font_path: String::new(),
            color: [230, 40, 40],
            width: 4.0,
            start_in_background: true,
            close_to_tray: true,
            autostart: true,
        }
    }
}

/// Prefer an ffmpeg shipped next to the executable, then fall back to PATH.
/// This is what lets the installer work without touching the user's PATH.
pub fn ffmpeg_default() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [
                dir.join("ffmpeg.exe"),
                dir.join("bin").join("ffmpeg.exe"),
                dir.join("ffmpeg").join("bin").join("ffmpeg.exe"),
            ] {
                if candidate.is_file() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
    }
    "ffmpeg".to_string()
}

impl Settings {
    fn file() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("snipxtool").join("settings.json"))
    }

    /// Settings written by previous builds of the app ("snipx" and the original
    /// "rustsnip"), read once for migration.
    fn legacy_file() -> Option<PathBuf> {
        let base = dirs::config_dir()?;
        for dir in ["snipx", "rustsnip"] {
            let p = base.join(dir).join("settings.json");
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    /// Whether SnipXTool has been run before on this machine (including under
    /// its previous names).
    pub fn exists() -> bool {
        Self::file().map(|p| p.exists()).unwrap_or(false)
    }

    pub fn load() -> Self {
        let mut cfg: Self = Self::file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .or_else(|| {
                // First run after the rename: keep the user's old preferences.
                Self::legacy_file()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            })
            .unwrap_or_default();
        if cfg.ffmpeg_path.trim().is_empty() || cfg.ffmpeg_path == "ffmpeg" {
            cfg.ffmpeg_path = ffmpeg_default();
        }
        cfg
    }

    pub fn save(&self) {
        if let Some(p) = Self::file() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}
