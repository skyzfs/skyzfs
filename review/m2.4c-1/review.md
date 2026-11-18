## Review of #117 at 1415db1 (M2.4c-1)

**Verdict: can merge. Nothing blocks.** I could not make two nodes commit for one volume, the backoff never hot-loops or starves the lease inside `turn`, and the desk loses no wake and answers no call wrongly. The second commit, 1415db1, answers Codex's finding rightly: both of its tests fail without it, no wait is stretched past the lease's renewal by it, and it opens no loop that spins.

I found one defect that matters for the daemon and not for this merge: the desk waits for the copies file's sync on the queue's thread (F1). It costs latency, not data, nothing calls the desk yet, and its fix changes what `Desk::answer` promises, so it should be settled (fixed, or accepted by the lead as the read on the queue's thread was) before M2.4c-2 puts the desk on the daemon's queue. Besides it: a hazard in `Uploader::run` (F2), a reading of the backoff that is right for refusals and too wide for plain errors (F3), what is left of the renewal's timing after 1415db1 (F4), a rare case in which both nodes lose the lease (F5), nine gaps in the tests, and notes for M2.4c-2.

I read the diff against 4e4e076, the lead's decisions in the notes (M2.4's shape, G7) and `skynbd-refs` before the description. Then, all on 1415db1 unless said:

- **The head's own tests** pass here, rustc 1.100.0: 21 unit tests, 3 crash, 22 driver, 2 queued, 9 uploader (`t_plain.out`; at 5bebcc0 the uploader had 7, `t_plain_5bebcc0.out`).
- **56 plants** of my own, one at a time, against `cargo test -p skyzfs-upload --lib --tests`, in 58 runs, two being run a second time with a scenario (`plants.py`, `plants.out`): 34 runs killed, 24 pass every test of the head. Of the 24, two are the second runs, two change nothing that can be seen or harmed (X4, Q10), and the other twenty are the gaps below. The table is at the end.
- **Sixteen scenarios** the head's tests do not have (`scenarios.rs`, run by `scenarios.py`; results in `scenarios.out`, and those of 5bebcc0 in `scenarios_5bebcc0.out`). They use the head's own fixture, the in-place store on the simulated disk, the real engine and the real refs store over the fault store, with a clock for each node's lease apart from tokio's. All sixteen pass.

### What 1415db1 changes, and where my results differ from 5bebcc0

The commit reads the clock again after the lease is kept and after the step (`uploader.rs:173`, `206`), counts the next renewal from when the take was sent, and a failed take's backoff and the rest after a lost lease from when the take came back (`uploader.rs:251` to `263`). `git diff 5bebcc0..1415db1` touches `uploader.rs` and its tests only.

- **Each new test fails without the fix.** X0 takes the whole change back: both new tests fail, and C1 gives the figures of 5bebcc0 again to the millisecond. X2 takes back the read after the step alone: both fail.
- **Does it stretch a wait past the lease's bound? No.** Every wait a turn returns is still the lesser of its own time and the time to the next renewal, and both are now counted from the true time, so a wait can only be shorter than before. C4 asserts the two ways out of a turn that take no step: a first take that fails after 5 s is tried again 1 s after it failed, and a renewal that takes 4 s in the middle of a backoff is followed by a wait of 6 s, which is what is left of the ten. B3 as before: twenty minutes of failing steps, at least 20 s left on the lease at every turn.
- **Does it shorten a wait to a loop that spins? No.** A turn now returns zero in one more case, where the renewal fell due during the step. The next turn then keeps the lease, and that either takes time or sets the next renewal a third of the lease ahead. C3 turns an uploader for ten minutes over lease calls that are slower than the renewal's interval: 12 s and succeeding, 12 s and failing from the first take, 12 s or 40 s and failing after the first take (unsure), with steps that find nothing or fail at once. The turns are 9 to 69 in ten minutes, against 2418 for an idle uploader whose lease answers at once, and no two turns that take no time and ask for no wait follow each other. A failing step's backoff is counted from the failure and is waited in full (the new test; C1's renewals).
- **The unsure window** is not changed by it: unsure has no bound in time on either head (question 1). While unsure the renewal is tried every third of the lease from when the last try was sent, so a try that takes longer than that is followed by the next at once; C3 shows 50 tries in ten minutes at 12 s a try, and the uploads take one step between two tries.
- **What it leaves** is the time of a step that is under way when the renewal falls due: F4.
- **Not held by a test:** the read of the clock after the lease is kept (X1) and the failed take's backoff counted from when it came back (X3) pass both new tests, because the take is instant in them. C4 fails with either. X5 (the next renewal counted from when the take came back, not from when it was sent) passes everything; it is the author's cautious choice and nothing pins it. X4 changes nothing that can be seen: a lost keeper is never kept again. See G9.
- **Other differences from 5bebcc0:** none in what the scenarios assert. C1's figures move as F4 says. The scenarios on real threads (D1 to D4) differ in counts that depend on timing: D2 1339 turns for 1313, D4 269 and 31 runs for 268 and 32. L1 to L5 and B1 to B3 are the same to the digit. The plants in `lease.rs`, `queued.rs` and `backoff.rs` were run on 1415db1 only, except K1 to K4, which I had run on 5bebcc0 too with the same results.
- **CI and Codex on 1415db1:** green; Codex's review of 1415db1 has no finding, and the author answered its one thread.

### The three questions

**(1) The lease is safe against split brain between holders of different names. Two nodes can both upload; only the one the row names can commit.**

The reason, in my words. A commit goes through `ActingRefStore`, so through `ObjectRefStore::update_as` (`skynbd-refs/src/object_store.rs:380` to `417`): it reads the refs object, checks that the lease row names this holder at this epoch (`lease::fence`, `lib.rs:90`), and writes head, the cut's row and the lease row back in one conditional put. No clock and no expiry is read there. So what `lease.rs` decides is who may take the lease and when, which is availability. Whether a commit lands is decided by the row at the instant of the swap, whatever the keeper believes. A keeper that is wrong about its lease in any way costs an upload that is refused, never a commit under another's lease.

That argument rests on two things this pull request cannot hold by itself, both M2.4c-2's:

- **The remote the uploader makes must be fenced by the epoch it is given.** `Uploader::new` takes any `make: FnMut(u64) -> R`. The tests' `fenced` wraps the refs in `ActingRefStore::new(refs, holder, epoch)`; a `make` that ignored its argument would commit with no fence at all, and nothing in the crate would notice.
- **A holder name must mean one daemon.** The refs store renews for the same holder at the same epoch (`lib.rs:126`), so two daemons under one name both hold epoch 1 and both pass the fence. Scenario L3 shows it step by step: each node lands whole cuts and deltas of its own image onto the other's head, with the row at `("node-a", 1, ...)` throughout. L2 runs 60 random programs that way: 4709 commits, 2580 by one node and 2129 by the other, 1140 changes of hand. See N1.

**L1 is my attempt at two nodes committing.** 400 programs from seeds: two nodes with a disk and a pool each and one volume in object storage, under two holder names, their uploaders turned in any order, a random number of turns at a time so that one stops anywhere inside the other's upload. Between turns: cuts on either pool, either node out of reach of the store, either node's clock jumping up to 100 s ahead or back, 300 ms to 70 s passing, a daemon dying and starting again, a clean stop with a release. After every turn in which head moved it asserts that the lease row names the node that turned at the epoch its remote was made with, that the keeper of that node holds that epoch, that no commit lands under an older epoch than one before it, that head reads as that node's volume at the instant of the cut head's group names, and that fsck passes. 156 017 turns, 25 455 commits (14 316 and 11 139), 535 changes of hand, 2876 turns with the lease unsure: no assert fired. At the end of each program both nodes reach the store and the clocks are set equal: somebody uploads in 345 programs, in 16 nobody does because the holder's daemon is stopped and the other had lost the lease, in 39 no uploader runs, and in none are both running with neither uploading.

The cases the question names:

| Case | What happens | Shown by |
|---|---|---|
| What `Unsure` allows | The uploads go on under the epoch last held: checks, reads, PUTs of segments and commits. There is no bound in time and no clock in it; the renewal is tried again every third of the lease's time. What bounds it is the fence at the next commit, or the next renewal that reaches the store. Another node gains nothing from our being unsure: it can take the lease once the row has expired by its own clock, exactly as if we held it, and from then our commit is refused | the keeper's two outage tests; L1. No test of the uploader holds it: G1 |
| Both nodes uploading | Possible, and harmless to head: a node that is unsure, or lost and not yet told, PUTs segments until its commit is refused. They are orphans for M4's collection (N3 of #114's review) | L1; L5 (20 480 bytes of segments become 61 440 with nothing landed) |
| Clock jumps | The expiry is written and judged by the wall clock of whoever calls the refs store; the renewals are scheduled on the monotonic clock. A node whose clock is ahead of the holder's by more than what is left of the lease takes a live lease at its start; the loser is told at its next renewal or commit. It costs the loser its uploads until its daemon starts again (lost is final), and no commit | L1: in 7 programs the lease changed hands once more when the clocks were set equal, and never twice |
| A renewal delayed past the expiry | Nobody took it: the take keeps the epoch. Somebody took it: lost | the head's lease tests |
| A take at the same epoch | A daemon that starts again under its name gets its predecessor's epoch. Right for one daemon per name; see N1 | the head's test; L3 |
| The row deleted | The renewal finds no row and the lease is lost, named as nobody at epoch 0, for good. Until that renewal the commits are refused as conflicts (G7): L5, under question 2 | L5. No test of the head holds it: G5 |
| The row another holder's, at an older or equal epoch | Lost: the renewal compares the holder too (`lease.rs:183`), and so does the fence. An equal epoch under another name can exist only after a row was deleted and taken anew | by reading; K2 passes the head's tests and L1, see G6 |
| The row ours at another epoch | Lost, naming ourselves | the head's test; K1 killed |
| A take that comes back at a newer epoch | Lost and given back. Safe; but the node that held it in between loses it too: F5 | L4; K4 and K4b killed |

Against the decisions in the notes: "a lease lost while serving stops the upload and not the serving" is as built (the uploader only ever stops itself), and "a start that cannot reach the object store serves and uploads when it can" is the `NoLease` state with its own backoff. "A start that finds the lease held by another holder is refused" is the daemon's; note that here the first take is made at the uploader's first turn, so after a start that could not reach the store, another holder is found while serving and reads as a lease lost.

**(2) The reading of "reset on a success" keeps the rule's point. Inside `turn` I found no hot loop and no starved lease. For plain errors the reading waits too long (F3).**

- **What counts as a success** is `Uploaded`, `UploadedNotHeld` and `FoundCommitted` (`uploader.rs:225` to `232`): head's row names a cut of ours. That is the right set. `FoundCommitted` belongs in it: B2 has two steps fail (waits 1 s, 2 s), then the check finds the cut committed, and the next failure waits 1 s. `Kept` resets nothing and stops the driver, so it does not matter. A cut lost or a mismatch resets nothing, which is right: nothing went up. Only `Uploaded` is held by a test of the head (G7).
- **A refused cycle waits twice.** The waits of a store that refuses every commit are 1 and 2 s, then 4 and 8, 16 and 32, 64 and 128, 256 and 300: the cap is reached in the fifth cycle, and from then a whole cut goes up every ten minutes where the rule's letter ("doubling from 1 s to 5 min") gives five. Both meet the rule's purpose. I would wait once, at the give-up: the first wait sits between the refused commit and one read of the row and head, and buys nothing the second does not. The lead's call. The test pins the doubling and not that the second wait is waited (G8).
- **Hot loops.** Every path of `turn` that returns a wait of zero has moved the driver on (a check that found a cut, a slice, a sync, a commit, a cut lost, after which the store offers nothing until the pool makes one), or has found the renewal due, which the next turn serves. Idle waits 250 ms, an error and a give-up wait, lost and stopped rest 60 s. P4, P7 and P13 remove one wait each and are killed. The one hot loop I found is outside `turn`: F2.
- **Starving.** A turn keeps the lease before it steps (`uploader.rs:171`), and a wait ends early for the lease (`next_wait`). B3: twenty minutes of failing steps, waits up to 300 s, and the row never has less than 20 s of its 30 s left when a turn begins; P8 (the wait does not end for the lease) is killed by the two new tests, and B3 fails with it. What is left is the time of one step: F4.
- **Waiting too long:** F3.
- **G7 now.** L5 removes the lease row under an uploader that holds the lease, and the pool makes a cut. The cut goes up, the commit is refused as a conflict (wait 1 s), the row and head are read again and the commit is given up (2 s); it goes up again at 3 s, is refused (4 s) and given up (8 s); at 10 s the renewal finds no row and the state is `LeaseLost { holder: "", epoch: 0 }`. Two uploads, nothing landed. So today the backoff slows it and the lease stops it, within a third of the lease's time. A store that never had a row cannot be met any more: the first take creates it.

**(3) The desk is sound: no deadlock, no lost wake, no call answered wrongly, at any moment of a close. But the queue's thread does wait, for a sync (F1).**

- **Locks.** One mutex, held only to move a call in or out (`queued.rs:127`, `131`, `141`, `161`). The store's call, the wake and the reply are all outside it. The queue's thread never awaits anything of the uploader's: `reply.send` does not block.
- **Wakes.** `ask` leaves the call under the lock and wakes after (`queued.rs:166`, `168`), so a queue that looked at an empty slot and went to sleep is woken by a wake that comes after the call is there. D3 is a queue that turns only when woken and never looks on its own: 20 000 calls, none unanswered. The head's tests hold that a call wakes (Q6 killed) and not that it wakes after the call is left (Q7b, G3).
- **A call answered wrongly.** Each call carries its own reply, so an answer cannot reach another call. A call whose asker went away (a cancelled `turn`) is replaced by the next call or answered to nobody.
- **Close.** A call the queue took before the close is answered; a call waiting is dropped and its asker gets the empty answer; later calls get it at once. D4 closes the desk from a third thread after 0 to 139 answers of an upload of about 131 calls, 300 times: the uploader always came to rest, and object storage held nothing new in 269 runs and the cut, whole and readable, in 31. A closed desk can land nothing by itself: the driver asks the store for the cut once more before it commits (`driver.rs:505`), and a closed desk has none. After a commit that landed, a closed release reads as "gone", the naming answers false, and the next instance finds the row naming the cut (rule 1 of the check). Q1 to Q5 change the close or one closed answer each; the head's closed-desk test kills all of them.
- **Threads and drop order.** The crate owns no thread and has no `Drop`; the desk lives in an `Arc`. Two things follow for M2.4c-2 (N4): the close must run on every way out of the queue's thread, and the wake can be called once after the close.

### Defects (none blocks this merge)

**F1. The desk waits for the copies file's sync on the queue's thread.** `queued.rs:84` answers `Call::SyncCopies` with `Store::sync_copies`, which begins the sync on the file's helper and then waits for its end (`local.rs:170` to `174`: `cut_copies_synced(true)`, down to `helper.results.recv()` in `skyzfs-local/src/fs.rs:665`). `Desk::answer` is called on the queue's thread, so every request of the pool waits for one `fdatasync` of the copies file, once for each uploaded cut that read a copy. The store has the two halves apart for this reason: `sync_cut_copies` is "on the file's helper, as the image's syncs are, so that the queue goes on meanwhile" (`inplace.rs:3042`), and the helper wakes the store's caller when the sync ends.

Scenario D1: a cut that read a copy, with the copies file's sync held for 501 ms as a slow disk holds it. The queue's thread is inside `Desk::answer` for all of it (longest single answer 505 ms), with one caller waiting at the end of the held sync and no turn of the queue's thread meanwhile. Certain.

How much it costs is the disk's sync time: the notes have 52 and 53 ms for a sync of the home image on the slow-sync machines of M1.3m's rounds, which is why that sync left the worker. It is paid only by a cut that read a copy, so while an upload lags behind the pool's writes, which is when the pool is busiest.

It is not this merge's to block: nothing calls the desk, and no data is at stake. But the description's sentence "a turn never waits on the uploader" reads as more than the code gives, and the fix is in the desk's shape (begin the sync at `answer`, keep the reply, and send it from a later turn once `cut_copies_synced(false)` has an outcome; `Store` then needs the two halves). So: fix it in M2.4c-2 before the desk is wired, or the lead accepts it and the notes say so, as they do for the read (the shape decided on 2026-11-15 names the read on the queue's thread, and not the sync). No test of the head sends a sync of the copies through the desk at all (G4).

**F2. An uploader whose `run` was stopped spins if it is turned again.** `run` (`uploader.rs:289`) drops `turn()` when `stop` resolves. `Driver::step` is not made to be dropped at an await: it has put `Stage::Check` in place of its stage (`driver.rs:313`), and `read` has moved the slice buffer out (`driver.rs:432`), so the driver is left checking with a buffer of zero blocks. The store refuses a read into such a buffer without abandoning the cut, so each cycle is: check (a read of head and the row in object storage), read refused, the cut counted lost, wait zero.

Scenario D2: a run stopped while a read waits at the desk, then run again for 1.5 s: 1339 turns, 669 cuts counted lost, 1342 checks answered, nothing uploaded, the cut still held. Certain. Nothing in this pull request turns an uploader after a stop, so it is a hazard for the caller and not a defect that runs today. Either `run` takes the uploader by value, or its doc says that an uploader is not turned again once `run` has returned. `run` has no test in the head (G2).

**F3. One backoff for refusals and for plain errors, reset only at a commit: an upload over a store that fails now and then waits minutes for each failure.** `uploader.rs:241` enters the same `Backoff` for any error as for a give-up, and only a cut committed resets it. A failed PUT of a slice loses nothing (the driver stays at its stage and goes on from there), and hundreds of steps succeed between two failures, yet every failure of one cut's upload waits twice as long as the one before, and from the tenth on 300 s.

Scenario B1, one cut of 64 blocks read a block a slice (512 calls that PUT sealed segments), tokio's time: no failure, uploaded at 0 s; every 50th call fails (10 failures), 811 s; every 20th (25 failures), 5311 s; every 8th (64), 17 011 s. Certain. With P3c planted (any step that goes on resets the wait) B1 gives 10, 25 and 64 s, a second for each failure, and the head's conflict test fails, as the description says of its plant 4.

The lead's rule speaks of a conflict that is given up. For that the author's reading is the right one. For an error the rule says nothing, and its purpose (do not send the whole cut again every cycle) does not apply, since an error does not start the cut over. A whole first upload of a large volume is where this bites: tens of thousands of PUTs, and each rare failure after the ninth costs five minutes. I suggest two counts, or one count with two resets: a refusal's wait is reset by a commit, an error's by any step that succeeds. It needs the lead's word because it reads the rule; it does not block.

**F4. A step that is under way when the renewal falls due still delays it by the step's own time.** This is what is left of Codex's finding after 1415db1, and it is small. The lease is kept between steps, so the time between two renewals is at most a third of the lease plus the longest step (and the take's own time). C2, an idle uploader with a 30 s lease whose check of the store takes its time only when it begins in the last quarter second before the renewal is due: checks of 5, 15, 19, 21 and 25 s give 14.75, 24.75, 28.75, 30.75 and 34.75 s between two renewals, and with the last two the row passes its expiry. C1, steps that fail after 0, 5, 15, 19, 21 and 25 s: at 5bebcc0 the longest time between renewals was 10, 15, 25, 29, 31 and 35 s; at 1415db1 it is 10, 11, 23, 27, 29 and 33 s. So the commit takes away the wait that was spent twice, and a step of more than two thirds of the lease can still let the row lapse when it begins at the wrong moment.

An expired row matters only if another daemon starts on the volume just then, and then the fence decides, so no commit is at stake. Nothing to fix here. For M2.4c-2: either the object store's timeouts and the queue's worst turn stay well under two thirds of the lease's time, or the renewal is made beside the step and not between steps.

**F5. A renewal's take that comes back at a newer epoch costs both nodes the lease.** `lease.rs:204` to `217`: the read and the take are two calls, and when another holder took the lease between them and that lease has expired too by our clock, our take steals it at a newer epoch; the keeper gives it back and is lost, naming nobody. Scenario L4, with a real keeper for the other node B: A is lost at `("", 3)`, the row is `("node-a", 3, 0)`, and B, alive and with nothing wrong, is lost at its next renewal as "lost to node-a at epoch 3". Ten lease times later both are still lost and nobody uploads until a daemon starts again.

It is safe (nobody commits), and the give-back is right as far as it goes (K4 takes it out: the row then also stays live for a lease's time). It is rare: B must take between A's read and A's take and then either not renew for longer than the lease or sit behind A's clock by more than the lease. A renewal that is one conditional write ("renew if the row is mine at this epoch", inside the refs store's own read and put) would close it and need no read first; `skynbd-refs` has no such call today. A note for the lead more than a defect of this pull request. The status line it leaves on B is misleading: it names A, which holds nothing.

### Gaps in the tests (not blocking), most severe first

Each is a plant that passes every test of the head.

**G1. "The uploads go on while the lease is unsure" has no test of the uploader.** P1 makes a turn take no step while the lease is unsure: everything passes. The head's outage test fails the uploads' way to the store together with the lease's, so no step could succeed while unsure, and the uploads resume only once the renewal holds again. K13 (a renewal whose take fails after its read succeeded loses the lease) passes too; only a failed read is held (K11 killed). The first new test of 1415db1 has what is needed, a way of its own for the lease: fail that one and let the uploads' way work.

**G2. `Uploader::run` has no test.** R1 (no sleep between turns), R2 (no stop during a turn) and R3 (nothing published) all pass. It is the function the daemon will call. See F2.

**G3. The order of the call and the wake.** Q7b wakes the queue's thread before the call is left, on an open desk: everything passes, because the upload test's queue looks at the desk every 5 ms whether woken or not. With it D3 loses 123 wakes of 20 000 and fails.

**G4. No sync of the copies goes through the desk.** Q8 answers `Call::SyncCopies` as done without asking the store: everything passes. The desk's upload test reads no copy. This is the path F1's fix will change.

**G5. The lease row gone at a renewal.** K3 takes the lease again where the renewal finds no row: everything passes. With it L5 does not come to "lease lost": the keeper makes the row anew at epoch 1 and uploads onto a volume whose row somebody removed.

**G6. Three more lines of `lease.rs`.** K8 (`fenced` does nothing): the next renewal finds the same thing, so no test sees it; an uploader would go on sending cuts for up to a third of the lease. K5 (another holder found at the renewal's take leaves the lease unsure): the next renewal's read loses it. K9 (an unsure lease is not released). K2 (the renewal ignores the holder where the epoch is ours) needs a row deleted and taken anew by another name; with it that holder's expired lease would be stolen and given back.

**G7. What counts as a success.** P2 (`FoundCommitted` does not reset the wait) and P3 (`UploadedNotHeld`) pass. B2 fails with P2: the wait after the cut was found committed is 4 s for 1 s.

**G8. Waits announced and not waited.** P4b enters the backoff at a give-up and returns a wait of zero: the conflict test passes, since it reads the wait from the status and not what `turn` returned. So "a refused cycle waits twice" is pinned as two doublings, not as two waits. P15 (the lease's backoff is never reset) and P6 (a stopped driver is stepped) pass as well.

**G9. The clock after the lease is kept (1415db1).** X1, X3 and X5, above. C4 holds the first two.

### Notes for M2.4c-2 and the lead

**N1. A holder name must mean one daemon on one node.** See question 1, L2 and L3. A restart under the same name is renewed at the same epoch, by the refs store's rule that a holder never fences itself, so the epoch tells nothing about two processes of one name. If the daemon's holder is the node's name, two nodes configured with one name, or a volume's files copied to a second node of the same name, both commit. Whatever M2.4c-2 uses as the holder should be unique per node at least, and the notes should say that the lease does not guard against a name used twice.

**N2. The lease's epoch reaches the commit only through `make`.** `EngineConfig.epoch` ("stamped into every commit") is not set from it in the tests' `make`, so the commits of this pull request's tests carry epoch 0. If the commit's epoch is meant to be the lease's, `make` in the daemon sets both.

**N3. `Keeper::release` and then another turn takes the lease again**, at the same epoch if nobody took it, or at a newer one, in which case the next commit is fenced and the row stays ours and live for a lease's time. The daemon should release after its last turn.

**N4. The desk's close and wake.** If the queue's thread ends without `close` (a panic), a call waiting is never answered and never dropped: `run` still returns at its stop, a bare `turn().await` does not. And `ask` can call the wake once after the desk has closed (it checks, leaves the call, the desk closes, it wakes), so the daemon's wake must be harmless once the queue has ended.

**N5. Figures after a close.** In D4's 269 runs that landed nothing, 254 count the cut as lost and 13 as committed and gone from the store. Neither happened; the daemon was stopping. If the status is read after a stop it should not show these.

**N6. Smaller things.** An idle uploader asks the queue's thread four times a second for a cut that comes once a transaction group; a wake from the store when a cut is ready would do instead, if the wakes show in M2.T4. Where the lease's calls take longer than a third of the lease, every turn renews before it steps, so the uploads go at one step a renewal (C3); that is a sick store, and the steps are slow there too. A stopped or lost uploader rests 60 s between turns whatever the lease's time, so a stopped one (a cut kept) lets its lease lapse and takes it again each minute; harmless, but it holds the lease while it uploads nothing. A lost uploader keeps its driver and engine in memory. `ask`'s comment "the slot is empty" does not hold after a cancelled `turn`; the code is right all the same.

### The plants

On 1415db1. X are for that commit's change, K for `lease.rs`, P and R for `uploader.rs`, B for `backoff.rs`, Q for `queued.rs`. "The new test of a failing step" and "of an idle check" are the two tests 1415db1 adds; "the conflict test" is `a_commit_refused_again_and_again_...`. A scenario that only records passes with any plant; it is run for its figures (`plants_detail/<id>.with`).

| | Plant | The head's tests | With a scenario |
|---|---|---|---|
| X0 | All of 1415db1's change taken back: one clock read a turn | killed: the new test of a failing step, the new test of an idle check | C1 passes |
| X1 | The clock is not read again after the lease is kept | **pass** |  |
| X1s | As X1 (the clock is not read again after the lease is kept), with the scenario of a slow take | **pass** | C4 fails |
| X2 | The clock is not read again after the step | killed: the new test of a failing step, the new test of an idle check | C1 passes |
| X3 | A failed take's backoff is counted from before the take | **pass** |  |
| X3s | As X3 (a failed take's backoff is counted from before the take), with the scenario of a slow take | **pass** | C4 fails |
| X4 | The rest after a lost lease is counted from before the take | **pass** |  |
| X5 | The next renewal is counted from when the take came back (not from when it was sent) | **pass** |  |
| K1 | A renewal takes the row as its own whatever the epoch | killed: `this_holders_name_at_another_epoch_is_lost` | L1 passes |
| K2 | A renewal takes the row as its own whatever the holder | **pass** | L1 passes |
| K3 | A renewal that finds no row takes the lease again | **pass** | L5 fails |
| K4 | A renewal that took the lease at a newer epoch does not give it back | killed: `a_lease_taken_and_let_go_by_another_between_the_read_and_the_take_is_lost` | L4 passes |
| K4b | A renewal that took the lease at a newer epoch holds it at that epoch | killed: `a_lease_taken_and_let_go_by_another_between_the_read_and_the_take_is_lost` | L1 passes |
| K5 | A renewal's take that finds another holder leaves the lease unsure | **pass** |  |
| K6 | A first take that finds another holder is tried again (not lost) | killed: `another_holders_lease_stops_the_uploads_for_good`, `another_holders_lease_at_the_first_take_is_lost_and_left_alone` |  |
| K7 | A lost lease is taken again at the next keep | killed: `another_holders_lease_stops_the_uploads_for_good`, `a_lease_taken_and_let_go_by_another_between_the_read_and_the_take_is_lost` and 1 more | L1 passes |
| K8 | A commit the fence refused does not lose the lease | **pass** | L1 passes |
| K9 | An unsure lease is not released | **pass** |  |
| K10 | An unsure lease has no epoch (LeaseState::epoch) | killed: `an_outage_longer_than_the_lease_does_not_lose_it_where_nobody_else_took_it` |  |
| K11 | A renewal whose read fails loses the lease | killed: `a_store_out_of_reach_for_longer_than_the_lease_does_not_stop_the_uploads`, `a_lease_another_holder_took_during_an_outage_is_lost_and_never_taken_back` and 1 more |  |
| K12 | A release leaves the keeper holding | killed: `a_fenced_commit_loses_the_lease_and_a_release_lets_a_held_one_go` |  |
| K13 | A renewal whose take fails loses the lease | **pass** |  |
| P1 | No step while the lease is unsure | **pass** |  |
| P2 | A cut found committed does not end the wait | **pass** | B2 fails |
| P3 | A cut uploaded and not held does not end the wait | **pass** |  |
| P3b | A cut uploaded does not end the wait | killed: the conflict test |  |
| P3c | Any step that goes on ends the wait (the reading the author sets aside) | killed: the conflict test | B1 passes |
| P4 | No wait after a step that failed | killed: `a_store_out_of_reach_for_longer_than_the_lease_does_not_stop_the_uploads`, the new test of a failing step |  |
| P4b | No wait after a commit given up | **pass** |  |
| P5 | A commit the fence refused is an error like another (wait and try again) | killed: `a_commit_the_fence_refuses_loses_the_lease_and_lands_nothing` | L1 passes |
| P6 | A stopped driver is stepped all the same | **pass** |  |
| P7 | A step is taken during the wait | killed: the conflict test, the new test of a failing step |  |
| P8 | The wait does not end for the lease's renewal | killed: the new test of a failing step, the new test of an idle check | B3 fails |
| P9 | The lease is kept at every turn | killed: `a_commit_the_fence_refuses_loses_the_lease_and_lands_nothing`, `the_lease_is_kept_every_third_of_its_time` |  |
| P10 | The lease is renewed at its whole time (not a third) | killed: the new test of a failing step, the new test of an idle check and 1 more |  |
| P11 | The driver is stepped though the lease is lost | killed: `another_holders_lease_stops_the_uploads_for_good` | L1 passes |
| P12 | The driver is stepped with no lease (at epoch 0) | killed: `no_lease_no_upload_and_the_cut_goes_up_once_the_lease_is_held` |  |
| P13 | An idle uploader asks again at once | killed: the new test of an idle check, `an_idle_uploader_asks_again_after_a_quarter_second` and 1 more |  |
| P14 | A failed take of the lease is tried again at once (no backoff) | killed: `no_lease_no_upload_and_the_cut_goes_up_once_the_lease_is_held` |  |
| P15 | The lease's backoff is never reset | **pass** |  |
| R1 | Run does not sleep between turns | **pass** |  |
| R2 | Run does not stop during a turn | **pass** |  |
| R3 | Run does not publish | **pass** |  |
| B1 | The wait does not double | killed: the conflict test, the new test of a failing step and 3 more |  |
| B2 | The wait has no upper bound | killed: `the_wait_doubles_from_a_second_to_five_minutes` |  |
| B3 | A success does not reset the wait | killed: the conflict test, `a_success_starts_the_waits_again` |  |
| Q1 | Close leaves the call waiting | killed: the closed-desk test |  |
| Q2 | A call on a closed desk is left there and waited for | killed: the closed-desk test, the desk's upload test do not end |  |
| Q3 | A read on a closed desk answers a slice of nothing (not an error) | killed: the closed-desk test |  |
| Q4 | A release on a closed desk answers that the cut was let go | killed: the closed-desk test | D4 passes |
| Q4b | A naming on a closed desk answers that it named | killed: the closed-desk test | D4 passes |
| Q5 | The sync of the copies on a closed desk answers done | killed: the closed-desk test | D4 passes |
| Q6 | A call does not wake the queue's thread | killed: the closed-desk test does not end | D3 fails |
| Q7 | The queue's thread is woken before the call is left | killed: the closed-desk test | D3 fails |
| Q8 | The desk answers the sync of the copies as done without asking the store | **pass** |  |
| Q9 | The desk answers a release without asking the store | killed: the desk's upload test |  |
| Q10 | The desk does not answer a call after it has closed, though it took it before | **pass** |  |
| Q7b | The queue's thread is woken before the call is left, on an open desk only | **pass** | D3 fails |

Read with care: K1, K4b, K7, K8, P5 and P11 pass L1 because each of them is caught, where it matters, by the fence or by the next renewal, which is what L1 asserts; L1 is no test of the keeper's own rules. Q4, Q4b and Q5 pass D4 because a closed desk lands nothing whatever it answers to a release, a naming or a sync (`driver.rs:505`). X4 and Q10 pass and need no test: a lost keeper is never kept again, and a call taken before the close and not answered reads to the uploader as the desk closing.

### The scenarios

All pass on 1415db1; "records" means the scenario prints what happens and asserts only that it ends.

| | What it does | Result |
|---|---|---|
| L1 | Two nodes, two names, 400 random programs (question 1); asserts | 25 455 commits, 535 changes of hand, no assert fired |
| L2 | The same programs with one holder name for both nodes, 60 programs; records | 4709 commits, 2580 and 2129, 1140 changes of hand |
| L3 | One holder name on two live daemons, step by step; records | both hold epoch 1; each lands whole cuts and deltas onto the other's head |
| L4 | A's renewal reads its row; B takes the expired lease; A's take lands at a newer epoch; records | both lost, row `("node-a", 3, 0)`, for good (F5) |
| L5 | The lease row removed under a holding uploader; records | two uploads refused in 10 s, then lease lost at the renewal |
| B1 | One cut of 64 blocks over a store that fails every Nth PUT; records | 0, 811, 5311, 17 011 s (F3) |
| B2 | A cut found committed resets the wait; asserts | waits 1, 2, then 1 |
| B3 | Twenty minutes of failing steps; asserts the row never passes its expiry | at least 20 s left at every turn |
| C1 | Steps that take 0 to 25 s and fail; records the renewals | 10 to 33 s between renewals; 10 to 35 s at 5bebcc0 (F4) |
| C2 | An idle uploader whose check takes 5 to 35 s, always or only just before the renewal is due; records | up to a third of the lease plus the check (F4) |
| C3 | Lease calls slower than the renewal's interval, eight cases, ten minutes each; asserts the loop does not spin | 9 to 69 turns; 2418 for an idle uploader with a quick lease |
| C4 | A slow take that fails, and a slow renewal during a backoff; asserts the waits after them | 1 s and 6 s |
| D1 | The copies file's sync held 501 ms during an upload through the desk; records | the queue's thread waits in `Desk::answer` 505 ms (F1) |
| D2 | A run stopped at a read and run again; records | 1339 turns in 1.5 s, nothing uploaded (F2) |
| D3 | 20 000 calls with a queue that turns only when woken; asserts none is left 200 ms | none |
| D4 | The desk closed from a third thread after 0 to 139 answers, 300 runs; asserts rest, and nothing or the whole cut in object storage | 269 nothing, 31 the cut |

C2, C3 and C4 are new for 1415db1 and were not run on 5bebcc0, except through X0, X1 and X3.

### CI and Codex

- **CI on 1415db1** is green: lint, gate, test, unit-arm64, coverage, e2e-tier-a, s3-backends, chart-lint, both k3s smokes; the pull request reads as mergeable and clean. It was green on 5bebcc0 too. No rig ran, rightly: the diff is in `crates/skyzfs-upload` alone, which no rig's paths name, and the daemon does not call it.
- **Codex** left one finding on 5bebcc0, P2, on the waits counted from the turn's start. It was right. The author answered it on its thread with 1415db1, and Codex's review of 1415db1 has no finding. F4 is what remains of the matter, and it is not Codex's finding any more.

### What I could not verify

- Anything with the daemon: the real queue's thread and its wake (libublk's), the status, the options, the start refused for a held lease. That is M2.4c-2.
- A real object store and real clocks. The scenarios run on the in-memory store behind the fault store and on tokio's paused clock; two "nodes" are two fixtures in one process, turned one after the other, so a commit of one never overlaps a commit of the other in time. The conditional put that the fence rests on is `skynbd-refs`' own and was not under review; I read it and did not test it against MinIO.
- What F1 costs on a real disk. D1 holds the sync in the simulated disk; the figure is the disk's.
- F5's likelihood beyond the argument given.
- The plants in `lease.rs`, `queued.rs` and `backoff.rs` on 5bebcc0, beyond K1 to K4: the run there was cut short when the new head came, and those files are the same in both.

### Files

`review/m2.4c-1/` on `skyzfs-review`: `review.md` (this text), `plants.py`, `plants.out` and `plants_detail/` (each plant's test output, and the scenario's where one was run with it), `scenarios.rs`, `scenarios.py`, `scenarios.out`, `t_plain.out`, and from 5bebcc0 `scenarios_5bebcc0.out` and `t_plain_5bebcc0.out`.
