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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::types::ToSql;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row, Transaction};
use time::UtcOffset;

use crate::error::{AppError, AppResult};
use crate::query::{SortOrder, Token};
use crate::signature::MetaSignature;
use crate::workspace::{format_date, parse_date, split_tags, Workspace};

/// Schema version this build expects, stored in SQLite's `user_version`.
///
/// # Bump this when `meta::parse` changes
///
/// The `workspace` table caches the *result* of parsing `wsmeta.txt`, and sync
/// re-parses a file only when its [`crate::signature`] differs from the stored
/// one. So changing the parsing rules does not fix rows already stored: their
/// signature still matches and they are never re-read, keeping the old parse
/// forever. Any such change must bump this version and clear the stored
/// signatures in the migration, which forces a full re-read.
const SCHEMA_VERSION: i32 = 2;

/// Position of the last row of a page, used to resume the query after it.
///
/// Paging seeks by sort key rather than counting rows: `LIMIT n OFFSET x` makes
/// SQLite walk and discard `x` rows first, so the cost of a page grows with how
/// deep into the results it is. Comparing against the sort key instead lets the
/// index jump straight to the resume point, making a page cost depend only on
/// its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// `last_access` of the last row already returned.
    last_access: i64,
    /// `id` of the last row already returned; breaks ties.
    id: String,
}

impl Cursor {
    /// The cursor identifying a workspace as a resume point.
    pub fn for_workspace(ws: &Workspace) -> Cursor {
        Cursor {
            // Workspaces read back from the database always carry a value, since
            // the column is NOT NULL.
            last_access: ws.last_access.unwrap_or_default(),
            id: ws.id.clone(),
        }
    }

    /// The id of the last row already returned.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// One page of query results.
pub struct Page {
    /// The matching workspaces, in the requested order.
    pub workspaces: Vec<Workspace>,
    /// Cursor to pass back for the next page, or `None` at the end of results.
    pub next: Option<Cursor>,
}

impl Page {
    /// Whether more results exist after this page.
    pub fn has_more(&self) -> bool {
        self.next.is_some()
    }
}

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
        if version < 2 {
            self.migrate_to_v2()?;
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

    /// Add the change-detection signature columns and the sync state table.
    ///
    /// The signature columns let sync skip re-reading `wsmeta.txt` files that
    /// have not moved, which is where nearly all of its cost was.
    ///
    /// Both are nullable, where NULL means "this workspace has no `wsmeta.txt`"
    /// -- see [`crate::signature`] on why that must be the value's only meaning.
    /// Existing rows therefore come out of this migration claiming to have no
    /// metadata; for those that do have a `wsmeta.txt` that disagrees with the
    /// disk, so the next sync re-reads them and the index corrects itself. No
    /// separate data-fixing step is needed.
    ///
    /// The columns are deliberately left unindexed: they are only ever compared
    /// row by row during sync, never used to order or filter a query.
    ///
    /// `sync_state` holds which directory the index was last built from. Without
    /// it, pointing `workspaces_dir` at a different tree containing
    /// same-named workspaces would let their signatures match, so the stored
    /// (now wrong) paths would be served as current. `CHECK (id = 1)` plus the
    /// primary key make the table hold at most one row, so the invariant is
    /// enforced by the database rather than by every caller remembering it.
    fn migrate_to_v2(&self) -> AppResult<()> {
        self.conn
            .execute_batch(
                "BEGIN;
                 ALTER TABLE workspace ADD COLUMN meta_mtime INTEGER;
                 ALTER TABLE workspace ADD COLUMN meta_size  INTEGER;
                 CREATE TABLE sync_state (
                     id             INTEGER PRIMARY KEY CHECK (id = 1),
                     workspaces_dir TEXT NOT NULL
                 );
                 PRAGMA user_version = 2;
                 COMMIT;",
            )
            .map_err(|e| sql_err("cannot upgrade database schema to version 2", e))
    }

    /// Read the schema version recorded in the database header.
    fn schema_version(&self) -> AppResult<i32> {
        self.conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| sql_err("cannot read database schema version", e))
    }

    /// Read every workspace's stored metadata signature, keyed by id.
    ///
    /// One query for the whole table rather than a lookup per workspace: sync
    /// needs all of them, and the point of the signatures is to avoid per-file
    /// work, which per-row queries would reintroduce.
    pub fn stored_signatures(&self) -> AppResult<HashMap<String, Option<MetaSignature>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, meta_mtime, meta_size FROM workspace")
            .map_err(|e| sql_err("cannot prepare signature query", e))?;

        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let mtime: Option<i64> = row.get(1)?;
                let size: Option<i64> = row.get(2)?;
                Ok((id, signature_from_columns(mtime, size)))
            })
            .map_err(|e| sql_err("cannot read stored signatures", e))?;

        let mut stored = HashMap::new();
        for row in rows {
            let (id, signature) = row.map_err(|e| sql_err("cannot read a stored signature", e))?;
            stored.insert(id, signature);
        }
        Ok(stored)
    }

    /// The `workspaces_dir` the index was last synced against, if recorded.
    ///
    /// `None` on a database that has never been synced.
    pub fn synced_workspaces_dir(&self) -> AppResult<Option<String>> {
        self.conn
            .query_row("SELECT workspaces_dir FROM sync_state WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| sql_err("cannot read the recorded workspaces directory", e))
    }

    /// Record which `workspaces_dir` the index now reflects.
    ///
    /// Written through the single-row upsert the `CHECK (id = 1)` constraint
    /// makes possible, so there is no read-then-decide step that could race.
    pub fn set_synced_workspaces_dir(&self, dir: &str) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO sync_state (id, workspaces_dir) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET workspaces_dir = excluded.workspaces_dir",
                params![dir],
            )
            .map_err(|e| sql_err("cannot record the workspaces directory", e))?;
        Ok(())
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

    /// Fetch one page of workspaces matching `tokens`, ordered by `sort`.
    ///
    /// `limit` is how many rows the caller wants; one extra is fetched
    /// internally to learn whether more exist, so no separate COUNT is needed.
    /// Pass `after: None` for the first page, then the previous page's `next`.
    ///
    /// Matching is case-insensitive substring, against `slug` and `tags` only.
    /// `desc` is excluded because it can be arbitrarily long and would dominate
    /// the scan; searching it belongs to the future slow query.
    pub fn query(
        &self,
        tokens: &[Token],
        sort: SortOrder,
        limit: usize,
        after: Option<&Cursor>,
    ) -> AppResult<Page> {
        let mut conditions: Vec<String> = Vec::new();
        // Values are always bound, never interpolated: only the shape of the SQL
        // varies with the query, never its data.
        let mut binds: Vec<Box<dyn ToSql>> = Vec::new();

        for token in tokens {
            let (column_clause, needle) = match token {
                // A ':'-prefixed token deliberately narrows to tags only.
                Token::Tag(needle) => ("lower(tags) LIKE ? ESCAPE '\\'", needle),
                Token::Text(needle) => (
                    "(lower(slug) LIKE ? ESCAPE '\\' OR lower(tags) LIKE ? ESCAPE '\\')",
                    needle,
                ),
            };
            let pattern = like_pattern(needle);
            // The text form mentions the pattern twice, so bind it twice.
            let placeholders = column_clause.matches('?').count();
            for _ in 0..placeholders {
                binds.push(Box::new(pattern.clone()));
            }
            conditions.push(column_clause.to_string());
        }

        if let Some(cursor) = after {
            match sort {
                SortOrder::Recency => {
                    // Row-value comparison walks the (last_access DESC, id DESC)
                    // index directly from the resume point.
                    conditions.push("(last_access, id) < (?, ?)".to_string());
                    binds.push(Box::new(cursor.last_access));
                    binds.push(Box::new(cursor.id.clone()));
                }
                SortOrder::Id => {
                    conditions.push("id > ?".to_string());
                    binds.push(Box::new(cursor.id.clone()));
                }
            }
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT id, date, slug, tags, desc, path, last_access
             FROM workspace
             {}
             ORDER BY {}
             LIMIT ?",
            where_clause,
            order_by(sort),
        );
        // Over-fetch by one row purely as a "more exist" probe; it is never shown.
        binds.push(Box::new(limit as i64 + 1));

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| sql_err("cannot prepare workspace query", e))?;
        let bind_refs: Vec<&dyn ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let mut rows = stmt
            .query(bind_refs.as_slice())
            .map_err(|e| sql_err("cannot run workspace query", e))?;

        let mut workspaces = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| sql_err("cannot read workspace row", e))?
        {
            workspaces.push(read_workspace(row)?);
        }

        // Presence of the probe row means there is a further page; drop it and
        // hand back a cursor pointing at the last row we actually return.
        let next = if workspaces.len() > limit {
            workspaces.truncate(limit);
            workspaces.last().map(Cursor::for_workspace)
        } else {
            None
        };

        Ok(Page { workspaces, next })
    }

    /// Record an access to `id` at `timestamp`.
    ///
    /// Returns whether a row was updated: `false` means the id is not in the
    /// index, which callers may treat as "not a workspace" rather than a failure.
    /// This is the only operation that ever changes `last_access`.
    pub fn touch(&self, id: &str, timestamp: i64) -> AppResult<bool> {
        let updated = self
            .conn
            .execute(
                "UPDATE workspace SET last_access = ?1 WHERE id = ?2",
                params![timestamp, id],
            )
            .map_err(|e| sql_err(format!("cannot record access to '{}'", id), e))?;
        Ok(updated > 0)
    }
}

/// The ORDER BY clause for a sort order.
///
/// Both orders end in `id`, making them total: without a tiebreak, rows sharing
/// a `last_access` (every workspace created on the same day starts out that way)
/// could be returned in any order, and a keyset cursor could then skip or repeat
/// rows across pages.
fn order_by(sort: SortOrder) -> &'static str {
    match sort {
        SortOrder::Recency => "last_access DESC, id DESC",
        SortOrder::Id => "id ASC",
    }
}

/// Build a case-insensitive "contains" LIKE pattern for `needle`.
///
/// `%` and `_` are wildcards in LIKE, and `\` is our declared escape character,
/// so all three must be escaped for the needle to be matched literally;
/// otherwise typing `_` would silently match any character.
fn like_pattern(needle: &str) -> String {
    let mut pattern = String::with_capacity(needle.len() + 2);
    pattern.push('%');
    for ch in needle.to_lowercase().chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

/// Rebuild a `Workspace` from a result row.
fn read_workspace(row: &Row<'_>) -> AppResult<Workspace> {
    let id: String = row.get(0).map_err(|e| sql_err("cannot read id", e))?;
    let date_text: String = row.get(1).map_err(|e| sql_err("cannot read date", e))?;
    let date = parse_date(&date_text).ok_or_else(|| {
        AppError::runtime(format!(
            "workspace '{}' has an invalid stored date '{}'",
            id, date_text
        ))
    })?;
    let slug: String = row.get(2).map_err(|e| sql_err("cannot read slug", e))?;
    let tags: String = row.get(3).map_err(|e| sql_err("cannot read tags", e))?;
    let desc: String = row.get(4).map_err(|e| sql_err("cannot read desc", e))?;
    let path: String = row.get(5).map_err(|e| sql_err("cannot read path", e))?;
    let last_access: i64 = row
        .get(6)
        .map_err(|e| sql_err("cannot read last_access", e))?;

    Ok(Workspace {
        id,
        date,
        slug,
        path: PathBuf::from(path),
        tags: split_tags(&tags),
        desc,
        last_access: Some(last_access),
    })
}

/// Delete rows whose workspace is no longer on disk.///
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

/// Rebuild a signature from its two nullable columns.
///
/// Both columns are written together, so in practice they are either both set or
/// both NULL. A half-set pair could only come from external tampering; it is
/// read as "no metadata", which makes the next sync re-read the file and repair
/// the row rather than trusting half a signature.
fn signature_from_columns(mtime: Option<i64>, size: Option<i64>) -> Option<MetaSignature> {
    match (mtime, size) {
        (Some(mtime_ticks), Some(size)) => Some(MetaSignature {
            mtime_ticks,
            // Stored as INTEGER (i64); sizes are never negative, but a tampered
            // negative value must not wrap into a huge u64 and silently look
            // like a plausible size.
            size: u64::try_from(size).ok()?,
        }),
        _ => None,
    }
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

    /// A database at schema v1, holding one row, as an older build left it.
    ///
    /// Written out longhand rather than by calling `migrate_to_v1`, so the test
    /// keeps describing the real v1 shape even if that function is later
    /// changed or removed.
    fn v1_db_with_a_row() -> Db {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        conn.execute_batch(
            "CREATE TABLE workspace (
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
             INSERT INTO workspace
                 (id, date, slug, tags, desc, path, last_access)
             VALUES
                 ('20260730-alpha', '20260730', 'alpha', 'rust cli',
                  'a description', 'G:/ws/w/20260730-alpha', 1234567890);
             PRAGMA user_version = 1;",
        )
        .expect("build a v1 database");
        Db { conn }
    }

    #[test]
    fn a_v1_database_upgrades_to_the_current_version() {
        let db = v1_db_with_a_row();
        db.migrate().expect("migrate from v1");
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn upgrading_from_v1_preserves_access_history() {
        // The whole point of the database: losing this on an upgrade would
        // silently reset everyone's recency ordering.
        let db = v1_db_with_a_row();
        db.migrate().expect("migrate from v1");
        assert_eq!(stored_last_access(&db, "20260730-alpha"), Some(1234567890));
    }

    #[test]
    fn rows_from_v1_start_with_no_signature() {
        // They cannot have one, so the next sync must re-read their metadata
        // rather than trust the row. This is what makes the migration
        // self-healing with no separate data-fixing pass.
        let db = v1_db_with_a_row();
        db.migrate().expect("migrate from v1");
        let stored = db.stored_signatures().unwrap();
        assert_eq!(stored.get("20260730-alpha"), Some(&None));
    }

    #[test]
    fn sync_state_holds_at_most_one_row() {
        // The single-row invariant is enforced by the database rather than by
        // callers remembering it; a second row would make reads pick an
        // arbitrary one.
        let db = memory_db();
        db.set_synced_workspaces_dir("G:/ws/w").unwrap();
        db.set_synced_workspaces_dir("H:/other/w").unwrap();

        let count: i64 = db
            .conn
            .query_row("SELECT count(*) FROM sync_state", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            db.synced_workspaces_dir().unwrap(),
            Some("H:/other/w".to_string())
        );
    }

    #[test]
    fn sync_state_rejects_a_second_row() {
        let db = memory_db();
        db.set_synced_workspaces_dir("G:/ws/w").unwrap();
        let result = db.conn.execute(
            "INSERT INTO sync_state (id, workspaces_dir) VALUES (2, 'H:/other/w')",
            [],
        );
        assert!(result.is_err(), "CHECK (id = 1) did not reject id = 2");
    }

    #[test]
    fn a_database_never_synced_has_no_recorded_directory() {
        let db = memory_db();
        assert_eq!(db.synced_workspaces_dir().unwrap(), None);
    }

    #[test]
    fn a_half_written_signature_reads_as_absent() {
        // Only reachable by tampering, but it must degrade to "re-read this
        // file" rather than to a comparison against half a signature.
        let mut db = memory_db();
        db.sync(&[workspace("alpha", &[])], offset()).unwrap();
        db.conn
            .execute(
                "UPDATE workspace SET meta_mtime = 42, meta_size = NULL",
                [],
            )
            .unwrap();

        let stored = db.stored_signatures().unwrap();
        assert_eq!(stored.get("20260730-alpha"), Some(&None));
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

    /// Set a row's access time, standing in for a recorded visit.
    fn set_last_access(db: &Db, id: &str, ts: i64) {
        let n = db
            .conn
            .execute(
                "UPDATE workspace SET last_access = ?1 WHERE id = ?2",
                params![ts, id],
            )
            .unwrap();
        assert_eq!(n, 1, "expected to update exactly one row for '{}'", id);
    }

    /// Run a query and return the resulting ids in order.
    fn query_ids(db: &Db, query: &str, sort: SortOrder, limit: usize) -> Vec<String> {
        let tokens = crate::query::parse_str(query);
        let page = db.query(&tokens, sort, limit, None).unwrap();
        page.workspaces.into_iter().map(|ws| ws.id).collect()
    }

    #[test]
    fn empty_query_matches_everything() {
        let mut db = memory_db();
        db.sync(
            &[workspace("alpha", &[]), workspace("beta", &[])],
            offset(),
        )
        .unwrap();
        assert_eq!(query_ids(&db, "", SortOrder::Id, 10).len(), 2);
    }

    #[test]
    fn text_token_matches_slug_substring_anywhere() {
        let mut db = memory_db();
        db.sync(&[workspace("wsjump", &[])], offset()).unwrap();
        // Matching is "contains", not "starts with".
        assert_eq!(query_ids(&db, "jump", SortOrder::Id, 10).len(), 1);
        assert_eq!(query_ids(&db, "wsj", SortOrder::Id, 10).len(), 1);
        // But it is a substring, not a subsequence: that is the slow query's job.
        assert!(query_ids(&db, "wj", SortOrder::Id, 10).is_empty());
    }

    #[test]
    fn matching_ignores_case() {
        let mut db = memory_db();
        db.sync(&[workspace("WsJump", &["Tool"])], offset()).unwrap();
        assert_eq!(query_ids(&db, "wsjump", SortOrder::Id, 10).len(), 1);
        assert_eq!(query_ids(&db, ":tool", SortOrder::Id, 10).len(), 1);
    }

    #[test]
    fn text_token_matches_tags_too() {
        let mut db = memory_db();
        db.sync(&[workspace("alpha", &["rust"])], offset()).unwrap();
        assert_eq!(query_ids(&db, "rust", SortOrder::Id, 10).len(), 1);
    }

    #[test]
    fn text_token_does_not_match_desc() {
        let mut db = memory_db();
        let mut ws = workspace("alpha", &[]);
        ws.desc = "a note about databases".to_string();
        db.sync(&[ws], offset()).unwrap();
        // desc is excluded from the fast query by design.
        assert!(query_ids(&db, "databases", SortOrder::Id, 10).is_empty());
    }

    #[test]
    fn text_token_does_not_match_the_date_prefix() {
        let mut db = memory_db();
        db.sync(&[workspace("alpha", &[])], offset()).unwrap();
        // Only slug and tags are searched, so the user never has to type a date.
        assert!(query_ids(&db, "20260730", SortOrder::Id, 10).is_empty());
    }

    #[test]
    fn tag_token_searches_tags_only() {
        let mut db = memory_db();
        db.sync(
            &[workspace("rust", &[]), workspace("alpha", &["rust"])],
            offset(),
        )
        .unwrap();
        // ':rust' must ignore the workspace whose *slug* is "rust".
        assert_eq!(
            query_ids(&db, ":rust", SortOrder::Id, 10),
            vec!["20260730-alpha".to_string()]
        );
    }

    #[test]
    fn multiple_tokens_combine_with_and() {
        let mut db = memory_db();
        db.sync(
            &[
                workspace("alpha", &["rust", "tool"]),
                workspace("beta", &["rust"]),
            ],
            offset(),
        )
        .unwrap();
        assert_eq!(
            query_ids(&db, ":rust :tool", SortOrder::Id, 10),
            vec!["20260730-alpha".to_string()]
        );
    }

    #[test]
    fn wildcard_characters_are_matched_literally() {
        let mut db = memory_db();
        db.sync(
            &[workspace("plain", &[]), workspace("a_b", &[])],
            offset(),
        )
        .unwrap();
        // Unescaped, '_' is a LIKE wildcard and would also match "plain".
        assert_eq!(
            query_ids(&db, "a_b", SortOrder::Id, 10),
            vec!["20260730-a_b".to_string()]
        );
        // '%' likewise must not act as a wildcard.
        assert!(query_ids(&db, "%", SortOrder::Id, 10).is_empty());
    }

    #[test]
    fn quotes_in_a_query_are_data_not_sql() {
        let mut db = memory_db();
        db.sync(&[workspace("alpha", &[])], offset()).unwrap();
        // Values are bound, so this is a harmless (non-matching) needle rather
        // than syntax; the row must also survive.
        assert!(query_ids(&db, "'; DROP TABLE workspace; --", SortOrder::Id, 10).is_empty());
        assert_eq!(stored_ids(&db).len(), 1);
    }

    #[test]
    fn recency_order_puts_the_most_recent_first() {
        let mut db = memory_db();
        db.sync(
            &[
                workspace("alpha", &[]),
                workspace("beta", &[]),
                workspace("gamma", &[]),
            ],
            offset(),
        )
        .unwrap();
        set_last_access(&db, "20260730-alpha", 3_000);
        set_last_access(&db, "20260730-beta", 1_000);
        set_last_access(&db, "20260730-gamma", 2_000);

        assert_eq!(
            query_ids(&db, "", SortOrder::Recency, 10),
            vec![
                "20260730-alpha".to_string(),
                "20260730-gamma".to_string(),
                "20260730-beta".to_string(),
            ]
        );
    }

    #[test]
    fn id_order_is_lexicographic() {
        let mut db = memory_db();
        db.sync(
            &[workspace("gamma", &[]), workspace("alpha", &[])],
            offset(),
        )
        .unwrap();
        set_last_access(&db, "20260730-gamma", 9_000);

        // Explicit id order must ignore access times entirely.
        assert_eq!(
            query_ids(&db, "", SortOrder::Id, 10),
            vec!["20260730-alpha".to_string(), "20260730-gamma".to_string()]
        );
    }

    #[test]
    fn limit_is_respected_and_more_is_reported() {
        let mut db = memory_db();
        db.sync(
            &[
                workspace("alpha", &[]),
                workspace("beta", &[]),
                workspace("gamma", &[]),
            ],
            offset(),
        )
        .unwrap();

        let page = db.query(&[], SortOrder::Id, 2, None).unwrap();
        // The extra probe row must not leak into the results.
        assert_eq!(page.workspaces.len(), 2);
        assert!(page.has_more());
    }

    #[test]
    fn last_page_reports_no_more() {
        let mut db = memory_db();
        db.sync(
            &[workspace("alpha", &[]), workspace("beta", &[])],
            offset(),
        )
        .unwrap();

        let page = db.query(&[], SortOrder::Id, 2, None).unwrap();
        assert_eq!(page.workspaces.len(), 2);
        assert!(
            !page.has_more(),
            "exactly filling a page must not claim more results exist"
        );
    }

    /// Walk every page and collect ids, for both sort orders.
    fn paged_ids(db: &Db, sort: SortOrder, page_size: usize) -> Vec<String> {
        let mut ids = Vec::new();
        let mut cursor = None;
        loop {
            let page = db.query(&[], sort, page_size, cursor.as_ref()).unwrap();
            ids.extend(page.workspaces.iter().map(|ws| ws.id.clone()));
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        ids
    }

    #[test]
    fn paging_covers_every_row_exactly_once() {
        let mut db = memory_db();
        let all: Vec<Workspace> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| workspace(s, &[]))
            .collect();
        let ascending: Vec<String> = all.iter().map(|ws| ws.id.clone()).collect();
        db.sync(&all, offset()).unwrap();

        assert_eq!(paged_ids(&db, SortOrder::Id, 2), ascending);

        // These workspaces share a creation date and so a last_access, making
        // this the case where a missing tiebreak would skip or repeat rows
        // across pages: recency falls back to id DESC, hence reverse order.
        let descending: Vec<String> = ascending.iter().rev().cloned().collect();
        assert_eq!(paged_ids(&db, SortOrder::Recency, 2), descending);
    }

    #[test]
    fn paging_a_filtered_query_keeps_the_filter() {
        let mut db = memory_db();
        db.sync(
            &[
                workspace("alpha", &["keep"]),
                workspace("beta", &["keep"]),
                workspace("gamma", &[]),
            ],
            offset(),
        )
        .unwrap();

        let tokens = crate::query::parse_str(":keep");
        let first = db.query(&tokens, SortOrder::Id, 1, None).unwrap();
        assert_eq!(first.workspaces[0].id, "20260730-alpha");
        let second = db
            .query(&tokens, SortOrder::Id, 1, first.next.as_ref())
            .unwrap();
        assert_eq!(second.workspaces[0].id, "20260730-beta");
        assert!(!second.has_more());
    }

    #[test]
    fn query_round_trips_workspace_fields() {        let mut db = memory_db();
        let mut ws = workspace("wsjump", &["rust", "tool"]);
        ws.desc = "line one\nline two".to_string();
        let expected_path = ws.path.clone();
        let expected_date = ws.date;
        db.sync(&[ws], offset()).unwrap();

        let page = db.query(&[], SortOrder::Id, 10, None).unwrap();
        let got = &page.workspaces[0];
        assert_eq!(got.id, "20260730-wsjump");
        assert_eq!(got.date, expected_date);
        assert_eq!(got.slug, "wsjump");
        assert_eq!(got.tags, vec!["rust".to_string(), "tool".to_string()]);
        assert_eq!(got.desc, "line one\nline two");
        assert_eq!(got.path, expected_path);
        assert!(got.last_access.is_some());
    }

    #[test]
    fn touch_records_the_given_time() {
        let mut db = memory_db();
        db.sync(&[workspace("wsjump", &[])], offset()).unwrap();

        assert!(db.touch("20260730-wsjump", 1_800_000_000).unwrap());
        assert_eq!(
            stored_last_access(&db, "20260730-wsjump"),
            Some(1_800_000_000)
        );
    }

    #[test]
    fn touch_reports_an_unknown_id() {
        let db = memory_db();
        assert!(
            !db.touch("20260730-nope", 1_800_000_000).unwrap(),
            "touching an id that is not indexed must be reported, not silently succeed"
        );
    }

    #[test]
    fn touch_reorders_a_recency_query() {
        let mut db = memory_db();
        db.sync(
            &[workspace("alpha", &[]), workspace("beta", &[])],
            offset(),
        )
        .unwrap();

        // The whole point of recording an access: the visited workspace becomes
        // the first thing offered next time.
        db.touch("20260730-alpha", 1_800_000_000).unwrap();
        assert_eq!(
            query_ids(&db, "", SortOrder::Recency, 10).first().unwrap(),
            "20260730-alpha"
        );

        db.touch("20260730-beta", 1_800_000_001).unwrap();
        assert_eq!(
            query_ids(&db, "", SortOrder::Recency, 10).first().unwrap(),
            "20260730-beta"
        );
    }

    #[test]
    fn touch_survives_a_later_sync() {
        let mut db = memory_db();
        db.sync(&[workspace("alpha", &[])], offset()).unwrap();
        db.touch("20260730-alpha", 1_800_000_000).unwrap();

        db.sync(&[workspace("alpha", &[])], offset()).unwrap();
        assert_eq!(
            stored_last_access(&db, "20260730-alpha"),
            Some(1_800_000_000),
            "a recorded access must outlive the syncs that follow it"
        );
    }
}
