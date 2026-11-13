# M1.3y: direct I/O on a pool of I/O threads (pull request #58)

Branch `skyzfs-coding-m1.3y`, base `zfs-topology` at aa16288, head
1359259 (reviewed; 47068ad is the last head with code in it, 1359259
changes module documentation only).

## What the work package changes

`skyzfsd serve --store in-place` reads and writes the home image by
direct I/O (`O_DIRECT`) on a pool of I/O threads, eight by default
(`--io-threads N`, 1 to 64). The start asks the file system for its
direct-I/O alignment (`statx`, `STATX_DIOALIGN`) and refuses where it
offers none or its unit does not divide a block. The store serialises
requests that overlap and makes a flush wait for every write on the
threads. The status file is version 9 (2008 bytes) and carries the
threads' queue.

## The review

Nothing blocked the merge. The findings were about what the tests pin,
not about the product code:

1. Four defects planted in the ordering (`must_wait`, `accept`) survive
   every test of `skyzfs-local` and `skyzfsd`. The overlap test cannot
   tell its three clauses apart, because its read waits behind the
   deferred write whatever the clause says; the flush test holds two
   writes, so `.all(write)` passes where `.any(write)` is meant. Two
   sharper cases would pin them (a held read with a write behind it; a
   held write with a read behind it and nothing in between; a held read
   *and* write with a flush behind them).
2. Neither the conformance suite nor the power-cut property tests can
   catch those: on the simulated disk a write applies whole under the
   disk's lock, so a read beside an overlapping write never sees a
   part-written block and two overlapping writes never tear. The
   serialisation is for a real file system, and the kernel rigs are its
   only evidence.
3. The two tests for a panicking thread answer a defect by hanging:
   with the `Life` guard not handing its job back, the run never ends
   (stopped at 330 s). `release_and_drain` and the two `while` loops
   want the bound that `until_done` already has.
4. The module text's example of a file system that accepts direct I/O
   and buffers behind it (ext4 with `data=journal`) is, by the author's
   reading and mine, refused by the `STATX_DIOALIGN` check. The design
   §4.1 and §5.1 rest on the same example; the conclusion (keep the
   cache drop) holds either way. For the lead.

## The files here

- `plants.py WORKTREE [name...]`: plants sixteen defects in
  `crates/skyzfs-local/src/inplace.rs` and `inplace/pool.rs`, one at a
  time, runs the in-place tests under a limit and says which tests
  failed, or that the run hung. Each file is put back after its plant.
- `plants.txt`: its output at 1359259.
- `plants-whole-crate.txt`: the survivors re-run against every test of
  `skyzfs-local` and `skyzfsd` (`SKYZFS_PLANTS_SCOPE=whole`), so that
  "caught by nothing" is said of the crates and not of one module.
- `dioalign-probe.py DIR...`: whether each directory's file system
  takes an `O_DIRECT` open and what `statx`'s `STATX_DIOALIGN` says of
  it. Here: ext4 accepts the open and says 512/512; tmpfs accepts the
  open and says nothing, so the in-place store's start refuses it by
  the alignment and not by the open.
