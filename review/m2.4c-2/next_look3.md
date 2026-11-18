## Third look at #118, at 4c733d3 (M2.4c-2)

**Verdict: it can merge, from the review's side. No finding blocks, and the two defects of the last look (2.1, 2.2) are fixed, as are 2.4 and 2.5; 2.3 is left open with a reason I accept. CI is green on this head. What remains is not the review's: G1 is the lead's (the wiring in `run` rests on testing's courses: on this head six of seven pass, and urow ended with the rig's own error and no verdict).**

Read first the code at 4c733d3, then the answer (issuecomment-6975024406) and the description. They agree with the code in every point I checked, the two plants reported as missed included.

### What changed since da0b4c9

`git diff da0b4c9..4c733d3 --stat`: four files, 311 lines added and 81 removed, in two commits (5b8b800, 4c733d3).

- `bins/skyzfsd/src/upload.rs`: the `Runtime` type, `within`, the conflict arm with `unserved` and `found`, `ambient_region`.
- `bins/skyzfsd/src/upload/tests.rs`: the tests for them.
- `bins/skyzfsd/src/kill/tests.rs`: five lines, the test helper takes `block_on`'s new result.
- `crates/skyzfs-upload/src/queued.rs`: six lines of the module's doc (2.5), no code.

Nothing else: `serve.rs`, the crates' code, the workflows and the documents are as they were at da0b4c9.

### What I did

- The head's tests in both builds (`t3_hooks.out`, `t3_plain.out`): skyzfsd 137 + 3 with `--features test-hooks` and 101 + 3 plain (three tests more than at da0b4c9 in each), skyzfs-upload 26, 3, 22, 7, 24. All pass. I did not run skyzfs-ublk or skyzfs-local again; their code did not change.
- 45 plants, one at a time (`plants3.py`, `plants3.out`; the table is at the end): 16 new ones in what changed, and the 29 of the last look in `upload.rs` run again on this head, since `place` was taken apart. 41 KILLED, 4 SURVIVED, none NOT APPLIED.
- 20 scenarios in both builds (`scenarios3.rs`, `scenarios3.out`): the 19 of the last look, the two that failed there rewritten for the new type, and one more. All 20 pass in both builds.
- CI and Codex on 4c733d3, read at 10:35 UTC. Testing's course run 44280164949, whose daemon is 4c733d3.

### 1. The runtime (2.1, Codex's P2 on a18bedf): fixed

`upload::Runtime` (`bins/skyzfsd/src/upload.rs:160`) holds the tokio runtime, and its drop is `shutdown_background` (upload.rs:176). The fix is at the type and not at each return, which is the better shape: no path that drops one can wait.

**Is any runtime of the daemon's object storage still dropped the old way? No, as far as the code goes.** I looked for every `tokio::runtime`, `Builder`, `Runtime::new`, `block_on` and `tokio::main` in `bins/skyzfsd/src` and `crates/skyzfs-upload/src`, and in the crates under them (skynbd-engine, skynbd-store, skynbd-refs):

- One tokio runtime is built in the daemon's code, in `runtime()` (upload.rs:148), and it is returned inside the new type. The field is private and nothing gives it out.
- It has three holders, and they are all there are: `within` for `create` (upload.rs:278, called by `place_within` at 270); `Uploads::new` (upload.rs:733), which hands it to `Start`; and the uploader's thread, which gets it from `Start::into_parts` (upload.rs:772).
- `crates/skyzfs-upload/src` builds no runtime. In the three skynbd crates the only one is in a test module (`crates/skynbd-engine/src/index.rs`, inside `mod tests`).
- Nothing in the upload path leaves work on the blocking pool that the old drop was right to wait for: no `spawn_blocking` and no `tokio::fs` in the daemon, the upload crate, the engine, the store or the refs outside tests.

**The tests hold what they say.** T1 (the drop waits again) dies by three tests: `a_runtime_is_dropped_without_waiting_for_its_blocking_work`, `work_in_object_storage_ends_at_its_limit_with_blocking_work_left` and F2's `the_end_does_not_wait_for_work_left_on_the_runtime`. T3 (the limit a hundred times longer) and T4 (`within` on a plain tokio runtime) die by the second.

**My scenarios of the last look, run again** (3 s of blocking work each):

| scenario | at da0b4c9 | at 4c733d3 |
|---|---|---|
| a place given 200 ms, a store that never answers | came back after 3.0 s | after 201 ms |
| the drop of a `Start` that took the lease | 3.0 s | 8 microseconds |
| the drop of the runtime `into_parts` hands on (new) | not run | 9 microseconds |
| D7: the thread's end at exit 0, given 6 s | 151 microseconds | 173 microseconds |

`within` is private, so the first is its body on the daemon's own `runtime()` with my limit; the author's test calls `within` itself.

**T2, reported as missed because equivalent: I agree.** With the thread's `drop(runtime)` moved after `said.send(end)` (upload.rs:1023) every test passes, and nothing can tell the two orders apart: the drop no longer waits wherever it comes, so the join in `finish` does not either. The comment there ("Dropped before the answer ... so neither does the join") describes an order that has stopped mattering; harmless.

**One line unpinned, small.** T5: with `place_within` giving `within` a hundred times `PLACE_LIMIT` (upload.rs:270), every test passes. `place_within` makes a real client, so no test reaches it; the test pins `within`, and the distance to what `create` calls is that one argument. The same kind of gap as Y03 and U5 below. Not blocking.

A thing to know, not a finding: `create`'s limit now counts the making of the client. A serve's start still makes its client with no limit (`runtime.block_on(connect(remote))`, upload.rs:734), as before. By reading, `connect` asks nothing of the network (the region is given or comes from the environment, and the keys are looked for at the first request), and its doc says so; I have not run the SDK to see it.

### 2. The create that loses the head's swap (2.2): fixed

`Engine::create_volume` ends with `EngineError::Conflict` only where its swap of head from none finds a head (`crates/skynbd-engine/src/lib.rs:573`; the other source of that error, a fenced refs' refusal, cannot arise here because `create` writes through plain refs). The new arm (upload.rs:344) then does what a create does that finds a volume: the lease row first (`unserved`, upload.rs:363), the head read again, and the checks of a volume found (`found`, upload.rs:385). The split is a move of code: the first path calls the same two functions.

- U1 (the arm gone, the engine's error again), U2 (no lease check after the lost swap), U3 (no check of the volume found after it) and U3b (the loser says "made") each die, by `two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made` and `a_create_that_loses_the_heads_swap_refuses_a_volume_served_or_written_since`.
- The first path after the split: U6 (no lease check before the head is read), U7 (a lease row refuses nothing) and U8 (no check of a volume found) each die. A00 to A09 and V01 to V08 die again on this head, by the same tests as before.
- The race with nothing held (scenario `n2_...`, two creates on two threads, 300 rounds per build): in all 100 rounds of one size, in both builds, one create says "made" and the other "found empty"; the scenario now fails on anything else. At da0b4c9 the loser failed with "head CAS conflict: lease lost (I2)" in 19 and 10 of those rounds. In the 200 rounds of two sizes at most one says "made", and the superblock is of its size, as before.

Two remarks by reading, neither a finding. Where the head is gone again at the second read the arm ends with the engine's error; that needs a deletion between the two reads. And the loser has by then written an index node and a commit object that nothing names, and the superblock once more (the same but for its time), as it did at da0b4c9.

### 3. The ambient region (2.4): fixed, with `connect`'s line unpinned as said

`ambient_region` (upload.rs:249) gives us-east-1 where `AWS_REGION` is unset or empty and no region where it names one, and `S3Store::from_env` sets the client's region only when it is given one (`crates/skynbd-store/src/s3.rs:63`), so an empty `AWS_REGION` now means the same on both paths. U4 (an empty one left to the SDK) and U4b (us-east-1 whatever the variable says) die by `the_region_is_aws_region_or_the_default`; Y04b (an empty one taken as the region of fixed keys) dies by it too.

**U5, reported as missed because no test reaches `connect`: confirmed.** With `connect` giving the ambient client no region (upload.rs:207) every test passes, and so does Y03 again (the constant instead of `&keys.region`, upload.rs:201). `connect` is the one function between the two pinned readings of the environment and the client, and nothing calls it in a test. What it would take is a test that makes the client and reads the region back from it, or one against an S3 server; the cut-daemon rig runs `create` and `serve` against an S3 server with fixed keys and no `AWS_REGION`, so it goes through line 201 with the default region and never through line 207.

### The rest of the answer

- **2.3 (the no-head path does not look for a clone's base): left open.** I said I did not ask for a change, and the reason given stands: only the engine's own clone failing half way leaves that state, and the next create refuses the volume as a clone (scenario `n3_...`, unchanged). It is in the description's open items.
- **2.5:** the doc now says the queue's last turn and the main thread's close, and that a queue thread that panics has no last turn. That is what D5 shows.
- **The survivors of the last look** (E5, X4, P15, Q16, S02; W01 to W05, W08, W12): unchanged, and the code they are in did not change.

### Findings

Defects: none.

Gaps, small, none blocking: T5, U5 and Y03 above, three single arguments on the way to a real client (`place_within`'s limit, and the region `connect` gives each kind of client) that no test reaches.

### The courses (G1, the lead's)

Run 44280164949 (skyzfs-testing cd849a6, daemon built from 4c733d3), all seven courses:

- life, umoved, ukill, ulease, ulost and ufailed pass: `upload_reads`, `upload_content`, `upload_status`, `upload_balance` in each, and `upload_course` where the course has one. ulease passes with the judge corrected in cd849a6 ("head unchanged before the release, not none"), which is the reading of the last look.
- urow has no verdict: "the rig's own error in urow: the watcher had not fetched cut 11 (group 26) under head 340b7b21... 120 s after row2's row named it", worst exit 2. From the artifact (`watch.log`, `rows.tsv`, `uploaded.tsv`): the watcher's read of that head was overtaken ("head moved under the read", to the sixth commit, cut 13 of group 30), and it fetched cut 13 instead; the head stays there to the end. The daemon went on uploading under its lease while the course waited for a head that had passed. I read that as the course's timing again and not the daemon's, as the rig itself classes it; for testing to confirm.

So on this head six of the seven courses are green and the seventh did not judge. Whether that is enough is the lead's to say.

### CI and Codex

- CI on 4c733d3 at 10:35 UTC: 34 checks success, 5 skipped (the badges, the replay rig's reference-server job, skyzfs-ci's three doubles), none failed.
- Daemon rig on 24.04, 26.04 and 26.04-arm: 53 pins PASS on each, `pin_c_kill_with_applied_unacknowledged` NA, `pin_failed_volume_while_a_request_is_held` WAITING, as on da0b4c9 and e42ea6a.
- The replay rig through skyzfsd ran on the pull-request budget, so it is green and not a showing of design section 9's criterion, as before.
- Codex reviewed 4c733d3 and has no finding and no unresolved finding on it. Its P2 on a18bedf is answered on its thread (fixed in 5b8b800).

### What I could not verify

- `run`, the eventfd and the queue's thread under ublk: no kernel here.
- Anything against a real object store or the AWS SDK: that `connect` asks nothing of the network, what the client does with the region it is given, and that a name lookup that hangs is the blocking work the runtime's drop no longer waits for. The mechanism is shown with a blocking task in its place.
- How many of the same-size rounds of `n2_...` went through the conflict arm and how many read the head already made: the scenario sees only the outcome. The author's test holds a create at its superblock and takes the arm for certain.
- The cause of urow's error beyond its artifact.

### The plants

All against `cargo test -p skyzfsd --features skyzfsd/test-hooks` (D), except the three marked P, against `cargo test -p skyzfsd` (the default build). "new" are the plants in what changed since da0b4c9; "again" are the last look's plants in `upload.rs`, run on this head.

| plant | group | what was planted | tests | result |
|---|---|---|---|---|
| T1 | new | the runtime's drop waits for its blocking work | D | KILLED by a_runtime_is_dropped_without_waiting_for_its_blocking_work, the_end_does_not_wait_for_work_left_on_the_runtime, work_in_object_storage_ends_at_its_limit_with_blocking_work_left |
| T2 | new | the thread drops its runtime after its answer, not before | D | SURVIVED |
| T3 | new | the limit of the work in object storage is a hundred times longer | D | KILLED by work_in_object_storage_ends_at_its_limit_with_blocking_work_left |
| T4 | new | the work in object storage runs on a plain runtime | D | KILLED by work_in_object_storage_ends_at_its_limit_with_blocking_work_left |
| T5 | new | create's limit is a hundred times PLACE_LIMIT | D | SURVIVED |
| U1 | new | a create that loses the head's swap ends with the engine's error | D | KILLED by a_create_that_loses_the_heads_swap_refuses_a_volume_served_or_written_since, two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| U2 | new | no lease check after the lost swap | D | KILLED by a_create_that_loses_the_heads_swap_refuses_a_volume_served_or_written_since |
| U3 | new | no check of the volume found after the lost swap | D | KILLED by a_create_that_loses_the_heads_swap_refuses_a_volume_served_or_written_since |
| U3b | new | a create that loses the head's swap says it made the volume | D | KILLED by a_create_that_loses_the_heads_swap_refuses_a_volume_served_or_written_since, two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| U6 | new | no lease check before the head is read (the first, as it was) | D | KILLED by a_create_refuses_a_name_that_is_another_volumes |
| U7 | new | a lease row refuses nothing | D | KILLED by a_create_refuses_a_name_that_is_another_volumes, a_create_that_loses_the_heads_swap_refuses_a_volume_served_or_written_since |
| U8 | new | no check of a volume found under the name (the first, as it was) | D | KILLED by a_create_refuses_a_clone_under_the_name, a_create_refuses_a_head_that_only_looks_empty, a_create_refuses_a_name_that_is_another_volumes |
| U4 | new | an empty AWS_REGION is left to the SDK on the ambient path | D | KILLED by the_region_is_aws_region_or_the_default |
| U4b | new | the ambient client is given us-east-1 whatever AWS_REGION says | D | KILLED by the_region_is_aws_region_or_the_default |
| U5 | new | connect gives the ambient client no region | D | SURVIVED |
| Y04b | new | an empty AWS_REGION is the region of fixed keys | D | KILLED by the_region_is_aws_region_or_the_default |
| A00 | again | create takes a clone | D | KILLED by a_create_refuses_a_clone_under_the_name |
| A01 | again | create takes a head commit of another volume | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A02 | again | create takes a head commit that has a parent | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A03 | again | create takes a head commit that has segments | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A04 | again | create takes a head commit that names a group | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A05 | again | create takes a head commit that counts flushes | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A06 | again | create takes a head commit written under a lease | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A07 | again | create takes a head commit of another kind | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A08 | again | create takes a head commit whose index is not the empty one | D | KILLED by a_create_refuses_a_head_that_only_looks_empty |
| A09 | again | the empty index has no shard for a last part shard | D | KILLED by a_create_makes_the_volume_with_the_empty_commit_and_takes_it_again, a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size, two_creates_of_the_name_at_once_leave_the_superbl... |
| V01 | again | the create's superblock is written whether one is there or not | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size, two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| V02 | again | a create goes on over a superblock of another size or shards | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size, two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| V03 | again | a create goes on over a superblock of other shards | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size |
| V04 | again | a create goes on over a superblock of another size | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size, two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| V05 | again | a superblock that cannot be written does not refuse the create | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size |
| V06 | again | the refusal over a superblock with no head does not name the object | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size |
| V07 | again | the refusal tells to delete the superblock though the volume's head is made | D | KILLED by two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made |
| V08 | again | the refusal says a create of that size goes on, over a superblock of other shards | D | KILLED by a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size |
| Y01 | again | keys that come with a session token are signed with as fixed keys | D | KILLED by fixed_keys_sign_only_with_no_session_token |
| Y02 | again | fixed keys sign in us-east-1 whatever AWS_REGION says | D | KILLED by fixed_keys_sign_only_with_no_session_token |
| Y03 | again | connect names us-east-1 to the client of fixed keys, whatever the keys' region | D | SURVIVED |
| N02 | again | the thread posts after the desk is closed | D | KILLED by nothing_is_posted_once_the_desk_is_closed |
| N03 | again | the status names no holder while the lease is held | D | KILLED by the_end_at_exit_0_lets_the_lease_go, the_start_takes_a_lease_nobody_holds, the_status_shows_the_uploader_as_it_is |
| N04 | again | the status names no holder while the lease is unsure | D | KILLED by the_status_shows_the_uploader_as_it_is |
| N05 | again | the start's status names no holder | D | KILLED by the_start_takes_a_lease_nobody_holds |
| U12 | again | an end with no lease held says it was let go | D | KILLED by an_end_with_no_lease_held_says_it_was_kept |
| U20p | again | the uploader's commits are fenced by another epoch than the one held, default build | P | KILLED by the_daemons_remote_commits_under_the_epoch_it_is_made_for |
| U21 | again | the uploader's commits carry epoch 0, default build | P | KILLED by the_daemons_remote_commits_under_the_epoch_it_is_made_for |
| U26 | again | the uploader's commits are made as another holder, default build | P | KILLED by the_daemons_remote_commits_under_the_epoch_it_is_made_for |
