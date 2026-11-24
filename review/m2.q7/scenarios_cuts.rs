
// ---- The review's scenarios for #133 (Q7) at d040d95 ----
//
// Appended to crates/skyzfs-local/src/inplace/tests/cuts.rs for a run
// (`cargo test -p skyzfs-local --features test-hooks --lib -- review_q7`),
// and taken out again.

/// s1. F-A end to end: a release that comes to a store that has failed
/// already writes the header naming the cut and, with the sync on, makes
/// it durable; after the store's drop and a power cut that loses all
/// that no sync covered, a start that acknowledges the failure (in
/// another boot) takes the map up naming the released cut. With the sync
/// off, the same power cut loses the header and the map names no cut.
/// This is what the answer to F-A keeps the sync for.
#[test]
fn review_q7_s1_a_release_on_a_failed_store_names_the_base_for_the_acknowledging_start() {
    let acknowledging = Coming {
        acknowledging: true,
        ..AFTER_A_REBOOT
    };
    for header_sync in [false, true] {
        let case = format!("header sync {header_sync}");
        let disk = created(211, 0);
        let region = region();
        let mut store = one_cut(&disk, &region, IoMode::default(), header_sync);
        let id = store.cut_state_id().expect("a cut state");
        store.fail_now();
        assert!(store.is_failed(), "{case}");
        let syncs = disk.syncs_of(intent::INTENT_NAME);
        assert!(store.release_cut(1), "{case}");
        assert_eq!(phases(&store), [], "{case}: the cut is let go");
        assert_eq!(
            disk.syncs_of(intent::INTENT_NAME) - syncs,
            u64::from(header_sync),
            "{case}"
        );
        let (dropped, early) = dropped_within(store, Duration::from_millis(500));
        assert!(early || dropped.recv_timeout(DRAIN).is_ok(), "{case}: the drop ended");
        disk.power_cut(crate::sim::LoseAll);
        let store = start_as(&disk, 0, &region, IoMode::default(), acknowledging);
        let one = CommittedCut {
            state: id,
            number: 1,
            group: 7,
        };
        assert_eq!(store.committed_cut(), header_sync.then_some(one), "{case}");
        assert_eq!(
            matches!(store.intent_at_start(), Some(IntentAtStart::TakenUp { .. })),
            header_sync,
            "{case}: {:?}",
            store.intent_at_start()
        );
    }
}

/// s2. The header's sync begun by a release on a failed store fails:
/// nothing more happens to the failed volume, the release still answers
/// true with the cut let go, and the drop ends.
#[test]
fn review_q7_s2_a_failing_header_sync_on_a_failed_store_ends_the_drop() {
    let disk = created(212, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    store.fail_now();
    disk.fail_sync_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    assert_eq!(phases(&store), []);
    settle_map(&mut store);
    assert!(store.is_failed());
    let (dropped, early) = dropped_within(store, Duration::from_millis(500));
    assert!(early || dropped.recv_timeout(DRAIN).is_ok(), "the drop ended");
}

/// s3. A release on a failed store with the disk holding the header's
/// sync, then the store dropped while it is held, then the sync let go:
/// a start that acknowledges the failure after a power cut still takes
/// the map up naming the cut (the sync the drop did not wait for was
/// made all the same, after it).
#[test]
fn review_q7_s3_the_sync_the_drop_did_not_wait_for_still_covers_the_header() {
    let acknowledging = Coming {
        acknowledging: true,
        ..AFTER_A_REBOOT
    };
    let disk = created(213, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    let id = store.cut_state_id().expect("a cut state");
    store.fail_now();
    disk.hold_syncs_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    let (dropped, early) = dropped_within(store, Duration::from_millis(500));
    disk.release_syncs();
    assert!(early || dropped.recv_timeout(DRAIN).is_ok(), "the drop ended");
    disk.power_cut(crate::sim::LoseAll);
    let store = start_as(&disk, 0, &region, IoMode::default(), acknowledging);
    eprintln!(
        "REVIEW s3: early {early}, committed {:?}, at start {:?}",
        store.committed_cut(),
        store.intent_at_start()
    );
    let one = CommittedCut {
        state: id,
        number: 1,
        group: 7,
    };
    assert_eq!(store.committed_cut(), Some(one));
}

/// s1b. As s1 with the sync on, but the store polled once after the
/// release on the failed store, before its drop: whether a poll of a
/// failed store ends the header's sync (on the simulated disk a sync
/// begun is made only where its end is collected).
#[test]
fn review_q7_s1b_a_release_on_a_failed_store_polled_before_the_drop() {
    let acknowledging = Coming {
        acknowledging: true,
        ..AFTER_A_REBOOT
    };
    let disk = created(214, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    let id = store.cut_state_id().expect("a cut state");
    store.fail_now();
    assert!(store.release_cut(1));
    let mut done = Vec::new();
    store.poll(&mut done);
    eprintln!("REVIEW s1b: syncing after the poll {}", store.intent_syncing());
    drop(store);
    disk.power_cut(crate::sim::LoseAll);
    let store = start_as(&disk, 0, &region, IoMode::default(), acknowledging);
    eprintln!(
        "REVIEW s1b: committed {:?}, at start {:?}",
        store.committed_cut(),
        store.intent_at_start()
    );
    let one = CommittedCut {
        state: id,
        number: 1,
        group: 7,
    };
    assert_eq!(store.committed_cut(), Some(one));
}

/// s1c. The other order: the release comes to a healthy store and its
/// sync ends, then the store fails, then a power cut: the start that
/// acknowledges the failure takes the map up naming the cut. This is the
/// claim the answer to F-A rests on ("the next start acknowledges the
/// failure and takes the map up").
#[test]
fn review_q7_s1c_an_acknowledging_start_takes_up_a_map_synced_before_the_failure() {
    let acknowledging = Coming {
        acknowledging: true,
        ..AFTER_A_REBOOT
    };
    let disk = created(215, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    let id = store.cut_state_id().expect("a cut state");
    assert!(store.release_cut(1));
    settle_map(&mut store);
    store.fail_now();
    drop(store);
    disk.power_cut(crate::sim::LoseAll);
    let store = start_as(&disk, 0, &region, IoMode::default(), acknowledging);
    eprintln!(
        "REVIEW s1c: committed {:?}, at start {:?}, acknowledged",
        store.committed_cut(),
        store.intent_at_start()
    );
    let one = CommittedCut {
        state: id,
        number: 1,
        group: 7,
    };
    assert_eq!(store.committed_cut(), Some(one));
}
