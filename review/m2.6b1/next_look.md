## Next look at #142 (M2.6b1) at 347a17f

Reviewed head: **`347a17f`**, three commits on `05d79f4` (my first look):

- `6e8ea90`: T1 (golden bytes), T3, T4, S5, S7;
- `b79083a`: the lead's D3 (Codex's P2, a sync while a cut carries the
  request) and D4's text in `cuts.rs`;
- `347a17f`: Codex's second P2 (`make_dir` and a store path of one part).

The brief named `b79083a`; the head moved to `347a17f` during the look.
Everything below holds at `347a17f`. `347a17f` changes only `record.rs`
and its tests, so the planner's results at `b79083a` stand, and every
plant was run again at `347a17f` with the same verdicts.

**Verdict: can merge once CI on `347a17f` ends green (five runs still
going, listed under CI). No finding blocks.**

- One finding needs a decision from the lead now, for M2.6b2: N1. With a
  plain `zpool sync`, the assist makes no cut on an idle pool.
- Two small test gaps (T5, T6) and three text nits (S9 to S11) are left
  to the author. Fix them here or in b2.
- The deferrals to b2 are recorded in the description ("To M2.6b2, by
  the lead's decisions") and in the answer of 07:54. They cover F1/D1,
  T2/D2, D5, S1 to S4 and S6. I did not argue them again.

### Findings, most severe first

#### Defects

None in the code under review.

#### For the lead (a decision for b2)

- **N1. The assist's `zpool sync` writes no group on an idle pool, so it
  makes no cut. That is the very case of D3's fix.** It does not block
  b1, since the planner only names the action. Certainty: high on
  ZFS's behaviour, from the repository's own records; medium on the
  remedy.
  - **Where.** `crates/skyzfs-agent/src/plan.rs`:
    - line 30 (the rule), 87 to 89 and 177 (`Action::Assist`: "`zpool
      sync`, so that a group is written and a cut counted");
    - lines 295 to 318 (`assist_due`), whose case with a carrying cut
      exists to get a new cut past an abandoned one.
  - **Evidence.**
    - M2.T1 recorded that ZFS 2.4 commits groups that write nothing and
      writes no uberblock for them (impl notes, "ZFS 2.4 commits groups
      that write nothing…"; design §5.2).
    - OpenZFS's `vdev_config_sync` returns early when nothing changed in
      the group and the config is clean.
    - The publish rig writes 4 KiB before every `zpool sync` for exactly
      this reason (`tests/kernel/publish_agent.py:317-320`: "an idle
      pool's sync makes no cut").
    - The M2.5 notes say "an idle pool makes no next cut, so a publish
      asked again after a death waits or expires".
  - **Failing scenario.** The pool is idle after the hold.
    1. A publish is asked anew (`unknown`, `rewound`, `expired`, …) with
       a count read after the hold, so no cut is counted after it.
    2. The planner says `Assist` once a second from 10 s.
    3. A plain sync writes nothing, so no cut comes.
    4. The request expires after 10 min. The look finds nothing, and
       the publish is asked anew.
    5. This repeats for as long as the pool stays idle. Nothing bounds
       it: the planner does not read `attempts`.

    Codex's case is the same: an abandoned qualifying cut on an idle
    pool. D3's syncs every 10 s make no cut that could take the
    abandoned one's map.
  - **What b2 needs.** The assist must make a group that writes. The
    choices:
    - a small write, as the rig does (it touches the user's dataset);
    - `zpool sync -f`: in OpenZFS's `zpool_main.c`, undocumented,
      "force uberblock (and config including zpool cache file) update".
      It rewrites the labels and, each time, the cache file;
    - a sync task of the agent's own, such as a user property or a
      second hold placed and released.

    Whichever is chosen, a kernel check (M2.T5) should show that a cut
    the daemon counts follows on an idle pool. The planner's text could
    then say "a sync that writes a group" instead of `zpool sync`.
  - **Asked of the lead:** which way, and whether a publish that never
    lands should be bounded (`attempts`, or a report after N requests).

- **D4's remaining case may be empty.** A text matter for the lead's
  correction of design §5.2 at the merge. Certainty: medium.
  - `cuts.rs:139-141` now says "What is left: a checkpoint taken and
    rewound to within one publish's window … while the agent is down
    and no check runs."
  - Such a checkpoint is taken after the premise check at Taken. So it
    is taken after `pool@t` exists, and holds it. The same paragraph
    says a rewind to such a checkpoint publishes the snapshot asked for.
  - One taken before `pool@t` is seen at Taken, or was discarded and
    cannot be rewound to.
  - Unless the lead sees another case, the paragraph can say that
    nothing is left for a publish under way.

#### Test gaps (not blocking)

- **T5. The start at twice `ASSIST_AFTER` is pinned only where no sync is
  recorded.** Certain.
  - **Plant A5 survives.** The plant changes `last.max(from)` to `last`
    at `plan.rs:313`: a carrying cut is then synced 10 s after the last
    sync, whenever that was.
  - **When the two differ.** Only where the last sync is older than
    `accepted + ASSIST_AFTER`, which happens where it belongs to an
    earlier request of the same publish. `Seen::assisted_ms` is per
    publish, so it outlives an `AskAnew`. The plant then syncs as soon
    as 1 s after `accepted`, against "no sooner than twice
    `ASSIST_AFTER` after `accepted`" (`plan.rs:32-34`).
  - **Fix.** One row in `pending_a_sync_helps_only_where_no_cut_has_carried_the_request`:
    carried, `assisted(ASKED - 9 * SECOND)`, at `ASKED + 15 * SECOND`,
    `Wait`. The scratch test `review_n6_a_sync_of_an_earlier_request`
    pins the built rule.
- **T6. In `golden()` the bytes of each given-up name's `why` and
  `at_ms` are equal**, so swapping those two fields of `Abandoned` leaves
  `GOLDEN` unchanged. Certain.
  - **Where.** `record/tests.rs:396-405`: `.zip(0u64..)` makes `at_ms`
    the variant's index, and the body ends `02 77 3N 0N 0N` for each.
  - **Plant R14** (the two fields swapped) survives. A record written
    with a real time (about 1.7e12 ms) would not read back.
  - **Fix.** Distinct values (for example `at_ms` 1000 + i), and
    `GOLDEN` regenerated.
  - Every other field of `golden()` has a value of its own. `"i"`
    repeats `false`/`0` in `held`/`attempts`, but `"t"` separates them,
    and R1 is caught.

#### Suggestions and texts (not blocking)

- **S9. The description is behind the head.** It gives `501ccf7..b79083a`,
  six commits, "33 in the crate" and gates "at `b79083a`". `347a17f` is
  the seventh commit, and the crate has 34 tests.
- **S10.** `assist_due`'s doc (`plan.rs:303-307`) says "the request ends,
  at the latest at its expiry, after which nothing pends". True of one
  request. The publish then asks anew and the syncs go on (N1's loop).
  "after which it is looked up and asked anew" would be exact.
- **S11. A clock stepped back stops every sync** (S4, deferred). The
  scratch test `review_n7` shows how: a sync recorded an hour ahead of
  the clock stops every sync, with or without a carrying cut, until
  the clock passes it. A request asked anew inherits the wait if b2
  keeps `assisted_ms` across it. The same holds for the old branch.
  b2's guard can be small: a recorded sync later than now counts as
  none.

Nothing else found.

### What was asked to be judged

#### (1) The assist's timing (`assist_due`, `plan.rs:308-318`)

I derived the rule from what each case needs:

- **A cut on its way** needs time to be uploaded. 20 s after
  `accepted` is two group timeouts past the first sync.
- **An abandoned cut** needs a new group, at most one per 10 s.
- **No qualifying cut** keeps the old pace: once a second from 10 s
  (the lead's Q8).

The code does exactly that.

- **No spin.**
  - Carried for the whole default expiry: first sync at
    `accepted + 20 s` exactly, then every 10 s, 59 in ten minutes
    (`review_n1`).
  - Polled every millisecond around the boundary: syncs at 20, 30 and
    40 s, none a millisecond early.
  - Never carried: 591, one a second from 10 s (`review_n2`).
  - `qualifies` flipping at every look gives the slower pace, never
    faster than once a second (`review_n5`). It cannot flip in one
    daemon anyway: the newest cut only moves on.
- **The usual course.**
  - A sync at 10 s makes the carrying cut. The next sync, if the
    publish has not landed, comes at 20 s (`review_n3`).
  - A cut that stops qualifying goes back to once a second at once
    (`review_n4`).
- **Nothing outside pending.** No sync for:
  - Held and not asked (`Ask`);
  - `done` and the ends (`Look`);
  - the premise refused, or `load_guid` moved (`Withdraw`);
  - the daemon away, `busy` (`Wait`) (`review_n10`).

  A sync can come after the publish landed only within one look, before
  the next `progress` says `done`. That is harmless.
- **Restarts.**
  - `assisted_ms` is not in the record, so an agent restart syncs at
    once if one is due, then keeps the pace.
  - A daemon restart drops the request (`unknown`, asked anew, new
    `accepted`).
  - `accepted` is the daemon's last `accepted` (`record.rs:138`), so the
    start counts from when the daemon's expiry starts.
- **Clocks.**
  - A forward step gives one sync, then the pace (`review_n8`).
  - A backward step stops the syncs (S11).
  - Both branches are on the wall clock, as before (S4, b2).
- **Arithmetic.** There is no subtraction, the adds saturate, and
  `Duration` to ms is checked. Every combination of 0, 1, `MAX - 20000`,
  `MAX - 1` and `MAX` for `accepted`, now and the last sync plans
  without a panic. An `accepted` at the top is not due until now
  reaches it (`review_n9`).
- **The plants** (`plants142b.py`; results at both heads in
  `plants142b.out` and `plants142b-347a17f.out`):
  - The author's four are each KILLED by
    `pending_a_sync_helps_only_where_no_cut_has_carried_the_request`:
    - A1, the old rule;
    - A2, a sync every pause;
    - A3, the start at twice dropped;
    - A4, the last sync not counted.
  - My own are KILLED too:
    - A6 (`>` for `>=`);
    - A7 (the interval `ASSIST_PAUSE` while carried);
    - A8 (one sync only);
    - A9 (twice `ASSIST_AFTER` after the last sync);
    - A10 (`min` for `max`);
    - A11 (from three times);
    - A12 (the cut's test ignored).
  - A4 and A10 are also caught by the `held_until` row, through its new
    fixture.
  - A5 survives (T5).
- **The old scratch test** `review_codex_a_qualifying_newest_cut_stops_every_assist`
  (`review142.rs`) now fails, as it must: at 1, 5 and 9 minutes the
  planner says `Assist`. Every other scenario of the first look passes
  unchanged: F1, S1, S2 and S3 behave as at `05d79f4`, as deferred.

The rule is sound for what the planner can see. Whether a sync helps at
all on an idle pool is N1.

#### (2) The `held_until` row's fixture (`plan/tests.rs:430-436`)

The row still pins that the hold is kept, and the change made it
stronger, not weaker.

| Plant | Fixture as is | Row dropped | Fixture reverted, `Assist` expected |
|---|---|---|---|
| H0: nothing planted, fixture reverted (`Wait` kept) | — | — | the row FAILS at the head: the new rule gives `Assist` there, so the change was needed |
| H1: hold released past `held_until` with the daemon here | KILLED (the row) | SURVIVED | KILLED |
| H2: released past `held_until` only where no sync is due | KILLED (the row) | — | SURVIVED |
| H3: withdrawn past `held_until` with the daemon here | KILLED (the row) | — | — |

- **The row is the only pin of "here, past `held_until`, the hold is
  kept"** (H1d survives without it).
- **The other fixture would have been weaker.** The row could have kept
  no `assisted` and expected `Assist`. A planner that releases the hold
  only when no sync is due (H2) passes that. The author's fixture,
  with a sync a moment before so that `Wait` is due, catches it.
- `review_n11` shows the behaviour each way: past `held_until`, with the
  daemon here, the planner syncs or waits and never releases.

#### (3) T1's golden bytes (`record/tests.rs:332-457`)

- **The head matches the module docs** (decoded by hand):
  - `SZAG`;
  - version 1 as u16 little endian;
  - flags 0;
  - body length 458 as u32 little endian;
  - the postcard body;
  - 32 bytes of BLAKE3 over everything before.

  `golden()` has every `Phase` and every `Why`, and `decode(GOLDEN)` is
  compared too.
- **R1, R2 and R3 are now caught.** R1 and R2 by
  `version_1s_bytes_are_these`, R3 by `bytes_after_the_body_are_corrupt`.
- **New plants, each KILLED by `version_1s_bytes_are_these`** (some by
  older tests too):
  - R9: the version and flags big endian, written and read;
  - R10: the length big endian;
  - R11: the hash over the body alone;
  - R12: `Count`'s `after`/`load_guid` swapped;
  - R13: `Enrolled`'s GUIDs swapped;
  - R15: `Taken` and `Held` reordered;
  - R16: `asked_ms`/`held_until_ms` swapped;
  - R17: the manifest's length and hash swapped;
  - R18: another magic;
  - R19: `attempts` as a fixed-width integer;
  - R20: two `Why`s reordered.
- **SURVIVED:**
  - R14 (T6);
  - R21, a variant appended to `Why`. That is expected: version 1
    records still read, and the docs call only an insertion before
    another variant a format change.
- **Trailing data after the hash** is caught by the older
  `a_record_changed_or_cut_short_is_corrupt`, as a length mismatch.

#### (4) T3 and T4

- **T3** (`plan/tests.rs:218-227`, "here, the premise refused", `Take`):
  - I1 (the intent waits where the premise is refused) and I2 (gives
    up) are KILLED by
    `at_the_intent_the_snapshot_is_taken_or_given_up`;
  - with the row dropped, I1 SURVIVES, so the row is the pin.
- **T4** (`plan/tests.rs:693-709`, `Ours` with `.unheld()` and with
  `.gone()`, `Done`, for the five ends that look, held and asked):
  - U1 (`Hold`) and U2 (the name given up) are KILLED by the new rows,
    and also by `an_earlier_request_that_landed_is_this_publishs` with
    the new rows dropped;
  - U4 (ours with `pool@t` gone, given up as destroyed) is KILLED by
    the new `.gone()` rows alone (U4d survives without them);
  - so T4 is answered.

#### (5) `347a17f`: Codex's second P2 (`record.rs:529-549`)

Codex's finding was valid: `Path::new("agent").parent()` is `""`, and
`make_dir` skipped the sync. The fix is right.

- **The path forms** (scratch test `review_p_parent_of_each_form`):

  | Path | Directory synced |
  |---|---|
  | `agent`, `agent/`, `agent/.`, `./agent` | `.` |
  | `../agent` | `..` |
  | `a/b/agent/` | `a/b` |
  | `/var/lib/agent/` | `/var/lib` |
  | `/agent` | `/` |
  | `/`, `//` | nothing (`None`) |
  | `.`, `..` | `.` |
  | `a/..` | `a` |
  | `""` | `create_dir` fails first (`NotFound`) |

  For `.`, `..` and `a/..`, the directory synced does not hold the
  entry. But each names a directory that already exists, so no new
  entry needs a sync.
- **Traced.** A save and a second save from `Store::new("agent")`, in a
  temporary directory, under strace:
  1. `mkdir("agent")`, then `fsync` of `.`;
  2. the temporary file synced, renamed, and `agent` synced;
  3. at the second save, `EEXIST`, and `.` synced again (S5).
- **No other empty parent on the write path.**
  - `replace` syncs the directory it writes in (`record.rs:565`), never
    a parent. That directory cannot be empty, since `make_dir("")`
    fails before it.
  - `remove_manifest` syncs `<store>/manifests`.
  - `write_manifest`'s second `make_dir` has the store as its parent.
- **Plants:**
  - D1 (the old behaviour: an empty parent gives `None`), D5 (`.` for
    every relative path) and D6 (`/` for a root) are KILLED by
    `the_directory_synced_for_a_new_one_is_the_one_that_holds_it`;
  - D2 (`make_dir`'s old body back, `parent_of` unused), D3 (the new
    directory synced instead of its holder) and D4 (no sync) SURVIVE.
    The test pins `parent_of`, not what `make_dir` does with it. That
    is T2's class, deferred to b2 (D2).
- **Codex's thread** is answered (08:11). Codex reviewed `347a17f` with no
  findings (08:13).

### Gates (rustc 1.100.0; `rustup update stable` said unchanged)

- `cargo fmt --all --check`: clean at `b79083a` and `347a17f`.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean, plain,
  with `skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `skyzfs-ublk-sys/control-test-program`, at both heads.
- Tests at `b79083a`:
  - `skyzfs-agent`: 33;
  - `skyzfs-control`: 10;
  - `skyzfs-upload`: 60, 5, 25, 5, 23, 9, 60;
  - `skyzfs-local`: 323 and 1 ignored, plus 11;
  - `skyzfsd`: 142 plus 3;
  - with the test hooks: `skyzfs-local` 323 and 1 ignored plus 16,
    `skyzfsd` 181 plus 3.
- Tests at `347a17f`: `skyzfs-agent` 34. No other crate changed; the
  agent has no dependents.

All passed.

### CI

**On `b79083a`: all green.**

- `skyzfs-daemon-rig` 45568188748, read in the logs and not by colour:
  - the judge exited 0 on all three images;
  - the pins passed;
  - the controls failed as they must;
  - `pin_failed_volume_while_a_request_is_held` is WAITING, as
    documented.
- `ci` 45568188760 (all six jobs).
- `coverage` 45568188691.
- `skyzfs-replay-rig` 45568188859 (the daemon jobs).
- `skyzfs-control-rig` 45568188656.
- `skyzfs-publish-rig` 45568188992.
- `skyzfs-nodeb-rig` 45568188975.
- `skyzfs-cut-daemon` 45568188816.
- `e2e` 45568188795.
- `k3s-smoke` 45568189013.

**On `347a17f`, read at 08:17 UTC.**

- **Green:**
  - `skyzfs-control-rig` 45574074720;
  - `skyzfs-publish-rig` 45574074544;
  - `skyzfs-nodeb-rig` 45574074826;
  - `e2e` 45574074561;
  - `skyzfs-ci` (push) 45574063378.
- **Still going:**
  - `ci` 45574074689 (test and unit-arm64 running; lint, s3-backends,
    changes and chart-lint green);
  - `skyzfs-daemon-rig` 45574074524 (three daemon jobs);
  - `skyzfs-replay-rig` 45574074581;
  - `skyzfs-cut-daemon` 45574074692;
  - `coverage` 45574074459;
  - `k3s-smoke` 45574074549.

  The daemon is the same code as at `b79083a`: no crate changed by
  `347a17f` is a dependency of `skyzfsd`.

### Decisions for the lead

- **N1.** How b2's assist makes a group that writes on an idle pool:
  - a small write;
  - `zpool sync -f`;
  - a sync task of the agent's own.

  Also: a kernel check that a counted cut follows, and whether a
  publish that never lands is bounded.
- **D4's text.** Whether "What is left" in `cuts.rs:139-141` (and the
  design's §5.2 correction at the merge) names any case that costs a
  publish. As read here, it does not.

### Files here

- `review142b.rs`: the scratch scenarios of the assist and the
  `held_until` row (`review_n1` … `review_n11`). Included at the end of
  `plan/tests.rs` in a scratch worktree; they pass at the head.
- `review142c.rs`: the path cases of `parent_of`, and the ignored save
  from a relative store that was traced. Included at the end of
  `record/tests.rs`; they pass at `347a17f`.
- `plants142b.py`: the plants. `plants142b.out` (at `b79083a`) and
  `plants142b-347a17f.out` (all plants and the D plants, at `347a17f`)
  hold the results.
