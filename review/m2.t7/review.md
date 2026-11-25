# Review of #139: M2.T7, the channel rig and the publish rig's first kernel run

Head reviewed: **d169139**, range 6e060e7..d169139. It is cabd5a0 plus
two commits:
- 9a87140: the raw client keeps an answer read before a reset;
- d169139: pchannel waits for the peer's count in the status.

Most of the review was done at cabd5a0. The two later commits touch only
`tests/kernel/publish_agent.py`, `tests/kernel/cut-record.sh` and their
tests. Every finding below holds at d169139 unless it says otherwise.
File:line references are at d169139 unless they name another commit.

## Verdict

**Not ready to merge. Three findings block (B2 to B4).**

The publish rig is green at d169139: every course exits 0, and all six
were checked. But two courses pass without testing the property they
exist for, and the table they are judged by is not pinned.

- **B1 is fixed by d169139.**
  - At 9a87140, pchannel ended with exit 1 on `pin_status_control_peer: FAILED: control_peer is 0`.
  - That was the rig's race, not the daemon: the rig read the status less than 60 ms after the peer's request, and the daemon writes its status at most once every 100 ms. The status taken at the end of the same course read `control_peer 1`.
  - d169139 polls the status until the count shows, for up to 10 s. Its run reads `control_peer 1`, and its five plants are all killed (U1 to U5).
- **Two courses passed vacuously on the property they exist for, in all three kernel runs.**
  - prewind never offered the publish a committed cut that opens as a rewind. A daemon without the condition "opens as itself" passes it. (B2)
  - pkill's withdrawn publish never had a qualifying cut in the instance that knew it. Its absence rests on "pending publishes are in memory" alone, and its premise line says otherwise. (B3)
- **The table `EXPECTED` is not pinned.** Nine loosenings survive the pull request's tests and my own driver. (B4)
- **The fix at 9a87140 is right** (section "The fix at 9a87140" below):
  - it keeps an answer that was read before a reset, in either order on the wire;
  - it hides no failure that cabd5a0 did not already miss;
  - one of its two tests fails with the fix reverted, and the other pins a behaviour the two versions share.
- **What is sound:**
  - pplain, pkillman and pkillpub, and their pins, passed in all three runs;
  - the three controls were CAUGHT in all three runs;
  - pchannel's answers are as the design says (runs 2 and 3), and its counts show once read after the daemon's write (run 3);
  - node B's snap-row decode agrees with the uploader's codec, both ways, golden and random (about 46,000 cases);
  - L1's test catches MH7 in both directions, deterministically (250 of 250 runs);
  - R10 to R12 now live inside `withheld_pick` and are tested.

Codex had one finding at cabd5a0 (P2, the raw client and ECONNRESET). I
confirm it (reproduced 40 of 40 locally), and 9a87140 fixes it. Codex
has no findings at 9a87140 or at d169139, and does not raise B1 to B4.

## The kernel runs, line by line

The three runs, all on ubuntu-26.04 with ZFS 2.4.1:
- run 1 at cabd5a0: run 45525905062, job 138911057968;
- run 2 at 9a87140: run 45529175986, job 138919974907;
- run 3 at d169139: run 45530329742, job 138924124152.

I read all three artifacts and the job logs of runs 1 and 2.

| course | run 1 | run 2 | run 3 | what the lines really show |
|---|---|---|---|---|
| pplain | exit 0, every pin ok, the 3 controls CAUGHT | exit 0, the same | exit 0, the same | Real. t4 (G 20 above the snapshot) passed over committed cuts of groups 25 to 40 and named cut 24 of group 43: the G rule was exercised (rows.tsv of run 1: commits 10 to 15 carry cuts 18 to 23, passed over; commit 16 carries cut 24, named). t1 asked twice was done on one commit; t1-other was refused `taken`; t5 expired and is absent; t7 was refused `asked`, withdrawn, asked anew and done. |
| prewind | exit 0 | exit 0 | exit 0 | **Vacuous** on "opens as itself" (B2). The named cut opens as itself, but no rewind-opening cut was ever a candidate. |
| pkill | exit 0 | exit 0 | exit 0 | tk asked again was done; tw is absent. **tw's absence rests on the in-memory rule only** (B3). |
| pkillman | exit 0 | exit 0 | exit 0 | Real. serve.log has "killed after the framed manifest PUT 1 landed (…/snap/…), before its ref was made"; killed_rc is 137; `m1-between` has no ref and no row; asked again, m1 was done. The check reads none of the hook's evidence (N3). |
| pkillpub | exit 0 | exit 0 | exit 0 | Real. The hook fired "after the transaction of publish 1 (p1) landed"; `p1-between`'s ref is the commit that p1-end's `done` names (a732e8… in run 1, d97daf… in run 2, c97b63… in run 3), so the lookup found what landed. The check does not compare them (N3). |
| pchannel | **exit 2**: the rig's error at `peer` (ECONNRESET) | **exit 1**: `pin_status_control_peer` FAILED | exit 0 | Run 1: no pchannel line was judged. Run 2: the answers are all as the design says; the count line is the rig's race (B1). Run 3: every line ok. |

**Rig errors.**
- Run 1 had one: pchannel at its `peer` step. pchannel is the last course, so it masked only pchannel's own lines.
- Runs 2 and 3 had none.
- No other course's line was masked in any run.

**pchannel's lines, and what the `peer` step shows.** The answers are the
same in runs 2 and 3; the count lines differ.

| line | result | what it shows |
|---|---|---|
| `pin_answer_nu` | ok | Served while another holder had the lease (other-node at epoch 1), `cuts` was refused `no-upload`, and the refusal says why. |
| `pin_answer_version` | ok | The greeting `skyzfs-control 9` was refused `version`. |
| `pin_answer_malformed` | ok | A key the request does not take was refused `malformed`. |
| `pin_answer_name` | ok | `progress name=a/b` was refused `name`. |
| `pin_answer_peer` | ok | A client running as `nobody`, through a socket the rig had opened to 0666, got `refused peer this daemon answers its own user`. This is the `SO_PEERCRED` check on a real kernel. |
| `pin_answer_cuts` | ok | The raw path works for a well-formed request. |
| `pin_status_control_peer` | FAILED in run 2 (B1); ok in run 3 (`control_peer 1`, `age_ms 30`) | Run 2: the rig's race, not the daemon. Run 3: the peer's refusal is counted. |
| `pin_status_control_malformed` | ok | 2, judged "at least 2". |

Measured against the design (impl notes on M2.5-2: "mode 0600 …; only
the daemon's user and root are answered (`SO_PEERCRED`, others `peer`)"),
the `peer` step shows the second half and not the first:
- The socket's mode is never looked at. `ppeer` widens the socket to 0666 and then sets it to 0600, whatever it was before (`cut-record.sh:1642`, `:1650`). A daemon that made its socket 0666 passes. (N6)
- That the refused request is not handed on is visible in the status (`control_answered 4`, both mid-course and at the end) but is not pinned. With `cuts` as the peer's request, a daemon that refused and still acted could not show it.
- The table's `peer` entry is not pinned: "any refusal" survives every test (plant E7, B4).

## The fix at 9a87140 (asked by the lead)

`raw()` now:
- keeps an EPIPE or ECONNRESET from the write;
- reads until EOF or until a reset;
- raises only when nothing at all was read.

`fix_probe.py` drives the head's `raw()`, and cabd5a0's, against servers
that end a connection in each way, 20 trials each (`fix_probe.out`).

| the daemon… | cabd5a0 | 9a87140 | judged at 9a87140 |
|---|---|---|---|
| answers and closes, request unread (skyzfsd's peer path) | ECONNRESET, exit 2 (20/20) | the answer (20/20) | ok |
| answers and closes before the client writes | EPIPE, exit 2 (20/20) | the answer (20/20) | ok |
| answers after reading the request (EOF) | the answer | the answer | ok |
| writes half the answer line, then closes (unread or after a read) | ECONNRESET, exit 2 / the half line | the half line `refused peer this daem` | **ok** (pre-existing, N5) |
| writes only the greeting, then closes | ECONNRESET, exit 2 / `skyzfs-control 1` | `skyzfs-control 1` | FAILED (a verdict) |
| writes nothing, request unread | ECONNRESET, exit 2 | ECONNRESET, exit 2 | the rig's error |
| writes nothing, closes before the client writes | EPIPE, exit 2 | EPIPE, exit 2 | the rig's error |
| writes nothing after reading the request | `''` | `''` | FAILED (a verdict) |

**Does keeping an answer read before a reset hide a real failure?** No
failure that cabd5a0 caught is lost.

- **Empty answer, or a reset with no answer.** Still raised. It is the rig's error, exit 2, as before.
- **A reset in the middle of an answer.** This cannot happen on a Unix socket: queued bytes are delivered before the reset. A short answer is therefore one the daemon cut short.
- **A short answer line is taken as the answer.** This was already so at cabd5a0 on EOF; the fix extends it to the reset path. `refused peer this daem` passes the prefix `refused peer`.
  - skyzfsd's own `ask` refuses a line with no `\n` ("the connection ended inside a line").
  - Suggest: take `lines[1]` only when it ends with `\n`, else answer "no whole line", which is a verdict.
- **The same daemon fault is classed two ways.** "Nothing written" is a verdict (exit 1) when the request was read, and the rig's error (exit 2) when it was not. For the peer path the request is never read, so a skyzfsd that dropped its peer refusal and just closed would read as the rig's own error. The run would still be red. Suggest: once `connect` has succeeded, a close with nothing read is the answer `''` (a verdict), with the error in the log.

**Do the two tests fail with the fix reverted?** I put cabd5a0's
`publish_agent.py` under 9a87140's tests (5 runs each):
- `test_an_answer_before_a_reset_is_the_answer`: ERROR, 5 of 5. It fails, as it should.
- `test_a_reset_with_no_answer_is_the_rigs_error`: passes, 5 of 5. It pins behaviour common to both versions: the fix did not make "no answer" pass. That is a fair guard, not a test of the fix.
- At the head, both pass 50 of 50. The server's `MSG_PEEK | MSG_WAITALL` makes the reset deterministic.

Plants on the fix (`fix_plants.py`, `fix_plants.out`):

| id | plant | PR's tests |
|---|---|---|
| X1 | only ECONNRESET kept on the write (EPIPE is the rig's error again) | **SURVIVED** |
| X2 | a reset with no answer is not raised | KILLED |
| X3 | the read's reset forgets its error | KILLED |
| X4 | the read's reset raises again (the fix reverted on the read) | KILLED |
| X5 | any OSError on the write is kept | SURVIVED (near-equivalent: a write timeout then times the read out) |

X1 is the other order on the wire (the daemon closes before the client
writes, which `fix_probe` shows 20 of 20 with a 0.2 s write delay). It
is not tested, so a regression there would bring the flaky rig error
back. Suggest a test whose client writes late (for example, `socket`
patched in `publish_agent` as `fix_probe.py` does). Non-blocking.

## Blocking findings, most severe first

### B1 (fixed by d169139). `pin_status_control_peer` read a status written before the peer's request: a false verdict against the daemon (pchannel, exit 1 at 9a87140)

**Where.**
- `tests/kernel/cut-record.sh:1657-1658` at 9a87140: `ppeer` then `ustatus channel-status.txt`, with nothing between.
- `tests/kernel/publish_check.py:474-477`: `COUNTS`, judged from that file.
- The daemon's pace: `bins/skyzfsd/src/serve.rs:231` (`STATUS_INTERVAL_MS = 100`) and `crates/skyzfs-ublk/src/status.rs:1527` (a pace of at most one write per interval). `bins/skyzfsd/src/control/server.rs:297-298` counts and posts the peer before it writes the refusal.

**Scenario, run 2.**
1. The peer's answer arrives (agent.tsv, `raw peer 1 refused peer …`).
2. `ppeer` restores the modes and returns, and `ustatus` runs at once.
3. `channel-status.txt` reads `age_ms 60`, `control_answered 4`, `control_peer 0`: a file written 60 ms before the read, and so before the post.
4. The next write was due at most 100 ms after the previous one.
5. `status.txt`, taken later in the same course, reads `control_peer 1`.

The peer is the last request before the read, so this happens almost
every run.

**Certainty: high** (both status files are in the artifact
`publish-rig-ubuntu-26.04` of run 45529175986).

**Fix.** Read the status after it has been written past the request.
- One way: poll `ustatus` until `control_peer` is at least 1 or 5 s pass, then judge.
- Another: wait more than two status intervals.

Pin the counts exactly (N6). This line must not go to the lead as a
verdict against the daemon.

**At d169139: settled.**
- `status_until` (`cut-record.sh:1051-1061`, called at `:1678`) reads the status again every 0.2 s, with nothing done to the pool, until `control_peer` is at least 1 or 10 s pass.
- On a timeout it logs, returns 1 and leaves the last status for the check. A daemon that never counts the peer still fails the pin.
- Run 3 reads `control_peer 1` (`age_ms 30`) and pchannel exits 0.
- `test_cut_record.sh` covers the function and its call. My plants on them (`until_plants.py`, `until_plants.out`) are all KILLED:
  - U1: the status read once, as before;
  - U2: `return 0` at once;
  - U3: `-gt` for `-ge`;
  - U4: no limit (the test hangs);
  - U5: waiting on `control_malformed` instead.
- Left over: the counts are still judged "at least" (N6).

### B2. prewind passes vacuously: no cut that opens as a rewind was ever committed after the ask, so "opens as itself" is never exercised

**Where.**
- `tests/kernel/cut-record.sh:1554-1557`: the flow starts, `sleep 20`, then the store comes back.
- `tests/kernel/publish_check.py:451-461`: the premises `premise_held_at_rewind` (`cuts_held > 0`) and `premise_abandoned_cuts_at_g` (cuts counted ≤ C of group ≥ G).
- The rule this course is for: `crates/skyzfs-upload/src/publish.rs:182` (`&& cut.itself`). The uploader does commit and offer rewind-opening cuts (`driver.rs:611-614` passes `cut.itself` on).

**What the runs did.**

| | C, G | cuts after C with group ≥ G that open as a rewind | committed after the store came back | named |
|---|---|---|---|---|
| run 1 | 19, 30 | 20 to 25 (groups 30 to 41), counted 286.3 to 293.9 s | commit 3 = cut 4 (group 7), commit 4 = cut 8 (group 16), commit 5 = cut 33 (group 65, opens as itself), store up at about 306.8 s | cut 33 |
| run 2 | 20, 31 | 21 to 26 (groups 31 to 42), counted 356.0 to 363.3 s | commit 4 = cut 10 (group 18), commit 5 = cut 37 (group 75, opens as itself), store up at about 376.7 s | cut 37 |
| run 3 | 20, 31 | 21 to 26 (groups 31 to 42), counted 339.1 to 346.3 s | commit 2 = cut 7 (group 13: the status at the rewind reads `upload_state committing`, `upload_cut 7`), commit 3 = cut 35 (group 69, opens as itself), commit 4 = cut 36 | cut 35 |

- Commit numbers are consecutive, so nothing else was committed between them. Runs 1 and 2 show this in rows.tsv. In run 3, rows.tsv samples commits 4 and later; commit 2 is the cut the status names as under way, and commit 3 carries the published row's cut 35.
- The rewind-opening cuts were each replaced while waiting (`cuts_replaced` 12 then 30 in run 1) and never committed. The store returns 20 s after the flow starts, and the rewind-opening stretch lasts about 8 s.
- So with `&& cut.itself` removed from `carried_by`, the first committed qualifying cut is still 33, 37 and 35 in runs 1, 2 and 3: the same answer, and every line ok.

**The premise lines do not show the point.**
- "cuts held … : 2": the cuts committed after the store came back, before the named one, were of groups 7 and 16 (run 1), 18 (run 2) and 13 (run 3). They were below the rewind's group (24, 25 and 25), so they were of the common history, not of the abandoned one.
- "2 cuts counted up to C … the abandoned history's, which the publish must not name": those cuts are excluded by "counted after C" alone.
- The description's "so the abandoned history's cuts are held when the publish is asked" did not hold in any of the three runs.

**Certainty: high** for all three runs, from the artifacts. The timing makes it
likely in every run.

**Fix.** Make a rewind-opening cut, counted after C, of group ≥ G, be
committed after the ask. Then add a premise that checks it did happen.
- Bring the store back as soon as the publish is `accepted`, not 20 s later, and/or rewind further than eight groups, so that the new history spends longer below the abandoned top group.
- The premise needs a committed cut's identity: rows.tsv samples it (it misses commits, so a miss reads as the rig's error, not a pass), or node B could walk the commits from the published one back.

### B3. pkill: `premise_tw_cuts_past_g` counts the next instance's cuts; tw's withdrawal is never put to the test

**Where.**
- `tests/kernel/publish_check.py:444-450`: `past = [c for c in cuts if c[1] >= tw['group']]` over every instance.
- `tests/kernel/cut-record.sh:1576-1579`: tw is asked at +30 and withdrawn, then tk is asked and the daemon is killed at once.

**What the runs did.**
- Run 1: tw was asked with G 43 and withdrawn in instance 1 (pid 12289), whose newest cut was of group 18. All six cuts of group ≥ 43 were counted by instance 2 (pid 12544), which never knew tw.
- Runs 2 and 3: the same (instance 1's cuts end at group 18; instance 2's reach 63).

So `pin_tw_absent` rests only on "pending publishes are in memory". It
would hold for a daemon whose `withdraw` answered `withdrawn` and left
the publish queued in its uploader.
- pplain's t7 does not cover that case either: its G is +1000000.
- The line's text "tw would have qualified" is not true of any instance that knew tw.
- The lead's addition ("a kill after `withdraw` was answered but before the publish landed") is met to the letter only.

**Certainty: high** (request logs of all three runs).

**Fix.**
- After the withdrawal, sync instance 1 until it has counted (and committed) cuts past tw's G, then kill.
- Count the premise per instance: cuts of the pid that answered `withdrawn`, after the withdrawal, of group ≥ G, opening as itself.
- Optionally check `publish_withdrawn` in the status before the kill.

### B4. `EXPECTED` is not pinned in either direction by any test

**Where.** `tests/kernel/publish_check.py:80-133`.
`tests/kernel/test_publish_check.py` pins only two things:
- that a published name's end is `done ` (line 31);
- that `done` fails for tw-after (lines 60-63).

Its other tests build their input from the table itself, so they move
with it.

**Plants** (`plants_py.py`, each alone; "driver" is my end-to-end driver
`drive_publish_check.py`):
- Survive both the pull request's tests and the driver:
  - E3: tw-last may end `done`;
  - E5: att, any refusal;
  - E6: t5 may end `done`;
  - E7: peer, any refusal;
  - E8: m1-after may be `done`;
  - E9: t7-withdraw may still pend;
  - E10: t7-other `accepted`;
  - E11: nu may answer `cuts`;
  - E12: t1-again-end may be refused.
- Survive the pull request's tests, killed only by my driver: E1 (t1-other-end may end `done`), E4 (tk-after may read pending), and E13 to E17 (the tables `ABSENT`, `PUBLISHED`, `BETWEEN` and `COUNTS`).
- Killed by the pull request's tests: E2 alone (tw-after may be `done`).

A kernel run cannot catch a loosening: the daemon's right answer still
passes.

**Fix.** A test that pins each table exactly, so that a change to what
the rig accepts is made on purpose, as the replay and control rigs'
tables are. For example, an explicit literal copy in the test, with a
line on why each entry allows what it allows.

**Certainty: high.**

## Non-blocking defects, test gaps and suggestions

**N1. main()'s wiring is not reached by the pull request's tests.**
- M1 to M14 all survive them: premises always true, the controls placed where they cannot fail, absent names judged by the ref alone, the node B stages not judged, and so on.
- My driver (`drive_publish_check.py`, 34 cases, OK at cabd5a0 and at 9a87140; `publish_check.py` is the same at d169139) kills all but two:
  - M3: a MISSED control does not fail the run. Untested anywhere.
  - M14: an equivalent.
- The description's "`test_publish_check.py`: … the premises, the controls" overstates what is tested.
- Suggest taking a driver like mine into the pull request, with a case for M3.

**N2. Node B's `snap_row` keys are untested.**
- SR14, SR15 and SR16 survive everything: the row read at `snap/T`, the ref read as `snaps/T`, the manifest from `skyzfs/manifests/`. The test duplicates the manifest key's format string instead of calling the code that builds it.
- In a kernel run, any of these would show as FAILED pins blamed on the daemon.
- SR11 (the version read by its low byte alone) survives the pull request's tests; my cross-check kills it.
- Suggest: test `snap_row` against a `MemoryStore` with the uploader's own keys, and add a row of version 257.

**N3. pkillman and pkillpub do not judge their own premise.**
- The check reads neither `kill.tsv` (killed_rc 137 in all three runs) nor the hook's line in serve.log. pkillman would pass if the daemon died elsewhere or of something else before the manifest.
- pkillpub does not compare `p1-between`'s ref with p1-end's `done` commit, which is what "found, not made anew" means. They agreed in all three runs.
- `m1`/`p1` accept `none `, so `none (exit 2)`, an agent error of any cause in the killed flow, passes as an answer (`publish_check.py:111-124`).
- In all three runs, m1 and p1 answered `unknown`, so nothing was hidden there.

**N4. R10 to R12 and D10.**
- W1 to W4 are KILLED: the pick is inside `withheld_pick` and tested.
- W5 and W6 SURVIVE: a call site that bypasses `withheld_pick` (one line each, in `cut_check.main` and in node B).
- W7 (node B's lacks line not naming what was passed over: D10 itself) SURVIVES.
- A control that is "not built" still fails loudly, so a control cannot vanish silently. These are small gaps.

**N5. The raw client accepts an unended answer line, and classes "closed without a word" two ways.** See the fix section above.

**N6. pchannel's channel pins.**
- The socket's mode is neither recorded nor pinned (`cut-record.sh:1642`, `:1650`).
- `control_answered` is not pinned. It should stay 4, showing that the peer's request was not handed on.
- `COUNTS` is "at least" (`publish_check.py:476`). A daemon that counted the `name` refusal as malformed (3), or the peer twice, passes.
- Suggest exact values. Consider a peer request with an effect (a `publish` of a fresh name, then pin its absence).

**N7. "The first qualifying committed cut" is not checked.** Only the
named cut's properties are. In the cases I traced (pplain's t4 and
prewind, in all three runs) the named cut was the first committed
qualifying one, from rows.tsv and the commit numbers.

**N8. Small things.**
- `judge_cut` matches a cut by number and group without the state.
- The watcher's VERDICT lines are not judged by `publish_check`.
- `ctl_commit`'s detail prints 12-hex prefixes that read the same ("the ref is on c137ad953ada, done named c137ad953ada"): print the whole hashes.
- The summary shows pchannel as "NOT CHECKED" without the rig error's text (run 1).
- The stand-in agent releases its hold while a publish still pends (t7, tw and tk), unlike design §5.2's agent.
- The module docstring names a table `PREMISES` that does not exist.
- `raw`'s exit codes differ from its docstring.
- `premise_t4_cuts_below_g` counts counted cuts, not committed ones. They were committed in all three runs (run 1: cuts 18 to 23; runs 2 and 3: cuts 17 to 22).

**N9. The description against the code.**
- "Kernel runs: none yet" and "CI: to be added" are stale (the description is unchanged at d169139). There are three runs now: say what they show, with this review's readings of pchannel, prewind and pkill.
- 9a87140 and d169139 are not in the description's commit list.
- The claim about the tests of the premises and the controls (N1) overstates.

## What was checked, and how

**Local gates** (`gates.out`):
- At cabd5a0:
  - `cargo fmt --check` clean;
  - `cargo clippy -p skyzfs-nodeb --all-targets -D warnings` clean;
  - `cargo test -p skyzfs-nodeb`: 23 passed.
- At 9a87140, rustc 1.100.0:
  - `shellcheck -S warning` on every `*.sh`: clean;
  - `tests/kernel/test_cut_record.sh`: all passed;
  - `python3 -m unittest discover` in tests/kernel: 562 OK. Its "FAILED prefix" lines are printed by tests that expect them.
- At d169139: `shellcheck -S warning` clean; `test_cut_record.sh` all passed, its five new `status_until` checks included.

**Node B's decode against the uploader** (`xcheck_snaprow.rs`, appended
for a local run by `xcheck_snaprow.sh`; `xcheck_snaprow.out`, 29 passed):
- constants and keys agree;
- golden bytes laid out by hand from the documented table;
- 20,000 rows encoded by the uploader and decoded by node B;
- 20,000 random version-1 rows read by both, and re-encoded to the same bytes;
- refusals agree at every length from 0 to 300 across 12 versions (3,612 cases), and an unreadable row's line carries its bytes;
- 2,000 manifests judged against the uploader's length and BLAKE3, with one-bit and one-byte changes caught.

**Plants on node B** (`plants_rs.py`, `plants_rs.out`):
- SR1 to SR10, SR12, SR13 and SR17: KILLED by the pull request's tests.
- SR11: SURVIVED the pull request's tests, KILLED by the cross-check.
- SR14 to SR16: SURVIVED both (N2).
- MH7 (the original `|| row.is_some()`), MH7t, MH7f, MH7r and MHS: KILLED.
- MHA: SURVIVED (equivalent: head moved after the commit's PUT is still before the swap).

**L1's determinism.** `a_lost_swap_of_head_through_move_head` passed 200 of
200 alone and 50 of 50 under 16 threads.

**Plants on the Python side** (`plants_py.py`, `plants_py.out`, with
bytecode off):

| group | killed by the pull request's tests | survived the pull request's tests | of those, killed by my driver |
|---|---|---|---|
| E (the tables) | E2 | E1, E3 to E17 | E1, E4, E13 to E17 |
| J (the judges) | 15 of 16 | J13 (a prefix anywhere in the answer) | none |
| M (main's wiring) | none | M1 to M14 | all but M3 and M14 |
| A (the agent) | A1 to A12 | none | n/a |
| W (the pick) | W1 to W4 and W8 | W5 to W7 | none |

**The fix.**
- `fix_probe.py` and `fix_probe.out`: probes of `raw()` before and after.
- `fix_plants.py` and `fix_plants.out`: plants X1 to X5.
- The two new tests were run against the reverted client.
- `until_plants.py` and `until_plants.out`: plants U1 to U5 on d169139's `status_until`.

**CI at d169139, as of 02:23 UTC:**
- **Green:**
  - the publish rig (run 3);
  - ci, including `unit-arm64`;
  - skyzfs-ci, coverage, e2e, k3s-smoke;
  - the node B rig, the kernel probe, the control rig, and the cut rig through skyzfsd (cut-daemon).
- **Still running:** the daemon rig, the cut rig ("record and check" on four images) and the replay rig.
- Two things about those still running:
  - The cut rig's kernel jobs had not finished on any head of this pull request before d169139: each newer push cancelled them. They are the only kernel run of `cut_check`'s new `withheld_pick` (R10 to R12), so their result is owed before the merge.
  - The node B rig, which runs node B's side of the pick, is green at d169139. Its `ctl_lacks` caught at copies on both images and both layouts, with no write passed over, so D10's naming did not show.

At 9a87140 the publish rig was red (B1). The other runs there were
cancelled or green when d169139 was pushed.
