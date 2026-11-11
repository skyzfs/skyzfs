//! From a ublk request descriptor to the block target contract, and
//! from an outcome to the result the kernel is given. Pure: no queue,
//! no target, no kernel.
//!
//! The numbers are those of the kernel's `ublk_cmd.h`, as shipped with
//! `libublk-rs-sys` 0.1.2: the operation is the low byte of `op_flags`,
//! the flags are the rest, and positions are in sectors of 512 bytes.
//!
//! # The rules (design §4.1), and where each is kept
//!
//! - **The five operations.** Read, write, flush, discard and
//!   write-zeroes are translated. Anything else (write-same, the zone
//!   operations, a number this file does not know) is refused: the
//!   device does not advertise them.
//! - **Rejected requests reach nothing.** A request that is empty, not
//!   a multiple of 4 KiB in offset or length, beyond the end of the
//!   device, or a read or write above 1 MiB, is refused here and no
//!   operation is built for it. The kernel honours the limits the
//!   device advertises, so such a request means the limits were not
//!   advertised as intended; it is not split or rounded.
//! - **Forced unit access.** The device does not advertise it, and the
//!   block layer then never sets the flag. If one arrives all the same,
//!   it is the operation followed by a flush, acknowledged when the
//!   flush is: [`Plan::then_flush`]. That holds for a write, a
//!   write-zeroes and a discard. On a flush and on a read the flag
//!   means nothing more.
//! - **Never "not supported".** Every refusal and every failure is
//!   given to the kernel as [`EIO`], a plain I/O error. A flush or a
//!   discard answered "not supported" would make ZFS stop issuing it
//!   for good, and no other error is meant to tell ZFS anything. This
//!   includes the operations outside the five: nothing that speaks to
//!   this device sends them, and one error path is easier to keep
//!   right than two.
//! - **No short counts.** A read or a write that succeeded is answered
//!   with its whole length in bytes, the other operations with 0.
//!
//! # Flags
//!
//! | Flag | What is done |
//! |---|---|
//! | fail-fast (device, transport, driver), meta, swap | hints to the driver; ignored |
//! | no-unmap, on a write-zeroes | ignored: a write-zeroes is recorded as zeros and frees nothing anyway |
//! | FUA | the operation, then a flush (above) |
//! | need-registered-buffer, integrity, shared-memory zero copy | refused: they say the data is somewhere this transport does not look, and belong to features it does not turn on |
//! | any other bit | ignored. Design silent; the conservative reading taken here is that a bit the kernel adds later is a hint, as the driver's features are opt-in |
//!
//! A flush carries no range; whatever its sector fields hold is
//! ignored.

use skyzfs_blk::{BlkError, Geometry, BLOCK_SIZE, MAX_IO_BYTES};

/// The size of a sector, the unit of a descriptor's positions.
pub const SECTOR: u64 = 512;

/// The error every refused or failed request is completed with: a plain
/// I/O error (`EIO`), negated when it is given to the kernel.
pub const EIO: i32 = 5;

/// The operations of `ublk_cmd.h` (`UBLK_IO_OP_*`).
pub mod op {
    /// Read.
    pub const READ: u8 = 0;
    /// Write.
    pub const WRITE: u8 = 1;
    /// Flush.
    pub const FLUSH: u8 = 2;
    /// Discard.
    pub const DISCARD: u8 = 3;
    /// Write-same: not one of the five.
    pub const WRITE_SAME: u8 = 4;
    /// Write-zeroes.
    pub const WRITE_ZEROES: u8 = 5;
    /// The zone operations and the zone report: not among the five.
    pub const ZONED: [u8; 7] = [10, 11, 12, 13, 14, 15, 18];
}

/// The flags of `ublk_cmd.h` (`UBLK_IO_F_*`), as bits of `op_flags`.
pub mod flag {
    /// Fail fast on a device error.
    pub const FAILFAST_DEV: u32 = 1 << 8;
    /// Fail fast on a transport error.
    pub const FAILFAST_TRANSPORT: u32 = 1 << 9;
    /// Fail fast on a driver error.
    pub const FAILFAST_DRIVER: u32 = 1 << 10;
    /// Metadata.
    pub const META: u32 = 1 << 11;
    /// Forced unit access.
    pub const FUA: u32 = 1 << 13;
    /// A write-zeroes must not deallocate.
    pub const NOUNMAP: u32 = 1 << 15;
    /// Swap.
    pub const SWAP: u32 = 1 << 16;
    /// The server has to register the buffer itself.
    pub const NEED_REG_BUF: u32 = 1 << 17;
    /// The request has an integrity buffer.
    pub const INTEGRITY: u32 = 1 << 18;
    /// The data is in a registered shared-memory buffer.
    pub const SHMEM_ZC: u32 = 1 << 19;
    /// The flags that say the data is elsewhere.
    pub const DATA_ELSEWHERE: u32 = NEED_REG_BUF | INTEGRITY | SHMEM_ZC;
}

/// A request as the ublk driver describes it (`struct ublksrv_io_desc`
/// without the buffer address).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Desc {
    /// The operation in bits 0 to 7, the flags in bits 8 to 31.
    pub op_flags: u32,
    /// The length in sectors.
    pub nr_sectors: u32,
    /// The first sector.
    pub start_sector: u64,
}

impl Desc {
    /// The operation.
    pub fn op(&self) -> u8 {
        (self.op_flags & 0xff) as u8
    }

    /// The flags.
    pub fn flags(&self) -> u32 {
        self.op_flags & !0xff
    }
}

/// What a request asks for, in the contract's terms. Offsets and
/// lengths are in bytes, multiples of 4 KiB, inside the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Read `len` bytes at `offset`.
    Read {
        /// Byte offset.
        offset: u64,
        /// Length in bytes, at most 1 MiB.
        len: u64,
    },
    /// Write `len` bytes at `offset`.
    Write {
        /// Byte offset.
        offset: u64,
        /// Length in bytes, at most 1 MiB.
        len: u64,
    },
    /// Flush.
    Flush,
    /// Discard the range.
    Discard {
        /// Byte offset.
        offset: u64,
        /// Length in bytes.
        len: u64,
    },
    /// Write zeros over the range.
    WriteZeroes {
        /// Byte offset.
        offset: u64,
        /// Length in bytes.
        len: u64,
    },
}

/// A translated request: one operation of the contract, and whether a
/// flush follows it before the request is acknowledged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// The operation.
    pub action: Action,
    /// The request carried FUA: it is acknowledged only when a flush
    /// submitted after the operation was acknowledged has completed.
    pub then_flush: bool,
}

/// Why a request was refused. It reaches no target, and is completed
/// with [`EIO`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// Not one of the five operations.
    #[error("operation {0} is not one of the five")]
    Operation(u8),
    /// A flag that puts the data where this transport does not look.
    #[error("flags {0:#x} say the data is elsewhere")]
    Flags(u32),
    /// No sectors.
    #[error("an empty request")]
    Empty,
    /// The offset or the length is not a multiple of 4 KiB.
    #[error("sector {start}+{sectors} is not on 4 KiB blocks")]
    Misaligned {
        /// The first sector.
        start: u64,
        /// The length in sectors.
        sectors: u32,
    },
    /// The request reaches beyond the end of the device.
    #[error("sector {start}+{sectors} is beyond the end of the device")]
    OutOfBounds {
        /// The first sector.
        start: u64,
        /// The length in sectors.
        sectors: u32,
    },
    /// A read or a write above 1 MiB.
    #[error("a read or write of {sectors} sectors is above 1 MiB")]
    TooLarge {
        /// The length in sectors.
        sectors: u32,
    },
}

/// Defects, for the tests that show each rule is tested.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u32);

impl Mutants {
    /// FUA is dropped: no flush follows the operation.
    pub(crate) const FUA_WITHOUT_FLUSH: u32 = 1 << 0;
    /// A refused discard is answered "not supported".
    pub(crate) const DISCARD_NOT_SUPPORTED: u32 = 1 << 1;
    /// A misaligned request is rounded to blocks and passed on.
    pub(crate) const ROUND_MISALIGNED: u32 = 1 << 2;
    /// A request beyond the end of the device is passed on.
    pub(crate) const PASS_OUT_OF_BOUNDS: u32 = 1 << 3;
    /// A successful read or write is answered with 0 bytes.
    pub(crate) const SHORT_COUNT: u32 = 1 << 4;
    /// Write-same is taken for a write.
    pub(crate) const WRITE_SAME_IS_WRITE: u32 = 1 << 5;

    pub(crate) fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// Translates the requests of one device.
#[derive(Debug, Clone, Copy)]
pub struct Translator {
    size: u64,
    mutants: Mutants,
}

impl Translator {
    /// For a device of this geometry.
    pub fn new(geometry: &Geometry) -> Self {
        Self {
            size: geometry.size_bytes(),
            mutants: Mutants::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_mutants(geometry: &Geometry, mutants: u32) -> Self {
        Self {
            size: geometry.size_bytes(),
            mutants: Mutants(mutants),
        }
    }

    /// The plan for `desc`, or why it is refused.
    pub fn translate(&self, desc: Desc) -> Result<Plan, Refusal> {
        let flags = desc.flags();
        if flags & flag::DATA_ELSEWHERE != 0 {
            return Err(Refusal::Flags(flags & flag::DATA_ELSEWHERE));
        }
        let op = match desc.op() {
            op::WRITE_SAME if self.mutants.has(Mutants::WRITE_SAME_IS_WRITE) => op::WRITE,
            known @ (op::READ | op::WRITE | op::FLUSH | op::DISCARD | op::WRITE_ZEROES) => known,
            other => return Err(Refusal::Operation(other)),
        };
        if op == op::FLUSH {
            return Ok(Plan {
                action: Action::Flush,
                then_flush: false,
            });
        }
        let (offset, len) = self.range(desc, op == op::READ || op == op::WRITE)?;
        let action = match op {
            op::READ => Action::Read { offset, len },
            op::WRITE => Action::Write { offset, len },
            op::DISCARD => Action::Discard { offset, len },
            _ => Action::WriteZeroes { offset, len },
        };
        let fua = flags & flag::FUA != 0 && !self.mutants.has(Mutants::FUA_WITHOUT_FLUSH);
        Ok(Plan {
            action,
            then_flush: fua && op != op::READ,
        })
    }

    /// The byte range of a ranged request, checked against the device.
    fn range(&self, desc: Desc, carries_data: bool) -> Result<(u64, u64), Refusal> {
        let (start, sectors) = (desc.start_sector, desc.nr_sectors);
        if sectors == 0 {
            return Err(Refusal::Empty);
        }
        let per_block = BLOCK_SIZE / SECTOR;
        let (mut first, mut count) = (start, u64::from(sectors));
        if first % per_block != 0 || count % per_block != 0 {
            if !self.mutants.has(Mutants::ROUND_MISALIGNED) {
                return Err(Refusal::Misaligned { start, sectors });
            }
            first -= first % per_block;
            count = count.div_ceil(per_block) * per_block;
        }
        let len = count * SECTOR;
        if carries_data && len > MAX_IO_BYTES {
            return Err(Refusal::TooLarge { sectors });
        }
        let inside = first
            .checked_mul(SECTOR)
            .and_then(|offset| offset.checked_add(len).map(|end| (offset, end)))
            .filter(|(_, end)| *end <= self.size);
        match inside {
            Some((offset, _)) => Ok((offset, len)),
            None if self.mutants.has(Mutants::PASS_OUT_OF_BOUNDS) => {
                Ok((first.wrapping_mul(SECTOR), len))
            }
            None => Err(Refusal::OutOfBounds { start, sectors }),
        }
    }

    /// The result the kernel is given for a request that was refused:
    /// the negated [`EIO`], whatever the operation.
    pub fn refused(&self, desc: Desc) -> i32 {
        if desc.op() == op::DISCARD && self.mutants.has(Mutants::DISCARD_NOT_SUPPORTED) {
            return -95;
        }
        -EIO
    }

    /// The result the kernel is given for a request whose plan ended
    /// with `outcome`: the whole length in bytes for a read or a write
    /// that succeeded, 0 for the other operations, and the negated
    /// [`EIO`] for any failure, whatever its kind.
    pub fn result(&self, plan: &Plan, outcome: &Result<(), BlkError>) -> i32 {
        if outcome.is_err() {
            return -EIO;
        }
        match plan.action {
            Action::Read { len, .. } | Action::Write { len, .. } => {
                if self.mutants.has(Mutants::SHORT_COUNT) {
                    return 0;
                }
                // At most 1 MiB: it fits.
                i32::try_from(len).unwrap_or(-EIO)
            }
            Action::Flush | Action::Discard { .. } | Action::WriteZeroes { .. } => 0,
        }
    }
}

#[cfg(test)]
mod tests;
