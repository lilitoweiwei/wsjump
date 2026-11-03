//! Create one workspace, automatically establishing its category containers.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::workspace::{category_tag, valid_name};

/// Create `w/<name>` or `c/<categories>/<name>` and return the absolute path.
///
/// Category paths consist only of `_tag` components. Missing containers and
/// categories are created; existing targets are never overwritten. Empty
/// categories created before an IO failure may remain on disk.
pub fn create_workspace(
    config: &Config,
    name: &str,
    categories: Option<&str>,
) -> AppResult<PathBuf> {
    if !valid_name(name) || (categories.is_some() && name.starts_with('_')) {
        return Err(AppError::usage(format!("invalid workspace name: {name}")));
    }
    let category_parts = match categories {
        Some(path) => {
            let parts: Vec<_> = path.split(['/', '\\']).collect();
            if parts.iter().any(|part| category_tag(part).is_none()) {
                return Err(AppError::usage(
                    "category path must contain only nonempty _tag components without whitespace",
                ));
            }
            parts
        }
        None => Vec::new(),
    };
    if !config.root_dir.is_absolute() || !config.root_dir.is_dir() {
        return Err(AppError::runtime(format!(
            "root_dir is not an existing absolute directory: {}",
            config.root_dir.display()
        )));
    }

    let mut parent = config
        .root_dir
        .join(if categories.is_some() { "c" } else { "w" });
    ensure_directory(&parent)?;
    for part in category_parts {
        parent.push(part);
        ensure_directory(&parent)?;
    }

    let target = parent.join(name);
    fs::create_dir(&target).map_err(|error| {
        let reason = if error.kind() == std::io::ErrorKind::AlreadyExists {
            "workspace already exists".to_string()
        } else {
            format!("cannot create workspace: {error}")
        };
        AppError::runtime(format!("{}: {}", reason, target.display()))
    })?;
    Ok(target)
}

/// Establish one real directory; links and non-directory entries are conflicts.
fn ensure_directory(path: &Path) -> AppResult<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(path)
                .map(|metadata| metadata.file_type().is_dir())
                .unwrap_or(false)
            {
                Ok(())
            } else {
                Err(AppError::runtime(format!(
                    "not a directory: {}",
                    path.display()
                )))
            }
        }
        Err(error) => Err(AppError::runtime(format!(
            "cannot create directory '{}': {}",
            path.display(),
            error
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempTree;
    use crate::workspace::key_for_path;

    #[test]
    fn creates_flat_and_nested_workspaces_with_missing_categories() {
        let tree = TempTree::new("create");
        let config = Config::for_test(&tree.root);
        let flat = create_workspace(&config, "scratch", None).unwrap();
        assert_eq!(flat, tree.root.join("w/scratch"));
        let nested = create_workspace(&config, "plugin", Some("_ai/_opencode")).unwrap();
        assert_eq!(
            key_for_path(&tree.root, &nested).unwrap().as_deref(),
            Some("c/_ai/_opencode/plugin")
        );
        assert!(create_workspace(&config, "plugin", Some("_ai/_opencode")).is_err());
    }

    #[test]
    fn rejects_invalid_names_and_category_paths_before_creating_directories() {
        let tree = TempTree::new("invalid-create");
        let config = Config::for_test(&tree.root);
        for name in ["", "two words", "../escape", "a/b"] {
            assert_eq!(create_workspace(&config, name, None).unwrap_err().code, 2);
        }
        for categories in [
            "",
            "ai",
            "_",
            "_ai/../_tool",
            "_ai/demo",
            "/_ai",
            "_two words",
        ] {
            assert_eq!(
                create_workspace(&config, "demo", Some(categories))
                    .unwrap_err()
                    .code,
                2
            );
        }
        assert_eq!(
            create_workspace(&config, "_hidden", Some("_ai"))
                .unwrap_err()
                .code,
            2
        );
        assert!(!tree.root.join("c").exists());
        assert!(!tree.root.join("w").exists());
    }

    #[test]
    fn category_files_and_existing_workspace_contents_are_not_overwritten() {
        let tree = TempTree::new("conflict");
        tree.write("c/_ai", "category conflict");
        let config = Config::for_test(&tree.root);
        assert!(create_workspace(&config, "demo", Some("_ai/_tool")).is_err());
        tree.write("w/demo/keep", "contents");
        assert!(create_workspace(&config, "demo", None).is_err());
        assert_eq!(
            fs::read_to_string(tree.root.join("w/demo/keep")).unwrap(),
            "contents"
        );
    }
}
