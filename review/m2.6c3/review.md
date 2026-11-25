## Review of #149 (M2.6c-3): the program `skyzfs-agent`, head af50814

**The head.** Judged at `af50814`, checked with `git fetch origin` and
the pull request's `head.sha` before and after the work; it did not
move. Base `zfs-topology`. The branch sits on #146 (`0213a6c`) and #147
(`0f709ae`), neither merged, so the diff against the base carries
theirs. This review covers the package's own changes only:
`c21048c..af50814` less the merged commits, that is `b3410a7` (the
program, 14 files) and `af50814` (the merge's fixup: a test hook's
signature and the doc link in `queue.rs`). The two merges (`aac12a5`,
`f787641`) were read with `git show --remerge-diff`: their conflict
resolutions in `engine/tests.rs` and `record.rs` keep both sides and
add nothing else. Findings in #146's and #147's code are theirs; one
below (B1) repeats in this package a defect class an earlier review of
the planner found and had fixed.

**Verdict: it cannot merge yet. B1 blocks it.**

- **B1 (defect, blocking).** A person's give-up (`recover --give-up`)
  releases the hold and gives the publish up while its request still
  pends, when skyzfsd's uploader is in a long step and answers
  `progress` and `withdraw` with `refused no-answer`. The request then
  lands, and if the snapshot was destroyed after the give-up, `snap/t`
  names a cut without `pool@t`. Reproduced end to end on the fakes.
  It is N1 of #144's next look (fixed there for the age bound),
  reopened in the person's path. `recover --give-up` is the command a
  person reaches for exactly when a publish looks stuck, which is when
  the uploader is slow.
- **D2 (defect, should fix, does not block alone).** `publish` exits 7
  (unsettled) for a publish it made, when the publish had to take the
  next name (`skz-…-1`) because a dataset's `@t` of the first name
  appeared before the program ran. Reproduced.
- **The rest of the program is sound**: the parsing refuses what it
  should, the hooks are refused and not linked without the feature, the
  record dump reads without the lock and changes nothing, the record's
  version is handled as the lead ruled, and `Person::Stopped` is written
  through the same atomic save as every change.
- **Test gaps**: CI never builds the binary with `test-hooks` (T1, the
  author says so and proposes it for testing); the binary's own wiring
  is mostly untested (T2: 6 of 8 plants on `main.rs` survive).
- **Gates pass** for the touched crates in both feature modes (the
  workspace-wide gates could not run here: the shared disk was full).
  CI on the head: see "CI".

### Findings, most severe first

#### Defects

- **B1. A person's give-up lets the hold go while the request may
  still land. Blocks the merge.** Certain in the code; reproduced on the
  fakes with skyzfsd's answers as its control handler gives them.
  - **Where.** `crates/skyzfs-agent/src/plan.rs:483-512`, `given_up`.
    - Only `DaemonSeen::Away` (no answer at all to `cuts` or
      `progress`) waits for `held_until` (`plan.rs:486-488`).
    - Any answer that comes back is "here" (`plan.rs:492`). Of those,
      only `pending`, `accepted` and `refused asked` are read as a
      request that may pend (`plan.rs:494-497`).
    - `refused no-answer` (and `ended`, `no-upload`, `busy`) goes
      straight to `look`, then on `absent` to `Release`, then
      `Abandon(Person)`.
    - The planner's other give-ups read the same answers as a daemon
      away and keep the hold to `held_until`:
      `Code::Ended | Code::NoAnswer | Code::NoUpload => away()`
      (`plan.rs:842`, and the age bound's arm through `_ => away()`).
  - **Why `no-answer` means a request that may pend.** The engine reads
    the daemon as here when `cuts` is answered
    (`engine.rs:716-729`); `cuts` is answered at the store's desk and
    `look` from object storage, while `progress` and `withdraw` go to
    the uploader's inbox and are answered `refused no-answer` when the
    uploader has not answered within the limit
    (`bins/skyzfsd/src/control/handler.rs:181-190, 212-236`; its own
    text: "a publish may have been taken: ask again, or withdraw it").
    This is the reasoning of N1 in `review/m2.6b2/next_look.md`, which
    the lead accepted and #144 fixed in the age bound's arm.
  - **The planner's rows** (`r5_…` in `scratch149.rs`, output in
    `scratch149.out`). A publish asked, the hold there, `held_until`
    580 s away, no withdraw of its own answered yet, `look` absent
    (with `look` not yet made, the person's give-up first asks `look`):

    | Answer to `progress` | Person's give-up | Age bound's give-up |
    |---|---|---|
    | `refused no-answer` | `Release` | `Wait` |
    | `refused ended`, `refused no-upload`, `refused busy` | `Release` | `Wait` |
    | `unknown`, `expired`, `withdrawn`, `absent` | `Release` | `Withdraw` |

    The first row is the defect. `ended` and `no-upload` cannot hide a
    pending request (the uploader is gone, or there is none), and `busy`
    cannot come to `progress`; they differ from the other path but are
    harmless. The third row departs from "only once the daemon has
    answered a withdraw" (L5 on #144), which the description's "in the
    order of every give-up" promises; with skyzfsd answering one
    connection at a time in order (N5 of #144's third look) I found no
    loss from it. Once a withdraw of its own has been answered
    `withdrawn`, `unknown` or `expired` (`Seen::withdrawn`,
    `engine.rs:840-842`), nothing pends, and a release after a silent
    `progress` is safe; the scratch test asserts only without one.
  - **End to end** (`r1_…` and `r2_…` in `scratch149.rs`; output in
    `scratch149.out`). A daemon that passes everything to the fake but
    answers `progress` and `withdraw` with `refused no-answer`:
    - R1, busy pool, upload lag of a year: an agent stopped just after
      `accepted`; a second agent runs `recover --give-up` and settles.
      Outcome `Abandoned(Person)` within the first rounds, holds `[]`,
      the request still pending in the daemon.
    - R2, upload lag 60 s, in two of the fakes' pool modes: the same
      give-up; the person then runs `zfs destroy -r pool@t` (no hold
      refuses it), and the pool writes one more group. Idle pool: the
      cut after the destroy (group 9, cut 5) replaces the cuts that
      waited (up to the release's, group 8, which held `pool@t`), is
      committed, and `snap/t` is made on it. Abandoned mode: cut 5 is
      abandoned, its map goes to the next cut (group 10), and `snap/t`
      is made on that. Either way `snap/t` names a cut without
      `pool@t`: the wrong publish the hold exists to prevent
      (design §5.2, "the agent places a hold on the snapshot before it
      asks the daemon to publish and releases it after the ref is
      swapped"). Without the destroy, `snap/t` still appears for a
      publish the agent printed as `abandoned t person` and recorded so.
  - **The author's tests do not see it**: the table
    `a_person_gives_up_in_the_order_of_every_give_up` has no row for
    a refusal other than `asked` and `store`, and the engine test uses a
    daemon that answers.
  - **Fix.** Read the silent answers as the daemon away in `given_up`:
    before `held_until`, `refused no-answer` (and, for symmetry,
    `ended` and `no-upload`) is `Wait` for a publish asked. Plants M16
    and M17 (`plants149.py`) are two such one-line fixes; the PR's
    143 library tests all pass with either (they are "SURVIVED" in
    the table below, which here means the fix breaks nothing). Simpler
    still, and literally "the order of every give-up": give the
    person's give-up to the age bound's arm (`onward`'s `ended`
    branch) with `Why::Person`, keeping the person's two differences
    (a `done` that `look` does not confirm is given up, not stopped;
    a publish made is published). Add rows for `no-answer` before and
    after a withdraw to the table, and R2 as an engine test.

- **D2. `publish` exits 7 for a publish made under its next name.**
  Should fix; does not block alone. Certain; the path is rare.
  - **Where.** `crates/skyzfs-agent/src/program.rs:611-619`. `execute`
    looks for the outcome by the name `begin` returned. The engine
    renames a publish when the program finds a dataset's `@t` of that
    name (`engine.rs:959-972`, tested by
    `a_name_a_person_took_first_is_passed_over`). The outcome is then
    under `skz-…-1`, `find` gives `None`, and `exit_of(None)` is
    `UNSETTLED`.
  - **Scenario** (`r3_…` in `scratch149.rs`): a child's `@t` of the
    chosen name is made between `begin` and the program. Output:
    `code=7 out="snap/skz-20261125T010000Z-1 0400…\n"`. A rig or a
    supervisor reads "still under way at the limit" for a published
    snapshot and may publish again.
  - **When.** `fresh_name` checks only `pool@t` (`engine.rs:1193`), so
    a child's `@t` without the root's (a received dataset, a root
    snapshot destroyed without `-r`, another tool) also reaches it.
  - **Fix.** Follow the rename: the engine can return the publish's
    current name (or keep a map of renames), and `execute` look it up.
    Plant M18 (exit by `min(own, worst)`) shows a crude version breaks
    no test, but the right fix keeps "by its own publish".

#### Suggestions

- **S1. Outcome lines are printed only at the end of a run**
  (`program.rs:598-601`). A run killed after a publish left the record
  (SIGKILL, a test hook, systemd's stop) never prints its `snap/t` line,
  and no later run can: the record no longer has it. A `settle` of up to
  three hours prints nothing on standard output until it ends. Print each
  outcome as it happens, through a sink as the reports are.
- **S2. `record --dir` of a directory that does not exist prints
  `absent` and exits 0** (probe in `probes149.out`). A typo reads as "no
  record". Say "no such directory" and exit 1; keep `absent` for a
  directory without a record.
- **S3. A value option swallows the next option.** `recover … --name
  --give-up --retry` parses as name `--give-up`, choice retry, and runs
  (it then fails `NotUnderWay`, exit 1). `--volume --run R …` fails with
  `"R" is not an option`. Refuse a value that begins with `--`.
- **S4. `print!` and `println!` panic on a closed pipe**
  (`bins/skyzfs-agent/src/main.rs:42, 57`): `skyzfs-agent record --dir D
  | true` ends with "failed printing to stdout: Broken pipe", a
  backtrace and exit 101 (probe); `| head -1` does the same with a
  record longer than the pipe holds. Write with `write!` and ignore or
  map the error, as `execute` does.
- **S5. The exit table says more than it does.** 5 ("the pool's premise
  refused") comes only from `publish`'s `begin`; under `settle` a
  premise refused is `abandoned t premise` and 3. An error in the
  settling after `publish` made its own publish exits 1, with its
  `snap/` line printed. Say both in the module docs and the usage.
- **S6. `--help` after a command is "--help needs a value"** (exit 2).
  Nit.
- **S7. The feature `test-hooks` names a dependency that exists only on
  Linux** (`bins/skyzfs-agent/Cargo.toml:19-25`): on another target the
  feature does not build. Nothing builds there yet (goal: macOS and
  Windows clients). Nit.
- **S8. The binary's wiring could live in the library**, as everything
  it decides already does, so that the engine's fakes test it: see T2.
- **S9. The binary's docs overwrite the library's.** The bin target
  `skyzfs-agent` and the library `skyzfs_agent` document to the same
  `target/doc/skyzfs_agent/index.html`; `cargo doc` warns "output
  filename collision" (a cargo warning, so `RUSTDOCFLAGS=-D warnings`
  does not fail it), and in the gates' run the page left there was the
  binary's (functions only, no modules). A workspace `cargo doc` loses
  the library's front page. `doc = false` on the `[[bin]]` fixes it.
  Nit; CI does not build docs.

#### Test gaps

- **T1. CI never compiles the binary with `test-hooks`.** `ci.yml` and
  `skyzfs-ci.yml` run clippy and the tests with the daemon's and the
  binding's features only (`ci.yml:92-98, 169-174, 206-211`). Plant M3
  (the points never attached to the agent, so `--kill-at` and
  `--trace-points` do nothing) passes the plain suite and is caught only
  by `cargo test -p skyzfs-agent-cli --features test-hooks`. The rigs of
  M2.T5 will run that build. The author proposes adding it as testing's
  change; it should be in before a rig depends on the hooks, on amd64
  and in `unit-arm64` too. CLAUDE.md's "Two cargo features hold code
  that the workspace's tests do not build" becomes three.
- **T2. The binary's wiring is untested.** Of eight plants on
  `main.rs`, six survive both builds: exit 1 instead of 8 for another
  agent's lock (M1); `configured` skipped, so `--expiry-s`,
  `--assist-after-s` and `--age-max-s` are silently ignored by the real
  program (M2); reports written to standard output (M4) or not at all
  (M5); an unreadable record exiting 6 (M6); the volume's id read as
  zeros (M8). M2 and M8 matter most: M2 would make a rig's short expiry
  or age bound silently the default, and M8 would key the record to the
  wrong volume. Exit 8 is cheap to test (hold the directory with
  `Store::open` in the test, run the binary). The rest needs a fake
  daemon socket in `tests/program.rs`, or the wiring moved into the
  library (S8).
- **T3. The person's give-up table has no row for the silent answers**
  (B1) and the engine test only a daemon that answers.
- **T4. No test of a publish renamed through `execute`** (D2).

### The questions asked

**1. Exit codes and `settle`'s worst outcome.**

| Code | When |
|---|---|
| 0 | published (`publish`, `recover`: its own); `settle`: every outcome published, or none under way; `record` printed; `--help` |
| 1 | any error: ZFS, the daemon at `begin`, the file system, the record not saved, 8 under way (`Full`), `recover` of a name not under way, `remote` missing or malformed, a trace that cannot be written, a record that cannot be read by `record` |
| 2 | usage, an argument not UTF-8, a hook in a build without them |
| 3 | abandoned (own publish; `settle`: the worst) |
| 4 | stopped for a person |
| 5 | `publish`: the premise refused at `begin` only (S5) |
| 6 | the record corrupt (any version but 2 included), or another volume's |
| 7 | unsettled at the limit; also D2 |
| 8 | another agent holds the directory |
| 101 | a panic (S4) |
| 137 | the `--kill-at` hook (signal 9) |

- `settle`'s order is fixed and total: an error ends the run first
  (1, 5 or 6), then unsettled (7) > stopped (4) > abandoned (3) >
  published (0). It does not depend on the order of the outcomes. The
  order 7 over 4 is pinned by `settle_exits_by_the_worst_outcome`
  (plant M9 killed), so it is deliberate (decision L2).
- What a code can hide. `publish` and `recover` exit by their own
  publish, so another publish's stop or give-up shows only on standard
  output; documented and tested. Under `settle` a stop hides behind 7,
  and every outcome behind an error. A give-up never hides a stop. A
  hold kept for a daemon away shows as 7 with the `held-away` report on
  standard error. The record not saved is 1. The lines on standard
  output say the rest, but only if the run ends (S1).
- Errors go to standard error, reports to standard error before the
  act, outcomes to standard output. Nothing printed holds a secret:
  paths, the pool's name and GUIDs, hashes; `remote` holds no
  credentials (`bins/skyzfsd/src/remote.rs:34-37`), and the agent quotes
  at most one of its lines when the file is malformed.

**2. A person's stop outlasting a restart.**

- `Person::Stopped` is written by `commit(Effect::Stop, …)`
  (`engine.rs` in `act`, the `Stop::DoneNotSeen` arm), which is
  `Store::save`: a file beside the record, synced, renamed over it, the
  directory synced, as #142's review established. A kill before the
  save plans the same again: the daemon still says `done` (or, after
  its restart, `unknown`, and the publish is asked anew as before this
  package); a kill after it re-stops at once.
- A later round checks the stop before it reads anything
  (`engine.rs:672-677`) and the planner returns `Stop` for it whatever
  is seen (`plan.rs:438-442`); plants A2 and A3 are killed.
- No request is live at the daemon for a stopped publish: the stop is
  recorded only on `done`, which ends the request, and nothing is asked
  for it after. No withdraw is needed.
- A stop is keyed by the publish's name in its record. It is never
  renamed (a rename happens only at the program, at Intent). An agent
  run with another `--dir` does not see it, and that agent's sweep
  releases its hold (decision L4); one run with another `--pool` is
  refused by the premise.
- There is no race inside one agent; a second agent is refused the
  directory (exit 8). A person who wants to `recover` while a `settle`
  runs waits for it, up to the 3 h limit, or kills it (safe by the crash
  matrix); the usage could say so.
- The version: v2 is amended in place with a struct field
  (`Publish.person`), not only an appended variant. That breaks
  decoding of a v2 record written before it, which is what "a new
  version" exists for. It is consistent with the ruling as the record's
  docs state it (`record.rs:28-39`): no agent ran v2, this program is
  the first that can, and from its first run any change takes a new
  version. The golden bytes are re-pinned with every `Person` value and
  `Why::Person`. No rig writes a record (the publish rig's agent is a
  Python stand-in).

**3. A person's give-up through the order of every give-up.** The table
of (phase × daemon answer × look) is in `scratch149.out` (R5), against
the age bound's arm. Where the daemon answers plainly it agrees:
a pending request is withdrawn, then looked up, a publish made is
`Done`, else released and given up; Intent and Taken give up at once
(nothing could have been asked). It departs in three places:

- the silent answers (B1);
- the answers `unknown`, `expired`, `withdrawn` from `progress`, given
  up after a look without a withdraw of its own (B1's table, harmless
  as far as I found);
- a publish held and not recorded asked, with the daemon away, is
  released at once (`plan.rs:489`). The module docs say so and why
  (`plan.rs:146-153`): only an ask accepted and not recorded (a stop
  at step 8) could still land, and "the person's command is the bound
  the rules otherwise lack". The age bound's arm keeps that hold
  without limit (N1 of #144's next look: "or at all (held)"). This is a
  decision for the lead (L1), not a defect.

Reported at least once: a person's give-up makes no report; its outcome
line is printed at the end (S1). No new loop without a tick: each
action of `given_up` either waits, or is a withdraw, look or release
whose answer is kept for the next round, so each round moves to the
next step; a withdraw answered `pending` or `refused asked` would
withdraw again without a tick, as the age bound's arm would (#144's
code, not new here).

**4. The test hooks without the feature.** Built both ways.
- Without it, each of `--kill-at`, `--trace-points`, `--assist-after-s`
  and `--age-max-s`, as `--k v` and `--k=v`, is a usage error that says
  "test hook" (probes, exit 2); `record` refuses them as options it does
  not take. They are refused before any other check.
- No environment variable stands for them: the program reads argv only.
- The release build without the feature links no hook code: no
  `killed at`, no `--trace-points: ` text, no `kill_process` (the
  library's `program::points` is not referenced and is dropped). The
  refusal's and the usage's words are there, as they should be.
- The feature is not on by default and nothing in the workspace turns it
  on: `cargo tree --workspace -e features -i skyzfs-agent-cli` shows
  only `default` (empty), from the command line. It is a binary-only
  package, so no member can depend on it.
- Plants A9 and M7 (the hooks let through) are killed, by the library's
  parse test and the binary's plain test.

**5. The record dump.** `record::peek` reads the file with no lock and
creates nothing (a missing directory is not made; S2). A save renames a
whole file over the record, so a read sees one record or the next. A
torn, corrupt or other-version file loads `Corrupt` and exits 6 with
"the record is corrupt: …" (tested, `record` on "not a record"); a
directory where the record should be exits 1 (probe). It prints ids,
GUIDs, hashes and times, nothing sensitive. The `ms` fields are raw
milliseconds; fine for rigs.

**6. Anything else.**
- Arguments: unknown options, repeated options, missing values,
  positional words, zero or fractional seconds, an expiry over the
  maximum, `--retry=yes`, and both or neither of `--give-up`/`--retry`
  are refused (probes, `probes149.out`); S3 and S6 are the gaps.
- Signals: none are handled. SIGTERM or SIGINT ends the agent as a kill
  does; the record is never half written (atomic save), and a hold
  placed is always recorded first (`Holding` before `zfs hold`), so the
  next run meets it. A `zfs` child left running finishes on its own
  and is met the same way (the crash matrix). Lost: the outcome lines
  (S1).
- No `unwrap()` or `expect()` outside tests in the package's code.
- Cargo.lock adds only the package `skyzfs-agent-cli`; no new external
  crate (`rustix` 1.1.4 and `tempfile` were already there).
- Docs that overstate: the module docs and the description say a
  person's give-up withdraws a request that may pend first (B1) and
  that `publish` exits by its own publish (D2).
- Do the tests fail when the behaviour breaks? For the library, yes:
  all 10 of the author's plants re-made are killed, and my plants on
  the library are killed but the ones that are fixes (below). For the
  binary, mostly not (T2).

**7. Gates and CI.** See "Gates" and "CI" below.

### Plants

`plants149.py`, output `plants149.out`. Each applied alone to a clean
worktree of `af50814`, its suites run with a 600 s limit, the files
restored in a `finally`. LIB is `cargo test -p skyzfs-agent --lib`, BIN
`cargo test -p skyzfs-agent-cli`, BINH the same with `--features
test-hooks`.

| Plant | What it breaks | Result | Caught by |
|---|---|---|---|
| A1 | the stop not recorded | KILLED | LIB, 3 tests |
| A2 | a recorded stop not checked in the round | KILLED | LIB |
| A3 | a recorded stop planned as the agent's own | KILLED | LIB |
| A4 | give-up: no wait for `held_until` while away | KILLED | LIB |
| A5 | give-up: no withdraw | KILLED | LIB, 2 tests |
| A6 | give-up: a publish made given up | KILLED | LIB, 3 tests |
| A7 | `worst`: abandoned over stopped | KILLED | LIB |
| A8 | `publish` exits by the worst, not its own | KILLED | LIB |
| A9 | the hooks allowed without the feature | KILLED | LIB |
| A10 | the trace written after the kill | KILLED | LIB |
| M1 | bin: another agent's lock exits 1, not 8 | SURVIVED | (T2) |
| M2 | bin: `configured` skipped (expiry and hooks not applied) | SURVIVED | (T2) |
| M3 | bin: the points never attached | KILLED | BINH only (T1) |
| M4 | bin: reports to standard output | SURVIVED | (T2) |
| M5 | bin: reports not written | SURVIVED | (T2) |
| M6 | bin: an unreadable record exits 6, not 1 | SURVIVED | (T2) |
| M7 | bin: the hooks always taken | KILLED | BIN |
| M8 | bin: the volume's id read as zeros | SURVIVED | (T2) |
| M9 | `settle`: stopped over unsettled | KILLED | LIB (L2) |
| M10 | another volume's record exits 1, not 6 | KILLED | LIB |
| M11 | dump: the person's words swapped | KILLED | LIB |
| M12 | unsettled names not printed | KILLED | LIB |
| M13 | a repeated option taken | KILLED | LIB |
| M14 | give-up: released at once for a publish asked, daemon away | KILLED | LIB |
| M15 | give-up: held and not asked, daemon away, waits | KILLED | LIB (L1) |
| M16 | fix probe for B1: `no-answer` read as a request that may pend | SURVIVED | none: the fix breaks no test |
| M17 | fix probe for B1: `no-answer`, `ended`, `no-upload` read as away | SURVIVED | none: the fix breaks no test |
| M18 | fix probe for D2: `publish` exits by the worst where its name moved | SURVIVED | none: the fix breaks no test |
| M19 | `recover --give-up` recorded as a stop | KILLED | LIB, 4 tests |
| M20 | the record not peeked (a missing record an error) | KILLED | LIB, 5 tests; BIN |

All ten of the author's plants are killed. Of mine, every plant on the
library that is not a fix probe is killed; on the binary, six of eight
survive (T2), and M3 is caught only by the build with the hooks (T1).

### Gates

`gates149.sh`, log `gates149.log`, on a clean worktree of `af50814`
with rustc 1.100.0 (`rustup check`: up to date).

| Gate | Plain | `--features skyzfs-agent-cli/test-hooks` |
|---|---|---|
| `cargo fmt --all --check` | pass | (one run) |
| clippy `-p skyzfs-agent -p skyzfs-agent-cli --all-targets -D warnings` | pass | pass |
| tests, the same crates | 143 library, 4 binary: pass | 143 library, 4 binary: pass |
| `cargo doc --no-deps`, `RUSTDOCFLAGS=-D warnings` | pass, with S9's warning | pass, with S9's warning |
| release build without the feature | no hook code linked (Q4) | |
| `cargo tree --workspace -e features -i skyzfs-agent-cli` | only `default`, from the command line | |

Not run here: the workspace-wide clippy, tests and doc, in any mode.
The shared disk had under 300 MB free throughout (and 164 MB at the
end), too little for a workspace build beside the others' targets.
`af50814`'s change to `crates/skyzfs-ublk-sys/src/queue.rs:373` is in a
doc comment only (an intra-doc link to a private function made code
font; read in the diff), and its crate was not rebuilt here. CI's
`lint`, `test` and `unit-arm64` jobs run the workspace's fmt, clippy
and tests (below); no CI job builds docs, so neither that fix nor S9
is seen there.

### CI

Read with `gh api repos/skyzfs/skyzfs/actions/runs?head_sha=af50814…`
at 18:20 UTC on 2026-11-25, while the head was still `af50814`. Not
finished then, so not a verdict:

| Workflow (pull request) | State |
|---|---|
| `ci` | `lint`, `chart-lint`, `changes`: success; `test`, `unit-arm64`, `s3-backends`: running |
| `coverage` | running |
| `e2e`, `k3s-smoke`, `skyzfs-nodeb-rig` | success |
| `skyzfs-control-rig`, `skyzfs-cut-daemon`, `skyzfs-publish-rig` | success |
| `skyzfs-daemon-rig` | judge job: success; the three daemon jobs: running |
| `skyzfs-replay-rig` | planning job: success; the four "through skyzfsd" jobs: running |

The `skyzfs-ci` push run on the head ran its gate only (its jobs were
skipped). None of these builds `skyzfs-agent-cli` with `test-hooks`
(T1), and none drives the program: the publish rig's agent is still the
Python stand-in, so a green rig says nothing about B1 or D2.

### Decisions for the lead

- **L1.** A person's give-up of a publish held and not recorded asked,
  with the daemon away (and, once B1 is fixed, silent): released at
  once, as the PR reads it, or kept as the age bound keeps it? The
  author's table pins the first (plant M15, the conservative reading,
  is killed by it), so it is deliberate. A middle
  way keeps the person's command as the bound without releasing while
  a step-8 ask may land: at the give-up, record a time of
  now + `EXPIRY_MAX` + `HOLD_MARGIN` (any ask accepted before the
  give-up has expired by then) and release after it.
- **L2.** `settle` ranks unsettled (7) over stopped (4). A supervisor
  that retries on 7 learns of a stop only from standard output while
  anything else is under way. Keep (pinned by a test, so deliberate), or
  rank 4 first since only a person ends it?
- **L3.** The third feature in CI (T1), and CLAUDE.md's count of
  features, before M2.T5 runs on the hooks.
- **L4.** The sweep of an absent record releases every hold of the tag
  on the pool (the binary's own test does it on a fresh `--dir`). The
  program makes `--dir` a free argument, so a mistyped directory
  releases the holds of another record's publishes under way, including
  one whose request pends or one stopped for a person, and prints
  nothing. #144's sweep, not this package's code, but this package
  exposes it: should the program bind a pool to its directory, or the
  sweep of an absent record be refused while holds exist?

### Files

- `review.md`: this note.
- `scratch149.rs`, `scratch149.out`: the scratch tests R1 to R5 (copied
  into the worktree as `program/review_scratch.rs`, never committed to
  the product) and their output.
- `plants149.py`, `plants149.out`: the plants and their results.
- `gates149.sh`, `gates149.log`: the gates.
- `probes149.sh`, `probes149.out`: the command-line probes on the
  release binary.
