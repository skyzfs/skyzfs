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
//! - `create` makes a log store, ages its log as the log store's
//!   harness does, and enters the in-place store from it: the
//!   checkpoint of everything is part of every start.
//! - `kill` drops the store, kills the simulated process and starts
//!   again: the log store's open, then the in-place store. A store
//!   executes every request when it is submitted, so only flushes can
//!   be in flight, and a kill finds their syncs where the seed of the
//!   disk's delays has them.
//! - `fail_log` makes every write and every sync fail from then on, as
//!   the simulated disk fails them: a failed write leaves any subset of
//!   its sectors, a failed sync may drop what it did not make durable.
//! - `reopen_failed` takes the fault away, loses the process and
//!   starts with the failure acknowledged, with the kernel's cache kept,
//!   as the log store's harness does: what a failed sync left marked
//!   clean and never written is still in the cache. The start drops the
//!   cache and serves only what the disk holds, which is what makes the
//!   volume as after a power cut (rule R11); the case
//!   `reopened_failed_volume_holds_what_was_flushed` fails on a store
//!   that does not.

use std::time::Duration;

use skyzfs_blk::conformance::{Capabilities, Harness, HarnessError, Spec};
use skyzfs_blk::{BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, RequestId};

use super::{InPlace, Mutants};
use crate::format::Key;
use crate::sim::{SeededCut, SimDisk};
use crate::store::{OpenOptions, Store, StoreConfig};

/// The log's segments: as the log store's harness has them.
const SEGMENT: u64 = (1 << 20) + (128 << 10);

/// One environment the suite runs the in-place store in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InPlaceHarness {
    pub(crate) queue_depth: u32,
    /// The simulated disk ends a sync begun on the helper after up to
    /// this many looks ([`SimDisk::set_sync_delay`]).
    pub(crate) sync_delay: u64,
    pub(crate) mutants: u32,
    /// How the store reads and writes ([`super::IoMode`]).
    pub(crate) io: super::IoMode,
    /// The system's page, which is the simulated disk's write unit, its
    /// file system's block, and the volume's write unit: 4096 on amd64,
    /// 16 KiB or 64 KiB on some arm64 kernels. Passed in, not read
    /// from the host, so that every size runs everywhere.
    pub(crate) page: u64,
}

impl InPlaceHarness {
    fn options(&self, read_only: bool, acknowledge_failure: bool) -> OpenOptions {
        OpenOptions {
            queue_depth: self.queue_depth,
            read_only,
            acknowledge_failure,
            in_place_io: self.io,
            ..OpenOptions::default()
        }
    }

    /// Start on `disk`: the log store's open, then the in-place store.
    /// A volume marked as failed is not served again: everything fails.
    fn start(
        &self,
        disk: SimDisk,
        read_only: bool,
        acknowledge_failure: bool,
        before: Option<Geometry>,
    ) -> Result<OnSim, HarnessError> {
        disk.set_sync_delay(self.sync_delay);
        let opened = Store::open(disk.clone(), self.options(read_only, acknowledge_failure));
        let store = match opened {
            Ok((store, _)) => store,
            // A volume marked as failed, or one whose start meets the
            // failed disk, is not served.
            Err(error) => {
                let geometry =
                    before.ok_or_else(|| HarnessError(format!("the first start: {error}")))?;
                return Ok(OnSim {
                    inner: Inner::Gone(Vec::new()),
                    disk,
                    geometry,
                });
            }
        };
        let geometry = store.geometry();
        let inner = match store.into_in_place_with(None, Mutants(self.mutants)) {
            Ok(store) => Inner::Live(Box::new(store)),
            // The start's checkpoint or sync met the failed disk.
            Err(_) if before.is_some() => Inner::Gone(Vec::new()),
            Err(error) => return Err(HarnessError(format!("the in-place store: {error}"))),
        };
        Ok(OnSim {
            inner,
            disk,
            geometry,
        })
    }
}

/// An in-place store on a simulated disk, or what is left of one that
/// failed.
pub(crate) struct OnSim {
    inner: Inner,
    pub(crate) disk: SimDisk,
    geometry: Geometry,
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
            Inner::Live(store) => store.submit(request),
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
        let config = StoreConfig {
            volume: *b"in-place volume.",
            size_mib: 4,
            base: spec.base,
            align: 64,
            // Room for a request of a megabyte beside the two units a
            // segment keeps.
            segment_size: SEGMENT.max((1 << 20) + 3 * self.page),
            write_unit,
        };
        let key = Key(*b"a key for tests!");
        let mut store = Store::create_keyed(disk.clone(), &config, key, self.options(false, false))
            .map_err(|e| HarnessError(format!("create: {e}")))?;
        // A log with records in it, which the start applies: a megabyte
        // of what the volume holds anyway, and as many single blocks as
        // the seed decides, half of them not flushed.
        let blocks = (1 << 20) / skyzfs_blk::BLOCK_USIZE;
        let mut data = Buf::zeroed(blocks);
        for (i, block) in data.chunks_mut(skyzfs_blk::BLOCK_USIZE).enumerate() {
            spec.base.fill_block(i as u64, block);
        }
        let first = Buf::from_blocks(&data[..skyzfs_blk::BLOCK_USIZE])
            .ok_or_else(|| HarnessError("no buffer".into()))?;
        let mut ops = vec![Op::Write { offset: 0, data }];
        for i in 0..spec.seed % 24 {
            if i == spec.seed % 12 {
                ops.push(Op::Flush);
            }
            ops.push(Op::Write {
                offset: 0,
                data: first.clone(),
            });
        }
        let mut done = Vec::new();
        for (i, op) in ops.into_iter().enumerate() {
            store
                .submit(Request {
                    id: RequestId(i as u64),
                    op,
                })
                .map_err(|e| HarnessError(e.to_string()))?;
            store.wait(&mut done, Duration::ZERO);
        }
        if done.iter().any(|c| c.result.is_err()) {
            return Err(HarnessError("ageing the log failed".into()));
        }
        drop(store);
        disk.kill();
        self.start(disk, spec.read_only, false, None)
    }

    fn kill(&mut self, target: OnSim, _seed: u64) -> Result<OnSim, HarnessError> {
        let OnSim {
            inner,
            disk,
            geometry,
        } = target;
        drop(inner);
        disk.kill();
        self.start(disk, geometry.read_only(), false, Some(geometry))
    }

    /// The seeds of the simulated disk's cut worth trying: eight.
    fn cut_points(&mut self, _target: &OnSim) -> u64 {
        7
    }

    /// The simulated disk's own cut, drawn from `keep`.
    fn power_cut(&mut self, target: OnSim, keep: u64) -> Result<OnSim, HarnessError> {
        let OnSim {
            inner,
            disk,
            geometry,
        } = target;
        drop(inner);
        disk.power_cut(SeededCut::new(keep ^ 0x5EED_C0DE));
        self.start(disk, geometry.read_only(), false, Some(geometry))
    }

    fn fail_log(&mut self, target: &mut OnSim) -> Result<(), HarnessError> {
        target.disk.fail_write(0, true);
        target.disk.fail_sync(0, true);
        Ok(())
    }

    fn reopen_failed(&mut self, target: OnSim) -> Result<OnSim, HarnessError> {
        let OnSim {
            inner,
            disk,
            geometry,
        } = target;
        drop(inner);
        disk.clear_faults();
        disk.kill();
        self.start(disk, geometry.read_only(), true, Some(geometry))
    }
}
