# Review of #153 (M2.6c-2b) at 65d72ee

This note judges head **65d72eef805fc3a33ada4098fcaaf4aca18e6eff** of
`skyzfs-coding-m2.6c-2b`: the two commits 80a6b1d and 65d72ee on a5e883a,
14 files (`git diff origin/zfs-topology...origin/skyzfs-coding-m2.6c-2b`).
Read first: design §6.2 as changed at e6788b0 and 918cf59, the
implementation notes for M2.6 (Q10), the earlier notes in `review/m2.6c2/`,
`review/q10/` and `review/m2.6c3/`, and the description.

## Verdict

**Can merge at 65d72ee once the recording of the full read is in (#154),
with D1 fixed or ruled on by the lead.** None of the review's searches found
a wrong publish, a wrong manifest, a hold released while a request may pend,
a hold or record left behind, or a snapshot destroyed outside the lead's
rule. The bracket, the wholeness check, the adoption of a late take, the
destroy and the rename hold against every answer and person's action tried.

What blocks:
1. **The full read and the count have not run on 26.04 yet.** This is the
   author's open item 2, and #154 says this PR waits on it. The 24.04 half is
   in (run 45720134548, with the Lua of this head) and reads correctly here
   (below). The 26.04 run with scale 100, 1000 and 10000 (45720225770) was
   still running when this was written. Its time for the full read at 10000
   decides whether the read limit holds (T5).
2. **D1, the hot loop, needs a decision.** It is new in this PR and small
   to fix. I would fix it here, but whether it blocks is the lead's call. It
   does not affect correctness.

Codex: completed on 65d72ee with no findings. CI on 65d72ee: green (ci with
test, lint, unit-arm64, s3-backends and chart-lint; coverage; e2e;
k3s-smoke). The PR touches only `skyzfs-agent`, so no rig ran.

## Defects

### D1 (medium): a take that is never whole is destroyed and taken again in a loop with no pause

- **Where:**
  - `crates/skyzfs-agent/src/engine.rs:899` and `:904` (a take that is not
    whole, or not as before, goes to `retake`);
  - `:953-962` and `:966-980` (`retake`, then `rename`, return `Moved::Yes`);
  - `:516-522` (`settle` sleeps one `TICK` only when no publish moved);
  - `crates/skyzfs-agent/src/zfs.rs:304` (a dataset whose createtxg is at or
    below the group and that has no `@t` makes the take not whole);
  - `crates/skyzfs-agent/src/zfs/cli/members.lua:88` (the full read walks
    every child).
- **Scenario.** One dataset of the pool cannot get an `@t` from `zfs snapshot
  -r`, yet the full read lists it. Example: a full `zfs receive` into a new
  dataset of the pool, either under way or interrupted with `-s`.
  - In OpenZFS, libzfs's `zfs_snapshot_cb` skips a dataset (and its children)
    whose `inconsistent` property is set.
  - The channel program's children iterator skips only `$` and `%` names.
  - `dsl_dataset_snapshot_check_impl` refuses a snapshot of an inconsistent
    dataset with EBUSY, so not even an explicit take would cover it.
  - So every take reads as not whole. The agent runs `zfs destroy -r`, takes
    the next name, takes again, and so on, with no pause between rounds. Each
    turn writes two groups that the daemon cuts and uploads, plus record
    commits.
  - This goes on until the publish is two hours old (`AGE_MAX`, given up
    `TooOld` at Intent) or `settle`'s limit, and again for the next publish.
- **Shown in the fake.** Probe PB in `scratch153.rs` adds a dataset that the
  fake's take skips. The agent took and destroyed 399 times at one instant
  of the world's clock, and then the fake's own livelock guard stopped it
  ("2000 questions ... with no time passing"). The names alternate between
  `t` and `t-1`: `fresh_name` reuses a name that was destroyed within the
  same second. That is safe, because ZFS runs sync tasks in order, so a take
  still in the kernel lands before a later destroy of the same name.
- **Against the base.** The program take could not publish in this case
  either: EBUSY at the inconsistent dataset, then it destroyed what it took.
  But it waited one `TICK` between attempts. The loop with no pause is new.
- **Certainty.** High that the loop follows a take that is never whole
  (shown). Medium for the trigger on real ZFS: it rests on a source reading
  and has not been run. Testing could confirm it in the recorder: interrupt a
  `zfs receive -s` into a new dataset, then run `zfs snapshot -r` and the full
  read.
- **Possible fixes**, the last one for the lead:
  - (a) do not count a retake as a round that moved, so the next round waits
    one `TICK`;
  - (b) bound the retakes a publish may make in a row, and report;
  - (c) exempt a dataset that cannot be snapshotted (`inconsistent`, or
    `receive_resume_token` set) from wholeness. The restore destroys a
    dataset the manifest does not name, which for a partial receive is
    arguably right. This is a rule of design §6.2.

No other defect found.

## Notes for the lead (decisions, not defects)

- **N1. "The agent's own" is decided by name only.** This is the lead's rule.
  - A `pool@t` that a person took under the agent's exact name is adopted
    when it is whole and as before. Otherwise `zfs destroy -r` destroys it,
    including a member's `@t` from another group (`zfs.rs:291`, NotWhole).
  - The search accepts both, by the rule (`World::retakes`).
  - Possible hardening: `zfs snapshot -r -o skyzfs:take=<nonce> pool@t` sets a
    user property on every `@t` in the same sync task. The full read could
    return it, so the adoption would know its own take, and which attempt.
- **N2. The limits, against the figures.**
  - **The arithmetic** (change limit 120 s + 0.1 s × n, read limit 60 s +
    0.1 s × n, both capped at 1200 s):

    | Datasets | Change limit | Measured | Read limit |
    |---|---|---|---|
    | 101 | 130.1 s | 0.9 s | 70.1 s |
    | 1001 | 220.1 s | 5.2 s | 160.1 s |
    | 10001 | 1120.1 s | 391 s (2.9× margin) | 1060.1 s (`tagged.lua` 68.7 s) |

    The change limit reaches the cap at 10 800 datasets, the read limit at
    11 400; the test checks 10 800.
  - **Units and overflow.** Units are right (P13 killed). A count above
    u32 gives the cap, and the sums saturate (P17, the branch past u32,
    cannot be reached).
  - **The count.** It is never 0 while the pool exists, since the root is
    listed. It is read once per round, so it is stale by at most what a
    round creates.
  - **Where the cap starts to bite.** A power law through 5.2 s at 1000 and
    391 s at 10 000 (exponent 1.88; one run each, one runner) puts
    `zfs snapshot -r` past the 20-minute cap from about 18 000 datasets. From
    there every take is killed and adopted only when it lands late.
  - **A failed count gives every call the cap.** On a pool that holds every
    command, one round can wait the count's 60 s plus three calls of 20
    minutes before `STUCK_MAX` (4) makes calls fail at once: about 61
    minutes, against about 6 to 7 with the old fixed limits.
  - **`settle` can overrun its limit.** It checks its limit only between
    rounds, so a round can run past it by that much.
  - **`begin` runs on the base limits** (no count before it). That is fine
    at the measured sizes: the members read at 10 000 took 7 to 22 s
    against 60 s.
- **N3. FL-N1 under a limit that is too small.** More takes are killed in
  their ioctl, so more land late.
  - The adoption handles them: S1 and S2 found no wrong publish with a
    landing after 0 to 6 calls, or only at the next take.
  - The cost is a second `zfs snapshot -r` (the whole listing again, then
    EEXIST) and one stuck child for each kill.
  - Where a publish is given up at Intent while its take is still in the
    kernel (`TooOld` at the age bound, or `Premise` after a layout change;
    `plan.rs:433` and `:435`), the snapshot appears after the give-up, and
    nothing destroys it. It is attributable, since the give-up record carries
    its name. It is consistent with "given up, snapshot kept", but the fake
    never shows it: its pending take lands only on the agent's calls, and
    after the give-up the agent makes none. The search lands it after the
    run and counts what is left over.
- **N4. A property changed and changed back between the two readings is not
  seen.** The design says so (918cf59). The fake's `SetProperty` always sets
  a new value, so this cannot be shown there.
- **N5. A destroy killed at its limit that goes through anyway gives
  "ManifestLost, kept" for a snapshot that is gone.** This is the author's
  open item 4. The search's destroy mode 2 shows that the committed oracle
  flags it, as expected.

## Test gaps

- **T1. Three plants on the bracket survive the committed suite and are
  killed by the review's probes** (`plants153_search.out`; PA and PM in
  `scratch153.rs`, judged by an oracle that matches by GUID):
  - **P3**, the name not compared in `as_before`: a dataset renamed just as
    a late take lands publishes the new name for a take that had the old
    one (8 of 32 PA runs, 11 PM runs).
  - **P5**, the number of datasets not compared: a dataset destroyed with
    its snapshots just as a take lands publishes a take without that member
    (8 and 11).
  - **P7**, no early return in `take()` when `pool@t` is already there: a
    property set or a dataset destroyed between a late landing and the
    agent's next reading publishes the wrong manifest (3 and 3).

  The fake has no rename, no creation and no destruction of a dataset, and
  no action at the moment a late take lands, so no committed test can show
  these. Suggestion: add those actions to the fake, plus a late-take axis to
  the persons' matrix.
- **T2. Three more plants survive the probes too.**
  - **P6** (kind not compared) is equivalent, since a GUID cannot change
    kind.
  - **P8** (rename keeps the old reading) is near-equivalent: it matters only
    when a `pool@t2` lands before the first take of `t2`.
  - **P2** (GUID not compared) would need a dataset destroyed and created
    again with the same name and properties between the two readings. No
    test or probe does that.
- **T3. The Lua plants P20 and P21 survive**, since no ZFS runs locally.
  - The 24.04 recording of the full read, made by #154 with this head's
    Lua, parses with this head's parser (probe R154): three whole takes, at
    groups 32, 43 and 44.
  - Local properties, as expected:
    - root: `compression`, `mountpoint`;
    - child: `skyzfs:note`, and the odd values in hex;
    - vol: `refreservation`, `volsize`.
  - The partial and absent reads come back as Absent.
  - A recordings test on that fixture would kill both plants: P20 would make
    every recorded take not whole, and under P21 the child would carry the
    root's `compression`. Suggested for when the fixture is copied in.
- **T4. The crash and persons' matrices do not cross late takes.** The
  review's S1 does (15 162 runs) and found no wrong publish. There the
  committed oracle would need two allowances, both of which the review
  checked by hand:
  - a take a person destroyed before the adoption ("neither published nor
    given up": the agent goes on under the next name);
  - a dataset renamed after the take (the cut holds the `@t` under the new
    name).
- **T5. The full read's time at scale is not measured.** The old program's
  843 to 960 s at 10 000 included reading every property. If the full read
  passes 60 s + 100 ms per dataset, every round's reading before the take
  fails, and no publish is made on that pool. #154's 26.04 scale run will
  say.

## What the review did

- **Gates at 65d72ee** on a clean archive of the head, own target
  directory, debug info off, rustc 1.100.0 (`rustup update stable`: already
  current). Script: `gates153.sh`.
  - `cargo fmt --check`: clean.
  - `cargo clippy -p skyzfs-agent -p skyzfs-control --all-targets -D
    warnings`: clean.
  - `cargo test`: 136 passed in skyzfs-agent and 16 in skyzfs-control.
  - `cargo doc -p skyzfs-agent -p skyzfs-control --no-deps` with `-D
    warnings`: clean.
  - `skyzfs-agent-cli` and its `test-hooks` are #149's, not on this branch:
    not applicable.
- **Fixtures.** `recorded/ubuntu-24.04.json` and `ubuntu-26.04.json` are
  byte-identical to the `recorded-ubuntu-*.json` of runs 45703760219 and
  45703752703, and to a reconversion of those runs with f47d864's
  `zfs_cli_json.py`. The converter on `zfs-topology` (0197318) adds only
  `note`: 28 per image of 160 records, all "seen" or "caught at try".
- **The tests removed.** There are six, each listed in the description with
  where its check went. I checked each against the diff: 125 tests at the
  base and 136 at the head, so 17 added and 6 removed.
- **Dead references.** None in `crates/` or `docs/` (historical mentions
  only: `cli.rs:107`, impl-notes Q10). The recorder's `snapshot.lua` belongs
  to #154, which makes it optional.
- **Plants** (`plants153.py`, `plants153.out`): 43 plants, one at a time,
  restored in `finally`, 600 s limit each.
  - The author's 17 (B1-B4, B7-B9b, B11, B13, L1, L3, L7, R1, M5, M6, A12):
    all killed.
  - The review's 26 (P1-P10, P12-P25, P27, P28):
    - killed: 17, among them not-whole accepted (P1), EEXIST destroyed or
      kept (P9, P10), the limit off by one dataset (P12), in microseconds
      (P13), uncapped (P14), a failed count read as 0 or as the bases (P15,
      P16, P19), the full read without `-n` (P22), the count without `-t`
      (P23), and a missing snapshot read as an error at the destroy (P24);
    - survived: P2, P3, P5, P6, P7, P8 (T1 and T2), P17 (unreachable),
      P20, P21 (T3).
- **Searches** (`scratch153.rs`, patched in by `scratch153_patch.py`;
  output in `search153.out`).
  - **The knobs on the fake:**
    - a take killed and landing after 0, 1, 2, 3 or 5 calls, or only at the
      next take;
    - a take that fails before its ioctl;
    - a full read that fails once;
    - a destroy that goes through, is refused, or goes through and returns
      an error;
    - a person's action at every point, from 10 actions: set a property,
      create, destroy or rename a dataset, retake, destroy a child, hold
      `pool@t`, release a child, attach, destroy.
  - **The runs:**
    - S1: those actions at every point, 15 162 runs;
    - S2: an action as a late take lands, and the agent stopped at every
      point, 4 868 runs;
    - S3: a failed take or read, 7 680 runs;
    - PA and PM: the probes for the plants;
    - PB: the inconsistent dataset (D1);
    - R154: the recording.
  - **The oracles:** the committed `check`, and a copy that matches a
    published member to its cut by GUID.
  - **Result.** No wrong publish, no wrong manifest, no release while
    pending, no panic.
  - **What `check` flagged, all explained:**
    - renames, by name (588 in S1 and 560 in S3);
    - a take a person destroyed before the adoption (18);
    - a refused destroy with a member gone (158, the scratch's destroy
      mode 1).
  - **What was left in the pool after the landing:** always under a give-up
    record (Premise, Replaced, ManifestLost, TooOld). Nothing was left
    without one.
  - Not searched: a rewind or a reboot while a take is in the kernel.
- **CI and Codex** were read through the API only. Nothing was posted.
