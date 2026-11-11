//! An independent target, as a check on the suite that `MemTarget` cannot
//! give: it shares no code with `MemTarget`.
//!
//! Written by the review session during the review of pull request #4
//! (branch `skyzfs-review`, `review/m1.1/review_target.rs` at e82406d) and
//! adopted here. Changes made on adoption: the modes assert instead of
//! printing, buffers are the contract's `Buf`, and the target's seed comes
//! from the suite's `Spec` so that failures replay. The target's logic is
//! as the reviewer wrote it; keep it independent of `MemTarget`.
//!
//! Rule numbers refer to the crate documentation of `skyzfs-blk`.
use skyzfs_blk::conformance::{run_all, Config, Harness, HarnessError, Spec};
use skyzfs_blk::*;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// Correct. Mutations are appended and applied when delivered at ack (like MemTarget).
    Late,
    /// Correct. Mutations are appended to the log and applied at submit; acked in random order.
    EarlyApply,
    /// Correct. As Late, but a read samples the image at submit.
    EarlyRead,
    /// Broken: memory applies at ack, log appends at submit: replay order differs from memory.
    LogAtSubmitMemAtAck,
    /// Broken: a discard brings back the version before the last write.
    DiscardOlder,
    /// API: the write buffer does not come back.
    NoWriteBuf,
    /// Broken: every write also zeroes the block 128 further on.
    Stray,
}

#[derive(Clone)]
enum Eff {
    Data(Vec<u8>),
    Zeros,
}
#[derive(Clone)]
struct Rec {
    first: u64,
    effs: Vec<Eff>,
}

struct Q {
    req: Request,
    applied: bool,
    sampled: Option<Vec<u8>>,
    cover: usize,
    rec: Option<Rec>,
}

struct T {
    mode: Mode,
    g: Geometry,
    base: Vec<u8>,
    log: Vec<Rec>,
    durable: usize,
    image: Vec<u8>,
    q: Vec<Q>,
    rng: u64,
    failed: bool,
    prev: Vec<Vec<u8>>,
}

impl T {
    fn rnd(&mut self, n: usize) -> usize {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        if n == 0 {
            0
        } else {
            ((self.rng >> 33) as usize) % n
        }
    }
    fn replay(image: &mut [u8], r: &Rec) {
        for (i, e) in r.effs.iter().enumerate() {
            let at = (r.first as usize + i) * BLOCK_USIZE;
            match e {
                Eff::Data(d) => image[at..at + BLOCK_USIZE].copy_from_slice(d),
                Eff::Zeros => image[at..at + BLOCK_USIZE].fill(0),
            }
        }
    }
    fn rebuild(&mut self) {
        self.image = self.base.clone();
        for r in &self.log {
            Self::replay(&mut self.image, r);
        }
    }
    fn rec_of(&mut self, op: &Op) -> Option<Rec> {
        match op {
            Op::Write { offset, data } => Some(Rec {
                first: offset / BLOCK_SIZE,
                effs: data
                    .chunks(BLOCK_USIZE)
                    .map(|c| Eff::Data(c.to_vec()))
                    .collect(),
            }),
            Op::WriteZeroes { offset, len } => Some(Rec {
                first: offset / BLOCK_SIZE,
                effs: (0..len / BLOCK_SIZE).map(|_| Eff::Zeros).collect(),
            }),
            Op::Discard { offset, len } => {
                if self.mode == Mode::DiscardOlder {
                    let first = offset / BLOCK_SIZE;
                    let effs = (0..len / BLOCK_SIZE)
                        .map(|i| Eff::Data(self.prev[(first + i) as usize].clone()))
                        .collect();
                    Some(Rec { first, effs })
                } else {
                    Some(Rec {
                        first: offset / BLOCK_SIZE,
                        effs: vec![],
                    })
                }
            }
            _ => None,
        }
    }
    fn note_prev(&mut self, r: &Rec) {
        for (i, e) in r.effs.iter().enumerate() {
            if let Eff::Data(_) = e {
                let b = r.first as usize + i;
                let at = b * BLOCK_USIZE;
                self.prev[b] = self.image[at..at + BLOCK_USIZE].to_vec();
            }
        }
    }
    fn ok_to_apply(&self, op: &Op) -> bool {
        self.g.validate(op).is_ok() && !self.failed && !self.g.read_only()
    }
    fn finish(&mut self, mut q: Q) -> Completion {
        let id = q.req.id;
        if let Err(e) = self.g.validate(&q.req.op) {
            let buf = match q.req.op {
                Op::Read { buf, .. } => Some(buf),
                Op::Write { data, .. } => Some(data),
                _ => None,
            };
            return Completion {
                id,
                result: Err(e),
                buf,
            };
        }
        if self.failed {
            let buf = match q.req.op {
                Op::Read { buf, .. } => Some(buf),
                Op::Write { data, .. } => Some(data),
                _ => None,
            };
            return Completion {
                id,
                result: Err(BlkError::Failed),
                buf,
            };
        }
        let mutates = matches!(
            q.req.op,
            Op::Write { .. } | Op::WriteZeroes { .. } | Op::Discard { .. }
        );
        if self.g.read_only() && mutates {
            let buf = match q.req.op {
                Op::Write { data, .. } => Some(data),
                _ => None,
            };
            return Completion {
                id,
                result: Err(BlkError::ReadOnly),
                buf,
            };
        }
        if mutates && !q.applied {
            let rec = match q.rec.take() {
                Some(r) => r,
                None => self.rec_of(&q.req.op).unwrap(),
            };
            if self.mode == Mode::DiscardOlder && !matches!(q.req.op, Op::Discard { .. }) {
                self.note_prev(&rec);
            }
            Self::replay(&mut self.image, &rec);
            if self.mode == Mode::Stray
                && matches!(q.req.op, Op::Write { .. })
                && rec.effs.len() <= 8
            {
                let b = (rec.first + 128) % self.g.blocks();
                let extra = Rec {
                    first: b,
                    effs: vec![Eff::Zeros],
                };
                Self::replay(&mut self.image, &extra);
                self.log.push(rec);
                self.log.push(extra);
            } else if self.mode != Mode::LogAtSubmitMemAtAck {
                self.log.push(rec);
            }
        }
        match q.req.op {
            Op::Read { offset, mut buf } => {
                match q.sampled {
                    Some(s) => buf.copy_from_slice(&s),
                    None => {
                        let at = offset as usize;
                        let n = buf.len();
                        buf.copy_from_slice(&self.image[at..at + n])
                    }
                }
                Completion {
                    id,
                    result: Ok(()),
                    buf: Some(buf),
                }
            }
            Op::Write { data, .. } => Completion {
                id,
                result: Ok(()),
                buf: if self.mode == Mode::NoWriteBuf {
                    None
                } else {
                    Some(data)
                },
            },
            Op::Flush => {
                self.durable = self.durable.max(q.cover.min(self.log.len()));
                Completion {
                    id,
                    result: Ok(()),
                    buf: None,
                }
            }
            _ => Completion {
                id,
                result: Ok(()),
                buf: None,
            },
        }
    }
    fn some(&mut self, out: &mut Vec<Completion>, one: bool) -> usize {
        if self.q.is_empty() {
            return 0;
        }
        let n = self.rnd(self.q.len() + 1).max(usize::from(one));
        for _ in 0..n {
            let i = self.rnd(self.q.len());
            let q = self.q.remove(i);
            let c = self.finish(q);
            out.push(c);
        }
        n
    }
    fn land(&mut self) {
        // some in-flight late-apply requests reach the log, unacknowledged
        let qs = std::mem::take(&mut self.q);
        for q in qs {
            if self.rnd(2) == 1 {
                let _ = self.finish(q);
            }
        }
    }
}

impl BlockTarget for T {
    fn geometry(&self) -> Geometry {
        self.g
    }
    fn submit(&mut self, req: Request) -> Result<(), SubmitError> {
        if self.q.len() >= self.g.queue_depth() as usize {
            return Err(SubmitError {
                kind: SubmitErrorKind::Busy,
                request: req,
            });
        }
        if self.q.iter().any(|q| q.req.id == req.id) {
            return Err(SubmitError {
                kind: SubmitErrorKind::DuplicateId,
                request: req,
            });
        }
        let mut q = Q {
            applied: false,
            sampled: None,
            cover: self.log.len(),
            rec: None,
            req,
        };
        let mutates = matches!(
            q.req.op,
            Op::Write { .. } | Op::WriteZeroes { .. } | Op::Discard { .. }
        );
        if mutates && self.ok_to_apply(&q.req.op) {
            if self.mode == Mode::EarlyApply {
                let rec = self.rec_of(&q.req.op).unwrap();
                Self::replay(&mut self.image, &rec);
                self.log.push(rec);
                q.applied = true;
            } else if self.mode == Mode::LogAtSubmitMemAtAck {
                let rec = self.rec_of(&q.req.op).unwrap();
                self.log.push(rec.clone());
                q.rec = Some(rec);
            }
        }
        if self.mode == Mode::EarlyRead && self.g.validate(&q.req.op).is_ok() {
            if let Op::Read { offset, buf } = &q.req.op {
                let at = *offset as usize;
                q.sampled = Some(self.image[at..at + buf.len()].to_vec());
            }
        }
        if matches!(q.req.op, Op::Flush) {
            q.cover = self.log.len();
        }
        self.q.push(q);
        Ok(())
    }
    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        self.some(out, false)
    }
    fn wait(&mut self, out: &mut Vec<Completion>, _t: Duration) -> usize {
        self.some(out, true)
    }
}

struct H {
    mode: Mode,
}
impl Harness for H {
    type Target = T;
    fn create(&mut self, spec: &Spec) -> Result<T, HarnessError> {
        let g = Geometry::new(1, 16, spec.read_only).unwrap();
        let mut base = vec![0u8; g.size_bytes() as usize];
        for (b, c) in base.chunks_mut(BLOCK_USIZE).enumerate() {
            spec.base.fill_block(b as u64, c);
        }
        let prev = base.chunks(BLOCK_USIZE).map(|c| c.to_vec()).collect();
        Ok(T {
            mode: self.mode,
            g,
            image: base.clone(),
            base,
            log: vec![],
            durable: 0,
            q: vec![],
            rng: spec.seed * 7919 + 1,
            failed: false,
            prev,
        })
    }
    fn kill(&mut self, mut t: T, seed: u64) -> Result<T, HarnessError> {
        t.rng ^= seed;
        t.land();
        if t.mode == Mode::LogAtSubmitMemAtAck { /* log already has them */ }
        t.rebuild();
        t.durable = t.log.len();
        Ok(t)
    }
    fn cut_points(&mut self, t: &T) -> u64 {
        (t.log.len() - t.durable) as u64
    }
    fn power_cut(&mut self, mut t: T, keep: u64) -> Result<T, HarnessError> {
        t.rng ^= keep;
        t.land();
        let un = t.log.len() - t.durable;
        let keep = (keep % (un as u64 + 1)) as usize;
        t.log.truncate(t.durable + keep);
        t.rebuild();
        t.durable = t.log.len();
        Ok(t)
    }
    fn fail_log(&mut self, t: &mut T) -> Result<(), HarnessError> {
        t.failed = true;
        Ok(())
    }
    // Added with the hook itself (M1.2b): the volume comes back with
    // its whole log, as after a daemon loss, and serves again.
    fn reopen_failed(&mut self, mut t: T) -> Result<Result<T, T>, HarnessError> {
        t.land();
        t.rebuild();
        t.durable = t.log.len();
        t.failed = false;
        Ok(Ok(t))
    }
}

fn run(mode: Mode) -> skyzfs_blk::conformance::Report {
    run_all(
        &mut H { mode },
        &Config {
            cases: 96,
            ..Config::default()
        },
    )
}

/// A correct target must pass every case, with every rule exercised.
fn conforms(mode: Mode) {
    let report = run(mode);
    assert!(report.passed(), "{mode:?} is correct but failed:\n{report}");
    assert!(report.not_exercised().is_empty(), "{report}");
}

/// A broken target must fail, in the cases named.
fn caught(mode: Mode, by: &[&str]) {
    let report = run(mode);
    let failed = report.failed();
    for name in by {
        assert!(
            failed.contains(name),
            "{name} did not catch {mode:?}; failed: {failed:?}"
        );
    }
    assert!(!report.passed());
}

const CONCURRENT: &str = "prop_concurrent_programs_match_model";
const FAILURES: &str = "prop_failures_match_model";

/// R1 to R12: a correct target that applies each mutation when it is
/// acknowledged conforms.
#[test]
fn late_apply_conforms() {
    conforms(Mode::Late);
}

/// R7, R8: a correct target that appends to its log and applies on
/// receipt, and acknowledges in any order, conforms. The suite at df56432
/// rejected it (review finding B1: an operation in flight across a
/// completed flush).
#[test]
fn early_apply_conforms() {
    conforms(Mode::EarlyApply);
}

/// R3: a correct target whose reads take their data on receipt conforms.
/// The suite at df56432 rejected it (a read in flight across a completed
/// flush).
#[test]
fn early_read_conforms() {
    conforms(Mode::EarlyRead);
}

/// R8: a target whose log order differs from the order in memory is
/// caught: the winner of two overlapping operations must be the same in
/// memory and after replay.
#[test]
fn catches_log_order_differing_from_memory() {
    caught(Mode::LogAtSubmitMemAtAck, &[CONCURRENT, FAILURES]);
}

/// R6: a discard that brings back a version older than the content just
/// before it is caught.
#[test]
fn catches_discard_returning_an_older_version() {
    caught(Mode::DiscardOlder, &[CONCURRENT]);
}

/// The trait's promise that buffers make a round trip: a target that
/// keeps the buffer of a write is caught (review note N5).
#[test]
fn catches_a_write_buffer_that_does_not_come_back() {
    caught(Mode::NoWriteBuf, &["read_your_writes"]);
}

/// R4: a target whose writes also zero a block 128 further on is caught,
/// because the whole device is read back (review finding B3).
#[test]
fn catches_a_stray_write() {
    caught(Mode::Stray, &[CONCURRENT]);
}
