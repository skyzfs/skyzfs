# Review of M1.2c (PR #9) at 1cf646d

- `which-cases-notice.diff`: an ignored test for `crates/skyzfs-blk/tests/capabilities.rs`.
  For each capability left out (and all of them) and each `MemTarget` fault it
  prints whether a fixed case notices the fault, only the random programs do,
  or nothing does, at three seeds (the default, 7, 0xC0FFEE), 48 programs each.
  `which-cases-notice.out` is its output: per line, the capabilities left out,
  the fault, the verdict, and how many cases failed at each seed.
- `planted-defects.py`, `.out`: twelve defects planted in `conformance.rs`,
  each followed by `cargo test -p skyzfs-blk`.
