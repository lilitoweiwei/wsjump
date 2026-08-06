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
use crate::scan::{Discovered, Discovery};
use crate::signature::{self, MetaSignature};
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

/// Which workspaces a sync is allowed to touch.
///
/// Scope is separate from whether signatures are compared, because the two vary
/// independently: an automatic sync is whole-tree and incremental, an explicit
/// `wsjump sync` is whole-tree and forced, and `wsjump sync <path>` is one
/// workspace and forced. Keeping them as parameters of one routine rather than
/// as separate routines is what lets the incremental path be checked against the
/// forced one for equivalence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncScope {
    /// Reconcile the whole directory: rows absent from disk are deleted.
    Everything,
    /// Reconcile a single workspace, leaving every other row untouched.
    ///
    /// Deletion is still possible, but only of this id: the "in the database
    /// but not on disk" rule must not be applied to workspaces this sync never
    /// looked at, or a scoped sync would empty the index.
    OnlyWorkspace(String),
}

impl SyncScope {
    /// Whether a workspace with this id is within scope.
    fn includes(&self, id: &str) -> bool {
        match self {
            SyncScope::Everything => true,
            SyncScope::OnlyWorkspace(only) => only == id,
        }
    }
}

/// What a sync did, so callers can report it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Workspaces considered, whether or not they needed work.
    pub examined: usize,
    /// Workspaces whose metadata was re-read and stored.
    ///
    /// In a steady state this is zero, which is the entire point of the
    /// signatures; a test asserts on it directly, since an equivalent result
    /// cannot distinguish "correctly skipped" from "re-read everything".
    pub updated: usize,
    /// Rows removed because their workspace is gone from disk.
    pub deleted: usize,
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
            .query_row(
                "SELECT workspaces_dir FROM sync_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| sql_err("cannot read the recorded workspaces directory", e))
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
    /// Metadata is re-read only where the stored [`crate::signature`] disagrees
    /// with the file on disk, which is what keeps a steady-state sync from
    /// touching any `wsmeta.txt` at all. Pass `force` to re-read regardless,
    /// which is both the explicit `wsjump sync` and the reference behaviour the
    /// incremental path is tested against.
    ///
    /// Returns what it did, for the caller to report.
    pub fn sync(
        &mut self,
        found: &Discovery,
        offset: UtcOffset,
        scope: &SyncScope,
        force: bool,
    ) -> AppResult<SyncOutcome> {
        let stored = self.stored_signatures()?;
        let mut outcome = SyncOutcome::default();

        // Signatures describe files under a particular root. If that root has
        // moved, they describe files we are no longer looking at, and a matching
        // signature would wrongly keep a row whose stored path is now wrong.
        let root = found.root.to_string_lossy().into_owned();
        let force = force || self.synced_workspaces_dir()?.as_deref() != Some(root.as_str());

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
                         (id, date, slug, tags, desc, path, last_access,
                          meta_mtime, meta_size)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(id) DO UPDATE SET
                         date = excluded.date,
                         slug = excluded.slug,
                         tags = excluded.tags,
                         desc = excluded.desc,
                         path = excluded.path,
                         meta_mtime = excluded.meta_mtime,
                         meta_size = excluded.meta_size",
                )
                .map_err(|e| sql_err("cannot prepare workspace upsert", e))?;

            for entry in &found.workspaces {
                if !scope.includes(&entry.id) {
                    continue;
                }
                outcome.examined += 1;

                // A workspace absent from the database has no stored signature
                // to compare, so it is always read.
                let known = stored.contains_key(&entry.id);
                if known && !force && !signature::is_stale(stored[&entry.id], entry.signature) {
                    continue;
                }

                // The one expensive step, reached only for workspaces that
                // actually changed.
                let ws = entry.load();
                outcome.updated += 1;

                let (meta_mtime, meta_size) = signature_to_columns(entry.signature);
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
                        meta_mtime,
                        meta_size,
                    ])
                    .map_err(|e| sql_err(format!("cannot store workspace '{}'", ws.id), e))?;
            }

            outcome.deleted = delete_missing(&tx, &found.workspaces, scope)?;

            // Recorded inside the transaction: if the sync rolls back, so must
            // the claim about which directory the index reflects.
            //
            // Skipped for a scoped sync, which only looked at one workspace and
            // so cannot vouch for the tree as a whole.
            if matches!(scope, SyncScope::Everything) {
                tx.execute(
                    "INSERT INTO sync_state (id, workspaces_dir) VALUES (1, ?1)
                     ON CONFLICT(id) DO UPDATE SET workspaces_dir = excluded.workspaces_dir",
                    params![root],
                )
                .map_err(|e| sql_err("cannot record the workspaces directory", e))?;
            }
        }

        tx.commit()
            .map_err(|e| sql_err("cannot commit sync transaction", e))?;
        Ok(outcome)
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

/// Delete rows whose workspace is no longer on disk, returning how many went.
///
/// The ids present on disk go into a temporary table rather than a giant
/// `NOT IN (?, ?, ...)` list, which would grow the SQL with the number of
/// workspaces and can exceed SQLite's parameter limit.
///
/// A scoped sync only ever deletes its own id. Everything outside the scope was
/// never looked at, so "not on disk" says nothing about it -- applying the rule
/// regardless would empty the index down to the one workspace that was synced.
fn delete_missing(
    tx: &Transaction<'_>,
    found: &[Discovered],
    scope: &SyncScope,
) -> AppResult<usize> {
    // Dropped before returning so repeated syncs on one connection cannot
    // collide on the name.
    tx.execute_batch("CREATE TEMP TABLE present (id TEXT PRIMARY KEY)")
        .map_err(|e| sql_err("cannot create temporary id table", e))?;

    {
        let mut insert = tx
            .prepare("INSERT OR IGNORE INTO present (id) VALUES (?1)")
            .map_err(|e| sql_err("cannot prepare temporary id insert", e))?;
        for entry in found {
            insert
                .execute(params![entry.id])
                .map_err(|e| sql_err("cannot record present workspace id", e))?;
        }
    }

    let deleted = match scope {
        SyncScope::Everything => tx
            .execute(
                "DELETE FROM workspace
                 WHERE id NOT IN (SELECT id FROM present)",
                [],
            )
            .map_err(|e| sql_err("cannot remove deleted workspaces", e))?,
        SyncScope::OnlyWorkspace(id) => tx
            .execute(
                "DELETE FROM workspace
                 WHERE id = ?1 AND id NOT IN (SELECT id FROM present)",
                params![id],
            )
            .map_err(|e| sql_err("cannot remove a deleted workspace", e))?,
    };

    tx.execute_batch("DROP TABLE present")
        .map_err(|e| sql_err("cannot drop temporary id table", e))?;
    Ok(deleted)
}

/// Wrap a rusqlite error as a runtime `AppError` with our own context.
fn sql_err(context: impl Into<String>, e: rusqlite::Error) -> AppError {
    AppError::runtime(format!("{}: {}", context.into(), e))
}

/// The two column values to store for a signature.
///
/// A size too large for `i64` (absurd for a metadata file, but not
/// unrepresentable) stores as no signature at all rather than as half a pair, so
/// the file is simply re-read next time instead of being compared against a
/// value we could not record faithfully.
fn signature_to_columns(signature: Option<MetaSignature>) -> (Option<i64>, Option<i64>) {
    match signature {
        Some(sig) => match i64::try_from(sig.size) {
            Ok(size) => (Some(sig.mtime_ticks), Some(size)),
            Err(_) => (None, None),
        },
        None => (None, None),
    }
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
    use crate::config::Config;
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

    /// A temporary `w/` directory that cleans itself up.
    ///
    /// Sync's whole job is reconciling against a real directory, and its
    /// skip-when-unchanged behaviour is driven by real file timestamps. Faking
    /// that would mean testing something other than what runs, so these tests
    /// build actual directories.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new(tag: &str) -> TempTree {
            let name = format!(
                "wsjump-db-test-{}-{}-{:?}",
                tag,
                std::process::id(),
                std::thread::current().id()
            );
            let root = std::env::temp_dir().join(name);
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create temp tree");
            TempTree { root }
        }

        /// Create a workspace directory, with a `wsmeta.txt` if tags are given.
        fn add(&self, slug: &str, tags: &[&str]) -> PathBuf {
            let dir = self.root.join(format!("20260730-{}", slug));
            std::fs::create_dir_all(&dir).expect("create workspace dir");
            if !tags.is_empty() {
                let line = tags
                    .iter()
                    .map(|t| format!(":{}", t))
                    .collect::<Vec<_>>()
                    .join(" ");
                std::fs::write(dir.join("wsmeta.txt"), line).expect("write wsmeta");
            }
            dir
        }

        /// Remove a workspace directory, as deleting it by hand would.
        fn remove(&self, slug: &str) {
            let dir = self.root.join(format!("20260730-{}", slug));
            std::fs::remove_dir_all(dir).expect("remove workspace dir");
        }

        /// Overwrite a workspace's `wsmeta.txt` in place.
        fn write_meta(&self, slug: &str, body: &str) {
            let dir = self.root.join(format!("20260730-{}", slug));
            std::fs::write(dir.join("wsmeta.txt"), body).expect("write wsmeta");
        }

        /// Delete a workspace's `wsmeta.txt`.
        fn remove_meta(&self, slug: &str) {
            let dir = self.root.join(format!("20260730-{}", slug));
            std::fs::remove_file(dir.join("wsmeta.txt")).expect("remove wsmeta");
        }

        /// Discover the tree as sync would see it.
        fn discover(&self) -> Discovery {
            let config = Config::for_test(&self.root);
            crate::scan::discover(&config).expect("discover")
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Db {
        /// Sync the whole tree at the tests' fixed offset, comparing signatures.
        ///
        /// Scope, offset and force are the same in nearly every test, and
        /// spelling them out at each call buries the part that differs.
        fn sync_all(&mut self, workspaces: &[Workspace]) -> AppResult<SyncOutcome> {
            self.sync(
                &discovered(workspaces),
                offset(),
                &SyncScope::Everything,
                false,
            )
        }

        /// Sync a discovered tree, comparing signatures.
        fn sync_found(&mut self, found: &Discovery) -> AppResult<SyncOutcome> {
            self.sync(found, offset(), &SyncScope::Everything, false)
        }

        /// Sync a discovered tree, re-reading every workspace regardless.
        fn sync_forced(&mut self, found: &Discovery) -> AppResult<SyncOutcome> {
            self.sync(found, offset(), &SyncScope::Everything, true)
        }

        /// Insert rows directly, bypassing discovery.
        ///
        /// For tests about querying rather than syncing: they need rows with
        /// particular tags and descriptions, and going through sync would mean
        /// materialising a directory tree just to get metadata onto disk so it
        /// could be read straight back.
        fn seed_all(&self, workspaces: &[Workspace]) -> AppResult<()> {
            for ws in workspaces {
                self.conn
                    .execute(
                        "INSERT INTO workspace
                             (id, date, slug, tags, desc, path, last_access)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            ws.id,
                            format_date(ws.date),
                            ws.slug,
                            ws.tags_joined(),
                            ws.desc,
                            ws.path.to_string_lossy(),
                            ws.date_midnight(offset()),
                        ],
                    )
                    .map_err(|e| sql_err("seed workspace", e))?;
            }
            Ok(())
        }
    }

    /// Treat hand-built workspaces as if discovery had found them, with no
    /// metadata file.
    ///
    /// Used by the tests concerned with the reconcile rules rather than with
    /// change detection: what matters there is which rows appear, change or go.
    /// Tags therefore have to come from a real file, so tests that assert on tags
    /// use [`TempTree`].
    fn discovered(workspaces: &[Workspace]) -> Discovery {
        Discovery {
            // These workspaces are invented rather than found, so name the root
            // after where their paths claim to be; sync compares it against
            // sync_state, and a stable value keeps repeat syncs incremental.
            root: PathBuf::from("G:/ws/w"),
            workspaces: workspaces
                .iter()
                .map(|ws| Discovered {
                    id: ws.id.clone(),
                    date: ws.date,
                    slug: ws.slug.clone(),
                    path: ws.path.clone(),
                    signature: None,
                })
                .collect(),
        }
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

    /// The stored `tags` string of a row, or None if the row is absent.
    fn stored_tags(db: &Db, id: &str) -> Option<String> {
        db.conn
            .query_row(
                "SELECT tags FROM workspace WHERE id = ?1",
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

    /// Every row's meaningful content, for comparing two databases.
    ///
    /// Paths and signatures are excluded: two temporary trees live at different
    /// locations and their files have different timestamps, so those differ for
    /// reasons that have nothing to do with whether sync reconciled correctly.
    fn rows_for_comparison(db: &Db) -> Vec<(String, String, String, String, i64)> {
        let mut stmt = db
            .conn
            .prepare(
                "SELECT id, date, slug, tags, desc, last_access
                 FROM workspace ORDER BY id",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                let tags: String = row.get(3)?;
                let desc: String = row.get(4)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    format!("{}|{}", tags, desc),
                    row.get::<_, i64>(5)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows
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
        let tree_a = TempTree::new("state-a");
        let tree_b = TempTree::new("state-b");
        tree_a.add("alpha", &[]);
        tree_b.add("alpha", &[]);
        let mut db = memory_db();

        db.sync_found(&tree_a.discover()).unwrap();
        db.sync_found(&tree_b.discover()).unwrap();

        let count: i64 = db
            .conn
            .query_row("SELECT count(*) FROM sync_state", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            db.synced_workspaces_dir().unwrap(),
            Some(tree_b.root.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn sync_state_rejects_a_second_row() {
        let tree = TempTree::new("state-check");
        tree.add("alpha", &[]);
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

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
    fn repointing_the_workspaces_directory_rereads_everything() {
        // The trap: two trees holding a same-named workspace whose metadata files
        // are byte-identical AND share an mtime, so the signatures match exactly.
        // Without the recorded root, sync sees "unchanged" and keeps the row --
        // including its path, which now points into the old tree and would send
        // the user to the wrong place.
        let old_tree = TempTree::new("repoint-old");
        let new_tree = TempTree::new("repoint-new");
        let old_dir = old_tree.add("alpha", &[]);
        let new_dir = new_tree.add("alpha", &[]);
        old_tree.write_meta("alpha", ":same");
        new_tree.write_meta("alpha", ":same");

        // Force the mtimes to agree, so only the root differs.
        let shared = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000);
        for dir in [&old_dir, &new_dir] {
            let file = std::fs::File::options()
                .write(true)
                .open(dir.join("wsmeta.txt"))
                .expect("open wsmeta");
            file.set_modified(shared).expect("set mtime");
        }

        let mut db = memory_db();
        db.sync_found(&old_tree.discover()).unwrap();

        // Precondition: the signatures really are identical, so the only thing
        // that can trigger a re-read is the changed root.
        let old_sig = crate::signature::read(&old_dir);
        let new_sig = crate::signature::read(&new_dir);
        assert_eq!(
            old_sig, new_sig,
            "test is pointless unless the signatures match"
        );

        let outcome = db.sync_found(&new_tree.discover()).unwrap();

        assert_eq!(
            outcome.updated, 1,
            "a moved root must invalidate every signature"
        );
        let path: String = db
            .conn
            .query_row(
                "SELECT path FROM workspace WHERE id = ?1",
                params!["20260730-alpha"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            path.starts_with(&new_tree.root.to_string_lossy().into_owned()),
            "stored path must point into the new tree, got {}",
            path
        );
    }

    #[test]
    fn a_scoped_sync_does_not_claim_the_whole_directory() {
        // It only looked at one workspace, so it cannot vouch for the tree. If it
        // recorded the root anyway, a later whole-tree sync would believe the
        // index already matched and skip everything else.
        let tree = TempTree::new("scoped-state");
        tree.add("alpha", &[]);
        tree.add("beta", &[]);
        let mut db = memory_db();

        db.sync(
            &tree.discover(),
            offset(),
            &SyncScope::OnlyWorkspace("20260730-alpha".to_string()),
            false,
        )
        .unwrap();

        assert_eq!(db.synced_workspaces_dir().unwrap(), None);
    }

    #[test]
    fn a_half_written_signature_reads_as_absent() {
        // Only reachable by tampering, but it must degrade to "re-read this
        // file" rather than to a comparison against half a signature.
        let mut db = memory_db();
        db.sync_all(&[workspace("alpha", &[])]).unwrap();
        db.conn
            .execute("UPDATE workspace SET meta_mtime = 42, meta_size = NULL", [])
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
        db.sync_all(&[ws]).unwrap();

        assert_eq!(
            stored_last_access(&db, "20260730-wsjump"),
            Some(expected),
            "a new row's last_access must start at its creation date's midnight"
        );
    }

    #[test]
    fn sync_never_overwrites_last_access() {
        let tree = TempTree::new("last-access");
        tree.add("wsjump", &[]);
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

        // Simulate a recorded access far after the creation date.
        let accessed = 1_800_000_000;
        db.conn
            .execute(
                "UPDATE workspace SET last_access = ?1 WHERE id = ?2",
                params![accessed, "20260730-wsjump"],
            )
            .unwrap();

        // Re-syncing must refresh mirrored columns but leave history alone.
        tree.write_meta("wsjump", ":tool\nnow with a description");
        db.sync_found(&tree.discover()).unwrap();

        assert_eq!(
            stored_last_access(&db, "20260730-wsjump"),
            Some(accessed),
            "sync must not reset last_access, or recency ordering silently breaks"
        );
        assert_eq!(
            stored_tags(&db, "20260730-wsjump"),
            Some("tool".to_string()),
            "mirrored columns must still be refreshed"
        );
    }

    #[test]
    fn sync_removes_workspaces_gone_from_disk() {
        let mut db = memory_db();
        db.sync_all(&[workspace("alpha", &[]), workspace("beta", &[])])
            .unwrap();
        assert_eq!(stored_ids(&db).len(), 2);

        db.sync_all(&[workspace("beta", &[])]).unwrap();
        assert_eq!(stored_ids(&db), vec!["20260730-beta".to_string()]);
    }

    #[test]
    fn a_steady_state_sync_reads_no_metadata() {
        // The entire point of the signatures. An equivalence check cannot catch a
        // regression here, because re-reading everything produces the same rows;
        // only the count of re-reads distinguishes the two.
        let tree = TempTree::new("steady");
        tree.add("alpha", &["rust"]);
        tree.add("beta", &[]);
        let mut db = memory_db();

        let first = db.sync_found(&tree.discover()).unwrap();
        assert_eq!(first.updated, 2, "a first sync must read everything");

        let second = db.sync_found(&tree.discover()).unwrap();
        assert_eq!(
            second.updated, 0,
            "nothing changed on disk, so nothing may be re-read"
        );
        assert_eq!(second.examined, 2, "both are still checked");
    }

    #[test]
    fn only_the_changed_workspace_is_reread() {
        let tree = TempTree::new("one-change");
        tree.add("alpha", &["rust"]);
        tree.add("beta", &["tool"]);
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

        tree.write_meta("alpha", ":changed");
        let outcome = db.sync_found(&tree.discover()).unwrap();

        assert_eq!(outcome.updated, 1, "only alpha moved");
        assert_eq!(stored_tags(&db, "20260730-alpha"), Some("changed".to_string()));
        assert_eq!(stored_tags(&db, "20260730-beta"), Some("tool".to_string()));
    }

    #[test]
    fn an_in_place_edit_is_detected() {
        // The case a directory mtime cannot see, which is why the signature is
        // taken from the metadata file itself.
        let tree = TempTree::new("in-place");
        tree.add("alpha", &["before"]);
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

        tree.write_meta("alpha", ":after");
        db.sync_found(&tree.discover()).unwrap();

        assert_eq!(stored_tags(&db, "20260730-alpha"), Some("after".to_string()));
    }

    #[test]
    fn deleting_the_metadata_file_clears_what_it_contributed() {
        // Otherwise the old tags stay in the index for good and the user can
        // never get rid of them.
        let tree = TempTree::new("meta-gone");
        tree.add("alpha", &["stale"]);
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();
        assert_eq!(stored_tags(&db, "20260730-alpha"), Some("stale".to_string()));

        tree.remove_meta("alpha");
        db.sync_found(&tree.discover()).unwrap();

        assert_eq!(stored_tags(&db, "20260730-alpha"), Some(String::new()));
    }

    #[test]
    fn adding_a_metadata_file_is_detected() {
        let tree = TempTree::new("meta-added");
        tree.add("alpha", &[]);
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

        tree.write_meta("alpha", ":fresh");
        db.sync_found(&tree.discover()).unwrap();

        assert_eq!(stored_tags(&db, "20260730-alpha"), Some("fresh".to_string()));
    }

    #[test]
    fn an_edit_keeping_the_same_size_is_detected() {
        let tree = TempTree::new("same-size");
        tree.add("alpha", &[]);
        tree.write_meta("alpha", ":aaa :bbb");
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

        // Same byte count, different content.
        tree.write_meta("alpha", ":bbb :aaa");
        db.sync_found(&tree.discover()).unwrap();

        assert_eq!(
            stored_tags(&db, "20260730-alpha"),
            Some("bbb aaa".to_string())
        );
    }

    #[test]
    fn an_mtime_moving_backwards_is_detected() {
        // Restoring an older copy, or checking out an older revision, changes the
        // file while moving its mtime earlier. Comparing with '>' would miss it.
        let tree = TempTree::new("mtime-back");
        let dir = tree.add("alpha", &[]);
        tree.write_meta("alpha", ":current");
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

        tree.write_meta("alpha", ":restored");
        let file = std::fs::File::options()
            .write(true)
            .open(dir.join("wsmeta.txt"))
            .expect("open wsmeta");
        file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .expect("set an older mtime");
        drop(file);

        db.sync_found(&tree.discover()).unwrap();

        assert_eq!(
            stored_tags(&db, "20260730-alpha"),
            Some("restored".to_string())
        );
    }

    #[test]
    fn forcing_a_sync_rereads_everything() {
        let tree = TempTree::new("forced");
        tree.add("alpha", &["rust"]);
        tree.add("beta", &[]);
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

        let outcome = db.sync_forced(&tree.discover()).unwrap();

        assert_eq!(
            outcome.updated, 2,
            "a forced sync must not trust the signatures"
        );
    }

    #[test]
    fn forcing_a_sync_repairs_an_index_whose_signature_still_matches() {
        // The reason the forced path exists: if the stored row is wrong while its
        // signature agrees with disk, only a forced re-read can fix it.
        let tree = TempTree::new("repair");
        tree.add("alpha", &["real"]);
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

        // Corrupt the row without touching the file.
        db.conn
            .execute("UPDATE workspace SET tags = 'wrong'", [])
            .unwrap();

        db.sync_found(&tree.discover()).unwrap();
        assert_eq!(
            stored_tags(&db, "20260730-alpha"),
            Some("wrong".to_string()),
            "an incremental sync cannot see this, by design"
        );

        db.sync_forced(&tree.discover()).unwrap();
        assert_eq!(stored_tags(&db, "20260730-alpha"), Some("real".to_string()));
    }

    /// A named change to apply to a test tree.
    type TreeChange = (&'static str, fn(&TempTree));

    #[test]
    fn an_incremental_sync_matches_a_forced_one() {
        // The strongest guarantee available: whatever the sequence of changes, the
        // cheap path must land on the same rows as the exhaustive one.
        let changes: Vec<TreeChange> = vec![
            ("a new workspace", |t| {
                t.add("added", &["fresh"]);
            }),
            ("a deleted workspace", |t| t.remove("doomed")),
            ("an edited metadata file", |t| {
                t.write_meta("edited", ":changed")
            }),
            ("a deleted metadata file", |t| t.remove_meta("stripped")),
            ("a new metadata file", |t| t.write_meta("bare", ":gained")),
            ("a same-size edit", |t| t.write_meta("sized", ":yyy")),
        ];

        for (description, apply) in changes {
            let incremental_tree = TempTree::new("equiv-inc");
            let forced_tree = TempTree::new("equiv-forced");
            for tree in [&incremental_tree, &forced_tree] {
                tree.add("edited", &["before"]);
                tree.add("stripped", &["doomed-tag"]);
                tree.add("bare", &[]);
                tree.add("doomed", &[]);
                tree.add("sized", &[]);
                tree.write_meta("sized", ":xxx");
            }

            let mut incremental = memory_db();
            let mut forced = memory_db();
            incremental.sync_found(&incremental_tree.discover()).unwrap();
            forced.sync_found(&forced_tree.discover()).unwrap();

            apply(&incremental_tree);
            apply(&forced_tree);

            incremental.sync_found(&incremental_tree.discover()).unwrap();
            forced.sync_forced(&forced_tree.discover()).unwrap();

            assert_eq!(
                rows_for_comparison(&incremental),
                rows_for_comparison(&forced),
                "incremental and forced sync disagreed after {}",
                description
            );
        }
    }

    /// Sync only the named workspace, at the tests' fixed offset.
    fn sync_only(db: &mut Db, id: &str, workspaces: &[Workspace]) {
        db.sync(
            &discovered(workspaces),
            offset(),
            &SyncScope::OnlyWorkspace(id.to_string()),
            false,
        )
        .unwrap();
    }

    #[test]
    fn a_scoped_sync_leaves_other_rows_alone() {
        // The trap: deletion asks "which rows are not on disk?", and a scoped
        // sync only looked at one workspace. Applying that rule unscoped would
        // wipe every workspace the sync never examined.
        let mut db = memory_db();
        db.sync_all(&[workspace("alpha", &[]), workspace("beta", &[])])
            .unwrap();

        sync_only(&mut db, "20260730-alpha", &[workspace("alpha", &[])]);

        assert_eq!(
            stored_ids(&db),
            vec!["20260730-alpha".to_string(), "20260730-beta".to_string()]
        );
    }

    #[test]
    fn a_scoped_sync_updates_only_its_own_workspace() {
        let tree = TempTree::new("scoped-update");
        tree.add("alpha", &["old"]);
        tree.add("beta", &["old"]);
        let mut db = memory_db();
        db.sync_found(&tree.discover()).unwrap();

        // Both carry new tags on disk, but only alpha is in scope.
        tree.write_meta("alpha", ":new");
        tree.write_meta("beta", ":new");
        db.sync(
            &tree.discover(),
            offset(),
            &SyncScope::OnlyWorkspace("20260730-alpha".to_string()),
            false,
        )
        .unwrap();

        assert_eq!(stored_tags(&db, "20260730-alpha"), Some("new".to_string()));
        assert_eq!(stored_tags(&db, "20260730-beta"), Some("old".to_string()));
    }

    #[test]
    fn a_scoped_sync_deletes_its_workspace_when_it_is_gone() {
        // Removing a workspace and syncing just that path is a legitimate way to
        // let the index catch up, so the scope must still permit its own delete.
        let mut db = memory_db();
        db.sync_all(&[workspace("alpha", &[]), workspace("beta", &[])])
            .unwrap();

        sync_only(&mut db, "20260730-alpha", &[workspace("beta", &[])]);

        assert_eq!(stored_ids(&db), vec!["20260730-beta".to_string()]);
    }

    #[test]
    fn a_scoped_sync_can_add_a_new_workspace() {
        let mut db = memory_db();
        db.sync_all(&[workspace("alpha", &[])]).unwrap();

        sync_only(
            &mut db,
            "20260730-beta",
            &[workspace("alpha", &[]), workspace("beta", &[])],
        );

        assert_eq!(
            stored_ids(&db),
            vec!["20260730-alpha".to_string(), "20260730-beta".to_string()]
        );
    }

    #[test]
    fn sync_of_empty_disk_empties_the_index() {
        let mut db = memory_db();
        db.sync_all(&[workspace("alpha", &[])]).unwrap();
        db.sync_all(&[]).unwrap();
        assert!(stored_ids(&db).is_empty());
    }

    #[test]
    fn sync_can_run_twice_in_one_connection() {
        // The temp table used for deletion must not linger and collide.
        let mut db = memory_db();
        db.sync_all(&[workspace("alpha", &[])]).unwrap();
        db.sync_all(&[workspace("alpha", &[])]).unwrap();
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
        db.sync_all(&[workspace("alpha", &[]), workspace("beta", &[])])
            .unwrap();
        assert_eq!(query_ids(&db, "", SortOrder::Id, 10).len(), 2);
    }

    #[test]
    fn text_token_matches_slug_substring_anywhere() {
        let mut db = memory_db();
        db.sync_all(&[workspace("wsjump", &[])]).unwrap();
        // Matching is "contains", not "starts with".
        assert_eq!(query_ids(&db, "jump", SortOrder::Id, 10).len(), 1);
        assert_eq!(query_ids(&db, "wsj", SortOrder::Id, 10).len(), 1);
        // But it is a substring, not a subsequence: that is the slow query's job.
        assert!(query_ids(&db, "wj", SortOrder::Id, 10).is_empty());
    }

    #[test]
    fn matching_ignores_case() {
        let db = memory_db();
        db.seed_all(&[workspace("WsJump", &["Tool"])]).unwrap();
        assert_eq!(query_ids(&db, "wsjump", SortOrder::Id, 10).len(), 1);
        assert_eq!(query_ids(&db, ":tool", SortOrder::Id, 10).len(), 1);
    }

    #[test]
    fn text_token_matches_tags_too() {
        let db = memory_db();
        db.seed_all(&[workspace("alpha", &["rust"])]).unwrap();
        assert_eq!(query_ids(&db, "rust", SortOrder::Id, 10).len(), 1);
    }

    #[test]
    fn text_token_does_not_match_desc() {
        let mut db = memory_db();
        let mut ws = workspace("alpha", &[]);
        ws.desc = "a note about databases".to_string();
        db.sync_all(&[ws]).unwrap();
        // desc is excluded from the fast query by design.
        assert!(query_ids(&db, "databases", SortOrder::Id, 10).is_empty());
    }

    #[test]
    fn text_token_does_not_match_the_date_prefix() {
        let mut db = memory_db();
        db.sync_all(&[workspace("alpha", &[])]).unwrap();
        // Only slug and tags are searched, so the user never has to type a date.
        assert!(query_ids(&db, "20260730", SortOrder::Id, 10).is_empty());
    }

    #[test]
    fn tag_token_searches_tags_only() {
        let db = memory_db();
        db.seed_all(&[workspace("rust", &[]), workspace("alpha", &["rust"])])
            .unwrap();
        // ':rust' must ignore the workspace whose *slug* is "rust".
        assert_eq!(
            query_ids(&db, ":rust", SortOrder::Id, 10),
            vec!["20260730-alpha".to_string()]
        );
    }

    #[test]
    fn multiple_tokens_combine_with_and() {
        let db = memory_db();
        db.seed_all(&[
            workspace("alpha", &["rust", "tool"]),
            workspace("beta", &["rust"]),
        ])
        .unwrap();
        assert_eq!(
            query_ids(&db, ":rust :tool", SortOrder::Id, 10),
            vec!["20260730-alpha".to_string()]
        );
    }

    #[test]
    fn wildcard_characters_are_matched_literally() {
        let mut db = memory_db();
        db.sync_all(&[workspace("plain", &[]), workspace("a_b", &[])])
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
        db.sync_all(&[workspace("alpha", &[])]).unwrap();
        // Values are bound, so this is a harmless (non-matching) needle rather
        // than syntax; the row must also survive.
        assert!(query_ids(&db, "'; DROP TABLE workspace; --", SortOrder::Id, 10).is_empty());
        assert_eq!(stored_ids(&db).len(), 1);
    }

    #[test]
    fn recency_order_puts_the_most_recent_first() {
        let mut db = memory_db();
        db.sync_all(&[
            workspace("alpha", &[]),
            workspace("beta", &[]),
            workspace("gamma", &[]),
        ])
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
        db.sync_all(&[workspace("gamma", &[]), workspace("alpha", &[])])
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
        db.sync_all(&[
            workspace("alpha", &[]),
            workspace("beta", &[]),
            workspace("gamma", &[]),
        ])
        .unwrap();

        let page = db.query(&[], SortOrder::Id, 2, None).unwrap();
        // The extra probe row must not leak into the results.
        assert_eq!(page.workspaces.len(), 2);
        assert!(page.has_more());
    }

    #[test]
    fn last_page_reports_no_more() {
        let mut db = memory_db();
        db.sync_all(&[workspace("alpha", &[]), workspace("beta", &[])])
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
        db.sync_all(&all).unwrap();

        assert_eq!(paged_ids(&db, SortOrder::Id, 2), ascending);

        // These workspaces share a creation date and so a last_access, making
        // this the case where a missing tiebreak would skip or repeat rows
        // across pages: recency falls back to id DESC, hence reverse order.
        let descending: Vec<String> = ascending.iter().rev().cloned().collect();
        assert_eq!(paged_ids(&db, SortOrder::Recency, 2), descending);
    }

    #[test]
    fn paging_a_filtered_query_keeps_the_filter() {
        let db = memory_db();
        db.seed_all(&[
            workspace("alpha", &["keep"]),
            workspace("beta", &["keep"]),
            workspace("gamma", &[]),
        ])
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
    fn query_round_trips_workspace_fields() {
        let db = memory_db();
        let mut ws = workspace("wsjump", &["rust", "tool"]);
        ws.desc = "line one\nline two".to_string();
        let expected_path = ws.path.clone();
        let expected_date = ws.date;
        db.seed_all(&[ws]).unwrap();

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
        db.sync_all(&[workspace("wsjump", &[])]).unwrap();

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
        db.sync_all(&[workspace("alpha", &[]), workspace("beta", &[])])
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
        db.sync_all(&[workspace("alpha", &[])]).unwrap();
        db.touch("20260730-alpha", 1_800_000_000).unwrap();

        db.sync_all(&[workspace("alpha", &[])]).unwrap();
        assert_eq!(
            stored_last_access(&db, "20260730-alpha"),
            Some(1_800_000_000),
            "a recorded access must outlive the syncs that follow it"
        );
    }
}
