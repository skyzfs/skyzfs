## Review of #142 at 05d79f4 (M2.6b1)

The agent's pure half of M2.6, in the new crate `crates/skyzfs-agent`:

- the record (`record.rs`, SZAG version 1, written by temporary file,
  fsync, rename and directory fsync);
- the manifest (`manifest.rs`, version 1 canonical JSON);
- the snapshot names (`names.rs`);
- the planner (`plan.rs`, `plan(&Publish, &Seen) -> Action`, and its
  table in `plan/tests.rs`).

It also carries the follow-ups of #140's next look: texts in
`skyzfs-control`, `skyzfs-upload` and `skyzfsd`, and three tests (Wb, Q5,
Q7).

The commits are `64e4b1d`, `ac097d9`, `e5e9faf` and `05d79f4`, on
`zfs-topology` at `501ccf7`. The brief named `e5e9faf`. The head moved to
`05d79f4` during the review, so I reviewed that. It changes the
planner's docs on reading 1 and adds the table row
`an_earlier_request_that_landed_is_this_publishs`. I formed the findings
from the diff and design §5.2 and §6.2 before I read the description,
then checked the description against them.

**Verdict: can merge once the two runs still going end green on
`05d79f4` and Codex's one finding is answered on its thread. No finding
blocks.** One defect (F1) needs a decision from the lead (D1). It can be
fixed here or in M2.6b2: the planner has no caller yet.

### Findings, most severe first

#### Defects

- **F1. `unknown` and `refused rewound` give a publish up without a
  look when the snapshot is broken.** Not blocking. Certain on the
  planner; the scenario is rare.
  - Where:
    - `crates/skyzfs-agent/src/plan.rs:319-331` (`anew`);
    - `plan.rs:360-363` (`Reply::Unknown`);
    - `plan.rs:366` (`Code::Rewound`).
  - What it does. `anew` gives up (`Destroyed`, `Rewound`, `Replaced`
    or `Premise`) whenever `broken` is set. `Unknown` and `Rewound`
    reach it with no look at `snap/t` whenever the snapshot is broken.
  - Scenario:
    1. Daemon D1 publishes `snap/t` for request R1.
    2. Before the agent reads `done`, D1 stops and the pool is
       imported with `-T` to a group below G, so `pool@t` is gone.
       The same holds with the hold gone and a member renamed, or with
       a checkpoint taken after the swap, which refuses the premise.
    3. The next daemon answers `unknown`, or `refused rewound` because
       C is below its start count.
    4. The planner gives `Abandon(Destroyed)` (or `Rewound`,
       `Replaced`, `Premise`) and never asks for a look.
    5. `snap/t` stays published, and the record names `t` as given up.
  - The scratch test
    `review_f1_unknown_with_the_snapshot_gone_gives_up_without_a_look`
    shows each of these at the head.
  - It contradicts:
    - the docs' own reason for skipping the look (`plan.rs:36-39`: "a
      publish asked again is looked up by the uploader before it is
      made"), since a broken publish is never asked again;
    - the table's principle `an_end_that_may_hide_a_publish_is_looked_up_first`.
  - The planner is inconsistent: `expired` with `pool@t` gone and
    `snap/t` ours looks first and records `Done`. `unknown` in the same
    state records `Abandon(Destroyed)`.
  - Harm. The record and the store disagree with no report. M2.7's
    retention may then not count or remove the ref, and the user is
    told the publish was given up when it was made.
  - The fix is not just "look, then `Done`". With `pool@t` gone,
    `snap/t` that is ours is one of two things, and the agent cannot
    tell them apart without opening the cut:
    - a publish made before the rewind (good: the cut holds `pool@t`);
    - one made after it through a gap in rule (R) (the cut lacks
      `pool@t`).

    I recommend looking before giving up from `unknown`/`rewound`
    when broken. Where `snap/t` is ours and `pool@t` gone, record it
    under its own outcome and report it (`Stop`, or `Done` with a
    report), not a silent `Done`. This is D1.
  - The table pins the present behaviour on purpose. Plant P6 (look
    first when broken) is KILLED by the rows `unknown, gone` and
    `rewound, gone`, so a fix changes those rows deliberately.

#### Codex's finding (P2 on `plan.rs:340` at `e5e9faf`, unanswered)

- Codex says a qualifying newest cut that is abandoned stops every
  further `zpool sync` until the request expires.
- I checked it against the code, and it is valid. `qualifies`
  (`plan.rs:289-297`, used at `plan.rs:341`) reads the daemon's
  newest *counted* cut. That cut stays the newest after it is
  abandoned (`skyzfs-local` `CutState::newest_cut`).
- The effect is a delay, not a loss. The publish waits up to its expiry
  (10 min by default) on an idle pool. `expired` then gives a look,
  `Absent`, and `AskAnew`, and the publish completes.
- The scratch test `review_codex_a_qualifying_newest_cut_stops_every_assist`
  shows `Wait` at 1, 5 and 9 minutes.
- Not blocking. The author answers it on its thread: either a
  bounded assist while `pending` shows no progress, or the delay
  accepted and documented. This is D3.

#### Test gaps

- **T1. No golden bytes for the record**, contrary to CONTRIBUTING
  ("codecs get golden-bytes tests"). postcard is positional, so any
  reorder of fields or variants silently changes version 1. These
  plants all SURVIVED:
  - R1: two fields of `Publish` swapped;
  - R2: a variant inserted first in `Why`;
  - R3: bytes after the body accepted (`record.rs:357`).

  A test that encodes one record of every phase and every `Why` and
  compares with fixed bytes would kill all three. The manifest has
  golden bytes (`a_manifest_is_version_1s_canonical_json`).
- **T2. No fault test of the write path.** The description says so
  openly. These plants all SURVIVED:
  - R4: the file not synced before the rename;
  - R5: the directory not synced after it;
  - R6: written in place, with no temporary file;
  - R7: a new directory's parent not synced;
  - R8: a removed manifest's directory not synced.

  What a fault-injecting test needs:
  - a small file-system trait under `Store`: create, write, fsync
    file, rename, fsync dir, unlink, read;
  - an in-memory model of it that keeps a durable image and a
    volatile one;
  - a power cut injected after every operation of a save, a
    `write_manifest` and a `remove_manifest`, and each fsync made to
    fail in turn.

  After each cut it asserts that:
  - `load` gives the record from before or after the save, never
    `Corrupt`, and never `Absent` once a save has returned `Ok`;
  - every manifest that a `Taken`-or-later record names is there,
    whole.

  The alternative is dm-flakey or dm-log-writes in a CI job, as the
  replay rig does for the daemon. M2.6b2's crash matrix stops the
  engine between steps, so it does not cover this (D2).
- **T3. No table row for `Intent` with the premise refused.** The
  planner takes the snapshot whatever the premise. It then gives it
  up at `Taken`, keeping `pool@t`. The scratch test
  `review_s_intent_takes_whatever_the_premise` shows this. Whether
  that is wanted is S1; either way, a row should pin it.
- **T4. "Ours with the hold gone, `Done`" is pinned only by the new
  row's `unheld` assert, and only for `refused taken`.** Plants U1
  (ours unheld → `Hold`) and U2 (ours unheld → name given up) are
  KILLED by that assert alone. With it dropped they SURVIVE, although
  they change the look of `expired`, `withdrawn`, `attachment` and
  `state` too. A row with `expired`/`withdrawn`, `.unheld()` and
  `.gone()`, looked `Ours`, would pin them (see "The new row" below).

#### Suggestions (not blocking)

- **S1.** Check the premise at `Intent` too (`plan.rs:228-232`). That
  way a pool that is refused (a checkpoint, a mirror) does not get a
  `pool@t` the agent then keeps and gives up.
- **S2.** `done` read with `pool@t` gone and `load_guid` moved is
  recorded `Done` and nothing is reported (scratch test
  `review_s_done_after_a_move_with_the_snapshot_gone_is_done`). That
  is what a rewind that rule (R) missed looks like from the agent. A
  report there costs nothing.
- **S3.** `broken` compares members by name and GUID (`plan.rs:260-275`).
  A dataset renamed after the snapshot, or a pool imported under
  another name, gives `Replaced` and the publish is given up
  (`review_s_a_rename_gives_the_publish_up`). The snapshot GUIDs
  alone identify `@t`. Compare by GUID if renames are to survive.
- **S4.** `held_until` and `ASSIST_AFTER` are on the wall clock
  (`Seen::now_ms`). If the clock steps forward, the hold is released
  early, possibly while the request can still land. M2.6b2 should
  either take a monotonic time for the in-process part, or say why the
  wall clock is enough.
- **S5.** `make_dir` (`record.rs:527-535`) syncs the parent only when
  it creates the directory. Suppose a crash, or a failed parent sync,
  comes after `create_dir`. The next call then gets `AlreadyExists`
  and never syncs. Syncing the parent on both paths is cheap.
- **S6.** Nothing stops two agents from using one record directory.
  Both would write `.record.tmp` and rename over each other. M2.6b2
  should take a lock (`flock` on the directory or a lock file).
- **S7. Text nits.**
  - `skyzfs-control/src/lib.rs:359-361`, `Code::Taken`: "another cut,
    manifest or count". The row records no count; say "another cut
    or manifest".
  - The new row's doc (`plan/tests.rs:831-839`) tells of a request
    "refused `rewound`" that "the daemon carried". In one daemon a
    request refused `rewound` is dropped. The story holds only with an
    earlier daemon that carried R1 before a restart, and a later one
    that refused the re-ask. Say so.
  - The description says `load_guid` changes with "a new
    attachment". It changes at every import, which is what rule (L)
    needs; the attachment is the daemon's.
  - The description still gives `501ccf7..e5e9faf` and "30 in the
    crate". The head is `05d79f4`, with 31 tests.
  - The `Intent` rule in the docs (`plan.rs:10-13`) omits "with the
    daemon here" (it waits otherwise).
- **S8. The rule for `AskAnew` in M2.6b2.** The new count must be read
  after the hold is verified (`Count`'s docs). The engine must also
  compare the `load_guid` it read with that count against the one
  verified with the hold. Otherwise an import between the two makes a
  count of one attachment be sent with the hold of another. This is
  for M2.6b2 (D5).

Nothing else found.

### The new row (the second brief)

The row `an_earlier_request_that_landed_is_this_publishs`
(`plan/tests.rs:831-859`) has three asserts. The publish is asked anew
(count 9, another state), refused `taken`, and the look shows our row
of the first count. The asserts are:

- held: `Done`;
- `unheld`: `Done`;
- asked with C, held: `Done`, a control.

**It covers both an earlier request that landed with the hold kept
and one with the hold released.** Each plant was run with the row as
it is, with the row dropped, and with each assert dropped
(`plants142n.py`, `plants142n.out`):

| Plant | As is | Row dropped | `unheld` assert dropped | Held assert dropped |
|---|---|---|---|---|
| C1 the row's state and number compared with the count recorded last | KILLED (the row) | SURVIVED | KILLED (the row) | KILLED (the row) |
| C1n number only | KILLED (the row) | SURVIVED | KILLED | KILLED |
| C1s state only | KILLED (the row) | SURVIVED | KILLED | KILLED |
| C2 `refused taken` trusted with no look (the daemon's check compares the count) | KILLED (the row, `an_end_that_may_hide…`) | KILLED (`an_end_that_may_hide…`) | KILLED | KILLED |
| U1 ours with the hold gone → `Hold` | KILLED (the row) | SURVIVED | SURVIVED | KILLED (the row) |
| U2 ours with the hold gone → name given up | KILLED (the row) | SURVIVED | SURVIVED | KILLED (the row) |
| U3 `refused taken` with the hold gone → ask anew, no look | KILLED (the row, `an_end_that_may_hide…`) | KILLED (`an_end_that_may_hide…`) | KILLED | KILLED |

- **The count comparison is caught by this row alone.** That confirms
  the author's claim. Each of its two asked-anew asserts kills it on
  its own.
- **The `unheld` assert is load-bearing.** It alone kills U1 and U2
  (T4).
- **How C1 was planted.** `snap_seen` takes no count, so a planner
  that compares it must keep the row's cut. The plant does that
  through a thread-local set in `snap_seen` and read in `look`.
  libtest runs each test on its own thread. The baseline of each
  variant passes (31, or 30 with the row dropped).
- **Not covered by the row:** the phase `Held` (not yet `Asked`), and
  `withdrawn` after a withdrawal that lost. Neither matters to a count
  comparison: the planner reads no count on those paths.

The scratch tests in `review142.rs` all pass at `05d79f4` (37 with the
crate's 31). I rechecked them against the new docs:

- `review_reading_1_the_manifest_decides` is what the docs now say:
  state and number are not compared.
- F1 falls outside the docs' new sentence. The sentence is about a
  request that *landed*, while F1 is about the planner never looking.

### What was asked to be judged

#### (1) Reading 1 against design §5.2

Reading 1: `snap/t` is ours where its row records this manifest (length
and hash) and a group ≥ G (`plan.rs:206-221`).

- **It never takes another publish's snapshot as ours.**
  - The hash covers the pool's GUID, the name `t`, `createtxg` and
    the members' random 64-bit snapshot GUIDs.
  - A row with our manifest was written by a daemon under
    `carried_by`, for a request this agent made.
- **Its residual risk is rule (R)'s documented gaps.** These are N3 in
  `cuts.rs` (a rewind whose groups pass the top before a cut) and the
  checkpoint while the agent is down. The checkpoint is now closed by
  the premise check at `Taken`: see D4.
- **A count comparison would not reduce that risk.** The landed
  request's own count passes it. Compared with the count recorded
  *last*, it differs only after an `AskAnew`, and there it rejects the
  agent's own good publishes (rows 1 and 2 below). A landing through an
  (R) gap leaves `pool@t` gone (`broken`), so the planner gives up
  before any look and never reaches the comparison.

| # | Scenario | Reading 1 | A count comparison | Per design |
|---|---|---|---|---|
| 1 | Asked, the agent restarts, the daemon restarts (`unknown`), asked anew, `refused taken` | `Done` | name given up (misses our own) | reading 1 right |
| 2 | Pending, `load_guid` moves, the withdrawal loses to the landing, asked anew | `Done` | name given up, or `Stop(DoneNotSeen)` | reading 1 right |
| 3 | `-F` that keeps `pool@t` after an earlier landing | `Done` | name given up | reading 1 right |
| 4 | `-T` below G after an earlier daemon published | given up, no look (F1) | same | F1 |
| 5 | A rewind (R) missed, then `done` | `Done`, nothing reported (S2) | `Done` (R1's own count) | not caused by reading 1 |
| 6 | Another pool, another name or another manifest | `NotOurs` | `NotOurs` | right |
| 7 | A row of another version (daemon upgraded mid-publish) | `Unreadable`, name given up | same | misses our own; rare; accept |
| 8 | A clone, or the same manifest under two names | cannot happen (name and GUIDs in the hash) | — | — |
| 9 | A crash between each step | each phase replans from what is seen; the table has a row per phase | — | right, apart from F1 |

#### (2) The record's format and write path

The save is `record.rs:458-466`, through `replace` at `record.rs:540-553`.

- **A torn write cannot happen.** The temporary file is synced before
  the rename. A file-system bug shows as a hash error, which loads
  `Corrupt` and stops for a person. A corrupt record is never written
  over.
- **A crash after the rename but before the directory sync** leaves
  the old or the new record, and both are valid. `save` had not
  returned `Ok`. If the directory sync itself fails, `save` returns
  `Err` while the new record may be visible but not durable. M2.6b2
  must reload and not go on.
- **A partial file** can only be the temporary one. It has a dot name,
  is never read, and is truncated at the next save. A stale one is
  never cleaned up, which is harmless.
- **The version and unknown fields.** Another version loads `Corrupt`
  and is not written over, so the record fails closed. postcard has no
  unknown fields: any change of field or variant is a new version, and
  nothing pins version 1 (T1).
- **Left for M2.6b2:**
  - no lock between two agents (S6);
  - no check that the record belongs to this volume;
  - nothing done where the record is `Absent` but manifests are
    present.
- A fault-injecting test is described in T2.

#### (3) The manifest's canonical form

There is one hash per manifest. `decode` re-encodes and compares the
bytes (`manifest.rs:216`), and plant M1 shows that this is tested.

- Keys are sorted. ZFS names and property names are ASCII, so byte
  order is code-point order. The docs could say "sorted by bytes".
- Duplicate JSON keys and duplicate members are refused, because
  members must be strictly increasing.
- Escapes are the shortest.
- GUIDs are decimal strings with no leading zero; plant M2 shows the
  round trip is tested. `createtxg` is an exact `u64`.
- There is no locale dependence. White space and invalid UTF-8 are
  refused.
- Validation is loose in one place: `check` (`manifest.rs:225-250`)
  accepts a member such as `tank//x` or one containing `@`. The
  agent's own encoder never makes one, so the hash is still unique.

#### #140's follow-ups and the texts

- **Wb, Q5 and Q7** each fail on their plant:
  - Wb is KILLED by `control::tests::uploader::the_limit_begins_with_each_request`;
  - Q5 by `a_second_state_of_the_tops_group_is_remembered`;
  - Q7 by `an_uberblock_that_does_not_verify_does_not_raise_the_top`.

  Wb's first run used `--bin skyzfsd`, which holds no control test, so
  it was void. Its rerun with `--lib` is the result.
  `the_limit_begins_with_each_request` passed 30 of 30 runs alone and
  12 of 12 under parallel load.
- **The texts are accurate to the code:**
  - `Refusal::Taken` ("snap/T exists and does not record this
    request", `skyzfs-upload/src/publish.rs:262`);
  - the three `NoAnswer` texts ("within what was left of the
    request's … s", `bins/skyzfsd/src/control/handler.rs:203,231,253`);
  - the "one rule" paragraph of `refused rewound`;
  - the N3 limit in `cuts.rs`.

  The exception is `Code::Taken`'s "count" (S7). `cuts.rs:137-141`
  says "What is left … A check after the snapshot closes it". The
  planner's premise check at `Taken` is that check, so the gap is
  closed, and design §5.2's "is not caught" is now stale (D4).
- **CONTRIBUTING:** no `unwrap()` outside tests, the formats are
  versioned, and fmt and clippy are clean. The record lacks golden
  bytes (T1) and a fault test (T2).

### Gates (toolchain 1.100.0 stable, after `rustup update stable`)

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -D warnings`: clean in three
  configurations:
  - plain;
  - with `skyzfsd/test-hooks,skyzfs-local/test-hooks`;
  - with `skyzfs-ublk-sys/control-test-program`.
- Tests:
  - `skyzfs-agent`: 31 passed;
  - `skyzfs-control`: 10 passed;
  - `skyzfs-upload`: every suite ok (60, 5, 25, 5, 23, 9, 60);
  - `skyzfs-local`: 323 passed and 1 ignored, plus 11 integration;
    with `test-hooks`, 323 passed plus 16;
  - `skyzfsd`: 142 passed plus 3; with test hooks, 181 plus 3.

### Plants (one at a time, each file restored in `finally`, 600 s limit)

All results are in `plants142.out` and `plants142n.out`.

- **SURVIVED (test gaps T1, T2):** R1 to R8.
- **KILLED:**
  - M1, M2;
  - P1 to P5, P7;
  - P6 (F1's fix, by the rows that pin F1);
  - Q5, Q7, Wb;
  - C1, C1n, C1s, C2, U1, U2, U3 (the table above).
- **NOT APPLIED:** none.

### CI on `05d79f4` (read at 07:19 UTC)

- **Green:**
  - `skyzfs-control-rig` 45563053777;
  - `skyzfs-nodeb-rig` 45563053992;
  - `skyzfs-cut-daemon` 45563053957;
  - `skyzfs-publish-rig` 45563053823;
  - `skyzfs-replay-rig` 45563053891 (the daemon jobs);
  - `coverage` 45563053789;
  - `k3s-smoke` 45563053887;
  - `e2e` 45563054081;
  - `skyzfs-ci` (push) 45563045548.
- **Still going:**
  - `ci` 45563053935 (lint, test and s3-backends running; changes,
    chart-lint and unit-arm64 green);
  - `skyzfs-daemon-rig` 45563054024 (three daemon jobs running). Read
    its summary's table when it ends, not its colour.
- The runs on `e5e9faf` were cancelled by the push of `05d79f4`.
- Codex reviewed `e5e9faf` only.

### Decisions for the lead

- **D1 (F1).** Should the planner look before giving up from
  `unknown`/`refused rewound` when the snapshot is broken? Where
  `snap/t` is ours and `pool@t` gone, is that `Done`, `Done` with a
  report, or `Stop`? `expired` in the same state already gives `Done`.
  Here or in M2.6b2.
- **D2 (T2).** Does M2.6b2's crash matrix include faults of the file
  system (a model under a trait, or dm-flakey/dm-log-writes in CI), or
  are the syncs left to reading?
- **D3 (Codex P2).** A bounded assist while a request pends with no
  progress, or the delay of up to the expiry accepted and documented.
- **D4.** Design §5.2's sentence on a checkpoint taken before the
  snapshot says that case "is not caught". `cuts.rs:137-141`'s "What is
  left" says the same. The premise check at `Taken` now catches it, so
  both texts should say so.
- **D5 (S8).** The rule for `AskAnew` in M2.6b2: the `load_guid` read
  with the new count is compared with the one verified with the hold.

### Files here

- `review142.rs`: the scratch scenarios. They are included at the end of
  `crates/skyzfs-agent/src/plan/tests.rs` in a scratch worktree only,
  and pass at the head.
- `plants142.py`, `plants142.out`: the first plants.
- `plants142n.py`, `plants142n.out`: the plants of the new row.
