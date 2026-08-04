//! Cold scan of the `w/` directory into a list of workspaces.

use std::fs;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::meta;
use crate::workspace::{parse_dir_name, Workspace};

/// Scan every direct child directory of `workspaces_dir`.
///
/// Each valid workspace directory (name matches `YYYYMMDD[-slug]` with a real
/// calendar date) is turned into a `Workspace`. Names that do not match are
/// warned about on stderr and skipped.
///
/// The result is in filesystem enumeration order: scanning only discovers, it
/// does not order. Ordering is a query concern and belongs to the database.
/// Scanned workspaces carry `last_access: None` because access history lives in
/// the database, not on disk.
pub fn scan(config: &Config) -> AppResult<Vec<Workspace>> {
    let dir = &config.workspaces_dir;
    let entries = fs::read_dir(dir).map_err(|e| {
        AppError::runtime(format!(
            "cannot read workspaces directory '{}': {}",
            dir.display(),
            e
        ))
    })?;

    let mut workspaces: Vec<Workspace> = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("wsjump: warning: cannot read a directory entry: {}", e);
                continue;
            }
        };

        // Only directories are workspaces.
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "wsjump: warning: cannot determine type of '{}': {}",
                    entry.path().display(),
                    e
                );
                continue;
            }
        };
        if !file_type.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        let (date, slug) = match parse_dir_name(&name) {
            Some(parsed) => parsed,
            None => {
                eprintln!(
                    "wsjump: warning: '{}' is not a valid workspace name; skipping",
                    name
                );
                continue;
            }
        };

        let path = entry.path();
        let m = meta::load(&path);

        workspaces.push(Workspace {
            id: name,
            date,
            slug,
            path,
            tags: m.tags,
            desc: m.desc,
            last_access: None,
        });
    }

    Ok(workspaces)
}
