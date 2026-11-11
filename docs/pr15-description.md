## Work package

M1.3b: the daemon serves. The ublk binding (`crates/skyzfs-ublk-sys`), `skyzfsd create` and `skyzfsd serve`, built on the lifecycle, pump and records of M1.3a (docs/skyzfs-design.md §4.2). Commits: `1ddbb83..8b809a6` (52 commits on `zfs-topology`: 32 up to 499bdb4, which is what was first reviewed, and 20 that answer the review; "Changed in review" below lists them). The working history, with the merges and the temporary workflow's runs, is on the branch `skyzfs-coding-m1.3b`; the run links below are runs of that branch.

## What was done

- **The binding**, `crates/skyzfs-ublk-sys`, the only crate with `unsafe` (each block says why it is sound; `skyzfs-ublk` and `skyzfsd` keep `forbid(unsafe_code)`).
  - The queue is `libublk =0.4.8`. The binding itself sends add, lookup (info and parameters), asynchronous delete, begin and end of a recovery, with the opcodes and structures of the bindings the library ships, so the driver's own answers come through (EEXIST taken, ENODEV nothing there, EBUSY while a server is still being torn down). The library's control object is made with no flag, only for a device that exists, and sets parameters and starts a device this instance added.
  - `skyzfs-ublk-control set-params` sends the set-parameters command itself, for the rig; the daemon's own still goes through the library's object before a first start.
  - The library writes no file: nothing under `/run/ublksrvd` is written or removed by the daemon. Each control object tries once to read `/run/ublksrvd/NNNN.json` and prints `device reload json failed` on stderr when there is none: expected, not a fault.
  - `run_target` is not used (it stops the device).
  - Flags are reported under the mask of the bits asked for (asked 0x18, the driver reports more).
  - One copy per request: one locked `Buf` per tag, address given to the driver at each commit. The first write of each tag after a start arrives in the library's buffer and is copied once: counted, shown by `status` as `first_write_copies`.
  - **The request buffers are never freed and never unlocked.** A fetch command names a buffer's address, the driver copies an arriving write there on the queue's thread, and nothing in the library takes a fetch command back (its ring is a thread-local; dropping its queue object cancels nothing). So when the queue is dropped the buffers are forgotten, not freed: an address the driver was ever told stays valid memory nothing else uses until the process ends. Cost: nothing for the daemon (one life per process); 2 x depth x 1 MiB per served life in a process that serves several times (the kernel tests, the test program).
  - A device whose largest request is above what the tag buffers hold (1 MiB) is not served: `serve` ends with an error (exit 1) and leaves the device as it is. A device this daemon added always has 1 MiB.
  - The queue has no made-up buffer for a tag: if a tag's buffer is not in its place or is too short, the request fails with EIO and the store sees nothing.
  - At the end of a stop the queue hands the driver its last commits (one submit, no wait). Without it they stayed on the ring and the kernel issued those requests again.
  - `client_holds`: an exclusive open that is refused with EBUSY means held; every other answer, a successful open included, is unknown (`None`), never "not held".
- **`create --dir DIR --size BYTES [--run-dir DIR]`**: a new, empty volume. The id is 16 bytes from `/dev/urandom`; record alignment 4096 and 64 MiB log segments are constants recorded with the volume, not options. It replaces no volume, and it refuses a directory that holds anything but what an interrupted create leaves (`lock`, `home.new`, `home.img`, `log.000000`, `log.super.new`): exit 1, the file named, the directory untouched. Only files under exactly those names are ever removed. A create that fails removes what it made in a directory it made. One line is printed: size, id, and the store's identity. If a failure note of this boot lies under the directory it warns on stderr: the note stays and counts for the new volume. On a directory another instance has locked: exit 5.
- **`serve --dir DIR [--run-dir DIR] [--acknowledge-failure] [--queues 1] [--depth N]`**, in this order: the two deployment rules of design §4.1 (the process's memory locked; neither directory on ZFS, asked of the kernel with `statfs`), each with an `--unsafe-allow-*` override for tests; signals blocked before any thread exists and taken with `sigtimedwait`; the lifecycle's start (lock, failure note, acknowledged position, store, device); the queue's thread, then the start or the end of the recovery; then, once this instance's status file was written or the write was tried (at most 5 s), `ready /dev/ublkbN` on stdout. It ends on SIGTERM/SIGINT (0; 1 if the acknowledged position could not be stored), on a failed volume (3, after the failure is noted and the device deleted), or when the queue goes away (1). Refusals: 4 (failed in this boot), 5 (held by another instance or a live server). After an orderly stop the device stays and waits for the next server. The volume's lock is held until the cleanup after a failure is done: the main thread has a hold of its own on the lock the store took, for the whole of `serve`, whatever becomes of the store on the queue's thread. No write to stdout or stderr can end the daemon or change its exit status.
  - When the device serves, one line on stderr says where the start's time went (lock, position, recovery with the log's bytes and operations, device steps with the driver's busy answers, queue, start). When it ends, `first_write_copies` and `recovered_unacknowledged` are printed.
- **`status`** and **`detach`** ask the real driver, and open it only when the volume has a record of this boot to ask about: after a reboot, before the driver is loaded, both answer (detached, exit 0).
  - `status` shows counters only from a status file of this store (the file of a volume that was in the same directory before is not shown).
  - `detach` exit statuses: 0 only when nothing of the volume is left attached on this node (nothing was there, or deleted and the number free); **6** deleted and a client still holds the device; 1 left as it is because whose it is cannot be told (also the dead device with no note); 5 a server is live or a daemon holds the lock.
  - The driver's delete does not wait, so a number counts as held only if its device is still there 5 s after the delete: a detach, or a start that settles a failure, of a device that is held takes that much longer.
- **A test program, `skyzfs-ublk-control`** (in the binding's crate, built only with the cargo feature `control-test-program`, since it deletes whatever device number it is given): one control step per call (`add`, `info`, `delete`, `begin-recovery`, `client-holds`, `set-params`, `serve`, `take-over`), one line with the binding's own answer, exit 0 whatever the driver answered. For the rig's checks on devices of other servers (M1.T7).
- **Kernel tests of the binding** (`crates/skyzfs-ublk-sys/tests/kernel.rs`, ignored by default: root and the driver), each printing `control_<property> PASS|FAIL`. Ten now. A test fails, and deletes nothing, if its device number is not free when it begins.
- **Status file version 2** (`SKYZSTA2`, 144 bytes): `first_write_copies`, `recovered_unacknowledged` (a lower bound; why is in `Store::recovered_unacknowledged`).
- **The failure note's third value** (unknown), settled by the next start.
- **Test hooks**, only in a build with the cargo feature `test-hooks` (a default build refuses the options): `--commit-delay-ms`, `--hold-one-request`, SIGUSR1 (fails the log), `--fail-log-after`, `--recover-short`, `--boot-id-file`, `create --base-pattern`.
  - `--fail-log-after N` counts completed writes, write-zeroes and discards: the requests that append to the log. Reads and flushes do not count. (The first build counted every request, so the reads of a device's start tripped a small N before the rig's own write.)
  - `--commit-delay-ms N` gives each result to the kernel N ms after the store completed it, and stores the acknowledged position then. The first build stored the position before it kept the result back, so the record covered exactly what a kill left unanswered and the next start counted nothing (the rig's kill test with the delay found 0 over 10 take-overs, run 43028536432). The production path is not touched by this.
- **A fault found by CI**: `Served::fail_now` answered its caller before it woke the owner; `a_served_store_failed_on_demand_fails_every_later_request_and_is_found_marked` failed on three pushes of the working branch (8a773b5, 2a6f8c0, fe29194). Order turned round, test unchanged.

### Decided without being asked (accepted by the lead)

Where the design says no more, the conservative reading was taken and is marked in the code's documentation:

- Settling a failure note of value "unknown": no record of this boot, an unreadable record or an unreadable device note count as **held**.
- A settled value that cannot be written: held for that start.
- The identity `create` prints is the store's.
- `create` on a directory another instance has locked: exit 5.
- No boot id: `create` gives no warning.
- An exclusive open that succeeds says nothing: only a refusal with EBUSY is an answer (held).
- A directory with `home.pre` or `log.failed` and no volume is not a create's leftover: `create` refuses it.
- `ready` waits at most 5 s for the first status write and then is printed with a warning.
- A device that was added and never started reports size 0.
- Signals are taken with `sigtimedwait`, no handler runs.
- "Its own files off any pool it serves" is checked as: neither the volume's directory nor the run directory is on a file system of type ZFS (`statfs`). That refuses more than the rule asks, and it does not see a file system of another type that itself lies on ZFS (on a zvol or a loop file).
- A read is not part of the order of writes (now in design §4.1).

### Changed from the accepted proposal

- `create` makes the directory's last component if missing (mode 0700) and removes it again on failure: the rig creates volumes in directories it has not made.

### Changed in review (499bdb4..8b809a6)

Each is a commit of its own, with tests; the threads on this pull request have the detail.

| What | Commit |
|---|---|
| `create` removes only its own exact names and refuses any other file (blocking) | 1d8eb93 |
| the request buffers are never freed or unlocked (blocking) | 15bd6fd |
| an orderly stop hands its last commits to the driver; the delay hook's backlog at a stop | da67cde, 764b3e5 |
| a number is held only if its device outlasts 5 s of looks after the delete | fd22ab9 |
| a device with larger requests than the buffers is refused; no made-up buffer | 97d3f58 |
| no write to stdout or stderr can end the daemon | e9032f8 |
| a failing `create` removes what it made | 8a71aef |
| `status` and `detach` open the driver only when needed | 5b64c86 |
| the kernel tests: data by place and round, concurrent first writes (and that they were concurrent), I/O in every taken-over life, nothing deleted that a test did not add | 22828f4, 8b809a6 |
| the ZFS rule asks `statfs`; a relative run directory that does not exist | 41c69ae |
| `ready` after the first status write; no zero counters posted after the start | 702ab7b, 9c6f312 |
| the volume's lock held until the failure's cleanup is done: found in review that another `serve --acknowledge-failure` could add a device under the same number in the window, which this process then deleted | 25861f5, defddfc |
| `detach` exit status 6 | 4749be8 |
| `status` and the status file of another volume | 9dc952f |
| the test program behind a feature; documentation | 9f32dbb, 20f8a8c |

### Known costs

- A daemon killed between removing a device's note and its delete leaves this volume's device with no note; no start resolves it.
- A new volume allocates its first 64 MiB log segment at once.
- A detach or a settling start of a device a client holds takes 5 s longer.
- `rustix` (already in the lock file) is a direct dependency of `skyzfsd` on Linux, for `statfs`.
- The build needs libclang (bindgen in libublk-rs-sys); 39 crates enter the lock file: async-channel async-executor async-fs async-io async-lock async-net async-process async-signal async-task bindgen bitmaps blocking concurrent-queue crossbeam-utils darling darling_core darling_macro derive_setters event-listener event-listener-strategy futures-io futures-lite futures-timer home ident_case io-uring itertools lazycell libublk libublk-rs-sys linux-raw-sys parking piper polling rustc-hash rustix skyzfs-ublk-sys smol which.

## How it was verified

**Locally** at 8b809a6 (stable 1.99.1; no ublk in the container): `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean, also `-p skyzfsd --features test-hooks` and `-p skyzfs-ublk-sys --features control-test-program`; `cargo test --workspace`: 1061 passed, 0 failed, 16 ignored; `cargo test -p skyzfs-ublk-sys --features control-test-program`: 5 passed, 10 ignored (the kernel tests); `cargo test -p skyzfsd --features test-hooks`: 41 passed; docs of the skyzfs crates with `RUSTDOCFLAGS="-D warnings"` clean.

**Nothing changed in review has run on a kernel yet.** Since 14:26 UTC on 2026-11-11 the runners could not resolve the Ubuntu mirrors (an outage at Canonical), and every job that installs packages failed in its first step. Owed, in one run when they resolve: the ten kernel tests (three new: a stop under writers, how long a delete takes to free a number, a device with larger requests; three changed), the rig on the daemon, and the stop test against two builds with a defect put back (the buffers freed at a stop; the last commits of a stop not handed over), which must fail it, to show that the test catches what it is for. What follows is the evidence for the code up to 499bdb4.

**On a kernel** (GitHub runners, Ubuntu 24.04 and 26.04, kernel 7.0.0-azure), through a temporary workflow that is not part of this pull request (it ran the binding's kernel tests as root, then the daemon rig with `RIG_SKYZFSD` set to a `test-hooks` build of `skyzfsd` and **no** `--unsafe-allow-*` override):

- The binding's kernel tests, 7. Six passed on both images in run [43028536432](https://github.com/skyzfs/skyzfs/actions/runs/43028536432): the driver can reissue; a free number has nothing; add / second add "taken" / delete; start through the library's object made with no flag, and the device outlives its server; data of 4 KiB and 1 MiB whole in one life and across a take-over (first-write copies 1 and 1 at depth 8, the 1 MiB write arrived as one request); a take-over beside planted files of another server under `/run/ublksrvd` (a server's shape, then garbage), all left unchanged. The seventh, of the test program, failed there on an expectation of the test: `begin-recovery` on a device that was never started answers "busy", not another error. The expectation is corrected; **the corrected test and the `set-params` step have not passed on a kernel yet** (a run of checks a and b at the fix is going; its result is added here).
- The daemon rig on the daemon, checks a to e, three runs, each on both images:
  - [43013952798](https://github.com/skyzfs/skyzfs/actions/runs/43013952798) (first full run): every pin of a to d PASS; 20 of 20 kills under load served again, scrub clean; check c 26 cases, PASS 1, WEAKER 13, NOT_EXERCISED 12, as declared, no violation.
  - [43023027664](https://github.com/skyzfs/skyzfs/actions/runs/43023027664) (with the start's timing line): the same; the timing lines are the figures under "not done".
  - [43028536432](https://github.com/skyzfs/skyzfs/actions/runs/43028536432) (with the rig of M1.T6): 35 pins, 34 PASS, among them `pin_flush_counted`, `pin_held_request_completes` (held 300 s, 0 kernel log lines with an error), `pin_no_ublksrvd_entry_from_daemon` (220 serves). The judge ended with 2, "the rig itself failed", for two ERROR lines: `pin_kill_under_load_applied_unacknowledged` (the fault of `--commit-delay-ms` above; fixed, **not yet seen PASS on a kernel**; the same run of checks a and b will show it) and `ctl_recovered_log_too_short` (open, below).
- Seen in those runs: the library's control object made with no flag sets parameters and starts; dropping it deletes nothing; an add under a taken number answers "taken" and changes nothing; nothing of the daemon's under `/run/ublksrvd`; `recovered_unacknowledged` was non-zero without any delay hook in 8 of 49 take-overs (26.04, run 43013952798).
- In the first two runs two rig lines failed for reasons in the rig, fixed in M1.T6: `pin_flush_counted` on 26.04 (the rig read the status file at once; it may be 200 ms behind by its rule) and `pin_held_request_completes` (kernel log text matched the bare word "timeout").
- Not to be cited from run 43028536432: `pin_failed_unheld_volume_serves_when_acknowledged PASS`. Its write may have gone to a regular file and not to the daemon (below).

**skyzfs-ci** on `skyzfs-coding` at 499bdb4: run 43038572618, success. The runs of the pushes since fail in apt for the outage above. Wall time of a skyzfs-ci run, one run each: 5 min 18 s before the binding's crate (9ebc9a9), 7 min 24 s and 6 min 53 s after (b26feb8, 5c824d3; commits of the working branch). I do not know how much of either was served from a cache.

## Not done, and open questions

- **The daemon never checkpoints yet.** The log grows for as long as a volume is served, every start replays all of it, and the disk fills in the end. Measured in run 43023027664 on 26.04: over 22 take-overs the log grew from 0.4 GiB to 4.3 GiB and the start's recovery from 2.6 s to 34 s (140 to 160 MiB of log a second); lock, position, device steps and the begin and end of the recovery were under a millisecond together, the queue about 46 ms. The rule is in design §5.1 ("When the daemon checkpoints") and is work package M1.3c, which follows at once.
- **Open: `ctl_recovered_log_too_short ERROR: not created or served`**, both images, one run (43028536432), with the control at its new place at the end of the run. The rig waited 120 s for a serve to be ready. The daemon's log says it served (`serving device 3 (added) after 171 ms`, printed after `ready /dev/ublkb3`), and `ublk list` shows device 3 live with the daemon's pid. Cause not established. The testing session's likely cause is in the rig: a check before it writes with `dd of=/dev/ublkb3` after the daemon deleted that device, `dd` creates a regular file of that name, and the next device 3 gets no node. What settles it: one run with the rig's fix (M1.T6b: `conv=nocreat`, a block-device test in the waits, the pin reading the server's write count) merged for the run, with check d before the control. If the node is in fact missing while the driver shows the device live, `ready` has to wait for a node that is this device; nothing is built for that.
- **Open with it: `pin_failed_unheld_volume_serves_when_acknowledged`** has no valid result on the daemon yet, for the same reason and because the hook that fails the log counted reads then.
- The binding's kernel tests are ignored tests (root and the driver); no workflow in this branch runs them. They ran through the temporary workflow on `skyzfs-coding-m1.3b`.
- The test program and its four parse tests are not built by `cargo test --workspace` any more (the feature): a CI step with `--features control-test-program` would cover them. The workflows are the testing session's.
- The stop test cannot see whether a request arrived during a given stop; it reports the writers' counts. Whether it fails with either defect put back is not known until the run named above.
- The ZFS refusal has been tested with an injected answer only; no test here has a real ZFS mount.
- The concurrent first writes assert that the queue's count of first-write copies equals the tags the test's server saw begin with a write, not that it equals the depth: something else reading the device first (udev) takes a tag's first request.
- `begin-recovery` on a device that was never started is answered "busy" by the driver, the same as for a server still being torn down (seen in run 43028536432). The daemon never does that; noted for the design's state table.
- One queue only (`--queues` takes 1).
- The start's timings are on stderr, not in the status file.
- `skyzfs-ublk-control add --size` cannot give a device its size: the driver takes the size with the first start, so the size is the served file's. The option is accepted and repeated in the answer.
- The rig's own checks on devices of other servers (M1.T7) are the testing session's; this package gives them the test program.
- The temporary workflow `skyzfs-ublk-spike.yml` is not in this branch. The daemon rig's own workflow has a `daemon` input since M1.T6.
