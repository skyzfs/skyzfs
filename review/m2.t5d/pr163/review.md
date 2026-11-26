# Review of #163 (M2.T5 follow-ups, second round) at fafc771

Pull request: skyzfs/skyzfs#163 (`skyzfs-testing` into `zfs-topology`).
**Head: fafc771** (checked with `gh api .../pulls/163 --jq .head.sha`
at the start and again before writing this). I read the diff
(`git diff origin/zfs-topology...fafc771`) before the description.

Two commits:

- 94400f7: the work. Seven files under `tests/kernel/`: the agent rig's
  courses (`agent-record.sh`), its judge (`agent_check.py`), the course
  hooks in `cut-record.sh`, the CLI recorder (`zfs-cli-record.sh`) and
  the tests of all four.
- fafc771: the merge of `zfs-topology` at 1d14ee9. It brings one line of
  `docs/skyzfs-impl-notes.md` and nothing else. The merge base is
  1d14ee9, the tip of `zfs-topology`.

Line numbers are at fafc771. `review/m2.t5d/review.md` is the review of
#143 (also M2.T5d), so this one is in `pr163/` beside it and does not
replace it. The plant specs and their outputs are in `pr163/` (how to
run them is at the end).

## Verdict

- **Not ready to merge. One finding blocks (B1).**
- **The agent rig's k did not pass on fafc771.** It met the D7 path in
  the wild, and the path did not do its job:
  1. A node B read, 0.2 s after the store was thawed, failed. That was
     the rig's error (B2, older than this delta).
  2. The daemon was down at that moment: restart had killed it and had
     not yet served it again. So `agent_quiet`'s export could not finish.
  3. Node B's stages refused the check ("a pool named skzcut is still
     imported"), so D7's aim was missed.
  4. The job then hung for 67 minutes, until it was cancelled.
- **B1 needs a change of code:** `agent_quiet` must bring a daemon back
  before it exports. `cleanup`'s export should run under `lim`, as
  CLAUDE.md asks. Then k must be dispatched again and be green.
- **B2**, the store's thaw that does not wait for the store to answer,
  is a one-line change to the workflow. Fix it here, or the next k may
  stop the same way.
- **Everything else holds**, and none of it blocks:
  - D9 and D10, the judge's G-a to G-c, and the recorder's G1 to G3, N3
    and N-a are right.
  - Every one of #159's surviving plants that still applies is killed.
  - The gaps (G1 to G3) are tests and paths that are not pinned.
  - D-1 is a wrong comment.
- **Every other run on fafc771 is green**: ci, coverage, e2e, the publish
  rig, the cut rig on four images, cut-daemon, node B, and the agent
  rig's a1, a2 and d. The daemon, replay and probe rigs were cancelled,
  which is right.
- **Nothing here needs a decision of design.** G3 (whether a daemon that
  never gets ready is a verdict) is the author's call.

## Findings, most severe first

### B1. With no daemon serving, `agent_quiet` cannot export, and the k job hung (blocks)

Where: `agent-record.sh:657-683` (`agent_quiet`), and
`cut-record.sh:334` (`cleanup`).

**What happened** in run 45828678468, job k, on fafc771. I read the job
log and the artifact `agent-rig-ubuntu-26.04-k`.

| time | what |
|---|---|
| 13:28:06 to 13:30:01 | 16 cases, trace to K-restart-release-after-1, each with its pins ok |
| 13:30:01 to 13:30:04 | case K-restart-released-before-1 (restart): `asked:after` reached, then `akill_serve` killed the 16th instance, and the store was thawed (mark at 444.187 s) |
| 13:30:04 | `snaprow "$t" between.txt` failed: node B's `snap-row`, "refs: backend error: get: dispatch failure". The rig's own error. `serve_again` had not yet run: there is no `serve-17.log` |
| 13:30:04 to 13:32:12 | `agent_quiet`: `session_end` (mark at 444.902 s), then `lim 120 zpool export`, which `lim` gave up after 120 s. `judged-export.log` is empty, and the log says "skzcut did not export for the check" |
| 13:32:12 | the check: the 16 cases judged, 99 ok and no FAILED, then "rig_error: a pool named skzcut is still imported", exit 2 |
| 13:32:12 to 14:39:21 | no line at all, until the job was cancelled. A k job takes about 26 minutes (runs 45800708400 and 45827334221) |

**Why.** `agent_quiet` exports "as `agent_end` does". But `agent_end`
always runs with a daemon serving, and `agent_quiet` runs after any
error, including those that come while no daemon serves:

- in restart, between `akill_serve` (`:438`) and `serve_again`
  (`:442`), which is where this one came;
- in late, between `akill_serve` (`:596`) and `serve_again`;
- after a hook's kill in dhook, until `serve_again`;
- in `serve_start`'s new path (`cut-record.sh:402-411`): every
  `daemon_ready` verdict comes from a daemon that has ended. So that
  path always reaches `agent_quiet` with no daemon. That verdict would
  still be read, but only once the job's limit of 150 minutes ended it.

With no server, the device holds the pool's writes. So the export cannot
finish, and node B refuses to judge while the pool is imported.

**The hang.** After the check, `rig_error` exits 2, and `cleanup` runs:

- `SERVE_PID` is empty, so there is no daemon to stop.
- Then `timeout 120 zpool export -f "$POOL"` (`:334`) runs, behind the
  export that `lim` gave up on. `timeout(1)` waits for its child, which
  CLAUDE.md warns of for exactly this case.
- No other step of `cleanup` touches the pool.
- The log has no line after the check, so the exact command is not
  certain. It is almost certainly this one.

The same hang was possible before this delta, for any rig error while
the daemon was down. But D7 is the code meant for this path. Its first
real use missed node B's stages, and it left the job to hang.

**Certainty:** high on what happened and why the export failed. Medium
on which command hung.

**Fix:**

1. In `agent_quiet`, before the export: if no daemon serves
   (`[ -z "$SERVE_PID" ] || ! kill -0 "$SERVE_PID"`), serve one again,
   as `recover` does.
   - Its failure must be logged, not fatal: `rig_error` there would end
     the recording before the check.
   - Then the export can finish, and node B judges.
2. In `cleanup`, run the export under `lim 120`, not `timeout 120`, so
   that a pool that cannot export ends the job with its exit instead of
   hanging it.
3. A test of `agent_quiet` with no daemon, which must serve one before
   the export.
4. Dispatch k again on the new head. It must be green.

### B2. The thaw does not wait for the store to answer (should fix; workflow; older than this delta)

Where: `.github/workflows/skyzfs-agent-rig.yml:227`,
`CUT_S3_UP="docker start minio"`.

- The first start of MinIO polls `/minio/health/live` until it answers
  (`:180`). The thaw does not. `docker start` returns when the container
  starts, not when MinIO listens.
- In B1's case, node B read 0.2 s after the thaw, and the read could not
  connect ("dispatch failure").
- `snaprow`'s `|| true` (`agent-record.sh:441`) does not help: `snaprow`
  calls `rig_error` itself on a failure of node B (`:366`). So a read
  that is too early ends the course. Late's loop of reads (`:594`) has
  the same exposure.
- It is rare: in this run, 15 restart cases thawed and read before it
  without trouble. But #159's k (run 45800708400) made 21 such reads
  right after a thaw, so each k run has about 20 chances.

Certainty: high that the thaw does not wait; medium that this was the
cause (the error is the S3 client failing to connect, 0.2 s after
`docker start`).

Fix: make `CUT_S3_UP` wait as the first start does, for example
`docker start minio && for i in $(seq 1 60); do curl -sf http://127.0.0.1:9000/minio/health/live && exit 0; sleep 0.5; done; exit 1`.

### D-1. The reason given for `akill_serve` is wrong (should fix; comment and description only)

Where: `tests/kernel/agent-record.sh:275-278`, and the description's D10
bullet on `kill_serve`.

Both say that `kill -9` of a child not yet waited for succeeds, so that
before this change a daemon that had died by itself "went unseen". bash
does not work that way:

- bash reaps a background child as soon as it ends, with no `wait`. A
  `kill` of it afterwards fails with "No such process". `wait` still
  gives its status.
- Checked on bash 5.2.21, the version of Ubuntu 24.04:
  `bash -c 'sleep 0.1 & p=$!; sleep 1; kill -9 $p; echo $?; wait $p; echo $?'`
  prints `kill: (…) - No such process`, then `1`, then `0`.
- So at c48ed10, `kill_serve` (`cut-record.sh:968-972`, unchanged) did
  see a daemon that had died by itself. Its `kill -9 … || rig_error`
  made it the rig's error, exit 2.

What really changed is that such a daemon is now a verdict
(`daemon_serving`, exit 1) and no longer the rig's error. The new code
is right either way, because it judges `wait`'s status and not the
kill's. Only the stated reason is wrong. A later reader may trust it and
"fix" `kill_serve` to match.

Certainty: high.

Fix: in the comment and the description, say that before, a daemon that
had died by itself made `kill -9` fail and so was the rig's error. Now
its status makes it a verdict.

### G1. Unpinned paths of D9 and D10 (gap; low)

The new code is right on each of these paths, but no test pins it. Each
plant below changes one path and passes every test (Plants, below):

- **X06b: dhook exempt from the verdict whether or not the case had an
  error of its own.** This one matters most: a regression would hide a
  verdict. A daemon that died by itself in a dhook case would then pass
  with no verdict.
  - `test_agent_record.sh:183-198` tests dhook with an error (no
    verdict), and three other scenarios with an error (a verdict).
  - Missing: dhook with an empty `rig-error.txt`, which must give
    `daemon_serving`.
- **X04 and X04b: late's kill.** Late with `kill_serve`, or with no kill
  at all, passes.
  - `late_t` (`test_agent_record.sh:584-603`) drives only the two paths
    that stop early.
  - Missing: one call with `snaprow` succeeding. Its stub `akill_serve`
    is already there, and it would want
    `…;thaw;akill;serve_again;kill -CONT;await publish 700`.
  - Restart's use of `akill_serve` is pinned (X03 is killed).
- **Y09 and Y10: `end_daemon`'s hang path.** A hung stop sent to
  `rig_error`, or not killed, passes.
  - `ed_t` (`:683-703`) has only a daemon that ends 1 on SIGTERM.
  - Missing: a stand-in that ignores SIGTERM, which must give
    `daemon_stopped` and then go on.
- **Z08: the judge reads `cases/course`.** It does so only because its
  loop takes every directory under `cases/`.
  - The shell test pins the writer side (X15 is killed).
  - Missing: one case in `test_agent_check.py` with
    `cases/course/verdicts.tsv`, which must give
    `pin_course_daemon_stopped` FAILED and exit 1.
- **X25 and X26: `agent_quiet`'s bounds.** An unbounded `wait` on the
  writer or the agent passes.
  - Missing: a test with a `kill` stub that ignores SIGKILL, beside a
    child that sleeps, could pin both.
- **X12: `asettle` writes `snapshots-after.txt`.** A regression is
  visible anyway: the judge then stops with the rig's error.

Certainty: high (each plant was run alone, with a 600 s limit). None of
these is wrong today, and only X06b and Z08 could hide a verdict if they
regressed.

### G2. A daemon that ended 0 by itself before the rig's SIGTERM passes as a clean stop (gap; low)

Where: `cut-record.sh:585-592` (`end_daemon`) and
`agent-record.sh:264-272` (`reserve`).

- Both send `kill -TERM "$SERVE_PID" 2>/dev/null || true`. If the daemon
  has already ended, the kill fails, and the `|| true` drops the
  failure.
- `serve_end` then reads its status:
  - not 0: the verdict `daemon_stopped`, with a message that says "on
    SIGTERM";
  - 0: read as a clean stop, with no verdict.
- Scenario: the serve loop returns `Ok` when something outside ends it
  (a device deleted, a lease lost), after the course's export and
  before the stop. In an agent course that should be a verdict, as
  `akill_serve` and `recover` now make it.
- The window is small:
  - In `reserve`, the case before has just seen the daemon alive in
    `recover`.
  - In `end_daemon`, a daemon that died before `agent_end`'s export
    makes the export fail. That is the rig's error (exit 2), not a
    pass.

Certainty: high that the path exists; low that it matters.

Fix, if wanted: in `end_daemon`, replace `|| true` with
`|| daemon_wrong daemon_stopped "skyzfsd serve had ended before SIGTERM"`,
with the status read after. In `reserve`, do the same through
`verdict`.

### G3. A daemon that does not get ready is still the rig's error in an agent course (gap; low; for the author)

Where: `cut-record.sh:414` and `:422-423`.

- An instance that ends before it is ready is now the verdict
  `daemon_ready` (`:402-411`), as the description says.
- Two failures are still `rig_error`:
  - an instance that is alive but does not print `ready` in 120 s;
  - one that gives no status with `cut_state` in 10 s.

  A take-over that hangs is as much the daemon's fault as one that
  exits.
- The hook still runs, so the cases before it are judged. The only
  effect is exit 2 instead of 1.

Certainty: high. Whether this is worth a verdict is the author's call.

### G4. node B's `zpool list` waits with no limit after its timeout (observation; low certainty; older than this delta)

- With B1 fixed, one unbounded wait is left on that path. Node B's
  `snapshot_files` runs `zpool list` through
  `subprocess.run(…, timeout=60)` (`nodeb_check.py:316-320`). On a
  timeout, Python kills the child and then waits for it with no limit.
- Whether `zpool list` can block behind a stuck export depends on the
  kernel's locking, which I could not read here.
- In B1's run it did not block: node B's check of the pool returned at
  once.

No change is asked of this pull request.

### Nits

- **N-a. "Each step is bounded" is not quite true.** The thaw in
  `agent_quiet` (`agent-record.sh:667`) runs `bash -c "$S3_UP"` with no
  limit. The workflow's `S3_UP` is `docker start minio`, which ends in
  practice; B2's fix would bound it too. Either put it under `lim 90`,
  or say "each step but the thaw".
- **N-b. A rig error in `end_daemon` after a verdict runs `agent_quiet`
  on a course already ended.**
  - The case: `serve_end 10` fails after the SIGKILL
    (`cut-record.sh:589`), or `detach` fails (`:594`).
  - The hooks are still set at that point: `end()` clears them at
    `:635`, after `end_daemon`.
  - So `agent_quiet` writes a second `session-K-end` mark and tries to
    export a pool that is already exported. It logs the failure.
  - Harmless: the agent's judge reads no session marks, and the verdict
    in `cases/course` still ends the check 1.
- **N-c. The restart and destroy bullet in the description.** It says
  "with a kill before `asked`, nothing". The code (`:446`, `:478`)
  gives no verdict with any kill. A kill after `asked` reaches it first,
  so the two say the same in practice.

## Checks

### 1. D7: `agent_quiet` (`agent-record.sh:651-683`)

- **What it does**, in order:
  1. SIGCONT and SIGKILL to the agent, up to 5 s for it to go, and a
     `wait` only if it went.
  2. The store thawed, and the watcher's pause removed.
  3. The writer asked to stop, up to 60 s, then SIGKILL, and a `wait`
     only if it went.
  4. `session_end`.
  5. `zpool export` under `lim 120`, logged either way.
- **It assumes a daemon serves.** It does not when the error came
  while none did, and then the export cannot finish (B1).
- **Bounded:** yes, but for the thaw (N-a).
  - An agent stuck in D state is left, with no `wait` (`:663`).
  - SIGKILL to the writer's subshell may leave its `dd` running. If
    that `dd` is stuck on the pool, nothing could kill it anyway.
  - The agent's own `zfs` children are not killed either, as in
    `recover`. They end by themselves.
- **Never fatal:** the hook runs on the left of `||`, both in
  `rig_error` (`cut-record.sh:249`) and in `cleanup` (`:324`, under
  `set +e`). So errexit is off inside it, and none of its steps ends the
  recording. It calls nothing that calls `rig_error`.
- **Cannot hide a verdict:**
  - It runs before `$CHECK`, one step after the other, and writes no
    case file.
  - The judge reads every `verdicts.tsv` first
    (`agent_check.py:591-600`).
  - Any FAILED line ends it 1 (`:952-953`), whatever RigError comes
    after.
- **Plants:** X16 to X20, X22 to X24 and X27 are killed. X21 survives
  and is near-equivalent: without the stop file, the writer is killed
  after 60 s instead of ending by itself. X25 and X26 survive (G1).
  U05q (no SIGCONT) is equivalent, since SIGKILL also ends a stopped
  process.

### 2. D9: recover's exemption and `lim` (`agent-record.sh:295-319`, `:470`, `:523`)

- A daemon gone at the end of a case is the verdict `daemon_serving`,
  with its status. The one exemption (`:311`) needs both of these:
  - the case is dhook;
  - the case wrote its own error first.

  That is right: in every other scenario, the rig never stops the
  daemon.
- destroy's release and destroy, and gone's destroy, run under
  `lim 60`. Their output goes to a file, not to a command substitution,
  as CLAUDE.md asks. A failure is the case's error, and its log is kept
  as `destroy-failed.log`.
- **Plants:** X05, X06, S18r and U14 to U16 are killed. X06b survives
  (G1).

### 3. D10: the daemon's status as a verdict

- **`akill_serve`** (`:279-285`, used by restart at `:438` and late at
  `:596`):
  - Only status 137, the rig's SIGKILL, passes.
  - Any other status is `daemon_serving`. That includes an empty PID:
    `wait ""` returns 1.
  - The reason in its comment is wrong (D-1).
- **`serve_start`** (`cut-record.sh:402-411`):
  - An instance that ended before it was ready is `daemon_ready`,
    through `ON_DAEMON_WRONG`. The verdict goes to the case under way,
    or to `cases/course` if none is.
  - It is then the rig's error, so the hook judges.
  - Outside an agent course, nothing changes.
- **`end_daemon`** (`:585-592`):
  - A stop that hangs, or that ends with a status other than 0, goes
    through `daemon_wrong`. In an agent course that is the verdict
    `daemon_stopped` in `cases/course`; elsewhere it is `rig_error`.
  - `SERVE_LOG` names the instance's own log.
  - The one status read as ok that should not be is 0 before the
    SIGTERM (G2).
- **`cleanup`** (`:312-326`):
  - It acts when a recording ends nonzero with the hook still set:
    through errexit, or through an `exit` that did not pass through
    `rig_error`.
  - It clears the hook, writes the "rig error" line, then calls the
    hook. Status 1 from the hook stays 1; anything else becomes 2.
  - The outer loop (`:2066`) then finds the line, and does not write a
    second one.
  - A failure where bash ignores errexit goes on as before: in a
    function on the left of `||`, or in an `if`. That is not new.
- **The hooks are cleared** in `end()`, after `end_daemon` and before
  the check (`:635`). So a failure of the check itself is not judged
  twice.
- **Plants:**
  - Killed: X01 to X03, X13 to X15, Y01, Y03 to Y08, Y11, Y12 and Y14.
  - Y02 (the hook not cleared before `cleanup` runs it) survives. It is
    near-equivalent: nothing the hook runs calls `rig_error`.
  - Y13 (`SERVE_LOG` not set) survives. It affects only the message.
  - X04, X04b, Y09 and Y10 survive (G1).

### 4. The judge: G-a, G-b, G-c, K21, K22, N1, Q06 (`agent_check.py`)

- **G-a** (`:752-767`): a retry that published under T's name gives
  `pin_ID_name_not_given_up` FAILED where T had been given up, either
  on a run's output or in `record-after.txt`.
  - It matches the engine: `fresh_name` skips the names in the record
    and the abandoned names
    (`crates/skyzfs-agent/src/engine.rs:1607-1623`).
  - In a1 on this head, case A-intent-before-1 took T's name again, and
    the pin holds.
- **G-b and G-c** (`:776-786`):
  - The settle must publish exactly one name other than T.
  - `POOL@T` must be gone from `snapshots-after.txt`, compared as a
    whole word and not as a substring. The exception is a retry that
    made a new snapshot under T's name.
  - The recorder writes the file after the case
    (`agent-record.sh:397-398`). If that fails, it is the rig's error.
  - In a1 and a2 on this head, four moved cases hold both pins.
  - The exception is reachable. A moved publish takes its new name
    through `rename` (`engine.rs:1335-1338`). T is kept only in that
    process's `renamed` map, and is not given up. In a1,
    A-manifest-before-1 published as T-1 in T's own second, and its
    retry came one second later. A retry in that same second would
    take T again, with T's take destroyed: right, and covered by the
    exception. Plant Z04 (no exception) survives, so the exception is
    not pinned. A regression would give a false FAILED, which would be
    seen.
- **K21 and K22:** `pin_ID_stays_absent` is judged only where node B's
  row had no ref. A failure of node B there is a RigError, not
  "absent". Both are pinned.
- **N1:** the fixture's trace (`test_agent_check.py:767-769`) is the
  trace of run 45800708400's `K-sweep-take-after-1`, which I read in
  its artifact: the sweep, two intents, the take.
- **Q06:** `EXPECTED['plain']` is `'published'`. Both of my re-anchored
  plants on it (absent; None) are killed.
- **Plants:** Z01 to Z03 and Z05 to Z07 are killed. Z04 and Z08
  survive (above and G1).

### 5. The CLI recorder: G1, G2, G3, N3, N-a (`zfs-cli-record.sh`)

- **G1:**
  - retain's three pool commands run under `lim` (`:1053`, `:1063`,
    `:1069`).
  - The tests drive each one past its limit with a slow fake.
  - A line is written at the last call, and a free that does not read
    lets retain go on.
  - The pool is 16 GiB, or 32 GiB with K (`:1093`).
- **G2:** a test reads `main`'s text for the three `pools-*` records
  and for `make_pool`'s order.
- **G3:** `holds` runs under `writing` (`:1149`), beside the writer, as
  `take-cli-list` does.
- **N3:** each scale writes `scale LBL: K reached: I-1 of the K asked`
  (`:1075`).
- **N-a:** `lines_line` gives "N lines in S s" (`:700-701`).
- **Plants:** __REC_PLANTS__

### 6. CI on fafc771

The pull request's runs on fafc771 started at 13:23. An earlier set,
started by the push a minute before, was cancelled.

| workflow | run | result |
|---|---|---|
| ci | 45828678388 | success: lint, test, unit-arm64, s3-backends, chart-lint. Its job `test` runs the four test files this pull request touches |
| coverage | 45828678404 | success |
| e2e | 45828678454 | success |
| publish rig | 45828678583 | success; 9 recordings, all exit 0 |
| cut rig | 45828678534 | success on all four images; 10 recordings each, all exit 0 |
| cut rig on the daemon | 45828678589 | success on 26.04, 26.04-arm and 26.04-upload; 15, 15 and 8 recordings, all exit 0 |
| node B rig | 45828678510 | success on 26.04 and 26.04-arm |
| agent rig | 45828678468 | a1, a2 and d: success, each exit 0, with 150, 148 and 96 ok lines and every control CAUGHT. **k: cancelled** after 71 minutes in the rig's step, hung after a rig error (B1) |
| daemon rig, replay rig, kernel probe | 45828678506, 45828678465, 45828678381 | cancelled |

- **The three cancelled rigs.** Their paths include `tests/kernel/`, so
  they started. None of the scripts they run (`daemon-rig.sh`,
  `daemon_check.py`, `skyzfsd-ref.sh`, `replay-rig.sh`, `replay_check.py`,
  `replay_daemon.py`, `hole-rig.sh`, `ublk-probe.sh` and their tests)
  reads any of the seven files this pull request changes. They share
  only `tests/lib/`, which is unchanged. Cancelling them was right.
- **k** is the only job that drives destroy, gone, dhook, late and the
  K sweeps, where most of this delta's paths are. It has not passed on
  this head (B1). Its 16 cases before the error all passed their pins.

### 7. The description against the head

It is right on these points:

- the commits, and the merge of 1d14ee9, which is notes only;
- what D7, D9 and D10 do;
- the judge's G-a, G-b, G-c, K21, K22, N1 and Q06;
- the recorder's G1 to G3, N3 and N-a;
- the counts of the local tests: 115 shell checks, 79 Python tests and
  560 recorder checks, which match mine;
- set A's survivors and my shell survivors, now killed, and U05;
- the three cancelled rigs.

It is wrong, or not yet complete, on these:

- The reason for `akill_serve` (D-1).
- "Each step is bounded" (N-a).
- "New plants … the course directory: all 21 killed". That plant is on
  the writer's side. The judge's side is not pinned (Z08, G1).
- "CI on this head: the runs are listed in a comment when they end."
  There was no such comment when I wrote this, and the agent rig's k did
  not pass (B1).
- "No run has met D7, D9 or D10 in the wild." k on this head met D7,
  and the path did not reach node B (B1).

## Plants

Each plant changes one line or block that occurs exactly once, runs the
test that should catch it with a 600 s limit, then restores the file.
The three series ran one after the other in one worktree at fafc771,
never two at once.

| set | spec | plants | killed | survived |
|---|---|---|---|---|
| the judge: #159's survivors, re-anchored, and new | `p3_A.py` | 26 | 24 | Z04, Z08 |
| the courses and hooks: #159's survivors, and new | `p3_sh.py` | 71 | 55 | U05r, U05q, X04, X04b, X06b, X10, X11, X12, X12b, X21, X25, X26, Y02, Y09, Y10, Y13 |
| the CLI recorder: #159's survivors, and new | `p3_rec.py` | __REC_N__ | __REC_K__ | __REC_S__ |

- **All of #159's survivors that still apply are killed.** Each dies in
  the test that should catch it:
  - set A: all 16, P43 to N05p;
  - the shell set: all 23, S03 to V05, and S18 on the new text (S18r);
  - the recorder: __REC_OLD__.

  U19 and U20, which survived on #159 when run alone, are now killed by
  the two `late_t` checks.
- **Survivors that are equivalent or near it:**
  - U05r and U05q: SIGKILL ends a stopped process, as the description
    says.
  - X21 (without the stop file, the writer is killed after 60 s instead).
  - Y02 (nothing the hook runs calls `rig_error`).
  - X12b (the list taken before the retry and not after: G-c's take is
    destroyed by the settle, which runs before both points).
- **Survivors on constants and messages:** X10 and X11 (tooold's and
  both's waits) and Y13 (the log a message quotes).
- **Survivors that are gaps:** X04, X04b, X06b, X12, X25, X26, Y09, Y10,
  Z04 and Z08 (G1).

## How this was checked

- **Local tests**, in a clean worktree at fafc771, with Python run as
  `python3 -I`. All pass:
  - `test_agent_check.py`: 79 tests OK.
  - `test_agent_record.sh`: all passed (115 checks).
  - `test_zfs_cli_record.sh`: 560 checks, all passed.
  - `test_cut_record.sh`: all passed.
- **bash's handling of `kill` and `wait`** (D-1), on bash 5.2.21.
- **Artifacts read**, each zip downloaded into a new, empty directory:
  - the agent rig's a1, a2 and d on this head;
  - #159's k (run 45800708400), for N1;
  - the publish rig;
  - node B on both images;
  - cut-daemon's three jobs;
  - the cut rig's four images;
  - the agent rig's k on this head (the cancelled job), and its log.
- **The engine:** `fresh_name`, `retake` and `rename`
  (`crates/skyzfs-agent/src/engine.rs`), for G-a and the exception in
  G-c.
- **Pull request comments:** only Codex's summary (no findings at
  fafc771) and a relay of it. There are no inline comments or reviews.

**To run** (from the checkout's root, with a worktree at fafc771 as
`WT`):

- `python3 review/m2.t5b/pr159/plant2.py WT review/m2.t5d/pr163/p3_A.py`.
  The same command runs `p3_sh.py` and `p3_rec.py`.
- The outputs are the `.out` files of the same names.
