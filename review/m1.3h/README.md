# M1.3h (skyzfs/skyzfs#25): the daemon says where a request's time goes

Reviewed: `origin/zfs-topology` 06b6dff ... `origin/skyzfs-coding` e23d08c.

## What was done

- Diff read before the description. fmt, clippy (-D warnings) and the
  tests of skyzfs-local, skyzfs-ublk and skyzfsd: clean, all pass.
- Planted defects, one at a time, the store's or the status tests run
  against each:
  - A: the held request put back without its submission time: all 193
    pass.
  - G: a panic in the served worker's held branch: all 193 pass (no
    unit test reaches that branch).
  - B: the gate counted for every step: caught.
  - F: the status reader takes two step parts in the other order:
    caught (4 tests).
  - H: two `time_*` keys swapped: caught.
- A temporary test: 239 steps of an unserved store, the four parts'
  sum against the clock around the same steps.
- Not done: no kernel run; `clock_numbers` not rerun.

## Findings

1. Blocking. A third of a step's time is in no number. `checkpoint_step`
   walks the log to choose its batch (`replay_until`, records read and
   checked) before `apply` reads its first clock, and drops the index
   after the last. Temporary test, simulated disk: 239 steps, 51.5 ms
   by the clock, four parts 32.7 ms (63%; 65% in a debug build), own
   sync 0. The description's "their sum is the time in steps" does not
   hold. Outside a gated turn that time is counted nowhere.
2. Not blocking. The held branch of the served worker is reached by no
   unit test, and this package changed it. With the put-back of the
   submission time left out, a held request that is tried again is
   taken from the queue and dropped (`pop_front().zip(...)` gives
   nothing when the times run out): no answer, no error. One queue of
   pairs would make that impossible.
3. Not blocking. `reads_behind_step / read_wait` is not "the share of
   the reads' waiting that was behind a step": a request counts its
   whole wait if any step ended during it. It is an upper bound.
4. Not blocking. A write that finds the log full makes room inside its
   own call (a sync and a whole checkpoint): counted in the write's
   call and in the step parts both, and nobody behind it counts as
   behind a step. Same for the explicit checkpoint command.
5. Not blocking. The daemon takes the log's state and the times with
   two locks: the two can be of different moments.

## Part 5 of the description

- Option 3's rule "a step does not start writing while a read that
  began before the step is still running" is not needed while reads
  stay on the worker: every record a running step applies was appended
  before the read began and is still in the index, so the read takes
  those blocks from the log. The case arises only with option 4.
- Not named: the log's sync and the home image's sync are on one file
  system. On ext4 both go through one journal, so a flush on the
  worker can still wait for the step thread's sync.
