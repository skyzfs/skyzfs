## Next look at #101 at 449b20e

**Verdict: the review says it can merge once #100 is in and CI is green on the head.** I found no defect.

### What I checked

- **The commands' code is back to what I first reviewed at 7e4b7b8.** Against that commit `cut_commands.rs` differs only in its documentation and in one refusal's wording. My remark that led to the refusal after a failure was mistaken, and the lead decided to take the refusal out.
- **The tests pass here** (rustc 1.100.0): `skyzfsd` with its test hooks.
- **The new test holds the behaviour.** `after_a_failure_the_cuts_whose_flush_completed_are_written_and_let_go` runs on the in-place store, and fails under each of four plants on the store's side of the commands (`plants_next2.py`, `next2_plants.out`): once the volume has failed, a held cut is not read, no cut is listed, no cut is let go, a cut has no map.
- **S1 and S2 are taken.** With a command read again at each later turn, `a_turn_with_no_signal_reads_no_command` fails. With a write's hashes kept only at a label, `a_write_away_from_the_labels_has_its_hashes_and_its_bytes` fails.
- **One plant still passes**, as at the first look: the arm for a block that is not in the cut's map, which I found no way to reach.
- **Codex's P2** (the `write` command does not sync the copies): I agree with the answer. The command is a test hook, and M2.2c writes the copies by direct I/O.
- **CI on 449b20e** was still running when I posted: 17 checks passed, 17 running or queued. The conflict with the base is gone. The daemon jobs will be red on `pin_data_journal_refused` until #102 is in, as on #100.

S3 (`serve`'s own lines) stays with the rig, as agreed.

The files are in `review/m2.2b-2/` on `skyzfs-review`.
