//! Discovery of workspaces in the `w/` directory.
//!
//! Discovery is split from metadata loading on purpose. Enumerating the
//! directory is nearly free -- the entries carry everything a directory name can
//! tell us -- while opening each `wsmeta.txt` is where almost all of the cost
//! lives. Sync therefore discovers everything, then reads metadata only for the
//! workspaces whose [`crate::signature`] shows they changed.

use std::fs;
use std::path::PathBuf;

use time::Date;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::meta;
use crate::signature::{self, MetaSignature};
use crate::workspace::{parse_dir_name, Workspace};

/// A workspace found on disk, before its metadata has been read.
///
/// Everything here comes from the directory entry itself, so building one costs
/// nothing beyond the enumeration that found it.
pub struct Discovered {
    /// The whole directory name, e.g. "20260730-wsjump".
    pub id: String,
    /// Creation date parsed from the 8-digit prefix.
    pub date: Date,
    /// The part after the first '-'; may be empty.
    pub slug: String,
    /// Absolute path to the workspace directory.
    pub path: PathBuf,
    /// Current signature of its `wsmeta.txt`, or `None` if it has none.
    pub signature: Option<MetaSignature>,
}

impl Discovered {
    /// Read and parse this workspace's metadata, producing a full `Workspace`.
    ///
    /// This is the expensive part of a scan, so it is a separate step the caller
    /// invokes only when the signature says the file changed.
    pub fn load(&self) -> Workspace {
        let m = meta::load(&self.path);
        Workspace {
            id: self.id.clone(),
            date: self.date,
            slug: self.slug.clone(),
            path: self.path.clone(),
            tags: m.tags,
            desc: m.desc,
            last_access: None,
        }
    }
}

/// The result of enumerating a `w/` directory.
///
/// The root travels with the entries deliberately. Sync has to know which
/// directory the entries came from, in order to notice that `workspaces_dir` now
/// points somewhere else; keeping the two together makes it impossible to sync
/// one directory's entries while claiming they came from another.
pub struct Discovery {
    /// The directory that was enumerated.
    pub root: PathBuf,
    /// The workspaces found in it, in filesystem order.
    pub workspaces: Vec<Discovered>,
}

/// Enumerate every workspace directory under `workspaces_dir`.
///
/// Each valid workspace directory (name matches `YYYYMMDD[-slug]` with a real
/// calendar date) yields a [`Discovered`]. Names that do not match are warned
/// about on stderr and skipped.
///
/// No `wsmeta.txt` is opened here; only stat'ed, which is markedly cheaper.
/// The result is in filesystem enumeration order: discovery does not order,
/// because ordering is a query concern and belongs to the database.
pub fn discover(config: &Config) -> AppResult<Discovery> {
    let dir = &config.workspaces_dir;
    let entries = fs::read_dir(dir).map_err(|e| {
        AppError::runtime(format!(
            "cannot read workspaces directory '{}': {}",
            dir.display(),
            e
        ))
    })?;

    let mut found: Vec<Discovered> = Vec::new();

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
        let signature = signature::read(&path);

        found.push(Discovered {
            id: name,
            date,
            slug,
            path,
            signature,
        });
    }

    Ok(Discovery {
        root: dir.clone(),
        workspaces: found,
    })
}

/// Discover every workspace and read all of their metadata.
///
/// Used where the whole picture is needed at once and there is no stored
/// signature to compare against: the database-free fallback path.
pub fn scan(config: &Config) -> AppResult<Vec<Workspace>> {
    Ok(discover(config)?
        .workspaces
        .iter()
        .map(Discovered::load)
        .collect())
}
