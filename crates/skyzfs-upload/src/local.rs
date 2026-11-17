//! What the uploader asks of the in-place store: the cut being uploaded,
//! its blocks in slices, the copies synced before its commit, its
//! release, and the step of a mismatch.
//!
//! The store lives on the queue's thread and the uploader off it. The
//! daemon (M2.4c) answers these calls on the queue's thread between its
//! turns, by a channel, so each is one round trip there and back; the
//! tests and a store with no queue answer them at once ([`Local`] for
//! [`InPlace`]).

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
    /// (`InPlace::release_cut`). Whether it was the cut being uploaded.
    async fn release(&mut self, number: u64) -> bool;

    /// A mismatch: abandon every held cut, so that the next holds the
    /// whole volume (`InPlace::abandon_all_cuts`). The cuts abandoned.
    async fn abandon_all(&mut self) -> Vec<u64>;
}

impl<D: Dir> Local for InPlace<D> {
    async fn uploadable(&mut self) -> Option<Uploadable> {
        let first = *self.held_cuts().first()?;
        if first.phase != CutPhase::Uploading {
            return None;
        }
        Some(Uploadable {
            state: self.cut_state_id()?,
            number: first.number,
            group: first.state.txg,
            whole: self.cut_is_whole(first.number),
            base: self.committed_cut(),
        })
    }

    async fn read(&mut self, number: u64, from: u64, mut buf: Buf) -> (Buf, io::Result<CutSlice>) {
        let read = self.read_cut_from(number, from, &mut buf);
        (buf, read)
    }

    async fn sync_copies(&mut self, number: u64) -> io::Result<()> {
        self.sync_cut_copies(number)?;
        self.cut_copies_synced(true)
            .unwrap_or_else(|| Err(io::Error::other("the sync of the copies did not end")))
    }

    async fn release(&mut self, number: u64) -> bool {
        self.release_cut(number)
    }

    async fn abandon_all(&mut self) -> Vec<u64> {
        self.abandon_all_cuts()
    }
}
