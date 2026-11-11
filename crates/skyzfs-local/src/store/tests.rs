//! Scenario tests of the store on the simulated disk.
//!
//! Each scenario runs in every environment: both record alignments,
//! write units of 4 KiB and 16 KiB (the disk then rewrites 16 KiB around
//! every write), and lost sectors that read as their old content or as
//! garbage. `caught` also runs it with one defence of the store taken
//! out, and expects it to fail then.

use std::time::Duration;

use skyzfs_blk::{Base, BlkError, BlockTarget, Buf, Completion, Op, Request, RequestId};

use super::*;
use crate::io::SECTOR;
use crate::sim::{BySector, Fate, SimDisk};

type SimStore = Store<SimDisk>;
type Outcome = Result<(), String>;

const BLOCK: u64 = BLOCK_SIZE;
/// A segment that takes the largest write, at either alignment and
/// write unit.
const SEGMENT: u64 = (1 << 20) + (128 << 10);

#[derive(Debug, Clone, Copy)]
struct Env {
    mutants: u32,
    align: u32,
    unit: u32,
    garbage: bool,
    base: Base,
    segments: Option<usize>,
}

impl Env {
    fn config(&self) -> StoreConfig {
        StoreConfig {
            volume: *b"a volume to test",
            size_mib: 4,
            base: self.base,
            align: self.align,
            segment_size: SEGMENT,
            write_unit: self.unit,
        }
    }

    fn options(&self) -> OpenOptions {
        OpenOptions {
            queue_depth: 16,
            log_segments: self.segments,
            mutants: Mutants(self.mutants),
            ..OpenOptions::default()
        }
    }

    /// A new store on a new disk.
    fn fresh(&self) -> Result<(SimDisk, SimStore), String> {
        let disk = SimDisk::new(7);
        disk.set_write_unit(u64::from(self.unit));
        let key = Key(*b"a key for tests!");
        let store = Store::create_keyed(disk.clone(), &self.config(), key, self.options())
            .map_err(|e| e.to_string())?;
        Ok((disk, store))
    }

    /// The process is lost and the store opened again.
    fn reopen(&self, disk: &SimDisk) -> Result<SimStore, String> {
        disk.kill();
        Store::open(disk.clone(), self.options())
            .map(|(store, _)| store)
            .map_err(|e| e.to_string())
    }

    /// The same, acknowledging a failure.
    fn reopen_ack(&self, disk: &SimDisk) -> Result<SimStore, String> {
        disk.kill();
        let options = OpenOptions {
            acknowledge_failure: true,
            ..self.options()
        };
        Store::open(disk.clone(), options)
            .map(|(store, _)| store)
            .map_err(|e| e.to_string())
    }

    /// What a lost sector reads as in this environment.
    fn lost(&self) -> Fate {
        if self.garbage {
            Fate::Garbage
        } else {
            Fate::Durable
        }
    }

    /// What block `block` holds before anything is written.
    fn base_block(&self, block: u64) -> Vec<u8> {
        let mut out = vec![0u8; BLOCK as usize];
        self.base.fill_block(block, &mut out);
        out
    }
}

fn environments(mutants: u32) -> Vec<Env> {
    let mut out = Vec::new();
    for align in [64, 4096] {
        for unit in [4096, 16384] {
            for garbage in [false, true] {
                out.push(Env {
                    mutants,
                    align,
                    unit,
                    garbage,
                    base: Base::Empty,
                    segments: None,
                });
            }
        }
    }
    out
}

/// The scenario holds in every environment.
fn holds(scenario: fn(&Env) -> Outcome) {
    for env in environments(0) {
        if let Err(why) = scenario(&env) {
            panic!("the store broke its promise in {env:?}: {why}");
        }
    }
}

/// The scenario holds for the store as it is, and fails in every
/// environment for the store with `mutants`.
fn caught(scenario: fn(&Env) -> Outcome, mutants: u32) {
    caught_where(scenario, mutants, |_| true);
}

/// The same, where the defect only has something to act on in the
/// environments that `where_` selects.
fn caught_where(scenario: fn(&Env) -> Outcome, mutants: u32, where_: fn(&Env) -> bool) {
    holds(scenario);
    let mut ran = 0;
    for env in environments(mutants).into_iter().filter(where_) {
        ran += 1;
        if scenario(&env).is_ok() {
            panic!("the defect {mutants:#x} was not noticed in {env:?}");
        }
    }
    assert!(ran > 0);
}

/// Where a write rewrites more than its own block, and what it loses
/// reads as garbage.
fn garbled_neighbours(env: &Env) -> bool {
    env.unit > 4096 && env.garbage
}

/// Submit one request and take its completion.
fn run(store: &mut SimStore, op: Op) -> Result<Completion, String> {
    let id = RequestId(1);
    store
        .submit(Request { id, op })
        .map_err(|e| e.to_string())?;
    let mut done = Vec::new();
    store.wait(&mut done, Duration::ZERO);
    match done.pop() {
        Some(completion) if done.is_empty() && completion.id == id => Ok(completion),
        other => Err(format!(
            "one request gave {} completions",
            done.len() + usize::from(other.is_some())
        )),
    }
}

/// Run a request that must succeed.
fn ok(store: &mut SimStore, op: Op) -> Result<Option<Buf>, String> {
    let name = op.name();
    let done = run(store, op)?;
    done.result.map_err(|e| format!("{name}: {e}"))?;
    Ok(done.buf)
}

/// Write `count` blocks of `byte` at `block`.
fn write(store: &mut SimStore, block: u64, count: usize, byte: u8) -> Outcome {
    let op = Op::Write {
        offset: block * BLOCK,
        data: Buf::filled(count, byte),
    };
    ok(store, op).map(|_| ())
}

fn flush(store: &mut SimStore) -> Outcome {
    ok(store, Op::Flush).map(|_| ())
}

/// Read `count` blocks at `block`.
fn read(store: &mut SimStore, block: u64, count: usize) -> Result<Vec<u8>, String> {
    let op = Op::Read {
        offset: block * BLOCK,
        buf: Buf::filled(count, 0xEE),
    };
    match ok(store, op)? {
        Some(buf) => Ok(buf.to_vec()),
        None => Err("a read came back without its buffer".into()),
    }
}

/// The blocks hold `byte` throughout.
fn expect(store: &mut SimStore, block: u64, count: usize, byte: u8, what: &str) -> Outcome {
    let got = read(store, block, count)?;
    match got.iter().position(|b| *b != byte) {
        None => Ok(()),
        Some(at) => Err(format!(
            "{what}: block {} holds {:#04x} at byte {}, not {byte:#04x}",
            block + at as u64 / BLOCK,
            got[at],
            at as u64 % BLOCK
        )),
    }
}

fn checkpoint(store: &mut SimStore) -> Result<Lsn, String> {
    store.checkpoint().map_err(|e| format!("checkpoint: {e}"))
}

// ---------------------------------------------------------------------
// The nine defences.

/// A read takes the newest version of a block from the log. A write
/// that was acknowledged is seen by the next read, long before any
/// checkpoint brings it to the home image, and a later write to the
/// same block replaces it. Mutant: a read that takes every block from
/// the home image.
#[test]
fn a_read_takes_the_newest_version_from_the_log() {
    fn scenario(env: &Env) -> Outcome {
        let (_disk, mut store) = env.fresh()?;
        write(&mut store, 3, 4, 1)?;
        expect(&mut store, 3, 4, 1, "after the write")?;
        write(&mut store, 4, 2, 2)?;
        expect(&mut store, 3, 1, 1, "in front of the second write")?;
        expect(&mut store, 4, 2, 2, "the second write")?;
        expect(&mut store, 6, 1, 1, "behind the second write")?;
        expect(&mut store, 7, 1, 0, "beside both")
    }
    caught(scenario, Mutants::READ_HOME_ONLY);
}

/// An acknowledged write survives the process. The write is appended
/// to the log before it is acknowledged, so a store that is killed
/// right after the acknowledgement, with nothing synced, holds the
/// write when it is opened again. Mutant: a write acknowledged before
/// its append.
#[test]
fn an_acknowledged_write_survives_the_process() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 0, 2, 1)?;
        write(&mut store, 9, 3, 2)?;
        drop(store);
        let mut store = env.reopen(&disk)?;
        expect(&mut store, 0, 2, 1, "the first write after a kill")?;
        expect(&mut store, 9, 3, 2, "the last write after a kill")
    }
    caught(scenario, Mutants::ACK_BEFORE_APPEND);
}

/// An open rebuilds the index. What the log holds and no checkpoint has
/// applied is read from the log again after a reopen, with the newest
/// version winning. Mutant: an open that leaves the index empty.
#[test]
fn an_open_rebuilds_the_index_from_the_log() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 1, 4, 1)?;
        write(&mut store, 2, 1, 2)?;
        ok(
            &mut store,
            Op::WriteZeroes {
                offset: 4 * BLOCK,
                len: BLOCK,
            },
        )?;
        flush(&mut store)?;
        drop(store);
        let mut store = env.reopen(&disk)?;
        expect(&mut store, 1, 1, 1, "the older write")?;
        expect(&mut store, 2, 1, 2, "the newer write")?;
        expect(&mut store, 3, 1, 1, "the older write again")?;
        expect(&mut store, 4, 1, 0, "the zeros")
    }
    caught(scenario, Mutants::NO_INDEX_REBUILD);
}

/// A completed flush is durable. Everything acknowledged before the
/// flush is there after a power cut that loses every unsynced sector.
/// Mutant: a flush acknowledged without a sync.
#[test]
fn a_flush_makes_what_was_acknowledged_durable() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 0, 8, 1)?;
        write(&mut store, 4, 2, 2)?;
        flush(&mut store)?;
        drop(store);
        let lost = env.lost();
        disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        let mut store = env.reopen(&disk)?;
        expect(&mut store, 0, 4, 1, "the first write")?;
        expect(&mut store, 4, 2, 2, "the second write")?;
        expect(&mut store, 6, 2, 1, "the first write again")
    }
    caught(scenario, Mutants::FLUSH_NO_SYNC);
}

/// A failed volume serves nothing. After a write has failed on the
/// store's own disk, every later request fails, reads and flushes
/// included: also a read of a block that only the home image holds,
/// which the failed log has no part in. A store whose log is marked
/// failed does not open unless the failure is acknowledged. Mutant: a
/// store that goes on serving.
#[test]
fn a_failed_volume_serves_nothing() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 0, 1, 1)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        disk.fail_write(0, false);
        let failing = Op::Write {
            offset: 0,
            data: Buf::filled(1, 2),
        };
        if run(&mut store, failing)?.result != Err(BlkError::Failed) {
            return Err("the failing write did not report a failed volume".into());
        }
        let requests = [
            Op::Read {
                offset: 0,
                buf: Buf::zeroed(1),
            },
            Op::Flush,
            Op::Write {
                offset: BLOCK,
                data: Buf::filled(1, 3),
            },
            Op::Discard {
                offset: 0,
                len: BLOCK,
            },
        ];
        for op in requests {
            let name = op.name();
            if run(&mut store, op)?.result != Err(BlkError::Failed) {
                return Err(format!("a {name} was served on a failed volume"));
            }
        }
        if store.checkpoint().is_ok() || !store.is_failed() {
            return Err("a failed volume took a checkpoint".into());
        }
        drop(store);
        disk.kill();
        match Store::open(disk.clone(), env.options()) {
            Err(StoreError::Log(LogError::MarkedFailed)) => {}
            Err(other) => return Err(other.to_string()),
            Ok(_) => return Err("a failed store opened unacknowledged".into()),
        }
        let mut store = env.reopen_ack(&disk)?;
        expect(&mut store, 0, 1, 1, "what the flush covered")
    }
    caught(scenario, Mutants::SERVE_AFTER_FAILURE);
}

/// A checkpoint stops at the hold. Nothing at or above the hold is
/// given up, however much of the log is durable; with the hold released
/// the next checkpoint takes the rest. Mutant: a checkpoint that
/// ignores the hold.
#[test]
fn a_checkpoint_stops_at_the_hold() {
    fn scenario(env: &Env) -> Outcome {
        let (_disk, mut store) = env.fresh()?;
        write(&mut store, 0, 2, 1)?;
        let hold = store.position();
        store.set_hold(Some(hold));
        write(&mut store, 1, 2, 2)?;
        flush(&mut store)?;
        let start = checkpoint(&mut store)?;
        if start > hold || store.log_start() != start {
            return Err(format!(
                "the log starts at {start}, beyond the hold at {hold}"
            ));
        }
        if store.hold() != Some(hold) {
            return Err("the checkpoint changed the hold".into());
        }
        expect(&mut store, 0, 1, 1, "below the hold")?;
        expect(&mut store, 1, 2, 2, "above the hold")?;
        store.set_hold(None);
        if checkpoint(&mut store)? != store.position() {
            return Err("without a hold the checkpoint did not take the whole log".into());
        }
        expect(&mut store, 0, 1, 1, "after the second checkpoint")?;
        expect(&mut store, 1, 2, 2, "after the second checkpoint")
    }
    caught(scenario, Mutants::CHECKPOINT_PAST_HOLD);
}

/// The index lets go of a record only when the home image holds it. A
/// checkpoint that stops at a hold has applied the records below the
/// hold and no others; a block whose newest version lies above the
/// hold is still read from the log. Mutant: an index that lets go of
/// every durable record.
#[test]
fn the_index_lets_go_only_of_what_was_applied() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 5, 2, 1)?;
        store.set_hold(Some(store.position()));
        write(&mut store, 5, 1, 2)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        expect(&mut store, 5, 1, 2, "the newer version, above the hold")?;
        expect(&mut store, 6, 1, 1, "the older version, applied")?;
        drop(store);
        let mut store = env.reopen(&disk)?;
        expect(&mut store, 5, 1, 2, "after a reopen")?;
        expect(&mut store, 6, 1, 1, "after a reopen")
    }
    caught(scenario, Mutants::INDEX_SWITCH_EARLY);
}

/// The log is given up only after the home image is durable. A
/// checkpoint that cannot sync the home image fails the volume and has
/// not given up the log; one that gave it up first would have left the
/// records in neither place. After a power cut that loses everything
/// unsynced, what a flush covered must be there. Mutant: a checkpoint that gives up the
/// log first.
#[test]
fn the_log_is_given_up_only_after_the_home_image_is_durable() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 2, 4, 1)?;
        flush(&mut store)?;
        // The home image cannot be synced.
        disk.fail_sync_of(HOME_NAME);
        if store.checkpoint().is_ok() {
            return Err("the checkpoint did not notice the failed sync".into());
        }
        drop(store);
        let lost = env.lost();
        disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        let mut store = env.reopen_ack(&disk)?;
        expect(&mut store, 2, 4, 1, "what the flush covered")
    }
    caught(scenario, Mutants::RECLAIM_BEFORE_HOME_SYNC);
}

/// A checkpoint applies only durable records. A record that is not
/// synced can be lost to a power cut, and if a checkpoint had written
/// it into the home image, the volume would hold an operation, or half
/// of one, that the log never had. Here the first write is flushed and
/// checkpointed, so that only the home image holds it; the second is
/// not flushed. A checkpoint now has nothing to do. One that applied
/// the second write all the same finds its sync failing, so that what
/// it wrote into the home image is at the power cut's mercy: every
/// other sector survives. The blocks must hold the flushed write,
/// whole. Mutant: a checkpoint up to the end of the log.
#[test]
fn a_checkpoint_applies_only_durable_records() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 8, 4, 1)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        write(&mut store, 8, 4, 2)?;
        disk.fail_sync_of(HOME_NAME);
        // Nothing is durable beyond the log's start: nothing to apply,
        // nothing to sync, no error.
        let correct = env.mutants == 0;
        if correct && store.checkpoint() != Ok(store.log_start()) {
            return Err("a checkpoint with nothing durable to apply did something".into());
        }
        let _ = store.checkpoint();
        drop(store);
        disk.power_cut(BySector(|name: Option<&str>, index| {
            if name == Some(HOME_NAME) && index % 2 == 0 {
                Fate::Latest
            } else {
                Fate::Durable
            }
        }));
        let mut store = env.reopen_ack(&disk)?;
        let got = read(&mut store, 8, 4)?;
        if got.iter().all(|b| *b == 1) {
            Ok(())
        } else {
            let sectors: Vec<u8> = got.chunks(SECTOR as usize).map(|s| s[0]).collect();
            Err(format!("the blocks hold, sector by sector, {sectors:?}"))
        }
    }
    caught(scenario, Mutants::CHECKPOINT_UNSYNCED);
}

/// A checkpoint puts nothing at risk but what it writes. Writing one
/// block of the home image rewrites the whole write unit around it,
/// and a power cut can leave any rewritten sector as garbage. The
/// neighbours of the block may be held by the home image alone. Here
/// four blocks are flushed and checkpointed, then one of them is
/// written again and flushed, and the checkpoint that applies it is
/// cut off before its sync. All four must still read as what was
/// flushed: the checkpoint saved the neighbours first, durably, and the
/// open writes them back. Mutants: a checkpoint without preimages, and
/// one whose preimages are not synced before the home image is written.
#[test]
fn a_checkpoint_puts_only_its_own_blocks_at_risk() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 4, 4, 1)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        write(&mut store, 5, 1, 2)?;
        flush(&mut store)?;
        disk.fail_sync_of(HOME_NAME);
        if store.checkpoint().is_ok() {
            return Err("the checkpoint did not notice the failed sync".into());
        }
        drop(store);
        let lost = env.lost();
        disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        let mut store = env.reopen_ack(&disk)?;
        expect(&mut store, 4, 1, 1, "the neighbour in front")?;
        expect(&mut store, 5, 1, 2, "the block that was written")?;
        expect(&mut store, 6, 2, 1, "the neighbours behind")
    }
    caught_where(scenario, Mutants::NO_PREIMAGES, garbled_neighbours);
    caught_where(scenario, Mutants::PREIMAGES_UNSYNCED, garbled_neighbours);
}

// ---------------------------------------------------------------------
// The conformance suite.

use super::harness::SimHarness;
use skyzfs_blk::conformance::{run_all, Config};

fn harnesses(mutants: u32) -> Vec<SimHarness> {
    let mut out = Vec::new();
    for align in [64, 4096] {
        for unit in [4096, 16384] {
            for garbage in [false, true] {
                out.push(SimHarness {
                    align,
                    unit,
                    garbage,
                    queue_depth: 16,
                    segments: Some(3),
                    mutants,
                });
            }
        }
    }
    out
}

/// The contract, rules R1 to R12: the conformance suite of `skyzfs-blk`
/// passes against the store on the simulated disk in every environment.
#[test]
fn the_store_passes_the_conformance_suite() {
    let config = Config {
        cases: 48,
        ..Config::default()
    };
    for mut harness in harnesses(0) {
        let report = run_all(&mut harness, &config);
        assert!(report.passed(), "in {harness:?}:\n{report}");
        assert!(
            report.not_exercised().is_empty(),
            "in {harness:?} not exercised: {:?}",
            report.not_exercised()
        );
    }
}
