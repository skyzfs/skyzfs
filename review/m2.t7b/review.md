# Review of #141 (M2.T7b): the publish rig after M2.6a, at e0c51f8

Head reviewed: **e0c51f8** (`gh api repos/skyzfs/skyzfs/pulls/141`), on
`zfs-topology` at 501ccf7 (the merge base is the base's tip). Four
commits: f97f343, 92b805a, 3f593d0, e0c51f8. Only the publish rig's
files change (`tests/kernel/`, the workflow); no product code.

I formed my view from `git diff origin/zfs-topology...e0c51f8`, the
design's rule (R) (`crates/skyzfs-local/src/inplace/cuts.rs`'s module
docs) and the rigs' own recordings before reading the description, then
checked its claims. File:line references are at e0c51f8.

## Verdict

**Blocked. Two findings block the merge, both on the new course prewindf
and the new premises (B1, B2).** Everything else can merge:
- #139's D1 to D3 and #140's N1, N5 to N7 and S3 are settled.
- prewind's ground and top are right.
- `look`'s pins are right.
- `zero_newest_root` is moved unchanged.

- **B1.** prewindf cannot run. `zpool import -F` is refused on the pool
  the course makes, so every run ends as the rig's error before the
  check. The pull request's own publish rig run is red for this reason.
  The cut rig's rewindf has been refused `-F` the same way in every
  recording; there only `-FX` imports.
- **B2.** The two new premises read the daemon's own answer line: `r1
  refused rewound` when asked, and `f1 refused rewound` as an end. The
  fault each course is there to catch therefore ends exit 2 (the rig's
  error), not exit 1. That fault is a daemon that does not refuse r1, or
  does not end f1. This is #139's D1 again, which this pull request says
  it fixes. The tests that say these faults end 1 keep the right
  daemon's log line, which the faulty daemon would not write.

## The three things asked

**(1) prewindf's premise.** No, it is not the right premise, for two
reasons.
- It never meets a kernel (B1).
- It rests on the daemon's verdict (B2). "f1 ended `refused rewound` by a
  `publish ended` line" is what the course judges, not something the rig
  did. Every daemon that fails the course fails it as the rig's error.

The part about the ground is right. "A cut counted after f1's C that is
not above the groups before it" is rule (R), the running maximum. It
includes the equal group of the first cut of a `-F`/`-FX`, which R'
marks (equal group, another state). Plant R1 (strictly below the top)
is killed.

The premise should rest only on what the rig and the kernel did. A
sketch that passes every right-daemon test is in `premise_sketch.diff`:
- the ground is the cut lines after C, with no end line needed;
- for prewind, the anchor is r1's first `asked` line, whatever it
  answered;
- f1 was still within its expiry at the ground cut, by the monotonic
  clock that agent.tsv and the request log share;
- whether the `ended` line follows the ground becomes a `server` pin.

**Vacuous passes.** I found none.
- Exit 0 needs the `ended f1 refused rewound` line after a cut counted
  after C whose group is not above the running maximum. In this course
  only the rewind import makes such a cut.
- A daemon whose mark sees only groups strictly below the top would not
  end f1, and fails the course. Today it fails as exit 2 (B2); with the
  sketch, as exit 1 (`scenarios.out`).
- The one weak pin is `pin_f1_absent`. With G at createtxg + 1000000,
  no daemon that honours G could publish f1 in either history. So the
  course's verdict on (R) is `pin_answer_f1-end` alone (D-c below).

**(2) look.** Enough for the verb as the rig can reach it.
- t1 is judged field by field against node B's row: the commit, state,
  number, group, manifest length and hash (`publish_check.py:338-347`).
  With `judge_row`, the look's commit is the ref's and `done`'s.
- t5 and t3 must read `absent`. r1 with the store down must read
  `refused store` or `refused no-answer`, never `absent`.
- Every plant on these is killed: L1, L3, L7, R15 to R19, R21 and the
  ordering plants L9, L12, R48.
- Replayed on the run's own pplain and prewind recordings, a planted
  number, group or state in t1-look ends 1, and so do `absent` for r1
  and a `snap` for t5 (`replay141.out`).
- In the kernel run, t1-look matched cut 8 of group 16, and r1-look read
  `refused store … dispatch failure`.
- Not covered by the rig: `unreadable`, which `skyzfs-upload`'s
  `look/tests.rs` covers. Cheap additions are a look at f1 after its end
  and at tw after its withdrawal (both `absent`).

**(3) zero_newest_root.** Unchanged for rewindf.
- The old `rec_rewindf` body, diffed against `zero_newest_root` plus the
  new `rec_rewindf`, differs only at the function's boundary: `create_pool`, `load 6`
  and `export_pool` stay in `rec_rewindf`, and the locals move with the
  body. Nothing after the call uses them.
- The cut rig through skyzfsd at e0c51f8 (run 45562017928, 26.04) has
  rewindf's result keys and verdicts identical to those at 72a3b5e (run
  45553555779).
- The cut rig (run 45562017961) is green on all four images.
- Plants R45 (rewindf without the call) and L13 are killed by
  `test_cut_record.sh`.

## Findings, most severe first

### B1 (blocks). prewindf's `zpool import -F` is refused: the course is the rig's error on every run

**Where.** `tests/kernel/cut-record.sh:1686` (`import_pool -F`, under
`z 120`, whose failure is `rig_error`).

**What happens.** Publish rig run 45562017962 at e0c51f8, ubuntu-26.04:
```
[06:44:59] prewindf: recording through skyzfsd on /dev/ublkb0 (512 MiB)
cannot import 'skzcut': one or more devices is currently unavailable
[06:45:04] the rig's own error in prewindf: zpool import -d /dev/ublkb0 -o cachefile=none -F skzcut failed (status 1)
[06:45:58] done: worst exit 2
```
- The other six courses exit 0 with every line ok, but the run is red.
- prewindf's recording stops at the export: f1 was asked with C 10, and
  cuts 13 to 16 came from the export, with group 34 zeroed.

**Why it fails, and that it always will.**
- rewindf zeroes the root block in the same way, and has needed `-FX`
  in every recording I read:
  - cut rig run 45553555874 on four images;
  - cut rig through skyzfsd, runs 45553555779 and 45562017928.
- Each `rewind-f.txt` reads `== zpool import -F` / `cannot import
  'skzcut': one or more devices is currently unavailable` / `imported
  with -FX`.
- `cut_check.py:102-106` gives the reason: "In rewindf -F is refused
  (the export rewrote the labels' config in the failed group) and -FX
  writes the failed group again with another root block pointer".
- The description's "Then `zpool import -F` goes back past that group"
  was not true of this pool before the pull request either.

**Certainty: certain.**

**Fix.**
- Import as `rec_rewindf` does: `-F`, then `-FX`, recording which one
  imported.
- In rewindf's recording at e0c51f8, the export's last cut is cut 18 of
  group 43. The import's first cut is cut 19 of group 43 again, and it
  opens as itself. That is "equal group, another state", which R' marks
  and the lead's addition is about. So with `-FX` the course should
  reach its case.
- `premise_f1_import_f`'s `'imported with -F' in damage` matches `-FX`
  as a substring. Say which import it was, and change the docs' "-F"
  (D-a).

### B2 (blocks). The new premises are the daemon's verdict, so the faults the courses are for end exit 2

**Where.**
- `tests/kernel/publish_check.py:454-472`: `rewound_grounds` returns
  "found" only at a `NAME refused rewound` line.
- `:659-663`: `premise_r1_rewound_with_a_rewind_cut`.
- `:691-695`: `premise_f1_ended_by_a_rewind_cut`.

Both premises are checked after the pins (`premise()` raises, and the
course ends 2, `:529-532`, `:724-726`).

**Scenario 1, on the run's own recording** (`replay141.py` on run
45562017962's prewind files, node B stood in for):
- Take a daemon without rule (R) at the ask, as M2.5 was. It answers r1
  `accepted`, and with the store down the flow ends `pending after 300
  s`. Its request log has `publish asked … r1 accepted` where the right
  daemon wrote `r1 refused rewound`.
- The result: `pin_answer_r1 FAILED`, then
  `premise_r1_rewound_with_a_rewind_cut ERROR` ("no `r1 refused rewound`
  line in the request log"). **Exit 2.**

**Scenario 2, prewindf** (`scenarios.py`, the pull request's own driver
of `main`):
- Take a daemon that does not end f1 at `-F`'s first cut: no `ended`
  line, and `f1-end` reads `pending after 300 s`. This is the course's
  only target.
- The result: `pin_answer_f1-end FAILED`, then
  `premise_f1_ended_by_a_rewind_cut ERROR`. **Exit 2.**
- The same happens for a mark that sees only groups strictly below the
  top, and for f1 ended `expired`.

**The tests that say otherwise.**
- `test_publish_check_main.py:358`
  (`test_a_request_from_before_a_rewind_not_refused_rewound_ends_1`)
  changes only `ends.tsv`. Its request log still holds `r1 refused
  rewound`.
- `:437` (`test_prewindf_s_publish_not_ended_or_published_ends_1`)
  keeps the `ended f1 refused rewound` line while f1 is said to be
  pending.
- With the log as such a daemon writes it, both end 2 (`scenarios.out`:
  "the PR test" rows end 1, "realistic log" rows end 2).

**Why it blocks.**
- prewindf exists to give a verdict on one fault, and that fault is
  classed as the rig's error. `CLAUDE.md`: "Exit 2 is an error of the rig
  and not a verdict on the server."
- prewind's pin of (R) at the ask, its main pin since #140, is classed
  the same way.
- The FAILED line is still in the summary. But this is the pattern of
  #139's D1, which 92b805a fixes for `rewind_committed` and these two
  premises bring back.
- pkill's `premise_tw_committed_past_g` has had the same shape since
  #139: it needs `tw withdrawn`. It is less pressing, because a withdraw
  that is not answered `withdrawn` is not that course's target.

**Certainty: certain** that the code ends 2 in these cases.

**Fix** (`premise_sketch.diff`):
- For prewind, take the ground before r1's first `asked` line, whatever
  it answered.
- For prewindf, take any cut after f1's C that is not above the groups
  before it, with no end line needed.
- With that sketch, every row of `scenarios.out` that should be a
  verdict ends 1, and right recordings end 0. The tests it changes are
  three that pin today's behaviour: `test_no_refusal_line_is_no_ground`,
  `test_an_end_is_read_only_as_an_end`, and the first two cases of
  `test_prewindf_with_no_ground_for_the_end_is_the_rig_s_error`.
- Two refinements, so that a wrong course is still the rig's error:
  - **f1 was within its expiry at the ground cut.** Plant R41 (expiry
    5 s) survives the shell test. Under the sketch a run with it would
    blame the daemon.
  - **"The end follows the ground" as a `server` pin.**

### Defects that do not block

- **N1. Stale words.**
  - `publish_agent.py:51` still says `raw` exits 2 when "the read timed
    out". Since D3 that is the answer `no answer within 60 s` (`:201`,
    `:242`).
  - `publish_check.py:88` says "r1 was not made by an earlier one", but
    since D1 it is "an earlier one that opens as itself" (`:436`).
- **N2. `raw` waits out the limit for an answer that is not followed by
  a close.**
  - A daemon that writes the greeting and its answer but does not close
    gets that answer taken only after 60 s (`:242`, `late and not got`).
  - This is correct as a reading; plant R32, which makes it no answer,
    survives.
  - Whether skyzfsd's own `ask` waits for the close is not pinned here.

### Test gaps (suggestions)

Plants that survive the pull request's four suites (`plants141.out`).
None of them makes a course pass a fault in a kernel run.

| plant | what | why it is not worse |
|---|---|---|
| R5 | `rewound_grounds` takes any refusal of the name | Turns a refusal with another code into exit 1, which is better (B2) |
| R7 | the top carried across instances | prewind and prewindf have one instance |
| R26 | `rewind-top.txt` not checked as digits | `int()` then raises, and the generic handler ends 2 |
| R32 | see N2 | |
| R40 | t3's look before t5's | equivalent |
| R41 | f1's expiry 5 s | a run ends 2 today (B2's premise); after the sketch it needs the expiry premise |
| R42 | f1's end read within 3 s | the end comes during the import, so near-equivalent |

Also:
- The workflow's default course list
  (`skyzfs-publish-rig.yml:178`) is pinned by no test.
- A look at f1 after its end and at tw after its withdrawal would cost
  two lines each.

## What the pull request settles

- **#139's D1: settled.**
  - `rewind_committed` (`publish_check.py:436`) fails the premise only
    when the earlier cut opens as itself.
  - Replayed on the run's prewind recording, r1 made on the rewind-only
    cut 22 (group 31) or 23 (group 32) ends 1, with `pin_r1_cut` and
    `pin_r1_above_top` FAILED and no ERROR.
  - Plant R9 (the old condition) is killed.
- **D2: settled.** `answer_of` needs `HELLO` as the first whole line
  (`publish_agent.py:256`). Plant R30 is killed.
- **D3: settled**, apart from N1's wording.
  - Once connected, a time-out is the answer `no answer within N s`.
  - A socket with no listener is still the rig's error.
  - Plants R31 and R33 are killed.
- **#140's N1: settled.** The docs now say that the course pins (R) at
  the ask and the way out past the top, and that "opens as itself" rests
  on the unit tests and the cut rig: `cut-record.sh:1612-1620` and
  `publish_check.py:94-100`.
- **N5: settled.** My #140 plants re-run at e0c51f8
  (`rigplants72_at_head.out`):
  - S6 and S8 to S11, which survived at 72a3b5e, are killed.
  - S12, which no longer applies as written, is killed as R49.
- **N6: settled.** See (2) above.
- **N7: settled.** A3, A4 and A5 are killed.
- **S3: done.** `_i` is gone from `rec_rewind`.
- **#139's plants re-run at e0c51f8** (`next_plants139_at_head.out`):
  - 66 killed. M7, M10 and P9, which survived at c7cc03f, are now
    killed.
  - The survivors are the ones judged equivalent or harmless then: M14,
    P8, Q6, Q7, A20, S3, S4 and S10.
  - The not-applied are plants whose text this pull request rewrote on
    purpose: E13, E14, P2, P3, A14, A19, S1 and S2.
- **prewind's ground and top (f97f343) are right.** The kernel run:
  - The `-T` import was at group 24, and the top read from `cuts` after
    the export was group 42.
  - The ground: cuts 21 (group 30) and 22 (group 31), counted after C =
    20 and before `r1 refused rewound`.
  - The first cut to open as itself is cut 27 of group 42, the top's own
    group, which R' marks.
  - r1 asked anew with C = 27 was `accepted`. A count equal to the mark
    is not older than it, so the course also shows that boundary.
  - r1 was made by cut 28 of group 45, above the top.
  - Plant R11 (`>=` for `>`) is killed.

## Decisions for the lead

- **D-a. `-FX` for prewindf (B1).** A plain `-F` is refused on this pool
  shape, because the export rewrites the labels in the newest group.
  `-FX`'s first group is the zeroed one again, with another root: the
  case R' marks.
  - Accept "a rewind import by `-F`, which here takes `-FX`", as rewindf
    does.
  - Or name another way to damage the pool that `-F` alone goes back
    from.
  - My recommendation: `-F` then `-FX`, recorded.
- **D-b. What a premise may rest on (B2).**
  - Either a premise reads only what the rig and the kernel did, never
    the daemon's answer to the request it judges (my recommendation),
  - or a premise that fails after a `server` pin has FAILED ends 1.
  - The second also covers pkill's `tw withdrawn`. But it would blame
    the daemon for a course the rig got wrong (R41).
- **D-c. What prewindf judges.**
  - With G far above, f1 could not be published in either history. The
    course pins "the mark ends a waiting publish", not "no publish on a
    history without its snapshot", and `pin_f1_absent` cannot fail for a
    daemon that honours G.
  - Fine as the lead's addition reads. Say so in the docs.

## CI at e0c51f8 (read through the API)

All runs have finished.
- **Red:** `skyzfs-publish-rig` (run 45562017962). prewindf is the rig's
  error (B1). Its unit job is green.
- **Green:**
  - `ci` (lint with the runner's shellcheck, `unit-arm64`, test, …),
    `coverage`, `e2e`;
  - `skyzfs-nodeb-rig`, `skyzfs-kernel-probe`;
  - `skyzfs-cut-rig` (run 45562017961, four images);
  - `skyzfs-cut-daemon` (run 45562017928, 26.04, 26.04-arm, upload);
  - `skyzfs-replay-rig` (run 45562018051);
  - `skyzfs-daemon-rig` (run 45562018042: the daemon on three images,
    the stand-in on two).
- The push runs of the same head were cancelled by the pull request's
  runs.
- Codex: completed at e0c51f8, no findings.

## How it was checked

- **Local gates at e0c51f8, Python 3.13:**
  - `test_publish_agent.py`: 22 OK.
  - `test_publish_check.py`: 31 OK.
  - `test_publish_check_main.py`: 36 OK.
  - `test_cut_record.sh`: all passed.
  - `unittest discover` in `tests/kernel`: 624 OK.
  - shellcheck 0.9.0 (older than CI's): the two changed shell files have
    the same codes as at 501ccf7 (SC1091, SC2015, SC2002, and SC2317
    infos on indirectly called functions); no new code. CI's lint is
    green.
- **Plants** (`plants141.py`, `plants141.out`, `.json`;
  `plants141_rerun.out`): one at a time against the four suites, 600 s
  each, every file put back in a `finally`; the suites passed before and
  after.
  - The description's L1 to L13, rebuilt from its words: all KILLED.
  - Mine: R1 to R4, R6, R8 to R25, R30, R31, R33 to R36, R43 to R49
    KILLED; R5, R7, R26, R32, R40, R41 and R42 SURVIVED (above). R45 was
    first NOT APPLIED and was re-run with the right text.
- **Replays** (`replay141.py`, `replay141.out`): the head's
  `publish_check.main` on run 45562017962's pplain and prewind files,
  downloaded into a new empty directory and run with `python3 -I`. Each
  case changes only what a faulty daemon would change.
- **Scenarios** (`scenarios.py`, `scenarios.out`): the pull request's
  stand-in driver with request logs as the faulty daemons would write
  them.
- **Rewindf's `-F`/`-FX` outcome and its cut lines:** artifacts of runs
  45553555874, 45553555779 and 45562017928.
- **No kernel here.** prewindf could not be replayed: its recording ends
  at the refused import.
