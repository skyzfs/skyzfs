//! The conformance suite's harness for the store on the simulated disk.
//!
//! - `kill` executes some of the queued requests, drops the store, kills
//!   the simulated process and opens the store again.
//! - `power_cut` uses the freedoms of the simulated disk. The records
//!   that no sync covers are the units: the first `keep` survive whole.
//!   Beyond them, by turns: nothing survives; the next record is torn
//!   (every other sector of it survives); or the next record is lost
//!   and every later one survives. Sectors that belong to no such
//!   record (the home image, the preimages, superblocks, segment
//!   headers), names and lengths go as a seed decides.
//! - `fail_log` makes every write and every sync fail from then on.

use std::time::Duration;

use skyzfs_blk::conformance::{Harness, HarnessError, Spec};
use skyzfs_blk::{
    BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, SubmitError, BLOCK_USIZE,
};

use super::*;
use crate::io::SECTOR;
use crate::sim::{Cut, Fate, SimDisk};

/// One environment the suite runs in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SimHarness {
    pub(crate) align: u32,
    pub(crate) unit: u32,
    /// Whether a lost sector reads as garbage, or as its old content.
    pub(crate) garbage: bool,
    pub(crate) queue_depth: u32,
    /// The log's limit of segment files.
    pub(crate) segments: Option<usize>,
    pub(crate) mutants: u32,
}

impl SimHarness {
    pub(crate) const SEGMENT: u64 = (1 << 20) + (128 << 10);

    fn options(&self, read_only: bool) -> OpenOptions {
        OpenOptions {
            queue_depth: self.queue_depth,
            read_only,
            log_segments: self.segments,
            mutants: Mutants(self.mutants),
            ..OpenOptions::default()
        }
    }

    fn lost(&self) -> Fate {
        if self.garbage {
            Fate::Garbage
        } else {
            Fate::Durable
        }
    }

    fn open(&self, disk: SimDisk, read_only: bool) -> Result<OnSim, HarnessError> {
        let inner = match Store::open(disk.clone(), self.options(read_only)) {
            Ok((store, _)) => Inner::Live(Box::new(store)),
            // The volume failed and the client is still attached: the
            // daemon does not serve it again.
            Err(StoreError::Log(LogError::MarkedFailed)) => Inner::Gone(Vec::new()),
            Err(other) => return Err(HarnessError(format!("open: {other}"))),
        };
        let geometry = match &inner {
            Inner::Live(store) => store.geometry(),
            Inner::Gone(_) => return Err(HarnessError("no geometry of a failed store".into())),
        };
        Ok(OnSim {
            inner,
            disk,
            geometry,
            read_only,
        })
    }
}

/// A store on a simulated disk, or what is left of one that failed.
pub(crate) struct OnSim {
    inner: Inner,
    pub(crate) disk: SimDisk,
    geometry: Geometry,
    read_only: bool,
}

enum Inner {
    Live(Box<Store<SimDisk>>),
    /// The store failed and was not opened again: everything fails.
    Gone(Vec<Request>),
}

impl OnSim {
    pub(crate) fn store(&mut self) -> Option<&mut Store<SimDisk>> {
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

    fn submit(&mut self, request: Request) -> Result<(), SubmitError> {
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

    fn wait(&mut self, out: &mut Vec<Completion>, _: Duration) -> usize {
        self.poll(out)
    }
}

/// SplitMix64, for the fates the harness draws.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// What becomes of the records beyond the first `keep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Beyond {
    /// All are lost.
    Lost,
    /// The next one is torn, the rest are lost.
    Torn,
    /// The next one is lost, the rest survive.
    Hole,
}

/// A power cut by records.
struct RecordCut {
    /// The unsynced records: the file each is in and its bytes there.
    records: Vec<(String, u64, u64)>,
    keep: usize,
    beyond: Beyond,
    lost: Fate,
    rng: Rng,
}

impl Cut for RecordCut {
    fn sector(&mut self, name: Option<&str>, index: u64, versions: usize) -> Fate {
        let (from, to) = (index * SECTOR, (index + 1) * SECTOR);
        let mut inside = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, (file, start, end))| {
                Some(file.as_str()) == name && *start < to && *end > from
            })
            .map(|(i, _)| i);
        let Some(first) = inside.next() else {
            // Not a record's: the disk's own freedom.
            return match self.rng.next() % 3 {
                0 => Fate::Latest,
                1 => self.lost,
                _ => Fate::Version((self.rng.next() % versions.max(1) as u64) as usize),
            };
        };
        // A sector with a part of a surviving record survives.
        let survives = |i: usize| match self.beyond {
            _ if i < self.keep => true,
            Beyond::Hole => i > self.keep,
            Beyond::Torn => i == self.keep && index % 2 == 0,
            Beyond::Lost => false,
        };
        if survives(first) || inside.any(survives) {
            Fate::Latest
        } else {
            self.lost
        }
    }

    fn name(&mut self, _: usize, _: usize) -> bool {
        self.rng.next() % 4 != 0
    }

    fn length(&mut self, _: Option<&str>) -> bool {
        self.rng.next() % 2 == 0
    }
}

impl Harness for SimHarness {
    type Target = OnSim;

    fn create(&mut self, spec: &Spec) -> Result<OnSim, HarnessError> {
        let disk = SimDisk::new(spec.seed);
        disk.set_write_unit(u64::from(self.unit));
        let config = StoreConfig {
            volume: *b"conformance vol.",
            size_mib: 4,
            base: spec.base,
            align: self.align,
            segment_size: Self::SEGMENT,
            write_unit: self.unit,
        };
        let key = Key(*b"a key for tests!");
        let mut store = Store::create_keyed(disk.clone(), &config, key, self.options(false))
            .map_err(|e| HarnessError(format!("create: {e}")))?;
        // Age the log, so that the suite's programs meet the end of a
        // segment: write a megabyte of what the volume holds anyway,
        // and a number of single blocks that the seed decides.
        let blocks = (1 << 20) / BLOCK_USIZE;
        let mut data = Buf::zeroed(blocks);
        for (i, block) in data.chunks_mut(BLOCK_USIZE).enumerate() {
            spec.base.fill_block(i as u64, block);
        }
        let first = Buf::from_blocks(&data[..BLOCK_USIZE])
            .ok_or_else(|| HarnessError("no buffer".into()))?;
        let mut ops = vec![Op::Write { offset: 0, data }];
        for _ in 0..spec.seed % 24 {
            ops.push(Op::Write {
                offset: 0,
                data: first.clone(),
            });
        }
        ops.push(Op::Flush);
        let mut done = Vec::new();
        for (i, op) in ops.into_iter().enumerate() {
            store
                .submit(Request {
                    id: RequestId(i as u64),
                    op,
                })
                .map_err(|e| HarnessError(e.to_string()))?;
            store.poll(&mut done);
        }
        if done.iter().any(|c| c.result.is_err()) {
            return Err(HarnessError("ageing the log failed".into()));
        }
        drop(store);
        disk.kill();
        self.open(disk, spec.read_only)
    }

    fn kill(&mut self, mut target: OnSim, seed: u64) -> Result<OnSim, HarnessError> {
        if let Some(store) = target.store() {
            // Some of what is in flight got as far as it gets.
            let reached = seed as usize % (store.queue.len() + 1);
            store.run(reached, &mut Vec::new());
        }
        let OnSim {
            inner,
            disk,
            read_only,
            ..
        } = target;
        drop(inner);
        disk.kill();
        let geometry = target.geometry;
        let mut reopened = match self.open(disk.clone(), read_only) {
            Ok(reopened) => reopened,
            Err(_) => OnSim {
                inner: Inner::Gone(Vec::new()),
                disk,
                geometry,
                read_only,
            },
        };
        reopened.geometry = geometry;
        Ok(reopened)
    }

    fn cut_points(&mut self, target: &OnSim) -> u64 {
        match &target.inner {
            Inner::Live(store) => store.core.unsynced.len() as u64,
            Inner::Gone(_) => 0,
        }
    }

    fn power_cut(&mut self, mut target: OnSim, keep: u64) -> Result<OnSim, HarnessError> {
        let mut records = Vec::new();
        if let Some(store) = target.store() {
            let reached = keep as usize % (store.queue.len() + 1);
            store.run(reached, &mut Vec::new());
            for (start, end) in &store.core.unsynced {
                if let Some((file, at)) = store.core.log.locate(*start) {
                    records.push((file, at, at + (end.0 - start.0)));
                }
            }
        }
        let count = records.len();
        let (keep, beyond) = if keep as usize <= count {
            (
                keep as usize,
                [Beyond::Lost, Beyond::Torn, Beyond::Hole][(keep % 3) as usize],
            )
        } else {
            (
                keep as usize % (count + 1),
                [Beyond::Hole, Beyond::Lost, Beyond::Torn][(keep % 3) as usize],
            )
        };
        let OnSim {
            inner,
            disk,
            read_only,
            geometry,
        } = target;
        drop(inner);
        disk.power_cut(RecordCut {
            records,
            keep,
            beyond,
            lost: self.lost(),
            rng: Rng(keep as u64 ^ 0x5EED),
        });
        disk.set_write_unit(u64::from(self.unit));
        let mut reopened = self.open(disk, read_only)?;
        reopened.geometry = geometry;
        Ok(reopened)
    }

    fn fail_log(&mut self, target: &mut OnSim) -> Result<(), HarnessError> {
        target.disk.fail_write(0, true);
        target.disk.fail_sync(0, true);
        Ok(())
    }
}
