//! `wsjump new <slug...>`: create a new workspace directory named
//! `YYYYMMDD[-slug]` under `w/` and print its absolute path.

use std::fs;

use time::{OffsetDateTime, UtcOffset};

use crate::config::Config;
use crate::error::{AppError, AppResult};

/// Run the `new` subcommand.
///
/// - All positional args are joined with '-' to form the slug (empty if none).
/// - The current date (in the configured UTC offset) is always prefixed;
///   no dedup / validation of the slug is performed.
/// - An existing target directory is a conflict and aborts without overwrite.
pub fn run(config: &Config, args: &[String]) -> AppResult<()> {
    let slug = args.join("-");
    let date_prefix = today_prefix(config.utc_offset_hours)?;

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

    println!("{}", target.display());
    Ok(())
}

/// Compute today's date as "YYYYMMDD" in the given fixed UTC offset (hours).
fn today_prefix(utc_offset_hours: i8) -> AppResult<String> {
    let offset = UtcOffset::from_hms(utc_offset_hours, 0, 0).map_err(|e| {
        AppError::runtime(format!(
            "invalid utc_offset_hours {}: {}",
            utc_offset_hours, e
        ))
    })?;
    let now = OffsetDateTime::now_utc().to_offset(offset);
    let date = now.date();
    Ok(format!(
        "{:04}{:02}{:02}",
        date.year(),
        date.month() as u8,
        date.day()
    ))
}
