//! Configuration file location and parsing.
//!
//! `wsjump` reads a TOML config placed next to the executable (resolved via
//! `current_exe()`, which follows symlinks / junctions to the real binary in
//! `target/<profile>/`). The config tells the tool where `w/` lives and which
//! fixed UTC offset to use for date generation / parsing.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::{AppError, AppResult};

/// File name of the config, expected to sit beside the executable.
const CONFIG_FILE_NAME: &str = "wsjump.toml";

/// Parsed configuration.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Absolute path to the `w/` directory that holds all workspaces.
    pub workspaces_dir: PathBuf,
    /// Fixed UTC offset in hours used to derive "today" for date prefixes
    /// and to interpret directory-name dates (e.g. 8 for UTC+8).
    pub utc_offset_hours: i8,
}

impl Config {
    /// Load the config from the file next to the current executable.
    ///
    /// Any failure (cannot locate the exe, file missing, unreadable, or
    /// malformed TOML) is a runtime error: without config we cannot locate
    /// `w/` and therefore cannot work.
    pub fn load() -> AppResult<Config> {
        let path = config_path()?;
        let text = fs::read_to_string(&path).map_err(|e| {
            AppError::runtime(format!(
                "cannot read config file '{}': {}",
                path.display(),
                e
            ))
        })?;
        let config: Config = toml::from_str(&text).map_err(|e| {
            AppError::runtime(format!(
                "cannot parse config file '{}': {}",
                path.display(),
                e
            ))
        })?;
        Ok(config)
    }
}

/// Resolve the expected path of the config file: `<exe dir>/wsjump.toml`.
fn config_path() -> AppResult<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| AppError::runtime(format!("cannot locate current executable: {}", e)))?;
    let dir = exe.parent().ok_or_else(|| {
        AppError::runtime("current executable has no parent directory".to_string())
    })?;
    Ok(dir.join(CONFIG_FILE_NAME))
}
