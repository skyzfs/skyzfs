# Decision D6 (direct I/O on the image): design review

Read at `zfs-topology` 12b5493; the D6 text is 948f5d8 and the round's
table 374f71d. Comment:
https://github.com/skyzfs/skyzfs/pull/2#issuecomment-6849776748

Verdict: the decision holds; the text does not yet.

## The findings, shortest form

1. **§4.1 contradicts the new §4.2.** The failed-volume bullet still
   says "Within one boot a new instance finds everything the old one
   acknowledged in the kernel's cache". D6 rewrote §4.2 and left this.
2. **"Acknowledged when the device has taken it" holds over written
   extents only.** Over a hole or a `fallocate`'d extent the data is
   readable only once the extent tree says so, and that change is in
   the kernel's journal until a commit. Daemon loss is unaffected (the
   kernel is alive); a power cut can lose the write, which §4.1 allows
   because such a block's durable content is zeros. The claim rests on
   the image being written through — §10's open allocation row, which
   `fallocate` alone does not close.
3. **Silent fallback to buffered I/O** (ext4 `data=journal`) is not
   catchable by §5.1's refusal. The unit check itself is `statx` with
   `STATX_DIOALIGN` (6.1+).
4. **The start's sync has a new job** under D6: committing the previous
   instance's extent-tree changes and flushing the device's cache, not
   writing dirty pages. §4.1 and §5.1 still read as the latter.
5. Three claims past the table: "in four jobs" on fast-sync machines
   (three jobs, four rows), "cost the buffered store up to 17%" (that
   is the distance from the model; the cache's share is 8 to 10 points
   in one job), and "eight by default" (slow-sync evidence only; on the
   one fast-sync row eight threads was the worst of three).

## What is here

- `dio.py DIR`: direct writes over written extents, unwritten extents
  and holes, timing the `fdatasync` after each. Weak signal here (no
  journal statistics on this container's ext4, virtual disk), kept for
  the method.
- The extent split in finding 2 is `filefrag -v` around one direct
  write into a `fallocate`'d extent; the commands are in the comment.

## Traps met

- FIEMAP (and `filefrag`) reports the in-memory extent tree, so it
  shows the conversion at once and says nothing about its durability.
  The power-cut half of finding 2 cannot be shown here at all; the
  replay rig is where it would belong.
