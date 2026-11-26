# Next look at skyzfs/skyzfs#162 (M2.6c-5d) at 18ec0a6

Head checked with `gh api repos/skyzfs/skyzfs/pulls/162 --jq .head.sha`:
`18ec0a61974ee481d44472c539d3d92bd31c7f30`, branch `skyzfs-coding`. The
merge base is still 1d14ee9; `zfs-topology` is now at 8c06862, one line
of the implementation notes later (the snapshot reads at 1:20000, which
bear on N-c below). The delta since my review at b83ac36 is one commit,
`18ec0a6`, in `crates/skyzfs-agent/src/zfs/cli.rs` and its tests only.
Line numbers are at `18ec0a6`.

I read the delta, worked out D1 and F1 against OpenZFS 2.4.1's
`zfs_main.c` (`print_holds`, `zfs_do_holds`), `libzfs_dataset.c`
(`zfs_release_one`, `zfs_release`) and `zfs_namecheck.c` (`valid_char`),
ran the checks below, and only then read the author's answer
(issuecomment-7194442739) and the description.

## Verdict

**Nothing in the code blocks the merge from review.** D1 is fixed end to
end, F1 is fixed as asked, and every survivor of my two earlier looks is
killed. Three things should be settled before the lead merges, none of
them a defect that blocks:

1. **CI is not green on the head** (below). The agent rig's job k ended
   with exit 2, the rig's own error and not a verdict, in a case that
   has nothing to do with this delta. But k is the only job whose
   sweep course drives the new parsing on a real pool (ZFS 2.4.1), and
   it stopped before that course. Rerun k, and read its sweep lines,
   before the merge.
2. **F1's documented residual is real, and it is not harmless in the
   way the comment says** (F1-R). Nothing of another holder's is
   released, which I confirmed; but a crafted tag on any held snapshot
   of the pool makes every sweep fail, so every `publish` and `settle`,
   until that hold is released. The comment and the description should
   say so; whether to close it is the lead's call.
3. **Codex's new P2** (argument budget, `cli.rs:324`) is real; I
   reproduced it. It needs a non-default stack limit and names of
   almost the longest kind, so it is low. It has no answer on its
   thread yet.

## Findings, most severe first

### F1-R (low; the residual's consequence is understated): a crafted tag makes every sweep fail

- **Where.** `cli.rs:886-901` keeps a line where its first field is a
  snapshot this command asked for and its second the agent's tag. The
  comment at `cli.rs:892-895` and the description's "F1's residual" say
  that a forged line can still name an asked snapshot, and that "the
  sweep then releases the agent's tag there ... and nothing of another
  holder's".
- **That part is true.** `zfs release -r skyzfs-publish POOL@t` can only
  release that tag (`zfs_release_one`). No hold of the agent's is
  skipped either: each hold is printed by its own
  `printf("%s\t%s\t%s\n")`, so the agent's own line always starts a
  line, with the name exactly as asked.
- **What the comment leaves out.** The release then finds no hold of
  the tag. libzfs says `no such tag on this dataset` when no member had
  it (`zfs_release`: the list to release is empty, the last member's
  `ESRCH`), or, in a race, when a hold it found was gone at the
  release; with one agent a pool, only the first. `release` (`cli.rs:1203-1223`) then checks with
  `zfs holds -H -r POOL@t` (`tagged_snapshots`, `cli.rs:790-812`), which
  parses the same text the same way, reads the same forged line, and
  fails: `… kept the hold`. `sweep()` returns it (`engine.rs:788`), so
  `begin` (`engine.rs:644`) and the first round of every `settle`
  (`engine.rs:692`) fail, in every process, until that hold is released.
  In a directory with no marker the agent exits 6 instead, and its
  advice (`zfs release -r skyzfs-publish ...`) does not help, since
  there is no such hold.
- **No whole forged line is needed.** A tag that is the agent's
  followed by a tab (`skyzfs-publish\tx`) or a newline
  (`skyzfs-publish\nx`), on any held snapshot, does the same. So does
  `x\nSNAP\tskyzfs-publish` on SNAP itself.
- **A real hold of the agent's tag does less.** Placed by someone else,
  it is read, released once, and the sweep goes on. The crafted one
  stops the agent until a person finds it: the error names the
  snapshot, but `zfs holds` shows the tag's text across lines.
- **Who can.** Root, or a user given `hold` on any dataset of the
  pool, with a deliberately odd tag. I know of no tool that writes tags
  with tabs or newlines, so this is a hostile or broken holder, not an
  accident. Against the base (`tagged.lua` matched tags in the kernel)
  it is new in this pull request; against b83ac36 it is narrower (the
  forged name must now be a held snapshot of the same command).
- **Shown, end to end.** `skyzfs-agent settle` (plain build of
  18ec0a6) on a stand-in `zpool` and `zfs` as in `program.rs`, one held
  snapshot `tank/db@backup`, `zfs release` answering `no such tag`:

  | Tag on `tank/db@backup` | Marked directory | No marker |
  |---|---|---|
  | `x\ntank/db@backup\tskyzfs-publish` | exit 1: `zfs release -r skyzfs-publish tank@backup: tank/db@backup kept the hold: … no such tag on this dataset` | exit 6, naming `backup` |
  | `skyzfs-publish\tx` | the same exit 1 | exit 6 |

  A scratch unit test (not committed) gives `Ok({"backup"})` from
  `tagged()` for each of the three shapes above, and `release()` then
  fails with `kept the hold`.
- **What to do.** At least: the comment at `cli.rs:892-895` and the
  description's "F1's residual" say that the sweep then fails on every
  round until that hold is released, and that a tag beginning with the
  agent's and a tab or newline is enough. To close it, the lead may
  choose a guard in `tagged_once`: keep an agent's line only where it
  has exactly three fields, and where the lines of its snapshot do not
  outnumber that snapshot's `userrefs` from the list; otherwise skip
  the snapshot (its hold kept, the conservative side). Every newline in
  a tag adds a line, and a forged line counts only where it begins with
  an asked name, so a forged line always makes some asked snapshot's
  lines outnumber its `userrefs`; a tab after the tag adds a field. All
  three shapes above fail that guard. A hold added between the list and
  `zfs holds` would also skip its snapshot for one sweep, which is
  conservative. `tagged_snapshots` (below) has the same parse.

### CX (low; Codex's P2 on `cli.rs:324`): 1000 names of the longest kind do not fit at `LimitSTACK=1M`

- **The doc.** `HOLDS_AT_ONCE`'s doc (`cli.rs:318-323`, my N-d) says
  256 KiB is "just enough" at a stack limit of 1 MiB. It is not:
  Linux counts the pointer array and the environment against the same
  budget (`bprm_stack_limits` in `fs/exec.c`). My N-d said "1000 names
  of the longest kind" fit, which was wrong too.
- **Reproduced.** `/bin/true holds -H` with 1000 arguments of 255
  bytes, `RLIMIT_STACK` 1 MiB: `E2BIG` even with an empty environment.
  With 1000 of 250 bytes, or 960 of 255, it runs. The agent would then
  fail its sweep with the host's error on every round.
- **Reach.** It needs a unit that lowers the stack limit (none in the
  repository does) and about a thousand held snapshots in one command
  whose names average over about 250 bytes. The agent's own names are
  about 30 bytes.
- **Fix, either.** Bound a command by bytes as well (for example stop
  adding names past 96 KiB of names and their pointers: Linux always
  allows at least 128 KiB, whatever the stack limit), which keeps the
  1000/1000/500 pin; or say in the doc that the longest names do not
  fit at 1 MiB and why that is accepted. Codex's thread needs the
  author's answer either way.

### G-UTF (low, in the base since #147; for later): `tagged_snapshots` still reads `zfs holds -H -r` as UTF-8 and by the same parse

- **Where.** `cli.rs:797` (`self.zfs`, the UTF-8 `run`) and
  `cli.rs:802-811`.
- **In the sweep** it is reached only through `release`'s check after
  `no such tag`. A real stale hold never gives that: `zfs release -r`
  from `POOL@t` visits every file system and volume, releases the hold
  it finds and exits 0, and only the agent changes its tag. So it is
  reached only after F1-R. Shown: the forged tag plus a hold `caf\xe9` on
  `tank@backup` makes `settle` exit 1 with `zfs holds -H -r tank@backup:
  its output is not UTF-8`. Every other read of the sweep is clean:
  `props` and the counts print no one else's text, the list is read as
  bytes, and so are the holds.
- **Outside the sweep** it reads the holds of the agent's own `@t` in
  `held` and `hold`. A replication tool that holds the published
  snapshots is the likeliest other holder there is, and a tag of it
  that is not UTF-8 would fail those reads for that publish. A forged
  line there can make `held` read `Held::All` for a member that lacks
  the hold (my earlier note). Same fix as D1: read as bytes, and keep
  a line only where its first field is `X@t` under the pool. Not this
  pull request's; the lead's to place.

### Process: CI at 18ec0a6

Read with `gh api .../commits/18ec0a6.../check-runs` at about 14:55.

- **Green:** the CI suite's `test`, `lint` and `unit-arm64`; `e2e-tier-a`,
  `s3-backends`, `chart-lint`, both k3s smokes; the agent rig's a1, a2
  and d (Ubuntu 26.04) and "the check's logic". `skyzfs-ci.yml`'s jobs
  are skipped, as on any pull request.
- **Running:** `coverage` (in progress when I read it).
- **Failed: the agent rig's k** (run 45836590019, Ubuntu 26.04), exit
  2, the rig's own error. In course `arare`, case
  `K-restart-ask-before-2` was to kill the agent at the second ask. In
  this run one ask was enough (its trace has `ask before` once), so the
  point was never reached; the rig then looked for `serve-7.log`, which
  no daemon wrote, and stopped. At b83ac36 the same case reached its
  point. This delta changes only the sweep's read of holds, which is
  not on that path, so I read it as timing, not a verdict.
  - But the course `sweep`, which comes later in `arare`, did not run.
    It is the only check of `zfs list ... name,userrefs` and `zfs holds
    -H` on a real pool, and the bytes path is new. Rerun k (or dispatch
    the rig with `k`) and read `pin_sweep_swept` and
    `pin_unmarked_refused` before the merge.
  - #163 (testing) changes how the rig judges past its own errors; not
    read for this note.
- **The description** still says "CI at 18ec0a6: running; this line
  will link the runs".
- **Codex at 18ec0a6:** one new finding, the P2 above (CX), with no
  reply. Its earlier P2 (the recorder) is answered and resolved.

## D1: checked, fixed

- **The code** (`cli.rs:870-882`). `exec` then, on a non-zero exit,
  `failed(..., snapshot: true, pool)`, which is what `run` did before;
  only the UTF-8 check of the output is gone. The host's own error (a
  timeout) is still `Other` from `exec`. `failure()` reads stderr with
  `from_utf8_lossy` (`says`), so its classes hold on any bytes.
- **Scratch unit test** (not committed), each on the new code:
  - `dataset does not exist` with bytes 0xe9 in stderr: `NoSnapshot`,
    then `Vanished`, read again; the second read, with a hold
    `caf\xe9` beside the agent's, gives `{"skz-1"}` (four commands);
  - `no such pool or dataset` with a byte 0xe9, `zpool list` listing
    `tank`: `Vanished` three times, then the error (nine commands);
  - the same with `tank` not listed: `NoPool` of the holds' words;
  - another failure with stderr not UTF-8 and partial stdout: `Other`,
    with `exit 2` and the lossy message, nothing parsed;
  - a timed-out run: `Other`.
- **The program.** `skyzfs-agent settle` on a stand-in pool with a
  stale hold of the agent's on `tank@skz-old` and a hold `caf\xe9` on
  `tank/db@backup`, in a marked directory: b83ac36's binary exits 1
  (`zfs holds -H tank@skz-old tank/db@backup: its output is not
  UTF-8`); 18ec0a6's exits 0 and releases `tank@skz-old`.

## F1: checked, fixed as asked

- **The parse.** Lines are split at `\n`, fields at `\t`, both as bytes;
  a line is kept where the tag field equals the agent's tag and the
  first field is one of this command's names (`asked`, a set of the
  chunk's names as bytes). The name after `@` comes from that asked
  name. A snapshot's name cannot hold a tab or newline (`valid_char`
  allows letters, digits, `-_.:` and space), and `zfs holds` prints the
  name it was given (`zfs_open` keeps it), so the comparison is exact.
- **What I tried, each in a scratch test:**

  | Another holder's tag, or line | Read as the agent's? |
  |---|---|
  | `x\ntank/db@forged\tskyzfs-publish` (not listed) | no (the author's test) |
  | a forged line naming a snapshot with no hold, or `tank/db@backup ` (trailing space), `tank/db@backu`, `tank/DB@backup`, a name and `\r` | no |
  | a forged line naming a held snapshot of another chunk (1001 held) | no |
  | `skyzfs`, `skyzfs-publish-not` (the author's test); an empty field (by reading) | no |
  | `a\nb` before the agent's own hold on the same snapshot | the agent's hold is still read |
  | `x\nSNAP\tskyzfs-publish`, SNAP the tag's own snapshot (any asked one, by reading) | **yes** (F1-R) |
  | `skyzfs-publish\tx`, `skyzfs-publish\nx` | **yes** (F1-R) |
  | the agent's exact tag, placed by someone else | yes, by design (the tag is the agent's alone) |

## The pins

Each plant applied alone as exact text on a scratch worktree at the
head, then `cargo test -p skyzfs-agent -p skyzfs-agent-cli --features
skyzfs-agent-cli/test-hooks`, 600 s limit, restored after; the worktree
was clean at the end. Baseline: 206 + 7 passed. R1 to R5 and R7 are my
plants of `plants162.py`, remade on the rewritten site (the holds now go
through `failed`); Q3 and Q4 are `plants162b.py`'s text, unchanged.

| Plant | Result |
|---|---|
| R1 the sweep's holds confirmed on `names[0]` | killed: `only_the_commands_own_pool_is_looked_for`, `the_sweep_reads_again_what_vanished_under_it` |
| R2 its `Vanished` taken as no hold (chunk skipped) | killed: `…_vanished_under_it` |
| R3 its `Vanished` an error at once | killed: `…_vanished_under_it` |
| R4 a tag the agent's begins with kept | killed: `the_sweep_reads_the_holds_of_the_snapshots_that_have_one` |
| R5 a line with no tag an error | killed: four sweep tests (also by the empty piece after the last newline) |
| R7 only `zpool status -g` confirmed on `status` | killed: `only_the_commands_own_pool_is_looked_for` |
| Q3 a walk's `Vanished`/`NoPool` made `Other` by a failed count | killed: `a_count_past_the_datasets_limit_is_not_cut_off` |
| Q4 the failure cleared only by a snapshot count that succeeds | killed: `a_count_past_…` |
| S6 (the author's, remade) each holds on the list's limit | killed: `…_that_have_one` |
| S8 (remade) a snapshot gone taken as no hold | killed: `…_vanished_under_it` |
| S9 (remade) a snapshot gone an error at once | killed: `…_vanished_under_it` |
| N1 the asked-set check taken out | killed: `another_holders_tag_is_read_as_bytes` |
| N2 the holds checked as UTF-8 again | killed: `another_holders_tag_is_read_as_bytes` |
| N3 `failed` told the command reads no snapshot | killed: `…_vanished_under_it` |
| N4 a failed holds with output parsed as if it exited 0 | killed: `…_vanished_under_it` |
| N5 `asked` drawn from every snapshot listed, not this command's | survives |
| N6 a line kept by either check (`&&` for `\|\|`) | killed: `another_holders_…`, `…_that_have_one` |
| N7 the holds' run error given the count's note (`walked`) | survives |

- **No unmade plant matters.** N5 is a weaker form of the same check:
  with the per-command set, a forged line can name a snapshot of its
  own command only; with the whole list, any held one. Given F1-R, that
  changes little; a case with 1001 held snapshots would pin it if
  wanted. N7 would add a wrong but harmless note to a timed-out `zfs
  holds`. Neither needs a test.
- **The new tests do what the answer says.** TG-A's case
  (`cli/tests.rs:1686`) feeds `cannot get holds for 'tank@gone': no
  such pool or dataset` with `tank` listed and checks the five
  commands; the `NoPool` case (`cli/tests.rs:2614`) lists `tank@skz-1`,
  `holds` and `-H` but not `tank`. Q3 and Q4 (`cli/tests.rs:2290`,
  `2316`) then restore the failed count, so the old "a count that
  succeeds again forgets" step still tests what it did. R7's case is at
  `cli/tests.rs:2587`, R4's hold at `cli/tests.rs:1550`.

## Nits N-a to N-e, and the description

- **N-a: done.** `tagged_once`'s doc (`cli.rs:820-829`) says a dataset
  gone during the list is no error and the list's own message is read
  again defensively.
- **N-b: done** (41.6 s).
- **N-c: done, and now superseded.** `list_per_snapshot`'s doc
  (`cli.rs:390-394`) gives 0.40 and 1.06 ms as "the review of #162"
  read them, and "at 12 500 a dataset on 24.04 the margin may be about
  two". Those two figures are #159's (runs 45781769557 and
  45781845472, Ubuntu 26.04, beside writers), and the margin was my
  estimate. The base now has the measurement: run 45801291679, 12 500
  snapshots a dataset, the agent's list beside the writer at 1.56 ms
  a snapshot on 24.04 and 0.82 ms on 26.04, `holds -r` 1.43 and
  0.81 ms. That is a margin of about six on 24.04. Cite that run and
  drop "about two".
- **N-d: done, but wrong** (CX above).
- **N-e: done** in the code (`cli.rs:886-891`). Two small things:
  only a newline can make a line whose first field is not asked or that
  has no second field, not "a newline or a tab"; and the description's
  "Not done" still has the old paragraph "A conservative reading ... The
  sweep then keeps that hold", which N-e made untrue (no hold of the
  agent's is skipped).
- **The description's other claims hold**: the four feature sets'
  counts (below), the 11 plants of the answer (I made my own versions;
  all killed), and the TG-A/B/C wording. "CI at 18ec0a6: running" is to
  be updated, and "F1's residual" to be completed (F1-R).
- One long doc line (`cli.rs:394`, 101 characters) is not wrapped like
  its neighbours. Cosmetic.

## Gates

GATES_PLACEHOLDER
