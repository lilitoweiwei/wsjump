//! The wsjump database: workspace index plus persistent state.
//!
//! One SQLite file holds a `workspace` table that mirrors `w/` and carries each
//! workspace's last access time. Ordering and filtering are expressed as SQL, so
//! this module knows the `Workspace` type rather than being a generic key-value
//! store: ordering *is* a query, and queries live here.
//!
//! The database is a cache plus a small amount of state. It is rebuildable: if
//! deleted, the next sync recreates the index, though access history is lost and
//! every workspace falls back to its creation date.

use std::path::Path;

use rusqlite::{params, Connection, OpenFlags, Transaction};
use time::UtcOffset;

use crate::error::{AppError, AppResult};
use crate::workspace::{format_date, Workspace};

/// Schema version this build expects, stored in SQLite's `user_version`.
const SCHEMA_VERSION: i32 = 1;

/// An open connection to the wsjump database.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open the database at `path`, creating and migrating it as needed.
    ///
    /// A missing file is not an error: it means "first run", and an empty
    /// database is migrated up to the current schema.
    pub fn open(path: &Path) -> AppResult<Db> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|e| sql_err(format!("cannot open database '{}'", path.display()), e))?;

        // WAL lets readers and a writer coexist, which is what makes concurrent
        // wsjump invocations safe without any locking of our own. Setting it
        // reports the resulting mode, so it must be run as a query.
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(|e| sql_err("cannot enable WAL journal mode", e))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(AppError::runtime(format!(
                "database '{}' refused WAL journal mode (got '{}')",
                path.display(),
                mode
            )));
        }

        let db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Bring the schema up to [`SCHEMA_VERSION`].
    ///
    /// Migrations run in a single transaction each, so an interrupted upgrade
    /// leaves the recorded version and the actual schema in agreement.
    fn migrate(&self) -> AppResult<()> {
        let version = self.schema_version()?;
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        // A version we have never written means the file was produced by a newer
        // build. Migrating down is not something we can guess at, so refuse
        // instead of corrupting it; the caller degrades to a database-free path.
        if version > SCHEMA_VERSION {
            return Err(AppError::runtime(format!(
                "database schema version {} is newer than supported version {}",
                version, SCHEMA_VERSION
            )));
        }

        if version < 1 {
            self.migrate_to_v1()?;
        }
        Ok(())
    }

    /// Create the initial schema.
    ///
    /// `last_access` is `NOT NULL`: it is seeded on insert from the workspace's
    /// creation date, so a single column suffices for ordering and no COALESCE or
    /// expression index is needed. The index matches the recency ORDER BY exactly.
    ///
    /// Tags are stored as one flat string rather than a related table. Tag
    /// matching is substring matching, which a string plus LIKE expresses
    /// directly, and it keeps `tags` symmetric with `slug` when building queries.
    fn migrate_to_v1(&self) -> AppResult<()> {
        self.conn
            .execute_batch(
                "BEGIN;
                 CREATE TABLE workspace (
                     id          TEXT PRIMARY KEY,
                     date        TEXT NOT NULL,
                     slug        TEXT NOT NULL,
                     tags        TEXT NOT NULL,
                     desc        TEXT NOT NULL,
                     path        TEXT NOT NULL,
                     last_access INTEGER NOT NULL
                 );
                 CREATE INDEX idx_workspace_recency
                     ON workspace(last_access DESC, id DESC);
                 PRAGMA user_version = 1;
                 COMMIT;",
            )
            .map_err(|e| sql_err("cannot create database schema", e))
    }

    /// Read the schema version recorded in the database header.
    fn schema_version(&self) -> AppResult<i32> {
        self.conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| sql_err("cannot read database schema version", e))
    }

    /// Reconcile the database with the workspaces found on disk.
    ///
    /// Disk is the truth for everything except `last_access`, which exists only
    /// here. Accordingly:
    /// - on disk but not in the database: insert, seeding `last_access` from the
    ///   creation date;
    /// - in both: refresh the mirrored columns but **never** touch
    ///   `last_access`; overwriting it would erase real access history on every
    ///   sync and silently defeat recency ordering;
    /// - in the database but not on disk: delete, since keeping an access time
    ///   for a workspace that no longer exists only leaves an orphan row.
    ///
    /// This is the deliberately dumb implementation: a full rescan every time.
    /// It is correct but pays the cost an index is supposed to avoid, so it is
    /// kept as one replaceable method that callers do not see into.
    pub fn sync(&mut self, workspaces: &[Workspace], offset: UtcOffset) -> AppResult<()> {
        // One transaction, so a failure part-way cannot leave the index holding a
        // half-applied view of the directory.
        let tx = self
            .conn
            .transaction()
            .map_err(|e| sql_err("cannot begin sync transaction", e))?;

        {
            let mut upsert = tx
                .prepare(
                    "INSERT INTO workspace
                         (id, date, slug, tags, desc, path, last_access)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(id) DO UPDATE SET
                         date = excluded.date,
                         slug = excluded.slug,
                         tags = excluded.tags,
                         desc = excluded.desc,
                         path = excluded.path",
                )
                .map_err(|e| sql_err("cannot prepare workspace upsert", e))?;

            for ws in workspaces {
                upsert
                    .execute(params![
                        ws.id,
                        format_date(ws.date),
                        ws.slug,
                        ws.tags_joined(),
                        ws.desc,
                        ws.path.to_string_lossy(),
                        // Only ever consulted by the INSERT branch above; the
                        // UPDATE branch deliberately omits last_access.
                        ws.date_midnight(offset),
                    ])
                    .map_err(|e| sql_err(format!("cannot store workspace '{}'", ws.id), e))?;
            }

            delete_missing(&tx, workspaces)?;        }

        tx.commit()
            .map_err(|e| sql_err("cannot commit sync transaction", e))
    }
}

/// Delete rows whose workspace is no longer on disk.
///
/// The ids present on disk go into a temporary table rather than a giant
/// `NOT IN (?, ?, ...)` list, which would grow the SQL with the number of
/// workspaces and can exceed SQLite's parameter limit.
fn delete_missing(tx: &Transaction<'_>, workspaces: &[Workspace]) -> AppResult<()> {
    // Dropped before returning so repeated syncs on one connection cannot
    // collide on the name.
    tx.execute_batch("CREATE TEMP TABLE present (id TEXT PRIMARY KEY)")
        .map_err(|e| sql_err("cannot create temporary id table", e))?;

    {
        let mut insert = tx
            .prepare("INSERT OR IGNORE INTO present (id) VALUES (?1)")
            .map_err(|e| sql_err("cannot prepare temporary id insert", e))?;
        for ws in workspaces {
            insert
                .execute(params![ws.id])
                .map_err(|e| sql_err("cannot record present workspace id", e))?;
        }
    }

    tx.execute(
        "DELETE FROM workspace
         WHERE id NOT IN (SELECT id FROM present)",
        [],
    )
    .map_err(|e| sql_err("cannot remove deleted workspaces", e))?;

    tx.execute_batch("DROP TABLE present")
        .map_err(|e| sql_err("cannot drop temporary id table", e))?;
    Ok(())
}

/// Wrap a rusqlite error as a runtime `AppError` with our own context.
fn sql_err(context: impl Into<String>, e: rusqlite::Error) -> AppError {
    AppError::runtime(format!("{}: {}", context.into(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use time::{Date, Month};

    /// Open an in-memory database migrated to the current schema.
    fn memory_db() -> Db {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        let db = Db { conn };
        db.migrate().expect("migrate");
        db
    }

    /// UTC+8, the offset used throughout these tests.
    fn offset() -> UtcOffset {
        UtcOffset::from_hms(8, 0, 0).unwrap()
    }

    /// A workspace created on 2026-07-30 with the given slug and tags.
    fn workspace(slug: &str, tags: &[&str]) -> Workspace {
        Workspace {
            id: format!("20260730-{}", slug),
            date: Date::from_calendar_date(2026, Month::July, 30).unwrap(),
            slug: slug.to_string(),
            path: PathBuf::from(format!("G:/ws/w/20260730-{}", slug)),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            desc: String::new(),
            last_access: None,
        }
    }

    /// The stored `last_access` of a row, or None if the row is absent.
    fn stored_last_access(db: &Db, id: &str) -> Option<i64> {
        db.conn
            .query_row(
                "SELECT last_access FROM workspace WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok()
    }

    /// All ids currently in the database, ascending.
    fn stored_ids(db: &Db) -> Vec<String> {
        let mut stmt = db
            .conn
            .prepare("SELECT id FROM workspace ORDER BY id")
            .unwrap();
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        ids
    }

    #[test]
    fn fresh_database_is_migrated_to_current_version() {
        let db = memory_db();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn migrate_is_idempotent() {
        let db = memory_db();
        // Re-running must not fail on "table already exists".
        db.migrate().expect("second migrate");
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn newer_schema_version_is_refused() {
        let db = memory_db();
        db.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(db.migrate().is_err());
    }

    #[test]
    fn sync_seeds_last_access_from_creation_date() {
        let mut db = memory_db();
        let ws = workspace("wsjump", &[]);
        let expected = ws.date_midnight(offset());
        db.sync(&[ws], offset()).unwrap();

        assert_eq!(
            stored_last_access(&db, "20260730-wsjump"),
            Some(expected),
            "a new row's last_access must start at its creation date's midnight"
        );
    }

    #[test]
    fn sync_never_overwrites_last_access() {
        let mut db = memory_db();
        db.sync(&[workspace("wsjump", &[])], offset()).unwrap();

        // Simulate a recorded access far after the creation date.
        let accessed = 1_800_000_000;
        db.conn
            .execute(
                "UPDATE workspace SET last_access = ?1 WHERE id = ?2",
                params![accessed, "20260730-wsjump"],
            )
            .unwrap();

        // Re-syncing must refresh mirrored columns but leave history alone.
        let mut changed = workspace("wsjump", &["tool"]);
        changed.desc = "now with a description".to_string();
        db.sync(&[changed], offset()).unwrap();

        assert_eq!(
            stored_last_access(&db, "20260730-wsjump"),
            Some(accessed),
            "sync must not reset last_access, or recency ordering silently breaks"
        );
        let tags: String = db
            .conn
            .query_row(
                "SELECT tags FROM workspace WHERE id = ?1",
                params!["20260730-wsjump"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tags, "tool", "mirrored columns must still be refreshed");
    }

    #[test]
    fn sync_removes_workspaces_gone_from_disk() {
        let mut db = memory_db();
        db.sync(
            &[workspace("alpha", &[]), workspace("beta", &[])],
            offset(),
        )
        .unwrap();
        assert_eq!(stored_ids(&db).len(), 2);

        db.sync(&[workspace("beta", &[])], offset()).unwrap();
        assert_eq!(stored_ids(&db), vec!["20260730-beta".to_string()]);
    }

    #[test]
    fn sync_of_empty_disk_empties_the_index() {
        let mut db = memory_db();
        db.sync(&[workspace("alpha", &[])], offset()).unwrap();
        db.sync(&[], offset()).unwrap();
        assert!(stored_ids(&db).is_empty());
    }

    #[test]
    fn sync_can_run_twice_in_one_connection() {
        // The temp table used for deletion must not linger and collide.
        let mut db = memory_db();
        db.sync(&[workspace("alpha", &[])], offset()).unwrap();
        db.sync(&[workspace("alpha", &[])], offset()).unwrap();
        assert_eq!(stored_ids(&db).len(), 1);
    }
}
