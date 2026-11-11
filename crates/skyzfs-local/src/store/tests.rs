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
                    threaded: false,
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
        4,
        "and one when the worker ended"
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
    let defects: [(&str, u32); 13] = [
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

/// What one request of 16 blocks adds to the log at most: its data, its
/// header, and the padding to the next write unit and segment.
const ONE_REQUEST: u64 = (64 << 10) + (32 << 10);

/// Under a write load that lasts and never flushes, with the rule
/// asked once after every request, the log's live size stays below
/// the high mark and one request: the log stops growing, whatever
/// the load. No step is taken while the log is below the low mark,
/// steps are taken, the store syncs the log itself to have something
/// durable to take, and every block reads as written at the end and
/// after a restart.
#[test]
fn under_a_lasting_write_load_the_log_stays_at_the_high_mark() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut store) = env.fresh()?;
        store.set_marks(Some(MARKS));
        let mut newest = [0u8; 64];
        let (mut largest, mut below_low, mut was_high) = (0, 0u64, false);
        for n in 0..300u64 {
            let slot = (n * 7) % 64;
            let byte = (n % 250) as u8 + 1;
            write(&mut store, slot * 16, 16, byte)?;
            newest[slot as usize] = byte;
            let before = store.log_state();
            let stepped = store.tend().map_err(|e| format!("tend: {e}"))?;
            let after = store.log_state();
            largest = largest.max(before.live());
            was_high |= before.live() >= MARKS.high;
            if before.live() > MARKS.high + ONE_REQUEST {
                return Err(format!(
                    "after write {n} the log holds {} bytes",
                    before.live()
                ));
            }
            if stepped && !was_high {
                return Err(format!(
                    "a step at write {n}, before the high mark was reached"
                ));
            }
            if stepped != (after.steps > before.steps) || (stepped && after.start <= before.start) {
                return Err(format!(
                    "write {n}: stepped {stepped}, {before:?} to {after:?}"
                ));
            }
            if after.live() < MARKS.low {
                below_low += 1;
                // Below the low mark the rule rests until the high one.
                was_high = false;
            }
        }
        let state = store.log_state();
        if state.steps < 20 || largest < MARKS.high || below_low == 0 {
            return Err(format!(
                "the load did not exercise the rule: {state:?}, largest {largest}, below the low mark {below_low} times"
            ));
        }
        let check = |store: &mut SimStore, when: &str| -> Outcome {
            for (slot, byte) in newest.iter().enumerate() {
                expect(store, slot as u64 * 16, 16, *byte, when)?;
            }
            Ok(())
        };
        check(&mut store, "under the load")?;
        // Nothing the client was never promised durable is needed
        // after a kill, and what the steps took is in the home image.
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
    store.set_marks(Some(MARKS));
    for n in 0..12u64 {
        write(&mut store, n * 16, 16, 1).expect("write");
    }
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

/// The served store applies the rule on its worker: after every
/// request, and with no request at all until the log is below the low
/// mark. Requests are answered between the steps, the owner is woken
/// when a step begins and ends, and the log's state can be read while
/// the worker works.
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
    // The state is published a moment after the completion, so one
    // request more than the bound may be seen.
    assert!(
        largest <= MARKS.high + 2 * ONE_REQUEST,
        "the log reached {largest} bytes"
    );
    // No request any more: the worker goes on until the low mark.
    let began = std::time::Instant::now();
    loop {
        let state = served.log_state();
        if state.live() < MARKS.low && !state.stepping {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "idle, the log stayed at {state:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let state = served.log_state();
    assert!(state.steps >= 10, "{state:?}");
    // 200 completions, and two wake-ups a step.
    assert!(
        u64::from(calls.load(Ordering::SeqCst)) >= 200 + 2 * state.steps,
        "{} wake-ups for 200 requests and {} steps",
        calls.load(Ordering::SeqCst),
        state.steps
    );
    // It rests there: no step without a reason.
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
        let newest = (slot + if slot < 8 { 192 } else { 128 }) % 250 + 1;
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

/// A kill inside a step of the served store, through the worker's own
/// rule: the step waits in its middle (the test hook), the store shows
/// that a step is under way, and the process is killed there. The next
/// open finds every acknowledged write, the rule goes on where the
/// log was left, and the content stays right.
#[test]
fn a_kill_inside_a_step_of_the_worker_loses_nothing() {
    static NUMBER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    for env in environments(0) {
        let gate = std::env::temp_dir().join(format!(
            "skyzfs-step-gate-{}-{}",
            std::process::id(),
            NUMBER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&gate);
        let (disk, mut store) = env.fresh().unwrap();
        store.set_marks(Some(MARKS));
        store.set_step_pause(Some(StepPause::Until(gate.clone())));
        let mut served = store.serve().unwrap();
        let mut done = Vec::new();
        let mut newest = [0u8; 64];
        let mut n = 0u64;
        // Writes until the worker is inside its first step. It answers
        // the write that reached the mark and then waits in the step.
        while !served.log_state().stepping {
            assert!(n < 64, "no step after {n} writes: {:?}", served.log_state());
            let (slot, byte) = (n % 64, n as u8 + 1);
            done.clear();
            served
                .submit(Request {
                    id: RequestId(n),
                    op: Op::Write {
                        offset: slot * 16 * BLOCK,
                        data: Buf::filled(16, byte),
                    },
                })
                .unwrap();
            assert_eq!(served.wait(&mut done, Duration::from_secs(10)), 1);
            assert_eq!(done[0].result, Ok(()));
            newest[slot as usize] = byte;
            n += 1;
            // The worker decides about a step right after the answer.
            std::thread::sleep(Duration::from_millis(3));
        }
        assert_eq!(served.log_state().steps, 0, "the step has not ended");
        disk.kill();
        std::fs::write(&gate, b"").unwrap();
        drop(served);
        let _ = std::fs::remove_file(&gate);
        let mut store = env
            .reopen_ack(&disk)
            .unwrap_or_else(|why| panic!("after a kill inside a step in {env:?}: {why}"));
        let check = |store: &mut SimStore, when: &str| {
            for (slot, byte) in newest.iter().enumerate() {
                expect(store, slot as u64 * 16, 16, *byte, when)
                    .unwrap_or_else(|why| panic!("{env:?}: {why}"));
            }
        };
        check(&mut store, "after a kill inside a step");
        store.set_marks(Some(MARKS));
        let mut steps = 0;
        while store.tend().unwrap() {
            steps += 1;
        }
        assert!(steps > 0, "the rule did not go on after the restart");
        check(&mut store, "after the steps that followed");
    }
}
