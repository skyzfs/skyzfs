# Review of #158 (M2.T5): the agent rig, `skyzfs-agent publish` on a real pool through skyzfsd

Pull request: skyzfs/skyzfs#158 (`skyzfs-testing` into `zfs-topology`).
**Head asked for: de5eac3.** The head moved to **0604977** while I
reviewed (97a5005 and 0604977, read with `gh api` at the end); the last
section judges those two commits. The base is 77da00c. Line numbers are
at 0604977 unless a line says de5eac3; `agent-record.sh` is the same at
both heads.

The review of #128 that was in this directory is now `review-128.md`
(and its next look `next_look-128.md`). The plant scripts, their outputs
and the show-cases of this review are in `pr158/`.

The pull request changes test code and one workflow only (14 files,
+2530 -38): the new `skyzfs-agent-rig.yml`, `agent-record.sh`,
`agent_check.py` and their tests; small changes to `cut-record.sh`,
`cut_check.py`, `publish_check.py` (the carried third look of #145,
4e33026, which I read again and find right); and, in 0604977, the ZFS
CLI recorder for M2.6c-5.

## Verdict

- **At de5eac3, B1 and D1 block.** At 0604977, **97a5005 settles B1**
  (its rig run, below, is green under the new rule); **D1, D2 and D3
  still block**. Each is a fix of a few lines.
- D4, the test gaps and the nits do not block.
- **Scope:** 0604977 (the ZFS CLI recorder's manifest read, for
  M2.6c-5's fixtures) is not this work package. One pull request per
  work package: move it to its own pull request, or the lead accepts it
  here. Not a defect.
- Before the merge, the lead's ruling on #158 that 97a5005 cites should
  be written down: `kill.rs:84-100` on `zfs-topology` and impl-notes
  line 1335 (the #150 ruling) still say the opposite of what the rig now
  judges (B1).

## Blocking

### B1. The carry-swap pin at de5eac3 contradicts design §5.2's rule (R) (settled by 97a5005)

Certainty: high. File: `agent_check.py:602-614` at de5eac3
(`same = row is not None and (row['number'], row['group']) == kcut`;
carry-swap FAILED on a mismatch, carry-put INFO).

The pin required that the snapshot's row name the cut the hook killed
at. That premise comes from `kill.rs:84-100` ("after the restart the
publish asked again is made on the cut the kill line names (reasoned
from the code, not run)") and from the #150 ruling (impl-notes 1335).
The design says otherwise. The restarted daemon's rewind mark counts the
cuts it started with (`crates/skyzfs-local/src/inplace/cuts.rs:84-125`, "the count at the
start counts as one", the lead's answer to the plan's Q1); a publish
asked of the instance before is refused `rewound`
(`bins/skyzfsd/src/serve.rs:1222-1226`,
`crates/skyzfs-upload/src/publish.rs:687`); the agent asks anew and its new request's C is read
after the restart, so it includes the killed cut, which had committed
under carry-swap. The first cut that can carry the new request is
numbered above it.

**The FAILED line in run 45749017239 (d, carry-swap) is the pin's
premise, not the daemon's or the agent's fault.** The request log:

| time (s) | event |
|---|---|
| 133.397 | cut 19, group 25 (pid 7386), the hook's cut |
| 136.044 | `publish asked 25 18 accepted` |
| 137.360 | the restarted daemon starts (pid 8081) |
| 137.718 | `asked 25 20 accepted` (the agent's new request, C 20) |
| 138.644 | cut 21, group 27: the first cut above C 20 that opens as itself |
| 138.760 | done; the row names cut 21 |

The carry-put case of the same run had the same shape: cuts 15 and 16
before `asked 21 14`, then after the restart `asked 21 16`, carried by
cut 17. The INFO line's reason for carry-put ("design §5.2 allows it:
the named cut had not committed") was also not the reason.

**The author's rule (b)** (the row's cut is one the last accepted
request accepts) is sound against §5.2 with three conditions, all of
which 97a5005 meets:

1. ordered by the cut's number (> C, group ≥ G), not by the log's
   position after the acceptance: in the d-publish case cut 27 is
   logged at 147.786, before the acceptance at 147.959, and carries;
2. applied to the carry hooks only: in d-publish, both and late, the
   last ask is not accepted (it ends `refused taken`);
3. not "exactly the first carrier": an abandoned or replaced cut's heir
   may carry (D7), and the request log has no abandon lines to tell, so
   the first carrier can only be INFO.

Option (a) of the pull request (keep "same cut") would need a change of
the design, not of the rig.

Left after 97a5005 (outside this pull request): the `kill.rs:84-100`
docs and impl-notes line 1335 still state the old premise. The coding
session's next pull request on kill.rs and the lead's notes should say
what the rig now judges.

### D1. A published snapshot that was destroyed is reported as the rig's error (exit 2), not as FAILED

Certainty: high (run through the function with a stub, `pr158/sim_published.sh`).
File: `agent-record.sh:245-253` (`published()`), called at 270, 274, 279.

`published()` reads the files through `$MNT/.zfs/snapshot/$n`; when the
snapshot is gone the `cd` fails and it calls `rig_error`: the course
stops at once with exit 2, `recordings.tsv` written, and the summary
says NOT CHECKED for every case after it. Scenario: the agent (or a
settle) destroys `T` after node B has `snap/T` (the worst outcome this
rig exists to catch). The result is "RIG_ERROR(exit 2): reading the
files of skzcut@g1 failed".

The judge has a test for exactly this
(`test_agent_check.py:480`, `test_a_snapshot_destroyed_and_published_ends_1`),
but it feeds an input the recorder never writes. This answers the
author's first ask: here a premise does hide a verdict.

Fix: when the snapshot is gone, write its line to `published.tsv` with
a marker (no digests) and go on; let the judge's existing FAILED path
take it.

### D2. The first run's exit status is not judged where no kill fired

Certainty: high (show-cases 1 and 2 in `pr158/showcases.out`, built from
the test file's own fixtures; both exit 0).
Files: `agent_check.py:211-220` (`not_reached`), the case loop at 512.

- `not_reached` returns true for any `rc != '137'` with no hook line and
  fewer reaches. A run that panicked (101) or ended in ERROR (1) before
  `look:before:2` is judged "as a case with no kill", and the outcome
  comes from the settle. Show-case 1: a panic, then the settle
  publishes: exit 0.
- Cases with no kill (`w0` plain, `d1` restart) never judge the first
  run's status either. Show-case 2: restart, the agent's own run ends 1
  (the scenario says it goes on by itself and publishes), the settle
  publishes: exit 0.

What should fail: a run with no kill whose exit code is not the one
that goes with the scenario's expected outcome (0, PUBLISHED, for
plain, restart, late and dhook), and `not_reached` should hold only for
an exit the agent gives when it ends by itself with an answer, not for
1 (ERROR) or 101 (a panic).

### D3. A record that does not read after the case passes "clean"

Certainty: high (show-case 3, exit 0).
Files: `agent-record.sh:280` (`record ... > record-after.txt 2>&1 || true`),
`agent_check.py:281-301` (`clean`).

A dump that fails (exit 6, "the record is corrupt" on stderr) leaves a
non-empty `record-after.txt` with no `publish` line: `clean` finds
nothing under way and passes. The agent's record being unreadable after
a case is an agent defect the rig should report. Fix: keep the dump's
exit code (`record-after.rc`) and fail on anything but 0.

## Should fix, not blocking

### D4. Two agent failures are reported as the rig's error

Certainty: high (by reading). `agent-record.sh:493` (`rec_aplain`: the
traced publish ended non-zero → `rig_error`) and `:379` (`scen_sweep`:
the publish before the stray hold). Both are the agent's plain publish
failing: a verdict on the agent, exit 1, not exit 2. The run is red
either way and the log says why, so this does not block. Shell plant
S12 (drop the check at 493) survives the local tests.

## Test gaps

- **G1. The rare courses never kill the shared points after their own
  step** (medium). `rec_arare` (`agent-record.sh:513-531`) kills only
  the points of the rare trace that the plain trace lacks (the set
  difference of `acheck points`). So destroy's `look` and `unlink`, and
  too-old's `look`, `release`, `released` and `unlink` after the
  `withdraw`, are never killed in those courses: the recoveries after a
  destroy and after a withdrawal are not exercised by a kill. Select by
  (point, reach) after the course's first distinctive point instead.
- **G2. `both` has no premise that the daemon was killed.** `scen_both`
  (`agent-record.sh:467-473`) calls `kill_serve`; the judge checks the
  agent's kill only. Shell plant S19 (no daemon kill) survives; a
  premise line from the serve logs (two instances in the case) would
  catch it.
- **G3. Exit precedence** (note). `rc = max(...)` (`agent_check.py:435,
  445`): one premise ERROR anywhere makes the run exit 2 even when a
  verdict FAILED; the summary shows both lines, but the exit says "the
  rig's error, not a verdict". Suggest that FAILED keep exit 1 and the
  summary say that a premise also failed, or at least a line at the top
  of the summary when both occur.
- **G4. Path filter.** The pull request trigger builds `skyzfsd`, which
  depends on `skyzfs-ublk`, `skyzfs-ublk-sys`, `skyzfs-blk`,
  `skyzfs-uberblock`, `skyzfs-mapped`, `skynbd-types` and
  `skynbd-format`; none is in `paths` (`skyzfs-agent-rig.yml:55-75`;
  the daemon rig lists the first three). Nor are
  `tests/kernel/cut_check.py` and `cut_summary.py`, which the rig uses.
  A change to any of them does not run this rig.

### What the plants showed (the author's fourth ask)

Textual plants, one at a time, the file restored after each, 600 s
limit; KILLED means the local tests failed. Scripts and outputs in
`pr158/`. Sets 1, 2 and shell at de5eac3; the carry set at 0604977.

| Set | File planted | Plants | KILLED | SURVIVED |
|---|---|---|---|---|
| 1 | `agent_check.py` | 74 | 56 | 18 |
| 2 | `agent_check.py` (EXPECTED, dropped lines, premises) | 26 | 22 | 4 |
| shell | `agent-record.sh` | 23 | 10 | 13 |
| carry | `agent_check.py` (97a5005's rule) | 11 | 7 | 4 |

Survivors that matter (the others are benign: still exit 2, redundant
with another pin, or INFO only):

- P02: `killed_at` accepts a kill line for another point or reach. No
  test has a hook line that names a different point than the case.
- P13, P14, P17, P18, P20, Q01: the expected outcome of restart, dhook,
  late, both and assist set to None (no expectation) or to a wrong
  value. No test has a case of these scenarios that ends other than
  published. Q06: `EXPECTED['plain']` is never read.
- P43: the unmarked case's stray-hold check. P46: the criterion's
  check of the retry's name at node B. P48: the row's commit against
  the ref's commit. P52: the manifest's length and hash.
- P58, Q17: `ctl_manifest` reports CAUGHT whatever the check returned.
  P60: a watcher ERROR. P61: the destroyed premise. P67: `ctl_early`'s
  choice of the later name.
- Q23: a premise failure that skips the case's other verdicts. No test
  shows that a premise failure still reports the case's FAILED lines.
- Shell: S12 (the traced publish's status, D4), S13 (the rare courses'
  "first three reaches"), S14 and S15 (the retry's condition), S16 (the
  carry hooks' freeze), S17 (holds read before they are released), S19
  (G2), S20 (`await_agent`'s limit), S22 (the settle's age), S23 (the
  kill column of `cases.tsv`). `test_agent_record.sh` tests the helpers
  only; the scenarios need a kernel, but S14, S15, S17 and S23 could be
  tested with stubs, as `test_cut_record.sh` does.
- Carry: R06, `last_ask` matching a result that is not `accepted`
  (no test has a refused ask after an accepted one). R03 (redundant
  with `pin_ID_row`), R09 (no row: the outcome pin fails anyway), R11
  (INFO only) are benign.

Show-cases (`pr158/showcases.py`, built from `test_agent_check.py`'s own
fixtures, at 0604977): 1 and 2 (D2) exit 0, 3 (D3) exit 0, 4 (`both`
with no trace of a daemon kill, G2) exit 0, 5 (the retry printed a name
node B lacks) exit 1 as it should.

## The author's asks

1. **Premises are exit 2 and never hide a verdict.** `killed_at`
   (`agent_check.py:192`) is sound (P01, P03, P04 killed). The dhook
   kill-line patterns match the lines `kill.rs` prints (at de5eac3,
   505-511, 522-528, 542-548, 657, 849-852). The daemon's premise
   needs both rc 137 and its kill line. But `not_reached` is unsound
   (D2), and D1 and D4 turn verdicts into exit 2; G3 lets a premise
   mask a FAILED in the exit code.
2. **The carry hooks' freeze** (`agent-record.sh:404-446`) does not
   change what carry-put and carry-swap test: the first cut above C and
   of G or later that opens as itself carries the publish either way
   (cut 19 in run 45749017239, freeze or not); the freeze moves the
   carrier's read after the acceptance so that the hook, which never
   chooses a cut whose reading began before it, can fire. The recovery
   after the kill is the same. The comment's claim that the uploader
   makes the step again and does not read anew holds under
   `docker stop` (a failed step waits and repeats the same step,
   `crates/skyzfs-upload/src/uploader.rs:24-27`). Observation, no verdict: in the carry-swap
   case of run 45749017239 the agent's held → asking took 2.51 s,
   against about 0.05 s in the other cases.
3. **The carry-swap FAILED line**: the pin's premise, not the rig's
   bookkeeping, the agent or the daemon (B1). Rule (b) is sound with
   the three conditions in B1.
4. **Do the plants fail when the code is wrong**: mostly, with the
   survivors above. A run that should fail and passes: D2 (show-cases
   1 and 2), D3 (show-case 3), G2 (show-case 4).

## Nits

- `agent-record.sh:48` and `:107-108` say the workflow freezes the
  store with `docker pause`; it uses `docker stop minio` /
  `docker start minio` (`skyzfs-agent-rig.yml:209`).
- 97a5005: `pin_ID_no_older_than_the_killed_cut` is implied by
  `pin_ID_carried_by_the_last_request` whenever the last accepted ask
  came after the restart (its C counts the killed cut). It only adds
  something when the last accepted ask is the one before the kill.
  Harmless.
- `test_agent_check.py` and `test_agent_record.sh` run only in this
  workflow's `unit` job, not in `ci.yml` or `skyzfs-ci.yml` (the
  publish rig does the same). The push trigger
  (`tests/kernel/agent*`) does not match `test_agent_check.py`.

## The workflow

- `bash tests/lib/apt-fallback.sh` right after checkout in the one job
  that installs packages (record); `unit` and `plan` install none.
- Concurrency: per pull request or ref and event, cancel in progress
  except on dispatch, like the other rigs.
- Path filters: G4.

## CI

At de5eac3:

| Run | Workflow | Result |
|---|---|---|
| 45749017239 | agent rig (push) | failure: d 1 FAILED (B1); a1 121 ok, a2 121 ok, k 237 ok, no ERROR, controls 3 of 3 each |
| 45749017338 | skyzfs-ci (push) | success |
| 45750413897 | e2e | success |
| nodeb rig, attempt 2 | | success |
| 45750413915, 45750413879, 45750413908 | ci, coverage, agent rig (PR) | cancelled by the next push |
| daemon, replay, kernel probe, publish, cut (PR) | | cancelled |

At 0604977 (when I finished):

| Run | Workflow | Result |
|---|---|---|
| 45751597306 | agent rig (PR) | **success**: d green under rule (b): carry-put killed at cut 17, row cut 18; carry-swap killed at cut 22, row cut 23 (C 22, G 27) |
| 45751591147 | skyzfs-ci (push) | success |
| 45751597350 | cut-daemon | success |
| 45751597369 | e2e | success |
| 45751591216 | zfs-cli-record (push) | success |
| 45751597373, 45751597342 | ci, coverage | in progress |
| 45751625028 | zfs-cli-record (dispatch) | in progress |
| 45751597543, 45751597328, 45751597325, 45751597371, 45751597375, 45751597655 | daemon, replay, kernel probe, publish, cut, nodeb rigs | cancelled |

The daemon, replay and kernel-probe rigs run on this pull request by
their path filters (`tests/kernel/`) and were cancelled. The lead
decides whether they must finish before the merge; nothing in the
pull request changes what they run except `cut_check.py` and
`publish_check.py`, which the cut and publish rigs read (the publish
rig was green on a push of 4e33026 at 65dd875, run 45743709313).

## Local runs

At 0604977: `python3 -I tests/kernel/test_agent_check.py` 43 tests OK;
`bash tests/kernel/test_agent_record.sh`, `test_cut_record.sh` and
`test_zfs_cli_record.sh` all passed; `test_publish_check.py`,
`test_publish_check_main.py`, `test_cut_check.py` OK. shellcheck 0.9.0
(older than CI's): no warning on the new scripts; its SC2218 on
`test_zfs_cli_record.sh` is in the base too, and CI's lint passed.

## 97a5005 and 0604977

- **97a5005** ("the carry hooks' row against the last request
  accepted"): `last_ask`, `judge_carried`, `first_carrier`,
  `premise_ID_asked`, `pin_ID_carried_by_the_last_request` and, for
  carry-swap, `pin_ID_no_older_than_the_killed_cut`
  (`agent_check.py:342-377, 645-674`). I judged it against §5.2 and
  rule (R), not against the text of the lead's ruling, which I have not
  seen written down. It matches the reading in B1 and its run is
  green. Plants: 7 of 11 killed (R06 a gap, three benign).
  `premise_ID_asked` makes "a carry case with no accepted ask" the
  rig's error; a published row with no accepted ask in the daemon's
  own log could also be the daemon's fault, but the request log is the
  daemon's trace for the rig, so I accept it as a premise.
- **0604977**: the ZFS CLI recorder for M2.6c-5. Its tests pass and it
  runs in `ci.yml`, `skyzfs-ci.yml` and its own workflow. It is outside
  this work package (scope, above); I did not review it further.
