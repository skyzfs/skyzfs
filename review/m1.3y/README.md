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

From the code nothing blocked the merge. One thing blocked, from CI:
the daemon rig's ubuntu-26.04 job failed check c at this head, and its
cause was open when these notes were written.

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
trace is a cause above the daemon (possible, not shown, and weakened by
what follows the list):

1. The tool's write is one `write_all_at` on the device opened with
   `O_DIRECT`. A direct write to a block device that cannot drop a
   cached page of its range is done through the device's page cache
   instead (`blkdev_direct_write` returns 0 on `-EBUSY`, and
   `blkdev_write_iter` goes on with the buffered write; `block/fops.c`
   at v7.0).
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

The first version of the review named a trigger for step 1: udev's
probe after the tool's `blkdiscard` (`#5`, 2 ms before), reading the
device's tail through the cache, where the suite's window is. That was
wrong: the invalidation before a direct write fails on a dirty page or
a busy buffer, not on a page a reader holds. Testing then ran the check
the review had asked for (run 43443374413: 12,000 direct writes of the
two shapes, alone, after a discard with udev's change events, and
beside a buffered reader of the tail): every call reached the device as
one request and `direct_write_fallback` was never called. Nothing in
the suite was found that would dirty the device's cache. So the
sequence fits the trace but is not likely, and whether the failure is
this pull request's is open.

Also considered, and not made to fit: a short result committed by a
server (the pump commits a whole length or an error); a late write of
`#20`'s data (only the second server had it); the tag buffers (each the
largest request's size, back with its completion).

What should tell the cases apart, in testing's traced runs of check c
(with the daemon's request log): on `ublkb0` the write at sector 131016
issued as `+56`, or as `+24` and then `131040 + 32` (cut above the
daemon), or requeued after a completion (a short result from a server);
and, if the third server was handed all seven blocks, whether the first
three were issued to the disk under `home.img`.

### What the tests pin

1. Of eight defects planted in the ordering (`must_wait`, `accept`, the
   drain), three are caught by no test and one only by chance:
   - a read neither waits nor is waited for: nothing failed (one run);
   - a write does not wait for a read it overlaps: nothing fails;
   - a flush waits only when everything on the threads is a write:
     nothing fails;
   - a read does not wait for a write it overlaps: the conformance
     suite fails in some runs of the whole test binary (4 of 9; the
     large-page run twice, the 4 KiB run twice) and in none of 20 runs
     of the large-page test alone. The case is
     `failed_volume_fails_every_request`, "read #6: block 6 is Torn"
     (`m3-failure.txt`): a write of block 6 and a read of it are in
     flight when the disk fails, a failing write can leave a torn
     block on the simulated disk, and the read shows it when it runs
     beside the write.
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
  side (0 of 12), and as part of the whole test binary (2 of 4, then 1
  of 4 with the text kept); beside the same runs without the plant,
  which never failed. `m3-failure.txt` is the kept failure.
- `rig-26.04-volume60.tsv`: from the failed job's `c-trace.tsv.gz`,
  volume 60 from the discard to the last outstanding call's return,
  and the two reads the model refuses. Columns: ms, volume, event,
  call, and the call's kind, block and length or its result.
- `dioalign-probe.py DIR...`: whether each directory's file system
  takes an `O_DIRECT` open and what `statx`'s `STATX_DIOALIGN` says of
  it. Here: ext4 accepts the open and says 512/512; tmpfs accepts the
  open and says nothing, so the in-place store's start refuses it by
  the alignment and not by the open.

## The second look (head 1e5d4f6)

The comment is
https://github.com/skyzfs/skyzfs/pull/58#issuecomment-6857116836.
Nothing new blocked. The author's answers held: the seven plants that
no test caught, or that hung the run, each fail a test now and none
hangs (`plants-1e5d4f6.txt`). The red rig job still blocked, its cause
open.

The head adds `skyzfsd serve --request-log PATH` (the test build only):
a record of what the kernel handed the daemon and what it was given
back. What its `sent` line means, from reading the wrapper, the binding
and libublk 0.4.8 under it:

- The commits go to the kernel at the start of the queue's wait and the
  line is written when the wait returns. A `commit` followed by `sent`
  was given to the kernel; a `commit` with no `sent` (a kill inside the
  wait) may have been too. The module text at this head says the
  opposite of the second half.
- It is what the kernel was given, not what the driver took: the
  binding keeps a command's completion only when its result is OK and
  drops every other without a trace, so a refused commit is invisible.
- At a stop (not a kill) the library drops commits once it has seen an
  abort, and the record still counts them.
- A request that arrives during the pump's last flush has no `recv`
  line.
- The record costs two to three `write` calls a request on the queue's
  thread, so a failure rate with it is not the rate without it.

Leaking the store in the tests' `drained` (when a request does not end
within 10 s) is acceptable: the failing path only, and what is left is
idle. Any other panic while I/O is held still makes the store's drop
wait for ever; a guard that opens the simulated disk's gate when it
drops would cover that.

The first look's gates ran on rustc 1.97.0 (July), not the current
stable: `rustup update stable` had not taken. The second look ran on
1.100.0. Check `rustc --version` after the update, not before it.

## The third look (head 697ba46)

The comment is
https://github.com/skyzfs/skyzfs/pull/58#issuecomment-6857625473.
Nothing blocked from the code. The request log now writes `send N`
before the wait or flush that hands commits to the kernel, and its text
says what the record cannot show. `look3.sh` runs the gates on what
changed and two plants (`send` written after the wait, after the
flush); `look3.txt` is its output: both plants fail
`the_send_line_is_written_before_the_commits_go` and nothing else.

One exception to the new text, from reading libublk 0.4.8: when the
ring is full the library submits at once from inside a commit, and the
ring has only as many entries as the queue's depth, one of which the
binding's wake poll holds until the next wait. A turn that commits on
every tag after a wake therefore hands all but the last commit to the
kernel before their `send` line. The exact statement is: a commit is
with the kernel no earlier than its own `commit` line and no later than
the `sent`.

Left for a change of its own: the binding reporting ublk command
results other than OK, which it drops today.
