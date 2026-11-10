# SkyNBD PostgreSQL Read-Cache Study

**Status**: study, not a decision record. It informs the origin-advance
design's §7.1 read cache (`skynbd-origin-advance-design.md`) and the
per-StorageClass cache work it recommends; nothing here changes the
shipped stack by itself. Companion benchmark:
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
