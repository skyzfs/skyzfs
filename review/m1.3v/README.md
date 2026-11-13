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
