//! SQLite workspace cache and access history for the v3 directory model.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::types::ToSql;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row, Transaction};

use crate::error::{AppError, AppResult};
use crate::progress::{Progress, ProgressSink, Reporter};
use crate::query::{SortOrder, Token};
use crate::scan::{Discovered, Discovery};
use crate::signature::{self, MetaSignature};
use crate::workspace::{split_tags, Workspace};

/// Cached parsing semantics and table layout version. Change this when metadata
/// parsing or category rules change; incompatible caches must be rebuilt.
const SCHEMA_VERSION: i32 = 3;

/// Last row of a page; includes the unique key to disambiguate equal names/times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// Access time used by recency ordering.
    last_access: i64,
    /// Complete name used by name ordering.
    name: String,
    /// Unique path key used as the final tiebreaker in both orders.
    key: String,
}

impl Cursor {
    /// Build a resume position from the last workspace actually returned.
    pub fn for_workspace(workspace: &Workspace) -> Self {
        Self {
            last_access: workspace.last_access.unwrap_or_default(),
            name: workspace.name.clone(),
            key: workspace.key.clone(),
        }
    }

    /// Name component of the resume position, also used by scan fallback.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Unique component of the resume position, also used by scan fallback.
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// One query batch and its optional next-page position.
pub struct Page {
    /// Matching workspaces in the requested total order.
    pub workspaces: Vec<Workspace>,
    /// Resume after the last returned row; absent when no further row exists.
    pub next: Option<Cursor>,
}

impl Page {
    /// Whether another page exists.
    pub fn has_more(&self) -> bool {
        self.next.is_some()
    }
}

/// Which records a sync may update or delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncScope {
    /// Reconcile a complete discovery snapshot.
    Everything,
    /// Reconcile only this key; unrelated rows are left intact.
    OnlyWorkspace(String),
}

impl SyncScope {
    /// Whether a discovered key belongs to this synchronization request.
    fn includes(&self, key: &str) -> bool {
        match self {
            Self::Everything => true,
            Self::OnlyWorkspace(only) => only == key,
        }
    }
}

/// Counts for explicit sync reporting and incrementality checks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// In-scope workspaces examined, including unchanged entries.
    pub examined: usize,
    /// Workspaces whose metadata was loaded and stored.
    pub updated: usize,
    /// In-scope rows removed because their keys were absent from disk.
    pub deleted: usize,
}

/// Open v3 database connection; disk owns metadata, SQLite owns access history.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open or initialize a v3 database with WAL concurrency.
    /// Incompatible versions are reported, not migrated or overwritten.
    pub fn open(path: &Path) -> AppResult<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|error| sql_err(format!("cannot open database '{}'", path.display()), error))?;
        let db = Self { conn };
        db.initialize()?;
        let mode: String = db
            .conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(|error| sql_err("cannot enable WAL", error))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(AppError::runtime(format!(
                "database refused WAL mode: {mode}"
            )));
        }
        Ok(db)
    }

    /// Establish the only supported schema; version zero denotes a fresh file.
    fn initialize(&self) -> AppResult<()> {
        let version: i32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| sql_err("cannot read schema version", error))?;
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        if version != 0 {
            return Err(AppError::runtime(format!(
                "unsupported database schema {version}; v3 requires {SCHEMA_VERSION}; use a new db_file or remove the old database to rebuild"
            )));
        }
        // Fold searchable text in Rust: SQLite lower() only folds ASCII, while
        // the scan fallback supports Unicode case-insensitive matching.
        self.conn
            .execute_batch(
                "BEGIN;
             CREATE TABLE workspace (
                 key TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 name_fold TEXT NOT NULL,
                 tags TEXT NOT NULL,
                 tags_fold TEXT NOT NULL,
                 desc TEXT NOT NULL,
                 path TEXT NOT NULL,
                 last_access INTEGER NOT NULL,
                 meta_mtime INTEGER,
                 meta_size INTEGER
             );
             CREATE INDEX idx_workspace_recency ON workspace(last_access DESC, key DESC);
             CREATE INDEX idx_workspace_name ON workspace(name ASC, key ASC);
             CREATE TABLE sync_state (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 root_dir TEXT NOT NULL
             );
             PRAGMA user_version = 3;
             COMMIT;",
            )
            .map_err(|error| sql_err("cannot create v3 schema", error))
    }

    /// Read stored signatures in one query, keyed by portable workspace path.
    pub fn stored_signatures(&self) -> AppResult<HashMap<String, Option<MetaSignature>>> {
        let mut statement = self
            .conn
            .prepare("SELECT key, meta_mtime, meta_size FROM workspace")
            .map_err(|error| sql_err("cannot prepare signature query", error))?;
        let rows = statement
            .query_map([], |row| {
                let key: String = row.get(0)?;
                let mtime: Option<i64> = row.get(1)?;
                let size: Option<i64> = row.get(2)?;
                let signature = match (mtime, size) {
                    (Some(mtime_ticks), Some(size)) => u64::try_from(size)
                        .ok()
                        .map(|size| MetaSignature { mtime_ticks, size }),
                    _ => None,
                };
                Ok((key, signature))
            })
            .map_err(|error| sql_err("cannot query signatures", error))?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(|error| sql_err("cannot read signatures", error))
    }

    /// Root represented by the last complete synchronization, if any.
    pub fn synced_root_dir(&self) -> AppResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT root_dir FROM sync_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql_err("cannot read synchronized root", error))
    }

    /// Reconcile a complete discovery snapshot in one transaction.
    ///
    /// New keys and changed signatures load metadata; forced sync or a changed
    /// root reloads every in-scope row. Updates preserve real access history.
    pub fn sync(
        &mut self,
        found: &Discovery,
        scope: &SyncScope,
        force: bool,
        sink: &mut dyn ProgressSink,
    ) -> AppResult<SyncOutcome> {
        let stored = self.stored_signatures()?;
        let root = found.root.to_string_lossy().into_owned();
        let force = force || self.synced_root_dir()?.as_deref() != Some(&root);
        let mut outcome = SyncOutcome::default();
        let mut reporter = Reporter::new(sink);
        let total = found
            .workspaces
            .iter()
            .filter(|entry| scope.includes(&entry.key))
            .count();
        let tx = self
            .conn
            .transaction()
            .map_err(|error| sql_err("cannot begin sync", error))?;

        {
            let mut upsert = tx.prepare(
                "INSERT INTO workspace
                     (key, name, name_fold, tags, tags_fold, desc, path, last_access, meta_mtime, meta_size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(key) DO UPDATE SET
                     name = excluded.name, name_fold = excluded.name_fold,
                     tags = excluded.tags, tags_fold = excluded.tags_fold,
                     desc = excluded.desc, path = excluded.path,
                     meta_mtime = excluded.meta_mtime, meta_size = excluded.meta_size"
            ).map_err(|error| sql_err("cannot prepare workspace upsert", error))?;
            for entry in &found.workspaces {
                if !scope.includes(&entry.key) {
                    continue;
                }
                outcome.examined += 1;
                reporter.advance(Progress {
                    done: outcome.examined,
                    total,
                });
                let known = stored.contains_key(&entry.key);
                if known && !force && !signature::is_stale(stored[&entry.key], entry.signature) {
                    continue;
                }

                let workspace = entry.load();
                let tags = workspace.tags_joined();
                let (mtime, size) = signature_columns(entry.signature);
                // This value is ignored by the UPDATE branch. Avoid stat'ing
                // directory creation times for already-known workspaces.
                let initial_access = if known { 0 } else { entry.initial_access() };
                upsert
                    .execute(params![
                        workspace.key,
                        workspace.name,
                        workspace.name.to_lowercase(),
                        tags,
                        tags.to_lowercase(),
                        workspace.desc,
                        workspace.path.to_string_lossy(),
                        initial_access,
                        mtime,
                        size,
                    ])
                    .map_err(|error| sql_err(format!("cannot store '{}'", workspace.key), error))?;
                outcome.updated += 1;
            }
        }

        outcome.deleted = delete_missing(&tx, &found.workspaces, scope)?;
        // A single-workspace sync cannot certify the rest of a changed root.
        if matches!(scope, SyncScope::Everything) {
            tx.execute(
                "INSERT INTO sync_state (singleton, root_dir) VALUES (1, ?1)
                 ON CONFLICT(singleton) DO UPDATE SET root_dir = excluded.root_dir",
                params![root],
            )
            .map_err(|error| sql_err("cannot record synchronized root", error))?;
        }
        tx.commit()
            .map_err(|error| sql_err("cannot commit sync", error))?;
        reporter.finish();
        Ok(outcome)
    }

    /// Query one page by name/tag substring, with AND across tokens.
    /// Keyset paging includes the unique key so duplicate names never lose rows.
    pub fn query(
        &self,
        tokens: &[Token],
        sort: SortOrder,
        limit: usize,
        after: Option<&Cursor>,
    ) -> AppResult<Page> {
        let mut conditions = Vec::new();
        let mut binds: Vec<Box<dyn ToSql>> = Vec::new();
        for token in tokens {
            let (clause, needle) = match token {
                Token::Tag(needle) => ("(tags <> '' AND tags_fold LIKE ? ESCAPE '\\')", needle),
                Token::Text(needle) => (
                    "(name_fold LIKE ? ESCAPE '\\' OR tags_fold LIKE ? ESCAPE '\\')",
                    needle,
                ),
            };
            // Names and individual tags contain no whitespace. Matching the
            // joined tag string across its separator would disagree with scan.
            if needle.chars().any(char::is_whitespace) {
                conditions.push("0".to_string());
                continue;
            }
            for _ in 0..clause.matches('?').count() {
                binds.push(Box::new(like_pattern(needle)));
            }
            conditions.push(clause.to_string());
        }
        if let Some(cursor) = after {
            match sort {
                SortOrder::Recency => {
                    conditions.push("(last_access, key) < (?, ?)".to_string());
                    binds.push(Box::new(cursor.last_access));
                }
                SortOrder::Name => {
                    conditions.push("(name, key) > (?, ?)".to_string());
                    binds.push(Box::new(cursor.name.clone()));
                }
            }
            binds.push(Box::new(cursor.key.clone()));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let order = match sort {
            SortOrder::Recency => "last_access DESC, key DESC",
            SortOrder::Name => "name ASC, key ASC",
        };
        let sql = format!(
            "SELECT key, name, tags, desc, path, last_access FROM workspace {where_clause} ORDER BY {order} LIMIT ?"
        );
        binds.push(Box::new(limit as i64 + 1));
        let bind_refs: Vec<&dyn ToSql> = binds.iter().map(|value| value.as_ref()).collect();
        let mut statement = self
            .conn
            .prepare(&sql)
            .map_err(|error| sql_err("cannot prepare query", error))?;
        let mut workspaces = statement
            .query_map(bind_refs.as_slice(), read_workspace)
            .map_err(|error| sql_err("cannot query workspaces", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql_err("cannot read workspaces", error))?;

        let next = if workspaces.len() > limit {
            workspaces.truncate(limit);
            workspaces.last().map(Cursor::for_workspace)
        } else {
            None
        };
        Ok(Page { workspaces, next })
    }

    /// Record a visit; false means the key is absent from the index.
    pub fn touch(&self, key: &str, timestamp: i64) -> AppResult<bool> {
        self.conn
            .execute(
                "UPDATE workspace SET last_access = ?1 WHERE key = ?2",
                params![timestamp, key],
            )
            .map(|updated| updated > 0)
            .map_err(|error| sql_err(format!("cannot record visit to '{key}'"), error))
    }
}

/// Escape SQL LIKE wildcards so every query remains a literal substring match.
fn like_pattern(needle: &str) -> String {
    let mut pattern = String::from("%");
    for character in needle.to_lowercase().chars() {
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

/// Convert a file signature into nullable SQLite columns.
fn signature_columns(signature: Option<MetaSignature>) -> (Option<i64>, Option<i64>) {
    match signature {
        Some(signature) => match i64::try_from(signature.size) {
            Ok(size) => (Some(signature.mtime_ticks), Some(size)),
            Err(_) => (None, None),
        },
        None => (None, None),
    }
}

/// Decode a selected workspace row, retaining database access history.
fn read_workspace(row: &Row<'_>) -> rusqlite::Result<Workspace> {
    let tags: String = row.get(2)?;
    Ok(Workspace {
        key: row.get(0)?,
        name: row.get(1)?,
        tags: split_tags(&tags),
        desc: row.get(3)?,
        path: PathBuf::from(row.get::<_, String>(4)?),
        last_access: Some(row.get(5)?),
    })
}

/// Delete absent keys in scope without exceeding SQLite's bound-variable limit.
fn delete_missing(
    tx: &Transaction<'_>,
    found: &[Discovered],
    scope: &SyncScope,
) -> AppResult<usize> {
    tx.execute_batch("CREATE TEMP TABLE present (key TEXT PRIMARY KEY)")
        .map_err(|error| sql_err("cannot create present-key table", error))?;
    {
        let mut insert = tx
            .prepare("INSERT INTO present (key) VALUES (?1)")
            .map_err(|error| sql_err("cannot prepare present-key insert", error))?;
        for entry in found {
            insert
                .execute(params![entry.key])
                .map_err(|error| sql_err("cannot record present key", error))?;
        }
    }
    let deleted = match scope {
        SyncScope::Everything => tx.execute(
            "DELETE FROM workspace WHERE key NOT IN (SELECT key FROM present)",
            [],
        ),
        SyncScope::OnlyWorkspace(key) => tx.execute(
            "DELETE FROM workspace WHERE key = ?1 AND key NOT IN (SELECT key FROM present)",
            params![key],
        ),
    }
    .map_err(|error| sql_err("cannot remove absent workspaces", error))?;
    tx.execute_batch("DROP TABLE present")
        .map_err(|error| sql_err("cannot drop present-key table", error))?;
    Ok(deleted)
}

/// Add operation context to a SQLite failure.
fn sql_err(context: impl Into<String>, error: rusqlite::Error) -> AppError {
    AppError::runtime(format!("{}: {}", context.into(), error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::filter;
    use crate::progress::SilentSink;
    use crate::query;
    use crate::scan;
    use crate::test_support::TempTree;
    use crate::workspace::sort_by_name;

    /// A real-tree fixture paired with an isolated in-memory index.
    struct Fixture {
        tree: TempTree,
        db: Db,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let db = Db {
                conn: Connection::open_in_memory().unwrap(),
            };
            db.initialize().unwrap();
            Self {
                tree: TempTree::new(label),
                db,
            }
        }

        fn sync(&mut self, force: bool, scope: SyncScope) -> SyncOutcome {
            let found = scan::discover(&Config::for_test(&self.tree.root)).unwrap();
            self.db
                .sync(&found, &scope, force, &mut SilentSink)
                .unwrap()
        }

        fn all(&self, sort: SortOrder) -> Vec<Workspace> {
            self.db.query(&[], sort, 100, None).unwrap().workspaces
        }
    }

    #[test]
    fn initializes_v3_and_refuses_incompatible_schemas_without_erasing_them() {
        let fixture = Fixture::new("schema");
        fixture.db.initialize().unwrap();
        let version: i32 = fixture
            .db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        fixture
            .db
            .conn
            .execute_batch("PRAGMA user_version = 2")
            .unwrap();
        assert!(fixture.db.initialize().is_err());
        let version: i32 = fixture
            .db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn steady_sync_skips_reads_and_metadata_deletion_keeps_category_tags() {
        let mut fixture = Fixture::new("metadata");
        fixture
            .tree
            .write("c/_ai/demo/wsmeta.txt", ":ai rust rust\ndescription");
        fixture.tree.dir("w/bare");
        assert_eq!(fixture.sync(false, SyncScope::Everything).updated, 2);
        assert_eq!(fixture.sync(false, SyncScope::Everything).updated, 0);
        assert!(fixture.db.touch("c/_ai/demo", 42).unwrap());
        fixture
            .tree
            .write("c/_ai/demo/wsmeta.txt", ":tool\nnew description");
        assert_eq!(fixture.sync(false, SyncScope::Everything).updated, 1);
        std::fs::remove_file(fixture.tree.root.join("c/_ai/demo/wsmeta.txt")).unwrap();
        assert_eq!(fixture.sync(false, SyncScope::Everything).updated, 1);
        let demo = fixture
            .all(SortOrder::Name)
            .into_iter()
            .find(|ws| ws.name == "demo")
            .unwrap();
        assert_eq!(demo.tags, ["ai"]);
        assert!(demo.desc.is_empty());
        assert_eq!(demo.last_access, Some(42));
    }

    #[test]
    fn new_rows_seed_access_from_the_directory_creation_time() {
        let mut fixture = Fixture::new("initial-access");
        fixture.tree.dir("w/demo");
        let found = scan::discover(&Config::for_test(&fixture.tree.root)).unwrap();
        let expected = found.workspaces[0].initial_access();
        fixture
            .db
            .sync(&found, &SyncScope::Everything, false, &mut SilentSink)
            .unwrap();
        assert_eq!(fixture.all(SortOrder::Name)[0].last_access, Some(expected));
        assert!(!fixture.db.touch("w/missing", 123).unwrap());
    }

    #[test]
    fn category_rename_creates_new_keys_and_refreshes_tags_with_unchanged_files() {
        let mut fixture = Fixture::new("category-move");
        fixture.tree.write("c/_ai/demo/wsmeta.txt", ":tool");
        fixture.sync(false, SyncScope::Everything);
        fixture.db.touch("c/_ai/demo", 42).unwrap();
        let old_signature = scan::discover(&Config::for_test(&fixture.tree.root))
            .unwrap()
            .workspaces[0]
            .signature;
        std::fs::rename(
            fixture.tree.root.join("c/_ai"),
            fixture.tree.root.join("c/_model"),
        )
        .unwrap();
        let found = scan::discover(&Config::for_test(&fixture.tree.root)).unwrap();
        assert_eq!(found.workspaces[0].signature, old_signature);
        let outcome = fixture
            .db
            .sync(&found, &SyncScope::Everything, false, &mut SilentSink)
            .unwrap();
        assert_eq!((outcome.updated, outcome.deleted), (1, 1));
        let demo = &fixture.all(SortOrder::Name)[0];
        assert_eq!(demo.key, "c/_model/demo");
        assert_eq!(demo.tags, ["model", "tool"]);
        assert_ne!(demo.last_access, Some(42));
    }

    #[test]
    fn changed_root_refreshes_paths_even_when_signatures_are_absent() {
        let mut fixture = Fixture::new("root-old");
        fixture.tree.dir("w/demo");
        fixture.sync(false, SyncScope::Everything);
        fixture.db.touch("w/demo", 42).unwrap();
        let new_tree = TempTree::new("root-new");
        new_tree.dir("w/demo");
        let found = scan::discover(&Config::for_test(&new_tree.root)).unwrap();
        assert_eq!(
            fixture
                .db
                .sync(&found, &SyncScope::Everything, false, &mut SilentSink)
                .unwrap()
                .updated,
            1
        );
        let demo = &fixture.all(SortOrder::Name)[0];
        assert_eq!(demo.path, new_tree.root.join("w/demo"));
        assert_eq!(demo.last_access, Some(42));
    }

    #[test]
    fn scoped_sync_updates_or_deletes_only_its_workspace_and_does_not_certify_root() {
        let mut fixture = Fixture::new("scoped");
        fixture.tree.write("c/_ai/a/wsmeta.txt", ":before");
        fixture.tree.write("w/b/wsmeta.txt", ":before");
        fixture.sync(false, SyncScope::OnlyWorkspace("c/_ai/a".into()));
        assert_eq!(fixture.db.synced_root_dir().unwrap(), None);
        fixture.sync(false, SyncScope::Everything);
        fixture.tree.write("c/_ai/a/wsmeta.txt", ":after");
        fixture.tree.write("w/b/wsmeta.txt", ":after");
        fixture.sync(true, SyncScope::OnlyWorkspace("c/_ai/a".into()));
        assert_eq!(fixture.all(SortOrder::Name)[1].tags, ["before"]);
        std::fs::remove_dir_all(fixture.tree.root.join("c/_ai/a")).unwrap();
        assert_eq!(
            fixture
                .sync(true, SyncScope::OnlyWorkspace("c/_ai/a".into()))
                .deleted,
            1
        );
        assert_eq!(fixture.all(SortOrder::Name)[0].key, "w/b");
    }

    #[test]
    fn sql_and_scan_matching_agree_for_unicode_wildcards_empty_tags_and_whitespace() {
        let mut fixture = Fixture::new("matching");
        fixture
            .tree
            .write("c/_AI/Ä-demo/wsmeta.txt", ":ÖTool rust tool\nnot-searched");
        fixture.tree.dir("w/20260730-a_b");
        fixture.sync(false, SyncScope::Everything);
        let mut scanned = scan::scan(&Config::for_test(&fixture.tree.root)).unwrap();
        sort_by_name(&mut scanned);
        for text in [
            "",
            "ä",
            ":ötool",
            ":ai",
            "20260730",
            "a_b",
            "%",
            ":",
            ":rust :tool",
            "not-searched",
            "' OR 1=1",
        ] {
            let tokens = query::parse_str(text);
            let expected: Vec<_> = filter::filter(&scanned, &tokens)
                .iter()
                .map(|ws| ws.key.as_str())
                .collect();
            let actual = fixture
                .db
                .query(&tokens, SortOrder::Name, 100, None)
                .unwrap();
            assert_eq!(
                actual
                    .workspaces
                    .iter()
                    .map(|ws| ws.key.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "{text}"
            );
        }
        let tokens = vec![Token::Tag("rust tool".into())];
        assert!(fixture
            .db
            .query(&tokens, SortOrder::Name, 100, None)
            .unwrap()
            .workspaces
            .is_empty());
    }

    #[test]
    fn duplicate_names_and_access_times_page_without_skips_or_repeats() {
        let mut fixture = Fixture::new("paging");
        for path in ["c/_ai/demo", "c/_nvim/demo", "w/demo", "w/alpha"] {
            fixture.tree.dir(path);
        }
        fixture.sync(false, SyncScope::Everything);
        for workspace in fixture.all(SortOrder::Name) {
            fixture.db.touch(&workspace.key, 42).unwrap();
        }
        for sort in [SortOrder::Name, SortOrder::Recency] {
            let expected: Vec<_> = fixture.all(sort).into_iter().map(|ws| ws.key).collect();
            let mut keys = Vec::new();
            let mut cursor = None;
            loop {
                let page = fixture.db.query(&[], sort, 1, cursor.as_ref()).unwrap();
                keys.extend(page.workspaces.into_iter().map(|ws| ws.key));
                match page.next {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            assert_eq!(keys, expected);
        }
        fixture.db.touch("w/demo", 100).unwrap();
        assert_eq!(fixture.all(SortOrder::Recency)[0].key, "w/demo");
    }

    #[test]
    fn incremental_and_forced_sync_converge_after_structural_and_file_changes() {
        let mut fixture = Fixture::new("equivalence");
        fixture.tree.write("c/_ai/demo/wsmeta.txt", ":old");
        fixture.tree.dir("w/gone");
        fixture.sync(false, SyncScope::Everything);
        fixture.db.touch("c/_ai/demo", 42).unwrap();
        fixture
            .tree
            .write("c/_ai/demo/wsmeta.txt", ":updated\ndescription");
        fixture.tree.write("c/_new/added/wsmeta.txt", ":fresh");
        std::fs::remove_dir_all(fixture.tree.root.join("w/gone")).unwrap();
        fixture.sync(false, SyncScope::Everything);
        let before: Vec<_> = fixture
            .all(SortOrder::Name)
            .into_iter()
            .map(|ws| (ws.key, ws.tags, ws.desc, ws.path, ws.last_access))
            .collect();
        fixture.sync(true, SyncScope::Everything);
        let after: Vec<_> = fixture
            .all(SortOrder::Name)
            .into_iter()
            .map(|ws| (ws.key, ws.tags, ws.desc, ws.path, ws.last_access))
            .collect();
        assert_eq!(before, after);
        assert_eq!(fixture.sync(false, SyncScope::Everything).updated, 0);
    }

    #[test]
    fn same_size_edits_with_older_timestamps_are_reread_and_forced_sync_repairs_cache() {
        let mut fixture = Fixture::new("timestamp-edit");
        fixture.tree.write("c/_ai/demo/wsmeta.txt", ":old");
        fixture.sync(false, SyncScope::Everything);
        fixture.tree.write("c/_ai/demo/wsmeta.txt", ":new");
        let file = std::fs::File::options()
            .write(true)
            .open(fixture.tree.root.join("c/_ai/demo/wsmeta.txt"))
            .unwrap();
        file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .unwrap();
        drop(file);
        assert_eq!(fixture.sync(false, SyncScope::Everything).updated, 1);
        assert_eq!(fixture.all(SortOrder::Name)[0].tags, ["ai", "new"]);
        fixture
            .db
            .conn
            .execute(
                "UPDATE workspace SET tags = 'corrupt', tags_fold = 'corrupt'",
                [],
            )
            .unwrap();
        assert_eq!(fixture.sync(false, SyncScope::Everything).updated, 0);
        assert_eq!(fixture.all(SortOrder::Name)[0].tags, ["corrupt"]);
        assert_eq!(fixture.sync(true, SyncScope::Everything).updated, 1);
        assert_eq!(fixture.all(SortOrder::Name)[0].tags, ["ai", "new"]);
    }
}
