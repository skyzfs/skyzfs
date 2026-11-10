//! `MemTarget`: the contract in memory.
//!
//! It is the first implementation of [`BlockTarget`] and the one the
//! conformance suite is developed against, so it is built to use every
//! freedom the contract gives a target:
//!
//! - requests complete in an order drawn from a seed ([`Schedule`]),
//! - a flush makes durable exactly the writes acknowledged before it was
//!   submitted and nothing later,
//! - [`MemTarget::crash`] is a power cut: from a seed it drops all, none,
//!   or some of the writes no flush covers, by whole write or block by
//!   block, and lets some of the requests in flight land first,
//! - [`MemTarget::restart`] is a daemon loss: requests in flight are
//!   forgotten, some of them after taking effect, and everything
//!   acknowledged is kept,
//! - a discard zeroes the block, leaves it, or either from a seed
//!   ([`DiscardPolicy`]).
//!
//! [`Fault`] turns it into a deliberately broken target, to test that a
//! conformance run notices.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use crate::{
    BlkError, BlockTarget, Completion, Geometry, GeometryError, Op, Request, SubmitError,
    SubmitErrorKind, BLOCK_SIZE, BLOCK_USIZE,
};

/// The order in which a [`MemTarget`] completes what is in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// In submission order, everything at each `poll`.
    Fifo,
    /// In an order drawn from the target's seed. `poll` completes a random
    /// number of requests, possibly none; `wait` at least one.
    Seeded,
}

/// What a [`MemTarget`] does with a discard. All three honour the contract
/// (rule R6, open reading 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardPolicy {
    /// The blocks read as zeros afterwards.
    Zero,
    /// The blocks keep their content.
    Keep,
    /// Block by block, one or the other, from the target's seed.
    Seeded,
}

/// A deliberate breach of the contract, for testing a conformance run.
/// Each names the rule it breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// R5: a flush completes without making anything durable.
    FlushDoesNothing,
    /// R10: a power cut leaves half of a lost block behind.
    TearOnCrash,
    /// R2: a misaligned request completes `Ok`.
    AcceptMisaligned,
    /// R2: a request beyond the end of the device completes `Ok`.
    AcceptOutOfBounds,
    /// R3: reads are served from the durable image and miss unflushed
    /// writes.
    StaleReads,
    /// R8: writes take effect when submitted, while their completions are
    /// delivered in another order.
    ApplyAtSubmit,
    /// R3, R6: a discarded block reads as garbage.
    DiscardCorrupts,
    /// R7: write-zeroes completes without writing.
    WriteZeroesDoesNothing,
    /// R9: a daemon restart loses acknowledged, unflushed writes.
    RestartLosesWrites,
    /// R9: a request that was in flight at a restart fails when reissued.
    RejectReissued,
    /// R4: only the first block of a multi-block write is written.
    ShortWrite,
}

/// How to build a [`MemTarget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemConfig {
    /// Device size in blocks.
    pub blocks: u64,
    /// Largest read or write in blocks. The default of 256 (1 MiB) is a
    /// placeholder, not a design decision (open reading 2).
    pub max_io_blocks: u64,
    /// Requests accepted in flight.
    pub queue_depth: u32,
    /// Completion order.
    pub schedule: Schedule,
    /// Discard behaviour.
    pub discard: DiscardPolicy,
    /// Seed for the schedule and the discard policy.
    pub seed: u64,
    /// A deliberate breach of the contract, or `None` for a correct target.
    pub fault: Option<Fault>,
}

impl Default for MemConfig {
    fn default() -> Self {
        Self {
            blocks: 256,
            max_io_blocks: 256,
            queue_depth: 64,
            schedule: Schedule::Seeded,
            discard: DiscardPolicy::Seeded,
            seed: 0,
            fault: None,
        }
    }
}

/// SplitMix64: small, seedable, and good enough to pick who survives.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number below `n`, or 0 if `n` is 0.
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

/// One block of one write that no flush has covered yet.
#[derive(Debug, Clone)]
struct Unflushed {
    /// The write it belongs to.
    group: u64,
    block: u64,
    /// `None` is a block of zeros.
    data: Option<Box<[u8]>>,
}

#[derive(Debug, Clone)]
struct Queued {
    request: Request,
    /// For a flush: how much of the log existed when it was submitted.
    cover: u64,
    /// `Fault::ApplyAtSubmit` already carried it out.
    applied: bool,
}

/// An in-memory block target. See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct MemTarget {
    config: MemConfig,
    geometry: Geometry,
    /// What reads see.
    image: Vec<u8>,
    /// What a power cut is guaranteed to leave.
    durable: Vec<u8>,
    /// Acknowledged block writes not yet covered by a flush, in
    /// acknowledgement order.
    log: VecDeque<Unflushed>,
    /// How many log entries have ever been drained by flushes.
    log_base: u64,
    groups: u64,
    pending: Vec<Queued>,
    rng: Rng,
    /// `Fault::RejectReissued`: ids that were in flight at a restart.
    poisoned: Vec<u64>,
}

impl MemTarget {
    /// A fresh device: every block reads as zeros.
    pub fn new(config: MemConfig) -> Result<Self, GeometryError> {
        let geometry = Geometry::new(config.blocks, config.max_io_blocks, config.queue_depth)?;
        let bytes = usize::try_from(geometry.size_bytes()).map_err(|_| GeometryError::BadSize)?;
        Ok(Self {
            config,
            geometry,
            image: vec![0; bytes],
            durable: vec![0; bytes],
            log: VecDeque::new(),
            log_base: 0,
            groups: 0,
            pending: Vec::new(),
            rng: Rng(config.seed),
            poisoned: Vec::new(),
        })
    }

    /// The configuration the target was built with.
    pub fn config(&self) -> MemConfig {
        self.config
    }

    /// Power cut and return. Requests in flight are gone; some may have
    /// taken effect first. Of the writes no flush covers, `seed` decides
    /// which survive: all, none, some by whole write, or some block by
    /// block.
    pub fn crash(mut self, seed: u64) -> Self {
        let mut rng = Rng(seed ^ 0xC2A5_11ED);
        self.land_some_in_flight(&mut rng);
        let mode = rng.below(4);
        self.lose_power(&mut rng, mode);
        self
    }

    /// The daemon is lost and returns, the client lives on. Requests in
    /// flight are forgotten, some of them after taking effect; the
    /// transport is expected to reissue them. Nothing acknowledged is lost.
    pub fn restart(mut self, seed: u64) -> Self {
        let mut rng = Rng(seed ^ 0x5EED_DA7A);
        if self.config.fault == Some(Fault::RejectReissued) {
            self.poisoned = self.pending.iter().map(|q| q.request.id.0).collect();
        }
        self.land_some_in_flight(&mut rng);
        if self.config.fault == Some(Fault::RestartLosesWrites) {
            self.lose_power(&mut rng, 0);
        }
        self
    }

    /// Carry out a random subset of the requests in flight, in a random
    /// order, without acknowledging them, and forget the rest.
    fn land_some_in_flight(&mut self, rng: &mut Rng) {
        let mut pending = std::mem::take(&mut self.pending);
        while !pending.is_empty() {
            let queued = pending.swap_remove(rng.below(pending.len()));
            if rng.coin() {
                let _ = self.execute(queued);
            }
        }
    }

    /// Apply the survivors of the log to the durable image and make that
    /// the device. Modes: 0 none survive, 1 all, 2 by whole write, 3 by
    /// block.
    fn lose_power(&mut self, rng: &mut Rng, mode: usize) {
        let mut by_group: BTreeMap<u64, bool> = BTreeMap::new();
        for entry in std::mem::take(&mut self.log) {
            let keep = match mode {
                0 => false,
                1 => true,
                2 => *by_group.entry(entry.group).or_insert_with(|| rng.coin()),
                _ => rng.coin(),
            };
            let at = entry.block as usize * BLOCK_USIZE;
            let (len, apply) = match (keep, self.config.fault) {
                (true, _) => (BLOCK_USIZE, true),
                (false, Some(Fault::TearOnCrash)) => (BLOCK_USIZE / 2, true),
                (false, _) => (0, false),
            };
            if apply {
                let Some(dst) = self.durable.get_mut(at..at + len) else {
                    continue;
                };
                match &entry.data {
                    Some(data) => dst.copy_from_slice(&data[..len]),
                    None => dst.fill(0),
                }
            }
        }
        self.log_base = 0;
        self.image.clone_from(&self.durable);
    }

    fn log_end(&self) -> u64 {
        self.log_base + self.log.len() as u64
    }

    /// Store one block in the image and remember it as unflushed.
    fn store(&mut self, group: u64, block: u64, data: Option<&[u8]>) {
        let at = block as usize * BLOCK_USIZE;
        let Some(dst) = self.image.get_mut(at..at + BLOCK_USIZE) else {
            return;
        };
        match data {
            Some(data) => dst.copy_from_slice(data),
            None => dst.fill(0),
        }
        self.log.push_back(Unflushed {
            group,
            block,
            data: data.map(Box::from),
        });
    }

    fn store_write(&mut self, offset: u64, data: &[u8]) {
        self.groups += 1;
        let group = self.groups;
        let first = offset / BLOCK_SIZE;
        let limit = if self.config.fault == Some(Fault::ShortWrite) {
            1
        } else {
            usize::MAX
        };
        for (i, chunk) in data.chunks_exact(BLOCK_USIZE).take(limit).enumerate() {
            self.store(group, first + i as u64, Some(chunk));
        }
    }

    /// The contract's validation, as weakened by the configured fault.
    /// `Ok(false)` means "complete `Ok` without doing anything".
    fn admit(&self, op: &Op) -> Result<bool, BlkError> {
        match (self.geometry.validate(op), self.config.fault) {
            (Ok(()), _) => Ok(true),
            (Err(BlkError::Misaligned { .. }), Some(Fault::AcceptMisaligned)) => Ok(false),
            (Err(BlkError::OutOfBounds { .. }), Some(Fault::AcceptOutOfBounds)) => Ok(false),
            (Err(e), _) => Err(e),
        }
    }

    /// Carry out one request. Called at the moment of acknowledgement, so
    /// the order of calls is the acknowledgement order (rule R8).
    fn execute(&mut self, queued: Queued) -> Completion {
        let Queued {
            request,
            cover,
            applied,
        } = queued;
        let id = request.id;
        let admitted = self.admit(&request.op);
        let poisoned = self.poisoned.contains(&id.0);
        let (result, buf) = match (admitted, request.op) {
            (Err(e), Op::Read { buf, .. }) => (Err(e), Some(buf)),
            (Err(e), Op::Write { data, .. }) => (Err(e), Some(data)),
            (Err(e), _) => (Err(e), None),
            (Ok(_), Op::Read { buf, .. }) if poisoned => (
                Err(BlkError::Io(std::io::ErrorKind::AlreadyExists)),
                Some(buf),
            ),
            (Ok(_), Op::Write { data, .. }) if poisoned => (
                Err(BlkError::Io(std::io::ErrorKind::AlreadyExists)),
                Some(data),
            ),
            (Ok(_), _) if poisoned => (Err(BlkError::Io(std::io::ErrorKind::AlreadyExists)), None),
            (Ok(false), Op::Read { buf, .. }) => (Ok(()), Some(buf)),
            (Ok(false), Op::Write { data, .. }) => (Ok(()), Some(data)),
            (Ok(false), _) => (Ok(()), None),
            (Ok(true), Op::Read { offset, mut buf }) => {
                let source = if self.config.fault == Some(Fault::StaleReads) {
                    &self.durable
                } else {
                    &self.image
                };
                let at = offset as usize;
                match source.get(at..at + buf.len()) {
                    Some(src) => {
                        buf.copy_from_slice(src);
                        (Ok(()), Some(buf))
                    }
                    None => (
                        Err(BlkError::Io(std::io::ErrorKind::UnexpectedEof)),
                        Some(buf),
                    ),
                }
            }
            (Ok(true), Op::Write { offset, data }) => {
                if !applied {
                    self.store_write(offset, &data);
                }
                (Ok(()), Some(data))
            }
            (Ok(true), Op::WriteZeroes { offset, len }) => {
                if self.config.fault != Some(Fault::WriteZeroesDoesNothing) {
                    self.groups += 1;
                    let first = offset / BLOCK_SIZE;
                    for block in first..first + len / BLOCK_SIZE {
                        self.store(self.groups, block, None);
                    }
                }
                (Ok(()), None)
            }
            (Ok(true), Op::Discard { offset, len }) => {
                self.groups += 1;
                let first = offset / BLOCK_SIZE;
                for block in first..first + len / BLOCK_SIZE {
                    if self.config.fault == Some(Fault::DiscardCorrupts) {
                        self.store(self.groups, block, Some(&[0xDC; BLOCK_USIZE]));
                        continue;
                    }
                    let zero = match self.config.discard {
                        DiscardPolicy::Zero => true,
                        DiscardPolicy::Keep => false,
                        DiscardPolicy::Seeded => self.rng.coin(),
                    };
                    if zero {
                        self.store(self.groups, block, None);
                    }
                }
                (Ok(()), None)
            }
            (Ok(true), Op::Flush) => {
                if self.config.fault != Some(Fault::FlushDoesNothing) {
                    self.make_durable(cover);
                }
                (Ok(()), None)
            }
        };
        Completion { id, result, buf }
    }

    /// Move the log up to absolute position `cover` into the durable
    /// image: exactly the writes acknowledged before the flush was
    /// submitted (rule R5).
    fn make_durable(&mut self, cover: u64) {
        while self.log_base < cover {
            let Some(entry) = self.log.pop_front() else {
                break;
            };
            self.log_base += 1;
            let at = entry.block as usize * BLOCK_USIZE;
            if let Some(dst) = self.durable.get_mut(at..at + BLOCK_USIZE) {
                match &entry.data {
                    Some(data) => dst.copy_from_slice(data),
                    None => dst.fill(0),
                }
            }
        }
    }

    fn complete_some(&mut self, out: &mut Vec<Completion>, at_least_one: bool) -> usize {
        if self.pending.is_empty() {
            return 0;
        }
        let count = match self.config.schedule {
            Schedule::Fifo => self.pending.len(),
            Schedule::Seeded => {
                let n = self.rng.below(self.pending.len() + 1);
                n.max(usize::from(at_least_one))
            }
        };
        for _ in 0..count {
            let index = match self.config.schedule {
                Schedule::Fifo => 0,
                Schedule::Seeded => self.rng.below(self.pending.len()),
            };
            let queued = self.pending.remove(index);
            let completion = self.execute(queued);
            out.push(completion);
        }
        count
    }
}

impl BlockTarget for MemTarget {
    fn geometry(&self) -> Geometry {
        self.geometry
    }

    fn submit(&mut self, request: Request) -> Result<(), SubmitError> {
        let kind = if self.pending.len() >= self.geometry.queue_depth() as usize {
            Some(SubmitErrorKind::Busy)
        } else if self.pending.iter().any(|q| q.request.id == request.id) {
            Some(SubmitErrorKind::DuplicateId)
        } else {
            None
        };
        if let Some(kind) = kind {
            return Err(SubmitError { kind, request });
        }
        let mut applied = false;
        if self.config.fault == Some(Fault::ApplyAtSubmit) {
            if let Op::Write { offset, data } = &request.op {
                if self.geometry.validate(&request.op).is_ok() {
                    let (offset, data) = (*offset, data.clone());
                    self.store_write(offset, &data);
                    applied = true;
                }
            }
        }
        self.pending.push(Queued {
            request,
            cover: self.log_end(),
            applied,
        });
        Ok(())
    }

    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        self.complete_some(out, false)
    }

    fn wait(&mut self, out: &mut Vec<Completion>, _timeout: Duration) -> usize {
        self.complete_some(out, true)
    }
}

/// The conformance harness for [`MemTarget`]: every device it creates gets
/// the next seed, so a run of many cases sees many schedules.
#[cfg(feature = "conformance")]
#[derive(Debug, Clone)]
pub struct MemHarness {
    config: MemConfig,
    created: u64,
}

#[cfg(feature = "conformance")]
impl MemHarness {
    /// A harness that builds targets from `config`.
    pub fn new(config: MemConfig) -> Self {
        Self { config, created: 0 }
    }
}

#[cfg(feature = "conformance")]
impl crate::conformance::Harness for MemHarness {
    type Target = MemTarget;

    fn create(&mut self) -> Result<MemTarget, crate::conformance::HarnessError> {
        self.created += 1;
        let config = MemConfig {
            seed: self.config.seed.wrapping_add(self.created),
            ..self.config
        };
        MemTarget::new(config).map_err(|e| crate::conformance::HarnessError(e.to_string()))
    }

    fn power_cut(
        &mut self,
        target: MemTarget,
        seed: u64,
    ) -> Result<MemTarget, crate::conformance::HarnessError> {
        Ok(target.crash(seed))
    }

    fn restart(
        &mut self,
        target: MemTarget,
        seed: u64,
    ) -> Result<MemTarget, crate::conformance::HarnessError> {
        Ok(target.restart(seed))
    }
}

#[cfg(test)]
mod tests {
    //! `MemTarget`'s own hooks. Its conformance to the contract is tested
    //! in `tests/conformance_mem.rs`.

    use super::*;
    use crate::RequestId;

    const TICK: Duration = Duration::from_millis(1);

    fn target(schedule: Schedule) -> MemTarget {
        MemTarget::new(MemConfig {
            blocks: 16,
            max_io_blocks: 8,
            queue_depth: 8,
            schedule,
            ..MemConfig::default()
        })
        .unwrap()
    }

    fn run(t: &mut MemTarget, id: u64, op: Op) -> Completion {
        t.submit(Request {
            id: RequestId(id),
            op,
        })
        .unwrap();
        let mut out = Vec::new();
        while out.is_empty() {
            t.wait(&mut out, TICK);
        }
        out.remove(0)
    }

    fn write(t: &mut MemTarget, id: u64, block: u64, fill: u8) {
        let op = Op::Write {
            offset: block * BLOCK_SIZE,
            data: vec![fill; BLOCK_USIZE],
        };
        assert_eq!(run(t, id, op).result, Ok(()));
    }

    /// The first byte of each of the first eight blocks.
    fn survivors(t: &mut MemTarget) -> Vec<u8> {
        let op = Op::Read {
            offset: 0,
            buf: vec![0; 8 * BLOCK_USIZE],
        };
        let buf = run(t, 999, op).buf.unwrap();
        buf.chunks(BLOCK_USIZE).map(|b| b[0]).collect()
    }

    /// R10: the crash hook drops unflushed writes from its seed. The same
    /// seed gives the same survivors, and over a range of seeds the hook
    /// loses everything, keeps everything, and loses a part.
    #[test]
    fn crash_drops_unflushed_writes_by_seed() {
        let mut base = target(Schedule::Fifo);
        for block in 0..8 {
            write(&mut base, block, block, 0xA0 + block as u8);
        }
        let all: Vec<u8> = (0..8).map(|b| 0xA0 + b).collect();
        let mut outcomes = std::collections::BTreeSet::new();
        for seed in 0..64 {
            let left = survivors(&mut base.clone().crash(seed));
            assert_eq!(left, survivors(&mut base.clone().crash(seed)));
            for (block, byte) in left.iter().enumerate() {
                assert!(*byte == 0 || *byte == all[block]);
            }
            outcomes.insert(left);
        }
        assert!(outcomes.contains(&vec![0; 8]), "no seed lost everything");
        assert!(outcomes.contains(&all), "no seed kept everything");
        assert!(outcomes.len() > 8, "seeds do not vary what is lost");
    }

    /// R5, R10: the crash hook never drops a write that a completed flush
    /// covers, whatever the seed.
    #[test]
    fn crash_keeps_flushed_writes_for_every_seed() {
        let mut base = target(Schedule::Fifo);
        for block in 0..8 {
            write(&mut base, block, block, 0xB0 + block as u8);
        }
        assert_eq!(run(&mut base, 100, Op::Flush).result, Ok(()));
        write(&mut base, 101, 9, 0xFF);
        let all: Vec<u8> = (0..8).map(|b| 0xB0 + b).collect();
        for seed in 0..64 {
            assert_eq!(survivors(&mut base.clone().crash(seed)), all);
        }
    }

    /// R9: the restart hook forgets what is in flight and keeps every
    /// acknowledged write, flushed or not.
    #[test]
    fn restart_keeps_acknowledged_writes_and_forgets_in_flight() {
        let mut t = target(Schedule::Seeded);
        write(&mut t, 1, 0, 0x11);
        t.submit(Request {
            id: RequestId(2),
            op: Op::Flush,
        })
        .unwrap();
        for seed in 0..16 {
            let mut back = t.clone().restart(seed);
            let mut out = Vec::new();
            assert_eq!(back.wait(&mut out, TICK), 0, "nothing is in flight");
            assert_eq!(survivors(&mut back)[0], 0x11);
        }
    }

    /// R8: `submit` refuses more than `queue_depth` requests in flight and
    /// an id that is in flight, and hands the request back.
    #[test]
    fn submit_refuses_a_full_queue_and_a_duplicate_id() {
        let mut t = target(Schedule::Fifo);
        let flush = |id| Request {
            id: RequestId(id),
            op: Op::Flush,
        };
        t.submit(flush(1)).unwrap();
        let err = t.submit(flush(1)).unwrap_err();
        assert_eq!(err.kind, SubmitErrorKind::DuplicateId);
        assert_eq!(err.request, flush(1));
        for id in 2..=8 {
            t.submit(flush(id)).unwrap();
        }
        assert_eq!(t.submit(flush(9)).unwrap_err().kind, SubmitErrorKind::Busy);
        let mut out = Vec::new();
        assert_eq!(t.poll(&mut out), 8);
        t.submit(flush(9)).unwrap();
    }

    /// R8: with the seeded schedule the target completes independent
    /// requests out of submission order, so the suite's ordering cases
    /// have something to observe.
    #[test]
    fn seeded_schedule_completes_out_of_order() {
        let mut t = target(Schedule::Seeded);
        let mut reordered = false;
        for round in 0..8u64 {
            for i in 0..8 {
                t.submit(Request {
                    id: RequestId(round * 8 + i),
                    op: Op::Flush,
                })
                .unwrap();
            }
            let mut out = Vec::new();
            while out.len() < 8 {
                t.wait(&mut out, TICK);
            }
            let ids: Vec<u64> = out.iter().map(|c| c.id.0).collect();
            let mut sorted = ids.clone();
            sorted.sort_unstable();
            reordered |= ids != sorted;
            sorted.dedup();
            assert_eq!(sorted.len(), 8, "each request completes exactly once");
        }
        assert!(reordered);
    }
}
