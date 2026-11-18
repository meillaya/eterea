//! Small JSON preferences, separate from the archive database.
//!
//! Writes are atomic (temp file in the same directory, then rename) and the
//! last failure is exposed so the shell can show an honest status instead of
//! silently losing a preference.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

fn remember_error(message: String) {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = Some(message);
    }
}

/// Take the last write failure, if any, for the status line.
pub fn take_error() -> Option<String> {
    LAST_ERROR.lock().ok().and_then(|mut slot| slot.take())
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub width: f32,
    pub height: f32,
    pub x: i32,
    pub y: i32,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Stored {
    pub theme: Option<String>,
    pub avatars: Option<bool>,
    pub media: Option<bool>,
    pub text_size: Option<u8>,
    pub motion: Option<String>,
    #[serde(default)]
    pub window: Option<WindowGeometry>,
}

pub fn load() -> Stored {
    let Some(path) = path() else {
        return Stored::default();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Stored::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save(theme: Option<bool>, avatars: bool, media: bool, text_size: u8, motion: Option<bool>) {
    let mut stored = load();
    stored.theme = theme.map(|dark| if dark { "dark" } else { "light" }.to_string());
    stored.avatars = Some(avatars);
    stored.media = Some(media);
    stored.text_size = Some(text_size);
    stored.motion = motion.map(|reduced| {
        if reduced {
            "reduced".to_string()
        } else {
            "system".to_string()
        }
    });
    write(&stored);
}

/// Persist the window's last size and position without touching other settings.
pub fn save_window(geometry: WindowGeometry) {
    let mut stored = load();
    stored.window = Some(WindowGeometry {
        width: geometry.width.clamp(960.0, 8_000.0),
        height: geometry.height.clamp(640.0, 8_000.0),
        ..geometry
    });
    write(&stored);
}

fn write(stored: &Stored) {
    let Some(path) = path() else {
        return;
    };
    let bytes = match serde_json::to_vec_pretty(stored) {
        Ok(bytes) => bytes,
        Err(error) => {
            remember_error(format!("settings could not be encoded: {error}"));
            return;
        }
    };
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            remember_error(format!("settings folder could not be created: {error}"));
            return;
        }
    }
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if let Err(error) = std::fs::write(&temp, &bytes) {
        remember_error(format!("settings could not be written: {error}"));
        return;
    }
    if let Err(error) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        remember_error(format!("settings could not be replaced: {error}"));
    }
}

fn path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ETEREA_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(config).join("eterea/settings.json"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/eterea/settings.json"))
}

/// Path of the single-instance lock; `None` when the environment asks for an
/// isolated/tooling run.
pub fn instance_lock_path() -> Option<PathBuf> {
    if std::env::var_os("ETEREA_CONFIG_DIR").is_some()
        || std::env::var_os("ETEREA_DB").is_some()
        || std::env::var_os("ETEREA_DEMO").is_some()
    {
        return None;
    }
    path().map(|path| path.with_file_name("instance.lock"))
}
