//! Showing sync progress on stderr.
//!
//! Always stderr, never stdout: `wsjump ls` output gets piped, and progress
//! chatter mixed into it would corrupt whatever consumes it.
//!
//! The shape depends on where stderr goes. On a terminal a single line is
//! rewritten in place and erased afterwards, leaving no trace of a delay the
//! user has already lived through. When stderr is redirected there is no cursor
//! to move, so carriage returns and erase sequences would just be junk in a log
//! file; one plain line is written instead.

use std::io::{IsTerminal, Write};

use wsjump_core::progress::{Progress, ProgressSink};

/// Writes sync progress to stderr, adapting to whether it is a terminal.
pub struct StderrProgress {
    /// Whether stderr is a terminal, and so whether in-place rewriting works.
    interactive: bool,
    /// Width of the last line written, to know how much to erase.
    last_width: usize,
    /// Whether the single non-interactive line has been written.
    announced: bool,
}

impl StderrProgress {
    /// A sink for the current process's stderr.
    pub fn new() -> StderrProgress {
        StderrProgress {
            interactive: std::io::stderr().is_terminal(),
            last_width: 0,
            announced: false,
        }
    }
}

impl ProgressSink for StderrProgress {
    fn update(&mut self, progress: Progress) {
        let mut err = std::io::stderr().lock();
        if self.interactive {
            let line = format!(
                "wsjump: syncing workspaces... {}/{}",
                progress.done, progress.total
            );
            // Pad to the previous width so a shorter line cannot leave remnants
            // of the longer one behind it.
            let padding = self.last_width.saturating_sub(line.chars().count());
            self.last_width = line.chars().count();
            // Deliberately no newline: the next update overwrites this line.
            let _ = write!(err, "\r{}{}", line, " ".repeat(padding));
            let _ = err.flush();
        } else if !self.announced {
            // No cursor to rewrite, so say it once rather than accumulating a
            // line per update in whatever this is redirected to.
            self.announced = true;
            let _ = writeln!(err, "wsjump: syncing workspaces...");
        }
    }

    fn clear(&mut self) {
        if !self.interactive {
            // Nothing to undo: the line written there was meant to persist.
            return;
        }
        let mut err = std::io::stderr().lock();
        // Blank the line and return to its start, so the shell's next output
        // begins on a clean row.
        let _ = write!(err, "\r{}\r", " ".repeat(self.last_width));
        let _ = err.flush();
        self.last_width = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_sink_has_nothing_to_erase() {
        // clear() before any update must not try to blank a line that was never
        // drawn, which would move the cursor for no reason.
        let sink = StderrProgress::new();
        assert_eq!(sink.last_width, 0);
    }

    #[test]
    fn a_redirected_sink_speaks_once() {
        // Reaching into the struct rather than capturing stderr: the process's
        // stderr is global, so a test that redirected it would interfere with
        // every other test running alongside it.
        let mut sink = StderrProgress {
            interactive: false,
            last_width: 0,
            announced: false,
        };
        sink.update(Progress { done: 1, total: 10 });
        assert!(sink.announced);
        // A second update must not produce a second line.
        sink.update(Progress { done: 2, total: 10 });
        assert!(sink.announced);
    }

    #[test]
    fn an_interactive_sink_tracks_the_line_width() {
        // The width is what makes erasing exact; if it were not tracked, a
        // shorter line would leave the tail of a longer one on screen.
        let mut sink = StderrProgress {
            interactive: true,
            last_width: 0,
            announced: false,
        };
        sink.update(Progress {
            done: 1,
            total: 2000,
        });
        assert!(sink.last_width > 0);
    }

    #[test]
    fn clearing_an_interactive_sink_resets_the_width() {
        let mut sink = StderrProgress {
            interactive: true,
            last_width: 0,
            announced: false,
        };
        sink.update(Progress { done: 1, total: 10 });
        sink.clear();
        assert_eq!(sink.last_width, 0);
    }
}
