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
use crate::sim::{BySector, Fate, LoseAll, SimDisk};

type SimStore = Store<SimDisk>;
type Outcome = Result<(), String>;

const BLOCK: u64 = BLOCK_SIZE;
/// A segment that takes the largest write, at either alignment and
/// write unit.
const SEGMENT: u64 = (1 << 20) + (128 << 10);

#[derive(Debug, Clone, Copy)]
struct Env {
    mutants: u64,
    align: u32,
    unit: u32,
    garbage: bool,
    base: Base,
    segments: Option<usize>,
    /// Defects of the log under the store.
    log_mutants: u64,
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
            log_mutants: crate::log::Mutants(self.log_mutants),
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

fn environments(mutants: u64) -> Vec<Env> {
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
                    log_mutants: 0,
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
fn caught(scenario: fn(&Env) -> Outcome, mutants: u64) {
    caught_where(scenario, mutants, |_| true);
}

/// The same, where the defect only has something to act on in the
/// environments that `where_` selects.
fn caught_where(scenario: fn(&Env) -> Outcome, mutants: u64, where_: fn(&Env) -> bool) {
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
        store.set_hold(Some(hold)).map_err(|e| e.to_string())?;
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
        store.set_hold(None).map_err(|e| e.to_string())?;
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
        store
            .set_hold(Some(store.position()))
            .map_err(|e| e.to_string())?;
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
/// open writes them back. Mutants: a checkpoint without preimages, one
/// whose preimages are not synced before the home image is written, and
/// an open that does not write them back.
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
    caught_where(scenario, Mutants::NO_RESTORE, garbled_neighbours);
}

/// A neighbour is left out of the preimages only if a durable record
/// of the log writes it. One whose newest record is acknowledged and
/// not flushed is held by the home image alone as far as a power cut
/// goes, and is saved. Here four blocks are flushed and checkpointed;
/// block 4 is written again and flushed; block 5 is written and not
/// flushed; the checkpoint that applies block 4 is cut off before its
/// sync and the power cut takes the unflushed record. Block 5 must read
/// as the home image had it. Mutant: a neighbour counts as held by the
/// log whenever the index names a record for it. (The review's probe.)
#[test]
fn a_neighbour_whose_record_is_not_durable_is_saved() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 4, 4, 1)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        write(&mut store, 4, 1, 3)?;
        flush(&mut store)?;
        write(&mut store, 5, 1, 2)?;
        disk.fail_sync_of(HOME_NAME);
        if store.checkpoint().is_ok() {
            return Err("the checkpoint did not notice the failed sync".into());
        }
        drop(store);
        let lost = env.lost();
        disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        let mut store = env.reopen_ack(&disk)?;
        expect(&mut store, 4, 1, 3, "the block the checkpoint wrote")?;
        expect(&mut store, 5, 1, 1, "the neighbour whose record was lost")?;
        expect(&mut store, 6, 2, 1, "the neighbours behind")
    }
    caught_where(scenario, Mutants::UNSYNCED_COVERS, garbled_neighbours);
}

/// Where a write rewrites more than its own block.
fn large_units(env: &Env) -> bool {
    env.unit > 4096
}

/// A preimage file counts only for the batch it was written for. The
/// file is bound to the log's start, and stays on the disk, whole,
/// after its batch: once the log's start has moved it must not be
/// written back, because the blocks it saved may have been written
/// since. Here a checkpoint saves the neighbours of one block; then the
/// whole unit is written again, flushed and checkpointed, which needs
/// no preimages and leaves the old file alone; then the store is opened
/// again. The unit must hold what was written last. Mutant: an open
/// that takes a preimage file whatever start it was written for.
#[test]
fn a_preimage_file_of_an_earlier_batch_is_never_written_back() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 4, 4, 1)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        write(&mut store, 5, 1, 2)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        let saved = disk
            .list()
            .is_ok_and(|n| n.iter().any(|n| n == PREIMAGE_NAME));
        if saved != (env.unit > 4096) {
            return Err(format!("a preimage file was written: {saved}"));
        }
        write(&mut store, 4, 4, 3)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        drop(store);
        let mut store = env.reopen(&disk)?;
        expect(&mut store, 4, 4, 3, "the unit written last")
    }
    caught_where(scenario, Mutants::STALE_PREIMAGES, large_units);
}

/// Where the write unit is one block a checkpoint needs no preimages:
/// there is no preimage file, and a checkpoint costs two syncs, of the
/// home image and of the log's superblock.
#[test]
fn a_checkpoint_at_four_kib_units_has_no_preimages_and_no_extra_sync() {
    for env in environments(0).into_iter().filter(|env| env.unit == 4096) {
        let (disk, mut store) = env.fresh().unwrap();
        write(&mut store, 4, 4, 1).unwrap();
        flush(&mut store).unwrap();
        checkpoint(&mut store).unwrap();
        write(&mut store, 5, 1, 2).unwrap();
        write(&mut store, 9, 1, 2).unwrap();
        flush(&mut store).unwrap();
        let before = disk.stats();
        checkpoint(&mut store).unwrap();
        let after = disk.stats();
        assert_eq!(after.syncs - before.syncs, 2, "{env:?}");
        assert_eq!(after.dir_syncs, before.dir_syncs, "{env:?}");
        assert!(!disk.list().unwrap().iter().any(|n| n == PREIMAGE_NAME));
    }
}

/// A checkpoint works in batches, so the preimage file has a largest
/// size, and a power cut at any point of any batch, and again during
/// the writing back at the next open, loses nothing. The volume is
/// filled and checkpointed; then one block in each of its 256 write
/// units is written again, which gives three neighbours to save for
/// each; the checkpoint that applies them is stopped after a number of
/// writes and syncs, the power is cut, the open that follows is stopped
/// as well and the power cut again. After the next open every block
/// of the volume reads as what was flushed, and the preimage file never
/// held more than one batch.
#[test]
fn a_checkpoint_in_batches_can_be_cut_anywhere() {
    fn scenario(env: &Env) -> Outcome {
        let largest = (4096 + Preimages::table_len(PREIMAGE_BATCH) + PREIMAGE_BATCH * 4096) as u64;
        // Among the stops: the end of the first batch (the reclaim's
        // superblock write and its sync, about 89 and 90 at 16 KiB units, and
        // the next batch's preimage write and sync), and points inside
        // the second and the third batch.
        for stop in (0..40)
            .chain([55, 80])
            .chain(88..=93)
            .chain([120, 200, 400])
        {
            let (disk, mut store) = env.fresh()?;
            for mib in 0..4 {
                write(&mut store, mib * 256, 256, 9)?;
            }
            flush(&mut store)?;
            checkpoint(&mut store)?;
            for unit in 0..256u64 {
                write(&mut store, unit * 4 + 1, 1, (unit % 200) as u8 + 10)?;
            }
            flush(&mut store)?;
            let start = store.log_start();
            disk.halt_after(stop);
            let cut_off = store.checkpoint().is_err();
            if !cut_off && stop < 3 {
                return Err(format!("the checkpoint was not stopped at {stop}"));
            }
            if cut_off && store.log_start() == start && stop > 200 && env.unit > 4096 {
                return Err("after 200 writes and syncs no batch was done".into());
            }
            if let Ok(file) = disk.open(PREIMAGE_NAME) {
                let len = file.len().map_err(|e| e.to_string())?;
                if len > largest {
                    return Err(format!("the preimage file has {len} bytes"));
                }
                if env.unit == 4096 {
                    return Err("a preimage file at 4 KiB units".into());
                }
            }
            drop(store);
            let lost = env.lost();
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            disk.set_write_unit(u64::from(env.unit));
            // The open that writes the preimages back is cut off too.
            disk.halt_after(stop % 7);
            drop(Store::open(disk.clone(), env.options()));
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            disk.set_write_unit(u64::from(env.unit));
            let mut store = env.reopen_ack(&disk)?;
            for unit in 0..256u64 {
                let what = format!("unit {unit} after a cut at {stop}");
                expect(&mut store, unit * 4, 1, 9, &what)?;
                expect(&mut store, unit * 4 + 1, 1, (unit % 200) as u8 + 10, &what)?;
                expect(&mut store, unit * 4 + 2, 2, 9, &what)?;
            }
        }
        Ok(())
    }
    holds(scenario);
    // The reclaim that ends a batch is durable before the next batch
    // writes the home image. If it were not, a cut could bring back the
    // old start of the log beside a preimage file bound to the new one,
    // and the later batch's neighbours would have no protection. With a
    // log whose reclaim moves the start in memory only, this shows.
    for env in environments(0).into_iter().filter(garbled_neighbours) {
        let env = Env {
            log_mutants: crate::log::Mutants::REUSE_BEFORE_FLOOR_SYNC,
            ..env
        };
        assert!(
            scenario(&env).is_err(),
            "a reclaim that is not durable was not noticed in {env:?}"
        );
    }
}

/// The preimage file and the memory for it stay bounded however many
/// blocks a checkpoint has to save: 2048 isolated one-block
/// write-zeroes records on a volume of 2048 write units of 16 KiB make
/// 6144 blocks to save, and the file never holds more than one batch
/// of them; and the checkpoint's work on the log grows with the
/// records, not with their square. After a restart every block is
/// right.
#[test]
fn many_isolated_records_do_not_grow_the_preimage_file() {
    let env = Env {
        base: Base::Pattern(3),
        ..environments(0)[3]
    };
    assert!(env.unit == 16384 && env.garbage);
    let config = StoreConfig {
        size_mib: 32,
        ..env.config()
    };
    let disk = SimDisk::new(11);
    disk.set_write_unit(u64::from(env.unit));
    let key = Key(*b"a key for tests!");
    let mut store = Store::create_keyed(disk.clone(), &config, key, env.options()).unwrap();
    for unit in 0..2048u64 {
        let zeroes = Op::WriteZeroes {
            offset: (unit * 4 + 1) * BLOCK,
            len: BLOCK,
        };
        ok(&mut store, zeroes).unwrap();
    }
    flush(&mut store).unwrap();
    let before = disk.stats();
    let walked = store.core.log.walked();
    checkpoint(&mut store).unwrap();
    let batches = (disk.stats().syncs - before.syncs) / 3;
    assert!(batches >= 24, "{batches} batches");
    // And the log's records are validated a few times each, not once
    // for every batch behind them in the segment.
    let walked = store.core.log.walked() - walked;
    assert!(walked <= 8 * 2048, "{walked} records validated");
    let largest = (4096 + Preimages::table_len(PREIMAGE_BATCH) + PREIMAGE_BATCH * 4096) as u64;
    let len = disk.open(PREIMAGE_NAME).unwrap().len().unwrap();
    assert!(len <= largest && len > 0, "{len} bytes");
    drop(store);
    let mut store = env.reopen(&disk).unwrap();
    for unit in [0u64, 1, 85, 86, 1023, 1024, 2047] {
        let mut want = vec![env.base_block(unit * 4), vec![0u8; BLOCK as usize]];
        want.push(env.base_block(unit * 4 + 2));
        want.push(env.base_block(unit * 4 + 3));
        expect_blocks(&mut store, unit * 4, &want, "after the checkpoint").unwrap();
    }
}

// ---------------------------------------------------------------------
// Errors on the store's own files.

/// A write into the home image that fails, fails the volume, and the
/// log is not given up: the records the checkpoint was applying are
/// still there for the next instance. Mutant: a checkpoint that does
/// not look at the result of its writes, syncs, reclaims, and has the
/// records in neither place.
#[test]
fn a_failed_write_into_the_home_image_fails_the_volume_and_loses_nothing() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 4, 4, 1)?;
        flush(&mut store)?;
        disk.fail_write_of(HOME_NAME);
        let failed = store.checkpoint().is_err();
        let start = store.log_start();
        disk.clear_faults();
        if failed != store.is_failed() {
            return Err("the checkpoint's result and the volume disagree".into());
        }
        if failed && run(&mut store, Op::Flush)?.result.is_ok() {
            return Err("a failed volume served a flush".into());
        }
        drop(store);
        let mut store = env.reopen_ack(&disk)?;
        expect(&mut store, 4, 4, 1, "what was flushed")?;
        if !failed {
            return Err(format!(
                "the checkpoint did not notice the failed write; the log starts at {start}"
            ));
        }
        Ok(())
    }
    caught(scenario, Mutants::IGNORE_HOME_WRITE);
}

/// A volume that failed on its own files, and not on the log's, is as
/// failed for the next instance: the store marks the log, so an open
/// without an acknowledgement is refused after a restart, and one with
/// it finds what was flushed. Mutant: a failure that is not marked.
#[test]
fn a_volume_that_failed_on_its_own_files_stays_failed_after_a_restart() {
    fn scenario(env: &Env) -> Outcome {
        for file in [HOME_NAME, PREIMAGE_NAME] {
            if file == PREIMAGE_NAME && env.unit == 4096 {
                continue;
            }
            let (disk, mut store) = env.fresh()?;
            write(&mut store, 4, 4, 1)?;
            flush(&mut store)?;
            checkpoint(&mut store)?;
            write(&mut store, 5, 1, 2)?;
            flush(&mut store)?;
            disk.fail_sync_of(file);
            if store.checkpoint().is_ok() || !store.is_failed() {
                return Err(format!("a failed sync of {file} did not fail the volume"));
            }
            drop(store);
            disk.clear_faults();
            disk.kill();
            match Store::open(disk.clone(), env.options()) {
                Err(StoreError::Log(LogError::MarkedFailed)) => {}
                Err(other) => return Err(format!("after {file} failed: {other}")),
                Ok(_) => {
                    return Err(format!(
                        "a volume that failed on {file} opened unasked after a restart"
                    ))
                }
            }
            let mut store = env.reopen_ack(&disk)?;
            expect(&mut store, 4, 1, 1, "what was flushed")?;
            expect(&mut store, 5, 1, 2, "what was flushed")?;
        }
        Ok(())
    }
    caught(scenario, Mutants::FAIL_UNMARKED);
}

/// A read that fails on the store's own files fails the volume: the
/// read is answered with an error and so is every request after it,
/// whether the block was in the home image or in the log. A store that
/// cannot read its files does not go on serving the rest. Mutant: the
/// read fails alone.
#[test]
fn a_failed_read_fails_the_volume() {
    fn scenario(env: &Env) -> Outcome {
        for from_home in [true, false] {
            let (disk, mut store) = env.fresh()?;
            write(&mut store, 4, 4, 1)?;
            flush(&mut store)?;
            if from_home {
                checkpoint(&mut store)?;
                disk.fail_read_of(HOME_NAME);
            } else {
                disk.fail_read(0, false);
            }
            let read = Op::Read {
                offset: 4 * BLOCK,
                buf: Buf::zeroed(1),
            };
            if run(&mut store, read)?.result != Err(BlkError::Failed) {
                return Err("a read that could not be done succeeded".into());
            }
            disk.clear_faults();
            let again = Op::Read {
                offset: 5 * BLOCK,
                buf: Buf::zeroed(1),
            };
            if run(&mut store, again)?.result.is_ok() || !store.is_failed() {
                return Err(format!(
                    "after a failed read (from the home image: {from_home}) the volume served"
                ));
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::READ_ERROR_SERVED);
}

/// A flush that has nothing to make durable promises nothing and does
/// not touch the disk; one that has, on a disk that takes no more
/// syncs, fails and fails the volume (R11).
#[test]
fn a_flush_with_nothing_to_do_costs_nothing_and_one_with_work_finds_the_failure() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 1, 1, 1)?;
        flush(&mut store)?;
        let before = disk.stats();
        flush(&mut store)?;
        let after = disk.stats();
        if (after.syncs, after.dir_syncs, after.writes)
            != (before.syncs, before.dir_syncs, before.writes)
        {
            return Err("a flush with nothing to do touched the disk".into());
        }
        disk.fail_sync(0, true);
        flush(&mut store)?;
        if store.is_failed() {
            return Err("a flush with nothing to do failed the volume".into());
        }
        write(&mut store, 2, 1, 2)?;
        if run(&mut store, Op::Flush)?.result.is_ok() || !store.is_failed() {
            return Err("a flush with work to do succeeded on a disk that takes no syncs".into());
        }
        Ok(())
    }
    caught(scenario, Mutants::FLUSH_NO_SYNC);
}

/// A hold is taken only where a record starts or ends. A position
/// inside a record, or beyond the end of the log, is refused, the hold
/// stays as it was, and the volume is not harmed: a caller's mistake
/// does not become a failed volume at the next checkpoint. A position
/// at or below the log's start is taken. Mutant: any position is
/// taken, and the checkpoint that follows fails the volume.
#[test]
fn a_hold_is_taken_only_at_a_record_boundary() {
    fn scenario(env: &Env) -> Outcome {
        let (_disk, mut store) = env.fresh()?;
        write(&mut store, 0, 2, 1)?;
        let boundary = store.position();
        write(&mut store, 2, 2, 2)?;
        let end = store.position();
        flush(&mut store)?;
        store.set_hold(Some(boundary)).map_err(|e| e.to_string())?;
        for bad in [
            Lsn(boundary.0 + 1),
            Lsn(boundary.0 + 4096),
            Lsn(end.0 + (1 << 20)),
        ] {
            if store.set_hold(Some(bad)) != Err(NotABoundary(bad)) {
                return Err(format!("a hold at {bad} was taken"));
            }
            if store.hold() != Some(boundary) {
                return Err("a refused hold changed the hold".into());
            }
        }
        store.set_hold(Some(end)).map_err(|e| e.to_string())?;
        store
            .set_hold(Some(store.log_start()))
            .map_err(|e| e.to_string())?;
        store.set_hold(Some(Lsn(0))).map_err(|e| e.to_string())?;
        store.set_hold(Some(boundary)).map_err(|e| e.to_string())?;
        checkpoint(&mut store)?;
        if store.is_failed() {
            return Err("the checkpoint failed the volume".into());
        }
        expect(&mut store, 0, 2, 1, "below the hold")?;
        expect(&mut store, 2, 2, 2, "above the hold")
    }
    holds(scenario);
    // With the defect the mistake is taken, and shows as the reviewer
    // found it: the next checkpoint writes the home image and then
    // fails at the reclaim, which fails the volume.
    for env in environments(Mutants::HOLD_UNCHECKED) {
        let (_disk, mut store) = env.fresh().unwrap();
        write(&mut store, 0, 2, 1).unwrap();
        let inside = Lsn(store.position().0 + 4096);
        write(&mut store, 2, 2, 2).unwrap();
        flush(&mut store).unwrap();
        assert_eq!(store.set_hold(Some(inside)), Ok(()), "{env:?}");
        assert!(store.checkpoint().is_err() && store.is_failed(), "{env:?}");
    }
    // A position in a segment's header, after the log has rolled into
    // its second segment, is no boundary either: the hold is refused
    // and a checkpoint leaves the volume healthy. With a log that does
    // not look at such positions the hold is taken and the checkpoint
    // fails the volume.
    for header in [0, 1, 64, 4095] {
        for defect in [false, true] {
            for env in environments(0) {
                let env = Env {
                    log_mutants: if defect {
                        crate::log::Mutants::HEADER_IS_BOUNDARY
                    } else {
                        0
                    },
                    ..env
                };
                let (_disk, mut store) = env.fresh().unwrap();
                write(&mut store, 0, 256, 1).unwrap();
                write(&mut store, 256, 256, 2).unwrap();
                flush(&mut store).unwrap();
                assert!(store.position().0 > SEGMENT, "{env:?}");
                let at = Lsn(SEGMENT + header);
                let taken = store.set_hold(Some(at));
                let failed = store.checkpoint().is_err() || store.is_failed();
                if defect {
                    assert!(taken.is_ok() && failed, "{env:?} at {at}");
                } else {
                    assert_eq!(taken, Err(NotABoundary(at)), "{env:?}");
                    assert!(!failed, "{env:?} at {at}");
                    expect(&mut store, 0, 256, 1, "after the checkpoint").unwrap();
                    expect(&mut store, 256, 256, 2, "after the checkpoint").unwrap();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// One instance, and failure announced.

/// One instance per volume (design §4.2): while a store is open, a
/// second open of its directory is refused at once and leaves every
/// byte of the directory as it was; after the first instance's process
/// is lost the open succeeds. Opening is a recovery that writes, so a
/// second instance under a live writer would damage the log. Mutant: an
/// open without the lock.
#[test]
fn a_second_instance_is_refused_and_changes_nothing() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 1, 1, 1)?;
        flush(&mut store)?;
        write(&mut store, 2, 1, 2)?;
        let before = disk.image();
        match Store::open(disk.clone(), env.options()) {
            Err(StoreError::Locked) => {}
            Err(other) => return Err(format!("the second open: {other}")),
            Ok(_) => return Err("a second instance opened the store".into()),
        }
        let config = env.config();
        match Store::create_keyed(disk.clone(), &config, Key([1; 16]), env.options()) {
            Err(StoreError::Locked) => {}
            Err(other) => return Err(format!("a creation over it: {other}")),
            Ok(_) => return Err("a store was created over a live one".into()),
        }
        if disk.image() != before {
            return Err("the refused open changed the directory".into());
        }
        // The first instance goes on.
        write(&mut store, 3, 1, 3)?;
        flush(&mut store)?;
        let mut store = env.reopen(&disk)?;
        expect(&mut store, 1, 1, 1, "after the first instance")?;
        expect(&mut store, 2, 1, 2, "after the first instance")?;
        expect(&mut store, 3, 1, 3, "after the first instance")?;
        // And when an instance is dropped, the next may open.
        drop(store);
        Store::open(disk.clone(), env.options())
            .map(|_| ())
            .map_err(|e| format!("after a drop: {e}"))
    }
    caught(scenario, Mutants::NO_LOCK);
}

/// What an observer saw when it was called: whether the failure marker
/// was in the directory.
type Seen = std::sync::Arc<std::sync::Mutex<Vec<bool>>>;

fn observed(env: &Env, disk: &SimDisk, log_mutants: u64) -> (OpenOptions, Seen) {
    let seen = Seen::default();
    let (watch, record) = (disk.clone(), seen.clone());
    let options = OpenOptions {
        on_failure: Some(FailureObserver::new(move || {
            let there = watch
                .list()
                .is_ok_and(|names| names.iter().any(|n| n == crate::log::MARKER_NAME));
            record.lock().unwrap().push(there);
        })),
        log_mutants: crate::log::Mutants(log_mutants),
        ..env.options()
    };
    (options, seen)
}

/// Failure is announced before the marker is attempted (design §4.2):
/// the store's observer is called once, at the moment the volume fails,
/// while the marker is not yet in the directory, whether the failure is
/// the log's (a sync) or the store's own (the home image), and whether
/// the marker can then be written or the disk takes nothing more. A
/// healthy volume never calls it. Mutant: the call placed after the
/// marker.
#[test]
fn the_failure_is_announced_before_the_marker_is_attempted() {
    fn scenario(env: &Env, log_mutants: u64) -> Outcome {
        for case in 0..3 {
            let (disk, store) = env.fresh()?;
            drop(store);
            disk.kill();
            let (options, seen) = observed(env, &disk, log_mutants);
            let (mut store, _) = Store::open(disk.clone(), options).map_err(|e| e.to_string())?;
            write(&mut store, 1, 1, 1)?;
            flush(&mut store)?;
            checkpoint(&mut store)?;
            write(&mut store, 2, 1, 2)?;
            if !seen.lock().unwrap().is_empty() {
                return Err("a healthy volume called the observer".into());
            }
            let failed = match case {
                // The log's own failure.
                0 => {
                    disk.fail_sync(0, false);
                    run(&mut store, Op::Flush)?.result.is_err()
                }
                // The store's: the home image.
                1 => {
                    flush(&mut store)?;
                    disk.fail_sync_of(HOME_NAME);
                    store.checkpoint().is_err()
                }
                // A disk that takes nothing more, the marker included.
                _ => {
                    disk.halt_after(0);
                    run(&mut store, Op::Flush)?.result.is_err()
                }
            };
            if !failed || !store.is_failed() {
                return Err(format!("case {case}: the volume did not fail"));
            }
            // Later requests fail and announce nothing more.
            if run(&mut store, Op::Flush)?.result.is_ok() {
                return Err("a flush succeeded on a failed volume".into());
            }
            let seen = seen.lock().unwrap().clone();
            if seen != [false] {
                return Err(format!("case {case}: the observer saw {seen:?}"));
            }
            let marked = disk
                .list()
                .is_ok_and(|n| n.iter().any(|n| n == crate::log::MARKER_NAME));
            if marked != (case < 2) {
                return Err(format!("case {case}: the marker is there: {marked}"));
            }
        }
        Ok(())
    }
    for env in environments(0) {
        if let Err(why) = scenario(&env, 0) {
            panic!("in {env:?}: {why}");
        }
        assert!(
            scenario(&env, crate::log::Mutants::OBSERVE_AFTER_MARKER).is_err(),
            "the late call was not noticed in {env:?}"
        );
    }
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
        store
            .set_hold(Some(store.position()))
            .map_err(|e| e.to_string())?;
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
            if store.wait(&mut done, Duration::ZERO) == 0 {
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
        store.set_hold(None).map_err(|e| e.to_string())?;
        if store.poll(&mut done) != 0
            || store.wait(&mut done, Duration::ZERO) != 2
            || store.wait(&mut done, Duration::ZERO) != 0
        {
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

/// A creation removes only what a creation leaves, by its exact names.
/// A directory with any other name in it, also one that begins like a
/// store's own, is refused with every file as it was and nothing made,
/// not even the lock file.
#[test]
fn a_creation_refuses_a_directory_with_a_file_that_is_not_its_own() {
    let env = environments(0)[0];
    let key = Key(*b"a key for tests!");
    for stranger in [
        "home.tar",
        "home.pre",
        "home.img.bak",
        "log.txt",
        "log.000001",
        "log.failed",
        "log.super.old",
        "notes",
    ] {
        let disk = SimDisk::new(1);
        for name in [stranger, "home.new", "log.000000"] {
            let mut file = disk.create(name).unwrap();
            file.write_at(0, name.as_bytes()).unwrap();
            file.sync().unwrap();
        }
        disk.sync().unwrap();
        let before = disk.list().unwrap();
        let created = Store::create_keyed(disk.clone(), &env.config(), key, env.options());
        assert!(
            matches!(created, Err(StoreError::NotEmpty)),
            "{stranger}: {:?}",
            created.err()
        );
        assert_eq!(disk.list().unwrap(), before, "{stranger}");
        for name in &before {
            let mut bytes = vec![0u8; name.len()];
            disk.open(name).unwrap().read_at(0, &mut bytes).unwrap();
            assert_eq!(bytes, name.as_bytes(), "{stranger}: {name}");
        }
    }
    assert_eq!(crate::log::CREATE_LEFTOVERS[0], crate::log::slot_name(0));
    assert!(super::stranger(&CREATE_LEFTOVERS.map(str::to_owned)).is_none());
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
/// documentation gives, field by field, and back. Every part is tagged
/// under the volume's key and bound to the log's start and to each
/// block's place, so a file that is torn, stale, moved or for another
/// key is not taken for a whole one.
#[test]
fn the_preimage_file_golden_bytes() {
    let key = Key(*b"a key for tests!");
    let volume = *b"0123456789abcdef";
    let start = Lsn(0x1122_3344_5566);
    let mut blocks = vec![0xAAu8; BLOCK as usize];
    blocks.extend(vec![0xBBu8; BLOCK as usize]);
    let bytes = Preimages::encode(&key, &volume, start, &[7, 1 << 33], &blocks);
    assert_eq!(bytes.len(), 4 * 4096);
    assert_eq!(&bytes[..8], b"SKYZPRE\0");
    assert_eq!(bytes[8..16], [1, 0, 0, 0, 2, 0, 0, 0]);
    assert_eq!(&bytes[16..32], &volume);
    assert_eq!(bytes[32..40], start.0.to_le_bytes());
    assert!(bytes[48..504].iter().all(|b| *b == 0));
    assert!(bytes[512..4096].iter().all(|b| *b == 0));
    assert_eq!(bytes[4096..4104], 7u64.to_le_bytes());
    assert_eq!(bytes[4112..4120], (1u64 << 33).to_le_bytes());
    assert!(bytes[4128..8192].iter().all(|b| *b == 0));
    assert_eq!(&bytes[8192..], &blocks[..]);
    let tagged = |parts: &[&[u8]]| siphash24(&key, &parts.concat()).to_le_bytes();
    let at = start.0.to_le_bytes();
    assert_eq!(
        bytes[4104..4112],
        tagged(&[&at, &7u64.to_le_bytes(), &blocks[..4096]])
    );
    assert_eq!(
        bytes[4120..4128],
        tagged(&[&at, &(1u64 << 33).to_le_bytes(), &blocks[4096..]])
    );
    assert_eq!(bytes[40..48], tagged(&[&at, &bytes[4096..8192]]));
    assert_eq!(bytes[504..512], tagged(&[&bytes[..504]]));
    // The bytes themselves, so that a change of the tag function or of
    // the layout does not pass unnoticed.
    assert_eq!(
        crc32c::crc32c(&bytes),
        GOLDEN_PREIMAGE_CRC,
        "{:#010x}",
        crc32c::crc32c(&bytes)
    );

    // Whole: decoded. Damaged anywhere, for another volume, another
    // key or another start of the log, or with a block beyond the
    // device: nothing.
    let disk = SimDisk::new(0);
    let mut file = disk.create("p").unwrap();
    file.write_at(0, &bytes).unwrap();
    let decode = |file: &crate::sim::SimFile, key: &Key, volume: &[u8; 16], start, blocks| {
        Preimages::decode(file, key, volume, start, blocks).unwrap()
    };
    let decoded = decode(&file, &key, &volume, Some(start), 1 << 34).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!((decoded[0].0, decoded[1].0), (7, 1 << 33));
    assert_eq!(decoded[1].1, vec![0xBB; 4096]);
    assert!(decode(&file, &key, &volume, None, 1 << 34).is_some());
    assert!(decode(&file, &key, &volume, Some(start), 1 << 33).is_none());
    assert!(decode(&file, &key, b"another volume!!", Some(start), 1 << 34).is_none());
    assert!(decode(&file, &Key::PUBLIC, &volume, Some(start), 1 << 34).is_none());
    assert!(decode(&file, &key, &volume, Some(Lsn(start.0 + 64)), 1 << 34).is_none());
    for at in [
        0, 9, 13, 20, 33, 41, 100, 505, 4096, 4105, 4113, 8192, 12288, 16383,
    ] {
        let mut damaged = bytes.clone();
        damaged[at] ^= 1;
        file.write_at(0, &damaged).unwrap();
        assert!(
            decode(&file, &key, &volume, Some(start), 1 << 34).is_none(),
            "byte {at}"
        );
    }
    // The two blocks exchanged, each still with its own tag: a block is
    // bound to its place.
    let mut swapped = bytes.clone();
    swapped[4096..4104].copy_from_slice(&(1u64 << 33).to_le_bytes());
    swapped[4112..4120].copy_from_slice(&7u64.to_le_bytes());
    let table_tag = tagged(&[&at, &swapped[4096..8192]]);
    swapped[40..48].copy_from_slice(&table_tag);
    let head_tag = tagged(&[&swapped[..504]]);
    swapped[504..512].copy_from_slice(&head_tag);
    file.write_at(0, &swapped).unwrap();
    assert!(decode(&file, &key, &volume, Some(start), 1 << 34).is_none());
}

const GOLDEN_PREIMAGE_CRC: u32 = 0x71c8_1c1e;

// ---------------------------------------------------------------------
// The conformance suite.

use super::harness::SimHarness;
use skyzfs_blk::conformance::{run_all, Config};

fn harnesses(mutants: u64) -> Vec<SimHarness> {
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
                    threaded: false,
                    marks: None,
                    sync_delay: 0,
                    readers: 1,
                });
            }
        }
    }
    out
}

/// The contract holds while the store checkpoints by itself: the suite
/// passes in every environment with marks so low that steps fall
/// between its operations, at its kills and at its power cuts, with
/// the store called directly and behind its worker. Steps of two
/// records begin at 24 KiB of log and end below 12 KiB; a new log
/// holds 8 KiB. And the same with the steps' home-image syncs handed
/// over and ending up to three looks later, so that requests, syncs of
/// the log and rolls fall between a sync's begin and its end; served,
/// that one has three reader threads (the other served one has one).
#[test]
fn the_store_passes_the_conformance_suite_while_it_checkpoints_in_steps() {
    let config = Config {
        cases: 16,
        ..Config::default()
    };
    let marks = Marks {
        high: 24 << 10,
        low: 12 << 10,
        step: StepBound {
            payload: 16 << 10,
            records: 2,
        },
    };
    for (threaded, sync_delay, readers) in
        [(false, 0, 1), (true, 0, 1), (false, 3, 1), (true, 3, 3)]
    {
        for mut harness in harnesses(0) {
            harness.marks = Some(marks);
            harness.threaded = threaded;
            harness.sync_delay = sync_delay;
            harness.readers = readers;
            let report = run_all(&mut harness, &config);
            assert!(report.passed(), "in {harness:?}:\n{report}");
            if !threaded {
                // One case can find nothing to do: it cuts the power
                // between records that are not durable, and with marks
                // this low the store's own sync has made them durable
                // before the cut. That is the rule's effect, which the
                // contract allows, and no other case may be idle.
                let idle: Vec<_> = report
                    .not_exercised()
                    .into_iter()
                    .filter(|case| *case != "power_cut_at_every_point_leaves_a_prefix")
                    .collect();
                assert!(idle.is_empty(), "in {harness:?} not exercised: {idle:?}");
                assert!(report.weaker().is_empty(), "in {harness:?}:\n{report}");
            }
        }
    }
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
        assert!(report.weaker().is_empty(), "in {harness:?}:\n{report}");
    }
}

/// The contract, rules R1 to R12, through the worker thread: the suite
/// passes against [`Served`] too, at both write units. Here the thread
/// decides how far requests in flight got, so the fixed cases carry
/// this test and the random programs are fewer; the one case that
/// needs a power cut after a chosen record is not exercised.
#[test]
fn the_served_store_passes_the_conformance_suite() {
    let config = Config {
        cases: 12,
        ..Config::default()
    };
    for mut harness in harnesses(0) {
        if harness.align != 64 || !harness.garbage {
            continue;
        }
        harness.threaded = true;
        let report = run_all(&mut harness, &config);
        assert!(report.passed(), "in {harness:?}:\n{report}");
        // Behind the thread the harness cannot name the records a cut
        // keeps, so the one case that cuts after each record is left
        // to the inline driver.
        assert_eq!(
            report.not_exercised(),
            ["power_cut_at_every_point_leaves_a_prefix"],
            "in {harness:?}"
        );
        assert!(report.weaker().is_empty(), "in {harness:?}:\n{report}");
    }
}

/// What was acknowledged (design §4.2): the store reports where the
/// newest record starts among the mutations it has completed. A write,
/// a write-zeroes and a discard move it to a position at or beyond the
/// log's end before them and below its end after them; a read, a
/// flush, a rejected request and a checkpoint leave it; a new instance
/// starts with none.
#[test]
fn the_acknowledged_position_is_where_the_newest_completed_record_starts() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        if store.acknowledged().is_some() {
            return Err("a position before anything was acknowledged".into());
        }
        read(&mut store, 0, 1)?;
        flush(&mut store)?;
        if store.acknowledged().is_some() {
            return Err("a read or a flush gave a position".into());
        }
        let mutations = [
            Op::Write {
                offset: 0,
                data: Buf::filled(3, 1),
            },
            Op::WriteZeroes {
                offset: BLOCK,
                len: 2 * BLOCK,
            },
            Op::Discard {
                offset: 0,
                len: BLOCK,
            },
        ];
        for op in mutations {
            let before = store.position();
            ok(&mut store, op)?;
            let Some(at) = store.acknowledged() else {
                return Err("no position after a mutation".into());
            };
            if at < before || at >= store.position() {
                return Err(format!(
                    "{at} is not a record between {before} and {}",
                    store.position()
                ));
            }
            read(&mut store, 0, 4)?;
            flush(&mut store)?;
            let rejected = Op::Discard {
                offset: 1,
                len: BLOCK,
            };
            if run(&mut store, rejected)?.result.is_ok() {
                return Err("a misaligned discard was taken".into());
            }
            if store.acknowledged() != Some(at) {
                return Err("something that is no mutation moved the position".into());
            }
        }
        let at = store.acknowledged();
        checkpoint(&mut store)?;
        if store.acknowledged() != at {
            return Err("a checkpoint moved the position".into());
        }
        drop(store);
        let store = env.reopen(&disk)?;
        if store.acknowledged().is_some() {
            return Err("a new instance has a position".into());
        }
        Ok(())
    }
    holds(scenario);
}

/// What was acknowledged (design §4.2): after a kill at any point, the
/// prefix the next instance recovers ends beyond every position an
/// earlier instance reported, so an open that is told what the log
/// must reach ([`OpenOptions::must_reach`]) never refuses a store that
/// was only killed. Across changes of log segment,
/// behind checkpoints that gave the whole log up, with and without
/// flushes in between, and killed again before anything new was
/// written. Mutant: the position is the record's end, where the
/// recovered prefix ends exactly and not beyond.
#[test]
fn a_kill_never_leaves_the_log_ending_at_or_before_what_was_acknowledged() {
    fn scenario(env: &Env) -> Outcome {
        let env = Env {
            segments: Some(2),
            ..*env
        };
        let (disk, mut store) = env.fresh()?;
        let mut stored: Option<Lsn> = None;
        let (mut segments, mut emptied, mut kills) = (0, 0, 0);
        let mut segment = store.position().0 / SEGMENT;
        for round in 0..400u64 {
            let block = round * 7 % 1000;
            match round % 5 {
                0..=2 => write(&mut store, block, 1 + (round % 16) as usize, round as u8)?,
                3 => ok(
                    &mut store,
                    Op::WriteZeroes {
                        offset: block * BLOCK,
                        len: 8 * BLOCK,
                    },
                )
                .map(|_| ())?,
                _ => ok(
                    &mut store,
                    Op::Discard {
                        offset: block * BLOCK,
                        len: BLOCK,
                    },
                )
                .map(|_| ())?,
            }
            // What a transport would store before it tells its client.
            stored = stored.max(store.acknowledged());
            if store.position().0 / SEGMENT != segment {
                segment = store.position().0 / SEGMENT;
                segments += 1;
            }
            if round % 3 == 0 {
                flush(&mut store)?;
            }
            if round % 37 == 36 {
                flush(&mut store)?;
                checkpoint(&mut store)?;
                if store.log_start() == store.position() {
                    emptied += 1;
                }
            }
            // A kill after every operation for a while, then at
            // changing distances, and twice in a row.
            let kill = round < 60 || round % (2 + round % 5) == 0 || round % 37 == 36;
            if kill {
                for _ in 0..1 + round % 2 {
                    drop(store);
                    disk.kill();
                    let options = OpenOptions {
                        must_reach: stored,
                        ..env.options()
                    };
                    let (reopened, recovery) = Store::open(disk.clone(), options)
                        .map_err(|e| format!("round {round}, acknowledged {stored:?}: {e}"))?;
                    store = reopened;
                    kills += 1;
                    let end = recovery.end;
                    if stored.is_some_and(|stored| end <= stored) {
                        return Err(format!(
                            "round {round}: the recovered prefix ends at {end}, \
                             acknowledged was {stored:?}"
                        ));
                    }
                }
            }
        }
        if segments < 3 || emptied < 3 || kills < 100 {
            return Err(format!(
                "the scenario saw {segments} segment changes, {emptied} emptied logs, {kills} kills"
            ));
        }
        Ok(())
    }
    caught(scenario, Mutants::ACKNOWLEDGED_IS_END);
}

/// What was acknowledged (design §4.2), the other side: a log that
/// comes back without an acknowledged record is refused by the open
/// that is told what the log must reach, and that open writes nothing,
/// so every later open with the same position refuses again. A write
/// is flushed, a second is acknowledged and its position taken, and a
/// power cut loses the second.
///
/// The comparison has to be inside the open and before the recovery
/// writes: an open that is not told the position succeeds, reports a
/// prefix that ends at or before the lost record, and seals it with a
/// mark; from then on the log's end, and the prefix every later open
/// finds, lie beyond the place of the lost record, and an open that is
/// told the position no longer notices. The test shows that too.
/// Mutant (of the log): the comparison is made after the recovery has
/// sealed the prefix.
#[test]
fn an_open_refuses_a_log_that_lost_an_acknowledged_record_and_writes_nothing() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 0, 1, 1)?;
        flush(&mut store)?;
        write(&mut store, 1, 1, 2)?;
        let Some(stored) = store.acknowledged() else {
            return Err("no position".into());
        };
        drop(store);
        let lost = env.lost();
        disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        let told = OpenOptions {
            must_reach: Some(stored),
            ..env.options()
        };
        let before = disk.image();
        for attempt in 0..3 {
            match Store::open(disk.clone(), told.clone()) {
                Err(StoreError::Log(LogError::Short { end, must_reach })) => {
                    if must_reach != stored || end > stored {
                        return Err(format!("refused with {end} and {must_reach}"));
                    }
                }
                Err(other) => return Err(format!("attempt {attempt}: {other}")),
                Ok(_) => return Err(format!("attempt {attempt}: the short log was opened")),
            }
            disk.kill();
            if disk.image() != before {
                return Err(format!("attempt {attempt}: the refused open wrote"));
            }
        }
        // An open that is not told: it reports the short prefix, and
        // seals it.
        let (mut store, recovery) =
            Store::open(disk.clone(), env.options()).map_err(|e| e.to_string())?;
        expect(&mut store, 1, 1, 0, "the write that was lost")?;
        if recovery.end > stored {
            return Err(format!(
                "the prefix ends at {}, beyond the lost record at {stored}",
                recovery.end
            ));
        }
        if store.position() <= stored {
            return Err("the log's end did not move past the lost record".into());
        }
        drop(store);
        disk.kill();
        // The evidence is gone: this is why the daemon never opens a
        // store without the position it has stored.
        match Store::open(disk.clone(), told) {
            Ok((_, recovery)) if recovery.end > stored => Ok(()),
            Ok(_) => Err("the sealed prefix does not end beyond the lost record".into()),
            Err(e) => Err(format!("after an open that sealed: {e}")),
        }
    }
    holds(scenario);
    for env in environments(0) {
        let env = Env {
            log_mutants: crate::log::Mutants::COMPARE_AFTER_SEAL,
            ..env
        };
        assert!(
            scenario(&env).is_err(),
            "a comparison after the seal was not noticed in {env:?}"
        );
    }
}

/// A position below the log's start, of a record that a checkpoint has
/// given up, is reached; so is the position of the last record of a
/// log that was only killed; and an open that is told nothing is never
/// refused. A rig that pretends the log came back one record short
/// (`pretend_short_by`) is refused for the last record's position and
/// not for the one before it, with nothing written. A position that
/// was recorded for another store counts as none.
#[test]
fn an_open_is_refused_exactly_when_the_prefix_does_not_reach_the_position() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 0, 1, 1)?;
        let first = store.acknowledged();
        write(&mut store, 1, 1, 2)?;
        let last = store.acknowledged();
        drop(store);
        disk.kill();
        let open = |must_reach: Option<Lsn>, pretend_short_by: u32| {
            let options = OpenOptions {
                must_reach,
                pretend_short_by,
                ..env.options()
            };
            let before = disk.image();
            let opened = Store::open(disk.clone(), options).map(|_| ());
            disk.kill();
            let short = matches!(opened, Err(StoreError::Log(LogError::Short { .. })));
            if opened.is_err() && (!short || disk.image() != before) {
                return Err(format!("{opened:?}, or the refused open wrote"));
            }
            Ok(opened.is_ok())
        };
        for (must_reach, pretend, want) in [
            (last, 1, false),
            (first, 1, true),
            (first, 2, false),
            (last, 5, false),
            (None, 1, true),
            (last, 0, true),
            (first, 0, true),
            (None, 0, true),
        ] {
            let got = open(must_reach, pretend)?;
            if got != want {
                return Err(format!(
                    "must reach {must_reach:?}, short by {pretend}: opened {got}"
                ));
            }
        }
        // A position recorded for another store counts as none, and one
        // recorded for this store counts.
        let (store, _) = Store::open(disk.clone(), env.options()).map_err(|e| e.to_string())?;
        let identity = store.identity();
        drop(store);
        disk.kill();
        for (recorded_for, want) in [(identity, false), (*b"some other store", true)] {
            let options = OpenOptions {
                must_reach: last,
                must_reach_identity: Some(recorded_for),
                pretend_short_by: 1,
                ..env.options()
            };
            let opened = Store::open(disk.clone(), options).is_ok();
            disk.kill();
            if opened != want {
                return Err(format!(
                    "a position recorded for {recorded_for:?}: opened {opened}"
                ));
            }
        }
        // A checkpoint gives the whole log up: the position is below
        // the log's start and still reached.
        let (mut store, _) = Store::open(disk.clone(), env.options()).map_err(|e| e.to_string())?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        if store.log_start() <= last.unwrap_or(Lsn(0)) {
            return Err("the checkpoint did not give the record up".into());
        }
        drop(store);
        disk.kill();
        if !open(last, 0)? {
            return Err("a position below the log's start was not reached".into());
        }
        Ok(())
    }
    holds(scenario);
}

/// The store behind its worker reports the acknowledged position for
/// what its owner has taken, never ahead of it: before a completion is
/// taken there is none, although the worker has appended the record;
/// once taken, it is that record's start. The waker is called once for
/// every completion made ready.
#[test]
fn the_served_store_acknowledges_what_its_owner_has_taken() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let env = environments(0)[0];
    let (_disk, store) = env.fresh().unwrap();
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let waker = Waker::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let mut served = store.serve_with(Some(waker)).unwrap();
    let before = served.position().unwrap();
    let write = |id: u64, block: u64| Request {
        id: RequestId(id),
        op: Op::Write {
            offset: block * BLOCK,
            data: Buf::filled(1, 9),
        },
    };
    // The waker is called once the completion is ready to be taken.
    let ready = |n: u32| {
        for _ in 0..10_000 {
            if calls.load(Ordering::SeqCst) >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the waker was not called {n} times");
    };
    served.submit(write(1, 0)).unwrap();
    ready(1);
    let after_first = served.position().unwrap();
    assert!(after_first > before);
    assert_eq!(served.acknowledged(), None, "ahead of what was taken");
    let mut done = Vec::new();
    assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
    let first = served.acknowledged().unwrap();
    assert!(first >= before && first < after_first, "{first}");
    served.submit(write(2, 1)).unwrap();
    let read = Request {
        id: RequestId(3),
        op: Op::Read {
            offset: 0,
            buf: Buf::zeroed(1),
        },
    };
    served.submit(read).unwrap();
    ready(3);
    let end = served.position().unwrap();
    assert_eq!(
        served.acknowledged(),
        Some(first),
        "ahead of what was taken"
    );
    while done.len() < 3 {
        served.wait(&mut done, Duration::from_secs(10));
    }
    let second = served.acknowledged().unwrap();
    assert!(second >= after_first && second < end, "{second}");
    assert!(!served.gone());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "one call for each completion"
    );
    drop(served);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "and one when the worker ended, and one when the reader did"
    );
}

/// A worker that has ended tells its owner: the waker is called, `gone`
/// says so from then on, a request in flight stays in flight and a new
/// one is not taken. Before that, `gone` is false.
#[test]
fn a_worker_that_has_ended_tells_its_owner() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let env = environments(0)[0];
    let (_disk, store) = env.fresh().unwrap();
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let waker = Waker::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let mut served = store.serve_with(Some(waker)).unwrap();
    assert!(!served.gone());
    served.panic_worker();
    assert!(served.gone());
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the owner is woken");
    let flush = Request {
        id: RequestId(1),
        op: Op::Flush,
    };
    let refused = served.submit(flush).unwrap_err();
    assert_eq!(refused.kind, SubmitErrorKind::Busy);
    let mut done = Vec::new();
    assert_eq!(served.poll(&mut done), 0);
}

/// The store's identity is the same at every open of one store and
/// behind the worker, differs for a store made again for the same
/// volume with another key, and is not the key.
#[test]
fn the_identity_belongs_to_the_store_and_is_not_its_key() {
    let env = environments(0)[0];
    let (disk, store) = env.fresh().unwrap();
    let identity = store.identity();
    drop(store);
    let store = env.reopen(&disk).unwrap();
    assert_eq!(store.identity(), identity);
    let served = store.serve().unwrap();
    assert_eq!(served.identity(), identity);
    let key = *b"a key for tests!";
    assert_ne!(identity, key);
    assert_ne!(identity[..8], key[..8]);
    assert_ne!(identity[8..], key[8..]);
    assert_ne!(identity[..8], identity[8..]);
    // The same volume, made again: another key, another store.
    let other = SimDisk::new(7);
    let again = Store::create_keyed(
        other,
        &env.config(),
        Key(*b"another key here"),
        env.options(),
    )
    .unwrap();
    assert_ne!(again.identity(), identity);
}

/// Not a property of the store: a measurement, run by hand, of what the
/// conformance suite notices by itself. For each defect it prints the
/// cases that failed, and in how many of the eight environments. The
/// scenarios above are what catches each defect by name; this says how
/// much of that the suite's own cases and random programs repeat.
#[test]
#[ignore = "a measurement, run by hand"]
fn what_the_suite_notices_of_each_defect() {
    let defects: [(&str, u64); 13] = [
        ("STALE_PREIMAGES", Mutants::STALE_PREIMAGES),
        ("NO_RESTORE", Mutants::NO_RESTORE),
        ("CHECKPOINT_UNSYNCED", Mutants::CHECKPOINT_UNSYNCED),
        ("CHECKPOINT_PAST_HOLD", Mutants::CHECKPOINT_PAST_HOLD),
        (
            "RECLAIM_BEFORE_HOME_SYNC",
            Mutants::RECLAIM_BEFORE_HOME_SYNC,
        ),
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

/// The lock is a step of its own: it is taken without a recovery, an
/// open takes it over, and an open that fails hands it back still held.
#[test]
fn the_lock_is_taken_apart_from_the_open_and_comes_back_held_when_the_open_fails() {
    let env = environments(0)[0];
    let (disk, mut store) = env.fresh().unwrap();
    write(&mut store, 0, 1, 7).unwrap();
    let end = store.position();
    drop(store);
    disk.kill();
    let files = disk.list().unwrap();

    // Taking the lock reads and writes nothing, and keeps others out.
    let lock = Store::lock_volume(&disk).unwrap();
    assert_eq!(disk.list().unwrap(), files);
    assert!(matches!(Store::lock_volume(&disk), Err(StoreError::Locked)));
    assert!(matches!(
        Store::open(disk.clone(), env.options()),
        Err(StoreError::Locked)
    ));

    // An open that fails (the log does not reach the position) gives
    // the lock back, and it is still held.
    let options = OpenOptions {
        must_reach: Some(end),
        ..env.options()
    };
    let lock = match Store::open_locked(disk.clone(), lock, options) {
        Err((StoreError::Log(LogError::Short { .. }), lock)) => lock,
        other => panic!("{:?}", other.map(|_| ()).map_err(|(e, _)| e)),
    };
    assert!(matches!(Store::lock_volume(&disk), Err(StoreError::Locked)));

    // An open that succeeds takes it over for as long as the store
    // lives.
    let (store, _) = match Store::open_locked(disk.clone(), lock, env.options()) {
        Ok(opened) => opened,
        Err((e, _)) => panic!("{e}"),
    };
    assert!(matches!(Store::lock_volume(&disk), Err(StoreError::Locked)));
    drop(store);
    drop(Store::lock_volume(&disk).unwrap());
}

/// The hook for test rigs: a served store whose owner fails it on
/// demand answers what it completed before as completed, fails every
/// later request, tells its owner, and is found marked as failed by
/// the next open.
#[test]
fn a_served_store_failed_on_demand_fails_every_later_request_and_is_found_marked() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let env = environments(0)[0];
    let (disk, store) = env.fresh().unwrap();
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let waker = Waker::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let mut served = store.serve_with(Some(waker)).unwrap();
    let write = |id: u64, block: u64| Request {
        id: RequestId(id),
        op: Op::Write {
            offset: block * BLOCK,
            data: Buf::filled(1, 9),
        },
    };
    let mut done = Vec::new();
    served.submit(write(1, 0)).unwrap();
    assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
    assert_eq!(done[0].result, Ok(()));
    let woken = calls.load(Ordering::SeqCst);

    served.fail_now();
    assert!(
        calls.load(Ordering::SeqCst) > woken,
        "the owner is woken by the failure"
    );
    assert!(
        !served.gone(),
        "the worker lives: it answers with the failure"
    );

    let flush = Request {
        id: RequestId(3),
        op: Op::Flush,
    };
    let read = Request {
        id: RequestId(4),
        op: Op::Read {
            offset: 0,
            buf: Buf::zeroed(1),
        },
    };
    for request in [write(2, 1), flush, read] {
        done.clear();
        served.submit(request).unwrap();
        assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
        assert_eq!(done[0].result, Err(BlkError::Failed), "{:?}", done[0].id);
    }
    // Failing twice is failing once.
    served.fail_now();
    drop(served);

    match Store::open(disk.clone(), env.options()) {
        Err(StoreError::Log(LogError::MarkedFailed)) => {}
        other => panic!("the next open: {:?}", other.map(|_| ())),
    }
}

/// An open that is told the acknowledged position counts the
/// operations the log holds beyond it: what the life before appended
/// whole and never acknowledged. Told nothing, or a position recorded
/// for another store, it counts none.
#[test]
fn an_open_counts_the_operations_beyond_the_acknowledged_position() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        if store.recovered_unacknowledged() != 0 {
            return Err("a new store counts operations".into());
        }
        // A new log holds no operation; its bytes are the log's own
        // records (the mark a recovery or a creation leaves).
        if store.replayed().1 != 0 {
            return Err(format!("a new store replayed {:?}", store.replayed()));
        }
        write(&mut store, 0, 1, 1)?;
        let first = store.acknowledged();
        write(&mut store, 1, 1, 2)?;
        let second = store.acknowledged();
        write(&mut store, 2, 1, 3)?;
        let last = store.acknowledged();
        let identity = store.identity();
        drop(store);
        disk.kill();
        for (must_reach, recorded_for, want) in [
            (first, None, 2),
            (second, None, 1),
            (last, None, 0),
            (None, None, 0),
            (first, Some(identity), 2),
            (first, Some(*b"some other store"), 0),
        ] {
            let options = OpenOptions {
                must_reach,
                must_reach_identity: recorded_for,
                ..env.options()
            };
            let (store, _) = Store::open(disk.clone(), options).map_err(|e| e.to_string())?;
            let got = store.recovered_unacknowledged();
            let (bytes, records) = store.replayed();
            if records != 3 || bytes == 0 {
                return Err(format!(
                    "replayed {records} records in {bytes} bytes, not 3"
                ));
            }
            drop(store);
            disk.kill();
            if got != want {
                return Err(format!(
                    "must reach {must_reach:?} for {recorded_for:?}: counted {got}, not {want}"
                ));
            }
        }
        Ok(())
    }
    holds(scenario);
}

// ---------------------------------------------------------------------
// The bounded step of a checkpoint.

fn step(store: &mut SimStore, bound: StepBound) -> Result<Step, String> {
    store
        .checkpoint_step(bound)
        .map_err(|e| format!("checkpoint step: {e}"))
}

/// A step takes at most its bound of records and of bytes, at least
/// one record when there is one, a record that alone exceeds the bound
/// alone, and says whether more is left. With nothing to take it does
/// nothing and leaves the log's start where it was. After every step
/// every block reads as it was written.
#[test]
fn a_checkpoint_step_keeps_to_its_bound() {
    fn scenario(env: &Env) -> Outcome {
        let (_disk, mut store) = env.fresh()?;
        // Ten records of one block, then three of four blocks, then
        // one of eight.
        for n in 0..10u64 {
            write(&mut store, n * 3, 1, n as u8 + 1)?;
        }
        for n in 0..3u64 {
            write(&mut store, 100 + n * 8, 4, n as u8 + 20)?;
        }
        write(&mut store, 200, 8, 30)?;
        flush(&mut store)?;
        let check = |store: &mut SimStore, what: &str| -> Outcome {
            for n in 0..10u64 {
                expect(store, n * 3, 1, n as u8 + 1, what)?;
                expect(store, n * 3 + 1, 2, 0, what)?;
            }
            for n in 0..3u64 {
                expect(store, 100 + n * 8, 4, n as u8 + 20, what)?;
            }
            expect(store, 200, 8, 30, what)
        };
        // By records: 3, 3, 3, then the tenth and two of four blocks.
        let by_records = StepBound {
            payload: u64::MAX,
            records: 3,
        };
        let mut start = store.log_start();
        for round in 0..3 {
            let done = step(&mut store, by_records)?;
            if (done.records, done.payload, done.more) != (3, 3 * BLOCK, true)
                || done.start <= start
            {
                return Err(format!("round {round} by records: {done:?} from {start:?}"));
            }
            if done.start != store.log_start() {
                return Err(format!(
                    "the step says {done:?}, the log {:?}",
                    store.log_start()
                ));
            }
            start = done.start;
            check(&mut store, "after a step by records")?;
        }
        // By bytes: the tenth block and one record of four blocks fit
        // in 20 KiB, a second of four does not.
        let by_bytes = StepBound {
            payload: 5 * BLOCK,
            records: u64::MAX,
        };
        let done = step(&mut store, by_bytes)?;
        if (done.records, done.payload, done.more) != (2, 5 * BLOCK, true) {
            return Err(format!("by bytes: {done:?}"));
        }
        check(&mut store, "after a step by bytes")?;
        // A bound smaller than any record: one record a step, the
        // eight blocks alone and last.
        let tiny = StepBound {
            payload: 1,
            records: u64::MAX,
        };
        for (blocks, more) in [(4, true), (4, true), (8, false)] {
            let done = step(&mut store, tiny)?;
            if (done.records, done.payload, done.more) != (1, blocks * BLOCK, more) {
                return Err(format!("a record beyond the bound: {done:?}"));
            }
            check(&mut store, "after a step of one record")?;
        }
        let end = store.log_start();
        let done = step(&mut store, by_records)?;
        if (done.records, done.more, done.start) != (0, false, end) {
            return Err(format!("with nothing to take: {done:?}"));
        }
        drop(store);
        Ok(())
    }
    holds(scenario);
}

/// A step takes only what is durable and never makes anything durable
/// itself: records since the last flush stay until the log is synced,
/// which `sync_log` does without being a request. And it stops at the
/// hold.
#[test]
fn a_checkpoint_step_takes_only_durable_records_below_the_hold() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 0, 1, 1)?;
        write(&mut store, 8, 1, 2)?;
        flush(&mut store)?;
        let durable = store.log_durable();
        write(&mut store, 16, 1, 3)?;
        let all = StepBound {
            payload: u64::MAX,
            records: u64::MAX,
        };
        let done = step(&mut store, all)?;
        if (done.records, done.more) != (2, false) || done.start > durable {
            return Err(format!("with one record not durable: {done:?}"));
        }
        let syncs = disk.stats().syncs;
        if step(&mut store, all)?.records != 0 || disk.stats().syncs != syncs {
            return Err("a step with nothing durable to take did something".into());
        }
        store.sync_log().map_err(|e| e.to_string())?;
        if store.log_durable() != store.position() {
            return Err("sync_log did not make the log durable".into());
        }
        // The hold in front of the record: nothing to take.
        let held = store.log_start();
        store.set_hold(Some(held)).map_err(|e| format!("{e:?}"))?;
        if step(&mut store, all)?.records != 0 {
            return Err("a step went past the hold".into());
        }
        store.set_hold(None).map_err(|e| format!("{e:?}"))?;
        let done = step(&mut store, all)?;
        if (done.records, done.more) != (1, false) {
            return Err(format!("after the sync: {done:?}"));
        }
        expect(&mut store, 0, 1, 1, "first")?;
        expect(&mut store, 8, 1, 2, "second")?;
        expect(&mut store, 16, 1, 3, "third")
    }
    holds(scenario);
}

/// A step reads the log as far as it takes and no further: its work
/// does not grow with the log behind it.
#[test]
fn a_checkpoint_step_reads_only_what_it_takes() {
    let env = environments(0).remove(0);
    let (_disk, mut store) = env.fresh().expect("a store");
    for n in 0..400u64 {
        write(&mut store, n % 64, 1, n as u8).expect("write");
    }
    flush(&mut store).expect("flush");
    let bound = StepBound {
        payload: u64::MAX,
        records: 4,
    };
    let before = store.core.log.walked();
    let done = step(&mut store, bound).expect("step");
    assert_eq!(done.records, 4);
    let walked = store.core.log.walked() - before;
    // The step's own records and the one it stopped at, once to choose
    // them and again to save neighbours, to apply them and to move the
    // start: a few times five, never the 400 of the log.
    assert!(walked <= 40, "a step of 4 records walked {walked}");
}

/// A discard is a record of the bound like any other: after one write
/// and 5000 discards a step of one record takes one record, the log
/// still ends where it did, and the step walks a few records and not
/// the log. Steps of the daemon's bound work the log off in about five
/// steps and never take more than the bound's records. A discard
/// counts no bytes: a step writes nothing for it.
#[test]
fn discards_count_as_records_of_a_steps_bound() {
    const DISCARDS: u64 = 5000;
    let env = environments(0).remove(0);
    let (_disk, mut store) = env.fresh().expect("a store");
    write(&mut store, 3, 1, 7).expect("write");
    for n in 0..DISCARDS {
        ok(
            &mut store,
            Op::Discard {
                offset: (8 + n % 32) * BLOCK,
                len: BLOCK,
            },
        )
        .expect("discard");
    }
    flush(&mut store).expect("flush");
    let one = StepBound {
        payload: u64::MAX,
        records: 1,
    };
    let end = store.log_state().end;
    let before = store.core.log.walked();
    let done = step(&mut store, one).expect("step");
    let walked = store.core.log.walked() - before;
    assert_eq!((done.records, done.payload), (1, BLOCK), "the write");
    assert!(done.more);
    assert!(done.start.0 < end, "a step of one record took the log");
    assert!(walked <= 10, "a step of one record walked {walked}");
    // The next one is a discard: a record, and no bytes.
    let before = store.core.log.walked();
    let done = step(&mut store, one).expect("step");
    let walked = store.core.log.walked() - before;
    assert_eq!((done.records, done.payload), (1, 0), "a discard");
    assert!(walked <= 10, "a step of one discard walked {walked}");
    // A bound of no bytes at all still takes discards, up to its
    // records.
    let none = StepBound {
        payload: 0,
        records: 100,
    };
    let done = step(&mut store, none).expect("step");
    assert_eq!((done.records, done.payload), (100, 0));
    let mut steps = 0;
    loop {
        let before = store.core.log.walked();
        let done = step(&mut store, StepBound::DAEMON).expect("step");
        let walked = store.core.log.walked() - before;
        assert!(done.records <= StepBound::DAEMON.records, "{done:?}");
        assert!(
            walked <= 5 * (StepBound::DAEMON.records + 1),
            "a step of {} records walked {walked}",
            done.records
        );
        steps += 1;
        if !done.more {
            break;
        }
    }
    assert_eq!(steps, (DISCARDS - 101).div_ceil(StepBound::DAEMON.records));
    let state = store.log_state();
    assert_eq!(state.live(), 0, "{state:?}");
    expect(&mut store, 3, 1, 7, "after the steps").expect("the write");
}

/// What the worker shows of the log is of one moment: a reader never
/// sees a start beyond the end, also when each state published starts
/// beyond the end of the one before, as after a step that took a log
/// which was longer than the one last shown. And the live size of a
/// state that no store shows, with its start beyond its end, is zero
/// and no overflow.
#[test]
fn the_log_shown_never_starts_beyond_its_end() {
    const STATES: u64 = 200_000;
    let shown = Shown::default();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for n in 1..=STATES {
                shown.set(LogState {
                    start: 10 * n,
                    end: 10 * n + 5,
                    ..LogState::default()
                });
            }
        });
        loop {
            let state = shown.state();
            assert!(state.start <= state.end, "{state:?}");
            assert!(state.live() <= 5, "{state:?}");
            if state.start == 10 * STATES {
                break;
            }
        }
    });
    let torn = LogState {
        start: 20,
        end: 15,
        ..LogState::default()
    };
    assert_eq!(torn.live(), 0);
}

/// A checkpoint done in steps can be cut anywhere, as one done in one
/// call: the volume is filled and checkpointed, one block in each of
/// its 256 write units is written again, and the steps that apply them
/// are stopped after a number of writes and syncs with a power cut;
/// the open that follows is stopped and cut as well. After the next
/// open every block reads as what was flushed, and steps taken after
/// that finish the work.
#[test]
fn a_checkpoint_in_steps_can_be_cut_anywhere() {
    fn scenario(env: &Env) -> Outcome {
        let bound = StepBound {
            payload: 16 * BLOCK,
            records: 16,
        };
        for stop in (0..30).chain([44, 61, 90, 150, 300, 700]) {
            let (disk, mut store) = env.fresh()?;
            for mib in 0..4 {
                write(&mut store, mib * 256, 256, 9)?;
            }
            flush(&mut store)?;
            checkpoint(&mut store)?;
            for unit in 0..256u64 {
                write(&mut store, unit * 4 + 1, 1, (unit % 200) as u8 + 10)?;
            }
            flush(&mut store)?;
            disk.halt_after(stop);
            let mut steps = 0;
            let cut_off = loop {
                match store.checkpoint_step(bound) {
                    Err(_) => break true,
                    Ok(done) if !done.more => break false,
                    Ok(done) if done.records > 16 => {
                        return Err(format!("a step took {done:?}"));
                    }
                    Ok(_) => steps += 1,
                }
            };
            if !cut_off && steps < 15 {
                return Err(format!("256 records were taken in {steps} steps of 16"));
            }
            if !cut_off && stop < 3 {
                return Err(format!("the steps were not stopped at {stop}"));
            }
            drop(store);
            let lost = env.lost();
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            disk.set_write_unit(u64::from(env.unit));
            disk.halt_after(stop % 7);
            drop(Store::open(disk.clone(), env.options()));
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            disk.set_write_unit(u64::from(env.unit));
            let mut store = env.reopen_ack(&disk)?;
            let check = |store: &mut SimStore, when: &str| -> Outcome {
                for unit in 0..256u64 {
                    let what = format!("unit {unit} {when} a cut at {stop}");
                    expect(store, unit * 4, 1, 9, &what)?;
                    expect(store, unit * 4 + 1, 1, (unit % 200) as u8 + 10, &what)?;
                    expect(store, unit * 4 + 2, 2, 9, &what)?;
                }
                Ok(())
            };
            check(&mut store, "after")?;
            // The steps go on where the cut left the log.
            while step(&mut store, bound)?.more {}
            if store.log_start() < store.log_durable() {
                return Err("steps after the restart left durable records".into());
            }
            check(&mut store, "and the remaining steps after")?;
        }
        Ok(())
    }
    // The step keeps the checkpoint's order. Two of its defects show
    // here as they do for the checkpoint in one call: the log given up
    // before the home image is durable, and neighbours not saved.
    caught(scenario, Mutants::RECLAIM_BEFORE_HOME_SYNC);
    caught_where(scenario, Mutants::NO_PREIMAGES, garbled_neighbours);
}

// ---------------------------------------------------------------------
// The rule of the marks.

/// Marks for a volume of the tests' size: steps begin at 512 KiB of
/// log and end below 256 KiB; a step takes at most 128 KiB or 8
/// records, twice what the test's request adds.
const MARKS: Marks = Marks {
    high: 512 << 10,
    low: 256 << 10,
    step: StepBound {
        payload: 128 << 10,
        records: 8,
    },
};

/// How far one request of 16 blocks moves the log's end at most: its
/// record (the data, the header, the padding to the write unit), and,
/// where the record does not fit into what is left of a segment, the
/// padding to the next segment, which is less than one more record.
const ONE_REQUEST: u64 = 2 * ((64 << 10) + (32 << 10));

/// Under a write load that lasts and never flushes, the store taking
/// its step after every request, the log's live size stays below the
/// high mark and one request: the log stops growing, whatever the
/// load. Steps begin only at the high mark and end below the low one,
/// the store syncs the log itself to have something durable to take,
/// and every block reads as written at the end and after a kill.
#[test]
fn under_a_lasting_write_load_the_log_stays_at_the_high_mark() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        store.set_marks(Some(MARKS));
        let mut newest = [0u8; 64];
        // The rule as the test follows it: working from the first step
        // until the log is below the low mark.
        let (mut largest, mut rests, mut working) = (0, 0u64, false);
        for n in 0..300u64 {
            let slot = (n * 7) % 64;
            let byte = (n % 250) as u8 + 1;
            let before = store.log_state();
            write(&mut store, slot * 16, 16, byte)?;
            newest[slot as usize] = byte;
            let after = store.log_state();
            largest = largest.max(after.live());
            if after.live() > MARKS.high + ONE_REQUEST {
                return Err(format!(
                    "after write {n} the log holds {} bytes",
                    after.live()
                ));
            }
            let stepped = after.steps - before.steps;
            if stepped > 1 {
                return Err(format!("write {n} was followed by {stepped} steps"));
            }
            if stepped == 1 && after.start <= before.start {
                return Err(format!(
                    "write {n}: a step that took nothing, {before:?} to {after:?}"
                ));
            }
            // A step while the rule rested: the write must have taken
            // the log to the high mark.
            if stepped == 1 && !working && before.live() + ONE_REQUEST < MARKS.high {
                return Err(format!(
                    "a step at write {n} with {} bytes of log before it",
                    before.live()
                ));
            }
            if stepped == 0 && working {
                return Err(format!(
                    "no step at write {n}, above the low mark: {after:?}"
                ));
            }
            working = stepped == 1 || working;
            if after.live() < MARKS.low {
                if working {
                    rests += 1;
                }
                working = false;
            }
        }
        let state = store.log_state();
        // `largest` is seen after a step: one step below the high mark.
        if state.steps < 20 || largest + MARKS.step.payload < MARKS.high || rests < 2 {
            return Err(format!(
                "the load did not exercise the rule: {state:?}, largest {largest}, it came to rest {rests} times"
            ));
        }
        let check = |store: &mut SimStore, when: &str| -> Outcome {
            for (slot, byte) in newest.iter().enumerate() {
                expect(store, slot as u64 * 16, 16, *byte, when)?;
            }
            Ok(())
        };
        check(&mut store, "under the load")?;
        // What the steps took is in the home image, the rest in the
        // log: a kill loses nothing.
        drop(store);
        disk.kill();
        let mut store = env.reopen_ack(&disk)?;
        check(&mut store, "after a kill")
    }
    holds(scenario);
}

/// Without marks the store never checkpoints by itself, and a hold in
/// front of the log stops the rule without a sync or a spin.
#[test]
fn the_rule_rests_without_marks_and_behind_a_hold() {
    let env = environments(0).remove(0);
    let (disk, mut store) = env.fresh().expect("a store");
    for n in 0..12u64 {
        write(&mut store, n * 16, 16, 1).expect("write");
    }
    assert!(store.log_state().live() > MARKS.high);
    assert_eq!(store.tend(), Ok(false), "no marks, no step");
    let start = store.log_start();
    store.set_hold(Some(start)).expect("a hold at the start");
    store.set_marks(Some(MARKS));
    let syncs = disk.stats().syncs;
    assert_eq!(store.tend(), Ok(false), "nothing below the hold");
    assert_eq!(disk.stats().syncs, syncs, "and no sync that could not help");
    store.set_hold(None).expect("released");
    assert_eq!(store.tend(), Ok(true));
    assert_eq!(store.log_state().steps, 1);
}

/// A step that fails fails the volume: the rule returns the failure,
/// the store serves nothing more, and the next open finds it marked.
#[test]
fn a_step_that_fails_fails_the_volume() {
    let env = environments(0).remove(0);
    let (disk, mut store) = env.fresh().expect("a store");
    for n in 0..12u64 {
        write(&mut store, n * 16, 16, 1).expect("write");
    }
    // The marks only now, so that the first step is the one that fails.
    store.set_marks(Some(MARKS));
    disk.fail_sync_of(HOME_NAME);
    assert_eq!(store.tend(), Err(BlkError::Failed));
    assert!(store.is_failed());
    assert_eq!(store.tend(), Ok(false), "a failed volume is left alone");
    let done = run(&mut store, Op::Flush).expect("answered");
    assert_eq!(done.result, Err(BlkError::Failed));
    drop(store);
    disk.clear_faults();
    assert!(Store::open(disk.clone(), env.options()).is_err());
}

/// What the worker shows of its steps is of one moment: a reader that
/// looks while steps begin and end never sees a step that has ended
/// as under way, nor the count go back. Seen as one number (twice the
/// steps, plus one while a step is under way) the state only grows.
#[test]
fn the_steps_shown_never_go_back() {
    const STEPS: u64 = 200_000;
    let shown = Shown::default();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for step in 0..STEPS {
                shown.begin();
                shown.set(LogState {
                    steps: step + 1,
                    ..LogState::default()
                });
            }
        });
        let mut last = 0;
        loop {
            let state = shown.state();
            let now = state.steps * 2 + u64::from(state.stepping);
            assert!(
                now >= last,
                "after {last}: {} steps, under way {}",
                state.steps,
                state.stepping
            );
            last = now;
            if now == STEPS * 2 {
                break;
            }
        }
    });
    let state = shown.state();
    assert_eq!((state.steps, state.stepping), (STEPS, false));
}

/// Wait until a step is at the point of its pause: the hook has made
/// the file that says so. "Under way" alone is earlier than that.
fn at_the_point(gate: &std::path::Path, what: &str) {
    let reached = StepPause::reached(gate);
    let began = std::time::Instant::now();
    while !reached.exists() {
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "{what}: the step did not reach its point"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The served store applies the rule on its worker: after every
/// request, and with no request at all until the log is below the low
/// mark. Requests are answered between the steps, the owner is woken
/// when a step begins and ends, and the log's state can be read while
/// the worker works.
///
/// Nothing asserted here rests on how the worker's steps fall between
/// the test's requests. Where the 200 writes end the rule may be at
/// work or at rest (at rest the log is anywhere below the high mark),
/// so the idle part first makes the phase certain: a hold at the log's
/// start, writes behind it until the log is above the high mark, and
/// the hold released with no request. From there the worker steps to
/// below the low mark by itself, and there it rests for certain. The
/// same from a worker brought to rest first is
/// `a_hold_that_goes_or_moves_lets_the_idle_worker_checkpoint`.
#[test]
fn a_served_store_steps_between_requests_and_when_idle() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let env = environments(0)[0];
    let (disk, mut store) = env.fresh().unwrap();
    store.set_marks(Some(MARKS));
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let mut served = store
        .serve_with(Some(Waker::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })))
        .unwrap();
    assert_eq!(served.log_state().steps, 0);
    let mut done = Vec::new();
    let mut largest = 0;
    for n in 0..200u64 {
        done.clear();
        served
            .submit(Request {
                id: RequestId(n),
                op: Op::Write {
                    offset: (n % 64) * 16 * BLOCK,
                    data: Buf::filled(16, (n % 250) as u8 + 1),
                },
            })
            .unwrap();
        assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
        assert_eq!(done[0].result, Ok(()), "write {n}");
        largest = largest.max(served.log_state().live());
    }
    // With a step after every request the log is never seen more than
    // one request above the mark; steps that fall between the requests
    // as well only make it shorter.
    assert!(
        largest <= MARKS.high + 2 * ONE_REQUEST,
        "the log reached {largest} bytes"
    );
    // 200 requests of 64 KiB against a high mark of 512 KiB and steps
    // of at most 128 KiB: whatever the interleaving, many steps.
    let after_load = served.log_state();
    assert!(after_load.steps >= 10, "{after_load:?}");

    // The phase made certain. A hold at the log's start: a step may
    // move the start between the look and the hold, then the hold is
    // refused and tried again.
    let began = std::time::Instant::now();
    loop {
        let start = Lsn(served.log_state().start);
        if served.set_hold(Some(start)).is_ok() {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "no hold at the log's start"
        );
    }
    // Twelve writes behind the hold are more than the high mark by
    // themselves. Each rewrites what its slot holds already.
    let newest_of = |slot: u64| (slot + if slot < 8 { 192 } else { 128 }) % 250 + 1;
    for slot in 0..12u64 {
        done.clear();
        served
            .submit(Request {
                id: RequestId(500 + slot),
                op: Op::Write {
                    offset: slot * 16 * BLOCK,
                    data: Buf::filled(16, newest_of(slot) as u8),
                },
            })
            .unwrap();
        assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
        assert_eq!(done[0].result, Ok(()), "write {slot} behind the hold");
    }
    let held = served.log_state();
    assert!(held.live() >= MARKS.high, "{held:?}");
    served.set_hold(None).expect("released");
    // No request any more: from above the high mark the worker goes
    // on until the low mark.
    let began = std::time::Instant::now();
    let state = loop {
        let state = served.log_state();
        if state.live() < MARKS.low && !state.stepping {
            break state;
        }
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "idle, the log stayed at {state:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(state.steps > held.steps, "{state:?} after {held:?}");
    // 212 completions, and two wake-ups a step. The count of steps is
    // shown a moment before the wake-up for its end, so the wake-ups
    // are waited for.
    let began = std::time::Instant::now();
    while u64::from(calls.load(Ordering::SeqCst)) < 212 + 2 * state.steps {
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "{} wake-ups for 212 requests and {} steps",
            calls.load(Ordering::SeqCst),
            state.steps
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // It rests there, for certain: below the low mark, with no step
    // under way and no request, the rule takes nothing until the high
    // mark. No step without a reason.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(served.log_state().steps, state.steps);
    for slot in 0..64u64 {
        done.clear();
        served
            .submit(Request {
                id: RequestId(1000 + slot),
                op: Op::Read {
                    offset: slot * 16 * BLOCK,
                    buf: Buf::zeroed(16),
                },
            })
            .unwrap();
        assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
        // The last write to the slot: of 200, number slot + 192 or + 128.
        let newest = newest_of(slot);
        let buf = done[0].buf.as_ref().expect("the read's buffer");
        assert!(
            buf.iter().all(|b| u64::from(*b) == newest),
            "slot {slot} does not hold its newest write"
        );
    }
    drop(served);
    disk.kill();
    env.reopen_ack(&disk).expect("opens after a kill");
}

/// A kill, or a power cut, inside a step of the served store, through
/// the worker's own rule and at each of the step's points: the step
/// waits there (the test hook), the store shows that a step is under
/// way and has not counted it, and the process is lost there. The
/// next open finds every write it must (every acknowledged one after a
/// kill; every flushed one after a power cut, where each write was
/// flushed), the rule goes on where the log was left, and the content
/// stays right.
fn lost_inside_a_step(env: &Env, point: StepPoint, power_cut: bool) {
    static NUMBER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let what = format!("{point:?}, power cut {power_cut}, in {env:?}");
    let gate = std::env::temp_dir().join(format!(
        "skyzfs-step-gate-{}-{}",
        std::process::id(),
        NUMBER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    let (disk, mut store) = env.fresh().unwrap();
    store.set_marks(Some(MARKS));
    store.set_step_pause_at(StepPause::Until(gate.clone()), point);
    let mut served = store.serve().unwrap();
    let mut done = Vec::new();
    let mut newest = [0u8; 64];
    let mut n = 0u64;
    let mut ids = 0u64;
    // Writes until the worker is inside its first step. It answers
    // the request that reached the mark and then waits in the step.
    //
    // Whether a step follows an answer is read from the log, not
    // waited for: the state shown when a request is answered has that
    // request, and the rule takes its first step exactly when the
    // log's live size is at the high mark. So after each answer the
    // test knows whether to wait for the step to show or to go on,
    // however the worker is scheduled. (A sleep in its place would,
    // when too short, send one more request behind a step that waits
    // at its point for the test, and that request is never answered.)
    let step_follows = |served: &Served| {
        let state = served.log_state();
        if state.live() < MARKS.high && !state.stepping {
            return false;
        }
        let began = std::time::Instant::now();
        while !served.log_state().stepping {
            assert!(
                began.elapsed() < Duration::from_secs(20),
                "{what}: at the high mark and no step: {:?}",
                served.log_state()
            );
            std::thread::yield_now();
        }
        true
    };
    while !served.log_state().stepping {
        assert!(n < 64, "no step after {n} writes: {:?}", served.log_state());
        let (slot, byte) = (n % 64, n as u8 + 1);
        let mut ops = vec![Op::Write {
            offset: slot * 16 * BLOCK,
            data: Buf::filled(16, byte),
        }];
        if power_cut {
            ops.push(Op::Flush);
        }
        for op in ops {
            done.clear();
            served
                .submit(Request {
                    id: RequestId(ids),
                    op,
                })
                .unwrap();
            ids += 1;
            assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
            assert_eq!(done[0].result, Ok(()), "{what}");
            if step_follows(&served) {
                break;
            }
        }
        // Counted only if its flush was answered too, where there is
        // one: a write whose flush the step overtook is not promised
        // across a power cut, and reads as it was or as it is.
        if !power_cut || !served.log_state().stepping || done[0].buf.is_none() {
            newest[slot as usize] = byte;
        }
        n += 1;
    }
    // The step is under way; the loss comes when it is at the point.
    at_the_point(&gate, &what);
    let state = served.log_state();
    assert!(state.stepping, "{what}");
    assert_eq!(state.steps, 0, "{what}: the step has not ended");
    if power_cut {
        let lost = env.lost();
        disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        disk.set_write_unit(u64::from(env.unit));
    } else {
        disk.kill();
    }
    std::fs::write(&gate, b"").unwrap();
    drop(served);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    let mut store = env
        .reopen_ack(&disk)
        .unwrap_or_else(|why| panic!("{what}: {why}"));
    let check = |store: &mut SimStore, when: &str| {
        for (slot, byte) in newest.iter().enumerate() {
            let slot = slot as u64;
            // The last write before the loss may be the one whose
            // flush was not answered: its slot holds it or what was
            // there before.
            if power_cut && slot == (n - 1) % 64 {
                let got = read(store, slot * 16, 16).unwrap();
                let (old, new) = (*byte, (n - 1) as u8 + 1);
                assert!(
                    got.iter().all(|b| *b == old) || got.iter().all(|b| *b == new),
                    "{what}: slot {slot} {when} is neither {old} nor {new}"
                );
                continue;
            }
            expect(store, slot * 16, 16, *byte, when).unwrap_or_else(|why| panic!("{what}: {why}"));
        }
    };
    check(&mut store, "after the loss inside a step");
    store.set_marks(Some(MARKS));
    let found = store.log_state();
    let above = found.live() >= MARKS.high;
    let mut steps = 0;
    while store.tend().unwrap() {
        steps += 1;
    }
    // The rule goes on exactly when the log that survived is still
    // at the high mark. It is not after a loss behind the log's new
    // start being durable (the step is done), nor after a power cut
    // that took the unflushed record which had reached the mark.
    assert_eq!(
        steps > 0,
        above,
        "{what}: the rule after the restart, with the log at {found:?}"
    );
    if !power_cut
        && matches!(
            point,
            StepPoint::Saved | StepPoint::Written | StepPoint::Synced
        )
    {
        assert!(
            above,
            "{what}: a kill before the new start kept the log at {found:?}"
        );
    }
    check(&mut store, "after the steps that followed");
}

#[test]
fn a_kill_inside_a_step_of_the_worker_loses_nothing() {
    for env in environments(0) {
        for point in [
            StepPoint::Saved,
            StepPoint::Written,
            StepPoint::Synced,
            StepPoint::StartWritten,
            StepPoint::StartDurable,
        ] {
            lost_inside_a_step(&env, point, false);
        }
    }
}

#[test]
fn a_power_cut_inside_a_step_of_the_worker_loses_nothing_that_was_flushed() {
    for env in environments(0) {
        for point in [
            StepPoint::Saved,
            StepPoint::Written,
            StepPoint::Synced,
            StepPoint::StartWritten,
            StepPoint::StartDurable,
        ] {
            lost_inside_a_step(&env, point, true);
        }
    }
}

/// A record that alone exceeds the daemon's bound is a step of its
/// own: a write-zeroes of 12 MiB between two small writes, under the
/// bound of 8 MiB. The step before it ends in front of it, it is taken
/// alone, and the zeros are in the home image afterwards, over what
/// was there, also after a restart.
#[test]
fn a_write_zeroes_beyond_the_daemons_bound_is_a_step_of_its_own() {
    const ZEROED: u64 = 12 << 20;
    for env in environments(0) {
        let config = StoreConfig {
            size_mib: 32,
            ..env.config()
        };
        let disk = SimDisk::new(7);
        disk.set_write_unit(u64::from(env.unit));
        let key = Key(*b"a key for tests!");
        let mut store = Store::create_keyed(disk.clone(), &config, key, env.options()).unwrap();
        // Something for the zeros to replace, in the home image.
        let (first, last) = (1000u64, 1000 + ZEROED / BLOCK - 16);
        write(&mut store, first, 16, 5).unwrap();
        write(&mut store, last, 16, 6).unwrap();
        write(&mut store, 300, 4, 7).unwrap();
        flush(&mut store).unwrap();
        checkpoint(&mut store).unwrap();

        write(&mut store, 10, 1, 1).unwrap();
        ok(
            &mut store,
            Op::WriteZeroes {
                offset: first * BLOCK,
                len: ZEROED,
            },
        )
        .unwrap();
        write(&mut store, 20, 1, 2).unwrap();
        flush(&mut store).unwrap();
        for (records, payload, more) in [(1, BLOCK, true), (1, ZEROED, true), (1, BLOCK, false)] {
            let done = step(&mut store, StepBound::DAEMON).unwrap();
            assert_eq!(
                (done.records, done.payload, done.more),
                (records, payload, more),
                "{env:?}"
            );
        }
        let check = |store: &mut SimStore, when: &str| {
            for (block, count, byte) in [
                (first, 16, 0),
                (last, 16, 0),
                (300, 4, 7),
                (10, 1, 1),
                (20, 1, 2),
            ] {
                expect(store, block, count, byte, when)
                    .unwrap_or_else(|why| panic!("{env:?}: {why}"));
            }
        };
        check(&mut store, "after the steps");
        drop(store);
        disk.kill();
        let mut store = Store::open(disk.clone(), env.options()).unwrap().0;
        check(&mut store, "after a restart");
    }
}

/// A loss between a step's sync of the home image and the log's new
/// start, with a newer record for the same block further on in the
/// log, beyond the step's batch: the home image then holds the block's
/// older content, durably, and the log starts where it did. The next
/// open reads the newer content, and still does after the step is
/// applied again and after the steps that follow.
///
/// The loss is made in two ways, and each half asserts that the log
/// starts where it did before the step, which is what makes the step
/// be applied again:
///
/// - a kill: the store's worker takes the step and is held by the hook
///   after the home image is synced, before the new start is written,
///   and the process is lost there;
/// - a power cut: the step runs to the sync of the log's superblock,
///   which fails, and the power cut takes the new start that was
///   written and not synced.
#[test]
fn a_newer_record_beyond_a_steps_batch_wins_after_a_loss_between_its_sync_and_its_new_start() {
    static NUMBER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    fn scenario(env: &Env) -> Outcome {
        let one = StepBound {
            payload: u64::MAX,
            records: 1,
        };
        for power_cut in [false, true] {
            let (disk, mut store) = env.fresh()?;
            write(&mut store, 5, 1, 1)?;
            write(&mut store, 9, 1, 3)?;
            write(&mut store, 5, 1, 2)?;
            flush(&mut store)?;
            let before = store.log_state();
            if power_cut {
                // The step takes the first record alone, writes block
                // 5 as 1 into the home image and syncs it; the log's
                // new start cannot be made durable.
                disk.fail_sync_of(crate::log::SUPER_NAME);
                if store.checkpoint_step(one).is_ok() {
                    return Err("the step ended although its new start was not durable".into());
                }
                drop(store);
                // The fault stood for the loss; the restart has none.
                disk.clear_faults();
                let lost = env.lost();
                disk.power_cut(BySector(move |_: Option<&str>, _| lost));
                disk.set_write_unit(u64::from(env.unit));
            } else {
                // The worker finds the log at the high mark and takes
                // its first step, of one record, before any request.
                let gate = std::env::temp_dir().join(format!(
                    "skyzfs-newer-gate-{}-{}",
                    std::process::id(),
                    NUMBER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                ));
                let _ = std::fs::remove_file(&gate);
                let _ = std::fs::remove_file(StepPause::reached(&gate));
                store.set_marks(Some(Marks {
                    high: before.live(),
                    low: 1,
                    step: one,
                }));
                store.set_step_pause_at(StepPause::Until(gate.clone()), StepPoint::Synced);
                let served = store.serve().map_err(|e| e.to_string())?;
                at_the_point(&gate, "the newer record, the kill");
                if served.log_state().steps != 0 {
                    return Err("the step ended before the kill".into());
                }
                // Held behind the sync of the home image: lost there.
                disk.kill();
                std::fs::write(&gate, b"").map_err(|e| e.to_string())?;
                drop(served);
                let _ = std::fs::remove_file(&gate);
                let _ = std::fs::remove_file(StepPause::reached(&gate));
            }
            let mut store = env.reopen_ack(&disk)?;
            let found = store.log_state();
            if found.start != before.start {
                return Err(format!(
                    "power cut {power_cut}: the log started at {} and starts at {} after \
                     the loss: the step is not applied again",
                    before.start, found.start
                ));
            }
            let check = |store: &mut SimStore, when: &str| -> Outcome {
                let when = format!("{when} (power cut {power_cut})");
                expect(store, 5, 1, 2, &when)?;
                expect(store, 9, 1, 3, &when)?;
                expect(store, 6, 3, 0, &when)
            };
            check(&mut store, "after the loss")?;
            let again = step(&mut store, one)?;
            if again.records != 1 || again.start.0 <= before.start {
                return Err(format!("the step applied again took {again:?}"));
            }
            check(&mut store, "after the step applied again")?;
            while step(&mut store, one)?.more {}
            check(&mut store, "after the steps that followed")?;
            drop(store);
            let mut store = env.reopen(&disk)?;
            check(&mut store, "after another restart")?;
        }
        Ok(())
    }
    holds(scenario);
}

/// A long log that a start found is worked off once the store is
/// served, with no request at all: the worker asks the rule before it
/// waits for its first command.
#[test]
fn a_long_log_found_at_the_start_is_worked_off_without_a_request() {
    let env = environments(0)[0];
    let (disk, mut store) = env.fresh().unwrap();
    for n in 0..12u64 {
        write(&mut store, n * 16, 16, n as u8 + 1).unwrap();
    }
    drop(store);
    disk.kill();
    let mut store = env.reopen_ack(&disk).unwrap();
    assert!(store.log_state().live() > MARKS.high);
    store.set_marks(Some(MARKS));
    let served = store.serve().unwrap();
    let began = std::time::Instant::now();
    loop {
        let state = served.log_state();
        if state.live() < MARKS.low && !state.stepping {
            assert!(state.steps > 0);
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "with no request the log stayed at {state:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The owner is woken for a completion only after the log state that
/// has the completion's record is shown: a waker that looks at the
/// shown state at each wake, on the worker's own thread, finds the
/// log's end beyond where it was before the write, at the first wake
/// after the write. And the caller of a checkpoint finds the log as
/// the checkpoint left it when the call returns. What is asserted does
/// not rest on timing: the state is read by the waker, on the worker's
/// thread, between the worker's send and its next look at its queue,
/// and the test waits for that wake before it reads what was seen.
#[test]
fn the_log_shown_has_a_completion_when_the_owner_is_woken_for_it() {
    use std::sync::Mutex;
    let env = environments(0)[0];
    let (_disk, store) = env.fresh().unwrap();
    let shown: Arc<Mutex<Option<Arc<Shown>>>> = Arc::default();
    let seen: Arc<Mutex<Vec<LogState>>> = Arc::default();
    let waker = Waker::new({
        let (shown, seen) = (shown.clone(), seen.clone());
        move || {
            if let Some(shown) = shown.lock().unwrap().as_ref() {
                seen.lock().unwrap().push(shown.state());
            }
        }
    });
    let mut served = store.serve_with(Some(waker)).unwrap();
    *shown.lock().unwrap() = Some(served.log.clone());
    let before = served.log_state();
    for (id, block) in [(1, 0), (2, 5)] {
        let end = served.log_state().end;
        let wakes = seen.lock().unwrap().len();
        served
            .submit(Request {
                id: RequestId(id),
                op: Op::Write {
                    offset: block * BLOCK,
                    data: Buf::filled(1, 9),
                },
            })
            .unwrap();
        let mut done = Vec::new();
        assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
        // The completion can be taken before the worker has run the
        // waker: wait for this request's wake, whatever the scheduling.
        let asked = std::time::Instant::now();
        while seen.lock().unwrap().len() <= wakes {
            assert!(
                asked.elapsed() < Duration::from_secs(10),
                "write {id}: the owner was never woken for it"
            );
            std::thread::yield_now();
        }
        let at_the_wake = seen.lock().unwrap()[wakes];
        assert!(
            at_the_wake.end > end,
            "write {id}: woken with the log shown ending at {}, as before the write ({end})",
            at_the_wake.end
        );
    }
    assert!(served.log_state().end > before.end);
    served
        .submit(Request {
            id: RequestId(3),
            op: Op::Flush,
        })
        .unwrap();
    let mut done = Vec::new();
    assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
    served.checkpoint().unwrap();
    let after = served.log_state();
    assert!(
        after.start > before.start,
        "a checkpoint that has returned is shown: {after:?}, before {before:?}"
    );
}

/// A served store with marks and a hold, twelve writes of 16 blocks
/// and a flush behind it, and the worker at rest: the rule has been
/// asked after the flush and found nothing below the hold.
fn served_at_rest_behind_a_hold(first: u64) -> (Served, Lsn) {
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    store.set_marks(Some(MARKS));
    let start = store.log_start();
    store.set_hold(Some(start)).expect("a hold at the start");
    let mut served = store.serve().unwrap();
    let mut middle = start;
    let run = |served: &mut Served, id: u64, op: Op| {
        served
            .submit(Request {
                id: RequestId(id),
                op,
            })
            .unwrap();
        let mut done = Vec::new();
        assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
        assert_eq!(done[0].result, Ok(()));
    };
    for n in 0..12u64 {
        if n == first {
            middle = served.position().unwrap();
        }
        let op = Op::Write {
            offset: n * 16 * BLOCK,
            data: Buf::filled(16, 1),
        };
        run(&mut served, n, op);
    }
    run(&mut served, 99, Op::Flush);
    // Answered behind the rule's look after the flush: the worker is
    // at rest when this returns.
    served.position().unwrap();
    let state = served.log_state();
    assert!(state.live() > MARKS.high, "{state:?}");
    assert_eq!((state.steps, state.stepping), (0, false), "{state:?}");
    (served, middle)
}

/// The log comes to what `there` asks by itself, with no request.
fn comes_by_itself(served: &Served, what: &str, there: impl Fn(&LogState) -> bool) -> LogState {
    let asked = std::time::Instant::now();
    loop {
        let state = served.log_state();
        if there(&state) && !state.stepping {
            return state;
        }
        assert!(
            asked.elapsed() < Duration::from_secs(10),
            "{what}: not within 10 s with no request: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The worker asks the rule of the marks again when a hold changes,
/// with no request to follow: a hold released above the high mark lets
/// the log come below the low mark by itself, and a hold moved forward
/// lets it come up to the new hold, where the worker rests again.
/// Without that the worker slept on its queue with a long log until
/// the next request.
#[test]
fn a_hold_that_goes_or_moves_lets_the_idle_worker_checkpoint() {
    let (served, _) = served_at_rest_behind_a_hold(0);
    served.set_hold(None).expect("released");
    let state = comes_by_itself(&served, "after the hold was released", |s| {
        s.live() < MARKS.low
    });
    assert!(state.steps > 0, "{state:?}");

    let (served, middle) = served_at_rest_behind_a_hold(6);
    served.set_hold(Some(middle)).expect("moved forward");
    let state = comes_by_itself(&served, "after the hold moved", |s| s.start == middle.0);
    assert!(state.steps > 0, "{state:?}");
    // It rests there: what is left is above the low mark and behind
    // the hold.
    served.position().unwrap();
    let rests = served.log_state();
    assert_eq!(
        (rests.start, rests.stepping),
        (middle.0, false),
        "{rests:?}"
    );
    assert!(rests.live() >= MARKS.low, "{rests:?}");
    assert_eq!(rests.steps, state.steps, "no step beyond the hold");
}

/// What fills the log in the tests of a log whose records are short:
/// write-zeroes of four blocks, eight of which are one step by
/// [`MARKS`] as eight of 1 MiB are one step of the daemon's, or
/// discards, eight of which are one step by the records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Short {
    Zeroes,
    Discards,
}

impl Short {
    /// The `n`-th of them: over blocks 512 and up, four at a time.
    fn op(self, n: u64) -> Op {
        let (offset, len) = ((512 + (n % 128) * 4) * BLOCK, 4 * BLOCK);
        match self {
            Self::Zeroes => Op::WriteZeroes { offset, len },
            Self::Discards => Op::Discard { offset, len },
        }
    }
}

/// A store whose log holds short records up to the high mark of
/// [`MARKS`], with no marks set yet: blocks 512 to 528 were written
/// with nines and are in the home image, and the records zero or
/// discard blocks 512 to 1024 again and again. Returns how many
/// records there are.
fn a_log_of_short_records(env: &Env, short: Short) -> Result<(SimDisk, SimStore, u64), String> {
    let (disk, mut store) = env.fresh()?;
    write(&mut store, 512, 16, 9)?;
    flush(&mut store)?;
    checkpoint(&mut store)?;
    let mut records = 0;
    while store.log_state().live() < MARKS.high {
        ok(&mut store, short.op(records))?;
        records += 1;
    }
    Ok((disk, store, records))
}

/// What blocks 512 to 528 read as after the short records.
fn under_short_records(short: Short) -> u8 {
    match short {
        Short::Zeroes => 0,
        Short::Discards => 9,
    }
}

/// A log that reached the high mark with records that are short in
/// the log (a write-zeroes covers much of the volume with one header,
/// a discard writes nothing) stays at the high mark under writes: the
/// step's bound counts what a step writes into the home image and how
/// many records it walks, so a step takes eight such records, a few
/// hundred bytes or a few KiB of log, and a write adds more than
/// 64 KiB. One step after every request let such a log grow by nearly
/// a request with every request. The rule takes steps until the log is
/// below the high mark before the next request is executed.
#[test]
fn a_log_of_short_records_stays_at_the_high_mark_under_writes() {
    fn scenario(env: &Env, short: Short) -> Outcome {
        let (disk, mut store, records) = a_log_of_short_records(env, short)?;
        store.set_marks(Some(MARKS));
        let mut newest = [0u8; 32];
        let mut steps_of_one_write = 0;
        for n in 0..40u64 {
            let slot = (n * 5) % 32;
            let byte = (n % 250) as u8 + 1;
            let before = store.log_state();
            write(&mut store, slot * 16, 16, byte)?;
            newest[slot as usize] = byte;
            let after = store.log_state();
            steps_of_one_write = steps_of_one_write.max(after.steps - before.steps);
            // After the request and its steps the log is below the
            // high mark again, or within what the request added.
            if after.live() >= MARKS.high + ONE_REQUEST {
                return Err(format!(
                    "after write {n}, behind {records} short records, the log holds {} bytes: {after:?}",
                    after.live()
                ));
            }
        }
        // The load was what the test is about: a write was followed
        // by several steps, because one did not take what it added.
        if steps_of_one_write < 2 {
            return Err(format!(
                "no write needed more than {steps_of_one_write} steps: {:?}",
                store.log_state()
            ));
        }
        let check = |store: &mut SimStore, when: &str| -> Outcome {
            for (slot, byte) in newest.iter().enumerate() {
                expect(store, slot as u64 * 16, 16, *byte, when)?;
            }
            expect(store, 512, 16, under_short_records(short), when)
        };
        check(&mut store, "under the load")?;
        drop(store);
        disk.kill();
        let mut store = env.reopen_ack(&disk)?;
        check(&mut store, "after a kill")
    }
    for short in [Short::Zeroes, Short::Discards] {
        for env in environments(0) {
            if let Err(why) = scenario(&env, short) {
                panic!("{short:?}, {env:?}: {why}");
            }
        }
    }
}

/// The same on a served store, which is where requests wait in a
/// queue while the worker steps: sixteen writes are queued before the
/// worker starts, so that it has a request to execute after every
/// step whatever the scheduling, and three more rounds of sixteen
/// follow. A waker that looks at the log shown at every wake, on the
/// worker's thread, never finds it a request above the high mark. The
/// worker shows the log before each completion's wake, so the largest
/// state is seen, and nothing asserted rests on timing: the bound is
/// an upper one and holds at every moment.
#[test]
fn a_served_log_of_short_records_stays_at_the_high_mark_under_queued_writes() {
    use std::sync::Mutex;
    for (short, env) in [
        (Short::Zeroes, environments(0)[0]),
        (Short::Zeroes, environments(0)[4]),
        (Short::Discards, environments(0)[0]),
        (Short::Discards, environments(0)[4]),
    ] {
        let (_disk, mut store, records) = a_log_of_short_records(&env, short).unwrap();
        store.set_marks(Some(MARKS));
        let write_of = |n: u64| Request {
            id: RequestId(n),
            op: Op::Write {
                offset: (n % 32) * 16 * BLOCK,
                data: Buf::filled(16, (n % 250) as u8 + 1),
            },
        };
        for n in 0..16 {
            store.submit(write_of(n)).unwrap();
        }
        let shown: Arc<Mutex<Option<Arc<Shown>>>> = Arc::default();
        let seen: Arc<Mutex<Vec<LogState>>> = Arc::default();
        let waker = Waker::new({
            let (shown, seen) = (shown.clone(), seen.clone());
            move || {
                // The worker's first wakes can come before the test
                // has the store's shown state in hand: they wait for
                // it, so that none is missed.
                let asked = std::time::Instant::now();
                loop {
                    if let Some(shown) = shown.lock().unwrap().as_ref() {
                        seen.lock().unwrap().push(shown.state());
                        return;
                    }
                    if asked.elapsed() > Duration::from_secs(10) {
                        return;
                    }
                    std::thread::yield_now();
                }
            }
        });
        let mut served = store.serve_with(Some(waker)).unwrap();
        *shown.lock().unwrap() = Some(served.log.clone());
        let mut done = Vec::new();
        let mut collect = |served: &mut Served, upto: usize| {
            let asked = std::time::Instant::now();
            while done.len() < upto {
                served.wait(&mut done, Duration::from_secs(1));
                assert!(
                    asked.elapsed() < Duration::from_secs(60),
                    "{short:?}, {env:?}: {} of {upto} writes completed",
                    done.len()
                );
            }
        };
        collect(&mut served, 16);
        for round in 1..4u64 {
            for n in 0..16 {
                served.submit(write_of(round * 16 + n)).unwrap();
            }
            collect(&mut served, (round as usize + 1) * 16);
        }
        assert!(
            done.iter().all(|c| c.result == Ok(())),
            "{short:?}, {env:?}"
        );
        // Answered behind the last completion's wake.
        served.position().unwrap();
        let seen = seen.lock().unwrap().clone();
        assert!(seen.len() >= 64, "{} wakes for 64 writes", seen.len());
        let largest = seen.iter().map(LogState::live).max().unwrap();
        assert!(
            largest < MARKS.high + ONE_REQUEST,
            "{short:?}, {env:?}: behind {records} short records the log was seen at {largest} bytes, \
             the high mark is {}",
            MARKS.high
        );
        let state = served.log_state();
        // The writes added more than the log may hold, so every short
        // record was taken, at most eight of them a step.
        assert!(
            state.steps >= records / MARKS.step.records,
            "{short:?}, {env:?}: {state:?}"
        );
        // The last write of each slot is what the volume holds.
        for slot in 0..32u64 {
            served
                .submit(Request {
                    id: RequestId(1000 + slot),
                    op: Op::Read {
                        offset: slot * 16 * BLOCK,
                        buf: Buf::filled(16, 0xEE),
                    },
                })
                .unwrap();
            let mut read = Vec::new();
            assert_eq!(served.wait(&mut read, Duration::from_secs(10)), 1);
            let n = 32 + slot;
            let want = (n % 250) as u8 + 1;
            let buf = read.pop().unwrap().buf.expect("a read's data");
            assert!(
                buf.iter().all(|b| *b == want),
                "{short:?}, {env:?}: slot {slot}"
            );
        }
    }
}

/// The rule's own sync of the log is shown as the beginning of its
/// step: a log that passed the high mark with nothing flushed needs
/// that sync before anything can be taken, and it can take as long as
/// any sync. The owner is woken with a step under way before the disk
/// has seen that sync, so whoever looks during the sync finds a step
/// under way, and not a log at the high mark with nothing happening.
/// The waker reads the shown state and the disk's count of syncs on
/// the worker's thread, so what is compared is of the moment of the
/// wake; the test waits for the first step to end.
#[test]
fn the_rules_own_sync_is_shown_as_the_beginning_of_its_step() {
    use std::sync::Mutex;
    let env = environments(0)[0];
    let (disk, mut store) = env.fresh().unwrap();
    for n in 0..12u64 {
        write(&mut store, n * 16, 16, n as u8 + 1).unwrap();
    }
    let unflushed = store.log_state();
    assert!(unflushed.live() > MARKS.high, "{unflushed:?}");
    assert!(
        unflushed.durable - unflushed.start < MARKS.step.payload,
        "{unflushed:?}"
    );
    store.set_marks(Some(MARKS));
    let syncs_before = disk.stats().syncs;
    let shown: Arc<Mutex<Option<Arc<Shown>>>> = Arc::default();
    let seen: Arc<Mutex<Vec<(LogState, u64)>>> = Arc::default();
    let waker = Waker::new({
        let (shown, seen, disk) = (shown.clone(), seen.clone(), disk.clone());
        move || {
            let asked = std::time::Instant::now();
            loop {
                if let Some(shown) = shown.lock().unwrap().as_ref() {
                    seen.lock()
                        .unwrap()
                        .push((shown.state(), disk.stats().syncs));
                    return;
                }
                if asked.elapsed() > Duration::from_secs(10) {
                    return;
                }
                std::thread::yield_now();
            }
        }
    });
    let served = store.serve_with(Some(waker)).unwrap();
    *shown.lock().unwrap() = Some(served.log.clone());
    let after = comes_by_itself(&served, "the first step", |s| s.steps > 0);
    assert!(after.own_syncs > 0, "the step needed no sync: {after:?}");
    let seen = seen.lock().unwrap().clone();
    let (first, syncs) = seen
        .iter()
        .find(|(state, _)| state.stepping)
        .unwrap_or_else(|| panic!("no step was shown under way: {seen:?}"));
    assert_eq!(
        *syncs, syncs_before,
        "the owner was told of the step after the rule's sync of the log: {first:?}"
    );
}

// ---------------------------------------------------------------------
// Where the time goes (M1.3h).

/// A flush that arrives while a step is under way is counted with the
/// time it waited, which is at least what was left of the step. (A
/// flush, since M1.3j: a read no longer waits for the worker. The
/// variables below still say "read".)
/// The step's time is in the part it was spent in, and the numbers fit
/// each other and the clock of the test. The step is held in its hook
/// behind its writes, so the time is the home image's sync's part.
#[test]
fn a_flush_that_arrives_during_a_step_is_counted_with_what_was_left_of_the_step() {
    const HELD: Duration = Duration::from_millis(300);
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    for n in 0..12u64 {
        write(&mut store, n * 16, 16, n as u8 + 1).unwrap();
    }
    flush(&mut store).unwrap();
    assert!(store.log_state().live() > MARKS.high);
    store.set_marks(Some(MARKS));
    let gate = std::env::temp_dir().join(format!("skyzfs-timing-gate-{}", std::process::id()));
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    store.set_step_pause_at(StepPause::Until(gate.clone()), StepPoint::Written);
    let began = std::time::Instant::now();
    let mut served = store.serve().unwrap();
    at_the_point(&gate, "the first step");
    assert!(served.log_state().stepping);
    let submitted = std::time::Instant::now();
    served
        .submit(Request {
            id: RequestId(1),
            op: Op::Flush,
        })
        .unwrap();
    std::thread::sleep(HELD);
    std::fs::write(&gate, b"").unwrap();
    let mut done = Vec::new();
    while done.is_empty() {
        served.wait(&mut done, Duration::from_secs(1));
        assert!(submitted.elapsed() < Duration::from_secs(20), "no answer");
    }
    let answered = submitted.elapsed();
    assert_eq!(done[0].result, Ok(()));
    // Answered behind the completion's state.
    served.position().unwrap();
    let total = began.elapsed();
    let (timing, state) = (served.timing(), served.log_state());
    // The worker goes on stepping and looks for the gate at each
    // step: it is ended before the gate is removed.
    drop(served);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    let ns = |time: Duration| time.as_nanos() as u64;

    let read = timing.requests[2];
    assert_eq!((read.wait.count, read.call.count), (1, 1), "{timing:?}");
    assert!(read.wait.sum_ns >= ns(HELD), "{timing:?}");
    assert!(
        read.wait.sum_ns + read.call.sum_ns <= ns(answered),
        "{timing:?}"
    );
    assert_eq!(read.wait.max_ns, read.wait.sum_ns);
    // It is the only request.
    for kind in [0, 1, 3, 4] {
        assert_eq!(timing.requests[kind], RequestTiming::default());
    }
    // The step that was held: its time is in the part behind the
    // writes, and nowhere else.
    assert!(timing.step_home_sync.max_ns >= ns(HELD), "{timing:?}");
    // The other parts were measured for the same steps, and all the
    // parts together fit into the test's own time: none of them took
    // the held time as well.
    let others = [
        timing.step_walk,
        timing.step_save,
        timing.step_apply,
        timing.step_start,
        timing.step_end,
    ];
    for part in others {
        assert_eq!(part.count, timing.step_home_sync.count);
    }
    // The log was above the high mark after that step, so the read
    // waited at the gate as well, and for less than its whole wait.
    assert!(timing.gate.count >= 1, "{timing:?}");
    assert!(timing.gate.sum_ns < read.wait.sum_ns, "{timing:?}");
    // No part is longer than the test.
    let steps = others.iter().map(|part| part.sum_ns).sum::<u64>()
        + timing.step_home_sync.sum_ns
        + timing.step_own_sync.sum_ns;
    assert!(steps <= ns(total), "{timing:?}");
    // What the read waited for, by what the worker did meanwhile: the
    // parts are the whole wait, the largest is the home image's sync,
    // which the step was held in, and that is what the longest wait
    // is put down to.
    let home_sync = ACTIVITIES
        .iter()
        .position(|a| *a == "step_home_sync")
        .unwrap();
    let by = timing.wait_by[2];
    assert_eq!(by.iter().sum::<u64>(), read.wait.sum_ns, "{timing:?}");
    assert!(by[home_sync] >= ns(HELD), "{timing:?}");
    assert_eq!(timing.longest_wait_cause[2], home_sync as u64);
    assert_eq!(timing.longest_wait_cause_ns[2], by[home_sync]);
    assert_eq!(by[0], 0, "the worker was not idle while the read waited");
    // The worker's own time: no more than the test's, and the sync it
    // was held in is in it.
    assert!(timing.worker.iter().sum::<u64>() <= ns(total), "{timing:?}");
    assert!(timing.worker[home_sync] >= timing.step_home_sync.max_ns);
    assert_eq!(timing.worker[home_sync], timing.step_home_sync.sum_ns);
    assert!(timing.step_home_sync.count <= state.steps);
}

/// A flush (a read, before reads left the worker in M1.3j; the
/// variables still say so) submitted while a step is held, behind two
/// writes that were submitted in the same moment: its wait is the rest of the step, the
/// steps the gate takes after it, and the two writes' calls, and the
/// parts add up to the wait. The two writes' calls are in it whole.
#[test]
fn a_flush_behind_a_step_and_two_writes_has_its_wait_split_among_them() {
    const HELD: Duration = Duration::from_millis(200);
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    for n in 0..12u64 {
        write(&mut store, n * 16, 16, n as u8 + 1).unwrap();
    }
    flush(&mut store).unwrap();
    store.set_marks(Some(MARKS));
    let gate = std::env::temp_dir().join(format!("skyzfs-split-gate-{}", std::process::id()));
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    store.set_step_pause_at(StepPause::Until(gate.clone()), StepPoint::Written);
    let mut served = store.serve().unwrap();
    at_the_point(&gate, "the first step");
    let requests = [
        Op::Write {
            offset: 300 * BLOCK,
            data: Buf::filled(16, 7),
        },
        Op::Write {
            offset: 320 * BLOCK,
            data: Buf::filled(16, 8),
        },
        Op::Flush,
    ];
    for (n, op) in requests.into_iter().enumerate() {
        served
            .submit(Request {
                id: RequestId(n as u64),
                op,
            })
            .unwrap();
    }
    std::thread::sleep(HELD);
    std::fs::write(&gate, b"").unwrap();
    let asked = std::time::Instant::now();
    let mut done = Vec::new();
    while done.len() < 3 {
        served.wait(&mut done, Duration::from_secs(1));
        assert!(asked.elapsed() < Duration::from_secs(20), "no answer");
    }
    served.position().unwrap();
    let timing = served.timing();
    drop(served);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    assert!(done.iter().all(|c| c.result.is_ok()));

    let at = |name: &str| ACTIVITIES.iter().position(|a| *a == name).unwrap();
    let ns = HELD.as_nanos() as u64;
    let (reads, writes) = (timing.requests[2], timing.requests[1]);
    assert_eq!((reads.wait.count, writes.wait.count), (1, 2), "{timing:?}");
    let by = timing.wait_by[2];
    assert_eq!(by.iter().sum::<u64>(), reads.wait.sum_ns, "{timing:?}");
    // Both writes' calls lie wholly inside the read's wait.
    assert_eq!(by[at("write")], writes.call.sum_ns, "{timing:?}");
    assert!(by[at("write")] > 0, "{timing:?}");
    // The held step's part, and the rest: other steps' parts and the
    // loop's own work, and nothing under a call that did not happen.
    assert!(by[at("step_home_sync")] >= ns, "{timing:?}");
    for none in ["idle", "read", "flush", "discard", "zeroes"] {
        assert_eq!(by[at(none)], 0, "{none}: {timing:?}");
    }
    assert_eq!(timing.longest_wait_cause[2], at("step_home_sync") as u64);
    // The second write waited for the first one's call too; the first
    // for none.
    let by = timing.wait_by[1];
    assert_eq!(by.iter().sum::<u64>(), writes.wait.sum_ns, "{timing:?}");
    assert!(
        by[at("write")] > 0 && by[at("write")] < writes.call.sum_ns,
        "{timing:?}"
    );
}

/// A write that finds the log full makes room inside its own call: a
/// sync of the log and a whole checkpoint. The checkpoint's parts are
/// the step's, in the worker's time and in the wait of a flush queued
/// behind that write (a read, before M1.3j; the variables still say so), and the write's activity is only the rest of its
/// call. The checkpoint is held 200 ms in the home image's part, so
/// that part is at least that long and the time under "write" is
/// shorter than the write's call by at least that.
#[test]
fn a_flush_behind_a_write_that_makes_room_waits_for_the_steps_parts() {
    const HELD: Duration = Duration::from_millis(200);
    let env = Env {
        segments: Some(2),
        ..environments(0)[0]
    };
    let (_disk, mut store) = env.fresh().unwrap();
    let gate = std::env::temp_dir().join(format!("skyzfs-room-gate-{}", std::process::id()));
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    store.set_step_pause_at(StepPause::Until(gate.clone()), StepPoint::Written);
    // No marks: the worker takes no step of its own, and the only
    // checkpoint is the one a write makes when the log is full.
    let mut served = store.serve().unwrap();
    let reached = StepPause::reached(&gate);
    let began = std::time::Instant::now();
    let mut done = Vec::new();
    let mut writes = 0u64;
    // Megabytes, one at a time, until one of them is making room.
    'filling: loop {
        assert!(writes < 20, "the log never filled");
        served
            .submit(Request {
                id: RequestId(writes),
                op: Op::Write {
                    offset: (writes % 3) * 256 * BLOCK,
                    data: Buf::filled(256, writes as u8 + 1),
                },
            })
            .unwrap();
        writes += 1;
        while done.len() < writes as usize {
            if reached.exists() {
                break 'filling;
            }
            served.wait(&mut done, Duration::from_millis(1));
            assert!(began.elapsed() < Duration::from_secs(20), "no answer");
        }
    }
    let before = served.timing();
    let read = RequestId(100);
    served
        .submit(Request {
            id: read,
            op: Op::Flush,
        })
        .unwrap();
    std::thread::sleep(HELD);
    std::fs::write(&gate, b"").unwrap();
    while done.len() < writes as usize + 1 {
        served.wait(&mut done, Duration::from_secs(1));
        assert!(began.elapsed() < Duration::from_secs(40), "no answer");
    }
    served.position().unwrap();
    let timing = served.timing();
    drop(served);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(&reached);

    let at = |name: &str| ACTIVITIES.iter().position(|a| *a == name).unwrap();
    let ns = HELD.as_nanos() as u64;
    // No step before the write that made room.
    assert_eq!(before.step_apply.count, 0, "{before:?}");
    assert_eq!(before.requests[1].call.count, writes - 1, "{before:?}");
    let (reads, wrote) = (timing.requests[2], timing.requests[1]);
    assert_eq!((reads.wait.count, wrote.call.count), (1, writes));
    // The write that made room is the longest, and its call holds the
    // checkpoint whole.
    let made_room = wrote.call.max_ns;
    assert!(made_room >= ns, "{timing:?}");
    // The read's wait: the checkpoint's parts under their names, the
    // held one the largest, and under "write" only the rest of that
    // one write's call.
    let by = timing.wait_by[2];
    assert_eq!(by.iter().sum::<u64>(), reads.wait.sum_ns, "{timing:?}");
    assert!(by[at("step_home_sync")] >= ns, "{timing:?}");
    assert!(by[at("write")] <= made_room - ns, "{timing:?}");
    assert_eq!(timing.longest_wait_cause[2], at("step_home_sync") as u64);
    // Nothing under a call that did not happen. (The worker's receive
    // of the read, after the write, is a moment under "idle".)
    for none in ["read", "flush", "discard", "zeroes", "own_sync"] {
        assert_eq!(by[at(none)], 0, "{none}: {timing:?}");
    }
    // The worker's time: every part of the checkpoint is there as the
    // step's sums have it, and the writes' activity is their calls
    // less those parts.
    let parts = [
        ("step_walk", timing.step_walk),
        ("step_save", timing.step_save),
        ("step_apply", timing.step_apply),
        ("step_home_sync", timing.step_home_sync),
        ("step_log_start", timing.step_start),
        ("step_end", timing.step_end),
    ];
    let mut in_parts = 0;
    for (name, part) in parts {
        assert_eq!(timing.worker[at(name)], part.sum_ns, "{name}: {timing:?}");
        in_parts += part.sum_ns;
    }
    // And the step's two barriers, which are no part of the six.
    let barriers = timing.barrier_home.sum_ns + timing.barrier_reuse.sum_ns;
    assert_eq!(timing.worker[at("step_barrier")], barriers, "{timing:?}");
    in_parts += barriers;
    assert!(timing.step_apply.count >= 1, "{timing:?}");
    assert_eq!(
        (timing.barrier_home.count, timing.barrier_reuse.count),
        (timing.step_apply.count, timing.step_apply.count),
        "{timing:?}"
    );
    assert_eq!(
        timing.worker[at("write")],
        wrote.call.sum_ns - in_parts,
        "{timing:?}"
    );
}

/// An explicit checkpoint of a served store is no request, and its
/// parts are taken right after it: they are the step's activities in
/// the worker's time and in the wait of a flush that was submitted
/// meanwhile (a read, before M1.3j; the variables still say so), and "other" holds nothing of the checkpoint. The
/// checkpoint is held 200 ms in the home image's part while the read
/// is submitted, so the read is the next thing the worker does after
/// it: if the parts were not taken at once they would lie before the
/// span the read's begin closes, and be lost.
#[test]
fn an_explicit_checkpoints_parts_are_the_steps_activities_in_a_served_store() {
    const HELD: Duration = Duration::from_millis(200);
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    for n in 0..12u64 {
        write(&mut store, n * 16, 16, n as u8 + 1).unwrap();
    }
    flush(&mut store).unwrap();
    let gate = std::env::temp_dir().join(format!("skyzfs-explicit-gate-{}", std::process::id()));
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    store.set_step_pause_at(StepPause::Until(gate.clone()), StepPoint::Written);
    // No marks: the worker takes no step of its own, and the only
    // batches are the checkpoint's.
    let mut served = store.serve().unwrap();
    // The checkpoint is asked for without waiting for it.
    let (reply, result) = mpsc::channel();
    assert!(served.send(Command::Checkpoint(reply)));
    at_the_point(&gate, "the checkpoint");
    let asked = std::time::Instant::now();
    served
        .submit(Request {
            id: RequestId(1),
            op: Op::Flush,
        })
        .unwrap();
    std::thread::sleep(HELD);
    std::fs::write(&gate, b"").unwrap();
    let mut done = Vec::new();
    while done.is_empty() {
        served.wait(&mut done, Duration::from_secs(1));
        assert!(asked.elapsed() < Duration::from_secs(20), "no answer");
    }
    let checkpointed = result.recv_timeout(Duration::from_secs(20));
    served.position().unwrap();
    let timing = served.timing();
    let live = served.log_state().live();
    drop(served);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    assert!(matches!(checkpointed, Ok(Ok(_))), "{checkpointed:?}");
    // The checkpoint took the whole log.
    assert_eq!(live, 0);
    assert_eq!(done[0].result, Ok(()));

    let at = |name: &str| ACTIVITIES.iter().position(|a| *a == name).unwrap();
    let ns = HELD.as_nanos() as u64;
    let parts = [
        ("step_walk", timing.step_walk),
        ("step_save", timing.step_save),
        ("step_apply", timing.step_apply),
        ("step_home_sync", timing.step_home_sync),
        ("step_log_start", timing.step_start),
        ("step_end", timing.step_end),
    ];
    assert!(timing.step_apply.count >= 1, "{timing:?}");
    assert!(timing.step_home_sync.sum_ns >= ns, "{timing:?}");
    // The worker's time: every part as the step's sums have it.
    for (name, part) in parts {
        assert_eq!(timing.worker[at(name)], part.sum_ns, "{name}: {timing:?}");
    }
    // The read's wait: the held part under its name, and under
    // "other" no more than what is left of the wait beside the 200 ms
    // it spent behind that part.
    let reads = timing.requests[2];
    let by = timing.wait_by[2];
    assert_eq!(by.iter().sum::<u64>(), reads.wait.sum_ns, "{timing:?}");
    assert!(by[at("step_home_sync")] >= ns, "{timing:?}");
    assert!(by[at("other")] <= reads.wait.sum_ns - ns, "{timing:?}");
    assert_eq!(timing.longest_wait_cause[2], at("step_home_sync") as u64);
}

/// A completion that `wait` takes after waiting for it has its stamp,
/// as one that `poll` takes has. The worker is held in a step while
/// the read is submitted, so `wait` finds nothing at first and takes
/// the completion when it comes.
#[test]
fn a_completion_taken_by_wait_has_its_ready_time() {
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    for n in 0..12u64 {
        write(&mut store, n * 16, 16, n as u8 + 1).unwrap();
    }
    flush(&mut store).unwrap();
    store.set_marks(Some(MARKS));
    let gate = std::env::temp_dir().join(format!("skyzfs-stamp-gate-{}", std::process::id()));
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    store.set_step_pause_at(StepPause::Until(gate.clone()), StepPoint::Written);
    let mut served = store.serve().unwrap();
    at_the_point(&gate, "the first step");
    let submitted = std::time::Instant::now();
    served
        .submit(Request {
            id: RequestId(5),
            op: Op::Flush,
        })
        .unwrap();
    let mut done = Vec::new();
    // Nothing is ready while the step is held.
    assert_eq!(served.poll(&mut done), 0);
    let release = gate.clone();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(release, b"").unwrap();
    });
    let taken = served.wait(&mut done, Duration::from_secs(20));
    let after = std::time::Instant::now();
    let ready = served.ready_at(RequestId(5));
    releaser.join().unwrap();
    drop(served);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(StepPause::reached(&gate));
    assert_eq!((taken, done.len()), (1, 1));
    let ready = ready.expect("the completion that wait took has no stamp");
    assert!(ready >= submitted && ready <= after);
}

/// Every kind of request is counted under its own kind, once per
/// request that completed, with a wait and a call each; a request that
/// waited for no step is not among those that did; and the sums and
/// the longest fit each other.
#[test]
fn every_kind_of_request_is_timed_under_its_kind() {
    let env = environments(0)[0];
    let (_disk, store) = env.fresh().unwrap();
    let mut served = store.serve().unwrap();
    let ops = [
        Op::Write {
            offset: 0,
            data: Buf::filled(4, 7),
        },
        Op::Write {
            offset: 8 * BLOCK,
            data: Buf::filled(4, 8),
        },
        Op::Flush,
        Op::Read {
            offset: 0,
            buf: Buf::zeroed(4),
        },
        Op::Discard {
            offset: 16 * BLOCK,
            len: 4 * BLOCK,
        },
        Op::WriteZeroes {
            offset: 32 * BLOCK,
            len: 4 * BLOCK,
        },
        Op::WriteZeroes {
            offset: 40 * BLOCK,
            len: 4 * BLOCK,
        },
        Op::WriteZeroes {
            offset: 48 * BLOCK,
            len: 4 * BLOCK,
        },
    ];
    let mut done = Vec::new();
    for (n, op) in ops.into_iter().enumerate() {
        served
            .submit(Request {
                id: RequestId(n as u64),
                op,
            })
            .unwrap();
        while done.len() <= n {
            served.wait(&mut done, Duration::from_secs(10));
        }
    }
    served.position().unwrap();
    let timing = served.timing();
    let counts: Vec<u64> = timing.requests.iter().map(|k| k.wait.count).collect();
    // read, write, flush, discard, write-zeroes.
    assert_eq!(counts, [1, 2, 1, 1, 3], "{timing:?}");
    assert_eq!(
        TIMED_KINDS,
        ["read", "write", "flush", "discard", "write_zeroes"]
    );
    for kind in timing.requests {
        assert_eq!(kind.wait.count, kind.call.count);
        for spent in [kind.wait, kind.call] {
            assert!(spent.max_ns <= spent.sum_ns, "{timing:?}");
            assert!(spent.sum_ns <= spent.max_ns * spent.count, "{timing:?}");
        }
    }
    // Each kind's waits, by what the worker did meanwhile, are its
    // whole wait; the worker was idle between the requests, and its
    // calls are in its own time under their kinds.
    for (kind, by) in timing.wait_by.iter().enumerate() {
        assert_eq!(
            by.iter().sum::<u64>(),
            timing.requests[kind].wait.sum_ns,
            "{timing:?}"
        );
        // Reads are the reader's: its calls are in its time and not
        // in the worker's.
        let calls = timing.requests[kind].call.sum_ns;
        if kind == 0 {
            assert_eq!((timing.worker[1], timing.reader[1]), (0, calls));
        } else {
            assert_eq!(timing.worker[1 + kind], calls);
        }
    }
    assert!(timing.worker[0] > 0 && timing.reader[0] > 0, "{timing:?}");
    // The read found a record that the flush before it had made
    // durable: it did not have to ask about a sync.
    assert_eq!(timing.read_unflushed, [0; 3]);
    assert_eq!(ACTIVITIES[0], "idle");
    assert_eq!(
        ACTIVITIES[1..6],
        ["read", "write", "flush", "discard", "zeroes"]
    );
    // No marks: no step and no gate.
    assert_eq!(timing.gate, Spent::default());
    assert_eq!(timing.step_home_sync, Spent::default());
}

// ---------------------------------------------------------------------
// Reads leave the worker (M1.3j).

/// A file that a hook waits for, and the one the hook makes when it
/// has reached its point. Both are removed when this is made and when
/// it is dropped.
struct Gate(std::path::PathBuf);

impl Gate {
    fn new(what: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let name = format!("skyzfs-gate-{}-{what}-{n}", std::process::id());
        let gate = Gate(std::env::temp_dir().join(name));
        gate.clear();
        gate
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(StepPause::reached(&self.0));
    }

    fn pause(&self) -> StepPause {
        StepPause::Until(self.0.clone())
    }

    /// Whether the hook has reached its point.
    fn reached(&self) -> bool {
        StepPause::reached(&self.0).exists()
    }

    /// Wait until the hook has reached its point.
    fn wait_reached(&self, what: &str) -> Outcome {
        let began = std::time::Instant::now();
        while !self.reached() {
            if began.elapsed() > Duration::from_secs(20) {
                return Err(format!("{what}: the hook was not reached"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    /// Let the hook go on, now and at every later time.
    fn open(&self) {
        std::fs::write(&self.0, b"").unwrap();
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Opens gates when it is dropped. Declared after the served store,
/// so that it is dropped before it: a scenario that returns early
/// leaves no thread of the store waiting in a hook, which the store's
/// own drop would then wait for.
struct Opened<'a>(&'a [&'a Gate]);

impl Drop for Opened<'_> {
    fn drop(&mut self) {
        for gate in self.0 {
            gate.open();
        }
    }
}

/// Take completions of a served store until there are `n`.
fn take(served: &mut Served, done: &mut Vec<Completion>, n: usize, what: &str) -> Outcome {
    let began = std::time::Instant::now();
    while done.len() < n {
        served.wait(done, Duration::from_millis(100));
        if began.elapsed() > Duration::from_secs(20) {
            return Err(format!("{what}: {} of {n} completions", done.len()));
        }
    }
    Ok(())
}

fn read_request(id: u64, block: u64, count: usize) -> Request {
    Request {
        id: RequestId(id),
        op: Op::Read {
            offset: block * BLOCK,
            buf: Buf::zeroed(count),
        },
    }
}

fn write_request(id: u64, block: u64, count: usize, byte: u8) -> Request {
    Request {
        id: RequestId(id),
        op: Op::Write {
            offset: block * BLOCK,
            data: Buf::filled(count, byte),
        },
    }
}

fn submit(served: &mut Served, request: Request) -> Outcome {
    served.submit(request).map_err(|e| e.to_string())
}

/// Wait until a barrier more than the `before` that had waited is
/// waiting for a reader: the worker is then at it, and does nothing
/// else until the reader lets go.
fn at_a_barrier(shared: &Shared, before: u64, what: &str) -> Outcome {
    let began = std::time::Instant::now();
    while shared.pin.waited() <= before {
        if began.elapsed() > Duration::from_secs(20) {
            return Err(format!("{what}: no barrier waited for the reader"));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// The completion of request `id` among `done`.
fn completion(done: &[Completion], id: u64) -> Result<&Completion, String> {
    done.iter()
        .find(|c| c.id == RequestId(id))
        .ok_or_else(|| format!("request {id} has no completion"))
}

/// The second barrier (design §5.1): a segment file that a step freed
/// is not used again while a reader that found a record in it is still
/// reading.
///
/// The log has two segment files and may have no more. A block is in
/// a record of the first. A checkpoint is held where the home image is
/// durable and the log's start has not moved; a read of the block
/// looks up, finds the record, and is held before it reads the file.
/// Then the checkpoint goes on: it frees the first file. A
/// megabyte is written behind it, which needs a new segment, and that
/// is the freed file.
///
/// The store waits at the barrier: nothing completes while the reader
/// is held, and the reader, released, returns the block's content.
/// Mutant: no barrier. The checkpoint and the write complete while
/// the reader is held, the record's place holds another record, and
/// the read does not return the block.
#[test]
fn a_freed_segment_is_not_reused_under_a_reader() {
    caught(freed_segment_under_a_reader, Mutants::SLOT_REUSED_UNBARRED);
}

/// The reader's pin is taken before its lookup (design §5.1), and not
/// only before it reads a file.
///
/// The scenario of [`a_freed_segment_is_not_reused_under_a_reader`]:
/// the read is held after its lookup, with the lookup's lock released.
/// Mutant: the pin is taken after the lookup, behind the place where
/// the read is held. The step drops the index and passes its barrier
/// while the reader holds no pin, the freed file takes the write, and
/// the read, released, does not return the block.
#[test]
fn a_reader_holds_its_pin_from_before_its_lookup() {
    caught(freed_segment_under_a_reader, Mutants::PIN_AFTER_LOOKUP);
}

fn freed_segment_under_a_reader(env: &Env) -> Outcome {
    let env = Env {
        segments: Some(2),
        ..*env
    };
    let mutant = env.mutants & (Mutants::SLOT_REUSED_UNBARRED | Mutants::PIN_AFTER_LOOKUP) != 0;
    let (_disk, mut store) = env.fresh()?;
    write(&mut store, 0, 1, 0x11)?;
    megabyte(&mut store, 1, 2)?;
    megabyte(&mut store, 2, 3)?;
    flush(&mut store)?;
    if store.core.log.segment_files() != 2 {
        return Err("the log is not in two files".into());
    }
    let (step, reader) = (Gate::new("reuse-step"), Gate::new("reuse-read"));
    store.set_step_pause_at(step.pause(), StepPoint::Synced);
    store.set_read_pause(reader.pause());
    let shared = store.core.shared.clone();
    let mut served = store.serve().map_err(|e| e.to_string())?;
    let _opened = Opened(&[&step, &reader]);
    let (reply, checkpointed) = mpsc::channel();
    if !served.send(Command::Checkpoint(reply)) {
        return Err("the worker is gone".into());
    }
    step.wait_reached("the checkpoint")?;
    let waited = shared.pin.waited();
    submit(&mut served, read_request(100, 0, 1))?;
    reader.wait_reached("the read")?;
    submit(&mut served, write_request(1, 256, 256, 4))?;
    step.open();
    let mut done = Vec::new();
    if mutant {
        // Nothing holds the worker.
        take(&mut served, &mut done, 1, "the write, with no barrier")?;
    } else {
        // The worker waits at the barrier, and has one thread: nothing
        // can complete.
        at_a_barrier(&shared, waited, "the second barrier")?;
        if served.poll(&mut done) != 0 {
            return Err("a request completed while the reader was held".into());
        }
    }
    reader.open();
    take(&mut served, &mut done, 2, "both")?;
    let checkpointed = checkpointed.recv_timeout(Duration::from_secs(20));
    if !matches!(checkpointed, Ok(Ok(_))) {
        return Err(format!("the checkpoint: {checkpointed:?}"));
    }
    let read = completion(&done, 100)?;
    if read.result.is_err()
        || read
            .buf
            .as_ref()
            .is_none_or(|b| b.iter().any(|b| *b != 0x11))
    {
        return Err(format!("the held read came to {:?}", read.result));
    }
    if done.iter().any(|c| c.result.is_err()) || served.gone() {
        return Err("a request failed".into());
    }
    // The writes are there, and the block still is.
    submit(&mut served, read_request(101, 0, 1))?;
    submit(&mut served, read_request(102, 256, 256))?;
    take(&mut served, &mut done, 4, "the reads after")?;
    for (id, byte) in [(101, 0x11), (102, 4)] {
        let read = completion(&done, id)?;
        if read
            .buf
            .as_ref()
            .is_none_or(|b| b.iter().any(|b| *b != byte))
        {
            return Err(format!("read {id} after the step is not {byte}"));
        }
    }
    Ok(())
}

/// The first barrier (design §5.1): a step does not write into the
/// home image while a reader that looked up before is still reading.
///
/// A read of a block that nothing has written looks up, finds the
/// home image, and is held before it reads the file. A write to the
/// block is acknowledged and made durable, and a checkpoint is asked
/// for, which is to write that block into the home image.
///
/// The store waits at the barrier: the step does not reach the point
/// behind its writes while the reader is held, the reader, released,
/// returns the block as it was, wholly, and then the step goes on.
/// Mutant: no barrier. The step reaches the point behind its writes
/// with the reader still held, and the reader returns what the step
/// wrote.
///
/// What this does not show: that a read beside the write would return
/// a block that is part old and part new. The simulated disk applies
/// a write whole. That a buffered read beside a buffered write of the
/// same page can tear is assumed of the kernel (a probe for it is the
/// testing session's); the barrier is what the store does about it.
#[test]
fn a_step_does_not_write_the_home_image_under_a_reader() {
    fn scenario(env: &Env) -> Outcome {
        let mutant = env.mutants & Mutants::HOME_WRITTEN_UNBARRED != 0;
        let (_disk, mut store) = env.fresh()?;
        let (step, reader) = (Gate::new("home-step"), Gate::new("home-read"));
        store.set_step_pause_at(step.pause(), StepPoint::Written);
        store.set_read_pause(reader.pause());
        let shared = store.core.shared.clone();
        let mut served = store.serve().map_err(|e| e.to_string())?;
        let _opened = Opened(&[&step, &reader]);
        submit(&mut served, read_request(100, 5, 1))?;
        reader.wait_reached("the read")?;
        let mut done = Vec::new();
        submit(&mut served, write_request(1, 5, 1, 0x22))?;
        take(&mut served, &mut done, 1, "the write")?;
        submit(
            &mut served,
            Request {
                id: RequestId(2),
                op: Op::Flush,
            },
        )?;
        take(&mut served, &mut done, 2, "the flush")?;
        let (reply, checkpointed) = mpsc::channel();
        if !served.send(Command::Checkpoint(reply)) {
            return Err("the worker is gone".into());
        }
        if mutant {
            // The home image holds the write, with the reader that
            // found the block there still at it.
            step.wait_reached("the step, with no barrier")?;
        } else {
            // The step waits at the barrier before the home image's
            // writes: it cannot have written.
            at_a_barrier(&shared, 0, "the first barrier")?;
            if step.reached() {
                return Err("the step wrote the home image while a reader was held".into());
            }
        }
        reader.open();
        take(&mut served, &mut done, 3, "the read")?;
        let read = completion(&done, 100)?;
        if read.result.is_err() || read.buf.as_ref().is_none_or(|b| b.iter().any(|b| *b != 0)) {
            return Err(format!(
                "the held read came to {:?} and is not the block as it was",
                read.result
            ));
        }
        step.wait_reached("the step, after the reader")?;
        step.open();
        let checkpointed = checkpointed.recv_timeout(Duration::from_secs(20));
        if !matches!(checkpointed, Ok(Ok(_))) {
            return Err(format!("the checkpoint: {checkpointed:?}"));
        }
        // A read after the write's acknowledgement sees it.
        submit(&mut served, read_request(101, 5, 1))?;
        take(&mut served, &mut done, 4, "the read after")?;
        let read = completion(&done, 101)?;
        if read
            .buf
            .as_ref()
            .is_none_or(|b| b.iter().any(|b| *b != 0x22))
        {
            return Err("the read after the step does not see the write".into());
        }
        Ok(())
    }
    caught(scenario, Mutants::HOME_WRITTEN_UNBARRED);
}

/// A read submitted after a write was acknowledged sees it (rule R3),
/// with the reader and the worker really beside each other: the marks
/// are so low that steps fall between the requests, with their
/// barriers and their drops of the index, while reads go on. Every
/// read is of the block just written and of its neighbours.
#[test]
fn a_read_after_an_acknowledged_write_sees_it_while_the_store_steps() {
    fn scenario(env: &Env) -> Outcome {
        let (_disk, mut store) = env.fresh()?;
        store.set_marks(Some(Marks {
            high: 24 << 10,
            low: 12 << 10,
            step: StepBound {
                payload: 16 << 10,
                records: 2,
            },
        }));
        let mut served = store.serve().map_err(|e| e.to_string())?;
        // What each of 48 blocks holds.
        let mut model = [0u8; 48];
        let mut done = Vec::new();
        for round in 0..600u64 {
            let block = round * 7 % 46;
            let byte = (round % 250) as u8 + 1;
            let op = match round % 5 {
                4 => Op::WriteZeroes {
                    offset: block * BLOCK,
                    len: 2 * BLOCK,
                },
                _ => Op::Write {
                    offset: block * BLOCK,
                    data: Buf::filled(2, byte),
                },
            };
            let zeroes = matches!(op, Op::WriteZeroes { .. });
            submit(
                &mut served,
                Request {
                    id: RequestId(1),
                    op,
                },
            )?;
            take(&mut served, &mut done, 1, "a write")?;
            for at in block..block + 2 {
                model[at as usize] = if zeroes { 0 } else { byte };
            }
            if round % 11 == 0 {
                submit(
                    &mut served,
                    Request {
                        id: RequestId(2),
                        op: Op::Flush,
                    },
                )?;
                take(&mut served, &mut done, 2, "a flush")?;
            }
            done.clear();
            // The block before, the two written, the block after.
            let first = block.saturating_sub(1);
            submit(&mut served, read_request(3, first, 4))?;
            take(&mut served, &mut done, 1, "a read")?;
            let read = done.pop().ok_or("no completion")?;
            read.result.map_err(|e| format!("round {round}: {e}"))?;
            let got = read.buf.ok_or("a read without its buffer")?;
            for (n, block) in got.chunks(BLOCK as usize).enumerate() {
                let want = model[first as usize + n];
                if block.iter().any(|b| *b != want) {
                    return Err(format!(
                        "round {round}: block {} reads {} and holds {want}",
                        first as usize + n,
                        block[0]
                    ));
                }
            }
        }
        let state = served.log_state();
        if state.steps == 0 {
            return Err("no step was taken".into());
        }
        Ok(())
    }
    holds(scenario);
}

/// A reader that has ended tells its owner, as a worker does: the
/// waker is called, `gone` says so from then on, and nothing is taken
/// any more, a write no more than a read.
#[test]
fn a_reader_that_has_ended_tells_its_owner() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let env = environments(0)[0];
    let (_disk, store) = env.fresh().unwrap();
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let waker = Waker::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let mut served = store.serve_with(Some(waker)).unwrap();
    assert!(!served.gone());
    served.panic_reader();
    assert!(served.gone());
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the owner is woken");
    let refused = served.submit(read_request(1, 0, 1)).unwrap_err();
    assert_eq!(refused.kind, SubmitErrorKind::Busy);
    let refused = served.submit(write_request(2, 0, 1, 1)).unwrap_err();
    assert_eq!(refused.kind, SubmitErrorKind::Busy);
    let mut done = Vec::new();
    assert_eq!(served.poll(&mut done), 0);
}

/// A read behind a write that is held is answered (M1.3j: rule R8
/// orders mutations, and a read does not wait for the worker). The log
/// is full behind a hold and a write is held; the store is then
/// served, and a read of what was written before is answered while
/// the write stays held. With the hold released the write completes.
#[test]
fn a_served_read_is_answered_behind_a_held_write() {
    fn scenario(env: &Env) -> Outcome {
        let env = Env {
            segments: Some(2),
            ..*env
        };
        let (_disk, mut store) = env.fresh()?;
        megabyte(&mut store, 0, 1)?;
        store
            .set_hold(Some(store.position()))
            .map_err(|e| e.to_string())?;
        let mut done = Vec::new();
        let mut held = None;
        for round in 0..4u64 {
            store
                .submit(write_request(round, round % 3 * 256, 256, 10 + round as u8))
                .map_err(|e| e.to_string())?;
            if store.wait(&mut done, Duration::ZERO) == 0 {
                held = Some(round);
                break;
            }
        }
        let Some(held) = held else {
            return Err("no write was held behind the hold".into());
        };
        let completed = done.len();
        // Served with the held write at the head of its queue.
        let mut served = store.serve().map_err(|e| e.to_string())?;
        // A block no write of the rounds before the held one touched,
        // or the last of them did.
        let (block, byte) = match held {
            0 => (0, 1),
            n => ((n - 1) % 3 * 256, 10 + (n - 1) as u8),
        };
        submit(&mut served, read_request(100, block, 1))?;
        take(&mut served, &mut done, completed + 1, "the read")?;
        let read = completion(&done, 100)?;
        if read.result.is_err()
            || read
                .buf
                .as_ref()
                .is_none_or(|b| b.iter().any(|b| *b != byte))
        {
            return Err(format!("the read behind the held write: {:?}", read.result));
        }
        if done.len() != completed + 1 || served.gone() {
            return Err("the held write completed behind the hold".into());
        }
        served.set_hold(None).map_err(|e| e.to_string())?;
        take(&mut served, &mut done, completed + 2, "the held write")?;
        completion(&done, held)?
            .result
            .map_err(|e| format!("the held write: {e}"))
    }
    holds(scenario);
}

/// A read that the reader cannot do fails the volume, as on the
/// worker: the read is answered as failed, every later request fails,
/// a write among them, and the next open finds the volume marked.
/// Mutant: the error is served and the volume goes on.
#[test]
fn a_read_the_reader_cannot_do_fails_the_volume() {
    fn scenario(env: &Env) -> Outcome {
        for from_home in [true, false] {
            let (disk, mut store) = env.fresh()?;
            write(&mut store, 4, 4, 1)?;
            flush(&mut store)?;
            if from_home {
                checkpoint(&mut store)?;
            }
            let mut served = store.serve().map_err(|e| e.to_string())?;
            if from_home {
                disk.fail_read_of(HOME_NAME);
            } else {
                disk.fail_read(0, false);
            }
            let mut done = Vec::new();
            submit(&mut served, read_request(1, 4, 1))?;
            take(&mut served, &mut done, 1, "the read")?;
            if done[0].result != Err(BlkError::Failed) {
                return Err("a read that could not be done succeeded".into());
            }
            disk.clear_faults();
            submit(&mut served, read_request(2, 5, 1))?;
            submit(&mut served, write_request(3, 9, 1, 2))?;
            take(&mut served, &mut done, 3, "the requests after")?;
            if done[1..].iter().any(|c| c.result != Err(BlkError::Failed)) {
                return Err(format!(
                    "after a failed read (from the home image: {from_home}) the volume served"
                ));
            }
            if served.gone() {
                return Err("a thread ended".into());
            }
            drop(served);
            match Store::open(disk.clone(), env.options()) {
                Err(StoreError::Log(LogError::MarkedFailed)) => {}
                Ok(_) => return Err("the next open did not find the volume marked".into()),
                Err(other) => return Err(format!("the next open: {other}")),
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::READ_ERROR_SERVED);
}

/// A read of a record that no sync has covered, beside a sync of the
/// log that fails. After a failed sync the cache may show the file as
/// it was before the write, so what the reader reads then may not be
/// what was written, and the file alone does not say so.
///
/// A write is acknowledged and not flushed. A read of it looks up,
/// finds the record, and is held. The disk then refuses syncs, a
/// flush fails and is answered, and the reader is released. The read
/// is answered with the bytes that were written or as failed, never
/// with anything else: the reader checks the record's whole payload
/// against its checksum. In at least one environment the failed sync
/// took the record back and the read is answered as failed. Mutant:
/// the reader does not check, and in at least one environment
/// answers with what the cache showed after the failure.
#[test]
fn a_read_of_an_unflushed_record_is_not_answered_from_a_failed_log() {
    /// The read's result, and whether the bytes it returned are the
    /// write's.
    fn round(env: &Env) -> Result<(Result<(), BlkError>, bool), String> {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 0, 1, 1)?;
        flush(&mut store)?;
        let reader = Gate::new("unflushed-read");
        store.set_read_pause(reader.pause());
        let mut served = store.serve().map_err(|e| e.to_string())?;
        let _opened = Opened(&[&reader]);
        let mut done = Vec::new();
        submit(&mut served, write_request(1, 8, 8, 0x33))?;
        take(&mut served, &mut done, 1, "the write")?;
        submit(&mut served, read_request(100, 8, 8))?;
        reader.wait_reached("the read")?;
        disk.fail_sync(0, true);
        submit(
            &mut served,
            Request {
                id: RequestId(2),
                op: Op::Flush,
            },
        )?;
        take(&mut served, &mut done, 2, "the flush")?;
        if done[1].result != Err(BlkError::Failed) {
            return Err("the flush did not fail".into());
        }
        reader.open();
        take(&mut served, &mut done, 3, "the read")?;
        let read = completion(&done, 100)?;
        let written = read
            .buf
            .as_ref()
            .is_some_and(|b| b.iter().all(|b| *b == 0x33));
        Ok((read.result, written))
    }
    let mut failed = 0;
    for env in environments(0) {
        match round(&env) {
            Ok((Ok(()), true)) => {}
            Ok((Err(BlkError::Failed), _)) => failed += 1,
            other => panic!("the read came to {other:?} in {env:?}"),
        }
    }
    assert!(failed > 0, "no failed sync took the record back");
    let served = environments(Mutants::UNFLUSHED_READ_UNCHECKED)
        .iter()
        .filter(|env| matches!(round(env), Ok((Ok(()), false))))
        .count();
    assert!(served > 0, "the defect was noticed nowhere");
}

/// A read of a record that no sync has covered does not wait for a
/// sync of the log: the worker is held at the start of a flush's sync
/// while the read is submitted, and the read is answered with the
/// write's bytes before the flush is let go. The reader counts it:
/// one read checked, the rest of the record read beyond what it asked
/// for, no mismatch.
#[test]
fn a_read_of_an_unflushed_record_does_not_wait_for_a_sync() {
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    let sync = Gate::new("unflushed-sync");
    store.set_sync_pause(sync.pause());
    let mut served = store.serve().unwrap();
    let _opened = Opened(&[&sync]);
    let mut done = Vec::new();
    submit(&mut served, write_request(1, 8, 8, 0x44)).unwrap();
    take(&mut served, &mut done, 1, "the write").unwrap();
    served
        .submit(Request {
            id: RequestId(2),
            op: Op::Flush,
        })
        .unwrap();
    sync.wait_reached("the flush's sync").unwrap();
    // Two blocks of the eight: the other six are read to be checked.
    submit(&mut served, read_request(100, 10, 2)).unwrap();
    take(&mut served, &mut done, 2, "the read").unwrap();
    // The flush is still held: the read did not wait for it.
    assert!(completion(&done, 2).is_err());
    let read = completion(&done, 100).unwrap();
    assert_eq!(read.result, Ok(()));
    assert!(read.buf.as_ref().unwrap().iter().all(|b| *b == 0x44));
    let unflushed = served.timing().read_unflushed;
    sync.open();
    take(&mut served, &mut done, 3, "the flush").unwrap();
    assert_eq!(unflushed, [1, 6 * BLOCK, 0]);
}

/// A reader that found a record of a step's batch in the index reads
/// it after the log's start has moved past it (design §5.1): it asks
/// the log nothing that the worker changes, and reads the file and
/// the place it found under the lock. The step is held where the new
/// start is durable and the index has not let go of the batch; a read
/// of a block of the batch looks up then, and is held. The step goes
/// on, lets go of the batch in the index and waits at its second
/// barrier: the checkpoint does not complete while the reader is held.
/// Released, the read returns the record's data and the volume has not
/// failed.
#[test]
fn a_reader_reads_a_record_the_log_has_given_up_while_it_held_its_pin() {
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    write(&mut store, 4, 4, 0x55).unwrap();
    flush(&mut store).unwrap();
    let (step, reader) = (Gate::new("given-up-step"), Gate::new("given-up-read"));
    store.set_step_pause_at(step.pause(), StepPoint::StartDurable);
    store.set_read_pause(reader.pause());
    let shared = store.core.shared.clone();
    let mut served = store.serve().unwrap();
    let _opened = Opened(&[&step, &reader]);
    let (reply, checkpointed) = mpsc::channel();
    assert!(served.send(Command::Checkpoint(reply)));
    step.wait_reached("the step").unwrap();
    let start = served.log_state().start;
    submit(&mut served, read_request(100, 5, 2)).unwrap();
    reader.wait_reached("the read").unwrap();
    let waited = shared.pin.waited();
    step.open();
    // The step waits at its second barrier: it cannot have passed it.
    at_a_barrier(&shared, waited, "the second barrier").unwrap();
    assert!(checkpointed.try_recv().is_err());
    reader.open();
    let mut done = Vec::new();
    take(&mut served, &mut done, 1, "the read").unwrap();
    let new_start = checkpointed.recv_timeout(Duration::from_secs(20)).unwrap();
    let read = completion(&done, 100).unwrap();
    assert_eq!(read.result, Ok(()));
    assert!(read.buf.as_ref().unwrap().iter().all(|b| *b == 0x55));
    assert!(new_start.unwrap().0 > start, "the start did not move");
    // The volume goes on.
    submit(&mut served, read_request(101, 4, 1)).unwrap();
    take(&mut served, &mut done, 2, "a read after").unwrap();
    assert_eq!(completion(&done, 101).unwrap().result, Ok(()));
    assert!(!served.gone());
}

/// The reader does not answer a read it cannot do: the worker marks
/// the log and only then answers (design §5.1). Here the worker is
/// held in a step while the reader fails a read, so the mark has not
/// been made: no failed read is delivered. Then the process is lost
/// (the machine stops taking anything, and the disk's process is
/// killed): a restart finds no mark, opens the volume, and reads what
/// was flushed, as after any loss of the process. Mutant: the reader
/// answers the failure itself, before any mark.
#[test]
fn a_failed_read_is_not_answered_before_the_log_is_marked() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 4, 4, 0x66)?;
        flush(&mut store)?;
        let step = Gate::new("unmarked-step");
        store.set_step_pause_at(step.pause(), StepPoint::Saved);
        let mut served = store.serve().map_err(|e| e.to_string())?;
        let _opened = Opened(&[&step]);
        let (reply, _checkpointed) = mpsc::channel();
        if !served.send(Command::Checkpoint(reply)) {
            return Err("the worker is gone".into());
        }
        step.wait_reached("the step")?;
        disk.fail_read(0, true);
        submit(&mut served, read_request(100, 5, 1))?;
        // However long this is, no failed read can be delivered.
        std::thread::sleep(Duration::from_millis(50));
        let mut done = Vec::new();
        if served.poll(&mut done) != 0 {
            return Err(format!(
                "a read was answered before the log was marked: {:?}",
                done[0].result
            ));
        }
        // The process is lost here.
        disk.halt_after(0);
        step.open();
        drop(served);
        disk.kill();
        disk.clear_faults();
        let mut store = env.reopen(&disk)?;
        expect(&mut store, 4, 4, 0x66, "after the restart")
    }
    caught(scenario, Mutants::READ_ERROR_SERVED);
}

/// The reader is told that the volume has failed only after the log's
/// marker was attempted (design §5.1): a read that begins while the
/// worker is failing the volume and has not yet tried the marker is
/// not answered as failed by the reader on its own.
///
/// The worker is held inside the failure, at the moment the log tells
/// its observer, which is before the marker is attempted; a read of
/// a flushed block is submitted then. It is answered with the block,
/// as a read in flight at the failure may be (rule R11), and not as
/// failed. Once the worker goes on, every later request fails and the
/// next open finds the mark. Mutant: the reader is told before the
/// marker, and answers the read as failed while there is no mark.
#[test]
fn a_read_is_not_answered_as_failed_before_the_marker_is_attempted() {
    fn scenario(env: &Env) -> Outcome {
        let disk = SimDisk::new(7);
        disk.set_write_unit(u64::from(env.unit));
        let observer = Gate::new("marking");
        let pause = observer.pause();
        let options = OpenOptions {
            on_failure: Some(FailureObserver::new(move || pause.wait())),
            ..env.options()
        };
        let key = Key(*b"a key for tests!");
        let mut store = Store::create_keyed(disk.clone(), &env.config(), key, options)
            .map_err(|e| e.to_string())?;
        write(&mut store, 4, 4, 0x77)?;
        flush(&mut store)?;
        let mut served = store.serve().map_err(|e| e.to_string())?;
        let _opened = Opened(&[&observer]);
        let mut done = Vec::new();
        // The failure, asked for without waiting for it.
        let (reply, failed) = mpsc::channel();
        if !served.send(Command::Fail(reply)) {
            return Err("the worker is gone".into());
        }
        observer.wait_reached("the failure")?;
        submit(&mut served, read_request(100, 5, 1))?;
        take(&mut served, &mut done, 1, "the read")?;
        let answered = &done[0];
        if answered.result != Ok(())
            || answered
                .buf
                .as_ref()
                .is_none_or(|b| b.iter().any(|b| *b != 0x77))
        {
            return Err(format!(
                "a read was answered {:?} before the marker was attempted",
                answered.result
            ));
        }
        observer.open();
        failed
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| "the failure did not end")?;
        done.clear();
        submit(&mut served, read_request(101, 5, 1))?;
        take(&mut served, &mut done, 1, "a read after")?;
        if done[0].result != Err(BlkError::Failed) {
            return Err("a read after the failure was served".into());
        }
        drop(served);
        match Store::open(disk.clone(), env.options()) {
            Err(StoreError::Log(LogError::MarkedFailed)) => Ok(()),
            Ok(_) => Err("the next open did not find the volume marked".into()),
            Err(other) => Err(format!("the next open: {other}")),
        }
    }
    caught(scenario, Mutants::FAILED_SHOWN_BEFORE_MARK);
}

/// A barrier waits for the readers that were under way when it began,
/// and not for a moment with no reader: pins that overlap each other
/// without a gap, entered and left for as long as the test lets them,
/// do not hold a barrier. Mutant (the defect, given to the barrier):
/// it waits for a moment with no reader, which never comes while the
/// pins go on.
#[test]
fn a_barrier_is_not_held_by_reads_that_keep_arriving() {
    use std::sync::atomic::{AtomicBool, Ordering};
    for all in [false, true] {
        let pins = Arc::new(Pins::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (entered, first) = mpsc::channel();
        let chain = {
            let (pins, stop) = (pins.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut held = pins.enter();
                let _ = entered.send(());
                while !stop.load(Ordering::SeqCst) {
                    // The next pin is entered before the last is left.
                    let next = pins.enter();
                    drop(held);
                    held = next;
                    std::thread::sleep(Duration::from_micros(100));
                }
                drop(held);
            })
        };
        first.recv().unwrap();
        let (passed, barrier) = mpsc::channel();
        let waiting = {
            let pins = pins.clone();
            std::thread::spawn(move || {
                pins.barrier(all, false);
                let _ = passed.send(());
            })
        };
        let wait = if all {
            Duration::from_millis(300)
        } else {
            Duration::from_secs(20)
        };
        let result = barrier.recv_timeout(wait);
        stop.store(true, Ordering::SeqCst);
        chain.join().unwrap();
        waiting.join().unwrap();
        if all {
            assert!(result.is_err(), "the defect was not noticed");
        } else {
            assert!(
                result.is_ok(),
                "the barrier was held by reads that kept arriving"
            );
        }
    }
}

/// The reader's own account. A read's wait is laid over what the
/// reader did meanwhile: idle (its wake-up), the calls of reads before
/// it, anything else; and the three are the reader's whole time. The
/// first read is held 200 ms in its call; the second is submitted
/// meanwhile, so at least those 200 ms of its wait are behind a read.
/// Nothing of a read is in the worker's account.
#[test]
fn a_read_waits_for_the_reader_and_for_nothing_of_the_worker() {
    const HELD: Duration = Duration::from_millis(200);
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    write(&mut store, 0, 4, 9).unwrap();
    flush(&mut store).unwrap();
    let reader = Gate::new("timing-read");
    store.set_read_pause(reader.pause());
    let began = std::time::Instant::now();
    let mut served = store.serve().unwrap();
    served.submit(read_request(1, 0, 1)).unwrap();
    reader.wait_reached("the first read").unwrap();
    served.submit(read_request(2, 1, 1)).unwrap();
    std::thread::sleep(HELD);
    reader.open();
    let mut done = Vec::new();
    take(&mut served, &mut done, 2, "the reads").unwrap();
    let total = began.elapsed().as_nanos() as u64;
    let timing = served.timing();
    drop(served);
    let ns = HELD.as_nanos() as u64;
    let at = |name: &str| ACTIVITIES.iter().position(|a| *a == name).unwrap();

    let reads = timing.requests[0];
    assert_eq!((reads.wait.count, reads.call.count), (2, 2), "{timing:?}");
    // The first read's call holds the 200 ms, and the second read's
    // wait holds them behind that call.
    assert!(reads.call.max_ns >= ns, "{timing:?}");
    assert!(reads.wait.max_ns >= ns, "{timing:?}");
    let by = timing.wait_by[0];
    assert_eq!(by.iter().sum::<u64>(), reads.wait.sum_ns, "{timing:?}");
    assert!(by[at("read")] >= ns, "{timing:?}");
    assert_eq!(timing.longest_wait_cause[0], at("read") as u64);
    assert_eq!(timing.longest_wait_cause_ns[0], by[at("read")]);
    // A read waits for the reader's three activities and no other.
    for (activity, waited) in by.iter().enumerate() {
        let readers = ["idle", "read", "other"].contains(&ACTIVITIES[activity]);
        assert!(
            readers || *waited == 0,
            "{}: {timing:?}",
            ACTIVITIES[activity]
        );
    }
    // The reader's time: its calls are the reads' calls, and all of
    // it is no more than the test's.
    assert_eq!(timing.reader[1], reads.call.sum_ns, "{timing:?}");
    assert!(timing.reader.iter().sum::<u64>() <= total, "{timing:?}");
    // The worker did no read.
    assert_eq!(timing.worker[at("read")], 0, "{timing:?}");
    for kind in 1..5 {
        assert_eq!(timing.requests[kind], RequestTiming::default());
    }
}

/// What a barrier costs is counted. A reader is held 200 ms with its
/// pin while a checkpoint is asked for: the step waits at its first
/// barrier for at least that long, the wait is the worker's activity
/// "step_barrier" and in none of the step's parts, each batch has
/// passed both barriers, and a flush submitted meanwhile has the
/// barrier in its wait.
#[test]
fn a_steps_wait_at_a_barrier_is_counted_as_its_own_activity() {
    const HELD: Duration = Duration::from_millis(200);
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    write(&mut store, 0, 4, 9).unwrap();
    flush(&mut store).unwrap();
    let reader = Gate::new("barrier-read");
    store.set_read_pause(reader.pause());
    let mut served = store.serve().unwrap();
    served.submit(read_request(1, 100, 1)).unwrap();
    reader.wait_reached("the read").unwrap();
    let (reply, checkpointed) = mpsc::channel();
    assert!(served.send(Command::Checkpoint(reply)));
    served
        .submit(Request {
            id: RequestId(2),
            op: Op::Flush,
        })
        .unwrap();
    std::thread::sleep(HELD);
    reader.open();
    let mut done = Vec::new();
    take(&mut served, &mut done, 2, "the read and the flush").unwrap();
    assert!(matches!(
        checkpointed.recv_timeout(Duration::from_secs(20)),
        Ok(Ok(_))
    ));
    served.position().unwrap();
    let timing = served.timing();
    drop(served);
    let ns = HELD.as_nanos() as u64;
    let at = |name: &str| ACTIVITIES.iter().position(|a| *a == name).unwrap();

    let batches = timing.step_apply.count;
    assert!(batches >= 1, "{timing:?}");
    assert_eq!(
        (timing.barrier_home.count, timing.barrier_reuse.count),
        (batches, batches),
        "{timing:?}"
    );
    assert!(timing.barrier_home.max_ns >= ns, "{timing:?}");
    let barriers = timing.barrier_home.sum_ns + timing.barrier_reuse.sum_ns;
    assert_eq!(timing.worker[at("step_barrier")], barriers, "{timing:?}");
    // The step's parts are as their sums have them, without the wait.
    for (name, part) in [
        ("step_save", timing.step_save),
        ("step_apply", timing.step_apply),
        ("step_home_sync", timing.step_home_sync),
        ("step_log_start", timing.step_start),
        ("step_end", timing.step_end),
    ] {
        assert_eq!(timing.worker[at(name)], part.sum_ns, "{name}: {timing:?}");
    }
    // The flush waited behind the checkpoint, the barrier included.
    let by = timing.wait_by[2];
    assert_eq!(by.iter().sum::<u64>(), timing.requests[2].wait.sum_ns);
    assert!(by[at("step_barrier")] >= ns, "{timing:?}");
    assert_eq!(timing.longest_wait_cause[2], at("step_barrier") as u64);
}

// ---------------------------------------------------------------------
// Write-zeroes deallocates (M1.3f).

fn zeroes(store: &mut SimStore, block: u64, count: u64) -> Outcome {
    ok(
        store,
        Op::WriteZeroes {
            offset: block * BLOCK,
            len: count * BLOCK,
        },
    )
    .map(|_| ())
}

/// A write-zeroes record is applied by deallocating its range in the
/// home image: nothing is written for it, the range reads as zeros
/// under the load, after the steps and after a restart, and what lies
/// beside it stays, also where the range begins and ends inside a
/// write unit. Mutant: the zeros are written, as before M1.3f.
#[test]
fn a_write_zeroes_is_deallocated_and_nothing_is_written_for_it() {
    // Blocks 301 to 801: 500 blocks, not on a boundary of 4 blocks.
    const FIRST: u64 = 301;
    const COUNT: u64 = 500;
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        for mib in 0..4 {
            write(&mut store, mib * 256, 256, 9)?;
        }
        flush(&mut store)?;
        checkpoint(&mut store)?;
        if store.log_state().zeroes != Zeroes::Untried {
            return Err("a store that met no write-zeroes says how it treats them".into());
        }
        zeroes(&mut store, FIRST, COUNT)?;
        write(&mut store, 10, 1, 3)?;
        flush(&mut store)?;
        let before = disk.stats();
        while step(&mut store, StepBound::DAEMON)?.more {}
        let after = disk.stats();
        let (written, deallocated) = (
            after.written - before.written,
            after.deallocated - before.deallocated,
        );
        if deallocated != COUNT * BLOCK {
            return Err(format!("{deallocated} bytes were deallocated"));
        }
        // One block of data, the neighbours saved beside the range's
        // two edges, and the log's start: far less than the range.
        if written >= COUNT * BLOCK / 4 {
            return Err(format!(
                "{written} bytes were written for a range of {}",
                COUNT * BLOCK
            ));
        }
        if store.log_state().zeroes != Zeroes::Deallocated {
            return Err(format!("{:?}", store.log_state().zeroes));
        }
        // In reads of at most 128 blocks.
        let all = |store: &mut SimStore, from: u64, to: u64, byte: u8, when: &str| -> Outcome {
            let mut at = from;
            while at < to {
                let n = (to - at).min(128);
                expect(store, at, n as usize, byte, when)?;
                at += n;
            }
            Ok(())
        };
        let check = |store: &mut SimStore, when: &str| -> Outcome {
            all(store, 0, 10, 9, when)?;
            all(store, 10, 11, 3, when)?;
            all(store, 11, FIRST, 9, when)?;
            all(store, FIRST, FIRST + COUNT, 0, when)?;
            all(store, FIRST + COUNT, 1024, 9, when)
        };
        check(&mut store, "after the steps")?;
        drop(store);
        disk.kill();
        let mut store = env.reopen_ack(&disk)?;
        check(&mut store, "after a restart")
    }
    caught(scenario, Mutants::ZEROES_WRITTEN);
}

/// A write-zeroes record whose range is deallocated counts no bytes in
/// a step's bound, as a discard counts none: once the file system has
/// shown that it deallocates, a step takes such records up to its
/// count of records, whatever they cover. The first of them after an
/// open still counts its length, because the step does not know yet
/// which it will be. Mutant: they count their length.
#[test]
fn deallocated_write_zeroes_count_no_bytes_in_a_step() {
    fn scenario(env: &Env) -> Outcome {
        let (_disk, mut store) = env.fresh()?;
        for n in 0..40 {
            zeroes(&mut store, n * 16, 16)?;
        }
        flush(&mut store)?;
        let bound = StepBound {
            payload: 16 * BLOCK,
            records: 30,
        };
        let first = step(&mut store, bound)?;
        if (first.records, first.payload) != (1, 16 * BLOCK) {
            return Err(format!("the first step, not knowing yet: {first:?}"));
        }
        let second = step(&mut store, bound)?;
        if (second.records, second.payload, second.more) != (30, 0, true) {
            return Err(format!("a step of deallocated ranges: {second:?}"));
        }
        Ok(())
    }
    caught(scenario, Mutants::ZEROES_WRITTEN);
}

/// A file system that cannot deallocate is not a failed volume: the
/// zeros are written, the range reads as zeros, the store says that it
/// writes them, and each such record counts its length in a step's
/// bound again, so a step still writes no more than its bound. The
/// same when the test hook refuses in the file system's place.
/// Mutants: the refusal fails the volume; written zeros count nothing,
/// so one step writes them all.
#[test]
fn a_file_system_without_holes_has_the_zeros_written() {
    fn scenario_with(env: &Env, by_hook: bool) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        if by_hook {
            store.set_refuse_holes(true);
        } else {
            disk.set_deallocation(false);
        }
        write(&mut store, 0, 256, 9)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        for n in 0..12 {
            zeroes(&mut store, 8 + n * 16, 16)?;
        }
        flush(&mut store)?;
        let bound = StepBound {
            payload: 32 * BLOCK,
            records: 30,
        };
        let before = disk.stats();
        let mut steps = 0;
        loop {
            let done = step(&mut store, bound)?;
            if done.payload > bound.payload || done.records > 2 {
                return Err(format!("a step beyond its bound: {done:?}"));
            }
            steps += 1;
            if !done.more {
                break;
            }
        }
        let after = disk.stats();
        if steps != 6 {
            return Err(format!("12 records of 16 blocks in {steps} steps of 32"));
        }
        if after.deallocated != before.deallocated {
            return Err("something was deallocated".into());
        }
        if after.written - before.written < 12 * 16 * BLOCK {
            return Err(format!(
                "{} bytes written for 12 ranges of 16 blocks",
                after.written - before.written
            ));
        }
        if store.log_state().zeroes != Zeroes::Written {
            return Err(format!("{:?}", store.log_state().zeroes));
        }
        if store.is_failed() {
            return Err("the volume failed".into());
        }
        expect(&mut store, 0, 8, 9, "before the ranges")?;
        expect(&mut store, 8, 12 * 16, 0, "the ranges")?;
        expect(&mut store, 200, 56, 9, "after the ranges")
    }
    fn scenario(env: &Env) -> Outcome {
        scenario_with(env, false)?;
        scenario_with(env, true)
    }
    caught(scenario, Mutants::NO_HOLES_FAILS);
    caught(scenario, Mutants::WRITTEN_ZEROES_FREE);
}

/// A store is created only in a directory whose file system has
/// blocks no larger than the store's write unit, and only where that
/// size can be read. A refused creation makes nothing, not even the
/// lock file. Mutant: the check at the creation is not made.
#[test]
fn a_store_is_created_only_where_blocks_are_no_larger_than_the_write_unit() {
    fn scenario(env: &Env) -> Outcome {
        let unit = u64::from(env.unit);
        let create = |block: Option<u64>| {
            let disk = SimDisk::new(7);
            disk.set_write_unit(unit);
            disk.set_block_size(block);
            let key = Key(*b"a key for tests!");
            let made = Store::create_keyed(disk.clone(), &env.config(), key, env.options());
            (disk, made.map(|_| ()))
        };
        for block in [512, 4096, unit] {
            let (_, made) = create(Some(block));
            made.map_err(|e| format!("blocks of {block}: {e}"))?;
        }
        for block in [2 * unit, 4 * unit, 1 << 20] {
            let (disk, made) = create(Some(block));
            let want = StoreError::Blocks {
                what: Under::Directory,
                block,
                unit,
            };
            if made != Err(want) {
                return Err(format!("blocks of {block}: {made:?}"));
            }
            let (names, stats) = (disk.list().map_err(|e| e.to_string())?, disk.stats());
            if !names.is_empty() || stats.writes != 0 {
                return Err(format!("a refused creation left {names:?}, {stats:?}"));
            }
        }
        let (disk, made) = create(None);
        if !matches!(
            made,
            Err(StoreError::BlocksUnknown {
                what: Under::Directory,
                ..
            })
        ) {
            return Err(format!("a block size that cannot be read: {made:?}"));
        }
        if !disk.list().map_err(|e| e.to_string())?.is_empty() {
            return Err("a refused creation left files".into());
        }
        Ok(())
    }
    caught(scenario, Mutants::CREATE_ON_ANY_BLOCKS);
}

/// A store whose directory now is on a file system with larger blocks
/// than its write unit (it was copied or moved), or one whose block
/// size cannot be read, is not opened: nothing is written, synced,
/// made or marked, and the same store opens again, whole, where the
/// blocks fit. Mutant: the check at an open is not made.
#[test]
fn an_open_is_refused_where_blocks_are_larger_than_the_write_unit() {
    fn scenario(env: &Env) -> Outcome {
        let unit = u64::from(env.unit);
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 3, 5, 9)?;
        flush(&mut store)?;
        write(&mut store, 40, 2, 8)?;
        drop(store);
        disk.kill();
        let names = disk.list().map_err(|e| e.to_string())?;
        let before = disk.stats();
        for block in [Some(2 * unit), Some(1 << 20), None] {
            disk.set_block_size(block);
            let opened = Store::open(disk.clone(), env.options()).map(|_| ());
            let refused = match (block, &opened) {
                (
                    Some(block),
                    Err(StoreError::Blocks {
                        what,
                        block: b,
                        unit: u,
                    }),
                ) => *what == Under::Directory && *b == block && *u == unit,
                (None, Err(StoreError::BlocksUnknown { .. })) => true,
                _ => false,
            };
            if !refused {
                return Err(format!("blocks of {block:?}: {opened:?}"));
            }
            let after = disk.stats();
            if (after.writes, after.syncs, after.dir_syncs)
                != (before.writes, before.syncs, before.dir_syncs)
            {
                return Err(format!("a refused open wrote: {before:?} then {after:?}"));
            }
            if disk.list().map_err(|e| e.to_string())? != names {
                return Err("a refused open changed the directory".into());
            }
        }
        disk.set_block_size(Some(unit));
        let (mut store, _) = Store::open(disk.clone(), env.options()).map_err(|e| e.to_string())?;
        expect(&mut store, 3, 5, 9, "what was flushed")?;
        expect(&mut store, 40, 2, 8, "what a kill does not lose")
    }
    caught(scenario, Mutants::OPEN_ON_ANY_BLOCKS);
}

/// The other measure of how far a write reaches: the page of the
/// system that writes. A store is not created with a write unit below
/// the system's page, and one made where pages were no larger than its
/// unit is not opened where they are larger: nothing is written, and
/// it opens again, whole, where the pages fit. Mutant: the page size
/// is not asked.
#[test]
fn a_store_is_refused_where_pages_are_larger_than_the_write_unit() {
    fn scenario(env: &Env) -> Outcome {
        let unit = u64::from(env.unit);
        let disk = SimDisk::new(7);
        disk.set_write_unit(4 * unit);
        disk.set_page_size(4 * unit);
        let key = Key(*b"a key for tests!");
        let made = Store::create_keyed(disk.clone(), &env.config(), key, env.options());
        let want = StoreError::Pages {
            page: 4 * unit,
            unit,
        };
        if made.as_ref().err() != Some(&want) {
            return Err(format!("a creation under larger pages: {:?}", made.err()));
        }
        if !disk.list().map_err(|e| e.to_string())?.is_empty() {
            return Err("a refused creation left files".into());
        }

        let (disk, mut store) = env.fresh()?;
        disk.set_page_size(unit);
        write(&mut store, 3, 5, 9)?;
        flush(&mut store)?;
        drop(store);
        disk.kill();
        let names = disk.list().map_err(|e| e.to_string())?;
        let before = disk.stats();
        disk.set_page_size(4 * unit);
        let opened = Store::open(disk.clone(), env.options()).map(|_| ());
        if opened != Err(want) {
            return Err(format!("an open under larger pages: {opened:?}"));
        }
        let after = disk.stats();
        if (after.writes, after.syncs, after.dir_syncs)
            != (before.writes, before.syncs, before.dir_syncs)
            || disk.list().map_err(|e| e.to_string())? != names
        {
            return Err(format!("a refused open wrote: {before:?} then {after:?}"));
        }
        disk.set_page_size(unit);
        let (mut store, _) = Store::open(disk.clone(), env.options()).map_err(|e| e.to_string())?;
        expect(&mut store, 3, 5, 9, "what was flushed")
    }
    caught(scenario, Mutants::UNDER_ANY_PAGES);
}

/// What the two refusals are for. The disk rewrites the whole block of
/// its file system around every write, in every file, and a power cut
/// leaves what was not synced as the environment says (as it was, or
/// garbage). The store runs on a disk whose block is `block`.
///
/// First the log: a write is flushed, a second is not, the power is
/// cut. The second write went into the file system block that holds
/// the first, durable record, unless the log leaves a unit that holds
/// a durable record alone and the unit is at least the block. Then the
/// checkpoint: single blocks are written and ranges zeroed beside
/// blocks that only the home image holds, the steps are stopped
/// anywhere (so also between the preimage file's sync and the home
/// image's), the power is cut, and cut again inside the open.
///
/// `log_stage` false leaves the first stage out: every record is then
/// durable before any cut, so what is lost is lost by a checkpoint's
/// writes.
fn on_blocks_of(env: &Env, block: u64, log_stage: bool) -> Outcome {
    let lost = env.lost();
    let cut = |disk: &SimDisk| {
        disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        disk.set_write_unit(SECTOR);
        disk.set_block_size(Some(block));
    };
    let bound = StepBound {
        payload: 16 * BLOCK,
        records: 16,
    };
    for stop in (0..30).chain([44, 61, 90, 150]) {
        let (disk, mut store) = env.fresh()?;
        // The block of the file system alone says how far a write
        // reaches: the device's own unit is one sector.
        disk.set_write_unit(SECTOR);
        disk.set_block_size(Some(block));
        write(&mut store, 0, 256, 9)?;
        write(&mut store, 256, 256, 9)?;
        flush(&mut store)?;
        if log_stage {
            write(&mut store, 300, 1, 5)?;
            write(&mut store, 301, 1, 6)?;
            drop(store);
            cut(&disk);
            store = env.reopen_ack(&disk)?;
            let what = "flushed, after a cut behind a later write";
            expect(&mut store, 0, 256, 9, what)?;
            expect(&mut store, 256, 256, 9, what)?;
        }
        checkpoint(&mut store)?;
        // Groups of 32 blocks, of which the first eight are touched:
        // the other 24 are held by the home image alone, in write
        // units no record of the batch touches.
        for group in 0..16u64 {
            zeroes(&mut store, group * 32 + 1, 5)?;
            write(&mut store, group * 32 + 7, 1, group as u8 + 10)?;
        }
        flush(&mut store)?;
        disk.halt_after(stop);
        while store.checkpoint_step(bound).is_ok_and(|done| done.more) {}
        drop(store);
        cut(&disk);
        disk.halt_after(stop % 7);
        drop(Store::open(disk.clone(), env.options()));
        cut(&disk);
        let mut store = env.reopen_ack(&disk)?;
        let check = |store: &mut SimStore, when: &str| -> Outcome {
            for group in 0..16u64 {
                let what = format!("group {group} {when} a cut at {stop}");
                expect(store, group * 32, 1, 9, &what)?;
                expect(store, group * 32 + 1, 5, 0, &what)?;
                expect(store, group * 32 + 6, 1, 9, &what)?;
                expect(store, group * 32 + 7, 1, group as u8 + 10, &what)?;
                expect(store, group * 32 + 8, 24, 9, &what)?;
            }
            Ok(())
        };
        check(&mut store, "after")?;
        while step(&mut store, bound)?.more {}
        check(&mut store, "and the remaining steps after")?;
    }
    Ok(())
}

/// A volume whose write unit is at least the block of its file system
/// survives power cuts anywhere, in every environment, on a disk that
/// rewrites whole blocks around every write.
#[test]
fn a_volume_whose_unit_is_at_least_the_block_survives_cuts_anywhere() {
    fn scenario(env: &Env) -> Outcome {
        on_blocks_of(env, u64::from(env.unit), true)?;
        on_blocks_of(env, 512, true)
    }
    holds(scenario);
}

/// The exposure, which is the store as it was before a volume's write
/// unit was held against the file system's block: with neither check
/// made, a volume on a file system with blocks of four write units
/// loses, at a power cut that leaves garbage, a block that nobody was
/// writing: a flushed record in the log, behind a later write, and,
/// with the log left out of it, a block of the home image beside a
/// checkpoint's write. Both in every such environment.
#[test]
fn a_volume_whose_unit_is_below_the_block_loses_what_nobody_wrote() {
    let unchecked = Mutants::CREATE_ON_ANY_BLOCKS | Mutants::OPEN_ON_ANY_BLOCKS;
    let mut ran = 0;
    for env in environments(unchecked) {
        if !env.garbage {
            continue;
        }
        ran += 1;
        let block = 4 * u64::from(env.unit);
        let in_the_log = on_blocks_of(&env, block, true).expect_err("a loss");
        assert!(
            in_the_log.contains("after a cut behind a later write"),
            "{in_the_log} in {env:?}"
        );
        // And with every record durable before the first cut: the
        // loss is then the checkpoint's, in the home image.
        let by_a_checkpoint = on_blocks_of(&env, block, false).expect_err("a loss");
        assert!(
            by_a_checkpoint.contains("a cut at"),
            "{by_a_checkpoint} in {env:?}"
        );
    }
    assert_eq!(ran, 4);
}

/// A file system that deallocated and then refuses: the step was sized
/// with write-zeroes counting nothing, so it ends with the record at
/// which the refusal came, having written that one record's zeros,
/// and the steps after it are sized by what they write. Nothing is
/// lost and the volume does not fail. Mutant: the step goes on and
/// writes all it took, far beyond its bound.
#[test]
fn a_step_ends_where_the_file_system_first_refuses_to_deallocate() {
    fn scenario(env: &Env) -> Outcome {
        let (_disk, mut store) = env.fresh()?;
        write(&mut store, 0, 256, 9)?;
        write(&mut store, 256, 256, 9)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        for n in 0..25 {
            zeroes(&mut store, 8 + n * 16, 16)?;
        }
        flush(&mut store)?;
        let bound = StepBound {
            payload: 32 * BLOCK,
            records: 30,
        };
        // The first step does not know yet and takes by length.
        let first = step(&mut store, bound)?;
        if store.log_state().zeroes != Zeroes::Deallocated || first.records != 2 {
            return Err(format!("the first step: {first:?}"));
        }
        store.set_refuse_holes(true);
        let at = step(&mut store, bound)?;
        if at.records != 1 || at.payload != 16 * BLOCK || !at.more {
            return Err(format!("the step with the refusal: {at:?}"));
        }
        if store.log_state().zeroes != Zeroes::Written {
            return Err(format!("{:?}", store.log_state().zeroes));
        }
        let mut steps = 0;
        loop {
            let done = step(&mut store, bound)?;
            if done.payload > bound.payload || done.records > 2 {
                return Err(format!("a step beyond its bound: {done:?}"));
            }
            steps += 1;
            if !done.more {
                break;
            }
        }
        if steps != 11 {
            return Err(format!("22 records of 16 blocks in {steps} steps of 32"));
        }
        if store.is_failed() {
            return Err("the volume failed".into());
        }
        expect(&mut store, 0, 8, 9, "before the ranges")?;
        expect(&mut store, 8, 200, 0, "the ranges")?;
        expect(&mut store, 208, 200, 0, "the ranges")?;
        expect(&mut store, 408, 104, 9, "after the ranges")?;
        // A whole checkpoint that meets the refusal takes all the same.
        let (_disk, mut store) = env.fresh()?;
        write(&mut store, 0, 256, 9)?;
        write(&mut store, 256, 256, 9)?;
        zeroes(&mut store, 8, 16)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        for n in 1..25 {
            zeroes(&mut store, 8 + n * 16, 16)?;
        }
        write(&mut store, 500, 4, 5)?;
        flush(&mut store)?;
        store.set_refuse_holes(true);
        checkpoint(&mut store)?;
        if store.log_start() < store.log_durable() {
            return Err("a checkpoint left durable records".into());
        }
        expect(&mut store, 8, 200, 0, "the ranges, by a checkpoint")?;
        expect(&mut store, 208, 200, 0, "the ranges, by a checkpoint")?;
        expect(&mut store, 408, 92, 9, "after the ranges, by a checkpoint")?;
        expect(&mut store, 500, 4, 5, "the write, by a checkpoint")
    }
    caught(scenario, Mutants::REFUSAL_ENDS_NO_STEP);
}

/// An I/O error from the deallocation is a failed write into the home
/// image: the volume fails, as for any other, and nothing is lost,
/// because the log keeps the record: the next open applies it again.
#[test]
fn a_deallocation_that_fails_fails_the_volume_and_loses_nothing() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        write(&mut store, 0, 256, 9)?;
        flush(&mut store)?;
        checkpoint(&mut store)?;
        zeroes(&mut store, 33, 100)?;
        write(&mut store, 200, 4, 5)?;
        flush(&mut store)?;
        disk.fail_deallocation();
        if store.checkpoint_step(StepBound::DAEMON).is_ok() {
            return Err("the step went on after the deallocation failed".into());
        }
        if !store.is_failed() {
            return Err("the volume did not fail".into());
        }
        drop(store);
        disk.kill();
        let mut store = env.reopen_ack(&disk)?;
        let check = |store: &mut SimStore, when: &str| -> Outcome {
            expect(store, 0, 33, 9, when)?;
            expect(store, 33, 100, 0, when)?;
            expect(store, 133, 67, 9, when)?;
            expect(store, 200, 4, 5, when)
        };
        check(&mut store, "after the failure")?;
        while step(&mut store, StepBound::DAEMON)?.more {}
        check(&mut store, "after the steps of the next instance")
    }
    holds(scenario);
}

/// Steps over holes, and over holes mixed with writes, can be cut
/// anywhere. The volume is filled and checkpointed; then in each of
/// its 128 groups of eight blocks, blocks 1 to 5 are zeroed (a range
/// that begins and ends inside a write unit of four blocks) and block
/// 7 is written. The steps are stopped after a number of writes,
/// deallocations and syncs with a power cut, and so is the open that
/// follows. After the next open every block reads as what was flushed:
/// the zeroed ones as zeros, their neighbours in the same units as
/// they were, the written ones as written; and the steps that remain
/// finish the work. Mutants, as for writes: the log given up before
/// the home image is durable; the neighbours not saved.
#[test]
fn steps_of_holes_and_writes_can_be_cut_anywhere() {
    fn scenario(env: &Env) -> Outcome {
        let bound = StepBound {
            payload: 16 * BLOCK,
            records: 16,
        };
        for stop in (0..30).chain([44, 61, 90, 150, 300, 700]) {
            let (disk, mut store) = env.fresh()?;
            for mib in 0..4 {
                write(&mut store, mib * 256, 256, 9)?;
            }
            flush(&mut store)?;
            checkpoint(&mut store)?;
            for group in 0..128u64 {
                zeroes(&mut store, group * 8 + 1, 5)?;
                write(&mut store, group * 8 + 7, 1, (group % 200) as u8 + 10)?;
            }
            flush(&mut store)?;
            disk.halt_after(stop);
            let cut_off = loop {
                match store.checkpoint_step(bound) {
                    Err(_) => break true,
                    Ok(done) if !done.more => break false,
                    Ok(done) if done.records > 16 => {
                        return Err(format!("a step took {done:?}"));
                    }
                    Ok(_) => {}
                }
            };
            if !cut_off && stop < 3 {
                return Err(format!("the steps were not stopped at {stop}"));
            }
            drop(store);
            let lost = env.lost();
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            disk.set_write_unit(u64::from(env.unit));
            disk.halt_after(stop % 7);
            drop(Store::open(disk.clone(), env.options()));
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            disk.set_write_unit(u64::from(env.unit));
            let mut store = env.reopen_ack(&disk)?;
            let check = |store: &mut SimStore, when: &str| -> Outcome {
                for group in 0..128u64 {
                    let what = format!("group {group} {when} a cut at {stop}");
                    expect(store, group * 8, 1, 9, &what)?;
                    expect(store, group * 8 + 1, 5, 0, &what)?;
                    expect(store, group * 8 + 6, 1, 9, &what)?;
                    expect(store, group * 8 + 7, 1, (group % 200) as u8 + 10, &what)?;
                }
                Ok(())
            };
            check(&mut store, "after")?;
            while step(&mut store, bound)?.more {}
            if store.log_start() < store.log_durable() {
                return Err("steps after the restart left durable records".into());
            }
            check(&mut store, "and the remaining steps after")?;
        }
        Ok(())
    }
    caught(scenario, Mutants::RECLAIM_BEFORE_HOME_SYNC);
    caught_where(scenario, Mutants::NO_PREIMAGES, garbled_neighbours);
}

/// A write inside a range that an earlier record zeroed, and a second
/// write-zeroes inside the first with a write inside that: the later
/// record wins wherever the steps are cut. Steps of at most five
/// records, so a step often ends between a write-zeroes and the write
/// into its range; the power is cut at every point and again inside
/// the open; the content is checked after the open, after the
/// remaining steps and after one more open.
#[test]
fn a_write_inside_a_zeroed_range_can_be_cut_anywhere() {
    fn scenario(env: &Env) -> Outcome {
        let bound = StepBound {
            payload: 16 * BLOCK,
            records: 5,
        };
        let byte = |group: u64| (group % 200) as u8 + 10;
        for stop in (0..40).chain([44, 61, 90, 150, 300, 700]) {
            let (disk, mut store) = env.fresh()?;
            write(&mut store, 0, 256, 9)?;
            write(&mut store, 256, 256, 9)?;
            flush(&mut store)?;
            checkpoint(&mut store)?;
            for group in 0..64u64 {
                zeroes(&mut store, group * 8 + 1, 5)?;
                write(&mut store, group * 8 + 3, 1, byte(group))?;
                if group % 3 == 0 {
                    zeroes(&mut store, group * 8 + 2, 2)?;
                    write(&mut store, group * 8 + 2, 1, byte(group) + 1)?;
                }
            }
            flush(&mut store)?;
            disk.halt_after(stop);
            loop {
                match store.checkpoint_step(bound) {
                    Err(_) => break,
                    Ok(done) if !done.more => break,
                    Ok(done) if done.records > 5 => {
                        return Err(format!("a step took {done:?}"));
                    }
                    Ok(_) => {}
                }
            }
            drop(store);
            let lost = env.lost();
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            disk.set_write_unit(u64::from(env.unit));
            disk.halt_after(stop % 7);
            drop(Store::open(disk.clone(), env.options()));
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            disk.set_write_unit(u64::from(env.unit));
            let mut store = env.reopen_ack(&disk)?;
            let check = |store: &mut SimStore, when: &str| -> Outcome {
                for group in 0..64u64 {
                    let what = format!("group {group} {when} a cut at {stop}");
                    let base = group * 8;
                    expect(store, base, 1, 9, &what)?;
                    expect(store, base + 1, 1, 0, &what)?;
                    if group % 3 == 0 {
                        expect(store, base + 2, 1, byte(group) + 1, &what)?;
                        expect(store, base + 3, 1, 0, &what)?;
                    } else {
                        expect(store, base + 2, 1, 0, &what)?;
                        expect(store, base + 3, 1, byte(group), &what)?;
                    }
                    expect(store, base + 4, 2, 0, &what)?;
                    expect(store, base + 6, 2, 9, &what)?;
                }
                Ok(())
            };
            check(&mut store, "after")?;
            while step(&mut store, bound)?.more {}
            if store.log_start() < store.log_durable() {
                return Err("steps after the restart left durable records".into());
            }
            check(&mut store, "and the remaining steps after")?;
            let mut store = env.reopen_ack(&disk)?;
            check(&mut store, "and one more open after")?;
        }
        Ok(())
    }
    caught(scenario, Mutants::RECLAIM_BEFORE_HOME_SYNC);
    caught_where(scenario, Mutants::NO_PREIMAGES, garbled_neighbours);
}

/// Behind a log of write-zeroes records a write waits for one step,
/// once the store knows that the file system deallocates: the records
/// count no bytes, so a step takes as many as its count of records
/// allows, which is more log than a write adds (here 64 records of
/// 4 KiB against a write of 64 KiB; in the daemon 1024 against one of
/// 1 MiB). On a file system that cannot deallocate the cost is back:
/// a write is followed by several steps. Record alignment 4096, the
/// daemon's. Mutant: the zeros are written.
#[test]
fn behind_deallocated_write_zeroes_a_write_waits_for_one_step() {
    const WIDE: Marks = Marks {
        step: StepBound {
            payload: 128 << 10,
            records: 64,
        },
        ..MARKS
    };
    fn most_steps(env: &Env, holes: bool) -> Result<u64, String> {
        let (disk, mut store, _) = a_log_of_short_records(env, Short::Zeroes)?;
        disk.set_deallocation(holes);
        flush(&mut store)?;
        // The first step finds out how the file system treats them.
        step(&mut store, WIDE.step)?;
        store.set_marks(Some(WIDE));
        let mut most = 0;
        for n in 0..12u64 {
            let before = store.log_state();
            write(&mut store, n * 16, 16, n as u8 + 1)?;
            let after = store.log_state();
            if after.live() >= WIDE.high + ONE_REQUEST {
                return Err(format!("the log holds {} bytes", after.live()));
            }
            // Only while short records are what a step finds.
            if after.start < before.end - WIDE.high / 2 {
                most = most.max(after.steps - before.steps);
            }
        }
        for n in 0..12u64 {
            expect(&mut store, n * 16, 16, n as u8 + 1, "under the load")?;
        }
        expect(&mut store, 512, 16, 0, "the zeroed blocks")?;
        Ok(most)
    }
    fn scenario(env: &Env) -> Outcome {
        match most_steps(env, true)? {
            1 => Ok(()),
            most => Err(format!("a write was followed by {most} steps")),
        }
    }
    fn at_the_daemons_alignment(env: &Env) -> bool {
        env.align == 4096
    }
    for env in environments(0).into_iter().filter(at_the_daemons_alignment) {
        scenario(&env).unwrap_or_else(|why| panic!("{env:?}: {why}"));
        let most = most_steps(&env, false).unwrap();
        assert!(most >= 2, "without holes a write waited for {most} steps");
    }
    let as_before = environments(Mutants::ZEROES_WRITTEN);
    for env in as_before.into_iter().filter(at_the_daemons_alignment) {
        assert!(
            scenario(&env).is_err(),
            "zeros written and one step a write in {env:?}"
        );
    }
}

/// Prints what one read of the monotonic clock costs here, and what a
/// timed request adds: run by hand
/// (`cargo test -p skyzfs-local --release -- --ignored clock_numbers --nocapture`).
#[test]
#[ignore = "prints numbers; run by hand"]
fn clock_numbers() {
    const READS: u32 = 5_000_000;
    let began = std::time::Instant::now();
    let mut last = began;
    for _ in 0..READS {
        last = std::hint::black_box(std::time::Instant::now());
    }
    let per_read = (last - began).as_nanos() as f64 / f64::from(READS);
    let mut spent = Spent::default();
    let began = std::time::Instant::now();
    for _ in 0..READS {
        let at = std::time::Instant::now();
        spent.add(std::hint::black_box(at.elapsed()));
    }
    let per_pair = began.elapsed().as_nanos() as f64 / f64::from(READS);
    println!("one clock read: {per_read:.1} ns; two reads and an add: {per_pair:.1} ns");
    assert_eq!(spent.count, u64::from(READS));
}

/// Prints how much of a worker's time that is not idle the step's
/// parts account for when it does nothing but step, on the simulated
/// disk: run by hand
/// (`cargo test -p skyzfs-local -- --ignored step_parts_numbers --nocapture`).
#[test]
#[ignore = "prints numbers; run by hand"]
fn step_parts_numbers() {
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    for n in 0..800u64 {
        write(&mut store, (n % 60) * 16, 16, (n % 200) as u8 + 1).unwrap();
    }
    flush(&mut store).unwrap();
    store.set_marks(Some(Marks { low: 1, ..MARKS }));
    let served = store.serve().unwrap();
    let state = comes_by_itself(&served, "the log worked off", |s| s.live() < MARKS.low);
    let (_, timing) = served.shown();
    let mut steps = 0;
    for (name, ns) in ACTIVITIES.iter().zip(timing.worker) {
        println!("{name}: {ns} ns");
        if name.starts_with("step_") || *name == "own_sync" {
            steps += ns;
        }
    }
    let busy: u64 = timing.worker[1..].iter().sum();
    println!(
        "{} steps; the parts are {steps} ns of the {busy} ns the worker was not idle: {:.1}%",
        state.steps,
        100.0 * steps as f64 / busy as f64
    );
}

/// The times of a served store hold at every look, and not only
/// between two spans. A thread that waits for work has its wait
/// counted as idle up to the look; a thread that is doing something
/// has its sums complete up to the end of its last span, and says up
/// to when (`Timing::worker_as_of_ns`, `Timing::reader_as_of_ns`). So
/// the worker's activities and the reader's each sum, between any two
/// looks, to the difference of their own moment exactly, with
/// requests between the looks or none.
///
/// And a thread that waits for work shows the wait as idle, whole:
/// with nothing submitted, there come two looks between which every
/// nanosecond of both threads is idle. Before this, the span open at a
/// look was in no sum, and a worker idle for seconds put the identity
/// out by seconds.
///
/// With several readers, the readers' times are summed over them, each
/// complete up to its own moment, and those moments are summed: the
/// identity is exact with one reader and with three, and with every
/// thread idle the readers' moment moves by exactly three times the
/// worker's.
#[test]
fn the_threads_times_sum_to_the_time_between_two_looks_exactly() {
    for readers in [1, 3] {
        threads_times_sum_exactly(readers);
    }
}

fn threads_times_sum_exactly(readers: usize) {
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    write(&mut store, 0, 4, 9).unwrap();
    flush(&mut store).unwrap();
    store.core.readers = readers;
    let mut served = store.serve().unwrap();
    let mut before = served.timing();
    let mut done = Vec::new();
    for round in 0..40u64 {
        if round % 2 == 0 {
            let wanted = done.len() + 2;
            submit(&mut served, write_request(2 * round, round % 8, 1, 7)).unwrap();
            submit(&mut served, read_request(2 * round + 1, round % 8, 2)).unwrap();
            take(&mut served, &mut done, wanted, "the round").unwrap();
        }
        if round % 10 == 5 {
            // A flush and a checkpoint: a step's parts and its
            // barriers' waits are in the worker's time too.
            let flushes = done.len() + 1;
            served
                .submit(Request {
                    id: RequestId(1000 + round),
                    op: Op::Flush,
                })
                .unwrap();
            take(&mut served, &mut done, flushes, "the flush").unwrap();
            let (reply, checkpointed) = mpsc::channel();
            assert!(served.send(Command::Checkpoint(reply)));
            assert!(matches!(
                checkpointed.recv_timeout(Duration::from_secs(20)),
                Ok(Ok(_))
            ));
        }
        let now = served.timing();
        identities_hold(&before, &now, &format!("round {round}"));
        before = now;
    }
    assert!(before.barrier_home.count >= 4, "{before:?}");
    assert!(before.step_apply.sum_ns > 0, "{before:?}");
    assert!(
        before.worker[ACTIVITIES
            .iter()
            .position(|a| *a == "step_barrier")
            .unwrap()]
            > 0
    );
    // Nothing more is submitted: both threads go to wait for work, and
    // then the time between two looks is all idle.
    let began = std::time::Instant::now();
    loop {
        let now = served.timing();
        let between = (
            now.worker_as_of_ns - before.worker_as_of_ns,
            now.reader_as_of_ns - before.reader_as_of_ns,
        );
        let idle = (
            now.worker[0] - before.worker[0],
            now.reader[0] - before.reader[0],
        );
        // Every thread idle at both looks: the readers' moment moves by
        // the worker's once per reader.
        if between.0 > 0 && idle == between && between.1 == readers as u64 * between.0 {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "no two looks with both threads idle between them: {idle:?} of {between:?}"
        );
        before = now;
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Between two looks, each thread's activities sum to the difference of
/// its own moment, and no sum or count went down.
fn identities_hold(before: &Timing, now: &Timing, when: &str) {
    let worker = now.worker.iter().sum::<u64>() - before.worker.iter().sum::<u64>();
    let reader = now.reader.iter().sum::<u64>() - before.reader.iter().sum::<u64>();
    assert_eq!(
        worker,
        now.worker_as_of_ns - before.worker_as_of_ns,
        "the worker, {when}"
    );
    assert_eq!(
        reader,
        now.reader_as_of_ns - before.reader_as_of_ns,
        "the reader, {when}"
    );
    let (earlier, later) = (monotonic(before), monotonic(now));
    for (at, (e, l)) in earlier.iter().zip(&later).enumerate() {
        assert!(
            l >= e,
            "{when}: sum {at} went down from {e} to {l}: {before:?} then {now:?}"
        );
    }
}

/// Every sum and count of a [`Timing`], which can only grow.
fn monotonic(t: &Timing) -> Vec<u64> {
    let mut out = Vec::new();
    for kind in &t.requests {
        out.extend([kind.wait.count, kind.wait.sum_ns, kind.call.sum_ns]);
    }
    for part in [
        t.step_walk,
        t.step_save,
        t.step_apply,
        t.step_home_sync,
        t.step_start,
        t.step_end,
        t.step_own_sync,
        t.gate,
        t.barrier_home,
        t.barrier_reuse,
    ] {
        out.extend([part.count, part.sum_ns]);
    }
    out.extend(t.worker);
    out.extend(t.wait_by.iter().flatten());
    out.extend(t.reader);
    out.extend(t.read_unflushed);
    out.extend([t.worker_as_of_ns, t.reader_as_of_ns]);
    out
}

/// No sum goes down from one look to the next, with the worker held
/// inside a step and the reader inside a read: a look does not count
/// what a thread is doing to an activity it did before, which the next
/// look would take back. Then the same with a client's flush held
/// inside its sync of the log. Shown by hand, not by a mutant: a look that
/// closed a busy thread's open span under the activity it last showed
/// ("other" for the worker, "idle" for a reader whose read is under
/// way) fails here, as `time_worker_other_ns` goes down once the step
/// shows its parts.
#[test]
fn no_sum_goes_down_between_looks_taken_inside_a_step_and_a_read() {
    let env = environments(0)[0];
    let (_disk, mut store) = env.fresh().unwrap();
    write(&mut store, 0, 4, 9).unwrap();
    write(&mut store, 8, 4, 3).unwrap();
    flush(&mut store).unwrap();
    let (step, reader) = (Gate::new("looks-step"), Gate::new("looks-read"));
    store.set_step_pause_at(step.pause(), StepPoint::Written);
    store.set_read_pause(reader.pause());
    let mut served = store.serve().unwrap();
    let _opened = Opened(&[&step, &reader]);
    let mut looks = vec![served.timing()];
    let (reply, checkpointed) = mpsc::channel();
    assert!(served.send(Command::Checkpoint(reply)));
    step.wait_reached("the step").unwrap();
    submit(&mut served, read_request(1, 8, 2)).unwrap();
    reader.wait_reached("the read").unwrap();
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(5));
        looks.push(served.timing());
    }
    reader.open();
    let mut done = Vec::new();
    take(&mut served, &mut done, 1, "the read").unwrap();
    looks.push(served.timing());
    step.open();
    assert!(matches!(
        checkpointed.recv_timeout(Duration::from_secs(20)),
        Ok(Ok(_))
    ));
    looks.push(served.timing());
    // And once both wait for work again.
    std::thread::sleep(Duration::from_millis(5));
    looks.push(served.timing());
    for (at, pair) in looks.windows(2).enumerate() {
        identities_hold(&pair[0], &pair[1], &format!("look {at} to {}", at + 1));
    }
    let (first, last) = (&looks[0], &looks[looks.len() - 1]);
    assert!(last.step_apply.sum_ns > first.step_apply.sum_ns, "{last:?}");
    drop(_opened);
    drop(served);

    // A request under way: a flush held inside its sync of the log.
    let (_disk, mut store) = env.fresh().unwrap();
    let sync = Gate::new("looks-sync");
    store.set_sync_pause(sync.pause());
    let mut served = store.serve().unwrap();
    let _opened = Opened(&[&sync]);
    let mut done = Vec::new();
    submit(&mut served, write_request(1, 8, 8, 0x44)).unwrap();
    take(&mut served, &mut done, 1, "the write").unwrap();
    let mut looks = vec![served.timing()];
    served
        .submit(Request {
            id: RequestId(2),
            op: Op::Flush,
        })
        .unwrap();
    sync.wait_reached("the flush's sync").unwrap();
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(5));
        looks.push(served.timing());
    }
    sync.open();
    take(&mut served, &mut done, 2, "the flush").unwrap();
    looks.push(served.timing());
    std::thread::sleep(Duration::from_millis(5));
    looks.push(served.timing());
    for (at, pair) in looks.windows(2).enumerate() {
        identities_hold(
            &pair[0],
            &pair[1],
            &format!("flush, look {at} to {}", at + 1),
        );
    }
    let flush = ACTIVITIES.iter().position(|a| *a == "flush").unwrap();
    assert!(
        looks[looks.len() - 1].worker[flush] >= 25_000_000,
        "{looks:?}"
    );
}

/// A store on a new disk whose hint hands writes over once `unit`
/// bytes wait (0: never).
fn hinted(unit: u64) -> (SimDisk, SimStore) {
    hinted_in(environments(0)[0], unit)
}

/// The same in `env`.
fn hinted_in(env: Env, unit: u64) -> (SimDisk, SimStore) {
    let disk = SimDisk::new(7);
    disk.set_write_unit(u64::from(env.unit));
    let options = OpenOptions {
        writeback_unit: unit,
        home_writeback_unit: unit,
        ..env.options()
    };
    let key = Key(*b"a key for tests!");
    let store = Store::create_keyed(disk.clone(), &env.config(), key, options).unwrap();
    (disk, store)
}

/// The hints made for files whose names start with `prefix`.
fn hints_of(disk: &SimDisk, prefix: &str) -> Vec<(u64, u64)> {
    disk.writebacks()
        .into_iter()
        .filter(|(name, _, _)| name.starts_with(prefix))
        .map(|(_, offset, len)| (offset, len))
        .collect()
}

/// The log's writes are handed to the device once a unit of them waits,
/// and as often as the unit says: 64 writes of a block, records of
/// 4160 bytes with an alignment of 64, are 260 KiB in one segment,
/// handed over in ranges that follow each other, each the first run of
/// records to reach the unit (16 records), and less than a unit is left
/// waiting. The flush then waits for less than a unit that was not
/// handed over, and says so.
#[test]
fn the_logs_writes_are_handed_over_once_a_unit_waits() {
    const UNIT: u64 = 64 << 10;
    let (disk, mut store) = hinted(UNIT);
    let before = hints_of(&disk, "log.").len();
    let first = store.timing().writeback[0];
    for block in 0..64 {
        write(&mut store, block, 1, 5).unwrap();
    }
    let hints = hints_of(&disk, "log.");
    let new = &hints[before..];
    const RECORD: u64 = 4160;
    let run = UNIT.div_ceil(RECORD) * RECORD;
    assert_eq!(new.len() as u64, 64 * RECORD / run, "{new:?}");
    for pair in new.windows(2) {
        assert_eq!(pair[0].0 + pair[0].1, pair[1].0, "{new:?}");
    }
    assert!(new.iter().all(|(_, len)| *len == run), "{new:?}");
    assert!(64 * RECORD - new.len() as u64 * run < UNIT);
    let timing = store.timing();
    assert_eq!(timing.writeback[0].count - first.count, new.len() as u64);
    assert_eq!(
        timing.writeback[0].bytes - first.bytes,
        new.len() as u64 * run
    );
    assert!(!timing.writeback[0].off);
    flush(&mut store).unwrap();
    let synced = store.timing().syncs[0];
    assert_eq!(synced.count, 1);
    assert!(synced.bytes >= 64 * RECORD, "{synced:?}");
    assert!(synced.bytes - synced.handed < UNIT, "{synced:?}");
}

/// The home image's writes in a step's apply are handed over once a
/// unit of them waits; what the step's sync of the home image waited
/// for is counted, and so is the preimage file's (a write unit of
/// 16 KiB, so that the step saves the neighbours of its blocks).
#[test]
fn the_home_images_writes_are_handed_over_in_the_apply() {
    const UNIT: u64 = 64 << 10;
    let env = environments(0)[2];
    assert_eq!(env.unit, 16384);
    let (disk, mut store) = hinted_in(env, UNIT);
    // Every other block of the first 128, so that the step saves
    // neighbours in a preimage file.
    for block in (0..128).step_by(2) {
        write(&mut store, block, 1, 6).unwrap();
    }
    flush(&mut store).unwrap();
    store.checkpoint().unwrap();
    let hints = hints_of(&disk, HOME_NAME);
    // 64 blocks of 4 KiB: four units.
    assert_eq!(hints.len(), 4, "{hints:?}");
    let timing = store.timing();
    assert_eq!(timing.writeback[1].count, 4);
    assert_eq!(timing.writeback[1].bytes, 4 * UNIT);
    let home = timing.syncs[3];
    assert!(home.count >= 1);
    assert_eq!(
        (home.bytes, home.handed),
        (64 * BLOCK, 4 * UNIT),
        "{home:?}"
    );
    let preimage = timing.syncs[4];
    assert!(preimage.count >= 1 && preimage.bytes > 0, "{preimage:?}");
    // The step's own superblock write.
    assert!(timing.syncs[5].count >= 1 && timing.syncs[5].bytes > 0);
    // The preimage file is written whole and synced at once: no hint.
    assert!(hints_of(&disk, PREIMAGE_NAME).is_empty());
}

/// With the unit 0 the hint is never made, and nothing else differs:
/// the same writes, flush and step read back the same.
#[test]
fn with_the_unit_zero_the_hint_is_never_made() {
    let (disk, mut store) = hinted(0);
    for block in 0..512 {
        write(&mut store, block, 1, 7).unwrap();
    }
    megabyte(&mut store, 2, 8).unwrap();
    flush(&mut store).unwrap();
    store.checkpoint().unwrap();
    assert!(disk.writebacks().is_empty(), "{:?}", disk.writebacks());
    assert_eq!(disk.stats().writebacks, 0);
    let timing = store.timing();
    assert_eq!(timing.writeback, [Hinted::default(); 2]);
    assert_eq!(timing.syncs[0].handed, 0);
    assert!(timing.syncs[0].bytes > 0);
    assert_eq!(read(&mut store, 511, 1).unwrap(), vec![7u8; BLOCK as usize]);
}

/// A file system without the call: the hint answers "unsupported", is
/// switched off for that file and says so, and the volume goes on and
/// keeps what it was given.
#[test]
fn an_unsupported_hint_switches_off_and_the_volume_goes_on() {
    let (disk, mut store) = hinted(64 << 10);
    disk.set_writeback(false);
    for block in 0..200 {
        write(&mut store, block, 1, 9).unwrap();
    }
    flush(&mut store).unwrap();
    store.checkpoint().unwrap();
    let timing = store.timing();
    assert!(
        timing.writeback[0].off && timing.writeback[1].off,
        "{timing:?}"
    );
    assert_eq!(
        (timing.writeback[0].count, timing.writeback[1].count),
        (0, 0)
    );
    assert!(disk.writebacks().is_empty());
    // Asked again it is not: the file's hint is off.
    disk.set_writeback(true);
    for block in 200..400 {
        write(&mut store, block, 1, 9).unwrap();
    }
    assert!(hints_of(&disk, "log.").is_empty());
    flush(&mut store).unwrap();
    assert_eq!(read(&mut store, 399, 1).unwrap(), vec![9u8; BLOCK as usize]);
    let env = environments(0)[0];
    let mut again = env.reopen(&disk).unwrap();
    assert_eq!(read(&mut again, 0, 1).unwrap(), vec![9u8; BLOCK as usize]);
}

/// Any other error of the hint is a failed write of the file: the
/// volume fails, the request that made the hint is answered as failed,
/// every later one too, and the next open finds the log marked.
#[test]
fn a_hint_that_fails_fails_the_volume() {
    for name in [crate::log::slot_name(0), HOME_NAME.to_string()] {
        let (disk, mut store) = hinted(64 << 10);
        disk.fail_writeback_of(&name);
        let mut failed = None;
        for block in 0..64 {
            if write(&mut store, block, 1, 3).is_err() {
                failed = Some(block);
                break;
            }
        }
        if name == HOME_NAME {
            assert_eq!(failed, None, "the log's hint failed: {name}");
            flush(&mut store).unwrap();
            assert!(store.checkpoint().is_err(), "{name}");
        } else {
            assert!(failed.is_some(), "{name}");
        }
        assert!(write(&mut store, 100, 1, 3).is_err(), "{name}");
        assert!(read(&mut store, 0, 1).is_err(), "{name}");
        let env = environments(0)[0];
        disk.clear_faults();
        let reopened = env.reopen(&disk);
        assert!(reopened.is_err(), "{name}: the log was not marked");
        assert!(env.reopen_ack(&disk).is_ok(), "{name}");
    }
}

/// A hint the system declines this time (short of memory: under
/// writeback that does not wait the kernel gives up on a page it
/// cannot get memory for) does not fail the volume. It is counted, the
/// bytes stay waiting and are offered again in one range with the next
/// unit's; and a hint declined just before a sync leaves its bytes to
/// the sync.
#[test]
fn a_hint_declined_this_time_is_offered_again_and_the_volume_goes_on() {
    const UNIT: u64 = 64 << 10;
    const RECORD: u64 = 4160;
    let run = UNIT.div_ceil(RECORD) * RECORD;
    let (disk, mut store) = hinted(UNIT);
    let before = hints_of(&disk, "log.").len();
    disk.refuse_writeback(1);
    // One unit's records: the hint is declined.
    for block in 0..run / RECORD {
        write(&mut store, block, 1, 4).unwrap();
    }
    let timing = store.timing();
    assert_eq!(
        (timing.writeback[0].count, timing.writeback[0].refused),
        (0, 1)
    );
    assert_eq!(hints_of(&disk, "log.").len(), before);
    // One unit more: both are offered in one range, from where the
    // declined one began.
    for block in run / RECORD..2 * run / RECORD {
        write(&mut store, block, 1, 4).unwrap();
    }
    let hints = hints_of(&disk, "log.");
    assert_eq!(hints.len(), before + 1, "{hints:?}");
    assert_eq!(hints[before].1, 2 * run, "{hints:?}");
    let timing = store.timing();
    assert_eq!(timing.writeback[0].count, 1);
    assert_eq!(timing.writeback[0].bytes, 2 * run);
    flush(&mut store).unwrap();
    let synced = store.timing().syncs[0];
    assert_eq!((synced.bytes, synced.handed), (2 * run, 2 * run));

    // Declined, and then a flush before the next unit: the sync covers
    // the bytes, none of them handed over.
    disk.refuse_writeback(1);
    for block in 0..run / RECORD {
        write(&mut store, 100 + block, 1, 5).unwrap();
    }
    flush(&mut store).unwrap();
    let timing = store.timing();
    assert_eq!(timing.writeback[0].refused, 2);
    let synced = timing.syncs[0];
    assert_eq!(synced.count, 2);
    assert!(
        synced.bytes >= 3 * run && synced.handed == 2 * run,
        "{synced:?}"
    );
    assert!(!store.core.failed);
    let env = environments(0)[0];
    let mut again = env.reopen(&disk).unwrap();
    assert_eq!(read(&mut again, 100, 1).unwrap(), vec![5u8; BLOCK as usize]);
    assert_eq!(read(&mut again, 0, 1).unwrap(), vec![4u8; BLOCK as usize]);
}

/// A log that rolls to a new segment file with the hint on: what waits
/// in the old file, its closing pad included, is handed over before the
/// writes go to the new one, and the new file's zeros and its records
/// are handed over there.
#[test]
fn a_roll_hands_over_what_waits_in_the_segment_it_leaves() {
    const UNIT: u64 = 64 << 10;
    let (disk, mut store) = hinted(UNIT);
    // More than one segment of records of 4160 bytes.
    let records = SEGMENT / 4160 + 20;
    for block in 0..records {
        write(&mut store, block % 1024, 1, 6).unwrap();
    }
    let all = disk.writebacks();
    let first_new = all
        .iter()
        .position(|(name, _, _)| name == "log.000001")
        .expect("the log rolled to a new file");
    // Just before the new file's first hint: the old file's last, the
    // rest of it that had not reached a unit.
    let (name, offset, len) = &all[first_new - 1];
    assert_eq!(name, "log.000000", "{all:?}");
    assert!(*len < UNIT, "a remainder, not a unit: {all:?}");
    let old: Vec<(u64, u64)> = hints_of(&disk, "log.000000");
    for pair in old.windows(2) {
        assert_eq!(pair[0].0 + pair[0].1, pair[1].0, "{old:?}");
    }
    // It reaches the closing pad's header, at the end of the records.
    let end = offset + len;
    assert!(end > SEGMENT - 2 * 4160 && end <= SEGMENT, "{end} {old:?}");
    // The new file: its zeros, whole, then its records.
    assert_eq!(all[first_new], ("log.000001".to_string(), 0, SEGMENT));
    assert!(
        hints_of(&disk, "log.000001").len() >= 2,
        "{:?}",
        hints_of(&disk, "log.000001")
    );
}

// ---------------------------------------------------------------------
// The home image's sync with the helper.

/// Wait for `what`, looking every millisecond, for up to 20 s.
fn until(mut what: impl FnMut() -> bool, which: &str) -> Outcome {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !what() {
        if std::time::Instant::now() > deadline {
            return Err(format!("no {which} in 20 s"));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// One request through a served store, and its completion.
fn through(served: &mut Served, id: u64, op: Op) -> Result<Completion, String> {
    served
        .submit(Request {
            id: RequestId(id),
            op,
        })
        .map_err(|e| e.to_string())?;
    let mut done = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while done.is_empty() {
        served.wait(&mut done, Duration::from_millis(100));
        if std::time::Instant::now() > deadline {
            return Err(format!("no answer to request {id}"));
        }
    }
    done.pop().ok_or_else(|| "no completion".to_string())
}

/// A served store whose rule has handed a step's home-image sync over,
/// below the high mark, and a disk that holds that sync at its end.
struct Helping {
    disk: SimDisk,
    served: Served,
    /// Where the log starts: the held step's batch begins here.
    start: Lsn,
    /// The record whose payload, 16 blocks of byte `first + 1` from
    /// block `first * 16`, is the batch's first.
    first: u64,
}

/// `records` records of 16 blocks each (record n holds byte n + 1 from
/// block n * 16) are written and made durable, at least `marks.high` of
/// log. A hold at the end of record `hold_after` keeps the rule's first
/// steps there, below the high mark and not below the low one: still
/// stepping. Then the disk holds the home image's syncs, the hold goes,
/// and the next step hands its sync over.
fn helping(env: &Env, records: u64, hold_after: u64, marks: Marks) -> Result<Helping, String> {
    helping_with(env, records, hold_after, marks, |_| {})
}

/// The same, with `prepare` done to the store before it is served.
fn helping_with(
    env: &Env,
    records: u64,
    hold_after: u64,
    marks: Marks,
    prepare: impl FnOnce(&mut SimStore),
) -> Result<Helping, String> {
    let (disk, mut store) = env.fresh()?;
    prepare(&mut store);
    let mut ends = Vec::new();
    for n in 0..records {
        write(&mut store, n * 16, 16, n as u8 + 1)?;
        ends.push(store.position());
    }
    flush(&mut store)?;
    if store.log_state().live() < marks.high {
        return Err("the log is below the high mark".into());
    }
    store.set_marks(Some(marks));
    let hold = ends[hold_after as usize - 1];
    store.set_hold(Some(hold)).map_err(|e| e.to_string())?;
    let served = store.serve().map_err(|e| e.to_string())?;
    until(
        || {
            let state = served.log_state();
            state.start == hold.0 && !state.stepping
        },
        "steps to the hold",
    )?;
    let state = served.log_state();
    if state.live() >= marks.high || state.live() < marks.low {
        return Err(format!("the log rests outside the marks: {state:?}"));
    }
    disk.hold_syncs_of(HOME_NAME);
    let begun = disk.stats().sync_begins;
    served.set_hold(None).map_err(|e| e.to_string())?;
    until(
        || disk.stats().sync_begins > begun,
        "step that hands its sync over",
    )?;
    Ok(Helping {
        disk,
        served,
        start: hold,
        first: hold_after,
    })
}

/// `op` waits for the helper's report: a thread lets the held sync end
/// only once a wait for it has begun, and finds that `op` has not
/// returned by then. Events, not times: what the thread looks for is a
/// caller blocked at the held sync's end.
fn waits_for_the_report(disk: &SimDisk, op: impl FnOnce() -> Outcome) -> Outcome {
    use std::sync::atomic::{AtomicBool, Ordering};
    let returned = Arc::new(AtomicBool::new(false));
    let releaser = {
        let (disk, returned) = (disk.clone(), returned.clone());
        std::thread::spawn(move || -> Outcome {
            let waited = until(|| disk.held_waiters() > 0, "wait for the report");
            let early = returned.load(Ordering::SeqCst);
            disk.release_syncs();
            waited?;
            if early {
                return Err("it returned before anything waited for the report".into());
            }
            Ok(())
        })
    };
    let done = op();
    returned.store(true, Ordering::SeqCst);
    let released = releaser
        .join()
        .map_err(|_| "the releasing thread panicked".to_string())?;
    done.and(released)
}

/// Marks under which a log of 12 records of 64 KiB is above the high
/// mark, and a hold after the sixth leaves it between the marks.
const HELPED: Marks = MARKS;

/// With the helper held at the home image's sync, the worker serves: a
/// write and a flush complete, and a read of a block of the batch is
/// answered, from the log, where the batch's records stay until the
/// step is taken up. Nothing is written to the home image while its
/// sync is under way, and the log's start does not move; once the
/// sync ends, the step is taken up and the start moves. Mutant: the
/// rule takes the next step while one waits for its report.
#[test]
fn a_held_helper_leaves_the_worker_serving() {
    fn scenario(env: &Env) -> Outcome {
        let Helping {
            disk,
            mut served,
            start,
            first,
        } = helping(env, 12, 6, HELPED)?;
        let write = Op::Write {
            offset: 600 * BLOCK,
            data: Buf::filled(16, 0x77),
        };
        let read = Op::Read {
            offset: first * 16 * BLOCK,
            buf: Buf::zeroed(16),
        };
        let mut answers = Vec::new();
        for (id, op) in [(1, write), (2, Op::Flush), (3, read)] {
            answers.push(through(&mut served, id, op)?);
        }
        for answer in &answers {
            if answer.result.is_err() {
                return Err(format!("{answer:?}"));
            }
        }
        let batch = answers[2].buf.as_ref().map(|buf| buf.to_vec());
        if batch != Some(vec![first as u8 + 1; 16 * BLOCK as usize]) {
            return Err("a block of the batch read wrong while its sync was held".into());
        }
        let written = disk.writes_while_syncing(HOME_NAME);
        let state = served.log_state();
        let held = (state.start, state.stepping);
        disk.release_syncs();
        if written > 0 {
            return Err(format!(
                "{written} writes to the home image while its sync was under way"
            ));
        }
        if held != (start.0, true) {
            return Err(format!("while held: start and stepping {held:?}"));
        }
        until(|| served.log_state().start > start.0, "step taken up")?;
        let timing = served.timing();
        if timing.helper.count < 1 || timing.helper_requests < 2 {
            return Err(format!("the helper's account: {timing:?}"));
        }
        drop(served);
        let mut store = env.reopen(&disk)?;
        expect(
            &mut store,
            600,
            16,
            0x77,
            "the write made while the sync was held",
        )?;
        expect(&mut store, first * 16, 16, first as u8 + 1, "the batch")?;
        Ok(())
    }
    holds(scenario);
    caught(scenario, Mutants::HOME_WRITTEN_BEFORE_REPORT);
}

/// With the helper held, an explicit checkpoint, a change of the hold,
/// and the end of the served store each wait for the report, and then
/// do what they do. (`skyzfsd detach` reaches the store as the serving
/// daemon's end: the drop of the served store.)
#[test]
fn a_checkpoint_a_hold_and_the_end_wait_for_the_report() {
    let env = environments(0)[0];
    let h = helping(&env, 12, 6, HELPED).unwrap();
    waits_for_the_report(&h.disk, || {
        h.served.checkpoint().map(|_| ()).map_err(|e| e.to_string())
    })
    .unwrap();
    // The checkpoint took the whole durable log.
    let state = h.served.log_state();
    assert_eq!(state.start, state.durable, "{state:?}");
    let timing = h.served.timing();
    assert!(timing.step_report.count >= 1, "{timing:?}");

    let h = helping(&env, 12, 6, HELPED).unwrap();
    let at = h.start;
    waits_for_the_report(&h.disk, || {
        h.served.set_hold(Some(at)).map_err(|e| e.to_string())
    })
    .unwrap();
    // The step was taken up under the hold it was chosen under, and the
    // new hold is in place: nothing below it is given up.
    assert!(h.served.log_state().start > at.0);
    assert!(h.served.checkpoint().is_ok());

    let Helping {
        disk,
        served,
        start,
        ..
    } = helping(&env, 12, 6, HELPED).unwrap();
    waits_for_the_report(&disk, || {
        drop(served);
        Ok(())
    })
    .unwrap();
    // The end took the step up: the next open finds the log's start
    // moved.
    let store = env.reopen(&disk).unwrap();
    assert!(store.log_start() > start);
}

/// With the helper held, a request that brings the log to the high
/// mark is answered, and the worker then waits for the report before
/// the next request: the log's bound is the high mark and one request,
/// as when the worker made the sync itself.
#[test]
fn at_the_high_mark_the_worker_waits_for_the_report() {
    let env = environments(0)[0];
    let Helping {
        disk, mut served, ..
    } = helping(&env, 12, 6, HELPED).unwrap();
    let mut id = 10;
    let mut write = |served: &mut Served| {
        id += 1;
        let op = Op::Write {
            offset: (700 + id % 4 * 16) * BLOCK,
            data: Buf::filled(16, id as u8),
        };
        through(served, id, op).map(|done| done.result.is_ok())
    };
    // Below the high mark the writes are answered with the sync held.
    while served.log_state().live() < HELPED.high {
        assert!(write(&mut served).unwrap());
    }
    // At it, the next request waits for the report.
    waits_for_the_report(&disk, || {
        if write(&mut served)? {
            Ok(())
        } else {
            Err("the write failed".into())
        }
    })
    .unwrap();
    let state = served.log_state();
    assert!(state.live() < HELPED.high + ONE_REQUEST, "{state:?}");
}

/// With the helper held, a write that finds the log full makes room:
/// a sync of the log, then a checkpoint, which waits for the report
/// first. The log here has three segment files and a high mark that
/// it does not reach before it is full.
#[test]
fn room_in_a_full_log_waits_for_the_report() {
    let env = Env {
        segments: Some(3),
        ..environments(0)[0]
    };
    // How many records of 64 KiB fill the three segments, and the log's
    // size then: what the high mark is set to, so that the first steps
    // begin at once.
    let (records, live) = {
        let (_disk, mut store) = env.fresh().unwrap();
        let identity = *store.core.log.identity();
        let mut records = 0;
        loop {
            let tail = store.position().0;
            let last = tail / identity.segment_size == 2;
            if last && !identity.fits(tail % identity.segment_size, 16 * BLOCK as u32) {
                break;
            }
            write(&mut store, records * 16 % 1024, 16, 1).unwrap();
            records += 1;
        }
        (records, store.log_state().live())
    };
    let marks = Marks {
        high: live,
        low: 64 << 10,
        step: MARKS.step,
    };
    let Helping {
        disk, mut served, ..
    } = helping(&env, records, 10, marks).unwrap();
    let rooms = served.timing().syncs[SyncKind::ROOM].count;
    waits_for_the_report(&disk, || {
        let op = Op::Write {
            offset: 1000 * BLOCK,
            data: Buf::filled(16, 0x5a),
        };
        match through(&mut served, 99, op)?.result {
            Ok(()) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    })
    .unwrap();
    let timing = served.timing();
    assert_eq!(timing.syncs[SyncKind::ROOM].count, rooms + 1, "{timing:?}");
    assert_eq!(timing.gate.count, 0, "{timing:?}");
    drop(served);
    let mut store = env.reopen(&disk).unwrap();
    expect(&mut store, 1000, 16, 0x5a, "the write that made room").unwrap();
}

/// A home-image sync that the helper reports as failed fails the
/// volume, as one the worker made would: later requests are answered
/// failed and the log is marked. What was acknowledged while the sync
/// was under way is there after the failure is acknowledged, and so is
/// the batch, which the log still held.
#[test]
fn a_failed_sync_with_the_helper_fails_the_volume_and_loses_nothing_acknowledged() {
    let env = environments(0)[0];
    let Helping {
        disk,
        mut served,
        first,
        ..
    } = helping(&env, 12, 6, HELPED).unwrap();
    for (id, block, byte) in [(1, 600, 0x51u8), (2, 700, 0x52)] {
        let op = Op::Write {
            offset: block * BLOCK,
            data: Buf::filled(16, byte),
        };
        assert_eq!(through(&mut served, id, op).unwrap().result, Ok(()));
    }
    disk.fail_sync_of(HOME_NAME);
    disk.release_syncs();
    assert_eq!(
        through(&mut served, 3, Op::Flush).unwrap().result,
        Err(BlkError::Failed)
    );
    drop(served);
    disk.clear_faults();
    match env.reopen(&disk) {
        Err(why) => assert!(why.contains("failed"), "{why}"),
        Ok(_) => panic!("the log is not marked"),
    }
    let mut store = env.reopen_ack(&disk).unwrap();
    expect(&mut store, 600, 16, 0x51, "acknowledged during the sync").unwrap();
    expect(&mut store, 700, 16, 0x52, "acknowledged during the sync").unwrap();
    expect(&mut store, first * 16, 16, first as u8 + 1, "the batch").unwrap();
}

/// A step whose home-image sync ends some requests after it began can
/// be cut anywhere: between the sync's begin and its end the store
/// appends, syncs the log for flushes and rolls to new segment files,
/// and after a power cut at any of those points every block reads the
/// last value a flush covered or a later one. Nothing is written to the
/// home image while its sync is under way. Mutants: the log's start
/// moved before the report; the next step taken while one waits for
/// its report.
#[test]
fn a_step_whose_sync_ends_later_can_be_cut_anywhere() {
    fn scenario(env: &Env) -> Outcome {
        let marks = Marks {
            high: 160 << 10,
            low: 64 << 10,
            step: StepBound {
                payload: 48 << 10,
                records: 4,
            },
        };
        let mut overlapped = 0;
        for stop in (0..60).chain((70..400).step_by(23)) {
            let disk = SimDisk::new(stop);
            disk.set_write_unit(u64::from(env.unit));
            disk.set_sync_delay(5);
            let options = OpenOptions {
                hand_over_home_sync: true,
                ..env.options()
            };
            let key = Key(*b"a key for tests!");
            let mut store = Store::create_keyed(disk.clone(), &env.config(), key, options)
                .map_err(|e| e.to_string())?;
            store.set_marks(Some(marks));
            // Per group of four blocks: the bytes written, in order, and
            // how many of them a flush covered.
            let mut written: Vec<Vec<u8>> = vec![Vec::new(); 64];
            let mut durable = [0usize; 64];
            disk.halt_after(stop);
            let mut n = 0u64;
            let finished = 'load: {
                for _ in 0..40 {
                    for _ in 0..4 {
                        let group = (n * 13 % 64) as usize;
                        let byte = (n % 250) as u8 + 1;
                        n += 1;
                        if write(&mut store, group as u64 * 4, 4, byte).is_err() {
                            break 'load false;
                        }
                        written[group].push(byte);
                    }
                    if flush(&mut store).is_err() {
                        break 'load false;
                    }
                    for (group, values) in written.iter().enumerate() {
                        durable[group] = values.len();
                    }
                }
                true
            };
            let during = disk.writes_while_syncing(HOME_NAME);
            if during > 0 {
                return Err(format!(
                    "{during} writes to the home image while its sync was under way, cut at {stop}"
                ));
            }
            overlapped += store.timing().helper_requests;
            if finished && store.timing().helper.count == 0 {
                return Err("no step handed its sync over".into());
            }
            drop(store);
            let lost = env.lost();
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
            disk.set_write_unit(u64::from(env.unit));
            let mut store = env.reopen_ack(&disk)?;
            for (group, values) in written.iter().enumerate() {
                let got = read(&mut store, group as u64 * 4, 4)?;
                let allowed = match durable[group] {
                    0 => std::iter::once(0)
                        .chain(values.iter().copied())
                        .collect::<Vec<_>>(),
                    covered => values[covered - 1..].to_vec(),
                };
                let first = got[0];
                if got.iter().any(|b| *b != first) || !allowed.contains(&first) {
                    return Err(format!(
                        "group {group} reads {first:#04x} after a cut at {stop}; \
                         written {values:?}, a flush covered {}",
                        durable[group]
                    ));
                }
            }
        }
        if overlapped == 0 {
            return Err("no request fell between a sync's begin and its end".into());
        }
        Ok(())
    }
    holds(scenario);
    caught(scenario, Mutants::START_BEFORE_REPORT);
    caught(scenario, Mutants::HOME_WRITTEN_BEFORE_REPORT);
}

/// A sync that cannot be begun (the helper cannot be started: no thread,
/// no descriptor) is no fault of the volume's: the worker makes it
/// itself and the step completes, nothing is marked, and what was
/// flushed survives a power cut. The next step tries the helper again.
/// The status shows it in its existing numbers: steps without a sync of
/// the helper's, whose `step_home_sync` holds a whole sync.
#[test]
fn a_sync_that_cannot_be_begun_is_made_by_the_worker() {
    fn scenario(env: &Env) -> Outcome {
        for refused in [u64::MAX, 1] {
            let disk = SimDisk::new(7);
            disk.set_write_unit(u64::from(env.unit));
            disk.refuse_sync_begins(refused);
            let options = OpenOptions {
                hand_over_home_sync: true,
                ..env.options()
            };
            let key = Key(*b"a key for tests!");
            let mut store = Store::create_keyed(disk.clone(), &env.config(), key, options)
                .map_err(|e| e.to_string())?;
            store.set_marks(Some(MARKS));
            for n in 0..48u64 {
                write(&mut store, n % 16 * 16, 16, n as u8 + 1)?;
                if n % 4 == 3 {
                    flush(&mut store)?;
                }
            }
            let (timing, steps) = (store.timing(), store.log_state().steps);
            if store.is_failed() || steps == 0 {
                return Err(format!("failed {} after {steps} steps", store.is_failed()));
            }
            let helped = timing.helper.count;
            let expected = if refused == 1 {
                helped >= 1
            } else {
                helped == 0
            };
            if !expected || timing.step_home_sync.count < steps - helped {
                return Err(format!("{refused} refused: {steps} steps, {timing:?}"));
            }
            drop(store);
            disk.power_cut(LoseAll);
            disk.set_write_unit(u64::from(env.unit));
            let mut store = env.reopen(&disk)?;
            for n in 32..48u64 {
                expect(&mut store, n % 16 * 16, 16, n as u8 + 1, "after the cut")?;
            }
        }
        Ok(())
    }
    holds(scenario);
}

/// A volume that fails while a step's sync is with the helper does not
/// take the step up when the helper reports: the step goes no further
/// than its sync (it never reaches its point `Synced`), the log's start
/// stays, and the next open applies the batch again.
#[test]
fn a_volume_that_failed_meanwhile_does_not_take_the_step_up() {
    let env = environments(0)[0];
    let gate = std::env::temp_dir().join(format!("skyzfs-meanwhile-{}", std::process::id()));
    let reached = StepPause::reached(&gate);
    std::fs::write(&gate, b"").unwrap();
    let pause = StepPause::Until(gate.clone());
    let Helping {
        disk,
        served,
        start,
        first,
    } = helping_with(&env, 12, 6, HELPED, |store| {
        store.set_step_pause_at(pause, StepPoint::Synced)
    })
    .unwrap();
    // The steps to the hold went through the point; this one has not.
    let _ = std::fs::remove_file(&reached);
    served.fail_now();
    disk.release_syncs();
    until(|| served.timing().helper.count >= 1, "report taken").unwrap();
    let state = served.log_state();
    drop(served);
    let taken_up = reached.exists();
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(&reached);
    assert!(!taken_up, "the step was taken up on a failed volume");
    assert_eq!(state.start, start.0, "{state:?}");
    let mut store = env.reopen_ack(&disk).unwrap();
    assert_eq!(store.log_start(), start);
    expect(&mut store, first * 16, 16, first as u8 + 1, "the batch").unwrap();
}

// ---------------------------------------------------------------------
// Several readers (M1.3n).

/// A read held in one reader leaves the others reading: with two
/// readers, a read is held after its lookup, with its pin, and reads of
/// other blocks submitted after it complete meanwhile, one from a
/// record of the log and one from the home image. Released, the held
/// read returns its block.
#[test]
fn a_read_held_in_one_reader_leaves_the_others_reading() {
    fn scenario(env: &Env) -> Outcome {
        let (_disk, mut store) = env.fresh()?;
        write(&mut store, 0, 1, 0x11)?;
        write(&mut store, 5, 1, 0x55)?;
        flush(&mut store)?;
        store.core.readers = 2;
        let held = Gate::new("held-read");
        store.set_read_pause_at(0, held.pause());
        let mut served = store.serve().map_err(|e| e.to_string())?;
        let _opened = Opened(&[&held]);
        submit(&mut served, read_request(100, 0, 1))?;
        held.wait_reached("the held read")?;
        submit(&mut served, read_request(101, 5, 1))?;
        submit(&mut served, read_request(102, 9, 1))?;
        let mut done = Vec::new();
        take(&mut served, &mut done, 2, "the reads beside the held one")?;
        if done.iter().any(|c| c.id == RequestId(100)) {
            return Err("the held read completed while it was held".into());
        }
        for (id, byte) in [(101, 0x55), (102, 0)] {
            let read = completion(&done, id)?;
            if read.result.is_err()
                || read
                    .buf
                    .as_ref()
                    .is_none_or(|b| b.iter().any(|b| *b != byte))
            {
                return Err(format!("read {id} is not {byte:#x}"));
            }
        }
        held.open();
        take(&mut served, &mut done, 3, "the held read")?;
        let read = completion(&done, 100)?;
        if read.result.is_err()
            || read
                .buf
                .as_ref()
                .is_none_or(|b| b.iter().any(|b| *b != 0x11))
        {
            return Err("the held read does not return its block".into());
        }
        Ok(())
    }
    holds(scenario);
}

/// Release the two held readers, `first` first: after the first has
/// completed and the barrier has woken and looked again, the barrier
/// still waits (none has passed since `passed`) and nothing the worker
/// does has completed. Then the second.
fn release_one_after_the_other(
    served: &mut Served,
    shared: &Shared,
    gates: [&Gate; 2],
    done: &mut Vec<Completion>,
    alone: impl Fn() -> Outcome,
) -> Outcome {
    let (woke, passed) = shared.pin.woke_and_passed();
    let before = done.len();
    gates[0].open();
    take(served, done, before + 1, "the read released first")?;
    let began = std::time::Instant::now();
    while shared.pin.woke_and_passed().0 <= woke {
        if began.elapsed() > Duration::from_secs(20) {
            return Err("the barrier did not look again when a reader left".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if shared.pin.woke_and_passed().1 != passed {
        return Err("the barrier passed with a reader still pinned".into());
    }
    alone()?;
    gates[1].open();
    Ok(())
}

/// The two orders of release, as gates of blocks `blocks`.
const ORDERS: [[usize; 2]; 2] = [[0, 1], [1, 0]];

/// The first barrier waits for every reader that is pinned, with two
/// readers: two reads of blocks that only the home image holds are held
/// after their lookups, writes to both blocks are made durable, and a
/// checkpoint is asked for. The step waits at the barrier; when one
/// reader is released and has completed, it still waits for the other
/// and has not written; released too, both return the blocks as they
/// were. In both orders. Mutants: the barrier waits for one reader
/// only; there is no barrier; the readers pin after their lookups.
fn two_held_readers_at_the_first_barrier(env: &Env) -> Outcome {
    let unbarred = env.mutants & (Mutants::HOME_WRITTEN_UNBARRED | Mutants::PIN_AFTER_LOOKUP) != 0;
    for order in ORDERS {
        let blocks = [0u64, 5];
        let (_disk, mut store) = env.fresh()?;
        store.core.readers = 2;
        let step = Gate::new("two-home-step");
        let gates = [Gate::new("two-home-a"), Gate::new("two-home-b")];
        store.set_step_pause_at(step.pause(), StepPoint::Written);
        for (block, gate) in blocks.iter().zip(&gates) {
            store.set_read_pause_at(*block, gate.pause());
        }
        let shared = store.core.shared.clone();
        let mut served = store.serve().map_err(|e| e.to_string())?;
        let _opened = Opened(&[&step, &gates[0], &gates[1]]);
        for (n, (block, gate)) in blocks.iter().zip(&gates).enumerate() {
            submit(&mut served, read_request(100 + n as u64, *block, 1))?;
            gate.wait_reached("a read")?;
        }
        let mut done = Vec::new();
        for (n, block) in blocks.iter().enumerate() {
            submit(&mut served, write_request(1 + n as u64, *block, 1, 0x22))?;
        }
        submit(
            &mut served,
            Request {
                id: RequestId(3),
                op: Op::Flush,
            },
        )?;
        take(&mut served, &mut done, 3, "the writes and the flush")?;
        let (reply, checkpointed) = mpsc::channel();
        if !served.send(Command::Checkpoint(reply)) {
            return Err("the worker is gone".into());
        }
        let first = [&gates[order[0]], &gates[order[1]]];
        if unbarred {
            step.wait_reached("the step, with no barrier")?;
            first[0].open();
            first[1].open();
        } else {
            at_a_barrier(&shared, 0, "the first barrier")?;
            release_one_after_the_other(&mut served, &shared, first, &mut done, || {
                if step.reached() {
                    return Err("the step wrote the home image with a reader held".into());
                }
                Ok(())
            })?;
        }
        take(&mut served, &mut done, 5, "both reads")?;
        for id in [100, 101] {
            let read = completion(&done, id)?;
            if read.result.is_err() || read.buf.as_ref().is_none_or(|b| b.iter().any(|b| *b != 0)) {
                return Err(format!(
                    "held read {id} is not its block as it was, order {order:?}"
                ));
            }
        }
        step.wait_reached("the step, after the readers")?;
        step.open();
        let checkpointed = checkpointed.recv_timeout(Duration::from_secs(20));
        if !matches!(checkpointed, Ok(Ok(_))) {
            return Err(format!("the checkpoint: {checkpointed:?}"));
        }
    }
    Ok(())
}

/// The second barrier, the same way: two blocks are in records of the
/// log's first segment file, reads of both are held after their lookups
/// while a checkpoint stands where it is to free that file, and a
/// megabyte that needs the freed file is written behind it. When one
/// reader is released and has completed, the step still waits and the
/// write has not completed; released too, both return their blocks. In
/// both orders. Mutants: as for the first barrier.
fn two_held_readers_at_the_second_barrier(env: &Env) -> Outcome {
    let unbarred = env.mutants & (Mutants::SLOT_REUSED_UNBARRED | Mutants::PIN_AFTER_LOOKUP) != 0;
    let env = Env {
        segments: Some(2),
        ..*env
    };
    for order in ORDERS {
        let (_disk, mut store) = env.fresh()?;
        write(&mut store, 0, 1, 0x11)?;
        write(&mut store, 1, 1, 0x12)?;
        megabyte(&mut store, 1, 2)?;
        megabyte(&mut store, 2, 3)?;
        flush(&mut store)?;
        if store.core.log.segment_files() != 2 {
            return Err("the log is not in two files".into());
        }
        store.core.readers = 2;
        let step = Gate::new("two-reuse-step");
        let gates = [Gate::new("two-reuse-a"), Gate::new("two-reuse-b")];
        store.set_step_pause_at(step.pause(), StepPoint::Synced);
        for (block, gate) in [0u64, 1].iter().zip(&gates) {
            store.set_read_pause_at(*block, gate.pause());
        }
        let shared = store.core.shared.clone();
        let mut served = store.serve().map_err(|e| e.to_string())?;
        let _opened = Opened(&[&step, &gates[0], &gates[1]]);
        let (reply, checkpointed) = mpsc::channel();
        if !served.send(Command::Checkpoint(reply)) {
            return Err("the worker is gone".into());
        }
        step.wait_reached("the checkpoint")?;
        let waited = shared.pin.waited();
        for (n, gate) in gates.iter().enumerate() {
            submit(&mut served, read_request(100 + n as u64, n as u64, 1))?;
            gate.wait_reached("a read")?;
        }
        submit(&mut served, write_request(1, 256, 256, 4))?;
        step.open();
        let mut done = Vec::new();
        let first = [&gates[order[0]], &gates[order[1]]];
        if unbarred {
            take(&mut served, &mut done, 1, "the write, with no barrier")?;
            first[0].open();
            first[1].open();
        } else {
            at_a_barrier(&shared, waited, "the second barrier")?;
            let mut more = Vec::new();
            release_one_after_the_other(&mut served, &shared, first, &mut done, || Ok(()))?;
            // The worker waits at the barrier, and nothing behind it
            // completed while the second reader was held.
            if done.len() != 1 || served.poll(&mut more) > 1 {
                return Err("the write completed with a reader held".into());
            }
            done.append(&mut more);
        }
        take(&mut served, &mut done, 3, "both reads and the write")?;
        let checkpointed = checkpointed.recv_timeout(Duration::from_secs(20));
        if !matches!(checkpointed, Ok(Ok(_))) {
            return Err(format!("the checkpoint: {checkpointed:?}"));
        }
        for (id, byte) in [(100, 0x11), (101, 0x12)] {
            let read = completion(&done, id)?;
            if read.result.is_err()
                || read
                    .buf
                    .as_ref()
                    .is_none_or(|b| b.iter().any(|b| *b != byte))
            {
                return Err(format!(
                    "held read {id} does not return its block, order {order:?}"
                ));
            }
        }
        if done.iter().any(|c| c.result.is_err()) || served.gone() {
            return Err("a request failed".into());
        }
    }
    Ok(())
}

/// Acceptance (b): each barrier waits for a pin held by any of the
/// readers, shown with two readers pinned and released one after the
/// other.
#[test]
fn each_barrier_waits_for_every_reader_that_is_pinned() {
    caught(
        two_held_readers_at_the_first_barrier,
        Mutants::BARRIER_WAITS_FOR_ONE,
    );
    caught(
        two_held_readers_at_the_second_barrier,
        Mutants::BARRIER_WAITS_FOR_ONE,
    );
    for env in environments(Mutants::HOME_WRITTEN_UNBARRED) {
        assert!(
            two_held_readers_at_the_first_barrier(&env).is_err(),
            "{env:?}"
        );
    }
    for env in environments(Mutants::SLOT_REUSED_UNBARRED) {
        assert!(
            two_held_readers_at_the_second_barrier(&env).is_err(),
            "{env:?}"
        );
    }
}

/// Acceptance (c): the pin is taken before the lookup with several
/// readers too. Mutant: the pin after the lookup, caught at both
/// barriers with two readers.
#[test]
fn several_readers_hold_their_pins_from_before_their_lookups() {
    for env in environments(Mutants::PIN_AFTER_LOOKUP) {
        assert!(
            two_held_readers_at_the_first_barrier(&env).is_err(),
            "{env:?}"
        );
        assert!(
            two_held_readers_at_the_second_barrier(&env).is_err(),
            "{env:?}"
        );
    }
}
