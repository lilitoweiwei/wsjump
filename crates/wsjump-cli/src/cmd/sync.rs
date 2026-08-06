//! `wsjump sync [<path>]`: bring the index back in line with the directory.
//!
//! Sync happens automatically on every invocation, so this exists for the case
//! automation cannot cover: an index whose stored signatures agree with disk
//! while its contents do not. That is only reachable through a signature
//! collision or outside tampering, but when it happens nothing else will fix it.

use std::path::PathBuf;

use wsjump_core::config::Config;
use wsjump_core::error::{AppError, AppResult};
use wsjump_core::index::Index;
use wsjump_core::workspace::id_for_possibly_missing_path;
use crate::progress::StderrProgress;

/// Run the `sync` subcommand.
///
/// With no path the whole directory is re-read. With one, only that workspace is,
/// which is both a convenience and the cheap way out of a signature collision.
pub fn run(config: &Config, path: Option<&String>) -> AppResult<()> {
    let (_index, outcome) = match path {
        Some(path) => {
            let target = PathBuf::from(path);
            // Deliberately tolerant of a missing directory: "I deleted this
            // workspace, update the index" is a normal reason to run this.
            let id = id_for_possibly_missing_path(&config.workspaces_dir, &target)?
                .ok_or_else(|| {
                    AppError::usage(format!(
                        "'{}' does not name a workspace under the workspaces directory",
                        target.display()
                    ))
                })?;
            Index::open_syncing_only(config, &id, &mut StderrProgress::new())?
        }
        None => Index::open_forcing_sync(config, &mut StderrProgress::new())?,
    };

    // An explicit command reports what it did, unlike the automatic sync, which
    // stays silent. On stderr, so stdout carries only real output.
    eprintln!(
        "wsjump: synced {} workspace(s), updated {}, removed {}",
        outcome.examined, outcome.updated, outcome.deleted
    );
    Ok(())
}
