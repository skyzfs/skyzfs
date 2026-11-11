//! The SkyZFS local store (design §5.1). So far: the write log, and the
//! disk it is tested on.
//!
//! - [`io`]: the few file and directory operations the store uses, as
//!   traits, so that the same code runs on real files and on a disk that
//!   fails on purpose;
//! - [`sim`]: `SimDisk`, that disk, in memory and reproducible from a
//!   seed;
//! - [`fs`]: the same operations on real files;
//! - [`mod@format`]: the log's bytes;
//! - [`log`]: the log: append, sync, recover, replay, reclaim.
//!
//! No block target sits on the log yet, and the log knows nothing of
//! blocks: a record is a kind, a range of the volume, and for a write
//! its data.
//!
//! # What the log promises
//!
//! 1. **Process loss.** When [`Log::append`](log::Log::append) has
//!    returned, the record is there after the process is lost, and so
//!    is every record before it.
//! 2. **Power loss.** After a power cut the log is a prefix of the
//!    records that were appended, in order, each whole. The prefix
//!    contains every record that a successful
//!    [`Log::sync`](log::Log::sync) covered.
//! 3. **Recovery is final.** What [`Log::open`](log::Log::open) returns
//!    is durable before it returns, and what it left out never comes
//!    back: not after a second power cut, and not behind records
//!    appended later.
//! 4. **Failure is final.** After a write or a sync has failed, nothing
//!    succeeds. The next open refuses the log until the caller
//!    acknowledges the failure, and then the log holds at least what
//!    every successful sync covered.
//!
//! # Layout
//!
//! A log is a directory with these files:
//!
//! | File | Content |
//! |---|---|
//! | `log.super` | The superblock, in two copies |
//! | `log.000000`, `log.000001`, ... | Segment files, all of the size the superblock gives |
//! | `log.failed` | Empty. Its existence says that the log has failed |
//!
//! A **position** ([`format::Lsn`]) is a byte offset in an unbounded
//! space. With segments of `S` bytes, segment `k` holds positions
//! `k * S .. (k + 1) * S`, whichever file it is in.
//!
//! Every integer is little-endian. Every structure has a version and
//! reserved bytes, written as zero and required to be zero. Checksums
//! are CRC32C.
//!
//! **Superblock.** Two copies of 512 bytes, at offsets 0 and 4096 of
//! `log.super`, so that they never share a sector. An update is written
//! to the copy that does not hold the current superblock, and synced.
//! The valid copy with the higher counter counts.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZLOG\0` |
//! | 8 | 4 | version, 1 |
//! | 12 | 4 | reserved |
//! | 16 | 16 | volume id |
//! | 32 | 8 | volume size in bytes |
//! | 40 | 4 | volume block size |
//! | 44 | 4 | record alignment, 64 or 4096 |
//! | 48 | 8 | segment size |
//! | 56 | 8 | counter: incremented at every write of a copy |
//! | 64 | 8 | epoch: incremented by every recovery |
//! | 72 | 8 | start of the log |
//! | 80 | 8 | durable floor: a position that was durable when the copy was written |
//! | 88 | 420 | reserved |
//! | 508 | 4 | CRC32C of bytes 0..508 |
//!
//! **Segment file.** The first 4096 bytes are for its header, of which
//! 512 are used. The header says which part of the log the file holds
//! now; the file's name says nothing, and a file is given another part
//! by overwriting the header in place. Records follow from byte 4096.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZSEG\0` |
//! | 8 | 4 | version, 1 |
//! | 12 | 4 | reserved |
//! | 16 | 16 | volume id |
//! | 32 | 8 | base: the position of the file's first byte |
//! | 40 | 8 | epoch in which the file was given this base |
//! | 48 | 460 | reserved |
//! | 508 | 4 | CRC32C of bytes 0..508 |
//!
//! **Record.** A header of 64 bytes, padded to the record alignment,
//! then the payload, padded to the alignment. With alignment 4096 the
//! header has a block of its own and the payload starts on a block, so
//! that both could go out in one direct write from aligned memory.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 4 | magic `SKZR` |
//! | 4 | 1 | version, 1 |
//! | 5 | 1 | kind: 1 write, 2 write-zeroes, 3 discard, 4 pad, 5 mark |
//! | 6 | 2 | reserved |
//! | 8 | 8 | the record's own position |
//! | 16 | 8 | epoch of the instance that wrote it |
//! | 24 | 8 | byte offset on the volume |
//! | 32 | 8 | byte length on the volume (pad: bytes skipped) |
//! | 40 | 4 | payload length |
//! | 44 | 4 | CRC32C of the payload |
//! | 48 | 12 | reserved |
//! | 60 | 4 | CRC32C of bytes 0..60, seeded with the chain value |
//!
//! The **chain value** of a record is the header checksum of the record
//! before it. For the first record of a segment it is the CRC32C of the
//! volume id followed by the segment's base, so that a replay can start
//! at any segment.
//!
//! A **pad** closes a segment: the log continues at byte 4096 of the
//! segment with the next base. A segment keeps its last block free for
//! it. A **mark** is nothing; recovery writes one (below). Neither is
//! ever returned by a replay.
//!
//! **Where the next record is.** Directly behind the record before it,
//! or at the next 4096-byte boundary. The log uses the boundary for the
//! first record after a sync and after a recovery, and a reader looks
//! at the boundary first.
//!
//! # Why a block with durable records is never written again
//!
//! Nothing on the disk is assumed to be written atomically. A sector
//! that is being written when the power fails may hold the old bytes,
//! the new bytes, or neither. Kernels write back whole pages and disks
//! whole physical sectors, so a small write rewrites everything around
//! it in its 4096-byte block. A record appended into the block that
//! holds the end of a durable record would therefore put that record at
//! risk again. So the record after a sync or a recovery starts a new
//! block. The cost is at most one block of log space for each sync at
//! alignment 64, and nothing at 4096.
//!
//! The superblock follows the same rule by having two copies, and a
//! segment header shares its block with nothing.
//!
//! # Why recovery yields a prefix
//!
//! Recovery ([`Log::open`](log::Log::open)) does this, in this order:
//!
//! 1. It syncs every file of the log and the directory. Whatever an
//!    earlier process wrote and did not sync is now with the device.
//! 2. It reads the superblock and walks the records from the start of
//!    the log, **reading from the device** and not through the
//!    operating system's cache. It stops at the first place where no
//!    valid next record is.
//! 3. It increments the epoch in the superblock and syncs that.
//! 4. It seals the end of what it found: at the next block boundary it
//!    writes a mark (or, in a segment's last block, a pad) of the new
//!    epoch, and syncs it. Where nothing was ever written behind the
//!    prefix, it writes nothing.
//! 5. It erases the header of every segment file that claims a part of
//!    the log beyond the prefix, or a part that a newer file holds.
//!
//! A record is valid if its header's checksum matches under the chain
//! value of the record before it; it names the position it is at; its
//! epoch is not below the epoch of the record before it, nor of its
//! segment's header, nor above the superblock's; its lengths fit the
//! segment; and its payload matches its checksum. A segment follows the
//! one before it if the file's header names the next base, is not older
//! than the pad that closed the segment before it, and, of two files
//! that name the same base, is the one of the later epoch.
//!
//! **A torn record is not accepted.** Any missing or foreign sector of
//! a record is in its header or its payload, and each has a checksum.
//!
//! **A record after a gap is not accepted.** The walk only ever moves
//! from a valid record to the next, and stops for good at the first
//! place without one. What recovery reports about records that seem to
//! follow the break ([`log::Recovery::follows`]) is for diagnosis: the
//! log ends at the break whatever follows it, and nothing behind a
//! break is ever replayed or repaired.
//!
//! **The prefix contains every synced record.** A sync returns only
//! after every segment file with unsynced bytes, and the directory if
//! a segment file was created, has been synced, so every sector of
//! every record it covers was durable; no later write touches those
//! blocks; so each such record is still valid, and the walk passes it.
//!
//! **The prefix is durable when recovery returns** because step 1
//! synced everything and step 2 validated what the device holds. This
//! matters after a failed sync: an operating system may then mark the
//! unwritten pages clean, go on showing them to every reader, also in a
//! new process, and report success for the next sync. A walk through
//! the cache would accept records that are not on the disk.
//!
//! **What recovery left out never comes back.** Suppose the prefix
//! ends at position `P`, and something that was written behind it
//! survives: a later record `R`, or a later segment.
//!
//! - To accept `R` again, a walk must first accept a record that ends
//!   where `R` starts and whose header checksum `R` is chained to. The
//!   records that were there are gone or damaged; a record written
//!   later has the new epoch in its header, so it is a different
//!   header with a different checksum. (And `R`, being older than that
//!   record, also fails the epoch rule.)
//! - The first record of a segment is chained to a value that does not
//!   change. There the epoch decides: after recovery a segment's file
//!   gets its header in the new epoch, and a first record older than
//!   its segment's header is refused.
//! - The record that was at `P` itself, if its header survived and
//!   only its payload was lost, could be made whole again by a later
//!   write of the same payload. The seal prevents it. If `P` is on a
//!   block boundary the mark overwrites that header. If not, the mark
//!   is at the boundary behind `P`, and a reader looks there first; the
//!   old header is in a block that is never written again and is never
//!   looked at.
//! - A stale segment file has its header erased; and if it had not, it
//!   is older than the pad that will close the segment before it.
//!
//! A power cut during recovery itself leaves the superblock's previous
//! copy and the same files, and the next recovery finds the same
//! prefix.
//!
//! # Order of acknowledgements
//!
//! **A record may be acknowledged only when its own write and the
//! write of every record before it have returned.** Otherwise a record
//! could be acknowledged behind one whose write is still in progress,
//! and the loss of the process would leave a torn record with an
//! acknowledged one behind the break. The log is an object with one
//! writer, and `append` writes in order, one record at a time, so this
//! holds by construction. Whoever puts the log behind several threads
//! or an asynchronous interface must keep it.
//!
//! # Failure
//!
//! After a failed write or sync the log is failed. A failed sync is
//! never tried again: the data it did not write may be gone from the
//! cache, and a later sync that succeeds would vouch for nothing. The
//! failing call creates `log.failed` and syncs the directory, and
//! reports whether that worked. [`Log::open`](log::Log::open) refuses a
//! log with that file unless the caller acknowledges the failure; then
//! it recovers as usual and removes the file.
//!
//! The gap: if the disk refuses the marker too, a new instance cannot
//! know that the log failed. The log it finds still holds what every
//! successful sync covered, because recovery reads the device. That
//! such an instance does not serve a client that was told more is the
//! transport's duty (design §4.1: it gives up the attachment).
//!
//! # Limits
//!
//! Not modelled, and not defended against:
//!
//! - **A disk that lies about a sync.** If a sync returns before the
//!   data is persistent, a power cut can lose synced records. On macOS
//!   the standard library's sync is the full flush (`F_FULLFSYNC`).
//! - **Misdirected writes**: data that lands at another place than it
//!   was written to. A record names its own position, so a misdirected
//!   record is not accepted elsewhere, but what it overwrote is lost.
//! - **Corruption that a 32-bit checksum does not see.** The checksums
//!   are there to refuse torn, stale and misplaced records after a
//!   crash. Positions, epochs and the chain do part of that work, and
//!   ZFS checksums every block above the log. They are not an integrity
//!   guarantee against bit rot or against anyone who chooses the data:
//!   a client that could predict the log's headers could write a
//!   payload that looks like them.
//! - **Corruption of durable records.** A flipped bit in the middle of
//!   the log ends the log there. If that is below the durable floor in
//!   the superblock, open fails; the floor is only advanced by
//!   recoveries and reclaims, so above it the log is silently shorter
//!   and only the report says why. Nothing is repaired.
//! - **A lost newest superblock copy.** If records exist in an epoch
//!   above the superblock that was found, the newest copy is gone and
//!   open fails. Writing both copies at every update would survive it,
//!   at the cost of a second sync.
//! - **Running out of space.** Creating a segment file fails the log
//!   like any other write error.
//!
//! Costs of the choices made here, not measured yet:
//!
//! - **Buffered I/O.** The log's data is in the operating system's page
//!   cache as well as in ZFS's ARC until it is synced and the kernel
//!   lets go of it. A sync has to write the dirty pages before it can
//!   flush the device. After a failed sync the cache cannot be trusted,
//!   which is why recovery needs reads from the device, and where a
//!   file system has none (tmpfs)
//!   [`Recovery::checked_device`](log::Recovery::checked_device) is
//!   false and the prefix may hold records that were never durable.
//! - **What direct I/O would change.** No second copy of the data in
//!   memory; a sync that only has to flush the device; no cache that
//!   can disagree with the disk, so the from-the-device read path and
//!   its "not possible here" case go away. It would cost the alignment:
//!   every record then begins with a 4096-byte header block, which
//!   doubles the log space and the bytes written for 4096-byte writes.
//!   The format already has that alignment as a parameter, so it is a
//!   matter of how the files are opened, not of a new format.
//! - **Segment files are created on demand**, and a new one is filled
//!   with zeros first, on the commit path. Creating them ahead of need
//!   belongs to the store that owns the log.
//! - **The hold is not stored.** After an open the caller sets it again
//!   before the first reclaim.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod format;
pub mod fs;
pub mod io;
pub mod log;
pub mod sim;
