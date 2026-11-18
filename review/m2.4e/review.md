## Review of #125 at 0f56669 (M2.4e)

The image's holes at the state's beginning: a whole begin records which
64 KiB chunks of the home image were holes (cut state version 3), a cut
with `lo` 0 that holds the whole volume reads a block of such a chunk
that is not in its map as zeros and makes no copy for a write into it,
the uploader writes those blocks as runs of zero extents, and the set is
dropped once no cut can read by it. Six commits from `25cbf07` to
`113dfb3` on `4dafc03`, and `0f56669`, a merge of `zfs-topology`. The
head did not move while I reviewed it.

**Verdict: not yet. One finding blocks the merge, and it is not in the
product code: the cut rig is red on the head (F1).** The rig's
recording `bound` relied on every first write under the held first cut
being copied, which this pull request removes on purpose; the rig now
fails its check that a cut is abandoned at the bound, on amd64 and on
arm64, and the bound has lost its only kernel coverage. The fix belongs
in the rig, not in the store, and which fix, and whether #125 waits for
it, is the lead's decision.

The product code I could not break. The rule `hole_of` is sound: "not in
the map of a whole cut with `lo` 0" does mean "nothing wrote or zeroed
it since the state began", every path that changes a block notes it
first, and the drop cannot come before the last cut that reads by the
set has gone. The author's argument that H01, H03 and H04 are each
equivalent alone is right; I could not build a scenario that separates
any of them. Of my 13 plants, 8 are caught by the pull request's own
tests and 5 survive: two (X01, X02) show that a mismatch while the set
holds has no test (G1), three (X11 to X13) that the zeros over old bytes
are tested in one block only (G2); my scenarios catch all but X13.
Codex's one finding (P2, the cost of the run's end in `slice_from`) is
right; I measured it (F2). It does not change what is read, so it does
not block by itself.

### What I did

- Read the diff against `origin/zfs-topology` and the code around it,
  with design §5.2 and the notes on M2.2–M2.4, before the description;
  then the description, against the code.
- Gates at the head with rustc 1.100.0: `cargo fmt --all --check` clean;
  `cargo clippy --workspace --all-targets -- -D warnings` clean plain,
  with `skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `skyzfs-ublk-sys/control-test-program`. Tests of the crates the diff
  touches, 0 failed: `skyzfs-local` 293 in the library (1 ignored) and
  15 on real files; `skyzfs-ublk` 154 (+2); `skyzfs-upload` 26 in the
  library and 62 in its test files (`holes.rs` 4); `skyzfsd` 102 (+3).
  With the test hooks: `skyzfs-local` 293 and 15, `skyzfsd` 138 (+3).
- 13 textual plants, one at a time, each run against the tests of its
  crate with a limit of 1500 s, the file restored after each
  (`plants.py`, `plants.out`). For a plant that survives, my scenarios of
  the same kind are run with it in place.
- 8 scenarios of my own (`scenarios_state.rs`, 7, appended to the
  state's test module for a run; `scenarios_upload.rs`, 1, appended to
  the uploader's `tests/holes.rs`). All pass at the head
  (`scenarios.out`). `scenarios_with_x01.out` is the full output with
  X01 in place (`plants.py ... scenarios-with X01`, which writes
  `scenarios_with.out`).
- CI and Codex on the head, read once each. No kernel here.

The worktree of the head was clean after every run.

### Findings, most severe first

**F1. Blocks the merge. The cut rig fails on the head: no cut is
abandoned at the bound, because the first cut of a sparse image no
longer copies.** Certainty: high.

Run 44371078092, `record through skyzfsd and check`, jobs
`ubuntu-26.04` (135037915289) and `ubuntu-26.04-arm` (135037915369),
the same line in both:

    bound: cuts_abandoned: FAIL: '0' abandoned at the bound, with the
    copies '0' bytes of '268435456'; the holder kept the first cut while
    the pool wrote 320 MiB of new data, so at least one must be

`tests/kernel/cut-record.sh:765-781` (`rec_bound`) pauses the holder,
creates the pool on the new state and writes ten files of 32 MiB, on
the premise in its comment: "Every block first written after the cut is
copied, the copies pass the bound ... and the cut is abandoned". Before
M2.4e the first cut of a state begun with nothing known held the whole
volume and every first write under it was copied. Now the image is
sparse at the begin, the first cut has `lo` 0 and reads the image's
holes as zeros, and a write into a hole makes no copy
(`state.rs:1513-1523`, `reads_content`). ZFS writes the new data into
free space, which on a new image is hole, so nothing is copied: 0 bytes
of a 256 MiB bound. The store does what M2.4e means it to do; that is
also the first kernel evidence for it (320 MiB written under a held
whole cut, no copy, on both architectures). The variant
`ubuntu-26.04-upload` passed, and in every recording of both failing
jobs `held_content` passed.

What is lost: the bound (D7: copies past the bound abandon the cut and
its map goes to the cut that waits) is now driven on a kernel by
nothing. The store's own tests still reach it (on images with no holes
and by writes over data), so this is a gap in kernel coverage, not a
known defect; but the rig is red and the rule is not to weaken the
check.

What I would do (the rig is testing's, and the choice is the lead's):
record `bound` on an image written through at create (`create
--zero-fill`, which the description and
`a_volume_made_zero_filled_has_no_holes_and_copies_as_before` name as
the case that copies as before; `hole-rig.sh` already knows how to make
one), so the check stays as it is; and keep a sparse recording whose
check is the opposite (a held whole cut over new data copies nothing),
which is what M2.4e is for. The description does not mention the cut
rig; its "not done" list should.

**F2. Should be fixed, does not block by itself. Each run of zeros in a
slice scans the hole set to the end of its run of holes, twice: on a new
image, to the end of the volume.** Certainty: high (Codex's P2 on the
same lines, checked and measured).

`state.rs:2493-2495`:

    let end = next_mapped.map_or(self.hole_run_end(block), |next| {
        next.min(self.hole_run_end(block))
    });

`map_or` evaluates its first argument whatever the option holds, so
`hole_run_end` runs twice when a mapped block follows; and
`hole_run_end` (`state.rs:1481-1500`) walks the set's words from the
block to the end of its run of hole chunks, where the run is then cut at
`next_mapped`. A new image is one run of holes, so every gap between two
written blocks costs a walk to the volume's end. The slice is read on
the queue's thread (`read_cut_from`, through the desk), so foreground
requests wait for it.

Scenario R7: a volume that was one hole at the begin, 2048 blocks
written at its start (one a chunk), the first cut read in slices of 256
blocks. Debug build, `MemRegion`:

| Volume | Slices | All of them | The slowest |
|---|---|---|---|
| 16 GiB | 9 | 0.26 s | 38 ms |
| 64 GiB | 9 | 0.84 s | 143 ms |
| 256 GiB | 9 | 3.6 s | 521 ms |

Same writes, time in proportion to the volume. A release build is
faster by a constant I did not measure; the proportion stays, and a
slice can name up to 256 runs. The fix is small: bound the walk by
`next_mapped` (pass it to `hole_run_end` as a limit) and use
`map_or_else`. No test of the pull request would need to change; X06's
nine failures show the slicing is well pinned for a fix to be checked
against.

**F3. Low. The guide and the notes are behind the format (the lead's
files).** `CLAUDE.md:378` at the head says "Status is version 16"; it is
17. The notes (`docs/skyzfs-impl-notes.md:1119`) give 146 MiB for a
TiB's state; it is 148 (`the_state_of_a_tebibyte_takes_148_mib`; the
description says so). Neither says that the cut state is version 3 and
reads 2. The status module's table of file lengths lists 1008, 1016 and
1024 before 968 (`status.rs`), which only reads oddly.

### The five questions

**(1) Is "not in a lo-0 map" "nothing wrote it since the state began"?
Yes.** A whole cut with `lo` 0 holds in its map (first part and, after
a gift, the donor's map as `also`) every block noted from the state's
begin to its instant, and the set's chunk bits come from the image at
that begin. I looked for each way round it:

- *Write, write-zeroes:* `inplace.rs:1633` and `1667` note the change
  before the block is touched; the only other calls are the mutant
  `COPY_AFTER_OVERWRITE`. `zero()` (1751) is reached only from
  `write_zeroes`. No other write to the image in the store: the start's
  `check_direct_io` and sync come before the scan (`inplace.rs:1202`,
  `1217`, take-up at 1219), and nothing writes between the scan and
  serving.
- *Discard, trim:* changes no content in the store (`inplace.rs:1615`),
  so a discarded hole block still holds zeros, and a discarded data
  block is not in a hole chunk.
- *Rewind, `import -T`:* ZFS writes through the device; noted like any
  write.
- *A state that changes:* every new state is a new begin with a new
  scan; a state taken up keeps its set (`settle` drops it only when no
  cut can read by it). A reboot loses the region (tmpfs) and begins
  again.
- *Cuts with `lo` above 0:* never read holes (`hole_of` asks `lo == 0`);
  they read outside their map from the base, as before.
- *Partial maps:* a cut that took a donor's map reads it through
  `in_parts` (first and `also`); R4 checks a hole written between the
  two instants after `abandon_uploading`, with slices.
- *Region boundaries:* `image_holes` rounds inward to blocks
  (`inplace.rs:2484`), `hole_bits` sets a chunk only when a range
  holds all of it inside the volume, and a run stops at the volume's end
  (X07 caught, 5 tests). A failed or odd answer from `SEEK_DATA` gives
  no set (X08 caught).
- *The intent map's regions:* a begin from a trusted map is not whole
  and records no set (choice 2, `a_state_begun_from_an_intent_map_has_no_hole_set`).
- *A mismatch while the set holds* (`abandon_all`): every held map goes
  to the open map with `lo` 0 and the open epoch is marked whole, so the
  next cut reads by the set and by a map that still holds every write
  since the begin. Right at the head; R1, R2, R3 (a death at every
  store) and R6 check it, and no test of the pull request does (G1).

One thing is new in kind, and worth a sentence in the notes rather than
a change: before M2.4e a whole cut read every block, so it did not
depend on the map being complete. A whole cut with `lo` 0 now does, on
the same premise a delta cut always has (nothing writes the image
outside the daemon while a state is kept).

**(2) Can `drop_unneeded_holes` come too early? No.** It is called in
`release` (`state.rs:1833`) and `settle` (2027) only, last in each, and
stores Dropped only when no held cut has `lo` 0 and, with none held,
`OPEN_LO` is not 0. A drop while a whole `lo`-0 cut is held would not be
conservative (writes into its holes after its instant made no copy, and
it would read them from the image), so I checked every way to a `lo` of
0: `give` stores the heir's lower `lo` with the donor's map in one store
before the donor goes (`state.rs:1651-1683`); the open map takes the
donor's `lo` before `OPEN_ALSO`; `release` stores `OPEN_LO` = the
released number before the cut goes; `freeze` takes `OPEN_LO`. Once no
`lo` is 0, none becomes 0 again. A death between the cut going and the
drop leaves the set held and unread until `settle` drops it (X04 caught,
2 tests).

**(3) Begin and take-over. Right.** `begin_with` zeroes the region,
stores the bits, the count and Held, and the magic last
(`state.rs:1072-1085`); a death before the magic leaves no state, and the
next start begins and scans again (X03 caught by
`a_daemon_lost_inside_a_begin_with_holes_leaves_the_set_whole_or_no_state`).
A restart in the same boot takes the region up with its set. A named
region of version 2 at its own length is opened at that length and read
as version 2 with no set (`state.rs:408-426`,
`a_region_of_version_2_is_read_as_it_was_written` on a fixture); a new
begin in it stays version 2, with no holes, until the region file is
made again: conservative.

**(4) Zeros over stale bytes in the upload. Right, thinly tested.**
`write_slice` writes the runs after the blocks as zero extents
(`driver.rs:611-613`); the two never share a block. A partial upload is
never committed: `lost()` discards the remote's writes and the next
attempt begins at block 0 (`driver.rs:577`, and `from: 0` at 380), so
a run half written is written again whole. A whole cut goes up by `Rule::Whole` onto whatever
head holds, and the judge compares the volume at the instant, so stale
bytes under a hole would show as a content mismatch. The test of it
(`a_whole_cut_writes_its_holes_over_what_object_storage_held`) puts old
bytes in block 0 only, so the last block of a run (X11) and every run
after a slice's first (X12) can be left unwritten without a test
failing (G2). My U1 puts old bytes in every block and catches both. X13
(a slice with runs and no block writes no run) survives U1 as well: a
slice has no block only when its runs reach the volume's end.

**(5) Cut state v3, status v17, the old versions read. Right.** The
state reads 2 and 3 (`OLDEST_READ` 2, the start's check at
`inplace.rs:2394`), with a fixture of a version-2 region. The status
writes `SKYZST17` (1032 bytes) and reads 16 without the holes
(`a_file_of_the_sixteenth_version_is_read_without_the_holes`) and the
older ones as before. `skz_time.py` reads only `time_*` keys and is not
touched by the new ones. The cut rig's judge reads the report's keys,
and its checks other than `bound` passed on the head.

### H01, H03 and H04

The author says each is equivalent alone. I agree. The invariant: while
the set is Held, some held cut has `lo` 0, or none is held and
`OPEN_LO` is 0 (the drop keeps it; nothing else stores Dropped). WHOLE is
stored only at the begin (1) and by `abandon_all` with no cut held (the
open epoch), so:

- H03 (no `is_whole`): a cut with `lo` 0 has WHOLE in `(0, number]`:
  WHOLE is 1 or the epoch of an `abandon_all` that let every cut then
  held go, so every cut with `lo` 0 is whole. Equivalent.
- H01 (no `lo == 0`): a whole cut with `lo` above 0 needs WHOLE above
  `lo`, which only `abandon_all` gives; after it the next cut has `lo`
  0 again whenever the set is still held. A whole cut with `lo` above 0
  exists only after the drop, and then `in_hole` is false. Equivalent.
- H04 (a dropped set read): after the drop no cut has `lo` 0 and none
  gets one, so `hole_of` is false by its `lo` clause. Equivalent.

Together (H30) they are caught by
`a_cut_made_whole_after_the_drop_reads_the_holes_written_before_it`.

### Plants

| Id | Change | Result |
|---|---|---|
| X01 | `abandon_all` clears the open map before it marks the open epoch whole (harmless before M2.4e) | SURVIVED the pull request's tests; my R1, R2, R3, R6 fail with it |
| X02 | `abandon_all` drops the set (conservative) | SURVIVED; my R1 fails with it (a write into a hole is copied) |
| X03 | the set's word Held stored after the magic | KILLED (1 test) |
| X04 | `settle` does not drop a set no cut needs | KILLED (2) |
| X05 | `reads_content` forgets the map of a whole cut | KILLED (3) |
| X06 | a slice reuses a next block it passed | KILLED (9) |
| X07 | a run of holes goes past the volume's end | KILLED (5) |
| X08 | a failed `SEEK_DATA` takes the rest for a hole | KILLED (2) |
| X09 | the hole after the last data not taken | KILLED (4) |
| X10 | `SEEK_HOLE` asked from the offset, not from the data | KILLED (1) |
| X11 | the last block of each run of zeros not written | SURVIVED; my U1 fails with it |
| X12 | only a slice's first run of zeros written | SURVIVED; my U1 fails with it |
| X13 | a slice with no block writes none of its runs | SURVIVED; U1 too |

`plants.out` has each line with the tests that failed.

### Gaps (tests that would catch a later defect)

- **G1.** No test runs `abandon_all` while the set holds. X01 is the
  kind of change a later package could make for good reasons (the open
  epoch is whole, so its map "is not needed") and nothing would fail.
  R1 or R2 as a test closes it; a step `Mismatch` in the state's model
  test would close it more widely.
- **G2.** The zeros over old bytes are tested in block 0 only. U1 (old
  bytes in every block, writes spread so slices have several runs)
  catches X11 and X12. X13 needs a volume whose end is a hole that a
  slice starts in.

### Suggestions

- F2's fix in the same round as F1's.
- A line in the notes for (1)'s last point: a whole cut with `lo` 0 now
  rests on the map being complete since the begin.
- A region of version 2 stays version 2, with no holes, until it is made
  again (a reboot, the region being on tmpfs). Worth a line where the
  notes describe the format.

### CI and Codex on 0f56669

- Red: the cut rig, two of three jobs (F1).
- Green: `ci` (`test`, `lint`, `unit-arm64`), `skyzfs-ci`, `coverage`,
  `e2e`, `k3s-smoke`, the control rig, the daemon rig on all three images
  (run 44371078224; its five `ctl_*` FAIL lines are the negative
  controls, every other line PASS), and the replay rig's jobs through
  `skyzfsd` and "write into a hole" on both images (run 44371078459; the
  reference jobs skipped).
- Codex: one finding, P2, on `state.rs:2495` (F2 here), not yet answered
  on the pull request.
