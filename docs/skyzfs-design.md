# SkyZFS: Architecture and Roadmap

| | |
|---|---|
| Status | Draft. Decisions are recorded in §10.1. M0 is done. M1 is in progress: the contract and its conformance suite, the local store, the ublk transport and `skyzfsd` are built and run against the kernel on Ubuntu 24.04 and 26.04. M1 still lacks two things: the power-cut replay has run on the ublk project's reference server and not yet on `skyzfsd`, and the throughput criterion is not met (§9). |
| Date | 2026-11-12. First draft 2026-11-10 |
| Supersedes | The dm-thin node stack of `skynbd_design.md` and `skynbd-origin-advance-design.md`, for this repository. The object formats, refs and commit protocol carry over. |
| Evidence | `skynbd-postgres-study.md` §10 |
| Detail | `skyzfs-impl-notes.md`: for each milestone, its work packages, the design as built, what was measured, and the review record |

This document is the architecture and the roadmap. How each part was built, what the kernel and the benchmark showed, and every review are in `skyzfs-impl-notes.md`, by milestone and work package. "Notes" below means that document.

## 1. Context

SkyNBD keeps a volume in object storage and gives each node a local copy-on-write stack built from dm-thin and dm-cache. That stack works, but it is Linux-only, it amplifies PostgreSQL's writes, and every snapshot costs the workload throughput for as long as chunks are being copied.

The product goal has also widened. The target is easy, consistent data protection for individuals and small businesses, across clouds and vendors, with **native clients on Linux, macOS and Windows**. The core promise is unchanged: cheap snapshots every minute, keep the last *n*, and restore any of them instantly on any machine.

This document proposes replacing the node stack with OpenZFS on a block device served by a node-local Rust daemon, and lays out the path from a Linux prototype to clients on all three systems.

### 1.1 What the I/O study established

The topology benchmark modelled ZFS on a userspace block backend and compared it with the dm-thin two-volume split (`twovol`) on one machine, on equal backing and equal memory[^study].

| Question | Result |
|---|---|
| Throughput with the working set on local flash, no snapshots | Level. zfs-slog ran at 0.85 to 1.14 of twovol at 8 clients (median 1.04), and was ahead on five of six machines at 1 and at 32 clients. |
| Throughput with a snapshot every minute | zfs-slog ran 1.26 to 1.60 times twovol. A thin snapshot cost twovol 16 to 24% of its throughput. `zfs snapshot` cost nothing. |
| Latency with a snapshot every minute | p99 6.8 to 9.2 ms against 10.5 to 13.7 ms. Slowest transaction under 0.2 s against 1.5 to 4.1 s. |
| Flash writes | 37 to 75% of twovol's. |
| Cold origin at 10 ms | ZFS is bound by page reads. It needs a backend read cache with read-ahead, which the model did not have. |

The study also produced three rules that this design adopts: the log device must be allocated storage, dirty data must be capped to fit the pool's free space and the ARC, and the uploader must know what ZFS has freed.

## 2. Goals and non-goals

### 2.1 Goals

1. **One engine, three operating systems.** The Rust daemon and the object formats are identical on Linux, macOS and Windows. Only the last hop into the kernel differs.
2. **Snapshots that cost the workload nothing**, taken every minute, with no filesystem freeze.
3. **A snapshot is visible everywhere at one instant**: when its data is in object storage and one ref has been swapped.
4. **Instant restore and clone on any node and any supported OS**, reading lazily from object storage.
5. **Pools portable between operating systems.** A snapshot taken on a Mac restores on Linux and on Windows.
6. **Vendor independence.** Any S3-compatible store first, then other object APIs, then more than one target at once.
7. **Stock ZFS on Linux.** No patched kernel module where the distribution already ships one.

### 2.2 Non-goals for this document

- Multi-writer access to one volume.
- Application-consistent snapshots (VSS writers, database quiesce hooks). ZFS snapshots are atomic across the pool, which is crash consistency for every dataset at once.
- zvols carrying NTFS or APFS. Clients expose ZFS datasets (decision D1, §10.1).
- Kubernetes integration. `skynbd-csi-design.md` will need its own revision once M3 lands.
- Client user interface design.

## 3. Architecture

```mermaid
flowchart TB
  subgraph Host["One machine: Linux, macOS or Windows"]
    APP["Applications"] --> ZPL["ZFS datasets"]
    ZPL --> POOL["Pool: one data vdev, type disk<br/>plus a local log device"]
    POOL --> T{"Transport"}
    T -->|"Linux"| UBLK["ublk block device<br/>stock ZFS, stock kernel"]
    T -->|"macOS, Windows"| SHM["udrive_shm frontend<br/>inside the OpenZFS port"]
    UBLK --> BT
    SHM --> BT
    subgraph Daemon["skyzfsd (Rust)"]
      BT["Block target contract"]
      LOCAL["Local store<br/>write log, block cache, read-ahead"]
      ENG["Engine<br/>extent tree, cuts, commits"]
      UP["Uploader and fetcher"]
      REFS["Refs: compare-and-swap"]
      BT --> LOCAL --> ENG
      ENG --> UP
      ENG --> REFS
    end
    AGENT["skyzfs-agent<br/>snapshot schedule, retention,<br/>publish, restore"] --> ZPL
    AGENT --> Daemon
  end
  UP --> OBJ[("Object storage<br/>one or more vendors")]
  REFS --> OBJ
```

The design has four layers.

| Layer | Role | Portability |
|---|---|---|
| ZFS | Filesystem, snapshots, compression, checksums, encryption | OpenZFS on Linux upstream. macOS and Windows from their ports. |
| Transport | Carries block I/O from the kernel to the daemon | Per OS. ublk on Linux, `udrive_shm` elsewhere. |
| Block target contract | The semantics every transport must provide | One definition, one conformance suite. |
| Engine and local store | Write log, cache, cuts, upload, refs, garbage collection | One Rust codebase. |

The frontend is called `udrive_shm`: a userspace-served drive over shared memory. Two earlier names were dropped because each sat too close to an unrelated part of Linux: `udev_shm` (the device manager `udev`) and `udisk_shm` (the disk-management daemon `udisks`). The name says drive where the kernels say disk; it means the same object, a whole block device.

### 3.1 What carries over from SkyNBD

| Existing crate | Fate |
|---|---|
| `skynbd-types`, `-format`, `-tree`, `-store`, `-refs` | Kept. Extents, COW shard trees, commits, object keys and ref CAS are unchanged. |
| `skynbd-engine` | Kept and extended with the local store. Today a flush acknowledges only after upload and a ref swap[^d1]. ZFS flushes many times a second, so a flush must become local. |
| `skynbd-nbd` | Kept as a third transport: a fallback for Linux kernels without ublk, and a remote read-only export. |
| `skynbd-agent` | The dm-thin orchestration is replaced by ZFS orchestration. The capture state machine, watchdog and crash matrix tests are the model for the new agent. |
| `skynbd-api`, `skynbdctl` | Kept, with snapshot publish reshaped around cuts (§5.2). |

New code: `skyzfs-blk` (contract and conformance suite), `skyzfs-ublk`, `skyzfs-shm` (user side of the shared-memory transport), `skyzfs-local` (write log and cache), the `skyzfsd` binary, and `kmod/udrive_shm` (the C frontend and its per-OS glue, kept as patches against the two ports).

## 4. The block target

### 4.1 Contract

Every transport delivers the same operations to the daemon and must honour the same rules. The rules are written from what the daemon can observe, so that one reference model can express them and one suite can test them. This section was tightened after an independent review of the first draft (§10.2).

**Parameters.** A target reports these when it is opened, and they do not change while it is open:

| Parameter | Value |
|---|---|
| Size | Fixed, a multiple of 1 MiB |
| Logical and physical block size | 4096 bytes. Every offset and length is a multiple of it |
| Largest read or write | 1 MiB. A transport splits anything larger |
| Requests in flight | A stated bound. The transport holds the rest |
| Discard and write_zeroes | Any aligned range inside the device |
| Volatile write cache | Yes: flushes are meaningful and must be delivered |
| Forced-unit-access writes | Not advertised. ZFS does not send them. A transport that delivers one treats it as a write followed by a flush |
| Read-only | Set for an export that must never change, such as a remote read-only export over NBD (§3.1). Browsing a snapshot does not use it (§5.4) |

**Content.** A volume has a base: empty, which reads as zeros, or a commit, for a clone or a restore. The content of a 4 KiB block is what the last operation covering it in the daemon's order left there, or else the base.

| Operation | Rule |
|---|---|
| `read` | Returns the content of each block. |
| `write` | Replaces the content. |
| `write_zeroes` | A write of zeros, with a write's ordering and durability. It need not store data, but on a clone it must hide the base: it is a recorded zero extent, not an absence. |
| `discard` | The content of each block becomes unspecified until the next write to it: the content just before the discard, zeros or the base, possibly differing between reads, never bytes from another offset or volume. A crash may forget a discard. A discard is never applied to data written after it. |
| `flush` | Completes only when every operation acknowledged on the volume before the daemon received the flush is durable against power loss. That includes operations an earlier instance of the daemon acknowledged. It need not cover or wait for operations still in flight. It never waits for upload. |

**Order.** The daemon applies writes, write_zeroes and discards in one total order, which is the order of the write log, and applies each of them whole. An operation submitted after another was acknowledged follows it in that order. Two overlapping operations that are both in flight may be ordered either way round. The later one then wins on every block they share, and it is the same one in memory, in the log, after replay and in every cut. A read is not part of that order. A read submitted after an operation was acknowledged sees it on every block. A read in flight together with an overlapping write may see each block either before or after that write, block by block, but never a block that is itself partly written: that is what a disk gives, and ZFS does not read a block it is writing. A server that hands requests to its store as they arrive has to do the ordering itself: direct I/O to a file does not order two overlapping writes, and a stand-in server that relied on it returned blocks from both[^tear].

**Two kinds of failure.** They promise different things, and treating them as one corrupts a pool. The daemon cannot tell which one it is recovering from, so recovery is one procedure: every start replays the longest valid prefix of the write log and makes it durable before it serves the first request. A second crash then cannot take back what the first recovery exposed, and a flush received by the new instance covers what the old one acknowledged.

- **The daemon is lost and the kernel carries on.** ZFS never restarted, so nothing it was told is done may be undone. An operation is therefore acknowledged only once it would survive the daemon process: appended whole to the write log, though not necessarily flushed to the device. The longest valid prefix then contains every acknowledged operation. Requests that were not acknowledged are issued again. They may arrive in any order, and may include ones the lost daemon had already applied, so all five operations are idempotent. A reissued request carries nothing that ties it to its first delivery, so it is a new operation in the order, and one the lost daemon had applied is in the order twice. This shows only where two overlapping operations were in flight at the loss, which ZFS does not do.
- **Power is lost.** The recovered volume is a prefix of the daemon's order that includes at least what every completed flush covered. Every operation in it is whole: none is torn. Blocks that no lost operation touched are unchanged.

**Errors.**

- A request that is misaligned, empty or beyond the end is rejected and has no effect.
- A flush or a discard is never answered "not supported". ZFS would stop issuing it for good[^zfserr].
- A flush is never failed for a condition that can recover. It is held.
- The write log has failed for good once a write or a sync of it has failed. From then on the volume has **failed**: no flush the daemon receives succeeds, and from the first failure the daemon reports, **every later request fails**, reads included. Every flush that completed stays a promise: when the volume is next opened for a new client it holds a prefix of the daemon's order that includes at least what each of them covered, exactly as after a power cut. Nothing else about a failed volume's content is promised. ZFS ignores the result of the flushes on either side of its uberblocks[^zfserr], so it is the next write that has to fail, and a read of a block whose log record was lost must not return something older. The corner cases (requests in flight at the failure, a flush with nothing new to cover, a restart in between) are in the notes.
- A failed volume stays failed across a daemon restart for as long as its client stays attached: a new instance must never serve a shorter log to a ZFS that was told more. The log leaves a marker (§5.1), the transport gives up the attachment, and every start checks that the log it recovered reaches everything acknowledged in that boot (§4.2). A failed volume fails every request, including the reads and writes with which ZFS probes a device that has just returned an error. That is what makes ZFS suspend the pool instead of passing a read failure to an application[^readerr]. A transport must not answer with an error that means something else to ZFS, above all "not supported".
- A read that needs object storage that cannot be reached is **held until the storage answers**. It is not failed after a timeout. ZFS hands an ordinary read error straight to the application and counts it against the pool, and suspends the pool only when the device itself is gone[^readerr]. A network outage must not become application errors, so the daemon waits and reports the stall through its own status and alerts. ZFS reports it too, as a slow I/O. A held read of pool metadata can stall the open transaction group until storage returns, which is the same wait. Giving up is the administrator's decision: detaching the volume fails every request as a device that is gone, and `failmode=wait` then suspends the pool.
- On a read-only target, writes, write_zeroes and discards fail and a flush succeeds.

**Clients.** A kernel transport is one kind of client. The agent's own tools are another: reading labels to build an import, and rewriting a clone's GUID (§5.4). They use the same contract through a library interface in the daemon, exclusive with a kernel attachment to the same volume. No device node is needed for them, which matters on macOS and Windows, where there is none.

A conformance suite in `skyzfs-blk` exercises these rules against any implementation with a reference model. It distinguishes the two failures: after daemon loss every acknowledged operation is readable, and after power loss only flushed ones are guaranteed. Passing it is the entry condition for every transport and for the local store.
The suite drives an implementation through a harness, and a harness states what it cannot do: cut the power, start from a base that is not empty, serve a read-only volume, fail the log on demand, issue an invalid request, reopen a failed volume. A case that needs a missing capability is reported as not exercised, or runs in a weaker form and says so. Nothing is left out silently. That lets one suite run both in process and through a kernel block device, where a rig can do few of these things. So "the suite passes through `/dev/ublkbN`" is a much weaker statement than "the suite passes in process", and the durability of the whole stack rests on the store's own tests and on the replay rig (§9), not on that run. The counts are in the notes.

Two deployment rules are outside the suite and are covered by a stress test in the kernel rig: the daemon locks its memory, and it keeps its own files off any pool it serves. A daemon that page-faults onto its own pool deadlocks the machine. `skyzfsd serve` checks both before it touches a device.

### 4.2 Linux: ublk

ublk is the in-kernel userspace block driver, driven by io_uring passthrough commands[^ublk]. The daemon fetches requests per queue, copies data to or from its own buffers, and commits results. The kernel presents an ordinary block device, so ZFS uses its normal disk vdev and nothing in ZFS is patched.

- **Recovery.** ublk's user-recovery mode lets a restarted daemon take the device over[^ublk]. The contract needs the variant that reissues in-flight requests, not the one that fails them. With the failing variant ZFS sees I/O errors on its only vdev and has to retry them.
- **Flushes must reach the daemon.** The device advertises a volatile write cache and a 4096-byte logical block. Without the first the block layer completes flushes itself, and there would be no durability and no cuts.
- **One zio is not one request.** The kernel splits and merges to the advertised limits.
- **Held requests.** A read may be held for as long as object storage is unreachable (§4.1). The kernel lets a ublk device do that for minutes: on a stand-in server one read held for 300 s completed without an error, and the kernel logged nothing in that time, in one run on each of two images[^failrig]. Longer holds, many at once, and the daemon itself are for the rig in M3.
- **One instance per volume.** The store takes an exclusive lock on the volume's directory when it is opened and holds it until the process ends. Opening a store is a recovery that writes, so a second daemon is stopped before it opens one.
- **Attachment across restarts.** Before it adds a device the daemon records durably, in the volume's directory, the device's number, a fresh random nonce, the kernel's boot id and the parameters it will ask for, and it leaves a note under `/run`, which lives exactly as long as the boot. A restarted daemon takes a device over only when the note and the record agree. After a reboot no client can still be attached, and the daemon adds a new device.
- **A device waits for its server.** The driver reports three states: dead, live, and waiting for a server. When the daemon ends, by a kill or by an orderly stop, its device waits, with no limit in time, and the next daemon takes it over; requests that were in flight are issued again. The daemon sends no command that stops a device without deleting it.
- **A failed volume gives up its attachment.** When the write log fails, the daemon writes a failure note under `/run`, answers every request it holds with an error, deletes its device and exits, so that no later instance can serve a shorter log to the same import. The pool is suspended as soon as a read or a flush fails. A pool that only writes does not notice (§10). Deleting the device is final for that import: nothing short of a reboot was found that brings the pool back.
- **What was acknowledged is checked at every start.** The daemon stores, before a request is completed to the kernel, the position in the log that the acknowledgement rests on. Every start in the same boot compares the log it recovered with that position, and a log that falls short is a failed volume.
- **Detaching.** `skyzfsd detach` deletes the device of a volume whose daemon has stopped. It never stops a daemon, and it refuses a device with a live server.
- **Library.** The transport is built on the `libublk` crate[^libublk], pinned to one version and confined to one module, for the queue. The daemon sends the control commands itself, through a binding of its own (`skyzfs-ublk-sys`), and takes the driver's own answers: add, lookup, the asynchronous delete, and the begin and the end of a recovery. The binding has kernel tests of its own.
- **One copy per request**, into buffers that are the daemon's own and locked in memory: two per request slot, 128 MiB for a volume at one queue, a depth of 64 and 1 MiB requests. The zero-copy modes are not adopted (gate G1, §9).
- **Labels at byte 0.** `zpool create` partitions only what it recognises as a whole disk, and its device table does not list ublk[^wholedisk], so the pool's labels sit at the start of the device, as they do through `udrive_shm`. CI pins this (§6.2).
- **Kernels.** ublk was merged in Linux 6.0. Feature flags and their kernel versions must be pinned during M1 against the kernel documentation, not from memory. That is not done yet: the daemon asks for user recovery with reissue and has run on kernel 7.0 only, and nothing records the oldest kernel that has every flag it needs or makes the daemon say so on a kernel that lacks one. It is M1's to finish, beside its two open exit criteria (§9).
- **Evidence.** Four rigs run in GitHub Actions on Ubuntu 24.04 and 26.04 (kernel 7.0, ZFS 2.4.1): a kernel probe, a power-cut replay rig, a control rig for the binding, and a daemon rig that takes `skyzfsd` through kills, an orderly stop, a held request, a failed volume, the conformance suite and its checkpoints. What each has shown is in the notes.
- **Fallback.** Where ublk is missing, the existing NBD server on a local socket is the transport.

Detail: notes, M1, "The ublk transport".

### 4.3 macOS and Windows: `udrive_shm`

Neither system has an equivalent of ublk. Both have an OpenZFS port that the user must install in any case, and both ports are active and close to upstream: on 2026-11-10 the macOS port was at 2.4.3-rc3 and the Windows port at 2.4.4-rc3[^ports]. The proposal is a small frontend inside each port that hands zios to the daemon through shared memory.

**It is feasible. The frontend must not be a new vdev type.** Two facts from the port sources decide this:

1. The vdev type table is a static array in common code[^vdevtable]. A new type means patching it in every port.
2. The leaf's type string is stored in the labels and in the pool's own configuration. A ZFS that does not know the type rejects the configuration, and a file vdev refuses anything that is not a regular file[^vdevfile].

A pool created on a Mac must import on stock Linux, where the leaf is a disk at `/dev/ublkbN`. So the leaf must be of type `disk` on every system, and the frontend must sit **below** the disk vdev, selected by the device path.

| Port | Hook | Why it fits |
|---|---|---|
| macOS | A third provider in the port's layered-driver interface, next to its IOKit and vnode providers[^ldi]. | The disk vdev already opens by name and calls `ldi_strategy` and `ldi_ioctl`. The provider is tried first in `ldi_open_by_name`, for paths under `/dev/udrive_shm/`, and the vdev itself is not changed. |
| Windows | A dispatch on the path prefix at the top of the disk vdev's open, start and close. | The Windows disk vdev opens an NT path and builds IRPs[^winvdev]. A path under `/dev/udrive_shm/` takes the shared-memory route in its place, before the port chooses between the stored physical path and the path. |

**The path is absolute and names no node.** On both systems the data leaf's path is `/dev/udrive_shm/<name>`. Both ports' disk vdevs refuse a path that is not absolute before any provider sees it[^abspath], so a form such as `user:<name>`, which an earlier draft used, would never reach the frontend. Nothing exists at that path in the file system and nothing needs to: the frontend matches the prefix before the port looks anything up. Tools that stat or scan device nodes cannot be used on such a path. The agent therefore creates and imports these pools by handing ZFS a configuration it wrote itself (§5.4), with this path on the data leaf and neither a physical path nor a device id, since the Windows port opens a stored physical path in preference to the path[^abspath]. The path the labels hold is whatever the last system to import the pool wrote. It is never relied on.

The I/O path, common to both:

```mermaid
sequenceDiagram
    participant Z as ZFS zio pipeline
    participant F as udrive_shm frontend (kernel)
    participant R as Shared arena (rings and data)
    participant D as skyzfsd

    Note over D,F: Attach. The daemon allocates and locks the arena,<br/>then registers it by ioctl on the ZFS control device
    Z->>F: zio (read, write, flush, trim)
    F->>R: copy write data in, push descriptor
    F-->>D: wake if the daemon is sleeping
    D->>R: pop descriptor
    D->>D: local store (write log, cache)
    D->>R: copy read data in, push completion
    D-->>F: kick if the kernel asked for one
    F->>Z: copy read data out, complete zio
```

Design points:

- **The daemon owns the memory.** It allocates a page-aligned arena, locks it, and passes its address in the attach ioctl. The kernel wires and maps it. Each OS needs only two primitives: wire user pages, and sleep and wake. No Mach ports, section objects or new device nodes.
- **The control path is the port's existing control device.** `/dev/zfs` on macOS and the ZFS device on Windows already take ioctls from an administrator. Attach, wait and kick are three more.
- **Two single-producer rings and a data area.** Descriptors are fixed-size: request id, operation, offset, length, data offset in the arena, priority, and the transaction group as a hint. Completions carry an id and a status.
- **One copy each way**, in and out of the arena. The vdev queue above the frontend still bounds concurrency and aggregates adjacent I/O, so a ring depth of a few hundred is enough.
- **No daemon code in the frontend.** The frontend module contains nothing from the daemon, and the daemon nothing from the frontend. They share only the protocol's definition, one header under a permissive licence. This keeps the CDDL code of the ports and the FSL daemon apart (§10).
- **The kernel never trusts the arena.** Every descriptor field is copied out and validated before use. The daemon is privileged, but a corrupt daemon must not corrupt the kernel.
- **Daemon loss.** When the daemon's handle closes, the provider holds new and in-flight zios. A daemon that re-attaches under the same name within a timeout gets the in-flight requests again. Request ids carry an attach generation, so a completion from before the loss cannot complete a reissued request. Past the timeout every request fails, as a device that is gone, and keeps failing until the daemon is back, so that a flush error ZFS ignores cannot be followed by an uberblock write that succeeds (§4.1). ZFS's `failmode=wait` then suspends the pool until it is cleared. This reuses ZFS's own handling of a vanished disk.
- **A full data area makes the frontend wait**, holding nothing that memory reclaim needs. The area is sized for the largest request times the ring depth.
- **The frontend splits a zio larger than the largest request.** ZFS aggregates up to a tunable limit of 16 MiB, and a pool created elsewhere may hold records larger than 1 MiB.
- **The transaction group in the descriptor is a hint for diagnostics only.** Cuts are detected the same way on every transport (§5.2), so that one tested rule covers them all.
- **Bring-up shortcut.** Before the shared arena exists, the same descriptors can travel inside the wait and kick ioctls with the data copied by the kernel. That is slower and proves everything else.

**Size.** The common C code is a ring, an allocator for the data area, the lifecycle state machine and the vdev-facing calls. Per-OS glue is page wiring, the ioctl entries and the hook above. The estimate is 1,500 to 2,500 lines across both ports, plus tests. It is small in lines. It is not small in responsibility: it runs in the kernel, it must never deadlock against memory reclaim, and it must be rebased as the ports follow upstream.

**Why develop it on Linux first.** The common code can be compiled into userspace `libzpool`, where `ztest` can hammer it against the real daemon with no kernel in the loop, and into a patched Linux module in CI. The protocol is then proven before the first macOS or Windows build (M5).

## 5. Engine and local store

### 5.1 Local store

The daemon keeps two things on local flash.

- **Write log.** Append-only and preallocated. A flush is one write and one device flush that returns only when the data is persistent. On macOS that is `F_FULLFSYNC`, and on Windows `FlushFileBuffers`. Their cost sets commit latency and must be measured (§10). macOS's cheaper `F_BARRIERFSYNC` cannot implement the contract's flush: it orders writes but promises nothing about what is persistent when it returns[^barrier]. A mode built on it would keep the pool consistent after a power cut and could lose flushes that had completed. That is a different durability class, offered by name if at all, and never a faster implementation of the same contract.
  The log's format rests on five rules, each of which came out of a test or a review that broke the simpler version: nothing below the log is atomic; a block that holds flushed records is never written again; record headers are keyed; corruption of what was durable is reported, never repaired and never read as a shorter log; and a failed write or flush fails the log for good, which is the failed volume of §4.1.
- **Home image.** In M1 the store is the log and one file of the volume's size. A write, a write-zeroes and a discard are each one log record. An index in memory names, for each block, the newest record that writes it, and a read takes the block from that record, or from the home image if there is none. A checkpoint copies durable records into the home image, syncs it, moves the log's start, and only then lets go of those index entries. A discard is recorded in the log, for the cut, and changes nothing locally.
- **Checkpoints.** A start replays the whole log, so the log's length is the time a take-over costs. The store therefore checkpoints by itself: above a high mark on the log's live size it works the log off in bounded steps (at most 8 MiB or 1024 records, and one sync of the home image), with requests served between the steps, until the log is below a low mark. Only what is durable is checkpointed, nothing extra is done at a start or at a stop, and a failed step fails the volume.
- **Threads.** One worker owns every mutation, the flush, the log, the checkpoint and the order of all of them. Three things left the worker once M1 had measured where a request's time goes (§9): reads are answered by reader threads (`serve --readers`, 4 by default), which pin what they found against the checkpoint; the store hands what it wrote to the kernel's writeback early, so that a sync finds less to wait for; and the home image's sync runs on a helper thread while the worker goes on serving. What is on the disk, and in what order it gets there, is unchanged by all three.
- **The daemon says what it did.** `skyzfsd status` shows the log's live size, the steps taken and where a request's time goes, by thread and by activity. The benchmark reads it.
- **Limits of the M1 store.** Log segments are created on demand and not preallocated, so a full disk is found on the commit path, and a file system that fills during a checkpoint fails the volume, where §4.1 holds a condition that can recover (§10).
- **Block cache.** Everything written and everything fetched, with read-ahead around a miss. The study's cold-origin result is entirely a statement about this cache. Not built in M1: the store reads and writes its three files through the host's page cache, which stands in for it and which no memory limit of the workload accounts for. Whether the store keeps buffered I/O or does direct I/O with a cache of its own is open, and the benchmark's figures bear on it (§9).

Detail: notes, M1, "The local store".

Two policies cover the two kinds of machine.

| Policy | Local flash holds | For |
|---|---|---|
| `complete` | The whole volume. Nothing is evicted. | Desktops and laptops. The machine works offline and object storage is the protected copy. |
| `cache` | The working set. Clean, uploaded blocks may be evicted. | Servers, and any node that has just restored a volume. |

A restore always starts in `cache` and can be promoted to `complete` by hydrating in the background.

### 5.2 Cuts and snapshot visibility

ZFS writes a transaction group, flushes, writes the uberblocks, and flushes again[^txgorder]. The daemon recognises the uberblock writes by their position in the labels and reads the transaction group *N* from them.

A **cut** is a position in the write log: the end of the log at the moment the daemon receives the first flush after it has acknowledged an uberblock write for *N*. The log up to that position is a complete pool at *N*, whatever else was in flight, for two reasons[^cutproof]:

- Every block of *N* was acknowledged before ZFS submitted the first uberblock write, so the order rule (§4.1) puts it earlier in the log. The argument does not use ZFS's flush before the uberblocks, so it holds even where that flush is skipped or its error ignored.
- Nothing that uberblock *N* can reach is overwritten or discarded before the cut. Blocks freed in *N* become allocatable, and are trimmed, only after ZFS has finished writing *N*'s uberblocks and the flush that follows them. On a nearly full pool that protection covers *N* alone and not *N*−1, which is all a cut needs.

Other writes do land before a cut: blocks of the next transaction group, intent-log blocks that fell back to the main device, repairs. Within a group ZFS also rewrites, in place, blocks it allocated earlier in the same group, as its sync passes converge. Those rewrites all precede the uberblock. They add blocks that *N* does not reference, and nothing else. They can also make the first flush after the uberblock write a stray one, which the definition allows: one copy of the uberblock is enough to import at *N*.

Three details make the rule exact:

- **What counts as an uberblock write.** A write to an uberblock slot that carries the uberblock magic and a transaction group above zero. ZFS fills every slot with transaction group 0 when it creates a pool.
- **What counts as a repeat.** An uberblock whose transaction group and root block pointer both equal the previous one's makes no new cut. *N* alone does not identify a pool state: after a rewind import some versions write a different pool state under a number already used. A cut is identified by its log position, with *N* recorded beside it.
- **When a cut may be published.** Only once the log is durable up to its position. ZFS also ignores the result of the flush after the uberblocks[^zfserr], so it can believe *N* is safe on a volume that has just failed.

```mermaid
sequenceDiagram
    participant A as skyzfs-agent
    participant Z as ZFS
    participant D as skyzfsd
    participant O as Object storage
    participant B as Any other node

    A->>Z: zfs snapshot -r pool@t
    Z->>D: writes of txg N, flush, uberblocks, flush
    D->>D: mark cut N in the write log
    Z-->>A: done (blocks only the agent)
    A->>Z: read createtxg of pool@t
    A->>D: publish the first cut at or after createtxg as snap/t
    D->>O: upload segments up to cut N
    D->>O: write commit, then swap ref snap/t
    Note over O,B: The ref swap is the instant of visibility
    B->>O: list refs, sees snap/t
```

- **No freeze, no pause.** Applications keep writing throughout. The study measured no cost.
- **Atomic visibility.** A snapshot does not exist for other nodes until one compare-and-swap on the ref. A crash before it leaves no trace, and the existing commit protocol and its crash matrix apply unchanged.
- **Retention** is ref deletion plus `zfs destroy` locally. Garbage collection reclaims what no ref reaches.
- **Recovery point.** Between published snapshots the newest cut in object storage is the recovery point. With a local log device, a cut holds the last synced transaction group and not the fsynced tail (§6.1). That is accepted through M4 (decision D2). §5.5 sketches how the tail is recovered later.

### 5.3 What is uploaded

Most of what ZFS writes is dead within a minute: in the study 2 to 6 MB per thousand transactions was still live at the end of a window, against 12 to 19 MB written[^study]. A cut therefore uploads only blocks that are live **at the cut's position**:

- blocks overwritten before the cut are superseded in the extent tree,
- blocks ZFS freed before the cut arrive as discards, with `autotrim=on`, and are dropped; a free extent under 32 KiB never arrives, because autotrim skips it[^trimmin],
- intent-log blocks normally do not reach the device, because the intent log is on a local log device. ZFS falls back to the main device when the log device is full or has failed, and `logbias=throughput` and direct I/O log their data there by design.

Nothing after the cut may leak into it. Upload can lag the cut by minutes on a slow uplink, and in that time ZFS reuses uberblock slots and trims blocks that were live at the cut. The local store therefore keeps the cut-time version of every block until the cut is uploaded or abandoned. The engine's copy-on-write extent tree gives a cut its own root. The store must not reclaim what that root references. What bounds that space while the uplink is slow or absent, and what the daemon does at the bound, is not decided (§10).

### 5.4 Restore and clone

```mermaid
sequenceDiagram
    participant A as skyzfs-agent
    participant D as skyzfsd
    participant O as Object storage
    participant Z as ZFS

    A->>D: clone snap/t as new volume V2
    D->>O: new head ref for V2, pointing at the commit (constant time)
    D->>D: expose V2 as a block device, empty cache
    A->>D: offline and isolated: drop the log device from the pool, then give it a new GUID
    A->>Z: zpool import -f, by the new GUID, under a new name, from a configuration the agent wrote
    Z->>D: reads fetch lazily, with read-ahead
    A->>Z: add a local log device
    Note over A,Z: Mounted. Hydration continues in the background.
```

- **A new GUID before import.** ZFS refuses a pool whose GUID is already imported[^guid], and a clone must be importable on the node that still holds the original. `zpool reguid` only works on an imported pool, so the agent changes the GUID offline with `libzpool`. Which `libzpool` that is on Linux before M5, the distribution's shared library or one built from the pinned source, is open and is decided in M3; either way the tool that links it is a program of its own (§10, licensing). Only the pool's GUID changes: a clone keeps the original's vdev GUIDs[^reguidleaf], and nothing was found in ZFS that minds two imported pools whose leaves share one. M3's test on one host covers it. That is a real transaction: it writes labels and uberblocks, which the daemon sees as a cut on the new volume.
- **The log device goes first.** ZFS changes a pool's GUID only while the pool is healthy, and a pool whose log device cannot be opened is degraded[^reguid]. The cut does not contain the log device (§6.1). So the same offline session first removes the log device from the pool, and any cache or spare device the pool names, and only then changes the GUID. An earlier draft changed the GUID first and removed the log after the import, which ZFS would have refused. Removing a log device that is missing is the documented recovery after an import with `-m`. That it works in this session is for M3 to prove.
- **The offline session can open the clone and nothing else.** The copied labels and the pool's own configuration still name the source pool's log device, and may name cache and spare devices. On the node that holds the original, those paths lead to live devices that belong to the original pool and carry its GUID. Allowing a missing log does not protect them: ZFS counts a log device as missing only when it cannot be opened, and in `libzpool` a disk leaf is opened as a file by its path[^reguid]. A session that could open them would claim log blocks and write labels on the original's devices. Two things prevent it, and both are required. First, ZFS is never given a configuration the agent did not write: the agent reads the clone's labels through the daemon and replaces the path, physical path and device id of every leaf but the clone's data device with ones that cannot resolve. Second, the session runs where only the clone can be opened at all. With the frontend built into `libzpool` (M5) that is the frontend's own resolver, which knows the clone's name and fails every other path without touching the file system. Until then, on Linux, it is a private mount namespace in which the clone's device node is the only one. M3 tests this on one host with the original pool imported and writing to its log device, and checks that no device of the original was opened or written.
- **Labels are read through the daemon.** `zpool import` normally scans device nodes from user space. The agent builds the import configuration from labels it reads through the daemon's client interface (§4.1), which also works where there is no device node.
- **`-f`, and no `-m`.** The cut is of a pool that was imported elsewhere. Its log device was removed in the offline session, so the kernel import has nothing to miss and nothing of the original's to find.
- **Read-only browsing.** On the node that holds the pool, a snapshot is browsed through ZFS's own `.zfs/snapshot` directory, with no import at all. Anywhere else, browsing is a throwaway clone: the daemon serves the commit under a local overlay that is never uploaded, the agent gives the pool a new GUID as above, and the pool is imported with `readonly=on`, which skips log replay. The new GUID is needed even though nothing will be written. ZFS refuses any kernel import whose pool GUID is already open, read-only or not[^guid]. Without it a node could not browse a snapshot of a pool it has imported, or two snapshots of one pool at once.

### 5.5 Later: streaming the intent log

Through M4 a node loss costs everything since the newest uploaded cut. A reboot or a daemon crash costs nothing, because the log device is local and survives both. A later milestone (M9) closes the gap for tiers that need it, by shipping the intent log off the node continuously.

The sketch, to be designed in its own document before M9 starts:

- **The log device moves onto the daemon** as a second block target. Its writes go to the same local write log as the data vdev's, in one order.
- **The daemon uploads that ordered log as it is written**, as short-lived tail objects, in addition to the cuts.
- **A restore point is then any flush boundary in the stream.** Both devices as of one flush are exactly what a power cut would have left. ZFS imports that image and replays the intent log, which recovers every transaction fsynced before that point.
- **Tail objects are garbage once a later cut is published**, so they cost upload bandwidth but little storage.

Why both devices must come from the same point: the pool at a cut points at the start of a log chain, and ZFS frees and reuses log blocks once their transaction group has synced. A log device image taken much later than the cut no longer holds that chain, and replay would stop at the first block that does not belong. Pairing a cut with an arbitrary later log image is therefore not safe. Restoring both to one flush boundary is.

Open for that design: how much the stream adds to upload volume (intent-log blocks were three quarters of device writes without a separate log[^study]), whether a flush may wait for the stream to bound the loss to zero, and how the stream is throttled on a slow uplink.

## 6. The ZFS layer

### 6.1 Pool layout

One pool per protected volume: one data vdev on the transport, and one log device on local flash.

- **The log device is allocated storage.** A partition, or a file written through once. On a sparse file a lone commit took 1.3 to 2.4 times as long as the dm-thin stack's, against between the same and 1.3 times once the file had been written through[^study].
- **The log device is local on purpose.** It keeps the intent log out of the upload stream. The price is that a node loss drops what was fsynced since the newest uploaded cut. That fits a product whose promise is minute-level snapshots, it is accepted through M4 (decision D2), and it must be stated to users. M9 removes it for the tiers that need more (§5.5).

### 6.2 Portability rules

A pool must import on all three systems, so pools are created the same way everywhere.

| Rule | Reason |
|---|---|
| `compatibility=` pinned to one published feature set, the oldest of the three ports' | The ports track upstream at different speeds. |
| Leaf vdev of type `disk` | §4.3. |
| No partition table: the pool's first label is at byte 0 of the device | Cut detection finds the labels by position, and the layout must be the same on every system. |
| `ashift=12` | One block size for every transport. |
| `multihost=off` | It rewrites uberblocks outside a sync. |
| `failmode=wait`, which is the default | With `continue`, a read on a pool whose device has failed returns an error to the application, which §4.1 promises does not happen. |
| `xattr`, `normalization`, `casesensitivity` and `acltype` set explicitly at creation, to one set of values for all three systems | Most cannot change later, and the defaults differ between the systems. |
| ZFS native encryption on by default | The object store, and the vendor, see ciphertext for file contents and for most metadata. ZFS does not encrypt the pool's structure: dataset and snapshot names, the hierarchy, properties, file sizes and holes stay in clear[^encclear]. Keys stay with the user. |
| No pool checkpoint (`zpool checkpoint`) | While one exists ZFS refuses the GUID change and the removal of the log device that a clone needs (§5.4)[^ckptrefuse]. The agent takes none and reports a pool that has one: a cut taken meanwhile cannot be cloned or browsed. Whether the offline session should discard the checkpoint instead is open (M3). |
| `zfs_nocacheflush=0`, checked by the agent | With it set, the Linux disk vdev completes a flush without sending it[^nocacheflush]: no durability point and no cut. The parameter is one value for the whole module, so the agent refuses to serve any pool while it is set. |

File semantics do not port perfectly. ACL models, extended attributes and case rules differ between the three systems. The promise for a cross-OS restore is "your files and their contents", not every attribute. Same-OS restore is exact.

### 6.3 Sizing dirty data

ZFS's defaults assume a large pool and an uncapped ARC. A pool per volume is often small, and the study found two cliffs where writers wait out a whole sync[^study]:

- dirty data times 24 (`spa_asize_inflation`) must fit in the pool's free space,
- dirty data must stay under half the ARC.

`zfs_dirty_data_max` defaults to 10% of RAM regardless of either, and is one value for the whole module. The agent therefore sets it from the smallest pool and the ARC cap on the machine. Smaller transaction groups cost flash writes, 27% more in the one configuration measured, so the value is a trade to tune in M4 and not a constant.

## 7. Platform matrix

| | Linux | macOS | Windows |
|---|---|---|---|
| ZFS | Distribution packages, unpatched | OpenZFS on OS X port, patched with the frontend | OpenZFS on Windows port, patched with the frontend |
| Transport | ublk. NBD as fallback | `udrive_shm` under the LDI | `udrive_shm` under the disk vdev |
| Kernel code we ship | None | The ZFS kext with our patch | The ZFS driver with our patch |
| Install barrier | A package | Apple silicon: the user must lower the security policy to Reduced Security in Recovery and approve the extension[^applekext] | A signed driver. Microsoft requires an EV certificate and attestation signing through the Hardware Dev Center[^winsign] |
| Daemon | systemd service, io_uring | launchd daemon | Windows service |

**The macOS barrier is the largest risk in this design.** Apple discourages kernel extensions and is moving filesystems to user space with FSKit[^applekext][^fskit]. A ZFS kext needs the reduced-security step whatever we add to it, so the frontend adds no new barrier, but the barrier is real for a wide audience. The first releases accept it (decision D4): 1.x on macOS is for developers and enthusiasts, who can take that step. A client without kernel code follows in 2.x, on FSKit (§8).

## 8. Alternatives considered

| Alternative | Verdict |
|---|---|
| **A new vdev type** talking to the daemon | Rejected. Breaks import on stock ZFS (§4.3). |
| **A generic userspace block device per OS**, with the ports unpatched: DriverKit on macOS, WNBD or WinSpd on Windows[^wnbd] | Kept as the fallback for M6 and M7. It avoids carrying patches, and the device would serve other filesystems too. It adds a second kernel component to install and sign, puts a SCSI or block layer between ZFS and the daemon, and on macOS depends on an Apple entitlement that has not been investigated yet. |
| **A Linux VM on the Mac or PC** running the Linux stack, with files shared to the host | Rejected as the product, kept as a test vehicle. One kernel codebase and no extensions, but the files are not native to the host. |
| **ZFS in user space** behind FSKit on macOS and WinFsp on Windows, with the vdev in the daemon's own process | Later, for 2.x (decision D4). No kernel code at all, and the block target contract already allows an in-process transport. It needs the ZFS POSIX layer outside the kernel, which upstream does not provide, so the plan is to follow the macOS port of OpenZFS when it moves to FSKit and not to build that layer here. The daemon, the contract and the frontend's common code as built into `libzpool` (M5) carry over. |
| **Stay on dm-thin** | Rejected for this product. Linux only, and the snapshot cost is structural. |
| **An object-store vdev inside ZFS**, as Delphix presented in 2021[^delphix] | Not available upstream. Worth reading for its agent protocol and restart handling before M5. It ties the pool format to the object store, where this design keeps ZFS unmodified above a block contract. |

## 9. Roadmap

```mermaid
flowchart LR
  M0["M0 I/O model<br/>done"] --> M1["M1 Linux prototype<br/>ublk, local store"]
  M1 --> M2["M2 Cuts, upload,<br/>visible snapshots"]
  M2 --> M3["M3 Restore and<br/>clone anywhere"]
  M3 --> M4["M4 Efficiency and<br/>hardening"]
  M2 --> M5["M5 udrive_shm<br/>on Linux and libzpool"]
  M5 --> M6["M6 macOS client"]
  M5 --> M7["M7 Windows client"]
  M4 --> M8["M8 Product beta"]
  M4 --> M9["M9 Streaming<br/>intent-log backup"]
  M6 --> M8
  M7 --> M8
  M8 -.-> H["H After 1.x: ZFS in user space<br/>FSKit, with the macOS port"]
```

Sizes are relative: S is weeks, M is one to two months, L is a quarter, for a small team. They are estimates to be replaced after M1.

| Milestone | Size | Deliverables | Exit criteria |
|---|---|---|---|
| **M0** I/O model | Done | Topology benchmark, study §10 | Throughput level with twovol. Snapshots free. |
| **M1** Linux prototype | M | `skyzfs-blk` with the conformance suite. ublk transport. Local store with write log and home image (the whole volume; no cache and no eviction yet), no upload. `skyzfsd` serving one volume. | Pool creates, scrubs clean, and survives `kill -9` of the daemon under load with no error counted on the vdev (ZFS retries a lost request, so a server that loses them shows nowhere else). Power-cut replay: the pool imports at every record prefix of the log, not only at flush boundaries; at the first and the last prefix of every transaction group every block the newest uberblock reaches is present in every copy with the checksum its pointer records; and where the intent log is on the device under test, every file whose `fsync` had returned is intact at the record where it returned. An import alone does not show that: ZFS keeps two or three copies of pool metadata and reads a good one. tpcb within 10% of the benchmark model on the same machine. |
| **M2** Cuts and visibility | M | Uberblock detection, cuts, background upload, commit and ref swap, publish handshake in the agent. | A snapshot on node A is listed on node B once its ref is swapped, and imports read-only there with snapshot *t* present and holding the source's content, and with every block the pool reaches verified (`zdb -cc` on the read-only import, or a scrub of a throwaway clone: ZFS refuses a scrub on a read-only pool[^scrubro]). A clean pool alone would also pass a cut published one transaction group early, which lacks the snapshot. A snapshot every minute changes p99.9 by no more than run-to-run noise. The crash matrix passes at every step of publish. |
| **M3** Restore and clone | M | Lazy fetch with read-ahead, clone from a ref, offline GUID rewrite, import flow, throwaway clones for browsing, background hydration, both cache policies. | A restored volume mounts in under 30 s whatever its size. At a 10 ms origin tpcb reaches half its hydrated throughput within 5 minutes. A clone imports on the node that holds the original while the original is imported and writing to its log device, and no device of the original is opened or written by the clone operation. Ten minutes without object storage, under load with the `cache` policy, produce no application error and no pool error, and work resumes when storage returns. |
| **M4** Efficiency and hardening | L | Discard tracking, live-only upload, garbage collection and retention, encryption by default, dirty-data sizing in the agent, second object API, many pools on one node, metrics. | Bytes uploaded per cut within 1.5 times the live bytes ZFS reports. Seven-day soak with a snapshot every minute and no leak in store or log. Twenty pools on one node without a stalled second. |
| **M5** `udrive_shm` | M | Protocol specification. `skyzfs-shm`. Common C frontend built into `libzpool` and into a patched Linux module. | The conformance suite passes on ublk and on `udrive_shm`. `ztest` runs 24 h against the daemon. Daemon restart recovery works. tpcb within 10% of ublk. |
| **M6** macOS client, alpha | L | LDI provider in the port. Daemon on launchd. Local log and cache on APFS. Installer with the extension approval flow. Keychain for keys. | Mac snapshot restores on Linux and Linux snapshot restores on the Mac, verified by checksum. Survives sleep and wake, network loss and daemon restart. Commit latency measured with `F_FULLFSYNC`, and with `F_BARRIERFSYNC` only to price the weaker class of §5.1. |
| **M7** Windows client, alpha | L | Path dispatch in the port's disk vdev. Daemon as a service. Signed driver. | The same cross-OS restore pairs with Windows. Loads with memory integrity enabled. |
| **M8** Product beta | L | Client UI, snapshot browser, policies, two object-store targets at once, fleet view for small businesses, security review. | External beta users restore without help. |
| **M9** Streaming intent-log backup | M | Design document. Log device served by the daemon. Ordered tail upload. Restore to any flush boundary with log replay. | A node killed at a random moment under pgbench restores with every transaction that was acknowledged more than a stated lag before the kill, checked against the client's own record. The lag bound is a number in the design document. |
| **H** ZFS in user space on macOS, after 1.x | Not sized | Deferred by decision D4, and no longer a hedge for M6. A macOS client on FSKit, built on the macOS port once that supports FSKit, with the daemon and the `udrive_shm` code of M5 reused. | Planned once 1.x has shipped and the port's FSKit work exists to build on. |

**Where M1 stands (2026-11-12).** M1 still lacks two of its exit criteria. A smaller item is open beside them: the ublk feature flags and the kernels that have them are not pinned (§4.2).

**The power-cut replay has not been run on `skyzfsd`.** The replay rig that this criterion rests on has so far run against the ublk project's reference server. What the daemon's own log does at a power cut is covered by the store's tests on a simulated disk, and what it does at `kill -9` by the daemon rig on the kernel; neither is the criterion. Running the rig on the daemon is the next testing package.

**The throughput criterion is not met.**

- **The rule**, fixed before the data. A daemon row's ratio is its tps over the mean of the two model rows that enclose it in one job, on one machine. The criterion is met when, for each kind of machine and for each row (unbounded, and bounded to 1 GiB), the median ratio is at least 0.90 over at least four rows from at least two jobs on the merged daemon. A row counts only if its two model rows differ by at most 10% of their mean.
- **On machines whose disk is slow to sync** it is not met: the median is 0.876 unbounded and 0.839 bounded to 1 GiB, each over five rows from three jobs.
- **On machines with a fast sync**, which is what node-local flash is like, earlier rows were at 0.92 to 0.98. None has been measured under the rule yet.
- **What it is not.** The copy per request is not the cause. Reads waiting behind the worker, the worker's own syncs and the status thread were each measured, taken out of the way (M1.3j to M1.3n), and are no longer the cause either. Postgres waits beside the daemon and does not work harder.
- **What is left** is not one defect. The daemon writes every block twice, to its log and later to the home image (about 6.5 KiB more per transaction on the disk); its threads use 17 to 25 s of CPU in a window of two minutes, two thirds of it in the kernel, on a machine whose CPUs are all busy; and the machine's I/O pressure is twice the model's, which the ublk project's reference server shows as well. A profile (M1.T15) puts that kernel time in waking and switching threads and not in copying. Waking one reader per read in place of all four (M1.3p) took a third off the readers' CPU and left the throughput where it was: one row pair on one machine, both at 0.889. The daemon's own CPU is a tenth of the gap at most.
- **No sync on the CI runners costs a device flush.** Their work disk is mounted without barriers, so every ratio compares like with like, and none says what a flush costs on real flash. "Slow to sync" above means slow to write back, not slow to flush.
- **The instrument is at its limit on these machines.** One build of the daemon gave 0.718 and 0.936 in one job, and the reference server, which has no store, ran between the model's own rows and well below them.
- **Open, and the user's to decide:** whether the criterion should be judged on fast-sync machines, with the slow-sync gap carried as a recorded risk (§10).

The measurements, job by job, and the decisions taken from them are in the notes, M1, "The throughput criterion".

Decision gates:

| Gate | After | Question |
|---|---|---|
| G1 | M1 | Does ublk with one copy meet the model's numbers? If not, adopt the zero-copy modes before M2. **Measured 2026-11-12: the criterion is not met on slow-sync machines (above), and the copy is not the cause (notes, M1); the zero-copy modes are not adopted for it.** |
| G2 | M3 | Is cold restore good enough for the server case? If not, the read cache gets a milestone of its own before M4. |
| G3 | M5 | Carry patches on the two ports, or switch M6 and M7 to a generic block device? Decided by the measured size and rebase cost of the frontend. |
| G5 | M6 and M7 in alpha | Is the frontend stable enough to offer to the port maintainers (decision D3)? |
| G4 | Closed by decision D4 | Was: is a kernel extension still acceptable on current macOS for this audience? Yes for 1.x, whose macOS audience is developers and enthusiasts. Asked again when 2.x is planned. |

## 10. Risks and open questions

| Risk | Consequence | Mitigation |
|---|---|---|
| Apple removes or further restricts kernel extensions | No native macOS client until the FSKit client of 2.x | Accepted for 1.x (decision D4). The engine and the contract do not depend on the transport, and most of `udrive_shm` carries over to a client on FSKit. If Apple closes the door before 2.x exists, the macOS client waits for it. |
| Rebase cost of patches on two ports that follow upstream | Ongoing maintenance, carried alone until gate G5 | Keep the hook minimal and below the disk vdev. Rebase in CI against both ports' development branches from M5 on, so drift shows early. Gates G3 and G5. |
| Driver signing and distribution on Windows | Delay, and a dependency on Microsoft's process | Start the EV certificate and Hardware Dev Center account during M5. |
| A write log that fails under an imported pool leaves the pool suspended, and nothing short of a reboot was found that removes it (§4.2, measured on a stand-in) | The node must be rebooted before that volume's pool can be imported again. Its other pools keep running until then | The log is on local storage that the node already depends on. The failure is announced, the volume is refused until the reboot, and nothing else on the node waits on the suspended pool. Unmounting first, `zpool export -f` and `zpool destroy -f` were tried and do not remove it. |
| Daemon and kernel deadlock under memory pressure | Hung machine | Locked memory, daemon files off its own pools, a stress test in the kernel rig. The aim of an I/O path that allocates nothing is not met today: the store writes through the page cache (§5.1), which allocates in the kernel on every write, and the daemon does not set `PR_SET_IO_FLUSHER`. Neither has been shown to hang. To be settled in M4's hardening: direct I/O for the store's files or the flag, and an audit of the daemon's own allocations. |
| A call into the local store hangs and neither returns nor fails, as a write or a sync on a sick disk can | The requests behind it are held without bound, the volume is never declared failed, and an orderly stop waits with them | Open. Two places for a bound are known, each with a cost: an orderly stop leaves what the store holds to the next instance after a stated time, or the store declares the volume failed when a call has not returned in a stated time, which fails a volume for a disk that was only slow. Decided when a disk that hangs has been produced on the rig (notes, M1, "Risks in detail") |
| A pool that only writes does not notice that its device is gone: ZFS retries failed writes without end and suspends only at a failed read (§4.2) | The pool stays ONLINE for minutes over a deleted device and, with its log device, goes on acknowledging synchronous writes that no transaction group can carry. A node lost in that state loses all of it, beyond the bound D2 states | The daemon says it in every message of a failure or a refusal and in `skyzfsd status`. The agent makes the pool read, which suspends it within a second or two, and reports it lost for this boot (M2); until then that is the operator's step |
| The M1 store creates log segments on demand and fails the volume when its file system fills during a checkpoint | A full disk is found on the commit path, and a condition that could recover ends the volume, against §4.1 | Preallocate the log and hold on a full file system before a pool outside the test rigs is served from it. Until then both are stated limits of `skyzfs-local`. The cost of a checkpoint in batches, a superblock write and a sync per batch on platforms with write units above 4 KiB, is measured at gate G1. |
| The daemon does not reach the model's throughput on machines whose disk takes its time over a sync (§9) | M1 does not exit on its throughput criterion | Reads, the worker's syncs and the status thread have been taken out of the way (M1.3j to M1.3n), and the median stands at 0.876 unbounded and 0.839 at 1 GiB against 0.90 on such machines. What is left is not a wait inside the daemon, and its own CPU is a tenth of the gap at most (M1.T15). Fast-sync machines, which are what node-local flash is like, have been at 0.92 to 0.98; whether the criterion should be judged there is the user's decision and is open |
| Flush cost on real flash, above all `F_FULLFSYNC` on macOS. Nothing measured so far includes one: the CI runners' work disk is mounted without barriers (§9) | Slow commits, and a daemon-to-model ratio that differs from the runners' | Measure on Linux with barriers on before M4 exits, and in M6. The log device is the only place that pays it per commit. Whether to offer an ordering-only class is a product decision taken on those numbers (§5.1). |
| Many small pools on one machine sharing one ARC and one dirty-data limit | Stalls from a noisy neighbour | Measured in M4. |
| One workload behind the evidence: pgbench, 1.5 GiB, very compressible, fresh pool | Results may not hold for large or aged pools | Add a database larger than RAM and an aged pool to the benchmark before M4 exits. |
| Licensing | The frontend lives in CDDL code. The daemon is FSL. One dependency of the ublk transport is MPL-2.0 (`bitmaps`, through `libublk`) | Publish the frontend under CDDL and the protocol header under a permissive licence, and link no daemon code into the frontend module (§4.3). A preliminary check found the split sound on that condition. It is not a lawyer's opinion, and one is due before the frontend is distributed. Every binary of the daemon that leaves the project carries a NOTICES file for the MPL-2.0 crate, and a check of the dependencies' licences belongs in CI before the first one does (notes, M1, "Risks in detail"). The agent's offline tool that links `libzpool` (§5.4) is code linked with CDDL code as well; it falls under the same rule and the same review. |
| ZFS trims late and not everything: freed blocks reach the device as discards at least two transaction groups after the free, batched over 32, and a free extent under 32 KiB is never trimmed[^trimmin] | A cut carries blocks ZFS already considers free, so uploads exceed the live bytes | Measure in M4 against its upload criterion of 1.5 times the live bytes. Options: a smaller `zfs_trim_txg_batch` and `zfs_trim_extent_bytes_min`, or a second source of liveness. |
| Upload lags or the uplink is absent: the store keeps the cut-time version of every block of every cut not yet uploaded (§5.3) | Local flash fills beyond the volume's size, and nothing states a bound | Open, to be decided in M2's design before its first package: a bound on what is retained, and what happens at the bound. Either the oldest unpublished cut is abandoned, so the recovery point moves and the user is told, or writes are held. |
| twovol's 2 s stalls in the benchmark are unexplained | The far-tail comparison may flatter ZFS without snapshots | Does not affect the snapshot result. Listed in the study as open. |

### 10.1 Decisions

D1 to D3 were recorded on 2026-11-10, D4 on 2026-11-11.

| # | Question | Decision |
|---|---|---|
| D1 | What does a Mac or Windows user see: a ZFS dataset, or a zvol formatted natively? | **Datasets.** Snapshots stay freeze-free and atomic across the pool. zvols remain a non-goal. |
| D2 | Is losing the fsynced tail on node loss acceptable? | **Yes for M1, and through M4.** The log device stays local. Streaming backup of the intent log comes later as M9 (§5.5). |
| D3 | Offer the frontend to the port maintainers from the start? | **No.** They are better served by a stable proof of concept that is likely to stick. The frontend is developed out of tree and offered at gate G5. |
| D4 | Is a kernel extension acceptable for the macOS client? | **Yes for 1.x.** The first releases on macOS are for developers and enthusiasts and ship the ZFS kernel extension with the frontend. A client on FSKit comes later, in 2.x, when the macOS port of OpenZFS supports it. Most of the code, the `udrive_shm` frontend and the daemon, is reused. This closes gate G4 and moves the userspace work (H) behind 1.x. |

### 10.2 Review record

Every review of this design and of each work package, with its outcome, is in the notes under "Review record".

## References

[^study]: `docs/skynbd-postgres-study.md` §10, in particular §10.7 for the same-machine comparison, the dirty-data limits and the snapshot runs, and §10.3 for what reaches the vdev.
[^d1]: `docs/skynbd_design.md` §2, decision D1: flush-bounded durability with no local write-ahead log.
[^ublk]: Linux kernel documentation, "Userspace block device driver (ublk driver)", <https://docs.kernel.org/block/ublk.html>. Feature flags are defined in `include/uapi/linux/ublk_cmd.h`.
[^ports]: `openzfsonosx/openzfs-fork` and `openzfsonwindows/openzfs`, branch `development`, `META` files as cloned on 2026-11-10. Upstream `openzfs/zfs` carries `module/os/linux` and `module/os/freebsd` only.
[^vdevtable]: `module/zfs/vdev.c`, `vdev_ops_table`, identical in upstream and in both ports.
[^vdevfile]: `module/zfs/vdev.c`, `vdev_alloc`: an unknown type returns `EINVAL`. `module/zfs/vdev_file.c`, `vdev_file_open`: "Make sure it's a regular file".
[^ldi]: `openzfsonosx/openzfs-fork`, `module/os/macos/zfs/vdev_disk.c` and `include/os/macos/zfs/sys/ldi_impl_osx.h` (`LDI_TYPE_IOKIT`, `LDI_TYPE_VNODE`).
[^winvdev]: `openzfsonwindows/openzfs`, `module/os/windows/zfs/vdev_disk.c` (`ZwCreateFile`, `IoBuildAsynchronousFsdRequest`).
[^zfserr]: `module/zfs/zio.c`: a flush that returns `ENOTSUP` sets `vdev_nowritecache`, and `zio_flush` is issued with `ZIO_FLAG_DONT_PROPAGATE`. `module/zfs/vdev_label.c`: `vdev_config_sync` discards the result of the flush before the labels, and `vdev_uberblock_sync_list` that of the flush after the uberblocks. Read in upstream master on 2026-11-10. Observed once on ZFS 2.4.1 as packaged for Ubuntu 26.04: with a fault injected into every flush, `zpool sync` returned success, the pool stayed online with no errors counted, and ZFS went on issuing flushes[^probe].
[^libublk]: `libublk`, <https://github.com/ublk-org/libublk-rs>, version 0.4.8 as published on crates.io, MIT or Apache-2.0. Read on 2026-11-11: `src/ctrl.rs` (recovery, quiesce; `ublk_ctrl_need_retry` and `ublk_ctrl_cmd`, which return the result of a retry with the legacy opcode; `Drop for UblkCtrlInner`, which deletes the device of an object made for an add and not disowned), `src/io.rs` (queue handling, completion from another context, the buffer's address given at the fetch and at each completion), and the build script of `libublk-rs-sys`.
[^probe]: Workflow `skyzfs-kernel-probe`, runs of 2026-11-10: <https://github.com/skyzfs/skyzfs/actions/runs/42923293696> and, for the repeated kills, <https://github.com/skyzfs/skyzfs/actions/runs/42925564790>. The probe is `tests/kernel/ublk-probe.sh` on the `zfs-topology` branch, and the benchmark rows are `zfs-ublk` and `zfs-loop` in `tests/bench/topology-bench.sh` (runs 42919169910, 42919174435 and 42924784572).
[^failrig]: `tests/kernel/daemon-rig.sh` and `failed-volume-obs.sh`, workflows `skyzfs-daemon-rig` and `skyzfs-failed-volume`, work package M1.T4 (skyzfs/skyzfs#11), whose description has every figure with its run. Runs of 2026-11-11 on GitHub's Ubuntu 24.04 (kernel 7.0.0-1017-azure) and 26.04 (7.0.0-1019-azure) images, ZFS 2.4.1, each image on its own runner, one run per image: the daemon rig, <https://github.com/skyzfs/skyzfs/actions/runs/42972177917> and <https://github.com/skyzfs/skyzfs/actions/runs/42980469527>; a device deleted under an imported pool, <https://github.com/skyzfs/skyzfs/actions/runs/42971643028>; syncing beside a suspended pool, and `zpool export -f` and `zpool destroy -f` on one, <https://github.com/skyzfs/skyzfs/actions/runs/42975781582> (26.04 only), <https://github.com/skyzfs/skyzfs/actions/runs/42979323211>, <https://github.com/skyzfs/skyzfs/actions/runs/42985731155> (zed, `sync()` of everything) and <https://github.com/skyzfs/skyzfs/actions/runs/42986104286> (unmounting first); the green runs of the whole daemon rig, with the older-content observation, <https://github.com/skyzfs/skyzfs/actions/runs/42988261841> and <https://github.com/skyzfs/skyzfs/actions/runs/42992148479>. On the 24.04 image the ZFS commands are 2.2.2 beside the 2.4.1 kernel module. The server is a stand-in for `skyzfsd` built around the ublk project's reference server. Several of these runs ended red for faults of the rig that were then fixed, and one was cancelled part way; the figures quoted are from the checks that ran. The review of the rig checked the figures of the kill test, the held read, the kept device and the refusals against one image's logs, and corrected one: the rig's first figure for how long a killed server's device stays live was the length of one look, not that time.
[^tear]: The daemon rig's stand-in server, the ublk project's reference server over a file opened for direct I/O, in the first run of work package M1.T6: <https://github.com/skyzfs/skyzfs/actions/runs/43010288006>, Ubuntu 26.04. Two writes of blocks 12 to 15 and a write-zeroes of 13 to 14 were submitted in the same millisecond; after all three had returned, block 12 held one write's data and block 15 the other's. Seen once, in one of two image-runs with a commit delay and in none of about thirty without. The conformance suite's case for it is `overlapping_operations_in_flight_apply_whole`; its model lets a read in flight with a write mix per block (`model.rs`, rule R3).
[^readerr]: `module/zfs/zio.c`, `zio_done`: a failed read is marked for suspension only when its error is `ENXIO`. Any other error on a zio issued with `ZIO_FLAG_CANFAIL`, which demand reads are (`module/zfs/dbuf.c`), is returned to the caller. `zio_vdev_io_assess` turns a leaf error into `ENXIO` only once the vdev is no longer accessible. Slow I/O is reported after `zfs_deadman_ziotime_ms` (`module/zfs/spa_misc.c`), and with the default `zfs_deadman_failmode=wait` it is only reported. Read in upstream master on 2026-11-10.
[^wholedisk]: `lib/libzutil/os/linux/zutil_device_path_os.c`, `zfs_dev_is_whole_disk`, and `lib/libefi/rdwr_efi.c`, `efi_get_info`: device names are matched against a fixed list.
[^cutproof]: `module/zfs/spa.c`, `spa_sync`; `module/zfs/vdev_label.c`, `vdev_config_sync` and `vdev_uberblock_sync_list`; `module/zfs/metaslab.c`, `metaslab_sync_done` (deferred frees and the trim tree); `module/zfs/zio.c`, `zio_alloc_zil` (the fallback of intent-log blocks to the main device). Read in upstream master on 2026-11-10. The macOS and Windows port branches are still to be checked for the same order; that is due before M6 and M7 start.
[^txgorder]: `module/zfs/vdev_label.c`, `vdev_config_sync`: flush every written vdev, sync the even labels, write the uberblocks, flush, then sync the odd labels and flush.
[^abspath]: `openzfsonosx/openzfs-fork`, `module/os/macos/zfs/vdev_disk.c`, `vdev_disk_open`: "We must have a pathname, and it must be absolute", checked before `ldi_open_by_name`; `module/os/macos/zfs/ldi_osx.c`, `ldi_open_by_name`: IOKit media by path, then a device number from a vnode lookup, then a vnode. `openzfsonwindows/openzfs`, `module/os/windows/zfs/vdev_disk.c`, `vdev_disk_open`: the path must begin with `/`, `#` or `\`, and `vdev_physpath` is opened in preference to `vdev_path` when it is set. Branch `development` of both, read on 2026-11-11.
[^reguid]: `module/zfs/spa.c`, `spa_change_guid_check`: `ENXIO` unless the root vdev is healthy. `module/zfs/vdev.c`, `vdev_propagate_state`: a top-level log device that cannot be opened makes the root degraded. `module/zfs/spa.c`, `spa_check_for_missing_logs`: with `ZFS_IMPORT_MISSING_LOG`, a log device is missing only in the state `CANT_OPEN`. `module/zfs/vdev_file.c`: in a userland build `vdev_disk_ops` is the file vdev's, so a disk leaf is opened as a file by its path. Read in the macOS port's tree at 2.4.3-rc3 on 2026-11-11; this is common code.
[^guid]: `module/zfs/spa.c`, `spa_ld_parse_config`: an import fails with `EEXIST` when `spa_guid_exists` finds the pool GUID already open. In the kernel there is no exception for a read-only import. The only exception, for a read-only import of a checkpoint, is compiled for userland tools.
[^barrier]: Apple, `fcntl(2)`, `F_BARRIERFSYNC`: "no assumption should be made on what has been persisted or not when this call returns", and "typically useful to guarantee valid state on disk when ordering is a concern but durability is not". <https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/fcntl.2>, read 2026-11-10.
[^applekext]: Apple Platform Security, "Securely extending the kernel in macOS", <https://support.apple.com/guide/security/sec8e454101b>, and "Change security settings on the startup disk of a Mac with Apple silicon", <https://support.apple.com/guide/mac-help/mchl768f7291>.
[^fskit]: Howard Oakley, "How macOS is moving away from kernel extensions", 2024, <https://eclecticlight.co/2024/07/24/how-macos-is-moving-away-from-kernel-extensions/>.
[^winsign]: Microsoft, "Driver code signing requirements", <https://learn.microsoft.com/windows-hardware/drivers/dashboard/code-signing-reqs>. To be re-read against the current page before M7.
[^wnbd]: WNBD, <https://github.com/ceph/wnbd>, and WinSpd, <https://github.com/winfsp/winspd>. Both present a userspace-served disk through a Storport virtual miniport.
[^delphix]: "ZFS on Object Storage", Wilson, Ahrens, Dagnelie and Joseph, OpenZFS Developer Summit 2021, <https://openzfs.org/wiki/OpenZFS_Developer_Summit_2021>.
[^scrubro]: `module/zfs/zfs_ioctl.c`: the scrub ioctl is registered with `POOL_CHECK_READONLY`, and the older scan ioctl as one that modifies the pool, so both are refused on a pool imported read-only.
[^trimmin]: `module/zfs/vdev_trim.c`, `zfs_trim_extent_bytes_min`, 32 KiB by default: smaller free extents "are considered too small to efficiently process" and are skipped by automatic and manual trim.
[^encclear]: `man/man8/zfs-load-key.8`, "Encryption": "ZFS will not encrypt metadata related to the pool structure, including dataset and snapshot names, dataset hierarchy, properties, file size, file holes, and deduplication tables".
[^ckptrefuse]: `module/zfs/spa.c`, `spa_change_guid_check`, and `module/zfs/vdev_removal.c`, the check before a removal: both return `ZFS_ERR_CHECKPOINT_EXISTS` or `ZFS_ERR_DISCARDING_CHECKPOINT` while the pool has a checkpoint.
[^reguidleaf]: `module/zfs/spa.c`, `spa_change_guid_sync`: it sets the root vdev's GUID, which is the pool's, and adjusts the GUID sum. No leaf's GUID changes.
[^nocacheflush]: `module/os/linux/zfs/vdev_disk.c`, `vdev_disk_io_start`, the flush case: "Flushing disabled by operator, declare success".
