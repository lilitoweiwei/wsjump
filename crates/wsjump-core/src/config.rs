//! Configuration file location and parsing.
//!
//! `wsjump` reads a TOML config placed next to the executable. The path is
//! resolved via `current_exe()`, which on Windows returns the invoked path
//! (e.g. the symlink in `bin/`) WITHOUT resolving symlinks / junctions to the
//! real binary in `target/<profile>/`. The config therefore lives next to the
//! invoked binary in `bin/`. It tells the tool where `w/` lives and which fixed
//! UTC offset to use for date generation / parsing.
//!
//! Optional keys (`db_file`, `default_sort`) have defaults and are read through
//! accessors rather than public fields, so callers never handle the `Option`.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use time::UtcOffset;

use crate::error::{AppError, AppResult};
use crate::query::SortOrder;

/// File name of the config, expected to sit beside the executable.
const CONFIG_FILE_NAME: &str = "wsjump.toml";

/// File name of the database, used when `db_file` is not configured.
const DB_FILE_NAME: &str = "wsjump.db";

/// Parsed configuration.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Absolute path to the `w/` directory that holds all workspaces.
    pub workspaces_dir: PathBuf,
    /// Fixed UTC offset in hours used to derive "today" for date prefixes
    /// and to interpret directory-name dates (e.g. 8 for UTC+8).
    pub utc_offset_hours: i8,
    /// Where to keep the database. Optional; defaults to `wsjump.db` beside
    /// the executable. Use [`Config::db_file`] rather than reading this.
    #[serde(default)]
    db_file: Option<PathBuf>,
    /// Result order used when no `--sort` is given. Optional; defaults to
    /// [`SortOrder::DEFAULT`]. Use [`Config::default_sort`] to read it.
    #[serde(default)]
    default_sort: Option<SortOrder>,
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

    /// Resolve the database path: the configured `db_file`, else `wsjump.db`
    /// beside the executable (i.e. next to the config file).
    pub fn db_file(&self) -> AppResult<PathBuf> {
        match &self.db_file {
            Some(path) => Ok(path.clone()),
            None => Ok(exe_dir()?.join(DB_FILE_NAME)),
        }
    }

    /// The configured default result order, or [`SortOrder::DEFAULT`].
    pub fn default_sort(&self) -> SortOrder {
        self.default_sort.unwrap_or(SortOrder::DEFAULT)
    }

    /// The configured fixed UTC offset, validated.
    ///
    /// Every date computation (today's prefix, a directory date's midnight)
    /// must go through this so they cannot disagree about when a day starts.
    pub fn utc_offset(&self) -> AppResult<UtcOffset> {
        UtcOffset::from_hms(self.utc_offset_hours, 0, 0).map_err(|e| {
            AppError::runtime(format!(
                "invalid utc_offset_hours {}: {}",
                self.utc_offset_hours, e
            ))
        })
    }

    /// A config pointing at `dir`, for tests that need to scan a real tree.
    ///
    /// Not behind `cfg(test)`: integration-style tests in sibling crates need it
    /// too, and a second constructor is cheaper than exposing the fields.
    #[doc(hidden)]
    pub fn for_test(dir: &std::path::Path) -> Config {
        Config {
            workspaces_dir: dir.to_path_buf(),
            utc_offset_hours: 8,
            db_file: None,
            default_sort: None,
        }
    }
}

/// Resolve the expected path of the config file: `<exe dir>/wsjump.toml`.
fn config_path() -> AppResult<PathBuf> {
    Ok(exe_dir()?.join(CONFIG_FILE_NAME))
}

/// Directory holding the invoked executable (see the module comment on why
/// this is `bin/` rather than `target/<profile>/`).
fn exe_dir() -> AppResult<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| AppError::runtime(format!("cannot locate current executable: {}", e)))?;
    let dir = exe.parent().ok_or_else(|| {
        AppError::runtime("current executable has no parent directory".to_string())
    })?;
    Ok(dir.to_path_buf())
}
