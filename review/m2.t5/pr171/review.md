# Review of skyzfs/skyzfs#171 (M2.T5: the agent rig's run course, R-rewind, recorder follow-ups)

## The heads looked at

The review started at 7ab6c65. It then judged 4ccc4b4 (the round
line's fields after `under_way` read by name), and last **3b9dc1c**
(Codex's fix: the signal rule in `run_rounds_then_term` and
`pin_ID_no_round_after_sigterm`). The base is `zfs-topology` at 6868430.

The diffs read were:

- `git diff origin/zfs-topology...pr171`;
- `git diff 7ab6c65 4ccc4b4`;
- `git diff 4ccc4b4 3b9dc1c`.

The agent (`run`/`schedule` in program.rs, `Agent::sync_past` in
engine.rs) and design §5.2, §5.4 and §9 were read before the
description.

## Verdict at 3b9dc1c

No finding blocks the merge, once the agent rig's run on 3b9dc1c
(45923974050) is green on part r.

- **B1 is fixed.** At 4ccc4b4 the rig could give exit 1 against a
  correct agent: it sent SIGTERM right after a round that had overrun
  its tick, and Codex's P2 is the same finding. At 3b9dc1c the rig
  signals only after a round that left 3 s or more, and it stamps round
  lines until the run ends. The plants for the rule are in
  `plants_c-*.out`.
- **M1 should be fixed in this pull request.** `pin_R-rewind_synced_past`
  does not check the behaviour it is named for. The fix is small and the
  data is in the artifact. If it is not done here, the lead should
  record it as a follow-up.

Every other finding is low, a nit, or a question for the lead.

## B1 (was blocking at 4ccc4b4; fixed at 3b9dc1c). The SIGTERM could land inside a round

### At 4ccc4b4

`run_rounds_then_term` sent SIGTERM as soon as the N-th round line came.
`schedule` begins the next round at once after a round that overran its
tick, and asks `go()` only at the top of the loop. So the round went on,
printed its line, and `judge_rounds` reported "the rig saw N of N+1
round lines". `b1_probe.out` shows this: the judge at 4ccc4b4, given
R-rewind's lines with round 2 run to its tick, returns that failure and
also "round 1's publish ... was not printed published".

Run 45918311854 shows how close it came:

- Round 1 took 10488 ms against a 10 s tick.
- Round 2 began 488 ms late.
- Round 1's publish was refused `rewound` three times (see O1). It was
  published 0.7 s before round 2's line, which left 3.2 s to its next
  tick.

### At 3b9dc1c

- `round_left` is the run's interval less the last line's `took_ms` and
  `late_ms`. That is right: the next tick is due plus the interval when
  nothing was skipped, and the value is negative when the round overran.
- The rig waits up to three more rounds for one with 3 s or more left,
  signals only then, and gives a verdict and no signal otherwise.
- After the signal it keeps stamping lines until the run ends.
- `pin_ID_no_round_after_sigterm` (agent_check.py:628 and 725) fails any
  line whose arrival is after `term-at`.

### Questions about the rule

**Can a line after SIGTERM go undetected?** No, but it can be reported
under the wrong pin.

- `await_rounds` greps first, then checks the agent with `kill -0`.
- An agent that printed its line and ended between those two steps, a
  window of a few ms, leaves that line unstamped.
- `pin_no_round_after_sigterm` then passes. `judge_rounds`
  (agent_check.py:610) still fails "N of N+1", so the case is FAILED
  either way.
- Nit: grep once more after the agent has ended, so that the right pin
  names it.

**Can the cap hide a fault?** No.

- `judge_rounds` (every round ok, each beginning a publish, the ticks)
  and `judge_synced` (no later round syncs) judge the extra rounds too.
- A run that ends during them, or never leaves 3 s, gets the verdict
  "no round line with 3 s left".
- Low: when the agent died, that message names the wrong cause.
- Low: the agent's documentation lets a round overrun its tick, so a
  correct run on a slow runner could get exit 1 here. "No moment to
  test the signal" reads better as a case error (exit 2). This is
  testing's call; it is not blocking.

**Can the pin pass vacuously, or fail on a legitimate last round?**

- It can pass vacuously only with no arrivals, and then `judge_rounds`
  fails on the count.
- A legitimate round cannot fail it: every line present when the
  signal is sent was stamped before it. The exception is a race of a
  few ms. When the round before had overrun, the next round's line
  could arrive between `await_rounds`' grep and `round_left`'s read. It
  would then be read as the last line but not stamped before the kill,
  and the pin would fail it falsely. This is a nit: stamp what is
  present just before the kill.

**Is there a race with a round already in flight?** No. After a round
with 3 s or more left, the agent is in its tick wait, where `go()` is
asked every WAKE. The rig signals within about 0.1 s of reading the
line, so the margin covers the poll.

The rule's plants are in `plants_c-py.out` and `plants_c-ar.out`.

| Plant | Description | Result |
|---|---|---|
| C08 | the pin not judged | killed |
| C09 | `rounds_after`'s comparison loosened | killed |
| C10 | a bad `term-at` taken as 0 | killed |
| C11 | the pin also judged on runtwice's second signal | survives |

C11 survives because runtwice prints no round line, so the plant
changes nothing observable. Testing's own T01 to T08 are in the PR
body.

The rule's plants on the shell suite, `test_agent_record.sh` (`plants_c-ar.out`; C03 rerun alone in
`c03-rerun.out`, since its first lines were the local flake):

| Plant | Description | Result |
|---|---|---|
| C01 | the signal sent on the N-th line whatever is left | killed by "no signal after a round that overran its tick" |
| C02 | no cap | killed by "waits for three more rounds, not four" |
| C03 | a cap of four | killed by the same test |
| C04 | the signal sent with no round with 3 s left | killed |
| C05 | no stamping after the signal | killed by "a round line after the signal is stamped" |
| C06 | `round_left` without `late_ms` | killed |
| C07 | a 1 s margin | killed |

The unplanted suite at 3b9dc1c: 187 ok, all passed.

**On the runner** (run 45923974050, part r, 3b9dc1c), the rule acted in
the very case that was at risk. R-rewind's round 1 took 11017 ms, and
round 2 began 1017 ms late, took 6858 ms and so left 2125 ms. The rig
did not signal: it waited for round 3, which took 664 ms and left
9.3 s, and then signalled. The run ended 0.22 s later, with no round
line after the SIGTERM. Every `pin_*_no_round_after_sigterm` is ok
(R-run, R-kill, R-held, R-rewind).

**A new Codex P2 on 3b9dc1c** (test_agent_record.sh:1060) is open and
has no answer. The stand-in's `wait_point` stub assumes the Python
stand-in starts within 0.3 s; under load the SIGTERM can come before
its handlers are installed (`rc=143`). It is a flake of the local test,
not of the rig, and is the same kind of flake as the recover and
reserve ones I see under load. It does not block on its own merits,
but the author must answer it before saying "ready to merge".

## M1 (medium; test gap; still open at 3b9dc1c). `pin_R-rewind_synced_past` does not show that the begin came after the pool had passed the abandoned groups

`judge_synced` (agent_check.py:683) asks for:

- `synced=` above 0 in round 1;
- round 1's publish printed published;
- no later round synced;
- no `no publish begun` line.

It does not ask what the lead's M2.7d decision says: that `run` begins
no publish while the newest cut is behind.

Two regressions pass the pin, as long as the publish lands in time:

- a `sync_past` that returns `Clear` after one sync;
- a `run` that begins on `Behind`.

In both, the publish waits as pending, and the planner and the writer
carry the pool past.

A check with no clock is available in the artifact:

- The first `publish asked` line of round 1's name carries the count
  that the begin read before the take: 77 in run 45918311854.
- The `cut` line with that number must open `itself`: cut 77 is group
  88, `itself`.

`last_ask` (agent_check.py:516) already reads these lines, but it keeps
the last accepted ask, and this check needs the first. Do not compare
createtxg with the top before the export: in this run createtxg was 91
and the top 95, and the agent was right.

The run itself behaved correctly; I checked by hand
(`rewind_timeline-45918311854.out`). Six syncs took cuts 66 to 77, cut
76 at group 87 was the first to open as itself, and the intent came at
184.368 s, after cut 77.

## L1 (low; documentation). "The daemon holds every cut from then on" (agent-record.sh:812 at 3b9dc1c, and the description) is not so

By D7 the daemon holds at most two cuts: the one being uploaded and the
newest.

- The cut being uploaded is from the shared history. The freeze came at
  181.977 s, before cut 52 (group 58) and the rewind group 61.
- The cut that waits was cut 62 (group 95), the top of the abandoned
  history. Cut 63 (group 62) replaced it at the import, before `run`.

So no cut of the abandoned history was held at the publish. R-rewind
does not show §9's "with cuts of the abandoned history still held",
which the notes give to M2.T3's last stage. It does show §5.2's other
half, without pinning it: the publish was asked at groups 91 to 95,
refused `rewound` three times, and published by the first cut above 95,
which node B verified. An INFO line with the count of `refused rewound`
would record that.

## L2 (low). The premise is read before the run's own first `cuts`

With the writer off nothing should advance the pool between the two
reads. If something did, round 1 would print `synced=0` and get FAILED
rather than ERROR. It was not seen.

## Nits

- The header of skyzfs-agent-rig.yml (part r) lists M2.7a's cases, not
  R-rewind (M2.7d).
- `round_fields` (4ccc4b4): a field name with a digit or a capital, an
  empty value, a double space or a trailing space make the whole line
  "not one of its five", a rig error. That is stricter than "unknown
  fields are left"; say so in the docstring.
- 4043629: `ZCR_SETTLE_S=0000000030` is budgeted at 999999 s while the
  settle takes 30 s. This is harmless.
- `test_agent_record.sh` does not pin the values `await_rounds` stamps
  (plant S12, a constant stamp, survives). On a kernel run
  `judge_rounds` would fail such stamps, so this cannot turn into a
  pass.

## O1 for the lead: `sync_past` stops at a lower group than rule (R) lets a publish through

In run 45918311854:

- The pool was rewound from 95 to 61.
- The first cut that opened as itself was cut 76, at group **87**, after
  six syncs in 80 ms.
- The daemon refused `rewound` until the first cut above **95**, which
  came 15.7 s later.

The first cut came early because of the ring. The ring has 32 slots;
the cut lines fit 4 KiB slots exactly. The abandoned top 95 sat in slot
31, which the new history's group 63 overwrote. The abandoned groups
from 87 to 94 wrote nothing, so they left no uberblock. The highest
abandoned uberblock in view was then 86, and the daemon followed the
ring correctly.

Rule (R)'s mark is the highest group seen. It does not come down when
that group's slot is overwritten, so the rest of the gap closes at
about one group per txg timeout.

This is not a defect: the design says the mark only delays. But two
places in the text are inexact for 4 KiB slots:

- §5.2's "no cut opens as itself until the pool's groups have passed the
  abandoned ones" holds for the uberblocks still in the rings;
- the note "closes the window in seconds".

Decisions for the lead:

- whether `sync_past` should also pass the mark (the `cuts` reply would
  have to carry it), or the delay is accepted;
- whether the sentence should name the ring.

## The commits asked about

**4ccc4b4: the round line read by name.** `fields_probe.py` and
`fields_probe.out` checked these lines:

| Line | Result |
|---|---|
| No `bound=` (a line from before #172) | read |
| #172's trailing `bound=0|1`, in either order | read |
| An unknown field | read and left |
| Names with `synced` as a prefix (`syncedx=`, `synced_total=`) | not taken for `synced` |
| A field twice, or `synced=-1` | RigError |
| #172's real line | parses, and `judge_synced` is satisfied |

The tests kill each of these plants:

| Plant | Description |
|---|---|
| A12 | positional parsing restored |
| A13 | a duplicate field accepted |
| A14 | only the first field read |
| A15 | `synced` never read |
| A16 | `synced` not checked to be a count |

**4043629: my lows from the #169 look.**

| Low | How it is answered |
|---|---|
| D4 | Groups a second per round over each side's own window, with medians. The read's window holds its record's time, and its busy share is reported. |
| P15 | The writer's default margin is pinned (Z09 killed; Z10 in `plants-2.out`). |
| P18 | The K04 fake lists `@c`, and `release_quietly` is anchored (Z11 and Z12: `plants-2.out`). |
| Q04 | The pid is cleared before `rig_error` (plant Q04). |

Cut rounds are reported as `cut` on every side (Z03 and Z04 killed, Z13 in `plants-2.out`). No
recorder run has been dispatched on these commits, so there are no D4
figures from a kernel yet. The last dispatch was 45895208967, at
af0c183.

## The author's questions

1. **Is the premise real, and can the pin pass vacuously?** The premise
   is real; its second half (`opens=rewind` after the import) carries
   it.
   - No cut, or a numbering that did not continue below the top, gives
     ERROR (A01 to A03 and A10 killed).
   - The pin is gated on the premise (A09 killed), so it cannot pass
     vacuously.
   - With no sync, no publish or no `synced=` it fails (A04 to A08 and
     A11 killed).
   - The gap is M1.
2. **Does the freeze hold the abandoned cuts?** No: see L1.
   - The pool's newest uberblock is older than the daemon's newest cut:
     the top was 95, then cuts at 62 to 84 opened as `rewind`.
   - Node B judged the publish right: the row is cut 83, group 96,
     createtxg 91, opens itself; refs, content, read-only import,
     snapshot, 16 files.
3. **Can a verdict be wrong?**
   - At 4ccc4b4, yes: B1. At 3b9dc1c the rule closes it; see the
     remaining nits under B1.
   - The exit mapping is consistent: premise means ERROR, holds means
     FAILED, any FAILED means 1.
   - No NA hides a failure.
   - The three controls are CAUGHT in job r.

## Gates

At 4ccc4b4 (`gates.out`):

- **Python suites:** all pass with `python3 -I`.
- **`test_zfs_cli_record.sh`:** 732 ok, 0 FAIL.
- **`test_cut_record.sh` and `test_lim.sh`:** all passed.
- **`test_agent_record.sh`:** one run hit the known local temp-dir
  flake (recover and reserve checks), and two runs were clean.
- **shellcheck against the base:** only new SC2317 notes in the tests
  (stubs called indirectly).
- **YAML:** both workflows parse.

At 3b9dc1c:

- **`test_agent_check.py`:** 107 tests OK.
- **`test_agent_record.sh`:** 187 ok, all passed (`ar-control-3b9dc1c.out`).

## Plants at 4ccc4b4 (`plants.py`, `plants.out`)

| Group | Plants | Result |
|---|---|---|
| `agent_check.py`, premise and `synced_past` | A01-A11 | all killed |
| `agent_check.py`, round line by name | A12-A16 | all killed |
| `agent_check.py`, run course | A17-A28 | all killed (A22 run again with its text fixed, `plants-2.out`) |
| `agent-record.sh`, R-rewind and the run scenarios | S01, S03-S11, S13 | killed by their own tests |
| `agent-record.sh` | S12 (a constant stamp) | survives (see Nits) |
| `agent-record.sh` | S02 (no thaw) | its first lines were the load flake; run again in `plants-2.out` |
| `zfs-cli-record.sh` | Z01-Z04 (rates, the read's window, cut rounds) | killed |
| `zfs-cli-record.sh` | Z09 (the writing margin's default) | killed |
| `cut-record.sh` | Q04 | killed by "end_daemon: unreaped after SIGKILL, its pid let go" |

Further results, where they came before this was written, are in
`plants-2.out`.

## What the rigs show

### Agent rig run 45918311854, at 4ccc4b4

All six jobs succeeded, each with worst exit 0 and no FAILED or ERROR.

**Part r**, on one runner (ubuntu-26.04) in one run:

- the seven run cases ok;
- R-rewind's premise holds: imported at 61, top cut 95; after the
  import the newest cut is 65 and opens as a rewind;
- round 1 synced 6 times;
- two rounds;
- ended 0.26 s after the SIGTERM;
- every published name verified on node B;
- the controls caught.

The PR body's figures match the artifact. They leave out the three
refusals and the 3.2 s margin.

**It does not show:**

- that the begin came after the pass (M1, though I checked it by hand
  for this run);
- a held abandoned cut (L1);
- the 3 s rule, which came in 3b9dc1c.

### Other runs at 4ccc4b4

- Green: the cut rig (45918311923, four images), cut-daemon, nodeb,
  publish, ci, e2e, coverage and skyzfs-ci.
- The daemon rig, replay rig and probe were cancelled; they read no
  changed file.

### At 3b9dc1c

- **Agent rig 45923974050:** r, a1, a2 and d succeeded; k was still
  running when I looked.
- Part r's results are 101 ok, the three controls CAUGHT and 10 INFO,
  with no FAILED or ERROR.
- R-rewind on this second runner repeats the first in every figure:
  - imported at 61, top 95;
  - cuts up to group 84 open `rewind`, and the first to open `itself`
    is 87;
  - round 1 synced 6 times;
  - three `refused rewound`, then published at group 96.
- M1's check, which I applied by hand, holds there too: the first ask's
  count was 77, and cut 77 (group 88) opened as itself before the
  intent.
- **Other runs at 3b9dc1c:** green are ci, e2e, coverage, cut-daemon,
  nodeb, publish and skyzfs-ci. The cut rig is still running. Daemon,
  replay and probe were cancelled; they read no changed file.
