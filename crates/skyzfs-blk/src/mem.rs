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
//! - a mutation is appended to the log, and a read takes its data, either
//!   on receipt or at the acknowledgement ([`Timing`]), so the order of
//!   the log is not the order of the acknowledgements,
//! - a flush makes durable exactly the records that existed when it was
//!   submitted, and nothing later,
//! - [`MemTarget::kill`] is a daemon loss: requests in flight are
//!   forgotten, some of them after reaching the log; the log survives and
//!   recovery makes all of it durable,
//! - [`MemTarget::power_cut`] is a power cut at a point the test chooses:
//!   it keeps the durable records and that many more, and nothing after,
//! - [`MemTarget::fail_log`] loses the log for good, and the target
//!   either fails everything from that instant or goes on answering from
//!   memory until an append or a flush runs into the failure
//!   ([`LogFailure`]),
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

/// When a [`MemTarget`] does something that the contract lets it do at any
/// moment between receiving a request and acknowledging it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    /// When the request is acknowledged.
    AtAck,
    /// When the request is received.
    AtSubmit,
    /// One or the other, request by request, from the target's seed.
    Seeded,
}

/// How a [`MemTarget`] behaves once [`MemTarget::fail_log`] has taken its
/// log away. Both honour the contract (rule R11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFailure {
    /// The target knows at once: every request completed from then on
    /// fails, whatever it had already done.
    AtOnce,
    /// The target learns of the failure as a store does, from the next
    /// append or flush that needs the log. A read that has its data
    /// still completes successfully. A mutation that was in the log
    /// before the failure does too, unless its record is among what was
    /// lost, which the seed decides: then it fails, and reads answered
    /// from memory go on showing it. A flush that was in flight at the
    /// failure succeeds if its sync had finished, which the seed decides.
    /// A mutation that still has to be appended, and every flush received
    /// after the failure, fail. Until one of those has reported the
    /// failure a read is answered from memory. Everything received after
    /// the first reported failure fails, reads included, and so does
    /// everything a new instance receives; from then on an invalid
    /// request still in flight is failed as well, not rejected.
    WhenNoticed,
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
    /// R10: a power cut keeps part of a sector of the first write it
    /// loses: a tear finer than the volatile-cache disk's sector.
    TearOnCut,
    /// R10: a power cut leaves a sector that nothing ever wrote in a
    /// block the first operation it loses had written.
    CutLeavesForeign,
    /// R10: recovery serves the log it found without making it durable,
    /// so a second power cut takes back what the first recovery exposed.
    RecoveryNotDurable,
    /// R11: reads still succeed on a failed volume.
    FailedServesReads,
    /// R11: a flush succeeds on a failed log although it has something
    /// to make durable.
    FlushSucceedsOnFailedLog,
    /// R11: a daemon restart forgets that the volume has failed.
    FailureForgottenOnKill,
    /// R11: a failed volume that is opened again has lost the last
    /// record a flush covered.
    ReopenLosesFlushed,
    /// R12: a read-only target accepts writes.
    ReadOnlyAcceptsWrites,
    /// R4: a write of up to eight blocks also zeroes the block 128 blocks
    /// further on: an offset bug that damages a place the write never
    /// named.
    StrayWrite,
    /// The buffer of a write does not come back in its completion.
    DropWriteBuffer,
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
    /// When a write, write-zeroes or discard is appended to the log and
    /// becomes visible. A real write log appends on receipt and
    /// acknowledges later, so its order differs from the order of its
    /// acknowledgements.
    pub apply: Timing,
    /// When a read takes its data. A read taken on receipt and completed
    /// later returns the state from before operations acknowledged in
    /// between.
    pub read: Timing,
    /// Discard behaviour.
    pub discard: DiscardPolicy,
    /// What happens to requests around a failure of the log.
    pub failure: LogFailure,
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
            apply: Timing::Seeded,
            read: Timing::Seeded,
            discard: DiscardPolicy::Seeded,
            failure: LogFailure::AtOnce,
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
    /// The request's work is already done: a mutation is in the log, a
    /// read has its data, or (only after the log failed) a flush had
    /// finished its sync.
    early: bool,
    /// The request was received after the volume reported its failure.
    doomed: bool,
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
    /// The log is lost for good.
    failed: bool,
    /// A request has been failed for it, or a new instance has found it:
    /// every request received from now on fails.
    reported: bool,
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
            reported: false,
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
            if let Some((block, Effect::Data(data))) = first.effects.first_mut() {
                // The new block, with the old one's bytes from the middle
                // of its first sector on: torn inside a sector. At byte
                // 256, so that a model whose sector were half as large
                // would take it.
                let at = *block as usize * BLOCK_USIZE;
                data[256..512].copy_from_slice(&self.image[at + 256..at + 512]);
                first.effects.truncate(1);
                self.log.push(first);
            }
        }
        if self.fault(Fault::CutLeavesForeign) {
            let written = lost.iter().find_map(|record| {
                record
                    .effects
                    .iter()
                    .find_map(|(block, effect)| matches!(effect, Effect::Data(_)).then_some(*block))
            });
            if let Some(block) = written {
                // A sector of bytes no write ever held.
                let mut data = self.image
                    [block as usize * BLOCK_USIZE..(block as usize + 1) * BLOCK_USIZE]
                    .to_vec();
                data[..512].fill(0xA5);
                self.log.push(Record {
                    effects: vec![(block, Effect::Data(data.into_boxed_slice()))],
                });
            }
        }
        self.late_discards.clear();
        self.recover();
        self
    }

    /// The failed volume is repaired and opened again for a new client
    /// (rule R11): as after a power cut that keeps `keep` records beyond
    /// the durable ones, and no longer failed.
    pub fn reopen_failed(mut self, keep: u64) -> Self {
        self.failed = false;
        self.reported = false;
        if self.fault(Fault::ReopenLosesFlushed) && self.durable > 0 {
            self.durable -= 1;
            self.log.truncate(self.durable);
        }
        self.power_cut(keep)
    }

    /// The write log is lost for good (rule R11). What the requests in
    /// flight and the next ones get is [`MemConfig::failure`]'s choice.
    pub fn fail_log(&mut self) {
        if self.failed {
            return;
        }
        self.failed = true;
        match self.config.failure {
            LogFailure::AtOnce => self.reported = true,
            LogFailure::WhenNoticed => {
                let mut pending = std::mem::take(&mut self.pending);
                for queued in &mut pending {
                    let coin = self.rng.coin();
                    match queued.request.op {
                        Op::Read { .. } => {}
                        // The sync of a flush in flight may just have
                        // finished.
                        Op::Flush => {
                            if coin {
                                self.flush(queued.cover);
                                queued.early = true;
                            }
                        }
                        // The record of a mutation in the log may be among
                        // what was lost. It stays in memory, where reads
                        // see it, and the request fails.
                        _ => queued.early &= coin,
                    }
                }
                self.pending = pending;
            }
        }
    }

    /// What every start does: replay the log and make it durable before
    /// serving.
    fn recover(&mut self) {
        self.pending.clear();
        // A new instance finds a failed log before it serves anything.
        self.reported = self.failed;
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
            .chunks(BLOCK_USIZE)
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
    /// `done`: its work is already done. `doomed`: it was received after
    /// the volume reported its failure.
    fn refuse(&self, id: u64, op: &Op, done: bool, doomed: bool) -> Option<BlkError> {
        let is_read = matches!(op, Op::Read { .. });
        let blind_flush = *op == Op::Flush && self.fault(Fault::FlushSucceedsOnFailedLog);
        if self.failed && !(is_read && self.fault(Fault::FailedServesReads)) && !blind_flush {
            let fails = match self.config.failure {
                LogFailure::AtOnce => true,
                // Only what still needs the log runs into the failure.
                LogFailure::WhenNoticed => doomed || !(done || is_read),
            };
            if fails {
                return Some(BlkError::Failed);
            }
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
            early,
            doomed,
        } = queued;
        let id = request.id;
        let noticed =
            self.failed && self.reported && self.config.failure == LogFailure::WhenNoticed;
        let verdict = match self.admit(&request.op) {
            // Which error wins is not specified: this target, once it
            // knows it has failed, looks at nothing else.
            Err(_) if noticed => Err(BlkError::Failed),
            Err(e) => Err(e),
            Ok(false) => Ok(false),
            Ok(true) => match self.refuse(id.0, &request.op, early, doomed) {
                Some(e) => Err(e),
                None => Ok(true),
            },
        };
        if verdict == Err(BlkError::Failed) {
            self.reported = true;
        }
        if verdict == Ok(true) && !early {
            self.mutate(&request.op, split);
        }
        let result = verdict.map(|_| ());
        let buf = match request.op {
            Op::Read { offset, mut buf } => {
                if verdict == Ok(true) && !early {
                    self.fill(offset, &mut buf);
                }
                Some(buf)
            }
            Op::Write { .. } if self.fault(Fault::DropWriteBuffer) => None,
            Op::Write { data, .. } => Some(data),
            Op::Flush => {
                if verdict == Ok(true) {
                    for (first, count) in std::mem::take(&mut self.late_discards) {
                        let effects = (first..first + count).map(|b| (b, Effect::Zeros)).collect();
                        self.append(Record { effects });
                    }
                    self.flush(cover);
                }
                None
            }
            Op::Discard { .. } | Op::WriteZeroes { .. } => None,
        };
        Completion { id, result, buf }
    }

    /// Take a read's data.
    fn fill(&self, offset: u64, buf: &mut [u8]) {
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
    }

    /// Append a write, write-zeroes or discard to the log as one whole
    /// record and apply it. Nothing for a read or a flush.
    fn mutate(&mut self, op: &Op, split: usize) {
        match op {
            Op::Read { .. } | Op::Flush => {}
            Op::Write { offset, data } => {
                let take = if self.fault(Fault::ShortWrite) {
                    1
                } else {
                    usize::MAX
                };
                let mut record = self.write_record(*offset, data, split, take);
                if self.fault(Fault::StrayWrite) && data.blocks() <= 8 {
                    let stray = (offset / BLOCK_SIZE + 128) % self.geometry.blocks();
                    record.effects.push((stray, Effect::Zeros));
                }
                self.append(record);
            }
            Op::WriteZeroes { offset, len } => {
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
            }
            Op::Discard { offset, len } => {
                let first = offset / BLOCK_SIZE;
                let count = len / BLOCK_SIZE;
                if self.fault(Fault::DiscardAppliedLate) {
                    self.late_discards.push((first, count));
                    self.append(Record::default());
                } else {
                    let record = self.discard_record(first, count);
                    self.append(record);
                }
            }
        }
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
        let doomed = self.failed && self.reported;
        let admitted = self.geometry.validate(&request.op).is_ok()
            && self
                .refuse(request.id.0, &request.op, false, doomed)
                .is_none();
        let mut request = request;
        let mut split = 0;
        let mut early = false;
        if admitted {
            let timing = match &request.op {
                Op::Flush => Timing::AtAck,
                Op::Read { .. } => self.config.read,
                _ => self.config.apply,
            };
            early = match timing {
                Timing::AtAck => false,
                Timing::AtSubmit => true,
                Timing::Seeded => self.rng.coin(),
            };
            match &mut request.op {
                Op::Write { offset, data } if self.fault(Fault::SplitWrites) => {
                    // Half now, half at the acknowledgement.
                    early = false;
                    if data.blocks() > 1 {
                        split = data.blocks() / 2;
                        let record = self.write_record(*offset, data, 0, split);
                        self.append(record);
                    }
                }
                Op::Read { offset, buf } if early => {
                    let offset = *offset;
                    self.fill(offset, buf);
                }
                op if early => {
                    let op = op.clone();
                    self.mutate(&op, 0);
                }
                _ => {}
            }
        }
        self.pending.push(Queued {
            request,
            cover: self.log.len(),
            split,
            early,
            doomed,
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

/// The conformance harness for [`MemTarget`]. Each device's seed comes
/// from the configuration and from the suite's [`Spec`], so a run of many
/// cases sees many schedules and a failing one can be replayed.
///
/// [`Spec`]: crate::conformance::Spec
#[cfg(feature = "conformance")]
#[derive(Debug, Clone)]
pub struct MemHarness {
    config: MemConfig,
}

#[cfg(feature = "conformance")]
impl MemHarness {
    /// A harness that builds targets from `config`. The base, the
    /// read-only flag and part of the seed of each target come from the
    /// suite.
    pub fn new(config: MemConfig) -> Self {
        Self { config }
    }
}

#[cfg(feature = "conformance")]
impl crate::conformance::Harness for MemHarness {
    type Target = MemTarget;

    fn capabilities(&self) -> crate::conformance::Capabilities {
        crate::conformance::Capabilities::ALL
    }

    fn create(
        &mut self,
        spec: &crate::conformance::Spec,
    ) -> Result<MemTarget, crate::conformance::HarnessError> {
        let config = MemConfig {
            seed: self
                .config
                .seed
                .wrapping_add(spec.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
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

    fn reopen_failed(
        &mut self,
        target: MemTarget,
    ) -> Result<MemTarget, crate::conformance::HarnessError> {
        let keep = target.cut_points() / 2;
        Ok(target.reopen_failed(keep))
    }
}

#[cfg(test)]
mod tests {
    //! `MemTarget`'s own hooks. Its conformance to the contract is tested
    //! in `tests/conformance_mem.rs`.

    use super::*;
    use crate::{Buf, RequestId};

    const TICK: Duration = Duration::from_millis(1);

    fn target(schedule: Schedule) -> MemTarget {
        MemTarget::new(MemConfig {
            queue_depth: 8,
            schedule,
            apply: Timing::AtAck,
            read: Timing::AtAck,
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
            data: Buf::filled(1, fill),
        };
        assert_eq!(run(t, id, op).result, Ok(()));
    }

    /// The first byte of each of the first eight blocks.
    fn peek(t: &mut MemTarget) -> Vec<u8> {
        let op = Op::Read {
            offset: 0,
            buf: Buf::zeroed(8),
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
                data: Buf::filled(1, 0x22),
            },
        })
        .unwrap();
        let outcomes: std::collections::BTreeSet<u8> = (0..32)
            .map(|seed| peek(&mut t.clone().kill(seed))[0])
            .collect();
        assert_eq!(outcomes, [0, 0x22].into());
    }

    /// R8: with `Timing::AtSubmit` the log is appended on receipt, so its
    /// order is the order of submission whatever the order of the
    /// acknowledgements; and a read taken on receipt returns the state
    /// from before a write that is acknowledged while it is in flight.
    #[test]
    fn timings_at_submit_decouple_the_log_from_the_acknowledgements() {
        let mut t = MemTarget::new(MemConfig {
            apply: Timing::AtSubmit,
            read: Timing::AtSubmit,
            schedule: Schedule::Fifo,
            ..MemConfig::default()
        })
        .unwrap();
        let read = Request {
            id: RequestId(1),
            op: Op::Read {
                offset: 0,
                buf: Buf::zeroed(1),
            },
        };
        let write = |id, fill| Request {
            id: RequestId(id),
            op: Op::Write {
                offset: 0,
                data: Buf::filled(1, fill),
            },
        };
        t.submit(read).unwrap();
        t.submit(write(2, 0x44)).unwrap();
        t.submit(write(3, 0x55)).unwrap();
        // Both writes are in the log before either is acknowledged.
        assert_eq!(t.cut_points(), 2);
        let mut out = Vec::new();
        t.poll(&mut out);
        assert_eq!(out.len(), 3);
        // The read was taken before the writes.
        assert_eq!(out[0].buf.as_ref().unwrap()[0], 0);
        assert_eq!(peek(&mut t)[0], 0x55);
        assert_eq!(peek(&mut t.clone().power_cut(1))[0], 0x44);
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
            buf: Buf::zeroed(1),
        };
        assert_eq!(run(&mut t, 2, Op::Flush).result, Err(BlkError::Failed));
        assert_eq!(run(&mut t, 3, read.clone()).result, Err(BlkError::Failed));
        let mut t = t.kill(0);
        assert_eq!(run(&mut t, 4, read).result, Err(BlkError::Failed));
    }

    /// R11: a target that notices the failure late completes or fails
    /// what was already in its log, answers reads from memory with it
    /// either way, fails the first request that needs the log, and from
    /// then on fails everything.
    #[test]
    fn a_failure_noticed_late_spares_what_was_already_done() {
        let read = |id| Request {
            id: RequestId(id),
            op: Op::Read {
                offset: 0,
                buf: Buf::zeroed(1),
            },
        };
        let write = |id, fill| Request {
            id: RequestId(id),
            op: Op::Write {
                offset: 0,
                data: Buf::filled(1, fill),
            },
        };
        let mut logged_write = Vec::new();
        for seed in 0..16 {
            let mut t = MemTarget::new(MemConfig {
                queue_depth: 8,
                schedule: Schedule::Fifo,
                apply: Timing::AtSubmit,
                read: Timing::AtAck,
                failure: LogFailure::WhenNoticed,
                seed,
                ..MemConfig::default()
            })
            .unwrap();
            // Logged on receipt, not yet acknowledged.
            t.submit(write(1, 0x11)).unwrap();
            t.fail_log();
            // Received after the failure: a read, a write that cannot be
            // logged, a read behind it, and a misaligned request.
            t.submit(read(2)).unwrap();
            t.submit(write(3, 0x22)).unwrap();
            t.submit(read(4)).unwrap();
            t.submit(Request {
                id: RequestId(5),
                op: Op::Discard {
                    offset: 1,
                    len: BLOCK_SIZE,
                },
            })
            .unwrap();
            let mut out = Vec::new();
            t.poll(&mut out);
            assert_eq!(out.len(), 5);
            // The logged write: acknowledged, or its record was lost.
            assert!(matches!(out[0].result, Ok(()) | Err(BlkError::Failed)));
            logged_write.push(out[0].result.is_ok());
            // Reads see it either way, and not the write that could not
            // be logged, also once the failure has been reported.
            assert_eq!(out[2].result, Err(BlkError::Failed));
            for read in [&out[1], &out[3]] {
                assert_eq!(read.result, Ok(()));
                assert_eq!(read.buf.as_ref().unwrap()[0], 0x11);
            }
            // Completed after the report: failed, not rejected.
            assert_eq!(out[4].result, Err(BlkError::Failed));
            // Received after the report: fails, and so does everything
            // after a restart.
            out.clear();
            t.submit(read(6)).unwrap();
            t.poll(&mut out);
            assert_eq!(out[0].result, Err(BlkError::Failed));
            let mut t = t.kill(0);
            assert_eq!(run(&mut t, 7, Op::Flush).result, Err(BlkError::Failed));
        }
        assert!(logged_write.contains(&true) && logged_write.contains(&false));
    }

    /// R11: a flush received after the failure fails even when nothing
    /// has reported the failure yet, and a new instance fails reads at
    /// once.
    #[test]
    fn a_failure_noticed_late_fails_the_next_flush() {
        let mut t = MemTarget::new(MemConfig {
            schedule: Schedule::Fifo,
            failure: LogFailure::WhenNoticed,
            ..MemConfig::default()
        })
        .unwrap();
        t.fail_log();
        let read = Op::Read {
            offset: 0,
            buf: Buf::zeroed(1),
        };
        assert_eq!(run(&mut t.clone(), 1, read.clone()).result, Ok(()));
        assert_eq!(run(&mut t, 2, Op::Flush).result, Err(BlkError::Failed));
        assert_eq!(run(&mut t, 3, read.clone()).result, Err(BlkError::Failed));
        let mut fresh = MemTarget::new(MemConfig {
            failure: LogFailure::WhenNoticed,
            ..MemConfig::default()
        })
        .unwrap();
        fresh.fail_log();
        let mut fresh = fresh.kill(0);
        assert_eq!(run(&mut fresh, 4, read).result, Err(BlkError::Failed));
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
            buf: Buf::zeroed(1),
        };
        assert_eq!(&run(&mut t, 1, read).buf.unwrap()[..], &expected[..]);
        let write = Op::Write {
            offset: 0,
            data: Buf::filled(1, 1),
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
