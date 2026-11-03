//! Configuration beside the invoked executable, including the shared root.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::query::SortOrder;

/// Parsed v3 config. Unknown keys and relative roots are rejected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Absolute root containing the flat `w/` and categorized `c/` containers.
    pub root_dir: PathBuf,
    /// Optional database location; defaults beside the invoked executable.
    #[serde(default)]
    db_file: Option<PathBuf>,
    /// Optional query order; defaults to recency.
    #[serde(default)]
    default_sort: Option<SortOrder>,
}

impl Config {
    /// Load `wsjump.toml` beside the executable and validate its root.
    pub fn load() -> AppResult<Self> {
        let path = exe_dir()?.join("wsjump.toml");
        let text = fs::read_to_string(&path).map_err(|error| {
            AppError::runtime(format!(
                "cannot read config file '{}': {}",
                path.display(),
                error
            ))
        })?;
        Self::parse(&text)
    }

    /// Parse v3 settings; legacy keys are deliberately unsupported.
    fn parse(text: &str) -> AppResult<Self> {
        let config: Self = toml::from_str(text)
            .map_err(|error| AppError::runtime(format!("cannot parse config: {}", error)))?;
        if !config.root_dir.is_absolute() {
            return Err(AppError::runtime("root_dir must be an absolute path"));
        }
        Ok(config)
    }

    /// Resolve the configured database or the default executable-adjacent file.
    pub fn db_file(&self) -> AppResult<PathBuf> {
        match &self.db_file {
            Some(path) => Ok(path.clone()),
            None => Ok(exe_dir()?.join("wsjump.db")),
        }
    }

    /// Default query order when the command does not override it.
    pub fn default_sort(&self) -> SortOrder {
        self.default_sort.unwrap_or_default()
    }

    /// Build a config for real-tree tests in core and sibling crates.
    #[doc(hidden)]
    pub fn for_test(root: &Path) -> Self {
        Self {
            root_dir: root.to_path_buf(),
            db_file: None,
            default_sort: None,
        }
    }

    /// Build an isolated index configuration for integration-style tests.
    #[doc(hidden)]
    pub fn for_test_with_db(root: &Path, database: &Path) -> Self {
        Self {
            root_dir: root.to_path_buf(),
            db_file: Some(database.to_path_buf()),
            default_sort: None,
        }
    }
}

/// Directory containing the invoked executable, including a Windows bin symlink.
fn exe_dir() -> AppResult<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|error| AppError::runtime(format!("cannot locate executable: {}", error)))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| AppError::runtime("executable has no parent directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_config_requires_an_absolute_root_and_new_sort_name() {
        let root = std::env::temp_dir().to_string_lossy().replace('\\', "/");
        let config =
            Config::parse(&format!("root_dir = {root:?}\ndefault_sort = \"name\"")).unwrap();
        assert_eq!(config.default_sort(), SortOrder::Name);
        assert!(Config::parse("root_dir = \"relative\"").is_err());
        assert!(Config::parse("workspaces_dir = \"G:/ws/w\"\nutc_offset_hours = 8").is_err());
        assert!(Config::parse(&format!("root_dir = {root:?}\nutc_offset_hours = 8")).is_err());
        assert!(Config::parse(&format!("root_dir = {root:?}\ndefault_sort = \"id\"")).is_err());
    }
}
