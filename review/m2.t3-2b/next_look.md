## Next look at #111 at 1f71211 (the commit after 8210332 only)

**Verdict: it can merge. The commit is right and nothing blocks.** The rule is the daemon's own: a start takes the intent map up only if it has a base, and only a release answered `ok` gives it one. I found no way for the new rule to fail a right daemon. It is laxer than it needs to be in one place, which today's recording no longer reaches (R1), and the wait it adds makes course (a) carry less across the kill than before (R2). Both are remarks, not defects. Of 20 plants in the new logic 9 are caught and 11 survive; the survivors are test gaps and are listed below.

I formed this from the diff, the daemon's code and the runs' artifacts before I read the author's answers. Codex has no findings on this head.

### The rule against the design and the store

- **What the daemon does.** At every start the map is read and `trust` is asked (`inplace.rs:2294-2297`): it must name the start before this one and it must have a base, else it is begun anew with every region named (`intent.rs`, `trust`, `Distrust::NoBase`; the module's "Trust", third rule). Whether the start took the device over or created it plays no part. Design §5.2 gives the reason in one line: after a restart the named regions "are compared with the newest uploaded cut", and with no such cut there is nothing to compare with.
- **What gives a base.** Only `release_cut`, through `intent_committed` (`inplace.rs:2850-2895`): it names the base, clears, and writes the header page before it returns, and the command answers `ok` only after that (`cut_commands.rs`, `release`). So a release that the holder's log shows as answered is a base on the file, whatever kills the daemon next.
- **The clocks agree.** The holder's log and the request log both write `CLOCK_MONOTONIC` in nanoseconds (`cut_hold.py:167`, `record.rs`, `mono_ns`), so `committed > inst['ns']` compares like with like.
- **Once there is a base it stays**, since every later start takes the map up and keeps it. So holding every start after the first answered release to `taken_up` is right, and one time (the first) is enough.
- **No false verdict the other way.** A logged `ok` before a start with no base on the file would need the answer to come before the header's write, which the code does not do. The other order (the daemon committed, was killed, and the holder never logged the answer) falls under "either".

**On the failed run** (44165956107, the stage 2c branch): I read its artifact. The first holder asked `held` once, about 10 ms before the first cut was counted, got nothing and left 0.2 s later; the first release was answered 4.36 s after the second start. The daemon said "the intent map has no committed cut" and began the map with 128 of 128 regions named. The judge of this head on that recording: exit 0, `instance_2` PASS with "no cut committed before it", every other line as the run had it (`next_rejudge.out`). On the three earlier recordings it gives the lines the runs gave.

### `holder_wrote_one` (read only; no kernel here)

Its reasoning holds. The holder writes the line in `held.tsv` after the cut's write and before it sends the release (`cut_hold.py:223-226`), and it looks at the leave file only between two steps (`:243`). So a holder asked to leave once `held.tsv` has a line has had its release answered. The one way out is a release refused because the cut was abandoned under it, which needs the bound and does not happen on an idle pool; the judge then takes either, so nothing wrong follows.

On this head's run of the cut rig on the daemon (44169532248) the first release was answered 4.30 s and 4.52 s before the second start, and all three later starts say `taken_up` on both images.

One small thing: the loop does not look whether the holder is alive, so a holder that died is the rig's error only after 300 s.

### R1 (remark). Where no cut was committed, `taken_up` passes too

`cut_daemon_check.py:761-762`. With no release answered before a start, the judge takes `begun` or `taken_up`. A daemon that takes up a map with no base, which is what the store's third rule forbids, passes: I changed the failed run's second status to `intent_state taken_up` by hand and the judge says PASS, exit 0 (`next_lax.out`). The same branch keeps `cut_state` strict and refuses `off`, which I also ran.

Either is needed only for a release that was asked and not seen answered. Where no release was even asked before the start, the map cannot have a base and `begun` is the only right answer: the holder logs the ask before it signals the daemon (`cut_hold.py:167`). In the failed run the first release was asked 4.33 s after the second start, so the tighter rule decides that run, and the daemon passes it.

Two things follow from the wait, and I would say them in the description:

- Since the recording now waits for a commit, no recording reaches the "either" branch. The begin-anew of a map with no base is then shown by the store's unit test alone (`intent/tests.rs`, `Distrust::NoBase`); there is no mutant for it.
- Nothing fails if the commit does not come first after all (a refused release, a later change to the course): the line passes and only its text says "no cut committed before it". For `takeover` that could be the rig's error, as the floors of `edges` are. This is the same family as G1 of the first review.

### R2 (remark). Course (a) now carries a smaller cut across the kill

Before this commit the first holder was still writing the whole volume when it was asked to leave, so the cut held at the kill was the load's (about 3,500 blocks). Now it has caught up, and the cut held is a small one.

| Run | Cut written across the first kill | Blocks of it read from a copy the killed instance made |
|---|---|---|
| 8210332, three runs | 7, 9, 8 (3,447 to 3,541 blocks) | 20, 69, 28 |
| 1f71211, 26.04 | 10 (315 blocks) | 4 |
| 1f71211, 26.04-arm | 10 (312 blocks) | 4 |

The four are the cut's uberblock slots in the four labels (blocks 49, 113, 130993, 131057), which come round again once the writer has synced 32 groups. A daemon that loses its copies at a start is still caught on both recordings (`held_content` FAIL, exit 1; `next_copies_lost_at_start.out`), so nothing is lost today. But it now rests on four blocks, and on the writer reaching 32 groups in its 3 s (it made over 300 in these runs). The judged line with a floor that the author has put in the later package would hold it; this commit makes that line more worth having. Courses (b) and (c) are as before: 0 blocks.

### Plants (`next_plants.py`, `next_plants.out`)

Unit tests on this head pass here: 67, 13, 38 and 12. Twenty plants, one at a time, the file restored after each, the worktree clean afterwards.

**KILLED (9):** the first release asked instead of answered (F1); any command's `ok` (F2); a refused release (F3); no release answered read as time 0 (F7); the old rule, every later start strict (L1); every later start lax (L2); the comparison turned round (L5); the line not saying "no cut committed before it" (L11); a begun map after a commit being no verdict (L12).

**SURVIVED (11):**

| Plant | What survives | A test that would hold it |
|---|---|---|
| L6, L7 | the commit is held to the first start's time, or to the start before this one | the fixture's commit is at time 3, before the first start, which no run has. A commit between the first and the second start with a begun map at the second, wanted FAIL, holds both. L6 would make every later start lax in every real run |
| L8, L9 | where the map takes either, the cut state and the `cuts` line take either too | the `late` log with `cut_state begun`, and with a `cuts` line `begun`, at the second start: wanted FAIL |
| L10 | where either is taken, the map's state is not looked at (`off`, or nothing said, passes) | the `late` log with `intent_state off`: wanted FAIL |
| L4 | with no release answered at all, a later start must take the map up | a log with no release and a begun map at the second start: wanted PASS |
| F4 | the last release answered instead of the first | two answered releases, one before and one after the second start |
| F5 | a write's `ok` counts as a commit | a `write` line with its `ok` before the release, as every real log has |
| F6 | a malformed line ends the reading | small |
| L3, L13 | the first start's map takes either, or is not looked at | the first start with `intent_state taken_up`: wanted FAIL. This gap is older than the commit |

The judge's code is right in each of these places; the tests would not notice if it stopped being so.

### CI and Codex on 1f71211

- Codex: completed, no findings.
- Green: the cut rig on the daemon on ubuntu-26.04 and ubuntu-26.04-arm (run 44169532248), node B's rig on both images, lint, test, unit-arm64, e2e-tier-a, coverage, s3-backends. I ran this head's judge here on both takeover recordings of 44169532248: exit 0 and the run's lines.
- Green as well: the cut rig on dm-log-writes on its four images (run 44169532258). It runs `cut-record.sh`, which this commit changes, but not the takeover recording.
- The daemon rig, the replay rig, the control rig and the kernel probe were cancelled by the author again, with the push's duplicate runs. As before, none of those four runs a file this pull request changes.

### What I could only read

- `holder_wrote_one` and the course in `cut-record.sh`: no kernel, ublk or ZFS here. I read them against the artifacts of 44165956107 and 44169532248. shellcheck is not installed here; CI's lint ran it.
- That a release is refused only at the bound is from the store's code, not from a run.

### Evidence added to this directory

`next_plants.py`, `next_plants.out`; `next_rejudge.py`, `next_rejudge.out`; `next_lax.py`, `next_lax.out`; `next_held_across.out` and `next_copies_lost_at_start.out` (the first review's scripts on this head's two recordings). The artifacts are not kept here.
