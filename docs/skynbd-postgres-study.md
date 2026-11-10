# SkyNBD PostgreSQL Read-Cache Study

**Status**: study, not a decision record. It informs the origin-advance
design's §7.1 read cache (`skynbd-origin-advance-design.md`) and the
per-StorageClass cache work it recommends; nothing here changes the
shipped stack by itself. §10 explores a different stack (ZFS on a
userspace block backend) with the same benchmark. Companion benchmark:
`tests/bench/topology-bench.sh` and `.github/workflows/topology-bench.yml`.

---

## 1. The question

The §7.1 top-of-stack read cache is a writethrough dm-cache between the
filesystem and the thin shim. Writethrough is what makes it
disposable — never dirty, wipe it whole and nothing is lost — and it is
why a published-then-reclaimed block stays flash-local. The cost is
that every write to a cached block lands twice on local flash: once in
the pool, once in the cache carve. Both live on the node's SSD, so the
amplification is wear and bandwidth, not just latency.

Three questions followed from that, and this document records what was
measured for each:

1. Can a write-around cache (bcache, Open CAS) remove the second write
   while keeping clean thin deltas and restage warmth?
2. Does splitting PostgreSQL across two volumes — cached data, uncached
   WAL — capture most of the benefit without a new block layer?
3. What cache block size and cache size are reasonable defaults for a
   PostgreSQL OLTP data volume?

Everything below is measured on pgbench, which is a narrow and
write-heavy proxy for OLTP. The shapes carry; the absolutes do not.

## 2. Where the 2x actually comes from

dm-cache writethrough double-writes only on a write **hit**. A write
miss goes to the origin alone, and smq promotes in the background
later. But smq's hotspot tracking counts writes as hits, so the
write-hot set gets pulled onto flash and then pays the dual write on
every subsequent write. For PostgreSQL that set is the WAL segment
tail (rewritten on every commit fsync) and the checkpoint-rewritten
hot heap and index pages.

dm-cache has no write-around mode. Its passthrough mode invalidates on
write but never promotes on read, so it is not a substitute.

## 3. Write-around candidates

### 3.1 bcache writearound

Verified against the 6.12 source (`drivers/md/bcache/request.c`):

- In writearound mode every write bypasses the cache and goes to the
  backing device. The bypass path still inserts pointer-less
  invalidation keys covering the written range, so a cached copy can
  never go stale. Coherence therefore holds with the same argument
  §7.1 already makes: guest writes traverse the cache, agent-side
  mutations are content-preserving.
- Neither writearound nor writethrough ever holds dirty data. The cache
  stays disposable and `thin_delta` stays clean.
- Read misses promote from wherever they were served.

What weakens: a written block is invalidated rather than refreshed, so
its next read after a reclaim is one origin round trip before it is
flash-local again. That gap only affects blocks written and not re-read
before reclaim — exactly the blocks whose dual writes would be saved.

Costs to design around:

- bcache's backing superblock lives at the start of the backing device
  and the bcache device is the backing minus `data_offset`. Putting it
  on the shim as-is would publish a bcache signature into every
  generation and let any host attaching the RO export auto-register
  it. The benchmark uses a two-segment linear — 8 KiB of local carve,
  then the thin — so the bcache device is byte-for-byte the thin and
  nothing enters the thin's address space. Every swing/retarget path
  would have to handle a two-segment shim.
- Lifecycle is sysfs, not dm ioctls: register, attach, asynchronous
  stop, udev auto-registration to disable. The dm mock, the build-order
  list `observe` walks, and presence-keyed teardown all need a sibling.
- Defaults are wrong for a remote origin: `sequential_cutoff` (4 MiB)
  and the congested thresholds (2 ms read, 20 ms write) make large or
  slow reads bypass the cache. All three must be zeroed.
- Every bypassed write journals invalidation keys on the cache device,
  and a flush forces a journal write. Small, not free.
- No online backing-device resize.

### 3.2 Open CAS

Open CAS Linux is actively maintained (26.03.x line, releases in July
and August 2026, kernel support stated up to 7.0), builds out of tree,
and its Debian packaging produces a DKMS modules package. It writes
nothing to the core device, supports many cores per cache with a
hidden per-core purge, and switches modes at runtime. Two things
matter for this question:

- **Its "write-around" mode is not write-around.** The engine
  (`ocf/src/engine/engine_wa.c`) calls the write-through engine on a
  hit and the write-invalidate engine on a miss; the guide says the
  same. So a cached hot block is still double-written. WA only avoids
  allocating cold writes.
- **The mode that is write-around exists but is compile-gated.** OCF's
  write-invalidate mode pairs allocating reads with invalidate-on-write,
  which is bcache's semantics. casadm only lists it when built with
  `make CAS_EXT_EXP=1` (`casadm/Makefile` gates the `wi` entry behind
  `WI_AVAILABLE`); the kernel module accepts the mode unconditionally.
  The benchmark builds it that way.

Practical gotchas found by running it: casadm refuses any device path
that is not a `/dev/disk/by-id` or `/dev/disk/by-path` link (loop
devices have neither; a dm-linear wrapper gets a `dm-name-*` link),
and the sequential-cutoff flag names differ between the master man
page and the v26.03.4 binary.

## 4. What PostgreSQL writes, by volume

Measured on PostgreSQL 16, pgbench scale 200 (3 GB database), 8
clients, 180 s windows, checkpoints every 60 s, data and WAL on two
separate loop-backed ext4 volumes so `/proc/diskstats` attributes bytes
per volume. "RAM capped" runs pinned memory so the page cache was about
2 GiB and reads reached the device.

| Run | tps | WAL vol writes | data vol writes | data vol reads | WAL share of writes |
|---|---|---|---|---|---|
| tpcb, shared_buffers 256 MB, RAM uncapped | 5079 | 9.8 GB | 8.2 GB | 0 | 54% |
| 90% select / 10% tpcb, uncapped | 25422 | 5.6 GB | 6.6 GB | 0 | 46% |
| tpcb, 256 MB, RAM capped | 4447 | 7.7 GB | 6.3 GB | 8.1 GB | 55% |
| select-only, RAM capped | 21456 | 0.3 GB | 0.5 GB | 31.7 GB | 40% |
| tpcb, shared_buffers 2 GB, RAM capped | 5633 | 9.7 GB | 5.7 GB | 0 | 63% |

WAL device writes run 35 to 50% above PostgreSQL's own `wal_bytes`
because every commit fsync rewrites the current partial 8 KiB WAL
page. That churn is exactly what makes WAL blocks look hot to smq.

Modeled flash write volume, upper bound with the hot set resident:

| Run | single volume, dm-cache writethrough | data cached + WAL uncached | single bcache writearound |
|---|---|---|---|
| tpcb 256 MB, capped | 28.0 GB | 20.3 GB (−27%) | 14.0 GB + re-promotion, up to 8.1 GB |
| tpcb 2 GB, capped | 30.7 GB | 21.0 GB (−32%) | 15.4 GB + about 0 |

The two-volume split captures the certain part of the saving. Write-
around saves a flash write only for blocks written and not read back
from the device before the next write. WAL is never read back, so both
options save all of it. Data pages split by regime: when the database
fits in RAM, write-around saves the whole data doubling; when it does
not, dirtied pages are re-read from the device and write-around pays a
promotion write per re-read, converging on what writethrough paid up
front — and writethrough is then the better read cache, because the
written copy stays resident instead of being invalidated.

Commit latency: dm-cache clones every flush to both the origin and the
cache device (`num_flush_bios = 2`), so each WAL fsync on a cached
volume is two device flushes. Moving WAL off the cache removes that
entirely.

## 5. Cache block size and cache size

Every relation read and write PostgreSQL issued under pgbench was
traced with strace and replayed through an LRU cache at each block
size, warming on the first half of each trace and measuring the
second. Skewed runs used gaussian and exponential key distributions
with a 32 MB `shared_buffers` so the hot set reached the block layer.
Reads were 8 KiB in every run.

Hit rate over the measured half, by cache size as a fraction of the
database:

| Workload | Cache | 8K | 32K | 64K | 128K | 512K |
|---|---|---|---|---|---|---|
| uniform select | 10% | 11.5 | 12.4 | 14.2 | 15.0 | 15.7 |
| uniform select | 25% | 34.8 | 34.6 | 34.3 | 35.3 | 36.5 |
| gaussian select | 25% | 74.0 | 73.9 | 73.8 | 73.7 | 73.6 |
| gaussian select | 50% | 90.0 | 97.8 | 97.8 | 97.8 | 97.8 |
| exponential select | 25% | 59.4 | 59.4 | 59.2 | 59.2 | 58.9 |
| tpcb | 25% | 32.8 | 33.1 | 34.3 | 35.8 | 37.0 |

Bytes promoted into the cache over the same window, gaussian workload
at 25% cache:

| Block | 8K | 32K | 64K | 128K | 512K |
|---|---|---|---|---|---|
| promoted | 0.69 GB | 2.78 GB | 5.59 GB | 11.2 GB | 45.1 GB |

Hit rate at fixed cache bytes is flat across block sizes for 8 KiB
random page access; promotion traffic grows linearly with the block.
Larger blocks buy nothing and cost flash writes on the same device the
pool lives on. Two other numbers from the tpcb trace: at 25% cache and
64 KiB blocks, 18% of data page writes were cache hits, so the
data-side writethrough amplification is about 1.2x rather than 2x; at
50% cache it was 47%.

Defaults this supports for a PostgreSQL data volume:

1. **Cache block 32 KiB**, the dm-cache minimum. The read cache today
   reuses the thin chunk (64 KiB, dm-thin's own floor), so this needs a
   separate read-cache block parameter. Scale the block up only to keep
   the chunk count near the 1,000,000 that lvmcache's defaults cap at
   (`DEFAULT_CACHE_POOL_MAX_CHUNKS`), which is 32 GiB of cache at
   32 KiB. smq costs about 25 bytes of RAM per block.
2. **Cache size 25% of the data volume, 1 GiB floor, as a StorageClass
   parameter.** For uniform access the hit rate equals the cached
   fraction; for skewed access 25% gave 60 to 74% and 50% gave 84 to
   98%. The chart's node-wide 256 MiB default is far below any of this.
3. **`migration_threshold` 16384 sectors (8 MiB)** instead of the
   kernel's 1 MiB of in-flight migration I/O, since promotion ran at
   over 200 MB/s even at 32 KiB.
4. **smq, writethrough, metadata format 2** as today.
5. PostgreSQL side: `io_combine_limit` defaults to 128 KiB in PG17, so
   sequential scans span four 32 KiB blocks cleanly;
   `effective_io_concurrency` defaults to 1 and should be raised;
   `random_page_cost` around 1.1 for flash.

Caveats: LRU stands in for smq, strace slowed PostgreSQL to roughly a
third of native throughput, and pgbench's tables are narrower than
most real schemas.

## 6. The topology benchmark

`tests/bench/topology-bench.sh` rebuilds the §5/§7.1 node shape from
loop files without skynbd in the loop: an immutable pgbench origin
image, a fresh dm-thin pool per topology whose thin has the origin as a
read-only external origin (so every unwritten block is an "origin
read", the stand-in for the network), and a cache carve. It runs
pgbench on each candidate and measures at the block layer: origin
reads, pool writes, cache device writes and reads, plus tps, latency
and PostgreSQL's own counters. Topologies:

| name | stack |
|---|---|
| nocache | fs → thin (the floor) |
| baseline | fs → dm-cache writethrough smq → thin (the shipped §7.1 stack) |
| twovol | baseline for PGDATA, plus an uncached thin for `pg_wal` with no external origin |
| twovol-origin | twovol, but the WAL thin sits on its own delayed origin image with nothing mapped locally: the WAL volume right after an advance or a restore |
| twovol-hydrated | twovol-origin, then every chunk pulled local with whole-chunk writes before postgres starts: the WAL volume that is snapshotted but never advanced, or hydrated on attach |
| bcache | fs → bcache writearound, cutoffs zeroed, over the two-segment backing → thin |
| opencas | fs → Open CAS write-invalidate (casadm built with `CAS_EXT_EXP=1`) → thin |

The workflow runs one job per topology (a runner VM that dies takes its
step log with it, and one did, from a full disk) and a fan-in job that
renders the combined table into the step summary. It is meant for
shape checks: the runner is a shared 2-vCPU VM with loop files on a
virtual disk. Real numbers come from running the script as root on a
workstation NVMe.

Things the first seven runs taught about running this on a hosted
runner, all now handled by the script or workflow:

- The root disk has about 14 GB free before the workflow reclaims the
  usual runner ballast (40 GB after); images are sparse and a
  preflight refuses to start unless the worst case fits.
- Zeroing a device's first block with a buffered `dd` leaves a dirty
  page in the block device's page cache that writeback lands later,
  over the superblock dm-cache has meanwhile written with direct bios
  ("cache metadata: sb_check failed: magic 0", then the filesystem
  goes read-only). The zeroing must be O_DIRECT. The thin pool's
  metadata zeroing carried the same hazard.
- `su` opens a PAM session that pam_systemd parks in a fresh scope, so
  a cgroup joined before the su is silently escaped. The memory cap is
  applied to the postmaster tree after start.

### 6.1 Results, run 7

Scale 100 (1.5 GB database), cache at 25% of the database (381 MB),
PostgreSQL capped at 1 GB so reads reach the block layer, 8 clients,
30 s warm-up then 120 s measured, single run per cell.

| workload | topology | tps | lat ms | origin read MB | pool write MB | cache write MB | cache read MB | flash write MB |
|---|---|---|---|---|---|---|---|---|
| select | nocache | 9205 | 0.87 | 6 | 890 | 0 | 0 | 890 |
| select | baseline | 3895 | 2.05 | 846 | 426 | 531 | 1020 | 957 |
| select | twovol | 8556 | 0.93 | 4 | 946 | 614 | 2430 | 1560 |
| select | bcache | 4188 | 1.91 | 4 | 1127 | 6513 | 21 | 7641 |
| select | opencas | 4315 | 1.85 | 6 | 1194 | 4838 | 0 | 6032 |
| tpcb | nocache | 1794 | 4.46 | 958 | 5665 | 0 | 0 | 5665 |
| tpcb | baseline | 191 | 41.92 | 1719 | 1929 | 741 | 81 | 2669 |
| tpcb | twovol | 1780 | 4.49 | 391 | 5167 | 1004 | 393 | 6171 |
| tpcb | bcache | 1536 | 5.21 | 1279 | 5290 | 2520 | 19 | 7810 |
| tpcb | opencas | 1876 | 4.26 | 1064 | 5982 | 2215 | 1 | 8197 |

Absolute bytes over a fixed window mislead when throughput differs, so
the tpcb rows normalized by transactions completed:

| topology | transactions | pool MB / 1k txn | cache MB / 1k txn | flash MB / 1k txn | vs nocache |
|---|---|---|---|---|---|
| nocache | 215k | 26 | 0 | 26 | 1.0x |
| baseline | 23k | 84 | 32 | 116 | 4.4x |
| twovol | 214k | 24 | 5 | 29 | 1.1x |
| bcache | 184k | 29 | 14 | 42 | 1.6x |
| opencas | 225k | 27 | 10 | 36 | 1.4x |

Time-driven background writes (60 s checkpoints, CoW copies) are
spread over ten times fewer transactions in the baseline row, so its
figure is an upper bound; the other rows completed similar counts and
compare directly. The summary now prints these columns itself.

### 6.2 What the run shows

- **The shipped single-volume stack collapsed under tpcb in this run**:
  191 tps at 42 ms against 1794 tps for nocache, while twovol — the
  same dm-cache with WAL on the uncached volume — matched nocache. The
  mechanism that makes baseline the most flush-sensitive topology is
  real: every WAL fsync through dm-cache forces a metadata commit with
  a FUA superblock write on the cache device, on top of the dual data
  write and the flush cloned to both devices. But the magnitude here
  is not trustworthy. Baseline was also 2.4x slower than nocache on
  select-only, which barely fsyncs, and in run 9 (§6.3) the same stack
  matched nocache with the same flush path. Each matrix job is its own
  runner VM with its own virtual disk; run 7's baseline most likely
  landed on one with slow flushes. Three runs of the same topology:

  | run | origin | baseline tpcb | nocache tpcb | ratio |
  |---|---|---|---|---|
  | 6 | local, no memory cap | 1209 | 2154 | 56% |
  | 7 | local | 191 | 1794 | 11% |
  | 9 | 10 ms | 349 | 345 | 101% |
  | 11 | 10 ms | 366 | 376 | 97% |
  | 12 | 10 ms | 169 | 354 | 48% |

  Run 12's baseline was also 2.1x slower than nocache on select-only
  (3976 against 8431 tps), the same signature as run 7: the job's VM,
  not the flush path.

  The cost of the commit-per-flush path is therefore unmeasured, not
  established, and is an open item below.
- **bcache and Open CAS showed near-zero cache hits with huge
  insertion traffic** (7 to 9x flash write amplification on
  select-only). The likely mechanism is multi-level cache exclusion: an
  insert-on-every-miss block cache that is smaller than the page cache
  above it only ever sees pages the page cache just evicted, and cycles
  them out before they are asked for again. smq promotes only repeat
  hits, so dm-cache kept a stable set and served 1 to 2.4 GB from
  flash. This needs confirming on a workstation with a cache larger
  than the page cache before it is treated as a property of the
  designs rather than of this configuration.
- Per transaction, the ordering matches §4: twovol close to nocache,
  the write-around caches paying more in flash writes than the shared
  dm-cache because of miss-insertion churn.

### 6.3 Results, run 9: a 10 ms origin

Same configuration with `TB_ORIGIN_DELAY_MS=10`: a dm-delay under the
thin's external origin, so every unprovisioned read and every
copy-on-write the thin performs on first write to a chunk pays one
round trip — the multinode design's planning number for an object
store, and the stand-in for the RO NBD attach. Latency per bio only;
bandwidth and the daemon's segment locality are not modeled.

| workload | topology | tps | lat ms | origin read MB | pool read MB | pool write MB | cache write MB | cache read MB | flash MB / 1k txn |
|---|---|---|---|---|---|---|---|---|---|
| select | nocache | 8359 | 0.96 | 664 | 8051 | 305 | 0 | 0 | 0.3 |
| select | baseline | 8738 | 0.92 | 650 | 6771 | 445 | 535 | 2263 | 0.9 |
| select | twovol | 8747 | 0.91 | 3 | 7447 | 775 | 602 | 2401 | 1.3 |
| select | bcache | 4768 | 1.68 | 415 | 4618 | 392 | 6801 | 16 | 12.6 |
| select | opencas | 4859 | 1.65 | 360 | 4781 | 408 | 5111 | 0 | 9.5 |
| tpcb | nocache | 345 | 23.21 | 2029 | 189 | 2780 | 0 | 0 | 67 |
| tpcb | baseline | 349 | 22.95 | 2252 | 398 | 2849 | 915 | 131 | 90 |
| tpcb | twovol | 1412 | 5.66 | 1903 | 1234 | 5814 | 1142 | 362 | 41 |
| tpcb | bcache | 335 | 23.89 | 2004 | 180 | 2739 | 635 | 1 | 84 |
| tpcb | opencas | 361 | 22.13 | 2072 | 204 | 2864 | 490 | 0 | 77 |

What changed against §6.1:

- **Every single-volume topology fell to about 350 tps on tpcb, and
  twovol held 1412.** With a local-file origin nocache did 1794. The
  cost is the thin's copy-on-write, not the caches: PostgreSQL
  recycles WAL segments, those segments already exist in the origin
  image, and the first write to each 64 KiB chunk of a recycled
  segment waits one origin round trip while dm-thin copies the chunk
  in — for an 8 KiB write. Only twovol's WAL sits on a fresh thin with
  no external origin, so it never pays that. Data pages pay it too, but
  the WAL tail is where a commit waits on it.
- That cost is not a one-off warm-up in the real stack. Eager reclaim
  unmaps every published block, so after each advance the next write
  to a recycled WAL chunk fetches it from the origin again. A volume
  that carries WAL must either never be origin-backed (the twovol
  shape) or never have its write-hot chunks reclaimed.
- Baseline equals nocache here even though every fsync still takes
  the dm-cache commit path, which is the main reason §6.2's run-7
  collapse reads as instance variance rather than a stack property.
- On select-only the two dm-cache topologies now show real hits (2.3
  and 2.4 GB served from flash) and match nocache in throughput, while
  bcache and Open CAS still insert 5 to 7 GB and serve essentially
  nothing from their cache device, at half the throughput. Their own
  counters (logged before and after every run since run 10) confirm
  it is churn, not a measurement gap: bcache served 1,347 hits against
  514k misses in the select window (0.26%) with zero bypassed
  requests and writearound confirmed active; Open CAS served 5 hits
  against 527k full misses with occupancy pinned at 100% and 15 to 47%
  of reads passed through because the full cache could not evict fast
  enough to insert. An insert-on-miss cache smaller than the page
  cache above it only ever sees what the page cache just evicted, and
  cycles it out before it is asked for again; smq's promote-on-repeat
  avoids that. Whether either would perform with a cache larger than
  the page cache is still an open workstation question.

## 7. Snapshotting the WAL volume

The two-volume shape only helps if the WAL volume keeps the properties
the single volume had: immutable, restorable snapshots without any
PostgreSQL-side configuration. A data-volume snapshot on its own is
not a restorable database. Recovery starts at the redo point of the
last checkpoint recorded in `pg_control` and needs every WAL record
from there to the snapshot instant, and those records live only in
`pg_wal`; without them the only way in is `pg_resetwal`, which is data
loss. So the WAL volume has to be captured too, and the question is
how to do that without reintroducing the round trip §6.3 measured.

**The round trip is an advance cost, not a snapshot cost.** dm-thin
copies a chunk from the external origin only when a write lands on a
chunk that is unmapped locally and present in the origin
(`schedule_external_copy`). That state exists right after an advance
dropped the local delta, and right after a restore onto a fresh pool.
A snapshot that is uploaded while the local mappings stay in place
never puts the volume there. And advancing a WAL volume reclaims
nothing durable: PostgreSQL recycles segments, so the write working
set is the whole volume, every chunk is rewritten within one WAL cycle
(`max_wal_size`, 1 to 2 GB), and every one of those first writes
would be a fetch. Reclaim pays for volumes whose hot write set is much
smaller than their accumulated delta; WAL is the opposite case.

**One more dm-thin fact makes restore cheap.** `schedule_copy` skips
the origin read when the write bio covers the whole chunk
(`io_overwrites_block`, then `remap_and_issue_overwrite`). A volume
can therefore be pulled local at the origin's bandwidth rather than
its latency: read each region, write it back in whole-chunk bios, in
parallel. That is what `twovol-hydrated` does before starting
postgres, and what an agent would do on attach.

Strategy, assuming the CSI `VolumeGroupSnapshot` API for the pair:

1. **Both volumes carry an immutable origin chain; reclaim policy is
   per volume.** Data: snapshot, upload, advance, as designed. WAL:
   snapshot and upload on the same schedule, never advance. The whole
   local footprint of a WAL volume is bounded by `max_wal_size` times
   about 1.5, so the pool space "never advance" costs is 1 to 2 GB.
2. **Group-snapshot the pair, data before WAL.** PostgreSQL's
   WAL-before-data rule (a page is never written before its WAL record
   is durable) means data at T plus WAL at any T' at or after T is a
   valid crash-consistent restore; the reverse order is silently
   inconsistent. Atomicity across the two thins is not required, but
   is cheap when wanted: suspend both thin devices, create both
   snapshots, resume. The two uploads are recorded as one restore
   point.
3. **Hydrate the WAL volume on restore instead of letting writes fault
   it in.** After a restore the volume is entirely in the origin and
   the first WAL cycle would pay one round trip per 64 KiB chunk,
   about 16k of them per GB. Whole-chunk pre-copy brings a 1 to 2 GB
   volume local in seconds. The same mode fits any small volume whose
   write set is the whole volume.
4. **The WAL StorageClass carries no read cache.** WAL is read only
   during recovery and replication. This also takes the last
   writethrough double-write off the commit path (§4).

The alternative is PostgreSQL-side archiving (`archive_command` or
`pg_receivewal`), which would make the WAL volume scratch and give
point-in-time recovery, but it moves the guarantee into each tenant's
`postgresql.conf` and leaves the current segment unprotected between
archive ticks. Since the goal is that any volume pair the driver
provisions is restorable without database-specific configuration,
the storage-level group snapshot is the one to build; archiving can
sit on top of it.

The chunk-size floor limits what can be done for a WAL volume that is
*not* hydrated: dm-thin's minimum data block is 64 KiB, so the stall
granularity §6.3 measured cannot be reduced by configuration.

### 7.1 Results, runs 11 and 12

Same settings as run 9 (scale 100, 10 ms origin, 1 GB memory cap,
32 KiB cache blocks at 25%), now with the WAL origin image: 1025 MiB
of `pg_wal` on a 2 GB ext4, behind the same 10 ms dm-delay. Hydrating
the 2 GB WAL thin took 15 to 16 s with 8 streams (2 GB mapped, 32768
pool blocks), against a 10 ms-per-bio origin. tpcb, two runs:

| topology | run 11 tps | run 12 tps | lat ms (12) | origin read MB (12) | of which WAL origin | pool write MB (12) | WAL MB (12) | flash MB/ktxn (12) |
|---|---|---|---|---|---|---|---|---|
| nocache | 376 | 354 | 22.6 | 2050 | – | 2820 | 527 | 66.4 |
| baseline | 366 | 169 | 47.4 | 1668 | – | 1828 | 277 | 128.0 |
| twovol | 582 | 1515 | 5.3 | 1925 | – | 6063 | 1725 | 39.6 |
| twovol-origin | 414 | 370 | 21.6 | 2281 | 516 | 2926 | 547 | 81.8 |
| twovol-hydrated | 1399 | 1478 | 5.4 | 1853 | 0 | 5984 | 1684 | 39.8 |
| bcache | 328 | 338 | 23.7 | 2017 | – | 2755 | 507 | 83.8 |
| opencas | 337 | 316 | 25.4 | 1956 | – | 2648 | 479 | 81.4 |

- **A WAL volume on a cold origin behaves like the single-volume
  stacks.** twovol-origin did 414 and 370 tps, in the 320 to 420 band
  of every topology whose WAL sits on unmapped, origin-backed chunks.
  Giving the WAL its own volume buys nothing by itself; what bought
  the §6.3 result was that the WAL chunks were mapped locally.
- **The WAL origin counter shows where the time went.** In run 12
  twovol-origin's WAL volume took 678 MB of writes and pulled 516 MB
  from its origin: about 8,300 whole-chunk copies, each one origin
  round trip, on a commit path that serialises them. At 10 ms that is
  83 s of a 120 s window. The hydrated volume read 0 bytes from its
  origin in both runs.
- **Hydrated, the origin-backed WAL volume performs as the
  origin-less one.** twovol-hydrated did 1399 and 1478 tps against
  twovol's 1515 in run 12 (and 1412, 1499 in runs 9 and 10), with the
  same flash bytes per thousand transactions. Snapshotting the WAL
  volume and restoring it through an origin costs nothing at steady
  state, provided the chunks are local before the first commit lands.
- Run 11's twovol row (582 tps, and 5899 tps on select-only against
  8500 to 9200 for every other cached topology) and run 12's baseline
  row are the runner variance §6.2 describes: those jobs' VMs were
  slow on both workloads, fsync-bound and not.
- On select-only twovol-origin and twovol-hydrated matched nocache in
  both runs (8558 and 8415 against 8431 in run 12); the WAL origin is
  not in the read path.

## 8. Recommendations

1. **Take the two-volume route for PostgreSQL now**: cached PGDATA,
   uncached `pg_wal`. It removes the WAL doubling, the doubled fsync
   flushes and the per-fsync metadata commit (real in the code, of
   unmeasured size, §6.2) and, with a realistic origin, the
   copy-on-write round trip on every recycled WAL chunk that dominates
   everything else (§6.3).
   The WAL volume is snapshotted with the data volume as a group but
   never advanced, and hydrated from its origin on restore (§7).
   PostgreSQL operators already support a separate WAL volume.
2. **Make the read cache a StorageClass parameter.** Today it is
   node-global (an environment variable from the chart's node config)
   and CreateVolume accepts only `mode` and the reclaim dials. The
   split needs a per-class toggle and a per-class size.
3. **Decouple the read-cache block from the thin chunk** and default it
   to 32 KiB for database classes, sized at about 25% of the volume.
4. **Snapshot the pair as a group, data first, then WAL** (§7). Until
   consistency groups exist (`skynbd_design.md` D5 reserves the
   field), each volume has its own lineage, cycle and freeze, so the
   ordering rule is what makes a pair restorable: the WAL generation
   must be at or after the data generation and still hold every
   segment from the data generation's checkpoint redo point. Restore
   the newest WAL generation, then the newest data generation older
   than it.
5. **Keep bcache and Open CAS as later PoCs**, not as the fix. Both
   carry the non-dm integration cost; Open CAS additionally needs its
   compile-gated mode. The regime where they beat the split is a
   database that fits in RAM, where the read cache is doing little
   work anyway.

## 9. Open items

- Run `topology-bench.sh` on a workstation NVMe with the cache larger
  than the page cache (or the cap removed) to settle the bcache and
  Open CAS hit-rate anomaly and to get absolute numbers.
- Measure the dm-cache commit-per-flush cost properly: baseline against
  nocache at `TB_ORIGIN_DELAY_MS=0`, several repetitions, on real
  flash. On the runner it is entangled with per-VM disk flush latency
  (§6.2's three-run table).
- Model the origin's bandwidth and segment locality, not only its
  latency: dm-delay charges every bio the same round trip, while the
  daemon serves 8 MiB segments and may cache them.
- Once the workflow file is on the default branch, `workflow_dispatch`
  works and the push trigger it carries for development can go.

## 10. ZFS on a userspace block backend (exploration)

A different shape for the same problem: replace the dm-thin and
dm-cache layers with ZFS, on one vdev that is a ublk device served by
a node-local daemon with a local write log and the object store behind
it. Snapshots become cuts of the backend's write log at a transaction
group boundary, with no filesystem freeze. Nothing here is built; this
section records what the topology benchmark says about the I/O.

### 10.1 What the rows model

The vdev is a transient dm-snapshot whose chunk is one ZFS sector
(4 KiB at ashift 12) over a read-only, delayed image of an exported
pool holding the same pgbench database. Written blocks are local,
never-written blocks cost the origin round trip, and no write reads
first: dm-snapshot skips the origin copy when a bio covers a whole
chunk, and at chunk = sector every ZFS write does. PGDATA and `pg_wal`
are two datasets of the one pool.

| name | configuration |
|---|---|
| zfs | recordsize 8k (WAL dataset 128k), lz4, `full_page_writes=off`, `wal_recycle=off`, `wal_init_zero=off` |
| zfs-plain | zfs with compression off and `full_page_writes=on`: the control for what pgbench's very compressible rows and the missing full-page images contribute |
| zfs-l2arc | zfs plus an L2ARC device of 25% of the database, the counterpart of baseline's dm-cache |
| zfs-slog | zfs plus a separate 1 GiB log device on local flash, so ZIL blocks never reach the vdev: the ZFS-native form of the two-volume split |
| zfs-slog-l2arc | zfs-slog plus an L2ARC of twice the ARC cap (1536 MiB), with the WAL dataset excluded (`secondarycache=none`): the ZFS-native flash read cache |
| zfs-slog-rcache | zfs-slog plus a dm-cache (writethrough, smq, 25% of the database: baseline's cache) on the origin path under the vdev, standing in for a backend that keeps what it fetched |

The ARC is not charged to postgres's cgroup, so it is capped at the
memory limit minus `shared_buffers` (768 MiB here), which is what the
page cache gets in the other rows. A snapshot of both datasets at the
start of each measured window makes `written@` at the end report the
live bytes born in the window. The pool is 8 GiB in §10.2 to §10.6 and
32 GiB from §10.7 on, for the reason given there.

What the model leaves out, and which way it leans:

- **No backend read cache or read-ahead.** A block that is read is
  never local afterwards unless the ARC holds it. A real backend would
  keep what it fetched and fetch more than one 8 KiB record at a time.
  This is pessimistic for ZFS on a cold origin.
- **No backend log commit.** A flush on the transient store is the
  data device's flush alone; dm-thin pays a metadata commit. Slightly
  optimistic for ZFS.
- **4 KiB bios.** dm-snapshot splits every I/O at the chunk. Some CPU
  cost the real device would not have.

### 10.2 Results

ubuntu-26.04 runner, kernel 7.0, ZFS 2.4.1, PostgreSQL as packaged.
Scale 100, 1 GB memory cap, 8 clients, 30 s warm-up and 120 s
measured, tpcb then select on the same stack. One run per cell, one
runner VM per row.

tpcb, 10 ms origin:

| topology | tps | lat ms | origin read MB | flash MB/ktxn | vs nocache |
|---|---|---|---|---|---|
| nocache | 373 | 21.4 | 2100 | 65.2 | 1.00 |
| baseline | 370 | 21.6 | 2285 | 86.6 | 1.33 |
| twovol | 3590 | 2.2 | 1997 | 29.7 | 0.46 |
| twovol-origin | 400 | 20.0 | 2313 | 78.3 | 1.20 |
| twovol-hydrated | 2618 | 3.1 | 1893 | 32.6 | 0.50 |
| bcache | 371 | 21.6 | 2096 | 81.0 | 1.24 |
| opencas | 349 | 22.9 | 2045 | 78.3 | 1.20 |
| zfs | 583 | 13.7 | 340 | 18.7 | 0.29 |
| zfs-plain | 396 | 20.2 | 1023 | 54.1 | 0.83 |
| zfs-l2arc | 546 | 14.6 | 327 | 171.1 | 2.63 |

tpcb, local origin (`TB_ORIGIN_DELAY_MS=0`), the everything-local
bound:

| topology | tps | lat ms | flash MB/ktxn | vs nocache |
|---|---|---|---|---|
| nocache | 3384 | 2.4 | 22.0 | 1.00 |
| baseline | 3159 | 2.5 | 26.7 | 1.21 |
| twovol | 5576 | 1.4 | 22.3 | 1.01 |
| zfs | 3454 | 2.3 | 11.5 | 0.52 |
| zfs-plain | 2551 | 3.1 | 49.2 | 2.23 |

select-only:

| topology | tps, 10 ms origin | tps, local origin |
|---|---|---|
| nocache | 10732 | 20098 |
| baseline | 11820 | 19015 |
| twovol | 20290 | 20400 |
| zfs | 2666 | 26345 |
| zfs-plain | 1397 | 17136 |
| zfs-l2arc | 2742 | – |

What reached the ZFS vdev, per thousand tpcb transactions:

| origin | topology | vdev write MB | first-touch LBA MB | live at window end MB | ZIL MB |
|---|---|---|---|---|---|
| 10 ms | zfs | 18.7 | 19.1 | 5.9 | 14.6 |
| 10 ms | zfs-plain | 54.1 | 53.1 | 19.2 | 12.8 |
| local | zfs | 11.5 | 9.3 | 2.2 | 8.7 |
| local | zfs-plain | 49.2 | 8.7 | 6.3 | 10.1 |

`first-touch` is growth of the COW store: bytes written to LBAs never
written since the stage. `live at window end` is `written@` against
the window-start snapshot.

### 10.3 What the runs show

- **On a cold origin ZFS is bound by PostgreSQL's own page reads.**
  The zfs row took about one ARC miss per transaction, each one origin
  round trip, and nothing in the model makes a read block local. It
  beat the single-volume stacks by about 1.5x and lost to twovol by
  about 6x.
- **The thin stacks win the cold case by hydrating, as a side effect
  of their amplification.** Every first write to a chunk copies 64 KiB
  from the origin, so they read about 2 GB — the whole database —
  within the window and then run on flash. Their select run inherits
  that from the tpcb run before it. ZFS writes only the 8 KiB it was
  given, which is why it read 340 MB.
- **With everything local, ZFS matches single-volume ext4 in
  throughput and halves the flash writes** (3454 against 3384 tps,
  11.5 against 22.0 MB per thousand transactions), and leads on
  select-only. twovol is still 1.6x faster on tpcb, on a different
  machine; §10.7 repeats that comparison on one machine and the gap
  does not survive it.
- **The saving is compression and the absent full-page images, not
  the layout.** zfs-plain writes 2.2x what nocache does with a local
  origin and is 25% slower. Real schemas compress less than pgbench's.
- **Most of what reaches the vdev is dead within the window.** For
  zfs, 2.2 to 5.9 MB per thousand transactions was still live against
  11.5 to 18.7 written. ZIL blocks are three quarters of the writes.
  An uploader that sends one cut per window and knows what was freed
  carries a fifth to a third of the vdev traffic; one that does not
  carries nearly all of it, because on a cold origin almost every
  write landed on a never-written LBA. The ZIL does not stay inside
  its reserved metaslab at this pool size (8 GB).
- **L2ARC at 25% of the database is churn.** It wrote 10 GB to a
  381 MiB device during tpcb and 20 GB during select, and served about
  1% of ARC misses. The device is smaller than the ARC that feeds it,
  so it wraps every few seconds: the same failure as bcache and Open
  CAS in §6. A flash read cache for this shape belongs in the backend,
  below the vdev.
- **Pool import over the 10 ms origin took about 0.7 s.**

### 10.4 Snapshots during the window, and the separate log device

`TB_SNAP_MODES` takes a snapshot every 60 s while pgbench runs and
keeps the last two. The modes run as phases on one stack, so a row's
phases share a machine. For the thin stacks `freeze` is the agent's
capture bracket (fsfreeze, suspend, `create_snap`, resume, thaw) and
`suspend` is the same without the freeze; twovol snapshots both
volumes in one bracket. The ZFS stacks run `zfs snapshot` of both
datasets. tpcb, local origin, 300 s per phase, two runs with the thin
phases in opposite order (suspend first in run A, freeze first in
run B):

| topology | mode | tps A / B | vs none A / B | bracket ms, mean | stalled seconds A / B | max latency ms A / B | flash MB/ktxn |
|---|---|---|---|---|---|---|---|
| nocache | none | 4892 / 4628 | – | – | 0 / 0 | 188 / 65 | 19.5 |
| nocache | suspend | 3496 / 3825 | 0.71 / 0.83 | 37 / 45 | 9 / 8 | 6306 / 4788 | 26.9 / 26.4 |
| nocache | freeze | 3895 / 3820 | 0.80 / 0.83 | 288 / 493 | 3 / 2 | 2280 / 2961 | 26.2 / 26.3 |
| twovol | none | 4265 / 3728 | – | – | 7 / 2 | 2055 / 2052 | 22.4 / 22.6 |
| twovol | suspend | 3268 / 3046 | 0.77 / 0.82 | 62 / 62 | 14 / 7 | 5702 / 5868 | 30.5 / 31.2 |
| twovol | freeze | 3593 / 2662 | 0.84 / 0.71 | 339 / 502 | 7 / 1 | 3307 / 3026 | 29.8 / 32.1 |
| zfs | none | 1669 / 2948 | – | – | 0 / 1 | 193 / 2629 | 10.4 / 11.1 |
| zfs | zfs snapshot | 1741 / 3660 | 1.04 / 1.24 | 247 / 475 | 1 / 1 | 2023 / 1860 | 9.3 / 10.9 |
| zfs-slog | none | 3664 / 3768 | – | – | 0 / 0 | 703 / 626 | 11.5 |
| zfs-slog | zfs snapshot | 3717 / 3785 | 1.01 / 1.00 | 338 / 336 | 0 / 0 | 427 / 504 | 11.0 / 11.1 |

A stalled second is a whole second in which no transaction completed.
The bracket is what the mechanism holds: freeze to thaw, suspend to
resume, or the `zfs snapshot` call, which blocks only its caller.

- **A thin snapshot every minute costs 17 to 29% of throughput and
  about 35% more flash writes, with or without the freeze.** The cost
  is dm-thin's: after each snapshot the first write to every chunk
  copies 64 KiB to break sharing.
- **Dropping the freeze made the stalls worse, not better.** The
  bracket fell from 0.3–0.5 s to about 50 ms, but the suspend-only
  phases had 7 to 14 stalled seconds against 1 to 7 with the freeze,
  and a worst transaction of 4.8 to 6.3 s against 2.3 to 3.3 s — in
  all four comparisons, in either phase order. The likely reason is
  that the freeze flushes the dirty backlog before the snapshot, while
  without it that backlog is written afterwards, when every chunk has
  to be copied first. The `sync` mode below tests that.
- **`zfs snapshot` cost nothing measurable**: throughput and flash
  writes are unchanged. With the log device there was no stalled
  second in any phase and the worst transaction stayed under 0.7 s.
- **The log device moves the ZIL off the vdev without reducing total
  flash writes.** With a local origin the vdev takes 5.1 MB per
  thousand transactions against 10.4 to 11.1 without it, and the log
  device takes 6.3; with the 10 ms origin, 8.0 against 18.9. Live
  bytes at the end of a 300 s window were 0.9.
- **zfs-slog was also faster and steadier than zfs** (3664 to 3785
  tps over four phases on two machines, against 1669 to 3660). Part of
  that is the model: a flush on the vdev is an fsync of a sparse file
  that is still growing, and the log device is a small file that stops
  growing after one pass. The size of the real effect is unmeasured.
- On the 10 ms origin zfs-slog matches zfs (720 against 725 tps): both
  are bound by page reads, as in §10.3.

**Sync, then suspend.** A third thin mode runs syncfs on the
filesystem and then takes the suspend-only snapshot: the flush a
freeze does, with writers running. Two more runs, nocache and twovol,
phases in opposite order (C: none, sync, suspend, freeze; D: none,
freeze, suspend, sync):

| topology | mode | tps vs none C / D | pause ms, mean | sync before it, ms | stalled seconds C / D | max latency ms C / D |
|---|---|---|---|---|---|---|
| nocache | sync | 0.75 / 0.83 | 66 / 48 | 180–1300 | 2 / 1 | 3820 / 2152 |
| nocache | suspend | 0.54 / 0.81 | 50 / 47 | – | 10 / 10 | 4246 / 5102 |
| nocache | freeze | 0.62 / 0.81 | 1037 / 304 | – | 3 / 2 | 2734 / 2766 |
| twovol | sync | 0.69 / 0.67 | 55 / 49 | 300–980 | 2 / 6 | 3072 / 3752 |
| twovol | suspend | 0.78 / 0.68 | 51 / 46 | – | 11 / 9 | 4205 / 5258 |
| twovol | freeze | 0.83 / 0.65 | 574 / 277 | – | 4 / 9 | 2794 / 3899 |

The `none` phases had 0 stalled seconds for nocache and 0 and 3 for
twovol.

- **The flush is what the freeze contributes.** Syncing first brings
  the suspend-only snapshot from about 10 stalled seconds to 1 or 2 on
  nocache, level with the freeze, while writers are paused for about
  50 ms and not for the 0.3 to 1 s the freeze holds. twovol follows
  the same order in run C and is too noisy to rank in run D.
- **So a freeze-free capture is viable if it syncs first**, and the
  agent's existing suspend-only path is the wrong one to promote as it
  stands.
- **It does not remove the cost of a thin snapshot.** Every thin mode
  still loses 17 to 46% of throughput, writes about 40% more to flash,
  and has a worst transaction of 2 to 5 s.

### 10.5 A read cache under the vdev

§10.3's cold result is a statement about the model's missing read
cache. zfs-slog-rcache puts baseline's dm-cache on the origin path,
below the dm-snapshot. The origin is immutable, so that cache sees
only reads of blocks ZFS has never written: no write is doubled,
nothing in it can go stale, and blocks already on local flash do not
compete for it. It is where a backend's cache of fetched objects
would sit. smq promotes on repeat reads only, where a backend would
keep every block it paid a round trip for, so this is a floor.

Same settings as §10.2 unless stated; one run per cell.

| origin, window | workload | baseline | twovol | zfs-slog | zfs-slog-rcache |
|---|---|---|---|---|---|
| 10 ms, 120 s | tpcb tps | 377 | 3159 | 366 | 1327 |
| 10 ms, 120 s | select tps | 14695 | 20148 | 1706 | 11056 |
| 10 ms, 300 s | tpcb tps | 1520 | 2563 | 1369 | 2328 |
| 10 ms, 300 s | select tps | 19460 | 18807 | 15804 | 31103 |
| 10 ms, 300 s | tpcb flash MB/ktxn | 34.6 | 26.8 | 16.7 | 14.7 |
| 3 ms, 300 s | tpcb tps | 2956 | 5546 | 3017 | 3085 |
| 3 ms, 300 s | select tps | 19612 | 20856 | 27769 | 28158 |
| 3 ms, 300 s | tpcb flash MB/ktxn | 25.7 | 21.5 | 12.6 | 13.0 |

- **At 10 ms the cache closes most of the gap.** tpcb goes from 1369
  to 2328 over 300 s, within 10% of twovol at about half the flash
  writes, and select-only becomes the fastest row measured. In the
  first 120 s the gain is larger (3.6x on tpcb, 6.5x on select) and
  twovol is still 2.4x ahead on tpcb.
- **At 3 ms the cache makes no measurable difference over 300 s.**
  zfs-slog without it matches baseline on tpcb, leads every thin row
  on select, and writes about half as much to flash. twovol is 1.8x
  faster on tpcb.
- **Every stack hydrates given time.** baseline went from 377 tps in
  a 120 s window to 1520 in 300 s on the same 10 ms origin, and
  zfs-slog from 366 to 1369. The 120 s rows in §10.2 measure the
  first two minutes after a stage.
- **L2ARC works once it is larger than the ARC.** At 200% of the
  database (10 ms, 300 s) it served 2.3 million reads on select-only
  for 23291 tps. It did not help tpcb (1226 tps) and wrote 2.7 GB to
  flash doing so, 24.3 MB per thousand transactions against 16.7 for
  zfs-slog. It is fed only from what the ARC evicts, so it warms
  behind the workload.
- zfs-slog's own 10 ms, 120 s tpcb figure was 720 in the earlier run
  and 366 here. Read single cells accordingly.

### 10.6 ZFS-native read caching: L2ARC and recordsize

dm-cache in §10.5 stands in for a cache the backend would own. The
ZFS-native options are an L2ARC that is large enough, and a larger
recordsize so that a miss brings in more than one page. ZFS has no
other read-ahead for random reads: file prefetch follows sequential
streams only. 300 s windows, tpcb then select, one run per cell; the
L2ARC feed ran at module defaults (ZFS 2.4.1, which has no
`l2arc_dwpd_limit`).

Throughput, tps:

| origin | recordsize | workload | zfs-slog | zfs-slog-l2arc | zfs-slog-rcache |
|---|---|---|---|---|---|
| 10 ms | 8k | tpcb | 1413 | 1425 | 2307 |
| 10 ms | 8k | select | 16578 | 23593 | 23080 |
| 10 ms | 16k | tpcb | 2516 | 2470 | 4105 |
| 10 ms | 16k | select | 31359 | 30355 | 36696 |
| 3 ms | 8k | tpcb | 3341 | 4047 | 3446 |
| 3 ms | 8k | select | 24158 | 52893 | 24046 |
| 3 ms | 16k | tpcb | 5172 | 3455 | 3894 |
| 3 ms | 16k | select | 73086 | 30910 | 43009 |

twovol, which is ext4 and has no recordsize, ran beside the 8k rows:
3163 tps on tpcb and 18820 on select at 10 ms, 3428 and 18818 at 3 ms.

Flash writes on tpcb, MB per thousand transactions:

| origin | recordsize | zfs-slog | zfs-slog-l2arc | zfs-slog-rcache |
|---|---|---|---|---|
| 10 ms | 8k | 16.6 | 24.0 | 15.4 |
| 10 ms | 16k | 12.5 | 17.1 | 11.1 |
| 3 ms | 8k | 12.4 | 17.6 | 12.6 |
| 3 ms | 16k | 9.9 | 15.0 | 10.8 |

twovol wrote 25.2 at 10 ms and 22.9 at 3 ms.

- **An L2ARC of twice the ARC helps reads and not tpcb.** At 10 ms
  and 8k it served 2.2 million reads on select-only and matched the
  cache under the vdev (23593 against 23080), but tpcb did not move
  (1425 against 1413) where the cache under the vdev gave 1.6x. In
  the first 120 s after a stage the order is the same: on select 7903
  for L2ARC, 11925 for the cache under the vdev and 1721 with
  neither; on tpcb 717, 1291 and 468.
- **It costs flash.** Feeding it added 40 to 50% to tpcb's flash
  writes in every pairing.
- **16k records were the larger gain, and most of it is compression,
  not read-ahead.** At ashift 12 an 8 KiB record can shrink only to
  4 KiB, a 16 KiB one to 4, 8 or 12. pgbench's rows compress about
  4x, so the PGDATA dataset fell from 879 MiB to 444 MiB and fit
  inside the 768 MiB ARC: the ARC hit rate went from 88% to 98–100%
  and origin reads on select-only to zero. tpcb rose 1.5 to 1.8x and
  flash writes fell, because a rewritten record is also smaller. On
  less compressible data the dataset will not halve; the read-ahead
  share of the gain is not separated out here.
- **With 16k records the L2ARC had nothing to do** (the data was in
  RAM) and was pure cost: 3.5 to 4.3 GB written, a handful of hits.
- **16k records plus the cache under the vdev was the fastest tpcb
  row at 10 ms** (4105), above twovol on a different machine, at less
  than half twovol's flash writes.
- **At 3 ms the cache under the vdev makes no measurable difference
  at either recordsize**, as in §10.5.
- **Machines differ by more than 2x.** The 3 ms, 16k select cells ran
  the same in-memory workload (ARC hit rate 100%, both caches idle)
  and measured 73086, 30910 and 43009. Differences smaller than that
  between rows in CPU-bound cells are not evidence of anything.

**The L2ARC feed knobs** (`TB_ZFS_PARAMS`), zfs-slog-l2arc at 10 ms
with 8k records:

| setting | tpcb tps | tpcb flash MB/ktxn | L2ARC written in tpcb | select tps | ARC misses served by L2ARC, select |
|---|---|---|---|---|---|
| defaults | 1628 | 23.6 | 3.5 GB | 30432 | 96% |
| `l2arc_mfuonly=1` | 1675 | 19.0 | 1.3 GB | 31211 | 94% |
| `l2arc_mfuonly=2` | 1443 | 19.4 | 1.3 GB | 23170 | 95% |
| `l2arc_write_max` 4 MiB/s | 1605 | 23.1 | 3.5 GB | 45037 | 97% |
| `l2arc_noprefetch=0 l2arc_headroom=0`, three runs | 408, 906, 1387 | 29.2, 26.1, 24.2 | 1.3, 2.3, 3.2 GB | 8630, 20088, 23189 | 91%, 96%, 96% |

- **`l2arc_mfuonly` is the one worth setting.** Caching only
  repeatedly used blocks cut what tpcb wrote to the L2ARC by about
  60% and its flash overhead from about +40% to about +15% over
  zfs-slog's 16.6, and kept the select-only benefit.
- **No setting moves tpcb.** Its misses are first reads of a block,
  and an L2ARC holds only what the ARC has already seen.
- **Lowering `l2arc_write_max` did nothing**: the feed never reached
  the cap.
- **`l2arc_noprefetch=0 l2arc_headroom=0` bought nothing** and two of
  its three runs were well below the default's tpcb. Whether that is
  the setting or the machines is not settled; there is no reason to
  use it.

### 10.7 Everything local: twovol against zfs-slog on one machine

Every table above gives each topology its own runner VM, and two
runners differ by more than 2x (§10.6). When the working set fits
local flash and no read goes to the origin, the stacks differ by less
than the machines do, so "twovol is 1.6x faster" in §10.3 could not be
told apart from the machine. The workflow's `sequence` input runs
several configurations one after another on one runner. Below: tpcb,
undelayed origin, 8 clients and 180 s unless stated, one run per cell.
A letter is one runner; where a sequence starts and ends with twovol,
both results are given and ratios are against their mean.

**Two sequences of eight configurations**, in opposite order:

| configuration | A tps | A vs twovol | B tps | B vs twovol |
|---|---|---|---|---|
| twovol | 5546 | 1.00 | 3589 | 1.00 |
| twovol, `full_page_writes=off` | 5110 | 0.92 | 3492 | 0.97 |
| zfs-slog 8k | 4771 | 0.86 | 3674 | 1.02 |
| zfs-slog 16k | 5110 | 0.92 | 3744 | 1.04 |
| zfs-slog 8k, ARC 3 GiB | 5467 | 0.99 | 3885 | 1.08 |
| twovol, `synchronous_commit=off` | 7337 | 1.32 | 4286 | 1.19 |
| zfs-slog 8k, `synchronous_commit=off` | 7722 | 1.39 | 4940 | 1.38 |
| zfs-slog 8k, `sync=disabled` | 7188 | 1.30 | 4714 | 1.31 |

PostgreSQL's own I/O timers and the CPU on the plain rows:

| | A twovol | A zfs-slog 8k | A zfs-slog 16k | B twovol | B zfs-slog 8k | B zfs-slog 16k |
|---|---|---|---|---|---|---|
| WAL syncs per second | 2239 | 2008 | 1982 | 1675 | 1754 | 1696 |
| transactions per WAL sync | 2.48 | 2.38 | 2.58 | 2.14 | 2.09 | 2.21 |
| one WAL sync, µs | 335 | 310 | 299 | 504 | 464 | 434 |
| relation reads, ms per transaction | 0.207 | 0.185 | 0.035 | 0.268 | 0.234 | 0.066 |
| CPU, ms per transaction | 0.41 | 0.49 | 0.44 | 0.80 | 0.88 | 0.83 |
| CPU user / system / iowait / idle, % | 24/33/23/20 | 21/38/20/21 | 22/34/21/23 | 35/37/16/12 | 37/44/5/14 | 37/41/6/16 |
| flash MB/ktxn | 21.4 | 10.8 | 9.8 | 23.0 | 11.8 | 10.9 |
| seconds with no transaction | 2 | 11 | 4 | 0 | 0 | 0 |
| slowest transaction, ms | 2051 | 1964 | 3087 | 1100 | 107 | 665 |

- **Neither stack wins on both machines.** zfs-slog ran at 0.86 to
  0.99 of twovol on A and at 1.02 to 1.08 on B. Most of the gap in the
  earlier tables was the machine: A and B differ by 1.5x on the same
  configuration.
- **The wait for the commit is the largest cost on both, and without
  it ZFS is ahead** (7722 against 7337, 4940 against 4286). Its data
  path is not the slower one.
- **Under load a WAL sync costs the same on both**: about 0.3 ms on A
  and 0.45 to 0.5 ms on B, shared by 2.1 to 2.6 transactions.
- **ZFS spends more CPU and waits less on the device**: 4 to 20% more
  CPU per transaction, nearly all of it system time, and a lower
  iowait share. B is the machine with less CPU to spare, and ZFS still
  led there.
- **Reads cost the same at 8k.** The ARC is capped to what the page
  cache gets, and relation reads take the same time per transaction.
  16k records or a 3 GiB ARC cut them four- to fivefold, for 2 to 15%
  more throughput.
- **Full-page writes are not what holds twovol back.** Turning them
  off, as the ZFS rows have them, cut its flash writes by a third and
  did not make it faster.

**Two limits on dirty data.** On A every ZFS run had whole seconds
with no transaction, and its slowest transaction took exactly as long
as its longest transaction-group sync. Two kstat counters say why, and
three more sequences test each cause by removing it:

| machine | configuration | tps | vs twovol | space-check waits | ARC dirty waits | seconds with no transaction | slowest transaction ms | longest txg sync ms |
|---|---|---|---|---|---|---|---|---|
| A | zfs-slog 8k | 4771 | 0.86 | 172 | 0 | 11 | 1964 | 1959 |
| A | zfs-slog 16k | 5110 | 0.92 | 274 | 0 | 4 | 3087 | 3084 |
| B | zfs-slog 8k | 3674 | 1.02 | 0 | 0 | 0 | 107 | 863 |
| B | zfs-slog 16k | 3744 | 1.04 | 195 | 0 | 0 | 665 | 662 |
| C | twovol, first / last | 5411 / 5408 | 1.00 | – | – | 4 / 3 | 2053 / 2054 | – |
| C | zfs-slog 16k | 5107 | 0.94 | 255 | 0 | 7 | 4232 | 4225 |
| C | zfs-slog 16k, 32 GiB pool | 5745 | 1.06 | 0 | 119 | 1 | 1777 | 1775 |
| C | zfs-slog 8k, 32 GiB pool | 5289 | 0.98 | 0 | 0 | 0 | 666 | 2416 |
| D | twovol, first / last | 5287 / 5203 | 1.00 | – | – | 2 / 3 | 2051 / 2052 | – |
| D | zfs-slog 16k | 5334 | 1.02 | 285 | 0 | 1 | 1170 | 1168 |
| D | zfs-slog 16k, `spa_asize_inflation=6` | 5402 | 1.03 | 0 | 105 | 3 | 1354 | 1353 |
| D | zfs-slog 8k, `spa_asize_inflation=6` | 5291 | 1.01 | 0 | 0 | 0 | 806 | 2292 |
| E | twovol, first / last | 5838 / 5862 | 1.00 | – | – | 3 / 1 | 2281 / 2006 | – |
| E | zfs-slog 16k | 5306 | 0.91 | 304 | 0 | 3 | 1369 | 1365 |
| E | zfs-slog 16k, `spa_asize_inflation=6` | 5617 | 0.96 | 0 | 111 | 8 | 1573 | 1570 |
| E | zfs-slog 8k, `spa_asize_inflation=6` | 5801 | 0.99 | 0 | 0 | 0 | 250 | 2256 |

Rows without a pool size ran the 8 GiB pool of every earlier table.
`space-check waits` is the `dmu_tx_quota` kstat and `ARC dirty waits`
is `dmu_tx_dirty_throttle`.

- **The pool was too small for its dirty data.** ZFS charges dirty
  data against the pool's free space at `spa_asize_inflation` = 24
  times its size: the worst case of triple-parity RAID-Z, three copies
  and ditto blocks, whatever the pool is made of
  (`dmu_objset_willuse_space`). When used space plus that estimate
  passes the pool's size, `dsl_dir_tempreserve_space` turns the
  transaction away and `dmu_tx_wait` makes the writer wait for a
  transaction group to sync. An 8 GiB pool with 1 to 2 GiB used has
  room for roughly 240 to 280 MiB of dirty data, and tpcb at 5000 tps
  dirties 230 to 270 MiB per group. A 32 GiB pool or an inflation of 6
  brought the waits to zero and throughput up by 12% on C, 1% on D and
  6% on E; the size of the loss follows how long that machine's syncs
  take.
- **The ARC cap is the second limit.** Dirty data may not exceed half
  the ARC (`arc_tempreserve_space`; the 50% is not a tunable), which
  is 384 MiB under the 768 MiB cap, and the penalty is the same wait.
  16k records dirty 317 MiB per group, which is the point where ZFS
  starts a sync on a 16 GiB machine (20% of `zfs_dirty_data_max`,
  itself 10% of RAM), and the group being synced plus the one filling
  pass 384 MiB. 8k records dirty 245 to 270 MiB per group and stayed
  under it at 8 clients.
- **The write throttle never ran.** ZFS's graduated delay starts at
  60% of `zfs_dirty_data_max`, about 1 GiB here. Both limits above sit
  far below that, so writers went from no delay to waiting out a whole
  sync, 1 to 4 s on these runners. The delay counters read 0 in every
  run.
- **With neither limit in play zfs-slog is level with twovol**: 0.98,
  1.01 and 0.99 at 8k on C, D and E, with no second without a
  transaction and a slowest transaction of 250 to 806 ms. twovol's
  slowest is about 2050 ms on four of the five machines, cause unknown
  (§10.9).
- **The earlier ZFS rows all ran the 8 GiB pool.** At a 10 ms origin
  the space check does not fire, because throughput is too low to
  dirty that much (0 waits in all six ZFS rows of a default run). The
  local-origin and 3 ms cells above about 3500 tps in §10.2 to §10.6
  are understated by up to the 12% measured here.

**Capping dirty data.** `zfs_dirty_data_max` at 192 MiB, a quarter of
the ARC cap, keeps dirty data below both limits. Machine H, 16k unless
stated. H slowed down during the sequence (twovol 3209 tps first and
1924 last), so its throughput is not comparable between rows and is
left out:

| configuration | waits, both kinds | txgs in 180 s | seconds with no transaction | slowest 1 s, tps | slowest transaction ms | vdev MB/ktxn | flash MB/ktxn |
|---|---|---|---|---|---|---|---|
| 32 GiB pool | 30 | 36 | 3 | 0 | 1889 | 5.2 | 11.3 |
| 32 GiB pool, 192 MiB cap | 0 | 312 | 0 | 284 | 605 | 8.9 | 14.4 |
| 8 GiB pool, 192 MiB cap | 0 | 380 | 0 | 289 | 676 | 8.8 | 14.3 |
| 8k, 32 GiB pool, 192 MiB cap | 0 | 270 | 0 | 78 | 502 | 12.3 | 18.2 |

- **The cap removes the waits, on the small pool too.**
- **It costs flash writes.** ZFS starts a sync at 20% of the cap, so
  transaction groups come every half second and not every 5 s, and
  each one rewrites its metadata and every page touched since the
  last. The vdev took 70% more and total flash 27% more at 16k. The
  8k row wrote 18.2 MB/ktxn against 10.6 to 10.9 for 8k without a cap
  on C to E, and twovol writes 21 to 23.

**The commit path alone.** pg_test_fsync on the WAL filesystem before
each run, one 8 kB write and fdatasync with nothing else running, µs:

| machine | twovol | zfs-slog, sparse log file | zfs-slog, log written through | same with `zil_nocacheflush=1` |
|---|---|---|---|---|
| A | 157, 507, 887 | 333–577 | – | – |
| B | 235–289 | 374–393 | – | – |
| C | 172, 213 | 322–377 | – | – |
| D | 157, 164 | 321–376 | 210 | – |
| E | 148, 150 | 291–347 | 184 | – |
| F | 323, 323, 470 | 435 | 258, 310, 486 | 209 |
| G | 217–225 | 408 | 281–287 | 194 |
| I | 228–269 | – | 231–240 | – |
| J | 170, 555, 597 | – | 227, 230, 494 | – |

Each figure is one 2 s test, and a stall of the runner's disk inside
it shows: A, F and J each have figures two to five times their others.

- **Most of ZFS's slower lone commit was the rig.** The log device was
  a sparse file. A ZIL block lands on blocks the host filesystem has
  not allocated yet, so the flush behind it commits that filesystem's
  journal. twovol's WAL does not pay this, because PostgreSQL
  zero-fills a WAL segment before it syncs into it, and a flash
  partition has no such cost. On the machines with steady figures a
  lone ZFS commit took 1.3 to 2.4 times twovol's with the sparse file
  (B to E, G) and between the same and 1.3 times with the image
  written through once (D, E, G, I).
- **About a third of a ZFS commit is the device flush** (310 to 209 µs
  on F, 284 to 194 on G), and one client gains 30 to 35% without it
  in the table below.
- **Under 8 clients the sparse file made no difference** (5457 against
  5402 tps on D, 5622 against 5617 on E): group commit shares the
  flush.

One client, tpcb, 120 s, 16k records:

| machine | twovol tps | zfs-slog, sparse log | zfs-slog, log written through | same with `zil_nocacheflush=1` | WAL sync µs: twovol / written through / no flush |
|---|---|---|---|---|---|
| F | 706 | 586 | 633 | 857 | 707 / 867 / 492 |
| G | 891 | 965 | 966 | 1256 | 433 / 474 / 283 |

**1, 8 and 32 clients, with ZFS sized for the node.** zfs-slog with
16k records, a 32 GiB pool, the log device written through and the
192 MiB dirty cap, against twovol, 180 s per cell (I ran its 32-client
cells first, J its one-client cells first):

| machine | clients | twovol tps | zfs-slog tps | zfs-slog vs twovol | p99 ms, twovol / zfs-slog | slowest transaction ms | slowest 1 s, tps | flash MB/ktxn |
|---|---|---|---|---|---|---|---|---|
| I | 1 | 963 | 997 | 1.04 | 2.2 / 2.2 | 52 / 40 | 285 / 822 | 42.5 / 22.6 |
| I | 8 | 3564 | 3843 | 1.08 | 5.8 / 7.1 | 1109 / 37 | 140 / 2954 | 23.1 / 14.1 |
| I | 32 | 3955 | 4708 | 1.19 | 40.2 / 28.1 | 1737 / 66 | 0 / 3871 | 20.3 / 11.0 |
| J | 1 | 456 | 655 | 1.43 | 32.8 / 13.0 | 636 / 532 | 6 / 61 | 60.7 / 22.5 |
| J | 8 | 2415 | 2760 | 1.14 | 48.1 / 20.8 | 2021 / 390 | 0 / 660 | 25.3 / 14.3 |
| J | 32 | 4298 | 3501 | 0.81 | 80.1 / 96.6 | 2056 / 692 | 0 / 629 | 20.2 / 10.8 |

- **At 1 and 8 clients zfs-slog was ahead on both machines**, at 40
  to 60% of twovol's flash writes.
- **At 32 clients the two machines disagree** (1.19 and 0.81). J has
  the slower disk — a WAL sync took 1.2 to 1.6 ms there at 32 clients
  against 0.9 to 1.1 ms on I — and ran its 32-client ZFS cell last,
  with no twovol cell after it to show drift; H lost 40% over one
  sequence. Without the cap, on F and G over 120 s, zfs-slog ran at
  0.71 and 0.92 of twovol at 32 clients with 27 and 207 ARC dirty
  waits. Not settled.
- **No zfs-slog cell had a second without a transaction**, and its
  slowest transaction was 37 to 692 ms. twovol's slowest second at 8
  and 32 clients was 0 to 140 tps and its slowest transaction 1.1 to
  2.1 s.
- **The cap's price shows at 8 clients**: 14.1 and 14.3 MB/ktxn,
  against 9.5 to 10.9 for the uncapped 16k rows above.

The benchmark's defaults now follow this: the ZFS pool is 32 GiB and
the log device is written through before use. The dirty cap is left to
`TB_ZFS_PARAMS`, because the right value is a trade between stalls and
flash writes that these runs only sample at one point.

### 10.8 What this means for the design

1. The backend's read side decides cold performance: what it keeps of
   what it fetched, and how much it fetches around a miss. ZFS's write
   path is not the limit. A plain repeat-read cache under the vdev
   already brings tpcb within 10% of twovol at a 10 ms origin, and at
   3 ms the stack does not need one (§10.5). ZFS's own L2ARC covers
   read-mostly work but not tpcb, and costs flash writes (§10.6).
2. The uploader needs to know what ZFS freed. Without that it uploads
   three to five times the live data.
3. ZIL traffic is the largest single component at the vdev. A separate
   log device on local flash keeps it out of the backend (§10.4), at
   the price of cuts that hold the last synced transaction group and
   not the fsynced tail. The log device has to be allocated storage:
   on a sparse file a lone commit took up to 2.4 times as long
   (§10.7).
4. With the working set on local flash, twovol and zfs-slog are level
   in throughput once both run on one machine and ZFS's pool and dirty
   data are sized for each other (§10.7): within 2% at 8 clients with
   8k records, ahead at 1 and 8 clients with 16k records and a dirty
   cap, unsettled at 32. zfs-slog wrote 40 to 60% of twovol's flash
   bytes and had no second without a transaction in those runs, where
   twovol had up to 4. A snapshot every minute costs the thin stacks
   17 to 46% of their throughput and costs zfs-slog nothing (§10.4).
   ZFS's case rests on those and on a much simpler node stack.
5. The dm-thin capture can drop the freeze if it syncs first (§10.4):
   the pause falls to about 50 ms with no more stalls than the freeze.
   The per-snapshot cost stays, because it is the copy-on-write after
   the snapshot, which the freeze does not cause.
6. A pool per volume means small pools, and ZFS's dirty-data defaults
   assume a large pool and an uncapped ARC. Writers wait out a whole
   sync when 24 times the dirty data exceeds the pool's free space, or
   when dirty data exceeds half the ARC. The limit meant to slow them
   first, `zfs_dirty_data_max`, defaults to 10% of RAM whatever the
   pool or the ARC cap (§10.7). It is one value for the whole module,
   so the node has to set it for its smallest pool and its ARC, and
   can lower `spa_asize_inflation`, which a single-device pool does
   not need at 24. Smaller transaction groups cost flash writes: 27%
   more at 16k with a 192 MiB cap.

### 10.9 Not yet measured

- A backend read cache that keeps every fetched block and reads
  ahead; §10.5 measures only a repeat-read cache.
- `logbias=throughput` on the WAL dataset.
- recordsize 16k on data that does not compress 4x, and with the ARC
  smaller than the compressed database, to separate read-ahead from
  compression.
- Why some zfs-slog tpcb cells at 10 ms run several times slower than
  others with a p99 of 140 to 200 ms (366 and 468 tps in 120 s
  windows against 720; 408 against 1387 in §10.6).
- Why twovol stalls for whole seconds with no snapshots at all (2 and
  7 stalled seconds in §10.4's `none` phases), and why its slowest
  transaction is about 2050 ms on most runners (§10.7).
- A dirty-data cap between 192 MiB and the default, and one at half
  the ARC cap, to find where the waits end and what each costs in
  flash writes; and the 8k-record row at 1 and 32 clients.
- §10.2 to §10.6 again with the 32 GiB pool, for the local-origin and
  3 ms cells that the space check held back.
- Snapshots during the window on the 10 ms origin.
- A database several times larger than RAM, where sparse updates
  rewrite a 128 KiB indirect block per 8 KiB page.
- Repetitions. Every cell in §10.2, §10.5 and §10.6 is one run,
  §10.4 has two per table, §10.7 has one run per cell on each of its
  machines, and §6.2 documents how far two runner VMs can differ.
