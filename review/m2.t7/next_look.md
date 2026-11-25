# Next look at #139 (M2.T7): the channel rig at c7cc03f

Head reviewed: **c7cc03f**. The review before was at d169139
(`review.md` in this directory). New since then, five commits:
- af9b7f0: `crates/skyzfs-control/` added to the paths of the four rigs that build skyzfsd or the uploader; `ctl_commit` changes the first digit;
- 5d662d8: the premises of prewind and pkill (B2, B3), the tables held to literal copies (B4), the kill courses' premises and pkillpub's commit (N3), exact counts and the socket's mode (N6);
- 1e6ec51: `raw` takes only a whole line, and nothing written is a verdict (N5, X1);
- 338a484: node B's `snap-row` tested against the uploader's own keys (N2);
- c7cc03f: a driver of `publish_check.main` (N1, M3).

I formed my view from the diff and the code first, then read the
author's answers (issue comment 7152484647) as claims to check.
File:line references are at c7cc03f.

## Verdict

**Can merge. No finding blocks.** B1 to B4 are settled.

- **B2 is settled.** prewind can no longer pass without a rewind-only cut having been offered to r1.
  - The premise requires that node B's row, read after the store came back, names a cut that is of the state asked, counted after C, of group G or later, and that opens only as a rewind by its `cut` line.
  - If r1 was made by that same cut, the premise holds and `pin_r1_cut` gives the verdict (exit 1).
  - Replayed on the kernel run's own prewind files: a daemon without "opens as itself" ends exit 1. As the run went, it ends 0.
- **B3 is settled.** pkill's premise counts only the cuts of the instance that answered `tw withdrawn`, after that line, that open as itself and are past tw's G. The committed cut must be one of those.
  - Replayed on the run's own pkill files: a daemon whose `withdraw` left tw queued ends exit 1. `pin_tw_absent` fails, and so does `pin_tw_withdrawn_status` when the status shows it.
- **B4 is settled.** All 20 loosenings of the tables are killed by the pull request's tests. That includes the nine that survived before (E3, E5 to E12), and E17 to E20 on COUNTS, TW_STATUS and KILLED_BY.
- **N1/M3 are settled.** The new driver kills M3 and a missed `ctl_files` (M3f), and 12 of the 15 wiring plants in all. The three survivors cannot hide a verdict in a kernel run.
- **N2 is settled.** SR11, SR14, SR15 and SR16 are each killed by the new node B test.
- **N3 and N6 are settled.** K1 to K4, C1 to C4 and S6 are killed.
- **N5 classes the closes as the review asked.** Once connected, a close with nothing written is the answer `''` (a verdict), whichever way the wire ends. A cut-short line and a lone greeting are `no answer line: …` (a verdict). Two nuances remain (D2, D3).
- **Nothing regressed.**
  - The fix of 9a87140 still holds: the answer before a reset is kept in both orders on the wire, 20 of 20 each, and plants A16 and A17 are killed.
  - The wait added by d169139 (B1) still holds: plant S12 is killed.
  - Every pull-request run at c7cc03f is green. The push runs of the same head that repeat them were cancelled by the author.
  - Codex reports no findings at c7cc03f.
- **The reasons for what was not taken mostly hold.** W5 to W7's reason is not accurate (below), but they can wait.

What remains are three small defects (D1 to D3), test gaps and a line of
the description. None of them blocks.

## The kernel run at c7cc03f

Publish rig run 45539186244 (ubuntu-26.04, one runner, ZFS as in the
earlier runs): every course exits 0, every line is ok, and the three
controls are CAUGHT. I read its artifact and the job log, and checked
the two new premises against the request logs.

**prewind.** Times are from the daemon's monotonic clock, in seconds.
- The rewind was at group 25. Before it, the history reached cut 18 of group 43.
- The agent's `cuts` read C = 20 (group 29, `opens=rewind`). The snapshot's group G was 31.
- Cut 21 (group 31) and cut 22 (group 32), both `rewind`, were counted at 291.15. This was before the publish was answered.
- r1 was answered `accepted` at 293.48. The store came back after that, because `asked` waits for the answer.
- `rewind-row.txt` names cut 22 of group 32 at head commit 2.
  - No commit could land while the store was down, so commit 2 landed after the ask. This is with r1 pending.
  - The run's statuses point to commit 1 being the first cut (number 1, below C): `upload_blocks 0` at the rewind, and 16 of 18 cuts replaced.
- The rig then nudged a group a second. Cuts 23 to 26 (groups 33 to 42) were committed, all `rewind`. Cut 27 (group 45, `itself`) made r1.
- Without "opens as itself", the uploader would have made r1 on cut 22. If r1 was still being looked up at that commit, `absent()` would find cut 22 among the 64 known cuts. If it was waiting, `committed()` would give it cut 22. Either way the result is `pin_r1_cut FAILED`, exit 1 (`replay_faults.out`).

**pkill.**
- tw was asked with C 6 and G 43, and withdrawn at 308.99 by pid 12429. That line in the request log is `publish withdraw … tw withdrawn`.
- The same pid then counted cuts 10 to 22. The four of group 43 or later (cuts 19 to 22) all open as itself.
- `tw-row.txt` names cut 19 of group 43 at head commit 15.
- `tw-status.txt` (age 210 ms, `cuts 19`) reads withdrawn 1, pending 0 and published 0.
- tk was asked at 314.35, and the next instance (pid 12913) starts at 317.44.
- tw had been accepted and looked up while the store was up, so it was waiting for a cut. Had the withdrawal left it queued, cut 19's commit would have made it before the kill.

**pkillman and pkillpub.**
- `killed_rc 137` in both.
- The hook's words are in serve.log.
- pkillpub's ref after the kill is on the commit that p1-end's `done` names.

**pchannel.**
- The counts are `control_answered 4`, `control_peer 1` and `control_malformed 2`, and the socket's mode is 600.
- `control_answered` is counted only on the path that hands a request on (`bins/skyzfsd/src/control/server.rs:310-311`). The peer's refusal returns before it (`:293-302`), so the exact 4 does show that the peer's request was not handed on.

**The rest.** No watcher VERDICT line in any course.

## Findings, most severe first

None blocks the merge.

### D1. In prewind, a premise checked after the pins can turn a FAILED server pin into exit 2

**Where.**
- `tests/kernel/publish_check.py:425-428`: a premise that fails raises, and the course ends 2.
- `:353`: `named < number` fails the premise.
- `:554`: the premise is checked after r1's pins have been printed.

**Scenario** (replayed on the run's own prewind files, `replay_faults.out`):
- A daemon without "opens as itself" makes r1 on the rewind-only cut 21 of group 31, while node B's row read later names cut 22.
- `pin_r1_cut` is FAILED ("cut 21 (group 31) opens only with a rewind").
- Then `premise_rewind_cut_committed` is ERROR ("r1 was made by cut 21, before cut 22 was committed"), and the course ends **exit 2**: the rig's error.

**Why `named < number` cannot guard a right daemon.**
- Within one history, a cut counted earlier is of a lower group.
- So a cut counted before a rewind-only cut opens only as a rewind too.
- If r1 was made by such an earlier cut, the daemon is at fault, and the branch only reclassifies that fault.

**How likely.**
- In the shape the run took, the uploader commits the oldest held cut (below C) and then the newest (cut 22). The same fault then lands on cut 22 and ends exit 1, as it should.
- Reaching exit 2 needs two rewind-only cuts past C to be committed, for example if `CUT_S3_DOWN` did not take the store down.

**A related case.**
- A daemon that labels every cut `itself`, in its uploader and in its log alike, fails the same premise ("opens as itself"). The course again ends 2.
- Node B's import of a rewind-only cut would most likely open the abandoned history, and so fail the snapshot stage.
- The premise and `pin_r1_cut` both take "opens as itself" from the daemon's own `cut` line. The rig knows the rewind group (`rewind-txg`) and the groups before it, and could tell it on its own.

**Certainty.** High that the code ends 2 in these cases; low that a run meets one. The FAILED line is still in the summary.

**Suggest.**
- Fail the premise on `named < number` only when cut `named` opens as itself in the log, and leave the other case to `pin_r1_cut`.
- Or end 1 whenever a `server` line has FAILED, whatever premise fails after it.

### D2. `raw` takes a whole line with no greeting as the answer; skyzfsd's own `ask` refuses it

**Where.**
- `tests/kernel/publish_agent.py:234`: `rest = … else whole`.
- `tests/kernel/test_publish_agent.py:356` pins this: `(b'cuts 1\n', 'cuts 1')`.
- skyzfsd's `ask` refuses such an answer (`bins/skyzfsd/src/control.rs:682-686`, "the daemon greets with …, not `skyzfs-control 1`").

**Scenario.**
- A skyzfsd whose peer refusal (`server.rs:299`) lost its greeting would pass `pin_answer_peer`.
- Its real clients of another user would then get an error, not `refused peer`.
- Probe at c7cc03f (`next_probe.out`): "the answer with no greeting" gives ok, 20 of 20.

The author's answer says "only a whole line after the greeting is an
answer", which the code does not do.

**Certainty: high. Suggest:** answer `no answer line: …` when the first whole line is not the greeting.

### D3. A daemon that never answers, or that has gone, is the rig's error

**Where.** `tests/kernel/publish_agent.py:194` (a 60 s limit) and `:215-218`. The docstring says so.

**Probe** (`next_probe.out`, with the limit cut to 1 s for the probe):
- a socket accepted and never answered: `timed out`, exit 2;
- a socket file with no listener: `ECONNREFUSED`, exit 2.

**Why it matters.**
- In pchannel the rig started the daemon and has already had answers from it. A hang of 60 s after `connect` then points at the daemon's accept loop, not at the rig.
- A daemon that died on one request answers it `''` (a verdict). The next `praw` then fails to connect, so the course ends as the rig's error before the check runs.

**Certainty: high** that it is classed so; whether to class it so is a judgement call. **Suggest:** after a successful `connect`, a read that times out is the answer `no answer within 60 s`. Leave the rest as it is.

### Test gaps (suggestions)

Each plant below survived the pull request's tests. None makes a course pass a fault in a kernel run.

| plant | what it does | why it is not worse |
|---|---|---|
| M10 | `pin_asking_twice_is_once` compared with itself | No driver case gives `t1-again-end` another commit. In a kernel run a second transaction would move the ref off t1's `done` commit, and `pin_t1_row` would fail. |
| M7 | `premise_held_at_rewind` takes any status | No driver case has `cuts_held 0`. The premise now only backs up `premise_rewind_cut_committed`. |
| P9 | `main` never hands r1's cut to the premise | `test_prewind_with_r1_made_before_the_rewind_cut_ends_2` (`test_publish_check_main.py:349-351`) ends 2 because its row's cut 10 opens as itself, not because r1 came before it. Its name says what it does not test. The branch is covered in the unit test (P2 is killed). |
| S3, S4, S10 | the arguments at `row_past`'s call sites (`cut-record.sh:1599`, `:1628`): nudge or not, the order of C and G | Only the order of the calls is tested. Each plant makes the course slower or red with exit 2, never a pass. |
| M14, P8, Q6, Q7, A20 | equivalents in these courses | One cut state and one numbering per log; `praw` accepts exit 0 and 1 alike. |

### The description

All of it is up to date except one sentence. prewind's sentence "so the
abandoned history's cuts are held when the publish is asked" does not
match this run, and the check no longer rests on it.
- The two cuts held at the rewind were most likely cut 1 and cut 18, by the figures above.
- At the ask they were cut 1 and cut 22, the present history's rewind-only cut.
- The abandoned history's cuts had been replaced.

Suggest: "a cut of the present history that opens only as a rewind is
held when the publish is asked".

## What was not taken: do the reasons hold?

- **W5 to W7.** The reason is "call sites in the cut rig and node B, outside this package". That is not accurate: `withheld_pick` and its two call sites came with aefc6a4, in this pull request. They were non-blocking, though, and putting them in the next test-gap package is fine if the lead records that.
- **N7.** The reason holds. The request log has every counted cut but not which ones the uploader committed (it replaces held cuts), and `rows.tsv` samples commits. In this run prewind's `rows.tsv` shows commits 3 to 6 as cuts 23 to 26, all rewind-only, and cut 27 made r1, consistent with "the first qualifying".
- **N8.**
  - `judge_cut` has no state to match: the `cut` line has no state column, and `judge_row` checks the row's state. This holds.
  - The `PREMISES` docstring, `raw`'s exit codes and `ctl_commit`'s ids are fixed.
  - The watcher's VERDICT lines: deferring them is fine, but the gap is real. `cut-record.sh:438-439` says the watcher's verdict "goes to the check", and `publish_check` does not read `watch.log`. No course of this run has one.
  - The stand-in agent's hold and t4's counted-versus-committed cuts: deferring them is fine.
- **N3's `none ` for m1/p1.** The reason holds now. The kill's own premise (exit status 137 and the hook's words) is judged, and so are the found/absent pins.
- **N6's peer request with an effect.** The reason holds. The exact `control_answered 4` shows the request was not handed on, given where the server counts it.

## How it was checked

**Local gates** (`next_gates.out`, rustc 1.100.0, the current stable):
- `unittest discover` in tests/kernel: 599 OK.
- The three publish suites: 19, 23 and 22 tests OK.
- `test_cut_record.sh`: all passed.
- shellcheck: clean.
- `cargo fmt --check`: clean.
- clippy on skyzfs-nodeb with `-D warnings`: clean.
- `cargo test -p skyzfs-nodeb`: 24 passed.
- The new agent tests passed 50 of 50 alone and 48 of 48 run 16 at a time. The main driver passed 20 of 20.
- The head merges cleanly into `zfs-topology`.

**Plants on the check, the agent and the course script**
(`next_plants_py.py`, `next_plants_py.out`, `.json`). Each plant was
applied alone, and the file was put back in a finally. KILLED and
SURVIVED are against the pull request's own tests.

| group | killed | survived |
|---|---|---|
| E (tables; B4) | E1 to E20 | none |
| M (main's wiring; N1, M3) | M1 to M6, M3f, M8, M9, M11 to M13 | M7, M10, M14 |
| P (prewind's premise; B2) | P1 to P7 | P8 (equivalent), P9 |
| Q (pkill's premise; B3) | Q1 (B3 as it was), Q2 to Q5, Q8 to Q10 | Q6, Q7 (equivalent here) |
| K (kill premises, `pin_p1_found`; N3) | K1 to K4 | none |
| C (counts, socket mode; N6) | C1 to C4 | none |
| A (`raw`, `answer_of`; N5, X1) | A13 to A19 | A20 (equivalent) |
| S (course script) | S1, S2, S5 to S9, S11, S12 | S3, S4, S10 |

**Plants on node B** (`next_plants_rs.py`, `next_plants_rs.out`, `.json`):
- SR11, SR14, SR15 and SR16 are KILLED by `snap_row_reads_what_the_uploader_writes_where_it_writes_it`.
- SR1 and SR17 (controls) are KILLED.

**The daemon's own faults** (removing "opens as itself" from `carried_by`;
a `withdraw` that leaves the publish queued) need a kernel, so they were
not planted. `replay_faults.py` instead puts each through
`publish_check.main` on run 45539186244's own recordings: the request
log, the answers, the row files and the statuses. Node B is stood in for
and reports what the faulty daemon would have put in object storage
(`replay_faults.out`).

| case | exit | lines |
|---|---|---|
| prewind as run | 0 | |
| prewind, r1 on cut 22 | 1 | `pin_r1_cut` FAILED, the premise ok |
| prewind, r1 on cut 21 | 2 | `pin_r1_cut` FAILED, then the premise ERROR (D1) |
| pkill as run | 0 | |
| pkill, tw on cut 19, the status as read | 1 | `pin_tw_absent` FAILED |
| pkill, tw on cut 19, the status a queued daemon posts | 1 | `pin_tw_absent` and `pin_tw_withdrawn_status` FAILED |

**The raw client** (`next_probe.py`, `next_probe.out`, 20 trials per case):

| the daemon… | answer | classed |
|---|---|---|
| answers, request unread | the answer | ok |
| answers, closes before the client writes | the answer | ok |
| answers after reading the request | the answer | ok |
| half a line, either order | `no answer line: …` | verdict |
| greeting only, either order | `no answer line: skyzfs-control 1\n` | verdict |
| nothing, request unread (reset) | `''` | verdict |
| nothing, closes before the client writes (EPIPE) | `''` | verdict |
| nothing, after reading the request | `''` | verdict |
| the answer with no greeting | the answer | ok (D2) |
| another greeting | `skyzfs-control 2` | verdict |
| no listener | ECONNREFUSED | rig's error (D3) |
| accepted, never answered | timed out | rig's error (D3) |

**CI at c7cc03f, read through the API: every pull-request run is green** (the push runs of the same head were cancelled).
- The publish rig (run 45539186244). Its unit job runs the new driver.
- The node B rig.
- The cut rig on four images, and the cut rig through skyzfsd (with the upload job).
- The daemon rig: the daemon on three images, the stand-in on two.
- The replay rig, including the jobs through skyzfsd.
- The control rig and the kernel probe.
- ci (including `unit-arm64`), coverage, e2e and k3s-smoke.
- Codex's state for c7cc03f: completed, with no findings and none unresolved.
