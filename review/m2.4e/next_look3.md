# Next look 3 at #125, at d71869a and its merge c9b2b14 (M2.4e)

Since my last look (`next_look2.md`, at `af7b524`) the head moved three
times:

- `804f65d`: a run of zeros looks for its end a stretch of 4096 blocks
  at a time (F4, Codex's second P2);
- `d71869a`: a direct test of `Maps::first_in` over several summary
  words;
- `c9b2b14`: a merge of `zfs-topology` at `65dcfd6`, which carries
  #122, #123 and #124.

`state.rs`, `maps.rs` and `state/tests.rs` are the same at `c9b2b14`
as at `d71869a`. The base changed neither, so the line numbers below
hold at both.

**Verdict: F4 is fixed, the merge is right, and nothing I found blocks
the merge.**

- **The fix.** It gives exactly the slices the code at `af7b524` gave,
  checked on 86,532 random slices and 41,440 slices at the edges of
  stretches. It makes the cost linear: 37.6 million loads at 64 GiB
  become 4.1 million, and 553 million at 256 GiB become 16.5 million.
- **The test's bounds** fail the old code by a wide margin, and they fail
  any regression that brings back a quadratic slicing cost across
  slices.
- **What is not pinned** is the cost of a single run over many stretches
  (G3 below). Two one-word slips make it quadratic, and no test of the
  pull request catches them. That is a gap in the tests, not a defect.
  It is the cheapest thing left to close.
- **The merge, built and tested here.** My own resolution of the same
  three conflicts gave the same tree, byte for byte.
- **CI at `c9b2b14`.** The cut rig is green on amd64, arm64 and the
  upload job. `bound` abandons a cut at the bound on a zero-filled
  volume, and `boundsparse` abandons none while the pool writes 320 MiB
  into holes. So F1 is closed. Every other pull-request workflow is
  green, except the daemon rig, which was still running when I wrote
  this.

## Findings, most severe first

No defect found. Two gaps in the tests and one remark, none of which
blocks the merge.

### G3. The cost of one run over many stretches is not pinned

**Does not block. A test gap. Certainty: high** (planted and measured).

`zero_run_end` (`state.rs:1534-1550`) passes `from` (the stretch) to
both looks:

- `first_in(.., from, stretch)` at `:1542`;
- `hole_run_end(from, stop)` at `:1544`.

The run's first block, `block`, is also in scope, and passing it instead
gives the same ends. So two slips of one word each restore a quadratic
cost:

- **Z06**, `hole_run_end(block, stop)`: each stretch walks the hole set
  from the run's start.
- **Z07**, `first_in(.., block, stretch)`: each stretch reads the
  summary from the run's start.

Both survive every test of `skyzfs-local` and `skyzfs-upload`. The
pull request's cost tests have no run that crosses a stretch:

- the sparse test's runs are 240 blocks long, and end at a MiB edge;
- the large hole's runs are 511 blocks long.

The failing case is the commonest volume: a new, empty one, which is one
hole, sliced as its first cut. R13 (`scenarios_f4_cost.rs`) slices that
volume. Each row below is one `slice_from` call, with one run:

| Volume | d71869a | with Z07 | with Z06 |
|---|---|---|---|
| 4 GiB | 1,822 loads | 34,462 | 132,382 |
| 64 GiB | 28,702, 0.45 ms | 8.4 M, 0.11 s | 33.6 M, 0.44 s |
| 256 GiB | 114,718, 1.8 ms | 134 M, 1.8 s | 537 M, 7.1 s |
| 1 TiB | 458,782, 7.4 ms | about 2.1e9, about 30 s (extrapolated) | about 8.6e9, about 2 min (extrapolated) |

The plants run on the queue thread, in one call. The times are with the
tests' `MemRegion`, which takes a mutex per load.

R13 with its bound on (`REVIEW_BOUND=1`) catches both plants:

- the bound is four passes over the summary and the hole set, plus 16
  loads per run and per block read;
- it runs at 4, 16 and 64 GiB, in under a second at the head.

Either of these would close the gap:

- R13 as a test;
- the case `written = None` of
  `a_run_of_zeros_goes_on_over_the_looks_to_the_next_block_of_the_map`
  with a bound on its loads, at a volume of a few hundred stretches.

### G4. No random test of the slices crosses a stretch's edge

**Does not block. A test gap. Certainty: high.**

These tests check slices against `read`:

- the model's `World::upload`;
- the death runs;
- my R8 from the last look.

All of them use volumes under one stretch. `World` has 200 blocks
(`state/tests.rs:20`) and R8 at most 4063. Above a stretch, the slices
are pinned only by:

- `a_run_of_zeros_goes_on_over_the_looks_to_the_next_block_of_the_map`:
  five fixed writes, one slice from block 7;
- the counts of the two cost tests.

The plants on the edge logic are caught all the same:

- F4e and Z05 by the fixed test;
- Z08 by a hang;
- Z03, Z09 and Z10 by the model, through `first_in`'s masks inside a
  stretch.

What the pull request lacks is a test that would catch a slip at an edge
the fixed test does not visit. R10 and R11 (`scenarios_f4.rs`) cover
that; they are described under the third question below and run in
about 20 s. They are ready to move into the tests.

### Remark: one call's latency still grows with one run's length

**Not a finding against the change.**

A run is one `slice_from` call, however long it is. The empty 1 TiB
volume above is one call of 7.4 ms on the test region, against about
4.3 ms at `af7b524`. That is 1.07 ms at 256 GiB, extrapolated; the 1.4×
is the look's fixed cost per stretch. The mapped file is cheaper per
load than `MemRegion`.

This is the latency of one call once per upload, not a cost per slice.
A cap on a run's length per slice would bound it, if the lead ever wants
a per-call bound on the queue thread. I do not ask for it here.

## (1) Is F4 fixed? Yes

**The code.** `zero_run_end` loops over stretches aligned to 4096 blocks
(`RUN_LOOK`, `state.rs:871`), which is one summary word and four hole
words. In each stretch `[from, S)` it does three things:

- **The map.** `first_in(from, S)` (`maps.rs:324-362`) reads the one
  summary word for the stretch, masked to the range. It then reads only
  the bit words it names, up to the first block.
- **The hole set.** `hole_run_end(from, min(that block, S))` walks at
  most four hole words.
- **The test.** It returns when the end falls below `S` or `S` is the
  volume's end; otherwise `from = S`.

So each stretch costs O(1). A run costs O(1 + its length / 4096), and
the slices of a cut cost one pass over the summary and the hole set, plus
a constant per run and per block read. Before, each slice read the
summary from its first run to the volume's end.

**The same ends, argued.** Let `H` be the end of the hole run that holds
`block`, and `M` the next block of the map, both at or after `block`.
The old end was `min(H, M)`.

- In a stretch `[from, S)` with `H, M >= from`:
  - `stop = min(M, S)`;
  - `hole_run_end(from, stop) = min(H, stop)`.
- If that falls below `S`, it equals `min(H, M)`.
- If it equals `S`, then both `H` and `M` are at least `S`, and the
  next stretch starts with the same invariant.
- In a stretch that starts on a block of the map (`M = from`):
  - `first_in` gives `from`;
  - `hole_run_end(from, from)` gives `from`, because `last = from / 16`
    (`from` is aligned) and the walk does not run.
- In a stretch that starts on a chunk that is no hole,
  `hole_run_end` gives `from`.
- At the volume's last stretch it returns `min(H, M, blocks)`.

The loop ends: `from` strictly grows, and `S = blocks` returns.

**Measured** (`cost3.out`). This is R9: Codex's scenario, a 64 KiB chunk
of data in every MiB, nothing written, slices of 64. Loads are exact;
times are indicative.

| Volume | af7b524 loads | af7b524, all slices | d71869a loads | d71869a, all slices |
|---|---|---|---|---|
| 4 GiB | 384,793 | 5.7 ms | 257,559 | 4.4 ms |
| 16 GiB | 3,111,961 | 43 ms | 1,030,167 | 15 ms |
| 64 GiB | 37,613,593 | 0.50 s | 4,120,599 | 63 ms |
| 256 GiB | 553,107,481 | 7.4 s | 16,482,327 | 0.25 s |
| 1 TiB | about 9e9, extrapolated | about 2 min | 65,929,239 | 0.97 s |

- **The fix grows by 4.00× for each fourfold volume.** The old code grew
  8.1×, then 12.1×, then 14.7×.
- **Where the loads go.** The fix's loads are almost all the blocks
  read: about 3.9 loads each, a sixteenth of the volume here.
- **The author's figures.** The pull request's 64 GiB figures, 37,613,593
  and 4,120,599, are what I measured.

**The same results, by a differential.** `review_old_slice_from`
(`scenarios_f4.rs`) is `slice_from` as it was at `af7b524`, word for
word, run on the same state: `blocks_from` to the volume's end once a
slice, and `hole_run_end` at the next mapped block. Three scenarios
compare against it.

- **R10**, 400 random worlds:
  - volumes of 1 block to 6 stretches + 200, a quarter of them whole
    stretches;
  - hole runs up to 3 stretches long, a fifth of the volumes one hole;
  - writes before the instant: none, a few, many, or next to the edges;
  - writes into holes after the instant, into the open map, which a
    cut's look must not see;
  - every slice from a random block, with limits 1 to 70, 64 and
    unbounded.

  86,532 slices are equal three ways: new, old, and a greedy reading of
  `read`. 490 runs of zeros cross a stretch's edge.
- **R11**, the edges enumerated:
  - volumes of `RUN_LOOK - 1`, `RUN_LOOK`, `+1`, `+16`, `2 RUN_LOOK`,
    `+5`, `3 RUN_LOOK - 3` and `3 RUN_LOOK`;
  - four hole layouts: all hole; a hole up to the first edge; data in
    the chunk after each edge; data before each hole word but not at
    an edge;
  - a write at each block within 17 of an edge, two writes across an
    edge, or none;
  - slices from each of those blocks, 0 and the end, with limits 1, 2,
    64 and unbounded.

  41,440 cases, all equal three ways.
- **R12**, `first_in` against the set of blocks put in the maps:
  - 3000 random maps, asked one to three at a time;
  - ranges that are empty, inverted, past the end, over one word or many
    summary words, or one aligned stretch;
  - summaries that name empty words, as a death between a summary bit
    and its word leaves them.

  120,000 looks, all equal.

All three pass at `d71869a`. A last check on the reference: the old
algorithm uses the head's `hole_run_end`, which `804f65d` changed only
in its comment.

## (2) Is the bound of eight tight enough? For what it is meant to catch, yes

`the_slices_of_a_sparse_volume_read_the_map_and_hole_set_once_over`
(`state/tests.rs:2834`) has two bounds, both on exact counts.

**`loads <= 8 * once` (`:2838`).**

- `once` (`:2979`) is the blocks read, plus the runs, plus the summary
  and the hole words: 1,134,592 at 64 GiB, so the bound is 9,076,736.
- The fix makes 3.63 `once`; the old code makes 33.2.

Almost all of `once` is the blocks read (1,048,576). The summary and the
hole set are 20,480 of it. So this bound says "a few loads per block,
and nothing quadratic". It does not say "the summary once over"
literally: it leaves room for about 240 more passes over the summary and
the hole set.

**`loads <= 5 * quarter` (`:2843`).**

- The fix's ratio is 4.000; the old code's is 12.1.
- This bound is the sharper one. Add a cost `Q` at 64 GiB that grows as
  the square of the volume, so `Q / 16` at 16 GiB. Both bounds still
  hold only while `Q` is at most 1.5 million.
- That is about 91 loads per slice: a per-slice look over 2.2% of the
  summary, or about 4% of the old code's quadratic term.

Anything per slice, or per run, in proportion to the volume is caught:

| Plant | Caught by |
|---|---|
| F4a, F4b (the author's) | the sparse test, both bounds |
| Z02 (the summary read from word 0 at every look) | the sparse test and the large-hole test |

**What it does not catch:**

- **A constant factor.** Z01, a stretch of 64 summary words instead of
  one, makes 1.5× the loads, 6.18 million at 64 GiB, and passes. That is
  not a regression worth a test.
- **A cost quadratic in one run's stretches.** The sparse world has no
  run that crosses a stretch (G3).

## (3) The stretch logic at its edges: right

Beyond the argument under (1), these edge cases are in R11 with every
`from` and limit, and pass:

- **A run that ends exactly at a stretch's edge.**
  - Because a chunk of data begins there: the first stretch returns
    `S`, then the next returns `from = S` at once.
  - Because a block of the map is at `S`: `first_in(S, ..) = S`, and
    `hole_run_end(S, S) = S`.
- **A block of the map at `S - 1`.** It is found in the first stretch,
  and the run ends there.
- **Two blocks of the map across an edge** (`S - 1` and `S`).
- **A hole run across the hole set's word edges** (chunk 63/64, every
  1024 blocks), inside a stretch and at one.
- **A volume ending inside a chunk, inside a stretch, or exactly at
  one.** `stretch = min(.., blocks)`, and `stretch >= blocks` returns
  `min(H, M, blocks)`. Z08 (`>` for `>=`) hangs: the last stretch
  returns `S = blocks` and then loops on `from = blocks`.
- **A volume of less than one stretch**: one look.
- **The stretch repeated over a whole hole of six stretches**: R10's
  whole-hole worlds and the fixed test.
- **Writes after the instant.** They do not stop a run, because the open
  map is not among the cut's maps.

`first_in` keeps its contract outside the state's use as well. R12
covers ranges over several summary words and summaries that name empty
words. Z04 (a block past `until` given) is equivalent at the state's
level, since the loop looks again from `S`. Only `d71869a`'s direct test
catches it, which is what that test is for.

## (4) Plants

Each plant was made alone in a worktree of `d71869a` by `plants3.py`.
The tests the author ran were run with a limit of 600 s:
`cargo test -p skyzfs-local -p skyzfs-upload --lib --tests`. The file
was restored in a `finally`. For a plant that survived, my R10 to R13
were run with it, R13 with its bound. The worktree was clean after the
run. The results are in `plants3.out`.

**A sample of the author's, made again from their descriptions**: all
five are caught by the tests the author names.

| Id | The change | Result |
|---|---|---|
| F4b | the map's look not bounded by the stretch | KILLED: the sparse test |
| F4c | the hole look not bounded where the map has no block in the stretch | KILLED: the large-hole test |
| F4d | `first_in` takes the blocks of `from`'s word before `from` | KILLED: 23 tests, the death runs among them. The author counts 22; my version of the plant may differ by a mask |
| F4e | a run cut where a stretch ends | KILLED: `a_run_of_zeros_goes_on_...` |
| F4g | `first_in` stops at the first summary word | KILLED: `the_first_block_in_a_range_...` |

**Mine:**

| Id | The change | Result |
|---|---|---|
| Z01 | `RUN_LOOK` 64 summary words | SURVIVED. The results are the same and the cost is 1.5× and linear (R9). Equivalent for the tests' purpose. R10 and R11 size their volumes from `RUN_LOOK`, so they grew 64-fold and were cut at the limit. That says nothing about the plant |
| Z02 | the summary read from its first word at each look (cost only) | KILLED: both cost tests |
| Z03 | the summary's last word in the range dropped | KILLED: 15 tests |
| Z04 | `first_in` gives a block past `until` | KILLED: `d71869a`'s test alone; equivalent at the state's level |
| Z05 | `end < stop` for `end < stretch` | KILLED: 2 tests |
| Z06 | `hole_run_end(block, stop)` | **SURVIVED; R13 catches it (G3)** |
| Z07 | `first_in(.., block, stretch)` | **SURVIVED; R13 catches it (G3)** |
| Z08 | `stretch > blocks` for `>=` | KILLED: a hang, at the limit |
| Z09 | `from`'s mask on every word of `from`'s summary word | KILLED: 15 tests |
| Z10 | the summary's first word not masked below `from`'s word | KILLED: 21 tests |

## The merge c9b2b14

**The resolution.** It keeps both sides in each of the three conflicts:

- **`serve.rs`**: the imports `CutHoles` and `IntentFigures` together.
- **`status.rs`**: the base's rows 968 (moved up by #124), 976
  (#124's wording) and 984, then this pull request's 1008, 1016 and
  1024, in order of offset.
- **`driver.rs`**: #122's `head_of`, then the four-argument
  `write_slice`.

Before the author's merge came, I had resolved the same conflicts on my
own in a scratch worktree: `d71869a` merged with `65dcfd6`. `git diff`
between my tree and `c9b2b14` is empty.

**The base's changes meet this pull request in these places:**

- **`write_slice`**: one caller, `driver.rs:633`, passes `slice.zeroes`.
  #122's publishing reads no blocks.
- **`inplace.rs`**: #124 changed only the intent header's sync, and the
  merge was automatic.
- **The status rows**: the base keeps the length at 1008, so the new
  rows at 1008 to 1032 collide with nothing.

**Gates on `c9b2b14`**, at rustc 1.100.0 (2026-11-09), the current
stable:

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo clippy -p skyzfs-local -p skyzfs-upload -p skyzfs-ublk -p skyzfsd --all-targets --features skyzfsd/test-hooks,skyzfs-local/test-hooks -- -D warnings`:
  clean.
- `cargo test -p skyzfs-local -p skyzfs-upload -p skyzfs-ublk -p skyzfsd`,
  0 failed:
  - `skyzfs-local`: 307 in the library (1 ignored) and 16 on real files;
  - `skyzfs-ublk`: 155, and 2 in its test files;
  - `skyzfs-upload`: 43 in the library, and 87 in its test files
    (`holes.rs` 5, `publish.rs` 17, `uploader.rs` 31);
  - `skyzfsd`: 103, and 3 in its test file.
- The same with the test hooks for `skyzfs-local` and `skyzfsd`, 0
  failed: 307 and 16; `skyzfsd` 139 and 3.

Not run here: the workspace's other crates' tests, and clippy with
`skyzfs-ublk-sys/control-test-program`. The pull request touches
neither, and CI runs both.

## CI and Codex

**At `d71869a`.** `skyzfs-ci` (run 44409340292) is green: gate, lint,
test and unit-arm64. No pull-request workflow ran, because the branch
conflicted with its base.

**At `c9b2b14`**, at 23:30 UTC, pull-request runs:

| Workflow | Run | Result |
|---|---|---|
| `skyzfs-cut-daemon` | 44416413852 | green |
| `ci` | 44416413908 | green: test, lint, unit-arm64, s3-backends, chart-lint |
| `skyzfs-replay-rig` | 44416413864 | green: the jobs "through skyzfsd" and "write into a hole", on 24.04 and 26.04 |
| `skyzfs-control-rig` | 44416413994 | green |
| `coverage` | 44416413892 | green |
| `e2e` | 44416413866 | green |
| `k3s-smoke` | 44416413912 | green |
| `skyzfs-daemon-rig` | 44416413859 | still running on 24.04, 26.04 and 26.04-arm |

The cut rig's jobs are green on ubuntu-26.04 and ubuntu-26.04-arm, and
its upload job too, each with worst exit 0. From the job logs:

- **`bound`** (both images): `cuts_abandoned: PASS: '1' abandoned at the
  bound`, on a `--zero-fill` volume, with holes 'dropped' and 0 chunks.
- **`boundsparse`** (both images): `cuts_abandoned: PASS: '0'
  abandoned`, with 8192 chunks, while the pool wrote 320 MiB into holes.
- **`held_content`**: PASS in every recording. For `boundsparse`,
  393,216 blocks compared on amd64 and 262,144 on arm64.

That closes F1 on the merged code. Testing's dispatched run had not
appeared when I wrote this.

**Codex.** Both threads are answered and resolved:

- F2, at `0f56669`;
- F4, at `af7b524`. The answer is in thread r4621577972, and issue
  comment 6993149813 holds the details.

I checked that answer against the code and the measurements above. Its
figures are right, and its plants F4b to F4g do what it says.

Codex has posted no review of `d71869a` or `c9b2b14`.

## Before the merge

1. **The daemon rig green on `c9b2b14`** (run 44416413859). It is the
   one pull-request workflow still running.
2. **Codex's look at `c9b2b14`, if it comes**, read once before "ready
   to merge".
3. **Optional, and cheap:**
   - G3: R13, or a load bound on the empty-volume case;
   - G4: R10 and R11.

   Both are in `scenarios_f4_cost.rs` and `scenarios_f4.rs`.

None of these is a defect in the product code.

## Files

| File | What it is |
|---|---|
| `scenarios_f4.rs` | R10, R11, R12, and the old `slice_from`. Appended to `state/tests.rs` at `d71869a` |
| `scenarios_f4_cost.rs` | R9 and R13. They build at `af7b524` and at `d71869a` |
| `scen.sh` | Appends scenario files, runs cargo and restores the file |
| `plants3.py`, `plants3.out` | The plants and their results |
| `plant_cost.py` | R9 or R13 figures with a plant in place |
| `cost3.out` | The figures of R9 and R13 before and after, and with Z01, Z06 and Z07 |
