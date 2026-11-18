## Review of #122 at 70d0f23 (M2.5-1)

The uploader publishes a cut as `snap/t`: `Request`, `Publishing` and the
row `skyzfs/snap/<t>` in `crates/skyzfs-upload/src/publish.rs`, the
lookup and the publish transaction in `remote.rs`, their steps in
`driver.rs`, and a crash matrix in `tests/publish.rs`. One commit on
`ec8c91b` (`zfs-topology`). Nothing can ask for a publish yet: the daemon
gets its control channel in M2.5-2.

**Verdict: one finding blocks the merge in my judgement (F1): a publish
can name a commit that is not the cut's, and say it is done.** It needs
another writer to move head beside the uploader, which is the case the
uploader's `Mismatch::HeadMoved` exists for, and the fix is small. The
rest is sound as far as I can break it: the publish rule, the publish as
a transaction of its own, the lookup before and after every transaction,
and what a failure at any object-store operation leaves. Five more
findings do not block (F2 to F6, F6 being the one Codex also raised), one
needs the lead's decision before M2.5-2 or M2.6 (Q1), and the tests have
three gaps worth a line each (T1 to T3).

If the lead merges on the ground that nothing reaches this code before
M2.5-2, then F1 is a condition of M2.5-2 and should be written down as
one.

CI on the head when I last looked: 36 jobs green, none failed (unit on
amd64 and arm64, the control rig on three images, the replay rig's jobs
through `skyzfsd`, coverage, e2e). The daemon rig's three daemon jobs
were still running; I did not wait for them, and their tables are the
lead's to read. Codex reviewed 70d0f23 and left one finding, P2, which is
my F6.

### What I did

- Read design §5.2 (publish, atomic visibility), the M2.5 row of the
  implementation notes, and the diff against `origin/zfs-topology`
  before the description; then the description, against the code.
- Gates at the head with rustc 1.100.0: `cargo fmt --all --check` clean;
  `cargo clippy --workspace --all-targets -- -D warnings` clean plain,
  with `skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `skyzfs-ublk-sys/control-test-program`. Tests, 0 failed:
  `skyzfs-upload` 103 (38 in the library, 3 crash, 22 driver, 9 publish,
  7 queued, 24 uploader); `skyzfsd` 105 plain and 141 with `test-hooks`.
- 57 textual plants, one at a time, each against
  `cargo test -p skyzfs-upload` with a limit of 300 s (`plants.py`,
  `plants.out`): 49 caught, 8 survive, none failed to apply. A plant
  that survives is also run against my scenarios.
- 15 scenarios of my own, appended to `tests/publish.rs`
  (`scenarios.rs`, r1 to r10) and to `tests/uploader.rs`
  (`scenarios_up.rs`, u1 to u5) for a run; results in `scenarios.out`. A
  scenario states what should hold; those that fail at the head show a
  finding and are named with it.
- No kernel here, and the daemon does not ask for a publish yet, so
  nothing of mine or of CI drove a publish through `skyzfsd`.

The worktree of the head was clean after every run.

### The three questions

**(1) The publish rule. Sound, but for F2.**
`publish.rs`: `carried_by` 127-132, `committed` 505-515, `absent`
533-548, `found` 552-574.

- *What qualifies.* A cut of the request's cut state, counted after *C*,
  of group *G* or later, that opens as itself: the four conditions of
  design §5.2, and each is held by a test (plants A01 to A08, all
  caught: `>` for `>=` on the count, `>` for `>=` on the group, each
  condition dropped, and both places that say whether a cut opens as
  itself).
- *First known.* A publish asked late goes on the oldest cut in view
  that carries it (64 cuts, the newest), and one that waits takes the
  first cut committed after it that carries it. Plants E02, E03, E05,
  E06, E07 are caught. No later cut takes a publish from the first.
- *The first qualifying cut abandoned.* An abandoned cut is never
  committed, so it is never offered; the cut that takes its map is the
  next committed and takes the publish (the pull request's test, and r7
  for a cut the store let go of between its commit and the publish).
- *Across a restart.* What is pending is in memory only, so a new
  uploader publishes nothing unasked. Asked again, it looks the name up
  first: found with a row that records the request it answers done, and
  absent it goes on the first cut it comes to know, which may be a later
  cut than the first uploader would have taken. That is still a cut that
  carries the request, so it is within the design ("it is later, on the
  same history"). The matrix's restart half checks exactly this.
- *A name already published for a later cut.* Done, where the row names
  a cut that carries the request and the same manifest (`recorded_in`
  136-145); otherwise `Taken`. Two cuts that both qualify: the older.
- *Cuts with other numbers after a restart.* Numbers begin again with a
  cut state, and the rule compares the state first. That is where F2
  lies: until the uploader has checked its first cut it does not know
  the state and accepts a request of any.

**(2) The publish as its own transaction, after the cut's commit.
It matches the design; F1 and F3 are its two weak points.**
Design §5.2's diagram has "write commit, then swap ref snap/t", and
"Atomic visibility" asks for one compare-and-swap on the ref and that
the existing commit protocol and its crash matrix apply unchanged. A
publish inside the head swap would have changed that protocol and could
not serve a publish asked after the cut's commit, nor one found on a
later cut. So the change from the plan is toward the design's text, not
away from it. The M2.5 row's criterion "the ref is swapped only after
the commit it names is whole in object storage" holds by construction:
a cut is known to the publishes by its commit's hash, and that exists
only once the commit returned or was found (`driver.rs` 468, 662). A
transaction made with no lookup before it is caught (E04, G01).

- *Can a taken name block the uploads or loop for ever?* No. The lookup
  finds the other's ref, the publish ends `Refused(Taken)` and the cuts
  go on (the pull request's test). Taken between the lookup and the
  transaction: the transaction is refused once, the lookup after it
  finds the ref, the publish ends refused, nothing of ours is written
  beside their ref, and the next cut goes up (r9, passes).
- *A lost reply.* The transaction is never made again without a lookup
  (`failed` 600-605, stage `Recheck`): found with our row, it is done.
  A failed lookup after a lost reply is taken again (r6, passes).
- *Lease loss and fencing.* The transaction goes through
  `RefStore::transact`, so with `ActingRefStore` it is fenced as the
  head swap is. u2: another takes the lease, the publish is refused,
  nothing lands (no ref, no row), and the uploader is at rest with the
  lease lost. The two manifests are PUT before the fenced transaction
  and are not fenced: a fenced uploader leaves two objects that nothing
  names (N3). The publish stays pending in a stopped uploader (F4).
- *Idempotent?* Yes: the compare-and-swap expects no ref, and the same
  request asked again is the same pending publish, or done from the row.
- *A commit that is not durable?* No, as above.
- *The wrong head?* Yes, in one case: F1.
- *Can a publish block the uploads?* Not by a name taken. By a publish
  whose transaction cannot be made, yes: F3.

**(3) The crash matrix. Complete at the object store, for one shape of
run.**
The run of `a_failure_at_any_operation_leaves_no_ref_or_a_whole_one` has
27 object-store operations (r4 prints them by step):

| ops | step | what |
|---|---|---|
| 0 | `PublishLooked` | lookup of `snap/t` (`get_versioned refs`) |
| 1-8 | `Began` | check, open |
| 9-12 | `Read` | four segments |
| 13-22 | `Uploaded` | last segment, nodes, commit object, `get_versioned refs`, `put_if refs`: the head swap is op 22 |
| 23-24 | `Published` | PUT ZFS manifest, PUT SkyNBD's manifest |
| 25 | | `get_versioned refs` of the publish |
| 26 | | `put_if refs`: the publish |

The matrix fails each of the 27 before and after (the reply lost), and
goes on with the same uploader and with a new one: 108 cases. So every
kill point between the head swap and the publish that an object store
can tell apart is in it: after 22 (the swap landed unheard), before and
after 23, 24, 25, and before and after 26. It asserts for each case on
which cut the publish must end, not only that it ends. The points that
are not object-store operations fold onto those. A death between the
swap's reply and the store's release, on either side of `committed()`,
is "after 22" to a new uploader: the cut is still held, it is found
committed and takes the publish. A death after the release is "before
23": the publish goes on the next cut. A death after the publish's
reply and before the outcome is recorded is "after 26". A death inside
the store's release is the store's own matter and M2.4's tests.

Plants on what the matrix holds: G01 (transaction again without a
lookup), G02 (no lookup after a failure), G03, G04, G05, G07, G08, G09,
G10, G11, G12, G13 are caught by it, and so are H01 and H02 (the commit a
cut is known by is the one before it).

Not in the matrix, and what I found there:

- a publish on a cut that is not head (asked late, on an older known
  cut): r5 runs the 5 operations of such a publish both ways with both
  continuations, 20 cases, and all end with no ref or a whole one;
- two faults in a row: r6, passes;
- the store letting go of the cut between commit and publish
  (`Released::Gone`): r7, passes;
- head moved by another between the swap and the read after it: r1 and
  r2, **fail: F1**;
- anything through `Uploader`, or with a fenced ref store: T2.

### Findings

Most severe first. "Certainty" is how sure I am that the code does what
I say, not how likely the case is.

**F1. A publish names head as read, which need not be the cut's commit
(defect; blocks in my judgement; certainty high, reproduced two ways).**
`driver.rs` 464-468 (the check finds the row naming the cut) and 617-619
(the read after a failed commit):

```rust
// The row names the cut, so head is its commit: the row
// moves only with head.
self.committed(&cut, seen.head);
```

The row moves only with head when the uploader moves them. Another
writer moves head without the row: an operator's write through SkyNBD,
or `Engine::rollback`, which swaps head to a snapshot's commit and
leaves the rows alone; with this package a SkyZFS volume gets snapshots
to roll back to. The uploader knows this case: `judge` gives
`Mismatch::HeadMoved` for it when a cut is to be uploaded, and
`Fixture::move_head` exists to test it. But `judge`'s first rule, "the
row names the cut: committed", does not look at head, and until this
pull request it did not need to, since nothing was done with head. Now
head as read becomes the snapshot's commit.

- r1, same uploader: `pool@t` asked, cut 1 (group 7) is committed and
  the reply to its swap is lost; head is moved by another; the uploader
  reads the row and head again, finds the row naming cut 1, and steps
  on. Result: `Uploaded{1}`, `Published{name: "t", number: 1}`, progress
  `Done`; `snap/t` names the other writer's commit (group none), its
  row says cut 1 of group 7, and the commit's content is not the volume
  at cut 1's instant.
- r2, new uploader: the same with the daemon dead after the lost reply,
  head moved while it is down, and the request asked again of the next
  uploader: `PublishLooked`, `FoundCommitted{1}`, `Published`, the same
  wrong ref.

So node B lists `snap/t`, with a row and two manifests that vouch for
it, and imports something that is not the pool at the cut, or not a
pool. Nothing reports it at the publish; the check of the next cut,
reading the same head, would give `HeadMoved` (`check.rs` 89-100), and
by then the ref is there.

Suggested: hand a cut to the publishes only where head is known to be
its commit. The least change is the test `judge` already makes for an
upload, head's group against the cut's (with the group of head as read:
in `observe`'s `current: false` branch `head_group` is the engine's, not
that of the head just read, `remote.rs` 194-201), and otherwise treat it
as `HeadMoved`, or at least do not offer the cut. A group can repeat
after a rewind import, so the whole answer is a cut's row that names its
commit; the hash is known before the swap. And a line in the matrix or
beside `move_head`'s test.

**F2. A request of another cut state asked before the first check is
accepted and pends for ever (defect; does not block; certainty high).**
`publish.rs` 446: the state is compared only once known, and it becomes
known at the first check of an uploadable cut (`driver.rs` 457), not
when the uploader starts. r3: a request with a state that is not the
store's, asked of a new uploader before any cut: `Accepted`. Five cuts
later its progress is still `Pending`; the same request asked again is
`Refused(State)` while it pends; the name asked with the right state is
`Refused(Asked)`. Nothing ends it but the uploader's end, and it holds
one of the 64 places. The unit test at `publish/tests.rs` 120-123 states
the acceptance on purpose ("one whose cut state is not known yet takes a
request of any state"), and plant C03 (the driver never tells the state)
survives the pull request's tests.

How an agent comes to hold a stale state within one attachment I did not
establish; it is what the field and its refusal are for, so I take it
that there is such a case. The store knows its state before any cut
(`cut_state_id()`): set it where the driver is made, and when a state
becomes known, refuse what pends for another.

**F3. A publish that cannot be made stops every upload, and is tried
again without a growing wait (defect or decision; does not block;
certainty high for u3, u4 is a model).**
`driver.rs` 368: a publish with work is stepped before the cut, always.

- u3, through `Uploader` on a fenced store: every PUT under
  `skyzfs/manifest/` fails (a bucket policy that names the prefixes it
  allows would do it: the key is new with this pull request), the rest
  works. In 600 s: 600 failed publishes, the longest wait 1 s, and cut 2
  never goes up. The wait does not grow because each failed transaction
  is followed by a lookup that succeeds, and any step that succeeds
  resets the backoff (`uploader.rs` 257-259). With reads that work and
  writes that do not, the pair lookup, transaction repeats once a second
  for as long as the daemon lives.
- u4, with a remote of mine whose publish answers `Conflict` and whose
  lookup finds nothing, while a cut is at its commit: 1000 publishes in
  2000 turns with no wait at all. `uploader.rs` 307 takes a `Conflict`
  while `doing == Committing` for the commit's own, and the comment
  above it promises that "no conflict makes the uploader turn twice
  without a wait". A publish is now a second source of `Conflict` at
  that stage. On a real store I know one way to it, a fenced transaction
  with no lease row (`LeaseLost` with no holder reads as `Conflict`,
  `skynbd-engine/src/lib.rs` 64-67), and I did not reproduce it there: a
  renewal probably ends it. So: low, but the arm should not match a
  publish's step.

For the lead: whether a publish may hold the uploads at all. A publish
that fails N times could step aside for the cut and be tried after it;
it loses nothing, since it would go on the same known cut.

**F4. A publish asked of an uploader that will not step again pends for
ever, and the asker is not told (gap; low; certainty high).**
`Uploader::publishing()` (`uploader.rs` 171-176) accepts whatever state
the uploader is in. u2: after the lease is lost for good, a pending
publish stays `Pending`, the same request asked again is `Accepted`, and
so is a new name. u5: after the store kept its cut (`Step::Kept`) the
uploader is `Stopped` and `turn()` returns at 241-244 without stepping,
though the cut is committed and a step of the driver alone would publish
on it. Before the lease is held the first time a publish waits and is
made once it is (u1, passes), which is right. M2.5-2 has to tell an
agent "this uploader publishes nothing"; today the type gives it no way
to.

**F5. A `Done` is answered from memory with no lookup (low; certainty
high).**
`publish.rs` 459-468. r10: `snap/t` is published, then deleted in object
storage (retention is ref deletion, design §5.2), and the pool takes a
new `pool@t`. Asked of the same uploader: `Refused(Taken)`, though the
name is free; and the first request asked again answers `Done` with a
commit no ref names. It lasts until 1024 other outcomes pushed it out or
the daemon ends. A refused outcome is looked up anew (470-473); a done
one could be too, at the cost of one read.

**F6. The row does not name its commit, so a ref made anew beside a row
left behind reads as our publish (low; certainty high; Codex's P2).**
`SnapRow` v1 holds the cut, the snapshot id and the manifest's length
and hash; `recorded_in` (136-145) takes the commit and does not check
it. `Engine::snapshot_delete` removes the ref and leaves
`skyzfs/snap/<t>`. r8: `snap/t` published on cut 1, deleted, and made
again by another on cut 2's commit; a new uploader asked the first
request again answers `Published{found: true, number: 1}` for a ref on
cut 2's commit. Put the commit's hash in the row and compare it in
`found`; it costs 32 bytes and the row is not yet in any store, so v1
can still change without a v2. Related: no one deletes a row, so the volume's
refs object, which every head swap reads and rewrites, grows by 86 bytes
and a key for every snapshot ever published.

### For the lead

**Q1. A pending publish cannot be withdrawn or replaced.** A name that
pends is `Refused(Asked)` for any other request, and there is no
request to take one back. The design has the agent hold the snapshot
until the ref is swapped; an agent that gives up (its timeout, the
snapshot destroyed by an operator with the hold released) leaves a
publish that a later cut will carry out: `snap/t` then names a cut
without the snapshot, which is what the rule exists to prevent. It needs
either a withdrawal, or the rule that an agent never releases the hold
while its publish pends in a live daemon. To decide before M2.5-2 fixes
the requests.

### Notes

- N1. A publish asked late goes on a known cut up to 64 cuts back, and
  so names a commit that no ref has reached since head moved on.
  SkyNBD's collector takes the refs as its roots
  (`skynbd-engine/src/lib.rs` 1183-1205), so a collection between the
  next cut's commit and the publish could sweep what only that commit
  reaches, and the ref would name a commit that is not whole. Nothing
  collects a SkyZFS volume today; it belongs on the list of whoever
  brings collection to one.
- N2. The description's "No emap is written ... it is the head's commit"
  is true of a publish on head only.
- N3. A publish that fails after its PUTs leaves the ZFS manifest and
  SkyNBD's manifest behind, and a new uploader makes a new snapshot id.
  SkyNBD's collector sweeps `snap/`; nothing sweeps `skyzfs/manifest/`.
  "A crash before it leaves no trace" (design §5.2) holds for what
  another node sees, not for the bucket.
- N4. `Publishing::set_attachment` on an uploader with publishes pending
  keeps them: they would be carried out on the new attachment. Nothing
  calls it twice; say so in its comment or drop what pends.
- N5. A name is `[A-Za-z0-9._-]`, as SkyNBD's. ZFS allows `:` in a
  snapshot's name, and the common snapshot tools put a time with colons
  there. For M2.6: how the agent names such a snapshot.
- N6. `Refusal::Taken` does not say what took the name; an agent cannot
  tell "another manifest under my name" from "an operator's snapshot".
  One more case reads as `Taken`, by reading and not by a run: a
  publish that landed before a reboot, asked again after it with the new
  cut state. The row has this manifest and a cut of the old state, and
  `recorded_in` wants a cut of the request's state. That is the careful
  answer, since cuts of two states cannot be compared, but the agent of
  M2.6 then needs its own way to see that the snapshot is published.
- N7. The M2.5 row's "listed through `skynbd-api` and `skynbdctl` as any
  ref is" has no test here: the tests read `snap/t` from the ref store
  and check SkyNBD's manifest, and do not list through
  `Engine::snapshot_list`. It lists refs by prefix, so I expect it to
  hold.
- N8. After a daemon's death between a cut's release and its publish,
  the publish asked again waits for the next cut that carries it (the
  lead's Q2, and the matrix says so). An idle pool writes no uberblock,
  so that cut comes only if something makes one: the agent has to sync
  the pool while its publish pends (M2.6). The matrix always writes a
  cut 2 there, so it does not show the wait.

### The tests

57 plants: 49 are caught by the pull request's own tests, 8 survive.
The description's claim that the tests hold the behaviour is true of the
rule, the lookup and the matrix; it is not true of three things:

- **T1. No test has the publish's transaction itself refused.** G06
  survives: a transaction refused because the name exists reads as
  published, so the uploader would answer `Done` for another's ref. In
  `a_name_taken_is_refused_and_the_cuts_go_on` the lookup finds the name
  first, so the transaction is never refused. r9 (taken between the
  lookup and the transaction) catches it; it or its like belongs in
  `tests/publish.rs`.
- **T2. No test asks for a publish through `Uploader`, or with a fenced
  ref store.** K01 and K02 survive: with either, a publish asked of the
  uploader is dropped or never made, and every test passes. The diff of
  `tests/uploader.rs` is the two new methods on its fakes, and
  `tests/publish.rs` drives a `Driver` on a plain `ObjectRefStore`. u1
  and u2 catch both plants; u2 is also the only run in which the fence
  refuses a publish. This is the path M2.5-2 will call.
- **T3. A row with another manifest of the same length.** B02 survives:
  `recorded_in` without the hash, with the length kept. The test's other
  manifest differs in length. One line.

The other survivors:

- C03, the driver never tells the publishes the cut state: nothing
  fails. That is F2 seen from the other side: no test has a request
  refused for its state through a driver.
- J05, the count of requests refused once looked up: not asserted.
- E09, the newest publish with work is stepped before the oldest: no
  test depends on the order, and nothing else does.
- E08, the stage after a failed transaction's lookup: no difference can
  be seen, since no cut is committed while a publish has work, so the
  first known cut that carries the publish is the cut it was on.

The matrix itself I could not fault for what it covers: it asserts the
cut each case must end on, a ref with its row and both manifests whole
or no ref and no row, and the content of the snapshot against the volume
at the cut's instant (`published()`). r1 and r2 show F1 with a check of the
same kind.

### Other checks

- No `unwrap()` or `expect()` in the diff outside tests.
- Versioned formats: `SnapRow` is version 1, 86 bytes, with its layout
  in the module's comment and tests for a wrong version and a wrong
  length (plants I01 and I02 are caught). The ZFS manifest is stored as
  given (the agent's format, design §5.4). The cut's row is unchanged.
- Status and logs: unchanged. `PublishFigures` and `Publishing::pending`
  are there for M2.5-2 and nothing shows them yet, so the judges in
  `tests/kernel/` and `skz_time.py` have nothing new to read. No code in
  `bins/` matches on `Step`, so the three new steps break no consumer.
- `bins/skyzfsd/src/kill.rs` passes `snapshot` and `publish` through and
  counts swaps at the commit only, so `--kill-at` means what it meant.
- `docs/` is untouched, as it should be; the notes' M2.5 row will need
  the change of plan (the publish is its own transaction) and, if
  accepted, F3's and Q1's answers.
