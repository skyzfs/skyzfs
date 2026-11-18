//! The store as the uploader's thread reaches it in the daemon (M2.4c):
//! one call at a time, answered on the queue's thread between its turns.
//!
//! The in-place store lives on the queue's thread and is never shared.
//! The uploader puts a call on a [`Desk`] and wakes that thread; after
//! its next turn the thread answers the call from the store
//! ([`Desk::answer`]) and the uploader's future resolves. A call costs a
//! round trip and one turn's wait, and a turn never waits on the
//! uploader: the thread answers what is there and goes on.
//!
//! When the queue's thread stops serving, the desk is closed
//! ([`Desk::close`]): a call waiting is dropped, and every call after is
//! answered as by a store that holds nothing ([`Queued`]'s doc): the
//! uploader idles and lands nothing more, until the daemon stops its
//! thread, which it does when the queue stops.

use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use skyzfs_blk::Buf;
use skyzfs_local::inplace::CutSlice;
use tokio::sync::oneshot;

use crate::local::{Local, Released, Store, Uploadable};

/// A call of [`Local`], as it travels.
#[derive(Debug)]
pub enum Call {
    /// [`Local::uploadable`].
    Uploadable,
    /// [`Local::read`].
    Read {
        /// The cut.
        number: u64,
        /// The first block of its map to read.
        from: u64,
        /// The buffer to read into.
        buf: Buf,
    },
    /// [`Local::sync_copies`].
    SyncCopies(u64),
    /// [`Local::release`].
    Release(u64),
    /// [`Local::name_committed`].
    NameCommitted {
        /// The cut.
        number: u64,
        /// Its group.
        group: u64,
    },
    /// [`Local::abandon_all`].
    AbandonAll,
}

/// The answer to a [`Call`].
#[derive(Debug)]
pub enum Answer {
    /// To [`Call::Uploadable`].
    Uploadable(Option<Uploadable>),
    /// To [`Call::Read`]: the buffer back, and the slice.
    Read(Buf, io::Result<CutSlice>),
    /// To [`Call::SyncCopies`].
    SyncCopies(io::Result<()>),
    /// To [`Call::Release`].
    Release(Released),
    /// To [`Call::NameCommitted`].
    NameCommitted(bool),
    /// To [`Call::AbandonAll`].
    AbandonAll(Vec<u64>),
}

/// Answer `call` from `store`.
pub fn answer_from(store: &mut impl Store, call: Call) -> Answer {
    match call {
        Call::Uploadable => Answer::Uploadable(store.uploadable()),
        Call::Read {
            number,
            from,
            mut buf,
        } => {
            let read = store.read(number, from, &mut buf);
            Answer::Read(buf, read)
        }
        Call::SyncCopies(number) => Answer::SyncCopies(store.sync_copies(number)),
        Call::Release(number) => Answer::Release(store.release(number)),
        Call::NameCommitted { number, group } => {
            Answer::NameCommitted(store.name_committed(number, group))
        }
        Call::AbandonAll => Answer::AbandonAll(store.abandon_all()),
    }
}

/// Where the uploader leaves its call and the queue's thread answers it.
pub struct Desk {
    slot: Mutex<Slot>,
    /// Gives the queue's thread a turn.
    wake: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Slot {
    call: Option<(Call, oneshot::Sender<Answer>)>,
    closed: bool,
    /// The calls answered, for the status and the tests.
    answered: u64,
}

impl Desk {
    /// A desk whose calls `wake` the queue's thread.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            slot: Mutex::new(Slot::default()),
            wake: Box::new(wake),
        })
    }

    /// The slot. A panic while it was held left nothing half done in it
    /// (each holder only moves a call in or out), so a poisoned lock is
    /// taken as it is.
    fn slot(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// On the queue's thread, after a turn: answer the call waiting, if
    /// there is one, from `store`. Whether one was answered.
    pub fn answer(&self, store: &mut impl Store) -> bool {
        let Some((call, reply)) = self.slot().call.take() else {
            return false;
        };
        let answer = answer_from(store, call);
        self.slot().answered += 1;
        // The uploader may have stopped waiting; its answer is then not
        // wanted.
        let _ = reply.send(answer);
        true
    }

    /// The queue's thread serves no more: drop the call waiting, and
    /// answer every call after as a store that holds nothing would.
    pub fn close(&self) {
        let mut slot = self.slot();
        slot.closed = true;
        slot.call = None;
    }

    /// Whether the desk is closed.
    pub fn closed(&self) -> bool {
        self.slot().closed
    }

    /// The calls answered so far.
    pub fn answered(&self) -> u64 {
        self.slot().answered
    }

    /// Leave `call`, wake the queue's thread and wait for the answer.
    /// `None` where the desk is closed, or closes before the answer.
    async fn ask(&self, call: Call) -> Option<Answer> {
        let (reply, answer) = oneshot::channel();
        {
            let mut slot = self.slot();
            if slot.closed {
                return None;
            }
            // One caller, which waits for each answer: the slot is empty.
            slot.call = Some((call, reply));
        }
        (self.wake)();
        answer.await.ok()
    }
}

/// [`Local`] over a [`Desk`]: the uploader's side.
///
/// Once the desk is closed each call answers as a store that holds
/// nothing: no cut to upload, a read or a sync that fails, a release that
/// finds the cut gone, a naming that does not name, and no cut abandoned.
/// None of these lands anything in object storage: the uploader idles
/// from its next turn.
pub struct Queued {
    desk: Arc<Desk>,
}

impl Queued {
    /// The uploader's side of `desk`.
    pub fn new(desk: Arc<Desk>) -> Self {
        Self { desk }
    }

    /// The desk.
    pub fn desk(&self) -> &Arc<Desk> {
        &self.desk
    }
}

fn gone() -> io::Error {
    io::Error::other("the store is no longer served")
}

impl Local for Queued {
    async fn uploadable(&mut self) -> Option<Uploadable> {
        match self.desk.ask(Call::Uploadable).await {
            Some(Answer::Uploadable(cut)) => cut,
            _ => None,
        }
    }

    async fn read(&mut self, number: u64, from: u64, buf: Buf) -> (Buf, io::Result<CutSlice>) {
        match self.desk.ask(Call::Read { number, from, buf }).await {
            Some(Answer::Read(buf, read)) => (buf, read),
            _ => (Buf::zeroed(0), Err(gone())),
        }
    }

    async fn sync_copies(&mut self, number: u64) -> io::Result<()> {
        match self.desk.ask(Call::SyncCopies(number)).await {
            Some(Answer::SyncCopies(synced)) => synced,
            _ => Err(gone()),
        }
    }

    async fn release(&mut self, number: u64) -> Released {
        match self.desk.ask(Call::Release(number)).await {
            Some(Answer::Release(released)) => released,
            _ => Released::Gone,
        }
    }

    async fn name_committed(&mut self, number: u64, group: u64) -> bool {
        match self.desk.ask(Call::NameCommitted { number, group }).await {
            Some(Answer::NameCommitted(named)) => named,
            _ => false,
        }
    }

    async fn abandon_all(&mut self) -> Vec<u64> {
        match self.desk.ask(Call::AbandonAll).await {
            Some(Answer::AbandonAll(abandoned)) => abandoned,
            _ => Vec::new(),
        }
    }
}
