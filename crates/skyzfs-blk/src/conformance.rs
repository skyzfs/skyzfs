//! The conformance suite: rules R1 to R10 of the [crate documentation](crate)
//! run against any [`BlockTarget`].
//!
//! Passing it is the entry condition for every transport and store
//! (design §4.1).
//!
//! # Using it
//!
//! Implement [`Harness`] for the thing under test: how to create a fresh
//! device, how to cut its power and bring it back, and how to lose and
//! restart its daemon. Then:
//!
//! ```
//! use skyzfs_blk::conformance::{run_all, Config};
//! use skyzfs_blk::mem::{MemConfig, MemHarness};
//!
//! let mut harness = MemHarness::new(MemConfig::default());
//! let report = run_all(&mut harness, &Config { cases: 8, ..Config::default() });
//! assert!(report.passed(), "{report}");
//! ```
//!
//! [`cases`] lists the cases with the rule each verifies, for a rig that
//! wants one test per case.
//!
//! # How it judges
//!
//! Every request the suite submits and every completion it receives goes
//! through [`Model`]. A case fails when the model reports a
//! [`Violation`](crate::model::Violation), when a request does not
//! complete within [`Config::timeout`], or when `submit` refuses a request
//! the contract obliges the target to take. The property cases run random
//! programs of requests, reaps, power cuts and restarts with proptest, and
//! report the shrunk program on failure.
//!
//! The suite assumes a fault-free environment: an I/O error on a valid
//! request fails the case.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use proptest::prelude::*;
use proptest::strategy::Union;
use proptest::test_runner::{Config as ProptestConfig, RngAlgorithm, TestError, TestRng};
use proptest::test_runner::{TestCaseError, TestRunner};

use crate::model::Model;
use crate::{
    BlkError, BlockTarget, Completion, Geometry, Op, Request, RequestId, SubmitErrorKind,
    BLOCK_SIZE, BLOCK_USIZE,
};

/// The harness could not do what the suite asked of it. This is a failure
/// of the rig, not a verdict on the target, but it fails the case.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("harness: {0}")]
pub struct HarnessError(pub String);

/// What the suite needs from the thing under test.
pub trait Harness {
    /// The target type.
    type Target: BlockTarget;

    /// A fresh device on which every block reads as zeros. Each call must
    /// give a device independent of the earlier ones. The device needs at
    /// least 16 blocks and should be small: the suite reads back the first
    /// [`Config::window_blocks`] blocks often.
    fn create(&mut self) -> Result<Self::Target, HarnessError>;

    /// Crash and reopen: cut the power under `target` and return the
    /// device as it comes back (rule R10). Requests in flight are lost.
    /// `seed` is for a harness that can choose what a cut loses; a real
    /// rig may ignore it.
    fn power_cut(&mut self, target: Self::Target, seed: u64) -> Result<Self::Target, HarnessError>;

    /// Lose the daemon and bring it back while the client lives on (rule
    /// R9). The returned target has nothing in flight; the suite reissues
    /// what was. `seed` is as for [`Harness::power_cut`].
    fn restart(&mut self, target: Self::Target, seed: u64) -> Result<Self::Target, HarnessError>;
}

/// How hard to run the suite.
#[derive(Debug, Clone)]
pub struct Config {
    /// Random programs per property case.
    pub cases: u32,
    /// Seed for the property cases. A run is reproducible from it.
    pub seed: u64,
    /// The longest random program, in steps.
    pub max_steps: usize,
    /// The suite works in the first `window_blocks` blocks of the device,
    /// so that requests overlap. Clamped to the device.
    pub window_blocks: u64,
    /// How many power cuts or restarts, each with its own seed, the
    /// deterministic durability cases repeat.
    pub rounds: u32,
    /// How long to wait for a completion before the case fails.
    pub timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cases: 256,
            seed: 0x5ca1_ab1e,
            max_steps: 48,
            window_blocks: 24,
            rounds: 16,
            timeout: Duration::from_secs(10),
        }
    }
}

/// Why a case failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(pub String);

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<crate::model::Violation> for Failure {
    fn from(v: crate::model::Violation) -> Self {
        Failure(v.to_string())
    }
}

impl From<HarnessError> for Failure {
    fn from(e: HarnessError) -> Self {
        Failure(e.to_string())
    }
}

type Outcome = Result<(), Failure>;

fn fail<T>(message: impl Into<String>) -> Result<T, Failure> {
    Err(Failure(message.into()))
}

/// One case of the suite.
pub struct Case<H: Harness> {
    /// A stable name.
    pub name: &'static str,
    /// The contract rule the case verifies.
    pub rule: &'static str,
    /// Runs the case.
    pub run: fn(&mut H, &Config) -> Result<(), Failure>,
}

/// The result of one case.
#[derive(Debug, Clone)]
pub struct CaseOutcome {
    /// The case's name.
    pub name: &'static str,
    /// The rule it verifies.
    pub rule: &'static str,
    /// `Ok`, or why it failed.
    pub result: Result<(), Failure>,
}

/// The result of a run of the whole suite.
#[derive(Debug, Clone)]
pub struct Report {
    /// One entry per case, in the order of [`cases`].
    pub outcomes: Vec<CaseOutcome>,
}

impl Report {
    /// Whether every case passed.
    pub fn passed(&self) -> bool {
        self.outcomes.iter().all(|o| o.result.is_ok())
    }

    /// The names of the cases that failed.
    pub fn failed(&self) -> Vec<&'static str> {
        self.outcomes
            .iter()
            .filter(|o| o.result.is_err())
            .map(|o| o.name)
            .collect()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for o in &self.outcomes {
            match &o.result {
                Ok(()) => writeln!(f, "ok    {}", o.name)?,
                Err(e) => writeln!(f, "FAIL  {}\n      rule: {}\n      {}", o.name, o.rule, e)?,
            }
        }
        Ok(())
    }
}

/// Run every case and collect the results.
pub fn run_all<H: Harness>(harness: &mut H, config: &Config) -> Report {
    let outcomes = cases::<H>()
        .into_iter()
        .map(|case| CaseOutcome {
            name: case.name,
            rule: case.rule,
            result: (case.run)(harness, config),
        })
        .collect();
    Report { outcomes }
}

/// Run the case called `name`. `None` if there is no such case.
pub fn run_case<H: Harness>(
    harness: &mut H,
    config: &Config,
    name: &str,
) -> Option<Result<(), Failure>> {
    cases::<H>()
        .into_iter()
        .find(|case| case.name == name)
        .map(|case| (case.run)(harness, config))
}

/// The cases of the suite, each with the rule it verifies.
pub fn cases<H: Harness>() -> Vec<Case<H>> {
    vec![
        Case {
            name: "geometry_is_sane",
            rule: "R1: the device size and the largest request are non-zero multiples of 4 KiB; \
                   the geometry survives a power cut and a restart",
            run: geometry_is_sane::<H>,
        },
        Case {
            name: "fresh_device_reads_zeros",
            rule: "R3: a block never written reads as zeros",
            run: fresh_device_reads_zeros::<H>,
        },
        Case {
            name: "rejects_misaligned",
            rule: "R2: an offset or length that is not a multiple of 4 KiB is rejected, for all \
                   four ranged operations, without effect",
            run: rejects_misaligned::<H>,
        },
        Case {
            name: "rejects_out_of_bounds",
            rule: "R2: a request reaching past the end of the device is rejected without \
                   effect; a request ending exactly at the end is accepted",
            run: rejects_out_of_bounds::<H>,
        },
        Case {
            name: "rejects_zero_length_and_oversize",
            rule: "R2: a zero-length request, and a read or write longer than the stated \
                   maximum, are rejected; the maximum itself is accepted, and a discard and a \
                   write-zeroes may span the device",
            run: rejects_zero_length_and_oversize::<H>,
        },
        Case {
            name: "read_your_writes",
            rule: "R3, R4: once a write is acknowledged, reads return it, block for block, and \
                   an overwrite replaces it",
            run: read_your_writes::<H>,
        },
        Case {
            name: "write_zeroes_reads_zeros_and_is_durable",
            rule: "R7: write-zeroes reads as zeros and, once flushed, survives a power cut \
                   like a write",
            run: write_zeroes_reads_zeros_and_is_durable::<H>,
        },
        Case {
            name: "discard_reads_old_or_zeros",
            rule: "R3, R6: a discarded block reads as its last write or as zeros, also after \
                   a power cut; blocks next to the discard are untouched",
            run: discard_reads_old_or_zeros::<H>,
        },
        Case {
            name: "write_after_discard_wins",
            rule: "R6, R8: a write acknowledged after a discard is what reads return",
            run: write_after_discard_wins::<H>,
        },
        Case {
            name: "flushed_writes_survive_power_cut",
            rule: "R5, R10: every write acknowledged before a completed flush was submitted \
                   survives a power cut",
            run: flushed_writes_survive_power_cut::<H>,
        },
        Case {
            name: "unflushed_writes_are_old_or_new_never_torn",
            rule: "R4, R10: after a power cut a block holds its flushed content or one later \
                   write, never a mix, and what is read then is stable and survives a second \
                   power cut",
            run: unflushed_writes_are_old_or_new_never_torn::<H>,
        },
        Case {
            name: "flush_with_writes_in_flight",
            rule: "R5: a flush submitted while a write is in flight completes, and covers at \
                   least the writes acknowledged before it was submitted",
            run: flush_with_writes_in_flight::<H>,
        },
        Case {
            name: "queue_depth_requests_complete_in_any_order",
            rule: "R8: the target takes queue_depth requests in flight, completes each exactly \
                   once in whatever order, and refuses an id that is already in flight",
            run: queue_depth_requests_complete_in_any_order::<H>,
        },
        Case {
            name: "overlapping_writes_apply_in_ack_order",
            rule: "R8: of several writes in flight to one block, the one acknowledged last is \
                   what reads return",
            run: overlapping_writes_apply_in_ack_order::<H>,
        },
        Case {
            name: "restart_keeps_acknowledged_writes",
            rule: "R9: a write acknowledged before the daemon is lost is read back after it \
                   returns, and a flush after the restart makes it durable",
            run: restart_keeps_acknowledged_writes::<H>,
        },
        Case {
            name: "reissue_after_restart_is_idempotent",
            rule: "R9: requests in flight when the daemon is lost complete correctly when \
                   reissued under the same ids, whether or not they had already taken effect",
            run: reissue_after_restart_is_idempotent::<H>,
        },
        Case {
            name: "prop_invalid_requests_are_rejected_without_effect",
            rule: "R2: property: any invalid request is rejected and changes nothing",
            run: prop_invalid_requests::<H>,
        },
        Case {
            name: "prop_concurrent_programs_match_model",
            rule: "R3, R4, R6, R7, R8: property: random overlapping requests, reaped at random \
                   moments, read back as the model allows",
            run: prop_concurrent_programs::<H>,
        },
        Case {
            name: "prop_power_cuts_and_restarts_match_model",
            rule: "R5, R9, R10: property: random programs with power cuts and daemon restarts \
                   at random moments read back as the model allows",
            run: prop_crashing_programs::<H>,
        },
    ]
}

// ---------------------------------------------------------------------------
// The driver: a client that keeps the model in step with the target.
// ---------------------------------------------------------------------------

struct Driver<'a, H: Harness> {
    harness: &'a mut H,
    target: Option<H::Target>,
    geometry: Geometry,
    model: Model,
    /// A copy of every request in flight, for reissue after a restart.
    inflight: BTreeMap<RequestId, Request>,
    next_id: u64,
    stamp: u64,
    window: u64,
    timeout: Duration,
}

/// Fill `buf` with a pattern that names `stamp` and the block, differs in
/// every byte position group from any other stamp's, and is never zeros.
fn fill(buf: &mut [u8], stamp: u64, first_block: u64) {
    for (i, block) in buf.chunks_mut(BLOCK_USIZE).enumerate() {
        let mut z = stamp
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((first_block + i as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
        z ^= z >> 29;
        let word = (z | 0x0101_0101_0101_0101).to_le_bytes();
        for chunk in block.chunks_mut(8) {
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
}

impl<'a, H: Harness> Driver<'a, H> {
    fn new(harness: &'a mut H, config: &Config) -> Result<Self, Failure> {
        let target = harness.create()?;
        let geometry = target.geometry();
        if geometry.blocks() < 16 {
            return fail("the suite needs a device of at least 16 blocks");
        }
        Ok(Self {
            harness,
            target: Some(target),
            geometry,
            model: Model::new(geometry),
            inflight: BTreeMap::new(),
            next_id: 0,
            stamp: 0,
            window: config.window_blocks.clamp(16, geometry.blocks()),
            timeout: config.timeout,
        })
    }

    fn target(&mut self) -> Result<&mut H::Target, Failure> {
        match self.target.as_mut() {
            Some(t) => Ok(t),
            None => fail("no target: an earlier harness call failed"),
        }
    }

    /// Requests the suite keeps in flight at most.
    fn depth(&self) -> usize {
        (self.geometry.queue_depth() as usize).min(16)
    }

    /// Longest read or write the suite issues, in blocks.
    fn max_len(&self) -> u64 {
        (self.geometry.max_io_bytes() / BLOCK_SIZE).min(8)
    }

    fn submit(&mut self, op: Op) -> Result<RequestId, Failure> {
        self.next_id += 1;
        let request = Request {
            id: RequestId(self.next_id),
            op,
        };
        self.model.submit(&request)?;
        self.inflight.insert(request.id, request.clone());
        let id = request.id;
        if let Err(e) = self.target()?.submit(request) {
            return fail(format!(
                "R8: submit refused with {} requests in flight: {e}",
                self.inflight.len() - 1
            ));
        }
        Ok(id)
    }

    fn account(&mut self, done: &[Completion]) -> Outcome {
        for completion in done {
            self.model.complete(completion)?;
            self.inflight.remove(&completion.id);
        }
        Ok(())
    }

    /// Wait for at least one completion, if anything is in flight.
    fn reap(&mut self) -> Result<Vec<Completion>, Failure> {
        let mut done = Vec::new();
        if self.inflight.is_empty() {
            return Ok(done);
        }
        let timeout = self.timeout;
        if self.target()?.wait(&mut done, timeout) == 0 || done.is_empty() {
            return fail(format!(
                "R8: {} requests in flight and none completed within {timeout:?}",
                self.inflight.len()
            ));
        }
        self.account(&done)?;
        Ok(done)
    }

    /// Take what is ready without waiting.
    fn poll(&mut self) -> Outcome {
        let mut done = Vec::new();
        self.target()?.poll(&mut done);
        self.account(&done)
    }

    /// Reap until nothing is in flight.
    fn drain(&mut self) -> Result<Vec<Completion>, Failure> {
        let mut all = Vec::new();
        while !self.inflight.is_empty() {
            all.append(&mut self.reap()?);
        }
        Ok(all)
    }

    /// Submit one request and reap until it completes.
    fn run(&mut self, op: Op) -> Result<Completion, Failure> {
        let id = self.submit(op)?;
        loop {
            if let Some(c) = self.reap()?.into_iter().find(|c| c.id == id) {
                return Ok(c);
            }
            if !self.inflight.contains_key(&id) {
                return fail(format!("{id} left the queue without a completion"));
            }
        }
    }

    /// Submit, making room first if the queue is at the suite's depth.
    fn submit_with_room(&mut self, op: Op) -> Result<RequestId, Failure> {
        while self.inflight.len() >= self.depth() {
            self.reap()?;
        }
        self.submit(op)
    }

    fn data(&mut self, block: u64, count: u64) -> Vec<u8> {
        self.stamp += 1;
        let mut data = vec![0; (count * BLOCK_SIZE) as usize];
        fill(&mut data, self.stamp, block);
        data
    }

    fn write_op(&mut self, block: u64, count: u64) -> Op {
        Op::Write {
            offset: block * BLOCK_SIZE,
            data: self.data(block, count),
        }
    }

    /// The longest read or write the target takes, in blocks.
    fn max_io_blocks(&self) -> u64 {
        self.geometry.max_io_bytes() / BLOCK_SIZE
    }

    /// Write fresh data, in as many requests as the target's largest
    /// request needs, one after the other. Returns the data.
    fn write(&mut self, block: u64, count: u64) -> Result<Vec<u8>, Failure> {
        let data = self.data(block, count);
        let step = (self.max_io_blocks() * BLOCK_SIZE) as usize;
        let mut offset = block * BLOCK_SIZE;
        for chunk in data.chunks(step) {
            self.run(Op::Write {
                offset,
                data: chunk.to_vec(),
            })?;
            offset += chunk.len() as u64;
        }
        Ok(data)
    }

    /// Read, in as many requests as needed, and return the bytes. The
    /// model has checked them.
    fn read(&mut self, block: u64, count: u64) -> Result<Vec<u8>, Failure> {
        let step = self.max_io_blocks();
        let mut all = Vec::with_capacity((count * BLOCK_SIZE) as usize);
        let mut done = 0;
        while done < count {
            let n = step.min(count - done);
            match self.run(read_op(block + done, n))?.buf {
                Some(mut buf) => all.append(&mut buf),
                None => return fail("read completed without a buffer"),
            }
            done += n;
        }
        Ok(all)
    }

    fn expect_read(&mut self, block: u64, expected: &[u8], what: &str) -> Outcome {
        let got = self.read(block, expected.len() as u64 / BLOCK_SIZE)?;
        if got == expected {
            Ok(())
        } else {
            fail(format!(
                "{what}: block {block} does not read back as written"
            ))
        }
    }

    fn flush(&mut self) -> Outcome {
        self.run(Op::Flush).map(|_| ())
    }

    /// Read the whole window through the model.
    fn verify(&mut self) -> Outcome {
        let step = self.max_len();
        let mut block = 0;
        while block < self.window {
            let count = step.min(self.window - block);
            self.submit_with_room(read_op(block, count))?;
            block += count;
        }
        self.drain().map(|_| ())
    }

    fn check_geometry(&mut self) -> Outcome {
        let now = self.target()?.geometry();
        if now == self.geometry {
            Ok(())
        } else {
            fail(format!(
                "R1: geometry changed from {:?} to {now:?}",
                self.geometry
            ))
        }
    }

    fn power_cut(&mut self, seed: u64) -> Outcome {
        let Some(target) = self.target.take() else {
            return fail("no target: an earlier harness call failed");
        };
        self.target = Some(self.harness.power_cut(target, seed)?);
        self.model.power_cut();
        self.inflight.clear();
        self.check_geometry()
    }

    /// Lose the daemon, bring it back and reissue what was in flight.
    fn restart(&mut self, seed: u64) -> Outcome {
        let Some(target) = self.target.take() else {
            return fail("no target: an earlier harness call failed");
        };
        self.target = Some(self.harness.restart(target, seed)?);
        self.model.restart();
        self.check_geometry()?;
        let mut reissue: Vec<Request> = self.inflight.values().cloned().collect();
        if seed & 1 == 1 {
            reissue.reverse();
        }
        for request in reissue {
            let id = request.id;
            if let Err(e) = self.target()?.submit(request) {
                return fail(format!("R9: reissue of {id} after restart refused: {e}"));
            }
        }
        Ok(())
    }

    /// Submit an invalid request and check how it is rejected. A read or
    /// write that is also longer than the largest request may be rejected
    /// for that instead: the contract does not rank the reasons.
    fn expect_rejection(&mut self, op: Op, want: fn(&BlkError) -> bool, what: &str) -> Outcome {
        let name = op.name();
        let oversize = op.carries_data()
            && op
                .range()
                .is_some_and(|(_, len)| len > self.geometry.max_io_bytes());
        let done = self.run(op)?;
        match done.result {
            Err(e) if want(&e) || (oversize && is_too_large(&e)) => Ok(()),
            other => fail(format!(
                "R2: {name} {what}: expected a rejection, got {other:?}"
            )),
        }
    }
}

fn read_op(block: u64, count: u64) -> Op {
    Op::Read {
        offset: block * BLOCK_SIZE,
        buf: vec![0xEE; (count * BLOCK_SIZE) as usize],
    }
}

/// The four ranged operations over `offset, len`, with buffers of `len`
/// bytes for the two that carry data.
fn ranged_ops(offset: u64, len: u64) -> Vec<Op> {
    vec![
        Op::Read {
            offset,
            buf: vec![0; len as usize],
        },
        Op::Write {
            offset,
            data: vec![0x5A; len as usize],
        },
        Op::Discard { offset, len },
        Op::WriteZeroes { offset, len },
    ]
}

fn is_misaligned(e: &BlkError) -> bool {
    matches!(e, BlkError::Misaligned { .. })
}

fn is_out_of_bounds(e: &BlkError) -> bool {
    matches!(e, BlkError::OutOfBounds { .. })
}

fn is_zero_length(e: &BlkError) -> bool {
    matches!(e, BlkError::ZeroLength)
}

fn is_too_large(e: &BlkError) -> bool {
    matches!(e, BlkError::TooLarge { .. })
}

// ---------------------------------------------------------------------------
// Deterministic cases.
// ---------------------------------------------------------------------------

fn geometry_is_sane<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let g = d.geometry;
    if g.size_bytes() == 0 || g.size_bytes() % BLOCK_SIZE != 0 {
        return fail(format!("R1: device size {} is not blocks", g.size_bytes()));
    }
    if g.max_io_bytes() == 0 || g.max_io_bytes() % BLOCK_SIZE != 0 {
        return fail(format!(
            "R1: max_io_bytes {} is not blocks",
            g.max_io_bytes()
        ));
    }
    if g.queue_depth() == 0 {
        return fail("R1: queue depth is zero");
    }
    d.restart(1)?;
    d.power_cut(1)
}

fn fresh_device_reads_zeros<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    d.verify()?;
    let last = d.geometry.blocks() - 1;
    let got = d.read(last, 1)?;
    if got.iter().any(|b| *b != 0) {
        return fail("R3: the last block of a fresh device is not zeros");
    }
    Ok(())
}

fn rejects_misaligned<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let before = d.write(0, 4)?;
    for (offset, len) in [
        (1, BLOCK_SIZE),
        (512, BLOCK_SIZE),
        (BLOCK_SIZE - 1, BLOCK_SIZE),
        (BLOCK_SIZE + 2048, 2 * BLOCK_SIZE),
        (0, 1),
        (0, 512),
        (BLOCK_SIZE, BLOCK_SIZE + 512),
        (BLOCK_SIZE, 2 * BLOCK_SIZE - 1),
    ] {
        for op in ranged_ops(offset, len) {
            d.expect_rejection(op, is_misaligned, &format!("at {offset}+{len}"))?;
        }
    }
    d.expect_read(0, &before, "R2: after rejected misaligned requests")
}

fn rejects_out_of_bounds<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let size = d.geometry.size_bytes();
    let last = d.geometry.blocks() - 1;
    let before = d.write(last - 1, 2)?;
    let far = u64::MAX - u64::MAX % BLOCK_SIZE;
    for (offset, len) in [
        (size, BLOCK_SIZE),
        (size - BLOCK_SIZE, 2 * BLOCK_SIZE),
        (size + 16 * BLOCK_SIZE, BLOCK_SIZE),
        (far - BLOCK_SIZE, BLOCK_SIZE),
        (far, BLOCK_SIZE),
    ] {
        for op in ranged_ops(offset, len) {
            d.expect_rejection(op, is_out_of_bounds, &format!("at {offset}+{len}"))?;
        }
    }
    for len in [far, far - size + BLOCK_SIZE] {
        d.expect_rejection(
            Op::Discard { offset: 0, len },
            is_out_of_bounds,
            "longer than the device",
        )?;
        d.expect_rejection(
            Op::WriteZeroes {
                offset: BLOCK_SIZE,
                len,
            },
            is_out_of_bounds,
            "longer than the device",
        )?;
    }
    d.expect_read(
        last - 1,
        &before,
        "R2: after rejected out-of-bounds requests",
    )?;
    // The last block itself is in bounds.
    let data = d.write(last, 1)?;
    d.expect_read(last, &data, "R2: the last block")
}

fn rejects_zero_length_and_oversize<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    for op in ranged_ops(BLOCK_SIZE, 0) {
        d.expect_rejection(op, is_zero_length, "of zero length")?;
    }
    let max_blocks = d.geometry.max_io_bytes() / BLOCK_SIZE;
    // The maximum is honoured only if it is small enough to test sensibly
    // and fits the device.
    if max_blocks <= 4096 {
        if max_blocks <= d.geometry.blocks() {
            let data = d.write(0, max_blocks)?;
            d.expect_read(0, &data, "R2: a request of exactly max_io_bytes")?;
        }
        let over = (max_blocks + 1) * BLOCK_SIZE;
        d.expect_rejection(
            Op::Read {
                offset: 0,
                buf: vec![0; over as usize],
            },
            |e| is_too_large(e) || is_out_of_bounds(e),
            "longer than max_io_bytes",
        )?;
        d.expect_rejection(
            Op::Write {
                offset: 0,
                data: vec![1; over as usize],
            },
            |e| is_too_large(e) || is_out_of_bounds(e),
            "longer than max_io_bytes",
        )?;
        if max_blocks < d.geometry.blocks() {
            d.expect_rejection(
                Op::Write {
                    offset: 0,
                    data: vec![1; over as usize],
                },
                is_too_large,
                "longer than max_io_bytes but within the device",
            )?;
        }
    }
    // Discard and write-zeroes carry no data and may span the device.
    let size = d.geometry.size_bytes();
    d.run(Op::Discard {
        offset: 0,
        len: size,
    })?;
    d.run(Op::WriteZeroes {
        offset: 0,
        len: size,
    })?;
    d.verify()
}

fn read_your_writes<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let n = d.max_len();
    let a = d.write(2, 1)?;
    d.expect_read(2, &a, "R3: a single block")?;
    let b = d.write(2, 1)?;
    d.expect_read(2, &b, "R3: an overwrite")?;
    let c = d.write(4, n)?;
    d.expect_read(4, &c, "R3: a multi-block write")?;
    // Partial overwrite: the rest of the earlier write must remain.
    if n >= 3 {
        let mid = d.write(5, 1)?;
        let got = d.read(4, n)?;
        let b = BLOCK_USIZE;
        if got[..b] != c[..b] || got[b..2 * b] != mid[..] || got[2 * b..] != c[2 * b..] {
            return fail("R3: a partial overwrite disturbed its neighbours");
        }
    }
    // Reads of single blocks out of a multi-block write.
    let b = BLOCK_USIZE;
    d.expect_read(4, &c[..b], "R3: the first block of a multi-block write")?;
    d.verify()
}

fn write_zeroes_reads_zeros_and_is_durable<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    for round in 0..u64::from(cfg.rounds) {
        let data = d.write(0, 4)?;
        d.flush()?;
        d.run(Op::WriteZeroes {
            offset: BLOCK_SIZE,
            len: 2 * BLOCK_SIZE,
        })?;
        let b = BLOCK_USIZE;
        let mut expected = data.clone();
        expected[b..3 * b].fill(0);
        d.expect_read(0, &expected, "R7: after write-zeroes")?;
        d.flush()?;
        d.power_cut(round)?;
        d.expect_read(0, &expected, "R7: after write-zeroes, flush and power cut")?;
    }
    Ok(())
}

fn discard_reads_old_or_zeros<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    for round in 0..u64::from(cfg.rounds) {
        let data = d.write(0, 6)?;
        d.run(Op::Discard {
            offset: 2 * BLOCK_SIZE,
            len: 2 * BLOCK_SIZE,
        })?;
        // The model accepts old or zeros for blocks 2 and 3. The
        // neighbours are checked here by hand as well.
        for _ in 0..3 {
            let got = d.read(0, 6)?;
            let b = BLOCK_USIZE;
            if got[..2 * b] != data[..2 * b] || got[4 * b..] != data[4 * b..] {
                return fail("R6: a discard changed blocks outside its range");
            }
        }
        d.flush()?;
        d.power_cut(round)?;
        d.verify()?;
        d.verify()?;
    }
    Ok(())
}

fn write_after_discard_wins<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    for round in 0..u64::from(cfg.rounds) {
        d.write(0, 4)?;
        d.run(Op::Discard {
            offset: 0,
            len: 4 * BLOCK_SIZE,
        })?;
        let data = d.write(1, 2)?;
        d.expect_read(1, &data, "R8: a write after a discard")?;
        d.flush()?;
        d.power_cut(round)?;
        d.expect_read(
            1,
            &data,
            "R8: a flushed write after a discard, after a power cut",
        )?;
    }
    Ok(())
}

fn flushed_writes_survive_power_cut<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let n = d.max_len();
    for round in 0..u64::from(cfg.rounds) {
        let single = d.write(0, 1)?;
        let multi = d.write(2, n)?;
        d.flush()?;
        // Unflushed writes elsewhere must not cost the flushed ones.
        d.write(12, 2)?;
        d.power_cut(round)?;
        d.expect_read(0, &single, "R5: a flushed block after a power cut")?;
        d.expect_read(
            2,
            &multi,
            "R5: a flushed multi-block write after a power cut",
        )?;
        d.verify()?;
    }
    Ok(())
}

fn unflushed_writes_are_old_or_new_never_torn<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let n = d.max_len();
    for round in 0..u64::from(cfg.rounds) {
        d.write(0, n)?;
        d.write(8, 1)?;
        d.flush()?;
        // Two generations of unflushed writes over the flushed ones.
        d.write(0, n)?;
        d.write(8, 1)?;
        d.write(0, n.min(2))?;
        d.power_cut(round)?;
        // The model accepts, per block, the flushed content or one of the
        // later writes, whole. Reading twice checks stability, and the
        // second cut checks that what was read is durable.
        d.verify()?;
        d.verify()?;
        d.power_cut(round.wrapping_add(0x1000))?;
        d.verify()?;
    }
    Ok(())
}

fn flush_with_writes_in_flight<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    if d.depth() < 3 {
        return Ok(());
    }
    for round in 0..u64::from(cfg.rounds) {
        let acked = d.write(0, 2)?;
        // A write and two flushes in flight together. The first flush was
        // submitted after `acked` was acknowledged, so it covers it.
        let op = d.write_op(4, 2);
        d.submit(op)?;
        d.submit(Op::Flush)?;
        d.submit(Op::Flush)?;
        d.drain()?;
        d.power_cut(round)?;
        d.expect_read(0, &acked, "R5: a write acknowledged before the flush")?;
        d.verify()?;
    }
    Ok(())
}

fn queue_depth_requests_complete_in_any_order<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let depth = (d.geometry.queue_depth() as u64).min(d.window) as usize;
    for _ in 0..4 {
        // queue_depth independent writes, none reaped until all are in.
        let mut ids = Vec::new();
        for block in 0..depth as u64 {
            let op = d.write_op(block, 1);
            ids.push(d.submit(op)?);
        }
        if depth >= 2 {
            // An id already in flight must be refused.
            let request = Request {
                id: ids[0],
                op: Op::Flush,
            };
            // A target that has already completed the first request
            // internally still holds its id until the completion is
            // reaped.
            match d.target()?.submit(request) {
                Err(e) if e.kind == SubmitErrorKind::DuplicateId => {}
                Err(e) if e.kind == SubmitErrorKind::Busy => {}
                other => {
                    return fail(format!(
                        "R8: an id in flight was submitted again and got {other:?}"
                    ))
                }
            }
        }
        let done = d.drain()?;
        if done.len() != depth {
            return fail(format!(
                "R8: {depth} requests submitted, {} completions",
                done.len()
            ));
        }
        // And as many reads in flight together.
        for block in 0..depth as u64 {
            d.submit(read_op(block, 1))?;
        }
        d.drain()?;
    }
    Ok(())
}

fn overlapping_writes_apply_in_ack_order<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let k = d.depth().min(8) as u64;
    let two = d.max_len().min(2);
    for round in 0..u64::from(cfg.rounds) {
        // k writes to block 3, and k more that overlap one another by a
        // block, all in flight together.
        for _ in 0..k {
            let op = d.write_op(3, 1);
            d.submit(op)?;
        }
        d.drain()?;
        d.verify()?;
        for i in 0..k {
            let op = d.write_op(6 + (i + round) % 2, two);
            d.submit(op)?;
        }
        d.drain()?;
        d.verify()?;
    }
    Ok(())
}

fn restart_keeps_acknowledged_writes<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let n = d.max_len();
    for round in 0..u64::from(cfg.rounds) {
        let data = d.write(1, n)?;
        d.restart(round)?;
        d.expect_read(1, &data, "R9: an acknowledged write after a restart")?;
        let more = d.write(10, 1)?;
        d.restart(round)?;
        d.flush()?;
        d.power_cut(round)?;
        d.expect_read(1, &data, "R9, R5: written, restart, flush, power cut")?;
        d.expect_read(10, &more, "R9, R5: written, restart, flush, power cut")?;
    }
    Ok(())
}

fn reissue_after_restart_is_idempotent<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    let k = d.depth().clamp(2, 8) as u64;
    if d.depth() < 2 {
        return Ok(());
    }
    for round in 0..u64::from(cfg.rounds) {
        let before = d.write(15, 1)?;
        // In flight when the daemon goes: writes (two of them to one
        // block), a write-zeroes, a discard, a flush and a read.
        let mut expected = Vec::new();
        for i in 0..k.saturating_sub(4).max(1) {
            let op = d.write_op(i, 1);
            if let Op::Write { data, .. } = &op {
                expected.push((i, data.clone()));
            }
            d.submit_with_room(op)?;
        }
        for op in [
            d.write_op(9, 1),
            d.write_op(9, 1),
            Op::WriteZeroes {
                offset: 11 * BLOCK_SIZE,
                len: BLOCK_SIZE,
            },
            Op::Discard {
                offset: 12 * BLOCK_SIZE,
                len: BLOCK_SIZE,
            },
            Op::Flush,
            read_op(15, 1),
        ] {
            d.submit_with_room(op)?;
        }
        if round % 2 == 1 {
            // Let the target get part of the way.
            d.poll()?;
        }
        d.restart(round)?;
        d.drain()?;
        for (block, data) in &expected {
            d.expect_read(*block, data, "R9: a reissued write")?;
        }
        d.expect_read(15, &before, "R9: a block no reissued request wrote")?;
        d.verify()?;
        // Everything is acknowledged now: a flush makes it durable.
        d.flush()?;
        d.power_cut(round)?;
        for (block, data) in &expected {
            d.expect_read(
                *block,
                data,
                "R9, R5: a reissued write, flushed, after a cut",
            )?;
        }
        d.verify()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property cases.
// ---------------------------------------------------------------------------

/// One step of a random program. Spans are `(first block, blocks)`.
#[derive(Debug, Clone)]
enum Step {
    Write(u64, u64),
    Read(u64, u64),
    WriteZeroes(u64, u64),
    Discard(u64, u64),
    Flush,
    /// Wait for at least one completion.
    Reap,
    /// Take what is ready without waiting.
    Poll,
    /// Reap until nothing is in flight.
    Drain,
    PowerCut(u64),
    Restart(u64),
}

fn step_strategy(window: u64, max_len: u64, crashes: bool) -> impl Strategy<Value = Step> {
    let span = move || (0..window, 1..=max_len).prop_map(move |(b, l)| (b, l.min(window - b)));
    let mut choices: Vec<(u32, BoxedStrategy<Step>)> = vec![
        (8, span().prop_map(|(b, l)| Step::Write(b, l)).boxed()),
        (6, span().prop_map(|(b, l)| Step::Read(b, l)).boxed()),
        (2, span().prop_map(|(b, l)| Step::WriteZeroes(b, l)).boxed()),
        (2, span().prop_map(|(b, l)| Step::Discard(b, l)).boxed()),
        (3, Just(Step::Flush).boxed()),
        (4, Just(Step::Reap).boxed()),
        (2, Just(Step::Poll).boxed()),
        (1, Just(Step::Drain).boxed()),
    ];
    if crashes {
        choices.push((2, any::<u64>().prop_map(Step::PowerCut).boxed()));
        choices.push((2, any::<u64>().prop_map(Step::Restart).boxed()));
    }
    Union::new_weighted(choices)
}

fn run_program<H: Harness>(h: &mut H, cfg: &Config, program: &[Step]) -> Outcome {
    let mut d = Driver::new(h, cfg)?;
    for step in program {
        match *step {
            Step::Write(b, l) => {
                let op = d.write_op(b, l);
                d.submit_with_room(op)?;
            }
            Step::Read(b, l) => {
                d.submit_with_room(read_op(b, l))?;
            }
            Step::WriteZeroes(b, l) => {
                d.submit_with_room(Op::WriteZeroes {
                    offset: b * BLOCK_SIZE,
                    len: l * BLOCK_SIZE,
                })?;
            }
            Step::Discard(b, l) => {
                d.submit_with_room(Op::Discard {
                    offset: b * BLOCK_SIZE,
                    len: l * BLOCK_SIZE,
                })?;
            }
            Step::Flush => {
                d.submit_with_room(Op::Flush)?;
            }
            Step::Reap => {
                d.reap()?;
            }
            Step::Poll => d.poll()?,
            Step::Drain => {
                d.drain()?;
            }
            Step::PowerCut(seed) => {
                d.power_cut(seed)?;
                d.verify()?;
            }
            Step::Restart(seed) => d.restart(seed)?,
        }
    }
    // Whatever the program did: the device reads as the model allows now,
    // after a flush and a power cut, and again after that.
    d.drain()?;
    d.verify()?;
    d.flush()?;
    d.power_cut(program.len() as u64)?;
    d.verify()?;
    d.verify()
}

/// Run `test` on values of `strategy`, reproducibly from the config's
/// seed, and turn a failure into the shrunk input and its reason.
fn check<S: Strategy>(
    cfg: &Config,
    salt: u8,
    strategy: S,
    test: impl Fn(S::Value) -> Outcome,
) -> Outcome {
    let mut seed = [salt; 32];
    seed[..8].copy_from_slice(&cfg.seed.to_le_bytes());
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig {
            cases: cfg.cases,
            failure_persistence: None,
            max_shrink_iters: 8192,
            ..ProptestConfig::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &seed),
    );
    match runner.run(&strategy, |value| {
        test(value).map_err(|f| TestCaseError::fail(f.0))
    }) {
        Ok(()) => Ok(()),
        Err(TestError::Fail(reason, value)) => fail(format!(
            "{reason}\n      minimal failing input (seed {:#x}): {value:?}",
            cfg.seed
        )),
        Err(TestError::Abort(reason)) => fail(format!("property test aborted: {reason}")),
    }
}

/// The geometry and window of the devices the harness makes, read from a
/// probe device so that strategies can be sized to it.
fn probe<H: Harness>(h: &mut H, cfg: &Config) -> Result<(Geometry, u64, u64), Failure> {
    let d = Driver::new(h, cfg)?;
    Ok((d.geometry, d.window, d.max_len()))
}

fn prop_programs<H: Harness>(h: &mut H, cfg: &Config, crashes: bool) -> Outcome {
    let (_, window, max_len) = probe(h, cfg)?;
    let strategy = proptest::collection::vec(
        step_strategy(window, max_len, crashes),
        1..=cfg.max_steps.max(1),
    );
    let h = RefCell::new(h);
    check(cfg, u8::from(crashes), strategy, |program| {
        run_program(&mut **h.borrow_mut(), cfg, &program)
    })
}

fn prop_concurrent_programs<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    prop_programs(h, cfg, false)
}

fn prop_crashing_programs<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    prop_programs(h, cfg, true)
}

fn prop_invalid_requests<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let (geometry, _, _) = probe(h, cfg)?;
    let blocks = geometry.blocks();
    let max_blocks = (geometry.max_io_bytes() / BLOCK_SIZE).min(64);
    // Offsets and lengths around every limit: aligned or not, inside,
    // at and beyond the end, and near u64::MAX.
    let jitter = || prop_oneof![2 => Just(0u64), 1 => 1..BLOCK_SIZE];
    let offset = (
        prop_oneof![4 => 0..=blocks + 2, 1 => Just(u64::MAX / BLOCK_SIZE - 1)],
        jitter(),
    )
        .prop_map(|(b, j)| b * BLOCK_SIZE + j);
    let len = (0..=max_blocks + 2, jitter()).prop_map(|(b, j)| b * BLOCK_SIZE + j);
    let op = (0..4usize, offset, len)
        .prop_map(|(kind, offset, len)| ranged_ops(offset, len).swap_remove(kind))
        .prop_filter("must be invalid", move |op| geometry.validate(op).is_err());
    let strategy = proptest::collection::vec(op, 1..12);
    let h = RefCell::new(h);
    check(cfg, 2, strategy, |ops| {
        let mut guard = h.borrow_mut();
        let mut d = Driver::new(&mut **guard, cfg)?;
        let n = d.max_len();
        let last = d.geometry.blocks() - 1;
        let head = d.write(0, n)?;
        let tail = d.write(last, 1)?;
        for op in ops {
            d.submit_with_room(op)?;
        }
        d.drain()?;
        d.expect_read(0, &head, "R2: after rejected requests")?;
        d.expect_read(last, &tail, "R2: after rejected requests")?;
        d.verify()
    })
}
