
// ---- The review's scenarios for #120 (M2.4d) at 0b0789a ----
//
// Appended to crates/skyzfs-local/src/inplace/tests/cuts.rs by
// plants.py for a run, and taken out again: they use that module's
// helpers. `r*` scenarios are expected to pass at the head; a `changed_`
// scenario states what the store did before the pull request and is
// expected to fail at the head.

/// The store's drop on a thread of the test's: whether it ended within
/// `time`.
fn review_dropped_within(
    store: InPlace<SimDisk>,
    time: Duration,
) -> (std::sync::mpsc::Receiver<()>, bool) {
    let (tell, dropped) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(store);
        let _ = tell.send(());
    });
    let ended = dropped.recv_timeout(time).is_ok();
    (dropped, ended)
}

/// r1. A write into a region the release just cleared comes while the
/// header's sync is under way: it waits for that sync and then for one
/// of its own. Each sync of the map's file is counted once, as the
/// header's or as the write path's, the volume stays healthy, and after
/// a power cut the map names the released cut and the write's region.
#[test]
fn review_m24d_r1_a_write_behind_a_header_sync_waits_for_a_sync_of_its_own() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(101, 0);
        let region = region();
        let mut store = one_cut(&disk, &region, io, true);
        let id = store.cut_state_id().expect("a cut state");
        let before = intent_of(&store);
        let syncs = disk.syncs_of(intent::INTENT_NAME);
        disk.hold_syncs_of(intent::INTENT_NAME);
        assert!(store.release_cut(1), "{case}");
        assert_eq!(intent_of(&store).named, 0, "{case}");
        submit(&mut store, 1, write_op(150, 0x66));
        let mut done = Vec::new();
        polled(&mut store, &mut done, 1, Duration::from_millis(50));
        let early = ended(&done);
        let begun = disk.syncs_of(intent::INTENT_NAME) - syncs;
        disk.release_syncs();
        let store = drained(store, &mut done);
        assert!(early.is_empty(), "{case}: {early:?}");
        assert_eq!(begun, 1, "{case}: one sync at a time");
        assert_eq!(ended(&done), [(1, Ok(()))], "{case}");
        assert!(!store.is_failed(), "{case}");
        let after = intent_of(&store);
        assert_eq!(after.header_syncs.count, before.header_syncs.count + 1, "{case}");
        assert_eq!(after.syncs.count, before.syncs.count + 1, "{case}");
        assert_eq!(after.waits, before.waits + 1, "{case}");
        assert_eq!(disk.syncs_of(intent::INTENT_NAME) - syncs, 2, "{case}");
        assert_eq!(after.named, 1, "{case}");
        drop(store);
        disk.power_cut(crate::sim::LoseAll);
        let mut store = start_as(&disk, 0, &region, io, AFTER_A_REBOOT);
        let one = CommittedCut {
            state: id,
            number: 1,
            group: 7,
        };
        assert_eq!(store.committed_cut(), Some(one), "{case}");
        assert_eq!(
            store.intent_at_start(),
            Some(&IntentAtStart::TakenUp {
                named: 1,
                regions: 4
            }),
            "{case}"
        );
        group(&mut store, 400, 10, 100);
        assert!(store.cut_map(1).contains(&150), "{case}");
    }
}

/// r2. A second release while a write waits behind the first release's
/// header sync: its clear takes the write's bit and its header is due.
/// The next sync is the write path's, covers that header too, and is
/// counted with the write path's; no third sync follows. Settled by
/// polls or by the store's drop.
#[test]
fn review_m24d_r2_a_header_due_while_a_write_waits_is_covered_by_the_writes_sync() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        for by_drop in [false, true] {
            let case = format!("{io:?}, settled by the drop {by_drop}");
            let disk = created(102, 0);
            let region = region();
            let mut store = one_cut(&disk, &region, io, true);
            // Cut 2 by a group to the first label alone, so that region 2
            // is clean once cut 1 is released.
            assert!(run(&mut store, 300, uberblock_write(0, 9, 90))
                .result
                .is_ok());
            assert!(run(&mut store, 301, Op::Flush).result.is_ok());
            let id = store.cut_state_id().expect("a cut state");
            let before = intent_of(&store);
            let syncs = disk.syncs_of(intent::INTENT_NAME);
            disk.hold_syncs_of(intent::INTENT_NAME);
            assert!(store.release_cut(1), "{case}");
            assert_eq!(intent_of(&store).named, 1, "{case}: region 0 alone");
            submit(&mut store, 1, write_op(150, 0x66));
            let mut done = Vec::new();
            polled(&mut store, &mut done, 1, Duration::from_millis(50));
            let early = ended(&done);
            let released = store.release_cut(2);
            let begun = disk.syncs_of(intent::INTENT_NAME) - syncs;
            disk.release_syncs();
            assert!(early.is_empty(), "{case}: {early:?}");
            assert!(released, "{case}");
            assert_eq!(begun, 1, "{case}: one sync at a time");
            if by_drop {
                let (_dropped, ended) = review_dropped_within(store, DRAIN);
                assert!(ended, "{case}: the drop did not end");
            } else {
                let mut store = drained(store, &mut done);
                settle_map(&mut store);
                settle_map(&mut store);
                assert_eq!(ended(&done), [(1, Ok(()))], "{case}");
                assert!(!store.is_failed(), "{case}");
                let after = intent_of(&store);
                assert_eq!(
                    after.header_syncs.count,
                    before.header_syncs.count + 1,
                    "{case}"
                );
                assert_eq!(after.syncs.count, before.syncs.count + 1, "{case}");
                assert_eq!(after.named, 1, "{case}: region 2 alone");
                drop(store);
            }
            assert_eq!(disk.syncs_of(intent::INTENT_NAME) - syncs, 2, "{case}");
            disk.power_cut(crate::sim::LoseAll);
            let mut store = start_as(&disk, 0, &region, io, AFTER_A_REBOOT);
            let two = CommittedCut {
                state: id,
                number: 2,
                group: 9,
            };
            assert_eq!(store.committed_cut(), Some(two), "{case}");
            assert_eq!(
                store.intent_at_start(),
                Some(&IntentAtStart::TakenUp {
                    named: 1,
                    regions: 4
                }),
                "{case}"
            );
            group(&mut store, 400, 10, 100);
            assert!(store.cut_map(1).contains(&150), "{case}");
        }
    }
}

/// r3. A header is due when the sync under way fails the volume: no
/// sync follows, and the store's drop ends.
#[test]
fn review_m24d_r3_a_header_due_on_a_failed_volume_does_not_hold_the_drop() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(103, 0);
        let region = region();
        let mut store = one_cut(&disk, &region, io, true);
        write(&mut store, 6, 0x66).unwrap();
        group(&mut store, 200, 8, 80);
        let syncs = disk.syncs_of(intent::INTENT_NAME);
        disk.hold_syncs_of(intent::INTENT_NAME);
        assert!(store.release_cut(1), "{case}");
        assert!(store.release_cut(2), "{case}");
        settle_map(&mut store);
        disk.fail_sync_of(intent::INTENT_NAME);
        disk.release_syncs();
        settle_map(&mut store);
        assert!(store.is_failed(), "{case}");
        settle_map(&mut store);
        let (_dropped, ended) = review_dropped_within(store, DRAIN);
        assert!(ended, "{case}: the drop did not end");
        assert_eq!(
            disk.syncs_of(intent::INTENT_NAME) - syncs,
            1,
            "{case}: no sync after the failure"
        );
    }
}

/// r4. Where the map's file cannot sync on a helper, the header's sync
/// is made at the release: counted as a header's with no poll, and a
/// power cut keeps the base.
#[test]
fn review_m24d_r4_a_header_sync_with_no_helper_is_made_at_the_release() {
    let disk = created(104, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    let id = store.cut_state_id().expect("a cut state");
    let before = intent_of(&store);
    let begins = disk.stats().sync_begins;
    disk.refuse_sync_begins(1);
    assert!(store.release_cut(1));
    assert_eq!(disk.stats().sync_begins, begins, "nothing was begun");
    let after = intent_of(&store);
    assert_eq!(after.header_syncs.count, before.header_syncs.count + 1);
    assert_eq!(after.syncs, before.syncs);
    assert!(!store.is_failed());
    assert_eq!(phases(&store), []);
    drop(store);
    disk.power_cut(crate::sim::LoseAll);
    let store = start_as(&disk, 0, &region, IoMode::default(), AFTER_A_REBOOT);
    let one = CommittedCut {
        state: id,
        number: 1,
        group: 7,
    };
    assert_eq!(store.committed_cut(), Some(one));
}

/// r5. A header's sync that fails at once (made at the release, where
/// the file has no helper): the volume fails; `release_cut` answers true
/// and keeps the cut, as `sync_header` and `release_cut` say of a
/// failure at once, where a failure found later has let it go
/// (`a_header_sync_that_fails_fails_the_volume`). A naming answers
/// false.
#[test]
fn review_m24d_r5_a_header_sync_that_fails_at_once_keeps_the_cut() {
    let disk = created(105, 0);
    let mut store = one_cut(&disk, &region(), IoMode::default(), true);
    disk.refuse_sync_begins(1);
    disk.fail_sync_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    assert!(store.is_failed());
    assert_eq!(phases(&store), [(1, CutPhase::Uploading)]);
    drop(store);

    let disk = created(106, 0);
    let mut store = one_cut(&disk, &region(), IoMode::default(), true);
    assert_eq!(store.abandon_all_cuts(), [1]);
    disk.refuse_sync_begins(1);
    disk.fail_sync_of(intent::INTENT_NAME);
    assert!(!store.name_committed_cut(1, 7));
    assert!(store.is_failed());
}

/// r6. Random programs of writes, groups and releases with the header
/// synced at each release, polled once after every step. Once the map
/// has settled: every sync of the map's file since the start is counted
/// once, as a header's or as the write path's; the volume is healthy;
/// and after a power cut that loses everything no sync covered, the map
/// is taken up naming the newest released cut, and the next cut holds
/// every block that differs from it.
#[test]
fn review_m24d_r6_random_programs_keep_the_newest_released_cut_as_the_base() {
    let mut with_a_release = 0;
    for seed in 0..40 {
        let case = format!("seed {seed}");
        let steps = intent_program(seed, 40);
        let disk = created(seed, 0);
        let held = region();
        let mut store = start_syncing_header(&disk, 0, &held, ON_CALLER, TAKING_OVER, true);
        let id = store.cut_state_id().expect("a cut state");
        let before = intent_of(&store);
        let syncs = disk.syncs_of(intent::INTENT_NAME);
        let mut instants: Vec<Vec<Vec<u8>>> = Vec::new();
        let mut groups: Vec<u64> = Vec::new();
        let mut released: Option<u64> = None;
        let mut txg = 10;
        let mut rid = 100_000;
        for &step in &steps {
            match step {
                IntentStep::Write(block, byte) => {
                    rid += 1;
                    assert!(run(&mut store, rid, write_op(block, byte)).result.is_ok(), "{case}");
                }
                IntentStep::Zeroes(block) => {
                    rid += 1;
                    let zeroes = Op::WriteZeroes {
                        offset: block * BLOCK,
                        len: BLOCK,
                    };
                    assert!(run(&mut store, rid, zeroes).result.is_ok(), "{case}");
                }
                IntentStep::Group => {
                    txg += 1;
                    for label in 0..LABELS {
                        rid += 1;
                        assert!(
                            run(&mut store, rid, uberblock_write(label, txg, txg * 3))
                                .result
                                .is_ok(),
                            "{case}"
                        );
                    }
                    rid += 1;
                    assert!(run(&mut store, rid, Op::Flush).result.is_ok(), "{case}");
                    instants.push(volume_now(&mut store));
                    groups.push(txg);
                }
                IntentStep::Release => {
                    if let Some(oldest) = store.held_cuts().first().copied() {
                        if oldest.phase == CutPhase::Uploading && store.release_cut(oldest.number) {
                            released = Some(oldest.number);
                        }
                    }
                }
            }
            let mut done = Vec::new();
            store.poll(&mut done);
            assert!(done.is_empty(), "{case}");
            assert!(!store.is_failed(), "{case}");
        }
        settle_map(&mut store);
        settle_map(&mut store);
        let after = intent_of(&store);
        assert_eq!(
            disk.syncs_of(intent::INTENT_NAME) - syncs,
            (after.header_syncs.count - before.header_syncs.count)
                + (after.syncs.count - before.syncs.count),
            "{case}: each sync of the map counted once"
        );
        drop(store);
        disk.power_cut(crate::sim::LoseAll);
        let mut store = start_as(&disk, 0, &region(), ON_CALLER, AFTER_A_REBOOT);
        let Some(number) = released else {
            assert_eq!(store.committed_cut(), None, "{case}");
            continue;
        };
        with_a_release += 1;
        let base = CommittedCut {
            state: id,
            number,
            group: groups[number as usize - 1],
        };
        assert_eq!(store.committed_cut(), Some(base), "{case}");
        assert!(
            matches!(store.intent_at_start(), Some(IntentAtStart::TakenUp { .. })),
            "{case}: {:?}",
            store.intent_at_start()
        );
        let then = &instants[number as usize - 1];
        let now = volume_now(&mut store);
        group(&mut store, 500_000, 900, 901);
        let map: std::collections::BTreeSet<u64> = store.cut_map(1).into_iter().collect();
        for block in 0..VOLUME_BLOCKS {
            assert!(
                now[block as usize] == then[block as usize] || map.contains(&block),
                "{case}: block {block} differs from cut {number} and the next cut lacks it"
            );
        }
    }
    assert!(with_a_release >= 20, "{with_a_release} programs released a cut");
}

/// changed_r7. With the option off: a write waits for the map's sync,
/// which the disk holds, and the volume fails for another reason. Before
/// the pull request the store's drop ended without that sync (nothing
/// waited for the map any more); at the head it waits for the sync
/// under way, whatever the option says. On real files the helper's own
/// end waited for it already, so the difference shows on the simulated
/// disk only.
#[test]
fn review_m24d_changed_r7_the_drop_of_a_failed_store_waits_for_the_maps_sync_with_the_option_off() {
    let disk = created(107, 0);
    let mut store = committed_one(&disk, 0, IoMode::default());
    assert!(!intent_of(&store).header_sync);
    disk.hold_syncs_of(intent::INTENT_NAME);
    submit(&mut store, 1, write_op(150, 0x66));
    let mut done = Vec::new();
    polled(&mut store, &mut done, 1, Duration::from_millis(50));
    assert!(done.is_empty(), "{:?}", ended(&done));
    store.fail_now();
    polled(&mut store, &mut done, 1, DRAIN);
    assert_eq!(ended(&done), [(1, Err(BlkError::Failed))]);
    let (dropped, early) = review_dropped_within(store, Duration::from_millis(500));
    disk.release_syncs();
    let late = early || dropped.recv_timeout(DRAIN).is_ok();
    assert!(late, "the drop did not end once the sync was let go");
    assert!(
        early,
        "the drop of a failed store waited for the map's sync under way, with the option off"
    );
}
