//! The store reached through a desk (M2.4c): the store on a thread of its
//! own, as the daemon's queue holds it, answering the uploader's calls
//! between its turns; and a desk closed under the uploader.

mod common;

use std::sync::Arc;
use std::time::Duration;

use skynbd_refs::ObjectRefStore;
use skyzfs_blk::Buf;
use skyzfs_local::inplace::InPlace;
use skyzfs_local::sim::SimDisk;
use skyzfs_upload::lease::Keeper;
use skyzfs_upload::uploader::{State, Uploader};
use skyzfs_upload::{Desk, Local, Queued, Released};

use common::*;

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

/// A call waiting when the desk closes is dropped and answered as by a
/// store that holds nothing; so is every call after, and none of them
/// wakes the queue's thread.
#[tokio::test]
async fn a_closed_desk_answers_as_a_store_that_holds_nothing() {
    let (wake, mut woken) = tokio::sync::mpsc::unbounded_channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let mut waiting = Queued::new(Arc::clone(&desk));
    let call = tokio::spawn(async move { waiting.uploadable().await });
    woken
        .recv()
        .await
        .expect("the call woke the queue's thread");
    assert!(!call.is_finished(), "nobody has answered");

    desk.close();
    let answer = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .expect("the call waiting ends at the close")
        .expect("the task");
    assert!(answer.is_none(), "{answer:?}");

    let mut after = Queued::new(Arc::clone(&desk));
    assert!(after.uploadable().await.is_none());
    let (buf, read) = after.read(1, 0, Buf::zeroed(4096)).await;
    assert!(read.is_err(), "a read fails");
    assert_eq!(buf.len(), 0);
    assert!(after.sync_copies(1).await.is_err(), "a sync fails");
    assert_eq!(after.release(1).await, Released::Gone);
    assert!(!after.name_committed(1, 7).await);
    assert!(after.abandon_all().await.is_empty());
    assert!(woken.try_recv().is_err(), "no call after the close woke it");
    assert_eq!(desk.answered(), 0);
}
