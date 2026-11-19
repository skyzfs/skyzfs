# Next look at #130 at 09860a0: F1 to F4 answered, the lead's rule on zdb's abort

Pull request skyzfs/skyzfs#130, head `09860a0`, base `zfs-topology` at
`d646117` (the merge base; the base has not moved). New since the first
look (`review.md`, at `c205682`): `a9222c4` (the answers to F1 to F4,
S1 to S5 and the test gaps) and `09860a0` (the lead's rule: zdb's
signal 6 in the main pass counts against the image). Test code only,
under `tests/kernel/`. I read the code before the author's answer
(issue comment 6997954023), and checked each claim in it against the
code and by running it.

Scenarios: `next_scenarios.py` (output in `next_scenarios.out`). Plants:
`next_plants.py` (output in `next_plants.out`).

## Verdict

**No blocking finding. It can merge once the rigs at `09860a0` are
green.** F1 to F4 are fixed, with tests that kill the plants, and so is
every test gap from the first look. Before the merge I recommend one
small fix, N1 (the S4 cap). I asked for that cap in the first look, and
I was wrong: `-f` cannot clear the kernel's "pool is busy". Without the
fix, a job can again end with the pool left imported. N2 to N4 do not
block.

| | What | Certainty | Blocks |
|---|---|---|---|
| F1 | Fixed. A verdict found before a failed cleanup stays the prefix's verdict, at every cleanup site; the prefixes after it read "still imported"; the job exits 1 | driven | settled |
| F2 | Fixed for both cases raised (a catch the table does not expect, a class it does not name). One residual below (N2) | driven | settled |
| F3 | Fixed. `fuser` only on a mounted file system; its real output parsed right; `ps` matched by command name | driven, on the real `fuser` and `ps` | settled |
| F4 | Fixed: exports counted, not refusals. The words for forced exports are not quite right (N3) | driven | settled |
| N1 | The S4 cap sends a busy export to `-f` at once, but `-f` cannot clear the kernel's "pool is busy". After two long holds, a hold of under a second leaves the pool imported, and the rest of the job is lost. S4 was my suggestion, and it was wrong | certain (OpenZFS 2.2.2 read; driven with a stand-in) | no, but fix it in this PR |
| N2 | A control's rig errors still excuse a missing catch by the table's check when none of the prefixes the rig could not judge would have run that check | driven | no |
| N3 | "K of them still busy after every wait and exported with -f" is said of an export that did not wait, and of one whose `-f` failed | driven | no |
| N4 | The lead's rule counts any signal 6, not only an assertion: a SIGABRT with no assertion line counts against the image. Signal 11 stays the rig's in the main pass, but counts against at the end | driven | no; a question for the lead |

## (1) F1 and F2

### F1: `after_cleanup` (`replay_check.py:665-678`)

Every site that used to replace the result now goes through
`after_cleanup`:

- `light` (`losetup -d`);
- `light_on` (the export);
- `deep` (`losetup -d`);
- `deep_loops` (`dm-snapshot remove`);
- `deep_snapshot` (the export).

`grep` finds no other `res = (...)` replacement. A failure found inside
is kept, with "then X failed: …" added to its detail. A check that
passed becomes the cleanup's failure, as before.

My F1 reproducer, run again with the stand-in `zpool` (N1 in
`next_scenarios.out`): verify fails at 100, and the export stays busy
through every wait and through `-f`.

| prefix | result |
|---|---|
| 100 | `verify` |
| 101 | still imported |
| 102 | still imported |

`exit_code` now gives 1. At `c205682` it gave 2.

Every stage, each followed by a failed cleanup, and the prefix after it:

| inside | cleanup that failed | the prefix's result | the next prefix |
|---|---|---|---|
| mount fails | export | `mount` (a verdict) | still imported |
| verify fails | `losetup -d` | `verify` | (the stand-in's loop) |
| passes | export | `export` (the rig) | still imported |
| scrub finds errors | export | `deep: scrub` | still imported |
| scrub finds errors | `dmsetup remove` | `deep: scrub` | judged |
| scrub finds errors | `losetup -d` | `deep: scrub` | judged |
| fsync files missing | export | `fsync: files` | still imported |
| `zpool status` fails (the rig) | export | `deep: status` (the rig) | still imported |
| passes | export | `deep: export` (the rig) | still imported |

Two things hold, and both are right:

- A rig stage found inside stays the rig's.
- What a failed cleanup left behind still reaches the next prefix.

`replay_daemon.py` uses the same `Rig.light` and `Rig.deep`. The daemon
jobs gain the same fix: there too a verdict followed by a failed export
used to read as the rig's error.

### F2: `rig_errors_explain` (`replay_check.py:867-883`)

My cases again:

- C2, a superseded `drop` caught at 5 with the pool left at 7: exit 1.
- C2b, `reorder` of a class the table does not name: exit 1.

At `c205682`, both gave 2.

N2 in `next_scenarios.out` runs every row of `EXPECTED`, each with the
pool left at the control's last prefix, against three outcomes: not
caught, caught by the table's check, caught by another check.

- Exit 1 in exactly the rows where rig errors cannot explain the
  difference: "not caught" expected, but the control was caught
  (`drop` superseded, `drop` never reached, `reorder` never reached).
- Exit 2 everywhere else.
- A control that did not run, or that has no rig errors, keeps 1 (the
  tests pin both, B04 and B05 below).

**N2 (does not block; certain, driven).** Rig errors at any prefix
explain a missing catch by any check. The table's check might not run
at the prefixes the rig could not judge:

- the every-copy check runs at transaction-group points only;
- the scrub runs at flushes only;
- the fsync check runs at fsync marks only.

So a `reorder` control (the table: by the every-copy check) can be
judged at every point where that check ran, without a catch. If its
export is then refused busy at a later prefix where only the light
check ran, and the pool is left for the last one, `exit_code` gives 2
(N2a in the output). Nothing in the results records which checks a
prefix the rig could not judge would have run, so `rig_errors_explain`
cannot ask.

It needs a pool left imported at a control's tail, after the last point
where its check ran, so it is rare. It cannot make a job pass. A
follow-up could record in each result which checks were due (`copies`,
`deep`, `fsync`), and then excuse a missing catch only when a prefix
the rig could not judge had the table's check due.

## (2) The lead's rule: zdb's signal 6 in the main pass

`exit_code` keeps 1 for a main-pass prefix that failed with
`ZDB_ABORT` (`replay_check.py:906`). The string is exact:
`every_copy_verdict` (`:618`) writes it with no prefix, and `copies()`
runs only in `point`, never under `deep: `. A control still counts it as
a catch (`tally_control`). `replay_daemon.classify` already counted it
against the daemon, so the two rigs now agree.

Can a rig error come through as a fault of the image? The rig's own
causes cannot make signal 6:

- `run()` turns a command past its limit into 124, never a signal. Driven:
  `run(['sleep', '5'], 1)` gives 124 after 1.0 s.
- A command that could not start is 127.
- Killing a process from outside gives signal 9 (the out-of-memory
  killer, a cgroup) or 15 (cancelling the job).
- `zfsutils-linux` 2.2.2 only recommends `zfs-zed`, and the jobs install
  with `--no-install-recommends`, so zed does not run. Each hosted job
  gets a fresh VM, so nothing is left over from another job.

**N4 (does not block; a question for the lead).**

- **Any SIGABRT counts, not only an assertion.** The rule as the lead
  stated it is "zdb aborting on an assertion". The code counts any
  signal 6. Driven: `sh -c 'kill -ABRT $$'` gives `run()` rc -6, and
  `every_copy_verdict` returns `ZDB_ABORT` with an empty detail. That is
  not zdb's assertion, but glibc's heap checks abort the same way.
  - Narrowing it: count only when zdb printed an `ASSERT`, `VERIFY` or
    `PANIC` line, which the detail already extracts. A real assertion
    gives that detail (driven).
  - That would also match how a control counts it.
- **Signal 11 is the rig's in the main pass but counts against at the
  end.**
  - In the main pass, signal 11 (SIGSEGV) and 7 (SIGBUS) stay the rig's
    (exit 2 with a pool left).
  - zdb crashing on a damaged image is as much evidence as an assertion.
  - At the end, zdb `-bcc` killed by any signal (-6, -9, -11) keeps 1,
    because only 124 and 127 are excused. So the out-of-memory killer
    at the end reads as a verdict.
  - Both directions are rare. The lead may want one rule for signals
    in both places.

## (3) F3, F4, S1 to S5, the test gaps

### F3: `Rig.held` (`replay_check.py:1160-1191`)

- **Mounts first.** It reads `/proc/mounts` under the alternate root
  first, and asks `fuser -vm` only when the pool's mount point is among
  them. On a plain directory it records "not asked: not mounted"
  (driven on this machine).
- **`fuser_holders` on the real psmisc 23.7 output.** Run through the
  rig's own `run()`, which merges stderr: header, path line, kernel
  mount line, then the processes.
  - It keeps the processes and drops the header and the kernel mount
    line, for `/proc` and for `/` (105 processes, none of them the
    header).
  - The head is kept now, not the tail.
- **`zfs_processes`, run on real `ps` with stand-in binaries running.**
  - `zdb` and `zpool` match.
  - A shell whose arguments say `zpool export` does not match, so a
    process is matched by its name and not by its arguments.
  - No kernel thread matches (their names, such as `z_cl_iss` or
    `txg_sync`, are not in the set).
  - Linux cuts a command name to 15 characters (`zfs_ids_to_path_x`
    shows as `zfs_ids_to_path`). The four names in the set are short,
    so the cut cannot matter.
  - One false match, which does not matter: a process that names
    itself "zed watcher" matches, and its record's arguments start at
    "watcher". `ps` prints a name with a space, and `split(None, 3)`
    cuts it.
- **Is the set right?**
  - A pool's kernel reference from user space is held by a process
    inside a ZFS ioctl. `zpool` and `zfs` cover what the rig runs. The
    likeliest such holder is one of its own commands killed by `run()`'s
    limit while still in the kernel; it keeps its name and is listed.
  - `zdb` works in user space and holds no kernel reference, but
    listing it does no harm.
  - `zed` would hold one only briefly.
  - Not in the set: `mount.zfs` (used only by `mount -t zfs`, which the
    rig never runs) and `zvol_id` (the pool has no volumes).
  - The kernel's own holds show in no process, and the comment says so.
  - No change needed.

### F4 and N3

- **F4 is fixed.** Each refusal carries the number of its export. The
  summary counts distinct exports, then refused attempts, and lists the
  forced exports with the exit code of their `-f` (`export_forced`).
- **N3 (does not block; driven, N5 in the output).** The line says
  "3 of them still busy after every wait and exported with -f (exit
  codes 0, 0, 1)".
  - The third export did not wait at all (`waited: false`, the cap).
  - Its `-f` failed, so it was not exported.
  - Suggested words: "K went to -f (W after every wait, the rest at
    once); -f exit codes …".

**S7 (suggestion).** `replay_daemon.py` uses the same `Rig.export` but
writes neither `export_busy` nor `export_forced` to its results or its
summary. In the daemon jobs, a busy export shows only in the log, and
there it says "at prefix 0", because the daemon rig never moves
`Rig.at`.

This pull request's replay run at `09860a0` (44445511640) shows it:

- "through skyzfsd, ubuntu-24.04" (job 135297925802) logged four busy
  exports and "through skyzfsd, ubuntu-26.04" (135297925735) one.
- Every one of them reads "export of skzreplay busy at prefix 0: tried
  again in 1 s; its file systems already unmounted; ZFS commands
  running: none; mounts under the alternate root: none", and each was
  exported when tried again.

That is F3's new record on a real kernel:

- the pool already unmounted;
- no ZFS command running;
- so the hold was the kernel's own, as the comment says;
- and none needed `-f`.

### N1: the cap on waiting (S4) (does not block; fix in this PR)

**N1 (certain).** `EXPORT_BUSY_THROUGHOUT_MAX` (`:469`, used at
`:1141`) skips every wait once two exports were busy through all of
them. The comment says this is "so that a cause `-f` clears cannot cost
every export 15 s". But `-f` cannot clear "pool is busy":

- That message is the export ioctl's EBUSY, and the ioctl runs after
  the unmount.
- In OpenZFS 2.2.2, `spa_export_common` returns EBUSY whenever the pool
  holds references, whatever `force` says (`module/zfs/spa.c:6554`).
- `zfs_ioc_pool_export` passes `force` through and does nothing else
  with it.
- `-f` only forces the unmount before the ioctl.

**What follows:**

- With the cap reached, a busy export goes to `-f` at once, and `-f` is
  refused as long as the hold lasts. The pool is left imported, and
  every later check of the job can only say so. That is the failure
  this pull request set out to prevent.
- Driven with a stand-in that refuses `-f` like the kernel (N5): two
  exports busy through every wait whose `-f` then worked, then one busy
  for under a second.
  - With the cap: export exit codes 0, 0, 1, and the pool is left
    imported.
  - Without it: 0, 0, 0.
  - In this pull request's own replay run, all seven busy exports
    cleared within 1 s.

**How likely:** the cap is reached only by exports busy for all 15 s
whose `-f` then worked, so holds that end just after the last refusal.
After an export whose `-f` failed there are no more exports, so the cap
saves no time there.

**S4 was my suggestion in the first look, and it was wrong.** Fix:

- drop the cap (the waits cost time only where `-f` then works);
- or go to `-f` at once only for "cannot unmount ... busy", the one
  busy that `-f` acts on;
- and correct the comment either way.

`replay_daemon.py` uses the same `Rig.export`, so the daemon jobs have
the cap too.

### S1 to S6

- **S1:** done. The comment by `HELD_COPY_LIMIT` cites 798720 and 806912
  bytes with two cuts held, and bound's 80 to 84 MiB, from run
  44439786541. These match the readings in `review.md`.
- **S2:** done. `sleep 0.3` after the last sync, before the held
  reading. 0.3 s is more than the 200 ms in which the status file shows
  a change. The order test pins it (R08 to R10).
- **S3:** done. A held reading without `cut_holes` or `cuts_abandoned`,
  or with a count that is not a number, is ERROR (driven, section A of
  the first look's scenarios run again; J23 and J24).
- **S5:** done as the lead decided. zdb 124 or 127 at the end keeps a
  job with a pool left at exit 2. A job whose failures are all the
  rig's, with no pool left, stays 1, and a test pins it.
- **S6:** deferred to the next cut-rig package, as the author says.

The test gaps from the first look are now pinned:

| plant | what it changes |
|---|---|
| X07b, X07c | the record's `at` |
| X08 | the mounts |
| X09 | what the kernel said |
| X10 | `EXPORT_WAITS` |
| X16 | a leftover seen only in the main pass |
| X23 | `export_busy` in the results |
| R06 | the pause after the create |

All of them are KILLED now (table below).

## (4) The cut rig's half

Changed only as S1 to S3 and R06 asked:

- the comment;
- `sleep 0.3` in `rec_bound`;
- ERROR for a held reading without its keys;
- the order test's `create_pool` stand-in, which now says whether the
  holder was paused.

The first look's table of `bound_lines` gives the same lines, except the
two rows S3 changed from FAIL to ERROR. Nothing else changed there.

## Tests, shellcheck, plants

At `09860a0`:

- Every `test_*.py` under `tests/kernel` and `tests/bench` passes, among
  them `test_replay.py` (53), `test_cut_daemon_check.py` (104),
  `test_cut_daemon_main.py` (38) and `test_cut_plan.py` (11).
- Every `test_*.sh` under `tests/kernel`, `tests/lib` and `tests/bench`
  passes.
- shellcheck 0.11 on every tracked `*.sh` is clean.

Plants (`next_plants.py`, `next_plants.out`): one at a time, the file
restored in a `finally`, each test under a limit. The worktree was clean
afterwards. The first look's plants were redone on the new text, and new
ones were added for what changed.

| area | plants | result |
|---|---|---|
| `bound_lines` (J01 to J21, J22b) | the first look's, on the new text | all 22 KILLED |
| S3 (J23, J24) | `cut_holes` not required; a missing `cuts_abandoned` read as none | both KILLED |
| `rec_bound` (R01 to R10) | the first look's seven, R06 (the pause after the create) among them, and S2's wait left out, after the reading, or 0.1 s | all 10 KILLED |
| `cut_plan.py` (P01, P02) | as in the first look | both KILLED |
| busy export (X01 to X10) | as in the first look; X07 redone as X07b and X07c (its text is in two records now) | all 11 KILLED |
| exit 2 (X11 to X23) | as in the first look, X16 and X23 among them | all 13 KILLED |
| F1 (A01 to A08) | `after_cleanup` always replacing, each of its five call sites going back to replacing, the cleanup's failure dropped from the detail, a passed check whose cleanup failed passing | all 8 KILLED |
| F2 (B01 to B07) | each exclusion dropped, the old rule, a control without rig errors or not run excused, the table argument ignored | 6 KILLED. B06 (the default class changed) SURVIVED: every control has a class, so the default is never used |
| the lead's rule and S5 (C01 to C06) | `ZDB_ABORT` not counted; not a catch in a control; zdb's 124 and 127 at the end not excused, or every code excused | 4 KILLED. C02 (any zdb signal in the main pass counted against) and C06 (zdb `-bcc` killed by a signal at the end excused) SURVIVED |
| F3 (E01 to E12) | fuser on an unmounted directory, the header, the kernel mount line or the path kept, `ps` matched by arguments or not asked, `zed` or `zpool` out of the set, `mounted` not recorded, the words | 11 KILLED. E09 (fuser's tail kept, not its head) SURVIVED: no test output is over 1000 characters, and the holders are read from the whole output anyway |
| F4 and S4 (G01 to G06, H01 to H04) | refusals counted, the export number, `forced` and `export_forced`, `waited`, the `-f` exit code; no cap, a cap of one, busy-throughout not counted, the last refusal not recorded | all 10 KILLED |

In all: 99 KILLED, 4 SURVIVED, 1 NOT APPLIED (X07, redone as X07b and
X07c).

Every test gap from the first look is now closed. Two new small gaps:

- C02: no test has a zdb signal other than 6 in the main pass with a
  pool left. One `exit_code` case with signal 9 and a pool left,
  expecting 2, would pin it.
- C06: no test has zdb `-bcc` killed by a signal at the end. A case with
  `zdb_cc_rc=-6` and a pool left, expecting 1, would pin the lead's rule
  there.

Both depend on N4's answer.

## CI and Codex at 09860a0

Read-only, at `09860a0`, at the time of writing (no events subscribed):

- **Done and green:**
  - on the pull request: `coverage`, `e2e`, `skyzfs-kernel-probe` and
    `skyzfs-nodeb-rig`;
  - on the push: `skyzfs-ci`, `skyzfs-cut-daemon` and `skyzfs-nodeb-rig`.
- **`skyzfs-replay-rig` (44445511640):** all four daemon jobs are green.
  - "through skyzfsd" on 24.04 and on 26.04: exit 0, every checked
    prefix passed, `zdb -bcc` 0. Their busy exports are in S7.
  - The two "write into a hole" jobs.
  - The product and whole-pool layouts, which run `replay_check.py`'s
    `main` and so its exit code and its summary lines, were still
    running.
- **`skyzfs-cut-daemon` (PR run):** the logic job, 26.04-arm and upload
  are green; 26.04 was still running.
- **`skyzfs-cut-rig`:** the reference job and 26.04-arm are green; the
  others were still running.
- **`ci`:** all jobs are green except `test`, which was still running.
- **`skyzfs-daemon-rig`:** all five kernel jobs were still running.
  - The run at `c205682` was cancelled by the new push, so this run is
    the daemon rig's first on this pull request.
- **Codex** reviewed `09860a0` with no findings (the review summary's
  state: `has_any_findings: false`), and its security review completed.
  The author answered its P2 thread (review comment 4623636731).
  Resolution is not visible through REST.
- **No rigs' tables comment from the author yet.**

Before the merge, read the summaries of the product and whole-pool
jobs: the exit line, and the "Busy exports" line if there is one.
