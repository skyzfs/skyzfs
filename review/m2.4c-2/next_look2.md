## Next look at #118, at da0b4c9 (M2.4c-2)

**Verdict: nothing of mine blocks the merge any more. F1, the one finding that blocked, is fixed, and so are F2 to F6, G2 and the two tests owed from #117; the plants and scenarios bear the fixes out. Three things stand between this head and "ready to merge", none of them a blocking finding of the review: Codex's P2 on a18bedf ("Bound runtime shutdown after the create timeout") is right and has no answer yet (2.1 below, not blocking on my reading); G1 is the lead's, and testing's courses are not all green on this head, by the rig's own errors as far as the artifacts show (urow, then ulease); and the daemon's wiring in `run` is still reached by no test in the workspace (W01 to W05, W08, W12 pass every test, as before).**

Read first the code at da0b4c9, then the answer (issuecomment-6973378316) and the description. The answer and the code agree in every point I checked.

### What I did

- Built and ran the head's tests in both builds (rustc 1.100.0, `CARGO_INCREMENTAL=0`): with `--features test-hooks` skyzfsd 134 + 3, plain 98 + 3; skyzfs-upload 26, 3, 22, 7, 24; skyzfs-ublk 152, 1, 1. All pass (`t2_hooks.out`, `t2_plain.out`).
- 94 plants, one at a time, in two scratch worktrees (`plants2.py`, `plants2.out`; the table is at the end): 79 KILLED, 15 SURVIVED, none NOT APPLIED. Each of the 15 is accounted for below.
- 19 scenarios against the daemon's own code in both builds (`scenarios2.rs`, `scenarios2.out`): the 14 of the first look, adapted, and 5 new. 17 pass; the two that fail are those of finding 2.1, which are written to fail while the defect is there.
- CI on da0b4c9 and Codex's threads, read once at the end (09:16 UTC). Testing's dispatched course runs 44268306498 and 44272704862, both with the daemon built from da0b4c9, and their artifacts.

### The first look's findings

**F1 (blocked): fixed.** `place` takes a volume found under the name only where no clone-base object exists for it (`bins/skyzfsd/src/upload.rs:316`) and its head is the commit a create makes (`as_created`, upload.rs:444: this volume, no parent, no segments, no group, no flush, epoch 0, a checkpoint, the empty index). Plants A00 to A09, one per condition, all die. My S1 (a clone that holds data) is now refused: "is a clone of another volume: what it reads is its base's."

**F2: fixed.** The thread calls `runtime.shutdown_background()` before it sends the end's answer (upload.rs:927). With the call removed (N01) `the_end_does_not_wait_for_work_left_on_the_runtime` fails. The test is sound: it leaves 5 s of blocking work on the daemon's own runtime and holds `end(true, 2 s)` to less than its limit and to `Released`. My D7, with 3 s of blocking work and a limit of 6 s: `Released` after 151 microseconds. The same shape is still there in two other places: 2.1. The thread also posts nothing once the desk is closed: N02 dies by `nothing_is_posted_once_the_desk_is_closed`, and in scenario `n4_...` (a second cut under way, 2 s between the queue's end and the thread's end) nothing was posted after the close.

**F3: fixed.** `shown_of` names this daemon's holder while the lease is held or unsure, and `Start::shown` at the start. N03, N04, N05 die; S2 reads "node-a" at the start and after a turn.

**F4: fixed.** One line, and the test checks the words and that no two spaces stand together.

**F5: fixed, with one line unpinned.** `fixed_keys` returns the region with the keys and returns none where a session token is set; `region` is `AWS_REGION` or us-east-1 where it is unset or empty. Y01, Y02, Y04 die. Y03 survives: `connect` passing the constant instead of `&keys.region` (upload.rs:166) passes every test, because no test reaches `connect`. The test pins what `fixed_keys` returns, not what `connect` signs with; the distance between the two is that one argument. A gap, small, by reading plus the plant; not blocking.

**F6: fixed.** The three texts read as the answer says.

**G1: the lead's.** See "The courses" below.

**G2: fixed.** The thread's tests make their remote with the daemon's `maker`, and `the_daemons_remote_commits_under_the_epoch_it_is_made_for` kills U20p (fenced by another epoch than the one held), U21 (commits carry epoch 0) and U26 (commits made as another holder), each in the default build. The test checks both things: a remote made for the epoch before or after the lease's is refused by the fence and lands nothing, and the commit of the right one carries the lease's epoch.

**G3.** W13 and U12 are pinned (both die). W07 is left, and I accept the reason: the figure is printed only by `run`, and the plant changes nothing else.

### The items named for this look

**The two tests owed from #117 (P2, P3): they die now.** With "a cut found committed does not end the refusals' wait" planted, `a_cut_found_committed_ends_the_wait_of_the_refusals` fails; with the same for a cut committed and gone, `a_cut_committed_and_gone_ends_the_wait_of_the_refusals` fails. The test remote `Refusals` does what is needed for that: it lands the commit while it answers a conflict and hides it from the next read, so the cut is found committed (or committed and gone) while the refusals' backoff is raised. P3b (an upload does not end the wait) and P3c (any step that goes on ends it) die too.

**F3 of #117, the two waits and `rereading`: pinned, one clause aside.** E1 to E4, E6 and P4b die: the errors' wait is reset by any step that goes on, the refusals' only by a cut uploaded or found committed; a give-up waits in the refusals' backoff and an error in the errors'; a conflict right after a refused commit is waited after. E5 survives: with the clause `doing == Doing::Committing` taken out of the guard (`crates/skyzfs-upload/src/uploader.rs:288`), a conflict at any stage is followed at once by one more step. That costs at most one step without a wait, since `rereading` stops the second. Small; no test asked for.

**The renewal beside a step, and `run` by value.** B1 to B5, X1, X3, X5, R1, R2, R3 die. R4 (run does not stop during the sleep between turns) passes the upload crate's tests and dies in the daemon's (R4d, by `an_end_with_no_lease_held_says_it_was_kept`). X4 survives (the rest after a lost lease counted from before the take, a difference of the take's duration). P15 survives, and the author's account of it is right: the lease's backoff that the line resets is not read again on that path; the line could go.

**The desk's sync of the copies in two turns.** Q8 and Q7c die by tests that do not end within the limit; Q7b, Q11 to Q15 die by name. Q16 survives: a sync that failed and is found at a later turn, answered as done. I looked for what follows and found nothing: the store has abandoned the cut by then, and the driver's check before the commit loses it. No effect I can show.

**The lease's gaps from #117.** K3, K5, K8, K9, K13 die, and P1 (no step while the lease is unsure) and P6 (a stopped driver stepped) too.

**Codex's P2 on 8fae8f7 (a18bedf): fixed, and the race test is sound.** Where there is no head, `reserve` (upload.rs:352) writes the superblock with If-None-Match before `Engine::create_volume`, and `place` goes on only where it wrote it or the one there is of its size and shards. `two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made` holds the second create at its first superblock write until the first is done; V01 (the write is unconditional again), V02, V04 and V07 die by it. I did not rely on that one interleaving: scenario `n2_...` runs two creates of one name on two threads with nothing held, 300 rounds per build, two sizes and one. In every round at most one says "made", and where there is a head the superblock is of the size of a create that did not fail.

**The accepted reading (da0b4c9).** A create that stopped before its head holds the name for its size, and the refusal names the object and the bucket (`refused_over`, upload.rs:397; it reads the head again and says "made it first" where there is one by then). V03, V05, V06, V08 die by `a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size`. The message says "If none is under way" before it tells to delete, which is what an operator needs to be told.

**The default holder.** `<hostname>:<machine-id>`, refused with "give --holder" where the id is missing, not 32 hexadecimal digits, or all zero. H01 to H05 die. The author names the case left (an image cloned with its machine id and host name is one holder on two machines; S4 shows one holder takes its own row again at the same epoch, so the fence does not tell the two apart). That is for the notes, as they say.

**`cuts_replaced`, status v15.** T02 to T05, S01, S03 die. S02 survives, and it is no gap: the count it removes is inside the branch of the test mutant `REPLACED_AT_INSTANT`. Testing's courses check the balance with it (cuts = committed and let go + replaced + abandoned + held) and it holds in every course of both runs.

### New findings. None blocks.

Defects:

**2.1. Small: `create`'s limit does not bound the create, and a start dropped before the thread can wait as long. Codex's P2 on a18bedf (review comment 4611033373), checked: it is right, and it has no answer yet.**
`place_within` (upload.rs:221) ends its wait for object storage after `PLACE_LIMIT` and returns, which drops its runtime; dropping a tokio runtime waits for the work left on its blocking pool, where the HTTP client looks names up. The same holds for a `Start` that is dropped before the thread is started (a refusal after the take, a device that cannot be added): it owns the runtime and has no `Drop` of its own. This is F2's shape in the two places F2's fix did not touch.
Scenario `defect_n1_...`: `place_within`'s body with the limit made 200 ms, a store that never answers and 3 s of blocking work: the error comes back after 3.0 s. `defect_n1b_...`: the drop of a `Start` that took the lease, with 3 s of blocking work left: 3.0 s. Certain of the mechanism. That a name lookup against a resolver that does not answer is such work, and how long it lasts, is by reading and not shown.
What it costs: `create` or a refused start that hangs past its stated limit, on a host whose resolver does not answer. Nothing is written wrongly. The same fix as F2 (`shutdown_background` in both places) would do, with D7's test as the model.

**2.2. Small: the loser of two creates of one size fails with the engine's words.** Of two creates of the same name and size at once, the one that loses the head's swap ends with "making the volume ... head CAS conflict: lease lost (I2)" (`crates/skynbd-engine/src/lib.rs:83`). Scenario `n2_...` saw it in 19 of 100 same-size rounds in the default build and 10 of 100 with the hooks. The volume is whole and the next create finds it ("found empty"), so nothing is lost; the message names a lease that create never had. A create could read the head once more on that error and say "found". Certain.

Gaps and notes:

**2.3. For the record: the no-head path does not look for a clone's base.** A create over the superblock and base marker of a clone that stopped before its head says "made" (upload.rs:280 to 292), and the next create of the name refuses it as a clone (scenario `n3_...`). No tool I know leaves a clone in that state without the engine's own create having failed half way, so I do not ask for a change; one `exists` before the engine's create would close it.

**2.4. By reading only: an empty `AWS_REGION` means two things.** For fixed keys it is us-east-1 (upload.rs:213). On the ambient path the region is given to the client only where the variable is unset (`var_os("AWS_REGION").is_none()`, upload.rs:172), so an empty one is left to the SDK. I have not run the SDK against an empty region and do not know what it does with one.

**2.5. Doc.** `crates/skyzfs-upload/src/queued.rs:24` says the daemon closes the desk "on every way out of the queue's thread". A panic of that thread is a way out on which it does not (D5: the desk was left open, and the drop ended the uploader in 30 microseconds with the lease kept); the same sentence goes on to name the main thread's close, which is what covers that case.

### The wiring in `run`, and the courses (G1, the lead's)

W01, W02, W03 (the queue's thread answers the desk, closes it at its end, is woken by it), W04, W05 (the lease is let go only at exit 0), W08 (the lease refusal's exit code) and W12 (the hook command `release`) still pass every test in the workspace, as at the first look. The author counts none of them as killed and points to testing's courses, which is the honest account.

The courses on this head, as I read them. I give this because the author's condition for the merge is the courses green on the head:

- Run 44268306498 (skyzfs-testing 32d8d67, daemon da0b4c9): life, umoved, ukill, ulease, ulost pass, with node B comparing the content of what it fetched at head and the status balance. **urow fails** (`uploaded_whole` 2 not 3, 120 mismatches not 2). From the artifact: the course read the cut's row with head at its fourth commit, and wrote its row after the daemon had committed two more; the row it wrote names a group that was no longer head's, and by the check's rule (a whole cut is uploaded only where head's commit is the row's) every whole cut is then a mismatch. That is what the daemon did, and it is right. Testing came to the same reading in 11fd271 ("the rig's race, not the daemon's") and guards node B's writes with the row it read.
- Run 44272704862 (skyzfs-testing 11fd271, daemon da0b4c9): **ulease fails**, "node B read head bb7787... 0 before the other holder let the lease go: the daemon uploaded without the lease". From the artifact (`rows.tsv`, `release-ns.txt`, `create.log`): that head is the one `create` made ("(made)" in create.log; the cut's row beside it reads "none"). The other holder let go at 461.13 s; node B reads the daemon's lease at epoch 2 from 461.44 s on; head first moves at 464.51 s, to the first cut. The status taken under the other holder's lease reads `upload_state no_lease`, `uploaded 0`. So the daemon committed nothing before it held the lease, and the judge added in 11fd271 ("none at head before the release") takes the create's commit for an upload. My reading of the artifact, for testing to confirm: the rig's, not the daemon's.
- Run 44273356510 (skyzfs-testing c80ed5b) was under way when I stopped.

So on this head no course run is green as a whole yet, and neither failure is a verdict against the daemon as far as I can read the artifacts. Whether that meets "the courses green on the head" is the lead's to say.

### CI and Codex

- CI on da0b4c9: 34 checks success, 5 skipped (the badges, the replay rig's reference-server job, and skyzfs-ci's three jobs that double ci.yml's on a pull request). Mergeable state "clean".
- Daemon rig, all three images (24.04, 26.04, 26.04-arm): 53 pins PASS, `pin_c_kill_with_applied_unacknowledged` NA, `pin_failed_volume_while_a_request_is_held` WAITING, as on e42ea6a.
- Replay rig through skyzfsd: both jobs green on the pull-request budget, "Every record prefix: 118 of 2110" and "123 of 2219": not a showing of design section 9's criterion, as the summary says itself.
- Control rig, cut-daemon rig (26.04 and 26.04-arm), node B's rig, ci, coverage, e2e: success.
- Codex: its three findings on e42ea6a and its P2 on 8fae8f7 are fixed and answered on their threads. Its P2 on a18bedf is open and right (2.1). It reviewed da0b4c9 and raised nothing new.

### What I could not verify

- `run` itself, the eventfd and the queue's thread under ublk: no kernel here. The W plants above are the measure of that.
- Anything against a real object store or the AWS SDK: what `connect` signs with (Y03), an empty `AWS_REGION` on the ambient path (2.4), and that a name lookup is the blocking work of 2.1.
- The courses' failures beyond what their artifacts hold.
- I did not run the first look's other 38 killed plants again on this head; the 94 here are the ones the changes since e42ea6a touch, with the first look's survivors among them.

### The plants

D: `cargo test -p skyzfsd --features skyzfsd/test-hooks`; P: `cargo test -p skyzfsd` (default build); U: `cargo test -p skyzfs-ublk --lib`; L: `cargo test -p skyzfs-local --lib inplace::state`; C: `cargo test -p skyzfs-upload`. A plant whose tests did not end within 400 s counts as killed and says so.

| plant | file | what was planted | tests | result |
|---|---|---|---|---|
| P2 | `uploader.rs` | a cut found committed does not end the refusals' wait | C | KILLED by a_cut_found_committed_ends_the_wait_of_the_refusals |
| P3 | `uploader.rs` | a cut committed and gone does not end the refusals' wait | C | KILLED by a_cut_committed_and_gone_ends_the_wait_of_the_refusals |
| P3b | `uploader.rs` | a cut uploaded does not end the refusals' wait | C | KILLED by a_commit_refused_again_and_again_waits_longer_each_time_and_not_at_every_cycle |
| P3c | `uploader.rs` | any step that goes on ends the refusals' wait | C | KILLED by a_commit_refused_again_and_again_waits_longer_each_time_and_not_at_every_cycle, a_cut_committed_and_gone_ends_the_wait_of_the_refusals, a_cut_found_committed_ends_the_wait_of_th... |
| W01 | `serve.rs` | the queue's thread never answers the desk | D | SURVIVED |
| E1 | `uploader.rs` | the errors' wait is never reset | C | KILLED by a_cut_committed_and_gone_before_its_release_ends_the_wait, a_cut_found_committed_ends_the_wait, a_put_that_fails_now_and_then_waits_a_second_each_time_and_the_cut_goes_up |
| E2 | `uploader.rs` | a give-up waits in the errors' backoff | C | KILLED by a_commit_refused_again_and_again_waits_longer_each_time_and_not_at_every_cycle, a_cut_committed_and_gone_ends_the_wait_of_the_refusals, a_cut_found_committed_ends_the_wait_of_th... |
| W02 | `serve.rs` | the queue's thread does not close the desk at its end | D | SURVIVED |
| E3 | `uploader.rs` | an error waits in the refusals' backoff | C | KILLED by a_put_that_fails_now_and_then_waits_a_second_each_time_and_the_cut_goes_up |
| W03 | `serve.rs` | the desk's wake does nothing | D | SURVIVED |
| E4 | `uploader.rs` | a conflict right after a refused commit is not waited after | C | KILLED by a_conflict_right_after_a_refused_commit_is_waited_after |
| E5 | `uploader.rs` | a conflict anywhere but at the commit is followed at once by another step | C | SURVIVED |
| W04 | `serve.rs` | the lease is let go at every exit that reaches the end | D | SURVIVED |
| E6 | `uploader.rs` | a refused commit waits as an error does, and again at its give-up | C | KILLED by a_commit_refused_again_and_again_waits_longer_each_time_and_not_at_every_cycle, a_conflict_right_after_a_refused_commit_is_waited_after, a_cut_committed_and_gone_ends_the_wait_o... |
| P4b | `uploader.rs` | a give-up announces its wait and returns none | C | KILLED by a_commit_refused_again_and_again_waits_longer_each_time_and_not_at_every_cycle, a_cut_found_committed_ends_the_wait_of_the_refusals |
| W05 | `serve.rs` | the lease is never let go | D | SURVIVED |
| P1 | `uploader.rs` | no step while the lease is unsure | C | KILLED by the_uploads_go_on_while_the_lease_is_unsure |
| P6 | `uploader.rs` | a stopped driver is stepped all the same | C | KILLED by a_cut_kept_at_its_release_stops_the_uploader_which_rests |
| W07 | `serve.rs` | the lease's time is counted in the start's `devices` time | D | SURVIVED |
| P15 | `uploader.rs` | the lease's backoff is never reset | C | SURVIVED |
| B1 | `uploader.rs` | no renewal is made beside a step | C | KILLED by a_step_longer_than_two_thirds_of_the_lease_does_not_delay_its_renewal, a_step_that_ends_during_a_renewal_waits_for_it_and_counts_its_wait_from_its_own_end, an_idle_check_longer_... |
| W08 | `serve.rs` | a start refused for the lease exits 8, not 10 | D | SURVIVED |
| B2 | `uploader.rs` | a step that ends during a renewal counts its wait from the renewal's end | C | KILLED by a_step_that_ends_during_a_renewal_waits_for_it_and_counts_its_wait_from_its_own_end, an_idle_step_that_ends_during_a_renewal_counts_its_wait_from_its_own_end |
| W12 | `serve.rs` | the hook command `release` is taken in a daemon that uploads | D | SURVIVED |
| B3 | `uploader.rs` | a renewal under way when the step ends is dropped half done | C | KILLED by a_step_that_ends_during_a_renewal_waits_for_it_and_counts_its_wait_from_its_own_end, an_idle_step_that_ends_during_a_renewal_counts_its_wait_from_its_own_end |
| B4 | `uploader.rs` | the idle wait is counted from the turn's end, not the step's | C | KILLED by an_idle_step_that_ends_during_a_renewal_counts_its_wait_from_its_own_end |
| W13 | `serve.rs` | the lease's default time is an hour | D | KILLED by the_uploads_come_from_the_file_remote_and_the_lease_options_need_one |
| B5 | `uploader.rs` | a backoff is counted from the turn's end, not the step's | C | KILLED by a_step_that_ends_during_a_renewal_waits_for_it_and_counts_its_wait_from_its_own_end |
| X1 | `uploader.rs` | the clock is not read again after the lease is kept | C | KILLED by a_slow_first_take_that_fails_is_tried_again_a_second_after_it_failed, a_slow_renewal_during_a_backoff_is_followed_by_what_is_left_of_the_wait |
| H01 | `serve.rs` | the default holder is the host's name alone | D | KILLED by the_default_holder_needs_a_machine_id, the_holder_is_the_option_or_the_hosts_name_and_machine_id |
| X3 | `uploader.rs` | a failed take's backoff is counted from before the take | C | KILLED by a_slow_first_take_that_fails_is_tried_again_a_second_after_it_failed |
| X4 | `uploader.rs` | the rest after a lost lease is counted from before the take | C | SURVIVED |
| H02 | `serve.rs` | a machine id of another length is taken | D | KILLED by the_default_holder_needs_a_machine_id |
| X5 | `uploader.rs` | the next renewal is counted from when the take came back | C | KILLED by a_slow_renewal_during_a_backoff_is_followed_by_what_is_left_of_the_wait |
| H03 | `serve.rs` | a machine id that is not hexadecimal is taken | D | KILLED by the_default_holder_needs_a_machine_id |
| R1 | `uploader.rs` | run does not sleep between turns | C | KILLED by run_publishes_each_turn_sleeps_between_them_and_gives_back_the_lease |
| R2 | `uploader.rs` | run does not stop during a turn | C | KILLED by run_stops_during_a_turn |
| H04 | `serve.rs` | a machine id of all zeros is taken | D | KILLED by the_default_holder_needs_a_machine_id |
| R3 | `uploader.rs` | run does not publish | C | KILLED by run_publishes_each_turn_sleeps_between_them_and_gives_back_the_lease |
| R4 | `uploader.rs` | run does not stop during the sleep between turns | C | SURVIVED |
| H05 | `serve.rs` | the machine id is read from another file | D | KILLED by the_holder_is_the_option_or_the_hosts_name_and_machine_id |
| T02 | `serve.rs` | the status's cuts replaced are the cuts abandoned | D | KILLED by the_held_cuts_reach_the_status_each_in_its_field |
| T03 | `status.rs` | the status file does not carry the cuts replaced | U | KILLED by before_the_first_cut_the_file_has_no_group, status_prints_the_held_cuts, the_file_has_the_documented_layout and 1 more |
| T04 | `status.rs` | a status file of the fourteenth version is read as having cuts replaced | U | KILLED by a_file_of_the_fourteenth_version_is_read_without_the_uploader, a_file_of_the_thirteenth_version_is_read_without_the_intent_map, status_prints_the_held_cuts and 1 more |
| T05 | `status.rs` | the report does not print the cuts replaced | U+D | KILLED by status_prints_the_held_cuts, the_report_has_the_keys_the_rig_reads_and_says_whose_the_counters_are |
| S01 | `state.rs` | a cut still flushing at the next cut's instant is not counted replaced | L | KILLED by the_cuts_replaced_before_their_upload_are_counted |
| S02 | `state.rs` | a waiting cut replaced at a freeze is not counted | L | SURVIVED |
| S03 | `state.rs` | a waiting cut replaced when a newer cut's flush completes is not counted | L | KILLED by the_cuts_replaced_before_their_upload_are_counted |
| A00 | `upload.rs` | create takes a clone | D | KILLED by a_create_refuses_a_clone_under_the_name |
| A01 | `upload.rs` | create takes a head commit of another volume | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A02 | `upload.rs` | create takes a head commit that has a parent | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A03 | `upload.rs` | create takes a head commit that has segments | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A04 | `upload.rs` | create takes a head commit that names a group | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A05 | `upload.rs` | create takes a head commit that counts flushes | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A06 | `upload.rs` | create takes a head commit written under a lease | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A07 | `upload.rs` | create takes a head commit of another kind | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A08 | `upload.rs` | create takes a head commit whose index is not the empty one | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| Q8 | `queued.rs` | the desk answers the sync of the copies as done without asking the store | C | KILLED: the head's tests have no end within 400 s; still running: a_sync_of_the_copies_through_the_desk_leaves_the_queue_turning |
| A09 | `upload.rs` | the empty index has no shard for a last part shard | D | KILLED by a_create_makes_the_volume_with_the_empty_commit_and_takes_it_again, a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size, two_creates_of_the_name_at_once_leave_t... |
| Q11 | `queued.rs` | the desk waits for the sync of the copies on the queue's thread | C | KILLED by a_sync_of_the_copies_through_the_desk_leaves_the_queue_turning, a_sync_of_the_copies_under_way_at_the_close_ends_as_failed |
| V01 | `upload.rs` | the create's superblock is written whether one is there or not | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size, two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| Q12 | `queued.rs` | the reply to a sync under way is dropped | C | KILLED by a_sync_of_the_copies_through_the_desk_leaves_the_queue_turning |
| Q13 | `queued.rs` | the reply to a sync still under way at the next turn is dropped | C | KILLED by a_sync_of_the_copies_through_the_desk_leaves_the_queue_turning |
| V02 | `upload.rs` | a create goes on over a superblock of another size or shards | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size, two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| Q14 | `queued.rs` | the close keeps the reply to a sync under way | C | KILLED by a_sync_of_the_copies_under_way_at_the_close_ends_as_failed |
| V03 | `upload.rs` | a create goes on over a superblock of other shards | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size |
| Q15 | `queued.rs` | a reply is parked on a closed desk | C | KILLED by a_sync_whose_desk_closes_as_it_begins_ends_as_at_a_close |
| Q16 | `queued.rs` | a sync that ended at a later turn is answered as done whatever its outcome | C | SURVIVED |
| V04 | `upload.rs` | a create goes on over a superblock of another size | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size, two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| Q7b | `queued.rs` | the queue's thread is woken before the call is left as well, on an open desk | C | KILLED by a_closed_desk_answers_as_a_store_that_holds_nothing |
| V05 | `upload.rs` | a superblock that cannot be written does not refuse the create | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size |
| V06 | `upload.rs` | the refusal over a superblock with no head does not name the object | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size |
| V08 | `upload.rs` | the refusal says a create of that size goes on, over a superblock of other shards | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size |
| Y01 | `upload.rs` | keys that come with a session token are signed with as fixed keys | D | KILLED by fixed_keys_sign_only_with_no_session_token |
| Y02 | `upload.rs` | fixed keys sign in us-east-1 whatever AWS_REGION says | D | KILLED by fixed_keys_sign_only_with_no_session_token |
| Y03 | `upload.rs` | connect names us-east-1 to the client of fixed keys, whatever the keys' region | D | SURVIVED |
| Y04 | `upload.rs` | an empty AWS_REGION is the region | D | KILLED by the_region_is_aws_region_or_the_default |
| N01 | `upload.rs` | the thread's end waits for the runtime's blocking work | D | KILLED by the_end_does_not_wait_for_work_left_on_the_runtime |
| N02 | `upload.rs` | the thread posts after the desk is closed | D | KILLED by nothing_is_posted_once_the_desk_is_closed |
| N03 | `upload.rs` | the status names no holder while the lease is held | D | KILLED by the_end_at_exit_0_lets_the_lease_go, the_start_takes_a_lease_nobody_holds, the_status_shows_the_uploader_as_it_is |
| N04 | `upload.rs` | the status names no holder while the lease is unsure | D | KILLED by the_status_shows_the_uploader_as_it_is |
| N05 | `upload.rs` | the start's status names no holder | D | KILLED by the_start_takes_a_lease_nobody_holds |
| U12 | `upload.rs` | an end with no lease held says it was let go | D | KILLED by an_end_with_no_lease_held_says_it_was_kept |
| U20p | `upload.rs` | the uploader's commits are fenced by another epoch than the one held, default build | P | KILLED by the_daemons_remote_commits_under_the_epoch_it_is_made_for |
| U21 | `upload.rs` | the uploader's commits carry epoch 0, default build | P | KILLED by the_daemons_remote_commits_under_the_epoch_it_is_made_for |
| U26 | `upload.rs` | the uploader's commits are made as another holder, default build | P | KILLED by the_daemons_remote_commits_under_the_epoch_it_is_made_for |
| Q7c | `queued.rs` | a call does not wake the queue's thread | C | KILLED: the head's tests have no end within 400 s; still running: a_closed_desk_answers_as_a_store_that_holds_nothing, a_sync_whose_desk_closes_as_it_begins_ends_as_at_a_close |
| K3 | `lease.rs` | a renewal that finds no row takes the lease again | C | KILLED by a_renewal_that_finds_the_row_gone_loses_the_lease_and_takes_nothing |
| K5 | `lease.rs` | a renewal's take that finds another holder leaves the lease unsure | C | KILLED by a_lease_another_holder_took_between_the_read_and_the_take_is_lost_to_it |
| K8 | `lease.rs` | a commit the fence refused does not lose the lease | C | KILLED by a_fenced_commit_loses_a_held_lease_at_once |
| K9 | `lease.rs` | an unsure lease is not released | C | KILLED by an_unsure_lease_is_let_go_at_a_release |
| K13 | `lease.rs` | a renewal whose take fails loses the lease | C | KILLED by a_renewal_whose_take_fails_after_its_read_is_unsure |
| V07 | `upload.rs` | the refusal tells to delete the superblock though the volume's head is made | D | KILLED by two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| R4d | `uploader.rs` | run does not stop during the sleep between turns, against the daemon's tests | D | KILLED by an_end_with_no_lease_held_says_it_was_kept |
