## Review of #131 at c973611 (F5, and the follow-ups of #126, #127 and #129)

**Verdict: can merge. Nothing blocks. I found no defect in this pull request.**

F5 is closed. A keeper that holds the lease, or is unsure of it, now only renews, and a renewal can only push out the expiry of the row `(holder, epoch)` it last held. It never changes the holder or the epoch, and it never writes over a row it did not judge. Epochs only grow, so once the row has left `(holder, epoch)` it never comes back to it, short of the refs object being deleted. That makes `NotHeld` final, and "lost for good" is the right reading of it. I could not make two holders hold one lease through the renewal, on any store, in any order of the operations around it. The object store's retry judges the rule again on the row it reads after a refused write, with the clock read again: it does not retry blindly. The connect loop is right for a Unix socket on Linux.

What I have is a stale doc, a word missing from the rule's docs and some wrong references in the description (suggestions S1 to S4), three gaps in the tests (G1 to G3), and three notes on code that this pull request does not change (P1 to P3). None blocks.

I read the diff against 8e4638d first, then D-M3 (`docs/skynbd-multinode-design.md`), the notes' M2.4c entries, and my notes on #117 (L4, N1). The description came last. Everything below was run at c973611 unless said otherwise.

- **The head's tests, with the test hooks** (`cargo test -p skynbd-refs -p skyzfs-upload -p skyzfsd --features skyzfsd/test-hooks,skyzfs-local/test-hooks`) pass here (`t_hooks.out`):
  - skynbd-refs: unit 18, cadence 1, object_refs 15, object_refs_s3 2 (skipped without an endpoint).
  - skyzfs-upload: unit 51, crash 5, driver 22, holes 5, publish 17, queued 9, uploader 58.
  - skyzfsd: lib 178, closed_output 3.
  - The plain run is the same apart from skyzfsd's lib, 139 without the hooks (`t_plain.out`). The gates are under "Gates".
- **Scenarios the head's tests do not have.** All pass on the head except where said.
  - **E1 to E4** are on the refs stores (`scenarios_refs.rs`, `scn_refs.out`).
  - **K1 to K3** are on two real keepers (`scenarios_keeper.rs`, `scn_keeper.out`).
  - **C1 to C5** are on the connect (`scenarios_connect.rs`, `scn_connect.out`).
- **Plants.** 31 plants were each planted alone, against the tests of the crates they touch, with the file restored in a `finally`. Each suite ran in its own process group, killed whole at 900 s (`plants.py`, `plants.out`; the worktree was clean after).
  - Twelve are a sample of the author's seventeen, written again from the description's table. All twelve are killed, by tests the description names.
  - Nineteen are mine: thirteen are killed and six survive (U8, U9, U17, CA, CB, CD; G2 and G3).
  - None was not applied.
  - CA and CB were run once more against my connect scenarios, which kill both (`plants_scn_connect.out`).

### Defects

None.

### Suggestions (none blocks)

**S1. The doc of `LeaseState::Lost` still names a case that F5 removed** (`crates/skyzfs-upload/src/lease.rs:62` to `65`).
- It reads: "`holder` is empty where the row has gone, or where the lease was found taken and given back between two of this holder's reads". After F5 there is no read and take, so the second half cannot happen. An empty holder (epoch 0) now means one thing: no row (`renew`, `NotHeld(None)`, `lease.rs:196`).
- The notes' open list under M2.4c (`skyzfs-impl-notes.md`, "M2.4c as built") has two entries that go with this pull request: F5 itself, and "a lease lost in the race between the read and the take names no holder". Those are the lead's to remove.
- Certainty: high.

**S2. Say "expired or released" where the rule is stated** (`crates/skynbd-refs/src/lib.rs:60`, `147`, `279`).
- Renewing a released row of the same `(holder, epoch)` is intended:
  - It is what `lease_acquire` has always done for the same holder.
  - D-M3 treats a released lease as an expired one ("release zeroes expiry so epochs stay monotonic"; `lib.rs:171`, "a released lease is an expired one").
  - The conformance case pins it in its last step, and plant U6 is killed by it.
- It is also safe: nobody took the lease in between, or the epoch would differ.
- The keeper never renews after its own release, because `release` puts it in `None` and the next `keep` takes.
- The only way a released row gets renewed is by a second daemon under the same name (E3; K3 in `scn_keeper.out`). That is N1 (P3), not something new.
- The docs say "expired or not". The word "released" in the trait's doc and in `RenewOutcome::Renewed` would save the next reader the question this review asked.
- Certainty: high that it is intended. The wording is a suggestion.

**S3. The description's references.**
- "Design §5.2 and §6.2 (the lease, D-M3's fence)": `docs/skyzfs-design.md` §5.2 is "Cuts and snapshot visibility" and §6.2 is "Portability rules". The lease is D-M3 in `docs/skynbd-multinode-design.md`, and the notes' M2.4c entries.
- "Could hang past `--wait`": `skyzfsd` has no `--wait`. The client's bound is `ASK_LIMIT`, 40 s, applied to the connect and to each read and write.
- The summary line says "Codex's P2 on #127's `ask`". The P2 on `ask` is on #129 (`control.rs:662`, which the description links correctly). #127's own P2 was on the server's wake.
- The code's comment (`control.rs:700`, "Codex on #129") is right.

**S4 (optional). One deadline for the client's request.**
- `ask` now waits at most `limit` for the connect, then at most `limit` for each read and each write (`control.rs:666` to `669`).
- So a daemon that stalls in each phase holds the client for up to about three limits: the connect, a write of a manifest larger than the socket's buffer, and the read of the answer. A daemon that sends a byte now and then holds it longer, as before this pull request.
- The client never had one deadline per request. The server does: `REQUEST_LIMIT` and `ANSWER_LIMIT` are untouched. Nothing I found needs one on the client.
- If one is wanted, the `until` of `connect` can be passed on and what is left of it set as the read and write timeouts.
- Certainty: high on the arithmetic. Whether it matters is the lead's call.

### Gaps in the tests (none blocks)

**G1. No job of CI runs the renewal against real S3 semantics.**
- `tests/object_refs_s3.rs` runs `exercise_ref_store` and the fence over MinIO. It runs neither `exercise_lease` nor `exercise_lease_renew`, and no race at the write.
- The renewal's correctness on S3 rests on `put_if`'s mapping (412 and 409 → `PreconditionFailed`, `skynbd-store/src/s3.rs:262` to `266`) and on If-Match.
- I ran both conformance cases (E4) and E1's interleavings (up to two operations) over a live S3 endpoint, a local moto server, since MinIO could not be fetched here. Both pass apart from P2.
- Adding `exercise_lease_renew` to `object_refs_s3.rs`'s MinIO job is a few lines.

**G2. No test asks a socket where no daemon listens, or none exists, through `connect`.**
- Plant CA tries a refused or missing socket again until the limit. It turns "no daemon" (a stale socket, no file) into a 40 s wait for `skyzfsd cuts` and `publish`, with the message "its backlog is full". It survives every test.
- The commands' tests use a fake asker.
- Scenario C2 shows that the head answers at once: ECONNREFUSED for a stale socket and for a plain file, ENOENT for no file, each in under 10 µs. C2 and C4 fail with CA (5 s each, at their limit).
- Two more plants on the loop survive:
  - CB has no pause, which is a busy loop. With it, C5 spends 1.99 s of CPU in a 2 s wait, against 1.4 ms on the head.
  - CD keeps the pause at 1 ms.
- Neither is a wrong answer, only waste. A test like C2 would hold CA, and C5's bound on the CPU would hold CB. I did not run CD against C5.

**G3. Nothing holds the memory and redb stores' renewal to one critical section, or the acting store's pass-through.**
- These plants survive:
  - U8: redb reads the row outside its write transaction.
  - U9: the memory store reads under a read lock, then writes under another.
  - U17: the acting store renews at its own epoch, not the one asked.
- The risk is low:
  - `MemoryRefStore` is a store for tests.
  - A redb file has one process.
  - Nobody renews through an acting store: the daemon's keeper is given the plain refs (`bins/skyzfsd/src/upload.rs:746`).
- A race test like `steal_race_has_one_winner` for renew against acquire on each store would hold U8 and U9.

### Notes on code this pull request does not change

**P1. `MemoryStore`'s etag is a counter that starts again at 0 after a delete** (`skynbd-store/src/lib.rs:226` to `233`).
- So an etag read before a delete can match the object written after it (ABA).
- E1 on `MemoryStore` gives 44 of 6560 interleavings that differ from the model, and every one has a delete between the read and the write. For example: the row is `(a, 1)`; between the renewal's read and its write the object is deleted and `b` takes (`(b, 1)`, version 0 again, as the read was). The renewal's write then lands and writes `(a, 1)` over `b`'s row.
- With deletes left out, E1 has 0 differences in 4680 cases. On a store whose etag is a hash of the content, as S3's is, it has 0 in 6560.
- It concerns the test store only, and every `update_inner` operation, not only the renewal. It is worth a line in its doc, or a generation that survives the delete, so that a test of a delete race does not pass or fail for this reason.
- Certainty: high (E1).

**P2. On S3, a conditional write on a key deleted since its read is a `Backend` error, not a refused precondition** (moto answers 404, `s3.rs:271`).
- So a renewal whose refs object is deleted between its read and its write answers an error (the keeper is `Unsure`), where the model says `NotHeld(None)`. All 64 mismatches of E1 over live S3 are this case, and they are 64 of the 728 interleavings.
- It is safe: the next renewal reads no row and the keeper is lost. Every `update_inner` operation behaves the same.
- I have not checked what MinIO and AWS answer here.

**P3. N1 of #117's review is unchanged: two daemons under one name both hold the lease, at one epoch.**
- F5 does not change it. The rule compares the name and the epoch, and both daemons have the same of each.
- K3 shows it again, and adds what F5 makes visible: one daemon's clean release is undone by the other's next renewal (row `("node-a", 1, 0)`, then `("node-a", 1, …)`), as `lease_acquire` would have done before.
- What makes a holder name mean one daemon (the default `<hostname>:<machine-id>`) is outside this pull request.

### (1) Does the renewal's rule close L4 without opening another path?

Yes. The rule (`lib.rs:151` to `168`): the row `(holder, epoch)`, live, expired or released, gets a new expiry `now + ttl` with the same holder and epoch, and the answer is `Renewed`. Anything else, including no row, is `NotHeld` with the row as read, and nothing is written.

The keeper (`lease.rs:121` to `200`):
- It takes only from `None`: at its first turn, or after its own `release`.
- It renews only from `Held` or `Unsure`, at the epoch it last held.
- It does nothing in `Lost`.
- `Renewed` → `Held`, `NotHeld(row)` → `Lost{row}`, an error → `Unsure`.

Why no other path opens:
- **The rule never changes holder or epoch.** So the renewal cannot take a lease, and cannot move one from one epoch to another.
- **`NotHeld` is final.** The transitions of a row are:
  - a take by the same name keeps the epoch;
  - a steal moves to another name at epoch + 1;
  - a release keeps the name and the epoch.

  Once the row's holder and epoch are not `(a, e)`, its epoch is above `e`, and so is that of every later row. The one way back to `(a, e)` is a delete followed by a take at epoch 1 (P1, P2). So `NotHeld`, even on a stale read, is true from that read on. A keeper that calls it lost for good is right.
- **`Renewed` is true at the instant of its write.**
  - On the object store the write is conditional on the etag of the read the rule judged.
  - On the memory store it is made under one write lock.
  - On redb it is one write transaction (`redb_store.rs:343` to `368`).
  - The cadenced and acting stores pass the call to their authority and their inner store (`cadence.rs:297` to `305`, `acting.rs:130` to `138`). Plant U16, the cadenced store renewing in its cache, is killed.

Every interleaving, the keeper `a` renewing at epoch `e`:

| Between the renewal's read and its write | Answer | Row after | Shown by |
|---|---|---|---|
| nothing, the row live | Renewed | `(a, e)`, pushed out | conformance; E1 |
| nothing, the row expired (an outage) | Renewed | the same | conformance; E1; plant R3 killed |
| the clock moves only | Renewed (the etag is unchanged) | the same | E3 |
| `b` takes the expired row (L4) | NotHeld, the write refused, then judged again on `(b, e+1)`; `a` lost | `(b, e+1)`, `b`'s | the head's `a_take_between…`; E3; K1: `b` keeps epoch 2 for all 30 renewals over ten leases' time, and `a` holds in none |
| `b` takes, then its own lease expires too (L4 exactly) | NotHeld | `(b, e+1)` | the head's test; K1 |
| the same, `a`'s clock ahead by twice the lease | NotHeld | `(b, e+1)` | K2 |
| `b` takes and releases | NotHeld; `a` does not take back the released row | `(b, e+1, 0)`; the next taker bumps to `e+2` | E3 |
| `b` takes and releases, and a new daemon named `a` takes | NotHeld(`(a, e+2)`): the epoch tells the two `a`s apart | `(a, e+2)` | E3; plant U5, a keeper that adopts a newer epoch of its own name, killed |
| `b`'s take and `a`'s renewal race on the expired row | one conditional write wins. `a` first: `b`'s take reruns `acquire` on a live `(a, e)` and is refused. `b` first: `a`'s renewal reruns on `(b, e+1)` and is NotHeld | one holder | E1, both orders |
| a second daemon named `a` renews (N1) | Renewed | `(a, e)` | E3; P3 |
| a second daemon named `a` releases (N1) | the write refused, then judged again: `(a, e, 0)` is `a`'s, Renewed | revived | E3; S2; P3 |
| `a`'s own release | cannot happen: the keeper's `keep` and `release` take `&mut self` | | by reading |
| a commit of `a` (free renewal, fenced by `(holder, epoch)`, `object_store.rs:380` to `417`) | Renewed on the new etag | `(a, e)` | E1 (`Commit`) |
| the refs object deleted, then `b` takes | memory store: Renewed over `b`'s row (P1, test store only); S3: an error, `Unsure`, then lost at the next renewal (P2) | | E1 |
| a stale read | a stale etag fails the write and the row is read again. A stale NotHeld is still true (above) | | by reading; E1 |

E2 adds random programs: 600 sequences of 40 operations (acquire, renew at a fixed epoch, renew as the keeper would, release, commit, clock moves) run alike on the memory, redb, object and cadenced stores and on an independent model of D-M3. That is 24 000 operations, of which 6012 are renewals and 1038 renewed. No store differed from the model in any answer or any row.

### (2) Does `update_inner`'s retry judge the rule again on the row a racing take wrote?

Yes (`object_store.rs:315` to `370`):
1. A refused write (`PreconditionFailed`) goes back to the top of the loop.
2. `load` reads the object and its etag again.
3. The renewal's closure reruns on that state: `current` is taken from `state.owner`, and `clock()` is read again (`object_store.rs:551` to `577`). A `NotHeld` ends the call with nothing written.
4. After sixteen refused writes the answer is an error, so `Unsure`.
5. With no object, `load` gives `IfNoneMatch`, and the rule says `NotHeld(None)` before any write.
6. `fenced: false` is right for the call. The ambient fence would answer `LeaseLost` and not `NotHeld`, and its free renewal would add nothing.

The racing take put between the read and the write, on each store:

| Store | How the conflict is seen | Result |
|---|---|---|
| `MemoryStore` (version counter) | `put_if` compares versions under its lock | E1: 6560 interleavings, 2737 of them with the write refused and judged again; 0 differences without a delete (P1 with one) |
| a store whose etag is the content's hash, as S3's | the same, on the hash | E1: 6560 interleavings, 2217 refused and judged again; 0 differences |
| live S3 (moto, If-Match / If-None-Match) | 412 and 409 → `PreconditionFailed` (`s3.rs:262` to `266`) | E1 (up to two operations in between): 728 interleavings, 133 refused and judged again; 0 differences apart from the 64 deletes of P2. E4: both conformance cases pass |
| redb, the file store of the refs | one write transaction: no read and write apart to put a take between | E2; plant U8, the row read outside the transaction, survives (G3) |
| `MemoryRefStore` | one write lock | E2; plant U9 survives (G3) |

The two plants that make the retry blind are killed by the head's `a_take_between_the_renewals_read_and_its_write_is_not_undone`:
- U1: on a refused write, the state judged before is written again under the new etag.
- U2: the rule is judged once and its decision kept across retries.

U3, the renewal made a take in one write with `lease::acquire`, is killed by the conformance.

### (3) The connect's loop, for a Unix socket on Linux

`control.rs:704` to `740`. My reading of the kernel's `unix_stream_connect`, held up by C1 to C5:
- **A full backlog on a non-blocking socket is `EAGAIN`.** The listener's queue is full (`unix_recvq_full`), and the timeout is 0, so the kernel returns at once. There is no `EINPROGRESS` for `AF_UNIX` stream sockets: the connect either completes or answers `EAGAIN`. A socket that got `EAGAIN` is still unconnected and can be connected again. C1: room made in the backlog after 400 ms, connected at 414 ms.
- **`ECONNREFUSED`** comes from a socket file nobody listens on, a listener closed while the client waits, or a file that is not a socket. **`ENOENT`** comes from no file. Both return at once:
  - C2: under 10 µs each.
  - C4: the listener closed 300 ms into a wait at a full backlog; the client is told within one pause.
  - Plant CA, which retries them until the limit, is G2.
- **`EINTR`** cannot come from a non-blocking connect, which never sleeps. Handling it costs nothing.
- **The limit** is read on the monotonic clock. The last pause is cut to what is left, so the connect ends within the limit plus one try. C5 asks a full backlog with a limit of 2 s and gets `TimedOut` after 2.0001 s. The author's test, A2 and CC pin the same.
- **Backoff, not a busy loop.** The pause starts at 1 ms and doubles to 50 ms (`CONNECT_PAUSE`). That is about 800 tries in 40 s. C5 measures 1.4 ms of the thread's CPU over a 2 s wait. Plants CB (no pause) and CD (the pause not growing) survive the head's tests (G2).
- **Once connected the socket is set back to blocking.** The read and write timeouts then apply, as before. Plant A4, the socket left non-blocking, is killed by four tests.
- **One deadline per request:** see S4.

### (4) Do the follow-ups' tests fail when the code is broken?

Yes, for each follow-up, and for F5. The table is below. All twelve of the author's plants that I wrote again are killed, by tests the description names. My suites stop at the first test binary that fails, so they list fewer tests than the description does.

- **#127 (a).** The two waits for the queue's wake have a limit of 10 s. N2, the ask of the count not waking the queue's thread, now fails `a_closed_desk_answers_as_a_store_that_holds_nothing` in 42 s of the suite instead of hanging it. QW, a call that does not wake it, fails three tests of `queued.rs` and hangs none.
- **#127 (b).** `gave_up` is tested on six errors that pause and four that do not. N5, every error tried again at once, is killed.
- **#127 (c).** The `cuts` docs (`control.rs:103` to `106`, `249` to `257`) now say that for `cuts` it is the store's queue that ends or does not answer. That agrees with the server's code as I read it in #127.
- **#126 y3.** `cut_steps_that_fail_slowly_slow_the_publishes_by_one_step_at_most`. K13, the turns' count started again at a failure of the cut's step, is killed by it.
- **The 100-turn limit** in `the_uploads_go_on_while_the_lease_is_unsure`. K3, a failed renewal taken as lost, now fails four lease tests in 34 s and does not hang.

| Plant | What is planted | Result |
|---|---|---|
| R1 (author) | the renewal compares the epoch alone | KILLED: the memory, redb and cadenced conformance (11 s) |
| R3 (author) | an expired row of this holder and epoch is not renewed | KILLED: the same (10 s) |
| R6 (author) | not held: the row read is written back | KILLED: `a_take_between_the_renewals_read_and_its_write_is_not_undone` (10 s) |
| O1 (author) | the object store renews without looking at the row | KILLED: cadenced conformance (10 s) |
| K3 (author) | the keeper takes a failed renewal as lost | KILLED: four lease tests (34 s), no hang |
| A1 (author) | `ask` connects blocking | KILLED: `an_ask_of_a_socket_whose_backlog_is_full_ends_within_its_limit` |
| A2 (author) | the tries do not stop at the limit | KILLED: the same |
| A3 (author) | a full backlog is an error at once | KILLED: the same |
| A4 (author) | the stream is left non-blocking | KILLED: four tests |
| N5 (author) | every error of accept is tried again at once | KILLED: `accept_is_tried_again_at_once_only_after_a_peer_or_a_signal` |
| K13 (author) | the turns' count starts again at a failure of the cut's step | KILLED: `cut_steps_that_fail_slowly_slow_the_publishes_by_one_step_at_most` |
| N2 (author) | an ask of the count does not wake the queue's thread | KILLED: `a_closed_desk_answers_as_a_store_that_holds_nothing` (42 s), no hang |
| U1 | `update_inner` retries blindly: on a refused write the state judged before is written under the new etag | KILLED: `a_take_between…`, `concurrent_pins_are_not_lost` |
| U2 | the object store's renewal judges the rule once and keeps that answer on a retry | KILLED: `a_take_between…` |
| U3 | the object store's renewal is a take in one write (`lease::acquire`) | KILLED: cadenced conformance |
| U5 | the keeper adopts a newer epoch of its own name | KILLED: `this_holders_name_at_another_epoch_is_lost` |
| U6 | a released row is not renewed | KILLED: the memory, redb and cadenced conformance |
| U7 | NotHeld answers no row in place of the row read | KILLED: the same |
| U8 | redb reads the row outside the write transaction | SURVIVED (G3) |
| U9 | the memory store reads under a read lock, then writes under another | SURVIVED (G3) |
| U12 | an unsure keeper takes (`lease_acquire`) instead of renewing | KILLED: the keeper's outage tests |
| U13 | the renewal renews a newer epoch of its holder, writing its own older one | KILLED: the conformance |
| U15 | the renewal pushes the row's old expiry out, not the time now | KILLED: the conformance |
| U16 | the cadenced store renews in its cache, not the authority | KILLED: cadenced conformance |
| U17 | the acting store renews at its own epoch, not the one asked | SURVIVED (G3; nobody renews through it) |
| U18 | the keeper reads the row, then takes (two calls; the read decides) | KILLED: the keeper's race tests |
| CA | a refused or missing socket is tried again until the limit | SURVIVED (G2); my C2 and C4 kill it |
| CB | no pause between tries | SURVIVED (G2); my C5 kills it (1.99 s of CPU) |
| CC | the limit's error is not `TimedOut` | KILLED: the author's backlog test |
| CD | the pause does not grow | SURVIVED (G2) |
| QW | a call does not wake the queue's thread | KILLED: three tests of `queued.rs`, no hang |

### CI and Codex at c973611

- **Codex:** its code review and its security review of c973611 are complete, with no finding. The pull request has no review comments.
- **CI:** every check is green:
  - `ci`: lint, test, unit-arm64, gate, s3-backends.
  - `skyzfs-ci`, coverage, e2e, k3s.
  - The cut rig on the daemon, its upload job included.
  - The replay rig's jobs through skyzfsd.
  - The node B rig.
  - The daemon rig on ubuntu-24.04, ubuntu-26.04 and ubuntu-26.04-arm (run 44466442020; the judge ended 0 on all three).

### Gates

In a worktree of c973611, on rustc 1.100.0 (2026-11-09), which `rustup check` reports as the current stable (`gates.out`):
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo clippy -p skyzfsd --features test-hooks --all-targets -- -D warnings`: clean.
- `cargo clippy -p skyzfs-ublk-sys --features control-test-program --all-targets -- -D warnings`: clean.
- `cargo test -p skynbd-refs -p skyzfs-upload -p skyzfsd`, plain (`t_plain.out`) and with the test hooks (`t_hooks.out`): 0 failed.

The build directory was deleted afterwards.
