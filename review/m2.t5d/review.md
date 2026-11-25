# Review of #143 (M2.T5d): the bench's and the rigs' test gaps, at d3b75af

Head reviewed: **d3b75af** (`gh api repos/skyzfs/skyzfs/pulls/143`), into
`zfs-topology` at 59c4ad5 (the merge base is the base's tip). Five
commits: 8ef172f and 230dd28 (the bench), e5fab43 (the wiring tests of
the cut rig and node B), 07939ae and d3b75af (the publish rig). Only
test, rig and bench files change: no product code. The one workflow
change is the publish rig's default list of courses.

I formed my view from `git diff origin/zfs-topology...d3b75af` before I
read the description. Then I checked it three ways:

- against the rigs' recordings of this head's publish run (45588605464,
  ubuntu-26.04, prewindc's first kernel run) and of #141's (45569275289);
- against the daemon's request log as `record.rs` writes it;
- with plants in the working tree.

File:line references are at d3b75af.

## Verdict

**Can merge. No finding blocks.**

- **prewindc.** It tells a plain import from a rewind. A daemon that
  ends c1 at any import ends 1 with the premise ok. A rewind import, or
  no cut after the import, ends 2.
- **pin_watcher_reads** now judges the watcher's VERDICT lines in every
  course. The claim at `cut-record.sh:438-441` holds.
- **The ast wiring tests** catch what was asked: a bypass, a dropped
  call, a wrong or reordered argument, and a line without `{passed}`.
  They are blind to dead code and to what is done with `v` after the
  pick; the kernel rig's EXPECTED covers those. They fail on four
  harmless refactors.
- **N3** is fixed, **N4** is complete but for one comment, and **S1**
  is done.
- None of #132's, #139's or #141's pins regressed.
- No defect found. Three suggestions or test gaps (G1 to G3) and one
  nit (G4), none of which blocks.
- CI is green on d3b75af. The two rigs the author cancelled run
  nothing this pull request changes (below).

## Findings, most severe first

No defect. None of the following blocks the merge.

### G1. prewindc's premise does not tie c1's ask before the import, nor its expiry (test gap, low; certain)

**Where.** `publish_check.py:830-848` reads:

- the cut lines after c1's C,
- the mark `session-2-ready`,
- and up to c1's `withdraw` line.

It does not ask that c1 was asked before the export, nor that c1's
expiry runs past the read of its end.

**Two recordings that the premise lets through.** Each is the run's own
prewindc recording, changed, and replayed through `publish_check.main`:

- **c1 asked after the import.** Its C is 19, the first cut after the
  return, and its `asked` line is moved there. It **ends 0**, where it
  should be the rig's error. The course then shows nothing about an
  import, but the premise holds: there are cuts after C, after
  session-2-ready and before the withdrawal, and none is a ground.
- **c1 asked with `--expiry-s 30`.** It then (rightly) expires before
  `c1-end` is read, and it **ends 1** (`pin_answer_c1-end`), where it
  should be 2. A daemon that does its job reads as a verdict against
  it, because the course let the request expire.

**Reachable only through a rig change**, and `test_cut_record.sh` pins
both:

- the order: my plant S4, c1 asked after the import, is KILLED;
- the expiry: S7, `--expiry-s 30`, is KILLED.

C comes from the agent's request, not from the daemon, so no daemon can
move it. Hence low.

**Suggestion.** Two premises would make the course hold itself to what
it claims, as prewindf already does with `ask_deadline`:

- c1's `asked` line, or agent.tsv's send of c1, comes before
  `session-2-begin`;
- `ask_deadline(agent.tsv, 'c1')` is after the withdraw line.

### G2. The wiring tests pin the shape of the call, not its use, and fail on harmless refactors (test gap and brittleness, low; certain)

`test_cut_check.py:352-372` (`assert_pick_wiring`), used at
`test_cut_check.py:377` and `test_nodeb_check.py:300`.

**Plants in `cut_check.main` and `nodeb_check.main`.**

| plant | result |
|---|---|
| W5, W6: a bypass through `pick_withheld` | KILLED |
| W8, W9: the call dropped | KILLED |
| W10: the early cut passed | KILLED |
| W11: the arguments reordered | KILLED |
| W12: another list of blocks | KILLED |
| W13: `passed_over([])` | KILLED |
| W7, W7b, W7c: a line without `{passed}` | KILLED |
| W14, W15: the pick wrapped in `if False:` | SURVIVED |
| W16: `v` taken again from `pick_later` after the pick | SURVIVED |
| W17: the schedule keeps the picked write | SURVIVED |
| W18: node B's image keeps the picked write | SURVIVED |
| W19: `passed_over` before the pick | SURVIVED |
| W20: `passed_over(same[:1])` | SURVIVED |
| W21: the not-built branch taken whenever a write was passed over | SURVIVED |

**What catches the survivors.** On a kernel run, W14, W15 and W21 give
`not built`. W16 to W18 build another control or none. Either way the
result is against EXPECTED (`cut_check.py:92`: `withheld` is `caught`
in every recording; `nodeb_check.py:90`), so the rig fails. W19 is an
UnboundLocalError on any run. W20 only shortens a line.

**False failures.** Four harmless refactors fail the test:

- H1, keyword arguments;
- H2, `same` renamed;
- H3, `bps_at['mid']` for `bps`;
- H4, the pair unpacked on a line of its own.

`withheld_pick` imported by name (H5) passes.

The description says this plainly ("These pin the wiring, not a run"),
and the asked questions are answered. Brittleness is the cost of
reading source, and a maintainer who refactors gets a clear assertion.

**Suggestion, for later.** Move the withheld control's block into a
function, for example `withheld_control(ck, cut, bps, judge)`, that a
unit test can drive with a stand-in checker. It would then pin
behaviour and not shape.

### G3. `pin_watcher_reads` passes on an empty `watch.log` (observation, low)

`publish_check.py:677-683` fails only on a VERDICT line and makes an
ERROR line the rig's error. A watcher that ran and logged nothing
passes.

I found no way to reach that with a broken watcher. `watcher_end`
(`cut-record.sh:442-455`) makes any end but 0, or 1 with a VERDICT, the
rig's error, and the other pins judge the uploads. Noted so that the
pin's sentence ("node B read the content of every cut the watcher found
at head") is read as "of every cut it found", which may be none.

My plant V6 (the VERDICT match without its tab) survives. It is
equivalent on any line cut_watch.py writes.

### G4. Wording nit: N4's residue

`test_publish_check.py:58` says "`-F`'s first cuts are marks". On these
pools it is `-FX` that writes the damaged group again; `-F` is refused.

Every other place N4 named now says `-F` and else `-FX`:

- `publish_check.py:31-34`, `:76-79`, `:120` and `:202`;
- `cut-record.sh:31-32`, `:636-641` and `:1684-1690`;
- `test_cut_record.sh:478-480`;
- the workflow's comment.

`test_publish_check_main.py:162-167` stands in an `-F` that imported
(`imported with -F`), which is consistent with its own story.

## The things asked

### (1) prewindc's premise

**What it rests on.**

- the cut lines from c1's C up to c1's `withdraw` line, whatever the
  daemon answered there (`withdraw_line`, `publish_check.py:532`);
- the rig's mark `session-2-ready`, written after `zpool import`
  returned, on the log's clock;
- `published.tsv` for C.

It reads no answer of the daemon to c1.

**Does it tell a plain import from a rewind?** Yes.

- `grounds` covers every cut after C up to the withdrawal, the import's
  own cuts included.
- A rewind import writes a group at or below the top before it (rule
  (R), or R' for `-FX`), so it is a ground and the course ends 2.
- A plain import's groups go on above. `later` asks that cuts were
  counted after the import returned, so a course that made no cut there
  also ends 2.

**From the recording** (run 45588605464, prewindc):

- c1 was asked at 528.296 s with C 10 and G 1000021. The log reads
  `publish - asked 1000021 10 c1 accepted`.
- The export's cuts were 13 to 16 (groups 23, 24, 25, 34).
- The import's cuts were 17 (group 36) and 18 (38), above the top 34.
- After `session-2-ready` (531.504 s) the daemon counted 56 cuts, 19 to
  74 (groups 40 to 205).
- c1-end read `pending after 60 s`.
- At 591.197 s came `withdraw` and `ended c1 withdrawn`. The look read
  `absent`.
- Exit 0, and every course of the run ends 0.

**Replays through `publish_check.main`** on that run's recordings: 57 of
59 cases ended as they should. The two others are G1's.

- **Faults end 1.** In the S1 fault (c1 ended `refused rewound` at the
  import) the premise is ok and `pin_answer_c1-end` and
  `pin_answer_c1-withdraw` fail. That holds in three forms:
  - ended after cut 17;
  - the import's cuts 17 and 18 written as opening only as a rewind;
  - ended at the import's return, with no `ended` line.

  Also 1:
  - c1 ended as expired at the export;
  - ended `refused store`;
  - published on cut 20 despite G (`pin_c1_absent` also fails);
  - the withdrawal ignored;
  - the look answering a snap;
  - the watcher's VERDICT.
- **Wrong or malformed recordings end 2:**
  - a rewind (cut 17 of group 34 or of group 30, against the top 34);
  - a rewind with a right daemon that ended c1 at it (the premise
    overrides the pins' 1);
  - no cut once the import returned;
  - no withdraw line;
  - no `session-2-ready`;
  - no c1 in `published.tsv`;
  - no `watch.log`;
  - the watcher's ERROR;
  - a cut line that does not read.
- **Right recordings end 0:** all eight courses as run, and b76ab97's
  prewind and pplain through the new check.
- **No vacuous pass** other than G1's two. Plants C1, C2 and C15
  (premise without `not grounds`, without `later`, grounds over the
  whole log) are KILLED.

### (2) pin_watcher_reads

**What it pins.** No line of `watch.log` is a VERDICT (node B's `get`
of a cut at head ended 4: the bucket lacks what the commit needs). An
ERROR line (three failures in a row), or no `watch.log`, is the rig's
error. It is judged in every course.

**Planted with cut_watch.py's own lines** (a stand-in node B whose
`get` ends 4, or whose `row` fails three times), in each of the eight
courses on the CI recordings:

- VERDICT ends 1 in all eight;
- ERROR ends 2 in all eight.

Plants V1 to V5 (not read, VERDICT not judged, ERROR not an error, no
log taken as no verdict, the pin only in courses that publish) are
KILLED.

**`cut-record.sh:438-441`** ("Its verdict … goes to the check") now
holds for the publish rig. It already held for the cut-daemon rig
through `cut_daemon_check`'s upload lines.

### (3) The ast wiring tests

See G2. They catch every plant of the kinds asked: a dropped call, a
reordered call, a wrong argument. Dead code is caught only on a kernel
run, through EXPECTED. Four harmless refactors fail.

## Also checked

- **N3.** Fixed. `asked_line(name, after)` (`publish_check.py:522-529`)
  stops at the `asked` line whose sectors column is the first ask's C,
  and `record.rs` writes C there.
  - The run's prewind recording with the first ask's line dropped ends
    2; at b76ab97 it ended 0.
  - So does b76ab97's own prewind with that line dropped.
  - C swapped into the G column ends 2.
  - A daemon without rule (R) at the ask ends 1.
  - The stand-in's columns are load-bearing: with `log_text` writing
    `-` in G and C as before, 15 of the 43 main tests fail.
  - Plants A1 to A3 are KILLED.
- **N4.** See G4.
- **S1.** Done (prewindc), with the looks promised on #141: `f1-look`
  (prewindf) and `tw-look` (pkill, after the withdrawal and the commit
  past G, before tk). S11 to S13 pin them and are KILLED.
- **S2, S3 and #141's survivors** (N8, N11, N13, N19 to N21, N23) are
  not taken, as the description says. I agree that none lets a
  faulty daemon end 0. Each would tighten a premise's window, a
  boundary, the rig's own checks of `import_rewind_f`, or a label.
- **Tables both ways.** The Tables test asserts EXPECTED, PUBLISHED,
  ABSENT, LOOKS, BETWEEN, COUNTS and TW_STATUS by equality.
  - C8 to C14 KILLED: c1-end allowed `refused rewound`, c1-withdraw
    allowed another answer, c1 out of ABSENT, prewindc out of COURSES,
    the looks allowed a snap, c1-look out of the table.
  - S10 and S14 KILLED: prewindc out of `PUBLISH_NAMED` or the
    workflow's list.
- **The bench.**
  - B1 to B7 KILLED: the found-committed test dropped, a missing key
    read as found, a non-number read as none, read at the window's end,
    the clause naming the start's lease, the clause always "held", the
    paragraph without the condition.
  - #132's plants re-run at this head: N9, N10, N18 to N21, W7, O7,
    S19 and S19b now KILLED (they survived at #132's next look).
  - The seed rule's reason is exact, which I had doubted. The
    uploader counts `found_committed` only when a check finds that the
    row already names the cut (`crates/skyzfs-upload/src/driver.rs:557-606`).
    Within one instance, a commit that failed but landed is read again in
    the commit stage and counted as `uploaded`
    (`driver.rs:720-732`, `:772-781`). So a count above 0 at a window's
    start means an earlier instance, as `bracket.py:191-195` says.
    `crash.rs:129`'s `landed_unheard` is read after a restart.
- **Earlier pins, re-run at this head** (`*_at_head.out`):
  - **#139's 82 plants** (65 KILLED, 8 SURVIVED, 9 NOT APPLIED). As
    at #141's head, except Q9: #141 rewrote its text (B2: any answer
    to tw's withdrawal), so it is NOT APPLIED. It is absent at b76ab97
    and at the base.
  - **#132's 42 plants** (33 KILLED, 1 SURVIVED, R7 as before, 8 NOT
    APPLIED). Those NOT APPLIED (N1, N6, N7, N14 to N16, R10, R12) are
    so at the base as well (`na_check.out`): later pull requests
    rewrote that text.
  - **#141's next-look plants.** The seven this pull request's text
    displaced (L4, R4x, R13x, R25, R46, N24 and testing's P1) were
    aimed again at the new text (`reaim143.out`), and all seven are
    KILLED.
  - The rest: see "#141's plants" below.
- **Local suites at d3b75af.** All pass:
  - all 22 `tests/kernel/test_*.py` and `tests/bench/test_*.py`;
  - `test_cut_record.sh` and `test_copy_limit.sh`;
  - shellcheck 0.11.0 over `git ls-files '*.sh'`: clean.

### #141's plants

`p141_at_head.out` holds #141's next-look plants
(`review/m2.t7b/plants_next.py`), all 91 of them, re-run at d3b75af.
The result is 71 KILLED, 13 SURVIVED and 7 NOT APPLIED.

- **SURVIVED.** The 13 are the same as at b76ab97: R7x, R26, R32, R40,
  R42, N2, N8, N11, N13, N19, N20, N21 and N23.
- **NOT APPLIED.** The 7 are exactly the plants this pull request's
  text displaced: L4, R4x, R13x, R25, R46, N24 and P1. Aimed again,
  each is KILLED (`reaim143.out`).
- Every other plant has the result it had at b76ab97.

## CI at d3b75af (read only)

| workflow | run | state |
|---|---|---|
| skyzfs-publish-rig | 45588605464 | success (unit job; ubuntu-26.04, all eight courses exit 0) |
| skyzfs-cut-daemon | 45588605497 | success (26.04, 26.04-arm, 26.04-upload) |
| skyzfs-nodeb-rig | 45588605719 | success |
| skyzfs-kernel-probe | 45588605702 | success |
| ci / coverage / e2e | 45588605491 / 45588605588 / 45588605459 | success |
| skyzfs-cut-rig | 45588605705 | success (the reference; 24.04, 24.04-arm, 26.04, 26.04-arm) |
| skyzfs-replay-rig | 45588605461 | cancelled |
| skyzfs-daemon-rig | 45588605676 | cancelled |

**The cancelled runs.** The claim holds: nothing those two rigs run
changed.

- Both workflows trigger on `tests/kernel/**`.
- They run `daemon-rig.sh`, `daemon_check.py`, `replay*.py`,
  `skyzfsd-ref.sh`, `hole-rig.sh` and their own `test_*.sh`.
- None of these reads `cut-record.sh`, `publish_check.py`,
  `cut_check.py` or `nodeb_check.py`, or the tests changed here.

The push runs on the same head were cancelled when the pull request
opened.

**Codex.** Its review of d3b75af failed: the account had reached its
usage limit. There are no Codex findings to answer.

## Decisions for the lead

None needed for the merge. One would be the lead's if wanted:

- G1: whether prewindc's premise should also bind c1's ask before the
  import and its expiry. Today the course's order test pins both.

## Files here

- `plants143.py`, `plants143.out`, `plants143.json`: this review's 69
  plants (A, V, C, S, W, H, B). 59 KILLED (H1 to H4 among them, which
  are false failures) and 10 SURVIVED (V6, W14 to W21, H5).
- `plants143_whb.out`: W, H and B run again in a separate worktree; the
  results are the same.
- `reaim143.py`, `reaim143.out`: #141's plants displaced by this pull
  request's text, aimed again.
- `replay143.py`, `replay143.out`: the replays through
  `publish_check.main` on run 45588605464's recordings (and
  45569275289's).
- `stub_nodeb.py`, `watch_verdict.log`, `watch_error.log`: the
  stand-in node B, and the lines cut_watch.py wrote with it.
- `n3_catch.py`, `n3_catch.out`: the stand-in log's C column is
  load-bearing.
- `na_check.py`, `na_check.out`: whether earlier plants NOT APPLIED
  here were already so at the base.
- `p139_at_head.out`, `p132_at_head.out`, `p141_at_head.out`: #139's,
  #132's and #141's plants re-run at d3b75af. Their scripts are in
  `review/m2.t7/`, `review/m2.t5b/` and `review/m2.t7b/`.
