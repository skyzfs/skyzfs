# Review of #163 (M2.T5 follow-ups, second round) at fafc771

Pull request: skyzfs/skyzfs#163 (`skyzfs-testing` into `zfs-topology`).
**Head: fafc771** (checked with `gh api .../pulls/163 --jq .head.sha`
at the start and again before writing this). Diff read with
`git diff origin/zfs-topology...fafc771`, before the description.

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

__VERDICT__

## Findings, most severe first

### D-1. The reason given for `akill_serve` is wrong (should fix; comment and description only)

Where: `tests/kernel/agent-record.sh:275-278`, and the description's D10
bullet "`kill_serve`".

Both say that `kill -9` of a child not yet waited for succeeds, so that
before this change a daemon that had died by itself "went unseen". That
is not what bash does:

- bash reaps a background child when it ends, without a `wait`. A
  `kill` of it afterwards fails with "No such process". `wait` then
  still gives its status. Checked on bash 5.2.21, the version of
  Ubuntu 24.04:
  `bash -c 'sleep 0.1 & p=$!; sleep 1; kill -9 $p; echo $?; wait $p; echo $?'`
  prints `kill: (…) - No such process`, `1`, then `0`.
- So at c48ed10, `kill_serve` (`cut-record.sh:968-972`, unchanged) saw a
  daemon that had died by itself: its `kill -9 … || rig_error` made it
  the rig's error, exit 2.

What changed is that it is now a verdict (`daemon_serving`, exit 1) and
not the rig's error. The new code is right either way, since it judges
`wait`'s status and not the kill's. Only the reason is wrong, and a
later reader may "fix" `kill_serve` on its strength.

Certainty: high.

Fix: in the comment and the description, say that before, a daemon that
had died by itself made `kill -9` fail and so was the rig's error; now
its status makes it a verdict.

### G1. A daemon that ended 0 by itself before the rig's SIGTERM passes as a clean stop (gap; low)

Where: `cut-record.sh:585-592` (`end_daemon`) and
`agent-record.sh:264-272` (`reserve`).

- Both send `kill -TERM "$SERVE_PID" 2>/dev/null || true`. When the
  daemon has already ended, the kill fails and the failure is dropped.
  `serve_end` then reads its status. A status other than 0 is the
  verdict `daemon_stopped`, with a message that says "on SIGTERM". A
  status of 0 is read as a clean stop.
- Scenario: the serve loop returns `Ok` when something ends it from
  outside (a device deleted, a lease lost), after the course's export
  and before the stop. In an agent course that should be a verdict, as
  `akill_serve` and `recover` now make it.
- The window is small. In `reserve`, the previous case's `recover` has
  just seen the daemon alive. In `end_daemon`, a daemon that died before
  `agent_end`'s export makes the export fail, which is the rig's error
  (exit 2), not a pass.

Certainty: high that the path exists; low that it matters.

Fix, if wanted: `kill -TERM "$SERVE_PID" 2>/dev/null || daemon_wrong
daemon_stopped "skyzfsd serve had ended before SIGTERM (status …)"`,
and the same through `verdict` in `reserve`.

### G2. A daemon that does not get ready is still the rig's error in an agent course (gap; low; for the author)

Where: `cut-record.sh:414` and `:422-423`.

- An instance that ends before it is ready is now the verdict
  `daemon_ready` (`:402-411`), as the description says.
- An instance that is alive but does not print `ready` in 120 s, or
  gives no status with `cut_state` in 10 s, is still `rig_error`. A hang
  in a take-over is the daemon's fault as much as an exit is.
- The cases before it are still judged (the hook runs), so the only
  effect is exit 2 instead of 1.

Certainty: high. Whether it is worth a verdict is the author's call.

### G3. __G3_TITLE__

__G3_BODY__

### G4. On a suspended pool the hook path can still wait without limit (observation; low certainty; mostly older than this delta)

- D7 makes the hook path go on to node B's stages. `agent_quiet` exports
  under `lim 120` (`agent-record.sh:678`), which is right: `lim`
  abandons a command that does not end.
- If that export is stuck in the kernel, two later steps have no such
  guard:
  - node B's `snapshot_files` runs `zpool list` through
    `subprocess.run(…, timeout=60)` (`nodeb_check.py:316-320`). On a
    timeout Python kills the child and then waits for it with no limit.
  - `cleanup` runs `timeout 120 zpool export -f` (`cut-record.sh:334`).
    `timeout(1)` waits for its child, which CLAUDE.md warns of.
- Both are older than this delta, and the job's own limit bounds them.
  Whether a stuck export blocks `zpool list` depends on the kernel's
  locking, which I could not read here.

No change asked for this pull request. If a k job ever hangs after a rig
error, look here first.

### Nits

- **N-a. "Each step is bounded" is not quite so.** The thaw in
  `agent_quiet` (`agent-record.sh:667`) runs `bash -c "$S3_UP"`
  (`docker start minio` in the workflow) with no limit. In practice it
  ends. Either put it under `lim 60`, or say "each step but the thaw".
- **N-b. No judge test has a `cases/course` directory.** The shell test
  pins that `agent_daemon_wrong` writes there. The judge reads it only
  because its loop takes every directory under `cases/`. Plant Z08 (the
  loop skips `course`) survives. One case in `test_agent_check.py` with
  `cases/course/verdicts.tsv` holding `daemon_stopped` would pin
  `pin_course_daemon_stopped` and exit 1.
- **N-c. A rig error in `end_daemon` after a verdict runs `agent_quiet`
  on a course already ended.** If `serve_end 10` after the SIGKILL fails
  (`cut-record.sh:589`), or `detach` does (`:594`), the hooks are still
  set (`end()` clears them at `:635`, after `end_daemon`). So
  `agent_quiet` writes a second `session-K-end` mark and tries to export
  a pool already exported, which it logs. Harmless: the agent judge reads
  no session marks, and the verdict in `cases/course` still ends the
  check 1.

## Checks

### 1. D7: `agent_quiet` (`agent-record.sh:651-683`)

- **What it does**, in order: SIGCONT and SIGKILL to the agent, up to
  5 s for it to go, then `wait` only if it went; the store thawed and the
  watcher's pause removed; the writer asked to stop, up to 60 s, then
  SIGKILL and `wait` only if it went; `session_end`; `zpool export` under
  `lim 120`, logged either way.
- **Bounded:** yes, but for the thaw (N-a). An agent in D state is left
  behind with no `wait` (`:663`). A writer killed with SIGKILL may leave
  its `dd` running; if that `dd` is stuck on the pool it is unkillable
  anyway.
- **Not fatal:** the hook runs on the left of `||`, in `rig_error`
  (`cut-record.sh:249`) and in `cleanup` (`:324`, under `set +e`). So
  errexit is off inside it, and no step of it ends the recording. It
  calls nothing that calls `rig_error`.
- **Cannot hide a verdict:** it runs before `$CHECK`, in sequence, and
  writes no case file. The judge reads every `verdicts.tsv` first
  (`agent_check.py:591-600`), and any FAILED line ends it 1
  (`:952-953`), whatever RigError comes after.
- **Plants:** __D7_PLANTS__

### 2. D9: recover's exemption and `lim` (`agent-record.sh:295-319`, `:470`, `:523`)

- A daemon gone at the end of a case is the verdict `daemon_serving`,
  with its status, unless the case is dhook **and** the case wrote its
  own error first (`:311`). Right: in every other scenario the rig never
  stops the daemon.
- destroy's release and destroy, and gone's destroy, run under `lim 60`,
  with their output in a file and not in a command substitution, as
  CLAUDE.md asks. A failure is the case's error, with its log kept as
  `destroy-failed.log`.
- **Plants:** __D9_PLANTS__

### 3. D10: the daemon's status as a verdict

- **`akill_serve`** (`:279-285`, used by restart `:438` and late
  `:596`): only 137, the rig's SIGKILL, passes. Any other status, an
  empty PID included (`wait ""` returns 1), is `daemon_serving`. The
  reason in its comment is wrong (D-1).
- **`serve_start`** (`cut-record.sh:402-411`): an instance that ended
  before it was ready is `daemon_ready` through `ON_DAEMON_WRONG`, in the
  case under way or in `cases/course`, and then the rig's error, so the
  hook judges. Outside an agent course nothing changes.
- **`end_daemon`** (`:585-592`): a hang or a status other than 0 goes
  through `daemon_wrong`, which is the verdict `daemon_stopped` in
  `cases/course` in an agent course and `rig_error` elsewhere. `SERVE_LOG`
  names the instance's own log. The one status read as ok when it should
  not be is 0 before the SIGTERM (G1).
- **`cleanup`** (`:312-326`): a recording that ends nonzero with the
  hook still set (errexit, or an `exit` that did not pass through
  `rig_error`) writes its "rig error" line and calls the hook, with the
  hook cleared first. Status 1 from the hook stays 1; anything else
  is 2. The outer loop (`:2066`) then finds the line and does not write
  a second one. A failure where bash ignores errexit (in a function on
  the left of `||` or in an `if`) goes on as before; that is not new.
- **The hooks are cleared** in `end()` after `end_daemon` and before
  the check (`:635`), so a failure of the check itself is not judged
  twice.
- **Plants:** __D10_PLANTS__

### 4. The judge: G-a, G-b, G-c, K21, K22, N1, Q06 (`agent_check.py`)

- **G-a** (`:752-767`): a retry that published under T's name is
  `pin_ID_name_not_given_up` FAILED where T was given up, on any run's
  output or in `record-after.txt`. It matches the engine:
  `fresh_name` skips the record's names and its abandoned names
  (`crates/skyzfs-agent/src/engine.rs:1607-1623`).
- **G-b and G-c** (`:776-786`): the settle must publish exactly one name
  but T, and `POOL@T` must be gone from `snapshots-after.txt` (whole
  words, not a substring), unless the retry made a new snapshot under
  T's name. The recorder writes the file after the case
  (`agent-record.sh:397-398`), and its failure is the rig's error.
- **K21, K22:** `pin_ID_stays_absent` only where node B's row had no ref,
  and a node B failure there is RigError, not "absent". Pinned.
- **N1:** the fixture's trace (`test_agent_check.py:765-767`) is the
  trace of run 45800708400's `K-sweep-take-after-1`, which I read in its
  artifact: sweep, intent twice, take.
- **Q06:** `EXPECTED['plain']` is `'published'`, and both of my
  re-anchored plants on it (absent; None) are killed.
- **Plants:** __JUDGE_PLANTS__

### 5. The CLI recorder: G1, G2, G3, N3, N-a (`zfs-cli-record.sh`)

- **G1:** retain's three pool commands run under `lim` (`:1053`,
  `:1063`, `:1069`), and the tests drive each past its limit with a
  slow fake. A line is written at the last call, a free that does not
  read goes on, and the pool is 16 GiB, or 32 GiB with K (`:1093`).
- **G2:** a test reads `main`'s text for the three `pools-*` records and
  for `make_pool`'s order.
- **G3:** `holds` runs under `writing` (`:1149`), beside the writer, as
  `take-cli-list` does.
- **N3:** `scale LBL: K reached: I-1 of the K asked` (`:1075`).
- **N-a:** `lines_line` gives "N lines in S s" (`:699-702`).
- **Plants:** __REC_PLANTS__

### 6. CI on fafc771

__CI__

### 7. The description against the head

__DESC__

## Plants

__PLANT_TABLE__

## How this was checked

__HOW__
