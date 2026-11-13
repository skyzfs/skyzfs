use std::time::Duration;

use skyzfs_blk::conformance::{run_all, Config};
use skyzfs_blk::{
    Base, BlkError, BlockTarget, Buf, Completion, Op, Request, RequestId, BLOCK_USIZE,
};

use super::harness::{InPlaceHarness, OnSim};
use super::*;
use crate::format::Key;
use crate::io::Dir;
use crate::log::{LogError, MARKER_NAME};
use crate::sim::{SeededCut, SimDisk};
use crate::store::{OpenOptions, Store, StoreConfig, StoreError, HOME_NAME};

const BLOCK: u64 = 4096;
/// What a test gives a wait: it waits for a sync to end, not for the
/// time (see `InPlace::wait`).
const WAIT: Duration = Duration::from_secs(60);

/// Wait until a caller waits at a held sync of the disk: the event the
/// marker's tests wait for. The bound only ends a test that would
/// otherwise hang, as where the marker never reached its sync.
fn until_held(disk: &SimDisk) {
    let began = std::time::Instant::now();
    while disk.held_waiters() == 0 {
        assert!(began.elapsed() < WAIT, "no caller reached the held sync");
        std::thread::yield_now();
    }
}

fn harnesses(mutants: u32) -> Vec<InPlaceHarness> {
    harnesses_io(mutants, IoMode::default())
}

fn harnesses_io(mutants: u32, io: IoMode) -> Vec<InPlaceHarness> {
    [(1, 0), (16, 0), (16, 3)]
        .into_iter()
        .map(|(queue_depth, sync_delay)| InPlaceHarness {
            queue_depth,
            sync_delay,
            mutants,
            io,
            page: 4096,
        })
        .collect()
}

/// The I/O other than the default (direct I/O on eight threads, which
/// every test runs unless it says otherwise): direct I/O on one thread,
/// the fewest the daemon takes; direct I/O on the caller's thread; and
/// through the cache on the caller's thread, the store before decision
/// D6, which only tests still use.
const OTHER_IO: [IoMode; 3] = [
    IoMode {
        direct: true,
        threads: 1,
    },
    IoMode {
        direct: true,
        threads: 0,
    },
    IoMode {
        direct: false,
        threads: 0,
    },
];

/// Through the kernel's cache, on the caller's thread: the store before
/// decision D6, for the tests that need a writer through the cache.
const BUFFERED: IoMode = OTHER_IO[2];

/// The suite passes on the in-place store with each of the other I/O
/// modes too, with every capability.
#[test]
fn the_in_place_store_passes_the_conformance_suite_with_each_io() {
    let config = Config {
        cases: 32,
        ..Config::default()
    };
    for io in OTHER_IO {
        for mut harness in harnesses_io(0, io) {
            let report = run_all(&mut harness, &config);
            assert!(report.passed(), "in {harness:?}:\n{report}");
            for (case, why) in report.unexercised() {
                assert!(
                    harness.queue_depth == 1 && why.contains("one request at a time"),
                    "{case}: {why}"
                );
            }
            assert!(report.weaker().is_empty(), "{harness:?}:\n{report}");
        }
    }
}

/// The suite passes on the in-place store on the simulated disk, with
/// every capability: the disk's own power cuts (without garbage), and a
/// log that fails by the disk's own faults, which may tear what they
/// leave. With one request at a time and with sixteen, and with syncs
/// that end up to three looks after they begin, so that writes and
/// flushes arrive while a sync is under way. Only what needs more than
/// one request in flight is left out, at a depth of one.
#[test]
fn the_in_place_store_passes_the_conformance_suite() {
    let config = Config {
        cases: 32,
        ..Config::default()
    };
    for mut harness in harnesses(0) {
        let report = run_all(&mut harness, &config);
        assert!(report.passed(), "in {harness:?}:\n{report}");
        for (case, why) in report.unexercised() {
            assert!(
                harness.queue_depth == 1 && why.contains("one request at a time"),
                "{case}: {why}"
            );
        }
        assert!(report.weaker().is_empty(), "{harness:?}:\n{report}");
    }
}

/// The suite notices the defects of durability and of failure, each in
/// the cases named for it: a flush that makes nothing durable, a failed
/// sync tried again, a failed write that does not fail the volume, a
/// start that does not drop the kernel's cache where something wrote
/// through it. (A
/// flush that joins a sync begun
/// before a write it must cover needs a cut to land between the two at a
/// sector the write changed; the suite's programs do not reliably make
/// one, and `the_power_cut_test_notices_the_defects_of_durability`
/// catches it.)
#[test]
fn the_suite_notices_the_defects() {
    let config = Config {
        cases: 32,
        ..Config::default()
    };
    // The cases that must notice each defect: for a flush that syncs
    // nothing, the cases of the power cut, which the harness's cut
    // alone can fail (the failure cases notice it too).
    let cases: [(u32, &[&str]); 4] = [
        (
            Mutants::FLUSH_WITHOUT_SYNC,
            &[
                "flushed_operations_survive_power_cut",
                "flush_covers_what_was_acknowledged_before_it",
                "power_cut_at_every_point_leaves_what_a_volatile_cache_disk_may",
            ],
        ),
        (
            Mutants::SYNC_RETRIED,
            &[
                "failed_volume_fails_every_request",
                "reopened_failed_volume_holds_what_was_flushed",
            ],
        ),
        (Mutants::WRITE_ERROR_PASSES, &["prop_failures_match_model"]),
        // A start that leaves the kernel's cache as it was: the reopen
        // of a failed volume serves what a failed sync left only there.
        // Only a writer through the cache leaves pages there: the store
        // writes through it in this case alone ([`BUFFERED`]).
        (
            Mutants::CACHE_NOT_DROPPED | Mutants::CACHE_NOT_CHECKED,
            &["reopened_failed_volume_holds_what_was_flushed"],
        ),
    ];
    for (mutant, must_fail) in cases {
        let mut harness = if mutant & Mutants::CACHE_NOT_DROPPED != 0 {
            harnesses_io(mutant, BUFFERED)[2]
        } else {
            harnesses(mutant)[2]
        };
        let report = run_all(&mut harness, &config);
        let failed = report.failed();
        for case in must_fail {
            assert!(
                failed.contains(case),
                "the defect {mutant:#x} is not noticed by {case}: {failed:?}"
            );
        }
    }
}

/// A store on a simulated disk, with a log of a few records that the
/// start applies.
fn created(seed: u64, sync_delay: u64) -> SimDisk {
    created_at(seed, sync_delay, 4096)
}

/// The same on a system whose page is `page` bytes: the disk's write
/// unit, its file system's block and the volume's write unit.
fn created_at(seed: u64, sync_delay: u64, page: u64) -> SimDisk {
    let disk = SimDisk::new(seed);
    disk.set_write_unit(page);
    disk.set_page_size(page);
    disk.set_block_size(Some(page));
    disk.set_sync_delay(sync_delay);
    let config = StoreConfig {
        volume: *b"in-place volume.",
        size_mib: 1,
        base: Base::Empty,
        align: 64,
        // Room for a request of a megabyte beside the two units a
        // segment keeps.
        segment_size: (1 << 20) + (128 << 10).max(3 * page),
        write_unit: u32::try_from(page).unwrap(),
    };
    let mut store = Store::create_keyed(
        disk.clone(),
        &config,
        Key(*b"a key for tests!"),
        OpenOptions::default(),
    )
    .unwrap();
    // Something in the log: block 3 holds 0x33, not flushed.
    let data = Buf::filled(1, 0x33);
    store
        .submit(Request {
            id: RequestId(0),
            op: Op::Write {
                offset: 3 * BLOCK,
                data,
            },
        })
        .unwrap();
    let mut done = Vec::new();
    store.wait(&mut done, Duration::ZERO);
    assert_eq!(done[0].result, Ok(()));
    drop(store);
    disk.kill();
    disk
}

/// Start on `disk`: the log store's open, then the in-place store.
fn start(disk: &SimDisk, mutants: u32, acknowledge: bool) -> Result<InPlace<SimDisk>, StoreError> {
    start_io(disk, mutants, acknowledge, IoMode::default())
}

fn start_io(
    disk: &SimDisk,
    mutants: u32,
    acknowledge: bool,
    io: IoMode,
) -> Result<InPlace<SimDisk>, StoreError> {
    let options = OpenOptions {
        acknowledge_failure: acknowledge,
        in_place_io: io,
        ..OpenOptions::default()
    };
    let (store, _) = Store::open(disk.clone(), options)?;
    Ok(store
        .into_in_place_with(None, Mutants(mutants))
        .expect("the in-place store"))
}

/// Submit and run until it completes.
fn run(store: &mut InPlace<SimDisk>, id: u64, op: Op) -> Completion {
    store
        .submit(Request {
            id: RequestId(id),
            op,
        })
        .unwrap();
    let mut done = Vec::new();
    while done.is_empty() {
        store.wait(&mut done, WAIT);
    }
    done.pop().unwrap()
}

fn write(store: &mut InPlace<SimDisk>, block: u64, byte: u8) -> Result<(), BlkError> {
    run(
        store,
        block,
        Op::Write {
            offset: block * BLOCK,
            data: Buf::filled(1, byte),
        },
    )
    .result
}

fn read(store: &mut InPlace<SimDisk>, block: u64) -> Result<Vec<u8>, BlkError> {
    let done = run(
        store,
        1000 + block,
        Op::Read {
            offset: block * BLOCK,
            buf: Buf::filled(1, 0xEE),
        },
    );
    done.result?;
    Ok(done.buf.unwrap().to_vec())
}

/// The start applies the log and syncs the home image (acceptance 3):
/// what the log held is read, and what the in-place store wrote
/// before the process was lost, which no flush covered, is durable
/// once the next start is done. A power cut right after that start,
/// which keeps nothing that was not synced, takes nothing back.
#[test]
fn the_start_applies_the_log_and_makes_the_home_image_durable() {
    let disk = created(1, 0);
    let mut store = start(&disk, 0, false).unwrap();
    assert_eq!(read(&mut store, 3).unwrap(), vec![0x33; BLOCK_USIZE]);
    write(&mut store, 5, 0x55).unwrap();
    drop(store);
    disk.kill();
    let store = start(&disk, 0, false).unwrap();
    assert_eq!(store.counts().syncs, 1, "the start syncs");
    drop(store);
    disk.power_cut(crate::sim::LoseAll);
    let mut store = start(&disk, 0, false).unwrap();
    assert_eq!(read(&mut store, 3).unwrap(), vec![0x33; BLOCK_USIZE]);
    assert_eq!(read(&mut store, 5).unwrap(), vec![0x55; BLOCK_USIZE]);
}

/// A flush with nothing written since the last sync completes without
/// one; a flush after a write begins one.
#[test]
fn a_flush_with_nothing_to_make_durable_does_not_sync() {
    let disk = created(2, 0);
    let mut store = start(&disk, 0, false).unwrap();
    let before = store.counts();
    assert_eq!(run(&mut store, 1, Op::Flush).result, Ok(()));
    assert_eq!(store.counts().syncs, before.syncs);
    assert_eq!(store.counts().clean, before.clean + 1);
    write(&mut store, 5, 0x55).unwrap();
    assert_eq!(run(&mut store, 2, Op::Flush).result, Ok(()));
    assert_eq!(store.counts().syncs, before.syncs + 1);
}

/// While a sync is under way: a flush with nothing written since its
/// begin joins it, and one after a write waits for the next sync,
/// which begins when this one ends and covers it. Each write is done
/// before the flush behind it arrives (on the threads, a flush waits
/// for the writes there: `a_flush_waits_for_the_writes_on_the_threads`).
#[test]
fn a_flush_joins_a_sync_under_way_only_when_nothing_was_written_since_it_began() {
    for io in [IoMode::default(), OTHER_IO[1]] {
        let disk = created(3, 0);
        let mut store = start_io(&disk, 0, false, io).unwrap();
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
        // A write, done: its completion is taken, and no flush waits.
        let written = |store: &mut InPlace<SimDisk>, done: &mut Vec<Completion>, id, byte| {
            submit(
                store,
                id,
                Op::Write {
                    offset: 0,
                    data: Buf::filled(1, byte),
                },
            );
            while !done.iter().any(|c: &Completion| c.id.0 == id) {
                store.poll(done);
            }
        };
        let syncs = store.counts().syncs;
        written(&mut store, &mut done, 1, 1);
        submit(&mut store, 2, Op::Flush);
        assert!(store.syncing(), "{io:?}");
        submit(&mut store, 3, Op::Flush);
        assert_eq!(store.counts().joined, 1, "{io:?}");
        written(&mut store, &mut done, 4, 2);
        submit(&mut store, 5, Op::Flush);
        assert_eq!(
            store.counts().joined,
            1,
            "{io:?}: a flush behind a write joined"
        );
        store.poll(&mut done);
        let ids: Vec<u64> = done.iter().map(|c| c.id.0).collect();
        assert_eq!(ids, [1, 4], "{io:?}: only the writes are done");
        disk.release_syncs();
        while store.in_flight() > 0 {
            store.wait(&mut done, Duration::ZERO);
        }
        assert!(done.iter().all(|c| c.result.is_ok()), "{io:?}");
        assert_eq!(store.counts().syncs, syncs + 2, "{io:?}");
    }
}

/// A flush waits for every write on the threads: it covers what was
/// acknowledged before it, and this store also makes it wait for what
/// was submitted before it (the stricter reading). With the writes held
/// on the threads, the flush does not complete or begin its sync until
/// they are let go. The mutant that lets a flush pass the writes is
/// noticed: the flush completes while a write is still held.
#[test]
fn a_flush_waits_for_the_writes_on_the_threads() {
    for mutant in [0, Mutants::FLUSH_PASSES_WRITES] {
        let disk = created(3, 0);
        let mut store = start_io(&disk, mutant, false, IoMode::default()).unwrap();
        // Two writes, held on the threads before they touch the image.
        disk.hold_direct_io(2);
        for id in [1, 2] {
            store
                .submit(Request {
                    id: RequestId(id),
                    op: Op::Write {
                        offset: id * BLOCK,
                        data: Buf::filled(1, id as u8),
                    },
                })
                .unwrap();
        }
        until_held(&disk);
        store
            .submit(Request {
                id: RequestId(3),
                op: Op::Flush,
            })
            .unwrap();
        let mut done = Vec::new();
        store.poll(&mut done);
        let flushed = done.iter().any(|c| c.id.0 == 3);
        if mutant == 0 {
            assert!(!flushed, "the flush did not wait for the writes");
            assert!(!store.syncing(), "a sync began before the writes ended");
        } else {
            assert!(flushed, "the defect is not noticed");
        }
        disk.release_direct_io();
        while store.in_flight() > 0 {
            store.wait(&mut done, WAIT);
        }
        assert!(done.iter().all(|c| c.result.is_ok()));
    }
}

/// A request waits for an overlapping one on the threads, so that
/// overlapping requests take effect in the order they came: a write
/// held on the threads, then a write to the same block, then a read of
/// it. With the first write held, neither the second write nor the read
/// runs. The mutant that lets them pass is noticed: the read completes
/// while the first write is still held.
#[test]
fn an_overlapping_request_waits_for_the_one_on_the_threads() {
    for mutant in [0, Mutants::OVERLAP_UNORDERED] {
        let disk = created(3, 0);
        let mut store = start_io(&disk, mutant, false, IoMode::default()).unwrap();
        disk.hold_direct_io(1);
        store
            .submit(Request {
                id: RequestId(1),
                op: Op::Write {
                    offset: 0,
                    data: Buf::filled(1, 0x11),
                },
            })
            .unwrap();
        until_held(&disk);
        store
            .submit(Request {
                id: RequestId(2),
                op: Op::Write {
                    offset: 0,
                    data: Buf::filled(1, 0x22),
                },
            })
            .unwrap();
        store
            .submit(Request {
                id: RequestId(3),
                op: Op::Read {
                    offset: 0,
                    buf: Buf::filled(1, 0),
                },
            })
            .unwrap();
        let mut done = Vec::new();
        store.poll(&mut done);
        let read_done = done.iter().any(|c| c.id.0 == 3);
        if mutant == 0 {
            assert!(!read_done, "a request passed the write it overlaps");
            assert_eq!(store.io_queue().waiting, 2, "the two wait");
        } else {
            assert!(read_done, "the defect is not noticed");
        }
        disk.release_direct_io();
        while store.in_flight() > 0 {
            store.wait(&mut done, WAIT);
        }
        assert!(done.iter().all(|c| c.result.is_ok()));
    }
}

/// An I/O thread that panics dies and does not hang the store. The job
/// it held comes back as a failed request; the volume fails from the
/// death itself, so a panic in a read (which no error of its own fails
/// the volume for) fails it too; and a request that waited for the dead
/// thread fails with the volume, rather than waiting for a thread that
/// is gone. A later start finds the volume marked as failed.
#[test]
fn a_panicking_io_thread_fails_the_volume() {
    // One thread, so the panic takes the only worker and the second
    // request waits for a thread that is gone.
    let io = IoMode::for_tests(true, 1);
    let disk = created(3, 0);
    let mut store = start_io(&disk, 0, false, io).unwrap();
    disk.panic_in_direct_io(0);
    // A read panics (a read's own I/O error does not fail the volume),
    // and a write waits behind the dead thread.
    store
        .submit(Request {
            id: RequestId(1),
            op: Op::Read {
                offset: 0,
                buf: Buf::filled(1, 0),
            },
        })
        .unwrap();
    store
        .submit(Request {
            id: RequestId(2),
            op: Op::Write {
                offset: BLOCK,
                data: Buf::filled(1, 0x22),
            },
        })
        .unwrap();
    let mut done = Vec::new();
    while store.in_flight() > 0 {
        store.wait(&mut done, WAIT);
    }
    assert!(store.is_failed(), "the panic did not fail the volume");
    assert!(
        done.iter().all(|c| c.result == Err(BlkError::Failed)),
        "{done:?}"
    );
    drop(store);
    disk.kill();
    assert!(
        matches!(
            start(&disk, 0, false),
            Err(StoreError::Log(LogError::MarkedFailed))
        ),
        "the next start does not find the volume failed"
    );
}

/// The mutant that takes a thread's death for an I/O error of the one
/// job it held is noticed: a panic in a read then fails only that read
/// and leaves the volume serving, which it must not. One read alone, so
/// nothing waits for the dead thread and the store can be dropped.
#[test]
fn a_thread_death_ignored_is_noticed() {
    let io = IoMode::for_tests(true, 1);
    let disk = created(3, 0);
    let mut store = start_io(&disk, Mutants::DEATH_IGNORED, false, io).unwrap();
    disk.panic_in_direct_io(0);
    store
        .submit(Request {
            id: RequestId(1),
            op: Op::Read {
                offset: 0,
                buf: Buf::filled(1, 0),
            },
        })
        .unwrap();
    let mut done = Vec::new();
    while done.is_empty() {
        store.wait(&mut done, WAIT);
    }
    assert!(!store.is_failed(), "the defect is not noticed");
    assert!(matches!(done[0].result, Err(BlkError::Io(_))), "{done:?}");
}

/// The I/O queue the status file shows: what is on the threads now,
/// what waits, and the highs since the store began. With a write held
/// on the threads and two requests waiting behind an overlap, the
/// counts are one in flight and two waiting; once they are let go the
/// current counts fall to zero and the highs stay.
#[test]
fn the_io_queue_counts_what_is_on_the_threads() {
    let disk = created(3, 0);
    let mut store = start_io(&disk, 0, false, IoMode::default()).unwrap();
    assert_eq!(store.io_queue().threads, IoMode::DEFAULT_THREADS as u64);
    disk.hold_direct_io(1);
    store
        .submit(Request {
            id: RequestId(1),
            op: Op::Write {
                offset: 0,
                data: Buf::filled(1, 0x11),
            },
        })
        .unwrap();
    until_held(&disk);
    for id in [2, 3] {
        store
            .submit(Request {
                id: RequestId(id),
                op: Op::Write {
                    offset: 0,
                    data: Buf::filled(1, id as u8),
                },
            })
            .unwrap();
    }
    let q = store.io_queue();
    assert_eq!((q.in_flight, q.waiting), (1, 2), "{q:?}");
    disk.release_direct_io();
    let mut done = Vec::new();
    while store.in_flight() > 0 {
        store.wait(&mut done, WAIT);
    }
    let q = store.io_queue();
    assert_eq!((q.in_flight, q.waiting), (0, 0), "{q:?}");
    assert!(
        q.in_flight_max >= 1 && q.waiting_max >= 2 && q.waited >= 2,
        "{q:?}"
    );
}

/// A failed sync fails the volume for good: the flush fails, so does
/// every later request, reads included, and a flush is never answered
/// by a second sync that might succeed; the next start refuses the
/// volume until the failure is acknowledged.
#[test]
fn a_failed_sync_fails_the_volume_for_good() {
    let disk = created(4, 0);
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, 1, 0x11).unwrap();
    disk.fail_sync(0, false);
    assert_eq!(run(&mut store, 1, Op::Flush).result, Err(BlkError::Failed));
    assert!(store.is_failed());
    assert_eq!(write(&mut store, 2, 0x22), Err(BlkError::Failed));
    assert_eq!(read(&mut store, 1), Err(BlkError::Failed));
    assert_eq!(run(&mut store, 2, Op::Flush).result, Err(BlkError::Failed));
    drop(store);
    disk.kill();
    assert!(matches!(
        start(&disk, 0, false),
        Err(StoreError::Log(LogError::MarkedFailed))
    ));
    let mut store = start(&disk, 0, true).unwrap();
    assert!(read(&mut store, 1).is_ok());
}

/// A discard counts as a change: a flush behind a discard alone syncs,
/// and so fails on a disk whose syncs fail. Without that, it would
/// succeed after the client changed the volume.
#[test]
fn a_flush_behind_a_discard_syncs() {
    for mutants in [0, Mutants::DISCARD_UNCHANGED] {
        let disk = created(8, 0);
        let mut store = start(&disk, mutants, false).unwrap();
        disk.fail_sync(0, true);
        let discard = Op::Discard {
            offset: 0,
            len: BLOCK,
        };
        assert_eq!(run(&mut store, 1, discard).result, Ok(()));
        let flushed = run(&mut store, 2, Op::Flush).result;
        if mutants == 0 {
            assert_eq!(flushed, Err(BlkError::Failed));
        } else {
            assert_eq!(flushed, Ok(()), "the defect is not noticed");
        }
    }
}

/// A request that meets the failure is answered while the failure
/// marker is still being written: the marker needs the disk that has
/// just failed, and here its sync does not end until released. The
/// store runs on a thread of the test's, so that a store that waited
/// for the marker would hang only that thread; the test waits for the
/// answer, an event (the bound only ends a test that would otherwise
/// hang). After the release the store is dropped, and the next start
/// finds the marker.
#[test]
fn a_request_that_meets_the_failure_is_answered_while_the_marker_waits() {
    let disk = created(10, 0);
    let mut store = start(&disk, 0, false).unwrap();
    disk.hold_whole_syncs_of(MARKER_NAME);
    disk.fail_write(0, false);
    let (tell, answer) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = tell.send(write(&mut store, 1, 0x11));
        store
    });
    assert_eq!(answer.recv_timeout(WAIT), Ok(Err(BlkError::Failed)));
    // The marker's write is under way, held at its sync.
    until_held(&disk);
    disk.release_syncs();
    drop(worker.join().unwrap());
    disk.kill();
    assert!(matches!(
        start(&disk, 0, false),
        Err(StoreError::Log(LogError::MarkedFailed))
    ));
}

/// The test above notices a marker written on the caller's thread
/// (`MARKER_INLINE`), and so also a failure that waits for the marker's
/// thread before it answers: the marker's sync holds a caller while the
/// failed request is not answered. That is read once a caller waits at
/// the held sync, and needs no time: with the defect, the caller is the
/// store's thread, which answers only after the sync.
#[test]
fn the_marker_test_notices_a_marker_written_on_the_callers_thread() {
    let disk = created(10, 0);
    let mut store = start(&disk, Mutants::MARKER_INLINE, false).unwrap();
    disk.hold_whole_syncs_of(MARKER_NAME);
    disk.fail_write(0, false);
    let (tell, answer) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = tell.send(write(&mut store, 1, 0x11));
        store
    });
    until_held(&disk);
    assert_eq!(
        answer.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty),
        "the request was answered while the marker's sync held its caller"
    );
    disk.release_syncs();
    assert_eq!(answer.recv(), Ok(Err(BlkError::Failed)));
    drop(worker.join().unwrap());
}

/// A store that never failed ends its marker's thread without writing
/// a marker when it is dropped: the next start serves the volume.
#[test]
fn a_store_that_did_not_fail_writes_no_marker() {
    let disk = created(11, 0);
    let store = start(&disk, 0, false).unwrap();
    drop(store);
    disk.kill();
    assert!(!disk.list().unwrap().iter().any(|n| n == MARKER_NAME));
    assert!(start(&disk, 0, false).is_ok());
}

/// A failed write fails the volume as a failed sync does; a store
/// that lets it pass is noticed.
#[test]
fn a_failed_write_fails_the_volume() {
    let disk = created(5, 0);
    let mut defective = start(&disk, Mutants::WRITE_ERROR_PASSES, false).unwrap();
    disk.fail_write(0, false);
    assert_ne!(write(&mut defective, 1, 0x11), Err(BlkError::Failed));
    assert!(!defective.is_failed(), "the defect is not noticed");
    drop(defective);
    let disk = created(5, 0);
    let mut store = start(&disk, 0, false).unwrap();
    disk.fail_write(0, false);
    assert_eq!(write(&mut store, 1, 0x11), Err(BlkError::Failed));
    assert!(store.is_failed());
    assert_eq!(read(&mut store, 0), Err(BlkError::Failed));
    drop(store);
    disk.kill();
    assert!(matches!(
        start(&disk, 0, false),
        Err(StoreError::Log(LogError::MarkedFailed))
    ));
}

/// The log store reads what the in-place store wrote: it is in the home
/// image, and the empty log replays nothing over it. (Reading only: a
/// volume served in place must not be given back to the log store to
/// serve; see the ignored test below.)
#[test]
fn a_volume_goes_back_to_the_log_store() {
    let disk = created(6, 0);
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, 7, 0x77).unwrap();
    drop(store);
    disk.kill();
    let (mut store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
    store
        .submit(Request {
            id: RequestId(1),
            op: Op::Read {
                offset: 7 * BLOCK,
                buf: Buf::filled(1, 0),
            },
        })
        .unwrap();
    let mut done = Vec::new();
    store.wait(&mut done, Duration::ZERO);
    assert_eq!(done[0].result, Ok(()));
    assert_eq!(done[0].buf.as_deref(), Some(&[0x77; BLOCK_USIZE][..]));
}

/// Read one block through the log store.
fn log_read(store: &mut Store<SimDisk>, block: u64) -> Vec<u8> {
    store
        .submit(Request {
            id: RequestId(500 + block),
            op: Op::Read {
                offset: block * BLOCK,
                buf: Buf::filled(1, 0xEE),
            },
        })
        .unwrap();
    let mut done = Vec::new();
    while done.is_empty() {
        store.wait(&mut done, Duration::ZERO);
    }
    assert_eq!(done[0].result, Ok(()));
    done[0].buf.as_deref().unwrap().to_vec()
}

/// Back on the log store, a completed flush covers what the in-place
/// store acknowledged before its process was lost (rule R7): the log
/// store's open syncs the home image. The in-place store writes block
/// 7 and is lost; the volume is started with the log store (or, as the
/// control, with the in-place store again), which writes block 9 and
/// completes a flush; a power cut that keeps nothing unsynced leaves
/// both.
///
/// It fails: the log store's open does not sync the home image, and
/// its flush syncs the log alone, so block 7 is lost. Owed (the review
/// of M1.3r, finding A); the lead ruled that the log store's open is
/// not changed in M1.3r, and the module documentation and `skyzfsd
/// help` say that a volume served in place must not be given back to
/// the log store. Run it with `--ignored` to see the defect.
#[test]
#[ignore = "owed: back on the log store, a flush does not cover an in-place write (M1.3r, finding A)"]
fn a_flush_on_the_log_store_covers_what_the_in_place_store_wrote() {
    for back_in_place in [true, false] {
        let disk = created(6, 0);
        let mut store = start(&disk, 0, false).unwrap();
        write(&mut store, 7, 0x77).unwrap();
        drop(store);
        disk.kill();
        if back_in_place {
            let mut store = start(&disk, 0, false).unwrap();
            write(&mut store, 9, 0x99).unwrap();
            assert_eq!(run(&mut store, 2, Op::Flush).result, Ok(()));
        } else {
            let (mut store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
            assert_eq!(log_read(&mut store, 7), vec![0x77; BLOCK_USIZE]);
            for (id, op) in [
                (
                    1,
                    Op::Write {
                        offset: 9 * BLOCK,
                        data: Buf::filled(1, 0x99),
                    },
                ),
                (2, Op::Flush),
            ] {
                store
                    .submit(Request {
                        id: RequestId(id),
                        op,
                    })
                    .unwrap();
            }
            let mut done = Vec::new();
            while done.len() < 2 {
                store.wait(&mut done, Duration::ZERO);
            }
            assert!(done.iter().all(|c| c.result.is_ok()), "{done:?}");
        }
        disk.power_cut(crate::sim::LoseAll);
        let (mut store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
        assert_eq!(
            (log_read(&mut store, 7)[0], log_read(&mut store, 9)[0]),
            (0x77, 0x99),
            "back in place: {back_in_place}"
        );
    }
}

/// A wait with a zero timeout only looks: a sync under way that has not
/// ended is left so, and a wait with a timeout waits for its end.
#[test]
fn a_wait_with_no_time_does_not_wait_for_a_sync() {
    // The disk ends a sync after up to three looks, as its seed says:
    // some seeds leave it under way at the first look.
    let mut left_under_way = 0;
    for seed in 1..=20 {
        let disk = created(seed, 3);
        let mut store = start(&disk, 0, false).unwrap();
        write(&mut store, 1, 0x11).unwrap();
        store
            .submit(Request {
                id: RequestId(2),
                op: Op::Flush,
            })
            .unwrap();
        let mut done = Vec::new();
        if store.wait(&mut done, Duration::ZERO) == 0 {
            assert!(store.syncing());
            left_under_way += 1;
            assert_eq!(store.wait(&mut done, WAIT), 1);
        }
        assert_eq!(done[0].result, Ok(()));
    }
    assert!(left_under_way > 0, "every sync ended at the first look");
}

/// The start refuses a log store with requests in flight.
#[test]
fn the_start_refuses_a_store_with_requests_in_flight() {
    let disk = created(7, 0);
    let (mut store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
    store
        .submit(Request {
            id: RequestId(1),
            op: Op::Flush,
        })
        .unwrap();
    assert!(matches!(
        store.into_in_place(None),
        Err(IntoInPlaceError::InFlight)
    ));
}

/// A start whose conversion meets the failed disk, at each of the syncs
/// the conversion makes, gives `Failed`, and the next start finds the
/// volume marked as failed. Where the in-place store's own sync failed
/// (the last), the marker is handed back unwritten, with the lock: no
/// marker yet, and no other start, until the caller finishes it. The
/// start says what failed: here the log holds operations to apply, so
/// the image at that sync, the log store at every one before it (the
/// daemon's failure note gives the two different reasons). With
/// nothing to apply, see
/// `a_start_with_nothing_to_apply_reports_the_image_at_every_sync`.
#[test]
fn a_start_that_meets_the_failed_disk_fails_the_volume() {
    let mut handed_back = 0;
    let mut failing = 0;
    for nth in 0.. {
        let disk = created(12, 0);
        let (store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
        disk.fail_sync(nth, false);
        let start_failed = match store.into_in_place(None) {
            Ok(_) => break,
            Err(IntoInPlaceError::Failed(start_failed)) => start_failed,
            Err(other) => panic!("sync {nth}: {other}"),
        };
        failing += 1;
        let marked = || disk.list().unwrap().iter().any(|n| n == MARKER_NAME);
        let failed = start_failed.failed();
        assert_eq!(
            failed == StartFailed::Image,
            !marked(),
            "sync {nth}: {failed:?}"
        );
        if !marked() {
            handed_back += 1;
            assert!(matches!(
                Store::open(disk.clone(), OpenOptions::default()),
                Err(StoreError::Locked)
            ));
            // The lock is released after the marker: while the marker's
            // sync is held, no other start opens the volume.
            disk.hold_whole_syncs_of(MARKER_NAME);
            let finishing = std::thread::spawn(move || start_failed.finish());
            until_held(&disk);
            assert!(
                matches!(
                    Store::open(disk.clone(), OpenOptions::default()),
                    Err(StoreError::Locked)
                ),
                "sync {nth}: the volume opened while its marker was being written"
            );
            disk.release_syncs();
            finishing.join().unwrap();
            assert!(marked(), "sync {nth}: no marker after the finish");
        } else {
            drop(start_failed);
        }
        disk.clear_faults();
        disk.kill();
        assert!(
            matches!(
                start(&disk, 0, false),
                Err(StoreError::Log(LogError::MarkedFailed))
            ),
            "sync {nth}"
        );
    }
    assert!(failing >= 2, "{failing} syncs failed the conversion");
    assert_eq!(
        handed_back, 1,
        "the in-place store's own sync hands it back"
    );
}

/// A start whose log holds nothing to apply, as after an earlier start
/// in place (check g's volume, whose log held 0 operations): a failure
/// at each sync the start makes, the log store's hand-over (whose
/// marker is written) and the in-place store's own (whose marker is
/// handed back) alike, is reported as the image's. Nothing of the log
/// store's own is at stake there; a failure of the log file's own sync
/// is reported so too, which is accepted until M1.3t removes the
/// hand-over (`StartFailed`). The next start finds the volume failed.
#[test]
fn a_start_with_nothing_to_apply_reports_the_image_at_every_sync() {
    let mut failing = 0;
    let mut handed_back = 0;
    for nth in 0.. {
        let disk = created(12, 0);
        // A first start applies the log; the next has nothing to apply.
        drop(start(&disk, 0, false).unwrap());
        disk.kill();
        let (store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
        disk.fail_sync(nth, false);
        let start_failed = match store.into_in_place(None) {
            Ok(_) => break,
            Err(IntoInPlaceError::Failed(start_failed)) => start_failed,
            Err(other) => panic!("sync {nth}: {other}"),
        };
        failing += 1;
        assert_eq!(start_failed.failed(), StartFailed::Image, "sync {nth}");
        if !disk.list().unwrap().iter().any(|n| n == MARKER_NAME) {
            handed_back += 1;
        }
        drop(start_failed);
        disk.clear_faults();
        disk.kill();
        assert!(
            matches!(
                start(&disk, 0, false),
                Err(StoreError::Log(LogError::MarkedFailed))
            ),
            "sync {nth}"
        );
    }
    assert!(
        failing >= 2,
        "{failing} syncs failed the start: the hand-over's are not reached"
    );
    assert_eq!(
        handed_back, 1,
        "the in-place store's own sync hands it back"
    );
}

// The volatile-cache disk under the simulated disk's own power cuts.

/// What a block may hold: every version since the oldest that may
/// still come back.
#[derive(Debug, Clone)]
struct Versions {
    all: Vec<Vec<u8>>,
    /// The oldest version a power cut may leave: everything older is
    /// covered by a completed flush or a start.
    floor: usize,
}

impl Versions {
    fn newest(&self) -> usize {
        self.all.len() - 1
    }
}

/// One step of a random program.
#[derive(Debug, Clone, Copy)]
enum Step {
    Write { block: u64, count: u64, byte: u8 },
    Zeroes { block: u64, count: u64 },
    Discard { block: u64, count: u64 },
    Flush,
    Poll,
    Wait,
    Kill,
}

/// The blocks a program works in.
const WINDOW: u64 = 12;

fn steps(seed: u64, n: usize) -> Vec<Step> {
    let mut rng = seed | 1;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    (0..n)
        .map(|_| {
            let z = next();
            let count = 1 + (z >> 8) % 3;
            let block = (z >> 16) % (WINDOW - count + 1);
            match z % 16 {
                0..=6 => Step::Write {
                    block,
                    count,
                    byte: (z >> 24) as u8 | 1,
                },
                7 => Step::Zeroes { block, count },
                8 => Step::Discard { block, count },
                9..=11 => Step::Flush,
                12 | 13 => Step::Poll,
                14 => Step::Wait,
                _ => Step::Kill,
            }
        })
        .collect()
}

/// How a program ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// The simulated disk's power cut, drawn from the seed; with
    /// garbage, a sector the cut takes may hold anything.
    Cut { garbage: bool },
    /// A write (or, with `sync`, a sync) fails part of the way through
    /// the program, as the simulated disk fails them: a failed write
    /// leaves any subset of its sectors, a failed sync may drop what
    /// was not synced. The volume fails; the process is lost, and the
    /// volume is started again with the failure acknowledged.
    Failure { sync: bool },
}

/// A program on the in-place store, its ending, a start, and a read of
/// every block of the window: each block must be explained by the
/// volatile-cache disk ([`volatile_cache_explains`]). Whether the
/// volume had failed when the program ended.
fn volatile_cache_holds(seed: u64, ending: Ending, mutants: u32) -> Result<bool, String> {
    volatile_cache_holds_with(seed, ending, mutants, 4096, IoMode::default())
}

/// The same on a system whose page is `page` bytes ([`created_at`]).
fn volatile_cache_holds_at(
    seed: u64,
    ending: Ending,
    mutants: u32,
    page: u64,
) -> Result<bool, String> {
    volatile_cache_holds_with(seed, ending, mutants, page, IoMode::default())
}

/// [`volatile_cache_holds`], with the store's I/O done as `io` says.
fn volatile_cache_holds_io(
    seed: u64,
    ending: Ending,
    mutants: u32,
    io: IoMode,
) -> Result<bool, String> {
    volatile_cache_holds_with(seed, ending, mutants, 4096, io)
}

/// [`volatile_cache_holds`] on a page of `page` bytes, with the store's
/// I/O done as `io` says.
fn volatile_cache_holds_with(
    seed: u64,
    ending: Ending,
    mutants: u32,
    page: u64,
    io: IoMode,
) -> Result<bool, String> {
    let garbage = ending == Ending::Cut { garbage: true };
    // Where a failure is set up, and whether it has been.
    let fail_at = match ending {
        Ending::Failure { .. } => Some(seed as usize % 30),
        Ending::Cut { .. } => None,
    };
    let mut failing = false;
    let disk = created_at(seed, 3, page);
    let mut store = start_io(&disk, mutants, false, io).map_err(|e| e.to_string())?;
    let mut blocks: Vec<Versions> = (0..WINDOW)
        .map(|b| {
            let byte = if b == 3 { 0x33 } else { 0 };
            Versions {
                all: vec![vec![byte; BLOCK_USIZE]],
                floor: 0,
            }
        })
        .collect();
    // For each flush in flight: the newest version of each block when
    // it was received.
    let mut flushes: Vec<(RequestId, Vec<usize>)> = Vec::new();
    let mut id = 0u64;
    let mut done = Vec::new();
    // A request may fail once the failure is set up, and a failed flush
    // promises nothing.
    let settle = |done: &mut Vec<Completion>,
                  blocks: &mut Vec<Versions>,
                  flushes: &mut Vec<(RequestId, Vec<usize>)>,
                  failing: bool|
     -> Result<(), String> {
        for completion in done.drain(..) {
            let flush = flushes.iter().position(|(id, _)| *id == completion.id);
            match (completion.result, flush) {
                (Ok(()), Some(at)) => {
                    let (_, marks) = flushes.remove(at);
                    for (block, mark) in blocks.iter_mut().zip(marks) {
                        block.floor = block.floor.max(mark);
                    }
                }
                (Ok(()), None) => {}
                (Err(_), Some(at)) if failing => {
                    flushes.remove(at);
                }
                (Err(_), None) if failing => {}
                (Err(e), _) => return Err(format!("a request failed: {e}")),
            }
        }
        Ok(())
    };
    for (index, step) in steps(seed, 40).into_iter().enumerate() {
        if fail_at == Some(index) {
            failing = true;
            match ending {
                Ending::Failure { sync: true } => disk.fail_sync(0, false),
                _ => disk.fail_write(0, false),
            }
        }
        id += 1;
        let request = |op| Request {
            id: RequestId(id),
            op,
        };
        match step {
            Step::Write { block, count, byte } => {
                store
                    .submit(request(Op::Write {
                        offset: block * BLOCK,
                        data: Buf::filled(count as usize, byte),
                    }))
                    .map_err(|e| e.to_string())?;
                for b in block..block + count {
                    blocks[b as usize].all.push(vec![byte; BLOCK_USIZE]);
                }
            }
            Step::Zeroes { block, count } => {
                store
                    .submit(request(Op::WriteZeroes {
                        offset: block * BLOCK,
                        len: count * BLOCK,
                    }))
                    .map_err(|e| e.to_string())?;
                for b in block..block + count {
                    blocks[b as usize].all.push(vec![0; BLOCK_USIZE]);
                }
            }
            // The store leaves a discarded block as it is.
            Step::Discard { block, count } => store
                .submit(request(Op::Discard {
                    offset: block * BLOCK,
                    len: count * BLOCK,
                }))
                .map_err(|e| e.to_string())?,
            Step::Flush => {
                store
                    .submit(request(Op::Flush))
                    .map_err(|e| e.to_string())?;
                flushes.push((RequestId(id), blocks.iter().map(Versions::newest).collect()));
            }
            Step::Poll => {
                store.poll(&mut done);
            }
            Step::Wait => {
                store.wait(&mut done, WAIT);
            }
            // After a failure, the program ends with the process.
            Step::Kill if failing => break,
            Step::Kill => {
                store.poll(&mut done);
                settle(&mut done, &mut blocks, &mut flushes, failing)?;
                drop(store);
                disk.kill();
                // Flushes in flight are lost with the process; the
                // start's sync covers everything.
                flushes.clear();
                store = start_io(&disk, mutants, false, io).map_err(|e| e.to_string())?;
                for block in &mut blocks {
                    block.floor = block.newest();
                }
            }
        }
        settle(&mut done, &mut blocks, &mut flushes, failing)?;
    }
    // What the threads hold ends before the process does, as the
    // store's drop waits for it: a write among it that fails fails the
    // volume, and the program counts as one that failed.
    while store.io_queue().in_flight > 0 {
        store.wait(&mut done, WAIT);
        settle(&mut done, &mut blocks, &mut flushes, failing)?;
    }
    let failed = store.is_failed();
    drop(store);
    match ending {
        Ending::Cut { garbage } => disk.power_cut(if garbage {
            SeededCut::with_garbage(seed)
        } else {
            SeededCut::new(seed)
        }),
        Ending::Failure { .. } => {
            disk.clear_faults();
            disk.kill();
        }
    }
    let acknowledge = matches!(ending, Ending::Failure { .. });
    let mut store = start_io(&disk, 0, acknowledge, io).map_err(|e| e.to_string())?;
    for (b, block) in blocks.iter().enumerate() {
        let got = read(&mut store, b as u64).map_err(|e| e.to_string())?;
        let durable = &block.all[block.floor];
        let since = &block.all[block.floor + 1..];
        if let Err(sector) = volatile_cache_explains(&got, durable, since, garbage) {
            return Err(format!(
                "seed {seed}: block {b}, sector {sector}: {:#04x}, none of the versions {:?}",
                got[sector * 512],
                block.all[block.floor..]
                    .iter()
                    .map(|v| v[sector * 512])
                    .collect::<Vec<_>>()
            ));
        }
    }
    Ok(failed)
}

/// Acceptance, the volatile-cache disk: after random programs of
/// writes, write-zeroes, discards, flushes, kills and syncs that end
/// up to three looks after they begin, and the simulated disk's own
/// power cut (each sector its durable content or any version written
/// since, independently), every block is explained: what a completed
/// flush or a start covered is there or newer, and any other sector is
/// a version written since. With garbage, a sector that a cut could
/// take may be anything, and the rest holds as before.
#[test]
fn a_power_cut_leaves_what_the_volatile_cache_disk_promises() {
    for seed in 1..=200 {
        for garbage in [false, true] {
            let ending = Ending::Cut { garbage };
            if let Err(e) = volatile_cache_holds(seed, ending, 0) {
                panic!("{ending:?}: {e}");
            }
        }
    }
}

/// With each of the other I/O modes, a power cut, with garbage or
/// without, and a failing write or sync leave what the volatile-cache
/// disk promises, as with the default.
#[test]
fn each_io_keeps_what_the_volatile_cache_disk_promises() {
    for io in OTHER_IO {
        for seed in 1..=100 {
            for ending in [
                Ending::Cut { garbage: false },
                Ending::Cut { garbage: true },
                Ending::Failure { sync: false },
                Ending::Failure { sync: true },
            ] {
                if let Err(e) = volatile_cache_holds_io(seed, ending, 0, io) {
                    panic!("{io:?}, {ending:?}: {e}");
                }
            }
        }
    }
}

/// The start refuses, by reason, a file system that offers no direct
/// I/O and one whose unit for it is above a block (4 KiB), and the
/// volume has not failed: it starts where the unit is a block or less.
#[test]
fn a_start_refuses_direct_io_it_cannot_have() {
    let disk = created(21, 0);
    let refused = |disk: &SimDisk| {
        let (store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
        match store.into_in_place(None) {
            Err(IntoInPlaceError::Io(why)) => why,
            other => panic!("{other:?}"),
        }
    };
    disk.set_direct_align(None);
    assert!(refused(&disk).contains("no direct I/O"));
    for offset in [8192, 16384] {
        disk.set_direct_align(Some(crate::io::DirectAlign {
            memory: 512,
            offset,
        }));
        assert!(refused(&disk).contains(&offset.to_string()));
    }
    for (memory, offset) in [(4096, 4096), (512, 512), (4, 512)] {
        disk.set_direct_align(Some(crate::io::DirectAlign { memory, offset }));
        assert!(start(&disk, 0, false).is_ok(), "{memory} {offset}");
    }
}

/// Where this store promises less than the suite's rule R11: a write
/// or a sync that fails part of the way through leaves the blocks that
/// no completed flush covered as a power cut leaves them, so after a
/// failure and a start with the failure acknowledged, what a completed
/// flush or a start covered is intact and every other block is
/// explained by the volatile-cache disk. The test also notices when no
/// failure happened at all: some programs must fail.
#[test]
fn a_failure_leaves_what_the_volatile_cache_disk_promises() {
    for sync in [false, true] {
        let ending = Ending::Failure { sync };
        let mut failed = 0;
        for seed in 1..=200 {
            match volatile_cache_holds(seed, ending, 0) {
                Ok(true) => failed += 1,
                Ok(false) => {}
                Err(e) => panic!("{ending:?}: {e}"),
            }
        }
        assert!(failed >= 50, "{ending:?}: {failed} of 200 programs failed");
    }
}

/// The failure test notices a sync that is tried again after it
/// failed: the failed sync dropped what it did not make durable, and
/// the flush that the second sync answers promises it.
#[test]
fn the_failure_test_notices_a_sync_tried_again() {
    let ending = Ending::Failure { sync: true };
    let noticed =
        (1..=200).any(|seed| volatile_cache_holds(seed, ending, Mutants::SYNC_RETRIED).is_err());
    assert!(noticed, "the defect is not noticed");
}

/// The power-cut test notices the defects of durability: a flush that
/// joins a sync begun before a write it must cover, a flush that
/// makes nothing durable.
#[test]
fn the_power_cut_test_notices_the_defects_of_durability() {
    for mutant in [Mutants::FLUSH_JOINS_ANY_SYNC, Mutants::FLUSH_WITHOUT_SYNC] {
        let ending = Ending::Cut { garbage: false };
        let noticed = (1..=200).any(|seed| volatile_cache_holds(seed, ending, mutant).is_err());
        assert!(noticed, "the defect {mutant:#x} is not noticed");
    }
}

/// The check itself: a sector of the durable content or of a version
/// since passes; a sector of an older version, or of nothing, does
/// not; with garbage anything passes where versions are since, and
/// nothing changes where none are.
#[test]
fn the_volatile_cache_check_takes_what_it_should() {
    let old = vec![1u8; 1024];
    let durable = vec![2u8; 1024];
    let since = vec![vec![3u8; 1024]];
    let mixed: Vec<u8> = [vec![2u8; 512], vec![3u8; 512]].concat();
    assert_eq!(
        volatile_cache_explains(&mixed, &durable, &since, false),
        Ok(())
    );
    assert_eq!(
        volatile_cache_explains(&old, &durable, &since, false),
        Err(0)
    );
    let junk = vec![9u8; 1024];
    assert_eq!(
        volatile_cache_explains(&junk, &durable, &since, true),
        Ok(())
    );
    assert_eq!(volatile_cache_explains(&junk, &durable, &[], true), Err(0));
}

/// The harness's target is the store when it lives.
#[test]
fn the_harness_gives_the_store() {
    let mut harness = harnesses(0)[0];
    use skyzfs_blk::conformance::{Harness, Spec};
    let mut target: OnSim = harness
        .create(&Spec {
            base: Base::Empty,
            read_only: false,
            seed: 9,
        })
        .unwrap();
    assert!(target.store().is_some());
}

// Pages larger than 4 KiB, as some arm64 kernels and Apple silicon have.

/// The page sizes beyond amd64's that the store is tested at.
const LARGE_PAGES: [u64; 2] = [16 << 10, 64 << 10];

/// The suite passes on a system whose page, file-system block and
/// volume write unit are 16 KiB or 64 KiB: a write of one 4 KiB block
/// then rewrites a whole page around it. The page size is the
/// simulated disk's and passed in, so this runs on amd64 too.
#[test]
fn the_in_place_store_passes_the_conformance_suite_on_large_pages() {
    let config = Config {
        cases: 16,
        ..Config::default()
    };
    for page in LARGE_PAGES {
        let mut harness = InPlaceHarness {
            page,
            ..harnesses(0)[2]
        };
        let report = run_all(&mut harness, &config);
        assert!(report.passed(), "in {harness:?}:\n{report}");
        assert!(report.unexercised().is_empty(), "{harness:?}:\n{report}");
        assert!(report.weaker().is_empty(), "{harness:?}:\n{report}");
    }
}

/// The volatile-cache disk holds on large pages. A write of one block
/// rewrites the other blocks of its page with what the page cache holds
/// of them: their durable content or a version written since, so a cut
/// that takes such a sector leaves what the rule allows. A neighbour a
/// flush covered and nothing wrote since is rewritten with the bytes it
/// holds. The simulated disk's garbage is left out: a cut that turns a
/// rewritten neighbour's sector into garbage breaks the rule on any
/// page larger than a block, which is the store's documented exception
/// (`a_cut_with_garbage_may_reach_a_neighbour_on_large_pages`).
///
/// This is the store writing through the cache, as only tests have it;
/// with direct I/O, as served, every ending holds on large pages,
/// garbage included
/// (`direct_io_keeps_a_cut_with_garbage_from_the_neighbours_on_large_pages`).
#[test]
fn a_power_cut_leaves_what_the_volatile_cache_disk_promises_on_large_pages() {
    for page in LARGE_PAGES {
        for seed in 1..=100 {
            let ending = Ending::Cut { garbage: false };
            if let Err(e) = volatile_cache_holds_with(seed, ending, 0, page, BUFFERED) {
                panic!("page {page}: {e}");
            }
        }
    }
}

/// A failure on large pages leaves what a power cut leaves (R11).
#[test]
fn a_failure_leaves_what_the_volatile_cache_disk_promises_on_large_pages() {
    for page in LARGE_PAGES {
        for sync in [false, true] {
            let ending = Ending::Failure { sync };
            let mut failed = 0;
            for seed in 1..=100 {
                match volatile_cache_holds_at(seed, ending, 0, page) {
                    Ok(true) => failed += 1,
                    Ok(false) => {}
                    Err(e) => panic!("page {page}, {ending:?}: {e}"),
                }
            }
            assert!(failed >= 25, "page {page}, {ending:?}: {failed} failed");
        }
    }
}

/// The large-page tests can tell: the defects of durability are
/// noticed there as on 4 KiB pages.
#[test]
fn the_power_cut_test_notices_the_defects_of_durability_on_large_pages() {
    for page in LARGE_PAGES {
        for mutant in [Mutants::FLUSH_JOINS_ANY_SYNC, Mutants::FLUSH_WITHOUT_SYNC] {
            let ending = Ending::Cut { garbage: false };
            let noticed =
                (1..=200).any(|seed| volatile_cache_holds_at(seed, ending, mutant, page).is_err());
            assert!(
                noticed,
                "page {page}: the defect {mutant:#x} is not noticed"
            );
        }
    }
}

/// The exception of writes through the cache, pinned, and the reason
/// for decision D6: where a cut can write garbage, a sector of a block
/// that a flush covered and nothing wrote since can be lost on a large
/// page, because a write to another block of the page rewrote it. On
/// 4 KiB pages the same programs keep such blocks
/// (`a_power_cut_leaves_what_the_volatile_cache_disk_promises`), and so
/// does direct I/O on large pages
/// (`direct_io_keeps_a_cut_with_garbage_from_the_neighbours_on_large_pages`).
/// The store writes through the cache only in tests.
#[test]
fn a_cut_with_garbage_may_reach_a_neighbour_on_large_pages() {
    let ending = Ending::Cut { garbage: true };
    for page in LARGE_PAGES {
        let reached = (1..=200)
            .any(|seed| volatile_cache_holds_with(seed, ending, 0, page, BUFFERED).is_err());
        assert!(reached, "page {page}: no cut reached a neighbour");
    }
}

/// Direct I/O takes the exception away, on the simulated disk: a
/// direct write writes its own sectors and no other block of its page,
/// so a cut with garbage reaches only blocks being written, as on
/// 4 KiB pages. The same programs through the cache reach a neighbour
/// (`a_cut_with_garbage_may_reach_a_neighbour_on_large_pages`). The
/// other endings hold too.
///
/// What this rests on: the simulated disk takes a direct write to touch
/// nothing but its own sectors. On a file system whose block is larger
/// than the write, a direct write into a hole or an unwritten extent
/// also has the rest of that block zeroed. By my reading of the kernel
/// (not shown here) those zeros cannot be read back as garbage: the
/// block reads as zeros until its extent is converted to written, and
/// the journal commit that makes the conversion durable flushes the
/// disk's cache first. An image written with zeros at its creation
/// ([`crate::store::fill_home`]) has no such block.
#[test]
fn direct_io_keeps_a_cut_with_garbage_from_the_neighbours_on_large_pages() {
    for page in LARGE_PAGES {
        for io in [IoMode::default(), OTHER_IO[0], OTHER_IO[1]] {
            for ending in [
                Ending::Cut { garbage: true },
                Ending::Cut { garbage: false },
                Ending::Failure { sync: false },
                Ending::Failure { sync: true },
            ] {
                for seed in 1..=200 {
                    if let Err(e) = volatile_cache_holds_with(seed, ending, 0, page, io) {
                        panic!("page {page}, {io:?}, {ending:?}, seed {seed}: {e}");
                    }
                }
            }
        }
    }
}

/// A volume created where the page is 4 KiB is refused where it is
/// 16 KiB, on the way into the in-place store: a write there would
/// rewrite more than the volume's write unit. The refusal is the log
/// store's open, which the in-place store's start goes through.
#[test]
fn a_volume_of_small_pages_is_refused_on_large_pages() {
    let disk = created(5, 0);
    disk.set_page_size(16 << 10);
    disk.set_write_unit(16 << 10);
    let (opened, _) = match Store::open(disk.clone(), OpenOptions::default()) {
        Err(error) => (error, ()),
        Ok(_) => panic!("a volume of 4 KiB pages opened on 16 KiB pages"),
    };
    assert!(
        matches!(
            opened,
            StoreError::Pages {
                page: 16384,
                unit: 4096
            }
        ),
        "{opened}"
    );
    // Where the page is 4 KiB again, it starts.
    disk.set_page_size(4096);
    disk.set_write_unit(4096);
    assert!(start(&disk, 0, false).is_ok());
}

// Every start serves only what the disk holds.

/// What the reopened volume shows of block 4 after a failed sync, and
/// what it holds after a write elsewhere, a completed flush and a power
/// cut that keeps nothing unsynced: the two must be the same. A failed
/// sync can leave a page marked clean that the disk never got; a start
/// that served it from the cache would show what the cut then takes.
fn shown_survives(seed: u64, mutants: u32, io: IoMode) -> Result<bool, String> {
    let e = |e: BlkError| e.to_string();
    let disk = created(seed, 0);
    let mut store = start_io(&disk, mutants, false, io).map_err(|e| e.to_string())?;
    write(&mut store, 4, 0x11).map_err(e)?;
    run(&mut store, 100, Op::Flush).result.map_err(e)?;
    write(&mut store, 4, 0x44).map_err(e)?;
    disk.fail_sync(0, false);
    if run(&mut store, 101, Op::Flush).result.is_ok() {
        return Err("the flush did not fail".into());
    }
    drop(store);
    disk.clear_faults();
    disk.kill();
    let mut store = start_io(&disk, mutants, true, io).map_err(|e| e.to_string())?;
    let shown = read(&mut store, 4).map_err(e)?;
    write(&mut store, 5, 0x55).map_err(e)?;
    run(&mut store, 102, Op::Flush).result.map_err(e)?;
    drop(store);
    disk.power_cut(crate::sim::LoseAll);
    let mut store = start(&disk, 0, false).map_err(|e| e.to_string())?;
    Ok(read(&mut store, 4).map_err(e)? == shown)
}

/// Acceptance (design §4.1): what a start shows survives a completed
/// flush and a power cut, also after a failed sync, at 64 seeds of the
/// failed sync's leavings, with direct I/O and through the cache.
///
/// Through the cache, a failed sync can leave pages in the kernel's
/// cache that the disk never got, and a start that does not drop the
/// cache serves them: that is noticed. Direct I/O leaves no page there,
/// so the same defect changes nothing with it; what the drop still
/// guards under direct I/O is what a writer through the cache left
/// before the start: the log store's checkpoint that empties the log
/// into the image when a volume enters the in-place store (M1.3t
/// removes the log store), or a file system that serves direct
/// requests through its cache (the drop stays for it).
#[test]
fn every_start_serves_only_what_the_disk_holds() {
    for io in [IoMode::default(), BUFFERED] {
        for seed in 0..64 {
            assert_eq!(shown_survives(seed, 0, io), Ok(true), "{io:?}, seed {seed}");
        }
    }
    let dropped_nothing = Mutants::CACHE_NOT_DROPPED | Mutants::CACHE_NOT_CHECKED;
    let lost = (0..64)
        .filter(|&seed| shown_survives(seed, dropped_nothing, BUFFERED) == Ok(false))
        .count();
    assert!(lost > 0, "a start that keeps the cache is not noticed");
}

/// Open and enter the in-place store, as a daemon would.
fn enter(disk: &SimDisk, mutants: u32) -> Result<InPlace<SimDisk>, IntoInPlaceError> {
    let (store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
    store.into_in_place_with(None, Mutants(mutants))
}

/// A start whose drop of the cache leaves pages (something else maps,
/// locks or reads the file), or that runs on a kernel that cannot count them,
/// refuses the volume, which has not failed: the next start, with the
/// cache dropped, serves it. A start that does not check is noticed.
#[test]
fn a_start_refuses_a_volume_whose_cache_stays() {
    let disk = created(13, 0);
    disk.keep_cache(true);
    let refused = enter(&disk, 0);
    assert!(
        matches!(&refused, Err(IntoInPlaceError::CacheKept(why)) if why.contains("stay")),
        "{refused:?}"
    );
    drop(refused);
    assert!(
        enter(&disk, Mutants::CACHE_NOT_CHECKED).is_ok(),
        "the defect is noticed only if the start that does not check starts"
    );
    disk.keep_cache(false);
    disk.set_cache_counts(false);
    let refused = enter(&disk, 0);
    assert!(
        matches!(&refused, Err(IntoInPlaceError::CacheKept(why)) if why.contains("cachestat")),
        "{refused:?}"
    );
    drop(refused);
    disk.set_cache_counts(true);
    assert!(enter(&disk, 0).is_ok());
}

/// The count is shown to see the file before it is trusted to see
/// none: where it answers 0 whatever the cache holds (`cachestat`
/// through a file of an overlay file system), the start refuses, though
/// every page could still be there. A start that does not look is
/// noticed: it serves.
#[test]
fn a_start_refuses_where_the_count_cannot_see_the_file() {
    let disk = created(15, 0);
    disk.blind_cache_count(true);
    disk.keep_cache(true);
    let refused = enter(&disk, 0);
    assert!(
        matches!(&refused, Err(IntoInPlaceError::CacheKept(why)) if why.contains("cannot be observed")),
        "{refused:?}"
    );
    drop(refused);
    assert!(
        enter(&disk, Mutants::CACHE_SIGHT_UNCHECKED).is_ok(),
        "the defect is noticed only if the start that does not look starts"
    );
    disk.blind_cache_count(false);
    disk.keep_cache(false);
    assert!(enter(&disk, 0).is_ok());
}

/// The look follows the drop: a read before the drop starts the
/// kernel's read-ahead, which the drop cannot take while it is still
/// under I/O, and the start would refuse a healthy volume, as it did on
/// a pool's label (the third look at M1.3s). A start that looks first
/// is noticed: it refuses.
#[test]
fn the_look_comes_after_the_drop() {
    for (mutants, serves) in [(0, true), (Mutants::LOOK_BEFORE_DROP, false)] {
        let disk = created(17, 0);
        let (store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
        disk.hold_read_ahead_of(HOME_NAME, 3);
        let entered = store.into_in_place_with(None, Mutants(mutants));
        match entered {
            Ok(_) => assert!(serves, "{mutants:#x}: the start that looks first served"),
            Err(IntoInPlaceError::CacheKept(why)) if !serves => {
                assert!(why.contains("two drops"), "{why}");
            }
            Err(e) => panic!("{mutants:#x}: {e:?}"),
        }
    }
}

/// The drop is advisory and tried twice: a first drop that leaves
/// pages is not yet a refusal, a second one is.
#[test]
fn the_start_drops_the_cache_twice_before_it_refuses() {
    let disk = created(16, 0);
    disk.keep_cache_for(1);
    assert!(
        enter(&disk, 0).is_ok(),
        "one drop that left pages refused the start"
    );
    disk.keep_cache_for(2);
    let refused = enter(&disk, 0);
    assert!(
        matches!(&refused, Err(IntoInPlaceError::CacheKept(why)) if why.contains("two drops")),
        "{refused:?}"
    );
    drop(refused);
    disk.keep_cache_for(0);
    assert!(enter(&disk, 0).is_ok());
}
