## Review of #117 at 5bebcc0 (M2.4c-1)

**Verdict: can merge. Nothing blocks.** I could not make two nodes commit for one volume, the backoff never hot-loops or starves the lease inside `turn`, and the desk loses no wake and answers no call wrongly. I found one defect that matters for the daemon and not for this merge: the desk waits for the copies file's sync on the queue's thread (F1). It costs latency, not data, nothing calls the desk yet, and its fix changes what `Desk::answer` promises, so it should be settled (fixed, or accepted by the lead as the read on the queue's thread was) before M2.4c-2 puts the desk on the daemon's queue. Besides it: a hazard in `Uploader::run` (F2), a reading of the backoff that is right for refusals and too wide for plain errors (F3), Codex's finding confirmed with figures (F4), a rare case in which both nodes lose the lease (F5), @@NGAPS@@ gaps in the tests, and notes for M2.4c-2.

I read the diff against 4e4e076, the lead's decisions in the notes (M2.4's shape, G7) and `skynbd-refs` before the description. Then:

- **The head's own tests** pass here, rustc 1.100.0: 21 unit tests, 3 crash, 22 driver, 2 queued, 7 uploader (`t_plain.out`).
- **@@NPLANTS@@ plants** of my own, one at a time, against `cargo test -p skyzfs-upload --lib --tests`, some also with a scenario in place (`plants.py`, `plants.out`): @@KILLED@@ killed, @@SURVIVED@@ pass every test of the head. The table is below.
- **Thirteen scenarios** the head's tests do not have (`scenarios.rs`, run by `scenarios.py`; results in `scenarios.out`). They use the head's own fixture, the in-place store on the simulated disk, the real engine and the real refs store over the fault store, with a clock for each node's lease apart from tokio's.

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
| What `Unsure` allows | The uploads go on under the epoch last held: checks, reads, PUTs of segments and commits. There is no bound in time and no clock in it; the renewal is tried again every third of the lease's time. What bounds it is the fence at the next commit, or the next renewal that reaches the store. Another node gains nothing from our being unsure: it can take the lease once the row has expired by its own clock, exactly as if we held it, and from then our commit is refused | the head's two outage tests; L1 |
| Both nodes uploading | Possible, and harmless to head: a node that is unsure, or lost and not yet told, PUTs segments until its commit is refused. They are orphans for M4's collection (N3 of #114's review) | L1; L5 (20 480 bytes of segments become 61 440 with nothing landed) |
| Clock jumps | The expiry is written and judged by the wall clock of whoever calls the refs store; the renewals are scheduled on the monotonic clock. A clock that another node sees as behind lets that node take a live lease at its start; the loser is told at its next renewal or commit. It costs the loser its uploads until its daemon starts again (lost is final), and no commit | L1: in 7 programs the lease changed hands once more when the clocks were set equal, and never twice |
| A renewal delayed past the expiry | Nobody took it: the take keeps the epoch (the head's test). Somebody took it: lost (the head's test) | head |
| A take at the same epoch | A daemon that starts again under its name gets its predecessor's epoch. Right for one daemon per name; see N1 | head; L3 |
| The row deleted | The renewal finds no row and the lease is lost, named as nobody at epoch 0, for good. Until that renewal the commits are refused as conflicts (G7): L5, under question 2 | L5 |
| The row another holder's, at an older or equal epoch | Lost: the renewal compares the holder too (`lease.rs:183`), and so does the fence. An equal epoch under another name can exist after a row was deleted and taken anew | K2 @@K2@@ |
| The row ours at another epoch | Lost, naming ourselves | head; K1 killed |
| A take that comes back at a newer epoch | Lost and given back. Safe; but the node that held it in between loses it too: F5 | L4 |

Against the decisions in the notes: "a lease lost while serving stops the upload and not the serving" is as built (the uploader only ever stops itself), and "a start that cannot reach the object store serves and uploads when it can" is the `NoLease` state with its own backoff. "A start that finds the lease held by another holder is refused" is the daemon's; note that here the first take is made at the uploader's first turn, so after a start that could not reach the store, another holder is found while serving and reads as a lease lost.

**(2) The reading of "reset on a success" keeps the rule's point. Inside `turn` I found no hot loop and no starved lease. For plain errors the reading waits too long (F3).**

- **What counts as a success** is `Uploaded`, `UploadedNotHeld` and `FoundCommitted` (`uploader.rs:218` to `225`): head's row names a cut of ours. That is the right set. `FoundCommitted` belongs in it: B2 has two steps fail (waits 1 s, 2 s), then the check finds the cut committed, and the next failure waits 1 s; with P2 planted (found committed is no success) B2 fails and @@P2@@. `Kept` resets nothing and stops the driver, so it does not matter. A cut lost or a mismatch resets nothing, which is right: nothing went up.
- **A refused cycle waits twice.** The waits of a store that refuses every commit are 1 and 2 s, then 4 and 8, 16 and 32, 64 and 128, 256 and 300: the cap is reached in the fifth cycle, and from then a whole cut goes up every ten minutes where the rule's letter ("doubling from 1 s to 5 min") gives five. Both meet the rule's purpose. I would wait once, at the give-up: the first wait sits between the refused commit and one read of the row and head, and buys nothing the second does not. The lead's call, and the test pins whichever is chosen.
- **Hot loops.** Every path of `turn` that returns a wait of zero has moved the driver on: a check that found a cut, a slice, a sync, a commit, a cut lost (after which the store offers nothing until the pool makes one). Idle waits 250 ms, an error and a give-up wait, lost and stopped rest 60 s. P4, P4b, P7 and P13 remove one wait each: @@WAITS@@. The one hot loop I found is outside `turn`: F2.
- **Starving.** A turn keeps the lease before it steps (`uploader.rs:166`), and a wait ends early for the lease (`next_wait`). B3: twenty minutes of failing steps, waits up to 300 s, and the row never has less than 20 s of its 30 s left when a turn begins; with P8 planted (the wait does not end for the lease) B3 fails. What is left is the time of one step: F4.
- **Waiting too long:** F3.
- **G7 now.** L5 removes the lease row under an uploader that holds the lease, and the pool makes a cut. The cut goes up, the commit is refused as a conflict (wait 1 s), the row and head are read again and the commit is given up (2 s); it goes up again at 3 s, is refused (4 s) and given up (8 s); at 10 s the renewal finds no row and the state is `LeaseLost { holder: "", epoch: 0 }`. Two uploads, nothing landed. So today the backoff slows it and the lease stops it, within a third of the lease's time. A store that never had a row cannot be met any more: the first take creates it.

**(3) The desk is sound: no deadlock, no lost wake, no call answered wrongly, at any moment of a close. But the queue's thread does wait, for a sync (F1).**

- **Locks.** One mutex, held only to move a call in or out (`queued.rs:127`, `131`, `141`, `161`). The store's call, the wake and the reply are all outside it. The queue's thread never awaits anything of the uploader's: `reply.send` does not block.
- **Wakes.** `ask` leaves the call under the lock and wakes after (`queued.rs:166`, `168`), so a queue that looked at an empty slot and went to sleep is woken by a wake that comes after the call is there. D3 is a queue that turns only when woken and never looks on its own: 20 000 calls, none unanswered. The head's own test cannot show this, since its queue looks every 5 ms whether woken or not: @@Q67@@ (G-wake).
- **A call answered wrongly.** Each call carries its own reply, so an answer cannot reach another call. A call whose asker went away (a cancelled `turn`) is replaced by the next call or answered to nobody.
- **Close.** A call the queue took before the close is answered; a call waiting is dropped and its asker gets the empty answer; later calls get it at once. D4 closes the desk from a third thread after 0 to 139 answers of an upload of about 131 calls, 300 times: the uploader always came to rest, and object storage held nothing new in 268 runs and the cut, whole and readable, in 32. A closed desk can land nothing by itself: the driver asks the store for the cut once more before it commits (`driver.rs:505`), and a closed desk has none. After a commit that landed, a closed release reads as "gone", the naming answers false, and the next instance finds the row naming the cut (rule 1 of the check). Q4, Q4b and Q5 change one closed answer each: @@Q45@@.
- **Threads and drop order.** The crate owns no thread and has no `Drop`; the desk lives in an `Arc`. Two things follow for M2.4c-2 (N4): the close must run on every way out of the queue's thread, and the wake can be called once after the close.

### Defects (none blocks this merge)

**F1. The desk waits for the copies file's sync on the queue's thread.** `queued.rs:84` answers `Call::SyncCopies` with `Store::sync_copies`, which begins the sync on the file's helper and then waits for its end (`local.rs:170` to `174`: `cut_copies_synced(true)`, down to `helper.results.recv()` in `skyzfs-local/src/fs.rs:665`). `Desk::answer` is called on the queue's thread, so every request of the pool waits for one `fdatasync` of the copies file, once for each uploaded cut that read a copy. The store has the two halves apart for this reason: `sync_cut_copies` is "on the file's helper, as the image's syncs are, so that the queue goes on meanwhile" (`inplace.rs:3042`), and the helper wakes the store's caller when the sync ends.

Scenario D1: a cut that read a copy, with the copies file's sync held for 501 ms as a slow disk holds it. The queue's thread is inside `Desk::answer` for all of it (longest single answer 506 ms), with one caller waiting at the end of the held sync and no turn of the queue's thread meanwhile. Certain.

How much it costs is the disk's sync time: the notes have 52 and 53 ms for a sync of the home image on the slow-sync machines of M1.3m's rounds, which is why that sync left the worker. It is paid only by a cut that read a copy, so while an upload lags behind the pool's writes, which is when the pool is busiest.

It is not this merge's to block: nothing calls the desk, and no data is at stake. But the description's sentence "a turn never waits on the uploader" reads as more than the code gives, and the fix is in the desk's shape (begin the sync at `answer`, keep the reply, and send it from a later turn once `cut_copies_synced(false)` has an outcome; `Store` then needs the two halves). So: fix it in M2.4c-2 before the desk is wired, or the lead accepts it and the notes say so, as they do for the read (the shape decided on 2026-11-15 names the read on the queue's thread, and not the sync).

**F2. An uploader whose `run` was stopped spins if it is turned again.** `run` (`uploader.rs:275`) drops `turn()` when `stop` resolves. `Driver::step` is not made to be dropped at an await: it has put `Stage::Check` in place of its stage (`driver.rs:313`), and `read` has moved the slice buffer out (`driver.rs:432`), so the driver is left checking with a buffer of zero blocks. The store refuses a read into such a buffer without abandoning the cut, so each cycle is: check (a read of head and the row in object storage), read refused, the cut counted lost, wait zero.

Scenario D2: a run stopped while a read waits at the desk, then run again for 1.5 s: 1313 turns, 656 cuts counted lost, 1316 checks answered, nothing uploaded, the cut still held. Certain. Nothing in this pull request turns an uploader after a stop, so it is a hazard for the caller and not a defect that runs today. Either `run` takes the uploader by value, or its doc says that an uploader is not turned again once `run` has returned. `run` has no test in the head (R1 to R3: @@R@@).

**F3. One backoff for refusals and for plain errors, reset only at a commit: an upload over a store that fails now and then waits minutes for each failure.** `uploader.rs:234` enters the same `Backoff` for any error as for a give-up, and only a cut committed resets it. A failed PUT of a slice loses nothing (the driver stays at its stage and goes on from there), and hundreds of steps succeed between two failures, yet every failure of one cut's upload waits twice as long as the one before, and from the tenth on 300 s.

Scenario B1, one cut of 64 blocks read a block a slice (512 calls that PUT sealed segments), tokio's time: no failure, uploaded at 0 s; every 50th call fails (10 failures), 811 s; every 20th (25 failures), 5311 s; every 8th (64), 17 011 s. Certain. With P3c planted (any step that goes on resets the wait) B1 gives @@P3C@@, and the head's conflict test fails, as the description says of its plant 4.

The lead's rule speaks of a conflict that is given up. For that the author's reading is the right one. For an error the rule says nothing, and the purpose (do not send the whole cut again every cycle) does not apply, since an error does not start the cut over. A whole first upload of a large volume is where this bites: tens of thousands of PUTs, and each rare failure after the ninth costs five minutes. I suggest two counts, or one count with two resets: a refusal's wait is reset by a commit, an error's by any step that succeeds. It needs the lead's word because it reads the rule; it does not block.

**F4. Codex's P2 is right: a wait is counted from the turn's start, so the time of a step is added to the time between renewals.** `wait_after` and `next_wait` use the `now` read before the step (`uploader.rs:165`, `254`, `262`). Scenario C1, a store that answers a read of head after a time and with an error, lease 30 s, renewal every 10 s: steps of 0, 5, 15, 19, 21 and 25 s give at most 10, 15, 25, 29, 31 and 35 s between two renewals. So a step of more than about 20 s lets the row pass its expiry, where a clock read after the step would allow about 30 s. Certain, and small: an expired row matters only if another daemon starts on the volume just then, and then the fence decides. One line (`Instant::now()` after the step for the waits). Not blocking.

**F5. A renewal's take that comes back at a newer epoch costs both nodes the lease.** `lease.rs:204` to `217`: the read and the take are two calls, and when another holder took the lease between them and that lease has expired too by our clock, our take steals it at a newer epoch; the keeper gives it back and is lost, naming nobody. Scenario L4, with a real keeper for the other node B: A is lost at `("", 3)`, the row is `("node-a", 3, 0)`, and B, alive and with nothing wrong, is lost at its next renewal as "lost to node-a at epoch 3". Ten lease times later both are still lost and nobody uploads until a daemon starts again. With K4 planted (no give-back) @@K4@@.

It is safe (nobody commits), and rare: B must take between A's read and A's take and then either not renew for longer than the lease or sit behind A's clock by more than the lease. A renewal that is one conditional write ("renew if the row is mine at this epoch", inside the refs store's own read and put) would close it and need no read first; `skynbd-refs` has no such call today. A note for the lead more than a defect of this pull request. The status line it leaves on B is misleading: it names A, which holds nothing.

### Gaps in the tests (not blocking)

@@GAPS@@

### Notes for M2.4c-2 and the lead

**N1. A holder name must mean one daemon on one node.** See question 1, L2 and L3. A restart under the same name is renewed at the same epoch, by the refs store's rule that a holder never fences itself, so the epoch tells nothing about two processes of one name. If the daemon's holder is the node's name, two nodes configured with one name, or a volume's files copied to a second node of the same name, both commit. Whatever M2.4c-2 uses as the holder should be unique per node at least, and the notes should say that the lease does not guard against a name used twice.

**N2. The lease's epoch reaches the commit only through `make`.** `EngineConfig.epoch` ("stamped into every commit") is not set from it in the tests' `make`, so the commits of this pull request's tests carry epoch 0. If the commit's epoch is meant to be the lease's, `make` in the daemon sets both.

**N3. `Keeper::release` and then another turn takes the lease again**, at the same epoch if nobody took it, or at a newer one, in which case the next commit is fenced and the row stays ours and live for a lease's time. The daemon should release after its last turn.

**N4. The desk's close and wake.** If the queue's thread ends without `close` (a panic), a call waiting is never answered and never dropped: `run` still returns at its stop, a bare `turn().await` does not. And `ask` can call the wake once after the desk has closed (it checks, leaves the call, the desk closes, it wakes), so the daemon's wake must be harmless once the queue has ended.

**N5. Figures after a close.** In D4's 268 runs that landed nothing, 253 count the cut as lost and 15 as committed and gone from the store. Neither happened; the daemon was stopping. If the status is read after a stop it should not show these.

**N6. Smaller things.** An idle uploader asks the queue's thread four times a second for a cut that comes once a transaction group; a wake from the store when a cut is ready would do instead, if the wakes show in M2.T4. A stopped or lost uploader rests 60 s between turns whatever the lease's time, so a stopped one (a cut kept) lets its lease lapse and takes it again each minute; harmless, but it holds the lease while it uploads nothing. A lost uploader keeps its driver and engine in memory. `ask`'s comment "the slot is empty" does not hold after a cancelled `turn`; the code is right all the same.

### The plants

@@PLANTS@@

### The scenarios

All pass on the head; "records" means the scenario prints what happens and asserts only that it ends.

| | What it does | Result on the head |
|---|---|---|
| L1 | Two nodes, two names, 400 random programs (question 1) | asserts hold; 25 455 commits, 535 changes of hand |
| L2 | The same programs with one holder name for both nodes, 60 programs; records | 4709 commits, 2580 and 2129, 1140 changes of hand |
| L3 | One holder name on two live daemons, step by step; records | both hold epoch 1; each lands whole cuts and deltas onto the other's head |
| L4 | A's renewal reads its row; B takes the expired lease; A's take lands at a newer epoch; records | both lost, row `("node-a", 3, 0)`, for good (F5) |
| L5 | The lease row removed under a holding uploader; records | two uploads refused in 10 s, then lease lost at the renewal |
| B1 | One cut of 64 blocks over a store that fails every Nth PUT; records | 0, 811, 5311, 17 011 s (F3) |
| B2 | A cut found committed resets the wait; asserts | waits 1, 2, then 1 |
| B3 | Twenty minutes of failing steps; asserts the row never passes its expiry | at least 20 s left at every turn |
| C1 | Steps that take 0 to 25 s and fail; records the renewals | 10 to 35 s between renewals (F4) |
| D1 | The copies file's sync held 501 ms during an upload through the desk; records | the queue's thread waits in `Desk::answer` 506 ms (F1) |
| D2 | A run stopped at a read and run again; records | 1313 turns in 1.5 s, nothing uploaded (F2) |
| D3 | 20 000 calls with a queue that turns only when woken; asserts none is left 200 ms | none |
| D4 | The desk closed from a third thread after 0 to 139 answers, 300 runs; asserts rest, and nothing or the whole cut in object storage | 268 nothing, 32 the cut |

### CI and Codex

- **CI on 5bebcc0** is green: lint (2), gate, test (2), unit-arm64 (2), coverage, e2e-tier-a, s3-backends, chart-lint, both k3s smokes; the pull request reads as mergeable and clean. No rig ran, rightly: the diff is in `crates/skyzfs-upload` alone, which no rig's paths name, and the daemon does not call it.
- **Codex** left one finding, P2, on `uploader.rs:262`, not yet answered on its thread: F4 above. It is right. Its security review found nothing.

### What I could not verify

- Anything with the daemon: the real queue's thread and its wake (libublk's), the status, the options, the start refused for a held lease. That is M2.4c-2.
- A real object store and real clocks. The scenarios run on the in-memory store behind the fault store and on tokio's paused clock; two "nodes" are two fixtures in one process, turned one after the other, so a commit of one never overlaps a commit of the other in time. The conditional put that the fence rests on is `skynbd-refs`' own and was not under review; I read it and did not test it against MinIO.
- What F1 costs on a real disk. D1 holds the sync in the simulated disk; the figure is the disk's.
- F5's likelihood beyond the argument given.

### Files

`review/m2.4c-1/` on `skyzfs-review`: `review.md` (this text), `plants.py`, `plants.out` and `plants_detail/` (each plant's test output), `scenarios.rs`, `scenarios.py`, `scenarios.out`, `t_plain.out`.
