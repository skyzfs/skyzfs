## Next look at #127 at d65ffb9 (M2.5-2)

The answer to my review of 1595d6f is the issue comment of 19 November
(6995953926). It is in four commits of M2.5-2's own: 8b71360 (the fix),
4342c7e and 81c7824 (limits on two tests' waits), and ec8d25d (a test of
a peer that stops part way). Four merges sit on top:
- a8a1a33 and 03fd827 merge #126's 1f591fa and e6d5570;
- c6adca9 merges #125's c9b2b14;
- d65ffb9 merges `zfs-topology` at 0d42566, with #125 merged.

**Verdict: can merge.** Nothing blocks in my judgement.
- **F1 is closed.** No store call is left between two turns. `cuts` goes
  to the store's desk and never reaches the uploader.
- **F2, F3, F5, T1 and T2 are closed** as the answer says. F4 is closed
  for `cuts`. A publish still waits for the step under way, as
  documented, and time limits on object storage are left out, which I
  agree with.
- **The minor points are taken.**
- **New code:** I found no defect in `Desk::cuts_now`, the request
  deadline or the non-blocking wake. Two small test gaps and one nit
  remain (below); none blocks.
- **Plants:** all 29 of the author's are caught by the pull request's
  tests, as described. So is every one of mine from the first look that
  still applies.
- **CI** is green on every workflow at d65ffb9. The cut rig's `bound`
  line now passes on ubuntu-26.04 and ubuntu-26.04-arm.
- **Codex** reviewed d65ffb9 and found nothing.

### What I did

- Read the answer against the code: the diff 1595d6f..d65ffb9 of
  M2.5-2's own commits, and each merge.
- Gates at the head with rustc 1.100.0 (`rustup update stable`), on
  `skyzfsd`, `skyzfs-upload`, `skyzfs-ublk` and `skyzfs-local`:
  - `cargo fmt --all --check`: clean.
  - `cargo clippy --all-targets -- -D warnings`: clean, plain and with
    `skyzfsd/test-hooks,skyzfs-local/test-hooks`.
  - Tests with test hooks, 0 failed:
    - `skyzfsd`: 176 in the library, 3 in `closed_output`;
    - `skyzfs-upload`: 152;
    - `skyzfs-ublk`: 160;
    - `skyzfs-local`: 323.
  - Tests plain, 0 failed: `skyzfsd` 137 and 3.
  - Not run here, as before: the whole workspace and `control-test-program`.
    CI's `ci` run covers both and is green.
- Scenarios of my own, appended to the test files for a run and then
  removed. All pass at the head; the output is in `scenarios_2.out`.
  - Ten on the socket (`scenarios_control_2.rs`, R1 to R10). The first
    look's, rerun with the daemon's real limits, plus new edge cases of
    the deadline.
  - Three on the uploader (`scenarios_up_2.rs`, U1 to U3).
  - Four on the desk (`scenarios_queued_2.rs`, D1 to D4).
- 46 textual plants, one at a time, each file restored after its run
  (`plants_2.py`, then `plants_2b.py`; results in `plants_2.out`):
  - **The author's 29, as 27 plants.** Their script is not in the tree,
    so I wrote my own text for each from its description. A2d and A3d
    are A2 and A3 judged by the daemon's tests, so each pair is one
    plant.
  - **11 of mine from the first look** that still apply. M1 and M5 are
    the author's A9 and A10 now, and M13's code is gone.
  - **8 new ones on the new code** (N1 to N8).

  Each plant was run against the pull request's own tests (my scenarios
  skipped), stopping at the first suite that caught it. The second pass
  ran A2 and A3 against the daemon's tests too, and each plant that
  survived against my scenarios.

The worktree of the head was clean after every run.

### (1) F1: closed

**The fix:**
- `Uploader::control` (`crates/skyzfs-upload/src/uploader.rs:356`) is
  now synchronous. It touches only `Publishing`.
- `run_with` calls it in place, with nothing awaited around it (`:693`).
- `Control::Cuts`, `Local::cuts` and the desk's `Call::Cuts` are gone.
- A publish is judged against the cut state given at the start, which is
  the store's. 460e67c's read of the state at the ask is removed.

**How `cuts` is answered now:**
- The channel's handler asks the desk itself (`handler.rs:149`, `count`).
- `Desk::cuts_now` (`queued.rs:254`) leaves an ask in a slot of its own
  and wakes the queue's thread.
- `Desk::answer` (`queued.rs:171`) answers every waiting ask first, at
  the queue's next turn, from `Store::cuts`.

**My scenarios, rerun:**
- **The first look's scenario, for `publish`.** The author's
  `the_channels_requests_do_not_hold_back_the_leases_renewal` is that
  scenario. It passes, and plant A1 (a 21 s wait put back before the
  answer) fails it.
- **U1, bursts.** Each check of a cut takes 21 s, and sixteen requests
  (asks, progress, withdrawals) arrive at once at each of three moments:
  0.25 s, 1 ms and 5 ms before a renewal. All 48 are answered. The lease
  is taken at exactly 0, 10, 20 … 180 s.
- **U3, `cuts`, as the daemon asks it.** The uploader runs over a desk
  whose queue takes 21 s for each turn it is woken for. `cuts` is asked
  at the desk 0.25 s before the first renewal, and a publish goes
  through the inbox 0.25 s before the second.
  - `cuts` is answered after 11.25 s, at the end of the queue's turn
    under way.
  - The publish is accepted.
  - The lease is taken at exactly 0, 10 … 80 s.

  The uploader is not in `cuts`'s path at all, so the lease cannot wait
  on it.

**New problems looked for in `cuts_now`:**
- **Can it block the queue's turn?** No. `Store::cuts` for `InPlace`
  reads the cut list and the state id in memory (`local.rs:239`). The
  desk's lock is held only to move asks in and out.
- **Can it deadlock with a sync held on the disk?** No. The asks are
  answered before the parked sync is looked at. The author's sync test
  asks the count while the disk holds the sync. My plant N1 (a held sync
  withholds the count) is caught by it.
- **Can it give a stale or torn count?** No. The count is read once, on
  the queue's thread, at the turn after the ask. Every cut is counted on
  that same thread, so the count is whole. It is never older than the
  ask. D3 counts a cut on the queue's thread after the ask and before
  the answering turn, and the answer has it.
- **Can it be starved?**
  - Every ask wakes the queue (plant N2, no wake, is caught by the
    handler's tests), and the pump calls `desk.answer` after every turn
    (`serve.rs:1308`). The daemon serves one queue (`--queues` takes
    only 1).
  - D1: ten asks at once are answered at one turn, alike.
  - D4: a thousand asks that gave up while the queue took no turn leave
    nothing behind; the next ask is answered in 76 µs once the queue
    turns.
  - A queue that takes no turn at all, because it is held or stuck,
    gives `no-answer` naming the store after `ANSWER_LIMIT` (plant A5 is
    caught).
- **What happens at a close?** A closed desk answers `None`, which
  becomes `refused ended` (A3, A3d and A4 are caught). An ask left
  waiting when the desk closes ends at once (D2).

### (2) F2 and F3: closed

**F2. The wake no longer waits.** `server::wake` (`server.rs:99`)
connects with `SOCK_NONBLOCK`. When the backlog is full, `EAGAIN` returns
at once, and the connections already waiting there wake `accept`. I
reran R5 with the daemon's own socket and the kernel's backlog: one
trickler holds the thread, and the backlog is full after 4097
connections. `close()` returned `false` after 5.000 s. At 1595d6f it had
not returned after 20 s. R10: a wake at a path with no listener, at a
stale socket, or at a path too long for an address returns at once.

**F3. One deadline per request.** `Timed` (`server.rs:120`) gives each
read the time left until `REQUEST_LIMIT` (10 s), counted from the accept
(`:293`). All with the daemon's real limits:

| Scenario | Result |
|---|---|
| R3: a client sends half a request, then nothing | the next client is answered after 10.10 s |
| R4: a client sends a byte every 0.9 s | the next client is answered after 9.70 s; the trickler is cut off at 10.8 s |
| R6: a reasonable slow client sends a 1 MiB manifest in 64 KiB pieces over 4.8 s | accepted, with the manifest whole |
| R7: a manifest's last 3 bytes come 11 s after the request line | not taken, the handler not asked, nothing written back to that client; the client behind it answered at 9.99 s |
| R8: a handler takes 12 s, longer than `REQUEST_LIMIT` | its answer is written and read; the deadline covers reading the request, not writing the answer |
| R9: a client is queued behind a 12 s handler | answered: its deadline starts at its accept, not at its connect |
| R1: 20 concurrent clients | each answered, in 1.0 s |
| R2: over-long greeting and line; name of 128 bytes (taken) and of 129 bytes (refused `name`); `withdraw name=a<TAB>b` | refused `version`, `malformed`, `name` as listed, and each refusal reaches the client |

The author's own tests use a limit of 0.5 s. A7 (each read given
`IO_LIMIT`) and my N7 (a deadline 100 times as long) are caught by them.

### (3) F4, F5, T1, T2 and the minor points

- **F4:** closed for `cuts`, which is answered while a PUT stalls (the
  author's `the_count_is_given_while_a_put_of_the_uploader_stalls`, and
  U3). `publish`, `publish-progress` and `withdraw` still wait for the
  step under way, as `control.rs` documents. Limits on object storage
  operations are left for later; I agree that they do not belong here.
- **F5:**
  - The CLI's help (`cli.rs`), `Publishes::state` (`upload.rs`) and
    `serve.rs` now say that the state is named from the start and
    changes only at a start.
  - A closed desk reads `refused ended`.
  - Nit, not a finding: that refusal's text is "the uploader has
    ended", where for `cuts` it is the store's queue that has ended.
- **T1 and T2:** tests added. A9 and A10 (my M1 and M5) are caught.
- **The socket before `ready`:** `open_channel` (`serve.rs:1532`) now
  comes before the status wait and the `ready` line (`:1565`), with the
  uploader's thread started just before it. Checked by reading; the
  daemon rig and the cut rig, which wait for `ready`, are green with it.
- **A tab in a name:** refused `name` at parse (`control.rs:632`), so
  nothing reaches the request log (R2; A8 and N8 are caught).
- **The accept pause:** an `accept` that fails on the socket's own error
  pauses `ACCEPT_PAUSE` (100 ms). One whose peer gave up is retried at
  once (`gave_up`, `server.rs:321`). As the author says, nothing tests
  this: N5, every error retried at once, survives everything.

### (4) The merge resolutions

I compared each merge commit with git's own merge of its two parents
(`git merge-tree --write-tree`):
- a8a1a33, 03fd827 and d65ffb9 are exactly the automatic merge.
- c6adca9 had one conflict, in `crates/skyzfs-ublk/src/status.rs`: the
  layout table in the doc comment. It was resolved by keeping M2.5-2's
  rows for bytes 1032 to 1408 under #125's rows, which is right.
- `uploader.rs` at the head is #126's e6d5570 plus only M2.5-2's
  additions:
  - the module docs on the channel;
  - `Shown::publishes` and `PublishesShown`;
  - `Control`, `control()` and `post()`;
  - `run` as `run_with` with no requests.
  
  #126's `turn` (publishes and the cut sharing the time) is untouched.

The whole suite, my scenarios and the plants ran on the merged head.
A1, A11, M9 and N6, the plants in `uploader.rs` and `publish.rs`, ran
against #126's turns.

### Test gaps (minor, none blocks)

- **N3 survives the pull request's tests: nothing shows that a close ends
  a waiting ask of the count.** With the close's `slot.counts.clear()`
  removed, an ask left waiting at the queue's end would wait out
  `ANSWER_LIMIT` (30 s) and read `no-answer`, not `ended` at once. My D2
  catches it. A line in `a_closed_desk_answers_as_a_store_that_holds_nothing`
  would hold it.
- **N4 survives: nothing shows that the counts stay out of
  `Desk::answered`.** Its documentation says so. Only tests read that
  figure, so N4 changes nothing in the daemon. My D1 catches it.
- **N6 survives everything: nothing shows that a request other than an
  accepted publish leaves the sleep as it was.** The extra turn it would
  cause is harmless, because each turn is gated by its own wait and the
  lease's due time. I mention it only because `run_with`'s
  documentation states the behaviour.
- **N5:** see the accept pause above.

### Plants

All at d65ffb9, each against the pull request's own tests (`d`:
`skyzfsd`'s library with test hooks; `u`: `skyzfs-upload`'s library and
tests; `b`: `skyzfs-ublk`'s library). The tests that catch each plant
are in `plants_2.out`.

| Plant | Result |
|---|---|
| C1 to C5 (the channel's posts and its state in the status) | all caught |
| K1 to K4 (the kill points) | all caught |
| R1, R2 (the request log's lines) | caught |
| U1, U2 (an ended publish told; the publishes in the status) | caught |
| S1 to S3 (status v18, v17 read) | caught |
| A1 a store call between two turns | caught |
| A2, A2d the count only with one of the uploader's calls | caught, by the queue's tests and by the handler's |
| A3, A3d, A4 a closed desk read as a count of nothing | caught, by the queue's tests and by the handler's |
| A5 the count waited for with no limit | caught |
| A6 the wake's connect blocks | caught |
| A7 each read given `IO_LIMIT` | caught |
| A8 a name the uploader would refuse taken at parse | caught |
| A9 every user may ask (my M1) | caught |
| A10 another holder's reason dropped (my M5) | caught |
| A11 the state given at the start not kept | caught |
| M2, M3, M4, M6 to M12, M14 (first look) | all caught |
| N1 a held sync withholds the count | caught |
| N2 an ask of the count does not wake the queue | caught (by the handler's tests; the queue's tests poll every 5 ms) |
| N3 the close leaves the asks of the count waiting | survives; caught by D2 |
| N4 the counts counted as calls | survives; caught by D1 |
| N5 every accept error retried at once | survives everything (by reading, as the author says) |
| N6 any request begins the next turn at once | survives everything (harmless, see the test gaps) |
| N7 a deadline 100 times as long | caught |
| N8 a name checked only for `publish` | caught |

### CI and Codex

Runs on d65ffb9, all green:
- `ci`, including `unit-arm64`;
- `coverage`, `e2e`, `k3s-smoke`, `skyzfs-ci`;
- `skyzfs-control-rig` and `skyzfs-replay-rig`;
- `skyzfs-daemon-rig` (run 44430413263): ubuntu-24.04, ubuntu-26.04 and
  ubuntu-26.04-arm, on the daemon. Each table has 54 PASS and the one
  documented WAITING (`pin_failed_volume_while_a_request_is_held`). Its
  five FAIL lines are all negative controls (`ctl_*`).
- `skyzfs-cut-daemon` (run 44430413355): green on all three images.
  - On ubuntu-26.04 and ubuntu-26.04-arm, `bound: cuts_abandoned`
    passes: one cut abandoned at the bound, on a `--zero-fill` volume.
  - The new `boundsparse` line passes too: none abandoned on a sparse
    volume, with the holes set dropped after 8192 chunks.
  - Each job ends "worst exit 0".

No rig drives the channel yet, so the rigs show that the commits break
nothing they check, and nothing about the channel itself.

Codex reviewed d65ffb9 (code review and security review, both
completed) and reported no findings. The author answered its earlier
P2, on `server.rs`, on its thread; that P2 is F2, fixed above.
