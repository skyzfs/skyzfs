# Review of #180 (M2.T5: the agent rig's failed-while-serving course) at 427c9ec

Head 427c9ec2da5099286464ed06df43d928ac4c3f38. Base: `zfs-topology` at
e77be51. Four commits: ff52795 (the course), 8af6505 (the stamp race),
bd85971 and 427c9ec (f and fz in the default parts). No product code is
changed.

Read before the description: design §10 and M2.7c, the implementation
notes on M2.7c, `crates/skyzfs-agent/src/engine/lost.rs` and
`program.rs`, and the earlier reviews in `review/m2.7c/` and
`review/m2.7c/next_look/`.

## Verdict

**Not yet. One finding blocks: B1, Codex's P2.** The failure course
starts node B's watcher, but nothing judges what the watcher finds. The
finding is right; I confirmed it on this pull request's own artifact. It
has no answer on the pull request. The fix is small. Everything else is
low or a test gap, and none of it blocks.

The f and fz tables from this pull request's own agent rig run on
427c9ec read ok, pin by pin (below).

## Blocking

### B1 (medium, certain). The watcher's verdict in afail is never judged

Codex's thread is on `tests/kernel/agent_check.py:889`.

- **Where.**
  - `tests/kernel/agent_check.py:888-889`: `afail` goes to
    `agent_fail_check.main` before `pin_watcher_reads` (`:952-960`).
  - `tests/kernel/agent_fail_check.py` never reads `watch.log`.
  - `afail_end` (`tests/kernel/agent-record.sh`, near `:1310-1316`)
    touches `watch.stop`, waits up to 60 s, kills the watcher and
    discards its status. `watcher_end` in `cut-record.sh:526-543`, by
    contrast, logs a VERDICT and turns any other non-zero end into a rig
    error.
- **How it fails.** Suppose a failed volume leaves head at a cut that
  node B cannot read, for example an upload half done when the daemon
  dies. The watcher logs `VERDICT ...` and exits 1. Every U and W pin
  still holds, and the course exits 0. This course is the only one that
  fails a volume while it uploads, so it is where that check matters
  most.
- **Shown.** I took the f job's `afail/` directory from run 45964853040,
  appended one planted line `<ns>\tVERDICT node B could not read head
  (planted)` to `watch.log`, and ran
  `python3 -I tests/kernel/agent_check.py afail DIR DIR/serve.log x x`.
  It exited 0. The real watch.log files had 17 lines (f) and 20 lines
  (fz). Neither had a VERDICT or an ERROR line, so the run itself is
  clean.
- **Fix.** In `agent_fail_check.py`:
  - read `watch.log`;
  - treat no line, or an `ERROR` line, as a RigError;
  - judge `VERDICT` lines as `pin_watcher_reads` [kernel and server], as
    `agent_check.py` does.

  In `afail_end`, keep the watcher's status the way `watcher_end` does.
  A watcher killed after the 60 s grace is the rig's error, unless its
  log already carries a VERDICT. Add a unit test with a VERDICT line in
  `watch.log`, and one with an ERROR line.

## Defects (not blocking), most severe first

### D1 (low to medium, certain). U's case_error path turns the rig's error into a verdict against the agent

- **Where.** `tests/kernel/agent-record.sh:1225-1228`.
  - The guard reads: when round 1 left < 20 s to its tick,
    `case_error ...; afinish publish 1; return 0`.
  - `afinish` (`:1153-1156`) calls `await_agent publish 1`. `run` never
    ends by itself, so `await_agent` (`:235-247`) kills it with `-9` and
    writes `verdict publish_ended`.
- **How it fails.**
  - `agent_fail_check` reads every case's `verdicts.tsv`, which holds
    the FAILED pin [server], beside `rig-error.txt`.
  - `Judge.status` gives `1 if self.failed`, so the run ends with exit
    1, a verdict against the agent, when it should end with exit 2.
  - `arun`'s analogous path sends SIGTERM first and awaits the run.
  - The unit test `test_the_rig_s_error_in_a_case` builds the case
    without that verdict, and `test_agent_record.sh`'s
    "U: round 1 with less than 20 s" stubs `afinish`. So the tests do
    not show what really happens.
- **Likelihood.** Rare. Round 1 leaves about 55 s at the 60 s schedule.
- **Fix.** Send SIGTERM to the run and await it without a verdict, or
  kill it with no `verdict` line. Then test it without the stub.

### D2 (low, certain). POOL_LOST and DAEMON_GONE are set only in afail_end

- **Where.** `afail_end` (`agent-record.sh`, near `:1303`). Both flags
  are set only there.
- **How it fails.** An abort under `set -e`, or a `rig_error`, between
  W's `fail_now` and `afail_end` leaves POOL_LOST unset. `cleanup`
  then runs `lim 120 zpool export -f` on the suspended pool. That costs
  120 s and leaves a stuck export behind. It is bounded, so it is not
  a hang.
- **The opposite case.** On W's early-return paths, which come before
  any failure, `afail_end` still sets both flags. `end` then skips
  stopping the daemon, and `cleanup` SIGTERMs it with the pool
  imported.
- **Fix.** Set POOL_LOST and DAEMON_GONE where `fail_now` succeeds, and
  not in `afail_end`.

### D3 (low, likely). No premise that U's failure came before round 2's tick

- **The gap.** `pin_U_lost_line` assumes the failure, and so the lost
  line, came before round 2.
- **How it fails.** If `writer_off` plus the export took longer than
  `left` (at least 20 s by the guard), round 2 would run with the pool
  still imported or half exported. The pin would then FAIL as
  [server], when it is the rig that was late.
- **Fix.** Add a premise: the failure's stamp comes before
  `b1 + 60 s + late`, or before round 2's stamp.

## The pins against M2.7c

Each pin was checked against what the code prints (`lost.rs`,
`program.rs`) and against the f and fz runs.

- **The lost line** (held, step, verdict). Pinned exactly in U (`held=unknown
  step=not-imported reopen=- state=- looked_ms=- verdict=failed`) and in W
  (`held=yes`, step reopened or suspended, state SUSPENDED,
  `verdict=lost`). `held=unknown` for an exported pool rests on
  `client_holds` (`crates/skyzfs-ublk-sys/src/control.rs:175`), which
  returns `Some(true)` on EBUSY and `None` otherwise. The pin therefore
  pins the daemon's current behaviour. The description says so.
- **The refused start keeps the note.** Pinned (`pin_U_refused`: rc 4,
  the note still there).
- **The unheld round touches nothing.** Pinned (`pin_U_unheld`: no lost
  line, round 3 ok, nothing begun, the stderr words).
- **Exits 12 and 11.** Pinned (`pin_U_moved`, `pin_W_exit`,
  `pin_W_again`). The pins reject 9 and 10. The unit tests would not
  catch a weakening that let those back in (G4).
- **The 60 s ticks after the early round.** Pinned
  (`pin_U_default_schedule`, ±1 s, `late_ms` taken into account). This
  pins the lead's D1 ruling ("keep the slot", #173). If b-2 builds the
  quick retry or the reopen cap, this pin has to change on purpose.
- **looked_ms within 10 s.** Pinned, but it can hardly fail (G2).
- **SUSPENDED.** Pinned on the kstat file after the run (`pin_W_suspended`,
  [kernel]).
- **The second run.** Pinned (`pin_W_again`: 11 at round 1,
  `step=suspended`, within 2 s).

The plausible regressions:

| Regression | Caught by |
|---|---|
| Exit 9 or 10 again | `pin_U_moved`, `pin_W_exit` |
| The step not taken within seconds, U | `pin_U_step_within` (3 s) |
| The step not taken within seconds, W | Nothing (G1) |
| A pool touched in an unheld round | `pin_U_unheld` |
| A reopen every tick forever | `pin_W_exit` and `pin_W_again` (the run must end with 11) |
| A wrong verdict | Both lost-line pins |

## Test gaps

- **G1. W has no timing pin.**
  - The failure-to-end and step times are INFO only, so M2's "a failed
    volume is seen within seconds" for the held case rests on no pin on
    a kernel.
  - Two regressions would pass every W pin: a reopen that runs to its
    10 s limit, and a wait-loop watch that no longer works in W.
  - Example: round 2 at the 60 s tick, inside `afinish`'s 90 s, would
    still end with 11.
  - Suggestion: `pin_W_step_within`, note to lost line ≤ 5 s, or 3 s
    plus the reopen's `looked_ms`. Observed: ≤ 2.2 s over 10 jobs.
- **G2. `looked_ms ≤ 10.5 s` can hardly fail.** LOOK_MAX is 10 s, and
  `verdict=lost` after a reopen needs SUSPENDED.
- **G3. Not exercised on a kernel.** `verdict=retry`, the reopen cap
  and the quick retry. The last two are not built at e77be51. The
  description says so.
- **G4. Plants.** `plants.py` and `plants.out` here hold 59 plants in
  `agent_fail_check.py` and in the hand-off in `agent_check.py`, run
  against `test_agent_fail_check.py`. 52 were killed and 7 survived.
  These survivors matter:
  - **T1:** `STEP_WITHIN` 3 → 30 s. The tests use the constant
    symbolically, so no test holds the literal.
  - **U16:** `pin_U_moved` accepts 12 or 10. The tests only use rc 0.
  - **W1:** `pin_W_exit` accepts 11 or 9. The tests use 0 and timeout.
  - **U15:** round 3 may have begun something. No test covers it; a
    failed round 3 catches the realistic case.

  These are near-equivalent:
  - **U8:** the line after lost need not be round 2. The round-2 stderr
    words catch it.
  - **U10:** rc 4 or 0. The realistic failure is 124.
  - **R8:** any verdict word is parsed. The pins compare the verdict
    exactly. R8's first try had a broken plant; it was re-run alone
    after the fix.

  The fix: tests with rc 10, 9 and 4.0 s.
- **G5.** The unit tests of the U `case_error` path do not show the
  real verdict (D1).
- **G6 (B1).** No unit test has a VERDICT line in the afail course's
  `watch.log`.

## Clean-up after the failure (DAEMON_GONE, POOL_LOST)

Nothing is waited on without a limit after the failure.

- **The pool.**
  - `zpool status -v` runs under `lim 30`.
  - `zpool events` runs under `lim 30` and is cut with `head -n 4000`.
  - The kstat read cannot block.
  - `cleanup` skips the export when POOL_LOST is set.
  - `journalctl` runs under `lim 30` with `head -n 2000`.
- **The writer.** It gets `kill -9` and is not waited for.
- **The watcher.** Its `wait` has no limit, but the watcher touches no
  pool and has already been sent TERM. It is the same pattern as
  `watcher_end`.
- **The daemon.** `end` skips `end_daemon` when DAEMON_GONE is set.
- **Tests.** `test_cut_record.sh` covers both skips, with and without
  the flags.

The only weakness is D2.

## The stand-in's runtwice race (test_agent_record.sh, near :1015-1035)

The fix is sound.

- **What changed.** The handlers only append the signal to `got`.
  `nap()` polls every 10 ms. In `twice` mode it waits for two signals,
  then writes them all and exits.
- **Why it works.** Nothing exits inside a handler any more, so a
  nested handler cannot end the process before the outer one has
  written its signal.
- **The test can still fail.** If the rig sends one signal, the
  stand-in records one and the test fails.

There is a separate flake that is not this pull request's code, and
the base has it too.

- **The cause.** `trap 'rm -rf "$T"' EXIT` at `:15`, together with
  SIGTERM sent to background children before they exec (for example
  `sleep 30 & SERVE_PID=$!` at `:170`, then `kill "$SERVE_PID"` at
  `:173`). A forked child killed before its exec runs the parent's EXIT
  trap and deletes `$T`. Then 15 to 26 later tests fail.
- **Evidence.**
  - Instrumented, the trap fired in a child pid right after "recover:
    nothing left over".
  - Base e77be51 failed 2 of 4 runs under parallel load, and the head
    4 of 9. Serial runs in isolation mostly pass.
  - The bash behaviour reproduces alone:
    `bash -c 'trap "echo TRAP \$BASHPID main \$\$" EXIT; for i in $(seq 300); do sleep 30 & kill $!; done; wait'`
    prints TRAP from child pids.
- **Fix, for testing.**
  `trap '[ "$BASHPID" != '"$$"' ] || rm -rf "$T"' EXIT`.
  The new afail tests run in `( )` subshells and are not affected.

## The delta 8af6505..427c9ec (f and fz by default)

- **What changed.** Only the parts' default in the input and in the
  plan's fallback (`["a1","a2","d","k","r","f","fz"]`), the input's
  description, the header comment and `agent-record.sh:158`. It is
  consistent: arm64 and 24.04 stay by dispatch.
- **Cost.** In run 45964853040, f took 4 min 39 s and fz 4 min 49 s.
  That is about 9.5 runner-minutes per pull request that triggers the
  rig. They run in parallel with k, which is still running at 25+ min,
  so wall time does not grow.
- **The jobs.**
  - Names: "the agent through skyzfsd, f|fz, ubuntu-26.04".
  - Artifacts: `agent-rig-ubuntu-26.04-f` and `-fz`.
  - The summary is produced by `cut_summary.py`, as for the other parts.
  - `timeout-minutes: 150` is ample.
  - The apt-fallback step comes right after checkout. fz's
    `apt-get install zfs-zed` comes after it, under `net_try`.
- **Pushes.** A push of the rig's files to a testing branch now runs f
  and fz too. That is fine.
- **"By dispatch only" left behind.** Nothing in the workflow.
  - `agent-record.sh:1053` still says "afail, part f" (a nit).
  - In the implementation notes, `docs/skyzfs-impl-notes.md:1335` says
    "PR to follow" and "fixed in cf58d10". It is now #180, and the
    rebased fix is 8af6505. That line is the lead's.

## The f and fz tables at 427c9ec (run 45964853040)

Every pin is ok in both jobs, and the judge exited 0.

| | f (zed off) | fz (zed on) |
|---|---|---|
| U: note → lost line | 0.74 s | 0.04 s |
| U: rounds 3 and 4 after round 1 | 119.99 s, 179.97 s | 120.00 s, 180.02 s |
| U: refused start ended before round 3 | 117.8 s | 117.7 s |
| U: served and imported before round 4 | 59.2 s | 59.6 s |
| W: step | `reopened`, `reopen=ok looked_ms=0`, the reopen ≈ 1.0 s | `suspended` |
| W: failure → end (11) | 1.84 s | 0.26 s |
| W: failure after `asked:after` | 11.3 s | 11.8 s |
| W: assist syncs before the failure | 0 | 0 |
| The second run: 11 at round 1 | 10 ms | 8 ms |
| zed's journal | 1 line | 10 lines |
| "pool I/O is currently suspended" | `zpool status` only | `zpool status` only |

These agree with the description's two dispatches. Taken together, the
W failure-to-end times are 0.20 to 2.17 s over 10 jobs.

## CI at 427c9ec

At the last look everything finished was green:

- lint (shellcheck), test, unit-arm64, coverage, e2e, s3-backends,
  chart-lint;
- the agent rig's unit job and a1, a2, d, r, f and fz;
- the publish rig, node B, the cut daemon and the kernel probe.

Still running: the agent rig's k, and the cut rig's "record and check"
on 24.04, 24.04-arm and 26.04-arm.

Shellcheck: CI's lint job ran it. The local 0.9.0 shows only SC2317
notes, the same class as at the base.

## Nits

- `agent-record.sh:1053`: "afail, part f" should be "parts f and fz".
- `agent-record.sh:1075`: W has "the agent's assist syncs going", but
  no assist sync had begun before the failure in any of the 10 jobs. The
  description says this correctly ("not driven").
- In the description, "The run goes on (`pin_U_step_within`)": the run
  going on is in `pin_U_lost_line` (round 3 exists).
  `pin_U_step_within` is the timing.

## Decisions for the lead

1. **B1:** I count it as blocking. It is a check the course starts and
   then drops, in the course where it matters most.
2. **G1:** whether W gets a timing pin (I suggest 5 s from the note to
   the lost line) so that M2's "within seconds" for a held device is
   pinned and not only INFO.
3. **The order with #179** (open, e6530f1), which adds `step=held` and
   `reopen=held` to the lost line. `LOST_LINE` is strict (a full match
   on fixed step words), so whichever merges second must fold the other
   in. Otherwise the course ends with exit 2: loud, not silent.
4. **`pin_U_default_schedule` pins D1 ("keep the slot").** A b-2 change
   to quick retries must change it on purpose.
5. **The implementation notes' line on M2.7c on a kernel:** #180, and
   8af6505 for cf58d10.
