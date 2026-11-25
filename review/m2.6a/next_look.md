## Next look at #140 (M2.6a), head 72a3b5e

Reviewed at **72a3b5e**, the pull request's head. My review was at
`83b1d9d`; this look covers `83b1d9d..72a3b5e`, the author's answer
(issue comment 7153488337), and the commit `72a3b5e`, which changes only
the publish rig (`git diff --stat 7d1152a 72a3b5e` lists only files under
`tests/kernel/`; the Rust is that of `7d1152a`). I checked each answer
against the code, the tests, plants and the rigs' own recordings, not
against the answer's words.

**Verdict: can merge, once the three rigs still running on 72a3b5e end green.** Ten of thirteen runs are green, the publish rig among them. The cut rig, the daemon rig and the replay rig were still running at 05:47 UTC (below).

- B1, B2 and B3 of my review are settled (below).
- The three things asked of this look:
  1. prewind keeps its committed rewind-opening cut check, re-anchored on
     the first ask, and the replay of the rig's recordings shows the
     anchoring is load-bearing. But under rule (R) no publish is pending
     when that cut is committed, so the course no longer tests "opens as
     itself" with a publish waiting. That rule is pinned by unit tests and
     the cut rig instead (N1 below). Not blocking.
  2. pkillpub ending `refused taken` then `look p1` is an acceptable pin of
     N1 as kept: each step is pinned in both directions, and "found, not
     made anew" is pinned against node B's read between the kill and the
     take-over. Two small gaps (N4, N6).
  3. `EXPECTED` and `PUBLISHED` are pinned in both directions, and the new
     tests fail when the behaviour breaks: 10 of 10 plants in the check
     killed. Five plants in the course structure survive the shell test
     (N5).
- No finding blocks. The decisions for the lead are at the end.

### CI at 72a3b5e

- Green: `ci`, `coverage`, `e2e`, `k3s-smoke`, `skyzfs-ci`,
  `skyzfs-control-rig`, `skyzfs-cut-daemon`, `skyzfs-kernel-probe`,
  `skyzfs-nodeb-rig` and `skyzfs-publish-rig`. The publish rig was red at
  `7d1152a` (run 45551498087) and is green on 26.04 at `72a3b5e` (run
  45553555949).
- Still running at 05:47 UTC: `skyzfs-cut-rig` (run 45553555874; its
  four record-and-check jobs), `skyzfs-daemon-rig` (run 45553556047; five
  jobs), and `skyzfs-replay-rig` (run 45553555885). In the replay rig,
  six of seven jobs are green and the whole-pool layout is running.
- The same rigs at `7d1152a` were cancelled by the push of `72a3b5e`.

### The earlier blocking findings

**B1 (the cut rig's guard): settled.**
- `tests/kernel/cut_plan.py` and `.github/workflows/skyzfs-cut-daemon.yml`
  are byte-identical to `zfs-topology`'s tip `6758025` (#139's `af9b7f0`
  lines): `git diff origin/zfs-topology 72a3b5e` on both files is empty.
- `ci` / lint, `skyzfs-cut-daemon` and `skyzfs-ci` are green at
  `72a3b5e`.

**B2 (the rewind to a checkpoint; the lead's D1): settled.**
- The false premise in `cuts.rs` ("a group can only go down or repeat
  across cuts by a rewind or by a group that errs low") is gone.
  "What (R) does not see" names `--rewind-to-checkpoint` with the source
  line.
- The pin `a_rewind_to_a_checkpoint_is_not_marked` fails if the rule
  changes: plant Q2 (`<=` for `<`) kills it, among others.
- One sentence is inexact (suggestion S1 below).

**B3 (the stale mark; the lead's D2): settled for the channel.**
- The handler reads the store's mark at the desk and sends it in
  `Control::Ask`. The uploader sets it before it answers
  (`crates/skyzfs-upload/src/uploader.rs:413-414`).
- My two scratch cases, re-run with the mark as the channel now gives it
  (`review140n.rs`, `next_f2a_…`, `next_f2b_…`), are refused `rewound`
  and nothing is published on the abandoned history.
- The author's two tests in `tests/uploader.rs` are the same cases.
- Plants: B3a (no `set_rewound` at the ask) not run at this look (the run was stopped when the look ended); the author reports it caught; B3b (the handler
  sends 0) not run at this look (the run was stopped when the look ended); the author reports it caught; B3d (the mark set after the ask) not run at this look (the run was stopped when the look ended).
- The driver used without the channel still publishes on the abandoned
  history in that window, and `publish.rs`'s module docs now say so.

### What this look was asked to judge

**(a) The handler's shared limit (`Waiter.began`).**
- `began` is taken in the handler's closure, once per request
  (`bins/skyzfsd/src/control/handler.rs:80-85`). `within()` gives each
  wait what is left (`handler.rs:110`). Correct.
- Plant Wa (each wait gets the whole limit) not run at this look (the run was stopped when the look ended); the author reports it caught.
- Plant Wb (`began` taken once, when the handler is made) not run at this look (the run was stopped when the look ended). `a_publish_waits_for_the_store_and_the_uploader_within_one_limit` makes its handler just before the request, so I expect it to survive.
- Cosmetic: the three `NoAnswer` texts say "did not answer within {limit}
  s" (`handler.rs:203,230,251`). After a slow first wait, the second one
  waited less than that.

**(b) The argument in `publish.rs`'s module docs that the mark at the ask
is enough.**

It holds within its premises, which I checked one by one:
- A cut is counted at the flush barrier on the store's thread, before the
  flush completes, so before ZFS's sync of the snapshot returns.
- The desk is answered on that same thread, after what is already queued.
  So a cut counted before the ask is in the mark the ask carries.
- R' marks the first cut of `-F` (equal group, another state) and every
  `-T` cut up to and including the abandoned top's group.
- The cases R' does not see are named: the checkpoint, and a rewind that
  passes the top before any cut.

**(c) The cost of R' after `-T`.**
- The rig's own recording at `72a3b5e` (prewind, 26.04) shows it.
  - The import is at `-T 25`. The abandoned top is cut 18, of group 43.
  - Cuts 19 to 26 (groups 27 to 40) open only as a rewind, and are marked.
  - Cut 27 (group 43, the top's group in the new state) opens as itself,
    and R' marks it too.
  - The first cut that can carry a publish is cut 28, of group 46.
- So an agent that asks inside that window is refused once per cut, about
  every five seconds of the pool's groups, until the pool passes the top.
  The number of such asks is bounded by the ring (32 slots at ashift 12).
- The rig does not loop. It waits with `past_top` and then asks once,
  which is the cheaper way.
- My scratch table (`next_the_cost_after_t`) agrees: done on group 29
  after six asks.

### The publish rig at 72a3b5e

**(1) prewind keeps its committed rewind-opening cut check.**
- The premise `premise_rewind_cut_committed` is still computed. It now
  uses the first ask, read with `read_published(..., first=True)`
  (`tests/kernel/publish_check.py:569-586`).
- I replayed the rig's recordings through the pure functions of the
  check (`replay72.py`; output in `replay72.out`). The recordings are
  26.04, run 45551498087 at `7d1152a` and run 45553555949 at `72a3b5e`.
  - The check at `72a3b5e` on the `72a3b5e` recording: every answer as
    `EXPECTED`. The premise holds with the first ask (C = 20, cut 23 of
    group 33, a rewind) and fails with the ask anew (C = 27). So
    `first=True` is load-bearing.
  - The check at `72a3b5e` on the `7d1152a` recording: a rig error
    (labels `r1-anew` and others missing), not a pass.
  - The check at `7d1152a` on the `72a3b5e` recording: a rig error
    (labels it does not know).
- Plants K5 and K6 (the premise read from the last ask) are killed by
  `test_prewind_s_premise_is_of_the_first_ask`, among others.
- The limit of this check is N1 below.

**(2) pkillpub ending `refused taken`, then `look p1`.** Acceptable as
the pin of N1 as kept.
- Every step is deterministic. `p1-again` carries the old count (6),
  below the take-over's mark. `p1-anew` carries a count at or above the
  landed row's cut (9), so the lookup cannot take that row as its own.
- In the run: `refused taken`, then `snap commit=af43a26b… number=9
  group=15`.
- `pin_p1_found` compares the look's commit with the ref node B read
  between the kill and the take-over. So a second snapshot made anew
  fails it, and plant K9 (reading `p1-end` for the commit) is killed.
- `pin_p1_row` judges the landed row against the original request
  (C 6, G 13), which is the right request: the anew one is not recorded.
- If the lead later answers a request below the mark from its lookup
  first (my alternative in N1), `EXPECTED` must change on purpose. K3,
  which widens `p1-end` to `done`, is killed by the exact table.
- Gaps: N4 and N6.

**(3) The tables in both directions, and the new tests.**
- `test_the_tables_are_exactly_these` compares `EXPECTED` and `PUBLISHED`
  whole. `judge_ends` raises a rig error for a label that is missing or
  unknown (`publish_check.py:229-237`).
- `test_a_request_from_before_a_rewind_not_refused_rewound_ends_1` feeds
  M2.5's answers to all four courses and expects exit 1 with the pin
  `FAILED`.
- The rig's plants (`rigplants72.py`; output in `rigplants72.out`), one
  at a time, each file restored in `finally`:
  - In `publish_check.py`, K1 to K10: all killed.
  - In `publish_agent.py`: A1 (a snap's state unchecked) and A2 (`look`
    sends `publish-progress`) are killed. A3, A4 and A5 survive (N7).
  - In `cut-record.sh`: S1 to S5, S7 and S12 are killed by
    `test_cut_record.sh`. S6 and S8 to S11 survive (N5).
- The suites pass at `72a3b5e`: `test_publish_agent.py` (19 tests),
  `test_publish_check.py` (24), `test_publish_check_main.py` (24), and
  `test_cut_record.sh` (all passed). shellcheck finds nothing new on the
  two shell files against `7d1152a`.

### Findings at 72a3b5e

None blocks the merge.

#### Defects (not blocking)

**N1. prewind no longer tests "opens as itself" with a publish pending.
Its docs say it does.** Certain, from the rig's own recording.
- `r1` is refused at the ask. In `agent.tsv` the refusal came at
  151.268 s, and cut 23 was counted at 151.279 s, then committed once the
  store came back. So when the cut that the premise names is committed,
  no publish is waiting.
- Under R' no publish can wait across a cut that opens only as a rewind:
  R' marks every such cut that the store has seen, and the mark ends
  whatever is pending below it. So the kernel rig can no longer reach
  the case.
- The rule is still pinned elsewhere:
  - Plant C1 (`&& cut.itself` dropped in `carries`) is killed at `83b1d9d` (my review's plants); not re-run at this look by the
    unit tests of `publish.rs`.
  - The cut rig judges each cut's "itself" against the kernel's reference
    (`cut_daemon_check.py:621-632`).
- What to fix: the docs overstate what the course shows.
  - `cut-record.sh:1600-1602` says the rewind row is "the check's premise
    that a cut a daemon without 'opens as itself' would name was committed
    after the ask".
  - `publish_check.py:73-76` says `pin_answer_r1` and `pin_r1_cut` judge
    that daemon.
  - Only a daemon with neither (R) nor "opens as itself" would name that
    cut, and `pin_answer_r1` alone catches it. `pin_r1_cut` judges the ask
    anew.
  - Say instead that the course now pins (R) at the ask and the way out
    past the top, and that "opens as itself" rests on the unit tests and
    the cut rig.
- The commit message and the description ("the committed rewind-opening
  cut check is kept") are true of the premise and not of what it was for
  (#139's B2).

**N2. The proposed text for design §5.2 says "none of those opens as
itself anyway". The cut at the top's group does.**
- In the description's proposed §5.2: "after `-T`, each cut up to the
  abandoned history's top moves it, and none of those opens as itself
  anyway."
- In the rig's recording, cut 27, of group 43 (the abandoned top's group),
  opens as itself, and R' marks it.
- `cuts.rs`'s module docs have it right: "the cut at that group
  overwrites it and is as the first of a `-F`."
- The lead writes §5.2, so this is for the lead (D-b).

**N3. R' over-marks a pool made anew on a volume the same daemon serves.**
Certain in the fixture (`review140n.rs`, the table's rows "a pool made
anew"); safe.
- `zpool create` writes the rings with a template of group 0
  (`vdev_label.c` 1211-1246). The new pool's groups start low.
- The top stays the old pool's in memory, so every cut is marked until
  the new groups pass it or the daemon restarts. In the fixture: count 5,
  mark 5; after a restart, mark 3.
- Every publish is refused `rewound` until then. The docs' "errs high …
  only another writer" does not name this case. It delays and never
  lets a publish through.

**N4. The `taken` refusal names the wrong reason at the end of pkillpub.**
- `p1-end` reads `refused taken the name names another commit`.
- The name names the same commit. The row's cut (9) does not carry the
  request asked anew.
- The text is `Refusal::Taken`'s, for all of its cases
  (`crates/skyzfs-upload/src/publish.rs:259-263`). It predates this pull
  request, but this is now the answer after every take-over (N1 of my
  review, as kept).
- Something like "`snap/T` exists and does not record this request"
  would cover every case.

**N5. The shell test does not pin `rec_pkillat`'s steps, nor the
`record` arguments.** Certain, by plant.
- `test_cut_record.sh` pins the order of `rec_prewind` and `rec_pkill`
  (`test_cut_record.sh:430-435`), but not `rec_pkillat`, and not which
  `panew` records.
- Survivors, and what each would do on a run:
  - S6 (`panew r1-anew r1` without `record`) and S10 (the same for `m1`)
    would also pass the rig. `pin_r1_row` and `pin_m1_row` would judge the
    publish against the first ask's count, a weaker bound: in the
    recording C = 20 in place of 27.
  - Then a daemon that published `r1` anew on cut 27 would pass. Cut 27
    is the one counted at the ask anew's count. It opens as itself, so
    `pin_r1_cut` would not catch it either. The unit tests catch that
    fault (C4 of my review).
  - S8 (`p1` anew recorded) would turn the run red as a verdict against
    the server: `pin_p1_row` would judge cut 9 against the new count.
  - S9 (no `look p1`) and S11 (no ask again) would give a rig error.
- One more `order_of` line for `rec_pkillat`, and one that greps the
  three `panew` lines with their arguments, would pin it.

#### Suggestions

**N6. `look p1`'s other fields are not compared.**
- The check reads only the commit from `snap …`. `number`, `group`,
  `manifest-len` and `manifest-hash` are not compared with the row that
  `pin_p1_row` reads. They agree in the run: 9, 15 and 414.
- The unit tests pin them (L5 of my review was killed). Comparing
  `number` and `group` here would cost two lines.

**N7. Three small gaps in `publish_agent.py`'s parsing** (plants survive).
- A3: an `unreadable` answer's commit is not checked as 64 hex digits by
  any test.
- A4: `ask look` dispatching to `progress` is not tested through
  `main`. A run would catch it, with `pin_answer_p1-look` `FAILED`.
- A5: `manifest-len` as digits is not tested.

**N8. Plants on R' that survive.**
- Q5 (a second state of the top's group not remembered) survives. It only matters when a cut names the top's first state again after a second state was seen. Only a replayed or foreign write does that, so it is near-equivalent. A test, or dropping the branch, would settle it.
- Q7 (unverified uberblocks in the rings raise the top) survives. It errs high and only delays. No test has an unverified uberblock of a high group in the rings at the start.

**S1. "What is left" of the checkpoint case is put inexactly**
(`crates/skyzfs-local/src/inplace/cuts.rs:118-124`).
- A checkpoint taken after `pool@t` holds `pool@t`, so a rewind to it
  publishes the snapshot that was asked for. That is harmless.
- The case left open is a checkpoint taken before `pool@t` (and so
  before the snapshot) that the agent's last check before the snapshot
  did not see.
- A check after the snapshot closes it.

**S2.** The handler's `NoAnswer` texts state the whole limit ((a) above).

**S3.** `72a3b5e` adds a local `_i` to `rec_rewind` (`cut-record.sh:588`)
that nothing uses. It is harmless, but it is a change to a course of the
cut rig that the commit does not mention.

### Earlier non-blocking findings and tests

- N2 (`look`'s second read): done. L8 not run at this look (the run was stopped when the look ended); `a_failure_at_any_read_of_a_lookup_is_an_error` is there and passes.
- N3:
  - L4 (`lookup_of`'s volume) not run at this look (the run was stopped when the look ended); it is pinned, the author says, by
    `the_channels_look_reads_the_served_volume`.
  - R8a not run at this look (the run was stopped when the look ended). I agree it is equivalent through the channel: each ask
    carries the live mark, which is at or above the count at the start.
- N4 (the status's `rewound: 0`): done, `CutsShown { count, newest }`.
  The file's format is unchanged.
- T1: W1 (group parsed as number) not run at this look (the run was stopped when the look ended); `snap(3, 41, 19)` is in the round trip.
- T2: done for B2 and B3.
- R' plants Q1 to Q8 (my own): Q1 to Q4 and Q6 are killed by `tests/cuts.rs`. Q5 and Q7 survive (N8). Q8 did not finish
- Protocol version 2 (`d31c8aa`):
  - A client of version 1 compares the greeting, and refuses a line with
    an unknown code ("`x` is not a code"). So it fails closed, and
    CONTRIBUTING's "not said" exception does not apply. The bump is what
    the rule asks.
  - V1 (the greeting back to 1) not run at this look (the run was stopped when the look ended).
  - At `7d1152a`, G1 and G2 (the rig's greeting) were killed by the
    rig's tests.
- `bb318ed`'s move: since `83b1d9d`, `crates/skyzfs-control/src/lib.rs`
  changed only in its docs and `HELLO`, and `bins/skyzfsd/src/control.rs`
  only in one word of its docs. No regression.
- Codex reported no findings at `72a3b5e`.

### Decisions for the lead

- **D-a.** prewind (N1): accept that "opens as itself" now rests on the
  unit tests and the cut rig, with the docs corrected. Under R' the
  kernel cannot reach it without a test hook. My recommendation: accept.
- **D-b.** Design §5.2's text (N2): drop "none of those opens as itself
  anyway", or say "below that group".
- **D-c.** N1 as kept, pinned by pkillpub. A change to "answer from the
  lookup first" would change `EXPECTED` on purpose. Nothing to decide
  now unless you want that property back.
- **D-d.** The over-mark of a pool made anew (N3): name it in the docs,
  or accept it unnamed. It only delays.

### What I did

- Read `git diff 83b1d9d 72a3b5e` and the author's answer.
- At `7d1152a`, before `72a3b5e` (the Rust has not changed since), on
  rustc 1.100.0:
  - fmt: clean;
  - clippy `-D warnings`: clean, plain and with each of the two features;
  - the workspace's tests, plain: 841 passed, 0 failed;
  - `skyzfs-local` and `skyzfsd` with the test hooks: 520 passed, 0
    failed. The first try ran out of disk, and the second run passed;
  - my scenarios in `review140n.rs` (output in `scenarios_n.out`).
- Plants on the Rust (`plants140n.py`; output in `plants140n.out`), at
  the Rust of `72a3b5e`, one at a time, with suites `l` (`skyzfs-local`),
  `u` (`skyzfs-upload`), `d` (`skyzfsd` with the hooks) and `c`
  (`skyzfs-control`).
- At `72a3b5e`:
  - the rig's four suites and shellcheck;
  - the rig's plants (`rigplants72.py`);
  - the replay of the 26.04 recordings of runs 45551498087 and
    45553555949 (`replay72.py`).
- CI read through the REST API.
