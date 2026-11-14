//! The count of cuts (design §5.2; work package M2.1): each rule of
//! `cuts.rs` with a test that its defect fails.
//!
//! The volume is the tests' 1 MiB: four labels and nothing between
//! them, the rings at 128, 384, 640 and 896 KiB. Uberblocks are written
//! as a pool with `ashift` 12 writes them, one 4 KiB slot each, the slot
//! chosen by the group.

use skyzfs_uberblock::{Labels, MAGIC};

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

/// The uberblock write of group `txg` and root `root` to label `label`,
/// in the slot the group chooses.
fn uberblock_write(label: usize, txg: u64, root: u64) -> Op {
    Op::Write {
        offset: ring(label) + (txg % (RING_SLOTS)) * SLOT,
        data: uberblock(txg, root),
    }
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
        let mmp = |label: usize, txg: u64, root: u64| Op::Write {
            offset: ring(label) + (RING_SLOTS - 1) * SLOT,
            data: uberblock(txg, root),
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
