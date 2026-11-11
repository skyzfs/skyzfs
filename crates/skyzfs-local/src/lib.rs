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
//! are CRC32C; tags are SipHash-2-4 with a 64-bit result, under the
//! log's key (below).
//!
//! **Superblock.** Two copies of 512 bytes, at offsets 0 and 65536 of
//! `log.super`: the largest write unit apart, so that writing one never
//! rewrites the other. An update is written to the copy that does not
//! hold the current superblock, and synced. The valid copy with the
//! higher counter counts.
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
//! | 88 | 4 | write unit: a power of two from 4096 to 65536 |
//! | 92 | 16 | key: 128 random bits drawn when the log is created |
//! | 108 | 400 | reserved |
//! | 508 | 4 | CRC32C of bytes 0..508 |
//!
//! **Segment file.** The first write unit is for its header, of which
//! 512 bytes are used. The header says which part of the log the file
//! holds now; the file's name says nothing, and a file is given another
//! part by overwriting the header in place. Records follow from the
//! second write unit.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZSEG\0` |
//! | 8 | 4 | version, 1 |
//! | 12 | 4 | reserved |
//! | 16 | 16 | volume id |
//! | 32 | 8 | base: the position of the file's first byte |
//! | 40 | 8 | epoch in which the file was given this base |
//! | 48 | 456 | reserved |
//! | 504 | 8 | tag of eight zero bytes followed by bytes 0..504 |
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
//! | 48 | 8 | reserved |
//! | 56 | 8 | tag of the chain value (8 bytes) followed by bytes 0..56 |
//!
//! The **chain value** of a record is the tag of the record before it.
//! For the first record of a segment it is the tag of the volume id
//! followed by the segment's base, so that a replay can start at any
//! segment. A record's tag so covers everything a recovery acts on: the
//! kind, the position, the epoch, the range of the volume, the payload's
//! length and the payload's checksum, and through the chain every
//! record before it in the segment.
//!
//! **The key.** A walk looks for the next record at a unit boundary
//! first, and at alignment 64 that boundary can lie inside the payload
//! of a record, where the bytes are a client's. A client knows or can
//! guess every field of a header, so with a plain checksum it could
//! write a payload that a recovery takes for a record: a discard or a
//! write anywhere on the volume, and the end of the real log. The tag
//! is therefore keyed, with a key the client does not have. The key is
//! in the superblock and nowhere else: it is not part of
//! [`format::Identity`], it is in no error, report or debug print (those
//! name positions and kinds), and it must never be uploaded or logged.
//! What remains: whoever can read the log's files has the key, and
//! whoever can do that can already write them.
//!
//! A **pad** closes a segment: the log continues at the second write
//! unit of the segment with the next base. A segment keeps its last
//! write unit free for it. A **mark** is nothing; recovery writes one
//! (below). Neither is ever returned by a replay.
//!
//! **Where the next record is.** Directly behind the record before it,
//! or at the next multiple of the write unit. The log uses the boundary
//! for the first record after a sync and after a recovery, and a reader
//! looks at the boundary first.
//!
//! # Why a unit with durable records is never written again
//!
//! Nothing on the disk is assumed to be written atomically. A sector
//! that is being written when the power fails may hold the old bytes,
//! the new bytes, or neither. And a small write rewrites everything
//! around it: a kernel writes back whole pages, a disk whole physical
//! sectors. The **write unit** is the largest thing that is rewritten
//! as a whole. It is stored in the superblock, because it belongs to
//! the machine and not to the format: 4096 on most, 16384 where pages
//! are 16 KiB (Apple silicon, some arm64 kernels). A new log should
//! take [`format::Identity::system_write_unit`].
//!
//! A record appended into the unit that holds the end of a durable
//! record would put that record at risk again. So the first record
//! after a sync or a recovery starts a new unit. This holds from the
//! moment a sync is issued, not from its completion: a record appended
//! while the sync is in flight would dirty the unit again after the
//! sync has made it durable. The cost is at most one unit of log space
//! for each sync at alignment 64, and nothing where alignment and unit
//! are both 4096.
//!
//! The superblock follows the same rule by having two copies, and a
//! segment header shares its unit with nothing.
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
//! 4. It seals the end of what it found: at the next unit boundary it
//!    writes a mark (or, where the segment has no room to go on, a pad)
//!    of the new epoch, and syncs it. Always, whatever the place holds:
//!    that it reads as never written proves nothing, since a sector
//!    that was lost reads as it did before. The one exception is by
//!    structure and not by content: the prefix ends with the pad that
//!    closes a segment and no file holds the next segment.
//! 5. It erases the header of every segment file that claims a part of
//!    the log beyond the prefix, or a part that a newer file holds.
//! 6. If the log was marked as failed and the caller acknowledged it,
//!    it removes the marker and syncs the directory, last: a crash
//!    before that leaves the marker.
//!
//! A record is valid if its header's tag matches under the log's key
//! and the chain value of the record before it; it names the position it is at; its
//! epoch is not below the epoch of the record before it, nor of its
//! segment's header, nor above the superblock's; its lengths fit the
//! segment; and its payload matches its checksum. A segment follows the
//! one before it if the file's header names the next base, is not older
//! than the pad that closed the segment before it, and, of two files
//! that name the same base, is the one of the later epoch.
//!
//! **A torn record is not accepted.** Any missing or foreign sector of
//! a record is in its header or its payload; the header has its tag
//! and the payload its checksum, which the tag covers.
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
//! units; so each such record is still valid, and the walk passes it.
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
//!   where `R` starts and whose tag `R` is chained to. The
//!   records that were there are gone or damaged; a record written
//!   later has the new epoch in its header, so it is a different
//!   header with a different tag. (And `R`, being older than that
//!   record, also fails the epoch rule.)
//! - The first record of a segment is chained to a value that does not
//!   change. There the epoch decides: after recovery a segment's file
//!   gets its header in the new epoch, and a first record older than
//!   its segment's header is refused.
//! - The record that was at `P` itself, if its header survived and
//!   only its payload was lost, could be made whole again by a later
//!   write of the same payload. The seal prevents it. If `P` is on a
//!   unit boundary the mark overwrites that header. If not, the mark
//!   is at the boundary behind `P`, and a reader looks there first; the
//!   old header is in a unit that is never written again and is never
//!   looked at.
//! - A stale segment file has its header erased; and if it had not, it
//!   is older than the pad that will close the segment before it.
//!
//! A power cut during recovery itself leaves the superblock's previous
//! copy and the same files, and the next recovery finds the same
//! prefix.
//!
//! **What sealing costs.** One write unit of log space for each
//! recovery, and no new segment file, ever: a log whose files are all
//! in use recovers. A process that crashes in a loop uses one unit for
//! each start until the segment's last unit, which takes the closing
//! pad; from then on a recovery writes no seal. With segments of 64 MiB
//! and a unit of 4096 that is at most 16383 recoveries' worth of space
//! in one segment, and then none. The space comes back when the segment
//! is reclaimed.
//!
//! **A segment header is written once for each use of its file**: when
//! the log rolls into the file, which must be free (it holds nothing at
//! or above the start of the log, and the start that says so is
//! durable). It is never written while the file holds records the log
//! needs, and recovery erases only headers of files that are not
//! segments of the log it found.
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
//! it recovers as usual and removes the file. Whether the same client
//! is still attached, and so whether the log may be served again, is
//! for the caller to know.
//!
//! Every failed write or sync on the log's own files fails the log,
//! also inside a reclaim. A reclaim that fails while writing the new
//! start may have moved the start all the same: it only ever gives up
//! what was durable and below the hold, and since the log has failed,
//! no segment below the new start is reused.
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
//!   ZFS checksums every block above the log. A payload's checksum is
//!   not an integrity guarantee against bit rot.
//! - **Someone who can read the log's files.** The key keeps a client
//!   that only chooses payloads from forging a header. It is stored in
//!   the clear beside the log, and protects nothing from a reader of
//!   the device, who could write the device as well. A forged tag is
//!   right by chance with probability 2^-64 for each header a recovery
//!   looks at.
//! - **Corruption of what was durable is detected and reported, never
//!   repaired.** The log has no redundancy against media corruption,
//!   the superblock included; a volume that loses durable records falls
//!   back to its last uploaded cut. Where there is proof that something
//!   durable is gone, open fails and says why, and never returns an
//!   empty or a shorter log: a prefix that ends below the durable floor
//!   in the superblock ([`log::Corruption::BelowDurableFloor`]), and
//!   records or segments of a later epoch than the superblock that was
//!   found, which means the newest superblock copy is lost
//!   ([`log::Corruption::SuperblockBehind`]). The floor is only advanced
//!   by recoveries and reclaims. A flipped bit above it ends the log
//!   there, as a torn record would, and only recovery's report says
//!   why.
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
//!   The rule about write units is as good as the kernel's tracking of
//!   what is dirty: a kernel that writes back a whole multi-page folio
//!   when one page of it is dirty rewrites the neighbours too, with the
//!   same content.
//! - **What direct I/O would change.** No second copy of the data in
//!   memory; a sync that only has to flush the device; no cache that
//!   can disagree with the disk, so the from-the-device read path and
//!   its "not possible here" case go away; and the write unit becomes
//!   the device's block, whatever the kernel's pages and folios are. It
//!   would cost the alignment:
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
