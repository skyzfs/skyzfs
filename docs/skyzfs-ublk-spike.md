# The libublk spike: what the run showed

Work package M1.3b. The design (docs/skyzfs-design.md §4.2, "Library")
builds the transport on the `libublk` crate, pinned to `=0.4.8`, by a
reading of its source. This is the record of the run that replaced the
reading. It is not the binding, and nothing of `skyzfsd` is built on the
spike's program.

**Status: sent to the lead for a decision. No binding code is written
until the lead answers the recommendation at the end.**

## What ran

- The program: `crates/skyzfs-ublk/src/bin/skyzfs-ublk-spike.rs`, behind
  the cargo feature `spike`. A ublk server over a file in `/dev/shm`, one
  queue, depth 64, 4096-byte blocks, 1 MiB largest request, volatile
  write cache, user recovery with reissue. Requests are executed on a
  worker thread and completed on the queue's thread after an eventfd
  wake.
- The script: `tests/kernel/ublk-spike.sh`. No ZFS: the load is `fio`
  with verification, so a failed or lost write shows directly.
- The workflow: `.github/workflows/skyzfs-ublk-spike.yml`.
- The runs: skyzfs-ublk-spike run 1 (42997706015, commit 1534a35) and
  run 2 (42999759799, commit 814e080), each on `ubuntu-24.04` (kernel
  7.0.0-1017-azure) and `ubuntu-26.04` (kernel 7.0.0-1019-azure). So
  every answer below rests on **two runs per image** unless it says
  otherwise; the answers that needed the second version of the spike
  (the driver's own error codes, the nodes, the kill under a load that
  lasts) rest on **one run per image**. The two images agreed on every
  line but timings.

## The answers

### The queue

| # | Question | Answer |
|---|---|---|
| 1 **must** | A request executed on another thread can be completed on the queue's thread after an eventfd wake | **Yes.** A verified `fio` load (random writes of 4 to 128 KiB, depth 32, then a read-back with checksums) ended without error in every run. The eventfd is read by an SQE on the queue's ring with a user data that has bit 63 set. |
| 2 **must** | What wakes the queue's wait with no traffic | **The library's time-out, every 20 000 ms** (two wakes in 45 idle seconds, 20 000 ms apart, on both images). But `UblkQueue::wait_and_handle_io` does not call the handler on it. The binding has to loop on `flush_and_wake_io_tasks` itself, which returns `Ok(0)` then. The spike does. |
| 3 **must** | The buffer of a request | **The queue's own, one per tag, of the largest request's size** (`UblkDev::alloc_queue_io_bufs`). A completion takes the buffer's address and no length: the driver is told the address for the next request of the tag. Reads and writes of 4 KiB and 1 MiB came back as written. So `pump::Queue::take_buf` is "the first `len` bytes of the tag's buffer", and there is one copy per request **only if** the target reads and writes that memory in place. See "what follows" below. |
| 4 | An orderly stop | **Yes**: the queue's thread is told through the same eventfd, completes what the worker holds, and returns in 4 to 8 ms. The device is then reported QUIESCED, exactly as after a kill. `UblkCtrl::run_target` cannot be used: it stops the device (STOP_DEV) when the queue threads end. |

### The control surface

| # | Question | Answer |
|---|---|---|
| 5 **must** | The feature mask before any device exists | **Yes**: `0x7ffff`, reissue is in it. |
| 6 **must** | The four states of a device | **The driver has three, not four.** Added and never started: DEAD, server pid -1. Live: LIVE. Waiting for a server: QUIESCED (also after an orderly stop). Deleted and still held: **DEAD again**, until the holder lets go; then the device is gone. The two DEAD cases are told apart by the device's nodes: never started has `/dev/ublkcN` and no `/dev/ublkbN`; deleted and held has neither, and `/sys/class/ublk-char/ublkcN` is gone too. (One run per image for the nodes.) |
| 7 **must** | What the kernel reports against what was asked | Queues, depth and the largest request: as asked. **Flags: asked `0x18`, reported `0x2605a`.** The driver adds bits (1, 6, 13, 14, 17). The binding must compare under a mask of the bits the daemon sets. |
| 8 **must** | An add under a taken number fails with an error that can be told from others | **Not through the library. Yes from the driver.** See "the two defects" below. |
| 9 **must** | `disown()` right after the add | **Yes**: after it the device stays when the control object is dropped and when the process is killed. A control object that is not disowned deletes its device when dropped. |
| 10 **must** | The asynchronous delete returns at once on a held device | **Yes**: 12 to 18 ms on a device a process held open, 3 to 27 ms on others. "The plain delete is never called" does **not** hold inside the library: see below. |
| 11 **must** | Take-over after `kill -9` with writes in flight | **Yes.** Three kills per image under the verified load, with 1 to 32 requests in flight at the kill (`/sys/block/ublkb3/inflight`). Each time a new process began recovery on the same number, served, ended recovery; `fio` ended with status 0 and `err=0` after its verifying read-back; the kernel log had no I/O error. Begin and end of recovery each took under 1 ms. (One run per image.) |
| 12 | Parameters read back after a take-over | **Yes**, all of them, unchanged (`get_params`). A device that was never started has none. |
| 13 | Discard and write-zeroes | With both limits set, `blkdiscard` arrives as a discard, `blkdiscard -z` and `fallocate -p` as write-zeroes. With no write-zeroes limit, `blkdiscard -z` succeeds and arrives as **an ordinary write of zeros**. |
| 13a | The driver's limit on devices | **Not reached**: as root, 81 devices existed at once with `ublks_max` at 64, and no add was refused up to number 90. The limit does not bound what root adds. The add loop's end is therefore its own bound, not the driver's. |
| 13b | Recovery begun, server died before ending it | The device **stays QUIESCED**; a second begin and a take-over work. No fifth state. |
| 13c | The request flags | `translate::flag` and `translate::op` equal the constants of the header `libublk-rs-sys` 0.1.2 ships, bit by bit (bit 19 is `SHMEM_ZC`, bit 18 `INTEGRITY`). The driver set none of bits 17 to 19 in these runs. |

### Around a kill

| # | Question | Answer |
|---|---|---|
| 14 | From `kill -9` to "waiting" | The server pid reads -1 first and the state is still LIVE; QUIESCED followed **4 to 54 ms later on an idle device** (three kills per image, run 1) and **6 to 17 ms later under load** (three per image, run 2). The look is every 2 ms plus one control command. The kill itself is not timed more closely than about 15 ms. This is without a pool; the rig's own figure with a pool is the testing session's. |
| 15 | Can a client that holds a waiting device be detected without I/O | **Only an exclusive one.** An `O_EXCL` open of the block device fails with EBUSY while another exclusive holder exists, and succeeds at once (no I/O, also on a waiting device) with no holder **and with a plain holder**. So "refused" means held; "opened" does not mean free. On a deleted and held device the node is gone (ENOENT). |

### The build

| # | Question | Answer |
|---|---|---|
| 16 | What the build needs | `libclang-dev`, for `bindgen` in `libublk-rs-sys`. The kernel's ublk header is **not** needed: the crate ships its own. 26 to 39 s on a runner. The crate brings about 38 packages into the lock file, among them `smol` and `serde_json`. |
| 17 | `unsafe` in the binding | The spike has **seven** `unsafe` blocks: two to make a slice of the queue's buffer in the worker, one `Send` for the request that carries that pointer, two writes to the eventfd, one `eventfd()` and one to own its descriptor. The raw control command (below) adds two. |

## The two defects of the library's control path

Both were read in the source after the run showed them, and both are
in `libublk` 0.4.8's `src/ctrl.rs`.

**1. Every failed control command reads as "not supported".**
`ublk_ctrl_cmd` retries a failed command with the legacy opcode
(`ublk_ctrl_need_retry`) and returns the result of the retry. These
kernels have no legacy opcodes, so the retry answers EOPNOTSUPP (95)
and that is what the caller gets: for a number that is taken, for a
device that does not exist, for anything. Sent by the spike itself, the
same commands answer as the driver means them: `-17` (EEXIST) for an
add under a taken number, also under a deleted and held one, at once;
`-19` (ENODEV) for a lookup of nothing.

**2. A failed add deletes the device that holds the number, with the
delete that waits.** `UblkCtrlInner::new` builds the object, then adds;
when the add fails the object is dropped, and its `Drop` deletes "its"
device, which is whatever stands under that number, with `DEL_DEV`.
Seen: a device added by the script under number 6, then the library's
add under 6: errno 95, and device 6 was gone. And on a deleted and held
number the library's add did not return within the limit (10 s and
30 s): it sits in the delete that waits for the holder. The caller
cannot prevent it: the object never reaches the caller, so there is
nothing to disown.

The lifecycle's add loop walks numbers and depends on "taken" being an
answer. Through the library's add, that loop would delete a stranger's
device on every collision and hang on a held one.

Smaller: every lookup (`UblkCtrl::new_simple`) prints `device reload
json failed` on standard error, and a started device gets a file
`/run/ublksrvd/NNNN.json` written by the library.

## What follows for the code that exists

- `lifecycle::DeviceState`: the driver's `DEAD` has to be split by the
  nodes into `Added` and `Dead`, or the two merged. They can be merged
  at little cost: a note is removed before its device is deleted, so
  "this volume's note and DEAD" is a device that was never started (or
  one somebody stopped), and deleting it is right; DEAD without a note
  is left alone already.
- `Control::info` reports flags under the mask of the bits the daemon
  asked for.
- `Control::client_holds`: `Some(true)` when an exclusive open is
  refused with EBUSY; never `Some(false)`.
- `pump::Queue::take_buf` and `skyzfs_blk::Buf`: the queue's buffer is
  the library's `IoBuf`, not a `Buf`. For one copy per request, `Buf`
  needs a way to stand on memory it does not own, or the store has to
  take a slice. Otherwise there is a second copy between the two. This
  is a change in `skyzfs-blk` and is the lead's to decide.
- The pump's 20 s for an unstored position with no traffic is now
  seen, not read.

## Recommendation

Build the queue on `libublk` `=0.4.8`: the fetch, the buffers, the
commit, the eventfd wake, the time-out, and through a control object
that never owns a device, the parameters, the start, and the begin and
end of recovery. All of that ran as the design read it.

Do **not** use the library for three control commands: the add, the
lookup and the delete. Send them from the binding, as the spike's
`raw_ctrl` does: one function of about forty lines over the `io-uring`
crate the library already brings, two `unsafe` blocks. That gives the
driver's own answers and no delete the daemon did not ask for. A
control object for the library is then made for a device that exists
(`UBLK_DEV_F_RECOVER_DEV`, or no flag), and such an object deletes
nothing when dropped.

This is a small io_uring layer of our own, which the lead reserved. The
alternatives, and why not:

- *The library's add after a lookup that found the number free.* A
  stranger that takes the number in between loses its device to the
  failed add. Not acceptable.
- *Let the kernel choose the number (id -1), so no add collides.* The
  number is then known only after the add, and the lifecycle's rule
  "record, then add" cannot hold: a kill between the two leaves a
  device no record names.
- *A patched library.* The two fixes are small (return the first
  error; do not delete on a failed add), but they mean a fork or a
  wait for a release, and a pin to a git revision.

Not yet shown, and to be shown by the binding's first run before
anything rests on it: that a control object made without the add flag
for a device the binding added itself can set parameters and start it
(the source says so: `prep_start_dev` does both for any device that is
not QUIESCED).

## The workflow

`.github/workflows/skyzfs-ublk-spike.yml` is temporary. It is to be
removed before M1.3b's pull request, together with the feature `spike`,
the program and the script, unless the testing session wants the script
as a start for a check of the binding's control commands.
