# The libublk spike: what it must show before the binding is written

Work package M1.3b, first kernel commit. The design (docs/skyzfs-design.md
§4.2, "Library") builds the transport on the `libublk` crate, pinned to
`=0.4.8`, and says that decision "rests on a reading of the source". This
spike replaces the reading with a run. It is a small program, not the
binding: nothing of `skyzfsd` is built on it until every line marked
**must** below has an answer.

It runs in the testing session's daemon rig, on GitHub runners (the cloud
containers have no ublk). Until that rig is on `zfs-topology` this file is
a plan and nothing in it has run.

## The program

One binary in `crates/skyzfs-ublk` behind `cfg(target_os = "linux")` and a
cargo feature, so the workspace still builds without `libublk`, libclang
or the kernel headers.

- A memory target: one `Vec` of the device's size. No store, no log.
- One queue, depth 64, 4096-byte blocks, 1 MiB largest request, volatile
  write cache, user recovery with reissue.
- **Requests are executed on another thread.** The queue thread hands
  each request to a worker over a channel; the worker copies and puts the
  result on a list; an eventfd wakes the queue thread, which completes
  the request. This is the shape the pump needs (`pump::Queue::wait`) and
  the one the crate "describes and no example of it shows".
- Two ways to start: `add` (add a device, disown, start) and `takeover N`
  (begin recovery of device N, serve, end recovery).
- It prints one line per answer below, `spike_<name>=<value>`, for the
  rig to collect.

## What it must answer

Each item says what is asked, how the run shows it, and what follows from
either answer.

### The queue

1. **must** A request executed on another thread can be completed on the
   queue's thread after an eventfd wake.
   Shown by: a pool on the device takes a write load and scrubs clean.
   If not: M1 runs the serial store on the queue's own thread (design
   §4.2), and the pump's `Queue` trait loses its wake.
2. **must** What wakes the queue's wait with no traffic, and how often.
   The source has `UBLK_QUEUE_IDLE_SECS = 20` (`io.rs`). Shown by: the
   time between two returns of the wait on an idle device.
   Why: the pump retries an unstored position at the next wake; with no
   traffic that is this time-out. The pump's docs state 20 s as read,
   not as seen.
3. **must** Whether a buffer handed to a completion needs a length below
   its capacity, or the queue's own buffer is used, for one copy per
   request.
   Shown by: reads and writes of 4 KiB and 1 MiB compared with what was
   written, and the count of copies in the code path.
   Why: `pump::Queue::take_buf` assumes the queue lends a buffer of
   exactly the request's length.
4. An orderly stop: the queue thread can be told to stop from another
   thread (the same eventfd), finish what is in the target, and return
   without the device being deleted.

### The control surface

`lifecycle::Control` is a guess. Each call needs its `libublk` or raw
counterpart, or the trait changes.

5. **must** The feature mask can be read before any device exists, and
   names reissue (`UblkCtrl::get_features`, `UBLK_F_USER_RECOVERY_REISSUE`).
6. **must** A device can be looked up by number without owning it, and
   its state told apart: added and never started, live, waiting for a
   server, deleted and still held. Shown by: the state printed in each
   of the four, the last one with a pool imported on a deleted device.
   Why: `lifecycle::DeviceState` has exactly these four.
7. **must** The size, queues, depth and flags the kernel reports for a
   device, against what was asked at the add. Shown by: both printed.
   Why: the attachment record holds what was asked and the lifecycle
   compares it with what is reported. If bits differ, the binding masks
   to the bits the daemon sets, or the record is completed after the add.
8. **must** An add under a chosen number fails with an error that can be
   told from other failures when the number is taken (`EEXIST`), also
   when it is taken by a deleted-and-held device.
9. **must** `disown()` right after the add: after it, dropping the
   control object, and the process exiting by `kill -9`, both leave the
   device in place.
10. **must** The asynchronous delete returns at once on a device a pool
    holds (`del_dev_async`), and the plain one is never called.
11. **must** Take-over: after `kill -9` with writes in flight, a new
    process begins recovery on the same number, serves, ends recovery,
    and the requests in flight are issued again. Shown by: no failed
    application write, **no error counted on the vdev** (a server that
    does not reissue shows only there), a clean scrub.
12. Whether a device's parameters can be read back after a take-over
    (block size, largest request, discard and write-zeroes limits), so
    that the new instance follows the device and not its own options.
13. Both `max_discard_sectors` and `max_write_zeroes_sectors` must be set
    for the two operations to arrive; shown by a `blkdiscard` and a
    `blkdiscard -z` reaching the queue as their own operations.

13a. The driver's limit on the number of devices (64 by default, the
    `ublks_max` module parameter) against the add loop, which walks
    numbers up to 65535: what an add answers beyond the limit, and that
    leaked and deleted-but-held numbers count against it. The loop must
    end with "no number is free" there, not with another error.
13b. What the driver reports for a device after a recovery was begun and
    the server died before it ended the recovery: one of the four states,
    or a fifth the lifecycle does not know.
13c. The request flags: `UBLK_IO_F_SHMEM_ZC` is bit 19 in the header
    `libublk-rs-sys` 0.1.2 ships; Linux v7.0's header ends at bit 18.
    Check `translate::flag` against the source the binding is built on,
    bit by bit.

### Around a kill

14. The time from `kill -9` to the device waiting for a server, as the
    driver reports it. The testing session measured under about 130 ms
    with a stand-in; the lifecycle waits up to 5 s.
15. Whether a client that holds a waiting device can be detected without
    doing I/O (an exclusive open that fails, or nothing). Decides whether
    `detach` can refuse a held device beforehand.

### The build

16. What the build needs on the runner (libclang, kernel headers) and the
    line for CLAUDE.md. The workflows belong to the testing session: the
    packages are proposed to it, not added from this branch.
17. The count of `unsafe` blocks the binding module will need, each with
    its reason, from what the spike had to write.

## After the run

- Every **must** that holds: the binding is written against the calls the
  spike used, `lifecycle::Control` is given its real implementation, and
  the guesses in its docs are replaced by what was seen.
- A **must** that does not hold goes to the lead before anything else is
  built, with the output. In particular, no io_uring layer of our own is
  written without the lead's word.
- This file is then replaced by a short record of the answers, with the
  run it rests on and how many runs that was.
