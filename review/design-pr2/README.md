# The design pull request (skyzfs/skyzfs#2), reviewed at the project owner's request

`skyzfs-design-docs` at 887496c into `main`: `docs/skyzfs-design.md`,
`docs/skyzfs-impl-notes.md`, `docs/skynbd-postgres-study.md` §10.

**Verdict: the architecture, the contract and the cut rule hold up. I found
no counterexample to the cut rule. Ten things in the design are wrong,
unachievable as written, or missing; none of them unseats the design. Not
posted on GitHub.**

## What I did

- Read `skyzfs-design.md` whole, before the description and the threads.
- Checked its claims about ZFS against the OpenZFS 2.4.1 source (tag
  `zfs-2.4.1`), file and line below.
- Read the nine review threads and three comments on the pull request, so
  that nothing below repeats them.
- Checked the documents mechanically: footnotes, tables, fences, the
  cross-references into the notes, the headline numbers against the study.

## What I did not do

- The notes (176 KB): read the introduction, M1's status, the throughput
  rule, "Open at this writing" and the headings; not the rest.
- The study's §10: only the passages the design cites.
- The macOS and Windows ports' sources (§4.3): not read. An earlier review
  on the pull request did read them.
- Nothing was run. No kernel features here.

## The cut rule (§5.2): no counterexample

Read in 2.4.1:

- The order of a sync: flush, even labels, uberblocks, flush
  (`vdev_label.c:2095-2132`, `:1823-1834`); both flush results discarded
  (`zio.c:1731`, `ZIO_FLAG_DONT_PROPAGATE`).
- Frees of a group are released, and handed to the trim tree, in
  `vdev_sync_done`, after the uberblocks and their flush (`spa.c:10919`
  then `:10948`; `metaslab.c:4605-4618`, with the nearly-full case the
  design names).
- Blocks written in open context are waited for before the sync writes
  their parent (`dbuf.c:4807`); rewrites in place start at sync pass 2
  (`zio.c:128`).
- A failed read suspends the pool only on `ENXIO` (`zio.c:5705-5711`).
- No FUA writes in the Linux disk vdev; the whole-disk table has no ublk
  (`rdwr_efi.c:219-278`).

Reasoned, not read: pool checkpoints and their rewind, `zpool initialize`,
scrub repairs, intent-log blocks on the main device. None gives a block
that uberblock N reaches and that changes before the cut.

## Findings, most severe first

1. **M1's status says more than was run.** Design line 5 and §9: "What M1
   still lacks is its throughput criterion." M1's exit criterion includes
   the power-cut replay. The notes' own status says the replay rig was "so
   far run against the ublk project's reference server", and no workflow or
   script sets `RIG_DEVICE` or `RIG_PROVIDER` (the rig's two hooks "for
   skyzfsd"). So the replay has shown what ZFS does on a device that keeps
   the order, not that `skyzfsd` and its log do. Sure of what the documents
   and the tree say. I drop it when the rig has run with `skyzfsd` as the
   device and its log as the record, or §9 lists it as open.
2. **M2's exit criterion cannot be run, and would pass a wrong cut.**
   "Imports read-only there with a clean scrub": ZFS refuses a scrub on a
   read-only pool (`zfs_ioctl.c:7640-7643` and `:7662` with `:7419-7422`,
   `POOL_CHECK_READONLY`). And a cut published one group early imports and
   scrubs clean without the snapshot in it. Sure. Wants: a check that reads
   every block without writing (`zdb -cc`, or a scrub on a throwaway
   clone), and "snapshot `t` is there and its content equals the source's".
3. **A pool with a checkpoint cannot be cloned or browsed by §5.4.** ZFS
   refuses to change the GUID (`spa.c:1027-1031`) and to remove any vdev,
   the log device included (`vdev_removal.c:2447-2449`), while a pool
   checkpoint exists or is being discarded. §6.2 has no rule on it. Sure.
4. **"The object store, and the vendor, see ciphertext" (§6.2) is too
   strong.** ZFS encryption leaves dataset and snapshot names, the dataset
   hierarchy, properties, file sizes and holes in clear
   (`man/man8/zfs-load-key.8:231-234`). Sure. Either say so, or encrypt the
   objects in the uploader.
5. **Frees under 32 KiB never arrive as discards.** §5.3 says freed blocks
   "arrive as discards, with `autotrim=on`". Autotrim skips extents below
   `zfs_trim_extent_bytes_min`, 32 KiB (`vdev_trim.c:106`, `:1238`). With
   4 to 16 KiB blocks that is much of what a database frees. The risk row
   names only the batch of 32 groups. Sure of the code; the size of the
   effect is not measured. Bears on M4's "within 1.5 times the live bytes".
6. **Nothing says what bounds the local store.** §5.3 keeps every cut's
   blocks until the cut is uploaded or abandoned; `cache` evicts only
   uploaded blocks. On a slow or absent uplink that grows without a stated
   bound, and the design does not say what happens at the bound (writes
   held, or cuts abandoned, and by what rule). A gap, not an error.
7. **The deadlock row does not match the store as built.** §10 says "no
   allocation in the daemon's I/O path"; §5.1 says the store writes through
   the host's page cache, which allocates in the kernel on every write and
   can be throttled against dirty pages that only the daemon can clean.
   The kernel's remedy for a process in the I/O path, `PR_SET_IO_FLUSHER`,
   is not used (no match in `crates` or `bins`) and not mentioned. Not shown
   to hang; medium confidence that it matters.
8. **`zfs_nocacheflush=1` takes the flushes away.** The Linux disk vdev
   then completes a flush without sending it (`vdev_disk.c:1154-1156`). The
   daemon gets no flush: no cut, and nothing it wrote is ever asked to be
   durable. A module parameter some administrators set. Sure. Wants a rule
   in §6.2 and a check by the agent.
9. **Which `libzpool` does the offline session use on Linux?** §5.4 rests
   on it. The design's own footnote has Ubuntu 24.04 with 2.2.2 commands
   beside a 2.4.1 module. Not said: the distribution's library or one the
   project ships, and the licensing row covers the frontend, not an agent
   that links CDDL code. A gap.
10. **A clone keeps the original's vdev GUIDs.** Only the root's GUID
    changes (`spa.c:1056-1057`). I found nothing in the kernel that minds,
    and the event daemon matches the pool GUID when the event carries one
    (`zfs_mod.c:728-730`). Low. Worth one line in §5.4 and the event daemon
    running in M3's same-host test.

## Defects of the documents

- Design line 452: "**Where M1 stands**" follows the roadmap table with no
  blank line, so GitHub renders it as a row of the table.
- Gate G1 says "no on the faster runners"; §9 says fast-sync machines were
  at 0.92 to 0.98 and the slow-sync ones miss. Read together they
  contradict. G1's answer is from the daemon as first built.
- M1's deliverables list "write log and cache"; §5.1 says the block cache
  is not built in M1.
- §4.1: line 139 repeats what line 137 says about reads; the parameter
  table has two rows for discard.
- §6.1: "took up to 2.4 times as long". The study's 2.4 is against
  twovol's commit; against a log written through, its table gives about
  1.4 to 1.9.
- §4.2 "Kernels": flags and versions "must be pinned during M1". I found
  no record of it in the notes.
- Footnote `cutproof`: "the port branches are still to be checked". The
  pull request's own comment of 2026-11-11 says they were read.
- The pull request's description is stale: "two files" (three), "`-f -m`"
  (the design: `-f`, no `-m`), "what they changed is in §10.2" (now in the
  notes), the kernel evidence as "not our daemon" (the daemon has run).
  The design's date is still 2026-11-10.

## Suggestions

- M2's "no more than run-to-run noise" needs a rule fixed before the data,
  as M1's got; the notes show model rows 27% apart in one job.
- After `zpool create`, have the agent read the first label at byte 0 and
  refuse otherwise, so a later libzfs that learns ublk cannot move it.
