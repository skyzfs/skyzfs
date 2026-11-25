# Next look at #141 (M2.T7b): the publish rig after M2.6a, at b76ab97

Head reviewed: **b76ab97** (`gh api repos/skyzfs/skyzfs/pulls/141`), on
`zfs-topology` at 501ccf7 (the merge base is still the base's tip). Two
commits since my review at e0c51f8: 6647d4b (B1) and b76ab97 (B2, D-c,
N1). Only the publish rig's files change (`tests/kernel/cut-record.sh`,
`publish_agent.py`, `publish_check.py` and their three tests); no
product code, no workflow.

I formed my view from `git diff e0c51f8...b76ab97`, the daemon's request
log (`bins/skyzfsd/src/record.rs:128-144`, `serve.rs:1247-1252`), the
store's rule (`crates/skyzfs-local/src/inplace/cuts.rs`) and the rigs'
recordings, then read testing's answer and CI summary and checked their
claims. File:line references are at b76ab97.

## Verdict

**Can merge. No finding blocks.** B1 and B2 are settled, and so are
D-a, D-b and D-c as the lead ruled them.

- prewindf now runs on the kernel: run 45569275289 took it past the
  import (`-F` refused, `-FX` imported) to exit 0, with real values
  below.
- No premise reads the daemon's answer to the request it judges. Every
  fault I replayed through `publish_check.main` on the run's own
  recordings ends 1, the right recordings 0, and wrong or malformed
  recordings 2, with one exception (N3, a corner that needs a daemon
  whose request log lacks a line it always writes).
- Two small defects (N3, N4) and three suggestions, none of which
  blocks.

## The things asked

### (1) What the premises rest on (D-b)

Every premise of `publish_check.py` at b76ab97, and what it reads:

| premise | reads | the daemon's answer to the judged request? |
|---|---|---|
| `premise_t4_cuts_below_g` | cut lines | no |
| `premise_tw_committed_past_g` | the place of tw's `withdraw` line, whatever it answered (`:465`); cut lines; node B's row | no |
| `premise_r1_asked_after_a_rewind_cut` | cut lines up to r1's first `asked` line, whatever it answered (`:496-499`, `:722`) | no (but see N3) |
| `premise_held_at_rewind` | the daemon's status `cuts_held` | no: a status, unchanged since #139 |
| `premise_rewind_cut_committed` | node B's row, cut lines | no |
| `premise_f1_import_f` | the rig's `rewind-f.txt` (whole lines) | no |
| `premise_f1_waited_across_a_rewind_cut` | cut lines after f1's C, and agent.tsv's send of f1 (`:755-763`) | no |
| `premise_{m1,p1}_killed_by_the_hook`, `premise_p1_landed` | kill.tsv, the hook's words, node B | no |

So the shape of #139's D1, and of my B2, is gone: a daemon that does not
refuse, end or withdraw as the course asks fails a pin.

**`premise_f1_waited_across_a_rewind_cut`, from the recording.** Run
45569275289, prewindf:
- f1 was sent at 336.023 s on the monotonic clock, with C 10 and
  G 1000021, `--expiry-s 900`.
- The export's cuts were 13 to 16, of groups 23, 24, 25 and 34.
- After the damage, cut 17 of group 34 (equal to the top, 34) was
  counted at 340.414 s: the ground, 4.39 s after the send.
- Cuts 18 (group 35) and 19 (37) followed, then `publish ended f1
  refused rewound` at 340.458 s, 44 ms after cut 17, and then the
  import returned (`session-2-ready`).

**Replays** (`replay_next.py`, `replay_next.out`): the head's
`publish_check.main` on run 45569275289's prewindf, prewind, pkill and
pplain files, downloaded into new empty directories and read with
`python3 -I`, node B stood in for. 34 of 35 cases end as they should.
- **prewindf, each a fault, each ends 1** with the premise ok:
  - a daemon that never ends f1 (rule (R) not at the equal group, or not
    at all: no `ended` line, `f1-end` pending);
  - f1 ended `expired`, or `refused store`;
  - f1 ended `refused rewound` at the export, or 1 ns before cut 17;
  - f1 published on cut 18 and `done` (also `pin_f1_absent`);
  - the daemon's `ended` line missing while its answer is right
    (`pin_f1_ended_after_the_rewind_cut`: "the request log never ends
    f1").
- **prewind, each a fault, each ends 1** with the premise ok:
  - a daemon without rule (R) at the ask: r1 `accepted`, pending, and
    its log line `r1 accepted`;
  - the same daemon, r1 also made by rewind-only cut 23 once the store
    came back;
  - r1 refused with another code (`refused asked`);
  - r1 made by cut 23 (group 33), 22 (32) or the top's 27 (43);
  - r1-look `absent` with the store down.
- **pkill:** the withdrawal answered and logged `pending` ends 1 with
  the premise ok.
- **Wrong or malformed recordings, each ends 2:**
  - prewindf: cut 17 dropped (no ground); f1's C after the ground; f1's
    expiry 4 s (it expired before the ground); `rewind-f.txt` without
    its import line; agent.tsv without f1's publish, or with an expiry
    that is not a number; a cut line that does not read;
  - prewind: `rewind-top.txt` not a number; r1's ask before the `-T`
    import's cuts;
  - pkill: the daemon's `withdraw` line missing;
  - e0c51f8's prewindf recording (it stops at the refused `-F`): "the
    course left no answer for f1-end".
- **Right recordings end 0:** prewindf, prewind, pkill and pplain as
  run; prewindf with f1's expiry 5 s (the ground came 4.39 s after the
  send, so the premise holds by the real times) or with no
  `--expiry-s` (600 s); and e0c51f8's prewind recording through the new
  check.
- **The exception: prewind with the daemon's `asked` line of r1's first
  ask missing ends 0, not 2** (N3).

**The pull request's driver** (`scenarios.py` re-run, `scenarios_next.out`):
each of the five rows that ended 2 at e0c51f8 because of B2 (f1 never
ended, the mark strictly below the top, f1 ended `expired`, M2.5 at the
ask, r1 refused with another code) now ends 1 with the realistic log.
The right recordings end 0, and #139's D1 rows still end 1.

**agent.tsv's expiry** (`ask_deadline`, `publish_check.py:502-519`):
- *Missing:* no publish of f1 is the rig's error (exit 2; a test pins
  it, and so does my replay).
- *Zero:* the deadline is the send itself, so no ground is within it:
  exit 2 (the PR's test). The daemon would refuse it anyway
  (`control.rs:170`, and the command line takes 1 to 3600).
- *Unit:* seconds to nanoseconds, against the request log's
  `CLOCK_MONOTONIC` (`record.rs:230-232`); the agent's clock is
  `time.monotonic_ns`, the same clock on Linux. The daemon's deadline is
  `Instant::now()` at acceptance plus the expiry
  (`skyzfs-upload/src/publish.rs:700-702`), later than the agent's send,
  so the check's deadline is never later than the daemon's.
- *Per instance:* the deadline is of the first `publish --name f1` in
  the course's own agent.tsv. The daemon keeps a publish in memory, so
  an expiry across instances does not arise; prewindf has one instance
  (`obs_cuts`: 23 cut lines in 1 instance).
- *Fragile corners, none reachable from the agent:* `--expiry-s` as the
  last word reads as the default; the default 600 duplicates
  `EXPIRY_DEFAULT` and nothing ties them (S3). prewindf always passes
  900, which `test_cut_record.sh` now pins.
- The premise bounds the cut, not the end: a ground just inside the
  expiry could let a right daemon end f1 `expired` before the uploader's
  turn (44 ms here). With 900 s against a course of 5 s, that does not
  arise.

### (2) `import_rewind_f` and rewindf

- **Moved unchanged.** e0c51f8's `rec_rewindf` diffed against
  `import_rewind_f` plus the new `rec_rewindf` differs only at the
  function's boundary and its comment (`cut-record.sh:655-689`).
- **The cut rig** at e0c51f8 (run 45562017961) and b76ab97 (run
  45569275505), rewindf on each of the four images: the same 11 result
  keys with the same verdicts. Only the details move (record numbers);
  `ctl_later` is CAUGHT on 24.04 and 26.04-arm and NOT_BUILT on 26.04 and
  24.04-arm at both heads, as its table allows. Both import with `-FX`.
- **The cut rig through skyzfsd** (26.04) at e0c51f8 (run 45562017928)
  and b76ab97 (run 45569275288): the same 31 keys and verdicts, the same
  29 cut lines (export's last cut 18 of group 43, the import's first cut
  19 of group 43 again, opening as itself), `imported with -FX` and
  "groups committed after the import: 45 46 47" at both. The 26.04-arm
  and upload jobs are green.
- **prewindf on the kernel** (run 45569275289, ubuntu-26.04): `zero_newest_root`
  zeroed 3 copies of group 34's root block. `-F` was refused ("one or
  more devices is currently unavailable"; the kernel: `spa_load_retry:
  rewind, max txg: 33`, best uberblock 25, "label discarded as txg is
  too large (34 > 25)"), and `-FX` imported. It then pinned:
  - `pin_answer_f1` (`pending after 3 s`);
  - `pin_answer_f1-end` (`refused rewound a rewind may have been
    counted after the count asked from`);
  - `pin_f1_absent`;
  - `pin_f1_ended_after_the_rewind_cut`: "f1 ended `refused rewound`
    after cut 17 of group 34 … (the top before it 34)".

  The premises held: the import line `imported with -FX`, and the ground
  of cut 17 within f1's expiry.

### (3) D-c: does the documentation overclaim?

No.
- `publish_check.py:76-83`, `cut-record.sh:1677-1690` and the
  description each say that the course pins only that the mark ends a
  waiting publish, and that `pin_f1_absent` cannot fail for a daemon
  that honours G. That is right.
- One softness, not an overclaim: "the mark ends" reads as cause. The
  course shows that f1 is ended `refused rewound` after a cut that (R)
  marks. It has no control in which an import that is not a rewind
  leaves a waiting publish waiting. The unit tests
  `a_rewind_ends_the_publishes_that_wait_from_before_it` and
  `a_publish_given_its_cut_before_a_rewind_goes_on` cover the rule
  (`skyzfs-upload/src/publish/tests.rs:226-276`); S1 is a cheap control
  on the kernel.
- Four comments still say `-F` where the import is `-FX` (N4).

### (4) Test gaps, and the answers to N1 and N2

- **R41 (expiry): taken, and the reason holds.**
  - `test_cut_record.sh` pins `--expiry-s 900`, so plant R41 is killed.
  - A course whose f1 expired before the ground is now the rig's
    error: the PR's test uses expiry 0, and my replay at 4 s.
- **The workflow's default list: taken.** It is pinned against
  `PUBLISH_NAMED` (plant N24 is killed).
- **R5, R7, R26, R32, R40 and R42 left: agreed.**
  - R5's code is gone with `rewound_grounds`.
  - R7 survives again as R7x: prewind and prewindf have one instance.
  - The others are as judged at e0c51f8.
- **The looks at f1 after its end and at tw after its withdrawal: left
  for the bench and test-gap package.** Agreed: they were suggestions.
- **N1: fixed.**
  - `publish_agent.py:46-53` says that a time-out once connected is the
    answer `no answer within N s`, and exit 2 only an unreachable
    socket.
  - `publish_check.py:98-99` says "an earlier one that opens as
    itself".
- **N2: not changed, and the reason holds.** N2 was a reading, not a
  defect: a daemon that answers and does not close is judged on its
  answer, after the limit, never passed for something it did not say.
  Whether skyzfsd's own `ask` waits for the close is the daemon's to
  pin.

### (5) Plants at b76ab97

PLANTS_SUMMARY

## Findings, most severe first

Defects first, then suggestions. **None blocks the merge.**

### N3 (does not block). prewind's anchor is r1's first `asked` line in the log, not the first ask's

**Where.** `publish_check.py:496-499` (`asked_line`), used at `:722`.

**What happens.** `asked_line('r1')` stops at the first `publish asked`
line of r1 in the request log, whatever its count. When the line of the
first ask (`asked 31 20 r1 …`) is missing, the stop falls on the ask
anew's line (`asked 31 27 r1 accepted`). The ground is then cuts 21 to
26, and the premise holds. Replayed on run 45569275289's prewind, the
recording with that one line removed ends **0**, where the docstring
(`:100-104`) promises a ground "ahead of the line of r1's first ask".
pkill, with its `withdraw` line removed, ends 2, as it should.

**Why it does not block.** It needs a daemon whose request log lacks a
line it writes for every publish the uploader answers
(`record.rs:136-144`; only a request the channel refuses before the
uploader sees it has none). And it needs the course to be wrong as well
(r1 asked before the `-T` import's cuts) before a verdict would be
misplaced. With the right daemon's log the anchor is right: run
45569275289's ground is cuts 21 (group 31) and 22 (group 32), ahead of
`asked 31 20 r1 refused rewound`.

**Certainty: certain** of the behaviour, low impact.

**Fix (two lines).** Stop at the line of the first ask's own count:
the request log's `asked` line carries C in its sectors column
(`record.rs:285`). That is `asked_line('r1', r1['after'])` with `and
ln['sectors'] == str(after)`. With it my 35 replays all end as they
should. The stand-in log of `test_publish_check_main.py` then has to
write C in its `asked` lines as the daemon does: as it stands, 15 of
its tests read `-` there and fail with the change. Or take the anchor
from agent.tsv's send of the first `publish --name r1`, the rig's own
record, as prewindf now does for its expiry.

### N4 (does not block). `-F` where the import is `-FX`, and a comment from before B2

- `cut-record.sh:633-635` (`zero_newest_root`): "so that … `zpool
  import -F` goes back past it". On this pool shape `-F` is refused, and
  `-FX` writes the damaged group's number again.
- `publish_check.py:187`: "A rewind by `zpool import -F` while f1
  waits".
- `.github/workflows/skyzfs-publish-rig.yml:23`: "prewindf (a `zpool
  import -F` while a publish waits".
- `test_cut_record.sh:477-478`: "imported with -F".
- `publish_check.py:715-716`: "The premise is of the first ask, refused
  `rewound`". Since B2 it is of the first ask, whatever it was answered.

`rec_prewindf`'s comment and the check's docstring (`:76-83`, `:114-120`)
are right. **Certainty: certain; wording only.**

### Suggestions and test gaps

- **S1. A control for the cause of f1's end.**
  - The same course without the damage: export, then a plain import
    while f1 waits. f1 must still be pending at the end (or `expired` at
    its own deadline), and no cut is a ground.
  - It would show on the kernel that the end comes from the mark, not
    from the import as such. A daemon that ends every waiting publish
    `refused rewound` at any import passes prewindf today.
  - About one minute of a run.
- **S2. Tie the ground to the damage.** Count ground cuts only after the
  rig's own `damage` mark (marks.tsv, the same clock). Then a stale cut
  that errs low before the export (which (R) marks, conservatively)
  could not stand in for the rewind's cut. In run 45569275289 the
  only ground is cut 17, after the mark, so nothing changes for it.
- **S3. `ask_deadline`'s default.** The 600 s duplicates
  `EXPIRY_DEFAULT` with nothing to tie them, and `--expiry-s` as the
  last word reads as that default rather than as the rig's error.
  Neither is reachable from the agent, and prewindf passes 900.
- **Plant survivors**: PLANTS_SURVIVORS

## What the pull request settles

- **B1: settled (6647d4b).**
  - prewindf imports through `import_rewind_f`, `-F` then `-FX`. That is
    rewindf's loop moved unchanged, and rewindf calls it too.
  - `premise_f1_import_f` takes the whole line `imported with -F` or
    `imported with -FX` (`:752-754`), and a test pins a line that only
    contains the words. Plant M3 (the substring) is killed.
  - The kernel run reached the check (above).
- **B2: settled (b76ab97).**
  - No premise reads the daemon's answer to the request it judges
    (table above). Every fault replayed ends 1.
  - The tests now use the log as a faulty daemon writes it
    (`test_publish_check_main.py:377-390`, `:455-475`).
  - `judge_ended` is a `server` pin: f1's first `ended` line must be
    `refused rewound` and later than the ground's cut.
- **D-a: done**, as rewindf. The docs say `-F` or `-FX`, apart from N4.
- **D-b: done.** `publish_check.py:85-92` states the rule.
- **D-c: done** (above).
- **N1: fixed. N2: answered** (above).

## Decisions for the lead

None needed.

One reading to confirm. D-b as the docs state it lets a premise read
"the place of a request" in the request log, so the daemon's own
`asked` and `withdraw` lines (whatever they say) count as the rig's
ground. When such a line is missing, pkill ends 2, which is right. For
prewind, the anchor can fall on a later line (N3). Taking the place from
agent.tsv instead would make both independent of the daemon's log. I
recommend N3's two-line fix and leaving pkill as it is.

## CI at b76ab97 (read through the API)

All runs have finished.
- **Green:**
  - `skyzfs-publish-rig` (run 45569275289): all seven courses exit 0,
    and its unit job is green;
  - `skyzfs-cut-rig` (run 45569275505, four images);
  - `skyzfs-cut-daemon` (run 45569275288: 26.04, 26.04-arm, upload);
  - `skyzfs-nodeb-rig` (45569275555), `skyzfs-kernel-probe`
    (45569275323);
  - `ci` (45569275242), `coverage` (45569275341), `e2e` (45569275337).
- **Cancelled:** `skyzfs-daemon-rig` (45569275342) and
  `skyzfs-replay-rig` (45569275268), and the push runs of the same head.
  - testing's reason holds. Neither workflow nor anything it runs reads a
    file changed since e0c51f8: `git grep` finds `publish_check`,
    `publish_agent` and `cut-record.sh` only in the cut, cut-daemon,
    node B and publish workflows and their own files.
  - Their verdicts stand at e0c51f8: daemon rig 45562018042 and replay
    rig 45562018051, both green.
- **Codex:** completed at b76ab97 with no findings (its summary
  comment's state: `has_any_findings: false`).

testing's CI summary is accurate on every figure I checked:
- prewindf: C 10, G 1000021, `pending after 3 s`; three copies of group
  34's root zeroed; `-F` refused, `-FX` imported; cut 17 of group 34
  equal to the top; the `ended` line after it.
- prewind's ground: cuts 21 (group 31) and 22 (group 32) after C 20,
  with the top at 43.
- The looks: t1-look at cut 10 of group 18; t5 and t3 `absent`; r1-look
  `refused store`.

## How it was checked

- **Local gates at b76ab97, Python 3.13:**
  - `test_publish_check.py`: 34 OK.
  - `test_publish_check_main.py`: 37 OK.
  - `test_publish_agent.py`: 22 OK.
  - `test_cut_record.sh`: all passed.
  - `unittest discover` in `tests/kernel`: 628 OK (624 at e0c51f8).
  - shellcheck 0.9.0 (older than CI's) on `cut-record.sh` and
    `test_cut_record.sh`: the same codes at e0c51f8 and b76ab97 (SC2015,
    SC2002 and SC2317 infos); no new code. CI's lint is green.
- **Replays** (`replay_next.py`, `replay_next.out`): above. The
  artifacts of runs 45569275289, 45562017962, 45569275505, 45562017961,
  45569275288 and 45562017928 were each downloaded into a new empty
  directory and read only as data (`python3 -I`, paths as arguments).
- **Driver scenarios** (`scenarios.py` re-run on b76ab97,
  `scenarios_next.out`).
- **Plants** (`plants_next.py`, `plants_next.out`, `.json`): one at a
  time against the four suites, 600 s each, every file put back in a
  `finally`; the suites passed before and after.
- **N3's fix** was tried on a scratch copy of `tests/kernel`: the 35
  replays all end as they should, and the stand-in log's tests need C
  in their `asked` lines.
- **No kernel here.** Daemon-side behaviour comes from the recordings.
