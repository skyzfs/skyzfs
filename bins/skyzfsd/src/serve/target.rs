//! The store that serves the volume: the log store behind its worker
//! (the default), or the in-place store (`--store in-place`). The pump,
//! the status and the test hooks see one type.

use std::time::{Duration, Instant};

use skyzfs_blk::{BlockTarget, Completion, Geometry, Request, RequestId, SubmitError};
use skyzfs_local::fs::FsDir;
use skyzfs_local::inplace::InPlace;
use skyzfs_local::store::Served;
use skyzfs_ublk::pump::Positions;

/// The store that serves the volume.
pub enum Target {
    /// The log store, behind its worker thread.
    Log(Served),
    /// The in-place store, on the queue's thread.
    InPlace(Box<InPlace<FsDir>>),
}

impl Target {
    /// Fail the volume now: the test build's `--fail-log-after` and
    /// `SIGUSR1`.
    #[cfg(feature = "test-hooks")]
    pub fn fail_now(&mut self) {
        match self {
            Target::Log(served) => served.fail_now(),
            Target::InPlace(store) => store.fail_now(),
        }
    }
}

impl BlockTarget for Target {
    fn geometry(&self) -> Geometry {
        match self {
            Target::Log(served) => served.geometry(),
            Target::InPlace(store) => store.geometry(),
        }
    }

    fn submit(&mut self, request: Request) -> Result<(), SubmitError> {
        match self {
            Target::Log(served) => served.submit(request),
            Target::InPlace(store) => store.submit(request),
        }
    }

    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        match self {
            Target::Log(served) => served.poll(out),
            Target::InPlace(store) => store.poll(out),
        }
    }

    fn wait(&mut self, out: &mut Vec<Completion>, timeout: Duration) -> usize {
        match self {
            Target::Log(served) => served.wait(out, timeout),
            Target::InPlace(store) => store.wait(out, timeout),
        }
    }
}

impl Positions for Target {
    /// The in-place store has no log, so no position: the pump then
    /// commits without storing one, and the record of what was
    /// acknowledged stays as the last log store left it. A later start
    /// counts from that record only the log's records, of which the
    /// in-place store writes none; what the kernel issues again after a
    /// lost process is applied again, which rule R9 allows.
    fn acknowledged(&self) -> Option<u64> {
        match self {
            Target::Log(served) => Positions::acknowledged(served),
            Target::InPlace(_) => None,
        }
    }

    /// The in-place store has no thread of its own that could end.
    fn gone(&self) -> bool {
        match self {
            Target::Log(served) => Positions::gone(served),
            Target::InPlace(_) => false,
        }
    }

    fn ready_at(&self, id: RequestId) -> Option<Instant> {
        match self {
            Target::Log(served) => Positions::ready_at(served, id),
            Target::InPlace(_) => None,
        }
    }
}
