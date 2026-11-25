# Next look at #149 (M2.6c-3, the program `skyzfs-agent`) at 0ad7944

- Head: `0ad79443cb896bc981115d2de2b2c5ad0058d242`. Base: `zfs-topology` (the pull request's base sha `8f0390f`).
- Previous look: `af50814`, in `review.md` here and on the pull request (comment 7170794881).
- The author's answer: https://github.com/skyzfs/skyzfs/pull/149#issuecomment-7172976164
- Diff reviewed: `git diff af50814...0ad7944`, and the head as it stands.
  - The pull request's own commits are `2d806e8` (B1, L1, D2, L4) and `1cf1428` (the command-line items).
  - `5ee23ec` merges `zfs-topology`.
  - `0ad7944` merges #147's head `ab8db11`.

## Verdict

**Not yet.**

- B1 is fixed, and so are D2 and the rename livelock. An exhaustive search over the daemon's answers finds no release while a request pends and no wrong publish.
- L4 and the command-line items are as claimed, apart from nits.
- One new defect, **N5**, comes from L1 (recorded asked before the send).
  - The setup: the answer to an ask is lost, the daemon is silent, and the agent's wall clock steps forward.
  - What happens: the hold is released at once, and a wrong publish follows. It is reproduced on the fakes.
  - At `af50814` the same run kept the hold. The fix is one line.
  - I would fix it before the merge. Whether it blocks is the lead's call; see "Decisions".
- CI is not green on the head. `skyzfs-cut-daemon` failed on `ubuntu-26.04-upload`, in a course of the daemon's uploader that this pull request does not touch (see "CI").

## Defects

### N5 (should fix before merge): a lost answer and a step of the wall clock release the hold while the request pends

**Where**
- `crates/skyzfs-agent/src/engine.rs:1141-1155`, the `asking` commit in `Agent::ask`.
- `crates/skyzfs-agent/src/engine.rs:1175-1176`: `memory.accepted` is set only on `Accepted`.
- `crates/skyzfs-agent/src/engine.rs:838-849`, `now_for`: the monotonic clock is used only when `memory.accepted.at == asked_ms`, and the wall clock otherwise.

**Scenario**
1. The agent records `asking` with `held_until = sent + ASK_LIMIT + expiry + margin`, which is about `sent + 680 s` on the wall clock.
2. It sends the publish, and the daemon accepts it.
3. The answer is lost: either `Err` (a reset) or `refused no-answer`. Both leave the record asked, as L1 intends.
4. The daemon is then away for 900 s, and the agent's wall clock steps forward 2 h.
5. `now_for` reads the wall clock, because `accepted` was never set for this ask. `held_until` has passed, so the plan releases the hold in the next round.
6. The person (here the fake's destroyer) destroys `pool@t`, a group is written, and the request lands on a cut without `pool@t`. That is a **wrong publish**.

**Reproduced** by the scratch test N5 (`next149_scratch.rs`, output in `next149_scratch.out`), with the daemon `LostAnswer`.

At the head:

| Mode | Wall step 0 (control) | Wall step +2 h |
|---|---|---|
| Idle | no wrong publish | wrong publish: `snap/t` on cut 5, after the destroy; hold released 1 s after the loss |
| Busy | no wrong publish | wrong publish: `snap/t` on cut 6 |
| Abandoned | nothing published | nothing published |

- Each of the eight wrong publishes holds with `Err` and with `no-answer`, and for both the agent's own flow and a person's give-up in the same process.
- At `af50814` the agent's own flow with the same step published correctly. An `Err` there left the record `Held`, and nothing released a `Held` hold on time.
- So this is a regression from L1. L1 creates a state ("asked in this process, acceptance not seen") that the "Time" docs (engine.rs:49-66) do not cover and `now_for` reads on the wall clock.

**Fix (verified)**
- After the `asking` commit, set `memory.accepted = Some((sent, self.clock.mono_ms()))` (fix probe F1).
- With it, N5 passes in all 24 combinations, and the whole suite passes (F1 SURVIVED: the fix breaks no test).
- An `Accepted` answer overwrites it as today. A refusal that `!may_pend` puts the record back to `Held`, where `now_for` no longer reads it.

**What remains after the fix**

A person's give-up from a new process (`skyzfs-agent recover --give-up` while the agent is down) still reads the wall clock, as the "Time" docs say. That limit is the documented one (S4 of #142, R2 of the last look), and not this finding.

**Certainty**
- Certain on the fakes.
- In production it needs all of these: an answer lost after acceptance; a forward step of the wall clock of more than about 11 minutes between the loss and the next rounds; and the daemon away or silent until then.
- It is narrow, but it is the S4-of-#142 class, which the design treats as a correctness matter (a snapshot published without its hold).

## CI on the head (read at 20:30 UTC)

**Passed:** ci, coverage, e2e, k3s-smoke, skyzfs-ci (push), skyzfs-control-rig, skyzfs-nodeb-rig, skyzfs-publish-rig and skyzfs-replay-rig.

**skyzfs-daemon-rig passed on all three images** (ubuntu-24.04, ubuntu-26.04 and ubuntu-26.04-arm, on the daemon). Its logs show the controls caught and the one pin `pin_failed_volume_while_a_request_is_held` WAITING, as expected.

**Failed: skyzfs-cut-daemon**, run 45690023761, job "record through skyzfsd and check, ubuntu-26.04-upload".
- The course `uv1` failed with `upload_course: FAIL: the row at the end names commit 04f2a092…, head is b579ff50…: the whole cut after the row of version 1 did not write a row of version 2 naming its commit`.
- Earlier in the same course the rig logged `the daemon cut after the look before v1in (cut 12, …): again`.
- The other courses and both other images passed.
- **Not caused by this pull request.**
  - The daemon's code at `0ad7944` is the same as at #147's head `ab8db11`. `git diff ab8db11 0ad7944` outside `crates/skyzfs-agent`, `bins/skyzfs-agent` and Cargo changes only a doc comment in `crates/skyzfs-ublk-sys/src/queue.rs`.
  - The run on `ab8db11` (45683716288) passed, as did the run on `af50814`.
- So it is either an intermittent course of the uploader, or a race in the rig's read of the end row.
- It should go to the testing session, with the run's artifacts, before a rerun. A rerun alone would leave an intermittent failure unexplained.

## Test gaps (suggestions; the product is right at the head)

### T-a: "no answer at all leaves the record asked" is pinned by no test

`engine.rs:1190-1192`.
- Plant L1j makes an `Err` from the ask commit `Unasked`, back to `Held`. It SURVIVED the whole suite when run alone. (In the first pass, two timing tests of `host::` failed under load; see "Observations".)
- It is not harmless. With L1j, N5 gives a wrong publish with **no** clock step: a Busy run where the answer is lost (`Err`), then a person's give-up. The record is `Held`, so the give-up releases at once, and the request lands after the destroy.
- `the_record_says_asked_before_the_request_is_sent` (engine/tests.rs:2638) covers the stop before the send and the refusals, but not a send whose answer never comes.
- Suggestion: a test of the form of N5 at skew 0 (a fake daemon that accepts the publish and returns `Err`, then a give-up while the daemon is away) would pin both L1j and N5.

### T-b: the channel's wait in the asking `held_until` is pinned by no test

`engine.rs:1142-1145`.
- Plant L1b drops `ASK_LIMIT` (asking `held_until` = `sent + expiry + margin`), and it SURVIVED.
- The fakes accept at the send, so no test can see the 40 s.
- Low priority. A plan test that the asking record's `held_until` is at least `sent + ASK_LIMIT + expiry + HOLD_MARGIN` would do.

## Nits

1. **The stall test's doc** (engine/tests.rs:780-792), and the author's "one stop there costs one stall".
   - At `ask:after` and `asked:before`, the request was sent and accepted. The extra one in `run.asked` there is, as I read it, the `asked` record of that request that the stop skips: a missing sample rather than a stall counted again.
   - If so, "Such a stop counts one stall more" overstates the cost at two of the four points.
   - No change is needed to the test; see (1) below.
2. **The doc of `fresh_name`** (engine.rs:1319-1320, "may be tried once more … costs one round").
   - N4: with a chain of n renames in one second and a stop at the k-th program run, the restarted agent walks the chain again: k more program runs, up to n, not one.
   - It always ends, and the exit is right. Only the doc is off.
3. **S6's claim** "`--help` or `-h` … in place of a value" holds for `--help` only (program.rs:268).
   - `record --dir -h` exits 1 (`-h: No such file or directory`), and `recover --name -h --give-up` proceeds with the name `-h`.
   - Either the code or the claim and usage should change. Taking `-h` as a value is defensible.
4. **L4's doc** (engine.rs:706-707): "a record whose marker a person removed is refused too".
   - `recover` commits (engine.rs:677) before `settle` sweeps (engine.rs:635), and the commit writes a missing marker (record.rs:576-578).
   - So after `recover`, such a record is swept, not refused. It is harmless, since the directory is the agent's, but the sentence is true only for `publish` and for `settle` with no commit first.
5. **The text of `Error::Unmarked`** (engine.rs:362-369) says what is not released, but not what a person does next.
   - For example: check `--dir`; if it is right, write the marker by running a command that saves (or release the holds by hand with `zfs release skyzfs-agent …`).
6. **The "Time" module docs** (engine.rs:60-61) say of a hold released early that "its cut holds the snapshot all the same".
   - After a destroy, that is not so (R2 of the last look, and N5 above).
   - The sentence predates this pull request, but the N5 fix is a natural moment to correct it.

## Observations (no action asked)

- **"Given up then made (whole)."** In the searches below, 1,416 (N1a) and 891 (N1b) runs ended `abandoned t person` while `snap/t` was then made whole and correct.
  - Every such run has the daemon unable to answer `cuts` (`away`, or `no-upload`, which refuses `cuts` too) until past `held_until`, or `ended` with the same effect.
  - The agent cannot look, so it gives up when the hold's time is over, and the request lands afterwards, still on a cut that has `pool@t`.
  - The age bound behaves the same way. The outcome says "given up", and object storage has the snapshot.
  - Worth a sentence in the docs of `Why::Person`, perhaps.
- **The timing tests of `host::`** (`a_program_past_its_limit_goes_with_what_it_started`, `an_output_held_open_past_the_end_is_waited_for_only_to_the_limit`; #147's code) failed three times here while a four-thread search ran beside them, and passed when run alone.
  - Their margin is 1 s over a 300 ms limit. CI runners are not loaded this way, so this is for #147's record only.

## Answers to the three questions

### (1) The stall test's window: tight, or loose enough to pass a double count?

Its bounds are exact. N3 stops at each of the test's 134 stops and measures the extra stalls:

| Where the stop falls | Stops | Extra stalls |
|---|---|---|
| Outside the window | 98 | none, so no stop outside costs one |
| Inside the window (`asking:after`, `ask:before`, `ask:after`, `asked:before`) | 24 | exactly one |
| Inside the window | 12 | none |

So a double count is caught wherever it can be seen: it would read 2 > 1. The author's mutant (ST1) and mine (ST2: an ask anew from an `asking` record counts two) are both KILLED by this test.

At the 12 in-window stops with none extra, a double count would pass. That is not because the window is wide, but because nothing there can show it:
- points 19-22 come before the restart in the run, and the stop never fires;
- points 27-30 are the first ask after the reboot, where the stalls start again from 0 on the new attachment;
- points 951-954 are the last ask, and no `asked` record follows.

Reading the stalls also at the `asking` record would make the eight non-vacuous points observable. That is a suggestion; I have not tried it.

### (2) `no-upload`: silence in a give-up, nothing pending in `may_pend`. Consistent and safe?

Yes. The two read different requests (N2b in `next149_scratch.out` tabulates every code):
- To `publish`, `no-upload` is this daemon process's answer to the request just sent. A daemon with no uploader took nothing, so the record goes back to `Held`. Nothing else of this publish can pend there:
  - a `Held` record and an ask anew both come after the daemon said the earlier request ended;
  - a request of an earlier daemon process ended with that process.
- From `progress`, during a give-up, `no-upload` says nothing about a request that an earlier process may have accepted. The give-up waits until `held_until` if the publish is asked, and releases if it is held. That is the conservative reading, and costs at most `held_until`.
- In practice the `no_upload` handler refuses `cuts` too, so the agent sees the daemon as away and does not ask at all. A `no-upload` answer to `publish` needs a daemon replaced between `cuts` and `publish`.
- Both readings are pinned by tests: L1f (`no-upload` read as pending) and B1d (`no-upload` not counted as silence) are both KILLED.

### (3) Is the merge of #147's head `ab8db11` clean?

Yes.
- The trees of `0ad7944` and of `5ee23ec` (the merge of `zfs-topology`) each equal `git merge-tree --write-tree` of their parents, and `git show --remerge-diff` is empty for both: no conflict resolution, no change made in a merge.
- #147 is still open with head `ab8db11`. If #147 changes again, this branch needs the merge again.

## Verification

### B1's fix, by search over the daemon's answers (scratch tests N1a, N1b; `next149_search.out`)

**Set-up**
- A first agent stops at one of four points:
  - after `accepted` (the request pends);
  - after the send, not yet recorded;
  - after `asking`, not sent;
  - after `held`.
- A second agent then carries out a person's give-up against an oracle daemon. Its answers are, per request:
  - `cuts`: truthful or away;
  - `progress`: truthful, `no-answer`, `ended` or `no-upload`;
  - `withdraw`: truthful, `no-answer`, `no-answer` but carried out, or `ended`;
  - `look`: truthful, `no-answer` or `store`.
- After that come one of five tails (truthful, `no-answer`, `ended`, `no-upload`, away), each until 30/300/599/601/639/641/679/681 s or for ever.
- Modes: Busy, Idle and Abandoned. Lag: 60 s and one year. A destroyer destroys `pool@t` as soon as it is not held.

**Checks**
- No wrong publish.
- **New this look: a strict check.** At every release, the fake daemon's own state must not have the request pending, read without the channel's faults.
- Holds empty at the end, settled.

**Results**

| Search | Runs | Time | Wrong publishes | Releases while pending | Other violations |
|---|---|---|---|---|---|
| N1a, exhaustive to depth 4 | 57,276 | 59 s | 0 | 0 | 0 |
| N1b, 50 random seeds a case | 44,400 | 42 s | 0 | 0 | 0 |

Control for the strict check: with B1's fix reverted (plant B1a), N1a at depth 2 finds 1,944 runs of 7,992 that release while the request pends, from the first round on.

### N2: the planner, exhaustively (phase × reply × snapshot × withdrawn × time)

- 0 unsafe rows.
- A person's give-up differs from the age bound on `skyzfsd`'s answers only in two places:
  - `unknown`, `expired` or `withdrawn` without a withdraw of its own (harmless, as at the last look);
  - `done` with `look` absent, where the person releases and the age bound stops (deliberate).

### D2 and the rename livelock (N4, N4b)

- N4: rename chains of 1 to 6 names taken in one second, through `execute`. Each ends with exit 0, and the name made is taken by no one. A stop at each program run and a restart ends too; nit 2 is the cost.
- N4b: `recover --retry` follows a rename.
- Plants D2a-d are all KILLED.

### L4

- Plants L4a-g are all KILLED.
- A mistyped `--dir` with stale holds exits 6 and releases nothing, per `program::tests::a_mistyped_directory_exits_record_and_releases_nothing`. It was not probed on the binary, since there is no ZFS here.

### Command line (S2-S6, S9)

`probes149n.out`, on a build with hooks:
- S2:
  - `record --dir` of a missing path exits 1;
  - of a file, it exits 1 with "not a directory";
  - of an empty directory, it prints `absent`.
- S3:
  - `--name --give-up` and `--volume --run` exit 2 with the new message;
  - `--dir=--d` is taken as a value.
- S4: `record` and `--help | head -1` into a closed pipe both exit 0, with no panic.
- S6: `--help` after a command or in place of a value prints the usage (`-h` in place of a value does not; nit 3).
- S9: `cargo doc -D warnings` passes, and the collision is gone.

### Plants (`plants149n.py`, `plants149n.out`)

Each plant is applied alone as text to a clean worktree of the head, with the suites run under a 600 s limit and the files restored in `finally`. The worktree was clean afterwards.

| Plants | Verdict | Caught by |
|---|---|---|
| B1a-h (silence's definition, the withdraw guard, waits past `held_until`, `Held` waits) | KILLED, all 8 | `plan::tests::a_person_gives_up_in_the_order_of_every_give_up`; B1a/b also by `engine::tests::a_person_s_give_up_keeps_the_hold_while_the_uploader_is_silent` |
| L1a, L1c-i | KILLED | `engine::tests::the_record_says_asked_before_the_request_is_sent` |
| L1b (no `ASK_LIMIT` in asking `held_until`) | **SURVIVED** | T-b |
| L1j (no answer at all recorded held) | **SURVIVED** when run alone | T-a. The first pass's KILLED came only from the two load-sensitive `host::` tests. |
| ST1 (the author's), ST2 (mine) | KILLED | `a_stop_at_any_point_of_a_stalling_run_counts_each_stall_once` |
| D2a-d | KILLED | `program::tests::a_publish_made_under_its_next_name_exits_by_its_outcome` |
| L4a-g | KILLED | `record::tests::the_first_save_writes_the_agent_s_marker`, the crash matrices, `program::tests::a_mistyped_directory_exits_record_and_releases_nothing`, `settle_exits_by_the_worst_outcome`, the hooks binary test |
| S2a, S3a, S4a, S6a | KILLED | the command-line tests named in the output |
| F1 (the N5 fix) | SURVIVED, as intended | the fix breaks no test, and N5 passes with it |

### Gates (`gates149n.log`)

Run with `rustc 1.100.0` (current stable after `rustup update stable`) and a target directory of my own with `debug=0`. All pass:
- `cargo fmt --all --check`;
- `cargo clippy -p skyzfs-agent -p skyzfs-agent-cli -p skyzfs-control --all-targets -D warnings`, plain and with `--features skyzfs-agent-cli/test-hooks`;
- the tests, plain and with hooks: 154 library, 7 binary and 16 `skyzfs-control` tests;
- `cargo doc` with `-D warnings`.

## What blocks the merge

1. **N5**, if the lead agrees that it blocks. I recommend it: it is a regression from L1, it is a wrong publish, and the fix is one line with a test of the shape of N5. T-a's test would come with it.
2. **CI green on the head.** `skyzfs-cut-daemon` (ubuntu-26.04-upload, course `uv1`) needs the testing session's look and a rerun. Everything else is green, the daemon rig included.

The nits and T-b do not block.

## Decisions for the lead

1. Does N5 block? My recommendation is yes, with fix F1 and a test of a lost answer, with and without a wall step, in the same pull request.
2. The cut-daemon failure (`uv1`): to the testing session before any rerun? It is outside this pull request's code but red on its head.
3. Still open from the last look: L3/T1, the third feature in CI and CLAUDE.md's count, left to testing and the lead.
4. Also from the last look: S1, S8 and the rest of T2 are proposed as a follow-up pull request. I agree that none of them blocks.

## Files (in `review/m2.6c3/`)

- `next_look.md`: this note.
- `next149_scratch.rs`: the scratch tests N1a/N1b (searches with the strict check), N2/N2b (planner tables), N3 (stall window), N4/N4b (renames) and N5 (lost answer and wall step). It is included as `#[path = "review_scratch.rs"] mod review_scratch;` from `engine/tests.rs` in a scratch worktree.
- `next149_search.out`: N1a/N1b output, with and without the strict check, and the B1a control.
- `next149_scratch.out`: N2, N2b, N3, N4, N4b and N5 output, and N5 at the head, with F1 and with L1j.
- `plants149n.py` and `plants149n.out`: the plants and their table, with the re-run alone of L1b, L1j, L4e and S3a.
- `probes149n.sh` and `probes149n.out`: the command-line probes (paths replaced by `SCRATCH` and `TMP`).
- `gates149n.sh` and `gates149n.log`: the gates.
