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
//! The step acts on the agent's own pool (`--pool`), whatever its
//! devices are now: the device under it is gone, so no premise can be
//! checked, and the pool is the one this volume's agent was set up for.
//! That is this module's reading, the conservative one: a pool that is
//! not on the volume is made to read once, which suspends nothing that
//! has its devices.
//!
//! What a round of `run` does with each, and the exit codes, are the
//! program's (`crate::program`, "`run`").

use std::time::Duration;

use skyzfs_control::failure::{Failure, Held};

use super::{Agent, Clock, Daemon, Error, Keep, Noted};
use crate::zfs::{PoolState, Zfs};

/// How often [`Agent::lose`] looks at the pool's state after the reopen.
pub const LOOK_EVERY: Duration = Duration::from_millis(250);

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
    /// The pool was reopened, and looked at until it read SUSPENDED or
    /// [`LOOK_MAX`] passed.
    Reopened {
        /// The reopen's error, in words: not the signal ([module
        /// docs](self)).
        error: Option<String>,
        /// The state at the last look; `None` where the pool was no
        /// longer imported.
        state: Option<PoolState>,
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
            Step::Reopened { .. } => "reopened",
        }
    }

    /// Whether the pool reads SUSPENDED after the step.
    pub fn suspended(&self) -> bool {
        match self {
            Step::NotImported => false,
            Step::Suspended => true,
            Step::Reopened { state, .. } => state.as_ref().is_some_and(PoolState::suspended),
        }
    }
}

impl<Z: Zfs, D: Daemon, K: Keep, C: Clock> Agent<Z, D, K, C> {
    /// What the volume's failure note of this boot says ([module
    /// docs](self)). It touches no pool. An error is one of reading the
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

    /// Make the agent's pool read, so that it suspends ([module
    /// docs](self)): for a volume [`Failed::Lost`]. At most the reopen's
    /// limit and [`LOOK_MAX`], and the reads of the state in between;
    /// an end asked for meanwhile does not cut it. An error is one of
    /// reading the pool's state before the reopen; one of a look after
    /// it ends the look, and is that error.
    pub fn lose(&mut self) -> Result<Step, Error> {
        match self.zfs.pool_state(&self.pool)? {
            None => return Ok(Step::NotImported),
            Some(state) if state.suspended() => return Ok(Step::Suspended),
            Some(_) => {}
        }
        let error = self.zfs.reopen(&self.pool).err().map(|e| e.to_string());
        let from = self.clock.mono_ms();
        let most = u64::try_from(LOOK_MAX.as_millis()).unwrap_or(u64::MAX);
        loop {
            let state = self.zfs.pool_state(&self.pool)?;
            let looked_ms = self.clock.mono_ms().saturating_sub(from);
            let done = state.as_ref().is_none_or(PoolState::suspended);
            if done || looked_ms >= most {
                return Ok(Step::Reopened {
                    error,
                    state,
                    looked_ms,
                });
            }
            self.clock.sleep(LOOK_EVERY);
        }
    }
}

#[cfg(test)]
mod tests;
