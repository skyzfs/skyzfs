## Review of #118 at e42ea6a (M2.4c-2)

**Verdict: one finding blocks the merge on my reading, and it is a small one. `create` takes a clone that holds data as "found empty", and the first cut of the volume bound to it replaces the clone's data at head (F1).** No tool in the repository makes such a clone today, so if the lead would rather take the fix with M3, nothing else here blocks. With it fixed, or so decided, it can merge. I found no deadlock and no lost wake in the queue loop, no release of the lease under an upload, and no commit that lands once another holder has the lease. But I could only show that in a harness of my own: no test of the head and no rig on this head reaches the desk in the queue loop, the thread's start and end in `run`, or `--kill-at-upload` in a daemon (G1). With the line that answers the desk removed, every test of the head passes, and no rig that ran on e42ea6a would reach it. That is the lead's call and I do not count it as blocking, because the description says it plainly and M2.4d is the rig that is to show it.

**The base.** e42ea6a's parent is 1415db1, #117's head, and the merge base with `zfs-topology` is 4e4e076. I reviewed the one commit, `git diff 1415db1..e42ea6a` (21 files, 3886 lines added), and read #117's crate only to follow the calls. I formed my view from the code, the design and the notes before the description.

What I did:

- **The head's own tests** pass here, rustc 1.100.0: `skyzfsd` 86 and 3 plain, 122 with the test hooks; `skyzfs-upload` 21, 3, 22, 2 and 9; `skyzfs-ublk` 152, 1 and 1 (`t_plain.out`, `t_hooks.out`).
- **56 plants** of my own, one at a time, against the head's tests (`plants.py`, `plants.out`): 38 killed, 18 pass every test. None was "not applied". The table is below.
- **14 scenarios** the head's tests do not have (`scenarios.rs`, run by `scenarios.py` in both builds; `scenarios.out`). Eight of them run the daemon's loop without a kernel: `serve`'s own `pumped` over the in-place store on the simulated disk, a queue that waits as the kernel's does (a request, a wake, a stop, or 20 s of nothing), the two desk lines of the each-turn closure, and the daemon's `Start`, `maker` and `Uploading` over the real engine and refs store in memory. What is not the daemon's in it: the closure is a copy of `serve.rs:1120` to `1126`, since `run` cannot be called without ublk. Eleven scenarios pass on the head; three are named `defect_` and fail on it (F1, F2, F3).
- **CI and Codex**, read only, at the end of this review.

### The five questions

**(1) The start. The order is right, and a refusal leaves what the description says. Two things it leaves are worth knowing: the store's own marker goes on the acknowledging path, and a lease taken by a start that then fails is kept.**

The order as built: the volume's lock, the failure note judged (`noted_failure`), the store's open, the take of the lease (at most 10 s, under the lock), then the device. Where the take refuses, the open store is dropped, the lock goes back through `share_lock`, the lifecycle returns `Stop::Refused` before it would remove the failure note, and `serve` exits 10. The head's tests hold each step: taking the lease before the open, leaving the store open at a refusal, turning the refusal into an error and removing the note at a refusal are all killed (U01, U02, L01, L02).

- **The start token.** The open writes it, so a start refused for the lease costs the held cuts, and the next cut holds the whole volume. That follows from the lead's two decisions (B1 in the notes, and "a refused start writes only the token and the status"). Taking the lease before the open would avoid the cost, and U01 shows the head pins the order the lead chose. N1 is about how often that cost can come.
- **The failure note** in the run directory is kept (L02 killed). **The store's marker `volume.failed` is not**, on one path: a failure known only by the marker, a start with `--acknowledge-failure`, the lease another holder's. The acknowledging open removes the marker before the take, so the next start serves without the flag (S3). I think that is acceptable: the operator did acknowledge, and the new token makes the next cut whole. The author lists it as open. The comment at `lifecycle.rs:774` says the opposite (F6).
- **Exit 10** collides with nothing: no other use in the Rust, the shell, the Python or the workflows, and the head's test of the numbers kills a change of it (C02). Exit 8 for a held lock comes before the open, so no lease is taken on that path.
- **The lease is taken and the start fails afterwards** (the device cannot be added, the store's start is refused, exit 9, the uploader's thread cannot start): the lease is not given back. It is kept for its time, 30 s unless `--lease-ttl-s`. S5 shows it: another holder is refused until the time has run out, and the same holder's next start has it at once at the same epoch. That matches decision 4 ("kept at every other exit") and the author lists it. It costs another node one lease time after a failed start here, and nothing else.
- **A take with no answer in 10 s** serves the volume with "no lease yet". If the take had landed unheard, the uploader's first take holds the lease at the same epoch: the holder does not fence itself (S4).
- The take's time is on a line of its own and is subtracted from the start line's `devices`. Nothing pins the subtraction (W07 passes); it is a figure in a line, so I leave it at that.

**(2) The thread's end. Releasing only at exit 0 is right, the release cannot race the uploader, and I found no order of joins that lets a lease go under an upload.**

The order in `run` (`serve.rs:1466` to `1487`): stop the queue, join its thread (whose last turn closes the desk), finish the status, judge a failure that came with the end, compute the exit, and only then `thread.end(release)`. Every return of `run` after the uploader's thread started comes after that join. The release itself is done by the uploader's own thread, after its loop has returned and the turn under way has been dropped (`upload.rs:694` to `709`). So there is no second thread that could release while the first uploads.

| End | The lease | Shown by |
|---|---|---|
| SIGTERM, the device deleted, exit 0 | Let go: the row keeps holder and epoch, expiry 0, and another holder takes it at once at the next epoch | D1; the head's `the_end_at_exit_0_lets_the_lease_go` |
| A stop while a commit is under way | The commit is dropped, head stays whole, the lease is let go within the limit; the next instance uploads the cut | D3 |
| A stop between a commit's swap and the cut's release | The swap has landed and the cut is still held; the lease is let go; the next instance finds the cut committed, lets it go and uploads nothing twice | D4 |
| The volume fails (exit 3), exit 9, the queue's thread panics, any error of `run` | Kept: the handle is dropped, which closes the desk and stops the thread within 1 s | D5 for the panic; the head's `any_other_end_keeps_the_lease` |
| `kill -9` | Runs out in its time | by reading |
| The lease is another holder's by then | Nothing is let go of the other holder's lease, and the daemon's commit is refused before it lands | D6 |

Why not releasing at the other exits is right, in my words: after every non-zero exit either this node's daemon is expected back, and under its own name it has the lease again at once (S5), or the volume has failed and has nothing more to upload, and a kept lease costs another node 30 s. A release on a failure path would add up to 5 s of object storage to it.

Against real S3 a request that was already sent can still land after its future is dropped. The refs object's conditional put orders it against the release, and the fence is the holder and the epoch, both still this daemon's until another holder takes the lease; after that D6 applies. I could not run that against a real store.

Two things I would tell the lead: exit 0 lets the lease go while cuts may be held and not uploaded (N5), and the end's limit is not a limit in one case (F2).

**(3) The desk in the queue loop. No deadlock, no lost wake and no wrong answer that I could find, by reading and in the harness. The store's calls do run on the queue's thread, as designed. No rig covers any of it at this head.**

- **Deadlock.** The desk's lock is never held across a store call or a wake (`queued.rs:126` to `136`, `158` to `170`). The queue's thread never waits for the uploader: it answers what is there and goes on. The uploader waits on a channel that `close` drops.
- **Lost wake.** The call is put in the slot before the wake is sent. The queue drains the eventfd and arms it again inside `wait` (`queue.rs:804` to `808`), and `desk.answer` runs after that in the same turn (`serve.rs:1124`). A call that arrives before the drain is answered by this turn; one that arrives between the drain and the arming, or after, fires the poll. D1 shows the loop: a cut goes up 260 ms after its first write on a queue to which nothing else comes for 20 s (the 260 ms is the uploader's own poll of 250 ms). D2 is the control: with a desk whose wake does nothing, nothing goes up in 4 s. The eventfd itself I know only by reading; the harness's queue is a channel.
- **Blocking on the queue's thread.** Every call runs there between two turns: a read of up to 64 blocks, the sync of the copies file (an fsync), the release (the intent map's header). #117's review named the sync as its F1; with this pull request it is live. D1 counts nine calls for one small cut, and the second cut there goes through `Syncing`. I measured nothing: N4.
- **A call answered after the close.** It cannot happen at any exit. The main thread closes the desk only in `Uploading`'s end or drop, and both come after the queue's thread is joined. The queue's thread closes it itself at its last turn, on the thread that answers.
- **A panic of the queue's thread.** `pumped` catches it and does not call the closure with `ended`, so the queue's thread leaves the desk open. The drop of the handle closes it and ends the uploader in under a millisecond (D5).
- **A test harness.** The harness is the unit test the question asks for. It needs nothing that is not public already (`pumped`, `Pump`, `Desk`, `Start`, `maker`, `Uploading`); what it cannot reach is the closure in `run`. If the two desk lines were a function that `run` and a test both call, D1 and D2 would pin them.
- **The rigs.** The daemon rig, the replay rig through `skyzfsd`, the control rig and the cut-daemon rig all ran green on this head. All of them serve volumes that were created without `--endpoint`, so `uploads_of` returns nothing, `desk` is `None`, and the two lines are never reached. Nothing under `tests/` or `.github/` names `--endpoint`, `--holder`, `--serve-without-upload`, `--lease-ttl-s` or `--kill-at-upload`. So no rig and no pin covers this at e42ea6a, and W01, W02 and W03 (no answer, no close, no wake) pass every test. The description's row 8 says "the rigs (below)"; below it says the rigs drive volumes with no place. The rig meant is M2.4d's.

**(4) `create`'s order. Place, then the local volume, then the file `remote`. Each crash leaves something the next create handles, with one exception that the author names and one hole in the guard that they do not (F1).**

| Where it stops | What is left | The next step |
|---|---|---|
| Inside `place`, before head is swapped | A superblock and objects that no ref names | The next create writes them again and makes the volume |
| After `place`, before the local volume | An empty volume in object storage | The next create of the name and size finds it and takes it. A create of the name with another size is refused as "another volume's": the operator removes the objects or takes another name |
| The local volume refused after `place` (a directory that holds a volume, a file system that is refused) | The same empty volume in object storage | As above |
| After the local volume is in place, before `remote` is renamed (a kill or a power cut in a window of milliseconds) | A local volume with no place | `create` again is refused (the directory holds a volume). `serve` serves it with no upload and says nothing, because a volume without a place is a legitimate one. It cannot be given a place later: the author's open question, N3 |
| `remote` cannot be written (an error, not a death) | The volume, and a message that says to remove it and create again | Right, but the message has two runs of 21 spaces in it (F4) |

A half-made volume is not misjudged by `serve`: `remote` is written whole under another name and renamed, and a file that is not exactly the five lines is refused (R01, R02 killed). What `place` misjudges is a volume that was never half-made: a clone (F1). And "found empty" is taken twice: two creates of one name before either volume is served both succeed (S6), which is N3 too.

**(5) `--kill-at-upload`. It cannot fire in a default build, and it kills where it says. No rig uses it yet, and nothing shows it reaches the uploader in a daemon.**

- `kill.rs` is compiled only with the feature `test-hooks` on Linux (`lib.rs:37`). The option is a hook option: a default build refuses it as a usage error. I ran the default binary: `serve --kill-at-upload put:1` ends with exit 2, "only in a build with the feature test-hooks". The head's test kills a default build that takes it (C01). `Uploads::begin` drops the value in a default build.
- `put:N` counts a PUT of a segment once it has landed and kills before the step goes on; `swap:N` kills once the commit's transaction has landed and before the cut is let go. A failed PUT or commit is not counted. The head's tests kill each way I tried to move the point (K01 to K04). The count runs over the daemon's life, not per cut, and a cut that goes up again after a backoff is counted again; the rig's author should know that.
- What nothing shows: that the build with the hooks hands the kill point to the uploader. With `begin` using the plain `maker`, every test passes (U25). No rig names the option at this head. It is M2.4d's to show.

**The wiring against #117's lease.** #117's review left two things to this pull request. The remote the uploader makes must be fenced by the epoch it is given: it is (`upload.rs:496` to `510`), and D6 shows it through the daemon's own `maker`: another holder takes the lease, the daemon reads and PUTs its next cut, the commit is refused, nothing lands, and the status says who has the lease. And the commit carries the lease's epoch (D1 checks it). Neither is pinned by the default build's tests (G2). The other thing was that a holder's name must mean one daemon: N1. What `Unsure` means for a running daemon is as in #117: uploads go on under the epoch last held, and what stops them is the fence at the next commit. A start never sees `Unsure`; a first take that fails is `Unreached`, and the volume is served.

### Findings

**Defects**

**F1 (blocks). `create` takes a clone that holds data as a volume a create left empty.** `upload.rs:258`. The guard reads the head commit and refuses one with a parent, with segments or with a group. A clone's first commit has none of the three (`skynbd-engine/src/lib.rs:669` to `679`): it names the base's shard index and nothing else. A clone also inherits the base's shards (`lib.rs:646`), so a clone of a volume `skyzfsd` made passes the 128 MiB check, and it has no lease row until something serves it. Scenario: a clone of `pg-data` is made as `pg-clone` through `Engine::clone_volume`, under the id the name gives (`remote.rs:84` says the id is derived as the provisioner and node B derive it, so that each finds the volume the others name). Then `skyzfsd create --volume pg-clone --size <the same>` says "found empty" (S1). The first cut of the empty local volume holds the whole volume, object storage has no row, head's commit has no group, so the check's rule 2 uploads it onto the clone's head (S1b: the clone's block reads 7 before and 0 after). The clone's data is then at no ref. Certainty: certain, both steps run. This is Codex's first finding, and it is right.

How it can be reached today: not through the tools in the repository. `skynbd-api`'s provisioner, the one caller of `clone_volume` outside the engine's tests, takes the volume's lease before it clones when its refs are the object store's (`provision.rs:718` to `722`, then `809`), so its clones have a lease row and `place` refuses them one check earlier. The hole is open to a clone made through the engine directly, and to whatever M3 builds for "clone from a ref". Why I count it as blocking all the same: it is the lead's decision 1 ("refuse a name whose volume is not empty") not held for one of the two kinds of first commit the engine makes, the description says the volume is taken only when it is "exactly so", what it costs is a volume's data, and the fix is a few lines. A fix can be small: the empty commit's shard index is the empty index's, or the commit's kind is `Checkpoint` and not `Import`, or the volume has no clone-base object. Also, none of the three clauses is pinned by itself (U16, U17, U18 pass), so the test's volume "that holds data" trips at least two at once.

**F2. The end's limit does not bound the end.** `upload.rs:731` to `735`. `end` waits at most `limit` for the thread's answer and then joins the thread with no limit. The thread's last act is to drop its runtime, and a runtime's drop waits for its blocking pool. D7: with a blocking task of 8 s on the runtime, `end(true, 6 s)` says `Released` and returns after 8.0 s. In the daemon the blocking work would be a name lookup, which the HTTP client does on the blocking pool; that part is by reading and not traced here. With an endpoint given by address there is none. The same join is in the drop, so a non-zero exit can wait too. Certainty: certain for the mechanism, by reading for the cause. Not blocking: the device is already dealt with when this runs, and a resolver gives up in tens of seconds. `shutdown_background` on the runtime, or no join after the answer, would hold the limit.

**F3. The status names no holder while the lease is held.** `upload.rs:560` to `567` and `399`. `status.rs:125` documents `lease_holder` as "this daemon's while held, another's when lost", and the description's table says "the holder the row names". The code sets it only for a lost lease and for another holder's at the start, so a daemon that holds the lease shows `lease_holder` empty (S2, at the start and after a turn). Certainty: certain. This is Codex's third finding. Not blocking, but M2.4d's rig reads this file.

**F4. A message with two runs of 21 spaces.** `commands.rs:468`: "...could not be                      written in ... Remove ... and                      create it again". Two line continuations lost their backslash. Certainty: certain, by reading the bytes. No test reads the whole message.

**F5. With both keys in the environment, the client signs with no session token and in `us-east-1` whatever `AWS_REGION` says.** `upload.rs:143` to `152`, `skynbd-store/src/s3.rs:35`. Temporary credentials (STS, an SSO export) fail at the first request, and so does a bucket in another region. At `create` that is a refusal. At `serve` it is "no lease yet", and the volume is served with no upload: visible in the status only. Certainty: by reading; I have no store to run it against. This is Codex's second finding. The description says "as node B does", so it may be meant; then the help should say so.

**F6. Comments and messages that say more than the code does.**
- `lifecycle.rs:774` to `776`: "a start refused here changes no note, and the next start judges the failure as this one did". The note is unchanged; the judgement is not, where the failure was known by the store's marker alone (S3).
- "no device was touched" (`cli.rs:280`, `326`, `372`; `lifecycle.rs:525`; `upload.rs:27`): an acknowledging start has by then deleted the failed volume's waiting device, in `noted_failure`, before the open.
- `upload.rs:365`: "the volume has not failed" is also said on the acknowledging path, of a volume that has.

**Gaps in the tests**

**G1. Nothing at this head shows a daemon that uploads.** With each of these in place every test of the head passes, and no rig on this head serves a volume with a place: the queue's thread never answers the desk (W01), does not close it (W02), the desk's wake does nothing (W03), the lease is let go at every exit (W04) or never (W05), a refused start exits 8 (W08), the uploader's thread starts under another holder's lease (W11), the hook `release` is taken in a daemon that uploads (W12), the build with the hooks does not give the uploader its kill point (U25). My harness shows the loop works as built (D1 to D6), but its closure is a copy. Two ways to shrink the gap before M2.4d: make the desk's two lines and the choice of `release` functions that `run` and a test both call; or hold the merge until M2.4d's rig has run on it. The lead's call.

**G2. The daemon's `maker` is tested only in the build with the hooks.** The head's `uploading()` helper builds a `make` of its own (`upload/tests.rs:639` to `646`), so `maker` is reached only through `kill::maker`. In the default build an epoch off by one in the fence passes every test (U20p; D1, D3, D4 and D6 kill it), and in both builds a commit stamped with epoch 0 does (U21; D1 kills it).

**G3. Small ones.** The default lease time through `uploads_of` (W13), the lease's time in `devices` (W07), an end with no lease held that says "let go" (U12). U23 is no gap: the holder of a lost lease is set in two places and the plant removed one.

**Notes for the lead**

**N1. Every exit 10 costs a whole upload, and the holder's default is the host's name.** A start refused for the lease has written a new start token, so the cuts held are let go and the next cut holds the whole volume. That is decided. What makes it come up: after `kill -9`, a daemon that starts again under another name inside the lease's time (30 s) is refused, and its retry uploads the whole volume. The default name is `/proc/sys/kernel/hostname`. Where that changes between two starts (a pod's name), every restart inside 30 s costs a whole upload; where it is the same on two nodes ("localhost", a cloned image), the lease orders nothing, since the same name renews at the same epoch (N1 of #117's review, S5 here). The notes should say what the name must be.

**N2. A lease taken by a start that fails afterwards is kept for its time** (question 1, S5). Right by decision 4; one line in the notes.

**N3. `create`'s order leaves two things in the open.** A kill between the local volume and the file `remote` leaves a volume that `serve` serves with no upload and no word, and that cannot be given a place. Writing `remote` before the volume's own rename, with its names among the create's leftovers, would close the window. And "found empty" has no owner: two creates of one name (two directories, or two nodes) both succeed (S6), and the first whole cut of each replaces the other's head. Between two nodes the lease orders them; on one node under one holder name it does not. `serve` also does not compare the size in object storage with the local one. And by reading only: two creates of one name with two sizes at once can leave the loser's superblock under the winner's head, since the superblock is put without a condition and only head is swapped.

**N4. The store's calls on the queue's thread are live now**, the sync of the copies file among them (F1 of #117's review), and an idle uploader asks every 250 ms, so the queue of a volume with a place turns four times a second for ever. I measured neither. The benchmark's daemon rows serve a volume with no place, so they will not show it.

**N5. Exit 0 lets the lease go with cuts that may be held and not uploaded** (D3 and D4 end with one cut held). Another node can then take the lease and find object storage behind this node's disk. That is for the agent's handover to check, but the daemon's last line could say how many cuts it leaves held.

**N6. A start that could not reach object storage serves, and may then find the lease another holder's.** It goes on serving with no upload although the operator did not say `--serve-without-upload`. That is the same as a lease lost while serving, and I think right; the notes might say it.

**N7. The hook `write N PATH` of the cut-daemon rig and the uploader now read the same cut.** In a daemon that uploads, the uploader can let the cut go between two slices of a `write`. By reading only; it is for M2.4d's author.

### The plants

Each against `cargo test -p skyzfsd --features skyzfsd/test-hooks` (the daemon's tests in the default build are a subset of these), except: L01 and L02 also against `cargo test -p skyzfs-ublk --lib`, T01 against that alone, C01 against both builds of the daemon, and U20p against the default build. "passes" means every test of the head passes with the plant in place. U10 made two tests of the head wait for ever, and the script ended the run after 300 s.

| | File | The plant | Result |
|---|---|---|---|
| W01 | `serve.rs` | the queue's thread never answers the desk | **passes** |
| W02 | `serve.rs` | the queue's thread does not close the desk at its end | **passes** |
| W03 | `serve.rs` | the desk's wake does nothing | **passes** |
| W04 | `serve.rs` | the lease is let go at every exit that reaches the end | **passes** |
| W05 | `serve.rs` | the lease is never let go | **passes** |
| W06 | `serve.rs` | a volume with a place does not hold its cuts unless --hold-cuts | KILLED by `the_intent_region_reaches_the_store` |
| W07 | `serve.rs` | the lease's time is counted in the start's `devices` time | **passes** |
| W08 | `serve.rs` | a start refused for the lease exits 8, not 10 | **passes** |
| W09 | `serve.rs` | the lease's options are taken for a volume with no place | KILLED by `the_uploads_come_from_the_file_remote_and_the_lease_options_need_one` |
| W10 | `serve.rs` | --kill-at-upload is taken for a volume with no place | KILLED by `the_uploads_come_from_the_file_remote_and_the_lease_options_need_one` |
| W11 | `serve.rs` | the uploader's thread starts under another holder's lease (--serve-without-upload) | **passes** |
| W12 | `serve.rs` | the hook command `release` is taken in a daemon that uploads | **passes** |
| W13 | `serve.rs` | the lease's default time is an hour | **passes** |
| U01 | `upload.rs` | the lease is taken before the open | KILLED by `the_lease_is_taken_after_the_open_and_a_refusal_gives_the_lock_back` |
| U02 | `upload.rs` | a refusal leaves the store open | KILLED by `the_lease_is_taken_after_the_open_and_a_refusal_gives_the_lock_back` |
| U03 | `upload.rs` | another holder's lease never refuses the start | KILLED by `another_holders_lease_refuses_the_start_unless_the_operator_says_otherwise` |
| U04 | `upload.rs` | --serve-without-upload is not heeded | KILLED by `another_holders_lease_refuses_the_start_unless_the_operator_says_otherwise` |
| U05 | `upload.rs` | a take with no answer within the limit refuses the start | KILLED by `a_store_out_of_reach_or_silent_serves_with_no_lease_yet` |
| U06 | `upload.rs` | object storage out of reach refuses the start | KILLED by `a_store_out_of_reach_or_silent_serves_with_no_lease_yet` |
| U07 | `upload.rs` | AtStart::uploads says yes under another holder's lease | KILLED by `another_holders_lease_refuses_the_start_unless_the_operator_says_otherwise` |
| U08 | `upload.rs` | the uploader's thread lets the lease go at every end | KILLED by `any_other_end_keeps_the_lease` |
| U09 | `upload.rs` | the uploader's thread never lets the lease go | KILLED by `the_end_at_exit_0_lets_the_lease_go` |
| U10 | `upload.rs` | ending the thread does not close the desk | KILLED: the head's tests do not end (300 s): `any_other_end_keeps_the_lease` and `the_end_at_exit_0_lets_the_lease_go` wait for the close |
| U11 | `upload.rs` | a drop of the thread's handle (every exit but 0) lets the lease go | KILLED by `any_other_end_keeps_the_lease` |
| U12 | `upload.rs` | an end with no lease held says it was let go | **passes** |
| U13 | `upload.rs` | create takes a volume that has a lease row | KILLED by `a_create_refuses_a_name_that_is_another_volumes` |
| U14 | `upload.rs` | create takes a volume of another size | KILLED by `a_create_refuses_a_name_that_is_another_volumes` |
| U15 | `upload.rs` | create takes a volume with other shards | KILLED by `a_create_refuses_a_name_that_is_another_volumes` |
| U16 | `upload.rs` | create takes a head commit that has a parent | **passes** |
| U17 | `upload.rs` | create takes a head commit that has segments | **passes** |
| U18 | `upload.rs` | create takes a head commit that names a group | **passes** |
| U19 | `upload.rs` | the volume is made with shards of 64 MiB | KILLED by `a_create_makes_the_volume_with_the_empty_commit_and_takes_it_again` and 1 more |
| U20 | `upload.rs` | the uploader's commits are fenced by another epoch than the one held | KILLED by `the_uploaders_remote_falls_after_the_nth_swap_and_at_a_segment_before_its_swap` |
| U20p | `upload.rs` | the same as U20, against the default build's tests | **passes**; my scenarios kill it (D1, D3, D4, D6) |
| U21 | `upload.rs` | the uploader's commits carry epoch 0 | **passes**; my scenarios kill it (D1) |
| U22 | `upload.rs` | the status shows an unsure lease as held | KILLED by `the_status_shows_the_uploader_as_it_is` |
| U23 | `upload.rs` | the status names no holder for a lost lease | **passes** |
| U24 | `upload.rs` | the status does not show when the lease runs out | KILLED by `the_start_takes_a_lease_nobody_holds` and 1 more |
| U25 | `upload.rs` | the build with the test hooks does not give the uploader its kill point | **passes** |
| L01 | `lifecycle.rs` | a refused open ends the start as an error | KILLED by `a_refusal_after_an_acknowledging_open_keeps_the_failure_note` and 1 more |
| L02 | `lifecycle.rs` | a refused open removes the failure note | KILLED by `a_refusal_after_an_acknowledging_open_keeps_the_failure_note` |
| T01 | `status.rs` | a post of the store's counters wipes the upload's part | KILLED by `the_uploaders_post_is_kept_by_the_stores_and_written_on_its_own` |
| K01 | `kill.rs` | put:N counts every PUT, not the segments' | KILLED by `put_n_falls_once_the_nth_segment_put_has_landed` |
| K02 | `kill.rs` | put:N kills before the PUT is sent, and counts one that fails | KILLED by `put_n_falls_once_the_nth_segment_put_has_landed` and 1 more |
| K03 | `kill.rs` | swap:N kills before the commit is sent, and counts one that fails | KILLED by `the_uploaders_remote_falls_after_the_nth_swap_and_at_a_segment_before_its_swap` |
| K04 | `kill.rs` | swap:N also kills at the Nth segment PUT | KILLED by `put_n_falls_once_the_nth_segment_put_has_landed` |
| C01 | `cli.rs` | a default build takes --kill-at-upload | KILLED by `the_test_hooks_are_refused_in_a_default_build` |
| C02 | `cli.rs` | the exit status of a start refused for the lease is 9 | KILLED by `the_exit_statuses_are_the_agreed_numbers` |
| C03 | `cli.rs` | --kill-at-upload put:0 is taken | KILLED by `the_test_hooks_are_parsed_in_a_test_build` |
| C04 | `cli.rs` | --lease-ttl-s is not bounded | KILLED by `serve_takes_the_lease_options` |
| C05 | `cli.rs` | two of --endpoint --bucket --volume make a volume with no place | KILLED by `create_takes_a_place_in_object_storage_whole_or_not_at_all` |
| M01 | `commands.rs` | create does not write the file `remote` | KILLED by `create_whose_place_cannot_be_written_says_so` and 1 more |
| M02 | `commands.rs` | a place refused makes the local volume all the same, with no place | KILLED by `create_with_a_place_refused_makes_nothing` |
| X01 | `cut_commands.rs` | the hook command `release` is never refused | KILLED by `release_is_refused_in_a_daemon_that_uploads` |
| R01 | `remote.rs` | the file's id is not checked against the name's | KILLED by `a_file_that_is_not_exactly_the_five_lines_is_refused` |
| R02 | `remote.rs` | a line after the id is taken | KILLED by `a_file_that_is_not_exactly_the_five_lines_is_refused` |

### The scenarios

| | What | Result on the head |
|---|---|---|
| D1 | A cut made through the queue goes up while the device is idle; a second cut overwritten under the upload goes up as it was; a stop lets the lease go; the commit carries the lease's epoch | passes: 260 ms, nine calls, the second cut through `Syncing`, `Released`, the row at expiry 0 |
| D2 | The control: a desk whose wake does nothing | passes: nothing uploaded in 4 s |
| D3 | A stop while a commit is under way | passes: `Released` in 0.7 s, head whole, the next instance uploads the cut |
| D4 | A stop between the swap and the release | passes: the next instance finds the cut committed |
| D5 | The queue's thread panics | passes: the desk is left open, the drop ends the uploader, the lease kept |
| D6 | Another holder takes the lease; the daemon's next commit | passes: refused, nothing lands, `lease lost` to node-b at epoch 2, the end lets nothing go |
| D7 | The end with blocking work left on the runtime | **fails (F2)**: 8.0 s against a limit of 6 s |
| S1 | `create` on a clone that holds data | **fails (F1)**: "found empty" |
| S1b | The first cut of a volume bound to a clone | passes, and records the loss: the clone's block reads 7 before, 0 after |
| S2 | `lease_holder` while the lease is held | **fails (F3)**: empty |
| S3 | Exit 10 on an acknowledging start, the real store in a directory | passes: the marker removed, the token new, the next start opens without the flag |
| S4 | A take that landed unheard | passes: the uploader holds the same epoch |
| S5 | A start that took the lease and failed | passes: another holder refused for the lease's time, the same holder at once |
| S6 | Two creates of one name | passes: "made", then "found empty" |

### CI and Codex

All 37 checks on e42ea6a ended in success and two were skipped (the badges, and the replay rig's reference-server jobs, which this change does not trigger). What the rigs showed, from their logs and not their colour:

- **The daemon rig** on `skyzfsd`, Ubuntu 24.04, 26.04 and 26.04-arm: the judge passed on all three. On the two logs I read, 53 pins PASS, `pin_c_kill_with_applied_unacknowledged` NA as its table expects, and `pin_failed_volume_while_a_request_is_held` WAITING as before.
- **The replay rig through `skyzfsd`**, on 24.04 and 26.04, with its "write into a hole" jobs on both, the pull-request budget: exit 0, 121 of 2108 and 120 of 2062 prefixes checked, so design §9's criterion is not shown by these runs and is not meant to be.
- **The control rig** (24.04, 26.04, 26.04-arm), **the cut-daemon rig** (26.04, 26.04-arm) and **node B's rig**: success.
- All of them serve volumes with no place. They show that nothing changed for those.

Codex left three findings on e42ea6a, and I checked each against the code: the clone (P1) is F1 and right; the session token (P2) is F5 and right by reading; the holder in the status (P2) is F3 and right.

### What I could not verify

- `run` itself: there is no ublk here. The wiring lines ran only as a copy in my harness, and the eventfd wake only by reading.
- Anything against a real object store: the credentials (F5), a request that lands after its future is dropped, the limits of 10 s, 30 s and 5 s on a real network.
- That the HTTP client's name lookup is the blocking work of F2: by reading, not traced.
- The latency the desk's calls cost the pool (N4).
- I did not review #117's own files.

### Files

In `review/m2.4c-2/` on `skyzfs-review`: this file; `plants.py` and `plants.out`; `scenarios.rs`, `scenarios.py` and `scenarios.out`; `t_plain.out` and `t_hooks.out`.
