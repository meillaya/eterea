//! Small JSON preferences, separate from the archive database.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
pub struct Stored {
    pub theme: Option<String>,
    pub avatars: Option<bool>,
    pub media: Option<bool>,
    pub text_size: Option<u8>,
    pub motion: Option<String>,
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
    let Some(path) = path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let stored = Stored {
        theme: theme.map(|dark| if dark { "dark" } else { "light" }.to_string()),
        avatars: Some(avatars),
        media: Some(media),
        text_size: Some(text_size),
        motion: motion.map(|reduced| {
            if reduced {
                "reduced".to_string()
            } else {
                "system".to_string()
            }
        }),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&stored) {
        let _ = std::fs::write(path, bytes);
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
