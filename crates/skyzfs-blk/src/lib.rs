//! The SkyZFS block target contract (design §4.1), made executable.
//!
//! Every transport (ublk, the shared-memory frontend, NBD) delivers the same
//! five operations to the daemon and must honour the same rules. This crate
//! holds:
//!
//! - the contract as types: [`Geometry`], [`Op`], [`Request`],
//!   [`Completion`], [`BlkError`] and the [`BlockTarget`] trait,
//! - [`model::Model`], a reference model of what a client may observe,
//!   including what a power cut may leave behind,
//! - [`mem::MemTarget`], an in-memory implementation with crash and restart
//!   hooks,
//! - `conformance` (feature `conformance`), a suite that runs any
//!   implementation against the model.
//!
//! # The rules
//!
//! Rules R1 to R10 restate §4.1. Where §4.1 is silent or ambiguous the rule
//! is marked **(reading)**: it is this crate's conservative reading, not a
//! design decision, and is listed again under [Open readings](#open-readings).
//!
//! - **R1 Geometry.** Offsets and lengths are multiples of [`BLOCK_SIZE`]
//!   (4 KiB). The device size is a non-zero multiple of it.
//! - **R2 Rejection.** A request that is misaligned, of zero length
//!   **(reading)**, beyond the end of the device, or a read or write longer
//!   than [`Geometry::max_io_bytes`] **(reading)** completes with an error
//!   and has no effect. [`Geometry::validate`] is the one definition of
//!   "valid" and every implementation should call it.
//! - **R3 Read.** A read returns, for every block, the latest acknowledged
//!   write, or zeros if the block was never written. A block that has been
//!   discarded since its last write reads as either that last write or
//!   zeros, and the choice may differ from one read to the next
//!   **(reading)**.
//! - **R4 Write.** A write is acknowledged once it is visible to reads. It
//!   may be lost on a power cut until a later flush completes.
//! - **R5 Flush.** A flush completes only when every write acknowledged
//!   before the flush was submitted is durable. A write still in flight when
//!   the flush is submitted is not covered **(reading)**.
//! - **R6 Discard.** Advisory. It changes nothing a client may rely on
//!   except that the block may afterwards read as zeros (R3).
//! - **R7 Write zeroes.** A write of zeros, with the durability of a write.
//! - **R8 Order.** Writes to one block are applied in acknowledgement order:
//!   the block holds the data of the write whose completion was delivered
//!   last. Requests that do not overlap may complete in any order. Every
//!   accepted request completes exactly once.
//! - **R9 Daemon loss.** The daemon may disappear and return while its
//!   client lives on. The transport then reissues every request that was in
//!   flight, with the same id, and the result must be as if each had been
//!   issued once. Writes acknowledged before the loss are still there
//!   **(reading)**: the client is alive, will not resend them, and a later
//!   flush promises them durable.
//! - **R10 Power cut.** After a power cut every block holds exactly one of:
//!   the latest write to it that a completed flush covers, or any one later
//!   write to it, acknowledged or in flight. No block is ever a mix of two
//!   writes. Blocks of one multi-block write survive or vanish
//!   independently **(reading)**. What a client reads after the device
//!   returns is stable and is itself durable **(reading)**.
//!
//! Not expressible here, and so not checked by the suite: that the daemon
//! locks its memory and keeps its files off the pools it serves, that a
//! flush never waits for upload, and that a discard is *recorded* for the
//! uploader and the cache.
//!
//! # The shape of the API, and why
//!
//! [`BlockTarget`] is a submission and completion queue:
//!
//! ```text
//! submit(Request) -> Result<(), SubmitError>     never blocks
//! poll(&mut Vec<Completion>) -> usize            never blocks
//! wait(&mut Vec<Completion>, timeout) -> usize   blocks for the first one
//! ```
//!
//! - **Ids are the caller's.** A transport already has a tag for every
//!   request (ublk's queue and tag, the request id in a ring descriptor).
//!   The id comes back in the [`Completion`], so many requests can be in
//!   flight and complete in any order. It is also what makes a reissue
//!   after daemon loss recognisable to the caller.
//! - **Buffers are owned and make a round trip.** A read carries the buffer
//!   to fill, a write carries its data, and the completion hands the buffer
//!   back. A transport can recycle a fixed pool of buffers and never
//!   allocate on the I/O path, which the design asks of the daemon (§10).
//! - **Every accepted request completes, including a rejected one.** An
//!   invalid request is reported in its completion, not by `submit`, so a
//!   transport has one place where results turn into its own status codes.
//!   `submit` fails only for what the transport itself got wrong: more than
//!   [`Geometry::queue_depth`] requests in flight, or an id used twice.
//! - **`&mut self`, one owner per queue.** An io_uring loop or a ring loop
//!   is a single thread that owns its queue. A target with several queues
//!   hands out one handle per queue.
//! - **The same trait fits a store behind a transport.** A transport loop
//!   forwards requests to a `BlockTarget` store and its completions back;
//!   nothing in the trait is specific to the kernel side.
//!
//! Alternatives rejected:
//!
//! - *Blocking calls* (`fn read(&self, ..) -> Result<..>`). One thread per
//!   request in flight, and no out-of-order completion without a pool.
//! - *`async fn` or futures.* The transport loop is already the reactor.
//!   Futures would bring a runtime into the daemon's I/O path, and boxed
//!   futures allocate per request. An async wrapper can be layered on this
//!   trait; the reverse is not true.
//! - *A callback per request.* Closures allocate, and a completion that
//!   calls back into the submitter re-enters the store.
//! - *Borrowed buffers.* A borrow cannot outlive `submit` and still be held
//!   until a later `poll` without `unsafe` or a scoped API that forces
//!   completion in order.
//! - *A buffer type parameter* (for arena or zero-copy buffers). Design
//!   §4.2 starts with one copy per request, which `Vec<u8>` serves. Left
//!   for the day a zero-copy mode is adopted (gate G1).
//! - *Ids assigned by the target.* The caller would need a map from its tag
//!   to the target's ticket, and could not reissue under the same id.
//! - *A completion file descriptor or waker.* A real transport will want
//!   to sleep on its own event source and be woken by the store.
//!   [`BlockTarget::wait`] is enough for the suite and for a simple loop;
//!   a wake-up hook can be added without changing the rest.
//!
//! # Open readings
//!
//! These are the places where this crate had to choose. Each is the
//! conservative choice and each needs a decision in the design.
//!
//! 1. **Read after discard** (R3, R6). §4.1 says a read returns "the latest
//!    acknowledged write for every byte, or zeros" and that discard is
//!    advisory. Reading: per block, either, not necessarily the same on
//!    every read. The suite rejects anything else.
//! 2. **Largest request** (R2). §4.1 gives none. Reading: the target states
//!    one for reads and writes in its geometry and rejects longer ones.
//!    Discards and write-zeroes carry no data and may span the device.
//! 3. **Zero-length requests** (R2). Rejected.
//! 4. **Flush with writes in flight** (R5). Reading, by the letter of
//!    §4.1: only writes whose completion the client had before it submitted
//!    the flush are covered.
//! 5. **Daemon loss and acknowledged writes** (R9). §4.1 acknowledges a
//!    write "once in the daemon's memory" and says it "may be lost on a
//!    crash". Read literally, `kill -9` of the daemon loses writes that the
//!    still-running ZFS believes written, and a later flush would then
//!    succeed over a hole. Reading: only a power cut may lose acknowledged
//!    writes; a daemon restart may not. This constrains the local store
//!    (an acknowledged write must already be somewhere that outlives the
//!    process) and should be settled before `skyzfs-local` is designed.
//! 6. **Atomicity of a multi-block write across a power cut** (R10). §4.1
//!    promises only that no 4 KiB block is torn. Reading: nothing more.
//! 7. **Order between unflushed writes across a power cut** (R10). A later
//!    unflushed write may survive while an earlier one, to another block,
//!    does not. §4.1 does not promise a prefix.
//! 8. **Recovered state is durable** (R10). What reads return after a power
//!    cut does not change at a second power cut, even without a flush in
//!    between. ZFS relies on this at import.
//! 9. **Reads concurrent with writes** (R3). A read in flight together with
//!    a write to the same block may return either the old or the new
//!    block, whole.
//! 10. **Flags.** FUA, a no-unmap flag on write-zeroes, priorities and the
//!     transaction-group hint of §4.3 are not in §4.1 and are not modelled.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::time::Duration;

#[cfg(feature = "conformance")]
pub mod conformance;
pub mod mem;
pub mod model;

/// The block size of the contract: 4 KiB (rule R1). Pools are created with
/// `ashift=12`.
pub const BLOCK_SIZE: u64 = 4096;

/// `BLOCK_SIZE` as a `usize`, for slicing buffers.
pub const BLOCK_USIZE: usize = BLOCK_SIZE as usize;

/// What a target tells its client about itself (rule R1, R2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    size_bytes: u64,
    max_io_bytes: u64,
    queue_depth: u32,
}

/// Why a [`Geometry`] could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GeometryError {
    /// The device has no blocks, or its size overflows a `u64` in bytes.
    #[error("device size must be between one block and u64::MAX bytes")]
    BadSize,
    /// The largest read or write is not at least one block.
    #[error("the largest request must be at least one block")]
    BadMaxIo,
    /// No request could ever be in flight.
    #[error("queue depth must be at least one")]
    BadQueueDepth,
}

impl Geometry {
    /// A device of `blocks` blocks that takes reads and writes of up to
    /// `max_io_blocks` blocks and up to `queue_depth` requests in flight.
    pub fn new(blocks: u64, max_io_blocks: u64, queue_depth: u32) -> Result<Self, GeometryError> {
        let size_bytes = blocks
            .checked_mul(BLOCK_SIZE)
            .filter(|s| *s > 0)
            .ok_or(GeometryError::BadSize)?;
        let max_io_bytes = max_io_blocks
            .checked_mul(BLOCK_SIZE)
            .filter(|s| *s > 0)
            .ok_or(GeometryError::BadMaxIo)?;
        if queue_depth == 0 {
            return Err(GeometryError::BadQueueDepth);
        }
        Ok(Self {
            size_bytes,
            max_io_bytes,
            queue_depth,
        })
    }

    /// Device size in bytes. A non-zero multiple of [`BLOCK_SIZE`].
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Device size in blocks.
    pub fn blocks(&self) -> u64 {
        self.size_bytes / BLOCK_SIZE
    }

    /// The longest read or write the target accepts, in bytes. A non-zero
    /// multiple of [`BLOCK_SIZE`]. Discards and write-zeroes are bounded
    /// only by the device size (open reading 2).
    pub fn max_io_bytes(&self) -> u64 {
        self.max_io_bytes
    }

    /// How many requests the target accepts in flight at once. A request is
    /// in flight from `submit` until its completion is returned by `poll`
    /// or `wait`.
    pub fn queue_depth(&self) -> u32 {
        self.queue_depth
    }

    /// The one definition of a valid request (rule R2).
    ///
    /// Checks, in this order: zero length, alignment, the size limit for
    /// reads and writes, and the end of the device. A flush is always
    /// valid.
    pub fn validate(&self, op: &Op) -> Result<(), BlkError> {
        let Some((offset, len)) = op.range() else {
            return Ok(());
        };
        if len == 0 {
            return Err(BlkError::ZeroLength);
        }
        if offset % BLOCK_SIZE != 0 || len % BLOCK_SIZE != 0 {
            return Err(BlkError::Misaligned { offset, len });
        }
        if op.carries_data() && len > self.max_io_bytes {
            return Err(BlkError::TooLarge {
                len,
                max: self.max_io_bytes,
            });
        }
        match offset.checked_add(len) {
            Some(end) if end <= self.size_bytes => Ok(()),
            _ => Err(BlkError::OutOfBounds {
                offset,
                len,
                size: self.size_bytes,
            }),
        }
    }
}

/// The caller's name for a request in flight. It comes back in the
/// [`Completion`] and is reused when the request is reissued after a daemon
/// loss (rule R9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// The five operations of the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Fill `buf` from the device, starting at `offset`. The length of the
    /// read is the length of the buffer.
    Read {
        /// Byte offset on the device.
        offset: u64,
        /// The buffer to fill. Returned in the completion.
        buf: Vec<u8>,
    },
    /// Write `data` at `offset`.
    Write {
        /// Byte offset on the device.
        offset: u64,
        /// The data. Returned in the completion for reuse.
        data: Vec<u8>,
    },
    /// Make every write acknowledged so far durable (rule R5).
    Flush,
    /// The blocks are no longer needed (rule R6).
    Discard {
        /// Byte offset on the device.
        offset: u64,
        /// Length in bytes.
        len: u64,
    },
    /// Write zeros (rule R7).
    WriteZeroes {
        /// Byte offset on the device.
        offset: u64,
        /// Length in bytes.
        len: u64,
    },
}

impl Op {
    /// The byte range the operation covers, as `(offset, len)`. `None` for
    /// a flush.
    pub fn range(&self) -> Option<(u64, u64)> {
        match self {
            Op::Read { offset, buf } => Some((*offset, buf.len() as u64)),
            Op::Write { offset, data } => Some((*offset, data.len() as u64)),
            Op::Flush => None,
            Op::Discard { offset, len } | Op::WriteZeroes { offset, len } => Some((*offset, *len)),
        }
    }

    /// Whether the operation carries a data buffer, and so is bounded by
    /// [`Geometry::max_io_bytes`].
    pub fn carries_data(&self) -> bool {
        matches!(self, Op::Read { .. } | Op::Write { .. })
    }

    /// A short name for messages.
    pub fn name(&self) -> &'static str {
        match self {
            Op::Read { .. } => "read",
            Op::Write { .. } => "write",
            Op::Flush => "flush",
            Op::Discard { .. } => "discard",
            Op::WriteZeroes { .. } => "write_zeroes",
        }
    }
}

/// One request: an operation and the caller's id for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The caller's id. Unique among the requests in flight.
    pub id: RequestId,
    /// What to do.
    pub op: Op,
}

/// The result of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The id given in the [`Request`].
    pub id: RequestId,
    /// `Ok` once the operation has taken effect as the contract describes.
    pub result: Result<(), BlkError>,
    /// The buffer of a read (filled, if `result` is `Ok`) or of a write.
    /// `None` for the other operations.
    pub buf: Option<Vec<u8>>,
}

/// Why a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BlkError {
    /// Offset or length is not a multiple of [`BLOCK_SIZE`].
    #[error("offset {offset} or length {len} is not a multiple of 4096")]
    Misaligned {
        /// The offset asked for.
        offset: u64,
        /// The length asked for.
        len: u64,
    },
    /// The request covers no bytes.
    #[error("zero-length request")]
    ZeroLength,
    /// The request reaches past the end of the device.
    #[error("range {offset}+{len} is beyond the device size {size}")]
    OutOfBounds {
        /// The offset asked for.
        offset: u64,
        /// The length asked for.
        len: u64,
        /// The device size in bytes.
        size: u64,
    },
    /// A read or write longer than [`Geometry::max_io_bytes`].
    #[error("length {len} exceeds the largest request {max}")]
    TooLarge {
        /// The length asked for.
        len: u64,
        /// The limit.
        max: u64,
    },
    /// The target could not carry out a valid request.
    #[error("I/O error: {0}")]
    Io(std::io::ErrorKind),
}

impl BlkError {
    /// Whether this is a rejection of an invalid request (rule R2), as
    /// opposed to a failure of a valid one.
    pub fn is_rejection(&self) -> bool {
        !matches!(self, BlkError::Io(_))
    }
}

/// Why `submit` did not accept a request. The request is handed back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("request {} not accepted: {kind}", request.id)]
pub struct SubmitError {
    /// What went wrong.
    pub kind: SubmitErrorKind,
    /// The request, unchanged.
    pub request: Request,
}

/// The two ways `submit` can refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SubmitErrorKind {
    /// [`Geometry::queue_depth`] requests are already in flight. Reap
    /// completions and submit again.
    #[error("queue full")]
    Busy,
    /// A request with this id is already in flight.
    #[error("id already in flight")]
    DuplicateId,
}

/// A block target: anything that takes the five operations and honours
/// rules R1 to R10. See the [crate documentation](crate) for the rules and
/// for why the trait has this shape.
pub trait BlockTarget {
    /// The target's geometry. Constant for the life of the device,
    /// including across restarts and power cuts.
    fn geometry(&self) -> Geometry;

    /// Hand a request to the target. Never blocks.
    ///
    /// An accepted request completes exactly once, through [`poll`] or
    /// [`wait`], whatever its outcome. The target must accept at least
    /// [`Geometry::queue_depth`] requests in flight.
    ///
    /// [`poll`]: BlockTarget::poll
    /// [`wait`]: BlockTarget::wait
    fn submit(&mut self, request: Request) -> Result<(), SubmitError>;

    /// Append the completions that are ready to `out` and return how many
    /// were appended. Never blocks.
    ///
    /// The order of completions in `out`, within one call and across
    /// calls, is the acknowledgement order of rule R8.
    fn poll(&mut self, out: &mut Vec<Completion>) -> usize;

    /// As [`poll`](BlockTarget::poll), but if nothing is ready and requests
    /// are in flight, block until at least one completes or `timeout`
    /// passes. Returns 0 at once if nothing is in flight.
    fn wait(&mut self, out: &mut Vec<Completion>, timeout: Duration) -> usize;
}
