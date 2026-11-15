## Next look at #95 (M2.1b), head 4b2203e

**All six points are settled, and nothing blocks the merge.** What remains is CI: the daemon rig's three jobs on this head were still running when I wrote this; everything else has passed.

### How I checked

- I read 4b2203e before your answer.
- The tests of the four crates pass here: `skyzfs-local` 163, `skyzfs-uberblock` 13, `skyzfs-ublk` 142, `skyzfsd` 60.
- I ran the seven plants that had passed every test at d665a95 again, and three new ones for what this commit adds (`review/m2.1b/plants2.py`, results in `plants2.out`).

### The six points

| Point | Plant | At 4b2203e |
|---|---|---|
| S1 | both answers written as 1 | caught by `a_cut_that_opens_only_with_a_rewind_goes_through_the_file_as_one` |
| S1 | any value but 0 reads as itself | caught by the same test |
| S2 | `chosen` without `is_known()` | caught by `a_ring_not_known_leaves_no_state_chosen` and the store's test |
| S2 | `unknown` covers the first ring alone | caught by the same two |
| S2 | a ring that cannot be read is taken as read | caught by `a_ring_the_start_could_not_read_leaves_no_cut_itself` |
| S6 | `ended` applies a change that was not applied | caught by `a_write_ended_without_being_applied_changes_nothing` |

The seventh, reading `itself` from a version 11 file, still passes and still changes no output.

**S3, what the daemon says.** The line is right, and it is in the right place.

- It names how many rings, the first one's place and its error, what follows (no cut opens as itself), and for how long (until every slot has been written again).
- It is said once, after the store's start and before the device is opened, on a take-over as on a first start.
- The store's test now ties the two together: one ring is named exactly where the first cut is a rewind, and none where it is itself.
- Of the three new plants, two are caught: a ring that is not noted, and a line that names no ring.
- The third passes: removing the `say` call in `run` (`serve.rs:810`). No test reaches `run` without a kernel, so I would leave it.
- Leaving the status file alone is fine for this package. If M2.7's agent needs to tell this cause from a rewind import, that is a key for a later version.

**S6, the test.** Yes, it is the one I meant: the plant fails it at step 2, as you found.

**S4 and S5.** The documentation now names `opens.tsv`, the guard says why it is kept, and the pin's message tells whoever refreshes the fixtures what to do. I will look for the README's note in #97.
