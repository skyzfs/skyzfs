## Review of #120 at 0b0789a (M2.4d)

`serve --intent-header-sync`: a sync of the intent map's header at each
release of a committed cut and where a committed cut is named. One commit
on `587656d` (`zfs-topology`).

**Verdict: it can merge. I found no defect that blocks it.** The
bookkeeping is right as far as I can break it: a header is synced once
for each time it is written, never beside another sync of the map, and
the drop waits for it. The tests hold what the description says they
hold: 26 of my 35 plants are caught by the pull request's own tests, and
of the 9 that survive, 2 change nothing, 1 removes code that cannot be
reached, and 6 are gaps that my scenarios or a line in a test would
close. Five low findings follow; the first (F1) matters for reading the
benchmark this option exists for, and is a matter of words, not of code.

CI on the head is green in every job that ran (the daemon rig on three
images, the replay through `skyzfsd`, the control rig, `unit-arm64`).
The description says `k3s install smoke` needed a second attempt, for a
failure in SkyNBD's thin restage; the diff touches no `skynbd` file, so
I take that as unrelated, and I did not look into the first attempt.
Codex reviewed 0b0789a and has no findings.

### What I did

- Read the diff against `origin/zfs-topology` and the code around it
  before the description; then the description, against the code.
- Gates at the head with rustc 1.100.0: `cargo fmt --all --check` clean;
  `cargo clippy --workspace --all-targets -- -D warnings` clean plain,
  with `skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `skyzfs-ublk-sys/control-test-program`. Tests, 0 failed:
  `skyzfs-local` 275 in the library and 11 on real files (1 ignored),
  290 with `test-hooks` (1 ignored); `skyzfs-ublk` 155; `skyzfsd` 105
  plain and 141 with `test-hooks`; `skyzfs-upload` 82.
- 35 textual plants, one at a time, each run against the tests of its
  crate with a limit of 300 s (`plants.py`, `plants.out`).
- 8 scenarios of my own: 7 appended to the store's test module for a run
  (`scenarios.rs`) and 1 on real files with the test hooks
  (`scenarios_real.rs`); results in `scenarios.out`. One of the 7,
  `changed_r7`, is written to fail at the head: it shows F3.
- A variant run: every store of the cut tests started with the header's
  sync on (`variant.out`), to see whether the store-level power-cut
  tests still pass with it.
- No kernel here, so nothing of mine drove the daemon through ublk. The
  option is off by default and no rig turns it on, so CI says nothing
  about it either, beyond "off changes nothing".

The worktree of the head was clean after every run.

### The four questions

**(1) The sync bookkeeping. Sound.**
`crates/skyzfs-local/src/inplace.rs`: `sync_header` 3345-3359,
`begin_intent_sync` 2621-2660, `intent_synced` 2688-2751, the drop
909-937.

- *Begun twice?* No. `sync_header` begins a sync only when
  `intent.syncing` is none (3352); otherwise it sets `header_due`. A
  plant that begins one beside the sync under way hangs the pull
  request's test (A03).
- *A header written while a sync is under way.* That sync took its
  picture of the pages before the header was written (the simulated disk
  takes what a sync covers at its begin, so the test does depend on the
  second sync), so it does not cover the header. `header_due` makes
  `intent_synced` begin the next (2740-2749), alone or together with the
  writes that wait, and `begin_intent_sync` clears the mark (2631).
  Plants: not marked due A02, not cleared B01 (hangs: a sync after every
  sync), no next sync for a header alone C01: all caught.
- *At the drop.* The loop goes on while a sync is under way (915), so a
  header's sync that the daemon's stop finds in flight is waited for,
  and then the one that became due while it ran. E01 and E03 (the loop
  as before) are caught. The daemon drops the store on its main thread
  before it exits (`serve.rs`, `joined`), so a clean stop leaves the
  header synced.
- *After a failure.* A header's sync that fails fails the volume, as a
  failed sync of the map on the write path does (B05 caught). On a
  failed volume no sync follows (`!self.failed` at 2743 and at 917), and
  the drop ends: with the guard at 917 taken out the drop spins for
  ever, which only my scenario r3 shows (E04, see F4).
- *After a retry.* A release made again (after a death, or the uploader
  asking twice) writes the header again and syncs again: one sync more,
  nothing lost. Naming a cut that is the base already returns before
  the sync (3260-3290), rightly: nothing was written.
- *With a flush or a freeze.* Neither touches the map's sync state; the
  flush syncs the image, the header's sync the map's file on its own
  helper. The check for a lost sync in `intent_synced`
  (`waiting.begun < intent.begun && …`) is unaffected: every
  `begin_intent_sync` writes all dirty pages and counts in `begun`,
  whatever it was begun for. r6 (40 random programs with the option on)
  finds the newest released cut as the base after a power cut that loses
  everything unsynced, and the next cut holds every block that differs.
- *The wake.* On real files the sync ends on the helper thread, which
  wakes the owner with no request in flight; the next poll counts it
  (r8, ran on real files, not skipped). This is what the daemon's queue
  relies on and what the simulated disk cannot show.

So a header is left unsynced only on a failed volume, or by a power cut
before its sync ends, which costs what the description says: one whole
upload.

**(2) Counting a header's sync apart from `intent_syncs`. Sound.**

- A sync is a header's when no write waits at its begin
  (`syncing_header = intent.waiting.is_empty()`, 2632), and is counted
  once, in one of the two (2694-2700). r6 checks the balance over random
  programs: the disk's syncs of the map equal the two counts' sum. A
  header that is due while writes wait goes out with their sync and is
  counted with the write path's (r2); that is the right side to err on,
  since `intent_syncs` then still means "syncs a write waited for".
- No consumer adds or balances `intent_syncs`: `tests/kernel/*.py`,
  `tests/bench/skz_time.py` and the rigs read `skyzfsd status` keys by
  name, none parses the binary file or its length, and no formula
  relates `intent_syncs` to another figure. New keys after
  `intent_waits` disturb nothing.
- One thing the split hides is F1.

**(3) The tests' power-cut claims. They hold.**
With no sync begun at a release (A06), at a naming (A05), at all (A01),
or for the second header (A02, C01), or with the drop not waiting (E01),
the pull request's tests fail on the base cut they read back after the
cut of power. The option reaching the store and the figures are held
too (F01-F03, D01, D04, D05). What the tests do not hold is listed under
Gaps.

**(4) Status v16. Right.**
`crates/skyzfs-ublk/src/status.rs`: magic `SKYZST16`, 1008 bytes; the
flag at 976, the three figures at 984, 992, 1000. A file of version 15
(976 bytes) is still read, with no header figures and none printed
(S01, S03, S06, S08 caught); the layout test pins the offsets (S02,
S07). That is the cross-version test CONTRIBUTING.md asks for. The
intent map's own format does not change. Two small things: the doc
table's rows are out of order (F5), and "1 on; any other value off" is
written but not pinned (S04 survives: `!= 0` reads the same in every
test).

### Findings

None blocks the merge.

**F1 (low; words, but it matters for the benchmark). "Nothing waits for
it" is not so for a write into a clean region that arrives while the
header's sync is under way.**
`bins/skyzfsd/src/cli.rs:281` (the help), `inplace.rs:3167` and `:3335`,
`inplace/intent.rs:35`, `status.rs:136`, and the description.
The release does not wait, that part is true. But only one sync of the
map runs at a time, so a write whose region is clean waits for the
header's sync to end and then for a sync of its own. Scenario r1: with
the header's sync held on the disk, a write into a clean region does not
complete; let go, it completes after a second sync (header's +1, write
path's +1, two syncs on the disk). The regions a release has just
cleared are exactly the clean ones, so after each release the first
writes into them can wait up to one sync longer than with the option
off. No figure shows this: `intent_sync_ns` starts at the write path's
own sync, and `intent_waits` is a count. Whoever reads M2.4d's benchmark
rows should read tps and latency, not `intent_header_sync_ns` alone, and
the help should say "the release does not wait for it" in place of
"nothing waits for it". Certain.

**F2 (low; the description, and no test). A header's sync that fails at
once does not make `release_cut` return false.**
`inplace.rs:3195-3213` (the comment at 3205-3207), `sync_header`
3345-3359.
Where the map's file has no helper the sync is made at the release, on
the calling thread. If it fails there, `intent_committed` returns false,
and `release_cut` returns true with the cut kept (r5: true, the volume
failed, held cuts still `[(1, Uploading)]`); a naming returns false. The
description says "one that fails as it begins makes the release return
false, as a failed write of the header does". Neither does: the code
answers true with the cut kept for both, and says why in its comment.
The code is consistent and safe (the cut is not let go on a map that may
not be durable; a failure found later, on the helper, has let it go
already, as the pull request's own test shows), so only the description
is off. But nothing tests the case: A07 (the failure at once is not
reported by `sync_header`, so the cut is let go on a failed volume)
survives the pull request's tests and is caught by r5. Certain.

**F3 (low). The drop's new wait applies with the option off too.**
`inplace.rs:913-918`.
`intent.syncing.is_some()` is new in the loop's condition and is not
bound to the option. It differs from before only on a failed store with
a sync of the map under way: the drop now waits for that sync before
`marker.finish()` and before the lock is released. `changed_r7` fails at
the head and passes with the loop as it was (`scenarios_with.out`, E03).
On real files the same wait came anyway, at the helper's own end, but
after the lock was dropped; now it comes before. A map file that hangs
in `fdatasync` on a failed volume therefore holds the lock for as long.
I think this is acceptable; if "off changes nothing" is meant strictly,
`&& !failed` on that term, or the option, confines it. Certain of the
behaviour, unsure whether anyone minds.

**F4 (nit). The drop's term `intent.header_due && !failed` cannot be
reached, and its guard is held by no test.**
`inplace.rs:917`.
A header is due only while a sync is under way, and that sync's end
either begins the next or finds the volume failed; so the loop never
sees `header_due` with no sync under way and no failure. E02 (the term
removed) survives every test and every scenario of mine. The guard
`!failed` matters only if the term stays: without it a failed store with
a header due never ends its drop (E04, caught by r3 alone). Either drop
the term or keep it with a test like r3. The description's "the drop
does not begin a due sync: killed" must have been another edit than E02
(E01 or E03, which are caught).

**F5 (nit). Comments.**
`inplace.rs:993-998`, `:1004-1006`, `:1449` still say a sync of the map
is begun "on the write path"; a header's is not. `status.rs:136-137`:
the rows for 976 and 984 stand before the row for 968.

### Gaps in the tests (none blocks)

- **G1. No test has a write waiting with the option on.** B04 (with the
  option on, every sync is counted as a header's) survives; r1 and r2
  catch it.
- **G2. Nothing checks that the daemon's status carries the store's
  header figures.** D02 (`intent_shown` in `serve.rs` says off) and D03
  (it hands the write path's syncs) survive. A wrong line here would
  misreport the benchmark and nothing else.
- **G3. No test on real files, and none of the wake.** r8 passes.
- **G4. The store-level power-cut tests run with the option off only.**
  With it on they pass (`variant.out`: 82 pass; the 2 that fail count
  syncs or plant a defect that the header's sync in flight turns into a
  failed volume, which is the expected effect of the option and not a
  defect).
- **G5. A failure at once (no helper)** is not tested: F2.
- **S04.** "Any other value off" for the flag at 976 is not pinned.

### Notes for the lead and for testing

- `--intent-header-sync` is refused without `--hold-cuts`
  (`cli.rs:966-972`), also on a volume that uploads, which holds cuts
  anyway. A benchmark entry needs both:
  `TB_SKYZFSD_ARGS=--hold-cuts,--intent-header-sync`. The hooks'
  `release` command goes through `release_cut`, so the releaser's rows
  take the header's sync too.
- With no helper for the map's file the header's sync is made on the
  queue's thread, at the release (r4). That is the existing fallback
  for the map's syncs and not new, but it is the one case where the
  release does wait.
- `wait()` with a timeout now also blocks on a header's sync
  (`inplace.rs:2092-2102`). The daemon only polls, so this touches tests
  alone.
- A failed sync of the header fails the volume. That is the pull
  request's choice and the same as for any sync of the map; it means the
  option adds a way for a volume to fail that a volume without it does
  not have.

### Plants

`plants.out` has every line. KILLED 26, SURVIVED 9, NOT APPLIED 0.

| Plant | What it changes | Result |
|---|---|---|
| A01 | `sync_header` begins no sync | KILLED (4 tests) |
| A02 | a header written while a sync is under way is not marked due | KILLED (1) |
| A03 | a header's sync is begun beside the one under way | KILLED (hangs, cut at the limit) |
| A04 | the header is synced whatever the option says | KILLED (4) |
| A05 | `name_committed_cut` begins no sync | KILLED (1) |
| A06 | a release begins no sync | KILLED (3) |
| A07 | a failure at once is not reported by `sync_header` | SURVIVED; r5 catches it (F2) |
| B01 | a sync that begins does not clear the header due | KILLED (hangs, cut at the limit) |
| B02 | a header's sync is counted with the write path's | KILLED (3) |
| B03 | every sync is counted as a header's | KILLED (6) |
| B04 | with the option on, every sync is counted as a header's | SURVIVED; r1, r2 catch it (G1) |
| B05 | a header's sync that fails does not fail the volume | KILLED (1) |
| B06 | the mark of a header's sync is not taken at its end | SURVIVED; no difference (set again at each begin) |
| C01 | a sync that ends begins none for a header due alone | KILLED (1) |
| C02 | the header due is looked at whether or not a sync is under way | SURVIVED; no difference (none is) |
| E01 | the drop does not wait for a sync under way with no write waiting | KILLED (1) |
| E02 | the drop does not look at a header due | SURVIVED everything (F4: unreachable) |
| E03 | the drop's loop as before the pull request | KILLED (1) |
| E04 | the drop goes on for a header due on a failed volume | SURVIVED; r3 catches it (F4) |
| F01 | the option does not reach the store | KILLED (4) |
| F02 | the figures say the option is off | KILLED (1) |
| F03 | the figures give the write path's syncs as the header's | KILLED (3) |
| S01 | the header's sync is read from a file of version 15 | KILLED (2) |
| S02 | count and summed time in each other's place | KILLED (4) |
| S03 | a file of version 15 is not read | KILLED (2) |
| S04 | any value but 0 reads as on | SURVIVED (not pinned) |
| S05 | status always prints `intent_header_sync on` | KILLED (1) |
| S06 | status prints the header's keys for a file of any version | KILLED (1) |
| S07 | the file says on where the option is off | KILLED (4) |
| S08 | a file of version 15 is taken to be 984 bytes | KILLED (2) |
| D01 | serve does not hand the option to the store | KILLED (1) |
| D02 | the daemon's status says off whatever the store says | SURVIVED (G2) |
| D03 | the daemon's status gives the write path's syncs as the header's | SURVIVED (G2) |
| D04 | `--intent-header-sync` is taken without `--hold-cuts` | KILLED (1) |
| D05 | the option is on wherever cuts are held | KILLED (3) |

### Scenarios

| Scenario | What it shows | At the head |
|---|---|---|
| r1 | a write behind a header's sync waits for it, then for a sync of its own; each counted on its side; after a power cut the base and the region are named | passes |
| r2 | a header due while a write waits goes out with the write's sync and is counted with the write path's, by polls and by the drop | passes |
| r3 | a header due on a failed volume does not hold the drop | passes |
| r4 | with no helper the header's sync is made at the release | passes |
| r5 | a header's sync that fails at once: the release returns true with the cut kept and the volume failed; a naming returns false | passes |
| r6 | 40 random programs with the option on: every sync of the map counted once, the newest released cut is the base after a power cut, the next cut holds every block that differs | passes |
| changed_r7 | option off: the drop of a failed store waits for the map's sync under way | fails, as written (F3); passes with E03 |
| r8 | on real files the end of a header's sync wakes the owner, and the next poll counts it | passes |

To run them again: `plants.py WORKTREE TARGET_DIR scenarios` in a clean
worktree of the head; `plants` for the table above; `variant` for the
run with the option on everywhere; `scenarios-with E03` for F3.
