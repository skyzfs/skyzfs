## Next look at #122 at 9e4d4e8 (M2.5-1)

The head has not moved since the answer to my first look
([answer](https://github.com/skyzfs/skyzfs/pull/122#issuecomment-6983723374)):
`9e4d4e8`, three commits on `70d0f23` (`0609732` the answer, `feba8a7`
the deadline and the withdrawal, `9e4d4e8` a daemon test's figures).
`70d0f23..9e4d4e8` touches ten files: `skyzfs-upload` (`driver.rs`,
`publish.rs` and its unit tests, `remote.rs`, `uploader.rs`,
`tests/publish.rs`, `tests/uploader.rs`), `skynbd-engine`
(`Engine::commit_group` and its test) and two lines of
`bins/skyzfsd/src/upload/tests.rs`. Nothing else changed.

**Verdict: nothing blocks the merge.** F1 to F6 and T1 to T3 are
answered, and each answer holds against the scenario that showed the
finding, rerun on the new API. The deadline and the withdrawal (Q1, as
the lead decided it) hold against every race I could set up: a
transaction landed with its reply lost, a lookup failing across the
deadline, a withdrawal after done, while waiting, with the cut known,
after a failed transaction, after a restart, and the fence.

What is left, none of it blocking:

- **F1's remainder is real** (n1), and so is the same blindness in the
  check, which is M2.4's and not this pull request's (n2). Low
  likelihood, and when it happens the snapshot is silently wrong. The
  fix the lead has already scheduled (the cut's row names its commit)
  closes both. Details below.
- **Two new findings, low, in the model only:** a publish that cannot
  be made holds every later publish behind it (D2: u6, u7), and a
  publish withdrawn while its failed transaction waits for its lookup,
  then asked again, is answered `Accepted` and still ends `Withdrawn`
  (D1: q9).
- **Notes for M2.5-2 and M2.6** on the deadline's clock (q11) and on
  `Stopped` with a landed transaction (q10).

The tests hold what the answer says they hold. 93 plants, each alone
against `skyzfs-upload`'s tests: 88 caught, 5 survive, none failed to
apply. The author's 32 plants, rebuilt from the answer's table, are all
caught, F4a among them; so are 54 of my first look's 56 that still
apply: of the 8 that survived then, 6 now fail a test, and E08 and E09
still survive.
Of the 5 survivors, two show nothing (E08, N04), one is D2's order
(E09), and two are small gaps in the tests of the publishes' wait (N07,
N09).

CI on `9e4d4e8`: 44 checks, 40 green and 4 skipped, none failed; every
workflow run green, among them the daemon rig, the replay rig and the
cut daemon's jobs. Codex has no finding on `9e4d4e8`, and its P2 thread
(F6) is resolved. The base has moved since (`tests/kernel/`, a
workflow, docs) and the pull request still merges cleanly.

### What I did

- Read the answer, the diff `70d0f23..9e4d4e8`, the description as it
  now stands, design §5.2 and the M2.5 row of the implementation notes.
- Gates at `9e4d4e8` with rustc 1.100.0 (current stable):
  `cargo fmt --all --check` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean plain, with
  `skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `skyzfs-ublk-sys/control-test-program`. Tests, 0 failed:
  `skyzfs-upload` 123; `skynbd-engine` 96 (4 ignored);
  `skyzfsd` 105 plain and 141 with `test-hooks`. I did not run the
  whole workspace's tests here: the container's disk does not hold
  their build beside the other sessions' builds. CI's `unit` and
  `unit-arm64` ran them green on this head.
- 93 textual plants, one at a time, each against
  `cargo test -p skyzfs-upload --no-fail-fast` with a limit of 300 s;
  every anchor checked to occur exactly once before the run. A plant
  that survives is also run against my scenarios. My first look's
  plants keep their ids, re-anchored on the head (B05 has no text left:
  `Answer::Done` is gone); P.. are the author's, rebuilt from the
  answer's descriptions; N01 to N11 are new, on the new code.
- 34 scenarios of my own, appended for a run to `tests/publish.rs`
  (r1 to r10 and r3b, the first look's on the new API; n1 and n2, F1's
  remainder; q1 to q11, the deadline and the withdrawal; f1 to f3, the
  row's layout) and to `tests/uploader.rs` (u1 to u7). 29 pass and 5
  fail at the head (n1, n2, q9, u6, u7). Two that pass, f3 and q11,
  assert only their set-up and print what they find. Table at the
  end.
- As before, no kernel here and nothing in the daemon asks for a
  publish yet, so no run of mine or of CI drives a publish through
  `skyzfsd`.

### The answers

**F1, a publish names head as read: fixed for what it claims.**
`head_of` (`driver.rs` 752) hands a cut found committed to the
publishes only where head's commit is of the cut's group, and
`observe` (`remote.rs` 189-230) now gives head's commit's group even
when the engine is behind head and holds writes
(`Engine::commit_group`, `skynbd-engine/src/lib.rs` 868, one GET of
the commit object, only on that path). r1 (head moved after a lost
reply, the same uploader) and r2 (head moved, a new uploader): `snap/t`
stays absent, the publish stays pending, and `not_offered` is 1. Both
failed at `70d0f23`.

*The remainder, judged.* Head moved to a commit **of the cut's own
group** passes `head_of`. n1: cut 1 (group 7) is committed with its
reply lost, another writer moves head to a commit it made with group 7
and other content, and the same uploader goes on: it finds cut 1
committed, hands it on with head's commit, and publishes `snap/t` on
the other writer's commit, with a row that says cut 1 and
`Published { found: false }`. The snapshot is not the pool at `pool@t`
and nothing says so.

- *How it happens for real.* The answer names one way: after
  `zpool import -T`, a rollback (`Engine::rollback` sets head to the
  snapshot's own commit, so head carries that cut's group) to a
  snapshot of another cut state with the same group. The same holds
  within one cut state: a rewind makes groups go back (the rewind test
  writes cut 2 of group 40 after cut 1 of group 51), so a later cut can
  share a group with an earlier cut that was published. Either way it
  needs a rewind import, a published snapshot whose group equals the
  cut's, a rollback onto exactly that snapshot, and that rollback
  landing between the cut's head swap and the uploader's next read of
  it (a lost reply or a restart). Another writer's commit made by
  SkyNBD's own tools has no group and does not pass.
- *How bad.* A wrong snapshot that reads as right: high. But it is not
  the worst outcome of that case. n2 is the same case at the check,
  which this pull request did not change: after head moved to a commit
  of the row's group, `judge` matches head to the row by the group only,
  and cut 2 goes up as a delta onto the other history ("head holds the
  volume at cut 2: false"). The volume itself goes wrong, not only a
  snapshot of it. That is M2.4's: N1 of my review of M2.4b-2
  (`check.rs` compares the group alone, Q5 as the lead decided it).
- *Verdict.* Not a reason to hold this pull request: the likelihood is
  low, the root is the check's and older than M2.5, and the exact fix,
  the cut's row naming its commit, closes both at once. The lead has it
  as a package after M2.5-2 and before M2.6; it should name n1 and n2
  as its tests. Until then the description's "Not done" says it
  plainly, which is right.

**F2, a request of another cut state before the first check: fixed.**
r3: a request of a stale state is refused at once (`State`) and the
name stays free; asked with the store's state it is accepted. r3b: a
driver not told its state refuses before its first check and accepts
after it. `set_state` and `set_attachment` end what pends of another
(N4).

**F3, a publish that cannot be made stops the uploads: fixed.** A
publish has its own wait (`publish_at`, `publish_errors`,
`uploader.rs` 139-187, 353-369), and a turn steps a publish only where
one is due and its wait has passed (`uploader.rs` 275). u3 (a manifest
PUT that always fails): cut 2 goes up at 4 ms, and the publish fails
10 times in 600 s, each failure waited after. u4 (a publish refused
while a cut is at its commit): 9 sends and 495 s of waits in 2000
turns, and cut 2 is up. *Can a turn
still step a publish ahead of the cut?* Yes, and by design: a publish
whose wait has passed takes the turn, but a publish that succeeds has
at most a lookup and a transaction to take (and one more lookup after a
failure), at most 64 are pending, and one that fails waits its backoff, so the cut loses a
bounded number of turns, never its progress. r4 still counts 27
operations, so the kill points of the first look hold. Between
publishes there is a hold, which is D2.

**F4, an uploader that will not step again: fixed.** u2: fenced after
another took the lease, the publish lands nothing, ends
`Refused(Stopped)`, and what is asked after is refused. u5: once the
store keeps a cut, `Stopped` and the same. u1 passes.

**F5, `Done` from memory: fixed.** `Answer::Done` is gone. r10: a name
deleted and asked again is looked up anew and published on cut 2 with
the new manifest.

**F6, the row does not name its commit: fixed.** The row holds the
commit (version 1, 118 bytes), and `recorded_in` (`publish.rs` 187)
wants `row.commit == commit`. r8: another's ref made anew beside our
leftover row reads `Taken`. f2: our own ref made anew beside our own
leftover row (the ref deleted, the name asked again) replaces the row,
which then names cut 2's commit. f3: a ref remade by another on the
row's own commit reads as ours (`found: true`); its content is the
cut's, so that is the right answer.

*The versioned-format rule.* The layout changed from 86 to 118 bytes
and stayed version 1. CONTRIBUTING's rule asks for a bump and a
cross-version decode test when serialized bytes change. I do not ask
for one here: the 86-byte layout was never merged, and nothing could
have stored one (the daemon's `Publishing::default()` refuses every
request). What a leftover 86-byte row would do is safe all the same.
f1: it decodes to `Err(Length(86))`, the lookup reads the name as
`Taken`, and nothing panics. From the merge on, a change to the row
takes a version, and the row's version and length errors have their
tests (plants I01 and I02 are caught).

**Q1, the deadline and the withdrawal: the races hold.**

| | Race | Result |
|---|---|---|
| q1 | transaction landed, reply lost, looked up after the deadline | `Done`, no write; `found` 1, `expired` 0 |
| q2 | transaction did not land, looked up after the deadline | `Expired`, no write |
| q3 | lookups failing across the deadline | stays pending; then `Expired`, no transaction |
| q4 | withdraw after done | stays `Done`, `withdrawn` 0 |
| q5 | withdraw while waiting for a cut | `Withdrawn` at once; cut 2 carries nothing |
| q6 | withdraw with its cut known, before the transaction | `Withdrawn`, no write |
| q7 | withdraw after a failed transaction | pending until looked up: `Done` where it landed, `Withdrawn` where not; no write |
| q8 | withdraw on a restarted uploader | `Unknown`; asked again, `Done` |
| q10 | the uploader stopped (as when its lease is lost) with a transaction landed unheard | `Refused(Stopped)` while `snap/t` exists; a new uploader asked again says `Done` |
| u2 | fenced | `Refused(Stopped)`, nothing lands |
| q9 | withdraw while the lookup after a failed transaction is due, then the same request again | `Accepted`, then `Withdrawn`: D1 |
| q11 | asked again after a restart, 55 s into a 60 s expiry | published at 100 s: the deadline starts again at each acceptance (note below) |

The driver expires before each publish step (`driver.rs` 411) and the
uploader at each turn, so no transaction is begun after the deadline;
one begun before it may land after it, on a cut given before it, as
the module docs say.

**T1 to T3: the tests are there and catch.** T1 is
`a_name_taken_between_the_lookup_and_the_transaction_is_refused` (G06,
J05 caught); T2 is u1 to u5 ported to `tests/uploader.rs` (K01, K02
caught); T3 is a case of the rows table (B02 caught).

### Findings (none blocks)

**D1. A withdrawal outlives the same request asked again (low;
certain, q9).** `publish.rs` 592-597 and 654-660. A publish whose
transaction failed is withdrawn: it stays pending (`Recheck`) with its
lapse set, as it must until its lookup. The agent then asks the same
request again: `answer` finds it pending and equal and says `Accepted`,
but the lapse stays, so the lookup finds the name absent and the
publish ends `Withdrawn`. Nothing is published that should not be, and
`publish?` tells the truth at the end, but an `Accepted` that cannot
come true is a wrong answer. Clear a `Withdrawn` lapse when the same
request is asked again (and leave `Expired`, whose deadline has
passed), or refuse it. One line in `answer` and q9 as its test.

**D2. One publish that cannot be made holds every later one (low; model
only; certain, u6, u7).** `Publishing::next` (`publish.rs` 697-703)
takes the oldest publish with work, and the wait after a failure is
one for all publishes (`publish_at`, `uploader.rs` 362-363). u6: `a`
fails at every transaction, `b` is asked a second later and could be
made: in 600 s `a` is sent ten times (0, 1, 3, ... 511 s) and `b` never
once; both end expired. u7, with an expiry of two minutes: `a`'s last
wait runs to about 127 s, past both deadlines, and `b` expires at 121 s
without having been looked up once. Also, the wait is not
reset when a publish ends expired or withdrawn (`after_publish` resets
it only for `Published` and `PublishRefused`). I know of no path on a
real store where one publish fails and the others would not: a name
taken ends refused, and a store that fails, fails for all. So this is
low today. It becomes real if a request can carry something the store
refuses alone (a manifest too large for a store's limit, say). A fix
that keeps F3's answer: keep the wait per publish, and let `next` take
the oldest publish whose own wait has passed.

### Notes

- **q11, the deadline's clock (M2.5-2 and M2.6).** The deadline is
  counted from the acceptance on this uploader. A publish asked again
  after a restart starts a new deadline; asked again while pending, it
  keeps the old one. So an agent that counts its hold from its first
  request can let `pool@t` go while the next daemon may still publish
  it (q11: asked at 0 with 60 s, asked again of the next uploader at 55
  s, published at 100 s). The agent must count from its last request
  answered `Accepted`. The module docs say "counted from when it is
  accepted"; the channel's documentation in M2.5-2 should say which
  acceptance.
- **q10, `Stopped` with a landed transaction (M2.6).** An uploader that
  stops ends a publish whose transaction failed as `Refused(Stopped)`,
  though the transaction may have landed. The agent should read
  `Stopped` as "ask the next daemon", not as "not published".
- **The rows are never deleted** (the answer's own note). Each publish
  adds a row to the refs object, which every head swap reads and writes
  whole. Agreed that retention should delete the row with the ref.
- The description's "Not done" list matches the code as far as I read
  it.

### Plants

| Group | Plants | Caught |
|---|---|---|
| The rule (`carried_by`, a cut's `itself`) | A01 to A08 | 8 of 8 |
| What records a publish (`recorded_in`, the row read at the lookup) | B01 to B04 | 4 of 4 |
| The answers at once | C01 to C08 | 8 of 8 |
| The cuts in view, and the first that carries | E01 to E09 | 7 of 9: E08, E09 survive |
| The transaction, its failure and its lookup | G01 to G13 | 13 of 13 |
| The commit a publish names | H01, H02 | 2 of 2 |
| The row's key and format | I01, I02 | 2 of 2 |
| The figures and the outcomes kept | J01 to J08 | 8 of 8 |
| Through the uploader | K01, K02 | 2 of 2 |
| The author's: F1 to F6 | PF1a to PF1c, PF2a, PF2b, PF3a to PF3c, PF4a to PF4e, PF5, PF6 | 15 of 15 |
| The author's: the deadline and the withdrawal | PE01 to PE11 | 11 of 11 |
| New | N01 to N11 | 8 of 11: N04, N07, N09 survive |

The author's other six plants are my first look's C03, B02, G06, J05,
K01 and K02, counted in their groups above; all are caught.

New plants that are caught, to show what the new tests reach: N01 and
N02 (the read after a failed commit, or the check, offers head as read
whatever its group: F1 on each path alone), N03 (a publish whose cut
is known is not ended at its deadline), N05 (a lapsed publish found
absent ends only at the next `expire`), N06 (an attachment set anew
keeps what pends of another, N4), N08 (a publish's conflict goes
through the cut's arm, u4 of the first look), N10 (the row's commit
and snapshot id swap places, a layout that still reads back) and N11
(the row names no commit).

The survivors:

- **E08** (after a failed transaction, the first known cut that carries
  the publish instead of its own): no difference can be seen, as at the
  first look; the answer leaves it, and so do I.
- **N04** (the deadline exclusive, `>` for `>=`): a boundary at one
  instant; nothing to see.
- **E09** (the newest publish with work stepped first): no test depends
  on the order. With it, u6 and u7 pass: the order is half of D2, and a
  test of D2 would hold it.
- **N07** (a publish that ends does not reset the publishes' wait): no
  test has one publish fail, then one succeed, then a third fail, and
  checks that the third waits 1 s and not the doubled wait. Low; one
  test in `tests/uploader.rs`.
- **N09** (the uploader's sleep ignores when a publish may step,
  `uploader.rs` 464): no test has a publish's wait end while the cut
  waits longer (a cut in backoff). Without the line, the publish is
  retried at the cut's wait or the lease's renewal instead of its own.
  Low; the same kind of test.

### Scenarios

| Scenario | What it sets up | At `9e4d4e8` |
|---|---|---|
| r1 | F1: head moved after a lost reply, the same uploader | passes: nothing published, `not_offered` 1 |
| r2 | F1: head moved before a restart | passes: the same |
| r3 | F2: a request of another cut state, before the first check | passes: `Refused(State)` at once |
| r3b | F2: a driver not told its state | passes: refused before the first check, accepted after |
| r4 | the operations of each step | passes: 27 operations, as at the first look |
| r5 | a failure at each operation of a publish on an older cut | passes: no ref left before the transaction, published on cut 1 each time |
| r6 | a lost reply, then a failed lookup | passes |
| r7 | a cut taken from the store after its commit carries its publish | passes |
| r8 | F6: another's ref made anew beside our leftover row | passes: `Taken` |
| r9 | T1: the name taken between the lookup and the transaction | passes: `Taken`, cut 2 up, 1 error |
| r10 | F5: a name deleted and asked again | passes: published on cut 2 |
| n1 | F1's remainder: head moved to a commit of the cut's own group | **fails**: published on the other commit |
| n2 | the check, the same case (M2.4) | **fails**: cut 2 goes up onto the other history |
| q1 to q8, q10 | Q1's races (table above) | pass |
| q9 | withdraw, then the same request | **fails**: `Accepted`, then `Withdrawn` (D1) |
| q11 | the deadline after a restart | prints: published at 100 s |
| f1 | a row of `70d0f23`'s 86-byte layout | passes: `Err(Length(86))`, `Taken` |
| f2 | our ref made anew beside our own leftover row | passes: the row names cut 2's commit |
| f3 | a ref remade by another on the row's commit | prints: `Published { found: true }` |
| u1 | a publish asked before the lease is held | passes |
| u2 | a publish after another took the lease | passes: `Refused(Stopped)`, nothing lands |
| u3 | a manifest PUT that always fails | passes: cut 2 up at 4 ms, 10 failures in 600 s |
| u4 | a publish refused while a cut is at its commit | passes: 9 sends, cut 2 up |
| u5 | the store keeps the cut | passes: `Stopped` |
| u6 | `a` cannot be made, `b` can | **fails**: `b` never sent in 600 s (D2) |
| u7 | as u6 with a two-minute expiry | **fails**: `b` expires unlooked (D2) |

The scenarios and plants are not committed; they were run in a scratch
worktree of the head, which was clean after every run.
