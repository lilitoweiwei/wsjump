//! The workspace index: the query entry point used by every front-end.
//!
//! This ties the pieces together and, crucially, owns the decision of what to do
//! when the database is unusable. The database is an accelerator and a place to
//! keep access history; it is not on the critical path. A broken database must
//! never stop the user reaching a workspace, so this falls back to scanning the
//! directory directly, which always works because it needs nothing but `w/`.
//!
//! Front-ends see one API and therefore degrade identically; each does not get
//! to invent its own error handling.

use crate::config::Config;
use crate::db::{Cursor, Db, Page, SyncOutcome, SyncScope};
use crate::error::AppResult;
use crate::filter;
use crate::query::{SortOrder, Token};
use crate::scan::{self, Discovered};
use crate::workspace::{now_timestamp, sort_by_id, Workspace};

/// A queryable view of all workspaces, backed by the database when possible.
pub enum Index {
    /// The normal case: queries run as SQL and access history is recorded.
    Indexed(Db),
    /// The database is unusable. Queries run against a directory scan and
    /// recency ordering is unavailable, so results fall back to `id` order.
    Degraded(Vec<Workspace>),
}

impl Index {
    /// Open the index and bring it up to date with the directory.
    ///
    /// The sync runs once here rather than before every query: a process gets a
    /// consistent view of the directory at startup. For interactive use this
    /// matters, since re-syncing per keystroke would tie typing latency to the
    /// number of workspaces, while the directory is very unlikely to change
    /// during a session lasting seconds.
    ///
    /// Failure to open or sync is reported on stderr and degraded, not returned:
    /// the caller's real job is to find a workspace, and that is still possible.
    pub fn open(config: &Config) -> AppResult<Index> {
        Self::sync_and_open(config, false).map(|(index, _)| index)
    }

    /// Open the index, re-reading every workspace's metadata regardless of its
    /// stored signature, and report what that did.
    ///
    /// This is `wsjump sync`: the way to repair an index whose signatures agree
    /// with disk while its contents do not.
    pub fn open_forcing_sync(config: &Config) -> AppResult<(Index, SyncOutcome)> {
        Self::sync_and_open(config, true)
    }

    /// Open the index having synced only the given workspace.
    pub fn open_syncing_only(config: &Config, id: &str) -> AppResult<(Index, SyncOutcome)> {
        Self::open_with(config, &SyncScope::OnlyWorkspace(id.to_string()), true)
    }

    /// Open the index, syncing the whole tree.
    fn sync_and_open(config: &Config, force: bool) -> AppResult<(Index, SyncOutcome)> {
        Self::open_with(config, &SyncScope::Everything, force)
    }

    /// Discover the directory, then sync and open, degrading if the database
    /// cannot be used.
    ///
    /// Discovery deliberately stops short of reading any `wsmeta.txt`: that is
    /// the expensive part, and sync only needs it for workspaces whose signature
    /// moved. The database-free fallback does need all of it, so it pays that
    /// cost, but only when it is actually taken.
    fn open_with(
        config: &Config,
        scope: &SyncScope,
        force: bool,
    ) -> AppResult<(Index, SyncOutcome)> {
        let found = scan::discover(config)?;

        match Self::open_db(config, &found, scope, force) {
            Ok((db, outcome)) => Ok((Index::Indexed(db), outcome)),
            Err(message) => {
                eprintln!(
                    "wsjump: warning: {}; continuing without the database \
                     (results ordered by name, visits not recorded)",
                    message
                );
                // Only now is the metadata of every workspace needed, since
                // there is no index to answer from.
                let mut workspaces: Vec<Workspace> =
                    found.iter().map(scan::Discovered::load).collect();
                sort_by_id(&mut workspaces);
                Ok((Index::Degraded(workspaces), SyncOutcome::default()))
            }
        }
    }

    /// Open, migrate and sync the database, describing any failure as a string
    /// so the caller can decide to warn rather than abort.
    fn open_db(
        config: &Config,
        found: &[Discovered],
        scope: &SyncScope,
        force: bool,
    ) -> Result<(Db, SyncOutcome), String> {
        let path = config.db_file().map_err(|e| e.message)?;
        let offset = config.utc_offset().map_err(|e| e.message)?;
        let mut db = Db::open(&path).map_err(|e| e.message)?;
        let outcome = db
            .sync(found, offset, scope, force)
            .map_err(|e| e.message)?;
        Ok((db, outcome))
    }

    /// Whether recency ordering and visit recording are available.
    ///
    /// Front-ends use this to avoid offering recency ordering they cannot honour.
    pub fn is_indexed(&self) -> bool {
        matches!(self, Index::Indexed(_))
    }

    /// Fetch one page of workspaces matching `tokens`, in `sort` order.
    ///
    /// Pass `after: None` for the first page, then the previous page's `next`.
    pub fn query(
        &self,
        tokens: &[Token],
        sort: SortOrder,
        limit: usize,
        after: Option<&Cursor>,
    ) -> AppResult<Page> {
        match self {
            Index::Indexed(db) => db.query(tokens, sort, limit, after),
            // Without stored access times there is nothing to order by but the
            // name, whatever was asked for.
            Index::Degraded(workspaces) => Ok(page_from_scan(workspaces, tokens, limit, after)),
        }
    }

    /// Fetch every workspace matching `tokens`, in `sort` order.
    ///
    /// For callers that inherently need the whole result set and so gain nothing
    /// from paging.
    pub fn query_all(&self, tokens: &[Token], sort: SortOrder) -> AppResult<Vec<Workspace>> {
        let mut all = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.query(tokens, sort, PAGE_SIZE, cursor.as_ref())?;
            all.extend(page.workspaces);
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(all),
            }
        }
    }

    /// Record a visit to the workspace with the given id, at the current time.
    ///
    /// Best effort: a failure is warned about but not returned, since a recorded
    /// visit only affects future ordering and must never turn a successful jump
    /// into a failure.
    pub fn record_visit(&self, id: &str) {
        let Index::Indexed(db) = self else {
            return;
        };
        match db.touch(id, now_timestamp()) {
            Ok(true) => {}
            Ok(false) => eprintln!(
                "wsjump: warning: cannot record visit to '{}': not in the index",
                id
            ),
            Err(e) => eprintln!("wsjump: warning: cannot record visit to '{}': {}", id, e),
        }
    }
}

/// Page size used when walking the whole result set.
const PAGE_SIZE: usize = 256;

/// Answer a query from a scanned list, mimicking the database's paging.
fn page_from_scan(
    workspaces: &[Workspace],
    tokens: &[Token],
    limit: usize,
    after: Option<&Cursor>,
) -> Page {
    let matched = filter::filter(workspaces, tokens);

    // The list is in id order, so resuming means continuing past the cursor's id.
    let start = match after {
        Some(cursor) => matched
            .iter()
            .position(|ws| ws.id.as_str() > cursor.id())
            .unwrap_or(matched.len()),
        None => 0,
    };

    let remaining = &matched[start..];
    let page: Vec<Workspace> = remaining.iter().take(limit).map(|&ws| ws.clone()).collect();
    let next = if remaining.len() > limit {
        page.last().map(Cursor::for_workspace)
    } else {
        None
    };

    Page {
        workspaces: page,
        next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query;
    use std::path::PathBuf;
    use time::{Date, Month};

    /// A workspace created on 2026-07-30 with the given slug.
    fn workspace(slug: &str) -> Workspace {
        Workspace {
            id: format!("20260730-{}", slug),
            date: Date::from_calendar_date(2026, Month::July, 30).unwrap(),
            slug: slug.to_string(),
            path: PathBuf::from(format!("G:/ws/w/20260730-{}", slug)),
            tags: Vec::new(),
            desc: String::new(),
            last_access: None,
        }
    }

    /// A degraded index over the given slugs, in id order as `open` leaves it.
    fn degraded(slugs: &[&str]) -> Index {
        let mut workspaces: Vec<Workspace> = slugs.iter().map(|s| workspace(s)).collect();
        sort_by_id(&mut workspaces);
        Index::Degraded(workspaces)
    }

    #[test]
    fn degraded_index_reports_itself_as_such() {
        // Front-ends rely on this to stop offering recency ordering.
        assert!(!degraded(&["alpha"]).is_indexed());
    }

    #[test]
    fn degraded_query_still_filters() {
        let index = degraded(&["alpha", "beta"]);
        let tokens = query::parse_str("alpha");
        let page = index.query(&tokens, SortOrder::Recency, 10, None).unwrap();
        assert_eq!(page.workspaces.len(), 1);
        assert_eq!(page.workspaces[0].id, "20260730-alpha");
    }

    #[test]
    fn degraded_query_paging_covers_every_row_exactly_once() {
        let index = degraded(&["a", "b", "c", "d", "e"]);
        let expected: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| format!("20260730-{}", s))
            .collect();

        // Paging is re-implemented for this path, so it needs the same
        // exactly-once guarantee as the SQL one.
        let mut ids = Vec::new();
        let mut cursor = None;
        loop {
            let page = index
                .query(&[], SortOrder::Id, 2, cursor.as_ref())
                .unwrap();
            ids.extend(page.workspaces.iter().map(|ws| ws.id.clone()));
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(ids, expected);
    }

    #[test]
    fn degraded_query_reports_more_results() {
        let index = degraded(&["a", "b", "c"]);
        let page = index.query(&[], SortOrder::Id, 2, None).unwrap();
        assert_eq!(page.workspaces.len(), 2);
        assert!(page.has_more());
    }

    #[test]
    fn degraded_full_page_reports_no_more() {
        let index = degraded(&["a", "b"]);
        let page = index.query(&[], SortOrder::Id, 2, None).unwrap();
        assert!(!page.has_more());
    }

    #[test]
    fn degraded_query_all_returns_everything() {
        let index = degraded(&["a", "b", "c"]);
        let all = index.query_all(&[], SortOrder::Id).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn degraded_recency_request_falls_back_to_id_order() {
        let index = degraded(&["gamma", "alpha", "beta"]);
        // Recency cannot be honoured without stored access times; the order must
        // still be deterministic rather than arbitrary.
        let all = index.query_all(&[], SortOrder::Recency).unwrap();
        let ids: Vec<String> = all.into_iter().map(|ws| ws.id).collect();
        assert_eq!(
            ids,
            vec![
                "20260730-alpha".to_string(),
                "20260730-beta".to_string(),
                "20260730-gamma".to_string(),
            ]
        );
    }

    #[test]
    fn recording_a_visit_on_a_degraded_index_is_harmless() {
        // Nowhere to record it, but it must not panic or fail the caller.
        degraded(&["alpha"]).record_visit("20260730-alpha");
    }
}
