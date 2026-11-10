//! `MemTarget`: the contract in memory.
//!
//! It is the first implementation of [`BlockTarget`] and the one the
//! conformance suite is developed against. It keeps a simulated write log:
//! one record per write, write-zeroes or discard, appended whole at the
//! moment the operation is acknowledged, and a mark up to which the log is
//! durable. The device's content is its [`Base`] with the log replayed
//! over it.
//!
//! It is built to use every freedom the contract gives a target:
//!
//! - requests complete in an order drawn from a seed ([`Schedule`]),
//! - a flush makes durable exactly the records that existed when it was
//!   submitted, and nothing later,
//! - [`MemTarget::kill`] is a daemon loss: requests in flight are
//!   forgotten, some of them after reaching the log; the log survives and
//!   recovery makes all of it durable,
//! - [`MemTarget::power_cut`] is a power cut at a point the test chooses:
//!   it keeps the durable records and that many more, and nothing after,
//! - [`MemTarget::fail_log`] loses the log for good,
//! - a discard leaves the block, zeroes it, or returns it to the base
//!   ([`DiscardPolicy`]).
//!
//! [`Fault`] turns it into a deliberately broken target, to test that a
//! conformance run notices.

use std::time::Duration;

use crate::{
    Base, BlkError, BlockTarget, Completion, Geometry, GeometryError, Op, Request, SubmitError,
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

/// What a [`MemTarget`] does with a discarded block. All four honour the
/// contract (rule R6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardPolicy {
    /// The block reads as zeros afterwards.
    Zero,
    /// The block keeps its content.
    Keep,
    /// The block reads as the base afterwards.
    Base,
    /// Block by block, one of the three, from the target's seed.
    Seeded,
}

/// A deliberate breach of the contract, for testing a conformance run.
/// Each names the rule it breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// R2: a misaligned request completes `Ok`.
    AcceptMisaligned,
    /// R2: a request beyond the end of the device completes `Ok`.
    AcceptOutOfBounds,
    /// R3: reads are served from the durable content and miss
    /// acknowledged, unflushed operations.
    StaleReads,
    /// R4: only the first block of a multi-block write is written.
    ShortWrite,
    /// R5: write-zeroes completes without writing.
    WriteZeroesDoesNothing,
    /// R5: write-zeroes is stored as an absence, so on a volume with a
    /// base the base shows through.
    WriteZeroesShowsBase,
    /// R6: a discarded block reads as data that was never there.
    DiscardCorrupts,
    /// R6, R8: a discard takes effect at the next flush, and so hits data
    /// written after it.
    DiscardAppliedLate,
    /// R7: a flush completes without making anything durable.
    FlushDoesNothing,
    /// R7: the group-commit bug. A flush joins a sync that started before
    /// it arrived: it makes durable only what the previous flush saw.
    FlushJoinsOldSync,
    /// R7, R9: after a daemon loss, a flush covers only what the new
    /// instance acknowledged.
    FlushForgetsEarlierInstance,
    /// R8: a multi-block write is applied in two parts, so two
    /// overlapping writes can each win some of the blocks they share, and
    /// a power cut can keep half of one.
    SplitWrites,
    /// R9: an operation is acknowledged from memory, so a daemon loss
    /// loses whatever no flush covered.
    KillLosesUnflushed,
    /// R9: a request that was in flight at a daemon loss fails when it is
    /// reissued.
    RejectReissued,
    /// R10: a power cut keeps part of the first operation it loses.
    TearOnCut,
    /// R10: a power cut loses an operation and keeps later ones.
    CutDropsMiddle,
    /// R10: recovery serves the log it found without making it durable,
    /// so a second power cut takes back what the first recovery exposed.
    RecoveryNotDurable,
    /// R11: reads still succeed on a failed volume.
    FailedServesReads,
    /// R11: a daemon restart forgets that the volume has failed.
    FailureForgottenOnKill,
    /// R12: a read-only target accepts writes.
    ReadOnlyAcceptsWrites,
}

/// How to build a [`MemTarget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemConfig {
    /// Device size in MiB.
    pub size_mib: u64,
    /// Requests accepted in flight.
    pub queue_depth: u32,
    /// What the device holds before anything is written.
    pub base: Base,
    /// Whether the target is read-only.
    pub read_only: bool,
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
            size_mib: 1,
            queue_depth: 64,
            base: Base::Empty,
            read_only: false,
            schedule: Schedule::Seeded,
            discard: DiscardPolicy::Seeded,
            seed: 0,
            fault: None,
        }
    }
}

/// SplitMix64: small, seedable, and good enough to pick an order.
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

/// What a log record does to one block.
#[derive(Debug, Clone)]
enum Effect {
    Data(Box<[u8]>),
    Zeros,
    Base,
}

/// One whole operation in the write log.
#[derive(Debug, Clone, Default)]
struct Record {
    effects: Vec<(u64, Effect)>,
}

#[derive(Debug, Clone)]
struct Queued {
    request: Request,
    /// The length of the log when the request was submitted.
    cover: usize,
    /// `Fault::SplitWrites`: this many leading blocks are already logged.
    split: usize,
}

/// An in-memory block target. See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct MemTarget {
    config: MemConfig,
    geometry: Geometry,
    /// The base, materialised.
    base: Vec<u8>,
    /// The write log: whole operations in the daemon's order.
    log: Vec<Record>,
    /// Records before this index survive a power cut.
    durable: usize,
    /// The base with the whole log replayed: what reads see.
    image: Vec<u8>,
    pending: Vec<Queued>,
    rng: Rng,
    /// The log is lost for good: every request fails.
    failed: bool,
    /// `Fault::RejectReissued`: ids that were in flight at a kill.
    poisoned: Vec<u64>,
    /// `Fault::FlushJoinsOldSync`: what the previous flush saw.
    previous_cover: usize,
    /// `Fault::FlushForgetsEarlierInstance`: where this instance began.
    instance_start: usize,
    /// `Fault::DiscardAppliedLate`: discards waiting for the next flush.
    late_discards: Vec<(u64, u64)>,
}

impl MemTarget {
    /// A fresh device: every block reads as the base.
    pub fn new(config: MemConfig) -> Result<Self, GeometryError> {
        let geometry = Geometry::new(config.size_mib, config.queue_depth, config.read_only)?;
        let bytes = usize::try_from(geometry.size_bytes()).map_err(|_| GeometryError::BadSize)?;
        let mut base = vec![0; bytes];
        for (block, chunk) in base.chunks_mut(BLOCK_USIZE).enumerate() {
            config.base.fill_block(block as u64, chunk);
        }
        Ok(Self {
            config,
            geometry,
            image: base.clone(),
            base,
            log: Vec::new(),
            durable: 0,
            pending: Vec::new(),
            rng: Rng(config.seed),
            failed: false,
            poisoned: Vec::new(),
            previous_cover: 0,
            instance_start: 0,
            late_discards: Vec::new(),
        })
    }

    /// The configuration the target was built with.
    pub fn config(&self) -> MemConfig {
        self.config
    }

    fn fault(&self, fault: Fault) -> bool {
        self.config.fault == Some(fault)
    }

    /// The daemon is lost and returns; the client lives on (rule R9).
    /// Requests in flight are forgotten, some of them after reaching the
    /// log, as `seed` decides; the transport is expected to reissue them.
    /// The log survives, and recovery makes all of it durable.
    pub fn kill(mut self, seed: u64) -> Self {
        let mut rng = Rng(seed ^ 0x5EED_DA7A);
        if self.fault(Fault::RejectReissued) {
            self.poisoned = self.pending.iter().map(|q| q.request.id.0).collect();
        }
        self.land_some_in_flight(&mut rng);
        if self.fault(Fault::KillLosesUnflushed) {
            self.log.truncate(self.durable);
        }
        if self.fault(Fault::FailureForgottenOnKill) {
            self.failed = false;
        }
        self.recover();
        self
    }

    /// How many records a power cut could lose right now: the cut points
    /// of [`MemTarget::power_cut`] are `0..=cut_points()`.
    pub fn cut_points(&self) -> u64 {
        (self.log.len() - self.durable) as u64
    }

    /// Power is cut and returns (rule R10). Requests in flight are gone;
    /// some may have reached the log first. The device keeps the durable
    /// records and `keep` more, in log order, and nothing after them. A
    /// `keep` beyond what is there wraps around, so any number is a valid
    /// cut point.
    pub fn power_cut(mut self, keep: u64) -> Self {
        let mut rng = Rng(keep ^ 0xC2A5_11ED);
        self.land_some_in_flight(&mut rng);
        let unflushed = self.log.len() - self.durable;
        let keep = (keep % (unflushed as u64 + 1)) as usize;
        let mut lost = self.log.split_off(self.durable + keep);
        self.rebuild();
        if self.fault(Fault::TearOnCut) && !lost.is_empty() {
            let mut first = lost.remove(0);
            if first.effects.len() > 1 {
                first.effects.truncate(first.effects.len() / 2);
            } else if let Some((block, Effect::Data(data))) = first.effects.first_mut() {
                // Half the new block over half the old one.
                let at = *block as usize * BLOCK_USIZE;
                data[BLOCK_USIZE / 2..]
                    .copy_from_slice(&self.image[at + BLOCK_USIZE / 2..at + BLOCK_USIZE]);
            }
            self.log.push(first);
        }
        if self.fault(Fault::CutDropsMiddle) && keep >= 2 {
            self.log.remove(self.durable);
        }
        self.late_discards.clear();
        self.recover();
        self
    }

    /// The write log is lost for good (rule R11): every request fails from
    /// now on.
    pub fn fail_log(&mut self) {
        self.failed = true;
    }

    /// What every start does: replay the log and make it durable before
    /// serving.
    fn recover(&mut self) {
        self.pending.clear();
        self.rebuild();
        self.instance_start = self.log.len();
        self.previous_cover = self.previous_cover.min(self.log.len());
        if !self.fault(Fault::RecoveryNotDurable) && !self.fault(Fault::FlushForgetsEarlierInstance)
        {
            self.durable = self.log.len();
        }
    }

    /// Replay the log over the base.
    fn rebuild(&mut self) {
        self.image.clone_from(&self.base);
        for record in &self.log {
            Self::replay(&mut self.image, &self.base, record);
        }
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

    fn replay(image: &mut [u8], base: &[u8], record: &Record) {
        for (block, effect) in &record.effects {
            let at = *block as usize * BLOCK_USIZE;
            let Some(dst) = image.get_mut(at..at + BLOCK_USIZE) else {
                continue;
            };
            match effect {
                Effect::Data(data) => dst.copy_from_slice(data),
                Effect::Zeros => dst.fill(0),
                Effect::Base => dst.copy_from_slice(&base[at..at + BLOCK_USIZE]),
            }
        }
    }

    /// Append one whole operation to the log and apply it.
    fn append(&mut self, record: Record) {
        Self::replay(&mut self.image, &self.base, &record);
        self.log.push(record);
    }

    fn write_record(&self, offset: u64, data: &[u8], skip: usize, take: usize) -> Record {
        let first = offset / BLOCK_SIZE;
        let effects = data
            .chunks_exact(BLOCK_USIZE)
            .enumerate()
            .skip(skip)
            .take(take)
            .map(|(i, chunk)| (first + i as u64, Effect::Data(Box::from(chunk))))
            .collect();
        Record { effects }
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

    /// Whether a valid request must fail because of the target's state.
    fn refuse(&self, id: u64, op: &Op) -> Option<BlkError> {
        let is_read = matches!(op, Op::Read { .. });
        if self.failed && !(is_read && self.fault(Fault::FailedServesReads)) {
            return Some(BlkError::Failed);
        }
        if self.poisoned.contains(&id) {
            return Some(BlkError::Io(std::io::ErrorKind::AlreadyExists));
        }
        let mutates = !is_read && *op != Op::Flush;
        if self.config.read_only && mutates && !self.fault(Fault::ReadOnlyAcceptsWrites) {
            return Some(BlkError::ReadOnly);
        }
        None
    }

    /// Carry out one request. Called at the moment of acknowledgement.
    fn execute(&mut self, queued: Queued) -> Completion {
        let Queued {
            request,
            cover,
            split,
        } = queued;
        let id = request.id;
        let verdict = match self.admit(&request.op) {
            Err(e) => Err(e),
            Ok(false) => Ok(false),
            Ok(true) => match self.refuse(id.0, &request.op) {
                Some(e) => Err(e),
                None => Ok(true),
            },
        };
        let (result, buf) = match (verdict, request.op) {
            (Err(e), Op::Read { buf, .. }) => (Err(e), Some(buf)),
            (Err(e), Op::Write { data, .. }) => (Err(e), Some(data)),
            (Err(e), _) => (Err(e), None),
            (Ok(false), Op::Read { buf, .. }) => (Ok(()), Some(buf)),
            (Ok(false), Op::Write { data, .. }) => (Ok(()), Some(data)),
            (Ok(false), _) => (Ok(()), None),
            (Ok(true), Op::Read { offset, mut buf }) => {
                let at = offset as usize;
                let end = at + buf.len();
                if self.fault(Fault::StaleReads) {
                    let mut stale = self.base.clone();
                    for record in &self.log[..self.durable] {
                        Self::replay(&mut stale, &self.base, record);
                    }
                    buf.copy_from_slice(&stale[at..end]);
                } else {
                    buf.copy_from_slice(&self.image[at..end]);
                }
                (Ok(()), Some(buf))
            }
            (Ok(true), Op::Write { offset, data }) => {
                let take = if self.fault(Fault::ShortWrite) {
                    1
                } else {
                    usize::MAX
                };
                let record = self.write_record(offset, &data, split, take);
                self.append(record);
                (Ok(()), Some(data))
            }
            (Ok(true), Op::WriteZeroes { offset, len }) => {
                let first = offset / BLOCK_SIZE;
                let effects = (first..first + len / BLOCK_SIZE)
                    .filter(|_| !self.fault(Fault::WriteZeroesDoesNothing))
                    .map(|block| {
                        if self.fault(Fault::WriteZeroesShowsBase) {
                            (block, Effect::Base)
                        } else {
                            (block, Effect::Zeros)
                        }
                    })
                    .collect();
                self.append(Record { effects });
                (Ok(()), None)
            }
            (Ok(true), Op::Discard { offset, len }) => {
                let first = offset / BLOCK_SIZE;
                let count = len / BLOCK_SIZE;
                if self.fault(Fault::DiscardAppliedLate) {
                    self.late_discards.push((first, count));
                    self.append(Record::default());
                } else {
                    let record = self.discard_record(first, count);
                    self.append(record);
                }
                (Ok(()), None)
            }
            (Ok(true), Op::Flush) => {
                for (first, count) in std::mem::take(&mut self.late_discards) {
                    let effects = (first..first + count).map(|b| (b, Effect::Zeros)).collect();
                    self.append(Record { effects });
                }
                self.flush(cover);
                (Ok(()), None)
            }
        };
        Completion { id, result, buf }
    }

    fn discard_record(&mut self, first: u64, count: u64) -> Record {
        let mut effects = Vec::new();
        for block in first..first + count {
            if self.fault(Fault::DiscardCorrupts) {
                effects.push((block, Effect::Data(Box::from([0xDC; BLOCK_USIZE]))));
                continue;
            }
            let policy = match self.config.discard {
                DiscardPolicy::Seeded => match self.rng.below(3) {
                    0 => DiscardPolicy::Zero,
                    1 => DiscardPolicy::Base,
                    _ => DiscardPolicy::Keep,
                },
                fixed => fixed,
            };
            match policy {
                DiscardPolicy::Zero => effects.push((block, Effect::Zeros)),
                DiscardPolicy::Base => effects.push((block, Effect::Base)),
                DiscardPolicy::Keep | DiscardPolicy::Seeded => {}
            }
        }
        Record { effects }
    }

    /// Make durable exactly the records that existed when the flush was
    /// submitted (rule R7), as weakened by the configured fault.
    fn flush(&mut self, cover: usize) {
        let cover = cover.min(self.log.len());
        let upto = match self.config.fault {
            Some(Fault::FlushDoesNothing) => 0,
            Some(Fault::FlushJoinsOldSync) => {
                let next = cover.max(self.previous_cover);
                std::mem::replace(&mut self.previous_cover, next)
            }
            Some(Fault::FlushForgetsEarlierInstance) if self.durable < self.instance_start => 0,
            _ => cover,
        };
        self.durable = self.durable.max(upto);
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
        let mut split = 0;
        if self.fault(Fault::SplitWrites) {
            if let Op::Write { offset, data } = &request.op {
                let blocks = data.len() / BLOCK_USIZE;
                let admitted = self.geometry.validate(&request.op).is_ok()
                    && self.refuse(request.id.0, &request.op).is_none();
                if admitted && blocks > 1 {
                    split = blocks / 2;
                    let record = self.write_record(*offset, data, 0, split);
                    self.append(record);
                }
            }
        }
        self.pending.push(Queued {
            request,
            cover: self.log.len(),
            split,
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
    /// A harness that builds targets from `config`. The base and the
    /// read-only flag of each target come from the suite.
    pub fn new(config: MemConfig) -> Self {
        Self { config, created: 0 }
    }
}

#[cfg(feature = "conformance")]
impl crate::conformance::Harness for MemHarness {
    type Target = MemTarget;

    fn create(
        &mut self,
        spec: &crate::conformance::Spec,
    ) -> Result<MemTarget, crate::conformance::HarnessError> {
        self.created += 1;
        let config = MemConfig {
            seed: self.config.seed.wrapping_add(self.created),
            base: spec.base,
            read_only: spec.read_only,
            ..self.config
        };
        MemTarget::new(config).map_err(|e| crate::conformance::HarnessError(e.to_string()))
    }

    fn kill(
        &mut self,
        target: MemTarget,
        seed: u64,
    ) -> Result<MemTarget, crate::conformance::HarnessError> {
        Ok(target.kill(seed))
    }

    fn cut_points(&mut self, target: &MemTarget) -> u64 {
        target.cut_points()
    }

    fn power_cut(
        &mut self,
        target: MemTarget,
        keep: u64,
    ) -> Result<MemTarget, crate::conformance::HarnessError> {
        Ok(target.power_cut(keep))
    }

    fn fail_log(&mut self, target: &mut MemTarget) -> Result<(), crate::conformance::HarnessError> {
        target.fail_log();
        Ok(())
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
    fn peek(t: &mut MemTarget) -> Vec<u8> {
        let op = Op::Read {
            offset: 0,
            buf: vec![0; 8 * BLOCK_USIZE],
        };
        let buf = run(t, 999, op).buf.unwrap();
        buf.chunks(BLOCK_USIZE).map(|b| b[0]).collect()
    }

    /// R10: the power-cut hook cuts the simulated log where the test
    /// says. With eight unflushed one-block writes there are eight cut
    /// points beyond the durable part, and cutting at `k` leaves exactly
    /// the first `k` writes: a prefix, at every record boundary.
    #[test]
    fn power_cut_keeps_exactly_the_chosen_prefix() {
        let mut base = target(Schedule::Fifo);
        write(&mut base, 100, 7, 0x77);
        assert_eq!(run(&mut base, 101, Op::Flush).result, Ok(()));
        for block in 0..7 {
            write(&mut base, block, block, 0xA0 + block as u8);
        }
        assert_eq!(base.cut_points(), 7);
        for keep in 0..=7u64 {
            let mut after = base.clone().power_cut(keep);
            let expected: Vec<u8> = (0..7)
                .map(|b| if b < keep { 0xA0 + b as u8 } else { 0 })
                .chain([0x77])
                .collect();
            assert_eq!(peek(&mut after), expected, "cut at {keep}");
            // Recovery made what it found durable.
            assert_eq!(after.cut_points(), 0);
            assert_eq!(peek(&mut after.power_cut(0)), expected);
        }
    }

    /// R9: the daemon-loss hook keeps every acknowledged write, flushed
    /// or not, forgets what is in flight, and leaves the log durable.
    #[test]
    fn kill_keeps_the_log_and_forgets_in_flight() {
        let mut t = target(Schedule::Seeded);
        write(&mut t, 1, 0, 0x11);
        t.submit(Request {
            id: RequestId(2),
            op: Op::Flush,
        })
        .unwrap();
        for seed in 0..16 {
            let mut back = t.clone().kill(seed);
            let mut out = Vec::new();
            assert_eq!(back.wait(&mut out, TICK), 0, "nothing is in flight");
            assert_eq!(back.cut_points(), 0);
            assert_eq!(peek(&mut back)[0], 0x11);
        }
    }

    /// R9: the daemon-loss hook lets a request in flight reach the log or
    /// not, as its seed decides.
    #[test]
    fn kill_lands_some_requests_in_flight() {
        let mut t = target(Schedule::Seeded);
        t.submit(Request {
            id: RequestId(1),
            op: Op::Write {
                offset: 0,
                data: vec![0x22; BLOCK_USIZE],
            },
        })
        .unwrap();
        let outcomes: std::collections::BTreeSet<u8> = (0..32)
            .map(|seed| peek(&mut t.clone().kill(seed))[0])
            .collect();
        assert_eq!(outcomes, [0, 0x22].into());
    }

    /// R11: after the log-failure hook every request fails, and still
    /// does after a daemon restart.
    #[test]
    fn fail_log_fails_every_request() {
        let mut t = target(Schedule::Fifo);
        write(&mut t, 1, 0, 0x33);
        t.fail_log();
        let read = Op::Read {
            offset: 0,
            buf: vec![0; BLOCK_USIZE],
        };
        assert_eq!(run(&mut t, 2, Op::Flush).result, Err(BlkError::Failed));
        assert_eq!(run(&mut t, 3, read.clone()).result, Err(BlkError::Failed));
        let mut t = t.kill(0);
        assert_eq!(run(&mut t, 4, read).result, Err(BlkError::Failed));
    }

    /// R3, R12: a target opened read-only on a base reads as the base and
    /// refuses to write.
    #[test]
    fn read_only_target_serves_its_base() {
        let base = Base::Pattern(5);
        let mut t = MemTarget::new(MemConfig {
            base,
            read_only: true,
            ..MemConfig::default()
        })
        .unwrap();
        assert!(t.geometry().read_only());
        let mut expected = vec![0; BLOCK_USIZE];
        base.fill_block(3, &mut expected);
        let read = Op::Read {
            offset: 3 * BLOCK_SIZE,
            buf: vec![0; BLOCK_USIZE],
        };
        assert_eq!(run(&mut t, 1, read).buf.unwrap(), expected);
        let write = Op::Write {
            offset: 0,
            data: vec![1; BLOCK_USIZE],
        };
        assert_eq!(run(&mut t, 2, write).result, Err(BlkError::ReadOnly));
        assert_eq!(run(&mut t, 3, Op::Flush).result, Ok(()));
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
    /// requests out of submission order, each exactly once, so the suite
    /// has something to observe.
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
