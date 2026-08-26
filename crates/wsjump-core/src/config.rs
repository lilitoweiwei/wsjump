//! Configuration file location and parsing.
//!
//! `wsjump` reads a TOML config placed next to the executable. The install
//! directory is where the config (and by default the database) lives:
//!
//! - On Windows `current_exe()` reports the *invoked* path (e.g. the symlink
//!   in `bin/`) without resolving junctions to the real binary in
//!   `target/<profile>/`, so the install dir is simply the exe's parent.
//! - On POSIX the kernel has already resolved the symlink by the time the
//!   process starts, so `current_exe()` points at the real binary and the
//!   spelled install dir is searched for instead: first via `argv[0]` when it
//!   carries a directory part, then across `$PATH` entries whose `wsjump` /
//!   `wsjump-tui` canonicalises back to this binary (a symlink-in-PATH
//!   deployment such as `~/ws/bin`). The first candidate containing
//!   `wsjump.toml` wins.

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
    /// in the install directory (where the config lives).
    pub fn db_file(&self) -> AppResult<PathBuf> {
        match &self.db_file {
            Some(path) => Ok(path.clone()),
            None => Ok(install_dir()?.join(DB_FILE_NAME)),
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

/// Resolve the expected path of the config file: `<install dir>/wsjump.toml`.
fn config_path() -> AppResult<PathBuf> {
    Ok(install_dir()?.join(CONFIG_FILE_NAME))
}

/// The install directory: where `wsjump.toml` lives (see the module comment
/// for how it is located on each platform).
fn install_dir() -> AppResult<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    let exe = std::env::current_exe()
        .map_err(|e| AppError::runtime(format!("cannot locate current executable: {}", e)))?;
    push_unique(&mut candidates, exe.parent());

    // argv[0] keeps the *spelled* path, the only place a POSIX symlink
    // deployment (e.g. running `~/ws/bin/wsjump` directly) still shows up.
    if let Some(spelled) = std::env::args().next() {
        let p = PathBuf::from(&spelled);
        if p.components().count() > 1 {
            push_unique(&mut candidates, p.parent());
        }
    }

    // A bare `wsjump` on the PATH reached us through a symlink; scan PATH for
    // entries whose `wsjump` / `wsjump-tui` resolves back to this binary.
    let exe_real = exe.canonicalize().unwrap_or_else(|_| exe.clone());
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["wsjump", "wsjump-tui"] {
                let cand = dir.join(name);
                if let Ok(real) = cand.canonicalize() {
                    if real == exe_real {
                        push_unique(&mut candidates, Some(&dir));
                        break;
                    }
                }
            }
        }
    }

    for dir in &candidates {
        if dir.join(CONFIG_FILE_NAME).exists() {
            return Ok(dir.clone());
        }
    }

    let looked = candidates
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(AppError::runtime(format!(
        "no {} found next to the executable (looked in: {})",
        CONFIG_FILE_NAME, looked
    )))
}

/// Append `dir` to `out` unless it is missing or already present.
fn push_unique(out: &mut Vec<PathBuf>, dir: Option<&std::path::Path>) {
    if let Some(dir) = dir {
        let dir = dir.to_path_buf();
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
}
