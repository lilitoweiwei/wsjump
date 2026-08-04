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

use rusqlite::{Connection, OpenFlags};

use crate::error::{AppError, AppResult};

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
}

/// Wrap a rusqlite error as a runtime `AppError` with our own context.
fn sql_err(context: impl Into<String>, e: rusqlite::Error) -> AppError {
    AppError::runtime(format!("{}: {}", context.into(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open an in-memory database migrated to the current schema.
    fn memory_db() -> Db {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        let db = Db { conn };
        db.migrate().expect("migrate");
        db
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
}
