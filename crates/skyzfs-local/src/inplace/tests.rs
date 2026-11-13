use std::time::Duration;

use skyzfs_blk::conformance::{run_all, Config};
use skyzfs_blk::{
    Base, BlkError, BlockTarget, Buf, Completion, Op, Request, RequestId, BLOCK_USIZE,
};

use super::harness::{run_suite, InPlaceHarness, OnSim, NOT_MET, NOT_MET_REASON};
use super::*;
use crate::format::Key;
use crate::log::LogError;
use crate::sim::{SeededCut, SimDisk};
use crate::store::{OpenOptions, Store, StoreConfig, StoreError};

const BLOCK: u64 = 4096;
/// What a test gives a wait: it waits for a sync to end, not for the
/// time (see `InPlace::wait`).
const WAIT: Duration = Duration::from_secs(60);

/// The cases of the suite that need a power cut (its table "Needs P").
const NEED_A_POWER_CUT: [&str; 6] = [
    "flushed_operations_survive_power_cut",
    "flush_covers_what_was_acknowledged_before_it",
    "flush_covers_an_earlier_instance",
    "power_cut_at_every_point_leaves_a_prefix",
    "recovered_state_is_durable",
    "first_request_after_power_cut_may_be_a_write",
];

fn harnesses(mutants: u32) -> Vec<InPlaceHarness> {
    [(1, 0), (16, 0), (16, 3)]
        .into_iter()
        .map(|(queue_depth, sync_delay)| InPlaceHarness {
            queue_depth,
            sync_delay,
            mutants,
        })
        .collect()
}

/// Acceptance 1: the suite passes on the in-place store on the
/// simulated disk, without the power cut, with one request at a time
/// and with sixteen, and with syncs that end up to three looks after
/// they begin, so that writes and flushes arrive while a sync is under
/// way. Exactly the cases that need a power cut are not exercised, and
/// for that reason.
///
/// Two failure cases are not met by this store, by design (the
/// harness's `NOT_MET`): they are reported as not exercised, with that
/// reason, and
/// `the_cases_not_met_fail_on_torn_blocks_and_nothing_else_does` pins
/// why.
#[test]
fn the_in_place_store_passes_the_conformance_suite_without_a_power_cut() {
    let config = Config {
        cases: 32,
        ..Config::default()
    };
    for mut harness in harnesses(0) {
        let report = run_suite(&mut harness, &config);
        assert!(report.passed(), "in {harness:?}:\n{report}");
        let idle = report.not_exercised();
        for case in NEED_A_POWER_CUT.iter().chain(&NOT_MET) {
            assert!(idle.contains(case), "{case} ran in {harness:?}");
        }
        for (case, why) in report.unexercised() {
            let one_at_a_time = harness.queue_depth == 1 && why.contains("one request at a time");
            let cut = NEED_A_POWER_CUT.contains(&case) && why.contains("power");
            let not_met = NOT_MET.contains(&case) && why == NOT_MET_REASON;
            assert!(cut || not_met || one_at_a_time, "{case}: {why}");
        }
        for (case, what) in report.weaker() {
            assert!(what.contains("power"), "{case}: {what}");
        }
    }
}

/// Why the cases of `NOT_MET` are not met: run as the suite has them,
/// with the log failing by the simulated disk's faults, exactly those
/// two fail, each with a block the suite's model reports as torn (the
/// test checks that, not which block). That is the module's point 4 (a failure leaves unflushed
/// blocks as a power cut does).
#[test]
fn the_cases_not_met_fail_on_torn_blocks_and_nothing_else_does() {
    let config = Config {
        cases: 32,
        ..Config::default()
    };
    for mut harness in harnesses(0) {
        let report = run_all(&mut harness, &config);
        let mut failed = report.failed();
        failed.sort_unstable();
        assert_eq!(failed, NOT_MET, "in {harness:?}:\n{report}");
        for outcome in &report.outcomes {
            if let Err(failure) = &outcome.result {
                assert!(failure.0.contains("is Torn"), "{}: {failure}", outcome.name);
            }
        }
    }
}

/// The suite notices a failed sync that is tried again. (A failed write
/// that does not fail the volume is caught by
/// `a_failed_write_fails_the_volume`: the suite's cases that catch it
/// are the two this store does not meet.)
#[test]
fn the_suite_notices_a_failure_that_is_not_final() {
    let config = Config {
        cases: 32,
        ..Config::default()
    };
    let mut harness = harnesses(Mutants::SYNC_RETRIED)[1];
    let report = run_suite(&mut harness, &config);
    assert!(!report.passed(), "the defect is not noticed");
}

/// A store on a simulated disk, with a log of a few records that the
/// start applies.
fn created(seed: u64, sync_delay: u64) -> SimDisk {
    let disk = SimDisk::new(seed);
    disk.set_write_unit(4096);
    disk.set_sync_delay(sync_delay);
    let config = StoreConfig {
        volume: *b"in-place volume.",
        size_mib: 1,
        base: Base::Empty,
        align: 64,
        segment_size: (1 << 20) + (128 << 10),
        write_unit: 4096,
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
    let options = OpenOptions {
        acknowledge_failure: acknowledge,
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
/// which begins when this one ends and covers it.
#[test]
fn a_flush_joins_a_sync_under_way_only_when_nothing_was_written_since_it_began() {
    let disk = created(3, 0);
    let mut store = start(&disk, 0, false).unwrap();
    disk.hold_syncs_of(crate::store::HOME_NAME);
    let submit = |store: &mut InPlace<SimDisk>, id: u64, op: Op| {
        store
            .submit(Request {
                id: RequestId(id),
                op,
            })
            .unwrap()
    };
    let syncs = store.counts().syncs;
    submit(
        &mut store,
        1,
        Op::Write {
            offset: 0,
            data: Buf::filled(1, 1),
        },
    );
    submit(&mut store, 2, Op::Flush);
    assert!(store.syncing());
    submit(&mut store, 3, Op::Flush);
    assert_eq!(store.counts().joined, 1);
    submit(
        &mut store,
        4,
        Op::Write {
            offset: 0,
            data: Buf::filled(1, 2),
        },
    );
    submit(&mut store, 5, Op::Flush);
    assert_eq!(store.counts().joined, 1, "a flush behind a write joined");
    let mut done = Vec::new();
    store.poll(&mut done);
    let ids: Vec<u64> = done.iter().map(|c| c.id.0).collect();
    assert_eq!(ids, [1, 4], "only the writes are done");
    disk.release_syncs();
    while store.in_flight() > 0 {
        store.wait(&mut done, Duration::ZERO);
    }
    assert!(done.iter().all(|c| c.result.is_ok()));
    assert_eq!(store.counts().syncs, syncs + 2);
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
    assert_eq!(
        store.into_in_place(None).err(),
        Some(IntoInPlaceError::InFlight)
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
    let garbage = ending == Ending::Cut { garbage: true };
    // Where a failure is set up, and whether it has been.
    let fail_at = match ending {
        Ending::Failure { .. } => Some(seed as usize % 30),
        Ending::Cut { .. } => None,
    };
    let mut failing = false;
    let disk = created(seed, 3);
    let mut store = start(&disk, mutants, false).map_err(|e| e.to_string())?;
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
                store = start(&disk, mutants, false).map_err(|e| e.to_string())?;
                for block in &mut blocks {
                    block.floor = block.newest();
                }
            }
        }
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
    let mut store = start(&disk, 0, acknowledge).map_err(|e| e.to_string())?;
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
