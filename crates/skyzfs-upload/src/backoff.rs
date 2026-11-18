//! How long the uploader waits after a step that failed (M2.4c).
//!
//! The lead's decision on G7 of #114's review: a commit given up after a
//! conflict, or any error of a step, puts the uploader in a backoff that
//! doubles from one second to five minutes and is reset on a success. The
//! uploader reads a success as a cut committed or found committed, not as
//! any step that goes on ([`crate::uploader`]'s doc). Without it a store
//! that refuses every commit (a fencing
//! store with no lease row, the review's S9) would have the whole cut
//! uploaded again at every cycle, and its segments pile up.

use std::time::Duration;

/// The first wait.
pub const FIRST: Duration = Duration::from_secs(1);

/// The longest wait.
pub const LAST: Duration = Duration::from_secs(300);

/// The waits since the last step that succeeded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backoff {
    /// The next wait; `None` while nothing has failed since the last
    /// success.
    next: Option<Duration>,
}

impl Backoff {
    /// No failure yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// A step failed: how long to wait before the next. The wait doubles
    /// at each failure in a row, from [`FIRST`] to [`LAST`].
    pub fn failed(&mut self) -> Duration {
        let wait = self.next.unwrap_or(FIRST);
        self.next = Some(wait.saturating_mul(2).min(LAST));
        wait
    }

    /// A step succeeded: the next failure waits [`FIRST`] again.
    pub fn succeeded(&mut self) {
        self.next = None;
    }

    /// Whether the last step failed.
    pub fn failing(&self) -> bool {
        self.next.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_doubles_from_a_second_to_five_minutes() {
        let mut b = Backoff::new();
        let waits: Vec<u64> = (0..12).map(|_| b.failed().as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300, 300]);
        assert!(b.failing());
    }

    #[test]
    fn a_success_starts_the_waits_again() {
        let mut b = Backoff::new();
        assert!(!b.failing());
        b.failed();
        b.failed();
        b.failed();
        b.succeeded();
        assert!(!b.failing());
        assert_eq!(b.failed(), FIRST);
        assert_eq!(b.failed(), FIRST * 2);
    }
}
