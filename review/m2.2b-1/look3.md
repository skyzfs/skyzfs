## Look at #100 at 5fd84fb

**Verdict: the review says it can merge once CI is green on the head.** That needs #102 merged and `zfs-topology` merged in here, for the rig's one line (B1, decided by the lead). I found no defect in what this head adds.

One new finding does not block in my view (N1). It is Codex's new P2. It is real at the store, and `serve` cannot reach it today.

### What I checked

- **The change since 07a9f2d** is two tests and the merge of `zfs-topology`. No product code changed.
- **The tests pass here** (rustc 1.100.0): `skyzfs-local` with its test hooks, 205 with 1 ignored, and 13.
- **Both new tests hold what they are for** (`plants_next3.py`, `next3.out`):
  - with the state taken up before the copies are judged (K8), `a_start_that_heard_a_failed_sync_of_the_copies_lost_at_any_store` fails;
  - with the entry left in the word when its cut is counted, `a_second_start_after_a_death_in_an_abandoning_counts_the_cut_once` fails.
- **A start that finds the volume held writes no token:** agreed. The open takes the lock by value, and `a_second_instance_is_refused_and_changes_nothing` compares the whole simulated disk.
- **CI on 5fd84fb** was still running when I posted: 21 checks passed, 13 running or queued. The daemon jobs will be red on `pin_data_journal_refused` until #102 is in.

The files are in `review/m2.2b-1/` on `skyzfs-review`.

### N1 (does not block, in my view): a region's file shrunk and regrown by two refused opens is trusted again

This is Codex's P2 on `skyzfs-mapped/src/linux.rs:95` ([thread](https://github.com/skyzfs/skyzfs/pull/100#discussion_r4577612333)). I think it is right, and I reproduced its effect at the store.

- **Where.** `Mapped::open` gives the file the length asked (`linux.rs:93` to `:96`) before it allocates, maps and locks it. The store's open maps the region before it writes its token (`start.rs:549` to `:555`).
- **The sequence.**
  1. Cut 1 is held, with copies made for it.
  2. An open with a smaller bound is refused at the lock. It has cut the file short, and written no token.
  3. An open with the first bound is refused at the lock too. It has given the file its first length back, with zeros where the records of the copies were.
  4. The next open finds the length it asks for, so the region counts as kept. The header and the token are as they were, and the state is taken up.
- **What goes wrong.** Cut 1 reads a block whose record is gone from the image, which holds what was written after the cut's instant.
- **Reproduced** (`resized_twice.py`, `resized_twice.out`), on real files, with the file's length set as steps 2 and 3 leave it. I could not make the lock itself fail here. The next start shows `TakenUp, held [(1, 7)]`, and cut 1 reads blocks 6 and 7 as 0x66 where it held 0x55. Block 5 still reads right: a bound of one block keeps the first record.
- **Why it does not block.** `serve` passes no bound (`serve.rs:710`), and the region is in the volume's own run directory. So a volume's region is asked for with one length as long as the default bound and the layout stay as they are. It becomes reachable the day the bound is an option.
- **A fix.** Where the length found is not the length asked, cut the file to nothing before giving it its length. Then no later open finds a header in it. Here or in M2.2c is yours and the lead's to say; it should be in before the bound can be set.
- **How sure.** High for the store, by the run. For `serve`, by reading.
