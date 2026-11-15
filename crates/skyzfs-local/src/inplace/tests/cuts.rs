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
fn groups_counted(mutants: u64, io: IoMode) -> Vec<(u64, Option<u64>)> {
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
    let counts = |mutants: u64| {
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
    let counts = |mutants: u64| {
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
    let counts = |mutants: u64| {
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
    let counts = |mutants: u64| {
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
    let counts = |mutants: u64| {
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
    let opens = |mutants: u64, io: IoMode| {
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
    let opens = |mutants: u64| {
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
    let counts = |mutants: u64| {
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
    let opens = |mutants: u64| {
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
    let opens = |mutants: u64| {
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
    let counts = |mutants: u64| {
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
    let opens = |mutants: u64, forged: &dyn Fn(u64) -> Buf| {
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
    mutants: u64,
    region: &state::MemRegion,
    io: IoMode,
) -> InPlace<SimDisk> {
    start_as(disk, mutants, region, io, TAKING_OVER)
}

/// Start on `disk` with the cut state in `region`, as `coming` says.
fn start_as(
    disk: &SimDisk,
    mutants: u64,
    region: &state::MemRegion,
    io: IoMode,
    coming: Coming,
) -> InPlace<SimDisk> {
    let options = OpenOptions {
        io,
        acknowledge_failure: coming.acknowledging,
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(region.clone()),
            boot: coming.boot,
            bound: Some(TEST_BOUND),
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
    let read_back = |mutants: u64, io: IoMode| -> Vec<u8> {
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
fn lost_in_a_mark(mutants: u64, io: IoMode, death: u64) -> AfterDeath {
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
fn marks_survive_a_death(mutants: u64, io: IoMode) -> Result<u64, String> {
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
    mutants: u64,
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
    type Case<'a> = (&'a str, u64, Coming, &'a dyn Fn(&SimDisk), &'a str);
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
    let after_the_next_flush = |mutants: u64, taken_over: bool| -> Vec<u64> {
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
    let next_volume = |mutants: u64| -> (CutsTakenUp, Vec<u64>) {
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
fn opened_between(mutants: u64) -> (Option<CutsTakenUp>, Vec<u64>, Option<u8>) {
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
fn acknowledged_and_lost(mutants: u64) -> (Option<CutsTakenUp>, Vec<u64>) {
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
fn acknowledging_open_lost_at(after: u64, mutants: u64) -> (bool, Result<AfterStart, OpenError>) {
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
fn heard_and_lost(mutants: u64) -> (Option<CutsTakenUp>, Vec<u64>) {
    let (taken, held, died) = heard_and_lost_at(0, mutants);
    assert!(died, "the start made no store into the region");
    (taken, held)
}

/// The same with the start that hears the failure lost at store `death`
/// into the region: the next start's outcome and held cuts, and whether
/// the lost start died at all.
fn heard_and_lost_at(death: u64, mutants: u64) -> (Option<CutsTakenUp>, Vec<u64>, bool) {
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
