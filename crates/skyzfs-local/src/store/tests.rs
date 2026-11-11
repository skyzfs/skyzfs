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
// The operations.

/// The blocks hold what `want` gives for each.
fn expect_blocks(store: &mut SimStore, block: u64, want: &[Vec<u8>], what: &str) -> Outcome {
    let got = read(store, block, want.len())?;
    for (i, (got, want)) in got.chunks(BLOCK as usize).zip(want).enumerate() {
        if got != want.as_slice() {
            return Err(format!(
                "{what}: block {} starts with {:#04x}, not {:#04x}",
                block + i as u64,
                got[0],
                want[0]
            ));
        }
    }
    Ok(())
}

/// Discard and write-zeroes, on an empty volume and on one with a base.
/// A discard leaves every block as it was: written blocks keep their
/// data and unwritten ones the base, and a write after the discard is
/// not touched by it. A write-zeroes reads as zeros and hides the base.
/// Both stay so through a checkpoint, a reopen and a power cut after a
/// flush.
#[test]
fn a_discard_leaves_the_blocks_and_write_zeroes_zeroes_them() {
    fn scenario(env: &Env) -> Outcome {
        for base in [Base::Empty, Base::Pattern(0xBA5E)] {
            let env = Env { base, ..*env };
            let (disk, mut store) = env.fresh()?;
            let zeros = vec![0u8; BLOCK as usize];
            let ones = vec![1u8; BLOCK as usize];
            let twos = vec![2u8; BLOCK as usize];
            write(&mut store, 2, 2, 1)?;
            ok(
                &mut store,
                Op::Discard {
                    offset: 2 * BLOCK,
                    len: 4 * BLOCK,
                },
            )?;
            write(&mut store, 5, 1, 2)?;
            ok(
                &mut store,
                Op::WriteZeroes {
                    offset: 3 * BLOCK,
                    len: 2 * BLOCK,
                },
            )?;
            // Blocks 1 to 6: base, written, zeroed, zeroed, written
            // after the discard, base.
            let want = [
                env.base_block(1),
                ones.clone(),
                zeros.clone(),
                zeros.clone(),
                twos.clone(),
                env.base_block(6),
            ];
            expect_blocks(&mut store, 1, &want, "at once")?;
            flush(&mut store)?;
            checkpoint(&mut store)?;
            expect_blocks(&mut store, 1, &want, "after a checkpoint")?;
            drop(store);
            let mut store = env.reopen(&disk)?;
            expect_blocks(&mut store, 1, &want, "after a reopen")?;
            // A discard of what only the home image holds.
            ok(
                &mut store,
                Op::Discard {
                    offset: 0,
                    len: 8 * BLOCK,
                },
            )?;
            flush(&mut store)?;
            drop(store);
            let lost = env.lost();
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            let mut store = env.reopen(&disk)?;
            expect_blocks(&mut store, 1, &want, "after a discard and a power cut")?;
        }
        Ok(())
    }
    holds(scenario);
}

/// A read-only volume: reads are served and a flush succeeds; a write,
/// a write-zeroes and a discard are refused and change nothing.
#[test]
fn a_read_only_volume_refuses_changes() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 1, 1, 1)?;
        drop(store);
        disk.kill();
        let options = OpenOptions {
            read_only: true,
            ..env.options()
        };
        let (mut store, _) = Store::open(disk.clone(), options).map_err(|e| e.to_string())?;
        if !store.geometry().read_only() {
            return Err("the geometry does not say read-only".into());
        }
        let refused = [
            Op::Write {
                offset: BLOCK,
                data: Buf::filled(1, 2),
            },
            Op::WriteZeroes {
                offset: BLOCK,
                len: BLOCK,
            },
            Op::Discard {
                offset: BLOCK,
                len: BLOCK,
            },
        ];
        for op in refused {
            let name = op.name();
            if run(&mut store, op)?.result != Err(BlkError::ReadOnly) {
                return Err(format!("a {name} was not refused"));
            }
        }
        flush(&mut store)?;
        expect(&mut store, 1, 1, 1, "on the read-only volume")
    }
    holds(scenario);
}

/// A volume above 4 GiB: offsets beyond the 32-bit boundary are written
/// and read where they belong, in the log, in the home image after a
/// checkpoint, and after a reopen; the blocks at the same offsets
/// modulo 4 GiB are untouched.
#[test]
fn a_volume_above_four_gib() {
    fn scenario(env: &Env) -> Outcome {
        let disk = SimDisk::new(1);
        disk.set_write_unit(u64::from(env.unit));
        let config = StoreConfig {
            size_mib: 5 * 1024,
            ..env.config()
        };
        let key = Key(*b"a key for tests!");
        let mut store = Store::create_keyed(disk.clone(), &config, key, env.options())
            .map_err(|e| e.to_string())?;
        let four_gib = (4u64 << 30) / BLOCK;
        let last = store.geometry().blocks() - 1;
        if last < four_gib {
            return Err("the volume is not above 4 GiB".into());
        }
        write(&mut store, four_gib + 3, 2, 1)?;
        write(&mut store, last, 1, 2)?;
        ok(
            &mut store,
            Op::WriteZeroes {
                offset: (four_gib - 1) * BLOCK,
                len: 2 * BLOCK,
            },
        )?;
        for round in ["at once", "after a checkpoint", "after a reopen"] {
            expect(&mut store, four_gib + 3, 2, 1, round)?;
            expect(&mut store, last, 1, 2, round)?;
            expect(&mut store, 3, 2, 0, round)?;
            expect(&mut store, last - four_gib, 1, 0, round)?;
            match round {
                "at once" => {
                    flush(&mut store)?;
                    checkpoint(&mut store)?;
                }
                "after a checkpoint" => {
                    drop(store);
                    store = env.reopen(&disk)?;
                }
                _ => {}
            }
        }
        let beyond = Op::Write {
            offset: (last + 1) * BLOCK,
            data: Buf::filled(1, 3),
        };
        match run(&mut store, beyond)?.result {
            Err(BlkError::OutOfBounds { .. }) => Ok(()),
            other => Err(format!("a write beyond the end gave {other:?}")),
        }
    }
    holds(scenario);
}

/// A megabyte, the largest write: `count` of them at megabyte `at`.
fn megabyte(store: &mut SimStore, at: u64, byte: u8) -> Outcome {
    write(store, at * 256, 256, byte)
}

/// A full log. With a limit on the log's segment files, a write that
/// finds the log full makes the store checkpoint and carry on: nothing
/// is refused and the log gets no more files. While a hold keeps the
/// log from being given up, such a write is held, and so is everything
/// submitted behind it, a read included; when the hold is released they
/// complete, in order, once each.
#[test]
fn a_full_log_holds_writes_behind_a_hold_and_goes_on_without_one() {
    fn scenario(env: &Env) -> Outcome {
        let env = Env {
            segments: Some(2),
            ..*env
        };
        let (disk, mut store) = env.fresh()?;
        // No hold: megabyte after megabyte, more than two segments take.
        for round in 0..6u8 {
            megabyte(&mut store, u64::from(round % 3), round + 1)?;
        }
        if store.core.log.segment_files() != 2 {
            return Err(format!(
                "the log has {} segment files",
                store.core.log.segment_files()
            ));
        }
        expect(&mut store, 0, 256, 4, "megabyte 0")?;
        expect(&mut store, 256, 256, 5, "megabyte 1")?;
        expect(&mut store, 512, 256, 6, "megabyte 2")?;
        // A hold where the log ends now.
        store.set_hold(Some(store.position()));
        let mut done = Vec::new();
        let mut submitted = 0;
        let mut held = None;
        for round in 0..4u64 {
            let op = Op::Write {
                offset: round % 3 * (1 << 20),
                data: Buf::filled(256, 10 + round as u8),
            };
            store
                .submit(Request {
                    id: RequestId(round),
                    op,
                })
                .map_err(|e| e.to_string())?;
            submitted += 1;
            if store.poll(&mut done) == 0 {
                held = Some(round);
                break;
            }
        }
        let Some(held) = held else {
            return Err("no write was held behind the hold".into());
        };
        // A read behind the held write waits its turn.
        let read_id = RequestId(100);
        store
            .submit(Request {
                id: read_id,
                op: Op::Read {
                    offset: held % 3 * (1 << 20),
                    buf: Buf::zeroed(1),
                },
            })
            .map_err(|e| e.to_string())?;
        if store.poll(&mut done) != 0 || store.wait(&mut done, Duration::ZERO) != 0 {
            return Err("a request completed behind a held write".into());
        }
        if store.is_failed() {
            return Err("a full log failed the volume".into());
        }
        store.set_hold(None);
        if store.poll(&mut done) != 2 || store.poll(&mut done) != 0 {
            return Err("the held requests did not complete once each".into());
        }
        let ids: Vec<u64> = done.iter().map(|c| c.id.0).collect();
        let mut want: Vec<u64> = (0..submitted).collect();
        want.push(read_id.0);
        if ids != want || done.iter().any(|c| c.result.is_err()) {
            return Err(format!(
                "completions {ids:?}, expected {want:?} and no errors"
            ));
        }
        let seen = done.last().and_then(|c| c.buf.as_ref()).map(|b| b[0]);
        if seen != Some(10 + held as u8) {
            return Err(format!("the read behind the held write saw {seen:?}"));
        }
        flush(&mut store)?;
        drop(store);
        let lost = env.lost();
        disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        let mut store = env.reopen(&disk)?;
        expect(
            &mut store,
            held % 3 * 256,
            256,
            10 + held as u8,
            "after a power cut",
        )
    }
    holds(scenario);
}

/// Creation, interrupted. A power cut during creation leaves no store,
/// and creating again works; or a whole, empty one. Never one that
/// opens with something missing.
#[test]
fn an_interrupted_creation_leaves_no_store_or_a_whole_one() {
    let env = Env {
        base: Base::Pattern(5),
        ..environments(0)[0]
    };
    let (mut none, mut whole) = (0, 0);
    for seed in 0..48u64 {
        let disk = SimDisk::new(seed);
        disk.fail_sync(seed % 6, false);
        let key = Key(*b"a key for tests!");
        let created = Store::create_keyed(disk.clone(), &env.config(), key, env.options());
        assert!(created.is_err(), "seed {seed}");
        disk.power_cut(crate::sim::SeededCut::with_garbage(seed));
        let mut store = match Store::open(disk.clone(), env.options()) {
            Ok((store, _)) => {
                whole += 1;
                store
            }
            Err(StoreError::Log(LogError::NotFound)) => {
                none += 1;
                Store::create_keyed(disk.clone(), &env.config(), key, env.options()).unwrap()
            }
            Err(other) => panic!("seed {seed}: half a store: {other}"),
        };
        let want = [env.base_block(0), env.base_block(1)];
        expect_blocks(&mut store, 0, &want, "the base").unwrap();
        write(&mut store, 1, 1, 9).unwrap();
        expect(&mut store, 1, 1, 9, "a write").unwrap();
    }
    assert!(none > 0 && whole > 0, "none {none}, whole {whole}");
}

/// A checkpoint that cannot make its preimages durable has not touched
/// the home image: after a power cut that garbles everything unsynced,
/// every flushed block is there.
#[test]
fn a_checkpoint_writes_the_home_image_only_behind_durable_preimages() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 4, 4, 1)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        write(&mut store, 6, 1, 2)?;
        flush(&mut store)?;
        disk.fail_sync_of(PREIMAGE_NAME);
        let failed = store.checkpoint().is_err();
        if failed != (env.unit > 4096) {
            return Err(format!("the checkpoint failed: {failed}"));
        }
        drop(store);
        let lost = env.lost();
        disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        let mut store = env.reopen_ack(&disk)?;
        expect(&mut store, 4, 2, 1, "in front")?;
        expect(&mut store, 6, 1, 2, "the block written last")?;
        expect(&mut store, 7, 1, 1, "behind")
    }
    holds(scenario);
}

/// The preimage file's bytes, version 1, for two blocks: the layout the
/// documentation gives, field by field, and back.
#[test]
fn the_preimage_file_golden_bytes() {
    let volume = *b"0123456789abcdef";
    let mut blocks = vec![0xAAu8; BLOCK as usize];
    blocks.extend(vec![0xBBu8; BLOCK as usize]);
    let bytes = Preimages::encode(&volume, &[7, 1 << 33], &blocks);
    assert_eq!(bytes.len(), 4 * 4096);
    assert_eq!(&bytes[..8], b"SKYZPRE\0");
    assert_eq!(bytes[8..16], [1, 0, 0, 0, 2, 0, 0, 0]);
    assert_eq!(&bytes[16..32], &volume);
    assert!(bytes[36..508].iter().all(|b| *b == 0));
    assert_eq!(bytes[4096..4104], 7u64.to_le_bytes());
    assert_eq!(
        bytes[4104..4108],
        crc32c::crc32c(&blocks[..4096]).to_le_bytes()
    );
    assert_eq!(bytes[4112..4120], (1u64 << 33).to_le_bytes());
    assert!(bytes[4128..8192].iter().all(|b| *b == 0));
    assert_eq!(&bytes[8192..], &blocks[..]);
    assert_eq!(
        bytes[32..36],
        crc32c::crc32c(&bytes[4096..8192]).to_le_bytes()
    );
    assert_eq!(bytes[508..512], crc32c::crc32c(&bytes[..508]).to_le_bytes());

    // Whole: decoded. Damaged anywhere, for another volume, or with a
    // block beyond the device: nothing.
    let disk = SimDisk::new(0);
    let mut file = disk.create("p").unwrap();
    file.write_at(0, &bytes).unwrap();
    let decoded = Preimages::decode(&file, &volume, 1 << 34).unwrap().unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!((decoded[0].0, decoded[1].0), (7, 1 << 33));
    assert_eq!(decoded[1].1, vec![0xBB; 4096]);
    assert!(Preimages::decode(&file, &volume, 1 << 33)
        .unwrap()
        .is_none());
    assert!(Preimages::decode(&file, b"another volume!!", 1 << 34)
        .unwrap()
        .is_none());
    for at in [0, 9, 13, 20, 33, 40, 509, 4096, 4105, 8192, 16383] {
        let mut damaged = bytes.clone();
        damaged[at] ^= 1;
        file.write_at(0, &damaged).unwrap();
        assert!(
            Preimages::decode(&file, &volume, 1 << 34)
                .unwrap()
                .is_none(),
            "byte {at}"
        );
    }
    file.write_at(0, &bytes).unwrap();
    Preimages::invalidate(&mut file).unwrap();
    assert!(Preimages::decode(&file, &volume, 1 << 34)
        .unwrap()
        .is_none());
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

/// Not a property of the store: a measurement, run by hand, of what the
/// conformance suite notices by itself. For each defect it prints the
/// cases that failed, and in how many of the eight environments. The
/// scenarios above are what catches each defect by name; this says how
/// much of that the suite's own cases and random programs repeat.
#[test]
#[ignore = "a measurement, run by hand"]
fn what_the_suite_notices_of_each_defect() {
    let defects: [(&str, u32); 11] = [
        ("CHECKPOINT_UNSYNCED", Mutants::CHECKPOINT_UNSYNCED),
        ("CHECKPOINT_PAST_HOLD", Mutants::CHECKPOINT_PAST_HOLD),
        ("RECLAIM_BEFORE_HOME_SYNC", Mutants::RECLAIM_BEFORE_HOME_SYNC),
        ("INDEX_SWITCH_EARLY", Mutants::INDEX_SWITCH_EARLY),
        ("NO_INDEX_REBUILD", Mutants::NO_INDEX_REBUILD),
        ("FLUSH_NO_SYNC", Mutants::FLUSH_NO_SYNC),
        ("ACK_BEFORE_APPEND", Mutants::ACK_BEFORE_APPEND),
        ("SERVE_AFTER_FAILURE", Mutants::SERVE_AFTER_FAILURE),
        ("READ_HOME_ONLY", Mutants::READ_HOME_ONLY),
        ("NO_PREIMAGES", Mutants::NO_PREIMAGES),
        ("PREIMAGES_UNSYNCED", Mutants::PREIMAGES_UNSYNCED),
    ];
    let config = Config {
        cases: 48,
        ..Config::default()
    };
    for (name, mutants) in defects {
        let mut by_case: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut environments = 0;
        for mut harness in harnesses(mutants) {
            let report = run_all(&mut harness, &config);
            let failed = report.failed();
            if !failed.is_empty() {
                environments += 1;
            }
            for case in failed {
                *by_case.entry(case).or_default() += 1;
            }
        }
        println!("{name}: noticed in {environments} of 8 environments");
        for (case, count) in by_case {
            println!("    {case}: {count}");
        }
    }
}
