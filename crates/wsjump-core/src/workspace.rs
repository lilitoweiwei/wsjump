//! Workspace identity, tags, ordering and path ownership.

use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::error::{AppError, AppResult};
use crate::query::SortOrder;

/// A workspace discovered on disk or returned from the index.
#[derive(Clone)]
pub struct Workspace {
    /// Root-relative path, using '/' separators; changes when the workspace moves.
    pub key: String,
    /// Complete directory name, used for matching and display.
    pub name: String,
    /// Absolute directory path used for jumping.
    pub path: PathBuf,
    /// Distinct effective tags: ancestor categories followed by file tags.
    pub tags: Vec<String>,
    /// All metadata lines except the first tag line.
    pub desc: String,
    /// Recorded Unix seconds; absent on a database-free scan.
    pub last_access: Option<i64>,
}

impl Workspace {
    /// Serialize tags for SQLite; tags never contain whitespace.
    pub fn tags_joined(&self) -> String {
        self.tags.join(" ")
    }
}

/// Decode the flat tag representation used in SQLite.
pub fn split_tags(joined: &str) -> Vec<String> {
    joined.split_whitespace().map(str::to_string).collect()
}

/// Merge category and metadata tags, preserving the first occurrence of each.
pub fn merge_tags(categories: &[String], metadata: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    categories
        .iter()
        .chain(metadata)
        .filter(|tag| seen.insert(tag.as_str()))
        .cloned()
        .collect()
}

/// Collect distinct tags; recency assumes the input is already in access order.
pub fn all_tags(workspaces: &[Workspace], order: SortOrder) -> Vec<String> {
    match order {
        SortOrder::Recency => {
            let mut seen = HashSet::new();
            workspaces
                .iter()
                .flat_map(|ws| &ws.tags)
                .filter(|tag| seen.insert(tag.as_str()))
                .cloned()
                .collect()
        }
        SortOrder::Name => workspaces
            .iter()
            .flat_map(|ws| ws.tags.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

/// Whether a name is one nonempty directory component without whitespace.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '/' | '\\'))
        && matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
}

/// Extract one category tag, removing exactly one leading underscore.
pub fn category_tag(name: &str) -> Option<&str> {
    let tag = name.strip_prefix('_')?;
    valid_name(tag).then_some(tag)
}

/// Resolve a root-relative path to its workspace boundary and portable key.
///
/// Only the container and category levels are interpreted. Components below the
/// first workspace are project contents, regardless of their names.
pub fn key_for_relative_path(relative: &Path) -> Option<String> {
    let mut parts = relative.components();
    let Component::Normal(container) = parts.next()? else {
        return None;
    };
    let container = container.to_str()?;
    if container != "w" && container != "c" {
        return None;
    }

    let mut key = vec![container];
    for component in parts {
        let Component::Normal(name) = component else {
            return None;
        };
        let name = name.to_str()?;
        if !valid_name(name) {
            return None;
        }
        key.push(name);
        if container == "c" && name.starts_with('_') {
            category_tag(name)?;
        } else {
            return Some(key.join("/"));
        }
    }
    None
}

/// Find the workspace containing an existing path, canonicalizing both sides.
/// Workspace entrances must be actual directories, as in directory discovery.
pub fn key_for_path(root_dir: &Path, path: &Path) -> AppResult<Option<String>> {
    let root = canonical(root_dir)?;
    let target = canonical(path)?;
    let Some(key) = key_under(&root, &target) else {
        return Ok(None);
    };
    let entrance = root.join(&key);
    let is_directory = entrance
        .symlink_metadata()
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false);
    Ok(is_directory.then_some(key))
}

/// Resolve a sync target whose final component may have been deleted.
/// A missing ancestor cannot be canonicalized; use whole-tree sync in that case.
pub fn key_for_possibly_missing_path(root_dir: &Path, path: &Path) -> AppResult<Option<String>> {
    if path.exists() {
        return key_for_path(root_dir, path);
    }

    let Some(name) = path.file_name() else {
        return Ok(None);
    };
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let Ok(parent) = parent.canonicalize() else {
        return Ok(None);
    };
    Ok(key_under(&canonical(root_dir)?, &parent.join(name)))
}

/// Identify a workspace under an already canonicalized root.
fn key_under(root: &Path, target: &Path) -> Option<String> {
    key_for_relative_path(target.strip_prefix(root).ok()?)
}

/// Canonicalize a path with a useful runtime error on failure.
fn canonical(path: &Path) -> AppResult<PathBuf> {
    path.canonicalize().map_err(|error| {
        AppError::runtime(format!(
            "cannot resolve path '{}': {}",
            path.display(),
            error
        ))
    })
}

/// Convert a filesystem timestamp to Unix seconds, including pre-epoch times.
pub fn unix_timestamp(timestamp: SystemTime) -> i64 {
    match timestamp.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(0),
        Err(error) => {
            let duration = error.duration();
            let seconds = i64::try_from(duration.as_secs()).unwrap_or(0);
            (-seconds).saturating_sub(i64::from(duration.subsec_nanos() != 0))
        }
    }
}

/// Current Unix seconds for visit recording.
pub fn now_timestamp() -> i64 {
    unix_timestamp(SystemTime::now())
}

/// Sort by complete name, then unique path key to make the order total.
pub fn sort_by_name(workspaces: &mut [Workspace]) {
    workspaces.sort_by(|a, b| (&a.name, &a.key).cmp(&(&b.name, &b.key)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempTree;

    #[test]
    fn names_and_categories_follow_the_same_rules() {
        for name in ["demo", "20260730-wsjump", "工作区", "_private"] {
            assert!(valid_name(name));
        }
        for name in ["", ".", "..", "two words", "a/b", "a\\b", "a\tb"] {
            assert!(!valid_name(name));
        }
        assert_eq!(category_tag("_ai"), Some("ai"));
        assert_eq!(category_tag("__ai"), Some("_ai"));
        assert_eq!(category_tag("_"), None);
        assert_eq!(category_tag("_two words"), None);
    }

    #[test]
    fn relative_paths_stop_at_the_workspace_boundary() {
        for (path, key) in [
            ("w/demo/src/_cache", Some("w/demo")),
            ("w/_private/src", Some("w/_private")),
            (
                "c/_ai/_opencode/demo/src/_cache",
                Some("c/_ai/_opencode/demo"),
            ),
            ("c/demo", Some("c/demo")),
            ("c/_ai", None),
            ("c/_/demo", None),
            ("bin/demo", None),
            ("w", None),
        ] {
            assert_eq!(key_for_relative_path(Path::new(path)).as_deref(), key);
        }
    }

    #[test]
    fn existing_and_deleted_paths_resolve_to_the_same_key() {
        let tree = TempTree::new("ownership");
        let workspace = tree.dir("c/_ai/_opencode/demo");
        let nested = tree.dir("c/_ai/_opencode/demo/src/_cache");
        assert_eq!(
            key_for_path(&tree.root, &nested).unwrap().as_deref(),
            Some("c/_ai/_opencode/demo")
        );
        assert_eq!(
            key_for_path(&tree.root, &tree.root.join("c/_ai")).unwrap(),
            None
        );
        std::fs::remove_dir_all(&workspace).unwrap();
        assert_eq!(
            key_for_possibly_missing_path(&tree.root, &workspace)
                .unwrap()
                .as_deref(),
            Some("c/_ai/_opencode/demo")
        );
        assert_eq!(
            key_for_possibly_missing_path(&tree.root, &nested).unwrap(),
            None
        );
    }

    #[test]
    fn files_cannot_be_workspace_entrances() {
        let tree = TempTree::new("file-boundary");
        tree.write("w/readme", "text");
        assert_eq!(
            key_for_path(&tree.root, &tree.root.join("w/readme")).unwrap(),
            None
        );
    }

    #[test]
    fn tags_keep_category_order_and_deduplicate_exact_text() {
        let categories = vec!["ai".into(), "tool".into(), "ai".into()];
        let metadata = vec!["tool".into(), "rust".into()];
        assert_eq!(merge_tags(&categories, &metadata), ["ai", "tool", "rust"]);
    }

    #[test]
    fn tag_listing_keeps_access_order_or_sorts_alphabetically() {
        let workspace = |name: &str, tags: &[&str]| Workspace {
            key: format!("w/{name}"),
            name: name.into(),
            path: PathBuf::from(name),
            tags: tags.iter().map(|tag| (*tag).into()).collect(),
            desc: String::new(),
            last_access: None,
        };
        let workspaces = [
            workspace("recent", &["tool", "ai"]),
            workspace("older", &["ai", "rust"]),
        ];
        assert_eq!(
            all_tags(&workspaces, SortOrder::Recency),
            ["tool", "ai", "rust"]
        );
        assert_eq!(
            all_tags(&workspaces, SortOrder::Name),
            ["ai", "rust", "tool"]
        );
    }
}
