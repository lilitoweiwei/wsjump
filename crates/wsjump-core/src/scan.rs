//! Discover flat and categorized workspaces without reading metadata bodies.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::meta;
use crate::signature::{self, MetaSignature};
use crate::workspace::{
    category_tag, key_for_relative_path, merge_tags, unix_timestamp, valid_name, Workspace,
};

/// Structural workspace information and its current metadata-file signature.
pub struct Discovered {
    /// Portable root-relative path key, including the container.
    pub key: String,
    /// Complete workspace directory name.
    pub name: String,
    /// Absolute path for metadata loading and jumping.
    pub path: PathBuf,
    /// Tags inherited from category ancestors, outermost first.
    pub category_tags: Vec<String>,
    /// Current file signature; absent metadata is represented by `None`.
    pub signature: Option<MetaSignature>,
}

impl Discovered {
    /// Read file metadata and combine it with category tags.
    pub fn load(&self) -> Workspace {
        let metadata = meta::load(&self.path);
        Workspace {
            key: self.key.clone(),
            name: self.name.clone(),
            path: self.path.clone(),
            tags: merge_tags(&self.category_tags, &metadata.tags),
            desc: metadata.desc,
            last_access: None,
        }
    }

    /// Initial access time for a new database row; only inserts need this stat.
    pub fn initial_access(&self) -> i64 {
        fs::metadata(&self.path)
            .and_then(|metadata| metadata.created())
            .map(unix_timestamp)
            .unwrap_or(0)
    }
}

/// A complete discovery snapshot, tied to the root that produced it.
pub struct Discovery {
    /// Configured absolute root, used to invalidate paths when the root changes.
    pub root: PathBuf,
    /// Workspaces in filesystem traversal order; queries handle sorting.
    pub workspaces: Vec<Discovered>,
}

/// Enumerate `w/` and category branches of `c/`, stopping at workspace entrances.
///
/// Missing containers are empty. Unreadable branches abort discovery before
/// synchronization can mistakenly delete rows from an incomplete snapshot.
pub fn discover(config: &Config) -> AppResult<Discovery> {
    let root = &config.root_dir;
    if !root.is_absolute() || !root.is_dir() {
        return Err(AppError::runtime(format!(
            "root_dir must be an existing absolute directory: {}",
            root.display()
        )));
    }

    let mut pending = Vec::new();
    for (container, categorized) in [("w", false), ("c", true)] {
        let path = root.join(container);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {
                pending.push((path, Vec::<String>::new(), categorized));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(AppError::runtime(format!(
                    "container is not a directory: {}",
                    path.display()
                )))
            }
            Err(error) => return Err(scan_error(&path, error)),
        }
    }

    let mut workspaces = Vec::new();
    while let Some((directory, category_tags, categorized)) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| scan_error(&directory, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| scan_error(&directory, error))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| scan_error(&path, error))?;
            if !file_type.is_dir() {
                continue;
            }

            let raw_name = entry.file_name();
            let Some(name) = raw_name.to_str().filter(|name| valid_name(name)) else {
                eprintln!(
                    "wsjump: warning: invalid directory name '{}'; skipping",
                    path.display()
                );
                continue;
            };
            if categorized && name.starts_with('_') {
                let Some(tag) = category_tag(name) else {
                    eprintln!(
                        "wsjump: warning: invalid category '{}'; skipping",
                        path.display()
                    );
                    continue;
                };
                let mut inherited = category_tags.clone();
                inherited.push(tag.to_string());
                pending.push((path, inherited, true));
                continue;
            }

            // Category ancestry is encoded in the key. A moved category creates
            // new keys, so unchanged metadata signatures cannot retain old tags.
            let relative = path.strip_prefix(root).expect("entry below root");
            let key = key_for_relative_path(relative).expect("discovered workspace has a key");
            workspaces.push(Discovered {
                key,
                name: name.to_string(),
                signature: signature::read(&path),
                path,
                category_tags: category_tags.clone(),
            });
        }
    }
    Ok(Discovery {
        root: root.clone(),
        workspaces,
    })
}

/// Discover and load every workspace, for callers without an index cache.
pub fn scan(config: &Config) -> AppResult<Vec<Workspace>> {
    Ok(discover(config)?
        .workspaces
        .iter()
        .map(Discovered::load)
        .collect())
}

/// Attach the failed branch to an enumeration error.
fn scan_error(path: &Path, error: std::io::Error) -> AppError {
    AppError::runtime(format!("cannot scan '{}': {}", path.display(), error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempTree;
    use crate::workspace::sort_by_name;

    #[test]
    fn discovery_inherits_tags_and_stops_at_workspace_contents() {
        let tree = TempTree::new("discovery");
        tree.dir("w/20260730-old/src/_ignored/nested");
        tree.dir("w/_private");
        tree.dir("c/_ai/_opencode/plugin/src/_ignored/nested");
        tree.dir("c/_ai/demo");
        tree.dir("c/plain");
        tree.dir("c/_/invalid");
        tree.dir("c/_two words/invalid");
        tree.write(
            "c/_ai/_opencode/plugin/wsmeta.txt",
            ":opencode rust rust\ndescription",
        );
        let mut workspaces = scan(&Config::for_test(&tree.root)).unwrap();
        sort_by_name(&mut workspaces);
        assert_eq!(workspaces.len(), 5);
        let plugin = workspaces.iter().find(|ws| ws.name == "plugin").unwrap();
        assert_eq!(plugin.key, "c/_ai/_opencode/plugin");
        assert_eq!(plugin.tags, ["ai", "opencode", "rust"]);
        assert_eq!(plugin.desc, "description");
        assert_eq!(
            workspaces.iter().find(|ws| ws.name == "demo").unwrap().tags,
            ["ai"]
        );
        assert!(workspaces
            .iter()
            .find(|ws| ws.name == "_private")
            .unwrap()
            .tags
            .is_empty());
    }

    #[test]
    fn missing_containers_are_empty_but_wrong_types_are_errors() {
        let tree = TempTree::new("containers");
        let config = Config::for_test(&tree.root);
        assert!(discover(&config).unwrap().workspaces.is_empty());
        tree.dir("c/_ai/demo");
        assert_eq!(discover(&config).unwrap().workspaces.len(), 1);
        tree.write("w", "not a container");
        assert!(discover(&config).is_err());
    }
}
