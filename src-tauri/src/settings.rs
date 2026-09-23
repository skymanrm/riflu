//! User preferences, stored as JSON in the app config dir next to the token.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::Result;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// macOS: live in the menu bar with no Dock icon.
    pub menu_bar_mode: bool,
    /// Keep the window above other apps.
    pub always_on_top: bool,
    /// Compact window: art, track info and the base controls only.
    pub mini_player: bool,
    /// Rotor station id (`type:tag`); unset means Моя волна.
    pub station_id: Option<String>,
    /// Its display name, remembered so the label is right before the list loads.
    pub station_name: Option<String>,
}

fn path(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| crate::error::Error::Api(format!("no config dir: {e}")))?;
    std::fs::create_dir_all(&dir).ok();
    Ok(dir.join("settings.json"))
}

/// Unreadable or corrupt settings fall back to defaults rather than failing boot.
pub fn load(app: &AppHandle) -> Settings {
    path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<()> {
    let p = path(app)?;
    std::fs::write(p, serde_json::to_string_pretty(settings)?)
        .map_err(|e| crate::error::Error::Api(format!("could not save settings: {e}")))?;
    Ok(())
}
