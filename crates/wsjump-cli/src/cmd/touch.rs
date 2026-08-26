//! `wsjump touch [<path>]`: record a visit to a workspace so recency ordering
//! reflects it.

use std::env;
use std::path::PathBuf;

use wsjump_core::config::Config;
use wsjump_core::error::{AppError, AppResult};
use wsjump_core::index::Index;
use wsjump_core::progress::StderrProgress;
use wsjump_core::workspace::id_for_path;

/// Run the `touch` subcommand.
///
/// With no path, the current directory is used, so recording a visit after
/// changing into a workspace by hand is a bare `wsjump touch`. A path anywhere
/// inside a workspace records to that workspace.
pub fn run(config: &Config, path: Option<&String>) -> AppResult<()> {
    let target = match path {
        Some(path) => PathBuf::from(path),
        None => env::current_dir()
            .map_err(|e| AppError::runtime(format!("cannot determine current directory: {}", e)))?,
    };

    let id = id_for_path(&config.workspaces_dir, &target)?.ok_or_else(|| {
        AppError::runtime(format!("'{}' is not inside a workspace", target.display()))
    })?;

    // Opening the index also syncs it, so a workspace created moments ago is
    // already known and can be touched.
    let index = Index::open(config, &mut StderrProgress::new())?;
    index.record_visit(&id);
    Ok(())
}
