# Review of skyzfs/skyzfs#171 (M2.T5: the agent rig's run course, R-rewind, recorder follow-ups)

Heads: started at 7ab6c65, judged at **4ccc4b4** (one commit on 7ab6c65:
the round line's fields after `under_way` read by name). Base
`zfs-topology` at 6868430. Diff: `git diff origin/zfs-topology...pr171`
and `git diff 7ab6c65 4ccc4b4`. The agent (`run`, `Agent::sync_past`)
and design §5.2, §5.4 and §9 were read before the description.

## Verdict

Not ready to merge at 4ccc4b4. One finding blocks (B1): the run course
can give exit 1 against a correct agent, and the R-rewind run on this
head was about four seconds away from doing so. Codex's P2 on 7ab6c65
(agent-record.sh:678) is the same finding and has no answer on the pull
request. M1 is a test gap that I would fix in this pull request, since
the fix is small and the data it needs is already in the artifact. The
other findings do not block.

What the work package adds is sound: the by-name parsing in 4ccc4b4 is
right, 4043629 answers my #169 next-look lows, every local gate passes,
and the agent rig is green on all six jobs at 4ccc4b4. The rewind
premise cannot pass vacuously.

## Findings, most severe first

### B1 (blocking; a wrong verdict in the rig). SIGTERM can land in a round, and a correct run is then given exit 1

`run_rounds_then_term` (agent-record.sh:675-684) sends SIGTERM as soon as
the N-th round line arrives. Its comment says this happens "between two
rounds, while it waits for the next tick", but nothing makes sure of
that. `schedule` (program.rs, `tick += passed.max(1)`) starts the next
round at once after a round that overran its tick, and `go()` is only
asked at the top of the loop. A SIGTERM that comes after that check lets
the round go on: `sync_past` (which sets `clear` without asking `go()`
when it synced nothing), the begin, then a settle that ends at once. The
round's line is then printed. `await_rounds` stopped stamping arrivals at
N, so `judge_rounds` (agent_check.py:610-612) reports "the rig saw N of
N+1 round lines come" and the case FAILS (exit 1).

The same race affects R-rewind's own pin. If round 1's publish is still
under way when round 2 reaches its tick, round 2 overruns and its
`snap/` line is never printed. `judge_synced` (agent_check.py:686-687)
then says "round 1's publish ... was not printed published".

In run 45918311854 (job r, ubuntu-26.04, 4ccc4b4; timeline in
`rewind_timeline-45918311854.out`):

- Round 1 took 10488 ms against a 10 s tick, so round 2 began at once
  (`late_ms=488`).
- Round 1's publish was refused `rewound` three times (rule (R); see O1)
  and was published by cut 83 (group 96) at 200.04 s. That cut came
  5.1 s after cut 82 (group 95), at the pool's own pace of one group
  every txg timeout.
- Round 2 ended 0.7 s after that publish completed, 3.2 s before its next
  tick at about 204.2 s.

One more group needed above the ring's wrap point (O1) would have pushed
round 2 past its tick and made this case FAIL. The case passed with less
than one group's time to spare.

The fix is in the rig:

- Wait for a round line with at least about 3 s left before its next
  tick, as `scen_runbad` already does (agent-record.sh:768-770), or keep
  stamping arrivals until the run has ended.
- In R-rewind, wait (with a bound) for round 1's `snap/` line before
  counting the two rounds.

Certainty: high on the mechanism, which I traced through the code; the
run's timings show the margin. It was not reproduced, because it needs a
kernel run.

### M1 (medium; a test gap). `pin_R-rewind_synced_past` does not show that the begin came after the pool had passed the abandoned groups

`judge_synced` (agent_check.py:671-694) asks for four things:

- round 1 printed `synced=` above 0;
- round 1's publish was printed published;
- no later round synced;
- no `no publish begun` line.

It does not ask that the newest cut opened as itself before round 1
began its publish, which is the lead's M2.7d decision ("`run` begins no
publish while the newest cut is behind").

Two regressions would still pass this pin as long as the publish lands
within two rounds:

- a `sync_past` that returns `Clear` after its first sync;
- a `run` that begins on `Behind` without printing the line.

Either way the publish waits as pending while the planner and the writer
carry the pool past the abandoned groups.

The artifact already has what a real check needs, with no clock:

- The first `publish asked` line of round 1's name carries the count
  that the begin read before the take (here 77).
- The `cut` line with that number says whether it opened as itself
  (here cut 77, group 88, `itself`).

So the pin could require that the cut at the count of the first ask
opens as itself. `last_ask` (agent_check.py:516) already reads these
lines; it keeps the last accepted ask, and this check needs the first
ask, accepted or refused. A cross-check on
time:

- The case's `trace.tsv` puts the intent at 184.368 s.
- That falls between cut 77 (184.331 s) and cut 78 (group 91, the
  snapshot's createtxg, 184.437 s).

Do not use "createtxg above the top before the export" as the check. In
this run createtxg was 91, the top 95, and the agent was right.

Certainty: high. The gap can be read from the code; it was not planted,
because the regressions are in product code and show only through a
kernel.

### L1 (low; documentation). "The daemon holds every cut from then on" is not so, and R-rewind does not show the criterion's "with cuts of the abandoned history still held"

The comment at agent-record.sh:789-790 and the description's "After a
rewind import" paragraph say the freeze makes the daemon hold every cut.
That is not so. By D7 the daemon holds at most two cuts: the one being
uploaded and the newest.

- **The cut being uploaded** at the freeze is from the shared history
  before the rewind. The freeze came at 181.977 s, before cut 52
  (group 58) and the rewind group 61.
- **The cut that waits** at the export was cut 62 (group 95), the top of
  the abandoned history. Cut 63 (group 62, the new history) replaced it
  at the import, before `run` started.

So at the publish no cut of the abandoned history was held. That is not
a defect of the work package, which claims only M2.7d. But the lead
should not read R-rewind as evidence for §9's "published after a rewind
import, with cuts of the abandoned history still held". The notes give
that clause to M2.T3's last stage. Doing it here would take a freeze
while an abandoned cut is the one being uploaded.

What R-rewind does show, without pinning it, is the clause's other half
from §5.2: "with the pool's groups still below the abandoned ones". The
publish was asked at groups 91 to 95, refused `rewound` three times, and
published by the first cut above 95, and node B verified it. This
deserves an INFO line, such as the count of `refused rewound` for the
case's names.

### L2 (low). The premise is read before the run's own first `cuts`

`rewind-cuts.txt` is read after the thaw. With the writer off nothing
should advance the pool before `run` reads `cuts` itself. If something
did, round 1 would print `synced=0` and get FAILED, not ERROR. That is
unlikely, and it was not seen.

### Nits

- The workflow header (skyzfs-agent-rig.yml, part r) lists M2.7a's
  cases but not R-rewind (M2.7d).
- 4ccc4b4: a field name after `under_way` with a digit or a capital
  (`bound2=`, `Bound=`), an empty value, a double space or a trailing
  space makes the whole line "not one of its five", a rig error (exit 2).
  That is stricter than "unknown fields are left". It is fine as long as
  the agent's fields stay `[a-z_]` names; say so in `round_fields`'
  docstring.
- 4043629 (carried from the #169 look): `ZCR_SETTLE_S=0000000030` is
  budgeted at 999999 s while the settle takes 30 s. This is harmless.

## Observation for the lead (O1): where `sync_past` stops and where rule (R) lets a publish through are two different groups

In run 45918311854:

- The pool was rewound from 95 to 61.
- The first cut that opened as itself came at group **87** (cut 76),
  after six syncs in 80 ms.
- The daemon refused `rewound` until the first cut above **95**, which
  came 15.7 s later.

The first cut came early because of the ring. The ring has 32 slots;
the run fits 4 KiB slots exactly. The abandoned top 95 sat in slot 31,
which the new history's group 63 overwrote. Groups 87 to 94 of the
abandoned history wrote nothing, so they left no uberblock. From then on
the highest abandoned uberblock in view was 86. The daemon followed the
ring correctly, and 87 opened as itself.

Rule (R)'s mark is the highest group seen (95), and it does not come
down when that group's slot is overwritten. So:

- `sync_past` stops at "opens as itself", which is the design's
  condition and is met at 87;
- the rest of the gap is closed at about one group per txg timeout (or
  by the next round's take);
- the first publish after the rewind waited 15.7 s, with three refusals.

This is not a defect: the design says the mark errs high and only
delays. But two places in the text are not exact for 4 KiB slots:

- §5.2's "no cut opens as itself until the pool's groups have passed the
  abandoned ones". It holds for the abandoned uberblocks still in the
  rings, and here these were fewer than the abandoned groups.
- The note "closes the window in seconds".

This is also what makes B1's margin thin.

Decisions for the lead:

- whether `sync_past` should go on until it passes the mark (the `cuts`
  reply would have to carry it) or the delay is accepted;
- whether the design's sentence should name the ring.

## The two asked-about commits

**4ccc4b4 (round line by name).** The fields after `under_way` are
parsed by name, a regular expression of `name=value` pairs, then
`round_fields`. `fields_probe.py` (`fields_probe.out`) checked these
lines:

| Line | Result |
|---|---|
| No `bound=` (a line from before #172) | read, `synced` read |
| #172's trailing `bound=0` or `bound=1` | read |
| `bound=` before `synced=` | read |
| An unknown field | read and left |
| A name that has `synced` as its prefix (`syncedx=`, `synced_total=`, either order) | not taken for `synced` |
| A field twice | RigError |
| `synced=-1` | RigError |
| #172's real line | parses, and `judge_synced` is satisfied |

Five plants were checked against this code, and the tests kill each
one:

| Plant | Description |
|---|---|
| A12 | the parsing reverted to positional |
| A13 | a duplicate field accepted |
| A14 | only the first field read |
| A15 | `synced` never read |
| A16 | `synced` not checked to be a count |

The rigs restarted on 4ccc4b4: agent rig run 45918311854 (pull_request),
the push run cancelled as superseded. My judgement of the rigs is
against that run.

**4043629 (my #169 lows).**

| Low | How it is answered |
|---|---|
| D4 | Rates per round over each side's own window, with medians. The read's window now includes its record's own time, and its busy share is reported. |
| P15 | The writer's default margin is pinned (Z09 and Z10 killed). |
| P18 | The K04 fake lists `@c` and `release_quietly` is anchored (Z11 and Z12 killed). |
| Q04 | `end_daemon`'s pid is cleared before `rig_error` (plant Q04 killed). |

Rounds cut at their time limit are reported as `cut` on every side (Z03,
Z04 and Z13 killed). No recorder run has been dispatched on these
commits, so there are no D4 figures from a kernel yet. The last
dispatch, 45895208967, was at af0c183.

## Asked by the author

1. **Is the premise real, and can the pin pass vacuously?** The premise
   is real; its second half (`opens=rewind` after the import) carries
   it. The first half (top above the import's group) is close to always
   true after an export.
   - No cut before or after, or a numbering that did not continue below
     the top (opens itself), both give ERROR (A01-A03, A10 killed).
   - The pin is gated on the premise (A09 killed), so it cannot pass
     vacuously.
   - Without a sync, a publish or `synced=` it fails (A04-A08, A11
     killed).
   - The gap is M1: the pin does not show the begin came after the pool
     had passed the abandoned groups.
2. **Does the freeze, export and import hold the abandoned cuts?** No:
   see L1.
   - The situation "the pool's newest uberblock is older than the
     daemon's newest held cut" does occur: top 95, then cuts at 62 to 84
     open as `rewind`.
   - Node B judged the publish right: the row is cut 83, group 96,
     createtxg 91, opens itself; refs, content, read-only import,
     snapshot listed, 16 files as the source held them.
   - If the import changed the numbering, or the daemon had no cut, the
     result would be ERROR, not FAILED.
3. **Can a verdict be wrong?**
   - Yes: B1. A correct agent can be given exit 1 when a round overruns
     its tick at the SIGTERM.
   - The exit mapping is consistent: premise means ERROR (2), holds
     means FAILED (1), any FAILED means 1.
   - No NA hides a failure in this course.
   - The controls catch what they claim: `ctl_early`, `ctl_files` and
     `ctl_manifest` are CAUGHT in job r.

## Gates at 4ccc4b4 (local; `gates.out`)

- **Python suites:** every one passes with `python3 -I`: agent_check 106
  tests, publish_check 48, publish_check_main 55, nodeb_check 29,
  txg_probe 20, dsync_writer 15, zfs_cli_json 7, cut_check 42 and
  cut_daemon_check 104.
- **Shell suites:** `test_zfs_cli_record.sh` 732 ok with 0 FAIL;
  `test_cut_record.sh` and `test_lim.sh` all passed.
- **`test_agent_record.sh`:** run 1 hit the known local temp-dir flake
  (recover and reserve checks); runs 2 and 3 passed.
- **shellcheck against the base:** no new finding in the rig scripts.
  The tests have only new SC2317 notes (stubs called indirectly), the
  same kind the base has.
- **YAML:** both workflows parse.

## Plants (`plants.py`, `plants.out`)

Each plant was applied alone to a worktree at 4ccc4b4, with its suite
run under 600 s and the file restored. The results are in `plants.out`;
the summary is below.

PLANTS_SUMMARY

## What the rigs show at 4ccc4b4

Agent rig run 45918311854: all six jobs succeeded, each with worst exit
0 and no FAILED or ERROR line.

| Job | Duration |
|---|---|
| r | 3 m 20 s |
| a1 | 2 m 37 s |
| a2 | 2 m 56 s |
| d | 2 m 28 s |
| k | 21 m 35 s |

**What job r shows**, on one runner (ubuntu-26.04) in one run:

- the seven run cases ok;
- R-rewind's premise holds: imported at 61, top cut 95; after the
  import the newest cut is 65 and opens as a rewind;
- round 1 synced 6 times and began a publish that node B verified;
- two rounds;
- the run ended 0.26 s after SIGTERM;
- every published name verified on node B (row, manifest, node B);
- the three controls are CAUGHT.

**What it does not show:**

- that the begin came after the pass (M1);
- a held abandoned cut (L1);
- any margin against B1's race.

Other runs at the head:

- Green: cut-daemon, nodeb, publish, ci and e2e, and skyzfs-ci on the
  push.
- Still running when I looked: the cut rig and coverage.
- Cancelled at about one minute: the daemon rig, the replay rig and the
  kernel probe. None of them reads a changed file: no workflow but the
  agent, cut, cut-daemon, nodeb, publish, zfs-cli-record and the CI
  ones uses the changed scripts.
