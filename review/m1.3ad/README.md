# M1.3ad (SYNC_RETRIED noticed alone), pull request 70

Head 8063ef7 on 3d8716a. Review posted on 2026-11-14 (issue comment
6869538770): can merge, nothing blocking.

- `loop.txt` (`ackloop.sh` on the head's library tests, unplanted):
  `the_suite_notices_the_defects` passed 12 of 12 alone, 12 of 12 on
  one CPU, 12 of 12 beside the whole library suite.
- `plants70.txt` (`plants70.py`, 6 runs of the test alone each, and the
  in-place conformance tests once):
  - F1b, `finish_threads` ends the sync it began, no hold: 0 of 6 (the
    defect 0x4 not noticed). F1 (the same inside the hold) hangs: the
    held sync makes `settle(true)` wait for a release that comes after.
  - F2, no hold and no release in `fail_log`: 6 of 6. The hold is a
    guard: `sync_end` honours it, but nothing on the path calls it.
  - F3, no seed settles: 0 of 6, as at the base.
  - The conformance tests pass under all three.
- CI at the head: green but the daemon rig on 24.04, where only g.1's
  three lines differ (the premise pull request 72 is for).
- Lesson: a background plant script edits the worktree; read the
  committed code with `git show`, not the worktree, while it runs.

## Second look, head 7ed676a

Posted on 2026-11-14 (issue comment 6870573013): can merge.
`look2-posted.md`. `finish_threads` panics after its minute (in a
`#[cfg(test)]` impl), and the merge of 81247f2 (pull request 72) brings
no Rust file. The test alone at the head: 4 of 4, clippy clean.
