use std::time::Duration;

use skyzfs_blk::conformance::{run_all, Config};
use skyzfs_blk::{
    Base, BlkError, BlockTarget, Buf, Completion, Op, Request, RequestId, BLOCK_USIZE,
};

use super::harness::{InPlaceHarness, OnSim};
use super::*;
use crate::io::Dir;
use crate::sim::{LoseAll, SeededCut, SimDisk};
use crate::store::HOME_NAME;
use crate::volume::MARKER_NAME;

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

/// Wait, for at most 10 s, until request `id` is among `done`: for a
/// planted defect that lets a request run on the threads, where it ends
/// when a thread gets to it and not at once. Says whether it ended.
fn until_done(store: &mut InPlace<SimDisk>, done: &mut Vec<Completion>, id: u64) -> bool {
    let began = std::time::Instant::now();
    while !done.iter().any(|c| c.id.0 == id) {
        if began.elapsed() > Duration::from_secs(10) {
            return false;
        }
        store.wait(done, Duration::from_millis(10));
    }
    true
}

/// How long [`drained`] waits for what is in flight to end.
const DRAIN: Duration = Duration::from_secs(10);

/// Take every completion of what is in flight, for at most [`DRAIN`],
/// and give the store back. A store that does not drain in that time
/// has a defect that keeps a request from ending (a job a dead thread
/// did not hand back, say): it is leaked and not dropped, because its
/// drop waits for every request in flight, and the test fails instead
/// of hanging.
fn drained(mut store: InPlace<SimDisk>, done: &mut Vec<Completion>) -> InPlace<SimDisk> {
    let began = std::time::Instant::now();
    while store.in_flight() > 0 {
        if began.elapsed() > DRAIN {
            let left = store.in_flight();
            std::mem::forget(store);
            panic!(
                "{left} requests did not end within {DRAIN:?}; the store is \
                 leaked and not dropped, since its drop would wait for them"
            );
        }
        store.wait(done, Duration::from_millis(10));
    }
    store
}

/// Let what the disk holds go and take every completion: done before a
/// test asserts, so that a failed assertion fails the test and does not
/// leave a thread held under a store that waits for it when dropped.
fn release_and_drain(
    disk: &SimDisk,
    store: InPlace<SimDisk>,
    done: &mut Vec<Completion>,
) -> InPlace<SimDisk> {
    disk.release_direct_io();
    drained(store, done)
}

/// Lets the disk's held direct I/O go when dropped. Declared after the
/// store it guards, it is dropped before it: a panic while a request is
/// held on a thread (an [`until_held`] that times out, a submit that
/// fails) lets the thread go before the store's drop waits for it, and
/// the test fails instead of hanging.
struct Releases<'a>(&'a SimDisk);

impl Drop for Releases<'_> {
    fn drop(&mut self) {
        self.0.release_direct_io();
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
/// through it, a write acknowledged before it was written. A failed
/// sync tried again shows only where a sync is
/// under way when the failure comes; the harness makes that so for a
/// third of the seeds (`harness::settles_at_failure`), so that the
/// defect is noticed in every run, also when this test runs alone
/// (M1.3ad: before, it was noticed by
/// `reopened_failed_volume_holds_what_was_flushed` only when the I/O
/// threads had written before the failure, which on an idle machine
/// they had not). (A
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
    let cases: [(u32, &[&str]); 5] = [
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
        // A write acknowledged before a thread has written it: only a
        // kill that drops what no thread has begun, as the harness's
        // does, shows it.
        (
            Mutants::ACK_BEFORE_WRITE,
            &["kill_keeps_acknowledged_operations"],
        ),
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

/// A volume on a simulated disk, served once: block 3 holds 0x33,
/// written by an instance that was lost before any flush covered it.
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
    let config = VolumeConfig {
        volume: *b"in-place volume.",
        size_mib: 1,
        base: Base::Empty,
        write_unit: u32::try_from(page).unwrap(),
    };
    create(&disk, &config).unwrap();
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, 3, 0x33).unwrap();
    drop(store);
    disk.kill();
    disk
}

/// Start on `disk`: the open under the lock, then the start.
fn start(disk: &SimDisk, mutants: u32, acknowledge: bool) -> Result<InPlace<SimDisk>, OpenError> {
    start_io(disk, mutants, acknowledge, IoMode::default())
}

fn start_io(
    disk: &SimDisk,
    mutants: u32,
    acknowledge: bool,
    io: IoMode,
) -> Result<InPlace<SimDisk>, OpenError> {
    Ok(opened(disk, acknowledge, io)?
        .start_with(None, Mutants(mutants))
        .expect("the in-place store"))
}

/// The first phase of a start alone.
fn opened(disk: &SimDisk, acknowledge: bool, io: IoMode) -> Result<Opened<SimDisk>, OpenError> {
    let options = OpenOptions {
        acknowledge_failure: acknowledge,
        io,
        ..OpenOptions::default()
    };
    Opened::open(disk.clone(), lock(disk)?, options).map_err(|(e, _)| e)
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

/// The start syncs the home image: what the instance before it wrote,
/// which no flush covered, is durable once the next start is done. A
/// power cut right after that start, which keeps nothing that was not
/// synced, takes nothing back. (The test of the same with the power
/// cut drawn from seeds, which notices a start that does not sync, is
/// `a_power_cut_after_a_start_keeps_what_the_instance_before_wrote`.)
#[test]
fn the_start_makes_what_the_instance_before_wrote_durable() {
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

/// Whether what an instance wrote by direct I/O and never flushed is
/// still there after the next start, a power cut drawn from `seed`
/// right after that start, and a third start. The writes reach the
/// simulated device and not its durable state: only a sync makes them
/// durable, and the start's own is the only one between them and the
/// cut. The start's check of the cache does not see them, as a
/// kernel's count of the page cache does not see a device's volatile
/// cache: without the sync, nothing else stands between them and the
/// cut.
fn kept_after_a_start_and_a_cut(seed: u64, mutants: u32) -> bool {
    let disk = created(seed, 0);
    let mut store = start(&disk, 0, false).unwrap();
    for block in 4..12 {
        write(&mut store, block, block as u8).unwrap();
    }
    drop(store);
    disk.kill();
    drop(start(&disk, mutants, false).unwrap());
    disk.power_cut(SeededCut::new(seed));
    let mut store = start(&disk, 0, false).unwrap();
    (4..12).all(|block| read(&mut store, block).unwrap() == vec![block as u8; BLOCK_USIZE])
        && read(&mut store, 3).unwrap() == vec![0x33; BLOCK_USIZE]
}

/// A power cut right after a start keeps what the instance before it
/// wrote and never flushed, whatever the cut keeps of what was not
/// synced: the start's sync made it durable (point 3 of the module
/// documentation). A start that does not sync (`START_UNSYNCED`) is
/// noticed: some cut takes a write back.
#[test]
fn a_power_cut_after_a_start_keeps_what_the_instance_before_wrote() {
    for seed in 1..=50 {
        assert!(kept_after_a_start_and_a_cut(seed, 0), "seed {seed}");
    }
    let lost = (1..=50)
        .filter(|&seed| !kept_after_a_start_and_a_cut(seed, Mutants::START_UNSYNCED))
        .count();
    assert!(lost > 0, "a start that does not sync is not noticed");
}

/// How write-zeroes reach the home image, as the status file shows it:
/// with direct I/O they are written as zeros by direct I/O, and once
/// one is applied the store says so, whatever it said before; through
/// the cache (test builds) they are deallocated. A write-zeroes that
/// left the state as it was showed "not yet" on a fresh volume, or the
/// log store's "deallocated" from the start's checkpoint, while zeros
/// were written. (Found by Codex on #58.)
#[test]
fn the_store_says_how_it_applies_write_zeroes() {
    for (io, applied) in [
        (IoMode::default(), Zeroes::WrittenDirect),
        (OTHER_IO[0], Zeroes::WrittenDirect),
        (OTHER_IO[1], Zeroes::WrittenDirect),
        (BUFFERED, Zeroes::Deallocated),
    ] {
        let disk = created(1, 0);
        let mut store = start_io(&disk, 0, false, io).unwrap();
        assert_eq!(store.zeroes(), Zeroes::Untried, "{io:?}");
        let done = run(
            &mut store,
            1,
            Op::WriteZeroes {
                offset: 3 * BLOCK,
                len: BLOCK,
            },
        );
        assert_eq!(done.result, Ok(()), "{io:?}");
        assert_eq!(store.zeroes(), applied, "{io:?}");
        assert_eq!(read(&mut store, 3).unwrap(), vec![0; BLOCK_USIZE]);
    }
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
/// was submitted before it (the stricter reading). With a write held on
/// the threads beside a read, the flush does not complete or begin its
/// sync until they are let go: one write among the requests on the
/// threads is enough (a rule that waited only when every request there
/// wrote would pass this flush). The mutant that lets a flush pass the
/// writes is noticed: the flush completes while the write is held.
#[test]
fn a_flush_waits_for_the_writes_on_the_threads() {
    for mutant in [0, Mutants::FLUSH_PASSES_WRITES] {
        let disk = created(3, 0);
        let mut store = start_io(&disk, mutant, false, IoMode::default()).unwrap();
        // A write and a read, held on the threads before they touch the
        // image.
        disk.hold_direct_io(2);
        let _release = Releases(&disk);
        store
            .submit(Request {
                id: RequestId(1),
                op: Op::Write {
                    offset: BLOCK,
                    data: Buf::filled(1, 1),
                },
            })
            .unwrap();
        store
            .submit(Request {
                id: RequestId(2),
                op: Op::Read {
                    offset: 2 * BLOCK,
                    buf: Buf::filled(1, 0),
                },
            })
            .unwrap();
        until_held(&disk);
        store
            .submit(Request {
                id: RequestId(3),
                op: Op::Flush,
            })
            .unwrap();
        let mut done = Vec::new();
        store.poll(&mut done);
        // With the writes still held: the flush waits; with the defect
        // it runs, and ends when its sync does.
        let (flushed, syncing) = if mutant == 0 {
            (done.iter().any(|c| c.id.0 == 3), store.syncing())
        } else {
            (until_done(&mut store, &mut done, 3), false)
        };
        let _store = release_and_drain(&disk, store, &mut done);
        if mutant == 0 {
            assert!(!flushed, "the flush did not wait for the writes");
            assert!(!syncing, "a sync began before the writes ended");
        } else {
            assert!(flushed, "the defect is not noticed");
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
        let _release = Releases(&disk);
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
        // With the first write still held: the two wait; with the defect
        // they run on the threads, and the read ends when a thread gets
        // to it.
        let (read_done, waiting) = if mutant == 0 {
            (done.iter().any(|c| c.id.0 == 3), store.io_queue().waiting)
        } else {
            (until_done(&mut store, &mut done, 3), 0)
        };
        let _store = release_and_drain(&disk, store, &mut done);
        if mutant == 0 {
            assert!(!read_done, "a request passed the write it overlaps");
            assert_eq!(waiting, 2, "the two wait");
        } else {
            assert!(read_done, "the defect is not noticed");
        }
        assert!(done.iter().all(|c| c.result.is_ok()));
    }
}

/// A write waits for an overlapping read on the threads: a read held on
/// the threads, then a write of the same block. The write waits (one
/// request waiting), so the read sees the block as it was before the
/// write, whatever the threads do. Nothing else is in the queue, so it
/// is this rule and not the order behind a waiting request that keeps
/// it.
#[test]
fn a_write_waits_for_an_overlapping_read_on_the_threads() {
    let disk = created(3, 0);
    let mut store = start_io(&disk, 0, false, IoMode::default()).unwrap();
    disk.hold_direct_io(1);
    let _release = Releases(&disk);
    store
        .submit(Request {
            id: RequestId(1),
            op: Op::Read {
                offset: 0,
                buf: Buf::filled(1, 0xee),
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
    let mut done = Vec::new();
    store.poll(&mut done);
    let waiting = store.io_queue().waiting;
    let store = release_and_drain(&disk, store, &mut done);
    assert_eq!(waiting, 1, "the write did not wait for the read");
    let read = done.iter().find(|c| c.id.0 == 1).expect("the read");
    let seen = read.buf.as_ref().expect("its buffer");
    assert!(seen.iter().all(|&b| b == 0), "the read saw the later write");
    assert!(done.iter().all(|c| c.result.is_ok()));
    drop(store);
}

/// A read waits for an overlapping write on the threads: a write held
/// on the threads, then a read of the same block with nothing in
/// between. The read waits (one request waiting) and then sees the
/// write.
#[test]
fn a_read_waits_for_an_overlapping_write_on_the_threads() {
    let disk = created(3, 0);
    let mut store = start_io(&disk, 0, false, IoMode::default()).unwrap();
    disk.hold_direct_io(1);
    let _release = Releases(&disk);
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
            op: Op::Read {
                offset: 0,
                buf: Buf::filled(1, 0),
            },
        })
        .unwrap();
    let mut done = Vec::new();
    store.poll(&mut done);
    let waiting = store.io_queue().waiting;
    let store = release_and_drain(&disk, store, &mut done);
    assert_eq!(waiting, 1, "the read did not wait for the write");
    let read = done.iter().find(|c| c.id.0 == 2).expect("the read");
    let seen = read.buf.as_ref().expect("its buffer");
    assert!(seen.iter().all(|&b| b == 0x11), "the read missed the write");
    assert!(done.iter().all(|c| c.result.is_ok()));
    drop(store);
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
    let store = drained(store, &mut done);
    assert!(store.is_failed(), "the panic did not fail the volume");
    assert!(
        done.iter().all(|c| c.result == Err(BlkError::Failed)),
        "{done:?}"
    );
    drop(store);
    disk.kill();
    assert!(
        matches!(start(&disk, 0, false), Err(OpenError::MarkedFailed)),
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
    let store = drained(store, &mut done);
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
    let _release = Releases(&disk);
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
    let held = store.io_queue();
    let mut done = Vec::new();
    let store = release_and_drain(&disk, store, &mut done);
    assert_eq!((held.in_flight, held.waiting), (1, 2), "{held:?}");
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
        Err(OpenError::MarkedFailed)
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
        Err(OpenError::MarkedFailed)
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
        Err(OpenError::MarkedFailed)
    ));
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

/// A start whose sync meets the failed disk gives `Failed`: the volume
/// has failed, and the marker is handed back unwritten, with the lock:
/// no marker yet, and no other start, until the caller finishes it. The
/// lock is released after the marker, so that while the marker's sync
/// is held no other start opens the volume. The next start finds the
/// volume marked as failed. The start makes one sync, so a fault on the
/// second lets it serve.
#[test]
fn a_start_that_meets_the_failed_disk_fails_the_volume() {
    let mut failing = 0;
    for nth in 0.. {
        let disk = created(12, 0);
        let opened = opened(&disk, false, IoMode::default()).unwrap();
        disk.fail_sync(nth, false);
        let start_failed = match opened.start(None) {
            Ok(_) => break,
            Err(StartError::Failed(start_failed)) => start_failed,
            Err(other) => panic!("sync {nth}: {other}"),
        };
        failing += 1;
        let marked = || disk.list().unwrap().iter().any(|n| n == MARKER_NAME);
        assert!(!marked(), "sync {nth}: the marker was not handed back");
        assert!(matches!(lock(&disk), Err(OpenError::Locked)));
        disk.hold_whole_syncs_of(MARKER_NAME);
        let finishing = std::thread::spawn(move || start_failed.finish());
        until_held(&disk);
        assert!(
            matches!(lock(&disk), Err(OpenError::Locked)),
            "sync {nth}: the volume was unlocked while its marker was being written"
        );
        disk.release_syncs();
        finishing.join().unwrap();
        assert!(marked(), "sync {nth}: no marker after the finish");
        disk.clear_faults();
        disk.kill();
        assert!(
            matches!(start(&disk, 0, false), Err(OpenError::MarkedFailed)),
            "sync {nth}"
        );
    }
    assert_eq!(failing, 1, "the start makes one sync");
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
    let refused = |disk: &SimDisk| match opened(disk, false, IoMode::default()).unwrap().start(None)
    {
        Err(StartError::Io(why)) => why,
        other => panic!("{other:?}"),
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
/// ([`create_zero_filled`]) has no such block.
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
/// 16 KiB: the open reads the write unit from the volume file and holds
/// the system's page to it.
#[test]
fn a_volume_of_small_pages_is_refused_on_large_pages() {
    let disk = created(5, 0);
    disk.set_page_size(16 << 10);
    disk.set_write_unit(16 << 10);
    let refused = match start(&disk, 0, false) {
        Err(error) => error,
        Ok(_) => panic!("a volume of 4 KiB pages opened on 16 KiB pages"),
    };
    assert!(
        matches!(
            refused,
            OpenError::Pages {
                page: 16384,
                unit: 4096
            }
        ),
        "{refused}"
    );
    // Where the page is 4 KiB again, it starts.
    disk.set_page_size(4096);
    disk.set_write_unit(4096);
    assert!(start(&disk, 0, false).is_ok());
}

/// The home image is held to the write unit for itself, and not only
/// through its directory: an image that lies on a file system of its
/// own (a mount point, a link elsewhere) whose blocks are larger than
/// the volume's write unit, or whose block size cannot be read, is
/// refused by the open with the reason naming the home image, while the
/// directory's blocks fit. Such a file system can rewrite a whole block
/// around a write, neighbours that a flush covered included, which is
/// what holding every file system under the volume to its write unit
/// rules out ([`OpenError::Blocks`]). The design names the volume's
/// write unit and is silent on a home image elsewhere than its
/// directory: the open's existing reading, a refusal, is what is held
/// here. The refusal writes nothing and fails nothing: no marker, and the volume
/// opens once the image is back where the blocks fit. Plants: the open
/// checks the directory only (`HOME_ANY_BLOCKS`), or nothing
/// (`ANY_BLOCKS`): the image is opened.
#[test]
fn an_open_refuses_a_home_image_on_a_file_system_with_larger_blocks() {
    let unit = 4096;
    let open = |disk: &SimDisk, mutants: u32| {
        Opened::open_with(
            disk.clone(),
            lock(disk).expect("free"),
            OpenOptions::default(),
            Mutants(mutants),
        )
        .map(|_| ())
        .map_err(|(e, _)| e)
    };
    for block in [Some(2 * unit), Some(1 << 20), None] {
        let disk = created(42, 0);
        disk.set_block_size_of(HOME_NAME, block)
            .expect("the home image");
        assert_eq!(Dir::block_size(&disk).unwrap(), unit, "the directory's fit");
        let (names, stats) = (disk.list().unwrap(), disk.stats());
        let refused = open(&disk, 0);
        match (block, &refused) {
            (
                Some(larger),
                Err(OpenError::Blocks {
                    what: Under::Home,
                    block,
                    unit: u,
                }),
            ) => assert_eq!((*block, *u), (larger, unit)),
            (
                None,
                Err(OpenError::BlocksUnknown {
                    what: Under::Home, ..
                }),
            ) => {}
            other => panic!("{block:?}: {other:?}"),
        }
        let said = refused.unwrap_err().to_string();
        assert!(said.contains("home image"), "{said}");
        assert_eq!(disk.list().unwrap(), names, "{block:?}");
        assert!(!names.iter().any(|name| name == MARKER_NAME));
        assert_eq!(
            (disk.stats().writes, disk.stats().syncs),
            (stats.writes, stats.syncs),
            "{block:?}: written"
        );
        for plant in [Mutants::HOME_ANY_BLOCKS, Mutants::ANY_BLOCKS] {
            assert!(
                open(&disk, plant).is_ok(),
                "{block:?}: the defect {plant:#x} is not noticed"
            );
        }
        disk.same_block_size_as_the_disk(HOME_NAME)
            .expect("the home image");
        assert!(start(&disk, 0, false).is_ok(), "{block:?}: not again");
    }
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
/// before the start (until M1.3t, the log store's checkpoint that
/// emptied its log into the image when a volume entered the in-place
/// store), or a file system that serves direct requests through its
/// cache (the drop stays for it).
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

/// Open and start the in-place store, as a daemon would.
fn enter(disk: &SimDisk, mutants: u32) -> Result<InPlace<SimDisk>, StartError> {
    opened(disk, false, IoMode::default())
        .unwrap()
        .start_with(None, Mutants(mutants))
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
        matches!(&refused, Err(StartError::CacheKept(why)) if why.contains("stay")),
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
        matches!(&refused, Err(StartError::CacheKept(why)) if why.contains("cachestat")),
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
        matches!(&refused, Err(StartError::CacheKept(why)) if why.contains("cannot be observed")),
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
///
/// The look now reads its block alone, with no read-ahead
/// (`a_start_right_after_a_start_serves`), and a look before the drop
/// would then leave nothing the drop cannot take. The order still
/// stands where a read alone reads ahead after all (the advice is
/// advisory, and a system without it reads as usual), so the look
/// before the drop is planted with the look that reads ahead.
#[test]
fn the_look_comes_after_the_drop() {
    let ahead = Mutants::LOOK_READS_AHEAD;
    for (mutants, serves) in [
        (0, true),
        (ahead, true),
        (ahead | Mutants::LOOK_BEFORE_DROP, false),
    ] {
        let disk = created(17, 0);
        let opened = opened(&disk, false, IoMode::default()).unwrap();
        disk.hold_read_ahead_of(HOME_NAME, 3);
        let entered = opened.start_with(None, Mutants(mutants));
        match entered {
            Ok(_) => assert!(serves, "{mutants:#x}: the start that looks first served"),
            Err(StartError::CacheKept(why)) if !serves => {
                assert!(why.contains("two drops"), "{why}");
            }
            Err(e) => panic!("{mutants:#x}: {e:?}"),
        }
    }
}

/// The look reads its block alone: it leaves no read-ahead under I/O,
/// which the next start's drop could not take, so a start right after
/// a start serves. A look that reads ahead is noticed: the second
/// start refuses.
#[test]
fn a_start_right_after_a_start_serves() {
    for (mutants, serves) in [(0, true), (Mutants::LOOK_READS_AHEAD, false)] {
        let disk = created(18, 0);
        disk.hold_read_ahead_of(HOME_NAME, 3);
        if let Err(e) = enter(&disk, mutants) {
            panic!("{mutants:#x}: the first start: {e:?}");
        }
        match enter(&disk, mutants) {
            Ok(_) => assert!(serves, "{mutants:#x}: the second start served"),
            Err(StartError::CacheKept(why)) if !serves => {
                assert!(why.contains("two drops"), "{why}");
            }
            Err(e) => panic!("{mutants:#x}: the second start: {e:?}"),
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
        matches!(&refused, Err(StartError::CacheKept(why)) if why.contains("two drops")),
        "{refused:?}"
    );
    drop(refused);
    disk.keep_cache_for(0);
    assert!(enter(&disk, 0).is_ok());
}

/// Mark the volume on `disk` as failed by hand, durably.
fn mark(disk: &SimDisk) {
    use crate::io::File as _;
    let mut marker = disk.create(MARKER_NAME).unwrap();
    marker.sync().unwrap();
    drop(marker);
    disk.sync().unwrap();
}

/// An acknowledged failure marker is removed, durably, before the open
/// returns: a power cut right after the open finds no marker. Without
/// the directory's sync after the remove, the cut brings the marker
/// back and the next start refuses the volume once more.
#[test]
fn an_acknowledged_marker_is_gone_for_good_once_the_open_returns() {
    let io = IoMode::default();
    let disk = created(5, 0);
    mark(&disk);
    assert!(matches!(
        opened(&disk, false, io),
        Err(OpenError::MarkedFailed)
    ));
    let open = opened(&disk, true, io).unwrap();
    assert!(open.acknowledged());
    drop(open);
    disk.power_cut(LoseAll);
    let open = opened(&disk, false, io).expect("the marker came back after the cut");
    assert!(!open.acknowledged());
}

/// A marker the open cannot remove durably refuses the start, which has
/// served nothing, and the volume stays marked: the remove is not on
/// the disk until the directory's sync, and that sync failed.
#[test]
fn a_marker_that_cannot_be_removed_refuses_the_start() {
    let io = IoMode::default();
    let disk = created(6, 0);
    mark(&disk);
    disk.fail_sync(0, true);
    match opened(&disk, true, io) {
        Err(OpenError::Io { during, .. }) => assert_eq!(during, "removing the failure marker"),
        other => panic!("{other:?}"),
    }
    disk.clear_faults();
    disk.power_cut(LoseAll);
    assert!(matches!(
        opened(&disk, false, io),
        Err(OpenError::MarkedFailed)
    ));
}

/// The open refuses, by reason, a volume whose home image is missing or
/// not of the volume file's size, and a directory without a volume file
/// or with the log store's superblock. None of these marks the volume.
#[test]
fn the_open_refuses_a_volume_that_does_not_hold_together() {
    use crate::io::File as _;
    use crate::volume::{VolumeError, LEGACY_NAME, VOLUME_NAME};
    let io = IoMode::default();
    let unmarked = |disk: &SimDisk, what: &str| {
        assert!(
            !disk.list().unwrap().iter().any(|name| name == MARKER_NAME),
            "{what}: the refusal marked the volume"
        );
    };
    let disk = created(7, 0);
    let mut home = disk.open(HOME_NAME).unwrap();
    let len = home.len().unwrap();
    home.extend(len + 4096).unwrap();
    drop(home);
    assert!(
        matches!(opened(&disk, false, io), Err(OpenError::Damaged(why)) if why.contains("size")),
        "a home image of another size"
    );
    unmarked(&disk, "a home image of another size");
    disk.remove(HOME_NAME).unwrap();
    assert!(
        matches!(opened(&disk, false, io), Err(OpenError::Damaged(why)) if why.contains("no home image")),
        "no home image"
    );
    unmarked(&disk, "no home image");
    let disk = created(8, 0);
    disk.remove(VOLUME_NAME).unwrap();
    assert!(matches!(
        opened(&disk, false, io),
        Err(OpenError::Volume(VolumeError::Missing))
    ));
    unmarked(&disk, "no volume file");
    drop(disk.create(LEGACY_NAME).unwrap());
    assert!(matches!(
        opened(&disk, false, io),
        Err(OpenError::Volume(VolumeError::Legacy))
    ));
    unmarked(&disk, "a log store's directory");
}

/// A cut inside a create leaves either a whole volume or none. The power
/// goes after each of the create's writes and syncs in turn, and
/// whatever the cut keeps of what was not synced, the directory holds
/// no volume file, and a create run again makes one, or the volume
/// opens with its base. Without the home image's sync before the volume
/// file is made, a cut can keep the volume file and not the base.
#[test]
fn a_cut_inside_a_create_leaves_a_whole_volume_or_none() {
    use crate::volume::{Volume, VolumeError};
    let config = VolumeConfig {
        volume: *b"in-place volume.",
        size_mib: 1,
        base: Base::Pattern(7),
        write_unit: 4096,
    };
    let mut base = vec![0u8; BLOCK_USIZE];
    config.base.fill_block(9, &mut base);
    let mut found = [0u32; 2];
    for calls in 0..10 {
        for seed in 0..64 {
            let disk = SimDisk::new(seed);
            disk.halt_after(calls);
            let created = create(&disk, &config);
            disk.power_cut(SeededCut::new(seed));
            let at = format!("after {calls} calls, seed {seed}");
            match Volume::read(&disk) {
                Ok(_) => {
                    let mut store = start(&disk, 0, false).unwrap_or_else(|e| panic!("{at}: {e}"));
                    assert_eq!(read(&mut store, 9).unwrap(), base, "{at}");
                    found[0] += 1;
                }
                Err(VolumeError::Missing) => {
                    assert!(created.is_err(), "{at}: lost");
                    create(&disk, &config).unwrap_or_else(|e| panic!("{at}: again: {e}"));
                    found[1] += 1;
                }
                Err(other) => panic!("{at}: {other}"),
            }
        }
    }
    // Both outcomes happen, so the cuts reach inside the create.
    assert!(found[0] > 0 && found[1] > 0, "{found:?}");
}

/// `create --zero-fill` writes its zeros inside the creation, into the
/// image under its creation's name, under the lock and before the
/// volume file: while they are written there is no volume to serve,
/// and a creation whose zeros fail leaves none. Without the zeros the
/// same refusing disk lets the creation through, since an empty base
/// writes nothing into the image.
#[test]
fn a_create_with_zeros_makes_the_volume_only_once_they_are_written() {
    use crate::store::HOME_NEW_NAME;
    use crate::volume::{Volume, VolumeError};
    let config = VolumeConfig {
        volume: *b"in-place volume.",
        size_mib: 1,
        base: Base::Empty,
        write_unit: 4096,
    };
    let disk = SimDisk::new(3);
    disk.fail_write_of(HOME_NEW_NAME);
    let refused = create_zero_filled(&disk, &config);
    assert!(matches!(&refused, Err(OpenError::Io { .. })), "{refused:?}");
    assert!(matches!(Volume::read(&disk), Err(VolumeError::Missing)));
    assert!(
        create(&disk, &config).is_ok(),
        "an empty base writes nothing"
    );
    let disk = SimDisk::new(4);
    create_zero_filled(&disk, &config).unwrap();
    let mut store = start(&disk, 0, false).unwrap();
    assert_eq!(read(&mut store, 9).unwrap(), vec![0u8; BLOCK_USIZE]);
}

mod ported;
