# Next look at #123: 6cbfc0b (the answers to the next look at 72aaa06)

Pull request: skyzfs/skyzfs#123 (`skyzfs-testing` into `zfs-topology`).
**Head reviewed: 6cbfc0b**, checked with `gh api` when I began and again
at the end (below). It adds one commit since 72aaa06, which changes six
files:

- `topology-bench.sh` and its two tests;
- `cut_daemon_check.py` and its test;
- `test_skz_store.sh`.

I read the commit, testing's answers (issue comment 6990845851) and the
notes at `zfs-topology`'s tip 79d86d6. The base has moved since the pull
request was pushed: #122 and #124 merged, and the lead corrected the
notes. Neither merge touches a file of this pull request, and the
daemon's status is still version 16 there (`status.rs`, `VERSION`).

Local gates at 6cbfc0b, all passing:

- `test_copy_limit.sh`;
- `test_embedded_python.py`;
- `tests/lib/test_skz_store.sh`;
- `test_cut_daemon_check.py`, 100 tests;
- `test_cut_record.sh`;
- shellcheck 0.11.0, clean on every tracked `*.sh`.

## Verdict

**Not yet, and only because of CI.** The review has no blocking finding
at 6cbfc0b. One CI job is red, for a reason unrelated to this pull
request: the daemon rig's ubuntu-26.04 job on the stand-in ended with
exit 2, the rig's own error. ZFS stuck in the kernel in check d. The
daemon rig uses no file that this pull request changes.

When that job is rerun green, #123 can merge. The lead may also accept
the exit 2 as the known stall. Details are under "CI at 6cbfc0b".

F4 and F5, the two blocking findings, are fixed. So are G10, G11, G12, R9,
T2 and T11, and the balance's text. I checked each fix against the code
and planted against each (the table below). I also judged the real status
files of #125's daemon with the new judge.

What remains is low and does not block:

- **N1.** "Under the workload" can still be wrong by up to one round of
  the watcher, about 5 s, when the warm-up begins between the read before
  the commit and the read that sees it. The test "begun in the very second
  of the commit" pins such a case as "under".
- **N2.** Four plants on the watcher's mark and the teardown's NONE line
  survive. The code is right today, and each gap needs only a small
  test.
- **N3.** `bound`'s line still prints the copies read after the holder
  let go, the figure that F4 removed from `boundsparse`. It is not
  judged.

## F5: fixed

**Where the mark is written.** `run_workload` writes the time,
`date +%s`, into `$WORK/skz-workload-began`:

- before its `log "$tag: warm-up"` and the warm-up's pgbench
  (`topology-bench.sh:2696`);
- only while a watcher runs, and only if no mark exists yet.

Before that line, `run_workload` only names the pgbench log and maps the
workload's name. Per topology the order is:

1. the build (the import, a `zpool sync`, then the watcher);
2. `mount_and_start` (postgres);
3. `fsync_test`;
4. the workloads.

So the mark is the first warm-up's start, as the answers say, and what
the line calls "before the workload" is the setup: postgres's start and
pg_test_fsync. The test checks that the mark comes before the warm-up in
`run_workload`'s text (plant F1 is killed).

**The label.** At the read that first shows `uploaded` ≥ 1, the watcher
reads the mark. If the mark is a number not after `now`, the line says
"under the workload, M s after its warm-up began"; otherwise "before the
workload began". The tests cover:

- no mark;
- a mark before the commit;
- a mark in the same second;
- a mark after the commit;
- a mark that is not a number.

Plants F2 (any mark means under) and F3 (the same second means before)
are both killed.

**Can it still be false?**

- **No mark** (a row that failed before pgbench). The line says "before
  the workload began", which is true.
- **Written late.** No: nothing that writes the pool runs between the
  mark and the warm-up.
- **The seed committing at the mark: N1, below.** The label can be wrong
  by up to one round.
- **Several workloads, or a second phase.** One watcher and one mark
  serve the whole topology. A row after the first therefore reads "M s
  after its warm-up began", counted from the topology's first warm-up.
  That is what "the workload began" means in the table's text. A mark
  rewritten at every warm-up (plant F4) would only move M's origin, and
  it survives. That is harmless.
- **A stale mark.** `TB_WORK` is `/mnt/topology-bench` for every row of
  a job (`topology-bench.yml:173`), so a mark from an earlier row would
  still be there. The build removes it before the watcher starts
  (`:2192`). Without that `rm`, plant F5, an earlier row's mark would
  call every seed of this row "under the workload", with an M of
  minutes. F5 survives: see N2.
- **The daemon restarting during the rows.** A status unread three rounds
  running ends the watcher with its NONE line. A new daemon's status read
  in time keeps the watcher going, and the row's table marks "TWO
  DAEMONS". Nothing here is new.
- **The time origin.** "From the import" is counted from the watcher's
  start, after the import and a `zpool sync`. Close enough, and the same
  for every row.

**44372469282's figures.** The lead's notes at 79d86d6 correct round 5:
the rows with a wait of 0 "are NOT shown to have seeded under the load".
The seed committed 16 to 17 s before the warm-up. The notes point to the
cut rig (44378510338) for the holes under a write load. With the new
label, those rows would have read "after 22 s from the import, before
the workload began".

**G10.** Done as the answers say:

- The watcher's first line is "not committed by the window's end
  (watched from the import)". A row reads `.skzinfo` at its window's end
  and keeps a key's last value, so the line holds until the commit's
  line replaces it. Plant F6 is killed.
- The teardown appends "NONE by the end of the rows (N s after the
  import)" when the watcher is still alive. That line is written after
  the last row's table, so it reaches the record (the artifact's
  `.skzinfo`) and no table, which is what the answers say. Plant F7 (the
  line not written) is killed by the test's check of the text. Plants F8
  and F9 survive (N2).
- The status is read once a round: one `skyzfsd status` and `awk` on its
  text.
- The table's text says what the column holds with a wait of 0.

### N1. "Under the workload" can be wrong by one round (low; does not block)

The watcher sees the commit at a read. The commit itself came at some time
after the read before it. That read was one round earlier: one status
read (with `lim 30`) and `sleep 5`. `now` is taken after the read that
sees it. Suppose the warm-up begins between the two reads, and the
commit came before the warm-up. The line then says "under the workload,
M s after its warm-up began", with M below about 5, and it is false.

The test pins such a case. In its clock, `BEGAN=110 first_upload 100 3600
watch`:

- the status reads `uploaded 1` from 100;
- the read at 100 sees it;
- `now` is 110;
- the line says "under the workload, 0 s after its warm-up began".

The commit came at 100 or before, and the mark is 110.

It does not block. M is printed, and an M under a round tells a reader
that the commit was at the border. Round 5's case, 16 to 17 s before the
warm-up, now reads right.

Fix: keep the time of the read before. Say "under" only when that read
was at or after the mark. When the mark lies between the two reads, say
"as the workload began (within N s)". The same-second test then expects
that third wording.

### N2. Four plants survive on the mark and the teardown (low)

- **F4, the mark rewritten at every warm-up:** harmless (above).
- **F5, the stale mark not removed at the build:** it matters, since
  `TB_WORK` is shared by the rows of a job. A text check like the others
  would kill it: the `rm -f "$SKZ_SEED_BEGAN"` comes before the watcher
  starts in `build_zfs_direct`.
- **F8, the teardown writes its NONE line even after a watcher that
  committed,** and **F9, the line goes to `/dev/null`:** the teardown's
  NONE line is tested only by a `grep` for its text.
  - F8 would leave a record whose last `first_upload` says NONE after a
    seed that committed. A row reads before the teardown, so no table
    would show it.
  - A small test would kill both: the teardown's block run against a
    sleeping stand-in watcher and against one that has exited, with
    `SKZ_SEED_INFO` set.

None of them is wrong in the code at 6cbfc0b.

## F4 and G11: fixed

**The line now says what it measures.** `boundsparse` reads "'N'
abandoned ...; ... so none must be: the copies stayed under the bound".
The copies criterion is gone. The comment above it explains why:

- the status is read after the holder let every cut go;
- during the hold, #125's daemon copied 0.75 to 1 MiB (44378510338).

The module docstring says the same, and so does the unit test's
docstring. The unit test asserts that the line contains "the copies
stayed under the bound" and does not contain "nothing copied". Nothing
else in the tree still claims that nothing is copied:

- the comment at `cut-record.sh:773` says a write into a hole is not
  copied, which is true;
- `obs_copied_blocks` speaks of single blocks.

**The gate is `status_version >= 17`** (`cut_daemon_check.py:1515-1516`).
It reads `status_version` from `status.txt`, which every real status
carries: 16 on the base's daemon, 17 on #125's.

- **A version 16 daemon** is asked nothing: INFO, "status version 16,
  before 17 and M2.4e". That is right, since it keeps no holes. It reads
  so at 6cbfc0b in the push run's cut rig on the base's daemon
  (44392308845, ubuntu-26.04): `bound` PASS with '1' abandoned,
  `boundsparse` INFO, worst exit 0.
- **A version 17 status without `cut_holes`** is the rig's ERROR (exit
  2). It is loud, not silent. That holds even if another package takes
  version 17 before #125 does.
- **A version 17 daemon with `cut_holes none`** is judged. Abandons give
  a FAIL that adds "the daemon took no holes on this volume". With none
  abandoned it passes. That is honest, since the line claims only that
  the copies stayed under the bound.
- **A version 17 daemon whose hole set exists but does not work** copies
  the writes, abandons at the bound and fails. One that works only in
  part, with copies under 256 MiB of the 320 MiB written, passes. The
  line now claims no more than that. The test that would catch it, a
  status read while the first cut is held, is the next cut-rig package
  (the answers, at the lead's word).

**The unit test's status is the one the rig reads.** I judged the real
`boundsparse/status.txt` of both images in 44378510338 with the new
judge (`realstatus.py`, in the appendix). Each status reads:

- version 17;
- `cut_holes dropped`, 8192 chunks;
- copies 0;
- 0 abandoned.

Both give PASS, `(0, False)`. The test's fixture has the same values.
`bound`'s real statuses still pass with '1' abandoned. The base's
version 16 status (44340268684), judged as `boundsparse`, gives INFO.

Plants G1 to G9 are each killed:

- the gate on the key again;
- the gate at 18 or at 16;
- ERROR turned to INFO;
- the abandoned count not asked;
- the "took no holes" text dropped;
- `none` asked nothing;
- INFO unless the set holds;
- the version read from a key the status lacks.

### N3. `bound`'s line still prints the copies read after the let-go (low)

`bound` reads PASS, "'1' abandoned at the bound, with the copies '0'
bytes of '268435456'". The 0 is read after the holder let every cut go,
as in F4. It is not judged, but it reads as if the copies were 0 when
the cut was abandoned. Drop the figure, or say "(after the let-go)".
Also, `ok_cp` at `cut_daemon_check.py:1506` is now unused.

## R9, T2, T11, U1, U4, the balance

- **R9.** The check after the loop is now a function,
  `skz_upload_topologies`. `test_skz_store.sh` runs it alone:
  - the daemon alone, with or without blanks around it, is let through;
  - the daemon twice, the daemon beside the model, and no topology at all
    are refused;
  - with no uploads, anything is let through.

  It also checks the call and its exit 64. Plants R9 (the daemon alone
  refused), R9b (the check not called), R9c (the refusal exits 0) and R9d
  (the function lets everything through) are each killed.
- **T2 and T11.** The fixture's mismatches and abandoned-at-a-mismatch
  now go from 1 to 2. Both plants are killed.
- **U1 and U4** were not addressed, as agreed in the last look: a run
  shows both at once. I did not rerun them.
- **The balance.** The table's text says that found committed, gone and
  kept have no column, and that the status files give them when they are
  not 0. Good.

## Plants at 6cbfc0b

`plants4.py` (in the appendix) applies each plant to a copy of the
head's tests and workflows, one at a time, and restores the file after
each. A plant of the bench script is followed by `test_copy_limit.sh`,
`test_embedded_python.py` and `test_skz_store.sh`. A plant of the cut
judge is followed by `test_cut_daemon_check.py`. The list ran in two
parts on two copies.

| plant | result | killed by |
|---|---|---|
| G1 boundsparse gated on the key again, not the version | KILLED | cut_check |
| G2 the gate at 18 | KILLED | cut_check |
| G3 the gate at 16 (a daemon before M2.4e judged) | KILLED | cut_check |
| G4 version 17 without cut_holes: INFO, not the rig's error | KILLED | cut_check |
| G5 the abandoned not asked | KILLED | cut_check |
| G6 a FAIL with holes none does not say the daemon took none | KILLED | cut_check |
| G7 a version 17 daemon that took no holes (none) asked nothing | KILLED | cut_check |
| G8 INFO unless the set still holds at the end | KILLED | cut_check |
| G9 the version read from a key the status does not have | KILLED | cut_check |
| F1 the mark written after the first warm-up | KILLED | copy_limit |
| F2 under the workload whenever a mark exists, whatever its time | KILLED | copy_limit |
| F3 a mark in the second of the read: before the workload | KILLED | copy_limit |
| F4 the mark rewritten at every warm-up | SURVIVED | - |
| F5 the mark of an earlier run not removed at the build | SURVIVED | - |
| F6 the watcher's first line not written | KILLED | copy_limit |
| F7 the teardown's NONE line not written | KILLED | copy_limit |
| F8 the teardown writes NONE after a watcher that committed | SURVIVED | - |
| F9 the teardown's NONE line goes nowhere (no file set) | SURVIVED | - |
| R9 every upload row refused, the daemon alone too | KILLED | skz_store |
| R9b the check not called | KILLED | skz_store |
| R9c the check's refusal exits 0 | KILLED | skz_store |
| R9d the function lets everything through | KILLED | skz_store |
| T2 mismatches: the end value | KILLED | embedded |
| T11 abandoned at a mismatch: the end value | KILLED | embedded |

## CI at 6cbfc0b

Every run on 6cbfc0b has ended. I read them at 21:25 UTC. One job is red.

- **ci, e2e, coverage, kernel probe, nodeb rig, control rig, and
  skyzfs-ci (the push):** success.
- **The cut rig through the daemon, 44392321839 (the base's daemon,
  version 16):** success, with "worst exit 0" in all three jobs. On
  ubuntu-26.04 and ubuntu-26.04-arm:
  - `bound` reads PASS, "'1' abandoned at the bound";
  - `boundsparse` reads INFO, "the daemon keeps no holes (status version
    16, before 17 and M2.4e)", which is the new gate's text, as it should
    be on this daemon.

  The push run 44392308845 on ubuntu-26.04 reads the same.
- **The cut rig with the reference, 44392321945 and the push run
  44392308824:** success.
- **The replay rig, 44392321822:** success.
- **The daemon rig, 44392321857:** five jobs out of six have ended green.
  - On the daemon (ubuntu-24.04, 26.04 and 26.04-arm) and on the
    stand-in on 24.04, the judge passed. Every line that reads FAIL is a
    negative control (`ctl_*`), and on the daemon the one WAITING is the
    held-request pin, as at 72aaa06.
  - **The ubuntu-26.04 stand-in job (135113054631) failed with the
    judge's exit 2,** "the rig itself failed; this is not a verdict on
    the server". The rig step ran 49 minutes, against 36 for the same job
    at 72aaa06.
    - **What happened.** In check d, after the course "the device comes
      back with older content", `zpool clear` ran out of its 120 s and
      `zpool status` did not return in 60 s. Five zpool processes were
      waiting in the kernel: two in `txg_wait_synced`, one in
      `zfs_ioc_clear`, one in `spa_config_enter`, and one for the
      namespace lock. `txg_sync` was in state DR.
    - **What the rig did.** It recorded `zfs_stuck` and did not run the
      courses "d, scrub" and "d, not held". That is the M1.T32 handling,
      and the judge reads the missing lines as ERROR.
    - **The machine.** Kernel 7.0.0-1019-azure and ZFS 2.4.1, the same
      as at 72aaa06, where this job passed.
    - **Whether this pull request could have caused it.** The pull
      request changes `topology-bench.sh`, `cut_daemon_check.py` and four
      tests. The daemon rig uses none of them: neither `daemon-rig.sh`,
      `skyzfsd-ref.sh`, `daemon_check.py` nor its workflow reads them.
      It runs here only because a file under `tests/kernel/` and one
      under `tests/lib/` changed.
    - **For the lead.** This is the stall of M1.T25, "ZFS that made no
      progress after a suspension", which the notes record on arm64 (run
      43389048554). This one is on amd64 and with the stand-in, so it is
      not the daemon's. The waiting tasks' stacks are in the job's
      artifact (`zfs-stuck-stacks.txt`, artifact
      `daemon-rig-ubuntu-26.04-stand-in`).
    - **What to do.** Rerun that one job. Read the summary's table when
      it ends, not the colour.
- **Codex** has posted nothing since its one finding at f91bb6e, which
  was answered.

The base has moved (#122, #124 and the notes), with no file in common
with this pull request. The rigs on this head ran against the base as it
was when the pull request was pushed, as CLAUDE.md says.

## Can #123 merge at 6cbfc0b?

**Head: 6cbfc0b, read again with `gh api` at 21:25 UTC. Cannot merge
yet, and only because of one red CI job:** the daemon rig's
ubuntu-26.04 stand-in job, exit 2, ZFS stuck in the kernel in check d.
It is unrelated to this pull request (above). Rerun that job. When it is
green, #123 can merge: no finding of the review blocks it.

**Closed since 72aaa06:**

- F4: `boundsparse` now judges only what it measures.
- F5: the label says whether the seed committed before the workload or
  under it.
- G10, G11, G12, R9, T2, T11, and the balance's text.

From the first review, F1, F2 and F3 were already closed at 72aaa06.

**Open, none blocking:**

- N1: "under the workload" can be wrong by about one round, 5 s, at the
  border. The number printed shows that the commit was at the border.
- N2: four plants survive on the watcher's mark and the teardown's NONE
  line (F4, F5, F8, F9). The code is right; the gaps are in the tests.
- N3: `bound`'s line prints the copies read after the let-go, and
  `ok_cp` is unused.
- Left to later packages by the lead's word:
  - a status read while the first cut is held;
  - `bound`'s premise read from `status-start-1.txt`;
  - a bench row with the load before the seed.
- G4, which is the lead's decision.

**For #125.** The rig is the same as at 72aaa06 except for
`boundsparse`'s judge. On #125's daemon (version 17, holes `dropped`,
8192 chunks, 0 abandoned), the new judge gives PASS, and `bound` still
gives PASS with '1' abandoned, from the real status files of
44378510338. So the rerun of #125's cut rig on the merge should be green,
and `bound` still drives an abandonment at the bound.

## Appendix: the scripts

`realstatus.py TREE NAME STATUS...` judges the `cuts_abandoned` line of
NAME with TREE's judge. It uses the unit test's held fixture with each
real `status.txt`, which is parsed only. Run it with `python3 -I`, from
a directory of your own.

```python
"""realstatus.py TREE NAME STATUS...: judge NAME's cuts_abandoned line with TREE's judge,
on the test's held fixture but with each real status.txt (untrusted data, parsed only)."""
import sys
tree, name, *paths = sys.argv[1:]
sys.path.insert(0, f'{tree}/tests/kernel')
import test_cut_daemon_check as T, cut_daemon_check as cdc
for p in paths:
    st = cdc.read_status(open(p).read())
    case = T.HeldLines('test_a_right_daemon')
    if hasattr(T.HeldLines, 'setUpClass'):
        T.HeldLines.setUpClass()
    case.setUp()
    rc, lines = case.lines(name=name, status=st)
    print(name, {k: st.get(k) for k in ('status_version', 'cut_holes', 'cut_hole_chunks', 'cuts_abandoned', 'cut_copied_bytes')})
    print('  ->', rc, lines['cuts_abandoned'][0], lines['cuts_abandoned'][1][:200])
```

`plants4.py COPY [FIRST [LAST]]`:

```python
"""plants4.py COPY [FIRST [LAST]]: the plants of the next look at 6cbfc0b, one at a time in
COPY (a copy of the head's tests and workflows). A plant of the bench script is followed by
test_copy_limit.sh, test_embedded_python.py and tests/lib/test_skz_store.sh; a plant of the
cut judge by test_cut_daemon_check.py. KILLED: a test failed. SURVIVED: none did. NOT
APPLIED: the text was not found exactly once. The file is restored after each plant.
FIRST and LAST (0-based, LAST excluded) run a part of the list."""
import subprocess, sys
copy = sys.argv[1]
SH, CK = 'tests/bench/topology-bench.sh', 'tests/kernel/cut_daemon_check.py'
TESTS = {SH: {'copy_limit': ['bash', 'tests/bench/test_copy_limit.sh'],
              'embedded': ['python3', 'tests/bench/test_embedded_python.py'],
              'skz_store': ['bash', 'tests/lib/test_skz_store.sh']},
         CK: {'cut_check': ['python3', 'tests/kernel/test_cut_daemon_check.py']}}
def run(cmd):
    try:
        return subprocess.run(cmd, cwd=copy, capture_output=True, text=True, timeout=900).returncode
    except subprocess.TimeoutExpired:
        return 'TIMEOUT'
MARK = '    if [ -n "$SKZ_SEED_WATCH" ] && [ ! -e "$SKZ_SEED_BEGAN" ]; then date +%s > "$SKZ_SEED_BEGAN"; fi\n'
WARM = ('    log "$tag: warm-up ${WARMUP}s"\n'
        '    as_pg "\'$PGBENCH\' -h \'$SOCK\' -T $WARMUP -c $CLIENTS -j $JOBS -b $builtin bench" >/dev/null 2>&1\n')
ERR = ("put('cuts_abandoned', 'ERROR', f'status version {ver} and no cut_holes in the status: the rig cannot tell '\n"
       "                'whether the daemon kept the holes')\n            errored = True")
GATE = 'if not (ver.isdigit() and int(ver) >= 17):'
P = [
 # F4 and G11: the cut judge's boundsparse line
 ('G1 boundsparse gated on the key again, not the version', CK, GATE, 'if holes is None:'),
 ('G2 the gate at 18', CK, GATE, 'if not (ver.isdigit() and int(ver) >= 18):'),
 ('G3 the gate at 16 (a daemon before M2.4e judged)', CK, GATE, 'if not (ver.isdigit() and int(ver) >= 16):'),
 ('G4 version 17 without cut_holes: INFO, not the rig\'s error', CK, ERR,
  ERR.replace("'ERROR'", "'INFO'").replace('errored = True', 'pass')),
 ('G5 the abandoned not asked', CK, 'ok_ab = ab is not None and ab.isdigit() and int(ab) == 0', 'ok_ab = True'),
 ('G6 a FAIL with holes none does not say the daemon took none', CK,
  "why = '' if ok_ab or holes != 'none' else ': the daemon took no holes on this volume'", "why = ''"),
 ('G7 a version 17 daemon that took no holes (none) asked nothing', CK, GATE,
  "if not (ver.isdigit() and int(ver) >= 17) or holes == 'none':"),
 ('G8 INFO unless the set still holds at the end', CK, GATE,
  "if not (ver.isdigit() and int(ver) >= 17) or holes != 'held':"),
 ('G9 the version read from a key the status does not have', CK,
  "ver = status.get('status_version', '')", "ver = status.get('version', '')"),
 # F5 and G10: the seed's watcher and the mark of the workload
 ('F1 the mark written after the first warm-up', SH, MARK + WARM, WARM + MARK),
 ('F2 under the workload whenever a mark exists, whatever its time', SH,
  'if [[ $began =~ ^[0-9]+$ ]] && [ "$now" -ge "$began" ]; then', 'if [[ $began =~ ^[0-9]+$ ]]; then'),
 ('F3 a mark in the second of the read: before the workload', SH,
  'if [[ $began =~ ^[0-9]+$ ]] && [ "$now" -ge "$began" ]; then',
  'if [[ $began =~ ^[0-9]+$ ]] && [ "$now" -gt "$began" ]; then'),
 ('F4 the mark rewritten at every warm-up', SH, MARK,
  '    if [ -n "$SKZ_SEED_WATCH" ]; then date +%s > "$SKZ_SEED_BEGAN"; fi\n'),
 ('F5 the mark of an earlier run not removed at the build', SH, '        rm -f "$SKZ_SEED_BEGAN"\n', ''),
 ('F6 the watcher\'s first line not written', SH,
  'echo "first_upload not committed by the window\'s end (watched from the import)" >> "$OUT/$TOPO.skzinfo"', ':'),
 ('F7 the teardown\'s NONE line not written', SH,
  'echo "first_upload NONE by the end of the rows ($(( $(date +%s) - SKZ_SEED_T0 )) s after the import)" >> "$SKZ_SEED_INFO"',
  ':'),
 ('F8 the teardown writes NONE after a watcher that committed', SH,
  'if kill -0 "$SKZ_SEED_WATCH" 2>/dev/null; then', 'if true; then'),
 ('F9 the teardown\'s NONE line goes nowhere (no file set)', SH,
  'SKZ_SEED_T0=$(date +%s); SKZ_SEED_INFO="$OUT/$TOPO.skzinfo"', 'SKZ_SEED_T0=$(date +%s)'),
 # R9: the check after the loop
 ('R9 every upload row refused, the daemon alone too', SH,
  'if [ "$1" = 1 ] && [ "${topos[*]}" != zfs-skyzfsd ]; then', 'if [ "$1" = 1 ]; then'),
 ('R9b the check not called', SH, 'skz_upload_topologies "$SKZ_UPLOAD" "$TOPOLOGIES" || exit 64', 'true'),
 ('R9c the check\'s refusal exits 0', SH, 'skz_upload_topologies "$SKZ_UPLOAD" "$TOPOLOGIES" || exit 64',
  'skz_upload_topologies "$SKZ_UPLOAD" "$TOPOLOGIES" || exit 0'),
 ('R9d the function lets everything through', SH,
  'echo "TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: $2" >&2; return 1',
  'echo "TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: $2" >&2; return 0'),
 # T2 and T11: the fixture's counters
 ('T2 mismatches: the end value', SH, "{d('upload_mismatches')} | {d('upload_errors')}",
  "{sa.get('upload_mismatches', 'n/a')} | {d('upload_errors')}"),
 ('T11 abandoned at a mismatch: the end value', SH, "({d('upload_abandoned_at_mismatch')})",
  "({sa.get('upload_abandoned_at_mismatch', 'n/a')})"),
]
first = int(sys.argv[2]) if len(sys.argv) > 2 else 0
last = int(sys.argv[3]) if len(sys.argv) > 3 else len(P)
todo = P[first:last]
base = {k: run(c) for f in {p for _, p, _, _ in todo} for k, c in TESTS[f].items()}
assert all(v == 0 for v in base.values()), base
for name, path, old, new in todo:
    full = f'{copy}/{path}'
    text = open(full).read()
    if text.count(old) != 1:
        print(f'{name}: NOT APPLIED ({text.count(old)} matches)', flush=True)
        continue
    try:
        open(full, 'w').write(text.replace(old, new))
        r = {k: run(c) for k, c in TESTS[path].items()}
    finally:
        open(full, 'w').write(text)
    died = [k for k, v in r.items() if v != 0]
    print(f"{name}: {'KILLED by ' + ', '.join(died) if died else 'SURVIVED'}", flush=True)
```
