# Next look at skyzfs/skyzfs#168 (M2.6c-5f) at b30e452

Head checked with `gh api repos/skyzfs/skyzfs/pulls/168 --jq .head.sha`
at the start and at the end: `b30e452a0eff71e885cf4890e21ed1cd866de0dd`,
one commit on 85c4708, the head of my first review (`../review.md`). I
read `git diff 85c4708 b30e452` and formed the view below before I read
the author's answer (comment 7202990252), then checked the answer
against it.

## Verdict

**It can merge from review.** B1 is gone, and nothing else blocks.

- **B1:** fixed, and no other stray file is in the pull request.
- **L1** (accepted by the lead): written in `agents_name` and in the
  engine's sweep doc, and both are true.
- **N4':** "placed or released" at every site, true on each path.
- **TG2:** my four survivors (A2, A9, S2, N3m) now die, each by the
  test the description names and for the right reason. Of ten fresh
  plants on the new tests and the one code change, eight die. Two
  survive, both small test gaps (TG3).
- **N6** and **N7:** done, but for nits (N8, N7').
- **No behaviour changes** in non-test code but the message's words.
- **Gates:** fmt, clippy `-D warnings`, the tests and `cargo doc` with
  `-D warnings` are clean on all four feature sets, rustc 1.100.0; the
  counts match the description's. The agent's crate also docs clean
  with its private items.
- **CI at b30e452:** all six runs green, the agent rig's job k
  included (1015 pins ok, none failed, worst exit 0, the 36 K-sweep
  cases, all three controls caught). They built b30e452 merged into
  today's tip of `zfs-topology`.

What is left is two small test gaps and nits. None needs another round
before the merge.

## What the commit changes

`git diff --stat 85c4708 b30e452`: six files, +132/−2249, of which
−2214 is `cli.rs.orig`.

| File | What |
|---|---|
| `zfs/cli.rs.orig` | removed (B1) |
| `zfs/cli.rs` | docs, and one string: the "more lines than holds" message (N4') |
| `engine.rs`, `names.rs` | docs only (L1, N6) |
| `zfs/cli/tests.rs` | the message's text in its test; new cases in three tests (TG2) |
| `engine/tests.rs` | one new test (TG2) |

**No behaviour of non-test code changes but the message.** With every
line that begins `//` taken out, `cli.rs`, `engine.rs` and `names.rs`
differ in three lines, all in the message's string literal: "holds were
added between the two reads" became "holds were placed or released
between the two reads", reflowed. `engine/fake.rs` and `zfs.rs` are
unchanged. Nothing outside the crate reads that message: `git grep` of
the rigs, the checkers and the docs for its words finds nothing.

**The base.** `zfs-topology` has moved since 85c4708 (#166 and the
lead's notes, 3fa5a8e): `skyzfsd`'s control, docs in `skyzfs-control`, a
rustix feature. None of it is in the agent's crates. The pull request
merges with it cleanly (`git merge-tree`), and CI's pull-request runs
built that merge: the e2e job checked out `3c9488c`, "Merge b30e452
into 3fa5a8e".

## My first review's findings

### B1: fixed

- `git diff --name-only origin/zfs-topology...b30e452` lists seven
  files, all `.rs` in `crates/skyzfs-agent`: `engine.rs`,
  `engine/fake.rs`, `engine/tests.rs`, `names.rs`, `zfs.rs`,
  `zfs/cli.rs`, `zfs/cli/tests.rs`.
- `git ls-tree -r b30e452` has no `*.orig`, `*.rej`, `*.bak`, `*.swp`,
  `*~` or `*.diff`. The one `*.patch`, `tests/kernel/ublksrv-skz.patch`,
  is a real file already on `zfs-topology`.

### L1: written where the trade is, and true

- **`agents_name`** (`cli.rs:416-421`): a hold on a snapshot of the
  agent's that a person renamed while it was held (`zfs rename
  pool@skz-… pool@keep`) stays until the person releases it, accepted
  as the hand-placed tag was; and the prefix is matched case and all.
- **The engine's sweep doc** (`engine.rs:739-746`): only holds on
  snapshots with the agent's prefix are read (it links `names::PREFIX`
  and `Zfs::tagged`);
  a hold of the tag on another name, by hand or left by a rename, stays
  until a person releases it. "A hold of the tag is the agent's alone"
  became "… on a snapshot of the agent's name …".
- **`names::PREFIX`** (`names.rs:10-13`): the sweep depends on it, and a
  change of it would leave every hold on an older name unswept.
- **True?** Yes, against what my first review checked in OpenZFS 2.4.1:
  a rename does not look at holds; the publish's `zfs release -r TAG
  pool@skz-X` reaches only snapshots still named `@skz-X`; and
  `agents_name` is false for `@keep`, so no read of the agent's touches
  it again. (A rename to another `skz-…` name is still swept; the
  example makes plain that the doc means a rename out of the prefix.)
- **M3.** The note that design §5.4's restore must not be built on
  `Zfs::tagged` is in the description and the answer, not in the code.
  That is the lead's to carry into the notes.

### N4': fixed at every site, and true on each path

| Site | Now | True? |
|---|---|---|
| the message (`cli.rs:538-542`) | "holds were placed or released between the two reads" | yes: the sweep (count, then holds) gets more lines from a hold placed; `held`/`hold`/`release` (holds, then count) from one released |
| `outnumbered`'s doc (`cli.rs:515-521`) | names both orders, and "fewer lines … the change the other way round" | yes, both halves |
| `Odd`'s doc (`cli.rs:434-435`) | "a hold that changes between the two commands" | yes |
| `again_checked`'s doc (`cli.rs:976-977`) | "a hold that changed between its two commands" | yes |
| the module doc (`cli.rs:89-93`) | both orders, naming the sweep and `held`, `hold`, `release` | yes |

`git grep` finds "added between" nowhere in the crate's non-test code
now. In the tests it is left in three places, each right for its order:
`holds_that_do_not_add_up_…`'s doc (`cli/tests.rs:1971`) and the case at
`:2072`, both of the sweep (count first, so a hold added gives more
lines), and `:2360`, of `holds -r` (holds first, so a hold added gives
fewer lines, no error).

### TG2: all four die, for the right reason

Plants re-run on a scratch worktree of b30e452, each alone, against
both agent crates and every test target (`cargo test -p skyzfs-agent -p
skyzfs-agent-cli --features skyzfs-agent-cli/test-hooks --no-fail-fast`;
220 tests at the head), 600 s limit, restored after each; the worktree
was clean after all 15. `plants168n.py` and `plants168n.out` here; the
`.out` keeps the first lines of each failure.

| Plant | Killed by | Why it fails (the failing assertion) |
|---|---|---|
| A2, the prefix in any case | `the_sweep_reads_the_holds_of_the_snapshots_that_have_one` | `tests.rs:1645`, case `"tank@SKZ-1\t1\n"`: the sweep ran `zfs holds -H tank@SKZ-1`, which only the plant reads |
| A9, the fake's filter dropped | `engine::tests::the_sweep_leaves_a_hold_of_the_tag_on_a_name_not_the_agents` (new) | `engine/tests.rs:3719`: holds left `[]`, expected the three on `@keep`, `/db@SKZ-1`, `@skz` |
| S2, no final newline read again | `holds_that_do_not_add_up_are_an_error_that_names_the_snapshot` | `tests.rs:2070`: three list-and-holds reads, expected one |
| N3m, N3's message undone | `the_holds_under_a_snapshot_are_checked_as_the_sweeps_are` | `tests.rs:2308`: the message lacks "a line forged by a tag with a newline names a snapshot that never was" |

The author ran them against `skyzfs-agent`'s lib alone (213 tests); the
result is the same against both crates.

### Fresh plants on the new tests

| Plant | What | Result |
|---|---|---|
| N4pm | the message back to "added" (N4' undone) | killed: `holds_that_do_not_add_up_…`, by its exact text |
| A2u | `SKZ-` taken as well as `skz-` | killed: `the_sweep_reads_…` (`tank@SKZ-1`) |
| A9c | the fake's filter in any case | killed by the new engine test: `/db@SKZ-1` released |
| A9d | the fake's filter without its dash | killed by the new engine test: `@skz` released |
| A9f | the fake's filter on the pool's own snapshots alone | killed by the new engine test: `/db@SKZ-1` released |
| A9e | the fake's filter: the prefix anywhere in `t` | **survives** (TG3) |
| A7 | the CLI sweep's filter dropped | killed by `another_holders_odd_tag_…` and `the_sweep_reads_…`; **not** by the new engine test (N8) |
| N3a | N3's message without its advice (`; {ODD_HOLDS}`) | **survives** (TG3) |
| N3v | the count's missing snapshot an error at once, not `Vanished` | killed: `the_holds_check_reads_only_…`, `the_holds_under_…` |
| S2b | an output with no final newline taken as no holds | killed: `holds_that_do_not_add_up_…` |

Each of the new engine test's three names catches its own mutant (A9c,
A9d, A9f), and `@skz-stray` beside them shows that the sweep ran. The
test is not order-dependent: the fake's snapshots are a `BTreeMap`, and
`holds()` and the sorted `left` both read in byte order.

### N6: done, but for nits

| Item | At b30e452 |
|---|---|
| "after a read again" (`cli.rs:1212`) | now "after reads again where there are more lines than holds" (`cli.rs:1224-1225`). Right |
| "cannot forge … nor hide one", R1 | both sites (`cli.rs:1079-1083`, `1225-1228`) add "unless a hold of that snapshot changes between the two commands at the same time (… R1 of #165's review)". Right, if wider than R1 (N8) |
| the engine's sweep doc | fixed (L1 above) |
| `names::PREFIX` | fixed |
| `Record::check` | not changed. It was optional; still holds today, since `fresh_name` is the only source of a name |
| the 93-column line (`hold_lines`' doc) | reflowed (`cli.rs:465-467`). But the reflow of the message made a 103-column line (N8) |

### N7: the description, mostly fixed

- "where that snapshot has no marker" → "where the agent's directory has
  no marker". Fixed.
- The +2682: explained (B1). Fixed.
- The plants' scope: stated for each round. Fixed.
- The stale pointer: now `test_agent_record.sh:393`, but see N7'.

## Findings at b30e452, most severe first

**No defect.** Nothing below blocks.

### TG3 (low; certain): two fresh plants survive

| Plant | Survives because | A pin |
|---|---|---|
| N3a: N3's advice (`; {ODD_HOLDS}`) dropped from the count's missing-snapshot message (`cli.rs:1131-1132`) | the new case (`cli/tests.rs:2298-2317`) checks the command, the cause and the stderr, not the advice. The description's N3 says the message "gives the advice on odd tags" | `assert!(why.contains("release that hold by hand"))` in that case |
| A9e: the fake's filter takes `skz-` anywhere in `t` (`engine/fake.rs:1625`) | no engine fixture has `skz-` inside `t` but not at its start. The real filter's version (A5) dies by the CLI tests | a fourth name, `{POOL}@keep-skz-1`, in the new engine test's `others`. Only the fake; very low |

### N8 (nits): wording and one long line the commit made

- **A test doc names a mutant it does not kill**
  (`engine/tests.rs:3689-3690`): "Mutants: the fake's or the sweep's
  filter dropped". The engine's `sweep` has no filter, and this test
  runs the fake, so the CLI sweep's filter dropped (A7) passes it; two
  CLI tests kill it. "The fake's filter dropped, or taken in any case,
  without its dash, or on the pool's own snapshots alone" says what it
  kills (A9, A9c, A9d, A9f).
- **"nor hide one, unless …"** (`cli.rs:1079-1083` and `1225-1228`).
  The new "unless" qualifies both "forge" and "hide". R1 found that the
  race lets a forged line pass but never hides the agent's hold (its
  line is printed whole and read; the race only moves the count). The
  docs now concede more than is so. Conservative and harmless; the exact
  form is "cannot forge the agent's hold, unless …, and cannot hide one".
- **A 103-column line** (`cli.rs:541`, the message's third line). It is
  the only line over 100 columns in `cli.rs`; 85c4708 had none. The
  reflow of the message made it. rustfmt does not wrap string literals,
  so the gates pass. Move "where a tag" to the next line.
- **A ragged reflow** (`engine.rs:756-757`): "publish. One agent runs
  per pool, and" then "within it …". Cosmetic.

### N7' (nits): the description

- **"Not shown on real ZFS: a tag with a tab or a newline"** (under "Not
  done") contradicts "Real ZFS, recorded by testing" above it, which
  shows exactly that on both images. I checked the run's artifacts
  (45884296395, `zfs-cli-ubuntu-24.04` and `-26.04`):
  `mirror-holds-r-odd-tags` and `mirror-holds-odd-tags` print the tags
  `other\tholder` and `other\nholder` raw, as three lines (four fields,
  two, and `holder\tDATE`), the four-field line first on 24.04 and last
  on 26.04; `mirror-holds-r-counts-gone` exits 1, lists the other two
  snapshots and says `cannot open 'skzq…/child@c': dataset does not
  exist`. The record section is right. What is not shown is the agent
  reading such a tag on a real pool, and the narrowing on ZFS 2.2; say
  that.
- **The pointer** ("`test_agent_record.sh:393` … uses `skzcut@sweep-w`").
  Line 393 now reads `skzcut@skz-sweep-w`; the sentence is about
  2233537, in the present tense. A reading aid only.
- **N4's bullet** under "What was done" still says the message names
  "holds added between the two reads". It is the history of 2233537, and
  the N4' bullet below corrects it; a cold reader may trip on it.

## The author's answer, against the above

Read after the view above was formed. It agrees with what I found, item
by item, with these differences:

- "The long comment and doc lines are reflowed: … and the message's
  string." The message's reflow made the 103-column line (N8).
- TG2's plants were run against `skyzfs-agent`'s lib (213 tests); I ran
  them against both agent crates (220), with the same result.
- "Not done: a rig case … belongs to testing. I am passing your
  suggestion to them." Right; it was a suggestion for testing, not for
  this merge.

## Codex

Its review of b30e452 is completed with no findings (the summary
comment's state: `has_any_findings: false`). Its one finding on 2233537
(P2, the rig's fixtures) was answered at 85c4708.

## Gates

On a scratch worktree of b30e452, rustc 1.100.0 (`rustup update stable`
found nothing newer), `CARGO_INCREMENTAL=0`, no debug info, the target
cleaned between sets. `cargo fmt --all -- --check`: clean.

| Feature set | clippy `--workspace --all-targets -D warnings` | `cargo test --workspace` | `cargo doc --workspace --no-deps` (`RUSTDOCFLAGS=-D warnings`) |
|---|---|---|---|
| none | clean | 1969 passed, 0 failed, 16 ignored | clean |
| `skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean | 2027 passed, 0 failed, 16 ignored | clean |
| `skyzfs-ublk-sys/control-test-program` | clean | 1993 passed, 0 failed, 18 ignored | clean |
| `skyzfs-agent-cli/test-hooks` | clean | 1969 passed, 0 failed, 16 ignored | clean |

These match the description's table: one test more than at 85c4708 in
each set, the new engine test. The docs' new links are on private items
(`agents_name`, `Agent::sweep`), which `cargo doc` does not check, so I
also ran `RUSTDOCFLAGS=-D warnings cargo doc -p skyzfs-agent --no-deps
--document-private-items`: clean, no warning.

## CI at b30e452

Read from the head's check runs and job k's log. The pull-request runs
built `3c9488c`, b30e452 merged into 3fa5a8e, the tip of `zfs-topology`
with #166.

| Workflow | Run | Jobs |
|---|---|---|
| `ci` | 45892958834 | test, lint, unit-arm64, chart-lint, s3-backends: success |
| `coverage` | 45892958763 | success |
| `e2e` | 45892958752 | e2e-tier-a: success |
| `k3s-smoke` | 45892958766 | install and UDS smokes: success |
| `skyzfs-ci` (the push) | 45892952658 | gate: success; test, lint and unit-arm64 skipped, since `ci` runs them on the pull request |
| `skyzfs-agent-rig` | 45892958748 | the check's logic; a1, a2, d and k on ubuntu-26.04: success |

**Job k** (140159889704) ended "done: worst exit 0": 1015 pins ok, 107
INFO, none failed or in error. Its three controls were caught
(`ctl_early` at the snapshot, `ctl_files` at the files, `ctl_manifest`).
`pin_sweep_swept`, `pin_sweep_clean`, `pin_unmarked_refused` and
`pin_unmarked_outcome` are ok. 36 K-sweep cases ran, as at 85c4708,
with kills at `sweep:before:1` and `sweep:after:1` (`premise_…_killed`
ok).

Against job k at 85c4708 (1049 ok, 104 cases), this run has 100 cases.
The four missing are `K-assist-assist-before-3`/`-after-3` and
`K-destroy-look-before-2`/`-after-2`. The K cases follow the kill
points a run reaches, which vary from run to run: at 85c4708 three of
these four already ended "not reached" (`obs_…_not_reached`), and my
first review named both families as run to run. One sweep
case, `K-sweep-intent-before-1`, took the path where the retry reuses
the name this time: one more pin (`…_name_not_given_up`, ok) and one
more INFO. With the snapshots' time-stamped names folded, no other pin
differs.

## Decisions for the lead

None new. L1 is decided. The M3 note (design §5.4's restore is wider
than `Zfs::tagged`) is for the notes. The rig case for the other side of
the narrowing has gone to testing.

## What was not checked

- Nothing on a real pool by me (no ZFS here). The real-ZFS evidence is
  testing's recording above and the agent rig's job k at this head.
- I did not re-run my first review's 51 plants: the code they touch is
  unchanged but for the message, whose plants (N4m, N4pm) die.

## Files here

- `plants168n.py`: the 15 plants (BASE, my four survivors, ten fresh),
  and how they ran.
- `plants168n.out`: one JSON line per plant, with the first lines of
  each failure, and the worktree's state after.
