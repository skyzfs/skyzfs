//! The SkyZFS uploader (design §5.2 and §5.3; work package M2.4b-2): the
//! cuts the in-place store holds, committed one at a time into the
//! volume's object storage through the engine.
//!
//! A cut is uploaded once its flush has completed: it is then the oldest
//! held and marked as being uploaded (`skyzfs-local`'s cut state). Before
//! each cut the uploader reads head's row in the refs object
//! ([`row::ROW_KEY`]) and checks it against what the store assumes is
//! there ([`check::judge`]):
//!
//! 1. the row names the cut being uploaded: its commit had landed and the
//!    daemon died before it let the cut go. The cut is let go and nothing
//!    is uploaded;
//! 2. a cut that holds part of the volume is uploaded onto the cut the
//!    intent map names as committed, when the row names that cut and
//!    head's commit is the row's (its group is the row's);
//! 3. a cut that holds the whole volume needs no base: it is uploaded
//!    when head's commit is the row's, whatever cut the row names, or,
//!    with no row, when head's commit has no group;
//! 4. anything else is a mismatch: every held cut is abandoned, and the
//!    next cut holds the whole volume.
//!
//! The commit swaps head and the row in one transaction of the refs
//! object, the row's swap expecting the row as the check read it, or its
//! absence, so a row that moved since fails the commit and is not
//! overwritten. The uploader never moves head but onto a commit of its
//! own made on the head it checked.
//!
//! The driver ([`driver::Driver`]) is a state machine stepped by its
//! caller, a bounded piece of work a step: the daemon's uploader thread
//! (M2.4c) between its other duties. It reaches the store through
//! [`local::Local`] and object storage through [`remote::Remote`], which
//! the tests give a store that dies at any operation.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod backoff;
pub mod check;
pub mod driver;
pub mod lease;
pub mod local;
pub mod publish;
pub mod queued;
pub mod remote;
pub mod row;
pub mod uploader;

pub use backoff::Backoff;
pub use check::{judge, Mismatch, Rule, Verdict};
pub use driver::{Checked, Doing, Driver, Figures, Step, SLICE_BLOCKS};
pub use lease::{Keeper, LeaseState};
pub use local::{CutsNow, Local, Released, Store, Uploadable};
pub use publish::{Answer, Known, Progress, PublishFigures, Publishing, Refusal, Request};
pub use queued::{Desk, Queued};
pub use remote::{EngineRemote, Observed, Remote, RemoteError};
pub use row::{Named, Row, RowError, ROW_KEY, ROW_VERSION};
pub use uploader::{Control, PublishesShown, Shown, State, Uploader};
