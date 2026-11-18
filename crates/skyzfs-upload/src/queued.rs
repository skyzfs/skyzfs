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
//! Nor does a turn wait on the disk for the uploader. A read of a slice
//! is made on the queue's thread (the shape decided for M2.4), but the
//! sync of the copies file is not: the turn that takes the call begins
//! it on the file's helper ([`Store::begin_sync_copies`]) and keeps the
//! reply, and the turn that finds its end answers
//! ([`Store::copies_synced`]); the helper wakes the queue's thread when
//! the sync ends, as for the image's syncs (F1 of #117's review).
//!
//! When the queue's thread stops serving, the desk is closed
//! ([`Desk::close`]): a call waiting is dropped, a sync of the copies
//! under way is not answered, and every call after is answered as by a
//! store that holds nothing ([`Queued`]'s doc): the uploader idles and
//! lands nothing more, until the daemon stops its thread, which it does
//! when the queue stops. The daemon closes it at the queue's last turn,
//! and from the main thread when the uploader's thread is ended: a queue
//! thread that panics has no last turn, and the main thread's close is
//! the one that comes (#118's review, 2.5). A call
//! that found the desk open can still wake the queue's thread once after
//! the close (it leaves the call, the desk closes, it wakes), so the wake
//! must be harmless once the queue has ended: the daemon's is an event
//! descriptor that outlives the queue's thread.

use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use skyzfs_blk::Buf;
use skyzfs_local::inplace::CutSlice;
use tokio::sync::oneshot;

use crate::local::{not_ended, Local, Released, Store, Uploadable};

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

/// Answer `call` from `store` at once. A sync of the copies is begun
/// and waited for: [`Desk::answer`] takes it in two turns instead.
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
        Call::SyncCopies(number) => {
            Answer::SyncCopies(store.begin_sync_copies(number).and_then(|()| {
                store
                    .copies_synced(true)
                    .unwrap_or_else(|| Err(not_ended()))
            }))
        }
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
    /// The reply to a sync of the copies that an earlier turn began,
    /// sent by the turn that finds its end.
    syncing: Option<oneshot::Sender<Answer>>,
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
    /// there is one, from `store`, without waiting on the disk. A sync of
    /// the copies under way is answered first, once it has ended; until
    /// then a call waiting stays where it is. A sync of the copies asked
    /// for is begun, and answered here at once only if it has ended
    /// already. Whether anything was done: a call taken, or a sync's end
    /// answered.
    pub fn answer(&self, store: &mut impl Store) -> bool {
        let mut done = false;
        let syncing = self.slot().syncing.take();
        if let Some(reply) = syncing {
            let Some(synced) = store.copies_synced(false) else {
                self.park(reply);
                return false;
            };
            self.reply(reply, Answer::SyncCopies(synced));
            done = true;
        }
        let Some((call, reply)) = self.slot().call.take() else {
            return done;
        };
        match call {
            Call::SyncCopies(number) => {
                match store
                    .begin_sync_copies(number)
                    .map(|()| store.copies_synced(false))
                {
                    Err(e) => self.reply(reply, Answer::SyncCopies(Err(e))),
                    Ok(Some(synced)) => self.reply(reply, Answer::SyncCopies(synced)),
                    Ok(None) => self.park(reply),
                }
            }
            call => self.reply(reply, answer_from(store, call)),
        }
        true
    }

    /// Keep the reply to a sync of the copies under way, unless the desk
    /// closed meanwhile.
    fn park(&self, reply: oneshot::Sender<Answer>) {
        let mut slot = self.slot();
        if !slot.closed {
            slot.syncing = Some(reply);
        }
    }

    /// Send `answer`. The uploader may have stopped waiting; its answer
    /// is then not wanted.
    fn reply(&self, reply: oneshot::Sender<Answer>, answer: Answer) {
        self.slot().answered += 1;
        let _ = reply.send(answer);
    }

    /// The queue's thread serves no more: drop the call waiting and the
    /// reply to a sync under way, and answer every call after as a store
    /// that holds nothing would.
    pub fn close(&self) {
        let mut slot = self.slot();
        slot.closed = true;
        slot.call = None;
        slot.syncing = None;
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
            // One caller, which waits for each answer, so the slot is
            // empty, unless a turn of the uploader was dropped while its
            // call waited: that call is not wanted, and this one takes
            // its place.
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
