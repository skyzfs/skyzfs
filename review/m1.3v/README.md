# M1.3v (skyzfs/skyzfs#47): the in-place store at 16 and 64 KiB pages; `mapped()`

`skyzfs-coding-m1.3v` at cbeac1d into `zfs-topology`; the package's own
commit sits on #46 (4424460) merged with `zfs-topology`. Reviewed as
`git diff cbeac1d^ cbeac1d` (four files). Posted as
https://github.com/skyzfs/skyzfs/pull/47#issuecomment-6842887752.

**Verdict: nothing in the package's own files blocks. It cannot merge before
#46. One finding is a question of design; three are small.**

Findings:

1. On large pages garbage takes blocks that a flush covered; at 4 KiB it never
   does. Probe P (the store's own power-cut test with garbage, 200 seeds): a
   block nothing wrote since its last flush changed at 0 seeds at 4 KiB, 120 at
   16 KiB, 113 at 64 KiB. The refusal by write unit does not bear on it (the
   probe ran with the unit equal to the page). The device advertises a physical
   block of 4096 bytes whatever the page (`queue.rs:290`). For the lead.
2. The watching allocator's two `Relaxed` loads rely on amd64's memory order
   (`watch.rs:61-69`); test code, the instant of `watch()` only.
3. `fs::page_size()` takes 4096 when `/proc/self/auxv` cannot be read; `watch.rs`
   asks `sysconf`.
4. The hole the `mapped_in` test makes can be filled by another thread's
   one-page mapping before the asserts. Unlikely.

Holds: the sizing of `pages` (`bytes.div_ceil(host)`, the count `mincore`
writes, also where the page given is smaller than the system's); the test's
arithmetic; q1 caught, q2 passes on amd64 (the fix waits for arm64), q3 caught
(the large-page tests rest on the rewrite of neighbours); the audit's table
against my own search; `libublk` 0.4.8 takes the page from `sysconf`.

Not done: nothing on arm64 or on a large-page kernel; the workspace's tests,
clippy and feature builds.

## Second look, at ee96ba2

Posted as https://github.com/skyzfs/skyzfs/pull/47#issuecomment-6844216640.
ee96ba2 is a merge of cbeac1d and #46's 1280579 that also carries the answer;
read as `git diff $(git merge-tree --write-tree cbeac1d 1280579) ee96ba2`
(five files, 64 lines added).

**Verdict: nothing of the package's own blocks; findings 1 to 4 dropped. Still
behind #46** (it carries 1280579 with #46's finding A, and not 2eb79ec).

- 1: the limit is a paragraph of `inplace.rs`'s module documentation, in the
  words of design §10. Probe P at this head: 0, 120, 113 of 200 seeds again.
  Left over: line 109 lost its `//!` (the next paragraph runs on in the built
  documentation); "a block a completed flush covered" wants "and nothing wrote
  since"; the named test asserts only "some seed" per large page (the 0 at
  4 KiB is `a_power_cut_leaves_what_the_volatile_cache_disk_promises`), so 120
  and 113 are pinned by nothing.
- 2: `Acquire` loads of the count and a slot's start; the writer stores end
  then start with `SeqCst`. By reading.
- 3: `rustix::param::page_size()` with `use-libc-auxv`. In rustix 1.1.4 that is
  `sysconf(_SC_PAGESIZE)` (`backend/linux_raw/param/libc_auxv.rs:61`), not
  `getauxval` as the comments say; same effect. The feature moves rustix's
  other aux-vector reads to libc for the whole build.
- 4: the test retries when the hole is filled; q1 still fails it.

Ran at ee96ba2 (amd64, 4 KiB pages): `skyzfs-local` 272 passed, 4 ignored,
real files 3 + 3 + 12; `skyzfs-ublk-sys` 8 + 1; `skyzfsd` 53 + 3; q1 caught, q2
passes (no CI machine has pages above 4 KiB, the arm64 runners included), q3
caught; `cargo doc -p skyzfs-local`.
