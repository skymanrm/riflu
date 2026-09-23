//! Token at rest, in the per-user config dir next to the settings file.
//! It is a full account credential — see README.

use std::path::PathBuf;

use crate::auth::OAuthToken;
use crate::error::{Error, Result};

/// Must match `identifier` in tauri.conf.json. `settings.rs` reaches the same
/// directory through Tauri's `app_config_dir()`, which is
/// `dirs::config_dir()/<identifier>` — this resolves it the same way, because
/// the token is also read from the probe and from the 401 refresh, where there
/// is no `AppHandle` to ask.
const APP_DIR: &str = "ru.fanyagin.yamusic";
const FILE: &str = "token.json";

fn token_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| Error::Api("no user config directory".into()))?
        .join(APP_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Api(format!("could not create config dir: {e}")))?;
    Ok(dir.join(FILE))
}

pub fn save(token: &OAuthToken) -> Result<()> {
    let path = token_path()?;
    let raw = serde_json::to_string(token)?;

    write_private(&path, &raw).map_err(|e| Error::Api(format!("could not save token: {e}")))
}

pub fn load() -> Result<Option<OAuthToken>> {
    let path = token_path()?;
    Ok(std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok()))
}

pub fn clear() -> Result<()> {
    let path = token_path()?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Api(format!("could not remove token: {e}"))),
    }
}

/// Create with 0600 from the outset so the token is never briefly world-readable.
#[cfg(unix)]
fn write_private(path: &PathBuf, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(contents.as_bytes())
}

/// Windows: `%APPDATA%` is already per-user and unreadable by other accounts
/// without administrator rights, so a plain write carries the same exposure.
#[cfg(not(unix))]
fn write_private(path: &PathBuf, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)
}
