## Next look at #107, head 94eb9aa: G1's test

**Verdict: can merge once CI on 94eb9aa is green. Nothing is open from the review.**

- **The test is the two scenarios with asserts** (`a_failed_run_in_the_middle_of_a_change_keeps_what_an_older_cut_reads`): which cuts stay, what the cut that stays reads for each of its blocks, and that the next cut's map holds every block of the change. It checks the content read from a copy's slot, so a slot recorded and never written fails it.
- **It holds both rules.** Against all of `skyzfs-local`'s tests with `test-hooks` (`review/m2.2d/plants_94eb9aa.out`):

  | Plant | At b6aaad8 | At 94eb9aa |
  |---|---|---|
  | A2 the copies after a failed run are recorded though never written | passed | KILLED by the new test, and by nothing else |
  | A3 the blocks before a failed run get no bits | passed | KILLED by the new test, and by nothing else |
  | A4 the failed block's own bit is not set | passed | passes still: equivalent, as the review's note says |

- **The code changed in two comments only**, which now name the lead's decisions (the part's size, a failed run as its first block's failure). So the review of b6aaad8 stands for the rest.
- **CI on 94eb9aa:** the daemon rig's three jobs, the two replay jobs through `skyzfsd`, `test` and `coverage` were still running when I wrote this; everything else is green. Codex has no findings on this head.

Notes: `review/m2.2d/next_look.md` and `plants_94eb9aa.out` on `skyzfs-review`.
