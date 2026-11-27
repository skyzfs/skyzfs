# Review of skyzfs/skyzfs#181 (M2.T5: the agent rig's retention course, part rt)

Head at the start and at the end: 72a70ac. Base: zfs-topology at 8a91baf
(#179 merged at c1a3255, notes 8a91baf). Test and rig code only; no product
code changed.

View formed from the diff, design §5.2, `crates/skyzfs-agent/src/engine/retain.rs`
and `program.rs` (`schedule`) before the description was read.

## Verdict

Nothing blocks. The round model is right and is what the agent does. X and
B pin what the retire's docs promise without leaning on timing; S leans on
the rig's round schedule (F1). Two pins cannot fail on an agent that breaks
what they are named for (G1, G2). Several judge clauses have no unit test that
fails when the clause is broken (T1 to T4). Docs nits N1 to N4.

## The author's questions

1. **The round model** (`due_retired`, `judge_keep`, `judge_in_round`).
   Right. `program::schedule` runs begin, then `retain_while`, then
   `settle_while`; the `snap/` line is printed by the outcome sink when the
   settle ends a publish, and begin, `sync_past` and `restart` print none.
   So a name whose `snap/` line is in round r first counts at round r+1's
   retention, and `--keep N` leaves N+1 published between rounds. `judge_keep`
   flags a retire in a round <= the round of the keep-th later name's `snap/`
   line (it catches a retire after the settle, one round early), and
   `judge_in_round` flags one later than that round + 1 unless a report
   named the name (it catches N+2 kept). Both read the rounds of the `snap/`
   lines, not a fixed schedule, so a publish that settles a round late moves
   the due round with it. `due_retired` takes the names published before the
   last round line less the newest N; the rig signals only between rounds
   (`run_rounds_then_term`, 3 s left), so the last round's retention is
   whole. Kernel: K retired t1 to t4 in rounds 4 to 7 in both runs
   (45992040370 and the head's 45998074504). `ctl_K_keep_3` shows the judge
   fails on the case's own lines.
2. **S, X, B.** X (`pin_X-*_said`, `_request_log`,
   `pin_X-destroy_first_member`, `pin_X-retire_nothing_destroyed`) rests on
   kill points only: no timing. B's `pin_B_steps` reads run2's first round,
   when every name run1 printed is in the record. It requires exactly
   min(8, n1-1) and no round over 8, so it tells STEPS_MAX 7 and 9 from 8.
   No timing. Children first: X-destroy shows the first destroy is the
   deepest. S shows that `pool@t` and the failing child stay while the
   grandchild is gone. Together they pin deepest first and `pool@t` after
   a failing child. S leans on the schedule: F1.
3. **Pins that pass on a broken agent:** G1 (`--abandoned-keep-s` ignored), G2
   (a person's give-up destroyed with no `look`). The rest held up. A
   destroy by name rather than by GUID, `destroy -r`, a wrong commit, a
   retire asked again of a stuck name, a report said every round, a stuck
   name that blocks the others, more than eight in a round, a dropped
   name said aloud: each fails a pin.

## Findings, most severe first

### F1 (low to medium, not blocking): S's verdicts rest on each round settling its own publish

`agent-record.sh:1474-1494` (`ret_stuck`) dumps the record and the snapshots
after 4 round lines and lets the hold go there. `agent_retain_check.py:505-547`
judges `pin_S_stuck`, `pin_S_children_first` and `pin_S_others_go_on` against
those fixed points. The retire rounds follow the `snap/` lines (question 1),
so if round 3's publish settles in round 4, t2 is retired in round 5, after
the hold was let go at 4. `pin_S_others_go_on` is then FAILED, exit 1, on
a correct agent. Two such slips (t2's `snap/` line in round 4) leave t1
unretired at the dump: `pin_S_stuck` and `pin_S_children_first` FAILED. The
first run (45989050774), with the release after round 3, failed in exactly
this way. Rounds took 0.5 to 1 s of the 10 s tick in both green runs, so
this is unlikely, not impossible. Suggested: from the `snap/` lines, the
round t1 is due (the round after t2's) <= 4 and the round t2 is due (the
round after t3's) <= `released.rounds`, else a premise ERROR
(`premise_S_schedule`), not a verdict. Another way is for the recorder to
wait, bounded, for `retired t2` before the dump and the release. Certainty:
high for the mechanism, low for how often it happens.

### G1 (test gap, not blocking): `--abandoned-keep-s` is not pinned

A-old: the give-up comes in a settle and the next retention is a round
(10 s) later, past 5 s. A-members: no pin reads the round. An agent that
destroys a given-up name at once passes both. On the head the agent
retired A-members' name in round 2, which suggests it kept it, but nothing
judges that. Suggested: in A-members, run first with the default keep (24 h)
for two rounds and pin that the name is still in the record with its
`@t`. Then run the 5 s run as now. No timing is needed.

### G2 (test gap, not blocking): a person's give-up is not shown looked up before its destroy

`retain.rs` looks `snap/t` up before it destroys the members of a name a
person gave up (`Due::Destroy { look: true }`). In A-members `snap/t` is
absent, so an agent that skips the look passes. The daemon's request log
has no `look` lines (only asked, ended, retire, withdraw). The run's trace
would show it: pass `--trace-points` to A-members' `run` and pin a
`look-given-up` point for t before its first `destroy-member`. The landed
case stays unit-only, as the description says.

### T1 (test defect): `pin_S_released`'s window is not tested

`test_retired_while_held` and `test_retired_three_rounds_after`
(`test_agent_retain_check.py:562-572`) both keep the model's round-5
`retired s1`. So they fail on the "once" clause, not on the window. Planted
`rel < r` -> `0 < r` and `<= rel + 2` -> `<= rel + 3`: all 105 tests pass. With
isolated inputs (s1 retired only in round 4, or only in round 7) the check
itself gives FAILED and the plant gives ok (`isolate.out`).

### T2 (test defect): `test_a_kept_name_marked` does not isolate the mark

The test (`test_agent_retain_check.py:287`) writes a record with b13 only, so
`pin_B_kept` fails on "b12 is kept and not among the record's names
published". Planted the `retire != '-'` clause out: all tests pass. With
b12 unmarked and b13 `retire=ref`, the check gives FAILED and the plant ok.

### T3 (test gap): "once each, in order" of `judge_retired_once` is not isolated

Planting a compare that ignores duplicates, or one that ignores order:
all tests pass. `test_retired_out_of_order` also drops k1, so it fails
either way. In K, `pin_K_retired_in_round` backs up a duplicate. In B
nothing does: B with `retired b11` twice, or b2 before b1 in run2's round
1, gives FAILED on the check and ok with the plant.

### T4 (test gaps, low)

Each of these plants survives:
- `pin_K_retired`'s record clauses: the published-set clause and the
  stuck/abandoned clause are each redundant in the only test,
  `test_the_record_keeps_three`;
- `judge_keep`'s "fewer than keep after it" branch: without it,
  `names[i + keep]` raises, and an exception gives exit 2;
- `rig-error.tsv` -> exit 2 (the shell's `rig_error` forces 2 anyway);
- the generic exception -> exit 2;
- `ctl_K_keep_3` turning MISSED when the case's lines give it nothing;
- "once" in `pin_A-old_retired` and `pin_A-members_retired`;
- `pin_A-nothing_dropped`'s no `@t` clause;
- `pin_S_stuck`'s "not among the published" clause.

## Nits

- N1 `agent_retain_check.py:90`: "in round 2's retire of t1"; it is round 3
  (the recorder's comment at `agent-record.sh:1383` says so).
- N2 `agent_retain_check.py:70`: "t1 is retired in round 3, when t2 is
  published"; t2 is published in round 2 and counts from round 3.
- N3 `agent_retain_check.py:82-83`: `pin_S_others_go_on` "a name stuck counts
  for no room". The pin shows that a stuck name does not hold up the next
  retire. The room (`PUBLISHED_MAX` 1128) is out of the rig's reach.
- N4 `agent-record.sh:1352`: "so that a name has four members". After K
  makes `POOL/late` every later name has five (the runs show `members=5`).
- N5: the description's "not driven on a kernel" list leaves out several
  paths: the `refused other` stop (`stopped <t>`, `Retire::Other`), the
  early leave of names given up for room, the stuck cap's
  `forgotten ... (destroy failed; destroy by hand)`, and a destroy not run
  to its end (`NotRun`).
- N6 `agent-record.sh:1463`: `g0` from `zfs get` is not checked. An empty g0
  passes `premise_K_others`, since '' differs from the new GUID.
- N7: the course's own `zfs` calls (`ret_keep`, `ret_stuck`, `rt_after`) are
  not under `lim`, where `scen_destroy` uses `lim 60`. This follows
  `asettle`'s precedent. No case here suspends the pool or stops the daemon,
  so nothing can wait for ever. Note it only for consistency.

## Checked and fine

- Exit codes: `verdict` -> `verdicts.tsv` -> FAILED (1); `case_error` -> a
  premise ERROR (2); `rig_error` -> `rig-error.tsv` plus the check, then 2
  unless the check found a verdict. A verdict beats an error, and a case's
  RigError is judged per case.
- Bounds: `await_agent` 120 s and 60 s, `run_rounds_then_term` 120 s a round,
  `await_rounds publish 4 240`, `astamp 60`, `acmd` `timeout 900`, `nodeb`
  `timeout 300`, `rt_status` `timeout 30`. The rig's own hold is let go in
  `rt_after` on every path, including "no round 4".
- The hand-off: `agent_check.py` passes `argv[1:4]`, and `agent_judged` calls
  the same CUT_CHECK.
- The workflow: `rt` is in the plan's allowed list and in both defaults; the
  unit job runs the new tests; the triggers cover the new files through
  `tests/kernel/agent*` and `tests/kernel/test_agent_*`; the record job
  keeps its `apt-fallback` step, `timeout-minutes: 150` and its artifact
  per part. The arm64 images stay by dispatch.
- Re-judged the first run's artifact (45989050774) with the head's check:
  exit 1, with only `premise_S_held` (released after round 3) and
  `pin_A-old_left` (the dump before the give-up), both rig-side and fixed in
  72a70ac. As the description says.
- The green dispatch's tree (37f9722) differs from the head merged on
  8a91baf only in `retain.rs` (4 lines of docs) and the notes, as said.

## Gates run

- `python3 -I tests/kernel/test_agent_retain_check.py`: 105 OK;
  `test_agent_check.py` 111 OK; `test_agent_fail_check.py` 63 OK.
- `bash tests/kernel/test_agent_record.sh`, `test_cut_record.sh`: all passed.
- shellcheck 0.9.0 (local, older than CI's): the warnings on
  `agent-record.sh` and `cut-record.sh` are the same as on the base, and
  the +44 on `test_agent_record.sh` are the local version's SC2317/SC2218
  noise on stub functions. CI's lint is the verdict (see CI).
- Plants in the check: 62, each in a copy of `tests/kernel`, where the
  suite passes unplanted (`plants.py`, `plants.out`). 45 caught and 17
  survive; the meaningful survivors are T1 to T4. "caught" means the suite
  failed; the test names after it are the first failing ones and not always
  the one aimed at. Isolations: `isolate.py`, `isolate.out`.
- Plants in the recorder: 13 in a copy of `tests/kernel` and `tests/lib`,
  with a baseline that passes (`plants-sh.py`, `plants-sh.out`). Each is
  caught by the test aimed at it: no recover, the rig's hold or the agent's
  kept, abandoned names not asked of node B, K's keep, S's dump round and
  order, X's run2 keep, B's rounds, A-old's wait, A-person's give-up, a kill
  point, the cases.tsv line.

## CI at the head (72a70ac)

All completed runs at the head are green. The first set (45997990…) was
cancelled by the concurrency group when the second set started. In the
agent rig (45998074504) every part has worst exit 0 with no FAILED, ERROR
or MISSED line: rt 100 ok and 2 CAUGHT, a1 150 ok, a2 148 ok, d 96 ok,
k 1068 ok, r 98 ok, f and fz 21 ok each. That matters because
`agent_check.py`'s hand-off changed. Also green: ci (lint with shellcheck,
test, unit-arm64), coverage, e2e, the publish rig, the cut rig and
cut-daemon, node B, the daemon rig (24.04, 26.04, 26.04-arm daemon, and the
stand-ins), the replay rig and the probe. None of those sources a changed
file apart from the shared `cut-record.sh` list. Codex: no findings at
72a70ac.

## Kernel runs

- 45992040370 (dispatch, 37f9722, one runner, ubuntu-26.04): 100 ok, 2
  CAUGHT, 2 INFO, exit 0.
- Head 72a70ac, pull request run 45998074504, part rt: 100 ok, 2 CAUGHT,
  2 INFO, exit 0. K rounds 4 to 7; S stuck `members=5`, t2 in round 4, t1 in
  round 5; X said 0/1, 1/1, 0/1; B 8, 2, 1 (took_ms 3308, 1806, 1535); A-old
  given up in round 2 and retired in round 3; A-members retired in round 2.
