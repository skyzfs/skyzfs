//! The store reached through a desk (M2.4c): the store on a thread of its
//! own, as the daemon's queue holds it, answering the uploader's calls
//! between its turns; and a desk closed under the uploader.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use skynbd_refs::ObjectRefStore;
use skyzfs_blk::Buf;
use skyzfs_local::inplace::{Cut, CutSlice, InPlace};
use skyzfs_local::sim::SimDisk;
use skyzfs_upload::lease::Keeper;
use skyzfs_upload::uploader::{State, Uploader};
use skyzfs_upload::{CutsNow, Desk, Local, Queued, Released, Store, Uploadable};

use common::*;

/// The copies file's name in the volume's directory.
const COPIES: &str = "copies.img";

/// The queue's thread: after each wake (a turn), answer the call
/// waiting. It ends when the desk is closed and gives the store back.
fn queue_thread(
    desk: Arc<Desk>,
    mut store: InPlace<SimDisk>,
    woken: std::sync::mpsc::Receiver<()>,
) -> std::thread::JoinHandle<InPlace<SimDisk>> {
    std::thread::spawn(move || loop {
        // A turn comes on a wake or after a while, as the queue's do.
        let _ = woken.recv_timeout(Duration::from_millis(5));
        if desk.closed() {
            return store;
        }
        desk.answer(&mut store);
    })
}

/// The cut of group 7 goes up with every call answered on the store's own
/// thread; once the desk is closed nothing more is asked of the store.
#[tokio::test]
async fn a_cut_goes_up_through_the_desk_answered_on_the_stores_thread() {
    let mut fx = Fixture::new(51).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.write(&mut store, 70, 0x30);
    fx.group(&mut store, 7);

    let (wake, woken) = std::sync::mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = queue_thread(Arc::clone(&desk), store, woken);

    let refs = Arc::new(ObjectRefStore::new(fx.store.clone()));
    let lease = Keeper::new(refs, fx.vol, "node-a", 30_000);
    let mut remote = Some(fx.remote());
    let make = move |_epoch| remote.take().expect("made once");
    let mut up = Uploader::with_slices(Queued::new(Arc::clone(&desk)), lease, make, 16);

    let mut states = Vec::new();
    for _ in 0..200 {
        let wait = up.turn().await;
        let state = up.shown().state;
        states.push(state.clone());
        if state == State::Idle {
            break;
        }
        assert_eq!(wait, Duration::ZERO, "no wait while working: {states:?}");
    }
    assert_eq!(up.shown().state, State::Idle, "{states:?}");
    fx.assert_uploaded(7, "through the desk").await;
    assert_eq!(up.shown().figures.uploaded, 1);
    let answered = desk.answered();
    assert!(answered > 3, "each call went through the desk: {answered}");

    desk.close();
    let store = queue.join().expect("the queue's thread");
    assert!(store.held_cuts().is_empty(), "the cut was let go");

    // The desk closed: the uploader idles, and the store is asked nothing.
    up.turn().await;
    assert_eq!(up.shown().state, State::Idle);
    assert_eq!(desk.answered(), answered);
}

/// The queue's thread, counting its turns and timing the longest answer.
fn timed_queue_thread(
    desk: Arc<Desk>,
    mut store: InPlace<SimDisk>,
    woken: std::sync::mpsc::Receiver<()>,
    turns: Arc<AtomicU64>,
    longest: Arc<Mutex<Duration>>,
) -> std::thread::JoinHandle<InPlace<SimDisk>> {
    std::thread::spawn(move || loop {
        let _ = woken.recv_timeout(Duration::from_millis(5));
        if desk.closed() {
            return store;
        }
        let began = Instant::now();
        desk.answer(&mut store);
        let took = began.elapsed();
        let mut longest = longest.lock().expect("the longest");
        *longest = (*longest).max(took);
        drop(longest);
        turns.fetch_add(1, Ordering::SeqCst);
    })
}

/// A sync of the copies through the desk is begun at one turn and
/// answered at a later one: while the disk holds it, the queue's thread
/// goes on turning, no answer waits for it, and nobody waits at the
/// sync's end. Once it ends the cut goes up (F1 of #117's review). The
/// count of cuts is given meanwhile (M2.5-2).
///
/// Mutant: the count given only once the sync's end is answered.
#[tokio::test]
async fn a_sync_of_the_copies_through_the_desk_leaves_the_queue_turning() {
    const HELD: Duration = Duration::from_millis(400);
    let mut fx = Fixture::new(52).await;
    let mut store = fx.start();
    for block in 20..30 {
        fx.write(&mut store, block, block as u8);
    }
    fx.group(&mut store, 7);
    // Overwritten after the cut's instant: the cut reads them from copies.
    for block in 20..30 {
        fx.write(&mut store, block, 0xEE);
    }
    let disk = fx.disk.clone();
    disk.hold_syncs_of(COPIES);
    let synced_before = disk.syncs_of(COPIES);

    let (wake, woken) = std::sync::mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let (turns, longest) = (Arc::new(AtomicU64::new(0)), Arc::default());
    let queue = timed_queue_thread(
        Arc::clone(&desk),
        store,
        woken,
        Arc::clone(&turns),
        Arc::clone(&longest),
    );
    let refs = Arc::new(ObjectRefStore::new(fx.store.clone()));
    let lease = Keeper::new(refs, fx.vol, "node-a", 30_000);
    let mut remote = Some(fx.remote());
    let make = move |_epoch| remote.take().expect("made once");
    let mut up = Uploader::with_slices(Queued::new(Arc::clone(&desk)), lease, make, 4);

    let uploading = async {
        for _ in 0..400 {
            up.turn().await;
            if up.shown().figures.uploaded == 1 {
                return;
            }
        }
        panic!("not uploaded: {:?}", up.shown());
    };
    let holding = async {
        while disk.syncs_of(COPIES) == synced_before {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let at_begin = turns.load(Ordering::SeqCst);
        let count = tokio::time::timeout(HELD / 2, desk.cuts_now()).await;
        assert!(
            matches!(count, Ok(Some(now)) if now.count == 1),
            "the count while the sync is held: {count:?}"
        );
        tokio::time::sleep(HELD).await;
        let during = turns.load(Ordering::SeqCst) - at_begin;
        assert_eq!(disk.held_waiters(), 0, "nobody waits at the held sync");
        disk.release_syncs();
        during
    };
    let ((), during) = tokio::join!(uploading, holding);
    assert!(
        during >= 20,
        "the queue's thread turned {during} times in {HELD:?} of the held sync"
    );
    let longest = *longest.lock().expect("the longest");
    assert!(
        longest < HELD / 4,
        "no answer waited for the sync: the longest took {longest:?}"
    );
    fx.assert_uploaded(7, "copies synced through the desk")
        .await;
    desk.close();
    let store = queue.join().expect("the queue's thread");
    assert!(store.held_cuts().is_empty(), "the cut was let go");
}

/// A sync of the copies that fails, through the desk, is answered as
/// failed at the turn that finds its end: the cut is lost, and nothing
/// of it goes up.
#[tokio::test]
async fn a_failed_sync_of_the_copies_through_the_desk_loses_the_cut() {
    let mut fx = Fixture::new(53).await;
    let mut store = fx.start();
    for block in 20..30 {
        fx.write(&mut store, block, block as u8);
    }
    fx.group(&mut store, 7);
    for block in 20..30 {
        fx.write(&mut store, block, 0xEE);
    }
    fx.disk.fail_sync_of(COPIES);
    let (wake, woken) = std::sync::mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = queue_thread(Arc::clone(&desk), store, woken);
    let refs = Arc::new(ObjectRefStore::new(fx.store.clone()));
    let lease = Keeper::new(refs, fx.vol, "node-a", 30_000);
    let mut remote = Some(fx.remote());
    let make = move |_epoch| remote.take().expect("made once");
    let mut up = Uploader::with_slices(Queued::new(Arc::clone(&desk)), lease, make, 4);
    for _ in 0..400 {
        up.turn().await;
        if up.shown().figures.lost == 1 {
            break;
        }
    }
    let figures = up.shown().figures;
    assert_eq!((figures.lost, figures.uploaded), (1, 0), "{figures:?}");
    desk.close();
    let store = queue.join().expect("the queue's thread");
    assert!(store.held_cuts().is_empty(), "the cut was abandoned");
    assert_eq!(fx.uploaded().await.group, None, "nothing of it went up");
}

/// A sync of the copies under way when the desk closes is not answered
/// by the store: the uploader's call ends at the close, as failed, and
/// the queue's thread ends with the sync still held.
#[tokio::test]
async fn a_sync_of_the_copies_under_way_at_the_close_ends_as_failed() {
    let mut fx = Fixture::new(54).await;
    let mut store = fx.start();
    fx.write(&mut store, 20, 0x20);
    fx.group(&mut store, 7);
    let number = store.held_cuts()[0].number;
    let disk = fx.disk.clone();
    disk.hold_syncs_of(COPIES);
    let synced_before = disk.syncs_of(COPIES);
    let (wake, woken) = std::sync::mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = queue_thread(Arc::clone(&desk), store, woken);
    let mut up = Queued::new(Arc::clone(&desk));
    let syncing = async { up.sync_copies(number).await };
    let closing = async {
        while disk.syncs_of(COPIES) == synced_before {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        desk.close();
    };
    let (synced, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(syncing, closing)
    })
    .await
    .expect("the call ends at the close");
    let e = synced.expect_err("not synced");
    assert!(e.to_string().contains("no longer served"), "{e}");
    let store = queue.join().expect("the queue's thread ends");
    disk.release_syncs();
    drop(store);
}

/// A call waiting when the desk closes is dropped and answered as by a
/// store that holds nothing; so is every call after, and none of them
/// wakes the queue's thread. An ask of the count waiting at the close
/// ends at once, with no count, and so does every ask after (N3 of the
/// next look at #127: not after its asker's limit).
#[tokio::test]
async fn a_closed_desk_answers_as_a_store_that_holds_nothing() {
    let (wake, mut woken) = tokio::sync::mpsc::unbounded_channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let mut waiting = Queued::new(Arc::clone(&desk));
    let call = tokio::spawn(async move { waiting.uploadable().await });
    // Each wake within a limit: a call or ask that does not wake the
    // queue's thread fails here, rather than hanging the suite.
    tokio::time::timeout(Duration::from_secs(10), woken.recv())
        .await
        .expect("the call wakes the queue's thread")
        .expect("the call woke the queue's thread");
    assert!(!call.is_finished(), "nobody has answered");
    let asking = Arc::clone(&desk);
    let count = tokio::spawn(async move { asking.cuts_now().await });
    tokio::time::timeout(Duration::from_secs(10), woken.recv())
        .await
        .expect("the ask of the count wakes the queue's thread")
        .expect("the ask of the count woke the queue's thread");
    assert!(!count.is_finished(), "nobody has given the count");

    desk.close();
    let answer = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .expect("the call waiting ends at the close")
        .expect("the task");
    assert!(answer.is_none(), "{answer:?}");
    let counted = tokio::time::timeout(Duration::from_secs(5), count)
        .await
        .expect("the ask of the count waiting ends at the close")
        .expect("the task");
    assert_eq!(counted, None, "no count: the store is no longer served");

    let mut after = Queued::new(Arc::clone(&desk));
    assert!(after.uploadable().await.is_none());
    let (buf, read) = after.read(1, 0, Buf::zeroed(4096)).await;
    assert!(read.is_err(), "a read fails");
    assert_eq!(buf.len(), 0);
    assert!(after.sync_copies(1).await.is_err(), "a sync fails");
    assert_eq!(after.release(1).await, Released::Gone);
    assert!(!after.name_committed(1, 7).await);
    assert!(after.abandon_all().await.is_empty());
    let count = tokio::time::timeout(Duration::from_secs(5), desk.cuts_now()).await;
    assert_eq!(
        count,
        Ok(None),
        "the count, at once: the store is no longer served"
    );
    assert!(woken.try_recv().is_err(), "no call after the close woke it");
    assert_eq!(desk.answered(), 0);
}

/// A store whose sync of the copies is under way once begun, and whose
/// begin closes the desk: the close comes between the take of the call
/// and the keeping of its reply, which no other way reaches in a test.
struct ClosingAtTheSync(Arc<Desk>);

impl Store for ClosingAtTheSync {
    fn uploadable(&mut self) -> Option<Uploadable> {
        None
    }

    fn read(&mut self, _: u64, _: u64, _: &mut Buf) -> std::io::Result<CutSlice> {
        Err(std::io::Error::other("no cut"))
    }

    fn begin_sync_copies(&mut self, _: u64) -> std::io::Result<()> {
        self.0.close();
        Ok(())
    }

    fn copies_synced(&mut self, _: bool) -> Option<std::io::Result<()>> {
        None
    }

    fn release(&mut self, _: u64) -> Released {
        Released::Gone
    }

    fn name_committed(&mut self, _: u64, _: u64) -> bool {
        false
    }

    fn abandon_all(&mut self) -> Vec<u64> {
        Vec::new()
    }

    fn cuts(&mut self) -> CutsNow {
        CutsNow::default()
    }
}

/// A desk closed while a sync of the copies begins does not keep the
/// sync's reply for a turn that will never come: the call ends as at a
/// close (Q13 of this pull request's plants).
#[tokio::test]
async fn a_sync_whose_desk_closes_as_it_begins_ends_as_at_a_close() {
    let (wake, mut woken) = tokio::sync::mpsc::unbounded_channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let mut up = Queued::new(Arc::clone(&desk));
    let call = tokio::spawn(async move { up.sync_copies(7).await });
    tokio::time::timeout(Duration::from_secs(10), woken.recv())
        .await
        .expect("the call wakes the queue's thread")
        .expect("the call woke the queue's thread");
    desk.answer(&mut ClosingAtTheSync(Arc::clone(&desk)));
    assert!(desk.closed());
    let synced = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .expect("the call ends at the close")
        .expect("the task");
    let e = synced.expect_err("not synced");
    assert!(e.to_string().contains("no longer served"), "{e}");
}

/// A queue that turns only when woken and never looks on its own answers
/// every call: a call's wake comes after the call is left (G3 of #117's
/// review). Twenty thousand calls, each waited for at most ten seconds.
#[tokio::test]
async fn a_queue_that_turns_only_when_woken_answers_every_call() {
    let fx = Fixture::new(56).await;
    let mut store = fx.start();
    let (wake, woken) = std::sync::mpsc::channel();
    let last = wake.clone();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = {
        let desk = Arc::clone(&desk);
        std::thread::spawn(move || {
            while woken.recv().is_ok() && !desk.closed() {
                desk.answer(&mut store);
            }
        })
    };
    let mut local = Queued::new(Arc::clone(&desk));
    for call in 0..20_000 {
        let answered = tokio::time::timeout(Duration::from_secs(10), local.uploadable()).await;
        assert!(matches!(answered, Ok(None)), "call {call} was not answered");
    }
    assert_eq!(desk.answered(), 20_000);
    desk.close();
    let _ = last.send(());
    queue.join().expect("the queue's thread");
}

/// The count of cuts a request on the daemon's channel reads (M2.5-2) is
/// the store's, taken on the store's own thread when the call comes:
/// its cut state, the cuts counted in it, and the newest. Before the
/// first cut the count is 0 and there is no newest. The store's rewind
/// mark comes with it (M2.6a). The count is not a call:
/// [`Desk::answered`] leaves it out. Mutants: a count one short of the
/// store's; a count counted as a call; the mark left out.
#[tokio::test]
async fn the_count_of_cuts_is_read_through_the_desk_on_the_stores_thread() {
    let mut fx = Fixture::new(53).await;
    let mut store = fx.start();
    let state = store.cut_state_id();
    assert!(state.is_some(), "the store holds cuts");
    assert_eq!(
        Store::cuts(&mut store),
        CutsNow {
            state,
            count: 0,
            newest: None,
            rewound: 0,
        },
        "no cut yet"
    );
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    fx.write(&mut store, 11, 0x11);
    fx.group(&mut store, 8);
    // `zpool import -F` back to group 7: group 8 written anew, whose cut
    // marks the rewind (M2.6a).
    fx.write(&mut store, 12, 0x12);
    fx.group_anew(&mut store, 8);

    let (wake, woken) = std::sync::mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = queue_thread(Arc::clone(&desk), store, woken);
    let now = tokio::time::timeout(Duration::from_secs(5), desk.cuts_now())
        .await
        .expect("the queue's thread gives the count at its next turn, with no call waiting");
    assert_eq!(
        now,
        Some(CutsNow {
            state,
            count: 3,
            newest: Some(Cut {
                number: 3,
                group: 8,
                itself: true
            }),
            rewound: 3,
        })
    );
    assert_eq!(
        desk.answered(),
        0,
        "a count is not a call (N4 of the next look at #127)"
    );
    desk.close();
    let _ = queue.join();
}

/// The count of cuts is given while the uploader's step waits on object
/// storage: a PUT that stalls holds the uploader up, but not `cuts`,
/// which the desk answers at the queue's next turn (F1 and F4 of the
/// review of #127; its scenario stalled each PUT of the step).
///
/// Mutant: the count given only with one of the uploader's calls, which
/// waits for the uploader's step.
#[tokio::test]
async fn the_count_is_given_while_a_put_of_the_uploader_stalls() {
    let mut fx = Fixture::new(57).await;
    let mut store = fx.start();
    let state = store.cut_state_id();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    fx.store.inner().stall(true);
    fx.store.inner().record(true);
    let (wake, woken) = std::sync::mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = queue_thread(Arc::clone(&desk), store, woken);
    let refs = Arc::new(ObjectRefStore::new(fx.store.clone()));
    let lease = Keeper::new(refs, fx.vol, "node-a", 30_000);
    let mut remote = Some(fx.remote());
    let make = move |_epoch| remote.take().expect("made once");
    let mut up = Uploader::with_slices(Queued::new(Arc::clone(&desk)), lease, make, 4);
    let uploading = async {
        for _ in 0..1000 {
            up.turn().await;
        }
    };
    let asking = async {
        while !fx.store.inner().log().iter().any(|op| op == "put seg") {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let asked = Instant::now();
        let now = tokio::time::timeout(Duration::from_secs(5), desk.cuts_now()).await;
        (asked.elapsed(), now)
    };
    let (took, now) = tokio::select! {
        biased;
        got = asking => got,
        () = uploading => panic!("the uploader's step went on: {:?}", up.shown()),
    };
    assert_eq!(
        now,
        Ok(Some(CutsNow {
            state,
            count: 1,
            newest: Some(Cut {
                number: 1,
                group: 7,
                itself: true
            }),
            rewound: 0,
        })),
        "answered while the PUT stalls"
    );
    assert!(took < Duration::from_secs(1), "the count took {took:?}");
    assert_eq!(up.shown().figures.uploaded, 0, "nothing went up");
    fx.store.inner().stall(false);
    desk.close();
    let _ = queue.join();
}
