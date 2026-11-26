# Review of #163 (M2.T5 follow-ups, second round) at 4f0d632

Pull request: skyzfs/skyzfs#163 (`skyzfs-testing` into `zfs-topology`).
**Head: 4f0d632** (checked with `gh api .../pulls/163 --jq .head.sha`
before this round and again before writing). The first round was on
fafc771. I read its diff (`git diff origin/zfs-topology...fafc771`)
before the description. This round covers the three commits after it.

Five commits:

- 94400f7: the work. Seven files under `tests/kernel/`: the agent rig's
  courses (`agent-record.sh`), its judge (`agent_check.py`), the course
  hooks in `cut-record.sh`, the CLI recorder (`zfs-cli-record.sh`) and
  the tests of all four.
- fafc771: the merge of `zfs-topology` at 1d14ee9. It brings one line of
  `docs/skyzfs-impl-notes.md` and nothing else. The merge base is
  1d14ee9.
- 1eaa7ca: the CLI recorder for #162's sweep.
- 7399124: `serve_start` makes its ready file and its log before the
  fork.
- 4f0d632: the fix for the hang of the agent rig's job k on fafc771 (B1
  and B2 below).

`zfs-topology` has moved on by one commit since (8c06862, one line of
the notes). It merges cleanly with 4f0d632 and touches nothing here.
Line numbers are at 4f0d632 unless a finding says
otherwise. `review/m2.t5d/review.md` is the review of #143 (also
M2.T5d), so this one is in `pr163/` beside it and does not replace it.
The plant specs and their outputs are in `pr163/` (how to run them is
at the end).

## Verdict

- **Nothing blocks. It can merge from review __K_VERDICT__.**
- **B1 and B2, which blocked on fafc771, are fixed** by 4f0d632:
  - `agent_quiet` no longer exports a pool whose device has no server;
  - `cleanup`'s export runs under `lim`;
  - the thaw waits for MinIO to answer.
- **Skipping the export, rather than serving a daemon again, is
  acceptable.** It cannot make a run pass, it leaves the judge nothing
  it can misread, and it is safer than a restart on that path. Reasons
  under B1. What it costs: after such an error, node B's stages are not
  judged, and the description says so.
- **One new gap (G5), low:** a daemon that ended by itself, if a rig
  error comes before `recover` sees it, is now logged as "no daemon
  serves its device" and not made the verdict `daemon_serving`, as
  `recover` would. The run still fails (exit 2), so nothing is hidden as
  a pass.
- **Still open from the first round, none blocking:**
  - D-1: the comment on `akill_serve` still gives a wrong reason
    (`agent-record.sh:275-278`), and so does the description.
  - G1 to G4: paths that no test pins, and observations.
  - The nits.
- **The three new commits are right**, and every plant I ran on them is
  killed (__P4_SUMMARY__).
- **CI on 4f0d632:** __CI_SUMMARY__
- **Nothing here needs a decision of design.** G3 and G5 are the
  author's call.

## Findings, most severe first

### B1. With no daemon serving, `agent_quiet` could not export, and the k job hung (blocked on fafc771; fixed at 4f0d632)

Where, on fafc771: `agent-record.sh:657-683` (`agent_quiet`), and
`cut-record.sh:334` (`cleanup`).

**What happened** in run 45828678468, job k, on fafc771. I read the job
log and the artifact `agent-rig-ubuntu-26.04-k`.

| time | what |
|---|---|
| 13:28:06 to 13:30:01 | 16 cases, trace to K-restart-release-after-1, each with its pins ok |
| 13:30:01 to 13:30:04 | case K-restart-released-before-1 (restart): `asked:after` reached, then `akill_serve` killed the 16th instance, and the store was thawed (mark at 444.187 s) |
| 13:30:04 | `snaprow "$t" between.txt` failed: node B's `snap-row`, "refs: backend error: get: dispatch failure". The rig's own error. `serve_again` had not yet run: there is no `serve-17.log` |
| 13:30:04 to 13:32:12 | `agent_quiet`: `session_end`, then `lim 120 zpool export`, which `lim` gave up after 120 s. The log says "skzcut did not export for the check" |
| 13:32:12 | the check: the 16 cases judged, 99 ok and no FAILED, then "rig_error: a pool named skzcut is still imported", exit 2 |
| 13:32:12 to 14:39:21 | no line at all, until the job was cancelled. A k job takes about 26 minutes |

**Why.** `agent_quiet` ran after any rig error, including those that
come while no daemon serves: in restart and late between `akill_serve`
and `serve_again`, in dhook after the hook's kill, and after
`serve_start`'s `daemon_ready` path. With no server, the device holds
the pool's writes, so the export cannot finish. Then `cleanup` ran
`timeout 120 zpool export -f` behind it, and `timeout(1)` waits for a
child it cannot kill, which CLAUDE.md warns of.

**The fix at 4f0d632**, as I asked, except for the first point:

1. `agent_quiet` (`agent-record.sh:683-686`) does not export when
   `SERVE_PID` is empty or names a process that has ended. It logs
   "not exported for the check: no daemon serves its device" and
   returns. I had asked for a daemon to be served again instead.
2. `cleanup` exports under `lim 120`, with its output in
   `/dev/null` (`cut-record.sh:337`).
3. Tests: `quiet_t` drives `agent_quiet` with the daemon ended and with
   no daemon (`test_agent_record.sh:813-816`); the four `cleanup` cases
   (`:752-757`) now expect `lim 120 zpool export -f`.

**Skipping the export, rather than serving a daemon again: acceptable.**

- *It cannot make a run pass.* The skip comes only from `agent_judged`,
  after the error is written to `rig-error.tsv` and `recordings.tsv`.
  The judge reads `rig-error.tsv` and ends at least 2
  (`agent_check.py:634-637`). Node B refuses a pool still imported
  (`nodeb_check.py:318-320`), which is a RigError too. A FAILED among
  the cases done ends it 1. So the run ends 1 or 2, never 0.
- *It leaves nothing the judge misreads.* The judge reads node A only
  through files the course wrote before the error (`snapshots-after.txt`,
  `published.tsv` and the like); it runs no `zfs` or `zpool` of its own.
  The state left is the one a failed export left on fafc771, a pool
  still imported, without a stuck export process beside it. `session_end`
  still runs first, and the agent's judge reads no session marks.
- *It cannot hide a verdict that was written.* As before, every
  `verdicts.tsv` is read first, and the skip writes no case file.
- *A restart would be worse on this path.*
  - It would add an instance after the error: its own line in
    `instances.tsv`, a take-over, and a `serve_start` that can itself
    fail, call `rig_error` from inside the hook, and so need its own
    handling. `rig_error` has cleared the hook by then, so a second error
    would end the recording before the check.
  - The take-over would have to succeed on a device whose last server
    was killed mid-request; if it hung, `agent_quiet` would no longer be
    bounded.
  - What it would buy, node B's stages after such an error, is the
    judgement of names published before the error. The cases' own pins
    are judged either way.
- *What it costs:* after an error while no daemon serves, node B's
  stages and the controls are not judged, and the run is exit 2 (or 1).
  The description's last open question says so. On fafc771 that was
  already the outcome, with a hang added.
- *What I checked:* the plants V4 to V7 (the export tried with no
  daemon; the skip only on an empty pid; the skip always; the skip
  before `session_end`) are all killed. The cases where the skip fires:

  | how the daemon is down | `SERVE_PID` | skip right? |
  |---|---|---|
  | restart, late, both: between the rig's kill and `serve_again` | empty (`akill_serve`, `scen_both` clear it) | yes |
  | `serve_start`'s `daemon_ready` path | empty (cleared at `cut-record.sh:417`), verdict already written | yes |
  | dhook, after the hook made the daemon end | set, ended | yes, and no verdict is right (the rig armed the hook) |
  | any other scenario, a daemon that ended by itself, error before `recover` | set, ended | skip right, but no verdict (G5) |
  | a daemon alive but not answering | set, alive | no skip; the export waits 120 s under `lim`, as on fafc771 |

**Certainty:** high on what happened on fafc771 and on the fix. The fix
has not met the path in the wild: k on 4f0d632 had no rig error, so the
skip did not run there. It is pinned by tests and plants only.

### B2. The thaw did not wait for the store to answer (fixed at 4f0d632)

Where, on fafc771: `.github/workflows/skyzfs-agent-rig.yml:227`,
`CUT_S3_UP="docker start minio"`.

- `docker start` returns when the container starts, not when MinIO
  listens. In B1's case node B read 0.2 s after the thaw and could not
  connect. `snaprow` calls `rig_error` itself on a failure of node B
  (`agent-record.sh:366`), so the `|| true` after it does not help.
- **Fixed** at 4f0d632 in the agent rig (`:228`), the cut rig on the
  daemon (`skyzfs-cut-daemon.yml:224`) and the publish rig
  (`skyzfs-publish-rig.yml:190`): `docker start minio`, then up to 120
  probes of `/minio/health/live`, half a second apart, and exit 1 with a
  message if none answers. Every user of `CUT_S3_UP` already treats a
  nonzero exit as a failed thaw (`agent-record.sh:250`, `:667`;
  `cut-record.sh:1528` in unostore and `:1787` in prewind).
- On 4f0d632 it ran for real: the agent rig's d thawed four times, the
  publish rig's prewind once, each with exit 0.
- Nit N-d below on its stated bound.

### D-1. The reason given for `akill_serve` is wrong (should fix; comment and description only; still open)

Where: `tests/kernel/agent-record.sh:275-278`, unchanged at 4f0d632, and
the description's D10 bullet on `kill_serve`.

Both say that `kill -9` of a child not yet waited for succeeds, so that
before this change a daemon that had died by itself "went unseen". bash
does not work that way:

- bash reaps a background child as soon as it ends, with no `wait`. A
  `kill` of it afterwards fails with "No such process". `wait` still
  gives its status.
- Checked on bash 5.2.21, the version of Ubuntu 24.04:
  `bash -c 'sleep 0.1 & p=$!; sleep 1; kill -9 $p; echo $?; wait $p; echo $?'`
  prints `kill: (…) - No such process`, then `1`, then `0`.
- So at c48ed10, `kill_serve` (`cut-record.sh:968-972` there, unchanged)
  did see a daemon that had died by itself. Its `kill -9 … || rig_error`
  made it the rig's error, exit 2.

What really changed is that such a daemon is now a verdict
(`daemon_serving`, exit 1) and no longer the rig's error. The code is
right either way, because it judges `wait`'s status and not the kill's.
Only the stated reason is wrong, and a later reader may trust it and
"fix" `kill_serve` to match.

Certainty: high. Fix: say that before, a daemon that had died by itself
made `kill -9` fail and so was the rig's error; now its status makes it
a verdict.

### G5. `agent_quiet` does not make an ended daemon a verdict, as `recover` does (gap; low; new at 4f0d632)

Where: `agent-record.sh:683-686`.

- The skip treats "`SERVE_PID` set, process ended" as it treats "no
  `SERVE_PID`". It logs the same line, reads no status, and writes no
  verdict.
- Outside dhook those two differ. The rig clears `SERVE_PID` wherever it
  ends the daemon itself (`akill_serve`, `scen_both`, `serve_start`'s
  failure path, `recover`). So a set PID whose process has ended, outside
  dhook, is a daemon that ended by itself. `recover` (`:307-317`) makes
  exactly that the verdict `daemon_serving`, with its status.
- Scenario: a daemon ends by itself during a case, and a rig error that
  does not depend on it (a node B read in `snaprow`, a failed thaw)
  comes before `recover`. Today: exit 2, "no daemon serves its device",
  and the daemon's status is not read. With a verdict: exit 1,
  `daemon_serving`, with its status and log.
- Narrow: a daemon that ends during a case is mostly met by `recover`
  first, since `recover` runs before the settle (`:426-427`), and most
  of what a dead daemon breaks shows as the case's own error.
- Not a false pass either way. No worse than fafc771, where the export
  then waited 120 s and failed.

Certainty: high that the path exists; low that it matters.

Fix, if wanted: in the skip, when `SERVE_PID` is set and the scenario is
not dhook (or dhook with no `rig-error.txt` in the case), read the
status with `serve_end 5` and call
`agent_daemon_wrong daemon_serving "…"`, as `recover` does. The judge
already ends 1 on it.

### G1. Unpinned paths of D9 and D10 (gap; low)

The code is right on each of these paths, but no test pins it. Each
plant below changes one path and passes every test (Plants, below).
None of them is touched by the three new commits.

- **X06b: dhook exempt from the verdict whether or not the case had an
  error of its own.** This one matters most: a regression would hide a
  verdict. A daemon that died by itself in a dhook case would then pass
  with no verdict.
  - `test_agent_record.sh` tests dhook with an error (no verdict), and
    three other scenarios with an error (a verdict).
  - Missing: dhook with an empty `rig-error.txt`, which must give
    `daemon_serving`.
- **X04 and X04b: late's kill.** Late with `kill_serve`, or with no kill
  at all, passes. `late_t` drives only the two paths that stop early.
  Missing: one call with `snaprow` succeeding (its stub `akill_serve` is
  already there). Restart's use of `akill_serve` is pinned (X03 is
  killed).
- **Y09 and Y10: `end_daemon`'s hang path.** A hung stop sent to
  `rig_error`, or not killed, passes. `ed_t` has only a daemon that ends
  1 on SIGTERM. Missing: a stand-in that ignores SIGTERM, which must give
  `daemon_stopped` and then go on.
- **Z08: the judge reads `cases/course`.** It does so only because its
  loop takes every directory under `cases/`. The shell test pins the
  writer side (X15 is killed). Missing: one case in `test_agent_check.py`
  with `cases/course/verdicts.tsv`, which must give
  `pin_course_daemon_stopped` FAILED and exit 1.
- **X25 and X26: `agent_quiet`'s bounds.** An unbounded `wait` on the
  writer or the agent passes. A test with a `kill` stub that ignores
  SIGKILL, beside a child that sleeps, could pin both.
- **X12: `asettle` writes `snapshots-after.txt`.** A regression is
  visible anyway: the judge then stops with the rig's error.

Certainty: high (each plant was run alone, with a 600 s limit). Only
X06b and Z08 could hide a verdict if they regressed.

### G2. A daemon that ended 0 by itself before the rig's SIGTERM passes as a clean stop (gap; low)

Where: `cut-record.sh:595-601` (`end_daemon`) and
`agent-record.sh:264-272` (`reserve`).

- Both send `kill -TERM "$SERVE_PID" 2>/dev/null || true`. If the daemon
  has already ended, the kill fails and `|| true` drops it. `serve_end`
  then reads its status: not 0 is the verdict `daemon_stopped` (with a
  message that says "on SIGTERM"); 0 is read as a clean stop.
- Scenario: the serve loop returns `Ok` when something outside ends it
  (a device deleted, a lease lost), after the course's export and before
  the stop. In an agent course that should be a verdict.
- The window is small: in `reserve`, the case before has just seen the
  daemon alive in `recover`; in `end_daemon`, a daemon that died before
  `agent_end`'s export leaves that export blocked first.
- That last point is itself a hazard, older than this pull request:
  `export_pool` runs the export through `z`, which is `timeout(1)`
  (`cut-record.sh:254-256`, `:273`). A daemon that ends by itself
  between the last case's `recover` and `agent_end` would hang the job
  as B1 did, not give a verdict. The window is small, and within a case
  a later take-over serves the blocked commands, so I only note it.

Certainty: high that the path exists; low that it matters. Fix, if
wanted: replace `|| true` with
`|| daemon_wrong daemon_stopped "skyzfsd serve had ended before SIGTERM"`
(through `verdict` in `reserve`).

### G3. A daemon that does not get ready is still the rig's error in an agent course (gap; low; for the author)

Where: `cut-record.sh:424` and `:432-433`.

- An instance that ends before it is ready is the verdict `daemon_ready`
  (`:412-421`). Two failures are still `rig_error`: an instance alive
  that does not print `ready` in 120 s, and one that gives no status with
  `cut_state` in 10 s. A take-over that hangs is as much the daemon's
  fault as one that exits.
- The hook still runs, so the cases before it are judged. The only
  effect is exit 2 instead of 1.

Certainty: high. Whether this is worth a verdict is the author's call.

### G4. node B's `zpool list` waits with no limit after its timeout (observation; low certainty; older than this pull request)

- Node B's `snapshot_files` runs `zpool list` through
  `subprocess.run(…, timeout=60)` (`nodeb_check.py:316-320`). On a
  timeout, Python kills the child and then waits for it with no limit.
- Whether `zpool list` can block behind a stuck export depends on the
  kernel's locking, which I could not read here. In B1's run it did not.
- 4f0d632 makes this less likely: with no daemon, `agent_quiet` leaves
  no export stuck beside the check.

No change is asked of this pull request.

### Nits

- **N-a. "Each step is bounded"** (`agent-record.sh:654-656`). The thaw
  in `agent_quiet` (`:667`) still runs `bash -c "$S3_UP"` with no limit
  of the rig's. Since 4f0d632 the workflows' `S3_UP` bounds itself, so
  this holds in practice. Say "bounded, the thaw by the workflow's
  command".
- **N-b. A rig error in `end_daemon` after a verdict runs `agent_quiet`
  on a course already ended.** If `serve_end 10` fails after the SIGKILL
  (`cut-record.sh:599`), or `detach` fails (`:603-604`), the hooks are
  still set (`end()` clears them at `:645`, after `end_daemon`). So
  `agent_quiet` writes a second `session-K-end` mark, then either skips
  the export (no daemon) or tries one of a pool already exported, which
  fails at once and is logged. Harmless: the agent's judge reads
  no session marks, and the verdict in `cases/course` still ends the
  check 1.
- **N-c. The restart and destroy bullet in the description** says "with
  a kill before `asked`, nothing". The code gives no verdict with any
  kill. A kill after `asked` reaches it first, so the two say the same
  in practice.
- **N-d. The thaw's bound is not the 60 s its message says.** Each
  probe is `curl -m 2` then `sleep 0.5`, 120 times. Against a port that
  refuses at once that is about 60 s; against one that accepts and does
  not answer, up to 300 s. Harmless (the job's limit is far above it);
  the message could say "after 120 probes", or the loop could check
  elapsed time.
- **N-e. `cleanup`'s export with no daemon.** With `SERVE_PID` empty or
  ended, `cleanup`'s `lim 120 zpool export -f` (`cut-record.sh:337`)
  cannot finish either: it costs 120 s and leaves an export stuck. It
  is bounded now, so this is only time; it could skip as `agent_quiet`
  does.

## Checks

### 1. D7: `agent_quiet` (`agent-record.sh:651-692`)

- **What it does**, in order:
  1. SIGCONT and SIGKILL to the agent, up to 5 s for it to go, and a
     `wait` only if it went. `AGENT_PID` is cleared either way, so
     `cleanup` never waits on a stuck agent.
  2. The store thawed, and the watcher's pause removed.
  3. The writer asked to stop, up to 60 s, then SIGKILL, and a `wait`
     only if it went.
  4. `session_end`.
  5. Since 4f0d632: no export when no daemon serves (B1). Otherwise
     `zpool export` under `lim 120`, logged either way.
- **Bounded:** yes (N-a on the thaw). An agent stuck in D state is left
  with no `wait`. SIGKILL to the writer's subshell may leave its `dd`
  running; if that `dd` is stuck on the pool, nothing could kill it
  anyway.
- **Never fatal:** the hook runs on the left of `||`, both in
  `rig_error` (`cut-record.sh:249`) and in `cleanup` (`:324`, under
  `set +e`). So errexit is off inside it, and it calls nothing that
  calls `rig_error`.
- **Cannot hide a verdict:** it runs before `$CHECK` and writes no case
  file; the judge reads every `verdicts.tsv` first
  (`agent_check.py:591-600`); any FAILED line ends it 1 (`:952-953`),
  whatever RigError comes after.
- **Plants:** X16 to X20, X22 to X24 and X27 killed; X21 survives,
  near-equivalent (without the stop file, the writer is killed after
  60 s instead of ending by itself); X25 and X26 survive (G1); U05q (no
  SIGCONT) is equivalent. V4 to V7, on the skip, are killed.

### 2. D9: recover's exemption and `lim` (`agent-record.sh:295-319`, `:470`, `:523`)

- A daemon gone at the end of a case is the verdict `daemon_serving`,
  with its status. The one exemption (`:311`) needs both: the case is
  dhook, and the case wrote its own error first. That is right: in every
  other scenario, the rig never stops the daemon.
- destroy's release and destroy, and gone's destroy, run under `lim 60`,
  with their output in a file and not in a command substitution, as
  CLAUDE.md asks. A failure is the case's error, and its log is kept as
  `destroy-failed.log`.
- **Plants:** X05, X06, S18r and U14 to U16 killed. X06b survives (G1).

### 3. D10: the daemon's status as a verdict

- **`akill_serve`** (`:279-285`, used by restart and late): only 137,
  the rig's SIGKILL, passes. Any other status is `daemon_serving`,
  including an empty PID (`wait ""` returns 1). Its comment's reason is
  wrong (D-1).
- **`serve_start`** (`cut-record.sh:412-421`): an instance that ended
  before it was ready is `daemon_ready`, through `ON_DAEMON_WRONG`, in
  the case under way or in `cases/course`. It is then the rig's error,
  so the hook judges. Outside an agent course nothing changes.
- **`end_daemon`** (`:595-606`): a stop that hangs, or ends with a
  status other than 0, goes through `daemon_wrong`: in an agent course
  the verdict `daemon_stopped` in `cases/course`, elsewhere `rig_error`.
  `SERVE_LOG` names the instance's own log. The one status read as ok
  that should not be is 0 before the SIGTERM (G2).
- **`cleanup`** (`:312-349`): when a recording ends nonzero with the hook
  still set (errexit, or an `exit` that did not pass through
  `rig_error`), it clears the hook, writes the "rig error" line, then
  calls the hook. Status 1 from the hook stays 1; anything else becomes
  2. The outer loop (`:2076`) then finds the line and does not write a
  second. A failure where bash ignores errexit (in a function on the
  left of `||`, or in an `if`) goes on as before; that is not new.
- **The hooks are cleared** in `end()`, after `end_daemon` and before
  the check (`:645`), so a failure of the check itself is not judged
  twice.
- **Plants:** killed: X01 to X03, X13 to X15, Y01, Y03 to Y08, Y11, Y12
  and Y14, and V8 and V9 on `cleanup`'s `lim`. Y02 (the hook not cleared
  before `cleanup` runs it) survives, near-equivalent: nothing the hook
  runs calls `rig_error`. Y13 (`SERVE_LOG` not set) survives; it
  affects only the message. X04, X04b, Y09 and Y10 survive (G1).

### 4. The judge: G-a, G-b, G-c, K21, K22, N1, Q06 (`agent_check.py`, unchanged since fafc771)

- **G-a** (`:752-767`): a retry that published under T's name gives
  `pin_ID_name_not_given_up` FAILED where T had been given up, on a
  run's output or in `record-after.txt`. It matches the engine:
  `fresh_name` skips the names in the record and the abandoned names
  (`crates/skyzfs-agent/src/engine.rs:1607-1623`). In a1 on fafc771,
  case A-intent-before-1 took T's name again, and the pin holds.
- **G-b and G-c** (`:776-786`): the settle must publish exactly one name
  other than T, and `POOL@T` must be gone from `snapshots-after.txt`
  (compared as a whole word), except where a retry made a new snapshot
  under T's name. The recorder writes the file after the case
  (`agent-record.sh:397-398`); a failure there is the rig's error.
  - The exception is reachable. A moved publish takes its new name
    through `rename` (`engine.rs:1335-1338`); T is kept only in that
    process's `renamed` map and is not given up. A retry in T's own
    second would take T again. Plant Z04 (no exception) survives, so the
    exception is not pinned; a regression would give a false FAILED,
    which would be seen.
- **K21 and K22:** `pin_ID_stays_absent` is judged only where node B's
  row had no ref, and a failure of node B there is a RigError, not
  "absent". Both pinned.
- **N1:** the fixture's trace (`test_agent_check.py:767-769`) is the
  trace of run 45800708400's `K-sweep-take-after-1`, which I read in its
  artifact.
- **Q06:** `EXPECTED['plain']` is `'published'`. Both of my re-anchored
  plants on it are killed.
- **Plants:** Z01 to Z03 and Z05 to Z07 killed. Z04 and Z08 survive
  (above and G1).

### 5. The CLI recorder: G1, G2, G3, N3, N-a (`zfs-cli-record.sh`, from 94400f7)

- **G1:** retain's three pool commands run under `lim`; the tests drive
  each past its limit with a slow fake; a line is written at the last
  call; a free that does not read lets retain go on; the pool is 16 GiB,
  or 32 GiB with K.
- **G2:** a test reads `main`'s text for the three `pools-*` records and
  for `make_pool`'s order.
- **G3:** `holds` runs under `writing`, beside the writer, as
  `take-cli-list` does.
- **N3:** each scale writes `scale LBL: K reached: I-1 of the K asked`.
- **N-a:** `lines_line` gives "N lines in S s".
- **Plants:** 18 of 18 killed: the twelve of #159's recorder survivors
  that still apply (L1, L2, P1, P2, A04a, C06, C09, R10a, C16, C22, C23,
  C24), and six of mine (named W01 to W06 in `p3_rec.py`, not the
  description's W series): no "K reached" line; K reached one over;
  `lines_line` without seconds; holds with no load beside; K reached
  said as K asked; 32 GiB at K 0.

### 6. The three new commits

**1eaa7ca, the CLI recorder for #162's sweep** (`zfs-cli-record.sh`,
the workflow, the tests).

- `tagged.lua` is optional (`:384`, `:1332`): with none, no `tagged`
  record is made, `about.txt` says so (`:1394`), and the workflow fetches
  it only where `lua_ref` has it (`skyzfs-zfs-cli-record.yml:121-125`).
  `members.lua` is still required.
- The sweep's reads are recorded as #162 makes them:
  `SWEEP_LIST` is `zfs list -H -p -r -t snapshot -o name,userrefs`
  (`:386`), before any hold and again with holds; `held_names`
  (`:748`) takes the names with a numeric `userrefs` above 0; then
  `zfs holds -H` on those names (`:761`).
- What the sweep meets (`sweep_holds`, `:754-770`): a name that no
  longer exists when the holds are read (a snapshot taken and destroyed
  after the list), named first and last in the holds read; a hold with
  the tag `caf\xe9`, which is not UTF-8 (`:392`), read by both commands
  and released.
- At scale, the list and one holds read of at most 1000 names
  (`HOLDS_AT_ONCE`, `:389`, as #162 batches) are timed beside the writer,
  with their lines (`:1233-1243`). Only the first batch is timed; the
  line count says how many names it had.
- **The records are what #162 needs.** In the 24.04 artifact of the
  dispatch on this head (run 45843435778, with no `tagged.lua`):
  - `rig-sweep-holds-gone` and `-gone-last`: status 1, every other
    snapshot's holds still printed, and "cannot open 'skzq@gone':
    dataset does not exist" on stderr. So an adapter that drops the
    whole read on a nonzero status would lose the other holds: worth
    telling #162.
  - `rig-sweep-holds-odd`: the tag printed as its raw bytes.
  - `none-sweep-list` and `none-sweep-holds`: status 1 with "dataset
    does not exist".
  - Scale 1000: 1001 list lines in 1.65 s and 1000 holds lines in
    0.73 s beside the writer; scale 10-s1000: 11011 list lines in 5.80 s.
- Plants V10 to V18 (held names taken with any count; no cap of 1000;
  the gone snapshot not in the read, or not destroyed; the odd tag as
  UTF-8; the odd hold not released; `tagged.lua` required; the scale
  list with no load beside; a tagged run without `tagged.lua`):
  __P4_REC__.

**7399124, `serve_start`'s files** (`cut-record.sh:399-405`).

- Before, the child's redirections made `$VOL.ready` and the log after
  the fork. The loop that reads them could run first: a log not yet made
  made `sed` exit 2, which under `pipefail` ended the recording; a ready
  file not yet emptied named the last instance's device, which a
  take-over keeps, so the loop could take it before the new instance said
  `ready` (it needs the new log's line as well, so that window was
  narrow).
- Both are now emptied before the fork; the child truncates them again
  on its open, which is harmless since it writes only after.
- Right. A test reads the function's text for the order. Plants V1 to
  V3 are killed.

**4f0d632, the hang of k** (B1 and B2 above). Plants V4 to V9 are
killed. The thaw's wait is not driven by a test (the description says
so); it ran in CI on this head (B2).

### 7. CI

**On 4f0d632.** The push itself started a set of rig runs, which were
cancelled when the pull request's set started; the CLI recorder's push
run (its unit job only) and `skyzfs-ci` were green.

| workflow | run | result |
|---|---|---|
| ci | 45842997855 | success: lint, test, unit-arm64, s3-backends, chart-lint |
| coverage | 45842997654 | success |
| e2e | 45842997730 | success |
| publish rig | 45842997628 | success; 9 recordings, all exit 0; prewind's thaw through the new wait |
| cut rig on the daemon | 45842997902 | success on 26.04, 26.04-arm and 26.04-upload; 15 (26.04) and 8 (upload) recordings read, all exit 0 |
| node B rig | 45842997768 | success on 26.04 and 26.04-arm |
| agent rig | 45842997610 | a1, a2 and d: success, exit 0, with 150, 149 and 96 ok lines, every control CAUGHT, no FAILED or ERROR; d thawed four times through the new wait. **k: success**, exit 0, 102 cases (21 restart, 9 destroy, 37 sweep, 19 assist, 13 tooold, gone, plain, unmarked), 1031 ok lines, every control CAUGHT, no FAILED or ERROR, 26 minutes; 62 thaws, all through the new wait |
| cut rig | 45842997896 | __CUT_ROW__ |
| CLI recorder (dispatch) | 45843435778 | success: unit, lua, 24.04 and 26.04 (the 24.04 artifact says there was no `tagged.lua` at the `lua_ref` used). Its push run 45842982078 (unit only) is green too |
| daemon rig, replay rig, kernel probe | 45842997658, 45842997961, 45842997581 | cancelled by the author a minute after they started |

- **The three cancelled rigs.** Their paths include `tests/kernel/`, so
  they started. None of the scripts they run (`daemon-rig.sh`,
  `daemon_check.py`, `skyzfsd-ref.sh`, `replay-rig.sh`,
  `replay_check.py`, `replay_daemon.py`, `hole-rig.sh`, `ublk-probe.sh`
  and their tests) reads any file this pull request changes, and
  `tests/lib/` is unchanged. Cancelling them was right.
- **k** is the only job that drives destroy, gone, dhook, late and the K
  sweeps, where most of D9's and D10's paths are.

**On fafc771:** everything green but k (B1): ci, coverage, e2e, the
publish rig, the cut rig on four images (10 recordings each), cut-daemon
(15, 15 and 8 recordings), node B, and the agent rig's a1, a2 and d.

### 8. The description against the head

Right:

- the five commits, and the merge of 1d14ee9, which is notes only;
- what D7, D9 and D10 do; the judge's G-a, G-b, G-c, K21, K22, N1 and
  Q06; the recorder's G1 to G3, N3 and N-a;
- the three new commits, and its account of k's hang, which matches
  what I read in the artifact;
- the local test counts: 120 shell checks, 79 Python tests, 609 recorder
  checks, which match mine;
- the open question on node B after an error with no daemon, which is
  the cost of the skip.

Wrong or stale:

- The reason for `akill_serve` (D-1).
- "Each step is bounded" (N-a).
- "New plants … the course directory: all 21 killed": that plant is on
  the writer's side; the judge's side is not pinned (Z08, G1).
- **"No run has met D7, D9 or D10 in the wild."** k on fafc771 met D7,
  which is what 4f0d632 fixes; the description's own section on the hang
  says so.
- "CI on this head: the runs are listed in a comment when they end":
  there was no such comment yet when I wrote this. The runs are in
  section 7.

## Plants

Each plant changes one line or block that occurs exactly once, runs the
test that should catch it with a 600 s limit, then restores the file.
The series ran one after the other, never two at once: the first three
in a worktree at fafc771, the fourth in one at 4f0d632.

| set | spec | plants | killed | survived |
|---|---|---|---|---|
| the judge: #159's survivors, re-anchored, and new | `p3_A.py` | 26 | 24 | Z04, Z08 |
| the courses and hooks: #159's survivors, and new | `p3_sh.py` | 71 | 55 | U05r, U05q, X04, X04b, X06b, X10, X11, X12, X12b, X21, X25, X26, Y02, Y09, Y10, Y13 |
| the CLI recorder: #159's survivors, and new | `p3_rec.py` | 18 | 18 | none |
| the three new commits | `p4_new.py` | 18 | __P4_K__ | __P4_S__ |

- **All of #159's survivors that still apply are killed**, each in the
  test that should catch it: set A, all 16 (P43 to N05p); the shell set,
  all 23 (S03 to V05, and S18 on the new text as S18r); the recorder,
  all 12 (L1, L2, P1, P2, A04a, C06, C09, R10a, C16, C22, C23, C24). U19
  and U20, which survived on #159 when run alone, are killed by the two
  `late_t` checks.
- **The description's W, X and Y series** are its own; mine on the same
  code are V1 to V18 (`p4_new.py`), written from the diff: V1 to V3 match
  its X01 to X03, V4 to V9 its Y01 to Y06, and V10 to V18 cover what its
  W series names.
- **Survivors that are equivalent or near it:** U05r and U05q (SIGKILL
  ends a stopped process); X21 (without the stop file, the writer is
  killed after 60 s instead); Y02 (nothing the hook runs calls
  `rig_error`); X12b (the list taken before the retry and not after:
  G-c's take is destroyed by the settle, which runs before both points).
- **Survivors on constants and messages:** X10 and X11 (tooold's and
  both's waits) and Y13 (the log a message quotes).
- **Survivors that are gaps:** X04, X04b, X06b, X12, X25, X26, Y09, Y10,
  Z04 and Z08 (G1).

## How this was checked

- **Local tests**, in clean worktrees at fafc771 and at 4f0d632, with
  Python run as `python3 -I`. At 4f0d632 all pass:
  - `test_agent_check.py`: 79 tests OK.
  - `test_agent_record.sh`: all passed (120 checks).
  - `test_cut_record.sh`: all passed.
  - `test_zfs_cli_record.sh`: 609 checks, all passed.
  - shellcheck here is 0.9.0, which reports SC2317 and the like at the
    base as well; CI's lint job (a newer shellcheck) is green on
    4f0d632.
- **bash's handling of `kill` and `wait`** (D-1), on bash 5.2.21.
- **Artifacts read**, each zip in a new, empty directory:
  - on fafc771: the agent rig's a1, a2, d and k (the cancelled job) and
    k's log, the publish rig, node B on both images, cut-daemon's three
    jobs, the cut rig's four images, and #159's k (run 45800708400) for
    N1;
  - on 4f0d632: the agent rig's a1, a2, d and k, the publish rig,
    cut-daemon's 26.04 and 26.04-upload, and the CLI recorder's 24.04.
- **The engine:** `fresh_name`, `retake` and `rename`
  (`crates/skyzfs-agent/src/engine.rs`), for G-a and G-c's exception.
- **Pull request comments:** Codex's summary, which reports no findings
  at 4f0d632, and a relay of its summary at fafc771. No inline comments
  or reviews.

**To run** (from the checkout's root, with a worktree at the head named
as `WT`): `python3 review/m2.t5b/pr159/plant2.py WT
review/m2.t5d/pr163/p3_A.py`. The same command runs `p3_sh.py` and
`p3_rec.py` (at fafc771) and `p4_new.py` (at 4f0d632). The outputs are
the `.out` files of the same names.
