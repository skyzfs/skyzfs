# M1.3b (skyzfs/skyzfs#15) at 499bdb4: review notes

Nothing here ran on a kernel.

## The queue's buffers are freed while the ring still holds their fetches

Read from `libublk` 0.4.8 (`src/io.rs`) and the driver (v7.0):

- The queue's ring is a thread-local (`QUEUE_RING`, io.rs:208). `impl Drop
  for UblkQueue` (io.rs:1309) unregisters the fixed files and unmaps the
  command buffer. It does not close the ring and cancels nothing. The ring
  goes when the thread ends.
- `KernelQueue` drops `queue`, then `first` (the library's buffers), then
  `own` (one `Buf` per tag). `Pump` drops `queue` before `target`, so the
  store's shutdown runs on the queue's thread after the buffers are freed
  and before the thread ends.
- The driver copies a write's data to the address of the tag's last fetch
  or commit in task work of the queue's thread (`ublk_dispatch_req`,
  `ublk_map_io`), whenever that thread next returns to user space, for as
  long as the fetch is outstanding.

`freed-still-mapped.rs` (rustc -O, run here): eight buffers of 1 MiB + 4095
bytes made and freed on a second thread. Before any large buffer was freed
in the process, 0 of 8 are still mapped after the free (glibc gave each its
own mapping and unmapped it). After one such free, 8 of 8 stay mapped: glibc
raises its mmap threshold to the size of the first such block freed, and
later blocks of that size come from the heap.

## The last commits of an orderly stop are never submitted

`complete_io_cmd` pushes the commit on the ring's submission queue
(io.rs:1826, `__queue_io_cmd_no_state`) and submits only when the queue is
full. The submit is in the next `flush_and_wake_io_tasks`. `Pump::turn`
commits and returns `Stopped::Orderly` with no wait after it.

## `create`, run here with the branch's binary

- `mkdir d1; echo keep > d1/home.tar; echo keep > d1/log.txt; echo keep >
  d1/other; skyzfsd create --dir d1 --size 1048576`: exit 0, "created d1".
  Afterwards d1 holds `home.img lock log.000000 log.super other`:
  `home.tar` and `log.txt` are gone. The store removes every name that
  begins `home.` or `log.` as a leftover of an earlier create
  (`crates/skyzfs-local/src/store.rs`, `log.rs`).
- `skyzfsd create --dir d2 --size $((1<<50))` on a directory that does not
  exist: "creating the home image failed (FileTooLarge)", exit 1, and d2 is
  left, mode 0700, with `lock` and an empty `home.new`.
- `skyzfsd help | true`: a panic, "failed printing to stdout: Broken pipe",
  exit 101 (`bins/skyzfsd/src/main.rs:83`).

## Counts (a helper's run, in this container)

fmt clean; clippy -D warnings clean for skyzfs-blk, skyzfs-local,
skyzfs-ublk, skyzfs-ublk-sys and skyzfsd, with and without test-hooks (the
whole workspace does not build here: no libudev for the skynbd crates).
Tests: skyzfs-blk 132, skyzfs-local 152 and 3 ignored, skyzfs-ublk 100,
skyzfsd 31, skyzfs-ublk-sys 4 and 7 ignored; none failed. Of the 38 new
registry crates one is not MIT, Apache or BSD: `bitmaps` 3.2.1, MPL-2.0+,
a direct dependency of libublk.
