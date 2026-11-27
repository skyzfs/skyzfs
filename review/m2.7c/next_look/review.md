# Next look at skyzfs/skyzfs#173 (M2.7c) at 3b93a83 and e3fe645

**The head is `e3fe645902f83c8dd3d240015c9c4ba5dcc558ab`.** I checked
it with `gh api repos/skyzfs/skyzfs/pulls/173 --jq .head.sha` at the
end. The base is `zfs-topology` at 6b16935.

This look began at 3b93a83. The head moved to e3fe645 during it, and
the second half of this note judges that change too. The first review
(comment 7208246215, `../review.md`) was at 29f5538.

| Commit | What | Judged here |
|---|---|---|
| 89eeb5d | Merge of `zfs-topology` (26ac18e). Docs only: one line of the design and two of the notes | No: nothing of #173 |
| 19cc2de | #172's L3 with T2 and n1 to n6, moved here by the lead | Five plants, and where it meets the watch (D2). Its substance is the #172 review's |
| 3b93a83 | The answer to the first review: F1 to F5 as the lead ruled, F3, the error paths' tests, A-P5, the nits, a reopen ended by a signal | Yes |
| 8e0710a | Merge of `zfs-topology` (6b16935): #171, #174, the notes. Testing's rig files and docs; no Rust | No: nothing of #173 |
| e3fe645 | Codex's P1 (the watch asked once more before a begin) and P2 (a note that no client held touches no pool), with a test each | Yes |

`git diff --stat 3b93a83 e3fe645 -- '*.rs' '*.toml' Cargo.lock` is
`program.rs` and `program/tests.rs` only. So what I found at 3b93a83
holds at e3fe645, except where this note says it changed.

**Method.**
1. I read `git diff 29f5538 3b93a83` and the code it touches, then
   `git show e3fe645`.
2. I took the author's answer (comment 7209392349), the replies on
   Codex's threads and the description claim by claim.
3. I ran the gates, the plants and four scratch tests.

Paths are under `crates/skyzfs-agent/src/` unless another crate is
named. Line numbers are e3fe645's unless marked.

**Files beside this one:**
- `gates-3b93a83.sh` with `gates-3b93a83.out`, and `gates-e3fe645.sh`
  with `gates-e3fe645.out`;
- `plants-3b93a83.py` with `plants-3b93a83.out`, and `plants-e3fe645.py`
  with `plants-e3fe645.out`;
- `scratch-tests.rs`, holding N1 to N4. Each run appends it to
  `program/tests.rs` in a scratch tree. The runs are in
  `scratch-tests-3b93a83.out`, `scratch-tests-3b93a83-n4.out`,
  `scratch-tests-e3fe645.out` and `n4-under-w12-e3fe645.out`;
- `n2-fix.diff` with `n2-fix.out` (3b93a83). This was my fix for D2;
  e3fe645's fix differs and also passes N2;
- `d1-alt-e3fe645.diff` with `d1-alt-e3fe645.out`, an option for D1.

## Verdict at e3fe645

**It can merge from review's side. Nothing blocks.**

- **F1 to F5 are fixed** as the lead ruled. Each one's failing scenario
  now passes (table below).
- **Codex's two findings are fixed in e3fe645,** and each fix has a test
  that fails without it:
  - P1 is my D2: news just before a begin after syncs. The plant E1
    removes the extra ask; `a_volume_lost_during_the_syncs_begins_no_publish`
    fails.
  - P2 is my D3: a note that no client held went on to the begin and
    the settle. The plants E2 and E2b remove or narrow the gate;
    `a_volume_failed_with_no_client_touches_no_pool` fails.
  - My N2, which failed at 3b93a83, passes at e3fe645.

What is left, all low:

- **D1: a decision for the lead.** It is unchanged at e3fe645 and
  certain on the fake. A step begun early by the watch that does not
  take is retried two ticks after the failure: about 115 s at the
  default `--every-s 60`. A two-line option gives about 55 s and fails
  no test. How often a first step fails to take on a kernel is not
  known, and the daemon's order makes it unlikely.
- **Readings for the lead.** Retries have no end and no backoff. A ZFS
  command in flight is not cut. An unreadable note gives no step.
- **Test gaps.** At e3fe645, 34 of 40 plants fail a test, and the
  six that survive are W12, P17, P18, P19, P20 and X10 (see "Test
  gaps"). Three are worth a test:
  - W12: nothing pins the rounds after an early retry. My N4 fails
    under it and passes at the head.
  - P17: a lost round is not checked for ZFS calls.
  - P19 and P20: no program test drives a stop of the syncs by news.

  X10 is the author's own acknowledged gap, and P18 is near
  equivalent.
- **Nits:** n1 to n4, docs and the description.

**CI at e3fe645** (04:43 UTC): 41 jobs succeeded, 4 were skipped
and none failed. Ten were still running: both replay jobs, the replay
rig's two daemon jobs, the daemon rig's five jobs and the agent rig's
k. Codex had posted nothing on e3fe645 by then. At 3b93a83 one job had
failed, "the check's logic (no kernel)" of the agent rig. It is
testing's stand-in in `tests/kernel/test_agent_record.sh` (`runtwice`,
a signal-handler race in its fake agent), which this pull request
does not touch, and the same job passed at e3fe645. It is a flake for
testing, not a verdict on #173 (see "CI").

## What changed between the two heads

| | 3b93a83 | e3fe645 |
|---|---|---|
| News during `sync_past`'s last sync or its `cuts` ask, then a begin | The begin ran (N2: `begun=skz-… under_way=1 synced=3`, the step a round later) | No begin: `if !restart && clear && !agent.note_came()` (`program.rs:1479`); the settle ends at its first pass, and the next round takes the step at once. N2 passes |
| A note that no client held | The round went on to the restart, the syncs, the begin and the settle, which touch `--pool` before they reach the daemon | The round touches nothing else: `!unheld` joins the gate (`program.rs:1408`, `:1416`), and the round is ok |
| The rest | — | Unchanged. The tick logic, the verdicts, the watch and the step are byte for byte as at 3b93a83 |

**e3fe645's two changes are right.**
- **The extra ask.** It comes after `go` wherever `go` is asked:
  `clear` is `k == 0 || go()`, and the watch is asked only once
  `clear` is true. So the signal rule holds. With `k == 0`, the watch's
  first read is due a second after the round's own read. So a usual
  round makes no extra read, unless its restart and its `cuts` ask took
  over a second, and then the extra read is of one small file.
- **The unheld gate.** Its effects are these:
  - It leaves a pending `restart` for the first round with no note.
  - It holds back the `cuts` ask that would see a new attachment. Once
    a start with `--acknowledge-failure` removes the note, the next
    round asks it and ends with 12, as the docs say.
  - The watch's `seen` is the unheld note, so a lost note that follows
    is news.
- **The tests.** The unheld test now asserts no ZFS call and no
  program. The P1 test fails the volume just after the third sync and
  checks that no record was written.

## F1 to F5, each retried

| Finding | Ruling or claim | At e3fe645 | Retried |
|---|---|---|---|
| F1: the note read only at a round's start | Watched about once a second between rounds and inside the syncs and the settle | **Fixed**, and since e3fe645 before a begin too. A step under way is not cut ("Readings") | The first review's scenarios are now tests: lost while `run` waits, reopened 1.25 s after the failure (`a_volume_lost_while_run_waits_is_stepped_on_within_seconds`); lost while a publish settles, within 2 s (`a_volume_lost_while_a_publish_settles_is_stepped_on_within_seconds`). W1, W2, W3, W3b, W3c and W15 each fail them or the engine's own tests, and E1 fails the new one |
| F2: exit 9 whatever the step showed | `run` ends LOST only on SUSPENDED; otherwise the round fails and the next retries | **Fixed** (`Step::verdict`, `engine/lost.rs:175-189`; the end on `Verdict::Lost` in `schedule`) | A pool not shown suspended: `verdict=retry`, the round failed, the next round reopens again (`a_pool_not_shown_suspended_is_tried_again_at_the_next_round`). Each V plant and P1 to P4 fails a test |
| F3: a failed look hid the reopen | A look error after the reopen keeps the reopen's outcome (`unread`) | **Fixed** (`lose`; `state=unread` in the line) | `a_look_that_fails_after_the_reopen_keeps_the_reopen`; X1 to X3 |
| F4: not imported with held=unknown ended `run` | Not imported is `verdict=failed`, and `run` goes on | **Fixed** | `a_lost_note_with_no_pool_imported_is_reported_failed_and_run_goes_on`; V1, V2, V7, V7b |
| F5: 9 and 10 are `skyzfsd`'s | 11 LOST and 12 MOVED | **Fixed**. The docs, the usage text and the tests agree. `git grep` finds no other 9 or 10 for the agent; every other one in the tree is `skyzfsd`'s. One stale test doc (n1) | `the_codes_of_a_lost_volume_and_a_move_are_not_the_daemons`; P13, P14 |

**The author's two readings.** A round on a lost note takes only the
step; a round whose note cannot be read touches nothing. I agree with
both. Two points for the lead:

- **A lost round takes only the step,** and so does each retry round.
  A publish under way therefore stays in the record for the next boot.
  That is right for a pool that only writes.
- **An unreadable note** fails the round and gives no step, while the
  daemon's start counts an unreadable note as failed while held. An
  agent that cannot read the note therefore leaves a pool that may be
  lost ONLINE, round after round. A read error on a small file in the
  daemon's directory, while `run` itself goes on, is unlikely. Taking
  the step on such a note would be the conservative reading.

## Defects, most severe first

### D2. News just before a begin after syncs did not stop the begin (low; certain; Codex P1) — fixed in e3fe645

**At 3b93a83:**
- **Where.** `program.rs:1428` and `:1437` (3b93a83) read
  `clear = k == 0 || go();`. The module docs said the watch stops the
  engine "before a begin after syncs". `sync_past` asks the watch
  before each sync (`engine.rs:881-883`), not after the last.
- **N2** fails there. After a rewind import's three syncs, the note
  comes 1.5 s before the begin. Round 1 prints
  `begun=skz-20261125T010001Z under_way=1 synced=3`, and the step comes
  a round later with an Intent left under way.

**At e3fe645.** `program.rs:1479` asks the watch before every begin. N2
passes, and E1 shows that the author's own test pins it. My fix
(`n2-fix.diff`, both arms) did the same for `k > 0` only. The author's
fix covers more and costs nothing more.

### D1. A step that news began early, and that did not take, is retried two ticks after the failure (low; certain on the fake; a decision for the lead) — open at e3fe645

**Where.** `program.rs:1552`. After a round, `tick += passed.max(1)`.
A round that the watch began early takes the slot of the tick `run` was
waiting for, so the round after it comes at the tick after that. The
docs say so ("the tick after it is the next one due",
`program.rs:1554-1555`), and the description says "the round keeps its
tick's slot".

**Failing scenario** (N1; it fails at 3b93a83 and at e3fe645):
1. `--every-s 60`, an idle pool, and the volume fails at +5 s.
2. The watch sees it, and round 2 begins at about +6 s.
3. The reopen leaves the pool ONLINE: `verdict=retry`, and the round
   fails.
4. Round 3 comes at the 120 s tick. Its reopen is **115 000 ms after
   the failure**, and the pool suspends.

A step taken on time and retried waits one tick, 60 s. Either way, the
criterion's "within seconds" is lost once the first step does not take.

**When the first step may not take.** The watch reads the note within
about a second. The daemon deletes the device up to about 2 s after it
writes the note (`bins/skyzfsd/src/serve.rs:1653-1701`: note, then a
wait of up to 200 × 10 ms for its queue's thread, then the delete). So
an early step's reopen can open a device that is still there. In that
window the queue's thread answers what it holds with an I/O error, so
such a reopen most likely fails its reads and the pool suspends.

A first step that does not take needs something else. One case is a
daemon killed between the note and the delete: its device then waits
for a server and holds the reopen's reads. The reopen meets its 10 s
limit, and the look reads ONLINE. Neither case has been seen on a
kernel. The rig's planned course (Q6) would show the first.

**Options for the lead:**
1. **Keep it.** The docs already describe it.
2. **Let an early round not take the slot** (`d1-alt-e3fe645.diff`):
   ```rust
   let early = began < due;
   tick += if early { passed } else { passed.max(1) };
   ```
   With it, the retry in N1 comes 55 000 ms after the failure, at the
   slot `run` was waiting for. All 306 tests pass: the head's 302 and
   N1 to N4 (`d1-alt-e3fe645.out`).
3. **A short retry after a `retry` verdict,** such as a few seconds.
   Each retry is bounded: at most 10 s of reopen plus `LOOK_MAX` (10 s)
   of looks. `STUCK_MAX` (`host.rs:54`) holds hung reopens to four, and
   after that the reopen is refused without starting a child. So a
   short retry cannot make more than about one reopen per 10 s.

### D3. A note that no client held did not stop the round's begin and settle (low; Codex P2; predates 3b93a83) — fixed in e3fe645

**At 3b93a83.** A round on such a note went on to the restart, the
syncs, the begin and the settle. `begin` runs the count, the sweep and
the properties on `--pool` before it asks the daemon
(`engine.rs:912-916`), and each settle pass opens with the count and
the sweep. Meanwhile the docs and the line on standard error said "no
pool is touched".

The effect was small. A held=no note means no client had the device
open, so the pool on the volume is not imported and those calls fail.
My first review's row "held = No touches no pool … Yes"
(`../review.md:361`) was wrong in letter, and I missed this.

**At e3fe645** the gate holds it back. E2 and E2b each fail the
strengthened test, which now asserts no ZFS call at all.

## Readings for the lead (not defects)

- **A ZFS command in flight is not cut.** The watch is asked between
  steps, so a step that is running when the note comes runs to its
  end. The docs say this: "a second plus that step".
  - Examples are a sync of `sync_past`, a publish's assist, a hold or
    a snapshot, and a sweep's release.
  - On a pool that only writes, each of those is a change that waits
    for its limit (120 s base). So the step can come about two minutes
    after the failure when the failure lands inside such a call.
  - This is read from the design and from CLAUDE.md ("A command on a
    suspended pool may never return"), not run.
  - The Q6 course should inject the failure while a publish waits for
    its cut, and not only while `run` is idle.
- **Retries have no end and no backoff.** N3 is a record, not a verdict.
  A pool that never reads SUSPENDED after the reopen is reopened at
  every round for as long as `run` runs:
  - `--every-s 60`: 10 rounds in 10 minutes, all failed, 10 reopens;
  - `--every-s 1`: 60 rounds, 60 reopens, one about every 10 s.

  `LOOK_MAX` and `STUCK_MAX` bound the rate, so this is not hammering.
  But `zpool reopen` is asked without `-n`, so each one starts a scrub
  under way again (`zfs.rs`, the reopen's docs). Two cases fit: a
  wrong `--pool`, and a layout the premise would refuse that reads
  DEGRADED. A cap on retries, or a backoff, is the lead's call.
- **The watch does not break the rest of the round.**
  - `Stuck::Noted` is asked after `NoCut` and `Bound`
    (`engine.rs:872-883`). The bound's accounting is unchanged, and
    `bound` stays false on a `Noted` stop (P19).
  - `synced=` counts the syncs that returned (P20). `last_synced` is
    unchanged.
  - The signal rule holds. `go` is asked before the watch in the syncs,
    the settle, the wait and now the begin. A signal that comes with
    news ends `run` with 0 at the next round's `go`, before any step
    (W3c fails a test).

## Test gaps (surviving plants)

Each plant was applied alone to the head and run against `cargo test
-p skyzfs-agent --lib` with a 600 s limit, then restored. The runs
were:
- **At 3b93a83** (`plants-3b93a83.out`): 41 plants (the watch W, the
  verdict V, the round P1 to P5c). 40 failed a test, and W12 survived.
  The run was stopped when the head moved.
- **At e3fe645** (`plants-e3fe645.out`): 40 plants. These are e3fe645's
  E1 to E2c, the gate's P5 to P6 on the new gate, the rest of the P and
  X set, W12 again, and L1 to L5 on 19cc2de. 34 failed a test.

| Plant (e3fe645) | What it changes | Why it survives | A test that would catch it |
|---|---|---|---|
| W12 | `watch_note` only at round 1 | After an early retry the watch keeps round 1's `seen` and its sticky news, so the rounds that follow run back to back. N4 shows 30 reopens in 300 s, where the head makes 4 (`n4-under-w12-e3fe645.out`) | N4 (`scratch-tests.rs`). It also holds under D1's option 2 (5 reopens) |
| P17 | A settle before the step in a lost round | The lost-round tests count reopens and programs, not ZFS calls, so a settle's count and sweep go unseen | In `run_on_a_lost_volume_makes_the_pool_read_and_ends_with_11`, assert that the only ZFS call is the reopen, as the unheld test now asserts none |
| P18 | A restart before the step in a lost round | A restart reloads the record and makes no ZFS call: near equivalent | None needed |
| P19, P20 | A stop of the syncs by news reported as `bound=1`, or as `synced=0` | No program test drives a `Stuck::Noted` stop. The new P1 test fails the volume after the last sync, which ends `Clear` | A round whose note comes before the second of several syncs: its line reads `synced=1 bound=0`, and the next round takes the step |
| X10 | A pool gone during the look does not end it | The author's acknowledged gap (R-L1): the fake has no hook to export the pool during the look | As the description says |

Each of these fails a test:
- e3fe645's own plants E1, E2, E2b and E2c;
- the gate's P5, P5b, P5c and P6;
- P7 to P16 and P21;
- X1 to X9 and X11;
- the plants on 19cc2de, L1 to L5.

In particular, E1 fails only `a_volume_lost_during_the_syncs_begins_no_publish`,
and E2, E2b and E2c each fail only
`a_volume_failed_with_no_client_touches_no_pool`. So each fix rests on
its one test, which does catch it.

## Nits (at e3fe645)

- **n1.** `program/tests.rs:2949` still lists "an end with 9" among
  the mutants; it is 11 now.
- **n2.** A round that news began early prints `late_ms=0` and nothing
  that marks it early. In N1, round 2 began 54 s before its slot. A
  field such as `early_ms=` would let the rig see the watch at work.
- **n3.** `engine/lost.rs:58-60` says a wrong pool "is made to read
  once". It is reopened every round ("Readings").
- **n4.** The description is behind e3fe645 in three places:
  - Q2 says "The round goes on, and fails at the daemon, which is
    gone". It now touches nothing and is ok.
  - "Inside a round the syncs … and the settle stop on it": the begin
    now does too.
  - A wrong `--pool` "only makes it read": it is every round.

## Gates

All on rustc 1.100.0, the current stable after `rustup update stable`.

**At 3b93a83** (`gates-3b93a83.sh`, `gates-3b93a83.out`; stopped when
the head moved):
- `cargo fmt --check`: clean.
- Workspace, plain: clippy `-D warnings` clean; tests 2066 passed, 0
  failed, 16 ignored; `RUSTDOCFLAGS=-D warnings cargo doc` clean.
- The agent's, the CLI's and the control crate's tests, 20 times: no
  failed run.
- Clippy with `skyzfsd/test-hooks,skyzfs-local/test-hooks`: clean.

**At e3fe645** (`gates-e3fe645.sh`, `gates-e3fe645.out`). The Rust
delta is `program.rs` and its tests, so these runs cover the agent's
crates and the workspace's tests:
- `cargo fmt --check`: clean.
- Clippy `-D warnings` on `skyzfs-agent` and `skyzfs-agent-cli`, plain
  and with `skyzfs-agent-cli/test-hooks`: clean.
- Their tests and the control crate's: 333 passed, 0 failed, each way.
- `cargo doc` with `-D warnings`, each way: clean.
- The agent's lib tests, 10 times: no failed run (302 tests).
- The workspace's tests, plain: 2067 passed, 0 failed, 16 ignored (one
  more than at 3b93a83: the new test).

**Not run locally at e3fe645:** the workspace clippy with
`skyzfsd/test-hooks` and with `skyzfs-ublk-sys/control-test-program`.
No crate they build changed since 3b93a83, and CI's `test` and
`unit-arm64` passed at e3fe645.

## CI

| Head | When | Succeeded | Skipped | Failed | Cancelled | Running |
|---|---|---|---|---|---|---|
| e3fe645 | 04:43 UTC | 41 | 4 | 0 | 0 | 10: replay (two layouts), the replay rig through skyzfsd (24.04, 26.04), the daemon rig (24.04 and 26.04 daemon and stand-in, 26.04-arm daemon), the agent rig's k |
| 3b93a83 | at the end | 40 | 4 | 1 | 6 (by the push of e3fe645) | 0 |

**The failure at 3b93a83** is in run 45937023617, job 140302470174,
"the check's logic (no kernel)". The job prints:

```
FAIL runtwice: two signals of two kinds; the second ends it 1:
  got 'rc=1 rounds=none sig=SIGINT, ended=after verdicts='
  want 'rc=1 rounds=none sig=SIGINT,SIGTERM, ended=after verdicts='
```

That is the Python stand-in agent of `tests/kernel/test_agent_record.sh`,
not `skyzfs-agent`. The SIGINT handler runs nested inside the SIGTERM
handler, after the SIGTERM's append and before its write to the file,
and its `sys.exit(1)` unwinds before SIGTERM is recorded. #173 touches
no file under `tests/` or `.github/` (`git diff --stat 26ac18e 3b93a83
-- tests/ .github/` is empty), and the same job passed at e3fe645.
It is for testing.

The rigs that drive the agent on a kernel passed at e3fe645: the agent
rig's a1, a2, d and r, the publish rig and the record rigs. None of
them has a failed-volume course yet (Q6).
