//! A scenario of the review of #114 (M2.4b-2) in the store's own tests:
//! put into `crates/skyzfs-local/src/inplace/tests/cuts/review_m24b2.rs`
//! by `scenarios_store.py` and taken out again.

use super::*;

/// T1 (question 3, the count of commits). The daemon dies in a release,
/// after the intent map's pages and at the first store of the cut
/// state's release: the map on the disk names cut 2 already, and the
/// cut is still held. The next instance takes both up. A write into a
/// clean region waits for the map's sync, which the disk holds. The
/// uploader's check finds the row naming cut 2 and releases it again:
/// the second `clear` for one cut, on a healthy volume, takes the
/// waiting write's bit, as the first can. The write must go on once the
/// map holds its bit, and the volume must not fail.
#[test]
fn review_t1_a_release_made_again_after_a_death_takes_a_waiting_writes_bit() {
    state::tests::quiet_deaths();
    let disk = created(945, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, IoMode::default());
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    assert!(store.release_cut(1));
    // Cut 2 by a group to the first label alone, so that region 2 stays
    // clean (as the head's own test of a clear under a waiting write).
    assert!(run(&mut store, 300, uberblock_write(0, 9, 90)).result.is_ok());
    assert!(run(&mut store, 301, Op::Flush).result.is_ok());
    assert_eq!(phases(&store), [(2, CutPhase::Uploading)]);
    let id = store.cut_state_id().expect("a cut state");
    region.die_at(0);
    let lived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.release_cut(2)));
    region.revive();
    match lived {
        Ok(_) => panic!("the release did not die at its first store"),
        Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => {}
        Err(payload) => std::panic::resume_unwind(payload),
    }
    drop(store);
    disk.kill();
    let mut store = start_on(&disk, 0, &region, IoMode::default());
    assert_eq!(store.cut_state_id(), Some(id), "the state taken up");
    assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "cut 2 still held");
    let base = store.committed_cut().map(|cut| (cut.number, cut.group));
    println!("T1 after the death: held {:?}, the map's base {base:?}, named {}", phases(&store), intent_of(&store).named);
    assert_eq!(base, Some((2, 9)), "the map names cut 2 already");
    disk.hold_syncs_of(intent::INTENT_NAME);
    submit(&mut store, 1, write_op(150, 0x66));
    let mut done = Vec::new();
    polled(&mut store, &mut done, 1, Duration::from_millis(50));
    let early = ended(&done);
    let released = store.release_cut(2);
    disk.release_syncs();
    let store = drained(store, &mut done);
    println!(
        "T1 the release made again: {released}; the write that waited: {:?} (early {early:?}); the volume failed: {}; named {}",
        ended(&done),
        store.is_failed(),
        intent_of(&store).named
    );
    assert!(early.is_empty());
    assert!(released);
    assert_eq!(ended(&done), [(1, Ok(()))], "the write that waited goes on");
    assert!(!store.is_failed(), "a healthy volume failed");
    assert_eq!(phases(&store), []);
}
