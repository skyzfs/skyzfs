//! The SkyZFS block target contract (design §4.1), made executable.
//!
//! Every transport (ublk, the shared-memory frontend, NBD) delivers the same
//! five operations to the daemon and must honour the same rules. This crate
//! holds:
//!
//! - the contract as types: [`Geometry`], [`Op`], [`Request`],
//!   [`Completion`], [`BlkError`] and the [`BlockTarget`] trait,
//! - [`model::Model`], a reference model of what a client may observe,
//!   including what a daemon loss and a power cut may leave behind,
//! - [`mem::MemTarget`], an in-memory implementation with a simulated
//!   write log and hooks for daemon loss, power cuts and log failure,
//! - `conformance` (feature `conformance`), a suite that runs any
//!   implementation against the model.
//!
//! # The rules
//!
//! Rules R1 to R12 restate §4.1 in the order of that section. Tests name
//! the rule they verify by these numbers.
//!
//! - **R1 Parameters.** Reported at open and fixed while open: a size that
//!   is a multiple of 1 MiB, 4096-byte blocks, a largest read or write of
//!   1 MiB, discard and write-zeroes over any aligned range, a volatile
//!   write cache, no forced-unit-access, and a read-only flag.
//! - **R2 Rejection.** A request that is misaligned, empty or beyond the
//!   end is rejected and has no effect. So is a read or write longer than
//!   1 MiB: a transport splits before the target sees it.
//!   [`Geometry::validate`] is the one definition of "valid".
//! - **R3 Content and read.** A volume has a [`Base`]: empty, which reads
//!   as zeros, or a commit. A block's content is what the last operation
//!   covering it in the daemon's order left there, or else the base. A
//!   read received after an operation was acknowledged observes it. A read
//!   that overlaps an operation in flight sees, for each block, the
//!   content before it or after it.
//! - **R4 Write.** Replaces the content.
//! - **R5 Write zeroes.** A write of zeros. On a volume with a base it
//!   hides the base.
//! - **R6 Discard.** Each block becomes unspecified until the next write
//!   to it: the content just before the discard, zeros or the base,
//!   possibly differing between reads, never anything else. A discard is
//!   never applied to data written after it.
//! - **R7 Flush.** Completes only when every operation acknowledged on the
//!   volume before the flush was received is durable against power loss,
//!   including operations an earlier instance of the daemon acknowledged.
//!   It need not cover operations in flight.
//! - **R8 Order.** Writes, write-zeroes and discards are applied whole, in
//!   one total order. An operation submitted after another was
//!   acknowledged follows it. Two overlapping operations in flight
//!   together may go either way round; the later one wins on every block
//!   they share. Every accepted request completes exactly once.
//! - **R9 Daemon loss.** The daemon is lost and returns while its client
//!   lives on. Every acknowledged operation is still there. Requests that
//!   were not acknowledged are reissued, in any order, and may include
//!   ones the lost daemon had applied; the result is as the contract
//!   describes all the same.
//! - **R10 Power loss.** The recovered volume is a prefix of the daemon's
//!   order that includes at least what every completed flush covered.
//!   Every operation in it is whole. What recovery exposes is durable
//!   before the first request is served: a second power cut does not take
//!   it back.
//! - **R11 Failed volume.** Once the write log has failed for good, the
//!   flush fails and so does every later request, reads included, also
//!   after a daemon restart.
//! - **R12 Read-only.** On a read-only target writes, write-zeroes and
//!   discards fail and a flush succeeds.
//!
//! Outside the suite, by §4.1: locked memory and the daemon's files off
//! its own pools. Not checkable through this interface: that a flush never
//! waits for upload, that a flush is *held* for a recoverable condition,
//! that a discard is recorded for the uploader, and the held-then-failed
//! read against unreachable object storage (M3).
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
//! - **Buffers are owned, aligned, and make a round trip.** A read
//!   carries the buffer to fill, a write carries its data, and the
//!   completion hands the buffer back, whatever the result. A transport
//!   can recycle a fixed set of buffers and never allocate on the I/O
//!   path, which the design asks of the daemon (§10). A [`Buf`] is whole
//!   blocks in 4096-aligned memory, so a store can hand it to a log
//!   opened with direct I/O without a second copy.
//! - **Every accepted request completes, including a rejected one.** An
//!   invalid request is reported in its completion, not by `submit`, so a
//!   transport has one place where results turn into its own status codes.
//!   `submit` fails only for what the transport itself got wrong: more than
//!   [`Geometry::queue_depth`] requests in flight, or an id used twice.
//! - **`&mut self`, one owner per queue.** An io_uring loop or a ring loop
//!   is a single thread that owns its queue. A target with several queues
//!   would hand out one handle per queue. That is not in the trait or the
//!   suite yet: M1 has one queue, and ordering across queues is untested.
//! - **Transport-neutral.** Nothing in the trait is specific to a kernel
//!   transport. A transport loop forwards requests to a `BlockTarget`
//!   store and its completions back, and the agent's tools (label reads,
//!   the offline GUID rewrite, §5.4) drive the same trait in-process as
//!   the "library client" of §4.1.
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
//!   §4.2 starts with one copy per request, which one concrete aligned
//!   type serves. Left for the day a zero-copy mode is adopted (gate G1).
//! - *`Vec<u8>` buffers.* No alignment, so a direct-I/O log would have to
//!   copy every write a second time.
//! - *Ids assigned by the target.* The caller would need a map from its tag
//!   to the target's ticket, and could not reissue under the same id.
//! - *A completion file descriptor or waker.* A real transport will want
//!   to sleep on its own event source and be woken by the store.
//!   [`BlockTarget::wait`] is enough for the suite and for a simple loop;
//!   a wake-up hook can be added without changing the rest.
//!
//! # Notes
//!
//! - **A reissued operation can be in the order twice** (rule R9). W1 and
//!   W2 overlap and are in flight together. The daemon logs W1, then W2,
//!   acknowledges W2 and dies. W1 is reissued and logged again: the log
//!   is W1, W2, W1. A reissue carries nothing to recognise it by, so it is
//!   a new operation in the order. The model lets an operation that was in
//!   flight at a daemon loss occupy two positions: one before the loss,
//!   which may or may not exist, and one after.
//! - **When a failed volume starts failing** (rule R11). Once the log has
//!   failed for good no flush succeeds. Requests between the failure and
//!   its first report may succeed or fail: a store learns of the failure
//!   from an append or an fsync, not at the instant it happens. Every
//!   request submitted after the first reported failure fails.
//! - **The base race** (a delayed fetch of a base block racing a write to
//!   it) needs a base whose fetches a test can delay. The model and
//!   `MemTarget` have a non-empty base; the race case is deferred to M3.
//! - **Which error wins** when a request is wrong in two ways (two
//!   reasons to reject it; invalid and on a failed volume; a mutation on
//!   a volume that is read-only and failed) is unspecified; the model
//!   accepts either.
//! - **Limits of the test model**, not of the contract: the suite's
//!   random programs keep at most four writes, write-zeroes and discards
//!   in flight together, and after a power cut the suite reads back every
//!   block an unflushed operation touched before it issues anything else.
//!   See [`model`] for why, and `conformance` for the suite's other
//!   limits.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::time::Duration;

mod buf;
#[cfg(feature = "conformance")]
pub mod conformance;
pub mod mem;
pub mod model;

pub use buf::Buf;

/// The block size of the contract: 4 KiB (rule R1). Pools are created with
/// `ashift=12`.
pub const BLOCK_SIZE: u64 = 4096;

/// `BLOCK_SIZE` as a `usize`, for slicing buffers.
pub const BLOCK_USIZE: usize = BLOCK_SIZE as usize;

/// The largest read or write: 1 MiB (rule R1). A transport splits anything
/// larger.
pub const MAX_IO_BYTES: u64 = 1 << 20;

/// Device sizes are multiples of this: 1 MiB (rule R1).
pub const SIZE_UNIT: u64 = 1 << 20;

/// The parameters a target reports when it is opened (rule R1). They do not
/// change while it is open.
///
/// Fixed by the contract and so not fields: the block size
/// ([`BLOCK_SIZE`]), the largest read or write ([`MAX_IO_BYTES`]), a
/// volatile write cache (flushes are meaningful), discard supported, and
/// no forced-unit-access writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    size_bytes: u64,
    queue_depth: u32,
    read_only: bool,
}

/// Why a [`Geometry`] could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GeometryError {
    /// The size is zero, not a multiple of 1 MiB, or too large.
    #[error("device size must be a non-zero multiple of 1 MiB")]
    BadSize,
    /// No request could ever be in flight.
    #[error("queue depth must be at least one")]
    BadQueueDepth,
}

impl Geometry {
    /// A device of `size_mib` MiB that takes up to `queue_depth` requests
    /// in flight.
    pub fn new(size_mib: u64, queue_depth: u32, read_only: bool) -> Result<Self, GeometryError> {
        let size_bytes = size_mib
            .checked_mul(SIZE_UNIT)
            .filter(|s| *s > 0)
            .ok_or(GeometryError::BadSize)?;
        if queue_depth == 0 {
            return Err(GeometryError::BadQueueDepth);
        }
        Ok(Self {
            size_bytes,
            queue_depth,
            read_only,
        })
    }

    /// Device size in bytes. A non-zero multiple of [`SIZE_UNIT`].
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Device size in blocks.
    pub fn blocks(&self) -> u64 {
        self.size_bytes / BLOCK_SIZE
    }

    /// The longest read or write the target accepts: [`MAX_IO_BYTES`].
    /// Discards and write-zeroes are bounded only by the device.
    pub fn max_io_bytes(&self) -> u64 {
        MAX_IO_BYTES
    }

    /// How many requests the target accepts in flight at once. A request is
    /// in flight from `submit` until its completion is returned by `poll`
    /// or `wait`.
    pub fn queue_depth(&self) -> u32 {
        self.queue_depth
    }

    /// Whether the target is read-only (rule R12).
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// The one definition of a valid request (rule R2).
    ///
    /// Checks, in this order: zero length, alignment, the size limit for
    /// reads and writes, and the end of the device. A flush is always
    /// valid. Whether a valid request is allowed on a read-only or failed
    /// target is not decided here.
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
        if op.carries_data() && len > MAX_IO_BYTES {
            return Err(BlkError::TooLarge {
                len,
                max: MAX_IO_BYTES,
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

/// What a volume holds before anything is written to it (rule R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// Empty: every block reads as zeros.
    Empty,
    /// A commit, as for a clone or a restore. For tests its content is a
    /// pattern computed from the seed and the block number, so that a
    /// harness can build the same base in any store. No block of it is
    /// zeros.
    Pattern(u64),
}

impl Base {
    /// Fill `out`, one block long, with the base content of `block`.
    pub fn fill_block(&self, block: u64, out: &mut [u8]) {
        match self {
            Base::Empty => out.fill(0),
            Base::Pattern(seed) => {
                let mut z = seed
                    .wrapping_mul(0xD6E8_FEB8_6659_FD93)
                    .wrapping_add(block.wrapping_mul(0x9E37_79B9_7F4A_7C15));
                z ^= z >> 31;
                let word = (z | 0x8080_8080_8080_8080).to_le_bytes();
                for chunk in out.chunks_mut(8) {
                    chunk.copy_from_slice(&word[..chunk.len()]);
                }
            }
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
        buf: Buf,
    },
    /// Write `data` at `offset`.
    Write {
        /// Byte offset on the device.
        offset: u64,
        /// The data. Returned in the completion for reuse.
        data: Buf,
    },
    /// Make every operation acknowledged so far durable (rule R7).
    Flush,
    /// The blocks are no longer needed (rule R6).
    Discard {
        /// Byte offset on the device.
        offset: u64,
        /// Length in bytes.
        len: u64,
    },
    /// Write zeros (rule R5).
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
    /// The buffer of a read (filled, if `result` is `Ok`) or of a write,
    /// whatever the result: buffers make a round trip. `None` for the
    /// other operations.
    pub buf: Option<Buf>,
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
    /// A read or write longer than [`MAX_IO_BYTES`].
    #[error("length {len} exceeds the largest request {max}")]
    TooLarge {
        /// The length asked for.
        len: u64,
        /// The limit.
        max: u64,
    },
    /// A write, write-zeroes or discard on a read-only target (rule R12).
    #[error("the target is read-only")]
    ReadOnly,
    /// The volume has failed: its write log is lost for good (rule R11).
    /// Every later request fails with this too.
    #[error("the volume has failed")]
    Failed,
    /// The target could not carry out a valid request for another reason.
    #[error("I/O error: {0}")]
    Io(std::io::ErrorKind),
}

impl BlkError {
    /// Whether this is a rejection of an invalid request (rule R2), as
    /// opposed to a failure of a valid one.
    pub fn is_rejection(&self) -> bool {
        matches!(
            self,
            BlkError::Misaligned { .. }
                | BlkError::ZeroLength
                | BlkError::OutOfBounds { .. }
                | BlkError::TooLarge { .. }
        )
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
/// rules R1 to R12. See the [crate documentation](crate) for the rules and
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
    /// The order of completions says nothing about the daemon's order of
    /// operations (rule R8): only "acknowledged before submitted" does.
    fn poll(&mut self, out: &mut Vec<Completion>) -> usize;

    /// As [`poll`](BlockTarget::poll), but if nothing is ready and requests
    /// are in flight, block until at least one completes or `timeout`
    /// passes. Returns 0 at once if nothing is in flight.
    fn wait(&mut self, out: &mut Vec<Completion>, timeout: Duration) -> usize;
}
