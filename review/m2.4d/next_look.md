## Next look at M2.4d: #124 at 339cc03 (M2.4d-1)

The answers to the review of #120 (`review.md` beside this file): F1 to
F5 and the gaps G1 to G5 and S04. One commit, `339cc03`, on `ec8c91b`.
The head had not moved when I began or when I ended (the API's
`head.sha`). `zfs-topology` has moved on to `75e8425` since, with notes
and the cut rig's files only (no crate, no binary), and the pull request
merges onto it without a conflict, so CI's verdict on the merge holds.

**Verdict: it can merge. I found no defect that blocks it.** Each of the
five findings is answered as the description says, and each scenario of
mine is now a test that does what mine did. The drop's wait is confined
as asked, and taking out F4's term loses nothing. Two low findings
follow, both about the failed volume that F3 is about; neither changes
what a healthy volume does.

CI on 339cc03 is green in every run, all at their first attempt:
`skyzfs-ci` (push), `ci` (with `unit-arm64`), `coverage`, `e2e`,
`k3s-smoke`, the control rig, the daemon rig (`skyzfsd` on 24.04, 26.04
and 26.04-arm), the replay rig's jobs through `skyzfsd` (the reference
jobs are skipped, as for a pull request that touches no rig file) and
the cut rig on the daemon. No rig turns the option on, so CI says that
"off changes nothing", and the daemon's drop of a failed volume is only
what the store's tests show. Codex reviewed 339cc03 and has no findings.

### What I did

- Read the diff against `origin/zfs-topology`, then the description and
  the answer on #120's thread against the code.
- Gates at the head with rustc 1.100.0 (`rustup update stable`: current):
  `cargo fmt --all --check` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean plain, with
  `skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `skyzfs-ublk-sys/control-test-program`. Tests, 0 failed in each:
  `skyzfs-local` 293 passed plain and 298 with `test-hooks` (1 ignored
  each); `skyzfs-ublk` 156; `skyzfsd` 106 plain and 142 with
  `test-hooks`; `skyzfs-upload` 82. r8's test, run alone with
  `--nocapture`, printed no SKIPPED: it ran on real files here.
- 40 textual plants, one at a time, against the tests of
  their crate (`skyzfs-local` with the test hooks, so the tests on real
  files run too; `skyzfs-ublk`; `skyzfsd` with the test hooks), each with
  a limit of 420 s and the file restored after; for each one that
  survives and touches the drop or the sync's bookkeeping, my scenarios
  as well. The first look's plants were run again where their text is
  still there, so the description's table is checked plant by plant.
  Only this note is committed; the table below says what each plant
  changes, in the terms of the first look's `plants.py`.
- Six scenarios of my own (n1 to n6, the appendix), appended to the
  store's cut tests for a run and taken out again. All pass at the head.
- No kernel here.

The worktree of the head was clean after every run.

### The four questions

**(1) F3: the drop's wait confined to a volume that has not failed.
Right, and "off changes nothing" now holds strictly.**
`crates/skyzfs-local/src/inplace.rs:910-922`.

The condition is now `!waiting.is_empty() || (syncing.is_some() &&
!failed)`. On a failed volume it is `!waiting.is_empty()`, the condition
before #120, option on or off. On a healthy volume the new term matters
only where a sync of the map is under way with no write waiting, and
with the option off that state does not arise: every sync is begun by a
write that waits (`wait_for_intent`), by the end of a sync that leaves
writes waiting (`intent_synced`), or by the drop for writes that wait;
only `sync_header` begins one with none, and only with the option on;
and a write leaves `waiting` only at the sync's end or at a failure. n6
checks this over 40 random programs with the option off, polled after
every step: wherever nothing is in flight, no sync of the map is under
way. N3 and N5 (the term bound to the option, or to a header's sync)
change nothing, as expected.

The scenarios asked for, a failed store with a sync of the map under way:

| Under way | Option | The drop | Test |
|---|---|---|---|
| the write path's (a write waited and was failed) | off | ends at once, as before #120 | the pull request's (`the_drop_of_a_failed_store_does_not_wait_for_the_maps_sync_under_way`, was my `changed_r7`) |
| the write path's | on | ends at once | the same test, its second case |
| a header's (the release came while healthy) | on | ends at once | n1 |
| a header's begun by a release that came to the failed store | on | ends at once | n3 (F-A below) |
| a header's, another header due, the volume fails for another reason | on | ends at once, and no sync follows for the header due | n4 |
| a header's, another due, the sync fails inside the drop | on | waits (healthy when it began), fails the volume there, begins no second sync, ends | n5 |
| a header's, on a healthy volume | on | waits for it, then ends; a power cut after keeps the base | n2, and the pull request's tests of #120 |

Where the drop no longer waits, the sync ends on the map's file's
helper, which the drop of the `Intent` field joins (`SyncHelper`'s drop,
`crates/skyzfs-local/src/fs.rs:406-415`): after `marker.finish()` and after the lock, since
`lock` is declared before `intent` in `InPlace` and is dropped first.
That is the order before #120. Nothing is lost by it. The drop's wait
never made the sync end sooner: it ends when the disk ends it, and the
join still comes before the store is gone, so the daemon does not exit
before it either. What changes is only that the marker is written, and
the lock let go, before that sync ends rather than after; a start that
takes the lock meanwhile reads the pages as written, and a power cut
before the sync ends costs the same whole upload either way.

**(2) F4: the term `header_due && !failed` taken out. Nothing is lost,
and no sync is begun twice.**

A header is marked due only in `sync_header`, only while a sync is under
way (`inplace.rs:3371-3374`). That sync's end (`intent_synced`,
`:2745-2754`) begins the next where one is due and none is under way, on
a healthy volume; and every begin clears the mark (`:2636`), including
one begun by a write resumed at that end, which then covers the header
(r2, now `a_header_due_while_a_write_waits_is_covered_by_the_writes_sync`).
So on a healthy volume a header due with no sync under way does not
outlive the call that ends the sync, and the drop, which waits for the
sync under way, sees the next one begun. On a failed volume the mark can
stay (n4, r3), and nothing looks at it. Plants: the mark not set (A02),
not cleared (B01, hangs), no next sync for a header alone (C01), the
drop's loop as before #120 (E03): all caught. The term put back without
its guard (N4) fails the pull request's
`a_header_due_on_a_failed_volume_does_not_hold_the_drop`: the drop does
not end.

Begun twice: `sync_header` begins one only where none is under way
(A03 hangs a test), `intent_synced` only where none is (C02 survives:
no difference, as at the first look), `wait_for_intent` only where none
is, and the drop only where none is (`intent_syncing()` first). The
removal takes a path away and adds none.

**(3) The ported scenarios test what mine did.**
`cuts.rs` r1 to r6 are my scenarios with new names and `cargo fmt`; I
diffed them line by line. `changed_r7` became
`the_drop_of_a_failed_store_does_not_wait_for_the_maps_sync_under_way`,
turned the right way round (it fails with #120's condition: N1) and with
a second case with the option on; in that case the sync under way is
still the write path's, not a header's (F-B below). r8 in
`tests/inplace_real.rs` is mine but for the line it printed; it ran here
(the file system keeps a sync) and was not skipped. `intent_shown_of`
and its test, and `the_header_sync_flag_is_on_only_at_one`, are new.
Every plant of the first look that survived #120's tests and could be
caught is caught now: A07 by r5, B04 by r1 and r2, D02 and D03 by
`the_status_carries_the_stores_header_syncs_apart_from_the_write_paths`,
S04 by the flag test, E04 (now N4) by r3. The description's plant table
is right in every row I ran.

**(4) F1, F2, F5.**

- *F1.* The help (`bins/skyzfsd/src/cli.rs:279-285`), `release_cut`'s doc
  (`inplace.rs:3172-3174`), `sync_header`'s (`:3341-3363`), the intent
  map's module doc (`inplace/intent.rs:34-38`) and the status table's
  row for 976 (`crates/skyzfs-ublk/src/status.rs:137`) now say that the
  release does not wait and that a write into a clean region does, for
  that sync and then for one of its own; `sync_header`'s says that no
  figure counts it, and names the no-helper case where the release
  waits. Accurate. "Nothing waits" remains only in the notes' review
  row for #120, which is the lead's and is qualified there by "F1:
  writes into just-released regions do wait".
- *F2.* `sync_header`'s doc says what each caller answers on a failure
  at once; r4 and r5 are tests; A07 is caught. Right.
- *F5.* The fields `syncing` and `begun` and `intent_syncing` say "on the
  write path or for a header"; the other uses of "write path" in the
  file (`syncs` and `IntentFigures::syncs`, `:997` and `:1081`) mean
  the write path's own syncs and are right. The status table's rows are
  in the order 840, 968, 976, 984.

### Findings

None blocks the merge.

**F-A (low; a comment, and a behaviour to know before Q7 turns the option
on by default). "On a failed volume nothing is synced any more" is not so:
a release or a naming that comes to a failed store begins a header's
sync, and the drop now lets it run past the lock.**
`crates/skyzfs-local/src/inplace.rs:916-917` (the drop's comment),
`sync_header` `:3364-3378`.

`sync_header` does not look at `failed` (its `failed || !self.failed`
answers true for a volume that had failed before the call, so the case
was meant), and neither do `release_cut` and `intent_committed`. In the
daemon the uploader's calls are answered at every turn of the queue's
thread until the queue ends (`serve.rs:1183-1189`), the turns after a
failure included, so an upload that commits between a failure and the
queue's end releases its cut on the failed store. With the option on
that writes the header and begins a sync of the map with no write
waiting. n3: the cut is let go, one sync begins, and the drop ends at
once with it held on the disk. At #120's head the drop waited for it;
now it ends on the helper after the marker and the lock, as in (1). I
found nothing lost by that (the sync ends no later than it did, and the
daemon does not exit before it), so this is not a defect. But the
comment is wrong in the one case, and since the lead's Q7 makes the
option the default, that case becomes the default path. Either say so
in the comment ("nothing is synced for a write; a release that comes to
a failed store still begins a header's sync, which the helper's end
waits for, after the lock"), or have `sync_header` begin no sync on a
failed volume, as `intent_synced` begins no next one there. The first
keeps what the option is for on the one release that can still name a
committed cut after a failure (the next start acknowledges the failure
and takes the map up, so its base still decides delta or whole); the
second makes the comment true at that cost. I would take the first.
Certain of the behaviour; which way is the author's.

**F-B (low; a test). No test has a header's sync under way when the
store fails; the pull request's test of F3 has the write path's sync
under way in both its cases.**
`crates/skyzfs-local/src/inplace/tests/cuts.rs`, the last test.
In its case with the option on, the cut is released and its header's
sync ended before the write, so the sync the disk holds at the failure
is the write's. N6 (a failed store's drop still waits for a header's
sync under way, not for the write path's) survives every test of the
pull request; n1, n3 and n4 catch it. n1 is the test that closes it: a
release with the disk holding the header's sync, the store failed, the
drop ends at once. Worth adding if the code is touched for F-A.

**Nit.** The help's paragraph was not reflowed: "One that fails fails /
the volume. The / status says which (intent_header_sync)." leaves a
short line and a "The" alone at its end (`cli.rs:283-285`).

**Nit.** `intent_shown` (`serve.rs:600-605`), the two lines around the new
pure function, is held by no test: with it showing no intent map
whatever the store has (D06) every test passes. The function is what
can go wrong and it is tested; I would leave it.

### Plants

KILLED 34, SURVIVED 6, NOT APPLIED 0, of 40. A plant marked "no
difference" changes nothing by design and survives my scenarios too.
"Hangs" is a run cut at the limit with a test of the pull request's
still running.

| Plant | What it changes | Result |
|---|---|---|
| A01 | `sync_header` begins no sync | KILLED (12 tests) |
| A02 | a header written while a sync is under way is not marked due | KILLED (1) |
| A03 | a header's sync is begun beside the one under way | KILLED (hangs) |
| A04 | the header is synced whatever the option says | KILLED (4) |
| A05 | `name_committed_cut` begins no sync | KILLED (2) |
| A06 | a release begins no sync | KILLED (11) |
| A07 | a failure at once is not reported by `sync_header` | KILLED (1: r5's test) |
| B01 | a sync that begins does not clear the header due | KILLED (hangs) |
| B02 | a header's sync is counted with the write path's | KILLED (8) |
| B03 | every sync is counted as a header's | KILLED (8) |
| B04 | with the option on, every sync is counted as a header's | KILLED (2: r1's and r2's tests) |
| B05 | a header's sync that fails does not fail the volume | KILLED (3) |
| B06 | the mark of a header's sync is not taken at its end | SURVIVED; no difference (set again at each begin) |
| C01 | a sync that ends begins none for a header due alone | KILLED (1) |
| C02 | the header due is looked at whether or not a sync is under way | SURVIVED; no difference (none is) |
| E03 | the drop's loop as before #120 | KILLED (1) |
| N1 | the drop waits for a sync under way on a failed volume too (#120's head) | KILLED (1: the F3 test) |
| N2 | the drop waits for a sync under way on a failed volume only | KILLED (2) |
| N3 | the drop's wait bound to the option | SURVIVED; no difference (off, no sync is under way with no write waiting: n6) |
| N4 | the drop goes on for a header due, failed or not (F4's term without its guard) | KILLED (1: r3's test) |
| N5 | the drop waits only for a sync counted as a header's | SURVIVED; no difference (any other has a write waiting) |
| N6 | a failed store's drop still waits for a header's sync under way | SURVIVED; n1, n3, n4 catch it (F-B) |
| N7 | with no helper the map's sync is not made at all | KILLED (2: r4's and r5's tests) |
| F01 | the option does not reach the store | KILLED (12) |
| F02 | the figures say the option is off | KILLED (3) |
| F03 | the figures give the write path's syncs as the header's | KILLED (7) |
| S01 | the header's sync is read from a file of version 15 | KILLED (2) |
| S02 | count and summed time in each other's place | KILLED (4) |
| S03 | a file of version 15 is not read | KILLED (2) |
| S04 | any value but 0 at 976 reads as on | KILLED (1: the flag test) |
| S05 | status always prints `intent_header_sync on` | KILLED (1) |
| S06 | status prints the header's keys for a file of any version | KILLED (1) |
| S07 | the file says on where the option is off | KILLED (4) |
| S08 | a file of version 15 is taken to be 984 bytes | KILLED (2) |
| D01 | serve does not hand the option to the store | KILLED (1) |
| D02 | the daemon's status says off whatever the store says | KILLED (1: the `intent_shown_of` test) |
| D03 | the daemon's status gives the write path's syncs as the header's | KILLED (1: the same) |
| D06 | `intent_shown` shows no intent map whatever the store has | SURVIVED (the nit above) |
| D04 | `--intent-header-sync` is taken without `--hold-cuts` | KILLED (1) |
| D05 | the option is on wherever cuts are held | KILLED (3) |

E01, E02 and E04 of the first look were edits of #120's condition; its
text is gone, and N1 to N6 are their counterparts on the new one (E04 is
N4).

### Scenarios

Appended to the end of `cuts.rs` for a run (they use its helpers and
`dropped_within`), filter `review_m24d`. All pass at the head.

| Scenario | What it shows |
|---|---|
| n1 | a failed store with a header's sync under way and no write waiting: the drop ends without it |
| n2 | a healthy store: the drop waits for a header's sync under way for as long as the disk holds it, and a power cut after keeps the base |
| n3 | a release on a store that has failed already, option on: the cut is let go, a sync begins, and the drop does not wait for it (F-A) |
| n4 | a header due when the volume fails for another reason: the drop ends at once, and no sync follows |
| n5 | a header due and the sync under way fails inside the drop: the drop waited, the volume fails, no second sync, the drop ends |
| n6 | option off, 40 random programs: with nothing in flight no sync of the map is ever under way, so the drop's new term has nothing to wait for |

### Appendix: the scenarios

```rust
// ---- The review's scenarios for #124 (M2.4d-1) at 339cc03 ----
//
// Appended to crates/skyzfs-local/src/inplace/tests/cuts.rs for a run
// (`cargo test -p skyzfs-local --lib -- review_m24d`), and taken out
// again. The scenarios of the first look (scenarios.rs) are tests of the
// tree now; these are about the drop's loop as #124 leaves it.

/// n1. A failed store with a header's sync under way and no write
/// waiting (the pull request's test of the failed store has a write
/// path's sync under way): the drop ends without that sync.
#[test]
fn review_m24d1_n1_the_drop_of_a_failed_store_does_not_wait_for_a_headers_sync_under_way() {
    let disk = created(201, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    let syncs = disk.syncs_of(intent::INTENT_NAME);
    disk.hold_syncs_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    settle_map(&mut store);
    assert_eq!(disk.syncs_of(intent::INTENT_NAME) - syncs, 1);
    assert_eq!(intent_of(&store).header_syncs.count, 0, "under way still");
    store.fail_now();
    let (dropped, early) = dropped_within(store, Duration::from_millis(500));
    disk.release_syncs();
    let late = early || dropped.recv_timeout(DRAIN).is_ok();
    assert!(late, "the drop did not end once the sync was let go");
    assert!(
        early,
        "the drop of a failed store waited for the header's sync under way"
    );
}

/// n2. The other side of n1: on a store that has not failed, the drop
/// does wait for a header's sync under way, for as long as the disk
/// holds it, and a power cut after the drop keeps the base.
#[test]
fn review_m24d1_n2_the_drop_of_a_healthy_store_waits_for_a_headers_sync_under_way() {
    let disk = created(202, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    let id = store.cut_state_id().expect("a cut state");
    disk.hold_syncs_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    let (dropped, early) = dropped_within(store, Duration::from_millis(300));
    assert!(!early, "the drop ended with the header's sync under way");
    disk.release_syncs();
    assert!(
        dropped.recv_timeout(DRAIN).is_ok(),
        "the drop did not end once the sync was let go"
    );
    disk.power_cut(crate::sim::LoseAll);
    let store = start_as(&disk, 0, &region, IoMode::default(), AFTER_A_REBOOT);
    let one = CommittedCut {
        state: id,
        number: 1,
        group: 7,
    };
    assert_eq!(store.committed_cut(), Some(one));
}

/// n3. A release that comes to a store that has failed already, with the
/// header's sync on: what the store does (it lets the cut go and begins a
/// sync, as at the first look's head), and that the drop does not wait
/// for that sync.
#[test]
fn review_m24d1_n3_a_release_on_a_failed_store_begins_a_sync_the_drop_does_not_wait_for() {
    let disk = created(203, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    store.fail_now();
    let syncs = disk.syncs_of(intent::INTENT_NAME);
    disk.hold_syncs_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    assert_eq!(phases(&store), []);
    assert_eq!(disk.syncs_of(intent::INTENT_NAME) - syncs, 1);
    let (dropped, early) = dropped_within(store, Duration::from_millis(500));
    disk.release_syncs();
    let late = early || dropped.recv_timeout(DRAIN).is_ok();
    assert!(late, "the drop did not end once the sync was let go");
    assert!(early, "the drop of a failed store waited for the sync");
}

/// n4. A header is due (two releases, the first one's sync held) when
/// the volume fails for another reason: the drop ends without the sync
/// under way, and no sync follows it for the header that was due.
#[test]
fn review_m24d1_n4_a_header_due_when_the_volume_fails_for_another_reason_is_not_synced() {
    let disk = created(204, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    write(&mut store, 6, 0x66).unwrap();
    group(&mut store, 200, 8, 80);
    let syncs = disk.syncs_of(intent::INTENT_NAME);
    disk.hold_syncs_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    assert!(store.release_cut(2));
    settle_map(&mut store);
    store.fail_now();
    let (dropped, early) = dropped_within(store, Duration::from_millis(500));
    disk.release_syncs();
    let late = early || dropped.recv_timeout(DRAIN).is_ok();
    assert!(late, "the drop did not end once the sync was let go");
    assert!(early, "the drop of a failed store waited for the sync");
    assert_eq!(
        disk.syncs_of(intent::INTENT_NAME) - syncs,
        1,
        "no sync after the failure"
    );
}

/// n5. A header is due and the sync under way fails inside the drop: the
/// drop waited for it (the store was healthy when it began), the volume
/// fails there, no second sync follows, and the drop ends.
#[test]
fn review_m24d1_n5_a_sync_that_fails_inside_the_drop_ends_it_with_a_header_due() {
    let disk = created(205, 0);
    let region = region();
    let mut store = one_cut(&disk, &region, IoMode::default(), true);
    write(&mut store, 6, 0x66).unwrap();
    group(&mut store, 200, 8, 80);
    let syncs = disk.syncs_of(intent::INTENT_NAME);
    disk.hold_syncs_of(intent::INTENT_NAME);
    assert!(store.release_cut(1));
    assert!(store.release_cut(2));
    settle_map(&mut store);
    disk.fail_sync_of(intent::INTENT_NAME);
    let (dropped, early) = dropped_within(store, Duration::from_millis(300));
    assert!(!early, "the drop ended with the header's sync under way");
    disk.release_syncs();
    assert!(
        dropped.recv_timeout(DRAIN).is_ok(),
        "the drop did not end after the sync failed"
    );
    assert_eq!(
        disk.syncs_of(intent::INTENT_NAME) - syncs,
        1,
        "no sync after the failure"
    );
}

/// n6. With the option off, a store that has not failed never has a sync
/// of the map under way with no write waiting, so the drop's wait for one
/// cannot come to bear there: over random programs polled after every
/// step, wherever no request is in flight the map's sync has ended too.
/// (With the option on that is not so, which is the point of the wait.)
#[test]
fn review_m24d1_n6_with_the_option_off_no_sync_is_under_way_with_no_write_waiting() {
    for seed in 0..40 {
        let case = format!("seed {seed}");
        let disk = created(seed, 0);
        let held = region();
        let mut store = start_syncing_header(&disk, 0, &held, ON_CALLER, TAKING_OVER, false);
        let mut txg = 10;
        let mut rid = 100_000;
        for &step in &intent_program(seed, 40) {
            match step {
                IntentStep::Write(block, byte) => {
                    rid += 1;
                    assert!(
                        run(&mut store, rid, write_op(block, byte)).result.is_ok(),
                        "{case}"
                    );
                }
                IntentStep::Zeroes(block) => {
                    rid += 1;
                    let zeroes = Op::WriteZeroes {
                        offset: block * BLOCK,
                        len: BLOCK,
                    };
                    assert!(run(&mut store, rid, zeroes).result.is_ok(), "{case}");
                }
                IntentStep::Group => {
                    txg += 1;
                    for label in 0..LABELS {
                        rid += 1;
                        assert!(
                            run(&mut store, rid, uberblock_write(label, txg, txg * 3))
                                .result
                                .is_ok(),
                            "{case}"
                        );
                    }
                    rid += 1;
                    assert!(run(&mut store, rid, Op::Flush).result.is_ok(), "{case}");
                }
                IntentStep::Release => {
                    if let Some(oldest) = store.held_cuts().first().copied() {
                        if oldest.phase == CutPhase::Uploading {
                            store.release_cut(oldest.number);
                        }
                    }
                }
            }
            assert!(!store.is_failed(), "{case}");
            assert!(
                !store.intent_syncing(),
                "{case}: a sync of the map under way with nothing in flight"
            );
        }
    }
}
```
