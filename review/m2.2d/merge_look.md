## Look at #107, head e25bf3a: the merge with `zfs-topology` (#106 in)

**Verdict: the merge is confirmed. #107 can merge once CI on e25bf3a is green.**

- **Nothing was resolved by hand.** e25bf3a is 94eb9aa merged with `zfs-topology` at c1bb85c, and its tree is the one git's own merge of the two gives (`git merge-tree --write-tree`: 39d77b1, the same). The base has moved on since c1bb85c, with no Rust (#108's benchmark files, the guide and the notes).
- **Where the two packages meet, the order is #106's.** In `execute` (`inplace.rs:1529`) a write or a write-zeroes first has its regions named durably in the intent map, or waits for the map's sync; only then `write` and `write_zeroes` (`:1559`, `:1596`) call `note_change`, whose inside is #107's plan, copy and record; then the write begins. A write that waited goes the same way through `resume`. So a write's copies and its bits in the cut state come after its regions are durable in the map, and before the image changes.
- **What I looked for besides, and did not find:** a path where the two could interleave. The plan, the copies and the bits of a change run in one call on the queue's thread, so a commit's clear of the map (`intent_committed`, which reads the cut state's maps) sees a write either wholly noted or not at all, as on #106.
- **The tests pass on the merge here** (`review/m2.2d/merge_tests.sh`, `merge_tests_e25bf3a.out`): `skyzfs-local` 258 plain and 260 with its test hooks, `skyzfsd` 66 and 99, none failed.
- **CI on e25bf3a** had only begun when I wrote this: the daemon rig, the replay jobs and the unit jobs were all still running. Codex has no findings on this head.
