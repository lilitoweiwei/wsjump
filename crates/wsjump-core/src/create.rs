//! Creating a new workspace directory under `w/`.

use std::fs;
use std::path::PathBuf;

use time::OffsetDateTime;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::workspace::format_date;

/// Create a new workspace directory named `YYYYMMDD[-slug]` under the
/// configured `workspaces_dir` and return its absolute path.
///
/// - `slug` is used verbatim (callers join positional args as needed); an
///   empty slug yields a bare `YYYYMMDD` directory.
/// - The current date (in the configured UTC offset) is always prefixed;
///   no dedup / validation of the slug is performed.
/// - An existing target directory is a conflict and aborts without overwrite.
pub fn create_workspace(config: &Config, slug: &str) -> AppResult<PathBuf> {
    let date_prefix = today_prefix(config)?;

    // Empty slug -> "YYYYMMDD"; non-empty -> "YYYYMMDD-slug".
    let dir_name = if slug.is_empty() {
        date_prefix
    } else {
        format!("{}-{}", date_prefix, slug)
    };

    let target = config.workspaces_dir.join(&dir_name);
    if target.exists() {
        return Err(AppError::runtime(format!(
            "workspace already exists: {}",
            target.display()
        )));
    }

    fs::create_dir(&target).map_err(|e| {
        AppError::runtime(format!(
            "cannot create workspace '{}': {}",
            target.display(),
            e
        ))
    })?;

    Ok(target)
}

/// Compute today's date as "YYYYMMDD" in the configured fixed UTC offset.
fn today_prefix(config: &Config) -> AppResult<String> {
    let offset = config.utc_offset()?;
    let now = OffsetDateTime::now_utc().to_offset(offset);
    Ok(format_date(now.date()))
}
