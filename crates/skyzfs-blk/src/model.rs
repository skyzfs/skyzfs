//! A reference model of the contract's observable behaviour.
//!
//! The model sees what a client sees: requests as they are submitted,
//! completions in the order they are delivered, power cuts and daemon
//! restarts. It answers one question for every completion: *may a correct
//! target have produced this?*
//!
//! # How it works
//!
//! For every block the model keeps the list of **versions** that a power
//! cut could expose, oldest first. The first is the durable floor: the
//! latest version that a completed flush covers. Every acknowledged write,
//! write-zeroes and discard appends a version; a completed flush drops the
//! versions older than the newest one it covers (rules R4, R5, R7).
//!
//! A version is a set of **options**, because the model does not always
//! know what the block holds: after a power cut it is any one of the
//! versions that were there, or of the writes that were in flight (rule
//! R10). An option is a block's content plus a `trimmed` mark, which says
//! the block may also read as zeros because it was discarded (rules R3,
//! R6).
//!
//! A read is checked block by block against the options of the latest
//! version, widened by every write and discard that was in flight at any
//! time while the read was (open reading 9). A read that raced nothing
//! also **narrows** the version to the options it is consistent with, so a
//! target that answers one thing now and another later, with no write in
//! between, is caught (rule R10, open reading 8).
//!
//! # What the model does not catch
//!
//! Two reads that both race the same in-flight write may see new then old.
//! The model checks each read on its own and accepts that.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{BlkError, Completion, Geometry, Op, Request, RequestId, BLOCK_SIZE, BLOCK_USIZE};

/// The content of one block.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    Zeros,
    Data(Arc<[u8]>),
}

impl Content {
    fn of(bytes: &[u8]) -> Self {
        if bytes.iter().all(|b| *b == 0) {
            Content::Zeros
        } else {
            Content::Data(Arc::from(bytes))
        }
    }

    fn matches(&self, bytes: &[u8]) -> bool {
        match self {
            Content::Zeros => bytes.iter().all(|b| *b == 0),
            Content::Data(d) => d.as_ref() == bytes,
        }
    }

    fn byte(&self, i: usize) -> u8 {
        match self {
            Content::Zeros => 0,
            Content::Data(d) => d.get(i).copied().unwrap_or(0),
        }
    }
}

/// One thing a block may hold.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Opt {
    content: Content,
    /// Discarded since this content was written: zeros are also a valid
    /// read.
    trimmed: bool,
}

impl Opt {
    fn consistent_with(&self, bytes: &[u8]) -> bool {
        self.content.matches(bytes) || (self.trimmed && bytes.iter().all(|b| *b == 0))
    }
}

fn push_unique(opts: &mut Vec<Opt>, opt: Opt) {
    if !opts.contains(&opt) {
        opts.push(opt);
    }
}

#[derive(Debug, Clone)]
struct Version {
    /// Position in acknowledgement order.
    seq: u64,
    opts: Vec<Opt>,
}

fn initial_version() -> Version {
    Version {
        seq: 0,
        opts: vec![Opt {
            content: Content::Zeros,
            trimmed: false,
        }],
    }
}

/// What the model remembers about one block of an in-flight read.
#[derive(Debug, Clone)]
struct Window {
    opts: Vec<Opt>,
    /// The version the read saw at submission.
    seq: u64,
    /// No write or discard to this block was in flight during the read.
    pure: bool,
}

#[derive(Debug, Clone)]
enum Pending {
    /// Invalid: must be rejected.
    Invalid,
    Read {
        first: u64,
        windows: Vec<Window>,
    },
    /// A write or a write-zeroes.
    Write {
        first: u64,
        contents: Vec<Content>,
    },
    Discard {
        first: u64,
        count: u64,
    },
    /// `seq` is the acknowledgement counter when the flush was submitted.
    Flush {
        seq: u64,
    },
}

/// What an in-flight request may do to a block.
enum Effect<'a> {
    Store(&'a Content),
    Trim,
}

impl Pending {
    fn effect_on(&self, block: u64) -> Option<Effect<'_>> {
        match self {
            Pending::Write { first, contents } => block
                .checked_sub(*first)
                .and_then(|i| contents.get(i as usize))
                .map(Effect::Store),
            Pending::Discard { first, count } => {
                (block >= *first && block - *first < *count).then_some(Effect::Trim)
            }
            _ => None,
        }
    }

    fn written_blocks(&self) -> std::ops::Range<u64> {
        match self {
            Pending::Write { first, contents } => *first..*first + contents.len() as u64,
            Pending::Discard { first, count } => *first..*first + *count,
            _ => 0..0,
        }
    }
}

/// How a block that a read returned differs from everything allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mismatch {
    /// Every byte comes from some allowed content, but no single one: the
    /// block is a mix of two writes.
    Torn,
    /// The block holds bytes that no allowed content has there: stale,
    /// lost or foreign data.
    Unexpected,
}

/// A completion, or a step of the client, that the contract does not allow.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Violation {
    /// A completion for an id that is not in flight: a request completed
    /// twice, or never submitted (rule R8).
    #[error("completion for {id}, which is not in flight")]
    UnknownCompletion {
        /// The id in the completion.
        id: RequestId,
    },
    /// The client reused an id that is in flight. A fault of the caller of
    /// the model, not of the target.
    #[error("{id} submitted while already in flight")]
    DuplicateSubmit {
        /// The id.
        id: RequestId,
    },
    /// An invalid request was not rejected (rule R2).
    #[error("{id} is invalid and must be rejected, but completed with {got:?}")]
    NotRejected {
        /// The id.
        id: RequestId,
        /// What the target answered.
        got: Result<(), BlkError>,
    },
    /// A valid request failed.
    #[error("{id} is valid but failed: {error}")]
    UnexpectedError {
        /// The id.
        id: RequestId,
        /// The error.
        error: BlkError,
    },
    /// A read completed without its buffer, or with one of another length.
    #[error("read {id} returned a buffer of {got:?} bytes, expected {expected}")]
    BadBuffer {
        /// The id.
        id: RequestId,
        /// The length asked for.
        expected: usize,
        /// The length returned, or `None` if no buffer came back.
        got: Option<usize>,
    },
    /// A read returned a block that the contract does not allow (rules R3,
    /// R8, R10).
    #[error(
        "read {id} returned a block {block} that is {kind:?}; {allowed} contents were allowed"
    )]
    BadRead {
        /// The id.
        id: RequestId,
        /// The block number on the device.
        block: u64,
        /// Torn, or simply wrong.
        kind: Mismatch,
        /// How many distinct contents would have been accepted.
        allowed: usize,
    },
}

/// The reference model. See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct Model {
    geometry: Geometry,
    /// Blocks that differ from the initial state. An absent block has one
    /// version: zeros.
    blocks: BTreeMap<u64, Vec<Version>>,
    /// Blocks with more than one version, to keep flushes cheap.
    dirty: BTreeSet<u64>,
    inflight: BTreeMap<RequestId, Pending>,
    /// Counts acknowledgements. A version's `seq` is its value at the ack.
    ack_seq: u64,
    /// The highest `seq` covered by a completed flush.
    durable_seq: u64,
}

impl Model {
    /// A model of a fresh device: every block reads as zeros.
    pub fn new(geometry: Geometry) -> Self {
        Self {
            geometry,
            blocks: BTreeMap::new(),
            dirty: BTreeSet::new(),
            inflight: BTreeMap::new(),
            ack_seq: 0,
            durable_seq: 0,
        }
    }

    /// The geometry the model was built for.
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// How many requests are in flight.
    pub fn in_flight(&self) -> usize {
        self.inflight.len()
    }

    fn latest(&self, block: u64) -> Version {
        self.blocks
            .get(&block)
            .and_then(|v| v.last())
            .cloned()
            .unwrap_or_else(initial_version)
    }

    fn versions_mut(&mut self, block: u64) -> &mut Vec<Version> {
        self.blocks
            .entry(block)
            .or_insert_with(|| vec![initial_version()])
    }

    /// The client submits `request`. Call this before handing the request
    /// to the target. A request reissued after [`Model::restart`] must not
    /// be submitted to the model again.
    pub fn submit(&mut self, request: &Request) -> Result<(), Violation> {
        let id = request.id;
        if self.inflight.contains_key(&id) {
            return Err(Violation::DuplicateSubmit { id });
        }
        let pending = if self.geometry.validate(&request.op).is_err() {
            Pending::Invalid
        } else {
            match &request.op {
                Op::Flush => Pending::Flush { seq: self.ack_seq },
                Op::Read { offset, buf } => {
                    let first = offset / BLOCK_SIZE;
                    let count = buf.len() as u64 / BLOCK_SIZE;
                    let windows = (first..first + count)
                        .map(|block| self.open_window(block))
                        .collect();
                    Pending::Read { first, windows }
                }
                Op::Write { offset, data } => Pending::Write {
                    first: offset / BLOCK_SIZE,
                    contents: data.chunks(BLOCK_USIZE).map(Content::of).collect(),
                },
                Op::WriteZeroes { offset, len } => Pending::Write {
                    first: offset / BLOCK_SIZE,
                    contents: vec![Content::Zeros; (len / BLOCK_SIZE) as usize],
                },
                Op::Discard { offset, len } => Pending::Discard {
                    first: offset / BLOCK_SIZE,
                    count: len / BLOCK_SIZE,
                },
            }
        };
        self.widen_reads(&pending);
        self.inflight.insert(id, pending);
        Ok(())
    }

    /// What a read submitted now may see in `block`.
    fn open_window(&self, block: u64) -> Window {
        let latest = self.latest(block);
        let mut window = Window {
            opts: latest.opts,
            seq: latest.seq,
            pure: true,
        };
        for other in self.inflight.values() {
            if let Some(effect) = other.effect_on(block) {
                widen(&mut window, effect);
            }
        }
        window
    }

    /// A write or discard was submitted: reads in flight may see it.
    fn widen_reads(&mut self, new: &Pending) {
        let range = new.written_blocks();
        if range.is_empty() {
            return;
        }
        for other in self.inflight.values_mut() {
            let Pending::Read { first, windows } = other else {
                continue;
            };
            for (i, window) in windows.iter_mut().enumerate() {
                if let Some(effect) = new.effect_on(*first + i as u64) {
                    widen(window, effect);
                }
            }
        }
    }

    /// The target delivered `completion`. Completions must be passed in the
    /// order the target returned them. An error means the target broke the
    /// contract; the model is not meaningful afterwards.
    pub fn complete(&mut self, completion: &Completion) -> Result<(), Violation> {
        let id = completion.id;
        let pending = self
            .inflight
            .remove(&id)
            .ok_or(Violation::UnknownCompletion { id })?;
        if let Pending::Invalid = pending {
            return match completion.result {
                Err(e) if e.is_rejection() => Ok(()),
                got => Err(Violation::NotRejected { id, got }),
            };
        }
        if let Err(error) = completion.result {
            return Err(Violation::UnexpectedError { id, error });
        }
        match pending {
            Pending::Invalid => {}
            Pending::Flush { seq } => {
                self.durable_seq = self.durable_seq.max(seq);
                self.prune();
            }
            Pending::Write { first, contents } => {
                self.ack_seq += 1;
                let seq = self.ack_seq;
                for (i, content) in contents.into_iter().enumerate() {
                    let block = first + i as u64;
                    self.versions_mut(block).push(Version {
                        seq,
                        opts: vec![Opt {
                            content,
                            trimmed: false,
                        }],
                    });
                    self.dirty.insert(block);
                }
            }
            Pending::Discard { first, count } => {
                self.ack_seq += 1;
                let seq = self.ack_seq;
                for block in first..first + count {
                    let mut opts = self.latest(block).opts;
                    for opt in &mut opts {
                        opt.trimmed = true;
                    }
                    opts.dedup();
                    self.versions_mut(block).push(Version { seq, opts });
                    self.dirty.insert(block);
                }
            }
            Pending::Read { first, windows } => {
                let expected = windows.len() * BLOCK_USIZE;
                let buf = match &completion.buf {
                    Some(buf) if buf.len() == expected => buf,
                    other => {
                        return Err(Violation::BadBuffer {
                            id,
                            expected,
                            got: other.as_ref().map(Vec::len),
                        })
                    }
                };
                for (i, (window, bytes)) in windows.iter().zip(buf.chunks(BLOCK_USIZE)).enumerate()
                {
                    let block = first + i as u64;
                    if !window.opts.iter().any(|o| o.consistent_with(bytes)) {
                        return Err(Violation::BadRead {
                            id,
                            block,
                            kind: classify(&window.opts, bytes),
                            allowed: window.opts.len(),
                        });
                    }
                    if window.pure {
                        self.narrow(block, window.seq, bytes);
                    }
                }
            }
        }
        Ok(())
    }

    /// A read that raced nothing saw `bytes`: keep only the options that
    /// explain it.
    fn narrow(&mut self, block: u64, seq: u64, bytes: &[u8]) {
        let Some(latest) = self.blocks.get_mut(&block).and_then(|v| v.last_mut()) else {
            return;
        };
        if latest.seq == seq && latest.opts.len() > 1 {
            latest.opts.retain(|o| o.consistent_with(bytes));
        }
    }

    /// Drop the versions that a completed flush has made unreachable.
    fn prune(&mut self) {
        let durable = self.durable_seq;
        let blocks = &mut self.blocks;
        self.dirty.retain(|block| {
            let Some(versions) = blocks.get_mut(block) else {
                return false;
            };
            if let Some(floor) = versions.iter().rposition(|v| v.seq <= durable) {
                versions.drain(..floor);
            }
            versions.len() > 1
        });
    }

    /// The daemon was lost and has returned (rule R9). Requests in flight
    /// stay in flight: the transport reissues them under the same ids and
    /// their completions are checked as usual. Nothing acknowledged is
    /// lost, so the model's state does not change.
    pub fn restart(&mut self) {}

    /// Power was cut and the device has returned (rule R10). Requests in
    /// flight are gone and will not complete. Every block now holds one of
    /// its versions, or the data of a write that was in flight; the model
    /// learns which from the reads that follow.
    pub fn power_cut(&mut self) {
        let inflight = std::mem::take(&mut self.inflight);
        let mut touched: BTreeSet<u64> = std::mem::take(&mut self.dirty);
        for pending in inflight.values() {
            touched.extend(pending.written_blocks());
        }
        self.ack_seq += 1;
        let seq = self.ack_seq;
        for block in touched {
            let mut opts: Vec<Opt> = Vec::new();
            if let Some(versions) = self.blocks.get(&block) {
                for opt in versions.iter().flat_map(|v| v.opts.iter()) {
                    push_unique(&mut opts, opt.clone());
                }
            } else {
                opts = initial_version().opts;
            }
            let mut trim = false;
            for pending in inflight.values() {
                match pending.effect_on(block) {
                    Some(Effect::Store(content)) => push_unique(
                        &mut opts,
                        Opt {
                            content: content.clone(),
                            trimmed: false,
                        },
                    ),
                    Some(Effect::Trim) => trim = true,
                    None => {}
                }
            }
            if trim {
                // An in-flight discard may have landed after any of them.
                for opt in opts.clone() {
                    push_unique(
                        &mut opts,
                        Opt {
                            trimmed: true,
                            ..opt
                        },
                    );
                }
            }
            self.blocks.insert(block, vec![Version { seq, opts }]);
        }
    }
}

fn widen(window: &mut Window, effect: Effect<'_>) {
    window.pure = false;
    let content = match effect {
        Effect::Store(content) => content.clone(),
        Effect::Trim => Content::Zeros,
    };
    push_unique(
        &mut window.opts,
        Opt {
            content,
            trimmed: false,
        },
    );
}

fn classify(opts: &[Opt], bytes: &[u8]) -> Mismatch {
    let torn = bytes.iter().enumerate().all(|(i, b)| {
        opts.iter()
            .any(|o| o.content.byte(i) == *b || (o.trimmed && *b == 0))
    });
    if torn {
        Mismatch::Torn
    } else {
        Mismatch::Unexpected
    }
}

#[cfg(test)]
mod tests {
    //! The model is the yardstick of the suite, so its own judgement is
    //! pinned here with hand-written histories: for each rule, one history
    //! it must accept and one it must reject.

    use super::*;

    const B: usize = BLOCK_USIZE;

    struct Client {
        model: Model,
        next: u64,
    }

    impl Client {
        fn new() -> Self {
            Self {
                model: Model::new(Geometry::new(8, 4, 8).unwrap()),
                next: 0,
            }
        }

        fn submit(&mut self, op: Op) -> RequestId {
            self.next += 1;
            let id = RequestId(self.next);
            self.model.submit(&Request { id, op }).unwrap();
            id
        }

        fn ack(&mut self, id: RequestId) -> Result<(), Violation> {
            self.model.complete(&Completion {
                id,
                result: Ok(()),
                buf: None,
            })
        }

        fn write(&mut self, block: u64, fill: u8) -> RequestId {
            self.submit(Op::Write {
                offset: block * BLOCK_SIZE,
                data: vec![fill; B],
            })
        }

        fn read(&mut self, block: u64) -> RequestId {
            self.submit(Op::Read {
                offset: block * BLOCK_SIZE,
                buf: vec![0; B],
            })
        }

        fn returns(&mut self, id: RequestId, bytes: Vec<u8>) -> Result<(), Violation> {
            self.model.complete(&Completion {
                id,
                result: Ok(()),
                buf: Some(bytes),
            })
        }

        /// Read `block` with nothing else in flight; the target answers
        /// a block full of `fill`.
        fn sees(&mut self, block: u64, fill: u8) -> Result<(), Violation> {
            let id = self.read(block);
            self.returns(id, vec![fill; B])
        }

        fn flush(&mut self) {
            let id = self.submit(Op::Flush);
            self.ack(id).unwrap();
        }
    }

    /// R3: a block never written reads as zeros and as nothing else.
    #[test]
    fn fresh_block_reads_zeros_only() {
        let mut c = Client::new();
        c.sees(0, 0).unwrap();
        assert!(matches!(c.sees(0, 7), Err(Violation::BadRead { .. })));
    }

    /// R3, R4: a read returns the latest acknowledged write, not an older
    /// one.
    #[test]
    fn read_sees_latest_acknowledged_write() {
        let mut c = Client::new();
        let w = c.write(1, 1);
        c.ack(w).unwrap();
        let w = c.write(1, 2);
        c.ack(w).unwrap();
        c.sees(1, 2).unwrap();
        assert!(matches!(
            c.sees(1, 1),
            Err(Violation::BadRead {
                block: 1,
                kind: Mismatch::Unexpected,
                ..
            })
        ));
    }

    /// R8: two writes in flight to one block; the one acknowledged last
    /// wins, whatever the order of submission.
    #[test]
    fn overlapping_writes_resolve_in_ack_order() {
        let mut c = Client::new();
        let first = c.write(0, 1);
        let second = c.write(0, 2);
        c.ack(second).unwrap();
        c.ack(first).unwrap();
        c.sees(0, 1).unwrap();
        assert!(c.sees(0, 2).is_err());
    }

    /// Open reading 9: a read in flight with a write may return the old or
    /// the new block, but not a mix. The mix is reported as torn.
    #[test]
    fn read_racing_a_write_sees_old_or_new_whole() {
        for answer in [1u8, 2] {
            let mut c = Client::new();
            let w = c.write(0, 1);
            c.ack(w).unwrap();
            let r = c.read(0);
            let _w2 = c.write(0, 2);
            c.returns(r, vec![answer; B]).unwrap();
        }
        let mut c = Client::new();
        let w = c.write(0, 1);
        c.ack(w).unwrap();
        let r = c.read(0);
        let _w2 = c.write(0, 2);
        let mut mixed = vec![1u8; B];
        mixed[B / 2..].fill(2);
        assert!(matches!(
            c.returns(r, mixed),
            Err(Violation::BadRead {
                kind: Mismatch::Torn,
                ..
            })
        ));
    }

    /// R2: an invalid request must complete with a rejection; `Ok` and an
    /// I/O error are both violations.
    #[test]
    fn invalid_request_must_be_rejected() {
        let op = Op::Discard {
            offset: 1,
            len: BLOCK_SIZE,
        };
        let rejection = BlkError::Misaligned {
            offset: 1,
            len: BLOCK_SIZE,
        };
        let mut c = Client::new();
        let id = c.submit(op.clone());
        c.model
            .complete(&Completion {
                id,
                result: Err(rejection),
                buf: None,
            })
            .unwrap();
        let id = c.submit(op.clone());
        assert!(matches!(c.ack(id), Err(Violation::NotRejected { .. })));
        let id = c.submit(op);
        let io = Completion {
            id,
            result: Err(BlkError::Io(std::io::ErrorKind::Other)),
            buf: None,
        };
        assert!(matches!(
            c.model.complete(&io),
            Err(Violation::NotRejected { .. })
        ));
    }

    /// R8: every request completes exactly once; a second completion for
    /// the same id is a violation.
    #[test]
    fn double_completion_is_a_violation() {
        let mut c = Client::new();
        let w = c.write(0, 1);
        c.ack(w).unwrap();
        assert_eq!(c.ack(w), Err(Violation::UnknownCompletion { id: w }));
    }

    /// R5, R10: a write acknowledged before a completed flush was
    /// submitted survives a power cut; the older content is not accepted.
    #[test]
    fn flushed_write_survives_power_cut() {
        let mut c = Client::new();
        let w = c.write(0, 1);
        c.ack(w).unwrap();
        c.flush();
        c.model.power_cut();
        assert!(c.clone_sees(0, 0).is_err());
        c.sees(0, 1).unwrap();
    }

    /// R4, R10: an unflushed write may or may not survive a power cut, and
    /// a block that mixes old and new is torn.
    #[test]
    fn unflushed_write_survives_or_not_but_is_never_torn() {
        let mut c = Client::new();
        let w = c.write(0, 1);
        c.ack(w).unwrap();
        c.flush();
        let w = c.write(0, 2);
        c.ack(w).unwrap();
        c.model.power_cut();
        c.clone_sees(0, 1).unwrap();
        c.clone_sees(0, 2).unwrap();
        let r = c.read(0);
        let mut mixed = vec![1u8; B];
        mixed[..B / 2].fill(2);
        assert!(matches!(
            c.returns(r, mixed),
            Err(Violation::BadRead {
                kind: Mismatch::Torn,
                ..
            })
        ));
    }

    /// R5 (open reading 4): a write still in flight when the flush is
    /// submitted is not covered by it, even if it is acknowledged before
    /// the flush completes.
    #[test]
    fn flush_does_not_cover_a_write_in_flight() {
        let mut c = Client::new();
        let w = c.write(0, 1);
        let f = c.submit(Op::Flush);
        c.ack(w).unwrap();
        c.ack(f).unwrap();
        c.model.power_cut();
        c.clone_sees(0, 0).unwrap();
        c.clone_sees(0, 1).unwrap();
    }

    /// R10: a write in flight at the power cut may have landed.
    #[test]
    fn in_flight_write_may_land_at_power_cut() {
        let mut c = Client::new();
        let _w = c.write(0, 1);
        c.model.power_cut();
        assert_eq!(c.model.in_flight(), 0);
        c.clone_sees(0, 0).unwrap();
        c.clone_sees(0, 1).unwrap();
        assert!(c.sees(0, 2).is_err());
    }

    /// R10 (open reading 8): what is read after a power cut is stable, and
    /// survives a second power cut without a flush.
    #[test]
    fn recovered_state_is_stable_and_durable() {
        let mut c = Client::new();
        let w = c.write(0, 1);
        c.ack(w).unwrap();
        c.model.power_cut();
        c.sees(0, 1).unwrap();
        assert!(c.clone_sees(0, 0).is_err());
        c.model.power_cut();
        assert!(c.clone_sees(0, 0).is_err());
        c.sees(0, 1).unwrap();
    }

    /// R3, R6 (open reading 1): after a discard a block reads as its last
    /// write or as zeros, in any alternation, and as nothing else.
    #[test]
    fn discarded_block_reads_old_or_zeros() {
        let mut c = Client::new();
        let w = c.write(0, 1);
        c.ack(w).unwrap();
        let d = c.submit(Op::Discard {
            offset: 0,
            len: BLOCK_SIZE,
        });
        c.ack(d).unwrap();
        c.sees(0, 0).unwrap();
        c.sees(0, 1).unwrap();
        c.sees(0, 0).unwrap();
        assert!(c.sees(0, 9).is_err());
    }

    /// R6, R8: a write acknowledged after a discard ends the discard's
    /// effect: zeros are no longer accepted.
    #[test]
    fn write_after_discard_wins() {
        let mut c = Client::new();
        let d = c.submit(Op::Discard {
            offset: 0,
            len: BLOCK_SIZE,
        });
        c.ack(d).unwrap();
        let w = c.write(0, 1);
        c.ack(w).unwrap();
        assert!(c.clone_sees(0, 0).is_err());
        c.sees(0, 1).unwrap();
    }

    /// R7: write-zeroes reads as zeros, and with a flush survives a power
    /// cut like a write.
    #[test]
    fn write_zeroes_is_a_durable_write_of_zeros() {
        let mut c = Client::new();
        let w = c.write(0, 1);
        c.ack(w).unwrap();
        c.flush();
        let z = c.submit(Op::WriteZeroes {
            offset: 0,
            len: BLOCK_SIZE,
        });
        c.ack(z).unwrap();
        assert!(c.clone_sees(0, 1).is_err());
        c.flush();
        c.model.power_cut();
        assert!(c.clone_sees(0, 1).is_err());
        c.sees(0, 0).unwrap();
    }

    /// R9: a daemon restart loses nothing acknowledged and keeps requests
    /// in flight, to be completed after the reissue.
    #[test]
    fn restart_keeps_acknowledged_writes_and_in_flight_requests() {
        let mut c = Client::new();
        let w = c.write(0, 1);
        c.ack(w).unwrap();
        let w2 = c.write(1, 2);
        c.model.restart();
        assert_eq!(c.model.in_flight(), 1);
        assert!(c.clone_sees(0, 0).is_err());
        c.ack(w2).unwrap();
        c.sees(0, 1).unwrap();
        c.sees(1, 2).unwrap();
    }

    /// A read must hand back a buffer of the length asked for.
    #[test]
    fn read_without_buffer_is_a_violation() {
        let mut c = Client::new();
        let r = c.read(0);
        assert!(matches!(c.ack(r), Err(Violation::BadBuffer { .. })));
    }

    impl Client {
        /// As `sees`, on a copy: asks "would this answer be accepted?"
        /// without committing the model to it.
        fn clone_sees(&self, block: u64, fill: u8) -> Result<(), Violation> {
            let mut copy = Client {
                model: self.model.clone(),
                next: self.next,
            };
            copy.sees(block, fill)
        }
    }
}
