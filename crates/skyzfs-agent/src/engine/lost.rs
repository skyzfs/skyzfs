//! A volume that failed in this boot, and the pool on it (M2.7c; design
//! §10, the notes' "after a failure or a refusal, the pool of an
//! attached client is made to read").
//!
//! A daemon whose volume fails, or whose start refuses a failed volume,
//! writes a failure note under the run directory, deletes the volume's
//! device and exits (`skyzfs_control::failure`). A pool that only writes
//! does not notice: ZFS retries its failed writes without end, the pool
//! reads ONLINE, and applications are told "written" for data that is
//! only in the log device and in memory. The agent owns the pool, so it
//! acts:
//!
//! - **No note of this boot** ([`Failed::No`]): nothing.
//! - **A note that no client held the device** ([`Failed::Unheld`]):
//!   the volume failed, and no pool can be on its device. No pool is
//!   touched (the lead's answer to Q2). A start of the daemon with
//!   `--acknowledge-failure` serves the volume afresh and removes the
//!   note.
//! - **A note that a client held it, or may have** ([`Failed::Lost`]):
//!   the volume is lost for this boot, and so is a pool on it. The
//!   daemon refuses it until a reboot. [`Agent::lose`] makes the pool
//!   read, so that it suspends within seconds: `zpool reopen` within its
//!   limit ([`Zfs::reopen`]), then a look at the pool's state every
//!   [`LOOK_EVERY`] for at most [`LOOK_MAX`], until it reads SUSPENDED.
//!   **The look, not the command's status, is the signal**: the reopen
//!   returned with status 0 in about a second and the pool suspended
//!   about 1.0 to 1.1 s later (the daemon rig's check f), and where zed
//!   suspended the pool first (Ubuntu 26.04) the reopen is refused. The
//!   reopen is not made where the pool is not imported, or reads
//!   SUSPENDED already: so a process started again on the same note
//!   reports the volume lost again without a second reopen.
//! - **A file there that is not a whole note** is a failure while a
//!   client held the device, as the daemon's start counts it, and the
//!   agent takes the step (the lead's ruling); its report names the
//!   note as torn.
//!
//! What the step shows decides what `run` does ([`Step::verdict`], the
//! lead's rulings on F2 and F4 of the review of #173): only a pool shown
//! SUSPENDED makes the volume lost for `run`; a pool that is not
//! imported has nothing to suspend, and the volume is reported failed;
//! a step that could not show either is tried again.
//!
//! **While the engine works** ([`Agent::watch_note`], F1 of the review
//! of #173). A pool that only writes goes on acknowledging synchronous
//! writes after its device is gone (design §10), so `run` does not wait
//! for its next round to read a note: the engine reads it about every
//! [`NOTE_EVERY`] where it asks whether to go on (before each sync after
//! a rewind import and before each pass of the settle), and `run` reads
//! it as often while it waits for a tick. A read is of a small file
//! beside the daemon's socket, with no I/O on the pool. A note that
//! makes the volume lost, and that is not the one the round began with,
//! stops the engine there as an end would, and `run` begins its next
//! round at once.
//!
//! The step acts on the agent's own pool (`--pool`), whatever its
//! devices are now: the device under it is gone, so no premise can be
//! checked, and the pool is the one this volume's agent was set up for.
//! That is this module's reading, the conservative one: a pool that is
//! not on the volume is made to read, which suspends nothing that has
//! its devices. It is reopened at every round that takes the step, up
//! to [`REOPEN_CAP`] reopens of one note in a process, and at every
//! [`REOPEN_CAP`]th round after that (a reopen restarts a scrub under
//! way).
//!
//! What a round of `run` does with each, and the exit codes, are the
//! program's (`crate::program`, "`run`").

use std::time::Duration;

use skyzfs_control::failure::{Failure, Held};

use super::{Agent, Clock, Daemon, Error, Keep, Noted};
use crate::zfs::{PoolState, Zfs};

/// How often [`Agent::lose`] looks at the pool's state after the reopen.
pub const LOOK_EVERY: Duration = Duration::from_millis(250);

/// How often the engine reads the failure note while it works, and
/// `run` while it waits for a tick ([`Agent::watch_note`]; the lead's
/// ruling on F1 of the review of #173: "about every second").
pub const NOTE_EVERY: Duration = Duration::from_secs(1);

/// How many reopens of one note may leave the pool unsuspended before
/// `run` reopens only at every [`REOPEN_CAP`]th round (the lead's ruling
/// on the review of #173): a pool that never reads SUSPENDED, a wrong
/// `--pool` or one whose layout keeps it going, is not reopened every
/// round without end, and a scrub under way is not restarted each time.
/// Each round in between still looks at its state ([`Agent::look_lost`]).
pub const REOPEN_CAP: u64 = 10;

/// How soon `run` takes the step again after a round whose verdict is
/// [`Verdict::Retry`], for at most [`QUICK_RETRIES`] rounds in a row
/// before it goes back to one a tick (the lead's ruling on D1 of the
/// review of #173): "within seconds" holds for a retry too.
pub const QUICK_RETRY: Duration = Duration::from_secs(5);

/// How many quick retries ([`QUICK_RETRY`]) `run` takes in a row.
pub const QUICK_RETRIES: u32 = 3;

/// How long [`Agent::lose`] looks at most (the lead's answer to Q3 of
/// the plan of M2.7c; provisional). The pool suspended about 1.0 to
/// 1.1 s after the reopen in every run of check f.
pub const LOOK_MAX: Duration = Duration::from_secs(10);

/// What became of the volume in this boot, by its failure note
/// ([`Agent::failure`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failed {
    /// No failure note of this boot.
    No,
    /// The volume failed, and no client held its device: no pool is
    /// touched.
    Unheld(Failure),
    /// The volume failed while a client held its device, or may have,
    /// or its note is torn: lost for this boot.
    Lost(Note),
}

/// A note that makes the volume lost ([`Failed::Lost`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// A whole note, held [`Held::Yes`] or [`Held::Unknown`].
    Whole(Failure),
    /// A file that is not a whole note: held, as the daemon counts it.
    Torn,
}

impl Note {
    /// The reason's word ([`skyzfs_control::failure::Reason::word`]),
    /// `-` for a torn note.
    pub fn reason_word(&self) -> &'static str {
        match self {
            Note::Whole(f) => f.reason.word(),
            Note::Torn => "-",
        }
    }

    /// The holding's word; `yes` for a torn note.
    pub fn held_word(&self) -> &'static str {
        match self {
            Note::Whole(f) => f.held.word(),
            Note::Torn => Held::Yes.word(),
        }
    }
}

/// What [`Agent::lose`] did to the pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing: the pool is not imported.
    NotImported,
    /// Nothing: the pool reads SUSPENDED already.
    Suspended,
    /// Nothing: the pool reads `state`, not SUSPENDED, and the reopen is
    /// held back ([`Agent::look_lost`]): `run` caps the reopens of one
    /// note that leave the pool unsuspended ([`REOPEN_CAP`]).
    Held {
        /// The pool's state at the look.
        state: PoolState,
    },
    /// The pool was reopened, and looked at until it read SUSPENDED or
    /// [`LOOK_MAX`] passed.
    Reopened {
        /// The reopen's error, in words: not the signal ([`run`'s
        /// docs](crate::program)).
        error: Option<String>,
        /// The state at the last look; `None` where the pool was no
        /// longer imported, or the look failed (`unread`).
        state: Option<PoolState>,
        /// The error of the look that failed, in words, where one did:
        /// the look ends there, and the reopen's outcome is kept (F3 of
        /// the review of #173).
        unread: Option<String>,
        /// How long after the reopen returned the last look was, in
        /// milliseconds.
        looked_ms: u64,
    },
}

impl Step {
    /// The step's word for the `lost` line: `not-imported`, `suspended`
    /// or `reopened`.
    pub fn word(&self) -> &'static str {
        match self {
            Step::NotImported => "not-imported",
            Step::Suspended => "suspended",
            Step::Held { .. } => "held",
            Step::Reopened { .. } => "reopened",
        }
    }

    /// Whether the pool reads SUSPENDED after the step.
    pub fn suspended(&self) -> bool {
        match self {
            Step::NotImported | Step::Held { .. } => false,
            Step::Suspended => true,
            Step::Reopened { state, .. } => state.as_ref().is_some_and(PoolState::suspended),
        }
    }

    /// What the step shows, for `run` (this module's docs); `step` is
    /// `None` where the step failed before the reopen (the first look).
    pub fn verdict(step: Option<&Step>) -> Verdict {
        match step {
            None => Verdict::Retry,
            Some(Step::NotImported) => Verdict::Failed,
            Some(Step::Suspended) => Verdict::Lost,
            Some(Step::Held { .. }) => Verdict::Retry,
            Some(Step::Reopened {
                unread: Some(_), ..
            }) => Verdict::Retry,
            Some(Step::Reopened {
                state: Some(state), ..
            }) if state.suspended() => Verdict::Lost,
            Some(Step::Reopened { state: Some(_), .. }) => Verdict::Retry,
            Some(Step::Reopened { state: None, .. }) => Verdict::Failed,
        }
    }
}

/// What a step on a lost note shows ([`Step::verdict`]; the lead's
/// rulings on F2 and F4 of the review of #173).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The pool reads SUSPENDED: the volume is lost for this boot, and
    /// `run` ends.
    Lost,
    /// The pool is not imported, before the reopen or after it: nothing
    /// is suspended, and nothing can be. The volume is reported failed,
    /// and `run` goes on; a later start of the daemon may serve it.
    Failed,
    /// The step could not show either: the pool's state could not be
    /// read, or it did not read SUSPENDED within [`LOOK_MAX`] of the
    /// reopen (refused or not). The round fails, and the next tries
    /// again.
    Retry,
}

impl Verdict {
    /// The verdict's word for the `lost` line: `lost`, `failed` or
    /// `retry`.
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Lost => "lost",
            Verdict::Failed => "failed",
            Verdict::Retry => "retry",
        }
    }
}

/// The engine's watch on the failure note ([`Agent::watch_note`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Watch {
    /// What the round read before it set the watch: a note equal to it
    /// is not news. `None` where that read failed.
    seen: Option<Failed>,
    /// When the next read is due, on the clock that does not step.
    next_ms: u64,
    /// Whether a read saw news; kept until the watch is set again.
    came: bool,
}

impl<Z: Zfs, D: Daemon, K: Keep, C: Clock> Agent<Z, D, K, C> {
    /// What the volume's failure note of this boot says ([`run`'s
    /// docs](crate::program)). It touches no pool. An error is one of reading the
    /// note or the boot's id.
    pub fn failure(&mut self) -> Result<Failed, Error> {
        let noted = self
            .daemon
            .failure()
            .map_err(|e| Error::Daemon(format!("its failure note: {e}")))?;
        Ok(match noted {
            None => Failed::No,
            Some(Noted::NotWhole) => Failed::Lost(Note::Torn),
            Some(Noted::Failed(failure)) if failure.held == Held::No => Failed::Unheld(failure),
            Some(Noted::Failed(failure)) => Failed::Lost(Note::Whole(failure)),
        })
    }

    /// Watch the failure note while the engine works, from now until
    /// it is set again (this module's docs, "While the engine works"):
    /// [`Agent::sync_past`] and [`Agent::settle_while`] stop where
    /// they ask whether to go on and [`Agent::note_came`] says yes.
    /// `seen` is what the caller read last: only a note that makes the
    /// volume lost ([`Failed::Lost`]) and differs from it is news. The
    /// first read is [`NOTE_EVERY`] from now. For `skyzfs-agent run`;
    /// with no watch set, the engine reads no note.
    pub fn watch_note(&mut self, seen: Option<Failed>) {
        let next_ms = self.clock.mono_ms().saturating_add(every_ms());
        self.watch = Some(Watch {
            seen,
            next_ms,
            came: false,
        });
    }

    /// Whether the watch set by [`Agent::watch_note`] has seen news:
    /// the note is read where a read is due (at most once each
    /// [`NOTE_EVERY`]), and a yes stays until the watch is set again.
    /// A read that fails is not news: the next round's own read reports
    /// it. No with no watch set.
    pub fn note_came(&mut self) -> bool {
        let now = self.clock.mono_ms();
        let due = match &self.watch {
            None => return false,
            Some(watch) if watch.came => return true,
            Some(watch) => now >= watch.next_ms,
        };
        if !due {
            return false;
        }
        let read = self.failure();
        let Some(watch) = self.watch.as_mut() else {
            return false;
        };
        watch.next_ms = now.saturating_add(every_ms());
        if let Ok(now_failed @ Failed::Lost(_)) = read {
            watch.came = watch.seen != Some(now_failed);
        }
        watch.came
    }

    /// Make the agent's pool read, so that it suspends ([`run`'s
    /// docs](crate::program)): for a volume [`Failed::Lost`]. At most the reopen's
    /// limit and [`LOOK_MAX`], and the reads of the state in between;
    /// an end asked for meanwhile does not cut it. An error is one of
    /// reading the pool's state before the reopen; one of a look after
    /// it ends the look, and is in the step (`unread`), with the
    /// reopen's outcome.
    pub fn lose(&mut self) -> Result<Step, Error> {
        self.step_lost(true)
    }

    /// [`Agent::lose`] with the reopen held back: the pool's state is
    /// read once, and a pool that reads neither SUSPENDED nor gone is
    /// left as it is ([`Step::Held`]). For the rounds that `run`'s cap
    /// on reopens ([`REOPEN_CAP`]) does not reopen in: a pool that zed
    /// suspended meanwhile is still found lost.
    pub fn look_lost(&mut self) -> Result<Step, Error> {
        self.step_lost(false)
    }

    /// The step, with the reopen where `reopen` says.
    fn step_lost(&mut self, reopen: bool) -> Result<Step, Error> {
        match self.zfs.pool_state(&self.pool)? {
            None => return Ok(Step::NotImported),
            Some(state) if state.suspended() => return Ok(Step::Suspended),
            Some(state) if !reopen => return Ok(Step::Held { state }),
            Some(_) => {}
        }
        let error = self.zfs.reopen(&self.pool).err().map(|e| e.to_string());
        let from = self.clock.mono_ms();
        let most = u64::try_from(LOOK_MAX.as_millis()).unwrap_or(u64::MAX);
        loop {
            let looked = self.zfs.pool_state(&self.pool);
            let looked_ms = self.clock.mono_ms().saturating_sub(from);
            let state = match looked {
                Ok(state) => state,
                Err(e) => {
                    return Ok(Step::Reopened {
                        error,
                        state: None,
                        unread: Some(e.to_string()),
                        looked_ms,
                    })
                }
            };
            let done = state.as_ref().is_none_or(PoolState::suspended);
            if done || looked_ms >= most {
                return Ok(Step::Reopened {
                    error,
                    state,
                    unread: None,
                    looked_ms,
                });
            }
            self.clock.sleep(LOOK_EVERY);
        }
    }
}

/// [`NOTE_EVERY`] in milliseconds.
fn every_ms() -> u64 {
    u64::try_from(NOTE_EVERY.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
