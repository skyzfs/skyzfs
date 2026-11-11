//! The SkyZFS local store (design §5.1). So far: the write log, the
//! disk it is tested on, and a block target on the log and a home
//! image.
//!
//! - [`io`]: the few file and directory operations the store uses, as
//!   traits, so that the same code runs on real files and on a disk that
//!   fails on purpose;
//! - [`sim`]: `SimDisk`, that disk, in memory and reproducible from a
//!   seed;
//! - [`fs`]: the same operations on real files;
//! - [`mod@format`]: the log's bytes;
//! - [`log`]: the log: append, sync, recover, replay, reclaim;
//! - [`store`]: the block target ([`skyzfs_blk::BlockTarget`]) on the
//!   log and a home image: see [The store](#the-store).
//!
//! The log knows nothing of blocks: a record is a kind, a range of the
//! volume, and for a write its data.
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
//!    succeeds, a replay included. The next open refuses the log until the caller
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
//! This is version 1, and version 1 is not fixed until the store that
//! owns the log ships: no log exists outside tests, and the layout has
//! changed under this version number and may again.
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
//! | 64 | 8 | epoch: incremented at every write of a copy, so by every recovery and every reclaim |
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
//! | 16 | 8 | epoch: the superblock's when the record was written |
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
//! 128 bits from the operating system's cryptographic generator, and
//! a log is not created without them. It is
//! in the superblock and nowhere else: it is not part of
//! [`format::Identity`], it is in no error, report or debug print (those
//! name positions and kinds), and it must never be uploaded or logged.
//! What remains: whoever can read the log's files has the key, and
//! whoever can do that can already write them. That holds only if the
//! directory makes it so: **the log's directory must be readable by the
//! store's own user only.** On Unix the files are created with mode
//! 0600. A superblock whose key is all zeros is refused.
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
//!    closes a segment and no file holds the next segment. (Sealing
//!    always is a simplification, not the repair of a known failure:
//!    no history is known in which sealing only where something was
//!    written loses anything, since the epoch already refuses what
//!    could come back. It removes that case from the argument.)
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
//! **Every superblock starts an epoch.** A recovery and a reclaim each
//! write a superblock copy, and each copy has an epoch one above the
//! other's. The copy is synced before anything is written in its
//! epoch. So everything written after the newest copy, a record or the
//! header of a segment file, carries an epoch the older copy does not
//! have. If the newest copy is lost to corruption, an open with the
//! older one meets that epoch, in a record on its way or in the header
//! of any segment file whether on its way or not, and fails with
//! [`log::Corruption::SuperblockBehind`] before it has written
//! anything. Without this, after reclaims, the older copy would
//! describe a log made of files that have been reused since, the open
//! would return that older and shorter log, and its erasing of "stale"
//! segments would destroy the real one. The one case in which the
//! older copy is taken is that nothing was written since the newest:
//! then it describes the whole log, with the start it had before the
//! last reclaim. An epoch may rise inside a segment without a seal;
//! the walk only requires that it never falls.
//!
//! A reclaim that fails while writing its copy may or may not have
//! left it; the log has failed, nothing is written in the new epoch,
//! and the next open takes whichever copy is there.
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
//! **Three rules that only a crafted payload can test.** With honest
//! histories on the simulated disk nothing distinguishes a log without
//! them: that a walk looks at the unit boundary before it looks
//! directly behind a record; that recovery seals also where the prefix
//! does not end on a boundary; and that of two files claiming the same
//! part of the log the older claim is erased. They decide what happens
//! where old or foreign bytes could pass for a record at one of the two
//! places a walk looks, which without the key a client could arrange
//! and with it cannot. They are kept as a second line behind the tag.
//!
//! **A read error is not an end.** If a read fails during recovery,
//! `open` returns the error and has written nothing. Only a place that
//! was read and holds no valid record ends the log.
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
//! **What must change before an append can run during a sync.** Today
//! nothing runs between a sync's issue and its completion, and three
//! places in [`Log::sync`](log::Log::sync) rely on it:
//!
//! - the set of files to sync is cleared after the device syncs. A file
//!   dirtied in between would be forgotten, the next sync would skip it
//!   and report success, and a power cut would take records a sync
//!   claimed. The set must be taken when the sync is issued;
//! - the same for the flag that says the directory needs a sync, when a
//!   segment file is created in between;
//! - an append clears the reservation of a new unit that the sync took
//!   at issue. That is right for the append's own unit, but a second
//!   sync issued while the first is in flight must take the reservation
//!   again, and each sync needs its own target position.
//!
//! A test for this needs a simulated disk whose sync can be held open
//! between issue and completion.
//!
//! The same goes for a reclaim: it starts a new epoch, and the records
//! appended from then on carry it. With appends running during a
//! reclaim, the switch has to be ordered against them: no record of the
//! new epoch before the superblock that starts it is durable, and none
//! of the old epoch behind one of the new.
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
//! # What the log's caller is held to
//!
//! Three things the log refuses, because a caller that got them wrong
//! would break the promises above without the log noticing:
//!
//! - [`Log::replay`](log::Log::replay) ends at the durable position at
//!   the latest. What a replay returns can be acted on for good (the
//!   store writes it into the home image), and a record that no sync
//!   covers can still be lost.
//! - [`Log::reclaim`](log::Log::reclaim) takes only the start or the
//!   end of a record, as a replay does.
//! - With [`Log::set_segment_limit`](log::Log::set_segment_limit) an
//!   append that would need one segment file more than the limit
//!   returns [`LogError::Full`](log::LogError::Full). That is not a
//!   failure: nothing was written, the log goes on, and the append
//!   succeeds after a reclaim. The limit lives in memory, like the
//!   hold, and is not part of the format.
//!
//! [`Log::read_payload`](log::Log::read_payload) reads a part of one
//! record's payload by the position the append returned, synced or
//! not. It does not check the record's tag or checksum again: the
//! position comes from an append or a replay of this instance, which
//! did.
//!
//! # The store
//!
//! [`store::Store`] is a volume: the contract of `skyzfs-blk` (design
//! §4.1, rules R1 to R12 there) on three things.
//!
//! - **The log.** A write, a write-zeroes and a discard are each one
//!   record, appended in the order the requests were submitted.
//! - **The home image**, `home.img`: one file of the volume's size. It
//!   holds the volume as it was at the log's start, give or take
//!   blocks that a record still in the log writes again.
//! - **The index**, in memory: for every block that a write or a
//!   write-zeroes record of the log covers, the newest such record. A
//!   read takes a block from that record if there is one, and from the
//!   home image otherwise.
//!
//! A **checkpoint** moves the log's start forward: it writes the
//! records from the start up to the durable position, or up to the
//! hold if that is lower, into the home image; syncs the home image;
//! reclaims that part of the log; and only then drops those records
//! from the index. The store runs one when the log is full, and the
//! caller may at any time ([`store::Store::checkpoint`]).
//!
//! **One instance per volume** (design §4.2). Opening a store is a
//! recovery that writes: an epoch, a seal. Before it reads or writes
//! anything the store takes an exclusive lock on its directory
//! ([`io::Dir::try_lock`]) and holds it until it is dropped. A second
//! open, or a creation over a live store, fails at once with
//! [`store::StoreError::Locked`] and changes nothing. On real files the
//! lock is an advisory lock on the file `lock`, which the operating
//! system releases when the process ends, however it ends.
//!
//! ## One request at a time
//!
//! Requests are executed one at a time in the order of submission, and
//! each is completed before the next starts. That order is the
//! "single order" of rule R8, and it is the log's order.
//!
//! - [`store::Store`] does the work in the caller's thread: `submit`
//!   validates the id and the queue depth and queues the request;
//!   `wait` executes what is queued and returns the completions, so it
//!   does file I/O and never sleeps. Its `poll` does nothing and
//!   returns 0: the contract says `poll` never blocks, executing a
//!   request is I/O, and `wait` hands out every completion it makes,
//!   so none is ever waiting. A caller of this driver has to call
//!   `wait`. It is what the tests drive, because every step is then
//!   reproducible.
//! - [`store::Served`] ([`store::Store::serve`]) gives the store to one
//!   worker thread. `submit` sends the request to it, `poll` takes what
//!   has completed without blocking, `wait` blocks up to its timeout.
//!   A checkpoint and the hold go through the same queue, so they fall
//!   between two requests.
//!
//! For M1.3, each ublk queue is another caller of `submit`. What that
//! needs and is not here: completions routed back to the queue that
//! submitted (one channel carries them all today); reads that do not
//! wait behind a sync (the worker does everything, so a flush stalls
//! every read behind it, and group commit of several queues' writes
//! under one sync is not there either); and a bound on the worker's
//! queue per ublk queue, which today is the single `queue_depth`.
//!
//! ## Why the volume keeps the contract
//!
//! Call *the order* the order of the log's records. The volume's
//! content at any moment is: the home image, overlaid with the log's
//! records from its start to its end, in order. Reads return that
//! content (R3, R4) if two things hold, and the steps below keep them:
//!
//! - **(I) the index is the overlay**: it names, for each block, the
//!   last record in the log that writes it;
//! - **(H) below the log's start the home image is right**: a block
//!   that no record in the log writes has, in the home image, the
//!   content the order gave it before the log's start.
//!
//! *A request.* A write is acknowledged after
//! [`Log::append`](log::Log::append) has returned, and the index is
//! pointed at the new record before the next request runs. (I) holds;
//! (H) is untouched. A write-zeroes is a record of its own and reads
//! as zeros whatever the base is (R5). A discard is recorded, for
//! whoever uploads later, and changes nothing a read can see: the
//! blocks keep their content, which R6 allows and which is the one
//! choice that needs no reasoning about what "discarded" survives.
//! Nothing is written or erased locally, so **a discard is not a way
//! to make data unreadable on the local disk**. A
//! request that the geometry refuses is answered before anything else
//! happens (R2), and on a read-only volume nothing is appended (R12).
//!
//! *A flush* returns after [`Log::sync`](log::Log::sync) has. Every
//! request acknowledged before the flush was submitted was appended
//! before it, since requests run in submission order, so the sync
//! covers it (R7).
//!
//! *A checkpoint* works in batches of records, each from the log's
//! start. Where the write unit is one block there is one batch. Step
//! by step, with what a crash leaves after each:
//!
//! 1. Where the write unit is larger than a block: the blocks that
//!    share a write unit with a block about to be written, are not
//!    themselves written, and are not written by a durable record
//!    still in the log, are copied into `home.pre`, tagged for the
//!    log's current start, and the file is synced. (See [the preimage
//!    file](#the-preimage-file).) A crash here has changed nothing in
//!    the home image.
//! 2. The batch's records are written into the home image. Every
//!    block written is one that a record in the log writes, so (H)
//!    says nothing about it, and the index sends reads of it to the
//!    log. A block may now hold an older record's data than the
//!    newest; the index still names the newest. A crash here leaves
//!    the home image with any mixture in those blocks, and the log,
//!    which is durable up to the batch's end and beyond, with all of
//!    the records: the next open rebuilds the index from them and no
//!    read sees the mixture. Only records up to the durable position
//!    are written, because a record beyond it can be lost with the
//!    power, and then nothing would cover what the home image got from
//!    it. (This also holds for the hold: records above it stay in the
//!    log, and reads of them stay with the log.)
//! 3. The home image is synced. Now every block that only records of
//!    the batch write has its final content durably.
//! 4. The log is reclaimed up to the batch's end. This is the step
//!    that makes (H) apply to those blocks, and step 3 has made it
//!    true first. It is also what retires the preimage file, which
//!    counts only while the log starts where it did when the file was
//!    written. A crash between 3 and 4 leaves the records in the log
//!    and the file valid: the open writes the preimages back, which
//!    changes nothing, and the next checkpoint applies the records
//!    again, which writes the same bytes.
//! 5. The index drops the entries whose record is below the new start.
//!    Before the reclaim returned they still pointed into the log,
//!    which could still serve them; after it, the home image does.
//!    An entry whose record is above the new start stays, also for a
//!    block that the batch wrote.
//!
//! *The process is lost* (R9). Promise 1 of the log: every record whose
//! append returned is there, with everything before it. An open
//! recovers the log, restores preimages if a checkpoint was cut off,
//! and rebuilds the index by replaying the log from its start, which
//! establishes (I). (H) held before the loss or the log still has the
//! records, by the checkpoint's order. Requests that were in flight
//! were appended or not, each whole.
//!
//! *The power is cut* (R10). Promise 2: the log comes back as a prefix
//! of the order, each record whole, with every record a sync covered.
//! Promise 3: that prefix is durable before the open returns and what
//! was left out does not come back, so what is read after recovery is
//! stable. The home image is right for the same reason as above, plus
//! the preimages, which cover the one thing a power cut does that a
//! lost process does not: sectors of a write unit that were being
//! rewritten come back as anything. So the volume is the home image
//! overlaid with a prefix of the order.
//!
//! *Something fails* (R11). A failed append, sync, or write or sync of
//! the home image or the preimage file, and a failed read of any of
//! them, fails the volume: the store completes the request with an
//! error, writes the log's failure marker, and answers every later
//! request, reads included, with an error. **The failure is announced
//! before the marker is attempted**: the marker needs the disk that
//! has just failed, and a sync on a sick disk can take minutes or
//! never return, while a transport must give up its attachment at
//! once (design §4.2). So the store calls an observer
//! ([`store::OpenOptions::on_failure`]) once, when it decides, from
//! the thread that executes requests; the observer is to return
//! quickly. A flush that finds nothing to make durable does not touch
//! the disk and succeeds: it promises nothing new, and a disk that
//! would refuse the next sync and has not been asked has not failed
//! yet (design §4.1). The next write or sync that has work to do is
//! what finds a dead disk.
//! A full log is not a failure. The next open refuses the volume
//! unless it is told that the client knows
//! ([`store::OpenOptions::acknowledge_failure`]); then promise 4 gives
//! every record that a successful sync covered, so every completed
//! flush, and (H) holds by the checkpoint's order, since a checkpoint
//! that failed part way is a checkpoint cut off part way.
//!
//! *The log is full.* With a limit on the log's segment files
//! ([`store::OpenOptions::log_segments`]), a write that does not fit
//! makes the store sync the log and checkpoint, and try once more. If
//! a hold keeps the checkpoint from freeing a segment, the request is
//! neither completed nor failed: it stays first in the queue, and with
//! it everything behind it, until the hold moves
//! ([`store::Store::set_hold`]).
//!
//! ## The preimage file
//!
//! Writing one block of the home image rewrites the whole write unit
//! around it (see the rule about write units above), and a power cut
//! during that write can leave any sector of the unit as garbage. The
//! blocks a batch itself writes are covered by the log, and so is any
//! block that a durable record still in the log writes. The other
//! blocks of the unit may be in the home image alone. So where the
//! write unit is larger than a block, a batch first saves those in
//! `home.pre` and syncs it, and an open that finds the file whole and
//! current writes them back into the home image, and syncs it, before
//! anything is read from the home image. Where the write unit is one
//! block (4 KiB pages) there is no file and no extra sync.
//!
//! All integers little-endian. A tag is SipHash-2-4 under the volume's
//! key, the one the log's headers are tagged with.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZPRE\0` |
//! | 8 | 4 | version, 1 |
//! | 12 | 4 | number of blocks saved |
//! | 16 | 16 | the volume |
//! | 32 | 8 | the log's start when the batch began |
//! | 40 | 8 | tag of: that start, then the table |
//! | 48 | 456 | zero |
//! | 504 | 8 | tag of bytes 0 to 503 |
//! | 4096 | 16 each | the table: block number (8), tag of: the start, the block number, the block's content (8) |
//! | after the table, rounded up to 4096 | 4096 each | the blocks, in the table's order |
//!
//! - **Whole.** The file is whole if the header's tag, the table's tag
//!   and every block's tag match. The blocks are data a client wrote,
//!   going back to places in the home image, so nothing here is a
//!   plain checksum: content a client chose cannot make a torn file,
//!   or a file with a block moved to another place, look whole.
//! - **Current.** The file counts only if its start is where the log
//!   starts now. A batch ends by moving the log's start, durably, and
//!   the start never goes back, so a file of an earlier batch is never
//!   taken for the current one. That is why the file is not erased
//!   after a batch: nothing depends on it being gone. While the log's
//!   start has not moved, writing the file back is always right: a
//!   block in it either is in the home image alone, and then the file
//!   has its content as of that start, or a record in the log writes
//!   it, and then the home image's copy is not read.
//! - **A cut while writing back** leaves the file as it was, and the
//!   next open writes it back again.
//! - **Every point a batch can be cut at.** Let the log start at F
//!   when the batch begins, and at G when it has ended.
//!   1. *While the file is written, before its sync.* The home image
//!      is untouched. The file may be torn, or whole for F by chance
//!      of what survived; neither harms.
//!   2. *After the file's sync, while the home image is written.* The
//!      blocks the batch writes are in the log; the other blocks of
//!      their units are in the file or in a durable record. The open
//!      finds the file whole and bound to F, the log starts at F, and
//!      it writes them back.
//!   3. *After the home image's sync, before the reclaim.* As 2, and
//!      the writing back changes nothing.
//!   4. *During the reclaim.* The reclaim writes one copy of the
//!      superblock and syncs it. After a cut the log starts at F, and
//!      this is point 3, or at G, and this is point 5. It cannot start
//!      anywhere else, and once G is durable it never shows F again:
//!      every superblock that is written starts a new epoch, and a
//!      copy that is behind is known to be behind (see "Why recovery
//!      yields a prefix").
//!   5. *After the reclaim.* The log starts at G, the file is bound to
//!      F and counts for nothing. The home image was synced before the
//!      reclaim began, so everything below G is durably in it.
//!   6. *The next batch.* It writes its own file, bound to G, and syncs
//!      it before it touches the home image; if it has nothing to
//!      save it writes no file, and the old one, bound to F, stays
//!      dead. **The reclaim is durable before the next batch writes the
//!      home image**: `Log::reclaim` returns only after the superblock
//!      is synced. If it were not, a cut could bring back F beside a
//!      file bound to G, the file would not be written back, and that
//!      batch's neighbours would be unprotected. A test runs the
//!      batches on a log whose reclaim is not durable and sees exactly
//!      that.
//! - **Bounded.** A batch saves at most 256 blocks, so the file is at
//!   most 1 MiB and two blocks. A batch is as many records as fit that
//!   bound, counting for each record the rest of its first and last
//!   write unit, and at most 1024 records.
//! - **Cost**, where the write unit is larger than a block, for each
//!   batch: a read of each block saved, one write and one sync of
//!   `home.pre`, on top of the sync of the home image and the reclaim
//!   (a write and a sync of the superblock) that a batch costs
//!   anywhere. In the worst case, single-block writes scattered one to
//!   a 16 KiB unit, that is a batch for every 85 records, and so three
//!   syncs and about 1 MiB of preimages for every 340 KiB applied.
//!   Writes of whole units cost no preimages. Not measured.
//!
//! # Limits
//!
//! Of the store:
//!
//! - **One worker.** Everything is serial, a flush included: see above
//!   for what M1.3's queues need.
//! - **Preimages cost a checkpoint** on machines whose pages are larger
//!   than a block (16 KiB on Apple silicon and some arm64 kernels):
//!   see [the preimage file](#the-preimage-file). Direct I/O on the
//!   home image, whose write unit is then the device's block, would
//!   remove the need.
//! - **The log's files are created on demand**, up to the limit, and
//!   not when the volume is made: a full local disk is found on the
//!   commit path, as a failed volume, and not at creation as the
//!   design intends. The log's limit is in memory and its files are
//!   made by the append that needs one; creating them ahead belongs
//!   with making the limit a property of the volume.
//! - **The lock keeps out stores, not everything.** It is advisory: a
//!   program that writes the files without asking is not stopped.
//!   And it belongs to the open file, of which a child process holds
//!   a copy from its fork to its exec. Dropping the store unlocks, so
//!   a store that is closed and opened again while another thread
//!   starts a child is not found locked. A process that dies without
//!   dropping the store, at the moment a child of its own is between
//!   fork and exec, leaves the lock to that child until the exec, and
//!   the next instance may see `Locked` for that moment.
//! - **Write-zeroes is written out as zeros** by a checkpoint, and a
//!   discard frees nothing: no hole is punched in the home image.
//! - **The home image is written through the page cache**, so a
//!   checkpoint's data is in memory twice until it is synced.
//! - **The index is rebuilt at every open** by reading the whole log,
//!   and holds one entry for each run of blocks, in memory.
//! - **A read error fails the volume**, also where another copy could
//!   have served the read. Nothing is repaired here.
//! - **A read from the log does not check the record again.** A
//!   payload corrupted in the log after it was validated is served to
//!   a read as it is, and fails the volume at the next checkpoint,
//!   which checks every record before anything is made permanent.
//! - **A full file system fails the volume for good.** The home image
//!   is sparse, so a checkpoint's write into it can run out of space,
//!   and that is a failed write like any other. Design §4.1 holds a
//!   condition that can recover and does not fail on it, and a full
//!   file system is one: a known gap, for the package that accounts
//!   for space.
//! - **A hold is checked only inside the log.** A position at or below
//!   the log's start is taken as it is and holds the whole log.
//! - **No upload, no cuts, no eviction**: the home image is always
//!   complete (cache policy `complete`), and the hold is whatever the
//!   caller sets.
//! - **The file system's type is not checked** yet (see below: ext4
//!   and XFS).
//!
//! Of the log, not modelled and not defended against:
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
//!   ([`log::Corruption::SuperblockBehind`]). If the newest copy is
//!   lost and nothing was written since, the log opens with the start
//!   it had before the last reclaim: longer at the front, never
//!   shorter at the end. The floor is only advanced
//!   by recoveries and reclaims. A flipped bit above it ends the log
//!   there, as a torn record would, and only recovery's report says
//!   why.
//! - **A file system that accepts direct I/O and reads from its
//!   cache.** Recovery takes a successful open with `O_DIRECT` as a
//!   read from the device. Some file systems accept the flag and serve
//!   the read from memory (tmpfs on newer kernels, ZFS before 2.3 are
//!   believed to), and then
//!   [`Recovery::checked_device`](log::Recovery::checked_device) is
//!   true although nothing was checked: after a failed sync, recovery
//!   may there accept records that only the cache holds. The log is
//!   supported on **ext4 and XFS**. Checking the file system's type
//!   belongs to the store that owns the log.
//! - **Platforms other than Linux.** There is no read from the device
//!   on macOS and Windows yet, so `checked_device` is always false
//!   there. Off Unix, syncing a directory is not implemented and
//!   returns an error rather than success, so a log cannot be created
//!   or synced there: the Windows port has to supply it.
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
//!   file system has none
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
pub mod store;
