# Review of #130 at c205682: the cut rig's bound lines, the replay rig's busy export and exit 2

Pull request skyzfs/skyzfs#130, head `c205682`, base `zfs-topology` at
`d646117` (the merge base; the base has not moved). Two commits:
`baa64cb` (cut rig) and `c205682` (replay rig). Test code only, under
`tests/kernel/`. I read the code before the description.

## Verdict

**Two blocking findings, both in the new exit-2 rule. Both fixes are
small.** The cut rig's half (`bound_lines`, `rec_bound`, the plan's
test) is sound. I found nothing there that blocks.

| | Finding | Certainty | Blocks |
|---|---|---|---|
| F1 | A verdict on a prefix whose export then fails is replaced by `export`, so a job whose image is at fault can end with exit 2 and "Nothing else stands against the image" | high (driven) | yes |
| F2 | `exit_code` excuses every mismatched control that has a rig error, including mismatches a rig error cannot explain (Codex's P2, confirmed and wider) | high (driven) | yes |
| F3 | The busy record cannot name what held the pool in the failure that motivated it, and the summary's "held by" is fuser's header or a cut fragment | high (driven, and checked against OpenZFS 2.2.2) | no, but fix it in this pull request |
| F4 | "Busy exports: N" counts refused attempts, not exports | high | no |

The suggestions and test gaps (S1 to S6) are below the findings. None of
them blocks.

## Defects

### F1. A judging failure followed by a failed export reads as the rig's error, and the job exits 2 (blocks)

`replay_check.py:1137-1139` (`light_on`) and `:1233-1235`
(`deep_snapshot`) replace the result of the checks with
`('export', ...)` when the export after them fails. `deep_loops`
(`:1217-1221`) does the same with `dm-snapshot remove`. This predates the
pull request. Before it, the replacement only changed the wording,
because the job exited 1 either way. `exit_code` (`:812-831`) now reads
that wording. A prefix whose `verify`, `mount`, `deep: scrub` or
`fsync: files` failed, and whose export then failed, is `export` in
`failures`. Every prefix after it is "still imported". `exit_code` sees
no judging stage and returns 2. The summary then says
"**Exit 2: a pool was left imported, the rig's own error and not a
verdict.** ... Nothing else stands against the image."

Driven in `scenarios.py`, C1. A Rig with `zpool` stood in for:
`verify` fails at prefix 100, every export there is busy, and `-f` fails
too, as in run 44421456349. The result:

```
C1 verify fails at 100, the export stays busy: [(100, 'export'), (101, 'rig: a pool named skzreplay is still imported'), (102, ...)]
   job_verdict ok=False; exit_code = 2 (the prefix's verify verdict is gone: replaced by 'export')
```

The two events can be linked. A file that fails its checksum in a mounted
pool makes ZFS post checksum ereports. zed's agents open the pool to
diagnose them. An open pool holds the spa's reference, and an export
refuses a held pool as busy. The retries make this rarer. They do not
make it impossible: 44421456349 shows that `-f` does not help with the
pool's own "pool is busy". A rerun of the same image could repeat it. A
real defect would then be filed twice as "not a verdict".

The same holds for the main pass's `rig: zdb died with signal 6`.
`tally_control` counts that as a catch in a control ("zdb aborts on an
assertion when the pool it reads is damaged"), but in the main pass it
is a rig stage. With a pool left imported it now gives exit 2. That is a
judgment for the lead, so I note it here and do not count it.

**Fix.** Keep the result of the checks when the cleanup fails after
them. For example, in `light_on` and `deep_snapshot`:
`res = (res[0], res[1] + '; then the export failed: ' + ...) if res[0] else ('export', ...)`.
The same goes for `dm-snapshot remove` in `deep_loops`. Alternatively,
record the masked stage in a field that `exit_code` reads. Either way,
add a test that drives `Rig.point` through a failed verify and a failed
export into `exit_code`, and expects 1. The cleanup's failure still
reaches the next prefix as "still imported", so nothing else changes.

### F2. A control's mismatch is excused whenever the control also has a rig error (Codex P2; blocks)

`replay_check.py:826-828`:
`errored = {c['name'] for c in controls if c.get('rig_errors')}` and
`all(r['match'] or r['name'] in errored for r in control_rows)`. The
docstring says "every control that differs from the table differs by
the rig's errors". The code checks only that the control *has* rig
errors. `judge_controls` marks every control with a rig error as a
mismatch (`:757-759`). So a mismatch that the rig errors cannot explain
is excused too. There are two kinds:

- **Caught where the table says not caught.** Examples are a `drop`
  whose class is `superseded` or `never reached`, and a `reorder` that
  never reached. Missing prefixes cannot make a control caught.
- **A class the table does not name** ("nothing: the table does not name
  a ... for this control"). The class comes from zdb on the true image
  (`victim_class`), not from the checks a pool left imported stopped.

Driven in `scenarios.py`, C2. A superseded `drop` is caught by
`copies: differ` at prefix 5, and the pool is left at 7. `exit_code`
returns 2. Without the rig errors the same row reads "caught by the
every-copy check", `match=False`.

A mismatch that the rig errors can explain should still give 2: a
control that was expected to be caught but was not, or that was caught
by another check than the table's. **Fix:** judge each errored control
again without its rig errors. Keep exit 1 when it was caught although
its expectation is "not caught" (`want['by'] is None`), or when its
class is not in the table. A test for each.

### F3. What the busy record says held the pool (does not block; fix it here)

`replay_check.py:1073` runs `fuser -vm ALT/MOUNTPOINT`. `rig_lines`
(`:1582`) shows the first non-empty line of what was kept as "held by".

1. **In the failure that motivated the record, the mount point is no
   longer a mount point.** The message in 44421456349 is
   "cannot export 'skzreplay': pool is busy". In OpenZFS 2.2.2 that
   message comes from the `ZFS_IOC_POOL_EXPORT` ioctl's EBUSY
   (`zpool_standard_error_fmt`, `libzfs_util.c:670-672`). The ioctl runs
   only after `zpool_disable_datasets` has unmounted everything
   (`zpool_main.c:1928`). A failed unmount says "cannot unmount ...".
   `remove_mountpoint` (`libzfs_mount.c:865-884`) removes the directory
   only for a default or inherited mountpoint. This pool's is set at
   create (`replay-rig.sh:322`, `-O mountpoint=/skzreplay`), so
   `ALT/skzreplay` stays as an empty directory. `fuser -m` on a path that
   is not a mount point reports the file system that holds it: the
   runner's disk. So it lists every process with a file on that disk.
   That includes kernel threads, the runner and the rig's own `python3`.
   None of them holds the pool. What holds a spa reference is a process
   in a ZFS ioctl (a `zpool` or `zfs` command, zed) or the kernel. fuser
   cannot show either.
2. **The summary's "held by" never names a holder.** `fuser -v` prints
   its header (`USER PID ACCESS COMMAND`) first. On a short output, the
   first non-empty line is that header. On a long one the record keeps
   only the last 1000 characters (`fu.strip()[-1000:]`), and the first
   line is a cut fragment.

Driven in `scenarios.py`, B1, with the real `fuser` (psmisc 23.7) on an
unmounted directory:

```
export of skzreplay busy at prefix 0: tried again in 1 s; held by: USER        PID ACCESS COMMAND | /tmp/.../w/alt/skz: | root     kernel mount / | root          2 .rc.. kthreadd | ...
B1 ... first lines: ['nvironment-man', '                     root        101 Frc.m claude', ...]
   the summary says: Busy exports: 1, each tried again (the first at prefix 0, held by: nvironment-man). ...
```

The unit test's stand-in prints
`ALT/p:  root  4242 ..c.. udisksd`, which is not fuser's format. That is
why it passes.

The mounts line (`mounts` under the alternate root) is right and useful.
When it is empty, it says that the busy is the pool's and not a mount's.

**Fix:** run fuser only when the mount point is among `mounts`, and say
"not mounted" otherwise. Record the processes that can hold the pool's
reference: `ps -eo pid,etimes,args` filtered to `zpool|zfs|zdb|zed`,
under the same 30 s limit. Make the summary skip fuser's header and use
the head of the output, not its tail. Use fuser's real format in the
test.

### F4. "Busy exports: N" counts refused attempts (does not block)

`held()` appends one record per refused attempt (`:1081`). One export
that is busy throughout gives four records. The unit test's first case
(busy twice, then exported) gives two. `rig_lines` (`:1583`) prints
`len(busy)` as "Busy exports". Count distinct exports (for example by
`at`, or a counter in `export()`). Say how many went to `-f`.

## Asked in particular

### (1) `bound_lines`: is anything judged on a reading that cannot show it?

No. I built the inputs and ran `bound_lines` on each (`scenarios.py`,
section A):

| recording | input | lines |
|---|---|---|
| boundsparse | start held, 8192 chunks; held 896 KiB | premise PASS, copies PASS |
| boundsparse | held 4 MiB − 1 / 4 MiB / 320 MiB | PASS / FAIL / FAIL |
| boundsparse | abandoned, holes dropped | copies FAIL |
| boundsparse | held reading without `cut_holes`, or without `cuts_abandoned` | copies FAIL (S3) |
| boundsparse | held reading is an error text | copies ERROR, errored |
| boundsparse | held reading missing, or start missing | RigError (the rig's error) |
| boundsparse | start `none`/0, or `held`/0 | premise FAIL |
| boundsparse | start chunks not a number | premise ERROR |
| bound | start held/0 or none/0 | premise PASS, copies INFO |
| bound | start held/8192 (the zero fill did not happen) | premise FAIL |
| bound | held reading missing | RigError |

- **When each reading is taken.** The held reading is taken after the
  tenth `zpool sync` and before `rm -f hold.pause`. The holder has been
  paused since before `create_pool`, so the first cut is the one being
  uploaded and is held. `cut_holes held` in that reading can only be
  true while that cut is held: the daemon drops the set once that cut
  is uploaded or let go (`status.rs`, offset 1008). So the held reading
  cannot come from after the let-go. The premise is read from
  `status-start-1.txt`. On #125's daemon that reading said `held`/8192
  for boundsparse and `held`/0 for bound (44378510338), so it is taken
  after the hole set exists.
- **The version.** The gate reads the final `status.txt`. The two other
  readings come from the same instance (bound has one start), so their
  version is the same.
- **A missing file.** Missing files are RigError. A missing held reading
  can occur only for a recording that did not finish, and such a
  recording is never judged (`end` runs the check only after `rec_*`
  returned).
- **The 4 MiB limit.** The "0.75 to 1 MiB in 44378510338" that the
  comment cites was computed from the request log (blocks written before
  the first cut's instant and written again while it was held). It is
  not a status reading. It also covers only the first cut's copies.
  `cut_copied_bytes` is the copies of every held cut. At rest, after a
  sync, the cut that waits adds little, but this pull request's
  cut-daemon run is the first real reading. Read it before the merge
  (S1).

### (2) `rec_bound`: ordering, races, a dead daemon, an unreadable status

- **The order is right**, and the new test pins it: ten writes, each
  synced, then `ustatus status-bound-held.txt` while `hold.pause`
  exists, then the let-go, `load 2` and the export. Plants R01 to R05
  are killed.
- **A failed `skyzfsd status`** goes to `rig_error`. The recording exits
  2 and is never judged. The file then holds the error text, but nothing
  reads it.
- **A daemon that dies during the writes** makes the next `zpool sync`
  fail or time out (`z 60`), which is a `rig_error`. A daemon that dies
  between the last sync and the read leaves its last status, which still
  holds that sync's counts. The `load 2` after it then fails as a
  `rig_error`.
- **Staleness.** The status file shows a change within 200 ms
  (`status.rs`: one 100 ms interval plus one write). The read comes
  right after the last sync. It can miss the last 200 ms of copies. That
  can only lower the copies, and a daemon that copied the hole writes
  would pass the bound two syncs earlier. So staleness cannot hide the
  defect the line is for (S2).
- **Pre-existing, not in the diff:** nothing pins that the pause comes
  before `create_pool`. Plant R06 survives. If the holder released the
  first cut, bound would read FAIL, a verdict in the wrong direction.

### (3) `Rig.export` and `held`: could the retry mask a defect, could `-f` destroy evidence, is the match exact?

- **Masking.** No. An export failure was never a judging stage
  (`JUDGING`), before or after this pull request. The retry only changes
  how often a transient busy reaches `-f`, and it records each refusal.
  The checks of the image ran before the export.
- **`-f` and the evidence.** `-f` cannot change the image. The light
  check imports a read-only loop device (`losetup -r`, `readonly=on`).
  The deep check writes into a throwaway copy-on-write file over a
  read-only loop device. What held the pool is recorded before each wait
  and not before `-f` (the fifth attempt). That would be enough if the
  record could show the holder, which it cannot (F3).
- **The match.** It is a substring, `'busy' in out`, not exact. It
  matches EBUSY in all its forms: "pool is busy", "pool or dataset is
  busy", "target is busy", "Device or resource busy". A timeout
  (`TIMEOUT after 120s`) and an I/O error go straight to `-f`, as they
  should. The pool name (`skzreplay`) and the mount point contain no
  "busy". It is good enough.
- **Time.** One export that is busy throughout costs 15 s. A pass whose
  every export is busy (a cause that `-f` clears) would no longer finish
  within the reference job's 45 minutes. Before this pull request it
  passed silently (S4).

### (4) `exit_code`: exit 2 where the image is at fault, and the reverse

- **Image at fault, exit 2:** F1 (a verdict masked by a failed export)
  and F2 (a control mismatch that rig errors cannot explain). Nothing
  else found. A pool left imported cannot itself cause a judging
  failure: `light` and `deep` look for it before they import, and
  `copies` runs only after `light` passed.
- **The rig at fault, exit 1** (S5):
  - zdb at the end timed out (124) or could not start (127). The module's
    own rule says that is never a verdict (`scenarios.py` C3).
  - A failed export alone, with no pool left. That is a rig stage, and
    the lead's decision names only a pool left imported.
  - A pool left imported with the witness disagreeing correctly stays 1
    (C6).

## Suggestions and test gaps (none blocks)

- **S1.** Read `cut_copied_held` in this pull request's cut-daemon run
  (both images) before the merge, and put the figures in the notes. Only
  that run shows whether 4 MiB holds a margin over the copies of every
  held cut.
- **S2.** Judge the held reading's `age_ms`, or wait a quarter of a
  second before the read, as `end_daemon` waits a second.
- **S3.** In `bound_lines`, read a missing `cut_holes` or
  `cuts_abandoned` in the held reading as ERROR, not FAIL. The same daemon
  cannot produce it today, but it is a reading that cannot show the
  property.
- **S4.** Bound the waits per job: after two exports busy throughout,
  go straight to `-f` and say so.
- **S5.** Treat zdb's 124 and 127 at the end as the rig's in
  `exit_code`. Whether every job whose failures are all rig stages
  should exit 2, with or without a pool left, is for the lead to decide.
- **S6.** No test of `main()` in `test_cut_daemon_main.py` covers bound
  or boundsparse at status version 17. A main-level fixture would pin
  that a missing held reading ends in the rig's error and not in a
  silent pass.

## Tests, shellcheck, plants

At the head, in a worktree:

- Every `test_*.py` under `tests/kernel` and `tests/bench` passes:
  `test_replay.py` 51, `test_cut_daemon_check.py` 104,
  `test_cut_daemon_main.py` 38, `test_cut_plan.py` 11 (the cargo closure
  test ran, not skipped, with `--locked`), and the rest.
- Every `test_*.sh` under `tests/kernel`, `tests/lib` and `tests/bench`
  says "all passed".
- shellcheck 0.11.0 on every tracked `*.sh`: clean. Version 0.9.0 still
  has older notes that the repository's own scripts already carry.
  `SC2329` in the new disable line exists only from 0.10 on.

Plants (`plants.py`, `plants.out`): one at a time, each file restored in
a `finally`, each test under a limit. The worktree was clean afterwards.

PLANTS_TABLE

## CI and Codex at the head

CI_STATE
