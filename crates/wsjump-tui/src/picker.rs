//! Interactive picker.
//!
//! Stage 1 skeleton: sets up an inline ratatui terminal, waits for a quit key,
//! then tears the terminal down cleanly. The real UI (list, input, events)
//! arrives in stage 2.

use std::io::{self, Stderr};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};

use wsjump_core::error::{AppError, AppResult};

/// Number of terminal rows the inline UI occupies:
/// candidate list (10) + input line (1) + status line (1).
const VIEWPORT_HEIGHT: u16 = 12;

/// Outcome of a picker session.
// Stage 1 skeleton never selects; `Selected` is wired up in stage 2.
#[allow(dead_code)]
pub enum Outcome {
    /// User selected a workspace; carries its absolute path as a string.
    Selected(String),
    /// User aborted (Esc / Ctrl-C) or there was nothing to select.
    Aborted,
}

/// Run the picker. On success returns whether a workspace was selected.
///
/// The UI is drawn to stderr so that stdout stays clean for the selected path
/// (consumed by the `ju` shell function).
pub fn run() -> AppResult<Outcome> {
    let mut terminal = setup_terminal()?;
    let outcome = event_loop(&mut terminal);
    // Always attempt teardown, even if the loop errored, to avoid leaving the
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

/// Stage 1 event loop: block for keys, quit on Esc or Ctrl-C.
fn event_loop(_terminal: &mut Terminal<CrosstermBackend<Stderr>>) -> AppResult<Outcome> {
    loop {
        let evt = event::read().map_err(io_err)?;
        if let Event::Key(key) = evt {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let is_ctrl_c =
                key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
            if key.code == KeyCode::Esc || is_ctrl_c {
                return Ok(Outcome::Aborted);
            }
        }
    }
}

/// Map an IO error to a runtime `AppError`.
fn io_err(e: io::Error) -> AppError {
    AppError::runtime(format!("terminal error: {}", e))
}
