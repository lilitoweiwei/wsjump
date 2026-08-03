//! Picker application state.

use tui_input::Input;

use wsjump_core::workspace::Workspace;
use wsjump_core::{filter, query};

/// Holds all interactive state for the picker: the full workspace list, the
/// query input, the indices of currently matching workspaces, and which match
/// is selected.
pub struct App {
    /// All workspaces discovered by the scan (sorted by id in core).
    workspaces: Vec<Workspace>,
    /// The query input box (string + cursor), managed by `tui-input`.
    input: Input,
    /// Indices into `workspaces` of the current matches, in display order.
    matches: Vec<usize>,
    /// Index into `matches` of the highlighted row (not into `workspaces`).
    selected: usize,
}

impl App {
    /// Build the app from the full workspace list; initially everything matches.
    pub fn new(workspaces: Vec<Workspace>) -> Self {
        let matches = (0..workspaces.len()).collect();
        App {
            workspaces,
            input: Input::default(),
            matches,
            selected: 0,
        }
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

    /// Recompute matches from the current query and reset selection to the top.
    /// Call after any change to the input.
    pub fn refilter(&mut self) {
        let tokens = query::parse_str(self.input.value());
        self.matches = filter::filter_indices(&self.workspaces, &tokens);
        self.selected = 0;
    }

    /// The workspaces currently matching, in display order.
    pub fn matched(&self) -> impl Iterator<Item = &Workspace> {
        self.matches.iter().map(move |&i| &self.workspaces[i])
    }

    /// Number of current matches.
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Index of the highlighted row within the match list.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Move the selection down one row (clamped to the last match).
    pub fn select_next(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        if self.selected + 1 < self.matches.len() {
            self.selected += 1;
        }
    }

    /// Move the selection up one row (clamped to the first match).
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// The absolute path of the currently selected workspace, if any.
    pub fn selected_path(&self) -> Option<String> {
        self.matches
            .get(self.selected)
            .map(|&i| self.workspaces[i].path.display().to_string())
    }
}
