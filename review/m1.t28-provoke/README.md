# M1.T28 follow-up: the repeat rig provokes the late return (pull request #62)

Reviewed at 2f2aa70 against `zfs-topology` at 26c4f63. The review is
https://github.com/skyzfs/skyzfs/pull/62#issuecomment-6861908369.
Verdict: nothing blocks the merge.

## Findings

1. The provoker in `tests/kernel/c-repeat-rig.sh` runs bare `dd` with
   its errors discarded. On ubuntu-26.04 `dd` is the Rust coreutils;
   every other rig takes `"$DD"` from `tests/lib/gnu-dd.sh`. The runs
   show both loops run there; they do not show that `conv=nocreat`
   creates nothing with that `dd`. Not tried here: building the Rust
   `dd` was not permitted in this session.
2. The stop sequence (`pkill -P`, `kill`, `wait`) leaves the reader's
   current `dd` running: it is a grandchild. `provoker-stop.sh`
   simulates the process structure; with a process group nothing is
   left (`provoker-stop.txt`).

## What was confirmed

- The replays (`replay62.txt`): the artifact of run 43476467345, every
  volume of its three kept passes, through `skyzfs-blkcheck` built at
  b032325 (the model before), bb69c4c and 8e8cb23 (after). Only the two
  failing volumes are wrong, and only before.
- The trace library on a real kernel (`ftrace-state-2f2aa70.txt`): a
  buffer found as `7 (expanded: 1408)` reads 1410 afterwards.
- The plants (`plants62.txt`): the three new test lines each fail with
  their defect planted.
- `trace-look.txt`: in a kept trace `dd` issued about 22,000 buffered
  reads a pass, and one requeued request can leave two trace lines.

## Precision on the control

"2 of 20 before" joins the run before the `nocreat` change (2 of 10,
work disk `sda1`) with the run as merged (0 of 10, `nvme0n1p1` like the
five others). The provoker as merged has not itself produced the
failure with the model before.

## For next time

- A run's head can be a commit that is no longer on the branch. Fetch
  it by its hash and diff the files that matter against the head.
- Hand a subagent the reading of several job logs when only table rows
  are wanted: it kept about forty thousand tokens out of this session.

## Second look, 64e8d92

Both defects fixed: `"$DD"` from `gnu-dd.sh`, and the provoker in a
process group of its own, stopped as a group. `provoker-stop2.sh` runs
the script's own `provoke_start` and `provoke_stop` around a blocking
stand-in with no controlling terminal: nothing is left
(`provoker-stop2.txt`). It can merge from the review's side once CI is
green. Still open as a suggestion: one provoked run at the head, since
the provoker as it now stands has not run on a kernel.
