//! When the window follows scheduled runs ("Afficher la fenêtre de suivi"),
//! decides whether a newly observed agent profile may bring it forward.
//!
//! The agent dispatches due profiles one after another in a single series.
//! Showing the window (restore + focus) for each profile stole the keyboard
//! from whatever the user was doing, and undid a minimization made during
//! the series. The window is therefore shown once, for the first profile of
//! a series; the user then stays in control of it until the series ends.

use std::time::{Duration, Instant};

/// Without any live job for this long, the series is considered over even if
/// the queue file still names a current profile (agent stopped abruptly).
/// Far above the gap between two profiles of the same series, which follow
/// each other immediately in the dispatcher.
pub(crate) const QUIET_RESET: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub(crate) struct WindowFollow {
    /// The window was already brought forward for the current series.
    shown: bool,
    /// First observation without a live agent job, since the last live one.
    quiet_since: Option<Instant>,
}

impl WindowFollow {
    /// A live agent job was observed (every poll while it runs).
    pub(crate) fn active(&mut self) {
        self.quiet_since = None;
    }

    /// A new agent profile started: returns whether the window may be shown
    /// for it (first profile of the series that asks for the window).
    pub(crate) fn start(&mut self, wants_window: bool) -> bool {
        self.active();
        if wants_window && !self.shown {
            self.shown = true;
            return true;
        }
        false
    }

    /// No live agent job was observed. `queue_running` tells whether the
    /// agent's queue still names a current profile (between two profiles of
    /// the same series). The series ends when the queue is empty, or after
    /// [`QUIET_RESET`] without any live job.
    pub(crate) fn quiet(&mut self, queue_running: bool, now: Instant) {
        let since = *self.quiet_since.get_or_insert(now);
        if !queue_running || now.saturating_duration_since(since) >= QUIET_RESET {
            self.shown = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Given a series of three profiles asking for the window,
    /// when each profile starts in turn,
    /// then the window is brought forward only for the first one.
    #[test]
    fn series_shows_window_once() {
        let mut f = WindowFollow::default();
        let t = Instant::now();
        assert!(f.start(true));
        assert!(!f.start(true));
        // Gap between two profiles: queue still names the finished one.
        f.quiet(true, t);
        assert!(!f.start(true));
        f.quiet(true, t + Duration::from_secs(2));
        assert!(!f.start(true));
    }

    /// Given a finished series (empty queue),
    /// when a later series starts,
    /// then the window is shown again once.
    #[test]
    fn next_series_shows_again() {
        let mut f = WindowFollow::default();
        let t = Instant::now();
        assert!(f.start(true));
        f.quiet(false, t);
        assert!(f.start(true));
        assert!(!f.start(true));
    }

    /// Given an agent that stopped abruptly (queue file still names a
    /// profile), when nothing runs for the reset delay, then a later series
    /// may show the window again.
    #[test]
    fn stale_queue_resets_after_quiet_delay() {
        let mut f = WindowFollow::default();
        let t = Instant::now();
        assert!(f.start(true));
        f.quiet(true, t);
        f.quiet(true, t + QUIET_RESET - Duration::from_secs(1));
        assert!(!f.start(true));
        f.quiet(true, t + Duration::from_secs(100));
        f.quiet(true, t + Duration::from_secs(100) + QUIET_RESET);
        assert!(f.start(true));
    }

    /// Given a first profile without the window option,
    /// when a second profile of the series asks for it,
    /// then that one shows the window (still the first show of the series).
    #[test]
    fn profile_without_option_does_not_consume_the_show() {
        let mut f = WindowFollow::default();
        assert!(!f.start(false));
        assert!(f.start(true));
        assert!(!f.start(true));
    }
}
