# Next look at skyzfs/skyzfs#150 (M2.6c-4: the kill points `carry-put:N` and `carry-swap:N`)

| | |
|---|---|
| Head judged | `d629834` (`d629834be9c82bd895362a085e0941827fdcb9ef`), one commit on `aeeb3cd`; unchanged when this note was written |
| Base | `8f0390f` (the tip of `zfs-topology`) |
| What was read | `git diff aeeb3cd...d629834`: `bins/skyzfsd/src/{kill.rs, kill/tests.rs, upload.rs, upload/tests.rs}`, against `review.md` beside this note and the author's answer (issue comment 7174745360) |
| Codex | completed on `d629834` with no findings |
| CI on the head | all green (list at the end) |

## Verdict

**Not ready to merge as it stands. One small fix remains (R2).**

- **B1 is fixed.** The test now drops the receiver before it sends. In 6000 runs at the head it failed 0 times. At the old rate (6 in 9000), about 4 failures were expected.
- **F1(a), F2, Z, S1 and S2 are built as the lead ruled.** Each is tested through the real `Driver` and `kill::maker`.
- **Plants: 22 of 26 are caught by the pull request's tests.** That includes all nine of the author's I ran, and M3, M4, M6, M7 and M13, which survived at `aeeb3cd`. Of the four that survive:
  - N1 is caught by a scratch test of mine (G2);
  - N5 and N6 are a minor gap (G1);
  - M16 is unchanged (G3).
- **R2 blocks, in my view. It is a defect against F1(a) as ruled and against the module's own docs.** Certain in the code, unlikely in a run. Suppose publish N's transaction lands, but its call comes back failed. Then the hook is never told about the publish. It kills at a later cut, after `snap/<name>` exists, and the kill line says that cut was "chosen to carry publish N". The fix is a few lines in `KillRemote::publish`.
- **T is a suggestion: the docs misdescribe the residual window.** The window is real and needs no fault. It is the uploader's ordinary turn order whenever a publish's lookup took longer than the next cut's check and read. I built it in cases A and B, for both hooks. It is benign for M2.6's purpose. But the docs need three corrections, and the author's answer says the order cannot be built, which is wrong.
- **The default build is unchanged.** The S1 lines and the new `posted` argument reach nothing outside a test-hooks build.

## Findings, most severe first

### R2. Defect, certain in the code, unlikely: publish N lands but its call fails, and the hook kills after it

`bins/skyzfsd/src/kill.rs:790` (`self.inner.publish(commit, snapshot).await?;`) returns at the error, so `carry.published(...)` at `kill.rs:799` is never reached.

**The course.** `carry-swap:1`, case A:

1. Publish 1 ("t", *C* 6, *G* 7) is accepted while cut 7 uploads.
2. Cut 7 commits.
3. The publish's transaction lands: the `snap/` PUT and the refs `put_if` take effect. Its call then comes back as an error, here from a fault "after the operation completed". In a real run, a response lost after the write is the same thing.
4. The uploader calls `Driver::publish_failed` (`crates/skyzfs-upload/src/uploader.rs:603`), and the publish waits its first second.
5. The cut has the steps meanwhile. Cut 8 is read, which makes it chosen ("cut 8 (group 8) chosen to carry publish 1"), and committed. The hook then kills with `carry-swap:1: after the commit of cut 8 (group 8), chosen to carry publish 1, landed, before the cut was let go`.
6. By then publish 1 has been made on cut 7.
7. Without the kill, once the wait is over, the recheck finds `snap/t` and ends the publish `Published { number: 7, found: true }`. That path does not call `Remote::publish`. The hook would learn of the publish only at the post that follows, through `Carry::ended`, a second or more after the publish was made.

**Built.** `r150n_show_r2_a_publish_landed_whose_call_failed_lets_the_hook_kill_after_it` in `scratch150n.rs` passes at the head (`run150n.out`). Its log shows `put .../snap/...` and `put_if .../refs` before the kill line, no "no kill" line, and the later `found: true` on cut 7. `carry-put` goes through the same code. I did not run it under `carry-put`.

**Why it matters.**

- The course does not test what it is for: a kill between the acceptance and the ref.
- The module docs say: "where publish N is made, on any cut, or ends without being made, before the hook has fired, the hook says so and gives up" (`kill.rs:63-65`). That is the lead's F1(a).
- After the restart, the row names cut 7 and the kill line names cut 8. A rig that compares the two, as the docs advise, sees a mismatch. It will read that mismatch as the daemon making the publish on the wrong cut, unless it is told about this hole.

**The fix (small).** In `KillRemote::publish`, tell the hook on an error as well as on success. When publish N's call fails, give up, with a line such as `publish 1 (t): its call failed, it may have been made on cut 7 (group 7); no kill`. Or tell the hook before the inner call. Either way:

- R2 is closed.
- The course of R1 (below) also gives up rather than kill. That is conservative, and costs only that serve's kill.
- A test like my R2 course pins it.

### T. Suggestion (docs), certain, reachable with no fault: the window the docs leave open is the uploader's ordinary turn order

`kill.rs:65-68`: "Where the chosen cut's commit lands before the publish's step has made it on another cut, the hook still fires; the kill line names the cut chosen, which a rig compares with the cut the snapshot's row names after the restart."

The author's answer says: "I could not build that order with the driver: in each of your courses the publish's step runs before the next cut's commit." That holds for `Driver::step`, which always takes a publish's step first (`crates/skyzfs-upload/src/driver.rs:412-418`). The uploader does not use it:

- It takes `step_publish` or `step_cut` as `Turns::next` says (`uploader.rs:159-176`, called at `uploader.rs:502`).
- While both sides are due, the side that has had less time since both became due goes next. Ties alternate.
- The counts restart only at an idle cut step (`uploader.rs:526`).
- Its own test `a_slow_step_of_a_publish_gives_the_cut_as_long` (`uploader.rs:803`) shows one publish step followed by twenty cut steps.

**The course, case A.** Publish 1 is accepted while cut 7 uploads.

1. The publish's lookup takes a turn, a GET round trip, and that time counts for the publish.
2. Cut 7 commits, and the transaction is due at once.
3. The cut has had less counted time, so it takes the next turns: cut 8 begins and is read (chosen).
4. Cut 8 commits. That is the kill for `carry-swap`. For `carry-put`, the kill comes at cut 8's first segment PUT.
5. The publish's next step would have made it on cut 7.

**Case B** (cut 7 committed before the acceptance) runs the same way after the lookup.

No fault is needed. It is enough that:

- the lookup took longer than the next cut's begin and first read, which are local and fast;
- a next cut was ready, as it is under steady writes.

**Built.** These pass at the head, for both hooks:

- `r150n_show_t_the_uploaders_turns_let_the_hook_kill_at_the_next_cut` (case A);
- `r150n_show_t_case_b_the_uploaders_turns_let_the_hook_kill_at_the_next_cut` (case B).

Each drives `step_cut` and `step_publish` in the order above, through `kill::maker`. Each asserts that the publish was due when cut 8 was checked, that the kill falls at cut 8, and that the publish's next step makes it on cut 7. I did not drive the real `Uploader`, whose turns depend on measured times. The order is one its `Turns` gives.

**Why it is benign.**

- The kill still falls between the acceptance and the ref.
- Cut 7 was let go before the kill. A new uploader knows only the cuts it has committed or found committed since it began (`crates/skyzfs-upload/src/publish.rs:10-12`).
- So after the restart, cut 8 is found committed (`carry-swap`, not let go) or uploaded again (`carry-put`). The publish asked again, with the same *C*, *G* and cut state (the cut state outlives the daemon, design §5.2), is made on cut 8. That is the cut the kill line names. I reasoned this from the code and did not run a restart.
- The course therefore tests a kill at the commit, or segment PUT, of a cut that ends up carrying publish N.

**What the docs get wrong.**

1. They do not say how the window is reached, and the answer says the order cannot be built. It is the uploader's ordinary order whenever the publish is ahead in counted time.
2. For `carry-put` the window ends at the chosen cut's first segment PUT, not at its commit.
3. The rig's comparison does not detect T: the kill line and the row both name cut 8. It detects R2, where they differ (8 against 7). T needs no detection. The docs should say that in T the restarted serve makes the publish on the cut the kill line names, and that a mismatch means something else (R2, until it is fixed).

**Optional closure in code.** At the moment of firing, give up where a cut other than the chosen one, committed in this serve, carries publish N. `KillRemote::commit` and `KillLocal::release` see every commit, and the cut offered gives its state. Case B also needs the commits made before the acceptance. I do not recommend this unless the lead wants the kill line exact for the killed serve; the docs fix is enough for M2.6.

### R1. Note, documented: a publish's transaction that fails before it lands

A refs write that fails without landing makes the publish wait (1 s). The cut has the steps meanwhile, and `carry-swap` kills at cut 8. After the wait, publish 1 is made on cut 7.

`r150n_show_r1_a_failed_publish_step_lets_the_hook_kill_at_the_next_cut` passes at the head. This is the window the docs name, reached through a failure. It is benign for the same reason as T. If R2 is fixed by giving up when the call fails, this course gives up too.

### G1. Test gap, minor: the hook's "done" checks in `ended` and `published` are not tested through the post

Plants N5 and N6 survive both the pull request's tests and mine. Each removes the `seen.done` check:

- N5 removes it in `Carry::ended` (`kill.rs:335-338`);
- N6 removes it in `Carry::published` (`kill.rs:359-366`).

N5 matters in an ordinary course:

1. Publish N is made on a cut the hook did not choose.
2. `published` gives up and says so.
3. At the next post, `posted` passes the ended publishes, publish N among them as done.
4. Without the check, `ended` says a second "… ended (…) before the hook fired; no kill".

The pull request's F1 tests drive the `Driver` and do not post after the publish, so nothing catches it. The effect is only an extra line on standard error, but a rig that reads those lines would see two give-ups.

**Suggestion.** In the F1 tests, call `carry.posted(&driver.publishing().take_ended())` after the publish's step and assert the lines in full. N6 is the same, the other way round.

### G2. Test gap, minor: no test of the pull request orders the kill against the release

Plant N1 fires `carry-swap` after the inner release rather than before it. It survives the pull request's tests and is caught only by my `r150n_carry_swap_at_the_release_fires_before_the_cut_is_let_go`, whose store logs its releases. The kill line says "before the cut was let go", so a rig reads a guarantee into it.

**Suggestion.** Take that test, or assert the order in `carry_swap_fires_as_the_chosen_cut_is_let_go_where_its_commits_call_failed`.

### G3. Unchanged: M16 and M15

- **M16:** an ask still waited on is kept after its asker went. It survives as before. The window is microseconds wide and cannot be reached deterministically (`review.md`).
- **M15:** the wiring in `Uploads::begin`. It is left to a rig, as the answer says.

## Answers to what the coordinator asked

### 1. The residual window in `kill.rs`'s module docs

- **Real:** yes.
- **Reachable:** yes, with no fault, through the uploader's turns (T), and also through a failed publish step (R1).
- **Buildable with the Driver harness:** yes, with `step_cut` and `step_publish` in the order `Turns` gives (T, case A and case B, both hooks).
- **Correctly described:** no, in three respects (T above).
- **The case the docs claim to cover but do not:** R2. It is not the residual: there the publish is made before the kill.

### 2. `letting_go` at the release (F2)

**It is the point I meant.** `KillLocal::release` calls `Carry::letting_go` before the inner release (`kill.rs:573-576`). It fires only:

- under `carry-swap` (`kill.rs:488-490`);
- for the chosen cut (`chosen.number == number`);
- where the cut opens as itself and the hook has not fired (`Carry::fires`, `kill.rs:434-442`).

**It fires exactly once.** `fires` sets `done`, and the kill ends the process.

**Never before the commit landed.** The driver releases a cut only in two places:

- `found_committed` (`driver.rs:597-599`): the check found the row naming the cut;
- `uploaded` (`driver.rs:772-782`): after a commit that came back Ok, or a failed commit whose row names the cut.

Both come after the commit landed.

**Never on a cut without a commit.** A cut whose commit did not land is read again from block 0. It stays chosen (`reads` keeps a chosen cut read again), and it fires at its later commit through `Carry::committed`. An abandoned cut is not released.

**A chosen cut found committed** must have been read after the arming in this serve. So its commit landed in this serve, after the reading: this is F2's case.

**The order of kill and release is pinned.** `r150n_carry_swap_at_the_release_fires_before_the_cut_is_let_go` logs the store's release. It shows the kill line ("…landed (its call failed; seen as the cut was let go), before the cut was let go") before `release 7`. No test of the pull request orders the two: plant N1, which fires after the release, survives them and is caught by this test (G2).

### 3. F1(a): a kill at a cut that does not carry publish N while the line says it does

- **Case A, case B and Z under `Driver::step`:** the hook gives up with the right line. These are the pull request's tests, and the plants Q1, Q7 and Q8 are caught.
- **Under the uploader's turns (T):** the hook kills at cut 8 while, in the killed serve, the publish would be made on cut 7. After the restart it is made on cut 8. So the line is true of the outcome, not of the killed serve. Benign: see T.
- **R2:** the one course where the hook kills after publish N was made on another cut. A defect (above).
- **Publish N ended without being made** (expired, or ended refused once accepted, as when the uploader stops, `publish.rs` "A stopped uploader publishes nothing"): `Around::posted` passes the post's ended publishes, and `Carry::ended` gives up (`kill.rs:334-352`). This is caught by `a_publish_n_that_ends_without_being_made_gives_the_hook_up` (plants Q2 and Q6b).
- **A publish dropped because its asker went:** not counted. `answered`'s `Err` arm takes it as not answered (`kill.rs:319-323`). Plants P9 and M16 have the results.
- **A publish refused, including after a rewind:** not counted, since only `Answer::Accepted` counts (plant N2).
- **S2, distinct names.**
  - A name accepted again is not counted again (`names.insert`), for N=1 and N=2 (`a_name_accepted_again_is_not_counted_again`; plants Q4b, N3 and N4).
  - Publish 1 made while publish 2 is armed does not give the hook up (plant Q5).
  - A name asked again after publish N ended is not counted. The hook is done by then, so there is no kill in that serve. Conservative, and correct.

### 4. B1's flake

The test-hooks test binary at the head was copied aside and run one test per process, 6000 times, with its own target dir and `CARGO_PROFILE_DEV_DEBUG=0`. **0 failed** (`b1repeat150n.out`). At the rate measured at `aeeb3cd` (6 in 9000), about 4 failures were expected; the chance of none at that rate is about 2%. The fix is the one `review.md` gave.

### 5. Do the tests fail when each new behaviour breaks? And the default build

**How the plants were run.** Each plant was applied alone to a clean worktree of the head. Then `cargo test -p skyzfsd --features test-hooks -- kill:: carry the_test_hooks_are_parsed_in_a_test_build` ran with a 600 s limit, and the file was restored in a `finally` whatever happened (`plants150n.py`). The verdicts:

- **KILLED:** a test failed, or the limit was reached.
- **SURVIVED:** all passed.
- **NOT APPLIED:** the text was not found exactly once, or the plant did not compile.

A second pass ran the survivors again with my scratch tests appended (`SCRATCH=scratch150n.rs`).

**The plants.**

- **Q1 to Q9** are the author's, from the answer's table.
- **Q4 and Q6** as I first wrote them did not compile. **Q4b and Q6b** are the same changes, written so that they compile.
- **P9 and the M plants** are from `review.md`.
- **The N plants** are new.

| Plant | What it breaks | The pull request's tests | With my scratch tests |
|---|---|---|---|
| Q1 | `KillRemote::publish` does not tell the hook | KILLED | |
| Q2 | the ended publishes not read | KILLED | |
| Q3 | no fire at the chosen cut's release | KILLED | |
| Q4b | every acceptance counted | KILLED | |
| Q5 | any publish made gives the hook up | KILLED | |
| Q6b | any publish ended gives the hook up | KILLED | |
| Q7 | the chosen cut not told apart in the give-up line | KILLED | |
| Q8 | the release fires under `carry-put` too | KILLED | |
| Q9 | the release of any cut fires `carry-swap` | KILLED | |
| P9 | an ask whose asker went still waited on (B1's test) | KILLED | |
| M3 | a cut numbered C chosen | KILLED | |
| M4 | a cut of group G not chosen | KILLED | |
| M6 | C not checked | KILLED | |
| M7 | a read from a later block counts as a new reading | KILLED | |
| M13 | `maker` does not give the hook to the objects | KILLED | |
| M16 | an ask still waited on is kept after its asker went | SURVIVED | SURVIVED (G3) |
| N1 | `carry-swap` fires at the release after the cut was let go | SURVIVED | KILLED (G2) |
| N2 | a refused name counted among the names | KILLED | |
| N3 | armed again at each new name after the Nth | KILLED | |
| N4 | armed one name early | KILLED | |
| N5 | `ended` without its `done` check (a second give-up line) | SURVIVED | SURVIVED (G1) |
| N6 | `published` without its `done` check (a second give-up line) | SURVIVED | SURVIVED (G1) |
| N7 | a publish made on the chosen cut does not give the hook up | KILLED | |
| N8 | the arming line names G for C and C for G | KILLED | |
| N9 | the end of a choice never said | KILLED | |
| N10 | a chosen cut that does not open as itself not said so | KILLED | |

- The author's claim of 13 of 13 holds for the nine of their plants I ran: Q1 to Q9, with Q4 and Q6 as Q4b and Q6b.
- M3, M4, M6, M7 and M13, which survived at `aeeb3cd`, are now caught by the pull request's own tests.
- No plant hit the 600 s limit.

**The S1 lines and the default build.**

- `kill.rs` is compiled only with `cfg(all(target_os = "linux", feature = "test-hooks"))`.
- The CLI takes the hook flags only in a test-hooks build (`cli.rs:739`).
- In `upload.rs`, the trait method became `posted(&self, ended: &[(String, Progress)])`. The default `Plain` keeps it a no-op, and the call `around.posted(&s.publishes.ended)` (`upload.rs:1161`) borrows a vector the post already holds.
- Plain clippy and the plain tests are clean.

## Gates

They were run at the head on toolchain 1.100.0 (`rustup update stable` reported it current), on `skyzfsd`, the only crate changed. The logs are beside this note (`next-gate-*.log`).

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy -p skyzfsd --all-targets -- -D warnings` | clean |
| the same with `--features test-hooks` | clean |
| `cargo test -p skyzfsd` | 142 passed, plus 3 in `closed_output` |
| `cargo test -p skyzfsd --features test-hooks` | 194 passed, plus 3 in `closed_output` |
| my scratch tests (`run150n.sh`) | 5 of 5 pass at the head (`run150n.out`) |

## CI on `d629834`, read only

| Run | Workflow | State when read |
|---|---|---|
| 45701864095 | skyzfs-ci | success |
| 45701879421 | ci (`test`, `unit-arm64`) | success |
| 45701879333 | coverage | success |
| 45701879707 | e2e | success |
| 45701879489 | publish rig | success |
| 45701879577 | cut-daemon | success |
| 45701879699 | replay rig | success |
| 45701879479 | daemon rig (24.04, 26.04 and 26.04-arm, on the daemon) | success, ended 21:42 UTC; job conclusions only, the summary tables were not readable through the REST API |

**Not covered.** No job starts a daemon with `carry-put` or `carry-swap`. So M15 (the wiring in `Uploads::begin`) is still shown by no test, and CI has not shown the hook in the daemon. That is testing's item 7.

## Decisions for the lead

1. **R2.**
   - **(a)** Fix it in this pull request: give up when publish N's call fails, or tell the hook before the call, and add a test. **My recommendation; I would hold the merge for it.**
   - **(b)** Accept it as a residual. The module docs must then say so, and the rig that drives `carry-*` must know that a mismatch after a failed publish call is the hook's, not the daemon's.
2. **T.**
   - **(a)** Correct the module docs as above. **My recommendation; it can go with R2's commit.**
   - **(b)** Also close it in code, so that the kill line is exact for the killed serve. I do not think M2.6 needs it.
3. **The rig (testing's item 7).**
   - In T, the kill line's cut and the row's cut after the restart agree, and the course is a valid kill between acceptance and ref.
   - With R2 fixed, a mismatch is a finding against the daemon. With R2 open, it can be the hook's.

## Files beside this note

- `scratch150n.rs`: the scratch tests of this look, appended to `kill/tests.rs` for a run.
- `run150n.sh`: runs them against a clean worktree of the head and restores the file.
- `b1repeat150n.sh`: builds the test-hooks test binary, copies it aside, and runs B1's test `REPEAT` times.
- `plants150n.py`: the plants, each alone, with a 600 s limit and a restore in `finally`.
- `run150n.out`, `b1repeat150n.out`, `plants150n.out` (the first pass), `plants150n-q.out` (Q4b and Q6b), `plants150n-scratch.out` (the survivors with my scratch tests), `next-gate-*.log`: the outputs. Paths are replaced by `<worktree>`, `<target>` and `<scratch>`.
