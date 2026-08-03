//! Interactive picker: sets up an inline ratatui terminal, runs the
//! event loop over the app state, then tears the terminal down cleanly.

use std::io::{self, Stderr};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
use tui_input::backend::crossterm::EventHandler;

use wsjump_core::config::Config;
use wsjump_core::error::{AppError, AppResult};
use wsjump_core::scan;

use crate::app::App;
use crate::render;

/// Number of terminal rows the inline UI occupies:
/// candidate list (10) + input line (1) + status line (1).
const VIEWPORT_HEIGHT: u16 = 12;

/// Outcome of a picker session.
pub enum Outcome {
    /// User selected a workspace; carries its absolute path as a string.
    Selected(String),
    /// User aborted (Esc / Ctrl-C) or there was nothing to select.
    Aborted,
}

/// Run the picker. Loads config + workspaces, drives the UI, returns the
/// user's choice. The UI is drawn to stderr so stdout stays clean for the
/// selected path (consumed by the `ju` shell function).
pub fn run() -> AppResult<Outcome> {
    let config = Config::load()?;
    let workspaces = scan::scan(&config)?;
    let mut app = App::new(workspaces);

    let mut terminal = setup_terminal()?;
    let outcome = event_loop(&mut terminal, &mut app);
    // Always attempt teardown, even if the loop errored, so we never leave the
    // terminal in raw mode.
    let teardown = teardown_terminal(&mut terminal);
    let outcome = outcome?;
    teardown?;
    Ok(outcome)
}

/// Enter raw mode and build an inline (non-alternate-screen) terminal on stderr.
fn setup_terminal() -> AppResult<Terminal<CrosstermBackend<Stderr>>> {
    enable_raw_mode().map_err(io_err)?;
    let backend = CrosstermBackend::new(io::stderr());
    let terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_HEIGHT),
        },
    )
    .map_err(io_err)?;
    Ok(terminal)
}

/// Restore the terminal: leave raw mode and clear the inline viewport.
fn teardown_terminal(terminal: &mut Terminal<CrosstermBackend<Stderr>>) -> AppResult<()> {
    disable_raw_mode().map_err(io_err)?;
    terminal.clear().map_err(io_err)?;
    Ok(())
}

/// Draw + read keys until the user selects or aborts.
fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stderr>>,
    app: &mut App,
) -> AppResult<Outcome> {
    loop {
        terminal
            .draw(|frame| render::draw(frame, app))
            .map_err(io_err)?;

        let Event::Key(key) = event::read().map_err(io_err)? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if let Some(outcome) = handle_key(app, key)? {
            return Ok(outcome);
        }
    }
}

/// Handle a single key press. Returns `Some(outcome)` when the session should
/// end, `None` to keep looping.
fn handle_key(app: &mut App, key: KeyEvent) -> AppResult<Option<Outcome>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => Ok(Some(Outcome::Aborted)),
        KeyCode::Char('c') if ctrl => Ok(Some(Outcome::Aborted)),
        KeyCode::Enter => match app.selected_path() {
            Some(path) => Ok(Some(Outcome::Selected(path))),
            // Nothing to select (empty match set) -> treat as abort.
            None => Ok(Some(Outcome::Aborted)),
        },
        KeyCode::Up => {
            app.select_prev();
            Ok(None)
        }
        KeyCode::Down => {
            app.select_next();
            Ok(None)
        }
        // Everything else is fed to the input box; re-filter afterwards.
        _ => {
            if app.input_mut().handle_event(&Event::Key(key)).is_some() {
                app.refilter();
            }
            Ok(None)
        }
    }
}

/// Map an IO error to a runtime `AppError`.
fn io_err(e: io::Error) -> AppError {
    AppError::runtime(format!("terminal error: {}", e))
}
