# Review of skyzfs/skyzfs#167 (M2.7a) at 16f3b3b

Head checked with `gh api repos/skyzfs/skyzfs/pulls/167 --jq .head.sha`:
`16f3b3b8fd94dd33dc6adbd1f7e18f9b19f0ff02`, branch
`skyzfs-coding-m2.7a`, three commits on `zfs-topology` at 9e70246:
the package (99e00ae), a host test's race (eedd83e) and the fixes of
Codex's two findings (16f3b3b). The base has moved to 517d055 since,
by a notes commit only.

I began at 99e00ae. Before the description I read the diff, design
§5.2 and §9, the notes' row of M2.7 and the as-built lines of M2.6c,
the engine's module docs (rounds, memory, time, crash points), the
planner's answers to refusals, the host runner, the daemon adapter, and
what the agent rig's scripts read of the agent's output
(`tests/kernel/agent_check.py`, `agent-record.sh`). Then the
description and Codex's two threads. When 16f3b3b came I reviewed the
delta 99e00ae..16f3b3b on its own and re-ran the plants and gates
there. Line numbers below are at 16f3b3b unless they say otherwise.

Files beside this one: `scratch-tests.patch` (my tests, against
16f3b3b, not for merging as they are), `plants-99e00ae.py` and
`plants-16f3b3b.py` with their outputs, `gates-*.out`, `live.sh` and
`live.out`.

## Verdict

**Nothing blocks at 16f3b3b.** It can merge from review's side once CI
is green on 16f3b3b (in progress when I wrote this; green on 99e00ae).
I recommend one thing in this pull request first, and leave it to the
lead whether it must be (D0).

- **Codex's two findings were real, and both are fixed right in
  16f3b3b.** I had found the same two independently at 99e00ae (my F1
  and F2, with tests RA and RB that failed there). At 16f3b3b both of
  my tests pass. Each of the author's new tests fails when its fix is
  planted out (N1, N2 below).
  - Restarting before the settle is the right order. Skipping the
    settle would be the wrong one (reasons under F1).
  - The first-pass rule is right, and bounded.
- **G1 (test gap, recommended here): nothing tests the restart after a
  settle's error, or a restart that fails.** At 16f3b3b the plant "a
  settle's error does not restart" (N5) survives the author's whole
  suite. So do "a failed restart lets the round go on" at either place
  (N3, N4) and "Corrupt or Volume at a restart does not end `run`" (P2).
  - At 99e00ae the first of these was caught (the description's A4).
    16f3b3b moved the author's test to the begin's path, and the
    settle's path lost its test.
  - It is the rule that keeps the record on the disk from going
    backwards after a failed save, which no kill can do. My RI, RJ, RC
    and RF kill all four plants and pass at 16f3b3b. They are in
    `scratch-tests.patch`.
- **The round line (Q6)** is accurate in what it claims. Every path
  prints exactly one, no failed round reads `ok`, no tick is lost, and
  it is stable for tooling.
  - Three things a reader would get wrong are worth a sentence in the
    docs: `ok` is not "progress"; `failed` includes the two refusals
    (D2); and since 16f3b3b `begun=-` can sit after the `snap/` line of
    a publish whose begin "failed".
  - Wording is proposed below.
- **S1:** the worst a kill does is a `snap/` line told twice (same
  commit) or an `abandoned` line never told. No hold, snapshot or
  publish to the daemon is at stake.
  - The documented `abandoned` window misses one way in: a save that
    fails after its rename, with no kill (F5, my RE, still failing at
    16f3b3b).
  - "At least once" holds across a kill of the process, not a power
    cut.
- **The rest is low or a nit:** F4 (the restart rule is wider than it
  needs, and each restart re-tells stops and stall reports), F5, F6
  (`--device` fixed for the life of `run`, for M2.7c), and nine nits,
  two of them on docs that 16f3b3b left stale.

## Findings, most severe first

### F1 (was medium; fixed in 16f3b3b): a begin whose save failed was followed by a settle that wrote the intent out of the record

Codex's first thread. I found it independently at 99e00ae.

- **At 99e00ae.** When `begin` failed, the round still called
  `settle_while`, and the restart came only at the next round.
  `Agent::commit` (`engine.rs:1770-1776`) sets `self.record` only after
  `keep.save` returns `Ok`. A save whose rename landed and whose
  directory sync failed leaves the new record on the disk and the old
  one in the process, which is the case `restart`'s own docs name.
  - With publish A under way, B's intent save failed that way.
  - A's next step in the settle saved the process's copy (A alone), so
    B left the disk too.
  - My RA showed it: round 2 `failed ... begun=- under_way=0`, A's
    `snap/` line, and B never published nor in the record. Nothing
    leaked, since `begin` writes nothing after the intent.
  - The package's claim that "no path is new to the crash matrix" did
    not hold for that path.
- **The fix** (`program.rs:968-988`):
  - Where `begin`'s error `restarts`, the round reloads at once and
    then settles.
  - Where that reload fails, `restart` stays set and the round settles
    nothing (`:989`).
  - A settle's error sets `restart` for the next round (`:996`).
- **Is the order right?** Yes.
  - *Restart, then settle* is what a new process does after a kill at
    that point: `publish` loads the record and settles. The crash matrix
    covers it.
  - *Skip the settle* (my plant N7) would also be safe. But a begin
    that fails on a read (the daemon's `cuts`, a ZFS `props`) would then
    keep every publish under way from moving for as long as it fails.
    Under F4's rule each such error restarts, so the settle would wait
    on the daemon's `cuts`, a call the settle does not need.
  - The restart costs the sweep's walk again in the same round (F4).
    That is no more than 99e00ae paid one round later.
- **Tests.** The author's
  `a_begin_whose_save_failed_restarts_before_the_round_settles` fails
  with the fix planted out (N1), as do my RA, RC and RF. A failed
  reload at this point (N3) is caught only by my RC (G1).
- **Certainty.** Certain: read, and RA fails at 99e00ae and passes at
  16f3b3b.

### F2 (was medium; fixed in 16f3b3b): "at least one round of the engine" was not what `settle_while` gave

Codex's second thread. I found it independently at 99e00ae.

- **At 99e00ae.** `settle_while` ran `zfs count` and the sweep, then
  compared the clock with its end, and returned `Unsettled` before any
  publish's round when they had taken the budget. With `--every-s 1`
  and a count of 1.5 s nothing was ever published. My RB: 392 rounds
  over 600 s of fake time, eight publishes at `Intent`, and from round
  10 each round `failed` with 8 under way, for good.
- **The fix** (`engine.rs:760-787`): the limit is asked only after the
  first pass (`passes > 0`). `go` is still asked before every pass, the
  first too, so a SIGTERM is not delayed by it.
- **Is it right?** Yes.
  - A round is now bounded by the count and the sweep, one pass, and
    then the limit. Before, a pass could already run past the limit;
    now the count and the sweep can as well.
  - It also changes `publish`, `settle` and `recover`, which settle
    through the same function: they now make one pass even when
    `--limit-s` has passed before it. That is what their callers want.
    But the module docs still say "for at most `--limit-s` seconds"
    (`program.rs:35`, N7).
- **Tests.** The author's `a_round_settles_once_however_long_the_count_took`
  fails with the limit asked before the first pass (N2), as does my RB.
- **Certainty.** Certain.

### G1 (test gap, low to medium; recommended in this pull request): the restart after a settle's error, and a restart that fails, are untested

- **Where.** `program.rs:962-967` (a restart at the round's start that
  fails holds the round), `:980-985` (one after a begin's error that
  fails holds the settle) and `:996` (a settle's error restarts at the
  next round). `ends_run` (`:854`) names `Corrupt` and `Volume`, which
  in `run` come only from a restart's read.
- **Plants at 16f3b3b that the author's suite does not catch:**

  | Plant | Author's suite | Mine |
  |---|---|---|
  | N5: a settle's error does not restart | survives | RI, RJ |
  | N4: a failed restart at the round's start falls through | survives | RJ |
  | N3: a failed restart after a begin's error lets the round settle | survives | RC |
  | P2: only `Unmarked` ends `run` | survives | RF |

  At 99e00ae the first was the description's A4, caught by
  `a_failed_round_is_told_and_the_next_starts_again_from_the_record`.
  16f3b3b made that test's failed save restart at once, on the begin's
  path, so the settle's path lost its only test. The description's
  plant table still lists A4 as caught.
- **Why it matters.** Without the restart after a settle's failed save,
  the next round's `begin` saves the process's stale copy. The record
  on the disk then goes back a step: a `Forget` or `Done` undone. No
  kill can do that, so the crash matrix does not cover it. RI shows the
  symptom it is easiest to see: the `Forget` save fails after its
  rename, and under N5 the publish's `snap/` line is told again in the
  next round.
- **Fix.** Take RI, RJ, RC and RF (`scratch-tests.patch`, module
  `review167`), or tests like them. They pass at 16f3b3b. The patch
  needs its helpers folded into the file's own (`SaveFails` can take a
  failed load and a spoiled record with two more fields).

### F4 (low, for the lead): the restart rule is wider than it needs to be

- **Where.** `restarts` (`program.rs:866-868`): every error but `Full`
  and `Premise`.
- **What a restart costs.**
  - It forgets the sweep, so the next sweep lists every snapshot of the
    pool and runs `zfs holds` on those with a hold (M2.6c-5d).
  - It forgets each publish's anchor on the clock that does not step,
    so holds are reckoned on the wall clock again (the engine's
    "Time").
  - It forgets the planner's memories.
  - It clears `stopped` and `reported`, so every stop and every stall
    report is told again after it (from the code: `engine.rs:575-576`,
    `:1616`).
- **When.** After errors that are only reads and leave the process as
  its record: `cuts` not answered or no cuts yet (`Error::Daemon` from
  `begin`), or a ZFS read that failed. While the daemon is down, every
  tick restarts, walks the pool, and re-tells each stop. A timer that
  started `publish` anew did the same, so it is no regression. But it is
  not needed, and M2.7c's failed volume is a daemon that stops answering
  for a long time.
- **Options.** (a) Keep it as the conservative rule and say its cost in
  the docs. (b) Restart only after `Save`, `Io` and `Stopped`, the errors
  that can leave the process unlike its record. (D1.)
- **Certainty.** Certain from the code. The walk's cost at scale is
  M2.6c-5d's figures, not measured here.

### F5 (low, gap): the `abandoned` window opens without a kill too

- **Where.** `engine.rs:1712-1727`; the module docs `program.rs:81-87`
  say "a kill between the two loses that line".
- **Also.** In `run`, an `Abandon` save whose rename lands and whose
  sync fails returns `Error::Save` before the line. The next restart
  reads the publish among the abandoned, so no round tells it.
  - My RE shows it, with no kill: the give-up is in the record, and no
    `abandoned` line for it.
  - It fails at 99e00ae and at 16f3b3b, as expected: nothing was meant
    to change it.
  - The `Forget` side has no such case, since its line is out before
    the save.
- **And the fallback is bounded.** The record keeps the last 64
  give-ups (`ABANDONED_KEPT`), so a lost line is in `record`'s output
  only until 64 more.
- **Fix.** Name both in the docs now. M2.7b's record step closes both
  (D3).

### F6 (low, gap for M2.7c): `--device` is fixed for the life of `run`

- **Where.** `bins/skyzfs-agent/src/main.rs` builds `DaemonSocket` once
  with `run.device`. `engine.rs:1655-1667` keeps the last device number
  it read and uses it when a stat fails.
- **Scenario.** skyzfsd serves the volume again on a new device
  (`/dev/ublkbM`). A timer would start the next `publish` with the new
  path, but `run` keeps the old one. Every round's premise is then
  refused (`round n failed`, and `run` goes on) until a person restarts
  it.
  - If the old path is gone, the stat fails. The number kept from an
    earlier round is the old device's, and the premise fails on it.
  - After a restart nothing is kept, and the round fails with `the
    daemon: its device: ...`.
  - Nothing unsafe follows: only an agent that publishes nothing.
- **For M2.7c** (D4): `run` asks the daemon for its device, or the docs
  say that a new device needs a new `run`.
- **Certainty.** Certain from the code. That skyzfsd can come back on
  another number is from its `serve` and `delete` texts.

### Nits

- **N1.** The description says `Agent::restart` drops "reports made"
  and "kill points reached". It clears `reported` (which stall reports
  were told, so they are told again) and keeps `reports`. It clears
  `at` (the publish the next kill point is for). The engine's docs say
  it right.
- **N2.** `loaded` maps a record that cannot be read to `Error::Io`,
  whose text is "a manifest's file: ..." (`engine.rs:1821`, `:340`).
  That predates this package. But `run` now prints it as a round's
  error after a restart, as "round 2: a manifest's file: <the record's
  error>". A `Record` variant, or a word in the text, would say what
  failed.
- **N3.** After SIGTERM, a round still makes `begin`'s remaining calls
  and `settle_while`'s `count` and sweep before it asks `go`
  (`engine.rs:765-782`).
  - With ZFS calls of up to 60 s each, the end can take minutes.
    systemd's default stop timeout is 90 s, and its SIGKILL is safe.
  - Asking `go` at the top of `settle_while`, before the reads, would
    end sooner.
  - The docs' "between two rounds of the engine" is true; "within
    [`WAKE`]" holds only for the wait.
  - Seen with the binary: 11 s from SIGTERM to the exit, with fake ZFS
    calls of 3 s ("Driven for real", below).
- **N4.** The module docs say `run` "ends with 1 or 8 where it cannot
  start". A corrupt record or another volume's at the start gives 6
  (`program.rs:95-97`). The usage text has it right.
- **N5.** `signal-hook = "0.3"` is declared in the bin's `Cargo.toml`
  and not in `[workspace.dependencies]`. So is `rustix` there, so this
  follows the file it is in.
- **N6 (new in 16f3b3b).** The module docs' `failed` bullet
  (`program.rs:75-79`) still says "the next round reads the record
  again first". Since 16f3b3b that holds after a settle's error only;
  after a begin's error the reload is at once, before the settle.
  `schedule`'s docs (`:925-934`) were updated, but the module docs were
  not.
- **N7 (new in 16f3b3b).**
  - `program.rs:35`, "for at most `--limit-s` seconds": now at least one
    pass, whatever the limit (F2).
  - `engine.rs:739` is a doc line of 103 characters. rustfmt leaves
    comments alone, so `cargo fmt --check` passes.
- **N8 (new in eedd83e).** `stuck() <= before` is sound.
  - A child of the test before can be reaped during this one. The plant
    it guards (a reaper that never takes the count down) still fails it,
    since then nothing is ever taken down.
  - A sturdier form is the next test's: wait for `stuck() == 0` first,
    then check `== 0` at the end. That pins an over-count and an
    under-count alike.
  - I ran the host tests twenty times at 16f3b3b: see "Gates".
- **N9.** In a pass where nothing moved, `settle_while` sleeps `TICK`
  and then runs the count and the sweep again before it finds the limit
  passed (`engine.rs:792-794`, then `:765-782`). This predates
  16f3b3b. At a short `--every-s` with a slow count each round then
  makes two counts and two sweeps. Asking the limit before the sleep
  would save one.

## Decisions for the lead

- **D0 (G1): the tests of the restart paths.**
  - The settle's restart, and both failed-restart paths, have no test
    at 16f3b3b. N5 is the guard against a record going backwards.
  - By the precedent of #140 (plant survivors taken in a later package)
    this does not block.
  - I recommend taking RI, RJ, RC and RF here, since they exist and
    pass.
- **D1 (F4): how wide the restart rule is.**
  - (a) Keep "every error but `Full` and `Premise`", and say its cost:
    a walk of the pool and the stops re-told at each failed tick while
    the daemon is down.
  - (b) Restart only after `Save`, `Io` and `Stopped`.
  - 16f3b3b's order (restart, then settle) is right under either.
- **D2: `failed` for a refused begin.** Eight under way and the premise
  are refusals by design. Under a slow uplink, every round reads
  `failed` while it lasts.
  - Keep `ok|failed` and say so in the docs (the wording below does),
    or add a third word, `refused`.
  - M2.T4 should know which before it counts.
- **D3 (S1), for M2.7b.** The ruling to close the `abandoned` window in
  record v3 is the right place. Three things for its plan:
  - an `Abandoning` step saved before the line, and the removal after
    it;
  - retention's retiring of a ref and a snapshot told before the record
    forgets it, by the same rule as `snap/`;
  - consumers (M2.T4, the rigs) that take a repeated line as normal.

  One thing for the docs now: "at least once" is across a kill of the
  process, not a power cut. The line is not synced, and the save is.
- **D4 (F6), for M2.7c.** `--device` is read once for the life of
  `run`.

## The round line (Q6)

```text
round <n> ok|failed took_ms=<ms> late_ms=<ms> skipped=<k> begun=<t|-> under_way=<count>
```

**Every path.** One `write_all` prints it (`program.rs:1018`), once for
each round that ran. It comes after that round's error lines and before
the exit of an `ends_run` error. A round ends in one of these ways, and
each prints one line:

| Round | word | `begun` | `under_way` |
|---|---|---|---|
| begin and settle ran, no error (`Unsettled` is none) | ok | the name | as saved |
| begin refused (8 under way, premise) | failed | `-` | as saved |
| begin failed on a read or a save, reloaded, settled (16f3b3b) | failed | `-` | the record's, reloaded; the intent a failed save left is counted, and may already be published this round |
| begin failed, and its reload failed: no settle | failed | `-` | what the process held (may be stale) |
| restart at the round's start failed: nothing else ran | failed | `-` | what the process held (may be stale) |
| begin ran, a publish's step failed in the settle | failed | the name | the process's; a save that failed after its rename leaves it one step behind the disk |
| `go` said no in the settle (SIGTERM) | ok | the name | as saved; `run` then ends 0 |

`under_way` is always `agent.record()` (`program.rs:1016`), the
process's record, never a read of the disk.

No round is left without a line except by a kill or the second signal
(`_exit` at once). No line is printed for a round that did not run. So
no failed round reads `ok`, and no round is hidden.

**Is it accurate?**

- **`ok` means only that no engine call returned an error.** Most
  trouble does not return one: the daemon not answering `progress`,
  `look` failing, or a release that failed are seen by the planner and
  retried. A round in which nothing moved reads `ok`. That is what the
  docs say, but a reader will take `ok` as "healthy". A growing
  `under_way` is the sign to read.
- **`failed` also covers the two refusals by design** (D2).
- **`begun=-` does not mean that no publish began (since 16f3b3b).**
  A begin whose intent save failed after its rename has its intent in
  the record. The reload finds it, and the same round can publish it.
  I saw it in a test at 16f3b3b: `snap/skz-...010000Z` and then
  `round 1 failed ... begun=- under_way=0`. The docs say "`-` where its
  begin failed", which is true, but the line then reads as if nothing
  began.
- **`skipped` is right.** Over a run, rounds plus the sum of `skipped`
  is the number of ticks since the start. My RD: a round of 250 s at a
  60 s tick gives `skipped=3`, and 7 ticks are 4 rounds and 3 skipped.
  - The ticks are anchored at the start, so a round's length never
    makes them drift.
  - `late_ms` is below one tick, except where writing the previous line
    blocked (a full pipe). Those ticks are counted in the next round's
    `skipped`, so none is lost.
- **`took_ms`** includes the restart, the begin and the settle. With
  anything still under way, it is about one tick, by design.
- **`under_way`** counts publishes stopped for a person too, which no
  round moves.

**Is it stable for tooling?** Yes.

- The second word is positional. The rest are `key=value` in a fixed
  order, and no value holds a space (a publish's name is `skz-<UTC>`,
  with `-N`).
- `agent_check.py`'s `parse_out` keys on the first word and the field
  count, so it ignores a `round` line. `agent-record.sh` reads only
  `snap/` lines. Nothing in the repository reads the round line yet;
  `round_line`'s docs name M2.T4.

A tool needs two things that the docs do not say:

- The outcome lines made in a round come before its round line, and
  nothing is told between rounds. So each outcome line belongs to the
  round line that follows it.
- `<n>` starts at 1 in each process, and nothing marks a start or an
  end. A tool that reads a journal across restarts cuts it by process.

**Wording I would use** (module docs, `program.rs:66-79`):

> - `ok`: no engine call of the round returned an error. It says
>   nothing about progress: a publish that waits on the daemon is not
>   an error. `failed`: one did, each on standard error as
>   `skyzfs-agent: round <n>: <error>`; that includes a begin refused
>   for 8 under way or for the pool's premise.
> - `begun`: the publish this round began; `-` where its begin returned
>   an error. A begin whose save failed after it was written may still
>   have recorded its publish, which this or a later round then settles.
>   Where the record could not be read again, the round settled nothing
>   and `under_way` is what the process held before.
> - `under_way`: publishes in the process's record as the round ended,
>   those stopped for a person included.
> - After an error that may leave the process unlike its record, the
>   record is read again before the engine is asked anything more: at
>   once after a begin, at the next round after a settle.
> - The outcome lines a round makes come before its line; `<n>` counts
>   from 1 in each process.

## The outcome windows by kill point (S1)

16f3b3b changes nothing here. The steps at a publish's end are in
`engine.rs:1111-1158` and `:1712-1727`, and in the planner at
`plan.rs:545-546` and `:671-677`. The tables say what a kill between
two of the steps leaves. "Next" is the next start, or, in `run`, the
round after a restart.

**Published** (`look` saw `snap/t` with the commit that `Done` names):

| Kill after | Before | Line now | Next | Worst |
|---|---|---|---|---|
| look | `Done` save | none | plans `Done` again | none |
| `Done` save | `zfs release` | none | `Release`, `Released`, `Forget`: told once | none |
| `zfs release` | `Released` save | none | the hold is gone, so `Forget` at once: told once | none |
| `Released` save | the line | none | `Forget`: told once | none |
| the line | `Forget` save | `snap/t C` | still `Done`: told again | the same line twice |
| `Forget` save | `unlink` | `snap/t C` | not in the record; `loaded` removes the manifest's file | none |
| `unlink` | | `snap/t C` | nothing | none |

In `run`, without a kill:

- A save that fails before its rename is the fifth row: the same line
  twice in one process.
- One that fails after its rename is the sixth.
- Otherwise the same line is never told twice in one process.

**Abandoned** (the planner gives up: `Release` first where held, then
`Abandon`):

| Kill after | Before | Line now | Next | Worst |
|---|---|---|---|---|
| `zfs release` or `Released` | `Abandon` save | none | planned again; the give-up may not be (it may have rested on the process's memory) | none: no give-up recorded, none told |
| `Abandon` save | the line | none | among the abandoned, not under way: never told | the line lost |
| the line | `unlink` | `abandoned t W` | `loaded` removes the manifest's file | none |

As F5 says, an `Abandon` save that fails after its rename is the second
row, without a kill.

**Stopped** is told each time the process plans the stop. That is once
per process, and again after each restart in `run` (F4), for as long
as the publish stays under way. It is never lost, and sometimes
repeated.

**What is never at stake.** The ordering changes only when a line is
printed. Every effect on ZFS, the daemon and the record stays where it
was, so no hold or snapshot can leak and nothing is asked of the daemon
twice because of it. The line `snap/t` is true when told: `Done` is
recorded only once `look` saw the ref with this commit, and nothing the
agent does takes back a ref once swapped.

**Is the documented window complete?**

- **`snap/`:** yes. It is at least once across a kill, as documented,
  with the duplicate above.
- **`abandoned`:** two things are missing (F5). A failed save opens the
  window with no kill, and the record's fallback keeps only 64
  give-ups.
- **Both:** "at least once" is about the agent's process. The line is
  flushed to the descriptor before the save, but the save is synced and
  the line is not.
  - Where standard output is a file, a power cut can keep the save and
    lose the line.
  - Where it is a closed pipe, the line is dropped (`let _`).
  - The ref in object storage is the truth in every case, and the lines
    are reports.

What M2.7b should do is under D3.

## Restart, exit codes and signals

**Which errors end `run`, which restart, which only fail the round**
(`program.rs:854-868`, `:728-741`):

| Error | In `run` | Exit |
|---|---|---|
| `Corrupt`, `Volume` (in `run`, only from a restart's read), `Unmarked` (the sweep) | ends `run` after the round's line | 6 |
| `Full`, `Premise` (`begin`'s refusals, before it changes anything) | the round fails; no restart | none |
| `Daemon`, `Zfs`, `Save`, `Io`, `Stopped` | the round fails; restart at once from a begin, at the next round from a settle | none |
| `Unsettled` | not an error: the round's time is up | none |
| `NotUnderWay` | cannot come from `begin` or `settle_while` | none |

- That is the same split as `publish`'s exit codes. What exits 6 there
  ends `run`, and what exits 1 or 7 there is tried again at the next
  tick.
- The rewound rule (design §5.2, "(R)") does not reach this table. A
  `refused rewound` is a planner's answer (look, then ask again with a
  count read anew), not an error, so it neither fails a round nor
  restarts.
- A restart forgets the planner's memories, which is what a kill does,
  and the crash matrix covers a kill at every step.

**Can a failure loop, spin, drop a hold or publish twice?**

- **Loop.** A lasting failure fails every round for as long as it
  lasts: a full disk, a daemon that is down, a pool that fails its
  premise. Each round says so once on standard error. That is intended,
  since `run` must outlive them.
- **Spin.** No. Every round, failed or not, ends in the wait to the
  next tick, which sleeps in `WAKE` slices (`program.rs:1026-1031`).
  Within a settle, a pass in which nothing moved sleeps `TICK`
  (`engine.rs:793`). A failed restart does nothing else in its round,
  and then waits.
- **A hold.** No. A restart reloads the record and clears memory. Every
  hold the record names is still in it, and the sweep handles those it
  does not name.
- **Twice.** Not to the daemon and not in object storage: nothing here
  changes the order of an effect.
  - `settle_while` returns at the first error of a publish's round
    (`engine.rs:788`, `?`), so within a settle no step follows a failed
    save.
  - Since 16f3b3b no step follows one across `begin` either.
  - Across rounds the restart at `:996` sees to it, and that is G1's
    untested line.
  - An outcome line can be printed twice (S1).

**Tick scheduling** (`program.rs:942-1033`). I checked it by reading,
and with the fake clock.

- The ticks are anchored at the start: `due = start + tick * every`, and
  `tick` advances by `max(passed, 1)` from the tick the round was for.
  A round's length moves no later tick, so nothing drifts.
- A round that runs past one or more ticks is not cut. The next round
  begins at once, for the last tick that passed, and the ticks in
  between are its `skipped` (RD, and the author's test of a 130 s
  round).
- A round's settle runs to the next tick, and no further whatever its
  own lateness (`a_late_round_ends_at_its_next_tick`). Since 16f3b3b it
  always makes one pass of the engine, so a round can overrun by its
  count, its sweep and one pass.
- The clock is the engine's (`Clock::mono_ms`, `Clock::sleep`). The
  whole schedule runs on the fake clock in the tests, and on the
  monotonic clock in the binary.

**Signals** (`bins/skyzfs-agent/src/main.rs`, `scheduled`):

- **Registration.** SIGTERM and SIGINT each get
  `register_conditional_shutdown(sig, 1, told)` and then
  `register(sig, told)`. The first signal sets the flag, and a second
  exits at once with 1.
  - This is signal-hook's documented pattern, and its order is the one
    the pattern needs: handlers run in the order registered, so the
    conditional one sees the flag unset on the first signal.
  - The handlers are registered before the agent is built.
- **When `go` is read.** It is read before each round, before each pass
  of the engine in the settle, and every `WAKE` (250 ms) in the wait.
  So a first signal ends `run` within 250 ms while it waits, and
  otherwise after the call in progress and the rest of the round's
  reads (N3).
- **No unsafe code.** `main.rs` keeps `#![forbid(unsafe_code)]`;
  signal-hook's flag API is safe.
- **Children.** They run in their own process group (the host runner),
  so a terminal's SIGINT does not reach a `zfs` call in progress.
  Under systemd's default `KillMode=control-group` the children get
  SIGTERM too. The call then fails, the round reads `failed`, and `run`
  ends 0 after the line. That is harmless, since a kill there is in the
  crash matrix. A unit file for `run`, when one is written, may want
  `KillMode=mixed` so that a call in flight completes.

**Driven for real** (`live.sh`, `live.out`). I ran the binary at
16f3b3b as `run --every-s 5` (or 60), with fake `zfs` and `zpool` that
fail after a sleep, and no daemon:

| Case | Exit | From the signal to the exit |
|---|---|---|
| SIGTERM while it waits | 0 | 0.06 s |
| SIGINT while it waits | 0 | 0.04 s |
| SIGTERM in a ZFS call of 3 s | 0 | 11.1 s: three more of the round's calls ran after the signal (N3) |
| SIGTERM, then a second 0.5 s later, in a call | 1 | 0.004 s |
| SIGTERM, then SIGINT 0.5 s later | 1 | 0.004 s |

- Each run that exited 0 printed its round line first.
- Those that exited 1 printed none, as after a kill.
- The third case shows N3 at its source. A begin that failed on a ZFS
  read does not end the round: the round goes on to the settle's count
  and sweep before `go` is asked. With real limits of 60 s a call,
  that is minutes.

## The new dependency

`Cargo.lock` adds one package, `signal-hook` 0.3.18, used only by
`bins/skyzfs-agent`. Its dependencies, `libc` and `signal-hook-registry`
1.4.8, were already in the lock through tokio.

- **Licence:** `Apache-2.0/MIT`. Nothing in the repository checks
  licences yet (design §10's risk table puts a check in CI before a
  binary leaves the project), and a permissive pair passes any such
  check.
- **Version:** `signal-hook = "0.3"` resolves to 0.3.18. It is the
  usual crate for this, and its registry is the one tokio already
  links. The default features (`channel`, `iterator`) come along
  unused; `default-features = false` would compile a little less. Not
  worth a change.
- **Justified:** yes. A flag set by a signal, with a second signal that
  exits at once, is exactly `signal_hook::flag`'s two functions.
  Writing them by hand needs `unsafe`, which `main.rs` forbids.
- **Ground rules:** there is no `unwrap` or `expect` outside tests in
  the diff (at either head). On where the dependency is declared, see
  N5.

## Tests and plants

**My tests** (module `review167` in `program/tests.rs`, and one in
`engine/tests.rs`; `scratch-tests.patch`), each with what it shows at
16f3b3b:

| Test | Shows | 99e00ae | 16f3b3b |
|---|---|---|---|
| RA | a begin's save that failed after its rename, with A under way: B's intent is published | fails (F1) | passes |
| RB | `--every-s 1`, a count of 1.5 s: something is published | fails (F2) | passes |
| RC | that save, then a reload that fails: the round settles nothing, and both are published later | passes (other form) | passes |
| RD | a round of 250 s at 60 s: `skipped=3`, rounds plus skipped equals ticks | passes | passes |
| RE | an `Abandon` save that failed after its rename: is the line told? | fails (F5) | fails (F5) |
| RF | the record found corrupt at a restart: exit 6 after the round's line | passes (other form) | passes |
| RG | a stop is told once across rounds without a restart | passes | passes |
| RH | (engine) the memory of a publish goes when it is published | passes | passes |
| RI | a `Forget` save that failed after its rename: next round restarts, line told once | n/a | passes |
| RJ | the same, with the next restart failing: that round does nothing | n/a | passes |

- RC and RF have one form for each head, because 16f3b3b moves the
  reload after a begin's error into the same round. The forms at
  16f3b3b are in the patch.
- RE is a demonstration of F5, and fails until M2.7b.

**Plants at 16f3b3b**, each alone against
`cargo test -p skyzfs-agent --lib` with my tests in, restored after
each, with a 600 s limit (`plants-16f3b3b.out`). The baseline fails RE
only.

| Plant | Author's tests that kill it | Mine |
|---|---|---|
| N1: the begin's restart left to the next round, after the settle (16f3b3b's fix out) | `a_begin_whose_save_failed_restarts_before_the_round_settles` | RA, RC, RF |
| N2: the limit asked before the first pass (16f3b3b's fix out) | `a_round_settles_once_however_long_the_count_took` | RB |
| N3: a failed restart after a begin's error lets the round settle | **none** | RC |
| N4: a failed restart at the round's start falls through | **none** | RJ |
| N5: a settle's error does not restart | **none** | RI, RJ |
| N6: every begin's error restarts at once, refusals too | `a_begin_refused_for_eight_under_way_still_settles` | none |
| N7: a begin's error that restarts skips the settle | none | none |
| P2: only `Unmarked` ends `run` | **none** | RF |
| P3: `forget` drops the stops of publishes under way | **none** | RG |
| P4: `Forget` keeps the publish's memory | **none** | RH |
| P5: `restart` keeps the memory | none | none |
| P7: `restart` keeps the sweep done | none | none |

- N7 is the other design (F1), and only a liveness test would catch it.
- P5 and P7 are like the description's R2: a restart's clearing of
  memory is invisible to the fakes unless a test makes the memory
  matter. I would not hold the package for them.
- P3 and P4 are about the process's memory staying bounded, which the
  description claims. They survive the author's suite, and my RG and RH
  catch them.

**At 99e00ae** (`plants-99e00ae.out`), with my tests then:

- The author's A3, A4 and S1 are killed by the tests the description
  names.
- My FIX1 (a restart at once after a begin's error that restarts, then
  the settle: the same as 16f3b3b's) made RA pass. My FIX2 (a `first`
  flag in `settle_while`: the same rule as 16f3b3b's) made RB pass.
  The author's tests passed with each.
- P1 and P2 survived the author's suite then, as N3, N4 and P2 do now.

## Gates and CI

**At 99e00ae**, the whole workspace on rustc 1.100.0 (current
stable), with the target cleaned between sets. `cargo fmt --check` was
clean.

| Features | `clippy -D warnings` | `test` | `doc -D warnings` |
|---|---|---|---|
| none | clean | 1977 passed, 0 failed, 16 ignored | clean |
| `skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean | 2035 passed, 0 failed, 16 ignored | clean |
| `skyzfs-ublk-sys/control-test-program` | clean | 2001 passed, 0 failed, 18 ignored | clean |
| `skyzfs-agent-cli/test-hooks` | clean | 1977 passed, 0 failed, 16 ignored | clean |

These equal the description's figures at 99e00ae.

**At 16f3b3b** the delta touches `skyzfs-agent` alone. `cargo tree`
shows that neither `skyzfs-agent` nor `skyzfs-agent-cli` depends on a
crate behind the other two features. So I re-ran these, plain and with
`skyzfs-agent-cli/test-hooks`:

- `cargo fmt --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test -p skyzfs-agent -p skyzfs-agent-cli`: 230 passed, 0
  failed, each time.
- `RUSTDOCFLAGS=-D warnings cargo doc -p skyzfs-agent -p
  skyzfs-agent-cli --no-deps`: clean.
- The host tests twenty times over (eedd83e): no failure.

I did not re-run the other two feature sets at 16f3b3b; CI runs them.

**CI.** At 99e00ae: 42 check runs succeeded and 1 was skipped, every
rig green, including "ubuntu-26.04, daemon", which finished last. At
16f3b3b, when I wrote this: 34 succeeded, 3 were skipped, none had
failed, and 7 were still running (the daemon rig on three images, the
stand-in on two, the agent through skyzfsd, and coverage).

## The description

I read it after the code. It is accurate except where noted:

- **Codex's findings, the fixes and the two new tests.** Accurate. Each
  new test fails with its fix planted out, as the description says
  (N1, N2).
- **The plant table is the one from 99e00ae.** At 16f3b3b, A4 ("no
  restart after a failed round", killed by `a_failed_round_…`) holds
  for the begin's path only. The settle's path now survives (G1, N5).
- **"Agent::restart drops ... reports made, ... kill points reached".**
  Near enough, but see N1.
- **The `abandoned` window.** "A kill between that save and the line
  loses the line": a failed save does too (F5).
- **"The plant it was there for ... still fails it"** (eedd83e). Right,
  as N8 says. "It failed 4 times in 5 locally": I did not see it fail
  in the four workspace runs at 99e00ae, nor in twenty runs of the host
  tests at 16f3b3b (Gates). A race, so not a contradiction.
- **The gates table at 16f3b3b** (1979, 2037, 2003 and 1979 passed):
  consistent with mine at 99e00ae plus the two new tests.
- **"The binary's signal handling is not tested here."** True, and
  asking testing to drive `run` in the agent rig is right. I drove the signals by hand (above): they behave as described.
