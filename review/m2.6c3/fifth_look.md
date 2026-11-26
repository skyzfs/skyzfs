# Fifth look at #149 (M2.6c-3, the program `skyzfs-agent`) at 6ec5909

- **Head judged:** `6ec590973ca4adad241c1d1fec690a178b77ebd9`, read from the pull request before and after the work; it did not move. **Base:** `zfs-topology` at `f8e5e0c` (the pull request's base sha). `zfs-topology` is now at `3d8196b`, and the head merges with it cleanly (`git merge-tree`).
- **Previous look:** `5259bf5` (`fourth_look.md` here: finding C1, probes D1 to D3, fix probe F3).
- **The author's answer:** https://github.com/skyzfs/skyzfs/pull/149#issuecomment-7179233241
- **Diff reviewed:** `git diff 5259bf5 6ec5909`, one commit (`6ec5909`, "agent: send a publish request within a slack of its record's time, or not at all"). Three files, +246/-29:
  - `crates/skyzfs-agent/src/engine.rs`: the slack in the asking deadline, the check before the send, and the docs;
  - `crates/skyzfs-agent/src/plan.rs`: `ASKING_SLACK`, and the doc of `Seen::withdrawn` (nit 1);
  - `crates/skyzfs-agent/src/engine/tests.rs`: `pending_now` factored out, the N6 test's course (T1), and the C1 test.
- I formed my view from the diff before I read the answer and Codex's thread.

## Verdict at 6ec5909

**Nothing in `6ec5909` blocks the merge.** C1 is fixed as suggested, and the fix is right.

- **The fix holds up.**
  - The plants behind it are killed, and so are three more of mine.
  - Stops and wall-clock steps at every point of the course find nothing wrong (25,008 runs). Neither do restarts after a round that sent nothing (324 runs) or a save that is always slow.
  - The bound the fix gives is worked out below.
- **The rest of the answer checks out.** T1 and nit 1 are done, the correction about the recorded release is right, and Codex's thread is answered and resolved.
- **The gates pass**, and the earlier searches pass again at this head.
- **CI on the head is green:** all ten runs on `6ec5909` completed with success, the daemon rig at 01:45 UTC (see "CI on the head").
- **One new finding, F1.** It is outside this commit and older than this pull request.
  - The hold's budget rests on the premise that a request is accepted within `ASK_LIMIT` of its send. That fails when the agent's connection waits in skyzfsd's backlog: skyzfsd accepts and runs a queued publish whose client has already given up.
  - The agent alone cannot close this. The fix belongs to skyzfsd or the protocol.
  - I recommend a follow-up; the lead decides. Two comments in this pull request state the premise without its condition and should say it (nit 4).
- **The test gaps (T3 to T5), suggestion S10 and the doc nits do not block.**

## C1: the fix, judged

`Agent::ask` (`engine.rs:1121-1226`) at this head:
1. reads `sent` and `sent_mono` (`:1155-1156`);
2. records `asking` with `held_until = held_until(sent + ASKING_SLACK + ASK_LIMIT, expiry)`, all saturating (`:1157-1171`);
3. anchors the hold on `(sent, sent_mono)` and clears `withdrawn` (`:1177-1179`);
4. runs `point(Ask, false)` (`:1180`);
5. sends nothing, returning `Moved::No`, if `mono − sent_mono > ASKING_SLACK` (`:1186-1188`);
6. sends (`:1189`).

This is F3, with the slack at 10 s (`plan.rs:270`).

### (a) The course checks

The C1 test, `a_request_is_sent_within_the_slack_of_its_record_or_not_at_all` (`engine/tests.rs:3261-3300`), runs 144 rows:
- the first save takes 0, 10, 11, 41, 81 or 200 s;
- three modes, Idle, Abandoned and Busy;
- the answer lost as `Err` or as `no-answer`;
- a person's give-up or none, crossed with uploads that end or are stuck.

Every row asserts:
- no release while a request pends;
- no wrong publish;
- no hold left;
- the span, `held_until − asked == SLACK + ASK_LIMIT + EXPIRY_DEFAULT + HOLD_MARGIN`;
- a course:
  - a save within the slack sends at least one request;
  - past the slack with a give-up, nothing is sent;
  - otherwise at least one is sent, and there are more askings than sends.

| What a check pins | Pinned? | Evidence |
|---|---|---|
| The slack in the deadline | Yes, in every row | C1b (slack left out) KILLED by all 144 rows, through the span check |
| A check at all | Yes | C1a (no check) and C1h (operands swapped) KILLED by all 96 rows past the slack, through the course. On the fake only the 12 stuck rows at 200 s release early: the fake accepts at the send, and with the slack in the deadline its threshold is 90 s, so the rows at 81 s do not (nit 7) |
| Measured from the stamp, not from after the save | Yes | C1f (`sent_mono` read after the save) KILLED by all 96 rows past the slack |
| The threshold, from above | Yes | C1c (twice the slack) KILLED by the 24 rows at 11 s |
| **The boundary at exactly the slack** (`>`, not `>=`) | **By half the rows** | C1d (`>=`) KILLED by the 12 give-up rows at 10 s only. The other 12 rows at 10 s ask anew in the next round and send, so `sent ≥ 1` holds either way (T4) |
| The steady clock, not the wall clock | **No** | C1e (the check on the wall clock, from the record's `sent`) SURVIVES: no row steps the wall clock (T3) |
| The check after `point(Ask, false)` | Not needed | C1g SURVIVES. It is equivalent in production, where `point` is a hook of the tests and the rigs |

Probe E6 tightens the test in two ways. Within the slack it asserts `askings == sent`. It also adds rows where the wall clock steps back by the save's length.
- At the head: 288 rows, 0 failing.
- Under C1d: 48 failing, which is every row at 10 s, with and without the step.
- Under C1e: 96 failing, which is every step-back row past the slack.

So T3 and T4 are each closed by a few lines.

### (b) The gap between the check and the send: the bound

**The terms**, from the stamp `sent`:
- `held_until = sent + S + A + E + M`. Here S = `ASKING_SLACK` = 10 s, A = `ASK_LIMIT` = 40 s, M = `HOLD_MARGIN` = 40 s, and E is the expiry. So `held_until = sent + 90 s + E`.
- `s` is the time from the stamp to the check: the save, or a stop. `s ≤ 10 s`, or nothing is sent.
- `T` is the time from the check to skyzfsd's `accept`. It is made of:
  - the instructions up to `deadline()` in `skyzfs_control::ask` (`client.rs:40`), where the client's clock starts;
  - the connect, retried on `EAGAIN` while the backlog is full;
  - the wait in the listener's backlog. skyzfsd's channel serves one connection at a time (`serve`, `bins/skyzfsd/src/control/server.rs:264-317`).
- `R` is the read of the request after `accept`, at most `REQUEST_LIMIT` (10 s; `until`, `server.rs:293`).
- The handler then waits at most `ANSWER_LIMIT` (30 s) from its own start (`began`, `control/handler.rs:84`). The uploader drops an ask whose handler has stopped waiting (`skyzfs-upload/src/uploader.rs:411`).

**The bound.** Acceptance comes at the latest at `sent + s + T + R + 30`, and the request expires E after that. So:

`held_until − latest expiry = 90 − (s + T + R + 30) = 60 − s − T − R ≥ 40 − T` (with `s, R ≤ 10`).

- **With a free channel and the send right after the check** (`T ≈ 0`), the margin is at least 40 s, so `HOLD_MARGIN` stays whole, as the author says. With `s` and `R` small it is up to 60 s.
- **On the fake**, which accepts at the send (`R + 30 → 0`), the margin is `90 − s − X`, where X is a delay after the check.
  - Probe E3 delays the send by X after the check. The first early release is exactly where `s + X > 90`: save 0 s with X = 91 s, and save 10 s with X = 81 s.
  - These are the 12 stuck rows of 24, with no wrong publish.
- **A stop after the client's clock has started does not matter.** Every write and read waits only for what is left (`Deadline::left`, `client.rs:80`). Past the deadline the write fails with `TimedOut`, and a partial request is never handed on: the server's read fails at EOF.

**`T` grows in two ways:**
1. **A stop of the agent between the check (`:1186`) and `deadline()`.** The window is a few instructions; any check on a clock followed by an act leaves one. The agent cannot close it; only a deadline carried in the request can (F1, fix b). Against skyzfsd it needs a stop of over 40 to 60 s in that window. **I accept it as a residual and suggest documenting it** (nit 3).
2. **skyzfsd's backlog:** F1 below.

**Clock jumps and stops at every point.** Probe E2 runs on the fake clock, whose steady clock does not step:
- the agent stopped for 5, 11, 41, 51, 91 or 700 s;
- the wall clock stepped by ±2 h;
- both together;
- applied at every point of the course up to the 120th, then at every 7th.

That is 25,008 runs with 0 releases while pending and 0 wrong publishes. Under C1e there are 90 bad runs (a stop of 91 s or more plus a step back, at the `Asking` and `Ask` points); under C1a, 180.

Probe E1 widens D3 to saves of 0 to 1000 s: 4,212 runs, 0 early, 0 wrong.

**A restart after a round that sent nothing (probe E4, 324 runs, 0 bad).**
- The first round's save takes 11, 41 or 200 s, so nothing is sent.
- The agent is then dropped with the record `asked`, and a new agent starts. Its first rounds are one of:
  - plain;
  - the daemon away;
  - the wall clock 2 h ahead;
  - the wall clock 2 h back;
  - a person's give-up;
  - a take-over.
- **162 runs ask anew**, with a fresh `asked` and a fresh `held_until`, and record 0 attempts and 0 stalls.
- **The other 162 settle without asking:**
  - away: `Abandoned(Destroyed)`, released at `held_until` with nothing pending;
  - give-up: `Abandoned(Person)`;
  - 2 h ahead: `Abandoned(TooOld)`.
- No run leaves a hold or a record, and none is left unsettled within 6 h.
- The next round reads `progress` `unknown` and asks afresh, as the answer says.

**Does a round that sent nothing count as an attempt or a give-up?**
- **Attempts:** no. `attempts` goes up only at `accepted` (the `Asked` commit after the send).
- **Stalls:** yes, where the next ask anew has no cut counted since its count. `ask`'s doc says so (`engine.rs:1110-1120`).
- **Every save slow** (probe E5):
  - Idle and Abandoned end `Abandoned(NoProgress)` after 9 askings;
  - Busy ends `Abandoned(TooOld)` after 600 askings (saves of 11 s) or 119 (saves of 60 s);
  - with a give-up, `Abandoned(Person)`.
  - Nothing is ever sent, and no hold or record is left.
- The planner's own bounds end the publish. What the person is told is `NoProgress`, which names the daemon and not the slow save (S10).

### (c) The deadline arithmetic

- **Saturating throughout:** `sent.saturating_add(..).saturating_add(..)`, `plan::held_until` (`plan.rs:424`), `ms()` (`engine.rs:1469`) and the check's `saturating_sub`.
- **The check is on the steady clock,** from `sent_mono`, which is read next to `sent`. A wall step anywhere leaves it unchanged (E2, E6).
- **The hold is timed on the same steady clock** through N5's anchor. `now_for` reads the time as `asked_ms + (mono − sent_mono)`, so the wall-clock deadline and the steady-clock check agree.
- **After a restart** the anchor is gone and the wall clock is read again. That is N5's documented residual; E4's steps of ±2 h after a restart find nothing wrong.

## F1 (new; older than this pull request, in skyzfsd's channel; for the lead): a queued request runs after its client has given up

**Where:**
- `bins/skyzfsd/src/control/server.rs:264-317`: one connection at a time. The request's clock starts at `accept` (`:293`), and nothing asks whether the client is still there.
- `control/handler.rs:84`: the wait begins when the handler starts.
- `skyzfs-upload/src/uploader.rs:411`: an ask is dropped only if the handler's reply is closed.

**The premise that fails.** Three places say that a request sent within the slack is accepted, if at all, by `ASK_LIMIT` after:
- `engine.rs:1149-1154` (the comment above the stamp);
- `plan.rs:262-269` (the doc of `ASKING_SLACK`);
- the reasoning of N5 and C1 in my earlier looks, which leaned on `uploader.rs:411`.

That holds only while skyzfsd accepts the connection promptly. Linux keeps a connected Unix-stream socket in the listener's backlog with the data written to it, and keeps it after its client has closed. The server then accepts it, reads the whole request, and only its answer fails.

**Evidence** (section 6 and 7 of `fifth149_scratch.out`):
- **The socket alone** (a Python probe): the client writes a request, times out and closes. The server accepts afterwards, reads all 5,036 bytes, and gets `EPIPE` on the answer.
- **G1, on skyzfsd's real `Channel`** (a scratch test in `control/tests.rs`, `mod socket`, with the limits scaled down):
  - a publish is queued behind a connection that holds the thread;
  - its client gives up after 531 ms;
  - the publish is handed to the handler 2.0 s after its send, 1.47 s after its client gave up.

**The scenario.** It needs every step of this chain:
1. skyzfsd's channel is busy with connections ahead of the agent's publish. Each can hold it up to `REQUEST_LIMIT + ANSWER_LIMIT` (40 s): `look`s or publishes waiting on a slow object store, or other clients. Alternatively, the channel's thread is held some other way and then resumes.
2. The agent's client times out, so the answer is lost. The record stays `asked`, with `held_until = sent + 90 s + E`.
3. The uploader, free at that moment, accepts the queued publish more than 90 s after the stamp, and the expiry counts from then. This needs skyzfsd to have accepted the connection more than `60 − R` s after the stamp (50 s at the least, with a read of 10 s).
4. Nothing tells the agent `pending` until `held_until`: its later asks queue and time out too, or the daemon is away. It releases the hold, either by its own course or by a person's give-up (silent).
5. A person destroys `pool@t`, and the uploader carries the request on a cut after the destroy.

**Certainty.**
- The premise's failure is certain: it is shown on skyzfsd's own channel.
- The whole chain is not reproduced, and it needs an odd combination: the channel congested from the late acceptance to `held_until` (the expiry, 10 minutes by default) while the uploader answers.
- It is narrower than C1:
  - a skyzfsd that is wholly stopped also stops the pool's I/O, so a destroy cannot complete meanwhile;
  - a request larger than the socket's buffer cannot be queued whole, and a partial one is never handed on.
- **Severity:** medium-low.
- **It does not block #149.** This commit does not change it, and the agent alone can close it only by never releasing on silence, which would undo the lead's Q8.

**Fix (a follow-up work package in skyzfsd; the lead's choice):**
- **(a) skyzfsd drops a request whose client has hung up.** After reading it and before handing it on, poll the stream for `POLLRDHUP`/`POLLHUP`, then drop and count it.
  - This is small and needs no change to the protocol.
  - It covers a client that gave up and closed, which is what the agent's client does at its deadline.
- **(b) The request carries the time by which it must be taken.** `CLOCK_MONOTONIC` (or `CLOCK_BOOTTIME`) is shared by the processes of one node, and the agent and the daemon are on one node.
  - The daemon refuses a request past that time.
  - This is a versioned change to the protocol.
  - It also closes the residual window between the agent's check and the client's clock.

Either way, the comment at `engine.rs:1151-1154` and the doc of `ASKING_SLACK` should state the condition (nit 4).

## The rest of the change

- **T1 is done.** The N6 test asserts its course:
  - The assertion is `lost && holds empty && (!moved || withdraws ≥ 1 && publishes ≥ 2)` (`engine/tests.rs:3079-3080`).
  - The 8 rows pass, and N6a (the clearing removed) is still KILLED by exactly the 4 `moved` rows.
  - **T1a** (the agent no longer withdraws a pending request when the pool has moved) is KILLED by the N6 test and two tests of the plan, but through `recover(..).expect` in `n6_run` and not through the new check: with no withdraw, the publish ends before the loss.
  - I found no plant in the product that reaches the course check alone. It is defence in depth, and that is fine.
- **Nit 1 is done.** `Seen::withdrawn` now says "since a request of it was last asked".
- **The correction about the recorded release is right.** The recordings release a hold on the child alone. #153 at `0a19e1c` now says so in the doc of `a_hold_on_the_root_alone_as_recorded`. O1 of the last look is settled; the test's name still says "root alone", and its doc now explains it.
- **`pending_now` is the strict check, factored out** (`engine/tests.rs:2979`), and used by the N5, N6 and C1 tests.

## Codex's threads

Read with `gh api`, read-only, including the review threads' route.

| Thread | State | Checked against the code at `6ec5909` |
|---|---|---|
| P1, "Keep the hold when recovery receives an inconclusive answer" (`plan.rs`) | resolved | B1a and B1f KILLED at this head |
| P2, "Track the publish across snapshot-name collision retries" (`program.rs`) | resolved, outdated | D2a KILLED at this head |
| P2, "Revalidate the recorded deadline before sending the publish" (`engine.rs:1160`) | resolved; the author answered "Fixed in 6ec5909, as review's C1" with three points | The answer matches the code. It is right as far as the agent goes, with F1's condition on the channel |

Codex had posted no review of `6ec5909` by 01:46 UTC.

## Test gaps and suggestions (none blocks)

- **T3. No row steps the wall clock,** so a check on the wall clock (C1e) survives. Rows with the wall clock stepped back by the save's length kill it in 96 rows (E6).
- **T4. The boundary at exactly the slack is pinned by 12 of the 24 rows at 10 s.** Asserting `askings == sent` within the slack pins all 24 (E6: C1d fails 48 rows of 288).
- **T5. The C1 test has no row where the answer comes back.** Other tests cover that course, so this is optional.
- **S10. A send skipped for the slack is told to no one.** With every save slow, the person is told `NoProgress`, which points at the daemon. A report or a line when a send is skipped would let a person tell a slow disk under the agent's record from a stalled daemon.
- **Doc nits:**
  - **Nit 3.** These say "sent within the slack", but the slack is checked just before the send, and a stop after the check is not bounded by it:
    - the doc of `ASKING_SLACK` (`plan.rs:262-269`): "the most the agent lets pass between the time it reads ... and the send of the request";
    - `engine.rs:1151`: "sent within `ASKING_SLACK` of `sent`".
  - **Nit 4.** These state F1's premise without its condition (a free channel):
    - `engine.rs:1151-1154` and `plan.rs:265-266`;
    - the answer's "the 40 s of `HOLD_MARGIN` stay whole".
  - **Nit 5.** The module docs, step 8 (`engine.rs:19-21`), still say `held_until` is "from the send plus the channel's wait". It is from the stamp, plus the slack, plus the wait.
  - **Nit 6** (older). In `record.rs:215-219`, the docs of `Phase::Asked` are stale since L1, and leave out the slack:
    - `asked_ms` says "When the daemon last answered `accepted`";
    - `held_until_ms` says "The last `accepted`, plus the expiry, plus the channel's margin".
  - **Nit 7.** The C1 test's doc (`engine/tests.rs:3255-3258`) says that without the check "the rows past 80 s with uploads that never end release the hold early; 80 s is the fake's threshold". With the slack in the deadline the fake's threshold is 90 s: under C1a only the rows at 200 s release early, and the rows at 81 s do not (`plants149e.out`). The answer's "an early release needs `d` over 80 s, as you found" is the threshold before the fix.

## Observations

- **O2, for the testing session.** The daemon rig ran the stand-in as well on this pull request.
  - `which server` diffs `BASE...HEAD`. BASE is the pull request's recorded base (`f8e5e0c`), and HEAD is the merge commit `04abafe`, which merges the head into `291f813` (the tip of `zfs-topology` at the push).
  - So #154's `tests/kernel/` changes read as this pull request's. The pull request itself touches no `tests/` file.
  - This is harmless: more runs, about 33 minutes an image. But the choice reads the wrong diff. Diffing against the merge's first parent, or against the head sha, would fix it.
- **The rig built the head merged with `291f813`.** Since then `zfs-topology` changed only:
  - `tests/kernel/zfs-cli-record.sh` and `tests/kernel/test_zfs_cli_record.sh`;
  - the workflow `skyzfs-zfs-cli-record.yml`;
  - the notes.

  So the daemon it drove is the one `zfs-topology` would have after the merge.

## Verification

### Gates (`gates149e.log`, made by `gates149e.sh`)

Run on a clean worktree of `6ec5909`. The toolchain was rustc 1.100.0, current stable after `rustup update stable`. The target directory was my own, with `debug=0`, and is deleted.

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy -p skyzfs-agent -p skyzfs-agent-cli -p skyzfs-control --all-targets -D warnings`, plain and with `--features skyzfs-agent-cli/test-hooks` | pass |
| Tests, plain and with hooks | pass: 159 library (158 + the C1 test), 7 program, 16 `skyzfs-control` |
| `cargo doc --no-deps` of the three crates with `-D warnings`, plain and with `skyzfs-agent-cli/test-hooks` | pass |
| The N6 test alone, and the C1 test alone | pass |

- `skyzfs-agent-cli/test-hooks` is the only feature in the touched crates.
- I did not run the workspace-wide gates. The author reports them clean, with each feature.
- CI still does not build `skyzfs-agent-cli` with `test-hooks` (`ci.yml` and `skyzfs-ci.yml` name only `skyzfsd/test-hooks` and `skyzfs-ublk-sys/control-test-program`). That is L3/T1 of the first review, still open as ruled.

### Plants (`plants149e.py`, `plants149e.out`)

Each plant was applied alone as text to a clean worktree, with a 600 s limit and the files restored in `finally`. The worktree was clean afterwards. The suite is `cargo test -p skyzfs-agent --lib`.

| Plant | What it does | Verdict | Caught by |
|---|---|---|---|
| C1a | no check before the send (the author's mutant) | KILLED | the C1 test: all 96 rows past the slack; early release in the 12 stuck rows at 200 s |
| C1b | the slack left out of the deadline (the author's) | KILLED | the C1 test: all 144 rows (span) |
| C1c | the check against twice the slack (the author's) | KILLED | the C1 test: 24 rows at 11 s |
| C1d | `>=` (nothing sent at exactly the slack) | KILLED | the C1 test: the 12 give-up rows at 10 s only (T4) |
| C1e | the check on the wall clock, from `sent` | **SURVIVED** | (T3; killed by E6 and E2) |
| C1f | `sent_mono` read after the save | KILLED | the C1 test: 96 rows |
| C1g | the check before `point(Ask, false)` | SURVIVED | equivalent in production |
| C1h | the check's operands swapped | KILLED | the C1 test: 96 rows; early release at 200 s |
| T1a | no withdraw of a pending request on a moved pool | KILLED | the N6 test (through `recover`'s expect) and 2 tests of the plan |
| N6a | N6's clearing removed | KILLED | the N6 test: the 4 `moved` rows |
| N5a, B1a, B1f, D2a | earlier fixes reverted | KILLED | as in the fourth look |

### Scratch probes (`fifth149_scratch.rs` and `fifth149_channel_scratch.rs`; output in `fifth149_scratch.out`)

`fifth149_scratch.rs` is included as `#[path = "review_scratch_e.rs"] mod review_scratch_e;` from `engine/tests.rs` in a scratch worktree. `fifth149_channel_scratch.rs` is pasted into `mod socket` of `bins/skyzfsd/src/control/tests.rs` there. Neither is ever committed to the product. All runs were release builds.

| Probe | What | Result at the head |
|---|---|---|
| E1 | D3 widened: saves of 0 to 1000 s × 324 runs | 4,212 runs, 0 early, 0 wrong |
| E2 | stops (5 to 700 s) and wall steps (±2 h), alone and together, at every point up to the 120th, then every 7th | 25,008 runs, 0 bad. Under C1e: 90 bad; under C1a: 180 |
| E3 | the send delayed by X after the check | early only where save + X > 90 s (12 of 24 rows, the stuck ones); 0 wrong |
| E4 | a restart after a round that sent nothing | 324 runs, 0 bad; 162 asked anew, fresh, 0 attempts, 0 stalls |
| E5 | every asking save slow | nothing ever sent; ends `NoProgress`, `TooOld` or `Person`; no hold or record left |
| E6 | the C1 test tightened (T3, T4) | 288 rows, 0 failing. C1d: 48; C1e: 96 |
| G1 | skyzfsd's `Channel`: a publish queued behind a busy connection | handed on 1.47 s after its client gave up (F1) |

Section 1 of the output is a first pass: its E4 failed on expectations of my scratch that were too strict, and its E2 panicked in my scratch's own `expect`. Neither is a verdict on the product; the corrected runs follow it.

### The earlier searches, re-run at this head (`fifth149_search.out`)

| Test | Result |
|---|---|
| N1a, exhaustive to depth 4 | 114,552 runs, 0 wrong publishes, 0 other violations |
| N1b, 50 seeds a case | 88,800 runs, 0 wrong, 0 other |
| N5, N6, N2, N2b, N2c, N4, N4b | pass (N2c: 0 unsafe rows) |
| N3 | fails by design: it lists the same 12 known stops in the window; "over the allowance" is empty |
| D1, D2, D3 | pass (D2: 36 runs, 0 bad; D3: 0 released while pending at every save length) |

The run counts are the same as at `5259bf5`.

## CI on the head

Read with `gh api`, read-only, last at 01:46 UTC on 2026-11-26, with the head still `6ec5909`. **All ten runs on `6ec5909` completed with success.**

- **Completed with success:** `ci` (`changes`, `lint`, `test`, `unit-arm64`, `s3-backends`, `chart-lint`), `coverage`, `e2e`, `k3s-smoke`, `skyzfs-control-rig`, `skyzfs-cut-daemon` (no-kernel logic, `ubuntu-26.04`, `ubuntu-26.04-arm`, `ubuntu-26.04-upload`), `skyzfs-nodeb-rig` and `skyzfs-publish-rig`. Also `skyzfs-ci` on the push: its gate passed, and the rest was skipped because the pull request is open.
- **`skyzfs-daemon-rig` (run 45732038971):**
  - **On `skyzfsd`, in-place store** (`ubuntu-24.04`, `ubuntu-26.04` and `ubuntu-26.04-arm`), all green. The judge's lines read as at `5259bf5`:
    - 53 pins `PASS`;
    - 1 `WAITING`: `pin_failed_volume_while_a_request_is_held`;
    - `NA` on `pin_c_kill_with_applied_unacknowledged` and `ctl_delay_before_apply`, both named by the in-place store's table;
    - the five controls that must fail, fail.
  - **The stand-in** on `ubuntu-24.04` and `ubuntu-26.04`: green, with its own table's lines (30 `PASS`, 26 `WAITING` for checks of the daemon alone, and the five controls failing). The run ended green at 01:45 UTC.
- **None of these drives the program** (the publish rig's agent is still the Python stand-in), and none builds `skyzfs-agent-cli` with `test-hooks`. A green CI says nothing about C1; the plants and probes above do.

## What blocks the merge

**Nothing in `6ec5909`.** CI is green on the head, C1 is settled, Codex's three threads are settled, and T1 and nit 1 are done.

## Decisions for the lead

1. **F1:** a follow-up in skyzfsd, with fix (a) (drop a request whose client has hung up; small) or fix (b) (a deadline in the request; a change to the protocol, which also closes the agent's residual window), or accept the risk. I recommend (a) as a follow-up work package, and not blocking #149.
2. **T3, T4 and nits 3 to 7:** a few lines each. They can ride in this pull request if the author makes another commit, or go to the follow-up. They do not block.
3. **Still open, as ruled:** L3/T1 of the first review (`skyzfs-agent-cli/test-hooks` in CI's clippy, tests and doc), and the follow-up for S1, S8 and the rest of T2 of the first review.
4. **O2** is for the testing session (the daemon rig's choice of server on a pull request whose base has moved).

## Files (in `review/m2.6c3/`)

- `fifth_look.md`: this note.
- `fifth149_scratch.rs`: E1 to E6, included from `engine/tests.rs` in a scratch worktree, never committed to the product.
- `fifth149_channel_scratch.rs`: G1, pasted into skyzfsd's `control/tests.rs` (`mod socket`) in a scratch worktree.
- `fifth149_scratch.out`: E1 to E6, E2 and E6 under plants, G1, and the socket probe.
- `fifth149_search.out`: the third look's searches and the fourth look's D1 to D3, re-run at this head.
- `plants149e.py` and `plants149e.out`.
- `gates149e.sh` and `gates149e.log`.
