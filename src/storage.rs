//! Cross-platform config/data directory resolution and JSON load/save helpers.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{de::DeserializeOwned, Serialize};
use std::{fs, path::PathBuf};

// User data lives under the shipped product name — the folder the .msi install
// leaves behind should be one the user recognizes. Empty ORG keeps the Windows
// path flat (no nested folder).
const QUALIFIER: &str = "dev";
const ORG: &str = "";
const APP: &str = "RustedNote";

/// Returns the config directory, creating it if needed.
///
/// - Windows: `%APPDATA%\RustedNote\config`
/// - macOS:   `~/Library/Application Support/dev.RustedNote`
/// - Linux:   `~/.config/rustednote`
pub fn config_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from(QUALIFIER, ORG, APP)
        .context("could not determine a home/config directory")?;
    let dir = dirs.config_dir().to_path_buf();
    let fresh = !dir.exists();
    fs::create_dir_all(&dir).with_context(|| format!("creating config dir {dir:?}"))?;
    if fresh {
        adopt_legacy_config(&dir);
    }
    Ok(dir)
}

/// Up to 0.1.0 the config dir was keyed on the then-crate name
/// (`notepad_clone`) — that literal below is the *old on-disk path*, not the
/// current crate name, so it must not be renamed along with the package.
/// On the first run after upgrading, carry those files across so preferences,
/// bookmarks and the open session survive; the old copies are left in place for
/// the user to delete. Best effort — a failure here just means defaults.
fn adopt_legacy_config(dir: &PathBuf) {
    let Some(legacy) = ProjectDirs::from("dev", "notepad_clone", "notepad_clone") else {
        return;
    };
    let Ok(entries) = fs::read_dir(legacy.config_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().is_file() {
            let _ = fs::copy(entry.path(), dir.join(entry.file_name()));
        }
    }
}

pub fn config_file(name: &str) -> Result<PathBuf> {
    Ok(config_dir()?.join(name))
}

/// Load a JSON file into `T`, or return `T::default()` if it doesn't exist yet.
pub fn load_or_default<T: DeserializeOwned + Default>(name: &str) -> T {
    match config_file(name).and_then(|p| Ok((p.clone(), fs::read_to_string(&p)?))) {
        Ok((_, contents)) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

/// Persist `value` as pretty JSON. Writes atomically via a temp file + rename.
pub fn save<T: Serialize>(name: &str, value: &T) -> Result<()> {
    let path = config_file(name)?;
    let tmp = path.with_extension("tmp");
    let json = serde_json::to_string_pretty(value)?;
    fs::write(&tmp, json).with_context(|| format!("writing {tmp:?}"))?;
    fs::rename(&tmp, &path).with_context(|| format!("renaming into {path:?}"))?;
    Ok(())
}
