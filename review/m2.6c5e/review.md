# Review of skyzfs/skyzfs#165 (M2.6c-5e) at 6752363

Head checked with `gh api repos/skyzfs/skyzfs/pulls/165 --jq .head.sha`:
`6752363a9efc3dc312e73843b41b659eae4c6402`, branch `skyzfs-coding`, one
commit on `zfs-topology` at 9e70246 (the merge base, and the base's tip
when I read it). Only `crates/skyzfs-agent/src/zfs/cli.rs`,
`cli/tests.rs` and `cli/recordings.rs` change. Line numbers are at
6752363.

Before the description I read the diff, design §5.2, the module docs,
every caller of the changed functions (`engine.rs`'s `sweep`, `see` and
`ask_anew`; `plan.rs` where it reads `Held`), my notes on #162
(`review/m2.6c5d/`), the implementation notes on #162, and OpenZFS
2.4.1's `print_holds` and `zfs_do_list` (`zfs_main.c`), `zfs_for_each`
and `zfs_callback` (`zfs_iter.c`), `dsl_userhold.c`, and the `userrefs`
load and stats in `dsl_dataset.c`; for 2.2, `zfs_do_list`'s rule for
snapshots named with `-t snapshot` (`zfs_main.c` 2.2.2).

## Verdict

**Nothing blocks the merge from review.** The guard is built as the
lead ruled and does what it says. Without a change of holds between the
agent's two commands, no line forged by another holder's tag is taken,
whatever its shape, and no hold of the agent's is missed. G-UTF is
fixed in the three `-r` reads (`held`, and the checks of `hold` and
`release`).

- **Plants.** My versions of the author's 15 plants are all killed, by
  the tests the description names. Of my own 18, ten are killed, one
  is equivalent, and seven survive (TG1). All are gaps in tests, not in
  the code.
- **Gates.** fmt, clippy, the tests and the docs are clean on all four
  feature sets, with the description's counts (below).
- **CI.** Green on the head, job k of the agent rig included (below).

What needs a word before the merge, none of it a defect in the code:

1. **A4 (low; for the lead): the error costs more than F1-R did.** Any
   tab or newline in any holder's tag, on any held snapshot of the pool,
   now stops every `publish` and `settle` on that pool until a person
   releases it. At the base such a tag was skipped, unless it had one
   of F1-R's shapes. This is the ruling's choice. Two narrowings keep
   its safety and give most of that back; I tried the first.
2. **R1 (low): the residual race is as the description says**, but the
   remedy it names (a count on both sides of the holds) does not close
   it. Its worst outcome is worth a line: a member of the agent's own
   snapshot read as held for a round.
3. **Nits** on the docs and messages (N1 to N5), and the test gaps
   (TG1).

## The forgeries, shape by shape (question 1)

What ZFS prints and counts, from the 2.4.1 source:

- `print_holds` writes each hold with one `printf("%s\t%s\t%s\n",
  snapshot, tag, time)`. So each hold's first line begins with its own
  snapshot's name and a tab, and every hold starts a new line. The
  agent's own hold is always one whole line `X\tskyzfs-publish\tWHEN`.
  The time (`%a %b %e %H:%M %Y`, locale C) has no tab or newline.
- `userrefs` is `ds_userrefs`: one per tag in the snapshot's hold ZAP.
  Each hold adds one and each release takes one
  (`dsl_userhold.c:176`, `:513`), it is loaded with `zap_count`
  (`dsl_dataset.c:680-684`), and `zfs list` reads it from the
  snapshot's stats (`dsl_dataset.c:2854`). Tags are unique per snapshot
  and at most 256 bytes.

So a tag with k newlines prints 1 + k lines and counts one hold. Say
every line passes `hold_lines` (three fields, the first a snapshot
asked). Then the lines given to a snapshot X are exactly:

- its `userrefs`, one first line for each real hold;
- plus the forged lines that name X.

Any forged line therefore makes its target's lines exceed its
`userrefs`, and `outnumbered` (`cli.rs:461-488`) finds it. Nothing can
lower a snapshot's count of real first lines, since a malformed first
line is itself an error and is never skipped. The shapes asked about:

| Shape: another holder's tag on a held snapshot S | What the agent reads | Result |
|---|---|---|
| `skyzfs-publish\tjunk` | a line of 4 fields | error (`hold_lines`) |
| a tab anywhere in a one-line tag | 3 + (tabs) fields | error (`hold_lines`) |
| `x\nX\tskyzfs-publish\tdate`: a whole line forged with its date | `S\tx` (2 fields), then 4 fields | error (`hold_lines`) |
| `a\tb\nX\tskyzfs-publish`, X asked, X = S or not | two lines of 3 fields; X has `userrefs` + 1 lines | error (`outnumbered`) |
| the same, X a snapshot with `userrefs` 0 | sweep: X not asked; `-r`: X counted 0 | error (`hold_lines`; `outnumbered`) |
| the same, X an `@t` under the pool that does not exist (`-r` only) | the count fails `dataset does not exist` | `Vanished`, three reads, then that error (N3) |
| the same, X of another `@t` or another pool (`-r`), or of another command's names (sweep) | not asked | error (`hold_lines`) |
| the agent's hold on X, and the forging tag also on X | X: `userrefs` 2, lines 3 | error |
| the agent's hold on X, a plain hold of the forger on X, the forging tag on S | X: `userrefs` 2, lines 3 | error |
| a tag ending in `\r`, or with trailing spaces | three fields, compared as bytes | another holder's tag, read exactly |

**No forged line is taken, and no hold of the agent's is missed,
without a change of holds between the two commands.** Everything that
is not exact is an error after three reads, never a skip. What a forger
still gets is the error itself, on any held snapshot of the pool (A4).

My scratch tests (`scratch_tests.rs` here, not for the repository) ran
on a scratch worktree of the head:

| Scratch test | Result |
|---|---|
| sweep: forging tag `x\tY\ntank@skz-1\tskyzfs-publish` on `tank/db@backup`; `tank@skz-1` counted 1 by the list and holding nothing at the holds (a hold released in the window) | `Ok({"skz-1"})`: the forged line passes (R1) |
| `held`: the agent's hold on `tank@skz-1` only; forging tag `x\tY\ntank/db@skz-1\tskyzfs-publish` on `tank/db@skz-1`, which the count says has 2 holds (one added in the window) | `Ok(Held::All)` (R1). With the count 1: the error |
| a harmless tag `backup\t2026`, `note\nmore` or `trailing\n` on `tank/db@backup` (sweep) or on `tank/db@skz-1` beside the agent's (`held`) | an error after three reads in each of the six (A4); the sweep ran 6 commands |
| `held`: forging tag on `tank@skz-1` naming `tank/nope@skz-1`, which does not exist | `Err(Vanished("zfs list … tank/nope@skz-1 tank@skz-1: a snapshot read with a hold is gone: cannot open 'tank/nope@skz-1': dataset does not exist"))` after three reads (N3) |

## Findings, most severe first

### A4 (low; for the lead): the error stops every publish on the pool, for any tab or newline in any holder's tag

- **Where.** `hold_lines` (`cli.rs:419-451`) and `again_checked`
  (`cli.rs:916-933`). They are reached through:
  - `tagged` (`cli.rs:1522`), the sweep;
  - `held` (`cli.rs:1486`);
  - the checks of `hold` and `release` (`cli.rs:1422`, `cli.rs:1456`).
- **The changed test.** At the base, `another_holders_tag_is_read_as_bytes`
  (`cli/tests.rs:1869`) asserted two skips: a line forged for a
  snapshot not asked, and a line with no tab. Both are errors now, in
  `holds_that_do_not_add_up_are_an_error_that_names_the_snapshot`
  (`cli/tests.rs:1901`). The test was changed by the ruling, not
  weakened: the new test asserts the new behaviour, which is stricter.
- **What changed for a harmless tag.** At the base, `tagged_once`
  skipped a line whose second field was not the agent's tag or whose
  first was not asked. `tagged_snapshots` kept only lines whose second
  field was the tag. So `backup\t2026` or `note\nmore` on any snapshot
  was ignored. Only F1-R's shapes failed the sweep, and a tag that was
  not UTF-8 failed the three `-r` reads. At the head:

  | Caller | Base (9e70246): harmless tab or newline tag | Head |
  |---|---|---|
  | sweep (`begin`, every `settle` round) | ignored | error after three reads of the list and the holds |
  | `held`, each round of a publish whose `@t` has the tag | ignored | error after three `holds -r` |
  | the checks of `hold` and `release` | ignored | error after three reads |
  | any caller, a tag that is not UTF-8 | the sweep read it; the three `-r` reads failed | read (G-UTF) |

- **Scope.** The sweep reads the holds of every snapshot of the pool
  that has one, not only the agent's. So one such tag on a backup
  tool's snapshot, anywhere in the pool, does this, in every process,
  until a person releases that hold:
  - `begin` fails, so no publish starts;
  - every `settle` round fails (`engine.rs:779`).

  The error names the snapshot and says what to do, so it is found.
  Nothing is lost or released wrongly.
- **Is "error" right?**
  - **The checks of `hold` and `release`:** yes. They run only after
    ZFS refused, and must not pass on a doubt.
  - **`held`:** a skip would end in the hold's check failing anyway.
  - **The sweep:** it is the lead's trade. An error stops publishing,
    where a skip of the snapshot could leave a stale hold of the
    agent's unseen.

  I know of no tool that writes tabs or newlines in tags, so this needs
  a deliberately odd tag. It is the same class as F1-R, but now any
  such tag does it, not only the forging shapes.
- **Two narrowings that keep the ruling's safety.** They are the lead's
  call; neither is needed to merge.
  1. **Read tab-only tags exactly.** Take a line with three or more
     fields, the first a snapshot asked, as one hold whose tag is
     everything between the first tab and the last: names and the time
     have no tab. Only newlines add lines, so the count argument above
     is unchanged and nothing forged passes. But `backup\t2026`, and
     F1-R's `skyzfs-publish\tx`, are then read as what they are,
     another holder's tag, and stop nothing. Newlines must stay errors.
     **Tried on a scratch worktree:** `hold_lines` with the tag taken
     as `&line[first_tab + 1..last_tab]`. The agent's tests and my
     scratch tests then fail on exactly three assertions: the two
     cases "a line of four fields" (the sweep gives `Ok({"skz-1"})`,
     `held` gives `Ok(Held::Part)`, both right for that output) and my
     harmless `backup\t2026`. Every newline case is still an error, and
     the rest pass.
  2. **Let the sweep read only the agent's own names** (`@skz-…`; the
     agent's names are its own, `engine.rs:1157-1161`). The agent's
     holds are only there, unless a person put the agent's tag on
     another snapshot, which the sweep would then no longer release.
     An odd tag on someone else's snapshot then stops nothing.
- **Also.** The doc's reason for not skipping is not quite right (N1).

### R1 (low): the residual race; the remedy the description names does not close it

- **Where.** `tagged_once` reads the counts (its list) before the
  holds. `tagged_snapshots` reads them after `holds -r`
  (`cli.rs:1045-1062`). `again_checked` returns at the first read that
  adds up.
- **When a forged line passes.** Only where the forging tag stands and
  the target snapshot's holds changed in the window, by as many holds
  as there are forged lines:
  - a hold released between the sweep's list and its holds;
  - or a hold added between `holds -r` and the count.

  A forger with `hold` and `release` on the target can do that on
  purpose and time it: the agent's commands show in the process list.
  Without a forging tag, the race only makes an error, which is read
  again.
- **It never hides a hold of the agent's.** The agent's line is always
  whole and is read; the race only moves the count.
- **The worst outcome, by caller.**
  - **The sweep** reads a `t` as the agent's that is not. Its `zfs
    release -r skyzfs-publish POOL@t` releases nothing of another's and
    says `no such tag`. Then:
    - where the forging tag is under that `POOL@t`, the check fails;
    - where it is on another `t`, the check passes and the sweep goes
      on;
    - in a directory with no marker, exit 6 names that `t`.

    Harmless, as the description says, with the second case added.
  - **`held` and the hold's check** read a member's `@t` as held where
    the agent's hold is not there (someone released it by hand), so
    `Held::All` for one round.
    - The planner then does not hold again (`plan.rs:861`) and may ask
      the daemon to publish (`plan.rs:921`). `ask_anew` reads `held`
      again first (`engine.rs:1503-1506`), which needs a second race.
    - That member's `@t` then has no hold of the agent's, and that
      hold is what keeps it until the cut (design §5.2). A destroy of
      it in that window, by someone allowed to, could make the
      published cut lack it.
    - It needs a hostile holder with `hold` and `release` on the
      agent's own snapshots, a race won, and a destroy. At the base no
      race was needed (a forged line was simply read), so this is
      strictly narrower.
- **Nothing re-checks a read that passed.** But the next read (the next
  round, or the sweep of the next process) fails closed while the
  forging tag stands.
- **"Closing this would need the count read on both sides of the
  holds"** (the description) is not so. Take a count before and a
  count after. A forger releases a hold on the target after the first
  count and holds it again before the second. The holds then show
  `userrefs` − 1 real lines plus the forged one, and both counts show
  `userrefs`, so the forged line passes. Two counts narrow the window
  to two timed actions; they do not close it. The CLI has no atomic
  read of holds and their count. I would accept the race and say what
  it costs, as above.

### N1 (nit): the reason given for not skipping

- `cli.rs:92-93` and `cli.rs:413-415` say lines are not skipped
  because "a line skipped could hide a hold of the agent's". A skipped
  line cannot. The agent's own hold is always a whole line (above), and
  the base's comment said so (its `cli.rs:933-935`).
- **The real reason a line must not be skipped:** a skip breaks the
  count. Take a tag `x\nX\tskyzfs-publish` on X. Its first line `X\tx`
  has two fields. Skipped, X's lines equal its `userrefs` again, and the
  forged line passes. That is F1-R's third shape.
- The lead's ruling ("skipping would leave the agent's holds behind
  unseen") is about skipping a snapshot, and is right there.

### N2 (nit): a read that cannot add up is read three times

- `again_checked` (`cli.rs:924`) reads again on any `Odd`. Only
  `outnumbered` can change between reads. A line that `hold_lines`
  refuses is the tag's text, and comes back each time.
- So a persistent odd tag costs three full walks before the error:
  - in the sweep, three `zfs list -r` of every snapshot (about 9
    minutes at 101 000 snapshots, at 24.04's recorded 1.84 ms a
    snapshot);
  - in `held`, three `holds -r`.
- Reading again only for `outnumbered` would keep the race's retry and
  save that. Low: it only delays an error.

### N3 (nit): a forged line naming a missing `@t` reads as a vanished snapshot

- A forging tag under `POOL@t` can name `POOL/nope@t`, which does not
  exist. It passes `hold_lines`, since `under` only checks the name's
  shape. The count then fails `dataset does not exist`, which is
  `Vanished` (`cli.rs:1049-1057`). After three reads, `held` ends with
  `Vanished("… a snapshot read with a hold is gone …")` (scratch test).
- It fails closed, but with no word of an odd tag and none of
  `ODD_HOLDS`' advice.
- A name that `holds -r` printed and the count cannot open, three
  reads in a row, is more likely forged than vanished. Adding "or a
  line forged by another holder's tag" to that message would do.

### N4 (nit): the count's message says a tag forged a line

- `outnumbered`'s error (`cli.rs:475-479`) says "a line is forged by a
  tag with a newline". After three reads that is the likely cause.
- But holds that change at every read give the same error with no odd
  tag anywhere: a `zfs send -R` loop's temporary holds on the agent's
  snapshots, for example.
- "may be forged", or naming both causes, would not send a person
  looking for a tag that is not there.

### N5 (nits): the description

- **"on a read's limit (60 s)".** A read's limit is 60 s plus 100 ms
  for each dataset counted, at most 20 minutes.
- **The sweep's race outcome** needs its second case (R1).
- **"A holder whose tag has a tab or a newline … now stops the sweep,
  or the publish of that snapshot".** The sweep runs at every `begin`
  and every `settle` round and reads every held snapshot of the pool.
  So it stops every publish on the pool. Say so plainly.

### TG1 (low): seven survivors among my plants

Each survives `cargo test -p skyzfs-agent -p skyzfs-agent-cli
--features skyzfs-agent-cli/test-hooks`. The code is right in each
case; only the pin is missing.

| Plant | What it does | Why it survives | Were it so |
|---|---|---|---|
| M5 | the `-r` reads take a tag that the agent's begins (`starts_with`) | no `-r` test has another holder's tag that begins with `skyzfs-publish` (the sweep's test has one, and kills M17) | `held` reads `Held::All` from another holder's `skyzfs-publish-x`, and the release's check says "kept" |
| M6, M7 | the checks of `hold` and `release` do not read again on `Vanished` | only `held` has a vanished case; the description's "`hold` and `release` now read again on `Vanished` as well" is untested | a hold or release whose `@t` is being destroyed fails at once, as before this PR |
| M8 | the count's exit code not checked | its one failing case (a name gone) also leaves the count short, which is an `Odd` and is read again, with the same words | a count that fails with whole output is taken; one that fails short gives the count's error, not `Vanished` |
| M9 | the sweep's "asked" is any held snapshot of the pool, not only this command's | the forged-line cases name a snapshot outside the list; none names a held snapshot of another command | a forged line naming a held snapshot of another batch passes, since the count is per command: "asked" is what stops it |
| M10 | fewer lines than holds is an error too | every fixture's count equals its lines | a race reads again where it need not; nothing pins "fewer is no error" |
| M14 | the count's other failures (`NoPool`, a timeout) made `Vanished` | no test fails the count otherwise than with a name gone | a missing pool reads three times and ends `Vanished`, not `NoPool` |
| M4b | the count not cut into commands (all names in one) | no `-r` test names more than two snapshots | more than 1000 members with a hold, or long names, could reach `E2BIG` at a low stack limit |
| M3 | the count on a walk's limit | the test checks the limit at the base scale, where a walk's and a read's are both 60 s | a longer limit only |

M16 (the `-r` tag compared after a lossy UTF-8 decode) is equivalent:
a lossy decode equals the ASCII tag only where the bytes do.

**Fixes, all small:**

- `the_holds_under_a_snapshot_are_checked_as_the_sweeps_are`, more
  cases:
  - a member held by `skyzfs-publish-x` only;
  - a vanished count for `hold` and for `release`;
  - a count that fails `no such pool` with the pool not listed;
  - a count whose number exceeds the lines;
  - the count's limit after a count of datasets.
- In the sweep's test, a forged line naming a snapshot of another
  command: it is simplest as a unit test of `hold_lines` with a given
  `asked`.

## The plants

`plants165.py` and `plants165.out` are in this directory. Each plant was
applied alone as text (each site found exactly once), the tests above
were run with a 600 s limit, and the file was restored in a `finally`.
The worktree was clean after both runs.

- **G1 to G15**, my versions of the author's: all KILLED, by the tests
  the description names.
  - G1, G2, G3, G6, G7: both new tests.
  - G4, G10, G14: `holds_that_do_not_add_up_…`.
  - G5, G8, G9, G11, G12, G13: `the_holds_under_a_snapshot_…`.
  - G15: `another_holders_tag_is_read_as_bytes`.
- **Mine, killed:**
  - M1 (an empty output an error) and M4 (no command where there are
    no names): seven tests each;
  - M2 (`Odd` read twice only): both new tests;
  - M11 (the count lists only the snapshots with the agent's tag): four;
  - M12 (a snapshot counted 0 taken as 1), M13 (the count's
    `Vanished` an error at once): the `-r` test;
  - M15 (the sweep's final `Odd` taken as no hold): the sweep's test;
  - M17 (the sweep takes a tag that the agent's begins): the sweep's
    reading test.
- **Mine, survived:** M3, M4b, M5, M6, M7, M8, M9, M10, M14 (TG1); M16
  is equivalent.

## Question 3: the new count (`zfs list -H -p -o name,userrefs -t snapshot NAMES…`)

- **What it does on ZFS.** With `-t snapshot` and names but no `-r`,
  `zfs list` sets a recursion of depth 1 (`zfs_main.c`, 2.4.1 and
  2.2.2: "list all snapshots of the current dataset").
  - For a snapshot, that recursion visits nothing: `zfs_callback`
    lists the handle, and iterates the children only of a file system
    or a volume (`zfs_iter.c`). So each name is one `zfs_open`, not a
    walk of the pool.
  - `userrefs` comes from the open's stats.
  - A name that cannot be opened makes the command exit 1 after it has
    listed the others (`zfs_for_each`: `ret = 1`, and it goes on).
- **Its limit.** A read's limit a command (`cli.rs:1048`), with at most
  `HOLDS_AT_ONCE` names and `HOLDS_ARG_BYTES` of names a command
  (`holds_batches`).
  - That is right for a command that opens only what it names.
  - Its timeout is `Other` without `walked`, which is right too: its
    limit does not rest on the snapshot count.
  - The names are at most the members with a hold, so there are at
    most ceil(members / 1000) commands, each bounded.
- **Every walk caller stays bounded.**
  - `holds -r` keeps `walk_limit(holds_per_snapshot)`, and the sweep's
    list `walk_limit(list_per_snapshot)`, both capped at 35 minutes.
  - Each read is made at most three times (`VANISHED_READS`), as
    `held` and the sweep were before.
  - `hold` and `release` now read their check up to three times where
    they read once. That is at most two walks more, only after ZFS
    refused, and only while the reads vanish or do not add up.
- **Vanish and missing pool.**
  - `dataset does not exist` is `NoSnapshot` (the command names
    snapshots), made `Vanished`, and read again. On the next read,
    `holds -r` of a pool that is gone says `dataset does not exist`,
    which is `None`, so `held` gives `Held::No`, as before.
  - `no such pool or dataset` goes through `confirm`: `Vanished` where
    `zpool list` lists the pool, `NoPool` where it does not.
  - The first is tested (G12, M13); the second is not (M14).
- **Bytes.** The count's output is read lossily, and names are ASCII.
  The holds are bytes, and tags are compared as bytes (`cli.rs:1069`).
- **On a real pool.** Not recorded, as the description says. But the
  agent rig's jobs a1, a2 and d passed on the head (Ubuntu 26.04, ZFS
  2.4.1). Each publish there passes `held` with `Held::All` before it
  asks (`plan.rs:921`, `engine.rs:1505`). That runs this count after
  `holds -r`, and needs it to add up. So the count parses and adds up
  on 2.4.1 for the agent's own holds. Not shown:
  - 24.04 (the 2.2 userland);
  - a name gone;
  - tags with a tab or a newline as ZFS prints them, which the
    description asks testing to record.

## Gates

GATES

## CI at 6752363

CI

## What was not checked

- The race and the forgeries are shown on fakes written from the
  source, not on a pool. I did not hold a tag with a tab or a newline
  on a real snapshot.
- 24.04's userland (2.2) for the new count is read from 2.2.2's
  `zfs_main.c` only.
- The plant script, its output and the scratch tests are in this
  directory. They ran on a scratch worktree of the head, not in the
  main checkout.
