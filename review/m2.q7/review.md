## Review of #133 at d040d95 (Q7, F-A and F-B of #124, the follow-ups of #131)

**Verdict: can merge. Nothing blocks.** I found no defect in the code. Q7 does what the lead decided, on every path I could find, and nothing turns the sync off for a volume that uploads unless `--hold-cuts` is given as well. The answer to F-A is sound: the sync a release begins on a failed store cannot outlive the store, write after its drop, leak its thread or harm a take-over. The G2 and G3 tests hold the plants I asked them to hold, U8 and U9 included, and I could not make them flake.

What I have is non-blocking: one gap in the tests that bears on F-A (N1), a gap in the uploader's tests now that the default has changed (N2), a bound in the G2 test that does not hold the pause's cap (N3), and text that is stale in the description, in the help and in files this pull request does not own (N4 to N6).

The head had not moved when I began or when I ended: `head.sha` is d040d95, base e155dbf. I read the diff against e155dbf first, then my reviews of #124 and #131 and the author's answer on #131, then the description. Codex's code review and security review of d040d95 are complete, with no finding and no comment on the pull request, so there is nothing of Codex's to judge. The description was edited today (CI filled in); the head was not.

### Blocking findings

None.

### What I was asked to judge

#### 1. The conservative reading of Q7 holds on every path

- **The parse** (`bins/skyzfsd/src/cli.rs:1081-1088`). `--no-intent-header-sync` is a flag (`cli.rs:755`): a value after it, or the flag given twice, is refused by the strict `Given::collect`. Without `--hold-cuts` it is refused as a usage error ("only with --hold-cuts"), whether or not the volume uploads. `intent_header_sync = !no_header_sync`.
- **The one place the daemon builds the store's options** is `hold_cuts_of` (`bins/skyzfsd/src/serve.rs:645-658`). It is used where `serve.hold_cuts || uploads`, and passes `serve.intent_header_sync` unchanged. No other `OpenOptions` is built in `bins/`. No config file, environment variable or control command sets the field. The tests at `serve.rs:2491-2496` check both an uploading volume and `--hold-cuts`.
- **So a volume that uploads syncs the header unless `--hold-cuts --no-intent-header-sync` are both given**, which is the lead's decision.
- **The old option `--intent-header-sync` is now unknown.** A start given it fails with a usage error, exit 2. This is deliberate and in the description. The consequence for scripts is N6.
- **The status field** `intent_header_sync` (offset 976) reads 1 only where an intent map exists. It reads 0 where no cuts are held (`intent_shown`, `serve.rs:605-610`). See N4(b).
- **Plants.** Q1 to Q4 (the author's) are killed, and so are my Q6 (a volume that uploads never syncs) and Q8 (the flag ignored where `--hold-cuts` is given). Q9 survives; that is N5.

#### 2. The G3 tests hold U8 and U9

- U9 (the author's plant, re-planted from the description's table) is killed by `memory_store_renews_in_one_critical_section` (`crates/skynbd-refs/src/lib.rs:1409`, 300 rounds of `race_lease_renew_against_take`). It failed in the suite run and in 10 of 10 runs of the race tests alone.
- U8 is killed by both `redb_store_renews_in_one_write_transaction` (`crates/skynbd-refs/src/redb_store.rs:440`) and the fixed-order `a_take_committed_while_the_renewal_waits_is_seen_by_it` (`redb_store.rs:465`). It failed in the suite and in 10 of 10 runs alone.
- Two variants of my own:
  - **U9c**: the memory store judges under one write lock, drops it and writes under a second. Killed 10 of 10. tokio's `RwLock` is fair, so the window is wide.
  - **U8c**: redb judges in one write transaction, commits, and writes in a second. Only the race test catches it: in the suite run and 1 of 10 runs alone (2 of 11). The fixed-order test cannot see U8c by construction. Its take is committed before the renewal's first transaction begins, so the judge already sees it. Low risk: redb has one process, and such a split would be an odd regression. I suggest nothing beyond noting it.
- **Flakiness.** At the head I ran the three race tests 30 times idle and 15 times beside four busy loops: 45 of 45 passed. In the same loaded runs, the two F-A tests passed 15 of 15 (`flake.out`). The connect tests (`control::tests::socket::`) passed 16 of 16, 10 times under load (`socket_loaded.out`).

#### 3. The bound `(2..=13)` in `a_full_backlog_is_tried_again_after_pauses_that_grow`

- **The bound is safe but loose.** At 300 ms the nominal count is exactly 12, and 12 is also the most possible.
  - Tries 1 to 7 are separated by 1, 2, 4, 8, 16 and 32 ms, so try 7 comes at about 63 ms.
  - Four pauses of 50 ms put try 11 at about 263 ms. The last pause is cut to what is left (`control.rs:738`), so try 12 comes at 300 ms and the loop ends.
  - `std::thread::sleep` never returns early, so a late wake can only lower the count.
  - Measured: 12 in 20 of 20 runs idle and 20 of 20 under load (`tries_head.out`, `tries_head_loaded.out`).
  - 13 is unreachable. The lower bound 2 fails only if the thread stalls for the whole 300 ms after its first try, so the test is not flaky.
- **The comment at `bins/skyzfsd/src/control/tests.rs:446`** says "13 tries at most"; the true maximum is 12. The description says "allows 12", which is right.
- **It catches CB and CD, as claimed.** It does not hold the cap of 50 ms. See N3.

#### 4. F-A: a release on a failed store begins a header sync that the drop does not wait for

What happens, on real files:

- `release_cut` runs on a failed store. `intent_committed` writes the header naming the released cut on the caller's thread, and clears the bits of the regions with no change since the cut.
- `sync_header` (`crates/skyzfs-local/src/inplace.rs:3445`) then begins a sync of the map. That sync is one `fdatasync` handed to the map file's `SyncHelper` thread (`crates/skyzfs-local/src/fs.rs:363-415`). The release answers true.
- In `Drop for InPlace` (`inplace.rs:902`), `for_map` is false on a failed store with no write waiting (`inplace.rs:928-929`), so the loop does not wait for that sync.
- `marker.finish()` then waits for the failure marker.
- The fields drop in declaration order: `lock` (`inplace.rs:615`) before `intent` (`inplace.rs:677`). The map's file drops with `intent`, and `SyncHelper::drop` sets its jobs to `None` and joins the thread (`fs.rs:406-412`).

Against each question:

- **Can the sync outlive the store?** No. The store's drop does not return until the helper thread has joined, and the thread ends only after `sync_data` returns. The sync outlives the marker and the lock, by the length of one `fdatasync` of a small file, and nothing else.
- **Can it write after the drop?** No. The helper writes nothing; it calls `sync_data` on a file whose header was written before the drop began. Nothing of the store's writes after its drop returns.
- **Can it race a take-over?** It can overlap one, harmlessly. Once the lock is released, another process may open the volume while the old helper's `fdatasync` is still running.
  - The failure marker is already done, so the new start sees the failure.
  - The header and bits the new start reads are the ones the old store wrote before its drop, from the same page cache.
  - An `fdatasync` from another process on the same inode can only make data durable sooner. It cannot undo or reorder the new start's writes, and the new start syncs what it takes up before trusting it.
- **Can it leak a thread?** No: the helper is joined.
- **What the header says is conservative.** `note_change` (`inplace.rs:2919`) records a change in the cut state before a write begins, the writes that then fail included. So the release does not clear the bit of a region written after the cut.
- **One corner, low.** If the disk hangs `fdatasync` of the map, the drop hangs at the join, after the lock. The marker's own sync on the same disk comes first and would hang before it, so this adds no new way to hang in practice.

What pins it:

- `a_release_on_a_failed_store_begins_a_sync_the_drop_does_not_wait_for` (`crates/skyzfs-local/src/inplace/tests/cuts.rs:5356`) kills FA (no sync begun on a failed store) and my N6 (the drop loop waits for it).
- `the_drop_of_a_failed_store_does_not_wait_for_a_headers_sync_under_way` (`cuts.rs:5330`) kills N6.
- So the mechanism is pinned. The purpose is not: that a start that acknowledges the failure, in the next boot, takes the map up naming the released cut. N1 has the details and a test that would pin it.

### Non-blocking

**N1. The purpose of keeping the sync is not pinned, and the simulated disk cannot show it as written (test gap; sure).**

- No test runs the whole sequence: a release on a failed store, the drop, a power cut, then an acknowledging start.
- I wrote it as scenario s1 (`scenarios_cuts.rs`). With the sync on, it fails at the head: the start finds no committed cut.
- This is the simulator, not the product. `Drop for SimFile` (`crates/skyzfs-local/src/sim.rs:1590-1598`) ends a pending sync with `sync_over`, which only does bookkeeping and makes nothing durable. The real `SyncHelper` joins after the `fdatasync` has been made.
- s3 (the sync held across the drop, then let go) fails for the same reason.
- **s1b** puts one `poll` between the release and the drop, which collects the sync's end. It passes at the head and fails with FA planted (`scn_cuts2.out`, `scn_s1b_fa.out`). It is the end-to-end test of F-A's purpose, and I suggest adding it.
- **s1c** (the release on a healthy store whose sync ends, then the failure) passes. The acknowledging start takes the map up naming the cut, which is the claim the answer to F-A rests on.
- **A wider fix, the author's call.** Make `SimFile`'s drop complete a pending, not held, sync as the real helper's join does; s1 would then hold as written. It touches a simulator many tests use.
- **Pre-existing since M2.4d.** No test on real files pins the join itself, that the drop of the map's file waits for the helper's `fdatasync`. `tests/inplace_real.rs` would be the place.

**N2. The uploader's tests run the setting that is no longer the default (test gap; sure).**

- `crates/skyzfs-upload/tests/common/mod.rs:153` builds its store with `intent_header_sync: false`. The uploader's tests therefore drive the non-default path: release, then no header sync.
- I flipped it to `true`, and all 167 uploader tests pass (`upload_synced.out`).
- I suggest flipping it, or running both. A release from the uploader with the sync on is what every uploading volume now does.
- `crates/skyzfs-local/src/inplace/harness.rs:146` is also false, but that harness never lets a cut go, so it does not matter there.

**N3. The G2 test does not hold the pause's cap (test gap; sure).** Plants that survive it:

- **CE**, the pause keeps doubling past 50 ms: 10 tries at 300 ms.
- **CF**, the pause starts at 50 ms: 7 tries.
- **CI**, the last pause is not cut to what is left: up to 50 ms over the limit, harmless.

CE is the one that matters:

- `ask` waits up to `ASK_LIMIT` (40 s). Uncapped, the pause would grow to about 16 s, so a client could sit about 16 s after the daemon's backlog has room.
- My scenario fills the backlog and makes room 1.5 s into a wait of 5 s (`scenario_tries.rs`).
  - At the head the connect is made 18 ms after room is made (51 ms under load).
  - With CE, 548 ms after. With CF, 2.5 ms after (`tries_*.out`).
- **Suggestion:** a test that makes room part-way through the wait and asserts a connection within about 200 ms of it. That has wide margin both ways and kills CE. Or tighten the lower bound to 11 at 300 ms, which kills CE and CF but has less margin under load.
- Also fix the comment's "13" to 12.

**N4. The description (stale or imprecise; sure).**

- **(a) The rigs that drive the default are misnamed.** The description says the daemon rig and the replay rig drive the default now, and that these are their first runs with the sync on. Neither ever begins a header sync.
  - The replay rig serves without `--hold-cuts` (`tests/kernel/replay_daemon.py`), so it has no intent map.
  - The daemon rig holds cuts only in g.4, the refused start (`tests/kernel/daemon-rig.sh:2373-2489`), where no cut is let go.
  - The rig that does drive it is the cut rig. Under `--hold-cuts`, `tests/kernel/cut_hold.py` releases cuts; in the uploading job, the uploader does.
  - Its artifact `cut-daemon-ubuntu-26.04-upload` (run 44476642634) shows `intent_header_sync on`, with up to 38 header syncs in a status and the longest about 1.7 ms.
  - So the evidence exists; the description names the wrong rigs.
- **(b)** "`intent_header_sync` now reads 1 unless the option was given" holds only where cuts are held. Elsewhere it reads 0. The status documentation's row at offset 976 (`crates/skyzfs-ublk/src/status.rs:162`) could say so too.
- **(c) The N6 row names three tests.** In my run, my N6 was caught by two of them: `the_drop_of_a_failed_store_does_not_wait_for_the_maps_sync_under_way` passed. My plant adds `|| intent.syncing_header` to the drop's condition; the author's was likely wider. So this is the plant's text, not a defect. The row could say which plant it was.
- **(d)** "CI to be added", which I noted earlier, is gone: today's edit lists the runs.

**N5. The help (stale; sure).**

- `bins/skyzfsd/src/cli.rs:272-273` still says "Nothing lets a held cut go yet but the test hooks: it is off unless given." The uploader lets held cuts go. The text is older than this pull request, but it sits in the paragraph this pull request rewrote for the review's nit.
- The help's test only checks `HELP.contains("--no-intent-header-sync")` (`bins/skyzfsd/src/cli/tests.rs:453`). The usage lines inside `HELP` already satisfy that, so the paragraph is held by nothing: Q9 (the paragraph names the old option) survives.
- An assertion that `HELP` names `--intent-header-sync` nowhere but inside `--no-intent-header-sync` would kill Q9.

**N6. Text outside this pull request names the old option (stale; sure; not the author's to fix).**

- `CLAUDE.md:378` (the lead's) describes `serve --intent-header-sync` as off by default. It also says a benchmark entry passes `--hold-cuts,--intent-header-sync`, which this head refuses at the start, with exit 2.
- `tests/bench/topology-bench.sh:738,749` and `tests/bench/test_embedded_python.py:159` (testing's) name the old option.
- Bench rows that pass `--hold-cuts` alone now run with the sync on. Rows labelled the same way before and after this merge are not the same configuration, and the notes should say so when they are compared.

**Nit.** `crates/skynbd-refs/src/lib.rs:283` was not reflowed after the edit: "expired or released, in one conditional write ([`lease::renew`]). Never takes the" runs past its neighbours. rustfmt does not reflow comments.

### The follow-ups of #131

- **S1** is done. The doc of `LeaseState::Lost` in `crates/skyzfs-upload/src/lease.rs` is accurate: `NotHeld(None)` maps to an empty holder and epoch 0.
- **S2** is done: the renewal rule's docs say "live, expired or released".
- **S3** is done: #131's description is corrected.
- **S4** is left open as a low item, as the lead decided.
- **G1.** `lease_conformance_over_live_s3` (`crates/skynbd-refs/tests/object_refs_s3.rs:148`) runs in CI's `s3-backends` job and passed. That job's log cannot tell a run from a skip: the skip's message goes to stderr, which the harness captures. Locally, over moto 5.2.5 with `--nocapture`, it ran with no skip and passed (`s3_head.out`). With #131's O1 planted (the object store renews without looking at the row), it fails (`s3_O1.out`). The skip on a failed probe is older than this pull request.
- **G2 and G3** are above.

### Codex

Code review and security review are both completed on d040d95. They have no findings, and there are no review threads or comments. Nothing to judge.

### CI on d040d95

Every run is green:

| Run | Id |
|---|---|
| ci (lint, test, unit-arm64, s3-backends, chart-lint, changes) | 44476642709 |
| skyzfs-ci | 44476559918 |
| coverage | 44476642900 |
| e2e | 44476642968 |
| k3s-smoke | 44476642795 |
| the daemon rig | 44476642617 |
| the replay rig | 44476642856 |
| the cut rig, whose jobs include `ubuntu-26.04-upload` | 44476642634 |
| node B | 44476642943 |
| the control rig | 44476642669 |

### Gates run locally (`gates.sh`, `gates.out`)

After `rustup update stable` (rustc 1.100.0):

- **fmt:** clean.
- **clippy `-D warnings`:** clean on the workspace, on `skyzfsd --features test-hooks` and on `skyzfs-ublk-sys --features control-test-program`.
- **Tests at the head** (CI's `test` job ran the whole workspace):
  - `skynbd-refs`, `skyzfs-upload`, `skyzfsd` and `skyzfs-ublk`: 514 passed plain (`t_plain.out`), and 553 with `skyzfsd/test-hooks` (`t_hooks.out`).
  - `skyzfs-local`: 311 + 11 passed plain, 1 ignored (`t_local.out`); 311 + 16 passed with `test-hooks`, 1 ignored (`t_local_hooks.out`).
  - `skyzfs-ublk-sys --features control-test-program`: 37 passed; 11 ignored, which are the kernel tests (`t_ublksys.out`).

### Plants (`plants.py`, `plants.out`)

Each plant was put in one at a time in a clean worktree of d040d95 and taken out again. The worktree was clean after the run. Plants on a race were also run 10 times on the race tests alone.

| Plant | What | Result |
|---|---|---|
| Q1 (author) | the header sync is off unless asked for | KILLED: `serve_has_the_agreed_defaults_and_options`, `the_header_sync_is_on_unless_turned_off_with_held_cuts`, `the_intent_region_reaches_the_store` |
| Q2 (author) | `--no-intent-header-sync` taken without `--hold-cuts` | KILLED: `the_header_sync_is_on_unless_turned_off_with_held_cuts` |
| Q3 (author) | the old option still taken and ignored | KILLED: same |
| Q4 (author) | the daemon does not tell the store to sync | KILLED: `the_intent_region_reaches_the_store` |
| N6 (mine, as in #124's review) | a failed store's drop waits for a header's sync under way | KILLED: `a_release_on_a_failed_store_begins_a_sync_the_drop_does_not_wait_for`, `the_drop_of_a_failed_store_does_not_wait_for_a_headers_sync_under_way` |
| FA (author) | a release on a failed store begins no sync | KILLED: `a_release_on_a_failed_store_begins_a_sync_the_drop_does_not_wait_for` |
| CA (author) | a refused or missing socket tried again until the limit | KILLED: `no_daemon_at_the_socket_or_no_socket_fails_the_ask_at_once` |
| CB (author) | no pause between tries | KILLED: `a_full_backlog_is_tried_again_after_pauses_that_grow` |
| CD (author) | the pause does not grow | KILLED: same |
| U9 (author) | memory store: read under a read lock, write under another | KILLED; alone 10 of 10 |
| U8 (author) | redb reads the row outside its write transaction | KILLED by both redb tests; alone 10 of 10 each |
| U17 (author) | the acting store renews at its own epoch | KILLED: `a_renewal_passes_through_at_the_epoch_asked` |
| Q6 | a volume that uploads never syncs | KILLED: `the_intent_region_reaches_the_store` |
| Q8 | the flag ignored where `--hold-cuts` is given | KILLED: `the_header_sync_is_on_unless_turned_off_with_held_cuts`, `the_intent_region_reaches_the_store` |
| Q9 | the help's paragraph names the old option | SURVIVED (N5) |
| CE | the pause is not capped at 50 ms | SURVIVED (N3) |
| CF | the pause starts at 50 ms | SURVIVED (N3) |
| CI | the last pause is not cut to what is left | SURVIVED (harmless) |
| U9c | memory store: judge under one write lock, write under a second | KILLED; alone 10 of 10 |
| U8c | redb: judge in one write transaction, write in a second | KILLED by the race test only; alone 1 of 10 |

### Scenarios (not for merging; appended for a run and taken out)

| Scenario | At the head | With FA |
|---|---|---|
| s1: release on a failed store, drop, power cut, acknowledging start | sync off: no cut, as expected; sync on: no cut (simulator, N1) | no cut |
| s1b: s1 with one poll before the drop | the cut is named and taken up | no cut (killed) |
| s1c: release and sync on a healthy store, then failure, power cut, acknowledging start | the cut is named and taken up | (not run) |
| s2: the header's sync fails on a failed store | the release answers true, the drop ends | passes |
| s3: the sync held across the drop, then let go | no cut (simulator, N1) | no cut |
| tries: full backlog, 20 runs each | 12 at 300 ms, 26 at 1 s (25 to 26 loaded); room made 1.5 s in: connected 18 ms after (51 ms loaded) | CE: 10, 11, 548 ms; CF: 7, 21, 2.5 ms |
| uploader's tests with the sync on | 167 passed | (not run) |
| live S3 over moto | 3 of 3 ran and passed | O1: `lease_conformance_over_live_s3` fails |

### Files here

- `review.md`: this note.
- `plants.py`, `plants.out`.
- `scenarios_cuts.rs`, `side_cuts.sh`, `scn_cuts.out`, `scn_cuts_fa.out`, `scn_cuts2.out`, `scn_s1b_fa.out`.
- `scenario_tries.rs`, `side2.sh`, `tries_head.out`, `tries_head_loaded.out`, `tries_CE.out`, `tries_CF.out`, `socket_loaded.out`, `s3_head.out`, `s3_O1.out`.
- `upload_synced.out`.
- `flake.sh`, `flake.out`.
- `gates.sh`, `gates.out`, `t_plain.out`, `t_hooks.out`, `t_local.out`, `t_local_hooks.out`, `t_ublksys.out`.

Paths in the outputs are shortened to `<scratch>` and `<repo>`.
