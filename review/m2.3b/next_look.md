## Next look at #106, head ab2d7f2 (M2.3b)

**Verdict: can merge once the daemon rig's three jobs on ab2d7f2 are green.** Both defects of the review are fixed, the gaps are closed, and the two things I was asked to judge hold. One small defect is left, which Codex found on this head and I confirm by a run (N1 below). It does not block: `serve` cannot start a store in the mode that has it. I would still take the one line here.

I read 3c8c5fb and f7a11c3 from the code before the answer, and ran everything below on ab2d7f2 (rustc 1.100.0, `skyzfs-local` with `test-hooks`).

### What I was asked to judge

**The check in f7a11c3 (`intent_synced`, `inplace.rs:2610`). Right, and I found no way for it to fail a healthy volume.** The rule is: a request that is not covered when a sync ends was lost if it began waiting before that sync began (`waiting.begun < intent.begun`) and no commit came since (`waiting.commits == intent.commits`). I walked each way a request can still be uncovered at a sync's end:

| The request | begun | commits | The check |
|---|---|---|---|
| arrived with no sync under way: pushed with B, the sync it begins is B+1 and writes its page | less | same | must be covered, and is |
| arrived while sync B ran: its bit was set after the sync's pages were written | equal | same | exempt; pushed again with B, covered by sync B+1 or caught then |
| its bit was cleared by a commit while it waited (`intent_committed`) | less or equal | differs | exempt; set again and pushed with the present counters |
| the map's page was written again during the sync by a commit (#105's D1: the sync is credited with the narrower words) | any | differs | exempt |
| the file cannot begin a sync on a helper, so `begin_intent_sync` ends in `intent_synced` itself | less | same | must be covered, and is |

The counters are taken before `begin_intent_sync` in `wait_for_intent` and the sync counts itself at its top, so the first row holds on the inline path too. The only looseness goes the safe way round: a commit that did not touch the request's bit also exempts it, for one more sync. That can delay the verdict on a wrong map by one round per commit, and cannot go on without commits.

Planted, one at a time (`review/m2.3b/plants2.py`, results in `plants2.out`):

| Plant | Result |
|---|---|
| L1: the first exemption taken out (`lost \|= commits equal`) | KILLED by `writes_into_clean_regions_while_the_map_syncs_share_the_next_sync` |
| L2: the second exemption taken out (`lost \|= begun less`) | KILLED by `a_write_whose_bit_a_clear_takes_while_it_waits_sets_it_again` |
| L3: the check taken out | KILLED by `a_sync_of_the_map_that_leaves_a_bit_set_before_it_unwritten_fails_the_volume` |
| L4: a lost bit fails the waiting requests and not the volume | KILLED by the same test |

So each exemption is needed by a test of a healthy store, and the check itself by the test of the mutant.

**The drop's loop in 3c8c5fb (`inplace.rs:901`). Right.** It goes on while a request is on the threads, waits for the map, or is deferred, and each round does the one thing that can move it: waits for the map's sync, or fails what waits on a failed volume, or begins a sync. It ends: a deferred request that must still wait has either a job on the threads or a request waiting for the map in front of it, and both are waited for. Like the image's sync, it can block only on a sync the disk never ends. My reproducer of D2 now ends in both cases with the image written once (`drop_while_waiting_ab2d7f2.out`).

| Plant | Result |
|---|---|
| D2a: the loop does not look at the map's sync | KILLED by `a_store_dropped_while_a_write_waits_for_the_map_does_the_write_first` |
| D2b: the loop does not go on for a request that waits for the map | KILLED by the same test |
| D2c: `self.run_deferred()` taken out of the loop | **passes every test**; see N1 |

### New on this head

**N1 (defect, small, does not block). With no I/O threads, a request deferred behind a write that waited for the map is not answered when the volume fails, until the map's sync ends.** This is Codex's P2 on ab2d7f2 (`inplace.rs:1485`), and I missed it in the review: it was there at 7729360.

- **Why.** `settle_failed` answers the requests that wait for the map and leaves `deferred` as it is. With threads, the next `poll` runs what is deferred at the end of `collect`. Without threads `collect` returns at its first line (`inplace.rs:1862`), so nothing runs them until `intent_synced` does, when the map's sync ends.
- **By a run** (`review/m2.3b/failed_while_deferred.py`, `.out`): a write into a clean region waits, a read of its block waits behind it, the disk holds the map's sync, `fail_now()`. Within 200 ms of polling, 8 threads answer both with `Failed`; 0 threads answer the write only.
- **How far it reaches.** `serve` takes `--io-threads` 1 to 64, so the daemon has no store without threads today. It is the tests' `ON_CALLER` mode and the library's `IoMode { threads: 0 }`. On a disk whose sync never returns, such a store would hold the request for good, which is what `fail`'s comment says must not happen.
- **The drop.** This is the one case that needs the loop's own `run_deferred()`: with D2c planted, the same run's drop does not end in 15 s without threads, and ends as the head has it. No test of the head has the case, which is why D2c passes.
- **A fix.** Let `collect` run what is deferred before it returns for want of a pool (`let Some(pool) = &self.pool else { return self.run_deferred(); }`). That covers every caller of `settle_failed` at once. A test: my run's steps over `[IoMode::default(), ON_CALLER]`, asserting both answers while the sync is held, then the drop.

Coding answers Codex's thread either way.

### The review's findings

| Finding | State | How I checked |
|---|---|---|
| D1: the map's name not synced | fixed (`self.dir.sync()?` after the creation) | my reproducer now reads `TakenUp { named: 1, regions: 4 }` (`name_not_durable_ab2d7f2.out`); plant D1a KILLED by two tests |
| D2: the drop with a write waiting for the map | fixed | above |
| G1: no write-zeroes asks the map | closed | plant W3 KILLED by the power-cut test |
| G2: no flush alone behind a waiting write | closed | W2 KILLED by `a_flush_waits_for_a_write_that_waits_for_the_map` |
| G3: no second start on a taken-up map | closed | S8 KILLED by `after_a_reboot_the_next_cut_holds_the_regions_the_map_names` |
| G4: W9, W10, T4, T5 | closed | W9 and W10 KILLED by the two new tests; T4 and T5 have tests in `serve.rs` that I read, and they compare what the store was given and what the status shows |
| S1: the power-cut test narrowed in 144 of 1797 starts | in part: a floor of a third taken up and a tenth narrowed | accepted. The seeds are fixed, so the floor is a fact about the test and not a chance; the larger volume can wait for a test that needs it |
| S2: `named().len()` at every status write | done (`named_count`) | N1 of my plants KILLED; see the note below |
| Note: direct I/O refused after the map was written | done (`check_direct_io` before the cut state is taken up) | plant C1 KILLED by `a_start_refused_for_direct_io_creates_no_map` |
| Addendum: plant A9 did not end | done (the check above, mutant `INTENT_SET_UNWRITTEN`) | L3, L4 |

### Notes, nothing to do

- **`named_count`'s condition `region + 64 <= regions`** can be dropped without a test failing (my plant N2). It is equivalent as long as no bit past the last region is set, and nothing in the store sets one; only a whole file read from disk with such bits would count them. `named`, `covers` and the carry into the cut state all stop at `regions`, so nothing but the status's figure could be wrong. Leave it.
- **`narrowed_count.py`** of the review no longer applies (the test's assertion became the floor); the test's own floor replaces it.

### CI and Codex

- On ab2d7f2 every job is green but the daemon rig's three (`ubuntu-24.04`, `ubuntu-26.04`, `ubuntu-26.04-arm`, run 43727409263), still running when I wrote this. The replay jobs through `skyzfsd` and the cut rig's record-and-check are green.
- Codex: the P2 on 7729360 is answered and resolved. The P2 on ab2d7f2 is N1 above and is open.

### Files

`review/m2.3b/` on `skyzfs-review`: `next_look.md`, `plants2.py`, `plants2.out`, `plants_ab2d7f2.out`, `failed_while_deferred.py`, `failed_while_deferred.out`, `name_not_durable_ab2d7f2.out`, `drop_while_waiting_ab2d7f2.out`, `narrowed_count_ab2d7f2.out`.
