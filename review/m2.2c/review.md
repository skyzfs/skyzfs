## Review of #104 at 3251b30 (M2.2c)

**Verdict: the review says it can merge once CI is green on the head. I found no defect.**

One test gap does not block: nothing holds the rule that no copy falls back to the cache (S1). Two remarks on the documentation and on what an operator sees follow it.

### Your three questions

1. **Keeping the start's sync of the copies: yes, that is the right reading.** Design §5.2 keeps a sync for a file system that takes a direct write through its cache, and the start's sync is where a death left that to. It costs one sync a start. The doc in `take_up_cut_state` says what it covers, with one sentence that claims too much (R1).
2. **No fallback to the cache: yes.** A fallback would bring back the gap this package closes, silently, on exactly the file systems where it cannot be seen. Letting the cuts go is the owner's ruling for a copy that cannot be made. What is missing is a way for an operator to learn why (R2), and a test (S1).
3. **The new test pins the two rules it names.** With a copy written through the cache, or read through it, `the_copies_are_written_and_read_past_the_cache` fails and nothing else does. It does not pin the fallback rule (S1).

### What I checked

- **The code first.** The buffer the state hands to the copy's write is an aligned `Buf` (`state.rs:1661`), as direct I/O needs. A new copies file gets its direct descriptor at `create` as at `open` (`fs.rs`, `FsFile::new`). The daemon's I/O is always direct (`IoMode::with_threads`).
- **The tests pass here** (rustc 1.100.0): `skyzfs-local` with its test hooks and `skyzfs-mapped`, none failed.
- **On real files, a copy leaves no page** (`real_copy_pages.py`, `real_copy_pages.out`). Your description says the mechanism has not run on a real kernel. I ran it on ext4 here, with the kernel's own count (`cachestat`) of `copies.img` after three copies for a held cut and after the cut was read from one:

  | | pages after the copies | pages after the read |
  |---|---|---|
  | the head | 0 | 0 |
  | W, a copy written through the cache | 3 | 3 |
  | R, a copy read through the cache | 0 | 1 |

  The copies there are writes that extend a new file, which is the case I was least sure a file system would take past its cache.
- **Seven plants** (`plants.py`, `plants.out`): 5 caught, 2 pass.
  - Caught: W and R, by the new test alone, as you say; a copy written by direct I/O whatever the store's I/O is; the copies file not synced by the start; the region's file emptied only where it grows, by `a_region_only_cut_short_keeps_nothing`.
  - Pass: S1 below, and a copy read by direct I/O whatever the store's I/O is, which changes nothing in the daemon.
- **CI on 3251b30:** 31 checks passed, 3 were still running when I posted, none failed.

The files are in `review/m2.2c/` on `skyzfs-review`.

### S1 (test gap, does not block): the rule that no copy falls back to the cache has no test

(`inplace.rs:1927` to `:1935`. Confidence: high, run here.)

- With a copy whose direct write is refused written through the cache instead, every test passes.
- The new test fails the copies file's writes of both kinds at once, so a fallback fails as well and the cut still goes.
- The simulated disk can refuse direct I/O only for the whole disk (`set_direct_align(None)`). I tried that: the read of the image for the copy is refused first, so the copy's write is never reached, with or without the plant.
- A test needs the simulated disk to refuse direct I/O on one file. Then: cut 1 held, the copies file refuses direct writes, a write changes a block cut 1 reads; the cut must go and `copies.img` must hold no page.

### Remarks

**R1. One sentence in `take_up_cut_state` says more than holds.** (`inplace.rs:1967` to `:1970`.) It says of a file system that takes a direct write through its cache that "the kernel reports the failure to the first sync after it, this one". That is the claim Q1 showed false where the inode was evicted in between. The doc of `Copies` just above says it right. For such a file system the gap stays, and the sentence should say so.

**R2. Nothing says why a copy was refused.** (`state.rs:1612` to `:1616`; older than this pull request.) The error of a refused copy is dropped, and the status counts the cut as abandoned. With no fallback, a copies file that takes no direct I/O gives a daemon that serves, holds no cut, and shows only a count that climbs. The start checks direct I/O on the image and not on the copies file, which is fair for one file system. Keeping the last refusal's reason for the status or the log would let an operator see it. That may be M2.4's, where the status is to say how old the recovery point is.

**R3. For M2.T4.** A copy is now a direct read and a direct write on the queue's thread, one block at a time. A 1 MiB write over blocks a held cut reads is 256 of each before the write begins. That is the cost the criterion leaves to M2.T4; I name it so that the measurement includes a write of that shape.
