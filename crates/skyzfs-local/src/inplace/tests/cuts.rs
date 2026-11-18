//! The count of cuts (design §5.2; work package M2.1): each rule of
//! `cuts.rs` with a test that its defect fails.
//!
//! The volume is the tests' 1 MiB: four labels and nothing between
//! them, the rings at 128, 384, 640 and 896 KiB. Uberblocks are written
//! as a pool with `ashift` 12 writes them, one 4 KiB slot each, the slot
//! chosen by the group, with the label checksum for the slot's place
//! ([`slot_write`]).

use skyzfs_uberblock::{seal, ByteOrder, Labels, MAGIC};

use super::*;

const KIB: u64 = 1 << 10;
const SLOT: u64 = 4 * KIB;

/// Where label `label`'s ring begins on the tests' volume.
fn ring(label: usize) -> u64 {
    Labels::for_size(1 << 20)
        .rings()
        .nth(label)
        .expect("four labels")
        .start
}

/// A 4 KiB slot holding an uberblock of group `txg` whose root block
/// pointer is made from `root`, little endian, zeros after it.
fn uberblock(txg: u64, root: u64) -> Buf {
    let mut slot = Buf::filled(1, 0);
    let mut put = |at: usize, word: u64| slot[at..at + 8].copy_from_slice(&word.to_le_bytes());
    put(0, MAGIC);
    put(8, 5000);
    put(16, txg);
    for n in 0..16 {
        put(40 + 8 * n, root.rotate_left(n as u32) ^ 0x5a5a);
    }
    slot
}

/// The same with the uberblock's time set to `timestamp`.
fn stamped(txg: u64, root: u64, timestamp: u64) -> Buf {
    let mut slot = uberblock(txg, root);
    slot[32..40].copy_from_slice(&timestamp.to_le_bytes());
    slot
}

/// The same as a multihost write makes it: a later time, and the
/// multihost magic with sequence `seq` marked valid.
fn multihost(txg: u64, root: u64, timestamp: u64, seq: u64) -> Buf {
    let mut slot = stamped(txg, root, timestamp);
    slot[176..184].copy_from_slice(&skyzfs_uberblock::MMP_MAGIC.to_le_bytes());
    slot[192..200].copy_from_slice(&((seq << 32) | 0x02).to_le_bytes());
    slot
}

/// The uberblock write of group `txg` and root `root` to label `label`,
/// in the slot the group chooses.
fn uberblock_write(label: usize, txg: u64, root: u64) -> Op {
    slot_write(
        ring(label) + (txg % (RING_SLOTS)) * SLOT,
        uberblock(txg, root),
    )
}

/// The write of `data`, one slot, at `offset`, with its label checksum
/// as ZFS's write of a slot puts it there.
fn slot_write(offset: u64, mut data: Buf) -> Op {
    seal(&mut data, offset, ByteOrder::Little);
    Op::Write { offset, data }
}

/// A ring's 4 KiB slots.
const RING_SLOTS: u64 = skyzfs_uberblock::RING_SIZE / SLOT;

/// A write that reaches no ring: label 0's configuration.
fn other_write() -> Op {
    Op::Write {
        offset: 32 * KIB,
        data: Buf::filled(1, 0x77),
    }
}

/// The newest cut's group, and whether it opens as itself.
fn newest(store: &InPlace<SimDisk>) -> Option<(u64, bool)> {
    store.cuts().newest.map(|cut| (cut.group, cut.itself))
}

/// The count and the newest cut's group.
fn counted(store: &InPlace<SimDisk>) -> (u64, Option<u64>) {
    let cuts = store.cuts();
    if let Some(newest) = cuts.newest {
        assert_eq!(
            newest.number, cuts.count,
            "the newest cut has the count's number"
        );
    }
    (cuts.count, cuts.newest.map(|cut| cut.group))
}

/// A pool's groups as ZFS writes them: for each group the uberblock to
/// the four labels, then a flush; here with a stray flush between the
/// second and third label in group 6, and a flush with nothing before
/// it at the end. Each group makes one cut, carrying its group, however
/// many flushes follow; the stray flush is the cut of group 6 and the
/// labels after it are repeats. What the store counts after each step,
/// for `mutants` and `io`.
fn groups_counted(mutants: u128, io: IoMode) -> Vec<(u64, Option<u64>)> {
    let disk = created(5, 0);
    let mut store = start_io(&disk, mutants, false, io).unwrap();
    let mut seen = Vec::new();
    let mut id = 0;
    let mut step = |store: &mut InPlace<SimDisk>, op: Op| {
        id += 1;
        assert!(run(store, id, op).result.is_ok());
        seen.push(counted(store));
    };
    for label in 0..LABELS {
        step(&mut store, uberblock_write(label, 5, 50));
    }
    step(&mut store, Op::Flush);
    for label in 0..2 {
        step(&mut store, uberblock_write(label, 6, 60));
    }
    step(&mut store, Op::Flush);
    for label in 2..LABELS {
        step(&mut store, uberblock_write(label, 6, 60));
    }
    step(&mut store, Op::Flush);
    step(&mut store, Op::Flush);
    seen
}

const LABELS: usize = skyzfs_uberblock::LABELS;

/// The groups of a pool make one cut each, at the first flush after
/// the first of their uberblock writes, with every I/O the store has;
/// the repeats after it make none. Counting a repeat is noticed.
#[test]
fn each_group_makes_one_cut_at_the_first_flush_after_its_uberblock() {
    let none = (0, None);
    let five = (1, Some(5));
    let six = (2, Some(6));
    let wanted = [
        none, none, none, none, five, five, five, six, six, six, six, six,
    ];
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        assert_eq!(groups_counted(0, io), wanted, "{io:?}");
    }
    let repeats = groups_counted(Mutants::REPEAT_COUNTED, IoMode::default());
    assert_ne!(repeats, wanted, "counting a repeat is not noticed");
}

/// A repeat is the same group and the same root block pointer. After a
/// rewind import a pool can write another state under a group already
/// used, and that is a new cut; the same state again is not; nor is an
/// earlier group a repeat of a later one. A repeat told by its group
/// alone is noticed.
#[test]
fn a_new_root_under_a_group_already_used_makes_a_cut() {
    let counts = |mutants: u128| {
        let disk = created(6, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        let mut seen = Vec::new();
        for (id, (txg, root)) in [(10, 1), (10, 2), (10, 2), (8, 3)].into_iter().enumerate() {
            let id = 2 * id as u64;
            assert!(run(&mut store, id + 1, uberblock_write(0, txg, root))
                .result
                .is_ok());
            assert!(run(&mut store, id + 2, Op::Flush).result.is_ok());
            seen.push(counted(&store));
        }
        seen
    };
    let wanted = [(1, Some(10)), (2, Some(10)), (2, Some(10)), (3, Some(8))];
    assert_eq!(counts(0), wanted);
    assert_ne!(
        counts(Mutants::REPEAT_BY_GROUP),
        wanted,
        "a repeat by group alone is not noticed"
    );
}

/// A stale multihost write does not move the mark back (design §5.2,
/// "What counts as a repeat"). Group 5 is cut; group 6's uberblock is
/// written to the four labels; then a multihost write that still carries
/// group 5 (ZFS gives the multihost thread the new uberblock only once
/// group 6's writes have returned) lands in a ring's last slot before
/// the flush. It equals the newest cut, so it is a repeat: the flush
/// cuts group 6, and the multihost write of group 6 after it makes no
/// second cut. A repeat compared with the write before it alone is
/// noticed: it cuts group 5 again, and then group 6 twice.
#[test]
fn a_stale_multihost_write_does_not_move_the_mark_back() {
    let counts = |mutants: u128| {
        let disk = created(11, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        let mmp = |label: usize, txg: u64, root: u64| {
            slot_write(ring(label) + (RING_SLOTS - 1) * SLOT, uberblock(txg, root))
        };
        let mut seen = Vec::new();
        let mut id = 0;
        let mut step = |store: &mut InPlace<SimDisk>, op: Op| {
            id += 1;
            assert!(run(store, id, op).result.is_ok());
        };
        for label in 0..LABELS {
            step(&mut store, uberblock_write(label, 5, 50));
        }
        step(&mut store, Op::Flush);
        seen.push(counted(&store));
        for label in 0..LABELS {
            step(&mut store, uberblock_write(label, 6, 60));
        }
        step(&mut store, mmp(2, 5, 50));
        step(&mut store, Op::Flush);
        seen.push(counted(&store));
        step(&mut store, mmp(1, 6, 60));
        step(&mut store, Op::Flush);
        seen.push(counted(&store));
        seen
    };
    let wanted = [(1, Some(5)), (2, Some(6)), (2, Some(6))];
    assert_eq!(counts(0), wanted);
    assert_eq!(
        counts(Mutants::STALE_MOVES_MARK),
        [(1, Some(5)), (2, Some(5)), (3, Some(6))],
        "a stale write that moves the mark back is not noticed"
    );
}

/// An uberblock whose group is 0 makes no cut: a new label's ring,
/// written whole with an uberblock of group 0 at its start, and such a
/// slot written alone. Counting it is noticed.
#[test]
fn a_group_of_zero_makes_no_cut() {
    let counts = |mutants: u128| {
        let disk = created(7, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        let mut whole = Buf::filled(RING_SLOTS as usize, 0);
        whole[..BLOCK_USIZE].copy_from_slice(&uberblock(0, 9));
        let new_ring = Op::Write {
            offset: ring(1),
            data: whole,
        };
        assert!(run(&mut store, 1, new_ring).result.is_ok());
        assert!(run(&mut store, 2, Op::Flush).result.is_ok());
        assert!(run(&mut store, 3, uberblock_write(2, 0, 9)).result.is_ok());
        assert!(run(&mut store, 4, Op::Flush).result.is_ok());
        counted(&store)
    };
    assert_eq!(counts(0), (0, None));
    assert_ne!(
        counts(Mutants::GROUP_ZERO_COUNTED),
        (0, None),
        "a group of 0 is counted unnoticed"
    );
}

/// A flush received while the uberblock write is still being written
/// carries no cut: the write was not applied when the flush was
/// received, even though the flush waits for it. The next flush does.
/// The uberblock write is held on a thread, the flush received behind
/// it, then the write let go. Marking as received is noticed.
#[test]
fn a_flush_received_before_the_uberblock_write_was_applied_makes_no_cut() {
    let counts = |mutants: u128| {
        let disk = created(8, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        disk.hold_direct_io(1);
        let _release = Releases(&disk);
        let request = |id, op| Request {
            id: RequestId(id),
            op,
        };
        store.submit(request(1, uberblock_write(0, 21, 4))).unwrap();
        until_held(&disk);
        store.submit(request(2, Op::Flush)).unwrap();
        let mut done = Vec::new();
        store.poll(&mut done);
        let mut store = release_and_drain(&disk, store, &mut done);
        assert!(done.iter().all(|c| c.result.is_ok()) && done.len() == 2);
        let after_the_first = counted(&store);
        assert!(run(&mut store, 3, Op::Flush).result.is_ok());
        (after_the_first, counted(&store))
    };
    assert_eq!(counts(0), ((0, None), (1, Some(21))));
    let marked_early = counts(Mutants::MARK_AT_RECEIPT);
    assert_eq!(
        marked_early.0,
        (1, Some(21)),
        "marking as received is not noticed"
    );
}

/// A cut is counted at its flush's barrier, not when the flush is
/// received: after an uberblock write was applied, a write is held on
/// a thread and a flush received behind it; the flush carries the cut,
/// and until the write ends the count does not have it. Counting at
/// receipt is noticed.
#[test]
fn a_cut_is_counted_at_its_flushs_barrier() {
    let counts = |mutants: u128| {
        let disk = created(9, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        assert!(run(&mut store, 1, uberblock_write(3, 33, 7)).result.is_ok());
        disk.hold_direct_io(1);
        let _release = Releases(&disk);
        let request = |id, op| Request {
            id: RequestId(id),
            op,
        };
        store.submit(request(2, other_write())).unwrap();
        until_held(&disk);
        store.submit(request(3, Op::Flush)).unwrap();
        let mut done = Vec::new();
        store.poll(&mut done);
        let held = counted(&store);
        let flushed_while_held = done.iter().any(|c| c.id.0 == 3);
        let store = release_and_drain(&disk, store, &mut done);
        assert!(!flushed_while_held, "the flush did not wait for the write");
        assert!(done.iter().all(|c| c.result.is_ok()) && done.len() == 2);
        (held, counted(&store))
    };
    assert_eq!(counts(0), ((0, None), (1, Some(33))));
    assert_eq!(
        counts(Mutants::CUT_AT_RECEIPT).0,
        (1, Some(33)),
        "counting at receipt is not noticed"
    );
}

/// Writes that reach no ring, and writes to a ring without the magic,
/// make no cut, however many flushes follow; the labels' configuration
/// is written as ZFS writes it at every group that changes it.
#[test]
fn writes_without_an_uberblock_make_no_cut() {
    let disk = created(10, 0);
    let mut store = start(&disk, 0, false).unwrap();
    assert!(run(&mut store, 1, other_write()).result.is_ok());
    assert!(run(&mut store, 2, Op::Flush).result.is_ok());
    let zeros = Op::Write {
        offset: ring(2),
        data: Buf::filled(2, 0),
    };
    assert!(run(&mut store, 3, zeros).result.is_ok());
    let zeroes = Op::WriteZeroes {
        offset: ring(3),
        len: SLOT,
    };
    assert!(run(&mut store, 4, zeroes).result.is_ok());
    assert!(run(&mut store, 5, Op::Flush).result.is_ok());
    assert_eq!(counted(&store), (0, None));
}

/// A watcher hears each write applied, each flush's barrier, and each
/// cut after its flush's barrier, by request, with every I/O the store
/// has: a write that reaches no ring is applied too, and a flush with
/// no mark before it has a barrier and no cut.
#[test]
fn a_watcher_hears_each_write_applied_each_barrier_and_each_cut() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let disk = created(7, 0);
        let mut store = start_io(&disk, 0, false, io).unwrap();
        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        store.watch(Box::new({
            let heard = heard.clone();
            move |seen| heard.lock().expect("lock").push(seen)
        }));
        let ops = [
            other_write(),
            uberblock_write(0, 5, 50),
            Op::Flush,
            uberblock_write(1, 5, 50),
            Op::Flush,
        ];
        for (id, op) in ops.into_iter().enumerate() {
            assert!(run(&mut store, id as u64 + 1, op).result.is_ok(), "{io:?}");
        }
        let id = RequestId;
        let cut = Cut {
            number: 1,
            group: 5,
            itself: true,
        };
        assert_eq!(
            *heard.lock().expect("lock"),
            [
                Seen::Applied(id(1)),
                Seen::Applied(id(2)),
                Seen::Barrier(id(3)),
                Seen::Cut(id(3), cut),
                Seen::Applied(id(4)),
                Seen::Barrier(id(5)),
            ],
            "{io:?}"
        );
    }
}

/// A store begun on a volume that already holds a pool reads its rings
/// before the first request (design §5.2, "Whether a cut opens as
/// itself"), with every I/O the store has. The first attachment writes
/// group 10 to the four labels. The next is a rewind import to group 8,
/// whose cut does not open as itself: group 10 is still in every ring,
/// and an import opens that. The import's own group 10, written over
/// the old one's slot in every label, does. A start that does not read
/// the rings is noticed: it takes group 8 to open as itself.
#[test]
fn a_store_begun_on_a_pool_reads_its_rings_first() {
    let opens = |mutants: u128, io: IoMode| {
        let disk = created(12, 0);
        let mut id = 0;
        let mut group = |store: &mut InPlace<SimDisk>, txg: u64, root: u64| {
            for label in 0..LABELS {
                id += 1;
                assert!(run(store, id, uberblock_write(label, txg, root))
                    .result
                    .is_ok());
            }
            id += 1;
            assert!(run(store, id, Op::Flush).result.is_ok());
            newest(store)
        };
        let mut store = start_io(&disk, mutants, false, io).unwrap();
        let mut seen = vec![group(&mut store, 10, 1)];
        drop(store);
        let mut store = start_io(&disk, mutants, false, io).unwrap();
        seen.push(newest(&store));
        seen.push(group(&mut store, 8, 2));
        seen.push(group(&mut store, 10, 3));
        seen
    };
    let wanted = [Some((10, true)), None, Some((8, false)), Some((10, true))];
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        assert_eq!(opens(0, io), wanted, "{io:?}");
    }
    assert_eq!(
        opens(Mutants::RINGS_UNREAD, IoMode::default()),
        [Some((10, true)), None, Some((8, true)), Some((10, true))],
        "a start that does not read the rings is not noticed"
    );
}

/// Among uberblocks of one group an import takes the later time, then
/// the greater multihost sequence (`vdev_uberblock_compare`). Here a
/// multihost pool's copy of group 10 is left in label 2's multihost slot
/// when a rewind import writes another group 10, later: an import opens
/// the later one, so its cut opens as itself, while group 9 before it
/// does not. Taking the group alone for the order is noticed: it calls
/// the two states of group 10 a tie, and so a rewind.
#[test]
fn a_group_written_twice_opens_as_its_later_state() {
    let opens = |mutants: u128| {
        let disk = created(13, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        let mut id = 0;
        let mut seen = Vec::new();
        let mut step = |store: &mut InPlace<SimDisk>, writes: &[(usize, u64)], slot: Buf| {
            for &(label, at) in writes {
                id += 1;
                let write = slot_write(ring(label) + at * SLOT, slot.clone());
                assert!(run(store, id, write).result.is_ok());
            }
            id += 1;
            assert!(run(store, id, Op::Flush).result.is_ok());
            seen.push(newest(store));
        };
        let every = |at: u64| (0..LABELS).map(|label| (label, at)).collect::<Vec<_>>();
        step(&mut store, &every(10), stamped(10, 1, 100));
        // The multihost write of the same state, later.
        step(&mut store, &[(2, RING_SLOTS - 1)], stamped(10, 1, 150));
        // The rewind import's groups.
        step(&mut store, &every(9), stamped(9, 2, 200));
        step(&mut store, &every(10), stamped(10, 3, 300));
        seen
    };
    assert_eq!(
        opens(0),
        [
            Some((10, true)),
            Some((10, true)),
            Some((9, false)),
            Some((10, true))
        ]
    );
    assert_eq!(
        opens(Mutants::ORDER_BY_GROUP),
        [
            Some((10, true)),
            Some((10, true)),
            Some((9, false)),
            Some((10, false))
        ],
        "the group alone for the order is not noticed"
    );
}

/// What a write-zeroes leaves in a ring is known: zeros, which hold no
/// uberblock. Once group 10 is zeroed out of every ring, the cut of
/// group 8 opens as itself; before, it would not.
#[test]
fn a_ring_zeroed_holds_no_uberblock() {
    let disk = created(14, 0);
    let mut store = start(&disk, 0, false).unwrap();
    let mut id = 0;
    let mut step = |store: &mut InPlace<SimDisk>, op: Op| {
        id += 1;
        assert!(run(store, id, op).result.is_ok());
    };
    for label in 0..LABELS {
        step(&mut store, uberblock_write(label, 10, 1));
    }
    step(&mut store, Op::Flush);
    assert_eq!(newest(&store), Some((10, true)));
    for label in 0..LABELS {
        let zeroes = Op::WriteZeroes {
            offset: ring(label) + 10 * SLOT,
            len: SLOT,
        };
        step(&mut store, zeroes);
    }
    for label in 0..LABELS {
        step(&mut store, uberblock_write(label, 8, 2));
    }
    step(&mut store, Op::Flush);
    assert_eq!(newest(&store), Some((8, true)));
}

/// ZFS issues a group's four label writes at once, and a stray flush
/// can be received between two of them being applied (design §5.2, "a
/// stray flush between the uberblock copies"). The flush takes the mark
/// when it is received; a label write applied before that flush's
/// barrier is a repeat of the state the flush carries, so the next flush
/// makes no second cut. Here label 1's write is held on a thread while
/// the flush is received. A repeat not compared with the carried state
/// is noticed: the same state is cut twice (the review of M2.1, and
/// Codex's).
#[test]
fn a_label_write_applied_while_a_flush_carries_its_state_is_a_repeat() {
    let counts = |mutants: u128| {
        let disk = created(12, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        assert!(run(&mut store, 1, uberblock_write(0, 5, 50)).result.is_ok());
        disk.hold_direct_io(1);
        let _release = Releases(&disk);
        let request = |id, op| Request {
            id: RequestId(id),
            op,
        };
        store.submit(request(2, uberblock_write(1, 5, 50))).unwrap();
        until_held(&disk);
        store.submit(request(3, Op::Flush)).unwrap();
        let mut done = Vec::new();
        store.poll(&mut done);
        let mut store = release_and_drain(&disk, store, &mut done);
        assert!(done.iter().all(|c| c.result.is_ok()) && done.len() == 2);
        let at_the_stray_flush = counted(&store);
        assert!(run(&mut store, 4, Op::Flush).result.is_ok());
        (at_the_stray_flush, counted(&store))
    };
    assert_eq!(counts(0), ((1, Some(5)), (1, Some(5))));
    assert_eq!(
        counts(Mutants::REPEAT_WHILE_CARRIED),
        ((1, Some(5)), (2, Some(5))),
        "a repeat of the state a flush carries is not noticed"
    );
}

/// A flush that carries a cut and waits for a sync under way has its
/// barrier when it is executed, before a write received after it is
/// applied, and not when its own sync begins or ends. The home image's
/// syncs are held, so the sync of the flush before is under way when
/// the uberblock write is applied and the flush that carries its cut is
/// received. From the review of M2.1: it notices a barrier told at the
/// flush's own sync, at its beginning or at its end.
#[test]
fn a_flush_behind_a_sync_under_way_has_its_barrier_when_executed() {
    let disk = created(13, 0);
    let mut store = start(&disk, 0, false).unwrap();
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    store.watch(Box::new({
        let heard = heard.clone();
        move |seen| heard.lock().expect("lock").push(seen)
    }));
    disk.hold_syncs_of(crate::store::HOME_NAME);
    let submit = |store: &mut InPlace<SimDisk>, id: u64, op: Op| {
        store
            .submit(Request {
                id: RequestId(id),
                op,
            })
            .unwrap()
    };
    let mut done = Vec::new();
    submit(&mut store, 1, other_write());
    assert!(until_done(&mut store, &mut done, 1));
    submit(&mut store, 2, Op::Flush);
    assert!(store.syncing());
    submit(&mut store, 3, uberblock_write(0, 5, 50));
    assert!(until_done(&mut store, &mut done, 3));
    submit(&mut store, 4, Op::Flush);
    submit(
        &mut store,
        5,
        Op::Write {
            offset: 40 * KIB,
            data: Buf::filled(1, 0x55),
        },
    );
    assert!(until_done(&mut store, &mut done, 5));
    disk.release_syncs();
    while store.in_flight() > 0 {
        store.wait(&mut done, Duration::from_millis(10));
    }
    assert!(done.iter().all(|c| c.result.is_ok()));
    let id = RequestId;
    let cut = Cut {
        number: 1,
        group: 5,
        itself: true,
    };
    assert_eq!(
        *heard.lock().expect("lock"),
        [
            Seen::Applied(id(1)),
            Seen::Barrier(id(2)),
            Seen::Applied(id(3)),
            Seen::Barrier(id(4)),
            Seen::Cut(id(4), cut),
            Seen::Applied(id(5)),
        ]
    );
}

/// A flush that carries a cut and joins a sync under way has its
/// barrier and its cut. A stray flush received while the uberblock
/// write was on a thread begins a sync once the write is applied; the
/// flush after it, received after the write was applied, carries the
/// cut and joins that sync. From the review of M2.1: it notices a
/// barrier told at the flush's own sync, and a joined flush that has
/// none.
#[test]
fn a_flush_that_joins_a_sync_has_its_barrier_and_its_cut() {
    let disk = created(14, 0);
    let mut store = start(&disk, 0, false).unwrap();
    disk.hold_direct_io(1);
    let _release = Releases(&disk);
    let request = |id, op| Request {
        id: RequestId(id),
        op,
    };
    store.submit(request(1, uberblock_write(0, 7, 70))).unwrap();
    until_held(&disk);
    store.submit(request(2, Op::Flush)).unwrap();
    disk.hold_syncs_of(crate::store::HOME_NAME);
    let mut done = Vec::new();
    disk.release_direct_io();
    assert!(until_done(&mut store, &mut done, 1));
    assert!(store.syncing(), "the stray flush began a sync");
    store.submit(request(3, Op::Flush)).unwrap();
    assert_eq!(store.counts().joined, 1, "the flush joined it");
    let while_syncing = counted(&store);
    disk.release_syncs();
    while store.in_flight() > 0 {
        store.wait(&mut done, Duration::from_millis(10));
    }
    assert!(done.iter().all(|c| c.result.is_ok()) && done.len() == 3);
    assert_eq!(while_syncing, (1, Some(7)), "the joined flush's cut");
}

/// A multihost pool writes the cut's own uberblock again, with a later
/// time and a multihost sequence, into the ring's last slot. That copy
/// ranks above the uberblock that marks the cut, but it is the cut's
/// own state, and an import opens that state: the cut opens as itself,
/// here with the copy applied before the cut's flush. Comparing
/// uberblocks and not states is noticed: it calls every such cut a
/// rewind (the lead's reading of M2.1b, point 4).
#[test]
fn a_multihost_copy_of_the_cuts_own_state_leaves_it_itself() {
    let opens = |mutants: u128| {
        let disk = created(15, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        let mut id = 0;
        let mut step = |store: &mut InPlace<SimDisk>, op: Op| {
            id += 1;
            assert!(run(store, id, op).result.is_ok());
        };
        let write = slot_write;
        let mut seen = Vec::new();
        for (txg, root, at) in [(10, 1, 100), (11, 2, 200)] {
            for label in 0..LABELS {
                let slot = stamped(txg, root, at);
                step(&mut store, write(ring(label) + txg * SLOT, slot));
            }
            let copy = multihost(txg, root, at + 50, 3);
            step(&mut store, write(ring(2) + (RING_SLOTS - 1) * SLOT, copy));
            step(&mut store, Op::Flush);
            seen.push(newest(&store));
        }
        seen
    };
    assert_eq!(opens(0), [Some((10, true)), Some((11, true))]);
    assert_eq!(
        opens(Mutants::ITSELF_BY_RANK),
        [Some((10, false)), Some((11, false))],
        "uberblocks compared and not states is not noticed"
    );
}

/// A rewind import by `-F` begins its new history at the abandoned
/// best group (OpenZFS 2.4.1, `spa.c` 6342 and 4713): its first
/// uberblock has that group, another root and a later time, and an
/// import chooses it at once. Here a stray flush comes after two of its
/// four label writes, while the other two labels still hold the
/// abandoned state of the same group: that cut opens as itself, and the
/// labels after it are repeats. Taking the group alone for the order is
/// noticed: it calls the two states of group 12 a tie, and so a rewind.
#[test]
fn a_rewind_by_f_opens_as_its_first_group_at_once() {
    let opens = |mutants: u128| {
        let disk = created(16, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        let mut id = 0;
        let mut step = |store: &mut InPlace<SimDisk>, op: Op| {
            id += 1;
            assert!(run(store, id, op).result.is_ok());
        };
        let write = |label: usize, root: u64, at: u64| {
            slot_write(ring(label) + 12 * SLOT, stamped(12, root, at))
        };
        let mut seen = Vec::new();
        for label in 0..LABELS {
            step(&mut store, write(label, 1, 100));
        }
        step(&mut store, Op::Flush);
        seen.push((store.cuts().count, newest(&store)));
        // The import by -F: group 12 again, another root, later.
        for label in 0..2 {
            step(&mut store, write(label, 2, 200));
        }
        step(&mut store, Op::Flush);
        seen.push((store.cuts().count, newest(&store)));
        for label in 2..LABELS {
            step(&mut store, write(label, 2, 200));
        }
        step(&mut store, Op::Flush);
        seen.push((store.cuts().count, newest(&store)));
        seen
    };
    assert_eq!(
        opens(0),
        [
            (1, Some((12, true))),
            (2, Some((12, true))),
            (2, Some((12, true)))
        ]
    );
    assert_eq!(
        opens(Mutants::ORDER_BY_GROUP),
        [
            (1, Some((12, true))),
            (2, Some((12, false))),
            (2, Some((12, false)))
        ],
        "the group alone for the order is not noticed"
    );
}

/// Another state under the group a flush carries, applied between that
/// flush's receipt and its barrier, is not a repeat of the carried
/// state: it marks, and the next flush cuts it. Here the abandoned
/// state of group 12 marks; a write of group 12's new state, as the
/// first group of a rewind by `-F` writes it, is received and held on a
/// thread; a flush carries the abandoned state's cut; then the write is
/// let go before that flush's barrier. The abandoned state's cut does
/// not open as itself (the new state is later in view), and the new
/// state's does. A carried state compared by its group alone (the
/// defect `REPEAT_BY_GROUP`, for the carried comparison) is noticed:
/// the new state makes no cut (the review of M2.1, its C2).
#[test]
fn another_state_under_the_carried_group_marks() {
    let counts = |mutants: u128| {
        let disk = created(18, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        let write = |label: usize, root: u64, at: u64| {
            slot_write(ring(label) + 12 * SLOT, stamped(12, root, at))
        };
        assert!(run(&mut store, 1, write(0, 1, 100)).result.is_ok());
        disk.hold_direct_io(1);
        let _release = Releases(&disk);
        let request = |id, op| Request {
            id: RequestId(id),
            op,
        };
        store.submit(request(2, write(1, 2, 200))).unwrap();
        until_held(&disk);
        store.submit(request(3, Op::Flush)).unwrap();
        let mut done = Vec::new();
        store.poll(&mut done);
        let mut store = release_and_drain(&disk, store, &mut done);
        assert!(done.iter().all(|c| c.result.is_ok()) && done.len() == 2);
        let at_the_flush = (store.cuts().count, newest(&store));
        assert!(run(&mut store, 4, Op::Flush).result.is_ok());
        (at_the_flush, (store.cuts().count, newest(&store)))
    };
    assert_eq!(counts(0), ((1, Some((12, false))), (2, Some((12, true)))));
    assert_eq!(
        counts(Mutants::REPEAT_BY_GROUP),
        ((1, Some((12, false))), (1, Some((12, false)))),
        "a carried state compared by its group alone is not noticed"
    );
}

/// Bytes in a ring that carry the magic and a high group, but whose
/// label checksum does not hold, are not an uberblock: ZFS passes over
/// them, and so does the store (work package M2.1c). Here they are
/// written into label 1's slot of group 99 after group 11's labels and
/// before the flush that cuts group 11, once with no checksum and once
/// with the checksum of another place (a slot copied there). The cut is
/// group 11's, and it opens as itself. With the checksum not checked
/// (the defect) the bytes mark: the cut takes group 99, a group that
/// errs high, and says it opens as itself, as they are the greatest in
/// view and its own state; a publish could take it.
#[test]
fn bytes_zfs_would_pass_over_make_no_cut() {
    let opens = |mutants: u128, forged: &dyn Fn(u64) -> Buf| {
        let disk = created(17, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        let mut id = 0;
        let mut step = |store: &mut InPlace<SimDisk>, op: Op| {
            id += 1;
            assert!(run(store, id, op).result.is_ok());
        };
        let mut seen = Vec::new();
        for label in 0..LABELS {
            step(&mut store, uberblock_write(label, 10, 1));
        }
        step(&mut store, Op::Flush);
        seen.push((store.cuts().count, newest(&store)));
        for label in 0..LABELS {
            step(&mut store, uberblock_write(label, 11, 2));
        }
        let at = ring(1) + (99 % RING_SLOTS) * SLOT;
        step(
            &mut store,
            Op::Write {
                offset: at,
                data: forged(at),
            },
        );
        step(&mut store, Op::Flush);
        seen.push((store.cuts().count, newest(&store)));
        seen
    };
    let unsealed = |_: u64| uberblock(99, 9);
    let elsewhere = |at: u64| {
        let mut data = uberblock(99, 9);
        seal(&mut data, at - SLOT, ByteOrder::Little);
        data
    };
    let forgeries: [(&str, &dyn Fn(u64) -> Buf); 2] =
        [("no checksum", &unsealed), ("another place's", &elsewhere)];
    for (name, forged) in forgeries {
        assert_eq!(
            opens(0, forged),
            [(1, Some((10, true))), (2, Some((11, true)))],
            "{name}"
        );
        assert_eq!(
            opens(Mutants::CHECKSUM_UNCHECKED, forged),
            [(1, Some((10, true))), (2, Some((99, true)))],
            "{name}: a checksum not checked is not noticed"
        );
    }
}

// ---- Held cuts (M2.2): the cut state through the store ----

/// The tests' volume, in blocks.
const VOLUME_BLOCKS: u64 = (1 << 20) / BLOCK;
/// A bound of 64 blocks, which keeps the tests' region small.
const TEST_BOUND: u64 = 64 * BLOCK;
/// The tests' region of the intent map: 64 blocks, so that the tests'
/// volume of 1 MiB has four.
const TEST_INTENT_REGION: u64 = 64 * BLOCK;
/// The I/O on the caller's thread: a death at a store of the region
/// falls at the same point of a request every time.
const ON_CALLER: IoMode = OTHER_IO[1];

/// A region for the cut state of the tests' volume.
fn region() -> state::MemRegion {
    state::MemRegion::new(state::words_for(
        VOLUME_BLOCKS,
        state::records_for(TEST_BOUND),
    ))
}

/// How a test's start comes to the volume: in which boot, whether it
/// acknowledges a failure, and whether its device is one it took over.
#[derive(Debug, Clone, Copy)]
struct Coming {
    boot: [u8; 16],
    acknowledging: bool,
    taken_over: bool,
}

/// A daemon's start after a death of the one before, in the same boot:
/// it takes the device over.
const TAKING_OVER: Coming = Coming {
    boot: state::tests::BOOT,
    acknowledging: false,
    taken_over: true,
};

/// Start on `disk` with the cut state in `region`, taking the device
/// over.
fn start_on(
    disk: &SimDisk,
    mutants: u128,
    region: &state::MemRegion,
    io: IoMode,
) -> InPlace<SimDisk> {
    start_as(disk, mutants, region, io, TAKING_OVER)
}

/// Start on `disk` with the cut state in `region`, as `coming` says.
fn start_as(
    disk: &SimDisk,
    mutants: u128,
    region: &state::MemRegion,
    io: IoMode,
    coming: Coming,
) -> InPlace<SimDisk> {
    start_syncing_header(disk, mutants, region, io, coming, false)
}

/// As [`start_as`], and with `header_sync` the intent map's header is
/// synced at each release (M2.4d).
fn start_syncing_header(
    disk: &SimDisk,
    mutants: u128,
    region: &state::MemRegion,
    io: IoMode,
    coming: Coming,
    header_sync: bool,
) -> InPlace<SimDisk> {
    let options = OpenOptions {
        io,
        acknowledge_failure: coming.acknowledging,
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(region.clone()),
            boot: coming.boot,
            bound: Some(TEST_BOUND),
            intent_region: Some(TEST_INTENT_REGION),
            intent_header_sync: header_sync,
        }),
        ..OpenOptions::default()
    };
    let mut opened = Opened::open(disk.clone(), lock(disk).expect("the lock"), options)
        .map_err(|(e, _)| e)
        .expect("opens");
    if coming.taken_over {
        opened.device_taken_over();
    }
    opened
        .start_with(None, Mutants(mutants))
        .expect("the in-place store")
}

/// Group `txg`'s uberblock to the four labels, then the flush after it,
/// with ids from `id`.
fn group(store: &mut InPlace<SimDisk>, id: u64, txg: u64, root: u64) {
    for label in 0..LABELS {
        let done = run(store, id + label as u64, uberblock_write(label, txg, root));
        assert!(done.result.is_ok());
    }
    assert!(run(store, id + LABELS as u64, Op::Flush).result.is_ok());
}

/// Block `block` of held cut `number` as the cut holds it: from its own
/// map, or else from the held cuts before it, the oldest of which holds
/// the whole volume in these tests.
fn cut_block(store: &mut InPlace<SimDisk>, number: u64, block: u64) -> Vec<u8> {
    let held = store.held_cuts();
    let mut buf = Buf::filled(1, 0xEE);
    for cut in held.iter().rev().filter(|c| c.number <= number) {
        if store.read_cut(cut.number, block, &mut buf).expect("reads") {
            return buf.to_vec();
        }
    }
    panic!("block {block} of cut {number} is in no held cut's map");
}

/// The group of the uberblock that held cut `number` holds in label 0's
/// slot for group `txg`; 0 where the slot holds none.
fn group_in_cut(store: &mut InPlace<SimDisk>, number: u64, txg: u64) -> u64 {
    let offset = ring(0) + (txg % RING_SLOTS) * SLOT;
    let slot = cut_block(store, number, offset / BLOCK);
    let word = |at: usize| u64::from_le_bytes(slot[at..at + 8].try_into().expect("8 bytes"));
    if word(0) == MAGIC {
        word(16)
    } else {
        0
    }
}

/// A held cut reads as the volume was at its instant after a write has
/// changed a block of it, with every I/O the store has: the store copied
/// the block before the write began. Releasing the cut lets its copies
/// go. A copy made after the overwrite is noticed.
#[test]
fn a_held_cut_reads_as_the_volume_at_its_instant_after_an_overwrite() {
    let read_back = |mutants: u128, io: IoMode| -> Vec<u8> {
        let disk = created(30, 0);
        let region = region();
        let mut store = start_on(&disk, mutants, &region, io);
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        let held: Vec<(u64, u64, CutPhase)> = store
            .held_cuts()
            .iter()
            .map(|c| (c.number, c.state.txg, c.phase))
            .collect();
        assert_eq!(held, [(1, 7, CutPhase::Uploading)], "{io:?}");
        write(&mut store, 5, 0x66).unwrap();
        assert_eq!(read(&mut store, 5).unwrap(), vec![0x66; BLOCK_USIZE]);
        assert_eq!(cut_block(&mut store, 1, 3), vec![0x33; BLOCK_USIZE]);
        assert_eq!(group_in_cut(&mut store, 1, 7), 7, "{io:?}");
        let five = cut_block(&mut store, 1, 5);
        assert!(store.cut_figures().copied > 0);
        assert!(store.release_cut(1));
        assert!(store.held_cuts().is_empty());
        assert_eq!(store.cut_figures().copied, 0);
        five
    };
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        assert_eq!(read_back(0, io), vec![0x55; BLOCK_USIZE], "{io:?}");
    }
    assert_ne!(
        read_back(Mutants::COPY_AFTER_OVERWRITE, ON_CALLER),
        vec![0x55; BLOCK_USIZE],
        "a copy made after the overwrite is not noticed"
    );
}

/// What a death while group 8's uberblock write was under way left:
/// whether the run died, and the held cuts' (number, group, the group of
/// the uberblock the cut holds for it) after the next instance has had
/// everything the kernel issues again.
type AfterDeath = (bool, Vec<(u64, u64, u64)>);

/// Group 7 is cut and being uploaded. Group 8's uberblock write to label
/// 0 is received, applied and completed, and the daemon is lost at store
/// `death` of the cut state's region. The next instance gets a flush
/// first (a stray one, which the lost daemon never had: the kernel hands
/// a next instance what was in flight in any order), then the write
/// again unless its completion had gone to the transport, then ZFS's
/// flush after it.
fn lost_in_a_mark(mutants: u128, io: IoMode, death: u64) -> AfterDeath {
    state::tests::quiet_deaths();
    let disk = created(31, 0);
    let region = region();
    let mut store = start_on(&disk, mutants, &region, io);
    group(&mut store, 100, 7, 70);
    let id = RequestId(200);
    let mut done = Vec::new();
    region.die_at(death);
    let lived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store
            .submit(Request {
                id,
                op: uberblock_write(0, 8, 80),
            })
            .unwrap();
        while done.is_empty() {
            store.wait(&mut done, WAIT);
        }
    }));
    region.revive();
    let died = match lived {
        Ok(()) => false,
        Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => true,
        Err(payload) => std::panic::resume_unwind(payload),
    };
    let handed = done.iter().any(|c| c.id == id);
    if died {
        drop(store);
        disk.kill();
        store = start_on(&disk, mutants, &region, io);
    }
    assert!(run(&mut store, 201, Op::Flush).result.is_ok());
    if !handed {
        let again = run(&mut store, 202, uberblock_write(0, 8, 80));
        assert!(again.result.is_ok());
    }
    assert!(run(&mut store, 203, Op::Flush).result.is_ok());
    let held = store.held_cuts();
    let groups = held
        .iter()
        .map(|c| {
            (
                c.number,
                c.state.txg,
                group_in_cut(&mut store, c.number, c.state.txg),
            )
        })
        .collect();
    (died, groups)
}

/// The deaths at every store of a group's uberblock write, under
/// `mutants` and with I/O `io`: the first wrong outcome, if any. Each
/// held cut must hold the uberblock of its group (a mark recorded before
/// the write is whole cuts a group the cut does not hold), and group 8
/// must be cut (a mark recorded after the completion went to the
/// transport is lost with the daemon, and the kernel does not issue the
/// write again).
fn marks_survive_a_death(mutants: u128, io: IoMode) -> Result<u64, String> {
    let mut death = 0;
    loop {
        let (died, groups) = lost_in_a_mark(mutants, io, death);
        if let Some((number, txg, held)) = groups.iter().find(|(_, txg, held)| txg != held) {
            return Err(format!(
                "death at store {death}: cut {number} of group {txg} holds group {held}"
            ));
        }
        if groups.last().map(|g| g.1) != Some(8) {
            return Err(format!(
                "death at store {death}: group 8 was not cut, held {groups:?}"
            ));
        }
        if !died {
            return Ok(death);
        }
        death += 1;
    }
}

/// The mark is in the cut state once the uberblock write is whole and
/// before its completion goes to the transport (design §5.2): a death
/// at any store of the region while the write is under way leaves a
/// next instance that cuts the group once, holding its uberblock. A mark
/// recorded at receipt, and one recorded after the completion, are each
/// noticed. Both on the caller's thread and with one I/O thread, where
/// the mark is recorded when the pump takes the thread's result: without
/// that, the mark waits for the next request (the review of #98, S2).
#[test]
fn a_death_at_any_store_of_a_mark_leaves_the_group_cut_whole() {
    for io in [ON_CALLER, OTHER_IO[0]] {
        let stores = marks_survive_a_death(0, io).expect("every death");
        assert!(stores > 4, "{stores} stores, {io:?}");
        for (name, mutant) in [
            ("a mark recorded at receipt", Mutants::MARK_BEFORE_WHOLE),
            (
                "a mark recorded after the completion",
                Mutants::MARK_AFTER_COMPLETION,
            ),
        ] {
            assert!(
                marks_survive_a_death(mutant, io).is_err(),
                "{name}: not noticed, {io:?}"
            );
        }
    }
}

/// What a start makes of the cut held when the daemon before it was
/// lost: block 5 was written after cut 1's instant, so the cut reads it
/// from its copy. `between` does to the disk what happened between the
/// two daemons. What the start did, the held cuts' numbers, and block 5
/// of cut 1 if it is held.
fn after_a_loss(
    mutants: u128,
    coming: Coming,
    between: &dyn Fn(&SimDisk),
) -> (CutsTakenUp, Vec<u64>, Option<Vec<u8>>) {
    let disk = created(32, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    write(&mut store, 5, 0x66).unwrap();
    assert!(store.cut_figures().copied > 0);
    drop(store);
    disk.kill();
    between(&disk);
    let mut store = start_as(&disk, mutants, &region, ON_CALLER, coming);
    let taken = store
        .cuts_at_start()
        .cloned()
        .expect("the store holds cuts");
    let held: Vec<u64> = store.held_cuts().iter().map(|c| c.number).collect();
    // Cut 1, the oldest, holds the whole volume. A copy a wrongly
    // trusted state names may not read at all: that is no content.
    let five = (!held.is_empty()).then(|| {
        let mut buf = Buf::filled(1, 0xEE);
        match store.read_cut(1, 5, &mut buf) {
            Ok(true) => buf.to_vec(),
            _ => Vec::new(),
        }
    });
    (taken, held, five)
}

/// A copy that cannot be made (writes to the copies file refused, as a
/// full file system refuses them, or the image's read refused) lets go
/// the held cut that reads the block's content and every newer one; an
/// older cut, which reads its own copy of the block, stays; the
/// client's write goes on, and the volume does not fail (the owner's
/// ruling, through the lead, M2.2b). Where the cut that reads the block
/// is the one that counts the whole volume as changed, it goes too.
/// The defect fails the volume at the first refused copy.
#[test]
fn a_copy_that_cannot_be_made_lets_its_cuts_go_and_the_write_goes_on() {
    let refuse_copies = |disk: &SimDisk| disk.fail_write_of(COPIES_NAME);
    let refuse_image = |disk: &SimDisk| disk.fail_read_of(crate::store::HOME_NAME);
    /// A fault: its name, and what sets it on the disk.
    type Fault<'a> = (&'a str, &'a dyn Fn(&SimDisk));
    let faults: [Fault; 2] = [
        ("the copies file's write", &refuse_copies),
        ("the image's read", &refuse_image),
    ];
    for (fault, refuse) in faults {
        for io in [IoMode::default(), ON_CALLER] {
            for mutants in [0, Mutants::COPY_FAILURE_FAILS] {
                let case = format!("{fault}, {io:?}, mutants {mutants:#x}");
                let disk = created(34, 0);
                let region = region();
                let mut store = start_on(&disk, mutants, &region, io);
                write(&mut store, 5, 0x55).unwrap();
                // Cut 1, the whole volume (the state is new), uploaded.
                group(&mut store, 100, 7, 70);
                // Copied for cut 1, which reads 0x55.
                write(&mut store, 5, 0x66).unwrap();
                // Cut 2, waiting: its map holds block 5.
                group(&mut store, 200, 8, 80);
                let held = |store: &InPlace<SimDisk>| -> Vec<u64> {
                    store.held_cuts().iter().map(|c| c.number).collect()
                };
                assert_eq!(held(&store), [1, 2], "{case}");
                refuse(&disk);
                let written = write(&mut store, 5, 0x77);
                if mutants != 0 {
                    assert_eq!(written, Err(BlkError::Failed), "{case}");
                    continue;
                }
                assert_eq!(written, Ok(()), "{case}");
                assert_eq!(held(&store), [1], "{case}: cut 2 read 0x66, now gone");
                assert_eq!(store.cut_figures().abandoned, 1, "{case}");
                // Block 9 changed in no held cut's epochs: cut 1, the
                // whole volume, reads it, and goes.
                assert_eq!(write(&mut store, 9, 0x99), Ok(()), "{case}");
                assert_eq!(held(&store), Vec::<u64>::new(), "{case}");
                assert_eq!(store.cut_figures().abandoned, 2, "{case}");
                disk.clear_faults();
                assert_eq!(
                    read(&mut store, 5).unwrap(),
                    vec![0x77; BLOCK_USIZE],
                    "{case}"
                );
                assert_eq!(
                    read(&mut store, 9).unwrap(),
                    vec![0x99; BLOCK_USIZE],
                    "{case}"
                );
                // The volume serves on, and holds the next cut.
                group(&mut store, 300, 9, 90);
                assert_eq!(held(&store), [3], "{case}");
            }
        }
    }
}

/// The copies are written and read by direct I/O where the store's I/O
/// is (M2.2c): no page of the copies file is in the kernel's cache once
/// a copy is made, or once a held cut is read from it, so no copy waits
/// for a writeback whose failure can be lost (the review of #100, Q1).
/// A copy whose write the disk then fails lets go the cut that needs it
/// at that write, which goes on. Through the cache (the tests' I/O
/// only) the copy leaves its page: the count does see one. The defects,
/// each planted by hand (M2.2c's description): a copy written through
/// the cache, and one read through it.
#[test]
fn the_copies_are_written_and_read_past_the_cache() {
    let copies_pages = |disk: &SimDisk| -> u64 {
        disk.open(COPIES_NAME)
            .and_then(|file| file.cached_pages())
            .expect("the copies file's pages in the cache")
    };
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(36, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, io);
        write(&mut store, 5, 0x55).unwrap();
        // Cut 1, the whole volume (the state is new).
        group(&mut store, 100, 7, 70);
        let before = disk.writes_of(COPIES_NAME);
        // Copied for cut 1, which reads 0x55.
        write(&mut store, 5, 0x66).unwrap();
        assert_eq!(disk.writes_of(COPIES_NAME), before + 1, "{case}: one copy");
        let written = copies_pages(&disk);
        let mut buf = Buf::filled(1, 0xEE);
        assert_eq!(store.read_cut(1, 5, &mut buf).ok(), Some(true), "{case}");
        assert_eq!(buf.to_vec(), vec![0x55; BLOCK_USIZE], "{case}");
        let after_read = copies_pages(&disk);
        if io.direct {
            assert_eq!(written, 0, "{case}: the copy's write left a page");
            assert_eq!(after_read, 0, "{case}: the copy's read left a page");
        } else {
            assert!(
                written > 0,
                "{case}: a copy through the cache leaves its page"
            );
        }
        // The disk fails the next copy's write. Cut 1, the whole
        // volume, reads block 9, and goes at that write.
        disk.fail_write_of(COPIES_NAME);
        assert_eq!(write(&mut store, 9, 0x99), Ok(()), "{case}");
        assert!(store.held_cuts().is_empty(), "{case}");
        assert_eq!(store.cut_figures().abandoned, 1, "{case}");
        disk.clear_faults();
        assert_eq!(
            read(&mut store, 9).unwrap(),
            vec![0x99; BLOCK_USIZE],
            "{case}"
        );
    }
}

/// A write over a held cut copies its blocks in one read of the image
/// and one write of the copies file (M2.2d), not one of each a block
/// (M2.2c): 32 blocks, 128 KiB, the size of a ZFS record, every one of
/// which cut 1 reads. The cut reads each block as it was. The defect,
/// planted by hand: a copy a block.
///
/// Blocks 64 to 95 are the first half of label 1, outside every ring.
#[test]
fn a_write_over_a_held_cut_copies_its_blocks_in_one_read_and_one_write() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(60, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        for block in 64..96 {
            write(&mut store, block, block as u8).unwrap();
        }
        // Cut 1, the whole volume (the state is new).
        group(&mut store, 100, 7, 70);
        let reads = disk.reads_of(crate::store::HOME_NAME);
        let writes = disk.writes_of(COPIES_NAME);
        let done = run(
            &mut store,
            1,
            Op::Write {
                offset: 64 * BLOCK,
                data: Buf::filled(32, 0xAB),
            },
        );
        assert_eq!(done.result, Ok(()), "{case}");
        assert_eq!(
            disk.reads_of(crate::store::HOME_NAME),
            reads + 1,
            "{case}: one read of the image"
        );
        assert_eq!(
            disk.writes_of(COPIES_NAME),
            writes + 1,
            "{case}: one write of the copies"
        );
        assert_eq!(store.cut_figures().copied, 32 * BLOCK, "{case}");
        for block in 64..96 {
            let mut buf = Buf::filled(1, 0xEE);
            assert_eq!(
                store.read_cut(1, block, &mut buf).ok(),
                Some(true),
                "{case}"
            );
            assert_eq!(
                buf.to_vec(),
                vec![block as u8; BLOCK_USIZE],
                "{case}: block {block}"
            );
        }
    }
}

/// What a cut read in slices gave: each block with its content, a
/// block of a run of zeros with zeros, in order; and each slice's blocks
/// read with whether it said it read a copy.
type Sliced = (Vec<(u64, Vec<u8>)>, Vec<(Vec<u64>, bool)>);

/// Held cut `number` read in slices of `size` blocks from block 0 on.
fn sliced(store: &mut InPlace<SimDisk>, number: u64, size: usize) -> Sliced {
    let mut blocks = Vec::new();
    let mut slices = Vec::new();
    let mut buf = Buf::filled(size, 0xEE);
    let mut from = Some(0);
    while let Some(at) = from {
        let slice = store.read_cut_from(number, at, &mut buf).expect("reads");
        assert!(
            slice.blocks.len() <= size,
            "a slice of {} blocks in a buffer of {size}",
            slice.blocks.len()
        );
        assert!(
            !(slice.blocks.is_empty() && slice.zeroes.is_empty()),
            "an empty slice from {at}"
        );
        let mut got: Vec<(u64, Vec<u8>)> = slice
            .blocks
            .iter()
            .enumerate()
            .map(|(i, &block)| (block, buf[i * BLOCK_USIZE..(i + 1) * BLOCK_USIZE].to_vec()))
            .collect();
        got.extend(slice.zeroes.iter().flat_map(|&(first, count)| {
            (first..first + count).map(|block| (block, vec![0; BLOCK_USIZE]))
        }));
        got.sort_by_key(|(block, _)| *block);
        assert!(got.iter().all(|(block, _)| *block >= at));
        blocks.extend(got);
        slices.push((slice.blocks, slice.copied));
        from = slice.next;
    }
    (blocks, slices)
}

/// A held cut read in slices, as the upload reads it (M2.4), holds each
/// block as the cut reads it a block at a time: the whole volume of a
/// new state's first cut after writes over it that made copies, and the
/// second cut's few blocks, in slices of 1, 7 and 64 blocks. A slice
/// says where the map goes on, and whether it read a copy: exactly when
/// one of its blocks changed after the cut's instant. On a disk that
/// says where the image's holes are, cut 1 reads them as runs of zeros
/// (M2.4e); on one that cannot, it reads every block.
#[test]
fn a_cut_read_in_slices_holds_each_block_as_it_reads_alone() {
    let ios = [IoMode::default()].into_iter().chain(OTHER_IO);
    for (io, holes) in ios.flat_map(|io| [(io, true), (io, false)]) {
        let case = format!("{io:?}, holes {holes}");
        let disk = created(61, 0);
        disk.set_seek_data(holes);
        let mut store = start_on(&disk, 0, &region(), io);
        for block in 64..96 {
            write(&mut store, block, block as u8).unwrap();
        }
        // Cut 1, the whole volume (the state is new).
        group(&mut store, 100, 7, 70);
        // Copies for cut 1: a run, and two blocks alone.
        let mut changed: std::collections::BTreeSet<u64> = (70..80).chain([90, 5]).collect();
        for &block in &changed {
            write(&mut store, block, 0xC0).unwrap();
        }
        // Cut 2: those blocks and group 8's uberblocks, whose slots
        // cut 1 reads from copies too.
        group(&mut store, 200, 8, 80);
        changed.extend((0..LABELS).map(|label| (ring(label) + (8 % RING_SLOTS) * SLOT) / BLOCK));
        let phases: Vec<(u64, CutPhase)> = store
            .held_cuts()
            .iter()
            .map(|c| (c.number, c.phase))
            .collect();
        assert_eq!(
            phases,
            [(1, CutPhase::Uploading), (2, CutPhase::Waiting)],
            "{case}"
        );
        for number in [1, 2] {
            let alone: Vec<(u64, Vec<u8>)> = store
                .cut_map(number)
                .into_iter()
                .map(|block| {
                    let mut buf = Buf::filled(1, 0xEE);
                    assert_eq!(store.read_cut(number, block, &mut buf).ok(), Some(true));
                    (block, buf.to_vec())
                })
                .collect();
            assert_eq!(alone.len() == VOLUME_BLOCKS as usize, number == 1, "{case}");
            for size in [1, 7, 64] {
                let (blocks, slices) = sliced(&mut store, number, size);
                assert!(blocks == alone, "{case}: cut {number} in slices of {size}");
                for (blocks, copied) in slices {
                    let read_a_copy =
                        number == 1 && blocks.iter().any(|block| changed.contains(block));
                    assert_eq!(
                        copied, read_a_copy,
                        "{case}: cut {number}, the slice of {blocks:?}"
                    );
                }
            }
        }
        let zeros = store.cut_figures().hole_chunks;
        assert_eq!(zeros > 0, holes, "{case}: {zeros} hole chunks");
        assert_eq!(
            store
                .read_cut_from(1, 70, &mut Buf::filled(1, 0))
                .map(|s| s.blocks)
                .ok(),
            Some(vec![70]),
            "{case}: a slice from a block on"
        );
    }
}

/// Every slice of held cut `number` from its first block, each with the
/// runs of zeros it names.
fn zero_runs(store: &mut InPlace<SimDisk>, number: u64) -> Vec<(u64, u64)> {
    let mut runs = Vec::new();
    let mut buf = Buf::filled(16, 0xEE);
    let mut from = Some(0);
    while let Some(at) = from {
        let slice = store.read_cut_from(number, at, &mut buf).expect("reads");
        runs.extend(slice.zeroes);
        from = slice.next;
    }
    runs
}

/// A daemon that takes the device over between the state's beginning and
/// its first commit takes the state up with its hole set (M2.4e): the
/// set is kept whole, and the first cut still reads as the volume at its
/// instant: a block written into a hole before the instant as written,
/// one written into a hole after it as zeros, with no copy made for it,
/// and one written over data after it from its copy. The writes are
/// flushed before the death, so that the image holds them. Deaths at
/// every store of the state, and between a block's bit and its write,
/// are in `state/tests.rs`.
#[test]
fn a_take_over_before_the_first_commit_keeps_the_hole_set() {
    let disk = created(62, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    let begun = store.cut_figures();
    assert_eq!(begun.holes, CutHoles::Held);
    assert!(begun.hole_chunks > 0, "{begun:?}");
    for block in 64..96 {
        write(&mut store, block, block as u8).unwrap();
    }
    // Into a hole, before the instant.
    write(&mut store, 210, 0xD2).unwrap();
    group(&mut store, 100, 7, 70);
    let (instant, _) = sliced(&mut store, 1, 16);
    let block = |blocks: &[(u64, Vec<u8>)], at: u64| {
        blocks
            .iter()
            .find(|(b, _)| *b == at)
            .map(|(_, data)| data[0])
    };
    assert_eq!(block(&instant, 210), Some(0xD2));
    assert_eq!(block(&instant, 200), Some(0));
    // Into a hole and over data, after the instant.
    write(&mut store, 200, 0xC8).unwrap();
    write(&mut store, 70, 0x77).unwrap();
    assert_eq!(store.cut_figures().copied, BLOCK, "a copy of block 70 only");
    assert!(run(&mut store, 300, Op::Flush).result.is_ok());
    drop(store);
    disk.kill();

    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    assert_eq!(store.cuts_at_start(), Some(&CutsTakenUp::TakenUp));
    let taken = store.cut_figures();
    assert_eq!(
        (taken.holes, taken.hole_chunks),
        (CutHoles::Held, begun.hole_chunks)
    );
    let (after, _) = sliced(&mut store, 1, 16);
    assert!(after == instant, "cut 1 after the take-over");
    assert!(!zero_runs(&mut store, 1).is_empty());
}

/// A volume whose image has no holes (made with `create --zero-fill`)
/// has an empty hole set: its first cut reads every block of the image
/// with no run of zeros, and a write over a block of it that nothing had
/// written since the state began makes a copy, as before M2.4e. On a
/// sparse volume the same write, into a hole, makes none.
#[test]
fn a_volume_made_zero_filled_has_no_holes_and_copies_as_before() {
    for sparse in [true, false] {
        let case = if sparse { "sparse" } else { "zero-filled" };
        let disk = if sparse {
            created(63, 0)
        } else {
            let disk = SimDisk::new(63);
            disk.set_write_unit(BLOCK);
            disk.set_page_size(BLOCK);
            disk.set_block_size(Some(BLOCK));
            let config = VolumeConfig {
                volume: *b"in-place volume.",
                size_mib: 1,
                base: Base::Empty,
                write_unit: 4096,
            };
            create_zero_filled(&disk, &config).unwrap();
            disk
        };
        let mut store = start_on(&disk, 0, &region(), ON_CALLER);
        let figures = store.cut_figures();
        assert_eq!(figures.holes, CutHoles::Held, "{case}");
        assert_eq!(figures.hole_chunks == 0, !sparse, "{case}: {figures:?}");
        for block in 64..96 {
            write(&mut store, block, block as u8).unwrap();
        }
        group(&mut store, 100, 7, 70);
        write(&mut store, 200, 0xC8).unwrap();
        write(&mut store, 70, 0x77).unwrap();
        let copies = if sparse { 1 } else { 2 };
        assert_eq!(store.cut_figures().copied, copies * BLOCK, "{case}");
        let runs = zero_runs(&mut store, 1);
        assert_eq!(runs.is_empty(), !sparse, "{case}: {runs:?}");
        let (blocks, _) = sliced(&mut store, 1, 16);
        assert_eq!(blocks.len(), VOLUME_BLOCKS as usize, "{case}");
        let at = |b: u64| blocks[b as usize].1[0];
        assert_eq!((at(200), at(70), at(80)), (0, 70, 80), "{case}");
    }
}

/// The image's holes are taken in whole blocks of the volume (M2.4e):
/// on a file system of 512-byte blocks, one sector of data inside block
/// 16, written there before the store started, makes block 16 data, and
/// with it its chunk, and the first cut reads the sector. Block 16 is
/// the first of its chunk, so that the hole after the sector would hold
/// the whole chunk if a block were taken for a hole in part. An answer
/// of the file system that goes back before the offset asked gives no
/// set: the first cut then reads every block of the image, as before
/// M2.4e.
#[test]
fn the_holes_are_whole_blocks_and_an_answer_that_goes_back_gives_none() {
    for back in [false, true] {
        let disk = created(64, 0);
        disk.set_block_size_of(crate::store::HOME_NAME, Some(512))
            .unwrap();
        // A write of one sector writes no more than it: the disk's
        // write unit is a sector while it is made.
        disk.set_write_unit(512);
        let mut home = disk.open(crate::store::HOME_NAME).unwrap();
        home.write_at(16 * BLOCK + 512, &[0x5A; 512]).unwrap();
        drop(home);
        disk.set_write_unit(BLOCK);
        disk.set_seek_data_back(back);
        let mut store = start_on(&disk, 0, &region(), ON_CALLER);
        let figures = store.cut_figures();
        group(&mut store, 100, 7, 70);
        let runs = zero_runs(&mut store, 1);
        if back {
            assert_eq!(figures.holes, CutHoles::None);
            assert_eq!(figures.hole_chunks, 0);
            assert!(runs.is_empty(), "{runs:?}");
        } else {
            assert_eq!(figures.holes, CutHoles::Held);
            assert!(figures.hole_chunks > 0, "{figures:?}");
            assert!(!runs.is_empty());
            assert!(
                runs.iter()
                    .all(|&(first, n)| !(first..first + n).contains(&16)),
                "block 16 in a run of zeros: {runs:?}"
            );
        }
        let (blocks, _) = sliced(&mut store, 1, 16);
        let block = &blocks[16].1;
        assert_eq!(blocks[16].0, 16);
        assert!(block[..512].iter().all(|&b| b == 0), "back {back}");
        assert!(block[512..1024].iter().all(|&b| b == 0x5A), "back {back}");
    }
}

/// A slice of a held cut reads each run of the image, and each run of
/// copies in adjacent slots, in one read (M2.4, as M2.2d writes them):
/// 16 blocks unchanged since the cut and 16 written over it in one
/// write, in a slice of 32. The defect, planted by hand: a read a
/// block.
#[test]
fn a_slice_of_a_cut_reads_each_run_in_one_read() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(62, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        for block in 64..96 {
            write(&mut store, block, block as u8).unwrap();
        }
        // Cut 1, the whole volume (the state is new).
        group(&mut store, 100, 7, 70);
        let done = run(
            &mut store,
            1,
            Op::Write {
                offset: 80 * BLOCK,
                data: Buf::filled(16, 0xAB),
            },
        );
        assert_eq!(done.result, Ok(()), "{case}");
        let image = disk.reads_of(crate::store::HOME_NAME);
        let copies = disk.reads_of(COPIES_NAME);
        let mut buf = Buf::filled(32, 0xEE);
        let slice = store.read_cut_from(1, 64, &mut buf).expect("reads");
        assert_eq!(slice.blocks, (64..96).collect::<Vec<u64>>(), "{case}");
        assert_eq!(slice.next, Some(96), "{case}");
        assert!(slice.copied, "{case}");
        for (i, block) in (64..96).enumerate() {
            assert_eq!(
                buf[i * BLOCK_USIZE..(i + 1) * BLOCK_USIZE],
                vec![block as u8; BLOCK_USIZE],
                "{case}: block {block}"
            );
        }
        assert_eq!(
            disk.reads_of(crate::store::HOME_NAME),
            image + 1,
            "{case}: one read of the image"
        );
        assert_eq!(
            disk.reads_of(COPIES_NAME),
            copies + 1,
            "{case}: one read of the copies"
        );
    }
}

/// The phases of the held cuts, oldest first.
fn phases(store: &InPlace<SimDisk>) -> Vec<(u64, CutPhase)> {
    store
        .held_cuts()
        .iter()
        .map(|cut| (cut.number, cut.phase))
        .collect()
}

/// Held cut `number` read a block at a time.
fn alone(store: &mut InPlace<SimDisk>, number: u64) -> Vec<(u64, Vec<u8>)> {
    store
        .cut_map(number)
        .into_iter()
        .map(|block| {
            let mut buf = Buf::filled(1, 0xEE);
            assert_eq!(store.read_cut(number, block, &mut buf).ok(), Some(true));
            (block, buf.to_vec())
        })
        .collect()
}

/// Copies join a run only where their slots follow, whatever their
/// blocks do (review of M2.4b-1, G2): two blocks apart whose copies are
/// in adjacent slots are one read; two adjacent blocks whose copies are
/// the other way round, and adjacent blocks whose slots have a gap, are
/// not. Each block of each slice is what the cut reads alone, in slices
/// of 1 to 64 blocks. The defect, planted by hand: a copy two slots on
/// taken to follow.
#[test]
fn copies_are_one_read_only_where_their_slots_follow() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(73, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        for block in 128..160 {
            write(&mut store, block, block as u8).unwrap();
        }
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1));
        let own = [130u64, 132, 134, 135, 140, 141, 142, 150, 151];
        let content = |block: u64| vec![0x30 + (block - 130) as u8; BLOCK_USIZE];
        for block in own {
            write(&mut store, block, 0x30 + (block - 130) as u8).unwrap();
        }
        group(&mut store, 200, 8, 80);
        assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "{case}");
        assert!(!store.cut_is_whole(2), "{case}");
        // A copy each, in slots 0 to 6 in this order: 130 and 132 in
        // adjacent slots; 135 before 134; 140, 142, 141.
        for block in [130u64, 132, 135, 134, 140, 142, 141] {
            write(&mut store, block, 0xEE).unwrap();
        }
        // 150 and 151 in one write: their copies in slots 7 and 8.
        let done = run(
            &mut store,
            1,
            Op::Write {
                offset: 150 * BLOCK,
                data: Buf::filled(2, 0xEF),
            },
        );
        assert_eq!(done.result, Ok(()), "{case}");
        let alone = alone(&mut store, 2);
        for block in own {
            let read = alone.iter().find(|(b, _)| *b == block).map(|(_, c)| c);
            assert!(read == Some(&content(block)), "{case}: block {block} alone");
        }
        for size in [1, 2, 3, 4, 5, 16, 64] {
            let (blocks, _) = sliced(&mut store, 2, size);
            for ((block, got), (_, want)) in blocks.iter().zip(&alone) {
                assert!(got == want, "{case}: block {block} in slices of {size}");
            }
            assert!(blocks == alone, "{case}: cut 2 in slices of {size}");
        }
        let copies = disk.reads_of(COPIES_NAME);
        store
            .read_cut_from(2, 0, &mut Buf::filled(64, 0xEE))
            .expect("reads");
        // 130 and 132 in one read; 134, 135, 140, 141 and 142 a read
        // each; 150 and 151 in one read.
        assert_eq!(
            disk.reads_of(COPIES_NAME) - copies,
            7,
            "{case}: the reads of the copies"
        );
    }
}

/// A block of the image and a copy are never one read, though the copy's
/// slot is the block's number plus one, or the other way round (review
/// of M2.4b-1, G2). The defect, planted by hand: a copy taken to follow
/// a block of the image whose number its slot follows.
#[test]
fn a_copy_never_follows_a_block_of_the_image() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(75, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        let own = [0u64, 1, 2, 200, 201];
        for block in own {
            write(&mut store, block, 0x10 + block as u8).unwrap();
        }
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1));
        let content = |block: u64| vec![0x40 + (block % 64) as u8; BLOCK_USIZE];
        for block in own {
            write(&mut store, block, 0x40 + (block % 64) as u8).unwrap();
        }
        group(&mut store, 200, 8, 80);
        assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "{case}");
        // Copies: block 0 in slot 0, block 200 in slot 1, block 2 in slot
        // 2; blocks 1 and 201 stay in the image. In the map's order: copy
        // 0, image 1, copy 2, ..., copy 1, image 201.
        for block in [0u64, 200, 2] {
            write(&mut store, block, 0xEE).unwrap();
        }
        let map = store.cut_map(2);
        for size in [1, 2, 3, 64] {
            let (blocks, _) = sliced(&mut store, 2, size);
            assert_eq!(blocks.len(), map.len(), "{case}");
            for (block, got) in blocks {
                if own.contains(&block) {
                    assert!(
                        got == content(block),
                        "{case}: block {block} of cut 2 in slices of {size} holds {:#x}",
                        got[0]
                    );
                }
            }
        }
    }
}

/// Blocks of the image join a run only where their numbers follow
/// (review of M2.4b-1, G2): a cut that holds blocks 10, 12, 14, 15 and
/// 17, none changed since, reads only 14 and 15 in one read. The
/// defect, planted by hand: a block two on taken to follow.
#[test]
fn blocks_of_the_image_are_one_read_only_where_their_numbers_follow() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(76, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        for block in 8u64..20 {
            write(&mut store, block, 0x60 + block as u8).unwrap();
        }
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1));
        let own = [10u64, 12, 14, 15, 17];
        for block in own {
            write(&mut store, block, 0x20 + block as u8).unwrap();
        }
        group(&mut store, 200, 8, 80);
        assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "{case}");
        let map = store.cut_map(2);
        for size in [1, 2, 3, 64] {
            let (blocks, slices) = sliced(&mut store, 2, size);
            assert_eq!(blocks.len(), map.len(), "{case}");
            assert!(slices.iter().all(|(_, copied)| !copied), "{case}");
            for (block, got) in blocks {
                if own.contains(&block) {
                    assert!(
                        got == vec![0x20 + block as u8; BLOCK_USIZE],
                        "{case}: block {block} of cut 2 in slices of {size} holds {:#x}",
                        got[0]
                    );
                }
            }
        }
        let image = disk.reads_of(crate::store::HOME_NAME);
        store
            .read_cut_from(2, 0, &mut Buf::filled(64, 0xEE))
            .expect("reads");
        assert_eq!(
            (disk.reads_of(crate::store::HOME_NAME) - image) as usize,
            map.len() - 1,
            "{case}: only 14 and 15 are one read, of {map:?}"
        );
    }
}

/// A slice is refused for a cut that is not held, and for a buffer of
/// no block, which abandons nothing; a slice from the volume's end on
/// reads nothing and says the map is read. A buffer of part of a block,
/// or off a block's alignment, is not a `Buf` (review of M2.4b-1, N1).
#[test]
fn a_slice_of_a_cut_not_held_or_into_a_short_buffer_is_refused() {
    let disk = created(63, 0);
    let mut store = start_on(&disk, 0, &region(), ON_CALLER);
    group(&mut store, 100, 7, 70);
    let refused = |store: &mut InPlace<SimDisk>, number: u64, buf: &mut Buf| {
        store.read_cut_from(number, 0, buf).err().map(|e| e.kind())
    };
    assert_eq!(
        refused(&mut store, 2, &mut Buf::filled(1, 0)),
        Some(io::ErrorKind::NotFound)
    );
    assert_eq!(
        refused(&mut store, 1, &mut Buf::zeroed(0)),
        Some(io::ErrorKind::InvalidInput)
    );
    assert_eq!(
        store.held_cuts().first().map(|cut| cut.phase),
        Some(CutPhase::Uploading),
        "a refused buffer abandons nothing"
    );
    let end = store
        .read_cut_from(1, VOLUME_BLOCKS, &mut Buf::filled(1, 0))
        .expect("reads");
    assert!(end.blocks.is_empty() && end.next.is_none() && !end.copied);
    assert!(store.release_cut(1));
    assert_eq!(
        refused(&mut store, 1, &mut Buf::filled(1, 0)),
        Some(io::ErrorKind::NotFound)
    );
}

/// Cut 1, the whole volume, uploading, with a copy of block 70 made for
/// it, and cut 2 waiting behind it with block 70's new content.
fn two_cuts_with_a_copy(seed: u64, io: IoMode) -> (SimDisk, InPlace<SimDisk>) {
    let disk = created(seed, 0);
    let mut store = start_on(&disk, 0, &region(), io);
    write(&mut store, 70, 0x70).unwrap();
    group(&mut store, 100, 7, 70);
    write(&mut store, 70, 0xC0).unwrap();
    group(&mut store, 200, 8, 80);
    let phases: Vec<(u64, CutPhase)> = store
        .held_cuts()
        .iter()
        .map(|c| (c.number, c.phase))
        .collect();
    assert_eq!(phases, [(1, CutPhase::Uploading), (2, CutPhase::Waiting)]);
    (disk, store)
}

/// The upload syncs the copies file before it commits a cut it read
/// copies for (M2.4, the review of M2.2b-1), on the file's helper: the
/// sync covers the copy, and a cut whose sync fails is abandoned. Its
/// map goes to the cut that waits, which reads every block it read, as
/// at its own instant.
#[test]
fn a_failed_sync_of_the_copies_abandons_the_cut_being_uploaded() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let (disk, mut store) = two_cuts_with_a_copy(64, io);
        let slice = store
            .read_cut_from(1, 70, &mut Buf::filled(1, 0))
            .expect("reads");
        assert!(slice.copied, "{case}");
        let syncs = disk.syncs_of(COPIES_NAME);
        assert_eq!(
            store.sync_cut_copies(1).map_err(|e| e.kind()),
            Ok(()),
            "{case}"
        );
        assert_eq!(
            store
                .cut_copies_synced(true)
                .map(|r| r.map_err(|e| e.kind())),
            Some(Ok(())),
            "{case}"
        );
        assert_eq!(disk.syncs_of(COPIES_NAME), syncs + 1, "{case}: one sync");
        assert_eq!(
            store.held_cuts().len(),
            2,
            "{case}: a sync that succeeds abandons nothing"
        );
        disk.fail_sync_of(COPIES_NAME);
        assert_eq!(
            store.sync_cut_copies(1).map_err(|e| e.kind()),
            Ok(()),
            "{case}"
        );
        assert!(
            matches!(store.cut_copies_synced(true), Some(Err(_))),
            "{case}"
        );
        disk.clear_faults();
        let held: Vec<(u64, CutPhase)> = store
            .held_cuts()
            .iter()
            .map(|c| (c.number, c.phase))
            .collect();
        assert_eq!(held, [(2, CutPhase::Uploading)], "{case}");
        assert_eq!(store.cut_figures().abandoned, 1, "{case}");
        assert_eq!(
            store.cut_map(2).len() as u64,
            VOLUME_BLOCKS,
            "{case}: cut 2 took the whole volume"
        );
        assert_eq!(
            cut_block(&mut store, 2, 70),
            vec![0xC0; BLOCK_USIZE],
            "{case}"
        );
        assert_eq!(
            cut_block(&mut store, 2, 3),
            vec![0x33; BLOCK_USIZE],
            "{case}"
        );
    }
}

/// A sync of the copies is refused for a cut that is not the one being
/// uploaded, and while one is under way; its end with none begun says
/// so. A failed sync's end abandons nothing once its cut is gone.
#[test]
fn a_sync_of_the_copies_is_for_the_cut_being_uploaded_one_at_a_time() {
    let (disk, mut store) = two_cuts_with_a_copy(65, ON_CALLER);
    let kind = |r: io::Result<()>| r.map_err(|e| e.kind());
    assert!(store.cut_copies_synced(true).is_none(), "none begun");
    assert_eq!(
        kind(store.sync_cut_copies(2)),
        Err(io::ErrorKind::InvalidInput)
    );
    assert_eq!(
        kind(store.sync_cut_copies(3)),
        Err(io::ErrorKind::InvalidInput)
    );
    disk.hold_syncs_of(COPIES_NAME);
    assert_eq!(kind(store.sync_cut_copies(1)), Ok(()));
    assert_eq!(
        kind(store.sync_cut_copies(1)),
        Err(io::ErrorKind::InvalidInput)
    );
    assert!(store.cut_copies_synced(false).is_none(), "under way");
    disk.fail_sync_of(COPIES_NAME);
    // Cut 1 goes before the sync ends: released, as after its commit.
    assert!(store.release_cut(1));
    disk.release_syncs();
    assert!(matches!(store.cut_copies_synced(true), Some(Err(_))));
    disk.clear_faults();
    let held: Vec<(u64, CutPhase)> = store
        .held_cuts()
        .iter()
        .map(|c| (c.number, c.phase))
        .collect();
    assert_eq!(held, [(2, CutPhase::Uploading)]);
    assert_eq!(store.cut_figures().abandoned, 0);
}

/// A read for the cut being uploaded that fails abandons it (M2.4), as
/// a failed sync of its copies does; a read of a cut that waits fails
/// and keeps it.
#[test]
fn a_failed_read_of_the_cut_being_uploaded_abandons_it() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let (disk, mut store) = two_cuts_with_a_copy(66, io);
        disk.fail_read_of(crate::store::HOME_NAME);
        let failed = store.read_cut_from(2, 70, &mut Buf::filled(1, 0));
        assert!(
            failed.is_err_and(|e| e.kind() != io::ErrorKind::NotFound),
            "{case}"
        );
        assert_eq!(store.held_cuts().len(), 2, "{case}: cut 2 waits on");
        let failed = store.read_cut_from(1, 0, &mut Buf::filled(8, 0));
        assert!(
            failed.is_err_and(|e| e.kind() != io::ErrorKind::NotFound),
            "{case}"
        );
        disk.clear_faults();
        let held: Vec<(u64, CutPhase)> = store
            .held_cuts()
            .iter()
            .map(|c| (c.number, c.phase))
            .collect();
        assert_eq!(held, [(2, CutPhase::Uploading)], "{case}");
        assert_eq!(store.cut_figures().abandoned, 1, "{case}");
        assert_eq!(
            cut_block(&mut store, 2, 3),
            vec![0x33; BLOCK_USIZE],
            "{case}"
        );
    }
}

/// Where the copies file takes no direct I/O, a copy is never written
/// through the cache instead (M2.2c's reading 2, the review of #104,
/// S1): the copy fails, the cut that needs it goes, and the client's
/// write goes on. Nothing reaches the copies file, so no page of it
/// waits in the cache for a writeback. The defect, planted by hand: a
/// copy that falls back to the cache when its direct write is refused,
/// which keeps cut 1 and leaves a page.
#[test]
fn a_copy_the_file_system_will_not_write_directly_is_not_written_through_the_cache() {
    for io in [IoMode::default(), ON_CALLER] {
        let case = format!("{io:?}");
        let disk = created(34, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, io);
        write(&mut store, 5, 0x55).unwrap();
        // Cut 1, the whole volume (the state is new).
        group(&mut store, 100, 7, 70);
        disk.refuse_direct_io_of(COPIES_NAME);
        let before = disk.writes_of(COPIES_NAME);
        assert_eq!(write(&mut store, 5, 0x66), Ok(()), "{case}");
        assert!(store.held_cuts().is_empty(), "{case}: cut 1 is let go");
        assert_eq!(store.cut_figures().abandoned, 1, "{case}");
        assert_eq!(store.cut_figures().copied, 0, "{case}");
        assert_eq!(
            disk.writes_of(COPIES_NAME),
            before,
            "{case}: the copy was written another way"
        );
        let pages = disk
            .open(COPIES_NAME)
            .map_or(0, |file| file.cached_pages().expect("the count"));
        assert_eq!(pages, 0, "{case}: a page of the copies file is cached");
        disk.clear_faults();
        assert_eq!(
            read(&mut store, 5).unwrap(),
            vec![0x66; BLOCK_USIZE],
            "{case}"
        );
    }
}

/// The same with cut 1 kept: block 5 changed after cut 1's instant
/// and is copied for it before the copies are refused, so cut 1 still
/// reads 0x55 once cut 2 has gone, and its copies are not freed.
#[test]
fn a_cut_older_than_the_one_a_refused_copy_needs_reads_on() {
    for io in [IoMode::default(), ON_CALLER] {
        let disk = created(35, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, io);
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        write(&mut store, 5, 0x66).unwrap();
        group(&mut store, 200, 8, 80);
        disk.fail_write_of(COPIES_NAME);
        assert_eq!(write(&mut store, 5, 0x77), Ok(()), "{io:?}");
        let mut buf = Buf::filled(1, 0xEE);
        assert_eq!(store.read_cut(1, 5, &mut buf).ok(), Some(true), "{io:?}");
        assert_eq!(buf.to_vec(), vec![0x55; BLOCK_USIZE], "{io:?}");
        assert!(
            store.read_cut(2, 5, &mut buf).is_err(),
            "{io:?}: cut 2 is gone"
        );
    }
}

/// A start takes up the cut state the daemon before it left only where
/// it can trust it (M2.2b): in the same boot, after a sync of the copies
/// that succeeded, and when it does not acknowledge a failure; on a
/// device taken over or created. Otherwise it begins a new state, holds
/// no cut, and says why. Each rule's absence is noticed.
#[test]
fn a_held_cut_is_taken_up_only_where_its_state_is_trusted() {
    let nothing = |_: &SimDisk| {};
    let created_device = Coming {
        taken_over: false,
        ..TAKING_OVER
    };
    for coming in [TAKING_OVER, created_device] {
        let (taken, held, five) = after_a_loss(0, coming, &nothing);
        assert_eq!(taken, CutsTakenUp::TakenUp, "{coming:?}");
        assert_eq!(held, [1], "{coming:?}");
        assert_eq!(five, Some(vec![0x55; BLOCK_USIZE]), "{coming:?}");
    }
    let mut another_boot = TAKING_OVER;
    another_boot.boot[0] ^= 1;
    let acknowledging = Coming {
        acknowledging: true,
        ..TAKING_OVER
    };
    let unsynced = |disk: &SimDisk| disk.fail_sync_of(COPIES_NAME);
    let removed = |disk: &SimDisk| disk.remove(COPIES_NAME).expect("the copies file goes");
    /// A case: its name, the mutant that plants its rule away, how the
    /// start comes, what happened before it, and what the start says.
    type Case<'a> = (&'a str, u128, Coming, &'a dyn Fn(&SimDisk), &'a str);
    let cases: [Case; 4] = [
        (
            "another boot",
            Mutants::BOOT_UNCHECKED,
            another_boot,
            &nothing,
            "begun in another boot",
        ),
        (
            "a failure acknowledged",
            Mutants::ACKNOWLEDGED_TRUSTED,
            acknowledging,
            &nothing,
            "acknowledges a failure",
        ),
        (
            "a failed sync of the copies",
            Mutants::COPIES_SYNC_UNCHECKED,
            TAKING_OVER,
            &unsynced,
            "could not be synced",
        ),
        (
            "no copies file",
            Mutants::COPIES_SYNC_UNCHECKED,
            TAKING_OVER,
            &removed,
            "the copies file is missing",
        ),
    ];
    for (name, mutant, coming, between, says) in cases {
        let (taken, held, _) = after_a_loss(0, coming, between);
        match taken {
            CutsTakenUp::Begun(why) => assert!(why.contains(says), "{name}: {why}"),
            CutsTakenUp::TakenUp => panic!("{name}: the state was taken up"),
        }
        assert!(held.is_empty(), "{name}: {held:?}");
        let (taken, held, _) = after_a_loss(mutant, coming, between);
        assert_eq!(
            (taken, held),
            (CutsTakenUp::TakenUp, vec![1]),
            "{name}: not noticed"
        );
    }
}

/// A start on a device it took over keeps the mark the state holds, so
/// that the flush ZFS sends after the marked write cuts its group; a
/// start on a created device drops it, since no client of the old
/// device can send that flush to the new one (M2.2b). Keeping it is
/// noticed.
#[test]
fn a_start_on_a_created_device_keeps_no_mark() {
    let after_the_next_flush = |mutants: u128, taken_over: bool| -> Vec<u64> {
        let disk = created(33, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, ON_CALLER);
        group(&mut store, 100, 7, 70);
        assert!(run(&mut store, 200, uberblock_write(0, 8, 80))
            .result
            .is_ok());
        drop(store);
        disk.kill();
        let coming = Coming {
            taken_over,
            ..TAKING_OVER
        };
        let mut store = start_as(&disk, mutants, &region, ON_CALLER, coming);
        assert!(run(&mut store, 201, Op::Flush).result.is_ok());
        store.held_cuts().iter().map(|c| c.state.txg).collect()
    };
    assert_eq!(after_the_next_flush(0, true), [7, 8]);
    assert_eq!(after_the_next_flush(0, false), [7]);
    assert_eq!(
        after_the_next_flush(Mutants::MARK_KEPT_ON_CREATED, false),
        [7, 8],
        "a mark kept on a created device is not noticed"
    );
}

/// A region the open cannot have refuses the open by its reason and
/// with its name, before anything of the volume is written: a volume
/// marked as failed keeps its marker although the open was told to
/// acknowledge it.
#[test]
fn a_region_that_cannot_be_had_refuses_the_open() {
    let disk = created(34, 0);
    disk.create(crate::volume::MARKER_NAME)
        .and_then(|mut marker| marker.sync())
        .expect("the marker");
    let missing = std::env::temp_dir()
        .join("skyzfs-no-such-directory")
        .join("cuts.state");
    let options = OpenOptions {
        acknowledge_failure: true,
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Named(missing),
            boot: state::tests::BOOT,
            bound: Some(TEST_BOUND),
            intent_region: Some(TEST_INTENT_REGION),
            intent_header_sync: false,
        }),
        ..OpenOptions::default()
    };
    let Err((error, _lock)) = Opened::open(disk.clone(), lock(&disk).expect("the lock"), options)
    else {
        panic!("the open went on without its region");
    };
    match &error {
        OpenError::CutRegion(why) => assert!(why.contains("skyzfs-no-such-directory"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(disk
        .list()
        .expect("the names")
        .iter()
        .any(|name| name == crate::volume::MARKER_NAME));
}

/// A volume made again where one was, whose run directory and so whose
/// region is the same, does not take up the old volume's cuts: the
/// state carries the volume's identity (M2.2b). Not checking it is
/// noticed. The start token's rule is planted away in both runs: the
/// new volume here has none of the old one's tokens, which the token's
/// rule alone would refuse; the identity stands for a volume made again
/// in a directory that kept the old token's file.
#[test]
fn a_volume_made_again_does_not_take_up_the_cuts_of_the_one_before() {
    let next_volume = |mutants: u128| -> (CutsTakenUp, Vec<u64>) {
        let region = region();
        let old = created(35, 0);
        let mut store = start_on(&old, 0, &region, ON_CALLER);
        group(&mut store, 100, 7, 70);
        drop(store);
        old.kill();
        let new = created(36, 0);
        let store = start_on(&new, mutants, &region, ON_CALLER);
        let held = store.held_cuts().iter().map(|c| c.state.txg).collect();
        (store.cuts_at_start().cloned().expect("held cuts"), held)
    };
    match next_volume(Mutants::START_UNCHECKED) {
        (CutsTakenUp::Begun(why), held) => {
            assert!(why.contains("another volume"), "{why}");
            assert!(held.is_empty(), "{held:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        next_volume(Mutants::IDENTITY_UNCHECKED | Mutants::START_UNCHECKED),
        (CutsTakenUp::TakenUp, vec![7]),
        "a state of another volume taken up is not noticed"
    );
}

/// A start that could not read a ring goes on, and no cut opens as
/// itself until that ring has been written again (`read_rings`; the
/// review of #95, its S2, whose reproducer this is). The
/// start's reads are failed one at a time: among those after which the
/// store still starts, exactly four leave the first cut a rewind, the
/// four rings. Fails where a ring that could not be read is taken as
/// read (plant C4: every such start says `itself`).
#[test]
fn a_ring_the_start_could_not_read_leaves_no_cut_itself() {
    let mut rewinds = 0;
    let mut starts = 0;
    for after in 0..64 {
        let disk = created(19, 0);
        disk.fail_read(after, false);
        let Ok(opened) = opened(&disk, false, IoMode::default()) else {
            continue;
        };
        let Ok(mut store) = opened.start_with(None, Mutants(0)) else {
            continue;
        };
        disk.clear_faults();
        starts += 1;
        let mut id = 0;
        for label in 0..LABELS {
            id += 1;
            assert!(run(&mut store, id, uberblock_write(label, 10, 1))
                .result
                .is_ok());
        }
        id += 1;
        assert!(run(&mut store, id, Op::Flush).result.is_ok());
        match newest(&store) {
            Some((10, true)) => {}
            Some((10, false)) => rewinds += 1,
            other => panic!("after {after}: {other:?}"),
        }
        // The store says which ring it could not read: one where the
        // cut is a rewind, none where it is itself (S3).
        let unread = store.rings_unread().len();
        assert_eq!(
            unread,
            usize::from(newest(&store) == Some((10, false))),
            "after {after}: {:?}",
            store.rings_unread()
        );
    }
    assert!(starts > 4, "the starts that went on: {starts}");
    assert_eq!(rewinds, 4, "one start for each ring that could not be read");
}

/// A write the store received and ended without applying it (refused,
/// as a read-only volume refuses one) changes nothing: it does not mark,
/// and the view keeps what the volume holds, so the flush after it
/// counts no cut and the newest cut still opens as itself (the review
/// of #95, its S6: a rule that applied the change at its end passed
/// every test).
#[test]
fn a_write_ended_without_being_applied_changes_nothing() {
    use crate::inplace::cuts::CutRule;
    let mut rule = CutRule::new(1 << 20, Mutants(0));
    for (offset, len) in rule.rings() {
        rule.read_at_start(offset, &vec![0; len as usize]);
    }
    let mut id = 0;
    let mut group = |rule: &mut CutRule, txg: u64, root: u64, applied: bool| {
        for label in 0..LABELS {
            id += 1;
            rule.received(RequestId(id), &uberblock_write(label, txg, root));
            if applied {
                rule.applied(RequestId(id));
            }
            rule.ended(RequestId(id));
        }
        id += 1;
        rule.received(RequestId(id), &Op::Flush);
        rule.barrier(RequestId(id));
        rule.ended(RequestId(id));
    };
    let newest = |rule: &CutRule| {
        let cuts = rule.cuts();
        (cuts.count, cuts.newest.map(|cut| (cut.group, cut.itself)))
    };
    group(&mut rule, 10, 1, true);
    assert_eq!(newest(&rule), (1, Some((10, true))));
    group(&mut rule, 11, 2, false);
    assert_eq!(
        newest(&rule),
        (1, Some((10, true))),
        "writes ended unapplied marked or changed the view"
    );
    group(&mut rule, 11, 2, true);
    assert_eq!(newest(&rule), (2, Some((11, true))));
}

/// Bytes ZFS would pass over that lie in a ring when the store starts
/// are not in view either: the check applies to the rings read at the
/// start as it does to a write (work package M2.1c). The first
/// attachment writes group 10 and then forged bytes of group 99 into
/// label 1's slot, with no checksum; the next starts on that volume and
/// group 11's cut opens as itself, as an import would open group 11. A
/// start that takes the forged bytes for an uberblock holds group 99 as
/// the greatest in view, and every cut says it opens only with a rewind
/// until that slot is written again. From the review of #96, its S2.
#[test]
fn bytes_zfs_would_pass_over_at_the_start_are_not_in_view() {
    let disk = created(19, 0);
    let mut id = 0;
    let mut step = |store: &mut InPlace<SimDisk>, op: Op| {
        id += 1;
        assert!(run(store, id, op).result.is_ok());
    };
    let mut store = start(&disk, 0, false).unwrap();
    for label in 0..LABELS {
        step(&mut store, uberblock_write(label, 10, 1));
    }
    step(
        &mut store,
        Op::Write {
            offset: ring(1) + (99 % RING_SLOTS) * SLOT,
            data: uberblock(99, 9),
        },
    );
    step(&mut store, Op::Flush);
    assert_eq!(newest(&store), Some((10, true)));
    drop(store);
    let mut store = start(&disk, 0, false).unwrap();
    for label in 0..LABELS {
        step(&mut store, uberblock_write(label, 11, 2));
    }
    step(&mut store, Op::Flush);
    assert_eq!(newest(&store), Some((11, true)));
}

/// The store follows a slot by all its bytes (the review of #96, its
/// S1, and Codex's finding there), with slots of 8 KiB, as a pool of
/// `ashift` 13 writes them, and a writer other than ZFS writing half of
/// one:
///
/// 1. group 10 written whole to the four labels is cut, and opens as
///    itself;
/// 2. group 11 written whole to label 0 alone, then the second half of
///    that slot zeroed before the flush: on the volume the slot no
///    longer verifies and an import opens group 10, so cut 11 does not
///    open as itself;
/// 3. group 27, whose slot is 11's, written whole to the four labels:
///    that place is known again, and the cut opens as itself;
/// 4. on another volume, group 11 written whole to labels 1 to 3, and
///    group 12 to label 0 in two halves, head then tail: on the volume
///    label 0 holds a valid 12, greater than the cut's 11, so cut 11
///    does not open as itself.
///
/// Steps 2 and 4 fail where the view follows a slot by its first bytes
/// alone, as at 8da0a73: they say `itself`.
#[test]
fn a_slot_is_followed_by_all_its_bytes() {
    const BIG: u64 = 8 * KIB;
    let at =
        |label: usize, txg: u64| ring(label) + (txg % (skyzfs_uberblock::RING_SIZE / BIG)) * BIG;
    let whole = |label: usize, txg: u64| {
        let mut data = Buf::filled(2, 0);
        data[..BLOCK_USIZE].copy_from_slice(&uberblock(txg, txg));
        slot_write(at(label, txg), data)
    };
    let mut id = 0;
    let mut step = |store: &mut InPlace<SimDisk>, op: Op| {
        id += 1;
        assert!(run(store, id, op).result.is_ok());
    };
    let disk = created(23, 0);
    let mut store = start(&disk, 0, false).unwrap();
    for label in 0..LABELS {
        step(&mut store, whole(label, 10));
    }
    step(&mut store, Op::Flush);
    assert_eq!(newest(&store), Some((10, true)));
    step(&mut store, whole(0, 11));
    step(
        &mut store,
        Op::WriteZeroes {
            offset: at(0, 11) + BLOCK,
            len: BLOCK,
        },
    );
    step(&mut store, Op::Flush);
    assert_eq!(
        newest(&store),
        Some((11, false)),
        "a slot torn after its uberblock was applied was still taken whole"
    );
    for label in 0..LABELS {
        step(&mut store, whole(label, 27));
    }
    step(&mut store, Op::Flush);
    assert_eq!(newest(&store), Some((27, true)));
    drop(store);

    let disk = created(29, 0);
    let mut store = start(&disk, 0, false).unwrap();
    for label in 1..LABELS {
        step(&mut store, whole(label, 11));
    }
    let Op::Write { offset, data } = whole(0, 12) else {
        unreachable!("a write")
    };
    for half in 0..2 {
        let bytes = &data[half * BLOCK_USIZE..(half + 1) * BLOCK_USIZE];
        step(
            &mut store,
            Op::Write {
                offset: offset + half as u64 * BLOCK,
                data: Buf::from_blocks(bytes).expect("one block"),
            },
        );
    }
    step(&mut store, Op::Flush);
    assert_eq!(
        newest(&store),
        Some((11, false)),
        "a slot written in two halves was taken for an empty place"
    );
}

/// A slot whose fields are known not to verify is followed by the rest
/// of its bytes too (the next look of the review of #96, and Codex's
/// second finding there). Group 11 is written whole to labels 1 to 3.
/// Label 0 gets an 8 KiB slot of group 12 whose second half is damaged,
/// in one write that holds it whole: its place is known and holds no
/// uberblock. Then a write of the second half alone restores the slot:
/// on the volume label 0 now holds a valid 12, greater than the cut's
/// 11, so cut 11 does not open as itself. Fails where the view forgets
/// fields that did not verify, as at 110d2b6: it says `itself`.
#[test]
fn a_slot_known_not_to_verify_is_followed_by_the_rest_of_its_bytes() {
    const BIG: u64 = 8 * KIB;
    let at =
        |label: usize, txg: u64| ring(label) + (txg % (skyzfs_uberblock::RING_SIZE / BIG)) * BIG;
    let whole = |label: usize, txg: u64| {
        let mut data = Buf::filled(2, 0);
        data[..BLOCK_USIZE].copy_from_slice(&uberblock(txg, txg));
        slot_write(at(label, txg), data)
    };
    let mut id = 0;
    let mut step = |store: &mut InPlace<SimDisk>, op: Op| {
        id += 1;
        assert!(run(store, id, op).result.is_ok());
    };
    let disk = created(31, 0);
    let mut store = start(&disk, 0, false).unwrap();
    for label in 1..LABELS {
        step(&mut store, whole(label, 11));
    }
    let Op::Write { offset, data } = whole(0, 12) else {
        unreachable!("a write")
    };
    let mut damaged = data.clone();
    damaged[BLOCK_USIZE..].fill(0);
    step(
        &mut store,
        Op::Write {
            offset,
            data: damaged,
        },
    );
    step(
        &mut store,
        Op::Write {
            offset: offset + BLOCK,
            data: Buf::from_blocks(&data[BLOCK_USIZE..]).expect("one block"),
        },
    );
    step(&mut store, Op::Flush);
    assert_eq!(
        newest(&store),
        Some((11, false)),
        "a slot completed by its second half was taken for an empty place"
    );
}

/// What a death at store `death` of group 8's flush leaves, after the
/// next instance has had the flush again: whether the run died, the
/// count, and the held cuts' numbers and groups.
fn lost_in_a_cut(death: u64) -> (bool, u64, Vec<(u64, u64)>) {
    state::tests::quiet_deaths();
    let disk = created(33, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    group(&mut store, 100, 7, 70);
    for label in 0..LABELS {
        let done = run(
            &mut store,
            200 + label as u64,
            uberblock_write(label, 8, 80),
        );
        assert!(done.result.is_ok());
    }
    let id = RequestId(210);
    let mut done = Vec::new();
    region.die_at(death);
    let lived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store.submit(Request { id, op: Op::Flush }).unwrap();
        while done.is_empty() {
            store.wait(&mut done, WAIT);
        }
    }));
    region.revive();
    let died = match lived {
        Ok(()) => false,
        Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => true,
        Err(payload) => std::panic::resume_unwind(payload),
    };
    if died {
        drop(store);
        disk.kill();
        store = start_on(&disk, 0, &region, ON_CALLER);
        // The flush's completion never went out: the kernel issues it again.
        assert!(run(&mut store, 211, Op::Flush).result.is_ok());
    }
    assert!(run(&mut store, 212, Op::Flush).result.is_ok());
    let held = store
        .held_cuts()
        .iter()
        .map(|c| (c.number, c.state.txg))
        .collect();
    (died, store.cuts().count, held)
}

/// A death at any store of a cut's flush, through the store: the
/// kernel issues the flush again, and the group is cut once. Fails where
/// the start keeps a mark the newest cut took, or leaves the newest
/// cut's state unmade, or where the next instance is not given the
/// count (the review of #98, S2).
#[test]
fn a_death_at_any_store_of_a_cuts_flush_cuts_the_group_once() {
    let mut death = 0;
    loop {
        let (died, count, held) = lost_in_a_cut(death);
        assert_eq!(
            (count, held.clone()),
            (2, vec![(1, 7), (2, 8)]),
            "death at store {death}: count {count}, held {held:?}"
        );
        if !died {
            break;
        }
        death += 1;
    }
    assert!(death > 20, "{death} stores");
}

/// ZFS writes labels 0 and 2 and flushes, which cuts the group; the
/// daemon is lost; ZFS writes labels 1 and 3 and flushes. The repeat is
/// not cut again. Fails where the next instance is not given the newest
/// cut's state (the review of #98, S2).
#[test]
fn a_repeat_of_the_newest_cut_marks_nothing_after_a_lost_daemon() {
    let disk = created(35, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    group(&mut store, 100, 7, 70);
    for (i, label) in [0usize, 2].into_iter().enumerate() {
        let done = run(&mut store, 200 + i as u64, uberblock_write(label, 8, 80));
        assert!(done.result.is_ok());
    }
    assert!(run(&mut store, 210, Op::Flush).result.is_ok());
    assert_eq!(store.cuts().count, 2);
    drop(store);
    disk.kill();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    for (i, label) in [1usize, 3].into_iter().enumerate() {
        let done = run(&mut store, 220 + i as u64, uberblock_write(label, 8, 80));
        assert!(done.result.is_ok());
    }
    assert!(run(&mut store, 230, Op::Flush).result.is_ok());
    let held: Vec<(u64, u64)> = store
        .held_cuts()
        .iter()
        .map(|c| (c.number, c.state.txg))
        .collect();
    assert_eq!(
        store.cuts().count,
        2,
        "a repeat of the newest cut's uberblock was cut: {held:?}"
    );
}

/// A write-zeroes of two blocks after a cut, with every I/O the store
/// has: the cut reads both blocks as they were. Fails where a
/// write-zeroes does not pass through the cut state, and where only a
/// change's first block is noted (the review of #98, S2).
#[test]
fn a_held_cut_reads_as_the_volume_at_its_instant_after_a_write_zeroes() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let disk = created(36, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, io);
        write(&mut store, 5, 0x55).unwrap();
        write(&mut store, 6, 0x66).unwrap();
        group(&mut store, 100, 7, 70);
        let zeroed = run(
            &mut store,
            300,
            Op::WriteZeroes {
                offset: 5 * BLOCK,
                len: 2 * BLOCK,
            },
        );
        assert!(zeroed.result.is_ok(), "{io:?}");
        assert_eq!(read(&mut store, 5).unwrap(), vec![0; BLOCK_USIZE], "{io:?}");
        assert_eq!(
            cut_block(&mut store, 1, 5),
            vec![0x55; BLOCK_USIZE],
            "{io:?}"
        );
        assert_eq!(
            cut_block(&mut store, 1, 6),
            vec![0x66; BLOCK_USIZE],
            "{io:?}"
        );
    }
}

/// After the newest cut is let go, released or abandoned at the bound,
/// the next instance still says whether it opens as itself: the flag is
/// kept beside the newest cut's state, not only in the held cut. Fails
/// where the start reads the flag from the held cuts alone, as at
/// e905c2e: the status then shows a cut that opens as itself as a
/// rewind (the review of #98, D1).
#[test]
fn the_newest_cut_says_it_opens_as_itself_after_it_is_let_go() {
    for abandon in [false, true] {
        let disk = created(37, 0);
        // No hole set: every block outside the rings is one cut 1 reads,
        // and copies for it reach the bound (M2.4e).
        disk.set_seek_data(false);
        let region = region();
        let mut store = start_on(&disk, 0, &region, ON_CALLER);
        group(&mut store, 100, 7, 70);
        assert_eq!(newest(&store), Some((7, true)), "abandon {abandon}");
        if abandon {
            // Blocks outside the rings, each copied once for cut 1,
            // until the copies pass the bound.
            let outside = (0..VOLUME_BLOCKS).filter(|b| b % 64 < 32);
            for block in outside.take((TEST_BOUND / BLOCK) as usize + 1) {
                write(&mut store, block, 0x5A).unwrap();
            }
            assert_eq!(store.cut_figures().abandoned, 1);
        } else {
            assert!(store.release_cut(1));
        }
        assert!(store.held_cuts().is_empty(), "abandon {abandon}");
        drop(store);
        disk.kill();
        let store = start_on(&disk, 0, &region, ON_CALLER);
        assert_eq!(store.cuts().count, 1, "abandon {abandon}");
        assert_eq!(newest(&store), Some((7, true)), "abandon {abandon}");
    }
}

/// A held cut's block past the volume's end, or into a buffer of
/// another size than a block, is refused, not read from some other
/// map's bits (the review of #98).
#[test]
fn a_cut_is_read_only_within_the_volume() {
    let disk = created(38, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    group(&mut store, 100, 7, 70);
    let mut buf = Buf::filled(1, 0xEE);
    assert!(store.read_cut(1, VOLUME_BLOCKS - 1, &mut buf).is_ok());
    for block in [VOLUME_BLOCKS, u64::MAX / BLOCK] {
        let refused = store.read_cut(1, block, &mut buf);
        assert_eq!(
            refused.map_err(|e| e.kind()),
            Err(std::io::ErrorKind::InvalidInput),
            "block {block}"
        );
    }
    let mut two = Buf::filled(2, 0xEE);
    assert_eq!(
        store.read_cut(1, 0, &mut two).map_err(|e| e.kind()),
        Err(std::io::ErrorKind::InvalidInput)
    );
}

/// A death at any store of a write whose copy is refused, while it lets
/// cut 2 go: the next instance counts cut 2 abandoned once if it is
/// gone, and not at all if it is still held. Fails where the count is
/// stored after the cut is gone with no record of the abandoning, as
/// the bound's abandon did before the review of #98.
#[test]
fn a_death_while_a_refused_copy_lets_a_cut_go_counts_it_once() {
    let mut death = 0;
    loop {
        state::tests::quiet_deaths();
        let disk = created(39, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, ON_CALLER);
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        write(&mut store, 5, 0x66).unwrap();
        group(&mut store, 200, 8, 80);
        disk.fail_write_of(COPIES_NAME);
        region.die_at(death);
        let lived =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write(&mut store, 5, 0x77)));
        region.revive();
        let died = match lived {
            Ok(written) => {
                assert_eq!(written, Ok(()), "death at {death}");
                false
            }
            Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => true,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        if died {
            drop(store);
            disk.kill();
            disk.clear_faults();
            store = start_on(&disk, 0, &region, ON_CALLER);
        }
        let held: Vec<u64> = store.held_cuts().iter().map(|c| c.number).collect();
        let abandoned = store.cut_figures().abandoned;
        assert!(
            (held == [1] && abandoned == 1) || (held == [1, 2] && abandoned == 0),
            "death at store {death}: held {held:?}, {abandoned} abandoned"
        );
        if !died {
            break;
        }
        death += 1;
    }
    assert!(death > 3, "{death} stores");
}

/// The store's options with the cut state in `region`.
fn holding(region: &state::MemRegion, acknowledge: bool) -> OpenOptions {
    OpenOptions {
        io: ON_CALLER,
        acknowledge_failure: acknowledge,
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(region.clone()),
            boot: state::tests::BOOT,
            bound: Some(TEST_BOUND),
            intent_region: Some(TEST_INTENT_REGION),
            intent_header_sync: false,
        }),
        ..OpenOptions::default()
    }
}

/// What a start did with the cut state, and the held cuts' numbers.
type AfterStart = (Option<CutsTakenUp>, Vec<u64>);

/// What the start did with the state, and the held cuts' numbers.
fn after_start(store: &InPlace<SimDisk>) -> AfterStart {
    let held = store.held_cuts().iter().map(|c| c.number).collect();
    (store.cuts_at_start().cloned(), held)
}

/// Whether the start began a new state because the volume was opened
/// after the instance that kept the old one.
fn begun_for_an_open_between(taken: &Option<CutsTakenUp>) -> bool {
    matches!(taken, Some(CutsTakenUp::Begun(why)) if why.contains("opened after"))
}

/// Cut 1 is held with block 5 at 0x55. A start told of no region (a
/// daemon without `--hold-cuts`, or one whose run directory is another)
/// writes 0x66 to block 5, with no copy made. What the next start that
/// holds cuts does, under `mutants`, and what cut 1 reads for block 5
/// if it is held.
fn opened_between(mutants: u128) -> (Option<CutsTakenUp>, Vec<u64>, Option<u8>) {
    let disk = created(40, 0);
    let region = region();
    let mut store = start_on(&disk, mutants, &region, ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    assert_eq!(cut_block(&mut store, 1, 5), vec![0x55; BLOCK_USIZE]);
    drop(store);
    disk.kill();
    let options = OpenOptions {
        io: ON_CALLER,
        ..OpenOptions::default()
    };
    let mut store = Opened::open_with(
        disk.clone(),
        lock(&disk).expect("the lock"),
        options,
        Mutants(mutants),
    )
    .map_err(|(e, _)| e)
    .expect("opens")
    .start_with(None, Mutants(mutants))
    .expect("the in-place store");
    assert_eq!(store.cuts_at_start(), None);
    write(&mut store, 5, 0x66).unwrap();
    assert!(run(&mut store, 300, Op::Flush).result.is_ok());
    drop(store);
    disk.kill();
    let mut store = start_on(&disk, mutants, &region, ON_CALLER);
    let (taken, held) = after_start(&store);
    let five = held.contains(&1).then(|| cut_block(&mut store, 1, 5)[0]);
    (taken, held, five)
}

/// A start that holds no cuts, between two that do, leaves a state the
/// third does not take up: it wrote a block cut 1 reads with no copy
/// made, and its open changed the start token (the review of #100, D1,
/// and Codex's thread). With the token's rule planted away, the third
/// start takes cut 1 up, and it reads 0x66 for block 5, which held 0x55
/// at its instant. So it does where the open that holds no cuts leaves
/// the token as it found it (design §5.2: every start changes it,
/// whatever its options).
#[test]
fn a_start_that_holds_no_cuts_between_two_that_do() {
    let (taken, held, _) = opened_between(0);
    assert!(
        begun_for_an_open_between(&taken),
        "{taken:?}, held {held:?}"
    );
    assert!(held.is_empty(), "{held:?}");
    for (mutant, what) in [
        (
            Mutants::START_UNCHECKED,
            "the start token's rule planted away",
        ),
        (
            Mutants::START_KEPT_WITHOUT_CUTS,
            "a token not changed by an open that holds no cuts",
        ),
    ] {
        let (taken, held, five) = opened_between(mutant);
        assert_eq!(
            (taken, held, five),
            (Some(CutsTakenUp::TakenUp), vec![1], Some(0x66)),
            "{what} is not noticed"
        );
    }
}

/// Cut 1 is held and the volume fails. A start that acknowledges the
/// failure opens the volume, which takes the marker away, and is lost
/// before its second phase. What the next start does under `mutants`.
fn acknowledged_and_lost(mutants: u128) -> (Option<CutsTakenUp>, Vec<u64>) {
    let disk = created(41, 0);
    let region = region();
    let mut store = start_on(&disk, mutants, &region, ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    // The volume fails: a write of the image is refused.
    disk.fail_write_of(HOME_NAME);
    assert_eq!(write(&mut store, 6, 0x66), Err(BlkError::Failed));
    drop(store);
    disk.kill();
    disk.clear_faults();
    let open = |acknowledge: bool| {
        Opened::open_with(
            disk.clone(),
            lock(&disk).expect("the lock"),
            holding(&region, acknowledge),
            Mutants(mutants),
        )
        .map_err(|(e, _)| e)
    };
    // Without the acknowledgement the start is refused.
    let refused = open(false).err();
    assert!(
        matches!(refused, Some(OpenError::MarkedFailed)),
        "{refused:?}"
    );
    // With it the open takes the marker away, and the daemon is lost
    // before its second phase.
    let opened = open(true).expect("opens");
    assert!(opened.acknowledged());
    drop(opened);
    disk.kill();
    // The next start has nothing to acknowledge.
    let opened = open(false).expect("opens");
    assert!(!opened.acknowledged());
    let store = opened
        .start_with(None, Mutants(mutants))
        .expect("the in-place store");
    after_start(&store)
}

/// A failure acknowledged by a start that is lost before its second
/// phase: the marker is gone, and the next start, which acknowledges
/// nothing, does not take up the failed volume's state, as the
/// acknowledging open changed the start token before it took the marker
/// away (the review of #100, D2, and Codex's thread). With the token's
/// rule planted away it does.
#[test]
fn an_acknowledged_failure_whose_start_is_lost_before_its_second_phase() {
    let (taken, held) = acknowledged_and_lost(0);
    assert!(
        begun_for_an_open_between(&taken),
        "{taken:?}, held {held:?}"
    );
    assert_eq!(
        acknowledged_and_lost(Mutants::START_UNCHECKED),
        (Some(CutsTakenUp::TakenUp), vec![1]),
        "the start token's rule planted away is not noticed"
    );
}

/// Cut 1 is held and the volume fails. A start that acknowledges the
/// failure is lost after the first `after` writes and syncs of its open:
/// the disk takes nothing after them, and the daemon dies. Whether that
/// open came to its end, and what the next start, which acknowledges
/// nothing, makes of the volume under `mutants`: refused for the marker,
/// or opened, with what it took up.
fn acknowledging_open_lost_at(after: u64, mutants: u128) -> (bool, Result<AfterStart, OpenError>) {
    let disk = created(43, 0);
    let region = region();
    let mut store = start_on(&disk, mutants, &region, ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    disk.fail_write_of(HOME_NAME);
    assert_eq!(write(&mut store, 6, 0x66), Err(BlkError::Failed));
    drop(store);
    disk.kill();
    disk.clear_faults();
    let open = |acknowledge: bool| {
        Opened::open_with(
            disk.clone(),
            lock(&disk).expect("the lock"),
            holding(&region, acknowledge),
            Mutants(mutants),
        )
        .map_err(|(e, _)| e)
    };
    disk.halt_after(after);
    let opened = open(true).is_ok();
    disk.kill();
    disk.clear_faults();
    let next = open(false).map(|opened| {
        let store = opened
            .start_with(None, Mutants(mutants))
            .expect("the in-place store");
        after_start(&store)
    });
    (opened, next)
}

/// An open that acknowledges a failure changes the start token before
/// the marker goes (design §5.2: before the start acknowledges
/// anything). Lost at any of its writes and syncs, it leaves either the
/// marker, which refuses the next start, or a new token, under which the
/// next start does not take up the failed volume's state. Where the
/// token is written after the marker goes, a loss between the two
/// leaves neither, and the next start takes cut 1 up.
#[test]
fn an_acknowledging_open_lost_at_any_write_leaves_the_marker_or_a_new_token() {
    let mut lost = 0;
    for after in 0u64.. {
        let (opened, next) = acknowledging_open_lost_at(after, 0);
        match next {
            Err(OpenError::MarkedFailed) => {}
            Ok((taken, held)) => assert!(
                begun_for_an_open_between(&taken) && held.is_empty(),
                "the open lost after {after} writes and syncs: {taken:?}, held {held:?}"
            ),
            Err(e) => panic!("the open lost after {after} writes and syncs: {e:?}"),
        }
        if opened {
            break;
        }
        lost += 1;
    }
    assert!(lost > 0);
    let taken = (0u64..=lost).any(|after| {
        matches!(
            acknowledging_open_lost_at(after, Mutants::START_AFTER_ACKNOWLEDGING).1,
            Ok((Some(CutsTakenUp::TakenUp), ref held)) if held == &[1]
        )
    });
    assert!(
        taken,
        "the token written after the marker goes is not noticed"
    );
}

/// Cut 1 is held and a copy made for it. The writeback of the copies
/// fails; the next start's sync hears it, and that start is lost at its
/// first store into the region. What the start after it does under
/// `mutants`, the kernel having reported the failure once.
fn heard_and_lost(mutants: u128) -> (Option<CutsTakenUp>, Vec<u64>) {
    let (taken, held, died) = heard_and_lost_at(0, mutants);
    assert!(died, "the start made no store into the region");
    (taken, held)
}

/// The same with the start that hears the failure lost at store `death`
/// into the region: the next start's outcome and held cuts, and whether
/// the lost start died at all.
fn heard_and_lost_at(death: u64, mutants: u128) -> (Option<CutsTakenUp>, Vec<u64>, bool) {
    state::tests::quiet_deaths();
    let disk = created(42, 0);
    let region = region();
    let mut store = start_on(&disk, mutants, &region, ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    write(&mut store, 5, 0x66).unwrap();
    assert!(store.cut_figures().copied > 0);
    drop(store);
    disk.kill();
    disk.fail_sync_of(COPIES_NAME);
    region.die_at(death);
    let lost = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        start_on(&disk, mutants, &region, ON_CALLER)
    }));
    region.revive();
    let died = match lost {
        Ok(store) => {
            drop(store);
            false
        }
        Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => true,
        Err(payload) => std::panic::resume_unwind(payload),
    };
    disk.kill();
    // The kernel reports a failed writeback once: the next sync succeeds.
    disk.clear_faults();
    let store = start_on(&disk, mutants, &region, ON_CALLER);
    let (taken, held) = after_start(&store);
    (taken, held, died)
}

/// A failed sync of the copies, heard by a start that is lost before it
/// begins the new state, leaves a state the next start does not take
/// up, though its own sync succeeds: the lost start's open changed the
/// start token (the review of #100, D3). The state is read and judged
/// before anything is stored in it, so that start's first store is the
/// new state's. With the token's rule planted away the next start takes
/// the state up.
#[test]
fn a_failed_sync_of_the_copies_heard_by_a_start_that_is_lost() {
    let (taken, held) = heard_and_lost(0);
    assert!(
        begun_for_an_open_between(&taken),
        "{taken:?}, held {held:?}"
    );
    assert_eq!(
        heard_and_lost(Mutants::START_UNCHECKED),
        (Some(CutsTakenUp::TakenUp), vec![1]),
        "the start token's rule planted away is not noticed"
    );
}

/// A start that hears of a failed writeback of the copies, lost at any
/// store into the region, leaves no state that holds cut 1: it judges
/// the copies before it records its token in the state, so whatever it
/// stored is the new state's. Lost before that state is whole, the next
/// start begins one; lost after, the next start takes up the new state,
/// which holds no cut. Fails where the start takes the state up before
/// it has judged the copies (the next look at #100, K8): lost after the
/// token's two words, it leaves the old state under the token the next
/// start finds.
#[test]
fn a_start_that_heard_a_failed_sync_of_the_copies_lost_at_any_store() {
    let mut death = 0;
    loop {
        let (taken, held, died) = heard_and_lost_at(death, 0);
        assert!(
            taken.is_some() && held.is_empty(),
            "death at {death}: {taken:?}, held {held:?}"
        );
        if !died {
            break;
        }
        death += 1;
    }
    assert!(death > 2, "the start made {death} stores");
}

/// What the next start does after a start that trusted the state was
/// lost at store `death` of the region: the outcome, the held cuts, and
/// whether the lost start died at all. Cut 1 is held, and group 8's
/// flush was under way at the kill, so that the lost start has a cut's
/// flush to finish.
fn lost_while_taking_up(death: u64) -> (Option<CutsTakenUp>, Vec<u64>, bool) {
    state::tests::quiet_deaths();
    let disk = created(43, 3);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    group(&mut store, 100, 7, 70);
    for label in 0..LABELS {
        let done = run(
            &mut store,
            200 + label as u64,
            uberblock_write(label, 8, 80),
        );
        assert!(done.result.is_ok());
    }
    // Received and past its barrier, its sync not yet ended.
    store
        .submit(Request {
            id: RequestId(210),
            op: Op::Flush,
        })
        .unwrap();
    drop(store);
    disk.kill();
    region.die_at(death);
    let lost = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        start_on(&disk, 0, &region, ON_CALLER)
    }));
    region.revive();
    let died = match lost {
        Ok(store) => {
            drop(store);
            false
        }
        Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => true,
        Err(payload) => std::panic::resume_unwind(payload),
    };
    disk.kill();
    let store = start_on(&disk, 0, &region, ON_CALLER);
    let (taken, held) = after_start(&store);
    (taken, held, died)
}

/// A start that trusted the state records its new start token in it
/// before anything else, and only then finishes what a death
/// interrupted: lost while it finishes, it leaves a state the next start
/// takes up and finishes. Lost at the token's first or second word, it
/// leaves a token the next start does not find, and the state is begun
/// anew. Fails where the token is recorded after the state is settled:
/// a start lost while it settles would then cost the held cuts.
#[test]
fn a_start_lost_after_it_trusted_the_state_leaves_it_to_the_next() {
    let mut after_the_token = 0;
    let mut death = 0;
    loop {
        let (taken, held, died) = lost_while_taking_up(death);
        if death < 2 {
            assert!(
                begun_for_an_open_between(&taken),
                "death at {death}: {taken:?}, held {held:?}"
            );
        } else {
            assert_eq!(
                (taken.clone(), held.first().copied()),
                (Some(CutsTakenUp::TakenUp), Some(1)),
                "death at {death}: held {held:?}"
            );
        }
        if !died {
            break;
        }
        if death >= 2 {
            after_the_token += 1;
        }
        death += 1;
    }
    assert!(
        after_the_token > 0,
        "the start made no store after the token"
    );
}

// ---- the intent map (M2.3) ----

/// A boot other than the tests' one: a restart of the machine.
const OTHER_BOOT: [u8; 16] = *b"another boot....";

/// A start after a restart of the machine: another boot, and a device
/// it creates.
const AFTER_A_REBOOT: Coming = Coming {
    boot: OTHER_BOOT,
    acknowledging: false,
    taken_over: false,
};

/// The intent map's figures.
fn intent_of(store: &InPlace<SimDisk>) -> IntentFigures {
    store
        .intent_figures()
        .expect("the store keeps an intent map")
}

/// A store given no size of region keeps its map in regions of
/// `DEFAULT_INTENT_REGION`, the size the daemon's help names (G1 of
/// the review of #109: the help is held to the constant, and this
/// holds the store to it).
#[test]
fn a_store_given_no_region_keeps_the_map_in_regions_of_the_default() {
    let disk = created(53, 0);
    let options = OpenOptions {
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(region()),
            boot: state::tests::BOOT,
            bound: Some(TEST_BOUND),
            intent_region: None,
            intent_header_sync: false,
        }),
        ..OpenOptions::default()
    };
    let store = Opened::open(disk.clone(), lock(&disk).expect("the lock"), options)
        .map_err(|(e, _)| e)
        .expect("opens")
        .start_with(None, Mutants(0))
        .expect("the in-place store");
    let figures = intent_of(&store);
    assert_eq!(figures.region_bytes, DEFAULT_INTENT_REGION);
    assert_eq!(
        figures.regions,
        (VOLUME_BLOCKS * BLOCK).div_ceil(DEFAULT_INTENT_REGION)
    );
}

/// A store whose cut 1 holds block 5 at 0x55 and is committed (the
/// release stands in for it): nothing changed after its instant, so
/// the map names no region.
fn committed_one(disk: &SimDisk, mutants: u128, io: IoMode) -> InPlace<SimDisk> {
    let mut store = start_on(disk, mutants, &region(), io);
    assert!(
        matches!(store.intent_at_start(), Some(IntentAtStart::Begun(_))),
        "a new volume has no map to take up"
    );
    assert_eq!(intent_of(&store).named, 4, "a new map names every region");
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    assert!(store.release_cut(1));
    assert_eq!(intent_of(&store).named, 0, "nothing changed after cut 1");
    store
}

/// After a restart of the machine the cut state is gone, and the new
/// one counts as changed only the blocks of the regions the intent map
/// names: here region 2 (blocks 128 to 191), written after cut 1 was
/// committed, and not block 5, written before. A second write into the
/// same region does not sync the map again. The defect that carries no
/// region leaves block 150 out of the next cut.
///
/// The power cut keeps everything, or loses everything no sync covered,
/// creations no directory sync covered among them: the map that the
/// first start created is there all the same (D1 of the review of
/// #106).
#[test]
fn after_a_reboot_the_next_cut_holds_the_regions_the_map_names() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        for (mutants, lose_all) in [(0, false), (0, true), (Mutants::INTENT_NOT_CARRIED, false)] {
            let case = format!("{io:?}, mutants {mutants:#x}, losing all unsynced {lose_all}");
            let disk = created(37, 0);
            let mut store = committed_one(&disk, mutants, io);
            let syncs = intent_of(&store).syncs.count;
            write(&mut store, 150, 0x66).unwrap();
            assert_eq!(intent_of(&store).named, 1, "{case}");
            assert_eq!(intent_of(&store).syncs.count, syncs + 1, "{case}");
            write(&mut store, 151, 0x67).unwrap();
            assert_eq!(
                intent_of(&store).syncs.count,
                syncs + 1,
                "{case}: named already"
            );
            drop(store);
            if lose_all {
                disk.power_cut(crate::sim::LoseAll);
            } else {
                disk.power_cut(crate::sim::KeepAll);
            }
            let mut store = start_as(&disk, mutants, &region(), io, AFTER_A_REBOOT);
            assert!(
                matches!(store.cuts_at_start(), Some(CutsTakenUp::Begun(_))),
                "{case}: another boot's state is not taken up"
            );
            assert_eq!(
                store.intent_at_start(),
                Some(&IntentAtStart::TakenUp {
                    named: 1,
                    regions: 4
                }),
                "{case}"
            );
            group(&mut store, 200, 8, 80);
            let map = store.cut_map(1);
            assert!(!map.contains(&5), "{case}: block 5 is as cut 1 holds it");
            assert_eq!(map.contains(&150), mutants == 0, "{case}");
            assert_eq!(map.contains(&128), mutants == 0, "{case}: the whole region");
            // The start kept the map under its own token: the start
            // after the next power cut takes it up again (G3 of the
            // review of #106).
            drop(store);
            disk.power_cut(crate::sim::LoseAll);
            let store = start_as(&disk, mutants, &region(), io, AFTER_A_REBOOT);
            assert!(
                matches!(store.intent_at_start(), Some(IntentAtStart::TakenUp { .. })),
                "{case}: a second start after a power cut: {:?}",
                store.intent_at_start()
            );
        }
    }
}

/// A write into a region the map does not name begins only once the
/// region's bit is written and synced: where the page's write or the
/// sync fails, the write fails with the volume and the image is not
/// written (the lead's answer (a) to Q1 of M2.3). The defect writes
/// first.
#[test]
fn a_write_into_a_clean_region_begins_only_once_its_bit_is_synced() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        for (mutants, fault) in [
            (0, "sync"),
            (0, "write"),
            (Mutants::INTENT_WRITE_UNCOVERED, "sync"),
        ] {
            let case = format!("{io:?}, mutants {mutants:#x}, the map's {fault} fails");
            let disk = created(38, 0);
            let mut store = committed_one(&disk, mutants, io);
            let home_writes = disk.writes_of(crate::store::HOME_NAME);
            match fault {
                "sync" => disk.fail_sync_of(intent::INTENT_NAME),
                _ => disk.fail_write_of(intent::INTENT_NAME),
            }
            let written = write(&mut store, 150, 0x66);
            if mutants == 0 {
                assert_eq!(written, Err(BlkError::Failed), "{case}");
                assert!(store.is_failed(), "{case}");
                assert_eq!(
                    disk.writes_of(crate::store::HOME_NAME),
                    home_writes,
                    "{case}: the image was written"
                );
            } else {
                assert_eq!(written, Ok(()), "{case}: the defect");
            }
        }
    }
}

/// Submit `op` as request `id`, and do not wait for it.
fn submit(store: &mut InPlace<SimDisk>, id: u64, op: Op) {
    store
        .submit(Request {
            id: RequestId(id),
            op,
        })
        .unwrap();
}

/// Poll for `time`, or until `want` completions are taken, without
/// waiting for a sync: a sync the disk holds would not end.
fn polled(store: &mut InPlace<SimDisk>, done: &mut Vec<Completion>, want: usize, time: Duration) {
    let began = std::time::Instant::now();
    while done.len() < want && began.elapsed() < time {
        store.poll(done);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The ids of `done`, in the order they completed, each with its
/// result.
fn ended(done: &[Completion]) -> Vec<(u64, Result<(), BlkError>)> {
    done.iter().map(|c| (c.id.0, c.result)).collect()
}

fn write_op(block: u64, byte: u8) -> Op {
    Op::Write {
        offset: block * BLOCK,
        data: Buf::filled(1, byte),
    }
}

fn read_op(block: u64) -> Op {
    Op::Read {
        offset: block * BLOCK,
        buf: Buf::filled(1, 0xEE),
    }
}

/// A write into a region the map does not name waits for the map's
/// sync, which runs on the file's helper and not on the queue's thread
/// (the lead's ruling on M2.3b). While the disk holds that sync:
/// - a write into a region the map names, and a read elsewhere,
///   complete;
/// - a read of the waiting write's block and a flush wait behind it;
/// - the image does not have the waiting write.
///
/// When the sync ends, the write completes, then the read (which sees
/// it) and the flush.
#[test]
fn a_write_into_a_clean_region_waits_for_the_map_and_no_other_request_does() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(43, 0);
        let mut store = committed_one(&disk, 0, io);
        write(&mut store, 10, 0x10).unwrap();
        let before = intent_of(&store);
        let home_writes = disk.writes_of(crate::store::HOME_NAME);
        disk.hold_syncs_of(intent::INTENT_NAME);
        submit(&mut store, 1, write_op(150, 0x66));
        submit(&mut store, 2, write_op(11, 0x11));
        submit(&mut store, 3, read_op(12));
        submit(&mut store, 4, read_op(150));
        submit(&mut store, 5, Op::Flush);
        let mut done = Vec::new();
        polled(&mut store, &mut done, 2, DRAIN);
        polled(&mut store, &mut done, 3, Duration::from_millis(100));
        let mut early = ended(&done);
        early.sort_unstable_by_key(|(id, _)| *id);
        let waits = intent_of(&store).waits - before.waits;
        let image_written = disk.writes_of(crate::store::HOME_NAME) - home_writes;
        // Let the sync end before anything is asserted: the store's drop
        // would wait for it.
        disk.release_syncs();
        let store = drained(store, &mut done);
        assert_eq!(early, [(2, Ok(())), (3, Ok(()))], "{case}");
        assert_eq!(waits, 1, "{case}");
        assert_eq!(
            image_written, 1,
            "{case}: only the write into the named region reached the image"
        );
        let order: Vec<u64> = ended(&done)[2..].iter().map(|(id, _)| *id).collect();
        assert_eq!(order.len(), 3, "{case}");
        assert_eq!(order[0], 1, "{case}: the write first");
        assert!(ended(&done).iter().all(|(_, r)| r.is_ok()), "{case}");
        let read = done
            .iter()
            .find(|c| c.id.0 == 4)
            .and_then(|c| c.buf.as_ref());
        assert_eq!(
            read.map(|buf| buf.to_vec()),
            Some(vec![0x66; BLOCK as usize]),
            "{case}: the read waited for the write"
        );
        assert_eq!(
            intent_of(&store).syncs.count,
            before.syncs.count + 1,
            "{case}"
        );
    }
}

/// A store dropped while a write waits for the map's sync ends once the
/// sync has, and the write is done before it goes, as everything
/// submitted is (D2 of the review of #106, and Codex's P2 there): the
/// write alone, and with a read of its block waiting behind it. The
/// store is dropped on a thread of the test's, so that a drop that does
/// not end fails the test and does not hang it.
#[test]
fn a_store_dropped_while_a_write_waits_for_the_map_does_the_write_first() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        for behind in [false, true] {
            let case = format!("{io:?}, a read behind the write {behind}");
            let disk = created(46, 0);
            let mut store = committed_one(&disk, 0, io);
            disk.hold_syncs_of(intent::INTENT_NAME);
            submit(&mut store, 1, write_op(150, 0x66));
            if behind {
                submit(&mut store, 2, read_op(150));
            }
            let mut done = Vec::new();
            polled(&mut store, &mut done, 1, Duration::from_millis(50));
            disk.release_syncs();
            let (tell, dropped) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                drop(store);
                let _ = tell.send(());
            });
            assert!(done.is_empty(), "{case}: {:?}", ended(&done));
            assert_eq!(
                dropped.recv_timeout(DRAIN),
                Ok(()),
                "{case}: the drop did not end"
            );
            disk.kill();
            let mut store = start_on(&disk, 0, &region(), io);
            let block = read(&mut store, 150).unwrap();
            assert!(
                block.iter().all(|&b| b == 0x66),
                "{case}: the write was not done"
            );
        }
    }
}

/// A flush waits for a write submitted before it that waits for the
/// map, with nothing waiting between them (G2 of the review of #106):
/// it covers that write.
#[test]
fn a_flush_waits_for_a_write_that_waits_for_the_map() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(49, 0);
        let mut store = committed_one(&disk, 0, io);
        disk.hold_syncs_of(intent::INTENT_NAME);
        submit(&mut store, 1, write_op(150, 0x66));
        submit(&mut store, 2, Op::Flush);
        let mut done = Vec::new();
        polled(&mut store, &mut done, 1, Duration::from_millis(100));
        let early = ended(&done);
        disk.release_syncs();
        drop(drained(store, &mut done));
        assert!(early.is_empty(), "{case}: {early:?}");
        assert_eq!(ended(&done), [(1, Ok(())), (2, Ok(()))], "{case}");
    }
}

/// A write that waited for the map does not begin once the volume has
/// failed, though the sync it waited for ended well (G4 of the review
/// of #106). Two writes into one clean region wait for one sync, which
/// covers them both; the first fails at the image and fails the volume,
/// and the second fails with it and does not reach the image. On the
/// caller's thread, where the first write's failure is known before the
/// second would begin.
#[test]
fn a_write_that_waited_does_not_begin_after_the_volume_failed() {
    let disk = created(47, 0);
    let mut store = committed_one(&disk, 0, ON_CALLER);
    disk.hold_syncs_of(intent::INTENT_NAME);
    submit(&mut store, 1, write_op(150, 0x66));
    submit(&mut store, 2, write_op(160, 0x77));
    let mut done = Vec::new();
    polled(&mut store, &mut done, 1, Duration::from_millis(20));
    let early = ended(&done);
    disk.fail_write(0, false);
    disk.release_syncs();
    let store = drained(store, &mut done);
    let failed = store.is_failed();
    drop(store);
    assert!(early.is_empty(), "{early:?}");
    assert!(failed);
    assert_eq!(
        ended(&done),
        [(1, Err(BlkError::Failed)), (2, Err(BlkError::Failed))],
        "the first write failed at the image, the second with the volume"
    );
    disk.kill();
    disk.clear_faults();
    let acknowledging = Coming {
        boot: state::tests::BOOT,
        acknowledging: true,
        taken_over: true,
    };
    let mut store = start_as(&disk, 0, &region(), ON_CALLER, acknowledging);
    let block = read(&mut store, 160).unwrap();
    assert!(
        block.iter().all(|&b| b == 0),
        "the second write reached the image"
    );
}

/// A sync of the map that ends without making durable a bit set before
/// it began, with no commit since to clear it, fails the volume: the
/// map is wrong, and to set the bit again and sync would go on for
/// ever (the addendum to the review of #106). The defect of the map
/// that gets there: setting a bit does not ask for its page to be
/// written. The write fails with the volume.
///
/// The store runs on a thread of the test's, which sends what it saw
/// before it drops the store: where the volume does not fail, the drop
/// would sync for ever too, and the store is leaked instead.
#[test]
fn a_sync_of_the_map_that_leaves_a_bit_set_before_it_unwritten_fails_the_volume() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let (tell, told) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let disk = created(51, 0);
            let mut store = committed_one(&disk, Mutants::INTENT_SET_UNWRITTEN, io);
            submit(&mut store, 1, write_op(150, 0x66));
            let mut done = Vec::new();
            polled(&mut store, &mut done, 1, DRAIN);
            let failed = store.is_failed();
            let _ = tell.send((ended(&done), failed));
            if failed {
                drop(store);
            } else {
                std::mem::forget(store);
            }
        });
        assert_eq!(
            told.recv_timeout(DRAIN * 2),
            Ok((vec![(1, Err(BlkError::Failed))], true)),
            "{io:?}"
        );
    }
}

/// A request deferred behind a write that waits for the map is answered
/// when the volume fails, while the map's sync is still held, with I/O
/// threads or none (N1 of the next look at #106, and Codex's P2 on
/// ab2d7f2). Without threads nothing ran what was deferred until the
/// map's sync ended, which on a disk whose sync never returns is never.
/// Then the drop ends.
#[test]
fn a_request_deferred_behind_a_write_that_waits_for_the_map_is_answered_at_a_failure() {
    for io in [IoMode::default(), ON_CALLER] {
        let case = format!("{io:?}");
        let disk = created(52, 0);
        let mut store = committed_one(&disk, 0, io);
        disk.hold_syncs_of(intent::INTENT_NAME);
        submit(&mut store, 1, write_op(150, 0x66));
        submit(&mut store, 2, read_op(150));
        let mut done = Vec::new();
        polled(&mut store, &mut done, 1, Duration::from_millis(50));
        let early = ended(&done);
        store.fail_now();
        polled(&mut store, &mut done, 2, Duration::from_millis(500));
        let mut answered = ended(&done);
        answered.sort_by_key(|(id, _)| *id);
        disk.release_syncs();
        let (tell, dropped) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(store);
            let _ = tell.send(());
        });
        assert!(early.is_empty(), "{case}: {early:?}");
        assert_eq!(
            answered,
            [(1, Err(BlkError::Failed)), (2, Err(BlkError::Failed))],
            "{case}"
        );
        assert_eq!(
            dropped.recv_timeout(DRAIN),
            Ok(()),
            "{case}: the drop did not end"
        );
    }
}

/// A write of the map that fails when a commit clears bits fails the
/// volume, as a failed write of the store does (G4 of the review of
/// #106), and the cut is not let go (review of M2.4b-1): the map on the
/// disk may name it or not, page by page. `release_cut` says true: it
/// was the cut being uploaded, and the map may name it from now on.
/// What finds the kept cut after is the next test's. The defect,
/// planted by hand: the cut let go all the same.
#[test]
fn a_failed_write_of_the_map_at_a_commit_fails_the_volume() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let disk = created(48, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        assert!(!store.is_failed(), "{io:?}");
        disk.fail_write_of(intent::INTENT_NAME);
        assert!(store.release_cut(1), "{io:?}: the cut being uploaded");
        assert!(store.is_failed(), "{io:?}");
        let held: Vec<(u64, CutPhase)> = store
            .held_cuts()
            .iter()
            .map(|cut| (cut.number, cut.phase))
            .collect();
        assert_eq!(held, [(1, CutPhase::Uploading)], "{io:?}");
    }
}

/// What finds a cut kept at a failed write of the map (F1 of the review
/// of M2.4b-1). In the usual course the failure is marked on the disk:
/// a start that does not acknowledge it is refused, and the one that
/// does begins a new cut state and holds no cut. The map it takes up
/// names the cut released before, so the uploader's check finds a row
/// the map does not name: a mismatch, and one whole upload. Only where
/// the failure was never marked (a death before the marker; here the
/// planted defect that leaves the marker out) does the next start take
/// the state up and hold the cut, which a release then lets go.
#[test]
fn a_cut_kept_at_a_failed_write_of_the_map_is_held_again_only_where_no_marker_was_written() {
    for marked in [true, false] {
        let case = if marked { "marked" } else { "never marked" };
        let mutants = if marked { 0 } else { Mutants::FAIL_UNMARKED };
        let disk = created(79, 0);
        let region = region();
        let mut store = start_on(&disk, mutants, &region, ON_CALLER);
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1), "{case}");
        let id = store.cut_state_id().expect("a cut state");
        let one = CommittedCut {
            state: id,
            number: 1,
            group: 7,
        };
        write(&mut store, 6, 0x66).unwrap();
        group(&mut store, 200, 8, 80);
        disk.fail_write_of(intent::INTENT_NAME);
        assert!(store.release_cut(2), "{case}");
        assert!(store.is_failed(), "{case}");
        assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "{case}");
        // The drop waits for the marker's thread.
        drop(store);
        disk.kill();
        disk.clear_faults();
        if marked {
            assert!(
                matches!(
                    opened(&disk, false, ON_CALLER),
                    Err(OpenError::MarkedFailed)
                ),
                "{case}"
            );
            let acknowledging = Coming {
                acknowledging: true,
                ..TAKING_OVER
            };
            let store = start_as(&disk, 0, &region, ON_CALLER, acknowledging);
            assert_ne!(store.cut_state_id(), Some(id), "{case}: a new state");
            assert_eq!(phases(&store), [], "{case}");
            assert_eq!(store.committed_cut(), Some(one), "{case}");
        } else {
            let mut store = start_on(&disk, 0, &region, ON_CALLER);
            assert_eq!(store.cut_state_id(), Some(id), "{case}: taken up");
            assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "{case}");
            assert_eq!(store.committed_cut(), Some(one), "{case}");
            assert!(store.release_cut(2), "{case}");
            assert_eq!(phases(&store), [], "{case}");
            let two = CommittedCut {
                number: 2,
                group: 8,
                ..one
            };
            assert_eq!(store.committed_cut(), Some(two), "{case}");
        }
    }
}

/// The bound abandons the cut being uploaded after its commit has
/// landed and before its release (P1 of the review of M2.4b-1, the
/// review's scenario T5): the release finds it gone, and
/// `name_committed_cut` names it in the intent map as committed all the
/// same, clearing no bit. After a restart of the machine the map taken
/// up names it, and the next cut holds every block that differs from
/// the volume at its instant. That start begins a new state, so the map
/// then names a cut of the state before; when the bound takes the new
/// state's first cut after its commit, it is named over that one. The
/// defects, planted by hand: the call that names nothing, the header
/// not written, and a base of another state never replaced.
#[test]
fn a_committed_cut_the_bound_took_is_named_as_committed() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(77, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1), "{case}");
        let id = store.cut_state_id().expect("a cut state");
        let committed = |number, group| {
            Some(CommittedCut {
                state: id,
                number,
                group,
            })
        };
        // Cut 2 holds more blocks than the bound, so that overwriting
        // them all abandons it.
        let over = TEST_BOUND / BLOCK + 8;
        for block in 0..over {
            write(&mut store, 100 + block, 0x22).unwrap();
        }
        group(&mut store, 200, 8, 80);
        let at_cut_2 = volume_now(&mut store);
        assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "{case}");
        // Cut 2's commit lands here, and the pool writes over it.
        for block in 0..over {
            write(&mut store, 100 + block, 0x33).unwrap();
        }
        assert_eq!(phases(&store), [], "{case}: the bound took cut 2");
        assert!(!store.release_cut(2), "{case}: not held");
        assert_eq!(store.committed_cut(), committed(1, 7), "{case}");
        let named = intent_of(&store).named;
        assert!(store.name_committed_cut(2, 8), "{case}");
        assert_eq!(store.committed_cut(), committed(2, 8), "{case}");
        assert_eq!(intent_of(&store).named, named, "{case}: no bit cleared");
        assert!(store.name_committed_cut(2, 8), "{case}: named already");
        write(&mut store, 230, 0x44).unwrap();
        drop(store);
        disk.power_cut(crate::sim::KeepAll);
        let mut store = start_as(&disk, 0, &region(), io, AFTER_A_REBOOT);
        assert!(
            matches!(store.intent_at_start(), Some(IntentAtStart::TakenUp { .. })),
            "{case}: {:?}",
            store.intent_at_start()
        );
        assert_eq!(store.committed_cut(), committed(2, 8), "{case}");
        let now = volume_now(&mut store);
        group(&mut store, 400, 10, 100);
        let map: std::collections::BTreeSet<u64> = store.cut_map(1).into_iter().collect();
        for block in 0..VOLUME_BLOCKS {
            if now[block as usize] != at_cut_2[block as usize] {
                assert!(
                    map.contains(&block),
                    "{case}: block {block} differs from cut 2 and the next cut lacks it"
                );
            }
        }
        let new_id = store.cut_state_id().expect("the new state");
        assert_ne!(new_id, id, "{case}: a new state after the reboot");
        assert_eq!(phases(&store), [(1, CutPhase::Uploading)], "{case}");
        // The new state's cut 1 commits, and the pool writes over it.
        for block in 0..over {
            write(&mut store, 100 + block, 0x55).unwrap();
        }
        assert_eq!(phases(&store), [], "{case}: the bound took cut 1");
        assert!(!store.release_cut(1), "{case}: not held");
        assert!(
            store.name_committed_cut(1, 10),
            "{case}: over another state's"
        );
        assert_eq!(
            store.committed_cut(),
            Some(CommittedCut {
                state: new_id,
                number: 1,
                group: 10,
            }),
            "{case}"
        );
    }
}

/// A cut is named as committed only where the store no longer holds it,
/// it is a cut of this state, and the map names no newer cut of this
/// state; anything else changes nothing. A map that cannot be written
/// fails the volume, as at a release. The defects, planted by hand: each
/// of the three refusals taken out.
#[test]
fn a_cut_is_named_as_committed_only_when_it_is_gone_and_newer() {
    let disk = created(78, 0);
    let mut store = start_on(&disk, 0, &region(), ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    assert!(!store.name_committed_cut(1, 7), "held: released instead");
    assert!(!store.name_committed_cut(0, 7), "no cut 0");
    assert!(!store.name_committed_cut(2, 8), "no cut 2 yet");
    assert_eq!(store.committed_cut(), None);
    assert!(store.release_cut(1));
    write(&mut store, 6, 0x66).unwrap();
    group(&mut store, 200, 8, 80);
    write(&mut store, 7, 0x77).unwrap();
    group(&mut store, 300, 9, 90);
    assert!(store.release_cut(2));
    let id = store.cut_state_id().expect("a cut state");
    let base = Some(CommittedCut {
        state: id,
        number: 2,
        group: 8,
    });
    assert_eq!(store.committed_cut(), base);
    assert!(!store.name_committed_cut(1, 7), "older than the base");
    assert_eq!(store.committed_cut(), base);
    assert_eq!(store.abandon_all_cuts(), [3]);
    disk.fail_write_of(intent::INTENT_NAME);
    assert!(!store.name_committed_cut(3, 9), "the map's write failed");
    assert!(store.is_failed());
}

/// A start refused for want of direct I/O touches neither the cut state
/// nor the intent map: it creates no map (the review of #106).
#[test]
fn a_start_refused_for_direct_io_creates_no_map() {
    let disk = created(50, 0);
    disk.set_direct_align(None);
    let options = OpenOptions {
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(region()),
            boot: state::tests::BOOT,
            bound: Some(TEST_BOUND),
            intent_region: Some(TEST_INTENT_REGION),
            intent_header_sync: false,
        }),
        ..OpenOptions::default()
    };
    let started = Opened::open(disk.clone(), lock(&disk).expect("the lock"), options)
        .map_err(|(e, _)| e)
        .expect("opens")
        .start_with(None, Mutants::default());
    assert!(
        matches!(&started, Err(StartError::Io(why)) if why.contains("no direct I/O")),
        "{:?}",
        started.as_ref().map(|_| ())
    );
    drop(started);
    assert_eq!(
        disk.open(intent::INTENT_NAME)
            .map(|_| ())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
}

/// The writes that come into clean regions while a sync of the map is
/// under way wait for one sync together: four writes into four clean
/// regions cost two syncs of the map, not four.
#[test]
fn writes_into_clean_regions_while_the_map_syncs_share_the_next_sync() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(44, 0);
        let mut store = committed_one(&disk, 0, io);
        let syncs = intent_of(&store).syncs.count;
        disk.hold_syncs_of(intent::INTENT_NAME);
        for (id, block) in [(1, 10), (2, 70), (3, 150), (4, 200)] {
            submit(&mut store, id, write_op(block, id as u8));
        }
        let mut done = Vec::new();
        polled(&mut store, &mut done, 1, Duration::from_millis(100));
        let early = ended(&done);
        let waits = intent_of(&store).waits;
        disk.release_syncs();
        let store = drained(store, &mut done);
        assert!(early.is_empty(), "{case}: {early:?}");
        assert_eq!(waits, 4, "{case}");
        assert_eq!(done.len(), 4, "{case}");
        assert!(ended(&done).iter().all(|(_, r)| r.is_ok()), "{case}");
        assert_eq!(intent_of(&store).syncs.count, syncs + 2, "{case}");
        assert_eq!(intent_of(&store).named, 4, "{case}");
    }
}

/// A cut is committed while a write into a clean region waits for the
/// map: the clear takes the region's bit, since the write has changed
/// nothing yet, and writes its page. When the sync ends the bit is not
/// durable, so the write sets it again and waits for one more sync.
/// After a power cut the map names the region, and the next cut holds
/// the block. The defect begins every waiting write when a sync ends,
/// and the next cut misses the block.
///
/// Cut 2 is made by a group whose uberblock goes to the first label
/// alone, in region 0: a group to all four labels would name every
/// region of the tests' volume, and region 2 would not be clean.
#[test]
fn a_write_whose_bit_a_clear_takes_while_it_waits_sets_it_again() {
    for mutants in [0, Mutants::INTENT_RELEASED_UNCHECKED] {
        let case = format!("mutants {mutants:#x}");
        let disk = created(45, 0);
        let mut store = committed_one(&disk, mutants, IoMode::default());
        assert!(run(&mut store, 300, uberblock_write(0, 9, 90))
            .result
            .is_ok());
        assert!(run(&mut store, 301, Op::Flush).result.is_ok());
        assert_eq!(intent_of(&store).named, 1, "{case}: region 0 alone");
        let syncs = intent_of(&store).syncs.count;
        disk.hold_syncs_of(intent::INTENT_NAME);
        submit(&mut store, 1, write_op(150, 0x66));
        let mut done = Vec::new();
        polled(&mut store, &mut done, 1, Duration::from_millis(50));
        let early = ended(&done);
        let released = store.release_cut(2);
        disk.release_syncs();
        let store = drained(store, &mut done);
        assert!(early.is_empty(), "{case}: {early:?}");
        assert!(released, "{case}");
        assert_eq!(ended(&done), [(1, Ok(()))], "{case}");
        let figures = intent_of(&store);
        if mutants == 0 {
            assert_eq!(figures.syncs.count, syncs + 2, "{case}");
            assert_eq!(figures.named, 1, "{case}");
        } else {
            assert_eq!(figures.named, 0, "{case}: the defect");
        }
        drop(store);
        disk.power_cut(crate::sim::KeepAll);
        let mut store = start_as(&disk, mutants, &region(), IoMode::default(), AFTER_A_REBOOT);
        group(&mut store, 400, 10, 100);
        assert_eq!(store.cut_map(1).contains(&150), mutants == 0, "{case}");
    }
}

/// A release made again for the same cut must count as a commit, as the
/// first does (`intent.commits`, the guard of a write that waits for the
/// map). The daemon dies in the release of cut 2, after the map's pages
/// and at the cut state's first store: the map names cut 2 already, and
/// the cut is still held. The next instance takes both up. A write into
/// a clean region waits for the map's sync, and the uploader's check
/// finds the row naming cut 2 and releases it again: its `clear` takes
/// the waiting write's bit, as the first release's can. The write sets
/// the bit again and goes on once the map holds it, and the volume stays
/// healthy. The defect counts a release only where it moves the map's
/// base: the write finds its bit gone with no commit counted, takes it
/// for lost to the disk, and fails a healthy volume.
#[test]
fn a_release_made_again_after_a_death_counts_for_a_write_that_waits() {
    state::tests::quiet_deaths();
    let disk = created(80, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, IoMode::default());
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    assert!(store.release_cut(1));
    // Cut 2 by a group to the first label alone, so that region 2 stays
    // clean, as above.
    assert!(run(&mut store, 300, uberblock_write(0, 9, 90))
        .result
        .is_ok());
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
    assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "cut 2 held");
    assert_eq!(
        store.committed_cut().map(|cut| (cut.number, cut.group)),
        Some((2, 9)),
        "the map names cut 2 already"
    );
    let syncs = intent_of(&store).syncs.count;
    disk.hold_syncs_of(intent::INTENT_NAME);
    submit(&mut store, 1, write_op(150, 0x66));
    let mut done = Vec::new();
    polled(&mut store, &mut done, 1, Duration::from_millis(50));
    let early = ended(&done);
    let released = store.release_cut(2);
    disk.release_syncs();
    let store = drained(store, &mut done);
    assert!(early.is_empty(), "{early:?}");
    assert!(released);
    assert_eq!(ended(&done), [(1, Ok(()))], "the write that waited");
    assert!(!store.is_failed(), "a healthy volume failed");
    assert_eq!(phases(&store), []);
    assert_eq!(
        intent_of(&store).syncs.count,
        syncs + 2,
        "the bit set again"
    );
    assert_eq!(intent_of(&store).named, 1);
}

/// Where the map's file cannot sync on a helper (no thread could be
/// started for it), the sync is made where the write waits, and the
/// write goes on after it.
#[test]
fn a_map_whose_file_has_no_helper_is_synced_where_the_write_waits() {
    let disk = created(46, 0);
    let mut store = committed_one(&disk, 0, IoMode::default());
    let syncs = intent_of(&store).syncs.count;
    let begins = disk.stats().sync_begins;
    disk.refuse_sync_begins(1);
    write(&mut store, 150, 0x66).unwrap();
    assert_eq!(disk.stats().sync_begins, begins, "nothing was begun");
    assert_eq!(intent_of(&store).syncs.count, syncs + 1);
    assert_eq!(intent_of(&store).named, 1);
    assert!(!store.is_failed());
}

/// A start that did not keep the map came between (one that held no
/// cuts) and wrote block 150, whose region's bit was clear: the next
/// start finds that start's token, does not take the map up, and the
/// whole volume counts.
#[test]
fn a_map_is_not_taken_up_after_a_start_that_did_not_keep_it() {
    let disk = created(39, 0);
    drop(committed_one(&disk, 0, IoMode::default()));
    let mut between = start(&disk, 0, false).expect("a start with no held cuts");
    write(&mut between, 150, 0x66).unwrap();
    drop(between);
    disk.power_cut(crate::sim::KeepAll);
    let mut store = start_as(&disk, 0, &region(), IoMode::default(), AFTER_A_REBOOT);
    assert_eq!(
        store.intent_at_start(),
        Some(&IntentAtStart::Begun(intent::Distrust::Token.to_string()))
    );
    group(&mut store, 200, 8, 80);
    assert_eq!(store.cut_map(1).len() as u64, VOLUME_BLOCKS);
}

/// A start that cannot write and sync the intent map is refused before
/// it serves; the volume has not failed. The next start serves: it
/// takes up a map that names nothing, the refused start having written
/// nothing to the volume, or, where the failed sync tore the map's
/// header (as the simulated disk may), begins a new one.
#[test]
fn a_start_that_cannot_keep_the_map_is_refused() {
    let disk = created(40, 0);
    drop(committed_one(&disk, 0, IoMode::default()));
    disk.fail_sync_of(intent::INTENT_NAME);
    let options = OpenOptions {
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(region()),
            boot: state::tests::BOOT,
            bound: Some(TEST_BOUND),
            intent_region: Some(TEST_INTENT_REGION),
            intent_header_sync: false,
        }),
        ..OpenOptions::default()
    };
    let started = Opened::open(disk.clone(), lock(&disk).expect("the lock"), options)
        .map_err(|(e, _)| e)
        .expect("opens")
        .start_with(None, Mutants::default());
    assert!(
        matches!(started, Err(StartError::Intent(_))),
        "{:?}",
        started.map(|_| ())
    );
    drop(started);
    disk.clear_faults();
    let store = start_as(&disk, 0, &region(), IoMode::default(), AFTER_A_REBOOT);
    assert!(
        matches!(
            store.intent_at_start(),
            Some(IntentAtStart::TakenUp { named: 0, .. } | IntentAtStart::Begun(_))
        ),
        "{:?}",
        store.intent_at_start()
    );
    assert!(!store.is_failed());
}

/// A failure acknowledged in the same boot costs only the regions the
/// map names, not the whole volume (the lead's question 1 on M2.3): no
/// write began before its bit was durable, so to the map a failed
/// volume is a power cut. Block 150 (region 2) was written after cut 1
/// was committed; then a write of block 200 (region 3) fails, at the
/// image or at the map's sync, and the volume fails. The start that
/// acknowledges the failure does not take the cut state up, but takes
/// the map up: the next cut holds regions 2 and 3, or region 2 alone
/// where the map's failed sync lost region 3's bit (the write of block
/// 200 never began then), and not block 5. Where the failed sync tore
/// the bit page, the map is not taken up and the whole volume counts.
///
/// A start that does not acknowledge the failure comes first and is
/// refused for the marker. It writes no start token, so the start that
/// acknowledges finds the failed instance's token in the map (the third
/// point the review of #105 lists under (b)).
#[test]
fn a_start_that_acknowledges_a_failure_takes_the_map_up() {
    enum Failing {
        Image,
        MapSync,
    }
    for (seed, failing) in [(41, Failing::Image), (42, Failing::MapSync)] {
        let disk = created(seed, 0);
        let mut store = committed_one(&disk, 0, IoMode::default());
        write(&mut store, 150, 0x66).unwrap();
        match failing {
            Failing::Image => disk.fail_write_of(crate::store::HOME_NAME),
            Failing::MapSync => disk.fail_sync_of(intent::INTENT_NAME),
        }
        assert_eq!(write(&mut store, 200, 0x77), Err(BlkError::Failed));
        assert!(store.is_failed());
        drop(store);
        disk.clear_faults();
        // A start that does not acknowledge the failure is refused for
        // the marker first, and writes no start token: the one that
        // acknowledges it finds the failed instance's token in the map.
        assert!(matches!(
            opened(&disk, false, IoMode::default()),
            Err(OpenError::MarkedFailed)
        ));
        let acknowledging = Coming {
            boot: state::tests::BOOT,
            acknowledging: true,
            taken_over: true,
        };
        let mut store = start_as(&disk, 0, &region(), IoMode::default(), acknowledging);
        assert_eq!(
            store.cuts_at_start(),
            Some(&CutsTakenUp::Begun(
                "the start acknowledges a failure".to_owned()
            ))
        );
        let named = match (store.intent_at_start(), &failing) {
            (Some(IntentAtStart::TakenUp { named, regions: 4 }), _) => *named,
            (Some(IntentAtStart::Begun(why)), Failing::MapSync)
                if (0..2).any(|page| *why == intent::Distrust::Torn(page).to_string()) =>
            {
                4
            }
            (other, _) => panic!("{other:?}"),
        };
        group(&mut store, 200, 8, 80);
        let map = store.cut_map(1);
        // The regions named, and the slots the group itself wrote.
        let blocks = |regions: std::ops::Range<u64>| -> Vec<u64> {
            let mut blocks: Vec<u64> = regions.flat_map(|r| r * 64..(r + 1) * 64).collect();
            blocks.extend((0..LABELS).map(|label| (ring(label) + (8 % RING_SLOTS) * SLOT) / BLOCK));
            blocks.sort_unstable();
            blocks.dedup();
            blocks
        };
        match (failing, named) {
            (Failing::Image, 2) => assert_eq!(map, blocks(2..4)),
            (Failing::MapSync, 1) => assert_eq!(map, blocks(2..3)),
            (Failing::MapSync, 2) => assert_eq!(map, blocks(2..4)),
            (Failing::MapSync, 4) => assert_eq!(map.len() as u64, VOLUME_BLOCKS),
            (_, named) => panic!("{named} regions named"),
        }
    }
}

/// A step of the store-level programs of the intent map.
#[derive(Debug, Clone, Copy)]
enum IntentStep {
    /// Write a block whole with one byte.
    Write(u64, u8),
    /// Write zeroes over a block (G1 of the review of #106: a
    /// write-zeroes asks the map as a write does).
    Zeroes(u64),
    /// A transaction group: its uberblock to the four labels, then the
    /// flush that cuts.
    Group,
    /// The cut being uploaded is committed (released).
    Release,
}

fn intent_program(seed: u64, len: usize) -> Vec<IntentStep> {
    // SplitMix64.
    let mut state = seed ^ 0x1D7E_4A11;
    let mut below = |n: u64| {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % n
    };
    (0..len)
        .map(|_| match below(20) {
            0..=8 => {
                let block = below(VOLUME_BLOCKS);
                IntentStep::Write(block, 1 + below(250) as u8)
            }
            9 | 10 => IntentStep::Zeroes(below(VOLUME_BLOCKS)),
            11..=15 => IntentStep::Group,
            _ => IntentStep::Release,
        })
        .collect()
}

/// The volume as the store reads it now.
fn volume_now(store: &mut InPlace<SimDisk>) -> Vec<Vec<u8>> {
    (0..VOLUME_BLOCKS)
        .map(|block| read(store, block).expect("a read"))
        .collect()
}

/// What one run of the store-level test saw.
struct AfterTheCut {
    /// The disk halted before the program ended.
    halted: bool,
    /// The start after the cut took the map up.
    taken: bool,
    /// It took the map up and named fewer than every region.
    narrowed: bool,
}

/// Run `steps` on a new volume whose disk halts after `halt` of its
/// writes and syncs (none: never), cut the power, start as after a
/// restart of the machine, and check that the next cut holds every
/// block that differs from the newest released cut (M2.3's row). Under
/// `garbage` the cut may leave a lost sector as garbage.
fn intent_power_cut(
    seed: u64,
    steps: &[IntentStep],
    mutants: u128,
    halt: Option<u64>,
    garbage: bool,
) -> Result<AfterTheCut, String> {
    let disk = created(seed, 0);
    let held = region();
    let mut store = start_on(&disk, mutants, &held, ON_CALLER);
    // The volume at each cut's instant, by number, and the cut state's
    // region as it was then.
    let mut instants: Vec<Vec<Vec<u8>>> = Vec::new();
    let mut states: Vec<state::MemRegion> = Vec::new();
    let mut released: Option<u64> = None;
    let mut txg = 10;
    let mut id = 1000;
    if let Some(halt) = halt {
        disk.halt_after(halt);
    }
    let mut halted = false;
    'program: for &step in steps {
        match step {
            IntentStep::Write(block, byte) => {
                if write(&mut store, block, byte).is_err() {
                    halted = true;
                    break;
                }
            }
            IntentStep::Zeroes(block) => {
                id += 1;
                let zeroes = Op::WriteZeroes {
                    offset: block * BLOCK,
                    len: BLOCK,
                };
                if run(&mut store, id, zeroes).result.is_err() {
                    halted = true;
                    break;
                }
            }
            IntentStep::Group => {
                txg += 1;
                for label in 0..LABELS {
                    id += 1;
                    if run(&mut store, id, uberblock_write(label, txg, txg * 3))
                        .result
                        .is_err()
                    {
                        halted = true;
                        break 'program;
                    }
                }
                id += 1;
                if run(&mut store, id, Op::Flush).result.is_err() {
                    halted = true;
                    break;
                }
                instants.push(volume_now(&mut store));
                states.push(held.snapshot());
            }
            IntentStep::Release => {
                let Some(oldest) = store.held_cuts().first().copied() else {
                    continue;
                };
                if oldest.phase == CutPhase::Uploading && store.release_cut(oldest.number) {
                    released = Some(oldest.number);
                }
                if store.is_failed() {
                    halted = true;
                    break;
                }
            }
        }
    }
    drop(store);
    if garbage {
        disk.power_cut(crate::sim::SeededCut::with_garbage(seed));
    } else {
        disk.power_cut(crate::sim::SeededCut::new(seed));
    }
    // What survives of the cut state (the lead's reading of M2.3's
    // row): nothing, as of a run directory on tmpfs; the region as the
    // power cut left it, as of one on a disk; or a well-formed state
    // from an earlier group. The start, in another boot, takes none of
    // them up, and the map is what narrows the next cut.
    let survived = match seed % 3 {
        0 => region(),
        1 => held,
        _ => states
            .get(seed as usize % states.len().max(1))
            .cloned()
            .unwrap_or_else(region),
    };
    let mut store = start_as(&disk, mutants, &survived, ON_CALLER, AFTER_A_REBOOT);
    if !matches!(store.cuts_at_start(), Some(CutsTakenUp::Begun(_))) {
        return Err(format!(
            "a cut state of another boot was taken up: {:?} (halt {halt:?})",
            store.cuts_at_start()
        ));
    }
    let narrowed = match store.intent_at_start() {
        Some(IntentAtStart::TakenUp { named, regions }) => named < regions,
        Some(IntentAtStart::Begun(_)) => {
            // The whole volume is the next cut, which holds everything.
            return Ok(AfterTheCut {
                halted,
                taken: false,
                narrowed: false,
            });
        }
        None => return Err("the store keeps no intent map".to_owned()),
    };
    let Some(number) = released else {
        return Err("a map was taken up and no cut was committed".to_owned());
    };
    let base = &instants[number as usize - 1];
    let now = volume_now(&mut store);
    group(&mut store, 5000, 900, 901);
    let map: std::collections::BTreeSet<u64> = store.cut_map(1).into_iter().collect();
    for block in 0..VOLUME_BLOCKS {
        if now[block as usize] != base[block as usize] && !map.contains(&block) {
            return Err(format!(
                "block {block} differs from cut {number} and the next cut lacks it \
                 (halt {halt:?}, halted {halted})"
            ));
        }
    }
    Ok(AfterTheCut {
        halted,
        taken: true,
        narrowed,
    })
}

/// How the starts after the power cuts of the store-level test went.
#[derive(Debug, Default, Clone, Copy)]
struct Starts {
    all: u64,
    taken: u64,
    narrowed: u64,
}

/// Every prefix of the program of `seed`: the disk halts after each of
/// its writes and syncs in turn, then the power is cut.
fn intent_every_prefix(seed: u64, mutants: u128, garbage: bool) -> Result<Starts, String> {
    let steps = intent_program(seed, 30);
    let mut starts = Starts::default();
    for halt in 0.. {
        let after = intent_power_cut(seed, &steps, mutants, Some(halt), garbage)
            .map_err(|e| format!("seed {seed}: {e}"))?;
        starts.all += 1;
        starts.taken += u64::from(after.taken);
        starts.narrowed += u64::from(after.narrowed);
        if !after.halted {
            break;
        }
    }
    Ok(starts)
}

/// M2.3's row on the simulated disk: with the power cut at every prefix
/// of random programs and the intent map's file left as a power cut
/// leaves it (a well-formed one from before the writes no sync covered
/// included, and with garbage in lost sectors), the next cut holds every
/// block that differs from the newest committed cut. The cut state's
/// file is gone after the power cut, or survives as it was then or as
/// it was at an earlier group (by seed); the start, in another boot,
/// takes none of them up (the lead's reading of the row's "state file").
///
/// The test also requires that the map does its work in a fair share of
/// the starts, a third taken up and a tenth narrowed: of 1874, 794 take
/// it up and 268 of those name fewer than every region. A start before the first commit finds no base, and in
/// the tests' volume of 1 MiB every region holds a label, which every
/// group writes. Before the directory was synced after the map's
/// creation (D1 of the review of #106), a power cut that lost the
/// creation left no map, and half as many starts took one up.
#[test]
fn after_a_power_cut_the_next_cut_holds_every_block_changed_since_the_newest_commit() {
    let mut starts = Starts::default();
    for seed in 0..24 {
        let seen = intent_every_prefix(seed, 0, seed % 2 == 1).unwrap_or_else(|e| panic!("{e}"));
        starts.all += seen.all;
        starts.taken += seen.taken;
        starts.narrowed += seen.narrowed;
    }
    assert!(
        starts.taken * 3 > starts.all && starts.narrowed * 10 > starts.all,
        "the map did too little: {starts:?}"
    );
}

/// Each defect of the intent map that the store's own rules guard fails
/// the store-level test. A torn page taken up as it reads is the model
/// test's (`intent/model.rs`): here the four regions' bits lie in the
/// page's first sector, beside its number, which is checked whatever
/// the checksum says.
#[test]
fn the_store_level_test_notices_each_defect_of_the_intent_map() {
    for mutant in [
        Mutants::INTENT_WRITE_UNCOVERED,
        Mutants::INTENT_CLEARED_CHANGED,
        Mutants::INTENT_NOT_CARRIED,
        // A cut state that survived on a disk, of another boot, taken
        // up.
        Mutants::BOOT_UNCHECKED,
    ] {
        let caught = (0..24).any(|seed| intent_every_prefix(seed, mutant, seed % 2 == 1).is_err());
        assert!(caught, "the defect {mutant:#x} passed");
    }
}

/// The row in object storage is not what the held cuts were made
/// against (M2.4, the uploader's check): the store abandons every held
/// cut, counts each, and lets their copies go; its next cut holds the
/// whole volume as at its instant. The committed cut that the intent
/// map names stays as it was: nothing new was committed.
#[test]
fn a_mismatch_abandons_every_held_cut_and_the_next_cut_is_whole() {
    let (_disk, mut store) = two_cuts_with_a_copy(66, ON_CALLER);
    let id = store.cut_state_id().expect("the state's name");
    assert!(store.cut_is_whole(1), "the first cut of a new state");
    assert!(!store.release_cut(2), "cut 2 waits: it is not committed");
    assert_eq!(store.committed_cut(), None, "a new map names no cut");
    assert!(store.release_cut(1));
    let committed = Some(CommittedCut {
        state: id,
        number: 1,
        group: 7,
    });
    assert_eq!(store.committed_cut(), committed);
    // Block 70 is in cut 2's map: cut 2 reads it from a copy now.
    write(&mut store, 70, 0xD0).unwrap();
    write(&mut store, 9, 0x99).unwrap();
    group(&mut store, 300, 9, 90);
    assert!(!store.cut_is_whole(2), "cut 2 holds its own changes");
    assert!(store.cut_figures().copied > 0);
    let abandoned = store.cut_figures().abandoned;
    assert_eq!(store.abandon_all_cuts(), [2, 3]);
    assert!(store.held_cuts().is_empty());
    let figures = store.cut_figures();
    assert_eq!(figures.abandoned, abandoned + 2, "each counted once");
    assert_eq!(figures.copied, 0, "the copies go with the cuts");
    assert_eq!(store.committed_cut(), committed);
    write(&mut store, 11, 0xBB).unwrap();
    group(&mut store, 400, 10, 100);
    let four = store.held_cuts().last().map(|c| c.number);
    assert_eq!(four, Some(4));
    assert!(store.cut_is_whole(4));
    assert_eq!(store.cut_map(4).len() as u64, VOLUME_BLOCKS);
    write(&mut store, 70, 0xE0).unwrap();
    assert_eq!(cut_block(&mut store, 4, 70), vec![0xD0; BLOCK_USIZE]);
    assert_eq!(cut_block(&mut store, 4, 11), vec![0xBB; BLOCK_USIZE]);
    assert_eq!(cut_block(&mut store, 4, 3), vec![0x33; BLOCK_USIZE]);
}

/// The commit in object storage landed and the daemon died before it
/// let the cut go (M2.4, the lead's kill point for the uploader's check
/// (a)): at every store of the release, the next daemon either holds
/// the cut as the one being uploaded, under the same state's name, so
/// that check (a) finds it in the row and releases it; or the cut is
/// gone and the intent map already names it as the committed cut, so
/// that the next cut's base is the row. The intent map names the cut
/// before the cut state lets it go: the other order leaves a death in
/// which the cut is gone and the intent map names the cut before it,
/// and the uploader's check finds a mismatch.
#[test]
fn a_death_after_the_commit_leaves_the_cut_to_release_or_named_as_committed() {
    state::tests::quiet_deaths();
    let mut deaths = 0;
    for death in 0u64.. {
        let disk = created(67, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, ON_CALLER);
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        write(&mut store, 6, 0x66).unwrap();
        group(&mut store, 200, 8, 80);
        let id = store.cut_state_id().expect("the state's name");
        let committed = Some(CommittedCut {
            state: id,
            number: 1,
            group: 7,
        });
        region.die_at(death);
        let lived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.release_cut(1)));
        region.revive();
        let died = match lived {
            Ok(released) => {
                assert!(released);
                false
            }
            Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => true,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        if died {
            deaths += 1;
            drop(store);
            disk.kill();
            store = start_on(&disk, 0, &region, ON_CALLER);
            assert_eq!(
                store.cut_state_id(),
                Some(id),
                "a death at store {death}: the state keeps its name"
            );
            let one = store.held_cuts().into_iter().find(|c| c.number == 1);
            if let Some(one) = one {
                assert_eq!(
                    (one.phase, one.state.txg),
                    (CutPhase::Uploading, 7),
                    "a death at store {death}"
                );
                assert!(store.release_cut(1), "a death at store {death}");
            }
        }
        assert_eq!(store.committed_cut(), committed, "a death at store {death}");
        let next = store.held_cuts().first().map(|c| (c.number, c.phase, c.lo));
        assert_eq!(
            next,
            Some((2, CutPhase::Uploading, 1)),
            "a death at store {death}"
        );
        if !died {
            break;
        }
    }
    assert!(deaths > 0);
}

// ---- the intent map's header synced at a release (M2.4d) ----

/// Cut 1 (block 5 at 0x55, group 7) held and being uploaded, on a store
/// that takes the device over and syncs the map's header at each
/// release, or not.
fn one_cut(
    disk: &SimDisk,
    region: &state::MemRegion,
    io: IoMode,
    header_sync: bool,
) -> InPlace<SimDisk> {
    let mut store = start_syncing_header(disk, 0, region, io, TAKING_OVER, header_sync);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    store
}

/// One poll, which waits for nothing: it sees the end of a sync of the
/// map that has ended (with no delay, the simulated disk's begun syncs
/// end at the first look, unless it holds them).
fn settle_map(store: &mut InPlace<SimDisk>) {
    let mut done = Vec::new();
    store.poll(&mut done);
    assert!(done.is_empty(), "{:?}", ended(&done));
}

/// The header that names the cut a release lets go is synced only when
/// the store is told to (M2.4d, for the lead's Q7 of M2.4b; the daemon
/// tells it to unless `serve --no-intent-header-sync` is given): one
/// sync of the map, begun on its helper and counted apart from the write
/// path's, and a power cut after it keeps the base, so the start after a
/// restart of the machine takes the map up naming cut 1. Off, as built
/// in M2.3, the release syncs nothing, and the power cut that loses
/// everything no sync covered leaves the map as the start wrote it,
/// naming no cut: it is begun anew, and the next cut is the whole
/// volume. The defects, planted by hand: the sync not begun, and the
/// sync counted as the write path's.
#[test]
fn a_release_syncs_the_header_only_when_told_to() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        for header_sync in [false, true] {
            let case = format!("{io:?}, header sync {header_sync}");
            let disk = created(91, 0);
            let region = region();
            let mut store = one_cut(&disk, &region, io, header_sync);
            let id = store.cut_state_id().expect("a cut state");
            let before = intent_of(&store);
            let syncs = disk.syncs_of(intent::INTENT_NAME);
            assert!(store.release_cut(1), "{case}");
            settle_map(&mut store);
            let after = intent_of(&store);
            let once = u64::from(header_sync);
            assert_eq!(after.header_sync, header_sync, "{case}");
            assert_eq!(
                after.header_syncs.count,
                before.header_syncs.count + once,
                "{case}"
            );
            assert_eq!(after.syncs, before.syncs, "{case}: none of the write path");
            assert_eq!(after.waits, before.waits, "{case}");
            assert_eq!(disk.syncs_of(intent::INTENT_NAME) - syncs, once, "{case}");
            drop(store);
            disk.power_cut(crate::sim::LoseAll);
            let store = start_as(&disk, 0, &region, io, AFTER_A_REBOOT);
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
}

/// A header written while a sync of the map is under way, which does
/// not cover it, is synced by the next one, which begins when that one
/// ends though no write waits: here cut 1's release begins a sync the
/// disk holds, cut 2's release writes the header that names cut 2 and
/// begins no second sync beside the first, and once the disk lets the
/// first end, a second follows. A power cut then keeps cut 2 as the
/// base. Settled by polls, or by the store's drop, which waits for both
/// (on a thread of the test's, so that a drop that does not end fails
/// the test and does not hang it). The defects, planted by hand: the
/// header due not synced after the sync under way, and the drop that
/// does not wait for a header's sync.
#[test]
fn a_header_written_while_the_map_syncs_is_synced_after_it() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        for by_drop in [false, true] {
            let case = format!("{io:?}, settled by the drop {by_drop}");
            let disk = created(92, 0);
            let region = region();
            let mut store = one_cut(&disk, &region, io, true);
            write(&mut store, 6, 0x66).unwrap();
            group(&mut store, 200, 8, 80);
            let id = store.cut_state_id().expect("a cut state");
            let syncs = disk.syncs_of(intent::INTENT_NAME);
            disk.hold_syncs_of(intent::INTENT_NAME);
            assert!(store.release_cut(1), "{case}");
            assert!(store.release_cut(2), "{case}");
            settle_map(&mut store);
            assert_eq!(
                disk.syncs_of(intent::INTENT_NAME) - syncs,
                1,
                "{case}: one sync at a time"
            );
            disk.release_syncs();
            if by_drop {
                let (tell, dropped) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    drop(store);
                    let _ = tell.send(());
                });
                assert_eq!(
                    dropped.recv_timeout(DRAIN),
                    Ok(()),
                    "{case}: the drop did not end"
                );
            } else {
                settle_map(&mut store);
                settle_map(&mut store);
                assert_eq!(intent_of(&store).header_syncs.count, 2, "{case}");
                drop(store);
            }
            assert_eq!(disk.syncs_of(intent::INTENT_NAME) - syncs, 2, "{case}");
            disk.power_cut(crate::sim::LoseAll);
            let store = start_as(&disk, 0, &region, io, AFTER_A_REBOOT);
            let two = CommittedCut {
                state: id,
                number: 2,
                group: 8,
            };
            assert_eq!(store.committed_cut(), Some(two), "{case}");
        }
    }
}

/// Where a committed cut the store no longer holds is named
/// ([`InPlace::name_committed_cut`]), the header is synced as at a
/// release when the store is told to, and only then does a power cut
/// keep it. The defect, planted by hand: the naming syncs nothing.
#[test]
fn naming_a_committed_cut_syncs_the_header_only_when_told_to() {
    for header_sync in [false, true] {
        let case = format!("header sync {header_sync}");
        let disk = created(93, 0);
        let region = region();
        let mut store = one_cut(&disk, &region, ON_CALLER, header_sync);
        let id = store.cut_state_id().expect("a cut state");
        assert_eq!(store.abandon_all_cuts(), [1], "{case}");
        assert!(store.name_committed_cut(1, 7), "{case}");
        settle_map(&mut store);
        assert_eq!(
            intent_of(&store).header_syncs.count,
            u64::from(header_sync),
            "{case}"
        );
        drop(store);
        disk.power_cut(crate::sim::LoseAll);
        let store = start_as(&disk, 0, &region, ON_CALLER, AFTER_A_REBOOT);
        let one = CommittedCut {
            state: id,
            number: 1,
            group: 7,
        };
        assert_eq!(store.committed_cut(), header_sync.then_some(one), "{case}");
    }
}

/// A header's sync that fails fails the volume, as a sync of the map
/// for a write does, and the next write fails with it. The release let
/// the cut go when it wrote the header, before the sync ended, as it
/// does with no sync: the disk then holds the header as written or as
/// it was, and a start that acknowledges the failure takes the map up
/// as after a power cut. The defect, planted by hand: a header's sync
/// whose failure is not looked at.
#[test]
fn a_header_sync_that_fails_fails_the_volume() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(94, 0);
        let mut store = one_cut(&disk, &region(), io, true);
        disk.fail_sync_of(intent::INTENT_NAME);
        assert!(store.release_cut(1), "{case}");
        assert_eq!(phases(&store), [], "{case}: let go at the write");
        settle_map(&mut store);
        assert!(store.is_failed(), "{case}");
        assert_eq!(write(&mut store, 6, 0x66), Err(BlkError::Failed), "{case}");
    }
}

// The review of #120 (M2.4d) found these cases untested (G1 to G5,
// F2 and F3 there); its scenarios, kept as tests.

/// The store's drop on a thread of the test's: whether it ended within
/// `time`.
fn dropped_within(
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

/// A write into a region the release just cleared comes while the
/// header's sync is under way: it waits for that sync and then for one
/// of its own. Each sync of the map's file is counted once, as the
/// header's or as the write path's, the volume stays healthy, and after
/// a power cut the map names the released cut and the write's region.
#[test]
fn a_write_behind_a_header_sync_waits_for_it_then_for_a_sync_of_its_own() {
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
        assert_eq!(
            after.header_syncs.count,
            before.header_syncs.count + 1,
            "{case}"
        );
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

/// A second release while a write waits behind the first release's
/// header sync: its clear takes the write's bit and its header is due.
/// The next sync is the write path's, covers that header too, and is
/// counted with the write path's; no third sync follows. Settled by
/// polls or by the store's drop.
#[test]
fn a_header_due_while_a_write_waits_is_covered_by_the_writes_sync() {
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
                let (_dropped, ended) = dropped_within(store, DRAIN);
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

/// A header is due when the sync under way fails the volume: no
/// sync follows, and the store's drop ends.
#[test]
fn a_header_due_on_a_failed_volume_does_not_hold_the_drop() {
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
        let (_dropped, ended) = dropped_within(store, DRAIN);
        assert!(ended, "{case}: the drop did not end");
        assert_eq!(
            disk.syncs_of(intent::INTENT_NAME) - syncs,
            1,
            "{case}: no sync after the failure"
        );
    }
}

/// Where the map's file cannot sync on a helper, the header's sync
/// is made at the release: counted as a header's with no poll, and a
/// power cut keeps the base.
#[test]
fn a_header_sync_with_no_helper_is_made_at_the_release() {
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

/// A header's sync that fails at once (made at the release, where
/// the file has no helper): the volume fails; `release_cut` answers true
/// and keeps the cut, as `sync_header` and `release_cut` say of a
/// failure at once, where a failure found later has let it go
/// (`a_header_sync_that_fails_fails_the_volume`). A naming answers
/// false.
#[test]
fn a_header_sync_that_fails_at_once_keeps_the_cut() {
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

/// Random programs of writes, groups and releases with the header
/// synced at each release, polled once after every step. Once the map
/// has settled: every sync of the map's file since the start is counted
/// once, as a header's or as the write path's; the volume is healthy;
/// and after a power cut that loses everything no sync covered, the map
/// is taken up naming the newest released cut, and the next cut holds
/// every block that differs from it.
#[test]
fn with_the_header_synced_random_programs_keep_the_newest_released_cut_as_the_base() {
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
                    assert!(
                        run(&mut store, rid, write_op(block, byte)).result.is_ok(),
                        "{case}"
                    );
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
    assert!(
        with_a_release >= 20,
        "{with_a_release} programs released a cut"
    );
}

/// With the option off (and on), a write waits for the map's sync,
/// which the disk holds, and the volume fails for another reason: the
/// store's drop ends without waiting for that sync, as before M2.4d. A
/// failed volume syncs no write any more, so nothing is gained by
/// holding the drop, and the lock, for it (F3 of #120's review).
#[test]
fn the_drop_of_a_failed_store_does_not_wait_for_the_maps_sync_under_way() {
    for header_sync in [false, true] {
        let case = format!("header sync {header_sync}");
        let disk = created(107, 0);
        let mut store = if header_sync {
            // The cut released and its header's sync ended, so that the
            // write's region is clean and the write waits for the map.
            let mut store = one_cut(&disk, &region(), IoMode::default(), true);
            assert!(store.release_cut(1), "{case}");
            settle_map(&mut store);
            settle_map(&mut store);
            assert_eq!(intent_of(&store).header_syncs.count, 1, "{case}");
            store
        } else {
            committed_one(&disk, 0, IoMode::default())
        };
        assert_eq!(intent_of(&store).header_sync, header_sync, "{case}");
        disk.hold_syncs_of(intent::INTENT_NAME);
        submit(&mut store, 1, write_op(150, 0x66));
        let mut done = Vec::new();
        polled(&mut store, &mut done, 1, Duration::from_millis(50));
        assert!(done.is_empty(), "{case}: {:?}", ended(&done));
        store.fail_now();
        polled(&mut store, &mut done, 1, DRAIN);
        assert_eq!(ended(&done), [(1, Err(BlkError::Failed))], "{case}");
        let (dropped, early) = dropped_within(store, Duration::from_millis(500));
        disk.release_syncs();
        let late = early || dropped.recv_timeout(DRAIN).is_ok();
        assert!(
            late,
            "{case}: the drop did not end once the sync was let go"
        );
        assert!(
            early,
            "{case}: the drop of a failed store waited for the map's sync under way"
        );
    }
}

/// A header's sync is under way, with no write waiting, when the volume
/// fails: the drop ends without it (F-B of #124's review, its scenario
/// n1). The test above has a write's sync under way in both its cases.
#[test]
fn the_drop_of_a_failed_store_does_not_wait_for_a_headers_sync_under_way() {
    let disk = created(201, 0);
    let mut store = one_cut(&disk, &region(), IoMode::default(), true);
    let syncs = disk.syncs_of(intent::INTENT_NAME);
    disk.hold_syncs_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    settle_map(&mut store);
    assert_eq!(disk.syncs_of(intent::INTENT_NAME) - syncs, 1);
    assert_eq!(intent_of(&store).header_syncs.count, 0, "under way still");
    store.fail_now();
    let (dropped, early) = dropped_within(store, Duration::from_millis(500));
    disk.release_syncs();
    let late = early || dropped.recv_timeout(DRAIN).is_ok();
    assert!(late, "the drop did not end once the sync was let go");
    assert!(
        early,
        "the drop of a failed store waited for the header's sync under way"
    );
}

/// A release that comes to a store that has failed already, with the
/// header's sync on: the cut is let go and a sync of the map begins, and
/// the drop does not wait for it (F-A of #124's review, its scenario
/// n3). The next start acknowledges the failure and takes the map up, so
/// the header that sync covers still decides the next cut's base.
#[test]
fn a_release_on_a_failed_store_begins_a_sync_the_drop_does_not_wait_for() {
    let disk = created(203, 0);
    let mut store = one_cut(&disk, &region(), IoMode::default(), true);
    store.fail_now();
    let syncs = disk.syncs_of(intent::INTENT_NAME);
    disk.hold_syncs_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    assert_eq!(phases(&store), []);
    assert_eq!(disk.syncs_of(intent::INTENT_NAME) - syncs, 1);
    let (dropped, early) = dropped_within(store, Duration::from_millis(500));
    disk.release_syncs();
    let late = early || dropped.recv_timeout(DRAIN).is_ok();
    assert!(late, "the drop did not end once the sync was let go");
    assert!(early, "the drop of a failed store waited for the sync");
}
