//! The category tree below `c/`, for machine-readable output.
//!
//! Only `_tag` directories are categories; a plain directory below `c/` is a
//! workspace entrance, so it is not part of the tree. Names keep their leading
//! underscore: the prefix is part of the on-disk name and consumers render it
//! verbatim.

use std::fs;
use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::json;
use crate::workspace::{category_tag, valid_name};

/// One category directory and the categories nested below it.
pub struct Category {
    /// Complete directory name, including the leading underscore.
    pub name: String,
    /// Nested categories, ordered by name.
    pub children: Vec<Category>,
}

/// Scan the category tree below `root_dir`, ordered by name at every level.
///
/// A missing `c/` container is an empty tree; a `c/` that exists but is not a
/// directory is a runtime error, matching workspace discovery.
pub fn scan(root_dir: &Path) -> AppResult<Vec<Category>> {
    let container = root_dir.join("c");
    match fs::symlink_metadata(&container) {
        Ok(metadata) if metadata.file_type().is_dir() => scan_level(&container),
        Ok(_) => Err(AppError::runtime(format!(
            "container is not a directory: {}",
            container.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(AppError::runtime(format!(
            "cannot scan '{}': {}",
            container.display(),
            error
        ))),
    }
}

/// Enumerate one directory level, descending into `_tag` directories only.
fn scan_level(directory: &Path) -> AppResult<Vec<Category>> {
    let entries = fs::read_dir(directory).map_err(|error| scan_error(directory, &error))?;
    let mut categories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| scan_error(directory, &error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| scan_error(&path, &error))?;
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
        // A plain directory below `c/` is a workspace, not a category.
        if category_tag(name).is_none() {
            continue;
        }
        categories.push(Category {
            name: name.to_string(),
            children: scan_level(&path)?,
        });
    }
    categories.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(categories)
}

/// Encode the tree as nested `[name, [children]]` pairs.
pub fn to_json(categories: &[Category]) -> String {
    let parts: Vec<String> = categories
        .iter()
        .map(|category| {
            format!(
                "[{},{}]",
                json::quote(&category.name),
                to_json(&category.children)
            )
        })
        .collect();
    format!("[{}]", parts.join(","))
}

/// Attach the failed branch to an enumeration error.
fn scan_error(path: &Path, error: &std::io::Error) -> AppError {
    AppError::runtime(format!("cannot scan '{}': {}", path.display(), error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempTree;

    #[test]
    fn nested_categories_are_reported_with_their_underscores() {
        let tree = TempTree::new("categories");
        tree.dir("c/_ai/_opencode/plugin");
        tree.dir("c/_ai/demo");
        tree.dir("c/_life");
        tree.dir("c/plain-workspace");
        let categories = scan(&tree.root).unwrap();
        assert_eq!(categories.len(), 2);
        assert_eq!(categories[0].name, "_ai");
        assert_eq!(categories[0].children.len(), 1);
        assert_eq!(categories[0].children[0].name, "_opencode");
        assert!(categories[0].children[0].children.is_empty());
        assert_eq!(categories[1].name, "_life");
    }

    #[test]
    fn missing_container_is_an_empty_tree() {
        let tree = TempTree::new("categories-missing");
        assert!(scan(&tree.root).unwrap().is_empty());
    }

    #[test]
    fn invalid_category_names_are_skipped() {
        let tree = TempTree::new("categories-invalid");
        tree.dir("c/_/empty-tag");
        tree.dir("c/_two words/nested");
        tree.dir("c/_ok");
        let categories = scan(&tree.root).unwrap();
        assert_eq!(categories.len(), 1);
        assert_eq!(categories[0].name, "_ok");
    }

    #[test]
    fn json_uses_nested_pairs() {
        let tree = TempTree::new("categories-json");
        tree.dir("c/_ai/_opencode/plugin");
        tree.dir("c/_life");
        let categories = scan(&tree.root).unwrap();
        assert_eq!(
            to_json(&categories),
            "[[\"_ai\",[[\"_opencode\",[]]]],[\"_life\",[]]]"
        );
    }
}
