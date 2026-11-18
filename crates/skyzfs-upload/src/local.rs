//! What the uploader asks of the in-place store: the cut being uploaded,
//! its blocks in slices, the copies synced before its commit, its
//! release (or its naming, where it went after its commit), and the step
//! of a mismatch.
//!
//! The store lives on the queue's thread and the uploader off it. The
//! daemon (M2.4c) answers these calls on the queue's thread between its
//! turns ([`crate::queued`]), so each is one round trip there and back;
//! the tests and a store with no queue answer them at once ([`Store`],
//! which gives [`Local`]).

use std::io;

use skyzfs_blk::Buf;
use skyzfs_local::inplace::{CommittedCut, CutPhase, CutSlice, InPlace};
use skyzfs_local::io::Dir;

/// The cut being uploaded, as the store holds it, with what the check
/// needs to know of the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uploadable {
    /// The cut state's name (`CutState::id`).
    pub state: [u8; 16],
    /// The cut's number in that state.
    pub number: u64,
    /// The cut's transaction group.
    pub group: u64,
    /// Whether the cut holds the whole volume, so that it needs no base.
    pub whole: bool,
    /// Whether it opens as itself (design §5.2): only such a cut is
    /// published (M2.5).
    pub itself: bool,
    /// The cut the intent map names as committed: what the held cuts
    /// were made against, the uploader's assumed base. `None` where the
    /// map names none.
    pub base: Option<CommittedCut>,
}

impl Uploadable {
    /// The cut as its row names it.
    pub fn row(&self) -> CommittedCut {
        CommittedCut {
            state: self.state,
            number: self.number,
            group: self.group,
        }
    }
}

/// What [`Local::release`] did with a cut committed in object storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Released {
    /// The cut is let go, and the intent map names it as committed.
    LetGo,
    /// The cut was the one being uploaded, but the intent map's pages
    /// could not be written: the volume has failed, the map may name the
    /// cut from now on, and the cut stays held, the one being uploaded
    /// still (F2 of the review of M2.4b-1). The store will offer it
    /// again, and only the cut being uploaded can be let go, so no later
    /// cut can leave from this instance.
    Kept,
    /// It was not the cut being uploaded, and nothing changed: the bound,
    /// or a failed read or sync of the copies, took it after its commit
    /// had landed.
    Gone,
}

/// The store, as the uploader reaches it. Each call is answered where the
/// store lives; none waits on object storage.
// The uploader runs on one thread's runtime (M2.4c), so its futures need
// not be `Send`, and the trait is the crate's own: no bound is wanted.
#[allow(async_fn_in_trait)]
pub trait Local {
    /// The cut being uploaded: the oldest held, once its flush has
    /// completed. `None` while there is none.
    async fn uploadable(&mut self) -> Option<Uploadable>;

    /// Read held cut `number`'s blocks from block `from` on into `buf`,
    /// as many as it holds (`InPlace::read_cut_from`), and hand `buf`
    /// back. A read that fails abandons the cut if it is the one being
    /// uploaded; a cut that has gone is refused.
    async fn read(&mut self, number: u64, from: u64, buf: Buf) -> (Buf, io::Result<CutSlice>);

    /// Sync the copies file before cut `number` is committed, where the
    /// upload read a copy for it: an error where the cut is not the one
    /// being uploaded, and where the sync failed, which abandons it.
    async fn sync_copies(&mut self, number: u64) -> io::Result<()>;

    /// Cut `number` is committed in object storage: let it go
    /// (`InPlace::release_cut`), and say whether it went ([`Released`]).
    async fn release(&mut self, number: u64) -> Released;

    /// Cut `number`, of transaction group `group`, is committed in object
    /// storage, but [`Local::release`] found it gone: the bound took it
    /// after its commit had landed. Name it in the intent map as the
    /// committed cut (`InPlace::name_committed_cut`), so that the next
    /// check finds head's row to be the base and the next cut goes up as
    /// a delta onto it, not whole, unless that cut holds the whole volume
    /// itself (it took the map of a cut that did). Whether the map names
    /// it now.
    async fn name_committed(&mut self, number: u64, group: u64) -> bool;

    /// A mismatch: abandon every held cut, so that the next holds the
    /// whole volume (`InPlace::abandon_all_cuts`). The cuts abandoned.
    async fn abandon_all(&mut self) -> Vec<u64>;
}

/// [`Local`] answered where the store lives: the store's side of each
/// call. The daemon answers the uploader's calls with it on the queue's
/// thread (`queued`); the tests and a store with no queue use it as
/// [`Local`] directly. Every call is answered at once but the sync of the
/// copies, which comes in two halves, so that the queue's thread begins
/// it at one turn and takes its end at a later one, and does not wait
/// for the disk meanwhile (F1 of #117's review).
pub trait Store {
    /// [`Local::uploadable`].
    fn uploadable(&mut self) -> Option<Uploadable>;
    /// [`Local::read`], into `buf`.
    fn read(&mut self, number: u64, from: u64, buf: &mut Buf) -> io::Result<CutSlice>;
    /// [`Local::sync_copies`], its first half: begin the sync for cut
    /// `number`. An error is the sync's outcome, and nothing was begun.
    fn begin_sync_copies(&mut self, number: u64) -> io::Result<()>;
    /// Its second half: the outcome of the sync begun, once it has ended.
    /// `None` while it is under way, which only `wait` false answers, and
    /// where none was begun. With `wait` the caller waits for its end.
    fn copies_synced(&mut self, wait: bool) -> Option<io::Result<()>>;
    /// [`Local::release`].
    fn release(&mut self, number: u64) -> Released;
    /// [`Local::name_committed`].
    fn name_committed(&mut self, number: u64, group: u64) -> bool;
    /// [`Local::abandon_all`].
    fn abandon_all(&mut self) -> Vec<u64>;
}

/// A sync waited for that found none under way.
pub(crate) fn not_ended() -> io::Error {
    io::Error::other("the sync of the copies did not end")
}

impl<S: Store> Local for S {
    async fn uploadable(&mut self) -> Option<Uploadable> {
        Store::uploadable(self)
    }

    async fn read(&mut self, number: u64, from: u64, mut buf: Buf) -> (Buf, io::Result<CutSlice>) {
        let read = Store::read(self, number, from, &mut buf);
        (buf, read)
    }

    async fn sync_copies(&mut self, number: u64) -> io::Result<()> {
        Store::begin_sync_copies(self, number)?;
        Store::copies_synced(self, true).unwrap_or_else(|| Err(not_ended()))
    }

    async fn release(&mut self, number: u64) -> Released {
        Store::release(self, number)
    }

    async fn name_committed(&mut self, number: u64, group: u64) -> bool {
        Store::name_committed(self, number, group)
    }

    async fn abandon_all(&mut self) -> Vec<u64> {
        Store::abandon_all(self)
    }
}

impl<D: Dir> Store for InPlace<D> {
    fn uploadable(&mut self) -> Option<Uploadable> {
        let first = *self.held_cuts().first()?;
        if first.phase != CutPhase::Uploading {
            return None;
        }
        Some(Uploadable {
            state: self.cut_state_id()?,
            number: first.number,
            group: first.state.txg,
            whole: self.cut_is_whole(first.number),
            itself: first.itself,
            base: self.committed_cut(),
        })
    }

    fn read(&mut self, number: u64, from: u64, buf: &mut Buf) -> io::Result<CutSlice> {
        self.read_cut_from(number, from, buf)
    }

    fn begin_sync_copies(&mut self, number: u64) -> io::Result<()> {
        self.sync_cut_copies(number)
    }

    fn copies_synced(&mut self, wait: bool) -> Option<io::Result<()>> {
        self.cut_copies_synced(wait)
    }

    fn release(&mut self, number: u64) -> Released {
        if !self.release_cut(number) {
            return Released::Gone;
        }
        // True from `release_cut` says that it was the cut being
        // uploaded, not that it went: a failed write of the map keeps it.
        if self.held_cuts().iter().any(|cut| cut.number == number) {
            Released::Kept
        } else {
            Released::LetGo
        }
    }

    fn name_committed(&mut self, number: u64, group: u64) -> bool {
        self.name_committed_cut(number, group)
    }

    fn abandon_all(&mut self) -> Vec<u64> {
        self.abandon_all_cuts()
    }
}
