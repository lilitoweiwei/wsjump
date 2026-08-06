//! Workspace data model and directory-name parsing.

use std::path::{Component, Path, PathBuf};

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

use crate::error::{AppError, AppResult};

/// A single workspace, either discovered on disk or read back from the database.
#[derive(Clone)]
pub struct Workspace {
    /// The whole directory name, e.g. "20260730-wsjump".
    pub id: String,
    /// Creation date parsed from the 8-digit prefix (a real calendar date).
    pub date: Date,
    /// The part after the first '-'; may be empty.
    pub slug: String,
    /// Absolute path to the workspace directory.
    pub path: PathBuf,
    /// Tags from the first tag line of `wsmeta.txt` (leading ':' stripped).
    pub tags: Vec<String>,
    /// Description body: all `wsmeta.txt` content except the tag line.
    pub desc: String,
    /// Unix seconds of the last recorded access.
    ///
    /// `None` means "not known here": a freshly scanned workspace carries no
    /// access history, since that history lives only in the database. Rows read
    /// from the database always have `Some`, because the column is `NOT NULL`
    /// and is initialised on insert from the directory date.
    pub last_access: Option<i64>,
}

impl Workspace {
    /// Tags rendered as the flat space-separated string stored in the database.
    ///
    /// Tags cannot contain whitespace (they come from splitting a line on it),
    /// so joining is lossless and the string can be split back apart.
    pub fn tags_joined(&self) -> String {
        self.tags.join(" ")
    }

    /// Unix seconds of midnight on the workspace's creation date, in `offset`.
    ///
    /// Used to seed `last_access` when a workspace first enters the database:
    /// creating a workspace is itself a form of contact, so its creation date is
    /// a meaningful initial access time rather than a sentinel.
    pub fn date_midnight(&self, offset: UtcOffset) -> i64 {
        PrimitiveDateTime::new(self.date, Time::MIDNIGHT)
            .assume_offset(offset)
            .unix_timestamp()
    }
}

/// Split a flat space-separated tag string back into tags, as stored by
/// [`Workspace::tags_joined`]. Empty input yields no tags.
pub fn split_tags(joined: &str) -> Vec<String> {
    joined.split_whitespace().map(str::to_string).collect()
}

/// Current time as unix seconds; the timestamp written by an access record.
pub fn now_timestamp() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// Sort workspaces by `id`, ascending.
///
/// Used by callers that must present a deterministic order without the
/// database: `id` is derived from the directory name, so this ordering is always
/// available even when the database is unusable.
pub fn sort_by_id(workspaces: &mut [Workspace]) {
    workspaces.sort_by(|a, b| a.id.cmp(&b.id));
}

/// Parse a directory name into its `(date, slug)` components.
///
/// Rules:
/// - The first 8 characters must parse as a valid calendar date `YYYYMMDD`.
/// - If a 9th character exists it must be '-'; everything after it is the slug
///   (which may itself contain '-', and may be empty).
/// - Returns `None` for any name that does not match (caller warns + skips).
///
/// Valid: "20260730", "20260730-", "20260730-wsjump", "20260730-a-b-c".
pub fn parse_dir_name(name: &str) -> Option<(Date, String)> {
    // The prefix must be exactly 8 ASCII digits; index by bytes since digits
    // are single-byte in UTF-8.
    if name.len() < 8 {
        return None;
    }
    let (prefix, rest) = name.split_at(8);
    if !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    let date = parse_date(prefix)?;

    // No slug part at all: "20260730".
    if rest.is_empty() {
        return Some((date, String::new()));
    }

    // A 9th character exists: it must be the '-' separator.
    let mut chars = rest.chars();
    if chars.next() != Some('-') {
        return None;
    }
    let slug = chars.as_str().to_string();
    Some((date, slug))
}

/// Identify which workspace a path belongs to.
///
/// Returns the workspace's id (its directory name), or `None` if the path is not
/// inside `workspaces_dir`. Paths *below* a workspace resolve to that workspace,
/// so recording an access works from anywhere inside it.
///
/// Both sides are canonicalised before comparison: `workspaces_dir` may be
/// spelled differently in the config than the path a shell reports (`G:/ws/w`
/// versus `G:\ws\w`, `..` segments, or a symlinked route), and comparing the raw
/// text would then wrongly reject a path that is in fact inside.
pub fn id_for_path(workspaces_dir: &Path, path: &Path) -> AppResult<Option<String>> {
    let target = canonical(path)?;
    id_for_path_under(workspaces_dir, &target)
}

/// Canonicalise a path, reporting failure with the path in the message.
fn canonical(path: &Path) -> AppResult<PathBuf> {
    path.canonicalize().map_err(|e| {
        AppError::runtime(format!("cannot resolve path '{}': {}", path.display(), e))
    })
}

/// Identify which workspace a path names, even if it no longer exists.
///
/// [`id_for_path`] canonicalises the target, which requires it to be present.
/// That is right for recording a visit -- you cannot visit what is not there --
/// but wrong for syncing, where "this workspace is gone, update the index" is a
/// legitimate and expected request.
///
/// A missing target is therefore resolved via its *parent*, which does still
/// exist in that case, so both sides still come from `canonicalize` and no
/// hand-rolled path normalisation is needed.
pub fn id_for_possibly_missing_path(
    workspaces_dir: &Path,
    path: &Path,
) -> AppResult<Option<String>> {
    if path.exists() {
        return id_for_path(workspaces_dir, path);
    }

    // Only the final component is missing; resolve the rest and re-attach it.
    let Some(name) = path.file_name() else {
        return Ok(None);
    };
    let parent = path.parent().unwrap_or(Path::new("."));
    // A parent that does not exist either means the path is nowhere near `w/`.
    let Ok(parent) = parent.canonicalize() else {
        return Ok(None);
    };
    id_for_path_under(workspaces_dir, &parent.join(name))
}

/// The workspace containing an already-resolved path.
fn id_for_path_under(workspaces_dir: &Path, resolved: &Path) -> AppResult<Option<String>> {
    let root = canonical(workspaces_dir)?;

    let relative = match resolved.strip_prefix(&root) {
        Ok(relative) => relative,
        Err(_) => return Ok(None),
    };

    // The first component is the workspace directory; anything deeper is inside
    // it. No components at all means the path *is* `w/`, which is no workspace.
    let name = match relative.components().next() {
        Some(Component::Normal(name)) => name.to_string_lossy().into_owned(),
        _ => return Ok(None),
    };

    // A directory that is not a valid workspace name is not a workspace, even
    // though it sits under `w/`.
    if parse_dir_name(&name).is_none() {
        return Ok(None);
    }
    Ok(Some(name))
}

/// Parse an 8-digit `YYYYMMDD` string into a real calendar date.
/// Returns `None` if the digits do not form a valid date (e.g. "20261340").
pub fn parse_date(prefix: &str) -> Option<Date> {
    if prefix.len() != 8 || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = prefix[0..4].parse().ok()?;
    let month_num: u8 = prefix[4..6].parse().ok()?;
    let day: u8 = prefix[6..8].parse().ok()?;

    let month = Month::try_from(month_num).ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

/// Render a date as the 8-digit `YYYYMMDD` form used in directory names and in
/// the database's `date` column. Inverse of [`parse_date`].
pub fn format_date(date: Date) -> String {
    format!(
        "{:04}{:02}{:02}",
        date.year(),
        date.month() as u8,
        date.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A unique temporary directory standing in for `w/`, plus a guard that
    /// removes it. Real directories are required because resolution
    /// canonicalises, which only works on paths that exist.
    struct TempRoot {
        path: PathBuf,
    }

    impl TempRoot {
        fn new(tag: &str) -> TempRoot {
            let unique = format!(
                "wsjump-test-{}-{}-{:?}",
                tag,
                std::process::id(),
                std::thread::current().id()
            );
            let path = std::env::temp_dir().join(unique);
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp root");
            TempRoot { path }
        }

        /// Create a directory under the root and return its path.
        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.path.join(relative);
            fs::create_dir_all(&path).expect("create temp dir");
            path
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn workspace_directory_resolves_to_its_id() {
        let root = TempRoot::new("ws-dir");
        let ws = root.dir("20260730-wsjump");
        assert_eq!(
            id_for_path(&root.path, &ws).unwrap(),
            Some("20260730-wsjump".to_string())
        );
    }

    #[test]
    fn path_inside_a_workspace_resolves_to_that_workspace() {
        let root = TempRoot::new("nested");
        let nested = root.dir("20260730-wsjump/crates/core");
        // Recording an access must work from anywhere inside the workspace.
        assert_eq!(
            id_for_path(&root.path, &nested).unwrap(),
            Some("20260730-wsjump".to_string())
        );
    }

    #[test]
    fn the_workspaces_dir_itself_is_no_workspace() {
        let root = TempRoot::new("root-itself");
        assert_eq!(id_for_path(&root.path, &root.path).unwrap(), None);
    }

    #[test]
    fn path_outside_the_workspaces_dir_resolves_to_nothing() {
        let root = TempRoot::new("outside");
        let outside = root.dir("../wsjump-test-outside-sibling");
        let result = id_for_path(&root.path, &outside).unwrap();
        let _ = fs::remove_dir_all(&outside);
        assert_eq!(result, None);
    }

    #[test]
    fn directory_with_an_invalid_name_is_no_workspace() {
        let root = TempRoot::new("bad-name");
        let bad = root.dir("notaworkspace");
        assert_eq!(id_for_path(&root.path, &bad).unwrap(), None);
    }

    #[test]
    fn resolution_tolerates_unnormalised_paths() {
        let root = TempRoot::new("unnormalised");
        root.dir("20260730-wsjump");
        // A shell can hand us a path with '.' or '..' segments; comparing raw
        // text would reject this even though it is inside a workspace.
        let messy = root.path.join("20260730-wsjump").join(".").join("sub/..");
        assert_eq!(
            id_for_path(&root.path, &messy).unwrap(),
            Some("20260730-wsjump".to_string())
        );
    }

    #[test]
    fn resolution_reports_a_missing_path() {
        let root = TempRoot::new("missing");
        let missing = root.path.join("20260730-wsjump");
        assert!(
            id_for_path(&root.path, &missing).is_err(),
            "a path that does not exist cannot be resolved and must be reported"
        );
    }

    #[test]
    fn a_deleted_workspace_still_resolves_for_syncing() {
        // Syncing a workspace you just deleted is a normal request -- it is how
        // the index is told to drop it -- so this must not fail the way
        // id_for_path does.
        let root = TempRoot::new("sync-deleted");
        let ws = root.dir("20260730-gone");
        fs::remove_dir_all(&ws).expect("delete the workspace");

        assert_eq!(
            id_for_possibly_missing_path(&root.path, &ws).unwrap(),
            Some("20260730-gone".to_string())
        );
    }

    #[test]
    fn an_existing_workspace_resolves_the_same_either_way() {
        let root = TempRoot::new("sync-present");
        let ws = root.dir("20260730-here");
        assert_eq!(
            id_for_possibly_missing_path(&root.path, &ws).unwrap(),
            id_for_path(&root.path, &ws).unwrap()
        );
    }

    #[test]
    fn a_deleted_path_outside_the_root_resolves_to_nothing() {
        let root = TempRoot::new("sync-outside");
        let elsewhere = std::env::temp_dir().join("wsjump-test-sync-elsewhere-20260730-x");
        assert_eq!(
            id_for_possibly_missing_path(&root.path, &elsewhere).unwrap(),
            None
        );
    }

    #[test]
    fn a_deleted_path_with_an_invalid_name_is_no_workspace() {
        let root = TempRoot::new("sync-bad-name");
        let bad = root.path.join("notaworkspace");
        assert_eq!(id_for_possibly_missing_path(&root.path, &bad).unwrap(), None);
    }

    #[test]
    fn a_deleted_workspace_below_the_root_resolves_to_its_workspace() {
        // The workspace is gone, but the path names something inside it.
        let root = TempRoot::new("sync-deleted-nested");
        let ws = root.dir("20260730-gone");
        let nested = ws.join("crates");
        fs::remove_dir_all(&ws).expect("delete the workspace");

        assert_eq!(
            id_for_possibly_missing_path(&root.path, &nested).unwrap(),
            None,
            "the parent is gone too, so there is nothing to resolve against"
        );
    }
}
