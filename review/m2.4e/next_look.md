## Next look at #125, at da2b697 (M2.4e)

The head moved from 0f56669 (my review, `review.md` here) to
`da2b697`, one commit: "a slice looks for a hole run's end only up to
the next mapped block". It answers Codex's finding, which was my F2.

**Verdict: the code can merge. Nothing in it blocks. The merge waits
only on the cut rig, as the lead has ordered: #123 first, then
`zfs-topology` merged into this branch and the cut rig green on it.**
F1 was the rig's premise, not the daemon. Testing's change in #123
(`ab7fbce`) records `bound` on a zero-filled volume and adds
`boundsparse`. Run 44378510338 drove the daemon of 0f56669 with it, and
on ubuntu-26.04 and ubuntu-26.04-arm both courses pass:

- `bound`: 1 cut abandoned at the bound.
- `boundsparse`: 0 bytes copied and 0 abandoned while the pool wrote
  320 MiB into holes, with the set dropped afterwards and
  `held_content` passing (3 cuts written, 393216 blocks compared).

That is the kernel evidence for M2.4e that the old rig could not give.
Until #123 is on this branch, the cut rig queued on da2b697 runs the old
`bound` and will fail again; that run says nothing new.

F2's fix is right, and it gives the same slices as before. My G1 and G2
are still open: no test was added for them, and their five plants still
survive (below). They are gaps in the tests, not defects, and I do not
hold the merge on them. They are the cheapest things left to close.

### What changed, and nothing else

`git diff 0f56669 da2b697` touches two files only:

- `crates/skyzfs-local/src/inplace/state.rs`:
  - `hole_run_end` takes a block to stop at;
  - `slice_from` passes it the next mapped block, or the volume's end;
  - `MemRegion` counts its loads (`loads()`). `MemRegion` is the tests'
    region and the harness's; the daemon's is the mapped file, so
    counting costs nothing that ships.
- `crates/skyzfs-local/src/inplace/state/tests.rs`: one test,
  `the_slices_of_a_large_hole_read_the_hole_set_once_over`, and its
  copier `NoCopies`.

Nothing else changed: the hole set's life, the region versions, the
upload and the status are as I reviewed them at 0f56669. `cargo fmt
--all --check` is clean at da2b697. I did not run clippy myself this
time; see CI below.

### F2's fix

The new `hole_run_end(block, stop)` (`state.rs:1494-1517`):

- clamps `stop` to the volume;
- walks the chunk words only up to `last = ceil(stop / 16)`, the chunk
  that holds the block before `stop`;
- returns `min(chunk * 16, stop)`.

The slice calls it once per run, with `next_mapped.unwrap_or(blocks)`
(`state.rs:2509`); the eager `map_or` is gone.

**Same ends as before.** If the run of hole chunks ends before
`last`, the walk stops there and the end is `min(run end, stop)`, as
before. If the run goes on to `last` or past it, then `chunk` ends at or
above `last`, `chunk * 16 >= stop`, and the end is `stop`. Before, it
was `min(next_mapped, run end)`, also `stop`, since the run end is at
or past `last * 16 >= stop`. The block a run starts at always lies
below `stop`: the next mapped block is above it, and the run is only
asked for a block in a hole and not in the map. So the walk runs at
least once and never returns a run of length 0. Either way the run ends
at the first block that does not read `Zero`.

**Same slices, checked.** My new scenario R8 builds 300 random whole
cuts with `lo` 0:

- volumes of 64 to 4063 blocks, most ending inside a chunk;
- random hole ranges, up to 199 random writes before the instant;
- a random first block and a slice limit of 1 to 70 blocks.

It compares every slice of each cut, exactly (`blocks`, `zeroes`,
`next`), with a greedy reading of `read`:

- a block that does not read `Zero` is named for a read;
- a block that does begins a run to the first block that does not;
- a slice ends at its limit of blocks or at 256 runs.

| Code | Slices checked | Result |
|---|---|---|
| da2b697 | 11907 | all equal |
| 0f56669 (its `state.rs` and tests, R8 appended) | 11907 | all equal |

So the fix changes the cost and not the slices.

**R7 again** (debug build, the same scenario as in `review.md`: 2048
blocks written one a chunk at the start of a volume that was one hole,
slices of 256 blocks):

| Volume | 0f56669: all slices, slowest | da2b697: all slices, slowest |
|---|---|---|
| 16 GiB | 0.26 s, 38 ms | 0.5 ms, 0.08 ms |
| 64 GiB | 0.84 s, 143 ms | 0.8 ms, 0.33 ms |
| 256 GiB | 3.6 s, 521 ms | 2.0 to 5.9 ms, 1.5 to 4.2 ms |

A rerun of the old code beside the new gave 0.19 s and 3.0 s for 16 and
256 GiB. At da2b697 what is left grows with the volume once, not once per
gap: the last run, after the last written block, is walked to the end
of the volume, one pass over the set. The pull request's test bounds
the loads of all of a cut's slices at four passes over the open map and
the set: 69632 for 4 GiB, against 2134765 with the walk unbounded. It
fails with either of my plants Y03 and Y04 (below), which keep the
slices and restore the cost, so the cost is pinned.

### G1 and G2: still open

No test was added for either.

| Id | Change | At 0f56669 | At da2b697 |
|---|---|---|---|
| X01 | `abandon_all` clears the open map before it marks the open epoch whole | SURVIVED; R1, R2, R3, R6 fail | SURVIVED; R1, R2, R3, R6 fail |
| X02 | `abandon_all` drops the set (conservative) | SURVIVED; R1 fails | SURVIVED; R1 fails |
| X11 | the last block of each run of zeros not uploaded | SURVIVED; U1 fails | SURVIVED; U1 fails |
| X12 | only a slice's first run of zeros uploaded | SURVIVED; U1 fails | SURVIVED; U1 fails |
| X13 | a slice with no block uploads none of its runs | SURVIVED; U1 too | SURVIVED; U1 too |

- **G1, a mismatch while the set holds.** `abandon_all` with the set
  held is right at the head (R1 to R3 and R6 pass), but nothing in the
  pull request runs it. X01 is a change a later package could make for
  a reason that sounds good ("the open epoch is whole, its map is not
  needed"), and every test would still pass. R1 and R2 as tests close
  it; they are in `scenarios_state.rs`, ready to move.
- **G2, zeros over old bytes past block 0.**
  `a_whole_cut_writes_its_holes_over_what_object_storage_held` puts old
  bytes in block 0 only. U1 (`scenarios_upload.rs`) puts them in every
  block and catches X11 and X12. X13 would need a volume whose end is a
  run of holes that a slice begins in. It is the least likely of the
  three.

### What coding asked me to judge

**The hole set's life: right, and unchanged by da2b697.**

- The set is taken at a whole begin only, with the bits, the count and
  Held stored before the magic (`state.rs:1072-1085`).
- It is dropped in `release` and `settle` only, last in each, and only
  when no held cut has `lo` 0 and, with none held, `OPEN_LO` is not 0
  (`state.rs:1530-1545`).
- My argument in `review.md`, questions (2) and "H01, H03 and H04",
  holds as it was: once no `lo` is 0, none becomes 0 again, so the drop
  cannot come before the last reader.
- Y09 (a take-up that drops the set whatever holds it) is caught; X04
  (one that never drops it) was caught at 0f56669, and that code is
  unchanged.

**Region versions 2 and 3: right.**

- `Taken::open` keeps a named version 2 file at its own length
  (`state.rs:408-426`).
- `begin_with` writes version 2, with no set, into a region of that
  length.
- A restore takes the set only at version 3.
- The start's check reads 2 and 3 (`inplace.rs:2394`).

Plants against each of the four:

- Y05 (the open uses version 3's length for a version 2 file): caught.
- Y06 (a begin writes version 3 in a region of version 2's length):
  caught.
- Y08 (the start refuses version 2): caught, but only with the test
  hooks, since the test that catches it is behind that feature.
- Y07 (a version 2 state is read with a set): survives, and is
  equivalent. The set's word would lie past the end of the region, and
  a load there reads 0.

I agree with the one case `113dfb3` dropped from
`the_start_names_the_version_of_a_cut_state_only_where_it_is_another`
(a version 2 header at version 3's length). The open keeps such a file
at version 3's length and the state reads it as version 2, without a
set, which is conservative. No daemon writes such a file.

**The death tests of (b): they test what (b) says.** Plants that break
the same promises, and the tests that catch them:

| Promise | Plant | Caught by |
|---|---|---|
| The set is stored before the magic | X03 (at 0f56669, code unchanged) | `a_daemon_lost_inside_a_begin_with_holes_leaves_the_set_whole_or_no_state` |
| A take-up keeps the set | Y09 | `a_take_over_before_the_first_commit_keeps_the_hole_set`, `the_hole_set_holds_until_the_first_whole_cut_is_uploaded` and the death runs |
| A take-up drops a set no cut needs | X04 | the death runs and `the_hole_set_holds_until_the_first_whole_cut_is_uploaded` |
| A write into a hole before the instant is read | X05 | the model with deaths and the take-over test |

`a_death_between_a_holes_bit_and_its_write_reads_the_image` tests the
state: the bit is in the region before the write. That the store notes
before it writes is pinned by the store's existing mutant
`COPY_AFTER_OVERWRITE` and its tests, which this pull request does not
change. One limit, said plainly: these deaths are of the daemon over a
simulated disk and a memory region. A reboot loses the region (tmpfs),
and the next start begins and scans again
(`a_state_begun_after_a_reboot_records_the_holes_anew`). No rig kills
the daemon between begin and the first commit on a kernel. The cut
rig's kills are not aimed there, and I do not count them as showing it.

### Plants of this look

Each plant was made alone in a worktree of da2b697, the tests of its
crate were run, and the file was restored. For a plant that survives,
my scenarios of its kind were run with it.

| Id | Change | Result |
|---|---|---|
| X01 | `abandon_all` clears the open map before it marks the open epoch whole | SURVIVED the pull request's tests; R1, R2, R3, R6 fail with it (G1) |
| X02 | `abandon_all` drops the set (conservative) | SURVIVED; R1 fails with it (G1) |
| X11 | the last block of each run of zeros not uploaded | SURVIVED; U1 fails with it (G2) |
| X12 | only a slice's first run of zeros uploaded | SURVIVED; U1 fails with it (G2) |
| X13 | a slice with no block uploads none of its runs | SURVIVED; U1 too (G2) |
| Y01 | the walk stops at `stop / 16`, not at its ceiling (a run whose stop lies inside a chunk ends below its start) | KILLED: 6 tests fail, among them `a_whole_cut_slices_its_holes_as_runs_of_zeros`, `every_held_cut_reads_as_the_volume_at_its_instant` and the death runs; then a test grows without bound. I ran it by hand, its test binary under a 4 GiB memory limit, because in the first run, with no limit, a process was killed for lack of memory |
| Y02 | a run's end not cut at `stop` (it runs over the next mapped block) | KILLED: 9 tests |
| Y03 | the walk unbounded again, the run still cut at `stop` (Codex's cost, the same slices) | KILLED by `the_slices_of_a_large_hole_read_the_hole_set_once_over` alone |
| Y04 | the slice asks for the run's end to the volume's end and cuts it at the next mapped block after (the same) | KILLED by the same test alone |
| Y05 | a named region of version 2 opened at version 3's length | KILLED: `a_named_region_of_version_2_is_kept_at_its_own_length` |
| Y06 | a begin in a region of version 2's length writes version 3 | KILLED: `a_state_begun_in_a_region_of_version_2s_length_is_of_version_2` |
| Y07 | a state of version 2 read as having a set after its records | SURVIVED, and equivalent: in a region of version 2's length that word is past the end, and both regions read 0 there (`state.rs:295`, `skyzfs-mapped` `load`), so the set reads as none and nothing stores to it. A version 2 state in a longer region with words there that are not 0 is made by no daemon |
| Y08 | the start names version 2 as a version it does not read | KILLED, with the test hooks: `the_start_names_the_version_of_a_cut_state_only_where_it_is_another` (the test is `#[cfg(feature = "test-hooks")]`; without the feature the plant survives) |
| Y09 | a take-up drops the set whatever holds it (conservative) | KILLED: 6 tests, among them `a_take_over_before_the_first_commit_keeps_the_hole_set` and `the_hole_set_holds_until_the_first_whole_cut_is_uploaded` |

Of the 14, 8 are caught and 6 survive: the five of G1 and G2,
unchanged from 0f56669, and Y07, which is equivalent. Every
plant on da2b697's change (Y01 to Y04) is caught.

### CI and Codex on da2b697

- The cut rig on da2b697 (run 44382978577) failed only `bound:
  cuts_abandoned`, as expected with the old rig, on ubuntu-26.04 and
  ubuntu-26.04-arm. `held_content` passed in all 14 recordings on both.
- Green when I wrote this: `lint` and `unit-arm64` (run 44382983394),
  the control rig, `coverage`, `e2e`, `k3s-smoke`, and the replay rig's
  "write into a hole" jobs on both images (run 44382978968).
- Still running then: the daemon rig, the replay rig's jobs "through
  skyzfsd", and `ci`'s `test`. At 0f56669 all three were green, and da2b697 touches only the
  slicing and a test.
- Codex reviewed da2b697 and has no finding. Its thread on 0f56669 is
  answered and resolved.
- Testing's run 44378510338 is #123's rig on 0f56669's daemon. It
  passed on both images, `bound` and `boundsparse` included.

### My scenarios at da2b697

R1 to R8 and U1 all pass at da2b697. R8 also passes against 0f56669's
code. R8's source is below, so that it can be moved into the tests as
it stands. It goes beside R1 to R7 in `scenarios_state.rs` and needs
that file's `ReviewNoCopies`.

```rust
/// R8 (the next look, da2b697). Every slice of a whole cut with `lo` 0
/// is exactly what a greedy reading of `read` gives: from its first
/// block, a block that does not read `Zero` is named for a read, and a
/// block that does begins a run that goes on to the first block that
/// does not (or the volume's end); the slice ends at `limit` blocks or
/// [`SLICE_ZERO_RUNS`] runs. That is what `slice_from` gave before
/// da2b697 (its run ended at the first of the next mapped block and the
/// hole run's end, the first block that does not read `Zero`) and must
/// still give. Random hole sets, maps, volume lengths that end inside a
/// chunk, first blocks and slice limits; every slice of each cut.
#[test]
fn review_m24e_r8_slices_match_reads_over_random_worlds() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut slices = 0;
    for round in 0..300 {
        let blocks = 64 + next() % 4000;
        let bound = 64 * BLOCK_SIZE;
        let mut holes = Vec::new();
        let mut at = 0;
        while at < blocks {
            let len = (1 + next() % 300).min(blocks - at);
            if next() % 3 != 0 {
                holes.push((at, len));
            }
            at += len;
        }
        let region = MemRegion::new(words_for(blocks, records_for(bound)));
        let mut state =
            CutState::begin_with(region, blocks * BLOCK_SIZE, bound, OWNER, Mutants(0), None, Some(&holes));
        for _ in 0..next() % 200 {
            let first = next() % blocks;
            let count = (1 + next() % 8).min(blocks - first);
            state.change(first, count, &mut ReviewNoCopies).expect("no copy");
        }
        let number = state.freeze(&marked(1), true);
        state.flushed(number);
        let zero: Vec<bool> = (0..blocks)
            .map(|b| state.read(number, b) == Some(Read::Zero))
            .collect();
        let limit = 1 + (next() % 70) as usize;
        let mut from = if next() % 2 == 0 { 0 } else { next() % blocks };
        loop {
            let got = state.slice_from(number, from, limit).expect("held");
            let mut want = Slice::default();
            let mut block = from;
            while block < blocks && want.blocks.len() < limit && want.zeroes.len() < SLICE_ZERO_RUNS {
                if !zero[block as usize] {
                    want.blocks.push(block);
                    block += 1;
                    continue;
                }
                let mut end = block;
                while end < blocks && zero[end as usize] {
                    end += 1;
                }
                want.zeroes.push((block, end - block));
                block = end;
            }
            want.next = (block < blocks).then_some(block);
            assert_eq!(
                (&got.blocks, &got.zeroes, got.next),
                (&want.blocks, &want.zeroes, want.next),
                "round {round}, from {from}, limit {limit}"
            );
            slices += 1;
            match got.next {
                Some(n) => from = n,
                None => break,
            }
        }
    }
    eprintln!("R8: {slices} slices, each as the greedy reading gives it");
}
```
