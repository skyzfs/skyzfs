# M1.3t (the log store removed), pull request 69

## The owner's ruling for this review

Passed on by the lead on 2026-11-13:

- Only tests of the log store's internals may go. The storage
  contract's tests stay.
- Coding owes a table of every deleted test: which kind it was, and for
  a contract or pump/lifecycle property the in-place test that now
  holds the same property (ported where none existed).
- Judge that table against the code. A deleted test whose property is
  held nowhere any more is a blocking finding.

## First look, head 45b67f2 (base 3b98588)

Verdict: cannot merge at this head. The review is on the pull request.

### How the table was judged

1. `static-tests.sh` lists `file::fn` after `#[test]` at a commit: 732
   at the base, 555 at the head, 230 gone, 53 new. Every gone test is
   named in the description but the nine of `format.rs`.
2. Every test named under "held now by" exists at the head (159 names
   in 88 rows, by a script over the description's tables).
3. Three readers (subagents) read 77 rows against both commits:
   `first-reading-a.md`, `-b.md`, `-c.md`. Their verdicts are a first
   reading and nothing more. The reader of rows c was given the wrong
   column for the 5-column tables (my parse); `rows5.md` has those 22
   rows right, and I judged them myself.
4. Every PARTLY of the readers was put to a plant or to the code:
   `plants69.py`, `plants69.txt`.

### What the plants showed (`plants69.txt`)

Caught: P1 to P6 (the order of syncs in create and in the volume
file), P7 (an open refused for its volume file writes the marker), P9
to P13 (the refusals), P16 (the harness's kill drops the store), P18
(the marker's removal not synced), P23 (the write unit), S3 (a read
that ends on a thread after the failure).

Not caught by any test of skyzfs-local (and for the wide ones of
skyzfs-ublk and skyzfsd):

- S1: an I/O thread wakes the owner for its first result only. The
  deleted test counted one call per completion.
- S2: a block of twice the write unit is let through. The deleted tests
  refused 2x, 4x and 1 MiB; the port refuses 4x.
- S4: the observer is told at every `fail()`. The deleted tests ran a
  request after the failure and asked for one call.
- P7b: a refusal for a home image of another size writes the marker
  (a test of this pull request, not a port).
- S2b: the home image's own file system is not held to the unit (no
  test at the base either: the simulated disk has one block size).
- S5: the lock file's mode (no key is left in the directory).

`the_suite_notices_the_defects` failed under P23, which has nothing to
do with it: that led to the loop below.

### `the_suite_notices_the_defects` depends on the scheduler (`ackloop.sh`)

At the unplanted head, twelve runs each:

- alone: fails 12 of 12, on SYNC_RETRIED ("the defect 0x4 is not
  noticed by reopened_failed_volume_holds_what_was_flushed"). Coding
  says the base does the same and #70 is for it.
- alone on one CPU: fails 7 of 12, on ACK_BEFORE_WRITE not noticed by
  `kill_keeps_acknowledged_operations` (other cases notice it).
- beside the whole library suite: see `ackloop.txt`.

### CI at 45b67f2

Red in four jobs on
`tests/inplace_real.rs::every_start_serves_where_the_image_begins_with_a_hole`
("2 pages of it stay in the cache after two drops", at start 4, 1, 2,
35). `readahead.py` shows the mechanism as far as this machine goes: a
drop right after a read of block 0 through the cache leaves pages (in
539 of 2,000 rounds idle, 445 of 3,000 under writes, 64 of 2,000 on a
busy machine; 1 or 0 of 2,000 with 2 ms between); a second drop always
cleared them here.

### Codex

Both of its threads hold against the code: `TimedVolume` does not
forward `reaches_positions` (bins/skyzfsd/src/timing.rs), and `create
--zero-fill` fills after `inplace::create` released the lock.

### Gates

`gates.txt`: clippy on the skyzfs packages and skynbd-tree, plain and
with each feature, and their tests: clean (540 + 65 + 15 passed). The
whole workspace was not built here: the disk allowance ran out on the
skynbd crates, which this pull request does not change but for one
unused dependency of skynbd-tree.

### Posted, head ffde7bf

The review is `review-posted.md`, posted on the pull request on
2026-11-14 (issue comment 6868735458). d173019 (the look reads alone)
and ffde7bf (Codex's two findings) judged right. The plants of the
blocking findings ran again at ffde7bf: `plants69-ffde7bf.txt` (S1, S2,
S4 and P7b: nothing fails). Digests sent to coding and the lead. Next
look: the answers, and the rigs' tables at the head.

### Lessons

- A table parsed from a description: check the column count of every
  table before handing rows to a reader. The 5-column tables lost
  their last column, and one reader worked on wrong rows.
- A test that holds planted defects can itself depend on thread
  timing. Run such a test alone and on one CPU, not only in the suite.
