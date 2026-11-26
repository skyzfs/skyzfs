# Next look at #159 (M2.T5 follow-ups; the CLI recorder's retained snapshots) at 1a40bfc

Pull request: skyzfs/skyzfs#159 (`skyzfs-testing` into `zfs-topology`),
base 2489dda. **Head: 1a40bfc** (checked with `gh api .../pulls/159 --jq
.head.sha` at the start and again before writing this).

What is new since my review of b99f21a (`review.md`, comment
7186045831), answered in comment 7186426709:

- 78bb051: the agent rig's judge (B1, B2, D8, Codex's ordering, the
  empty trace) and its unit tests.
- 1a40bfc: the CLI recorder (C1: one name a call, a 32 GiB pool, an
  8 GiB floor, K at most 20000, R09's budget check) and writing's margin
  counted from when its logs began.

`agent-record.sh` and its tests are unchanged since bca1a27. Line
numbers are at 1a40bfc. The scripts and outputs of this look are in this
directory with the prefix `nl_` (how to run them is at the end).

## Verdict

- **Nothing in #159's own code blocks the merge.** Every fix claimed
  was checked against the code, on the head's runs and by plants:
  - B1 is fixed.
  - B2 is fixed, except one member of the new set (D11).
  - M1, D8, Codex's ordering, the empty trace, C1 and R09 are fixed.
- **CI is not all green on the head (M2, for the lead).** The agent rig's
  job k is red on one FAILED line, `pin_destroy_first_run`. That is a
  true verdict on the agent (`crates/skyzfs-agent`, which this pull
  request does not touch), not on the rig. Every other run that ran is
  green, the publish rig among them, so **M1 is settled**.
- **Should be fixed before the merge, but not blocking:**
  - D11: MOVED holds `destroy:after`, where the engine takes T again
    under its own name. It is one line and a unit case. No job reaches
    it today.
  - The description: it still describes the batching and its fallback,
    lists commits up to b99f21a, and says the kernel runs are pending.
- **Withdrawn from my draft:** a free-space read every 100 calls is
  often enough. The head's recording shows it (Measurements).
- **Test gaps (not blocking):**
  - a retry that reuses an abandoned T passes;
  - a double publish after a MOVED kill passes;
  - a take left behind after a MOVED kill is not looked for;
  - a scenario that adds no kill point is silent (K-destroy ran 0
    cases on this head);
  - the survivors listed below.

## M2: the agent rig's k is red on a verdict against the agent (for the lead)

Run 45781642042, job 139778306943 (k, 22.6 min, 94 cases). The only line
that is not ok:

    arare: pin_destroy_first_run: FAILED: [server] the first run ended 1, not 3: skyzfs-agent: zfs: no such pool

The case `destroy`, T `skz-20261126T082522Z`:

- The trace ends at `asked:after`. That is where the rig releases the
  hold and destroys `pool@T`, as a person would.
- The first run exited 1 with `zfs: no such pool`, though the pool
  exists. The settle then gave T up as `destroyed` (exit 3), and the
  retry published another name.
- Each pin after the first run is ok: outcome, criterion, stays_absent
  and clean.

**Likely mechanism** (medium certainty: the agent does not keep zfs's
stderr, and the artifact has only the agent's own message):

1. After the ask, the agent reads the holds of the take with
   `zfs holds -H -r pool@T` (`crates/skyzfs-agent/src/zfs/cli.rs:546`).
2. When the destroy lands between libzfs's open of a snapshot and its
   `lzc_get_holds`, `zfs_get_holds` turns ENOENT into EZFS_NOENT,
   whose text is "no such pool or dataset".
3. `failure()` (`cli.rs:924-926`) matches the substring `no such pool`
   in that message before it looks for `dataset does not exist`, so the
   error becomes `ZfsError::NoPool` and the agent exits 1.

17d057c (in the base) narrowed NoPool for `dataset does not exist` of
other names, but not this message.

The race is intermittent. On b99f21a, the destroy case's first run ended
3 in k. The pin and the agent code are both older than this pull request
(the pin is in the base's `agent_check.py:568`).

Consequence on this head:

- K-destroy ran 0 cases (9 on b99f21a). Its trace was a prefix of
  plain's, so `points` found no new point and stayed silent (Test gap
  G-e).
- `obs_points` in every job: failed and destroy were never reached.

The lead decides between:

- an agent work package, where a snapshot read treats "no such pool or
  dataset" of a snapshot's name as NoSnapshot, followed by a rerun of k;
- merging with k red on this one recorded line.

The rig did what it is for here.

## Defects

### D11. MOVED holds `destroy:after`, where the engine takes T again under its own name (latent false FAILED; should fix, not blocking)

Where: `tests/kernel/agent_check.py:353-359`, the comment and the set.
`expected()` (368-386) and `pin_ID_moved` (754-758) use it.

- After a kill at `destroy:after` of a take not adopted, the record is
  at Phase::Intent and `pool@T` is gone.
- By the planner's rule (`crates/skyzfs-agent/src/plan.rs:509-516`), the
  next start then takes T again under the same name and publishes T.
- The engine's own test says so: `a_crash_around_the_destroy_takes_again`
  (`crates/skyzfs-agent/src/engine/tests.rs:2197`) expects
  `published_as(&one.name)` for `after` and `published_anew()` only for
  `before`.
- The same holds for the destroy inside `failed()` at the last check
  (`engine.rs:1295-1297`): after it, the record is still at Intent, and
  the take is gone.

Scenario (the show-case SC5 in `nl_showcases.py`, built with
test_agent_check's fixtures): a kill at `destroy:after:1`, and the
settle publishes T itself.

- The judge expects `absent` and FAILS `pin_A-x_outcome` ("k1:
  published, not absent").
- It also FAILS `pin_A-x_moved` ("the settle published no name but k1").
- The run is right.
- With `destroy:before` (SC5b), the judge is right.

Certainty: high on the rule; low that it fires soon.
- No job reaches `destroy` or `failed` today (`obs_points` in a1, a2, d
  and k).
- A take must fail its check, or go unadopted, inside the first run
  before those points are reached.
- The plant M02 (drop `('destroy', 'after')`) survives, so nothing pins
  the right set.

The other members are right. At `failed:*`, `destroy:before`,
`take:after`, `manifest:*` and `taken:before`, `pool@T` exists at
Intent. The next start adopts it with no reading from before it,
`retake(false)` destroys it, and the publish takes the next name.

Fix:
1. Drop `('destroy', 'after')` from MOVED.
2. Say in the comment why.
3. Add a unit case shaped like SC5, which kills M02.

### The description is stale (should fix before the merge; not a code defect)

It is the report a reader takes cold (CLAUDE.md):

- Line 5 lists commits only up to b99f21a. 78bb051 and 1a40bfc are
  missing.
- Lines 53 and 59 describe "as many names a call as make about 10000
  snapshots" and "the fallback to one name a call". Both are gone.
- Line 73 says that the kernel runs on b99f21a are pending.
- It does not give the head's runs, k's red line, or the 32 GiB pool,
  the floor and the bound of K.

## Measurements (for the lead): the CLI recorder on 1a40bfc

Dispatch 45781769557, Ubuntu 26.04, ZFS 2.4.1, `10:1000 10:10000
100:1000`, success.

From `about.txt`, the pool's allocated bytes at each tenth (32 GiB pool,
no writer during retain):

| entry | K per dataset reached | snapshots | allocated | time |
|---|---|---|---|---|
| 10-s1000 | 1000 | 11000 | 0.187 GB | 123 s |
| 10-s10000 | 1000 | 11000 | 0.187 GB | 121 s |
| 10-s10000 | 2000 | 22000 | 0.531 GB | 242 s |
| 10-s10000 | 3000 | 33000 | 4.95 GB | 454 s |
| 10-s10000 | 4000 | 44000 | 12.16 GB | 681 s |
| 10-s10000 | 5000 | 55000 | 21.09 GB | 909 s |
| 10-s10000 | 5500, stopped before @r5501 with 8.26 GB free | 60500 | | 1080 s |
| 100-s1000 | 1000 | 101000 | 1.69 GB | 347 s |

- **The room a snapshot takes grows with how many its dataset already
  has.**
  - Up to 1000 a dataset, it is 14 to 17 KB a snapshot, the same at 11
    and at 101 datasets.
  - From 3000 to 4000 a dataset, it is about 0.66 MB a snapshot.
  - From 2000 to 3000 a dataset, it is about 0.40 MB a snapshot.
  - From 4000 to 5000, it is about 0.81 MB.
  - b99f21a's 1:100000 run agrees: 16 GiB was full at 9729 a dataset.
  - The cause is not known from these lines.
- **The floor works.** 10-s10000 stopped at 5500 with 8.26 GB free, and
  every step after it ran: the take, members, get, the pairs, and list
  with 60522 lines.
- **The room, not K's bound of 20000, limits K once there are more than
  one or two datasets.** With 11 datasets, 32 GiB holds 5500 a dataset
  above the floor. By the same growth, about 12000 with 2 datasets.
  That second figure is an estimate; dispatch 45781845472 was still
  running when I wrote this.
- **The records are named by the K asked.** In this run,
  `scale-10-s10000-*` are records at 5500. Only about.txt's line
  "stopped before @r5501", and the list's 60522 lines, say so (nit N3).
- **Time a call:** about 0.12 s a call at 11 datasets below 1000, and
  0.21 to 0.23 s from 2000 to 5000. At 101 datasets it was 0.23 to
  0.50 s. The lead's cap assumed about 0.12 s a call. The 80-minute
  budget (`scale_limit`) is checked before each call (R09, killed).
- **Why a read every 100 calls is often enough** (my withdrawn C2):
  1. Past 2000 a dataset, a dataset with K snapshots adds about 0.16·K
     to 0.18·K KB a snapshot.
  2. So 100 calls add at most about 18·(N+1)·K KB.
  3. Under the caps, (N+1)·K is at most 200000, so that is at most
     about 3.6 GB.
  4. That is less than the 8 GiB floor.
  5. In the run, the last 500 calls before the stop took 4.47 GB, about
     0.9 GB per read.
  6. The growth past 5500 a dataset is an extrapolation: it was not
     measured.

## Test gaps (none blocks)

- **G-a. A retry that takes an abandoned T again passes**
  (`agent_check.py:742-786`).
  - `reused` is any name the retry published under T. The engine never
    takes again a name it gave up: `fresh_name` skips names in the
    record and in the abandoned list (`engine.rs:1608-1610`).
  - In SC2, T was given up as `destroyed`, and the retry publishes a new
    snapshot under T with node B on it. The judge gives exit 0, with
    `obs_destroy_name_reused` as INFO only. SC2b (node B on another
    commit) is FAILED, which is right.
  - The plant N05p (no reuse when T is in an abandoned list) survives,
    so the stricter rule breaks no test.
  - Suggest: reuse of a T that is abandoned in the record, or in any
    run's output, is FAILED.
- **G-b. A double publish after a MOVED kill passes**
  (`agent_check.py:754-758`).
  - `pin_ID_moved` needs some name other than T, not exactly one.
  - In SC4, the settle publishes k1-1 and k1-2 for one take, and the
    judge gives exit 0. This is older than this pull request.
- **G-c. A take left behind after a MOVED kill is not looked for.**
  - `pin_ID_clean` reads the record, holds, manifests and assist, not
    the snapshots.
  - `snapshots.txt` has the data. In K-sweep-take-after-1 on this head,
    `pool@T` is gone from it, as it should be.
- **G-e. A scenario that adds no kill point is silent.**
  - `points` exits 0 with no output when the trace is a prefix of the
    base's (`agent_check.py:522`). That is pinned on purpose: K08a is
    killed.
  - The result: on this head, K-destroy ran 0 cases, and no line in
    the summary says so.
  - Suggest: an `obs_` line with each scenario's count of K cases.
- **K21 and K22 still survive** (re-indented anchors):
  - K21: `stays_absent` judged even where the settle published T.
  - K22: a failure of node B's `snap-row` read as absent.
- **The shell survivors are unchanged** (25 of 25 survive; see the list
  below). They wait for the next rig package with D7, D9 and D10, as
  the answer says.
- **Set A of #158** still survives: P43, P46, P48, P52, P61, P64, P65,
  P67, P73, P74, Q06, Q23, N05 and N07. P60 is now killed.
- **The recorder's survivors:**
  - C06: a free that is not a number stops nothing.
  - C09: no line at the last call when K is not a multiple of a tenth.
  - C16: 32 GiB without K too.
  - C20: the count is not recursive.
  - C22, C23, C24: `began` is not called, waits for nothing, or does not
    wait for the fill log. 1a40bfc's margin fix has no test.
  - R10a: a call with no time limit.
  - R14: retain run with K 0.
  - A04a, A07, A08, A09: the parse's guards. `nl_nk_check.out` shows
    that the workflow refuses these inputs first.

## Nits

- **N1.** The K-sweep fixture in `test_a_kill_between_the_take_and_taken`
  (`test_agent_check.py:710-712`) has `enrol` before/after and one
  `intent`.
  - The recorded K-sweep-take-after-1 traces (b99f21a's run, and the
    head's run, T `skz-20261126T083045Z`) have no `enrol` and two
    `intent` pairs.
  - The judge does not depend on the difference, but the comment says
    the fixture is that run.
- **N2.** `retain` runs `zpool list` without `lim` (`zfs-cli-record.sh:1032`
  and 1048). The pool is a fresh file-backed one, so the risk is low,
  but every other ZFS command in the function has a limit.
- **N3.** The records are named `scale-N-sK` by the K asked, not reached
  (Measurements). An `index.tsv` or summary line with the K reached
  would keep a reader of #157's R1 from taking the 5500 figure as
  10000.

## The questions asked

1. **B2 against design §5.2/§5.4 and engine.rs.**
   - The rule "killed between the take and `taken`, the next start
     destroys the take and publishes under a new name" is exact for
     every member but `destroy:after` (D11).
   - Could it pass a run that should fail? Only by G-b, a double
     publish.
   - Every other plant on it is killed: M01, M03 to M11.
   - The K-sweep fixture is real in its outcome but not in its trace's
     shape (N1).
   - On the head, 8 MOVED cases have `absent` and `moved` ok:
     - a1: manifest-before, taken-before;
     - a2: take-after, manifest-after;
     - k: K-sweep take-after, manifest-before, manifest-after,
       taken-before.
2. **B1's new pin.**
   - G7's case still fails: SC1, the withdrawn k1 published later, is
     FAILED on `stays_absent` and on `outcome`.
   - A reuse of the name in the same second never fails: SC3, and a1's
     A-intent-before-1 on the head, which reused T
     (`obs_..._name_reused` INFO, `stays_absent` ok against the retry's
     commit).
   - A wrong commit fails: SC2b and SC3b.
   - N01, N02, N04 and N06 are killed. N03 is killed by another test.
   - Left open: G-a.
3. **C1.**
   - `test_zfs_cli_record.sh`: 522 ok, exit 0, 86 s.
   - Of 29 plants on retain, the bound and `began`, one at a time, each
     restored, with a 600 s limit: 16 killed, 13 survived.
   - Killed: C01 to C05, C07, C08, C10, C13, C14, C15, C17, C18, C19,
     C21 and R09a. That covers the floor, its read interval, the tenths,
     a failing call, one name a call, recursion, 32 GiB, the bound of
     20000 and the budget.
   - The workflow's check and `scale_all` agree on 37 inputs
     (`nl_nk_check.out`).
4. **The surviving plants.**
   - Python: P60 and K24 are killed now. The rest still survive (Test
     gaps).
   - Shell: 25 of 25 survive, unchanged.
   - New Python plants on 78bb051: 21 of 23 killed. N05p survives (a
     probe of G-a), and M02 survives (D11).
   - D8: D01 and D02 are killed. Codex's ordering: X01 and X02 are
     killed. The empty trace: E01 and K08a are killed.
5. **CI of the head.** The table follows.

| run | what | result |
|---|---|---|
| 45781642042 | agent rig: a1, a2, d | success |
| 45781642042 | agent rig: k | **failure**: one line (M2) |
| 45781642039 | cut rig, 4 images | success; 10 courses exit 0 on each image |
| 45781641979 | cut-daemon, 3 jobs | success: 356 CAUGHT, 256 INFO, 399 PASS, 176 SAME, no FAILED |
| 45781641986 | node B rig: life, slog | success, exit 0 |
| 45781642083 | publish rig on skyzfsd, ubuntu-26.04 (M1) | success: 9 courses checked exit 0, only ok, INFO and CAUGHT |
| 45781642369 | ci: changes, lint, test, unit-arm64, chart-lint, s3-backends | success |
| 45781642143 | e2e | success |
| 45781642119 | coverage | success |
| 45781633380 | skyzfs-ci (push) | success |
| 45781633361 | zfs-cli-record (push) | success |
| 45781769557 | zfs-cli-record by dispatch | success (Measurements) |
| 45781845472 | zfs-cli-record by dispatch | still running at the time of writing |
| 45781642240, 45781641971, 45781641977 | daemon rig, probe, replay rig | cancelled by the author. Right: none of their scripts reads a file this pull request changes |

## How this was checked

- **Local tests**, in a scratch worktree at 1a40bfc, with Python run as
  `python3 -I`:
  - `test_agent_check.py`: 69 OK.
  - `test_zfs_cli_record.sh`: 522 ok.
  - `test_agent_record.sh` and `test_cut_record.sh`: all pass.
  - `test_publish_check`, `test_publish_check_main`, `test_cut_check`,
    `test_nodeb_check` and `test_cut_daemon_check`: OK.
- **shellcheck 0.9.0:**
  - `zfs-cli-record.sh` is clean.
  - `test_zfs_cli_record.sh` has the base's 16 SC2218 and one SC2034
    fewer.
- **Artifacts read:** a1, a2, d and k of the agent rig, the publish rig,
  node B, cut-daemon, the four cut rig images, and the recorder's
  dispatch. The judges' output, traces and case files were read in each.
- **Plants.** Each plant replaces one anchor that occurs exactly once,
  runs the test command with a 600 s limit, and restores the file
  (`plant2.py`, unchanged from my review of b99f21a).
  - `nl_gen.py` wrote the specs kept here as `nl_old_py.py` and
    `nl_old_sh.py`: the survivors of #158's sets and of my review, with
    anchors adapted to the head. K08's anchor is gone, so it became
    K08a in `nl_new_py.py`.
  - `nl_new_py.py` holds the new plants on 78bb051.
  - `nl_r1.py` and `nl_r2.py` hold the retain, bound and `began`
    plants, split in two to run side by side.
  - Their outputs are the `.out` files of the same names.
- **To run:**
  - Plants, from the checkout's root at 1a40bfc:
    `python3 review/m2.t5b/pr159/plant2.py . review/m2.t5b/pr159/nl_r1.py`
    (the same for each spec).
  - Show-cases: `python3 -I review/m2.t5b/pr159/nl_showcases.py .`
  - The input table: `bash review/m2.t5b/pr159/nl_nk_check.sh`.
