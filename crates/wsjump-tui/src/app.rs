//! Picker application state.

use tui_input::Input;

use wsjump_core::db::Cursor;
use wsjump_core::error::AppResult;
use wsjump_core::index::Index;
use wsjump_core::query::{self, SortOrder};
use wsjump_core::workspace::Workspace;

/// Number of characters to shift per horizontal scroll step.
const HSCROLL_STEP: usize = 8;

/// How many rows are fetched per query.
///
/// Comfortably more than the viewport so ordinary scrolling does not query on
/// every keypress, but small enough that a query never walks the whole index.
const BATCH_SIZE: usize = 32;

/// Interactive state for the picker.
///
/// Results are held one batch at a time rather than as the full workspace list
/// filtered in memory: the database does the filtering and ordering, and more
/// rows are fetched only when the user scrolls far enough to need them.
pub struct App {
    /// The queryable index; also where visits are recorded.
    index: Index,
    /// The query input box (string + cursor), managed by `tui-input`.
    input: Input,
    /// Requested result order.
    sort: SortOrder,
    /// Results fetched so far for the current query, in display order.
    loaded: Vec<Workspace>,
    /// Where to resume fetching, or `None` if everything has been fetched.
    cursor: Option<Cursor>,
    /// Index into `loaded` of the highlighted row.
    selected: usize,
    /// Horizontal scroll offset (in characters) applied to list rows so the
    /// user can reveal content truncated on the right.
    hscroll: usize,
}

impl App {
    /// Build the app and fetch the first batch of results.
    pub fn new(index: Index, default_sort: SortOrder) -> AppResult<Self> {
        // Without a database there are no access times to order by, so offering
        // recency would misdescribe what the user is looking at.
        let sort = if index.is_indexed() {
            default_sort
        } else {
            SortOrder::Name
        };

        let mut app = App {
            index,
            input: Input::default(),
            sort,
            loaded: Vec::new(),
            cursor: None,
            selected: 0,
            hscroll: 0,
        };
        app.reload()?;
        Ok(app)
    }

    /// Mutable access to the input box (for feeding key events).
    pub fn input_mut(&mut self) -> &mut Input {
        &mut self.input
    }

    /// The current query string.
    pub fn query(&self) -> &str {
        self.input.value()
    }

    /// The input cursor position (character index within the query).
    pub fn cursor(&self) -> usize {
        self.input.visual_cursor()
    }

    /// The current result order.
    pub fn sort(&self) -> SortOrder {
        self.sort
    }

    /// Whether ordering by recency is possible at all.
    pub fn can_sort_by_recency(&self) -> bool {
        self.index.is_indexed()
    }

    /// Switch between recency and name order, refetching from the start.
    ///
    /// Does nothing when there is no database, since recency cannot be honoured
    /// and pretending otherwise would misreport the order on screen.
    pub fn toggle_sort(&mut self) -> AppResult<()> {
        if !self.index.is_indexed() {
            return Ok(());
        }
        self.sort = self.sort.toggled();
        self.reload()
    }

    /// Discard results and fetch the first batch for the current query and order.
    /// Call after any change to the input.
    pub fn reload(&mut self) -> AppResult<()> {
        let tokens = query::parse_str(self.input.value());
        let page = self.index.query(&tokens, self.sort, BATCH_SIZE, None)?;
        self.loaded = page.workspaces;
        self.cursor = page.next;
        self.selected = 0;
        self.hscroll = 0;
        Ok(())
    }

    /// Fetch the next batch and append it, if any remain.
    fn load_more(&mut self) -> AppResult<()> {
        let Some(cursor) = self.cursor.clone() else {
            return Ok(());
        };
        let tokens = query::parse_str(self.input.value());
        let page = self
            .index
            .query(&tokens, self.sort, BATCH_SIZE, Some(&cursor))?;
        self.loaded.extend(page.workspaces);
        self.cursor = page.next;
        Ok(())
    }

    /// The results fetched so far, in display order.
    pub fn results(&self) -> &[Workspace] {
        &self.loaded
    }

    /// How many results have been fetched so far.
    ///
    /// This is not the total number of matches: counting those would mean a
    /// second query over the whole result set, which paging exists to avoid.
    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    /// Whether more results exist beyond those fetched.
    pub fn has_more(&self) -> bool {
        self.cursor.is_some()
    }

    /// Index of the highlighted row within the fetched results.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Move the selection down one row, fetching more results if it reaches the
    /// end of what has been fetched.
    pub fn select_next(&mut self) -> AppResult<()> {
        if self.loaded.is_empty() {
            return Ok(());
        }
        if self.selected + 1 >= self.loaded.len() {
            // At the bottom: extend the list rather than refusing to move.
            self.load_more()?;
        }
        if self.selected + 1 < self.loaded.len() {
            self.selected += 1;
        }
        Ok(())
    }

    /// Move the selection up one row (clamped to the first result).
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Current horizontal scroll offset (in characters).
    pub fn hscroll(&self) -> usize {
        self.hscroll
    }

    /// Scroll list content left (toward the beginning), clamped at 0.
    pub fn scroll_left(&mut self) {
        self.hscroll = self.hscroll.saturating_sub(HSCROLL_STEP);
    }

    /// Scroll list content right (reveal truncated content).
    pub fn scroll_right(&mut self) {
        self.hscroll += HSCROLL_STEP;
    }

    /// Record a visit to the selected workspace and return its absolute path.
    ///
    /// The visit is recorded here, at the moment of selection, because the actual
    /// directory change happens in the shell function and is beyond our reach.
    pub fn take_selected(&self) -> Option<String> {
        let ws = self.loaded.get(self.selected)?;
        self.index.record_visit(&ws.key);
        Some(ws.path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use wsjump_core::workspace::sort_by_name;

    /// A flat workspace with the given complete name.
    fn workspace(name: &str) -> Workspace {
        Workspace {
            path: PathBuf::from(format!("G:/ws/w/{}", name)),
            key: format!("w/{}", name),
            name: name.to_string(),
            tags: Vec::new(),
            desc: String::new(),
            last_access: None,
        }
    }

    /// An app over `count` workspaces named after letters, without a database.
    ///
    /// The degraded index is used because it needs no file, and the paging
    /// contract it implements is the same one the database implements.
    fn app_with(count: usize) -> App {
        let mut workspaces: Vec<Workspace> = (0..count)
            .map(|i| workspace(&((b'a' + i as u8) as char).to_string()))
            .collect();
        sort_by_name(&mut workspaces);
        App::new(Index::Degraded(workspaces), SortOrder::Name).expect("build app")
    }

    /// Type a query into the app and re-query.
    fn type_query(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.input_mut()
                .handle(tui_input::InputRequest::InsertChar(ch));
        }
        app.reload().expect("reload");
    }

    #[test]
    fn first_batch_is_loaded_on_startup() {
        let app = app_with(3);
        assert_eq!(app.loaded_count(), 3);
        assert!(!app.has_more());
        assert_eq!(app.selected(), 0);
    }

    #[test]
    fn a_large_result_set_is_not_all_loaded_at_once() {
        let app = app_with(BATCH_SIZE + 5);
        assert_eq!(app.loaded_count(), BATCH_SIZE);
        assert!(app.has_more(), "the rest must be reported as pending");
    }

    #[test]
    fn scrolling_past_the_batch_loads_more() {
        let mut app = app_with(BATCH_SIZE + 5);
        // Walk to the end of the first batch, then one row further.
        for _ in 0..BATCH_SIZE {
            app.select_next().expect("select next");
        }
        assert_eq!(app.selected(), BATCH_SIZE);
        assert!(
            app.loaded_count() > BATCH_SIZE,
            "reaching the end of the batch must fetch the next one"
        );
    }

    #[test]
    fn selection_stops_at_the_last_result() {
        let mut app = app_with(3);
        for _ in 0..10 {
            app.select_next().expect("select next");
        }
        assert_eq!(app.selected(), 2);
    }

    #[test]
    fn selection_stops_at_the_first_result() {
        let mut app = app_with(3);
        app.select_next().expect("select next");
        for _ in 0..10 {
            app.select_prev();
        }
        assert_eq!(app.selected(), 0);
    }

    #[test]
    fn selecting_in_an_empty_result_set_does_nothing() {
        let mut app = app_with(0);
        app.select_next().expect("select next");
        assert_eq!(app.selected(), 0);
        assert!(app.take_selected().is_none());
    }

    #[test]
    fn typing_filters_and_resets_the_selection() {
        let mut app = app_with(3);
        app.select_next().expect("select next");
        type_query(&mut app, "b");

        assert_eq!(app.loaded_count(), 1);
        assert_eq!(
            app.selected(),
            0,
            "a new query must not leave the highlight on a row from the old one"
        );
    }

    #[test]
    fn selected_path_follows_the_highlight() {
        let mut app = app_with(3);
        assert_eq!(
            app.take_selected().unwrap(),
            workspace("a").path.display().to_string()
        );
        app.select_next().expect("select next");
        assert_eq!(
            app.take_selected().unwrap(),
            workspace("b").path.display().to_string()
        );
    }

    #[test]
    fn horizontal_scroll_clamps_at_the_start() {
        let mut app = app_with(1);
        app.scroll_left();
        assert_eq!(app.hscroll(), 0);
        app.scroll_right();
        assert_eq!(app.hscroll(), HSCROLL_STEP);
        app.scroll_left();
        assert_eq!(app.hscroll(), 0);
    }

    #[test]
    fn scroll_is_reset_by_a_new_query() {
        let mut app = app_with(3);
        app.scroll_right();
        type_query(&mut app, "b");
        assert_eq!(
            app.hscroll(),
            0,
            "content shifted off-screen would otherwise hide the new results"
        );
    }

    #[test]
    fn recency_is_refused_without_a_database() {
        // Falling back silently would label the list 'recency' while showing name
        // order.
        let app =
            App::new(Index::Degraded(vec![workspace("a")]), SortOrder::Recency).expect("build app");
        assert!(!app.can_sort_by_recency());
        assert_eq!(app.sort(), SortOrder::Name);
    }

    #[test]
    fn toggling_sort_does_nothing_without_a_database() {
        let mut app = app_with(2);
        app.toggle_sort().expect("toggle");
        assert_eq!(app.sort(), SortOrder::Name);
    }

    /// A temporary database file, removed when dropped.
    struct TempDb {
        path: PathBuf,
    }

    impl TempDb {
        fn new(tag: &str) -> TempDb {
            let name = format!(
                "wsjump-tui-test-{}-{}-{:?}.db",
                tag,
                std::process::id(),
                std::thread::current().id()
            );
            let path = std::env::temp_dir().join(name);
            let _ = std::fs::remove_file(&path);
            TempDb { path }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            // WAL leaves sidecar files behind.
            for suffix in ["-wal", "-shm"] {
                let mut side = self.path.clone().into_os_string();
                side.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(side));
            }
            // The directory tree the workspaces were discovered from.
            let _ = std::fs::remove_dir_all(self.path.with_extension("tree"));
        }
    }

    /// An app backed by a real database holding the supplied workspace names.
    ///
    /// The workspaces are materialised as real directories, because sync now
    /// discovers them from disk rather than being handed a list.
    fn indexed_app(temp: &TempDb, names: &[&str], sort: SortOrder) -> App {
        let tree = temp.path.with_extension("tree");
        let _ = std::fs::remove_dir_all(&tree);
        for name in names {
            std::fs::create_dir_all(tree.join("w").join(name)).expect("create workspace dir");
        }
        let config = wsjump_core::config::Config::for_test(&tree);
        let found = wsjump_core::scan::discover(&config).expect("discover");

        let mut db = wsjump_core::db::Db::open(&temp.path).expect("open db");
        db.sync(
            &found,
            &wsjump_core::db::SyncScope::Everything,
            false,
            &mut wsjump_core::progress::SilentSink,
        )
        .expect("sync");
        for name in names {
            db.touch(&format!("w/{name}"), 1)
                .expect("seed equal access times");
        }
        App::new(Index::Indexed(db), sort).expect("build app")
    }

    #[test]
    fn sort_toggles_between_recency_and_name() {
        let temp = TempDb::new("toggle");
        let mut app = indexed_app(&temp, &["a", "b"], SortOrder::Recency);
        assert!(app.can_sort_by_recency());
        assert_eq!(app.sort(), SortOrder::Recency);

        app.toggle_sort().expect("toggle");
        assert_eq!(app.sort(), SortOrder::Name);
        app.toggle_sort().expect("toggle back");
        assert_eq!(app.sort(), SortOrder::Recency);
    }

    #[test]
    fn selecting_records_a_visit_that_reorders_later_queries() {
        let temp = TempDb::new("visit");
        let mut app = indexed_app(&temp, &["a", "b"], SortOrder::Recency);

        // These share an access time, so recency breaks the tie by key
        // descending: "b" leads until something is actually visited.
        assert_eq!(app.results()[0].key, "w/b");

        app.select_next().expect("select next");
        assert_eq!(app.results()[app.selected()].key, "w/a");
        app.take_selected().expect("select a workspace");

        // Recording the visit is the whole point of selection: the workspace just
        // jumped to must lead next time.
        app.reload().expect("reload");
        assert_eq!(app.results()[0].key, "w/a");
    }

    #[test]
    fn same_name_candidates_jump_to_their_independent_paths() {
        let mut first = workspace("demo");
        first.key = "c/_ai/demo".into();
        first.path = PathBuf::from("G:/ws/c/_ai/demo");
        let mut second = workspace("demo");
        second.key = "c/_nvim/demo".into();
        second.path = PathBuf::from("G:/ws/c/_nvim/demo");
        let mut app = App::new(Index::Degraded(vec![first, second]), SortOrder::Name).unwrap();
        assert_eq!(app.take_selected().as_deref(), Some("G:/ws/c/_ai/demo"));
        app.select_next().unwrap();
        assert_eq!(app.take_selected().as_deref(), Some("G:/ws/c/_nvim/demo"));
    }
}
