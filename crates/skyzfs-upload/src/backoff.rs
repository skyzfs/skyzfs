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
//!
//! A whole cut that goes before its commit has a wait of its own, longer
//! ([`WHOLE_FIRST`] to [`WHOLE_LAST`]; the lead's decision for M2.4f):
//! under a load that fills the bound of copies before a whole cut can be
//! read, every attempt is abandoned, and each leaves the segments it PUT
//! in object storage for M4's collection. In the bench's run 44412222348,
//! 33 whole cuts were abandoned in 356 s and left 22.7 GiB. One attempt
//! per wait bounds that, and a commit that lands ends the wait.

use std::time::Duration;

/// The first wait.
pub const FIRST: Duration = Duration::from_secs(1);

/// The longest wait.
pub const LAST: Duration = Duration::from_secs(300);

/// The first wait after a whole cut went before its commit (M2.4f).
pub const WHOLE_FIRST: Duration = Duration::from_secs(60);

/// The longest wait after a whole cut went before its commit (M2.4f):
/// once the load drops, the seed begins at most this long after.
pub const WHOLE_LAST: Duration = Duration::from_secs(15 * 60);

/// The waits since the last step that succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    /// The first wait.
    first: Duration,
    /// The longest wait.
    last: Duration,
    /// The next wait; `None` while nothing has failed since the last
    /// success.
    next: Option<Duration>,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

impl Backoff {
    /// No failure yet; waits from [`FIRST`] to [`LAST`].
    pub fn new() -> Self {
        Self::between(FIRST, LAST)
    }

    /// No failure yet; waits from `first`, doubling, to `last`.
    pub fn between(first: Duration, last: Duration) -> Self {
        Self {
            first,
            last,
            next: None,
        }
    }

    /// A step failed: how long to wait before the next. The wait doubles
    /// at each failure in a row, from the first wait to the longest.
    pub fn failed(&mut self) -> Duration {
        let wait = self.next.unwrap_or(self.first);
        self.next = Some(wait.saturating_mul(2).min(self.last));
        wait
    }

    /// A step succeeded: the next failure waits the first wait again.
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
    fn a_whole_cuts_wait_doubles_from_a_minute_to_fifteen() {
        let mut b = Backoff::between(WHOLE_FIRST, WHOLE_LAST);
        let waits: Vec<u64> = (0..7).map(|_| b.failed().as_secs()).collect();
        assert_eq!(waits, [60, 120, 240, 480, 900, 900, 900]);
        b.succeeded();
        assert_eq!(b.failed(), WHOLE_FIRST);
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
