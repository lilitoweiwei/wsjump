//! Reporting slow work in progress, so a pause is never unexplained.
//!
//! Sync is normally a few milliseconds and any output would be noise. But it can
//! be slow -- a first run, a changed root, a cold
//! cache, a very large tree -- and then silence leaves the user staring at a
//! stalled prompt.
//!
//! So progress is *deferred*: nothing is shown until the work has already taken
//! longer than a threshold, and the message appears while the user is still
//! waiting rather than as a summary afterwards, which would be useless.
//!
//! Core decides *when* there is something worth saying; front-ends decide how to
//! show it, because a terminal, a pipe and a full-screen UI need different
//! treatment.
//!
//! The one exception is [`StderrProgress`]: both of this project's front-ends
//! independently settled on "one stderr line" as their *how*, so that choice is
//! shared rather than duplicated.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

/// How long work must run before its progress is worth mentioning.
///
/// Chosen to sit above any plausible steady-state sync, so the fast path stays
/// completely silent, and below the point where a person starts wondering if
/// something is broken.
const QUIET_PERIOD: Duration = Duration::from_millis(200);

/// Shortest gap between updates once reporting has begun.
///
/// Without this, a large tree would emit a write syscall per workspace, which
/// can itself dominate the work being reported on.
const UPDATE_INTERVAL: Duration = Duration::from_millis(100);

/// How far along a piece of slow work is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Items dealt with so far.
    pub done: usize,
    /// Items in total, when known.
    pub total: usize,
}

/// Rate-limited progress reporting for one unit of work.
///
/// Construct one, call [`Reporter::advance`] as the work proceeds, and
/// [`Reporter::finish`] at the end. Nothing reaches the sink until the work has
/// outlasted the quiet period.
pub struct Reporter<'a> {
    started: Instant,
    last_update: Option<Instant>,
    /// Whether anything has been emitted, so `finish` knows if there is
    /// something to clean up.
    reported: bool,
    sink: &'a mut dyn ProgressSink,
}

/// Where progress goes. Implemented per front-end.
pub trait ProgressSink {
    /// Show current progress, replacing anything shown before.
    fn update(&mut self, progress: Progress);
    /// Work is over; remove any progress display.
    ///
    /// Only called when [`ProgressSink::update`] was called at least once, so an
    /// implementation never has to undo something it never did.
    fn clear(&mut self);
}

impl<'a> Reporter<'a> {
    /// Begin timing a unit of work reporting to `sink`.
    pub fn new(sink: &'a mut dyn ProgressSink) -> Reporter<'a> {
        Reporter {
            started: Instant::now(),
            last_update: None,
            reported: false,
            sink,
        }
    }

    /// Note progress, emitting an update if one is due.
    pub fn advance(&mut self, progress: Progress) {
        let now = Instant::now();
        if now.duration_since(self.started) < QUIET_PERIOD {
            return;
        }
        if let Some(last) = self.last_update {
            if now.duration_since(last) < UPDATE_INTERVAL {
                return;
            }
        }
        self.last_update = Some(now);
        self.reported = true;
        self.sink.update(progress);
    }

    /// Finish the work, clearing the display if anything was shown.
    pub fn finish(&mut self) {
        if self.reported {
            self.sink.clear();
            self.reported = false;
        }
    }
}

impl Drop for Reporter<'_> {
    /// Clear on the way out, so an early return or an error cannot leave a
    /// half-drawn progress line behind.
    fn drop(&mut self) {
        self.finish();
    }
}

/// A sink that discards progress, for callers with nothing to draw on.
pub struct SilentSink;

impl ProgressSink for SilentSink {
    fn update(&mut self, _progress: Progress) {}
    fn clear(&mut self) {}
}

/// Writes sync progress to stderr, adapting to whether it is a terminal.
///
/// Always stderr, never stdout: `wsjump ls` output gets piped, and progress
/// chatter mixed into it would corrupt whatever consumes it.
///
/// The shape depends on where stderr goes. On a terminal a single line is
/// rewritten in place and erased afterwards, leaving no trace of a delay the
/// user has already lived through. When stderr is redirected there is no cursor
/// to move, so carriage returns and erase sequences would just be junk in a log
/// file; one plain line is written instead.
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

impl Default for StderrProgress {
    fn default() -> StderrProgress {
        StderrProgress::new()
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
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What a front-end would have displayed.
    #[derive(Default)]
    struct Display {
        updates: Vec<Progress>,
        clears: usize,
    }

    /// A sink sharing its record, so a test can inspect it while the reporter is
    /// still alive -- which is the only way to check that an update arrives
    /// *during* the work rather than after it.
    #[derive(Clone, Default)]
    struct Recording(Rc<RefCell<Display>>);

    impl Recording {
        fn updates(&self) -> Vec<Progress> {
            self.0.borrow().updates.clone()
        }
        fn clears(&self) -> usize {
            self.0.borrow().clears
        }
    }

    impl ProgressSink for Recording {
        fn update(&mut self, progress: Progress) {
            self.0.borrow_mut().updates.push(progress);
        }
        fn clear(&mut self) {
            self.0.borrow_mut().clears += 1;
        }
    }

    fn progress(done: usize) -> Progress {
        Progress { done, total: 10 }
    }

    #[test]
    fn fast_work_says_nothing() {
        // The common case by far. Any output here would be noise on every single
        // invocation.
        let recording = Recording::default();
        {
            let mut sink = recording.clone();
            let mut reporter = Reporter::new(&mut sink);
            for i in 0..10 {
                reporter.advance(progress(i));
            }
            reporter.finish();
        }
        assert!(recording.updates().is_empty());
        assert_eq!(recording.clears(), 0, "nothing shown, so nothing to clear");
    }

    #[test]
    fn slow_work_reports_while_it_is_still_running() {
        // The point of reporting at all: the message has to arrive during the
        // wait, not as an epitaph for it.
        let recording = Recording::default();
        let mut sink = recording.clone();
        let mut reporter = Reporter::new(&mut sink);
        // Pretend the work has already been going a while.
        reporter.started -= QUIET_PERIOD;
        reporter.advance(progress(3));

        assert_eq!(
            recording.updates(),
            vec![progress(3)],
            "an update must be emitted before the work ends"
        );
    }

    #[test]
    fn updates_are_rate_limited() {
        // A large tree would otherwise emit a write per workspace, which can cost
        // more than the work being measured.
        let recording = Recording::default();
        {
            let mut sink = recording.clone();
            let mut reporter = Reporter::new(&mut sink);
            reporter.started -= QUIET_PERIOD;
            for i in 0..100 {
                reporter.advance(progress(i));
            }
            reporter.finish();
        }
        assert_eq!(
            recording.updates().len(),
            1,
            "back-to-back calls must collapse into one update"
        );
    }

    #[test]
    fn a_reported_run_is_cleared() {
        let recording = Recording::default();
        {
            let mut sink = recording.clone();
            let mut reporter = Reporter::new(&mut sink);
            reporter.started -= QUIET_PERIOD;
            reporter.advance(progress(1));
            reporter.finish();
        }
        assert_eq!(recording.clears(), 1);
    }

    #[test]
    fn clearing_twice_only_happens_once() {
        // finish() then drop must not clear twice; a second clear could erase a
        // line something else has since written.
        let recording = Recording::default();
        {
            let mut sink = recording.clone();
            let mut reporter = Reporter::new(&mut sink);
            reporter.started -= QUIET_PERIOD;
            reporter.advance(progress(1));
            reporter.finish();
            reporter.finish();
        }
        assert_eq!(recording.clears(), 1);
    }

    #[test]
    fn dropping_without_finishing_still_clears() {
        // An error path must not leave a partial progress line on screen.
        let recording = Recording::default();
        {
            let mut sink = recording.clone();
            let mut reporter = Reporter::new(&mut sink);
            reporter.started -= QUIET_PERIOD;
            reporter.advance(progress(1));
            // No finish(): simulate an early return.
        }
        assert_eq!(recording.clears(), 1);
    }

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
