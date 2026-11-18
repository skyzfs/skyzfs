//! The conformance suite's harness for the in-place store on the
//! simulated disk.
//!
//! It has every capability. A power cut is the simulated disk's own
//! ([`SimDisk::power_cut`] with a [`SeededCut`] drawn from the suite's
//! `keep`, without garbage): each sector keeps its durable content or
//! any version written since, which is rule R10, the volatile-cache
//! disk. The disk's garbage, which the rule does not allow and a real
//! disk may still write, is tested apart (`tests.rs`).
//!
//! - `create` makes a volume ([`super::create`]), serves it once with
//!   writes that no flush covers all of, and loses that process: the
//!   suite's first start is a start after a lost process.
//! - `kill` abandons the store as a kill of its process does
//!   ([`InPlace::abandon`]): the requests no I/O thread has begun, and
//!   those waiting in the store, are dropped undone, and the jobs under
//!   way end. Then it kills the simulated process and starts again: the
//!   open under the lock, then the start. What the threads had begun
//!   at the kill depends on their timing, which no seed decides; the
//!   suite submits every request in flight again
//!   ([`InFlight::Reissued`](skyzfs_blk::model::InFlight)), so each
//!   outcome is one the model allows. A kill finds the syncs of the
//!   flushes in flight where the seed of the disk's delays has them.
//! - `fail_log` makes every write and every sync fail from then on, as
//!   the simulated disk fails them: a failed write leaves any subset of
//!   its sectors, a failed sync may drop what it did not make durable.
//!   For a third of the seeds ([`settles_at_failure`]) the faults come
//!   once every job on the I/O threads has ended and what waited for
//!   them has begun, with the image's syncs held so that a flush's sync
//!   begun then is under way when the faults come and fails when it
//!   ends: a store that answers a failed sync otherwise than by failing
//!   the volume meets that every time (M1.3ad). For the other seeds the
//!   faults come wherever the threads are, which is where a real
//!   failure comes.
//! - `reopen_failed` takes the fault away, loses the process and
//!   starts with the failure acknowledged, with the kernel's cache kept:
//!   what a failed sync left marked clean and never written is still in
//!   the cache. The start drops the
//!   cache and serves only what the disk holds, which is what makes the
//!   volume as after a power cut (rule R11); the case
//!   `reopened_failed_volume_holds_what_was_flushed` fails on a store
//!   that does not.
//!
//! With [`InPlaceHarness::held_cuts`] the store holds cuts beneath the
//! suite's cases. A cut needs a mark and a flush after it, and the
//! suite writes no uberblock; an uberblock written by the harness would
//! change blocks the suite reads. So before each flush the suite sends,
//! the harness marks a new state as an applied uberblock write marks it
//! ([`InPlace::mark_as_applied`]), and every flush makes a cut. What a
//! held cut reads is judged by the cut state's own tests; here, that
//! the cuts change nothing a client sees.

use std::cell::RefCell;
use std::time::Duration;

use skyzfs_blk::conformance::{Capabilities, Harness, HarnessError, Spec};
use skyzfs_blk::{BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, RequestId};

use skyzfs_uberblock::{State, ROOT_WORDS};

use super::state::{self, MemRegion};
use super::{create, lock, InPlace, Mutants, OpenOptions, Opened, VolumeConfig};
use crate::sim::{SeededCut, SimDisk};
use crate::store::HOME_NAME;

/// The volume's size.
const VOLUME_MIB: u64 = 4;
/// The bound on the copies when the store holds cuts: sixteen blocks.
const HELD_BOUND: u64 = 16 * skyzfs_blk::BLOCK_SIZE;

/// What the cuts did beneath the suite's cases, on this thread (the
/// suite runs its cases on the caller's thread): the test of the suite
/// with held cuts checks that each happened.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Seen {
    /// The most cuts one volume counted.
    pub(crate) cuts: u64,
    /// Flushes received while copies were held.
    pub(crate) copied: u64,
    /// The most cuts one volume abandoned at the bound.
    pub(crate) abandoned: u64,
    /// Starts after a kill that took up held cuts.
    pub(crate) taken_up: u64,
}

thread_local! {
    static SEEN: RefCell<Seen> = RefCell::new(Seen::default());
}

/// What the cuts did on this thread since the last call.
pub(crate) fn seen() -> Seen {
    SEEN.with(|seen| seen.replace(Seen::default()))
}

/// Whether a target created with `seed` has its failure come when the
/// I/O threads have ended their jobs and a flush's sync is under way
/// (see the module documentation): a third of the seeds, which in the
/// suite's cases cross their other choices (the base or none by the
/// seed's parity; a kill or none by the seed modulo 4).
pub(crate) fn settles_at_failure(seed: u64) -> bool {
    seed % 3 == 1
}

/// One environment the suite runs the in-place store in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InPlaceHarness {
    pub(crate) queue_depth: u32,
    /// The simulated disk ends a sync begun on the helper after up to
    /// this many looks ([`SimDisk::set_sync_delay`]).
    pub(crate) sync_delay: u64,
    pub(crate) mutants: u128,
    /// How the store reads and writes ([`super::IoMode`]).
    pub(crate) io: super::IoMode,
    /// The system's page, which is the simulated disk's write unit, its
    /// file system's block, and the volume's write unit: 4096 on amd64,
    /// 16 KiB or 64 KiB on some arm64 kernels. Passed in, not read
    /// from the host, so that every size runs everywhere.
    pub(crate) page: u64,
    /// Whether the store holds cuts (see the module documentation).
    /// Each volume then has one cut state, with a bound of sixteen
    /// blocks, kept across kills; a power cut and a reopen after a
    /// failure begin a new one, as the disk lost there what was not
    /// synced. Every flush makes a cut, so writes are copied for held
    /// cuts, cuts are abandoned at the bound, and a start after a kill
    /// takes up the cuts held before it.
    pub(crate) held_cuts: bool,
}

impl InPlaceHarness {
    fn options(
        &self,
        read_only: bool,
        acknowledge_failure: bool,
        region: Option<MemRegion>,
    ) -> OpenOptions {
        OpenOptions {
            queue_depth: self.queue_depth,
            read_only,
            acknowledge_failure,
            io: self.io,
            hold_cuts: region.map(|region| super::HoldCuts {
                region: super::CutRegion::Memory(region),
                boot: [0; 16],
                bound: Some(HELD_BOUND),
                intent_region: None,
                intent_header_sync: false,
            }),
            ..OpenOptions::default()
        }
    }

    /// A new cut state for the volume, if the store holds cuts: sized
    /// for it, so that the store takes it up and makes none of its own.
    fn new_region(&self) -> Option<MemRegion> {
        self.held_cuts.then(|| {
            MemRegion::new(state::words_for(
                (VOLUME_MIB << 20) / skyzfs_blk::BLOCK_SIZE,
                state::records_for(HELD_BOUND),
            ))
        })
    }

    /// Start on `disk` with the cut state in `region`: the open under
    /// the lock, then the start. A volume marked as failed is not served
    /// again: everything fails.
    fn start(
        &self,
        disk: SimDisk,
        read_only: bool,
        acknowledge_failure: bool,
        before: Option<Geometry>,
        settle_at_failure: bool,
        region: Option<MemRegion>,
    ) -> Result<OnSim, HarnessError> {
        disk.set_sync_delay(self.sync_delay);
        let options = self.options(read_only, acknowledge_failure, region.clone());
        let opened = lock(&disk)
            .and_then(|lock| Opened::open(disk.clone(), lock, options).map_err(|(e, _)| e));
        let store = match opened {
            Ok(opened) => opened,
            // A volume marked as failed, or one whose start meets the
            // failed disk, is not served.
            Err(error) => {
                let geometry =
                    before.ok_or_else(|| HarnessError(format!("the first start: {error}")))?;
                return Ok(OnSim {
                    inner: Inner::Gone(Vec::new()),
                    disk,
                    geometry,
                    settle_at_failure,
                    region,
                    held_cuts: self.held_cuts,
                });
            }
        };
        let geometry = store.geometry();
        let inner = match store.start_with(None, Mutants(self.mutants)) {
            Ok(store) => {
                if before.is_some() && !store.held_cuts().is_empty() {
                    SEEN.with(|seen| seen.borrow_mut().taken_up += 1);
                }
                Inner::Live(Box::new(store))
            }
            // The start's sync met the failed disk.
            Err(_) if before.is_some() => Inner::Gone(Vec::new()),
            Err(error) => return Err(HarnessError(format!("the in-place store: {error}"))),
        };
        Ok(OnSim {
            inner,
            disk,
            geometry,
            settle_at_failure,
            region,
            held_cuts: self.held_cuts,
        })
    }
}

/// An in-place store on a simulated disk, or what is left of one that
/// failed.
pub(crate) struct OnSim {
    inner: Inner,
    pub(crate) disk: SimDisk,
    geometry: Geometry,
    /// [`settles_at_failure`] of the seed it was created with.
    settle_at_failure: bool,
    /// The volume's cut state, if the store holds cuts.
    region: Option<MemRegion>,
    /// Whether each flush makes a cut ([`InPlaceHarness::held_cuts`]).
    held_cuts: bool,
}

enum Inner {
    Live(Box<InPlace<SimDisk>>),
    /// The volume failed and was not served again: everything fails.
    Gone(Vec<Request>),
}

impl OnSim {
    pub(crate) fn store(&mut self) -> Option<&mut InPlace<SimDisk>> {
        match &mut self.inner {
            Inner::Live(store) => Some(store),
            Inner::Gone(_) => None,
        }
    }
}

fn buffer_of(op: Op) -> Option<Buf> {
    match op {
        Op::Read { buf, .. } => Some(buf),
        Op::Write { data, .. } => Some(data),
        _ => None,
    }
}

impl BlockTarget for OnSim {
    fn geometry(&self) -> Geometry {
        self.geometry
    }

    fn submit(&mut self, request: Request) -> Result<(), skyzfs_blk::SubmitError> {
        match &mut self.inner {
            Inner::Live(store) => {
                if self.held_cuts && matches!(request.op, Op::Flush) {
                    let figures = store.cut_figures();
                    let count = store.cuts().count;
                    SEEN.with(|seen| {
                        let mut seen = seen.borrow_mut();
                        seen.cuts = seen.cuts.max(count);
                        seen.copied += u64::from(figures.copied > 0);
                        seen.abandoned = seen.abandoned.max(figures.abandoned);
                    });
                    // A new state, which the flush cuts at its barrier;
                    // the same again if the last was not cut, which is
                    // then a repeat of the mark.
                    let group = count + 1;
                    store.mark_as_applied(State {
                        txg: group,
                        root: [group; ROOT_WORDS],
                    });
                }
                store.submit(request)
            }
            Inner::Gone(queue) => {
                queue.push(request);
                Ok(())
            }
        }
    }

    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        match &mut self.inner {
            Inner::Live(store) => store.poll(out),
            Inner::Gone(queue) => {
                let n = queue.len();
                out.extend(queue.drain(..).map(|Request { id, op }| Completion {
                    id,
                    result: Err(BlkError::Failed),
                    buf: buffer_of(op),
                }));
                n
            }
        }
    }

    fn wait(&mut self, out: &mut Vec<Completion>, timeout: Duration) -> usize {
        match &mut self.inner {
            Inner::Live(store) => store.wait(out, timeout),
            Inner::Gone(_) => self.poll(out),
        }
    }
}

impl Harness for InPlaceHarness {
    type Target = OnSim;

    fn capabilities(&self) -> Capabilities {
        Capabilities::ALL
    }

    fn create(&mut self, spec: &Spec) -> Result<OnSim, HarnessError> {
        let disk = SimDisk::new(spec.seed);
        disk.set_write_unit(self.page);
        disk.set_page_size(self.page);
        disk.set_block_size(Some(self.page));
        let write_unit =
            u32::try_from(self.page).map_err(|_| HarnessError("a page above 4 GiB".into()))?;
        let config = VolumeConfig {
            volume: *b"in-place volume.",
            size_mib: 4,
            base: spec.base,
            write_unit,
        };
        create(&disk, &config).map_err(|e| HarnessError(format!("create: {e}")))?;
        // A first instance that wrote and was lost: a megabyte of what
        // the volume holds anyway, and as many single blocks as the
        // seed decides, a flush among them, and the rest not flushed.
        // Without the defect under test: the ageing is not what is judged.
        let plain = InPlaceHarness {
            mutants: 0,
            ..*self
        };
        let mut first = plain.start(disk, false, false, None, false, self.new_region())?;
        let store = first
            .store()
            .ok_or_else(|| HarnessError("the first start did not serve".into()))?;
        let blocks = (1 << 20) / skyzfs_blk::BLOCK_USIZE;
        let mut data = Buf::zeroed(blocks);
        for (i, block) in data.chunks_mut(skyzfs_blk::BLOCK_USIZE).enumerate() {
            spec.base.fill_block(i as u64, block);
        }
        let one = Buf::from_blocks(&data[..skyzfs_blk::BLOCK_USIZE])
            .ok_or_else(|| HarnessError("no buffer".into()))?;
        let mut ops = vec![Op::Write { offset: 0, data }];
        for i in 0..spec.seed % 24 {
            if i == spec.seed % 12 {
                ops.push(Op::Flush);
            }
            ops.push(Op::Write {
                offset: 0,
                data: one.clone(),
            });
        }
        // One at a time, whatever the queue's depth.
        let mut done = Vec::new();
        for (i, op) in ops.into_iter().enumerate() {
            store
                .submit(Request {
                    id: RequestId(i as u64),
                    op,
                })
                .map_err(|e| HarnessError(e.to_string()))?;
            while done.len() <= i {
                store.wait(&mut done, Duration::from_secs(60));
            }
        }
        if done.iter().any(|c| c.result.is_err()) {
            return Err(HarnessError("the first instance's writes failed".into()));
        }
        let OnSim {
            inner,
            disk,
            region,
            ..
        } = first;
        drop(inner);
        disk.kill();
        self.start(
            disk,
            spec.read_only,
            false,
            None,
            settles_at_failure(spec.seed),
            region,
        )
    }

    /// The cut state is kept: it outlives the process.
    fn kill(&mut self, target: OnSim, _seed: u64) -> Result<OnSim, HarnessError> {
        let OnSim {
            inner,
            disk,
            geometry,
            settle_at_failure,
            region,
            ..
        } = target;
        if let Inner::Live(store) = inner {
            store.abandon();
        }
        disk.kill();
        self.start(
            disk,
            geometry.read_only(),
            false,
            Some(geometry),
            settle_at_failure,
            region,
        )
    }

    /// The seeds of the simulated disk's cut worth trying: eight.
    fn cut_points(&mut self, _target: &OnSim) -> u64 {
        7
    }

    /// The simulated disk's own cut, drawn from `keep`. The cut state,
    /// in memory, is lost with it.
    fn power_cut(&mut self, target: OnSim, keep: u64) -> Result<OnSim, HarnessError> {
        let OnSim {
            inner,
            disk,
            geometry,
            settle_at_failure,
            ..
        } = target;
        drop(inner);
        disk.power_cut(SeededCut::new(keep ^ 0x5EED_C0DE));
        self.start(
            disk,
            geometry.read_only(),
            false,
            Some(geometry),
            settle_at_failure,
            self.new_region(),
        )
    }

    fn fail_log(&mut self, target: &mut OnSim) -> Result<(), HarnessError> {
        let settle = target.settle_at_failure;
        if settle {
            // A guard: nothing on `finish_threads`'s path looks at a sync
            // today (only `settle`, from a poll or a wait, ends one). If
            // something ever did, the hold has it find the sync under
            // way rather than end it before the faults. The release
            // below also clears a hold of whole syncs, of which this
            // harness sets none.
            target.disk.hold_syncs_of(HOME_NAME);
            if let Some(store) = target.store() {
                store.finish_threads();
            }
        }
        target.disk.fail_write(0, true);
        target.disk.fail_sync(0, true);
        if settle {
            target.disk.release_syncs();
        }
        Ok(())
    }

    /// A new cut state: the volume is served as after a power cut.
    fn reopen_failed(&mut self, target: OnSim) -> Result<OnSim, HarnessError> {
        let OnSim {
            inner,
            disk,
            geometry,
            settle_at_failure,
            ..
        } = target;
        drop(inner);
        disk.clear_faults();
        disk.kill();
        self.start(
            disk,
            geometry.read_only(),
            true,
            Some(geometry),
            settle_at_failure,
            self.new_region(),
        )
    }
}
