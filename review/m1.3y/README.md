# M1.3y: direct I/O on a pool of I/O threads (pull request #58)

Branch `skyzfs-coding-m1.3y`, base `zfs-topology` at 78242e4, head
1359259 (reviewed; 47068ad is the last head with code in it, 1359259
changes module documentation only). The review is the comment
https://github.com/skyzfs/skyzfs/pull/58#issuecomment-6855734198.

## What the work package changes

`skyzfsd serve --store in-place` reads and writes the home image by
direct I/O (`O_DIRECT`) on a pool of I/O threads, eight by default
(`--io-threads N`, 1 to 64). The start asks the file system for its
direct-I/O alignment (`statx`, `STATX_DIOALIGN`) and refuses where it
offers none or its unit does not divide a block. The store serialises
requests that overlap and makes a flush wait for every write on the
threads. The status file is version 9 (2008 bytes) and carries the
threads' queue.

## The review

From the code nothing blocked the merge. One thing blocked as it stood,
from CI: the daemon rig's ubuntu-26.04 job failed check c at this head.

### The rig's failure (run 43430825700, job `ubuntu-26.04, daemon`)

`case_prop_failures_match_model` failed with "read #31: block 16377 is
Unexpected" on volume 60; `skyzfs-blkcheck --replay c-trace.tsv 60`
gives the same on the job's trace, and the other 61 volumes replay
clean. `rig-26.04-volume60.tsv` is the part of that trace that matters.

Two writes share blocks 16377 to 16379: `#20 write 16372+8` and
`#21 write 16377+7`. `#20` returns, the server is killed 1 ms later
with `#21` outstanding, the next server answers `#21`, and the
read-back finds `#20`'s data in the shared blocks and `#21`'s in
16380 to 16383. The model refuses that: a call outstanding at a kill is
issued again by the kernel, so the next server applies it whole after
`#20`.

No path to it was found in the store. What fits every line of the
trace is a cause above the daemon (likely, not shown):

1. The tool's write is one `write_all_at` on the device opened with
   `O_DIRECT`. A direct write to a block device that cannot drop a
   cached page of its range is done through the device's page cache
   instead (`blkdev_direct_write` returns 0 on `-EBUSY`, and
   `blkdev_write_iter` goes on with the buffered write; `block/fops.c`
   at v7.0). The tool had run `blkdiscard` on the device 2 ms before
   (`#5`), whose close of a writable descriptor makes udev probe a
   device it watches, and a probe reads the device's head and tail
   through the cache. The suite's window is the device's last 24
   blocks.
2. `#21` goes that way: its seven blocks are dirty in the device's
   cache.
3. `#20`, a direct write, first writes out what is dirty in its own
   range and waits for it: `#21`'s first three blocks, as one request.
4. `#20`'s own request follows and is answered; `#20` returns.
5. `#21`'s own write-out sends the last four blocks; the kill finds
   that request in flight, and the next server answers it.

Every server in that sequence does what the contract asks, and the
state is the one R8 allows (`#20` wins on every shared block). The
model is wrong to take `#21` for one request. The artifact holds only
the tool's side, so the requests themselves were not seen.

What would show it: on a kernel, read the tail of a ublk device through
the cache in a loop while two threads repeat `#20` and `#21`, and
compare the write requests in `/sys/block/ublkbN/stat` (requests plus
merges) with the write calls. What would help the next time: the rig
keeping what the server was handed for each volume of check c.

### What the tests pin

1. Of eight defects planted in the ordering (`must_wait`, `accept`, the
   drain), three are caught by no test and one only by chance:
   - a read neither waits nor is waited for: nothing failed (one run);
   - a write does not wait for a read it overlaps: nothing fails;
   - a flush waits only when everything on the threads is a write:
     nothing fails;
   - a read does not wait for a write it overlaps: the large-page
     conformance suite fails in some runs of the whole test binary
     (3 of 5) and in none of 20 runs of the test alone.
   The overlap test cannot tell its clauses apart, because its read
   waits behind the deferred write whatever the clause says; the flush
   test holds two writes, so `.all(write)` passes where `.any(write)`
   is meant. Sharper cases: a held read with a write behind it; a held
   write with a read behind it and nothing in between; a held read
   *and* write with a flush behind them.
2. The other four of the eight, and a drop that does not wait, are
   caught by the five power-cut and failure property tests.
3. The tests for a panicking thread answer a defect by hanging: with
   the `Life` guard not handing its job back, `take_back` not run, or
   the death not counted, the run never ends (stopped at 330 s).
   `release_and_drain` and the two `while` loops want the bound that
   `until_done` already has.
4. The module text's example of a file system that accepts direct I/O
   and buffers behind it (ext4 with `data=journal`) is, by the author's
   reading and mine, refused by the `STATX_DIOALIGN` check. The design
   §4.1 and §5.1 rest on the same example; the conclusion (keep the
   cache drop) holds either way. For the lead.

## The files here

- `plants.py WORKTREE [name...]`: plants sixteen defects in
  `crates/skyzfs-local/src/inplace.rs` and `inplace/pool.rs`, one at a
  time, runs the in-place tests under a limit and says which tests
  failed, or that the run hung. Each file is put back after its plant.
- `plants.txt`: its output at 1359259.
- `plants-whole-crate.txt`: the survivors re-run against every test of
  `skyzfs-local` and `skyzfsd` (`SKYZFS_PLANTS_SCOPE=whole`), so that
  "caught by nothing" is said of the crates and not of one module.
- `m3-again.sh`, `m3-load.sh`, `m3-whole.sh`, `m3-text.sh` and their
  `.txt`: the plant "a read does not wait for a write it overlaps"
  against the large-page suite: alone (0 of 8), four copies side by
  side (0 of 12), and as part of the whole test binary (fails in some
  runs); each beside the same runs without the plant, which never
  failed.
- `rig-26.04-volume60.tsv`: from the failed job's `c-trace.tsv.gz`,
  volume 60 from the discard to the last outstanding call's return,
  and the two reads the model refuses. Columns: ms, volume, event,
  call, and the call's kind, block and length or its result.
- `dioalign-probe.py DIR...`: whether each directory's file system
  takes an `O_DIRECT` open and what `statx`'s `STATX_DIOALIGN` says of
  it. Here: ext4 accepts the open and says 512/512; tmpfs accepts the
  open and says nothing, so the in-place store's start refuses it by
  the alignment and not by the open.
