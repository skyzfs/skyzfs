//! The log store's tests of the contract and of the serving machinery,
//! ported to the in-place store when the log store was removed (M1.3t).
//! What the log store's own tests held of the contract, of the failure
//! path and of the lock, and that no test of the in-place store held
//! yet, is held here. Each test names the one it stands in for, as it
//! was at a234314 (`store::tests`, `log::tests`, `tests/store_real.rs`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::store::{FailureObserver, Waker};
use crate::volume::{Volume, VolumeError, LEGACY_NAME, VOLUME_NAME};

/// A store started on `disk` with `options` and the defects `mutants`
/// planted in the open and in the store.
fn started(
    disk: &SimDisk,
    options: OpenOptions,
    mutants: u32,
) -> Result<InPlace<SimDisk>, OpenError> {
    let opened = Opened::open_with(disk.clone(), lock(disk)?, options, Mutants(mutants))
        .map_err(|(e, _)| e)?;
    Ok(opened
        .start_with(None, Mutants(mutants))
        .expect("the in-place store"))
}

fn flush(store: &mut InPlace<SimDisk>, id: u64) -> Result<(), BlkError> {
    run(store, id, Op::Flush).result
}

fn submit(store: &mut InPlace<SimDisk>, id: u64, op: Op) {
    store
        .submit(Request {
            id: RequestId(id),
            op,
        })
        .unwrap();
}

fn config(size_mib: u64, unit: u32) -> VolumeConfig {
    VolumeConfig {
        volume: *b"in-place volume.",
        size_mib,
        base: Base::Empty,
        write_unit: unit,
    }
}

/// A disk whose page, write unit and file system block are `unit`, and
/// nothing on it.
fn bare(seed: u64, unit: u64) -> SimDisk {
    let disk = SimDisk::new(seed);
    disk.set_write_unit(unit);
    disk.set_page_size(unit);
    disk.set_block_size(Some(unit));
    disk
}

// The failure path.

/// Whether a failed read failed only its own request, on the store with
/// `io` and the defects `mutants`: the volume serves on, reads, writes
/// and flushes, and the next start finds no marker. `Err` says what
/// broke the rule.
fn a_read_error_stays_with_its_read(io: IoMode, mutants: u32) -> Result<(), String> {
    let disk = created(20, 0);
    let mut store = start_io(&disk, mutants, false, io).unwrap();
    write(&mut store, 4, 0x44).unwrap();
    flush(&mut store, 1).unwrap();
    disk.fail_read(0, false);
    match read(&mut store, 4) {
        Err(BlkError::Io(_)) => {}
        other => return Err(format!("the failed read gave {other:?}")),
    }
    if store.is_failed() {
        return Err("the volume failed".into());
    }
    if read(&mut store, 4) != Ok(vec![0x44; BLOCK_USIZE]) {
        return Err("the next read did not succeed".into());
    }
    if write(&mut store, 5, 0x55).is_err() || flush(&mut store, 2).is_err() {
        return Err("a write or a flush after it failed".into());
    }
    drop(store);
    disk.kill();
    match start(&disk, 0, false) {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("the next start: {e}")),
    }
}

/// A failed read fails only that read: it changes nothing, so the
/// volume serves on and is not marked (the in-place rule; the log store
/// failed the volume instead). On the threads and on the caller's
/// thread, which are separate paths. The defect that fails the volume
/// on a failed read is noticed on both. (Stands in for
/// `store::tests::a_failed_read_fails_the_volume` and
/// `a_read_the_reader_cannot_do_fails_the_volume`.)
#[test]
fn a_failed_read_fails_only_its_own_request() {
    for io in [IoMode::default(), OTHER_IO[1], BUFFERED] {
        assert_eq!(a_read_error_stays_with_its_read(io, 0), Ok(()), "{io:?}");
        assert!(
            a_read_error_stays_with_its_read(io, Mutants::READ_ERROR_FAILS).is_err(),
            "{io:?}: the defect is not noticed"
        );
    }
}

/// A flush with nothing to make durable touches no disk: on a disk
/// whose syncs fail it succeeds and leaves the volume serving, and the
/// disk sees no write and no sync. A flush with work then meets the
/// failure and fails the volume. (Stands in for
/// `store::tests::a_flush_with_nothing_to_do_costs_nothing_and_one_with_work_finds_the_failure`.)
#[test]
fn a_flush_with_nothing_to_do_does_not_meet_a_failing_disk() {
    let disk = created(21, 0);
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, 4, 0x44).unwrap();
    flush(&mut store, 1).unwrap();
    let before = disk.stats();
    let clean = store.counts().clean;
    disk.fail_sync(0, true);
    assert_eq!(flush(&mut store, 2), Ok(()));
    assert_eq!(store.counts().clean, clean + 1);
    assert!(!store.is_failed());
    let after = disk.stats();
    assert_eq!(
        (after.writes, after.syncs, after.dir_syncs),
        (before.writes, before.syncs, before.dir_syncs)
    );
    write(&mut store, 5, 0x55).unwrap();
    assert_eq!(flush(&mut store, 3), Err(BlkError::Failed));
    assert!(store.is_failed());
}

/// How a volume is made to fail in [`the_failure_is_told_before_the_marker`].
#[derive(Debug, Clone, Copy)]
enum Failing {
    Sync,
    Write,
    /// Two writes on the threads at once, both failing: the failure is
    /// met twice.
    Writes,
    Now,
    Halted,
}

/// What the failure observer saw: how often it was called, and whether
/// the marker was in the directory at any call.
fn observed(how: Option<Failing>, mutants: u32) -> (u64, bool) {
    let disk = created(22, 0);
    let calls = Arc::new(AtomicU64::new(0));
    let saw_marker = Arc::new(AtomicBool::new(false));
    let options = OpenOptions {
        on_failure: Some({
            let (calls, saw_marker, disk) = (calls.clone(), saw_marker.clone(), disk.clone());
            FailureObserver::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                if disk
                    .list()
                    .is_ok_and(|names| names.iter().any(|n| n == MARKER_NAME))
                {
                    saw_marker.store(true, Ordering::SeqCst);
                }
            })
        }),
        ..OpenOptions::default()
    };
    let mut store = started(&disk, options, mutants).unwrap();
    write(&mut store, 4, 0x44).unwrap();
    match how {
        None => {
            flush(&mut store, 1).unwrap();
        }
        Some(Failing::Sync) => {
            disk.fail_sync(0, false);
            assert_eq!(flush(&mut store, 1), Err(BlkError::Failed));
        }
        Some(Failing::Write) => {
            disk.fail_write(0, false);
            assert!(write(&mut store, 5, 0x55).is_err());
        }
        Some(Failing::Writes) => {
            disk.fail_write(0, true);
            for block in [5, 6] {
                let data = Buf::filled(1, 0x55);
                submit(
                    &mut store,
                    block,
                    Op::Write {
                        offset: block * BLOCK,
                        data,
                    },
                );
            }
            let mut done = Vec::new();
            while done.len() < 2 {
                store.wait(&mut done, WAIT);
            }
            assert!(done.iter().all(|c| c.result.is_err()), "{done:?}");
        }
        Some(Failing::Now) => {
            store.fail_now();
            store.fail_now();
        }
        Some(Failing::Halted) => {
            disk.halt_after(0);
            assert!(flush(&mut store, 1).is_err());
        }
    }
    assert_eq!(store.is_failed(), how.is_some());
    if how.is_some() {
        // A request after the failure meets it again.
        assert!(write(&mut store, 7, 0x77).is_err());
    }
    drop(store);
    (
        calls.load(Ordering::SeqCst),
        saw_marker.load(Ordering::SeqCst),
    )
}

/// The volume's owner is told of a failure once, when it happens (also
/// when two writes meet it, and for a request after it), and
/// before the marker is in the directory (design §4.2: the failure is
/// announced first; the marker may take minutes on a sick disk). A
/// volume that does not fail never tells it. The defect that begins the
/// marker first is noticed, with the marker written inline so that the
/// order is fixed. (Stands in for
/// `store::tests::the_failure_is_announced_before_the_marker_is_attempted`
/// and `log::tests::the_observer_is_told_before_the_marker_is_attempted`.)
#[test]
fn the_failure_is_told_before_the_marker() {
    assert_eq!(observed(None, 0), (0, false));
    let ways = [
        Failing::Sync,
        Failing::Write,
        Failing::Writes,
        Failing::Now,
        Failing::Halted,
    ];
    for how in ways {
        for mutants in [0, Mutants::MARKER_INLINE] {
            assert_eq!(
                observed(Some(how), mutants),
                (1, false),
                "{how:?} {mutants}"
            );
        }
    }
    for how in [Failing::Sync, Failing::Write, Failing::Now] {
        assert_eq!(
            observed(
                Some(how),
                Mutants::MARKER_INLINE | Mutants::OBSERVE_AFTER_MARKER
            ),
            (1, true),
            "{how:?}: the defect is not noticed"
        );
    }
}

/// A start whose sync fails tells the observer once, and the marker is
/// not in the directory then: the start hands it back to be written by
/// its owner.
#[test]
fn a_start_that_fails_tells_the_observer_before_the_marker() {
    let disk = created(23, 0);
    let calls = Arc::new(AtomicU64::new(0));
    let saw_marker = Arc::new(AtomicBool::new(false));
    let options = OpenOptions {
        on_failure: Some({
            let (calls, saw_marker, disk) = (calls.clone(), saw_marker.clone(), disk.clone());
            FailureObserver::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                if disk
                    .list()
                    .is_ok_and(|names| names.iter().any(|n| n == MARKER_NAME))
                {
                    saw_marker.store(true, Ordering::SeqCst);
                }
            })
        }),
        ..OpenOptions::default()
    };
    let opened = Opened::open(disk.clone(), lock(&disk).unwrap(), options).unwrap();
    disk.fail_sync(0, true);
    let failed = opened.start(None);
    assert!(matches!(failed, Err(StartError::Failed(_))), "{failed:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!saw_marker.load(Ordering::SeqCst));
}

/// Whether the marker of a volume that failed is there after a power
/// cut right after the failure, with the defects `mutants`.
fn marked_after_a_cut(mutants: u32) -> bool {
    let disk = created(24, 0);
    let mut store = start(&disk, mutants, false).unwrap();
    write(&mut store, 4, 0x44).unwrap();
    disk.fail_sync(0, false);
    assert_eq!(flush(&mut store, 1), Err(BlkError::Failed));
    // The drop waits for the marker's thread.
    drop(store);
    disk.power_cut(LoseAll);
    matches!(start(&disk, 0, false), Err(OpenError::MarkedFailed))
}

/// The failure marker is durable once written: a power cut right after
/// the failure does not take it back. The defects that leave out the
/// directory's sync, and the marker itself, are noticed. (Stands in
/// for `log::tests::the_failure_marker_survives_a_power_cut`, and for
/// the planted defect `FAIL_UNMARKED` of `store::tests`.)
#[test]
fn the_failure_marker_survives_a_power_cut() {
    assert!(marked_after_a_cut(0));
    for defect in [Mutants::MARKER_UNSYNCED, Mutants::FAIL_UNMARKED] {
        assert!(
            !marked_after_a_cut(defect),
            "the defect {defect:#x} is not noticed"
        );
    }
}

/// A failure whose marker the disk refuses loses nothing that a flush
/// covered: after a power cut no marker is there, so the next start
/// serves the volume (the daemon's failure note stands in for the
/// marker for the rest of the boot), and it holds what was flushed.
/// (Stands in for `log::tests::a_failure_that_cannot_be_recorded_is_reported_as_such`;
/// the part where the failing call says that the marker was not
/// written was the log's: the in-place store does not keep that.)
#[test]
fn a_failure_whose_marker_is_refused_keeps_what_was_flushed() {
    let disk = created(25, 0);
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, 4, 0x44).unwrap();
    flush(&mut store, 1).unwrap();
    write(&mut store, 5, 0x55).unwrap();
    disk.fail_sync(0, true);
    assert_eq!(flush(&mut store, 2), Err(BlkError::Failed));
    drop(store);
    disk.clear_faults();
    disk.power_cut(LoseAll);
    let mut store = start(&disk, 0, false).expect("no marker reached the disk");
    assert_eq!(read(&mut store, 4), Ok(vec![0x44; BLOCK_USIZE]));
}

/// The rigs' hook fails a serving volume on demand: what completed
/// before stays completed, every later request fails, a second call is
/// the first, and the next start finds the volume marked. (Stands in
/// for `store::tests::a_served_store_failed_on_demand_fails_every_later_request_and_is_found_marked`
/// and the store's half of `failure_note.rs`.)
#[test]
fn a_store_failed_on_demand_fails_every_later_request() {
    let disk = created(26, 0);
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, 4, 0x44).unwrap();
    store.fail_now();
    assert!(store.is_failed());
    assert_eq!(write(&mut store, 5, 0x55), Err(BlkError::Failed));
    assert_eq!(read(&mut store, 4), Err(BlkError::Failed));
    assert_eq!(flush(&mut store, 1), Err(BlkError::Failed));
    store.fail_now();
    assert!(store.is_failed());
    drop(store);
    disk.kill();
    assert!(matches!(
        start(&disk, 0, false),
        Err(OpenError::MarkedFailed)
    ));
}

/// A read in flight on the threads when a flush's sync fails returns
/// the bytes written or fails, never other bytes. (Stands in for
/// `store::tests::a_read_of_an_unflushed_record_is_not_answered_from_a_failed_log`.)
#[test]
fn a_read_in_flight_across_a_failed_sync_returns_the_write_or_fails() {
    let disk = created(27, 0);
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, 4, 0x44).unwrap();
    disk.hold_direct_io(1);
    let _release = Releases(&disk);
    submit(
        &mut store,
        7,
        Op::Read {
            offset: 4 * BLOCK,
            buf: Buf::filled(1, 0xEE),
        },
    );
    until_held(&disk);
    disk.fail_sync(0, false);
    submit(&mut store, 8, Op::Flush);
    let mut done = Vec::new();
    let mut store = release_and_drain(&disk, store, &mut done);
    let read = done.iter().find(|c| c.id.0 == 7).expect("the read ended");
    match &read.result {
        Ok(()) => assert_eq!(
            read.buf.as_ref().map(|b| b.to_vec()),
            Some(vec![0x44; BLOCK_USIZE])
        ),
        Err(e) => assert_eq!(*e, BlkError::Failed),
    }
    let flushed = done.iter().find(|c| c.id.0 == 8).expect("the flush ended");
    assert_eq!(flushed.result, Err(BlkError::Failed));
    assert!(store.is_failed());
    assert_eq!(read_again(&mut store), Err(BlkError::Failed));
}

fn read_again(store: &mut InPlace<SimDisk>) -> Result<Vec<u8>, BlkError> {
    read(store, 4)
}

/// A write-zeroes that fails in the home image fails the volume, and
/// nothing a flush covered is lost: by direct I/O, where zeros are
/// written, and through the cache, where the range is deallocated.
/// (Stands in for `store::tests::a_deallocation_that_fails_fails_the_volume_and_loses_nothing`.)
#[test]
fn a_write_zeroes_that_fails_fails_the_volume_and_loses_nothing() {
    for io in [IoMode::default(), BUFFERED] {
        let disk = created(28, 0);
        let mut store = start_io(&disk, 0, false, io).unwrap();
        write(&mut store, 4, 0x44).unwrap();
        flush(&mut store, 1).unwrap();
        if io.direct() {
            disk.fail_write(0, false);
        } else {
            disk.fail_deallocation();
        }
        let zeroed = run(
            &mut store,
            2,
            Op::WriteZeroes {
                offset: 6 * BLOCK,
                len: BLOCK,
            },
        );
        assert!(zeroed.result.is_err(), "{io:?}");
        assert!(store.is_failed(), "{io:?}");
        drop(store);
        disk.kill();
        disk.clear_faults();
        assert!(matches!(
            start_io(&disk, 0, false, io),
            Err(OpenError::MarkedFailed)
        ));
        let mut store = start_io(&disk, 0, true, io).unwrap();
        assert_eq!(read(&mut store, 4), Ok(vec![0x44; BLOCK_USIZE]), "{io:?}");
    }
}

// The lock, the identity, create and open.

/// A second instance is refused while the first lives, and the refusals
/// change nothing: neither the lock nor a create over the volume writes
/// a byte, and the first instance serves on. Once it is gone the next
/// start finds what it wrote. (Stands in for
/// `store::tests::a_second_instance_is_refused_and_changes_nothing`.)
#[test]
fn a_second_instance_is_refused_and_changes_nothing() {
    let disk = created(29, 0);
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, 4, 0x44).unwrap();
    flush(&mut store, 1).unwrap();
    let before = disk.image();
    assert!(matches!(lock(&disk), Err(OpenError::Locked)));
    assert!(matches!(
        create(&disk, &config(1, 4096)),
        Err(OpenError::Locked)
    ));
    assert_eq!(disk.image(), before);
    write(&mut store, 5, 0x55).unwrap();
    flush(&mut store, 2).unwrap();
    drop(store);
    disk.kill();
    let mut store = start(&disk, 0, false).unwrap();
    assert_eq!(read(&mut store, 4), Ok(vec![0x44; BLOCK_USIZE]));
    assert_eq!(read(&mut store, 5), Ok(vec![0x55; BLOCK_USIZE]));
}

/// The lock is a step of its own: it writes nothing and excludes the
/// next; an open it is given that is refused hands it back still held,
/// so that nothing else takes the volume in between; once it is
/// dropped the volume is free. (Stands in for
/// `store::tests::the_lock_is_taken_apart_from_the_open_and_comes_back_held_when_the_open_fails`.)
#[test]
fn a_refused_open_hands_its_lock_back_still_held() {
    let disk = created(30, 0);
    mark(&disk);
    let before = disk.image();
    let held = lock(&disk).unwrap();
    assert_eq!(disk.image(), before, "the lock alone wrote");
    assert!(matches!(lock(&disk), Err(OpenError::Locked)));
    let (refused, held) = Opened::open(disk.clone(), held, OpenOptions::default()).unwrap_err();
    assert!(matches!(refused, OpenError::MarkedFailed), "{refused:?}");
    assert!(matches!(lock(&disk), Err(OpenError::Locked)));
    drop(held);
    assert!(lock(&disk).is_ok());
}

/// The identity is the volume's: the same at every start, read without
/// opening it, and another for a volume made again in the same way.
/// (Stands in for `store::tests::the_identity_belongs_to_the_store_and_is_not_its_key`;
/// the in-place store has no key.)
#[test]
fn the_identity_is_the_volumes() {
    let disk = created(31, 0);
    let store = start(&disk, 0, false).unwrap();
    let identity = store.identity();
    assert_eq!(identity_of(&disk), Some(identity));
    drop(store);
    disk.kill();
    let store = start(&disk, 0, false).unwrap();
    assert_eq!(store.identity(), identity);
    let again = created(31, 0);
    assert_ne!(identity_of(&again), Some(identity));
}

/// A create with parameters that describe no volume is refused, and
/// leaves the directory as it was: not even the lock file is made.
/// (Stands in for the part of `log::tests::create_and_open_check_what_is_there`
/// that is not the log's.)
#[test]
fn a_create_with_parameters_of_no_volume_makes_nothing() {
    for config in [config(0, 4096), config(1, 512), config(1, 1 << 17)] {
        let disk = bare(32, 4096);
        assert!(
            matches!(create(&disk, &config), Err(OpenError::InvalidConfig)),
            "{config:?}"
        );
        assert!(disk.list().unwrap().is_empty(), "{config:?}");
    }
}

/// A volume is made and opened only where the system's page and the
/// file system's blocks are no larger than its write unit, or the
/// block size can be read. A create refused for that makes nothing, an
/// open refused for it writes nothing, and the volume opens again once
/// they fit. The defects that leave out either check are noticed: the
/// create and the open then go through. (Stands in for
/// `store::tests::a_store_is_created_only_where_blocks_are_no_larger_than_the_write_unit`,
/// `an_open_is_refused_where_blocks_are_larger_than_the_write_unit` and
/// `a_store_is_refused_where_pages_are_larger_than_the_write_unit`.)
#[test]
fn create_and_open_hold_the_system_to_the_write_unit() {
    let unit: u32 = 4096;
    let u = u64::from(unit);
    // How the system differs, the defect that leaves its check out, and
    // whether a refusal says it. Twice the unit is the smallest block
    // in which a write rewrites a neighbour.
    type Shape = Box<dyn Fn(&SimDisk)>;
    type Says = Box<dyn Fn(&OpenError) -> bool>;
    let mut shapes: Vec<(String, Shape, u32, Says)> = Vec::new();
    for block in [2 * u, 4 * u, 1 << 20] {
        shapes.push((
            format!("blocks of {block}"),
            Box::new(move |disk: &SimDisk| disk.set_block_size(Some(block))),
            Mutants::ANY_BLOCKS,
            Box::new(move |e: &OpenError| {
                matches!(e, OpenError::Blocks { block: b, unit: n, .. } if (*b, *n) == (block, u))
            }),
        ));
    }
    shapes.push((
        "no block size".into(),
        Box::new(|disk: &SimDisk| disk.set_block_size(None)),
        Mutants::ANY_BLOCKS,
        Box::new(|e: &OpenError| matches!(e, OpenError::BlocksUnknown { .. })),
    ));
    for page in [2 * u, 4 * u] {
        shapes.push((
            format!("pages of {page}"),
            Box::new(move |disk: &SimDisk| disk.set_page_size(page)),
            Mutants::ANY_PAGES,
            Box::new(move |e: &OpenError| {
                matches!(e, OpenError::Pages { page: p, unit: n } if (*p, *n) == (page, u))
            }),
        ));
    }
    // The create.
    for (what, shape, check, says) in &shapes {
        let disk = bare(33, u);
        shape(&disk);
        let refused = create(&disk, &config(1, unit)).map(|_| ());
        assert!(matches!(&refused, Err(e) if says(e)), "{what}: {refused:?}");
        if let Err(OpenError::Blocks { what: under, .. }) = &refused {
            assert_eq!(*under, Under::Directory, "{what}");
        }
        assert!(disk.list().unwrap().is_empty(), "{what}: made something");
        assert_eq!(disk.stats().writes, 0, "{what}");
        let disk = bare(33, u);
        shape(&disk);
        assert!(
            start::create_with(&disk, &config(1, unit), Mutants(*check)).is_ok(),
            "{what}: the defect is not noticed at the create"
        );
    }
    // The open of a volume made where everything fitted.
    for (what, shape, check, says) in &shapes {
        let disk = created(34, 0);
        shape(&disk);
        let (names, stats) = (disk.list().unwrap(), disk.stats());
        let refused = opened(&disk, false, IoMode::default()).map(|_| ());
        assert!(matches!(&refused, Err(e) if says(e)), "{what}: {refused:?}");
        assert_eq!(disk.list().unwrap(), names, "{what}");
        assert_eq!(
            (disk.stats().writes, disk.stats().syncs),
            (stats.writes, stats.syncs),
            "{what}"
        );
        assert!(
            started(&disk, OpenOptions::default(), *check).is_ok(),
            "{what}: the defect is not noticed at the open"
        );
        disk.kill();
        disk.set_block_size(Some(u));
        disk.set_page_size(u);
        assert!(start(&disk, 0, false).is_ok(), "{what}: not again");
    }
}

/// Whether a flushed block of a volume on a disk that rewrites whole
/// blocks of `block` bytes, written through the cache, is still there
/// after a power cut that makes garbage of what it was rewriting, when
/// an unflushed write went to a neighbour in the same block. The volume
/// is made and opened with `mutants`.
fn a_flushed_neighbour_survives(seed: u64, block: u64, mutants: u32) -> bool {
    let unit = 4096;
    let disk = SimDisk::new(seed);
    disk.set_write_unit(unit);
    disk.set_page_size(unit);
    disk.set_block_size(Some(block));
    start::create_with(&disk, &config(1, 4096), Mutants(mutants)).unwrap();
    let options = OpenOptions {
        io: BUFFERED,
        ..OpenOptions::default()
    };
    let mut store = started(&disk, options.clone(), mutants).unwrap();
    write(&mut store, 0, 0x10).unwrap();
    flush(&mut store, 1).unwrap();
    write(&mut store, 1, 0x11).unwrap();
    drop(store);
    disk.power_cut(SeededCut::with_garbage(seed));
    let mut store = started(&disk, options, mutants).unwrap();
    read(&mut store, 0) == Ok(vec![0x10; BLOCK_USIZE])
}

/// Where the file system's block is no larger than the write unit, a
/// write puts no flushed neighbour at risk, whatever a power cut makes
/// of what was being written. Where the block is four units, a write
/// rewrites its neighbours and a cut can lose a block nobody wrote: the
/// checks of create and open are what keep a volume from such a disk,
/// and without them the loss is seen. (Stands in for
/// `store::tests::a_volume_whose_unit_is_at_least_the_block_survives_cuts_anywhere`
/// and `a_volume_whose_unit_is_below_the_block_loses_what_nobody_wrote`.)
#[test]
fn a_block_larger_than_the_write_unit_can_lose_what_nobody_wrote() {
    for seed in 0..32 {
        assert!(a_flushed_neighbour_survives(seed, 512, 0), "seed {seed}");
        assert!(a_flushed_neighbour_survives(seed, 4096, 0), "seed {seed}");
    }
    let lost = (0..32)
        .filter(|&seed| !a_flushed_neighbour_survives(seed, 4 * 4096, Mutants::ANY_BLOCKS))
        .count();
    assert!(lost > 0, "a block of four units lost nothing");
}

/// Offsets beyond 32 bits land where they belong: a volume of 5 GiB
/// takes writes at either side of 4 GiB and at its last block, the same
/// offsets modulo 4 GiB are not touched, a write-zeroes across 4 GiB
/// zeroes both sides, all of it through a restart; a request beyond the
/// end is refused. (Stands in for `store::tests::a_volume_above_four_gib`.)
#[test]
fn a_volume_above_four_gib() {
    let disk = bare(35, 4096);
    create(&disk, &config(5 * 1024, 4096)).unwrap();
    let four = (4u64 << 30) / BLOCK;
    let last = (5u64 << 30) / BLOCK - 1;
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, four + 3, 0x43).unwrap();
    write(&mut store, last, 0x4F).unwrap();
    write(&mut store, four - 1, 0x41).unwrap();
    write(&mut store, four, 0x42).unwrap();
    let zeroed = run(
        &mut store,
        9,
        Op::WriteZeroes {
            offset: (four - 1) * BLOCK,
            len: 2 * BLOCK,
        },
    );
    assert_eq!(zeroed.result, Ok(()));
    flush(&mut store, 1).unwrap();
    drop(store);
    disk.kill();
    let mut store = start(&disk, 0, false).unwrap();
    let zeros = vec![0; BLOCK_USIZE];
    assert_eq!(read(&mut store, four + 3), Ok(vec![0x43; BLOCK_USIZE]));
    assert_eq!(read(&mut store, last), Ok(vec![0x4F; BLOCK_USIZE]));
    assert_eq!(read(&mut store, four - 1), Ok(zeros.clone()));
    assert_eq!(read(&mut store, four), Ok(zeros.clone()));
    assert_eq!(
        read(&mut store, 3),
        Ok(zeros.clone()),
        "an alias was written"
    );
    assert_eq!(read(&mut store, last - four), Ok(zeros));
    let beyond = run(
        &mut store,
        2,
        Op::Read {
            offset: (last + 1) * BLOCK,
            buf: Buf::filled(1, 0),
        },
    );
    assert!(
        matches!(beyond.result, Err(BlkError::OutOfBounds { .. })),
        "{:?}",
        beyond.result
    );
}

/// A create cut short, by a failed sync or by the power at any of its
/// calls, with the cut making garbage of what was not synced, leaves no
/// volume, and a create again makes one, or a whole volume, which opens
/// with its base and takes a write. (Stands in for
/// `store::tests::an_interrupted_creation_leaves_no_store_or_a_whole_one`.)
#[test]
fn a_create_cut_short_leaves_no_volume_or_a_whole_one() {
    let config = VolumeConfig {
        base: Base::Pattern(7),
        ..config(1, 4096)
    };
    let mut base = vec![0u8; BLOCK_USIZE];
    config.base.fill_block(9, &mut base);
    let mut found = [0u32; 2];
    for calls in 0..8 {
        for seed in 0..16 {
            for by_sync in [false, true] {
                let disk = bare(seed, 4096);
                if by_sync {
                    disk.fail_sync(calls, false);
                } else {
                    disk.halt_after(calls);
                }
                let created = create(&disk, &config);
                disk.clear_faults();
                disk.power_cut(SeededCut::with_garbage(seed));
                let at = format!("after {calls} calls, seed {seed}, by sync {by_sync}");
                match Volume::read(&disk) {
                    Ok(_) => found[0] += 1,
                    Err(VolumeError::Missing) => {
                        assert!(created.is_err(), "{at}: lost");
                        create(&disk, &config).unwrap_or_else(|e| panic!("{at}: again: {e}"));
                        found[1] += 1;
                    }
                    Err(other) => panic!("{at}: {other}"),
                }
                let mut store = start(&disk, 0, false).unwrap_or_else(|e| panic!("{at}: {e}"));
                assert_eq!(read(&mut store, 9), Ok(base.clone()), "{at}");
                write(&mut store, 9, 0x99).unwrap();
                assert_eq!(read(&mut store, 9), Ok(vec![0x99; BLOCK_USIZE]), "{at}");
            }
        }
    }
    assert!(found[0] > 0 && found[1] > 0, "{found:?}");
}

/// The directory is that of a volume only if it holds a volume file;
/// one with the log store's superblock is refused by the create too,
/// and the create writes nothing there.
#[test]
fn a_create_leaves_a_log_stores_directory_as_it_was() {
    let disk = bare(36, 4096);
    drop(disk.create(LEGACY_NAME).unwrap());
    let before = disk.image();
    assert!(matches!(
        create(&disk, &config(1, 4096)),
        Err(OpenError::Volume(VolumeError::Legacy))
    ));
    assert_eq!(disk.image(), before);
    assert!(!disk.list().unwrap().iter().any(|n| n == VOLUME_NAME));
}

// Serving.

/// A request held on one of the threads holds no request for another
/// block: a read of another block is answered while a write is held,
/// and while a read is held. (Stands in for
/// `store::tests::a_served_read_is_answered_behind_a_held_write` and
/// `a_read_held_in_one_reader_leaves_the_others_reading`.)
#[test]
fn a_held_request_holds_no_request_for_another_block() {
    for first in [
        Op::Write {
            offset: 0,
            data: Buf::filled(1, 0x10),
        },
        Op::Read {
            offset: 0,
            buf: Buf::filled(1, 0),
        },
    ] {
        let disk = created(37, 0);
        let mut store = start(&disk, 0, false).unwrap();
        write(&mut store, 5, 0x55).unwrap();
        disk.hold_direct_io(1);
        let _release = Releases(&disk);
        submit(&mut store, 1, first);
        until_held(&disk);
        submit(
            &mut store,
            2,
            Op::Read {
                offset: 5 * BLOCK,
                buf: Buf::filled(1, 0),
            },
        );
        let mut done = Vec::new();
        assert!(
            until_done(&mut store, &mut done, 2),
            "the read waited for the held request"
        );
        assert!(!done.iter().any(|c| c.id.0 == 1), "the held request ended");
        let read = done.iter().find(|c| c.id.0 == 2).unwrap();
        assert_eq!(read.result, Ok(()));
        assert_eq!(
            read.buf.as_ref().map(|b| b.to_vec()),
            Some(vec![0x55; BLOCK_USIZE])
        );
        drop(release_and_drain(&disk, store, &mut done));
    }
}

/// A read does not wait for a flush's sync: with the sync held, a read
/// of what was written before the flush is answered with it. (Stands in
/// for `store::tests::a_read_of_an_unflushed_record_does_not_wait_for_a_sync`.)
#[test]
fn a_read_does_not_wait_for_a_sync() {
    for io in [IoMode::default(), OTHER_IO[1]] {
        let disk = created(38, 0);
        let mut store = start_io(&disk, 0, false, io).unwrap();
        write(&mut store, 4, 0x44).unwrap();
        disk.hold_syncs_of(crate::store::HOME_NAME);
        submit(&mut store, 1, Op::Flush);
        assert!(store.syncing(), "{io:?}");
        let mut done = Vec::new();
        submit(
            &mut store,
            2,
            Op::Read {
                offset: 4 * BLOCK,
                buf: Buf::filled(1, 0),
            },
        );
        let began = std::time::Instant::now();
        while !done.iter().any(|c: &Completion| c.id.0 == 2) {
            assert!(
                began.elapsed() < WAIT,
                "{io:?}: the read waited for the sync"
            );
            store.wait(&mut done, Duration::from_millis(10));
        }
        assert!(!done.iter().any(|c| c.id.0 == 1), "{io:?}: the flush ended");
        let read = done.iter().find(|c| c.id.0 == 2).unwrap();
        assert_eq!(
            read.buf.as_ref().map(|b| b.to_vec()),
            Some(vec![0x44; BLOCK_USIZE]),
            "{io:?}"
        );
        disk.release_syncs();
        let store = drained(store, &mut done);
        assert!(done.iter().all(|c| c.result.is_ok()), "{io:?}");
        drop(store);
    }
}

/// A sync the file cannot begin on its helper is made at once on the
/// store's thread: the flush succeeds, the volume does not fail, and
/// what it covered is durable. (Stands in for
/// `store::tests::a_sync_that_cannot_be_begun_is_made_by_the_worker`.)
#[test]
fn a_sync_that_cannot_be_begun_is_made_at_once() {
    let disk = created(39, 0);
    let mut store = start(&disk, 0, false).unwrap();
    disk.refuse_sync_begins(u64::MAX);
    let syncs = store.counts().syncs;
    write(&mut store, 4, 0x44).unwrap();
    assert_eq!(flush(&mut store, 1), Ok(()));
    assert!(!store.is_failed());
    assert_eq!(store.counts().syncs, syncs + 1);
    drop(store);
    disk.power_cut(LoseAll);
    let mut store = start(&disk, 0, false).unwrap();
    assert_eq!(read(&mut store, 4), Ok(vec![0x44; BLOCK_USIZE]));
}

/// Wait, for at most [`WAIT`], until the waker has been called more
/// than `than` times; says whether it was.
fn woken(wakes: &AtomicU64, than: u64) -> bool {
    let began = std::time::Instant::now();
    while wakes.load(Ordering::SeqCst) <= than {
        if began.elapsed() > WAIT {
            return false;
        }
        std::thread::yield_now();
    }
    true
}

/// The owner's waker is called for every completion the threads make
/// ready, and when a thread dies: the owner then takes what is ready
/// without waiting on the store. The pump only polls and relies on the
/// wake: a completion that woke nobody waits for the kernel's next
/// request, which at a depth of one never comes. So each of several
/// writes, one after another, must wake the owner, on one thread (which
/// then does them all) and on the default eight. The simulated disk's sync calls no
/// waker (it is asked), so the waker at the end of a sync on the file's
/// helper is held on real files (`tests/inplace_real.rs`). (Stands in
/// for the waker's part of
/// `store::tests::the_served_store_acknowledges_what_its_owner_has_taken`,
/// `a_worker_that_has_ended_tells_its_owner` and
/// `a_reader_that_has_ended_tells_its_owner`; their acknowledged
/// position and `gone` were the log store's.)
#[test]
fn the_waker_is_called_for_what_the_threads_make_ready() {
    let disk = created(40, 0);
    let wakes = Arc::new(AtomicU64::new(0));
    let waker = {
        let wakes = wakes.clone();
        Waker::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
        })
    };
    let one = IoMode::with_threads(1).unwrap();
    for io in [one, IoMode::default()] {
        let disk = created(40, 0);
        let mut store = opened(&disk, false, io)
            .unwrap()
            .start(Some(waker.clone()))
            .unwrap();
        for n in 0..4u64 {
            let before = wakes.load(Ordering::SeqCst);
            write(&mut store, 4 + n, 0x44).unwrap();
            assert!(
                woken(&wakes, before),
                "{io:?}: write {n} on the threads woke nobody"
            );
        }
    }
    let mut store = opened(&disk, false, IoMode::default())
        .unwrap()
        .start(Some(waker))
        .unwrap();
    let before = wakes.load(Ordering::SeqCst);
    disk.panic_in_direct_io(0);
    submit(
        &mut store,
        2,
        Op::Read {
            offset: 4 * BLOCK,
            buf: Buf::filled(1, 0),
        },
    );
    assert!(woken(&wakes, before), "a thread's death woke nobody");
    let mut done = Vec::new();
    let store = drained(store, &mut done);
    assert!(store.is_failed());
}
