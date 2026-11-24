# Next look at #132 (M2.T5b): heads 0a3c307 and 99b49b9

Pull request: skyzfs/skyzfs#132 (`skyzfs-testing` into `zfs-topology`).
**Head at this look: 99b49b9**, base 8e4638d. The earlier review was of
861b537 and 27fe831 (`review.md` in this directory). Since then:

- 69a73c4: only the window that let MinIO go measures the seed
  (`<tag>.released`, `released_here`);
- 0a3c307: seed rows apart from the median, the lease `none` at the
  window's start, D2 to D5, tests for S17 and S19;
- 99b49b9: the cut rig's `withheld` and node B's `lacks` controls pass
  over a write whose absence changes no copy.

The bench files are the same at 0a3c307 and 99b49b9. No commit
`ad3f2c7` exists on GitHub or in any ref I fetched: 99b49b9 is the rig
fix itself, cherry-picked onto 0a3c307 (its author date, 06:26, is
older than 0a3c307's). I formed my view from `git diff 27fe831..0a3c307`,
`git show 99b49b9` and the code before reading the answers.

## Verdict

**Nothing blocks the merge at 99b49b9.**

- The seed rule as built is the lead's option (i) together with
  Codex's mark of the releasing window. It is right, and the two parts
  agree.
- The seed rows are counted apart as the lead decided. Putting zi in the
  median is right.
- The D2 split is right for every status the bench can read but one,
  which the bench reaches only when the status file cannot be read.
- D3 to D5, S17 and S19 are fixed. D3 to D5 and S17 each have a test
  that catches its plant; S19's test asserts only a fragment of the
  paragraph (below).
- The rig fix is sound and cannot hide a blindness. A control that
  is not built still fails the job.
- CI: all ten pull request runs at 99b49b9 are green. Codex finished on
  99b49b9 with no finding, and its P2 thread from 27fe831 is answered and
  resolved.

**One item to change before "ready to merge" (text only, T1).** The
description still says "CI at 99b49b9 is running" and "99b49b9 fixes
it". All ten runs are now green. But the fixed path did not run in any
of them: none of the 44 control builds at the head passed over a write.
The description should say the fix is not yet shown in a run.

Non-blocking:

- one way a seed's aftermath can still enter the median (D6), a
  decision for the lead;
- small text items (D7 to D10);
- test gaps that the plants show (the table below);
- the answers on W7 and O7, which I judge below.

## For the lead

1. **D6: the median can still take a window that pays for the seed's
   aftermath.** It does so only in a case that run 44462902930 did not
   have. `seed_row` asks for `uploaded` 0 at the window's start.
   - Failing case: the seed commits a few seconds before a window, in
     its warm-up. For example, a `--zero-fill` volume with
     `TB_SKZ_UPLOAD_WAIT_S=0`, or `=window` on such a volume, where
     the seed outlives tpcb's window and commits in select's warm-up.
   - That window starts with `uploaded` 1 and `uploaded_whole` 1. Its
     first incremental upload falls inside it, and that upload holds
     every block written during the seed: in sw's tpcb window, 969,410
     blocks against sp's 571,214.
   - That window counts toward the median.
   - A cheap tightening: call a row a seed row when `uploaded` ≤
     `uploaded_whole` at its start, so no upload after the seed has
     committed. It changes no row of run 44462902930: every window
     there starts with 16 to 52 uploads, one of them whole.
   - Your call, now or in the next bench package. Certainty: high that
     the case passes the rule; medium that the bench will produce it.
2. **zi in the median is right.** Its seed committed with the pool idle
   (`WAIT_S=1800`), 35 s before tpcb's warm-up. tpcb's window started
   with 21 uploads and 0 cuts held. Its blocks per window (567,818
   tpcb, 109,265 select) are sp's (571,214, 104,010). zi measures the
   steady state on a zero-filled volume, which is a separate question
   from the seed.
3. **The rig fix in a bench pull request.** You asked for it. It
   changes only `tests/kernel/cut_check.py`, `nodeb_check.py` and
   `test_cut_check.py`. It is sound, and it makes the control
   self-checking, as described below.

## The seed rule and the seed rows

### `seed_row` (bracket.py:165-176)

A row is a seed row when it says it uploads and its status shows
`uploaded` '0' at the window's start. I checked it against
`status.rs`, against the bench's order and against the run.

- **Can a seed row slip into the median?** Yes, in one case: the
  window that starts right after the seed's commit (D6 above). A window
  in which the seed began or was still under way always has `uploaded`
  0 at its start: `uploaded` counts only commits of this instance, and
  the seed is its first commit on a fresh volume, which the bench makes
  for every entry.
- **Can a non-seed row be counted as a seed row?** Only in ways that
  do not touch the median:
  - A row whose uploader never committed has `uploaded` 0 at its start
    and is counted apart. MinIO broken or the lease never taken are two
    such cases. It has no verdict anyway (`not_uploading`), so it adds
    to "with none".
  - The other case is a daemon started again over a volume already
    seeded (`upload_found_committed` above 0), whose counters restart at
    0. The bench never restarts a daemon between workloads. Asking for
    `upload_found_committed` 0 would close it, as hardening.
  - A row whose start's status was not read has no `uploaded`. It counts
    as steady with no verdict, which is harmless.
- **Table check** (`diff_seed_next.py`): over 1,800,000 rows,
  `seed_row` is set exactly when the row uploads and `uploaded` is '0'
  at its start. There were 0 disagreements.
- **The summary** (topology-bench.sh:950-963) counts verdicts by their
  prefix, so a marked "BELOW 0.90 (seed inside window, ...)" counts as
  below. The plants show that the counts are tested only at zero (N9,
  N10).

### `released_here` (69a73c4) and the `none` rule (0a3c307)

The final rule (bracket.py:179-211) asks for both: the row's window
let MinIO go (`released_here` True), and the lease read `none` at that
window's start. This is not a replacement of one by the other, and
neither part is dead:

- `none` alone would still mark a later window whose lease was taken
  late (a store that refused the lease in the releasing window and
  took it in the next);
- `released_here` alone would still mark a releasing window that
  started with a lease. The bench cannot reach that case, because
  MinIO is frozen until after the start's read.

**Table check** (`diff_seed_next.py NEW PLAIN`, NEW at 99b49b9, PLAIN
at 861b537, the rule of #128): 1,800,000 rows that vary the mode,
`released_here` (True, False, absent), the pids, the lease and epoch at
each end, `uploaded` and `uploaded_whole`. **0 disagreements:**

- every non-window row gets the plain rule's verdict;
- a window row gets the marker exactly where the final rule holds (120
  rows);
- every other window row gets the plain verdict, plus "its window did
  not let MinIO go" when `released_here` is not True, or "its seed ...
  was not shown to commit inside it" when it is. The clause is added
  only when the plain reason is the lease;
- a sample of the marked rows, judged in a bracket 26% wide and with no
  model row after it, gets NO VERDICT and no marker.

**Contradictory or dead code:** none in the logic. Small items:

- `start` keeps the start's epoch (bracket.py:204), which nothing reads
  now (D9);
- old rows read "did not let MinIO go" (D7).

**Tests tied to the final rule:**

- `ASeedInTheWindow` refuses held and unsure at epoch 1, lost and
  missing at the start, and a row with `released_here` False or absent;
- the plants N12 to N16 (the rule) and N17 (the row's reading) are each
  killed;
- the shell side of the mark is not tested: plants N18 to N21, writing
  and removing `<tag>.released`, all survive. A broken mark gives NO
  VERDICT ("its window did not let MinIO go") and never a false
  verdict, so the gap is low.

**Run 44462902930 re-rendered at the head**, from its rows recorded at
861b537:

- as recorded (no `released_here`): sw's tpcb row reads "NO VERDICT:
  ... its window did not let MinIO go, so its seed was not measured from
  its release". The counts read "5 with a verdict (1 at least, 4 below),
  0 with none" and "Seed rows ... (0 with a verdict (0 at least, 0
  below), 1 with none)";
- with `released_here` set on sw's tpcb row, as the run's log supports:
  the row reads "BELOW 0.90 (seed inside window, lease at release
  (epoch 1))". The counts are the description's: steady "5 with a
  verdict (1 at least, 4 below)", seed rows "1 with a verdict (0 at
  least, 1 below)". The other five verdicts are unchanged.

### The D2 split in `one_server` (bracket.py:103-119)

A no-pid end with any key besides `read_at_ns` and `pid` reads "no
serving daemon at an end"; one with only the clock line reads "status
not read". I checked this against `status::report` (status.rs:1866-2116)
and `commands::status` (commands.rs:228-297). `pid` is printed only for
a live device of ours with this boot's status file. The other shapes
are:

| status the bench can read | no-pid fields | read as | right? |
|---|---|---|---|
| daemon gone: counters "left by a daemon that is gone" | yes | no serving daemon | yes |
| device waiting for a server, or dead | yes (`state`, `device`, counters) | no serving daemon | yes |
| volume detached, or a failure note | yes (`state`, `failure...`) | no serving daemon | yes |
| `skyzfsd status` failed before printing (no device record, `lim` timeout) | only `read_at_ns` | status not read | yes |
| **live device, status file unreadable** | yes (`state serving`, `device`, `queues`, `depth`) | **no serving daemon** | **no (D8)** |

The last row happens when `status::read` returns an error. The command
then prints the state and exits 1 with its error on stderr, and
`skz_status` drops both (topology-bench.sh:1830). The file is written
whole and renamed (status.rs:1414-1420), so a torn read cannot cause
it; an I/O error or a file the binary does not decode can. Since the
bench reads with the binary that serves, this is a nit.

`pid_table.py` at the head against 27fe831 (`pid_99b49b9.out`,
`pid_27fe831.out`): the 10 combinations with a gone end change from
"status not read" to "no serving daemon at an end" in all three tables,
and nothing else changes. All 30 verdicts are unchanged.

### D3 to D5, S17, S19

- **D3:** the paragraph (topology-bench.sh:930-938), the module
  docstring and `seed_in_window`'s docstring state the rule as built.
- **D4:** "at the start of the window that released it".
- **D5:** "BEFORE MinIO was let go (at R, the read at N): the freeze
  did not hold", tested with `REL=120`.
- **S17:** a seed row in a bracket 26% wide, and one with no model row
  after it, have NO VERDICT and no marker.
- **S19:** `test_embedded_python.py` asserts both counts and the
  fragment "is not one of design §9's median" of the paragraph's
  seed-row sentence.

D3 to D5 and S17 each have a test that kills its plant (table below).
S19's does not: nothing asserts the paragraph's seed-*rule* sentence
(plant S19, SURVIVED), and the seed-row sentence is asserted only by
that fragment, so removing its subject "A seed row" survives too
(S19b). Low: the paragraph is prose, and the counts it describes are
asserted.

### The earlier survivors, and the answers on W7 and O7

Of the five survivors of the last look, S17 and W13 are now killed
(W13 by the `REL=120` case of `test_copy_limit.sh`), S19 is covered
only in part (above), and W7 and O7 still survive.

- **W7:** the reason given for the order holds. The time is written
  before the unpause, so the watcher's lower bound is a true lower
  bound. The reason given for not testing it does not hold.
  `test_copy_limit.sh`'s `minio_resume` already stands in for
  `skz_minio` with a shell function, and `minio_freeze` tests the same
  kind of order ("the flag before the pause"). A stand-in that writes
  whether `$tt/released` exists when `unpause` is called would make it
  a one-line test. Low, as before: a late write moves the bound by the
  length of a `docker unpause`.
- **O7:** the column is `one_server`'s value, and `one_server`'s
  start-side cases are tested in `test_bracket.py`. That covers
  `one_server`, not the column's expression, and the plant changes the
  expression. O7 still survives. Low.

## The rig commit (99b49b9)

**What it does.** `pick_withheld` (cut_check.py:336-348) walks back
from the cut's uberblock write as before. Given `differs`, it takes the
first write whose absence changes the bytes of some copy of a reached
block, and lists the writes it passed over. `withholding_changes`
(319-333) answers that by building the bytes of the write's
intersection with each copy (`copy_ranges`, 289-298) twice from the
log over the base image, with and without the write (`bytes_at`,
301-316). Node B's `lacks` uses the same pick (nodeb_check.py:461).

**Is the comparison the right test of a wrong cut, given asize may
exceed psize?** Yes. The range compared is the DVA's
`[offset, offset + asize)`, a superset of the psize bytes the checksum
covers. This happens in the rig's own pools: at the head the `config`
controls name blocks of `1000L/200P` and `20000L/400P` on DVAs of
0x1000.

- When withholding a write changes no byte in those ranges, no check
  of any copy can catch it. The image differs, at most, outside every
  reached block. So passing over such a write never discards a control
  that could have been caught.
- The other direction is an edge: a change only in `[psize, asize)`
  would be picked and then missed. That miss is loud, not silent. ZFS
  zero-pads that tail on every write, so the psize part would have to
  match while the tail differs.
- A gang block's DVA asize covers its whole tree, which is not
  contiguous. That over-reach was already in `overlaps`. It errs in the
  same loud direction.

**Does it model the image the rig builds?** Yes:

- records `[0, prefix)` in record order over `base_read`, the same
  base image `reset` copies;
- discards zeroed, as `logwrites.advance`'s default, which
  `Check.advance` uses;
- records without data skipped, as `advance` skips them.

The control image is then built from the same schedule less the write.

**Is the inference sound, given that neither artifact keeps the data?**

- **Cut rig, run 44462542583** (24.04-arm, `rewind`). Its artifact
  keeps the fixture (`fixture/requests.tsv.gz`): offsets and lengths,
  but no data for these records.
  - It confirms that record 696 and record 686 are both 4096 bytes at
    111173632.
  - Both fall between cut 12's flush (record 670) and cut 13's
    uberblock write (698): group 26.
  - In the same group, six other places are written twice (678/689,
    679/688, 680/691, 683/693, 684/694, 685/695): later sync passes
    rewriting in place.
  - The control passed every copy (363 blocks, 521 copies). So 696's
    bytes in that copy were what 686 had left: an identical rewrite.
    That is inferred, not seen, but nothing else fits the pass save a
    gap in the copies check.
- **Node B, run 44475818541** (26.04-arm, `slog`, at 69a73c4). Its
  artifact keeps no fixture, and `work/img/cut.img` is the cut with
  record 887 applied. The line's `read snap/t` value differs between the
  cut and the `lacks` volume, but it is the ref's commit id
  (`skyzfs-nodeb get` prints `rv.commit`), not a digest of the content.
  It says nothing either way. The inference there rests on the pass
  (325 blocks, 507 copies, import, mount and files all clean) and on
  the analogy. **It is credible.**
- **More to the point, the fix no longer depends on it.** Say 887's or
  696's bytes did differ, and the pass came from a gap in the checks.
  `differs` would be True, the same write would be picked, and the
  control would miss again, loudly.

**Can it recur at the head?** Not for this reason: a write that
changes no reached copy is now passed over and named. A miss at the
head would mean a write that changes the bytes of a reached copy
passed every checksum. That is a verdict on the rig or the pool, and
worth a look if it comes.

**Can the control now vanish silently?** No.

- In the cut rig, `withheld` 'not built' is not `EXPECTED`'s 'caught'
  in any recording, so the line says DIFFERS and the job exits 1
  (cut_check.py:810-816).
- In node B, a control not built has `ok` False and `rc` 1
  (nodeb_check.py:491-504).
- In practice 'not built' would need every write before the uberblock
  that reaches a copy to change nothing, back to record 0. That would
  itself point to an error of the rig.

**In CI at the head** all 44 control builds were caught, and none
passed over a write:

- the cut rig: 4 images × 10 recordings;
- node B: 2 images × 2 recordings.

So the new path has not run in a kernel job. The cut rig's line will
name what it passes over. Node B's will not (D10).

**Tests.** `test_cut_check.py` covers:

- an identical rewrite;
- a write covered by a later one;
- a write of what the base held;
- bytes that differ only outside the copies;
- `bytes_at`'s order and discards.

The plants R1 to R6, R8 and R9 are killed. The ones that survive are
the wiring, which only the kernel rigs run:

- R10: `main` picks without `differs`;
- R11: `base_read` reads zeros;
- R12: node B picks without `differs`;
- R7: the prefix taken as the uberblock record. Equivalent unless a
  write between the uberblock write and the flush overlaps a reached
  copy, which ZFS does not do.

A pass-over in a kernel run would show the wiring. Until then, a
unit test of `main`'s pick over a small log would.

## Non-blocking defects

### D6. A window right after the seed's commit counts toward the median (low to medium; for the lead)

See "For the lead", item 1. `bracket.py:174`. **Certainty:** high that
such a row passes the rule; medium that the bench will produce one.

### D7. A row recorded before 69a73c4 reads "its window did not let MinIO go" (low, text)

- **Where:** `bracket.py:149-150` reads a missing `released_here` as
  False.
- **Effect:** for run 44462902930's sw tpcb row, the text is false. The
  row's window did let MinIO go, but the mark was not recorded then.
- **The verdict is right:** NO VERDICT, the conservative side. The
  description says so.
- **Fix:** "its window is not marked as the one that let MinIO go".

### D8. A live daemon whose status file cannot be read is "no serving daemon" (nit)

`bracket.py:115-118`; the table above. The bench reaches it only when
the file cannot be read. **Fix:** read `state serving` with no pid as
"status not read".

### D9. `seed_in_window` keeps the start's epoch, now unused (nit)

`bracket.py:204`.

### D10. Node B's line and text describe the old pick (nit)

- Its 'not built' text still says "no write before the uberblock write
  holds a block the cut reaches" (`nodeb_check.py:463`).
- Its docstring (46-47) still says the same.
- Node B does not pass `same`, so its line does not name what it passed
  over. The description says "The fixed control names any write it
  passes over", which holds for the cut rig only.

Pre-existing, still there: the seed clause says "under the lease held
at its end" even when the end's lease is not held (`bracket.py:152-154`).

## The description against the code

What it says of the seed rule, the seed rows, D2 to D5, the controls,
the run and zi matches the code and the re-render above. The items:

- **T1 (to change before "ready to merge"):**
  - "CI at 99b49b9 is running" is stale: all ten runs are green.
  - "99b49b9 fixes it" claims more than the run shows. Node B passed
    at the head on a recording whose picked writes (1081, 945, 724, 646)
    were caught directly, and no write was passed over anywhere.
- **Low:**
  - "The fixed control names any write it passes over" holds for the
    cut rig, not node B (D10).
  - "A `window` row that fails the seed rule gets the plain reason,
    followed by why its seed did not count" holds only when the plain
    reason is the lease. Other reasons get no clause.
- Its claims on the two rewind runs that caught (44471764965 and
  44471770545: records 717 and 778, caught) and on the misses are
  right.

## CI and Codex at 99b49b9

All ten pull request runs at 99b49b9 are green (created 07:08, all
ended by 08:10):

- ci 44477358450: changes, lint, unit-arm64, s3-backends, test,
  chart-lint;
- coverage 44477358458, e2e 44477358357, kernel probe 44477358361,
  control rig 44477358374;
- daemon rig 44477358449: the daemon on 24.04, 26.04 and 26.04-arm, the
  stand-in on 24.04 and 26.04, and the table's judge;
- replay rig 44477358532: both reference layouts, through skyzfsd and
  write into a hole on 24.04 and 26.04;
- cut rig 44477358362: 24.04, 24.04-arm, 26.04, 26.04-arm and the
  reference;
- cut daemon 44477358359: 26.04, 26.04-arm, 26.04-upload;
- node B rig 44477358578: 26.04, 26.04-arm and its logic.

The push runs of the cut rig and node B on the same head were
cancelled when the pull request runs began, which is the workflows'
concurrency, and skyzfs-ci's push run passed. Nothing needs a rerun.

I read the jobs' conclusions and the controls' lines, not every rig's
summary table.

**Codex:**

- 99b49b9: completed, no finding;
- 0a3c307 and 69a73c4: no finding posted;
- the P2 thread of 27fe831 (comment 4624979778): answered by the
  author and resolved.

## Local gates at 99b49b9

| Check | Result |
|---|---|
| shellcheck 0.11.0 on every tracked `*.sh` | clean |
| `tests/bench/test_*.py` | bracket 28, cut_release 13, embedded 7, perfcat 6, skz_time 39, syscalls 6: all OK |
| `tests/kernel/test_*.py` | control 21, cut_check 29, cut_daemon_check 104, cut_daemon_main 38, cut_hold 18, cut_plan 11, cut_ref 41, cut_watch 12, daemon_check 93, nodeb 28, replay 53, replay_daemon OK, replay_hole 24 |
| `test_copy_limit.sh` | 108 ok |
| `tests/lib/test_skz_store.sh` (as root) | 109 ok |
| `tests/kernel/test_cut_record.sh` | all passed |

## Plants at 99b49b9

`plants_next.py` plants one change at a time in a worktree at 99b49b9,
runs the tests of the file it changed (each under 600 s), and restores
the file in a `finally`. Of 42 plants, 28 are KILLED, 14 SURVIVED and none NOT APPLIED; the tree was clean after the last. Each plant's old text was found exactly once.

| Plant | Result | Why it survives |
|---|---|---|
| N1 D2 split dropped (every end with no pid: status not read) | KILLED by bracket, embedded |  |
| N2 D2 every end with no pid read as no serving daemon | KILLED by bracket, embedded |  |
| N3 D2 the clock line counted as a field | KILLED by bracket, embedded |  |
| N4 D2 the tables print a gone daemon as two daemons | KILLED by embedded |  |
| N5 seed_row never set | KILLED by bracket, embedded |  |
| N6 seed_row for a row that does not upload | KILLED by bracket |  |
| N7 seed_row from uploaded_whole | KILLED by bracket, embedded |  |
| N8 the median counts the seed rows too | KILLED by embedded |  |
| N9 the counts swap at least and below | SURVIVED | the counts are asserted only at zero verdicts |
| N10 a marked BELOW verdict not counted | SURVIVED | as N9: no marked BELOW verdict in the summary test |
| N11 the seed rows' line not printed | KILLED by embedded |  |
| N12 seed_in_window ignores released_here | KILLED by bracket |  |
| N13 a row with no released_here taken as released here | KILLED by bracket |  |
| N14 the clause "did not let MinIO go" dropped | KILLED by bracket |  |
| N15 the start rule back to 27fe831's (held at the end's epoch taken) | KILLED by bracket |  |
| N16 the start rule refuses only a held lease (lost, missing taken) | KILLED by bracket |  |
| N17 the row reads released_here True always | KILLED by embedded |  |
| N18 the window's mark not written | SURVIVED | the shell side of the mark is untested; a broken mark gives NO VERDICT, never a false verdict |
| N19 run_workload passes no tag (no mark) | SURVIVED | as N18 |
| N20 the mark written though the unpause fails | SURVIVED | as N18; MinIO stays frozen, so no seed commits and the rule still fails |
| N21 an earlier row's mark not removed | SURVIVED | as N18 |
| N22 D4 the watcher names the first window again | KILLED by copy_limit |  |
| N23 D5 the BEFORE line dropped | KILLED by copy_limit |  |
| N24 D5 the BEFORE line without watch | KILLED by copy_limit |  |
| W7 the release time written after the unpause | SURVIVED | untested order; low (see "The earlier survivors") |
| W13 the release time not asked to be before the read that saw it | KILLED by copy_limit |  |
| O7 the status column prints n/a for a start with no pid | SURVIVED | the column's expression is untested; `one_server` is |
| S17 the marker judged past a bracket too wide | KILLED by bracket |  |
| S19 the paragraph says nothing of the seed rule | SURVIVED | nothing asserts the seed-rule sentence |
| S19b the paragraph says nothing of the seed rows | SURVIVED | only the fragment "is not one of design §9's median" is asserted |
| R1 the pick ignores differs | KILLED by cut_check |  |
| R2 the passed-over writes not kept | KILLED by cut_check |  |
| R3 the whole record compared, not the copies | KILLED by cut_check |  |
| R4 a discard leaves the bytes | KILLED by cut_check |  |
| R5 the withheld write applied all the same | KILLED by cut_check |  |
| R6 the base image read as zeros | KILLED by cut_check |  |
| R7 the cut's records counted to its uberblock write, not its prefix | SURVIVED | equivalent unless a write between the uberblock write and the flush overlaps a reached copy |
| R8 differs always true | KILLED by cut_check |  |
| R9 a write's bytes taken from its start, not from the range | KILLED by cut_check |  |
| R10 the cut rig's main picks without differs | SURVIVED | the wiring in `main`: only a kernel run shows it |
| R11 base_read reads zeros | SURVIVED | as R10 |
| R12 node B picks without differs | SURVIVED | as R10, node B's wiring |

## Appendix: the scripts (in this directory)

Run each with `python3 -I`, from a directory of its own:

- `diff_seed_next.py NEW PLAIN`: the final seed rule and `seed_row`
  at NEW (99b49b9) against an oracle, and every other row against PLAIN
  (861b537), over 1,800,000 rows. It exits 1 on any disagreement.
  Output: `diff_seed_next.out`.
- `pid_table.py TREE` (from the first review), at 99b49b9. Output:
  `pid_99b49b9.out`.
- `plants_next.py TREE [FIRST [LAST]]`: the plants above. It needs root
  for `test_skz_store.sh`. Output: `plants_next.out`.
