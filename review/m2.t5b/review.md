# Review of #132 (M2.T5b): the seed let go at its window, one reading of a daemon's pid, and the seed judged inside its window

Pull request: skyzfs/skyzfs#132 (`skyzfs-testing` into `zfs-topology`).
**Heads reviewed: 861b537** (one commit on `dbb1d11`) **and 27fe831**
(one more, which carries the lead's decision on window rows: +115/-2 in
`bracket.py`, `test_bracket.py` and `topology-bench.sh`). I read the
diff and formed a view from the code before the description, then
checked the description against run 44462902930's artifacts and logs.

The pull request changes only test code: `tests/bench/bracket.py`,
`topology-bench.sh` and their tests (`test_bracket.py`,
`test_copy_limit.sh`, `test_embedded_python.py`), and
`tests/lib/test_skz_store.sh`. `zfs-topology` has moved since the pull
request's base. It now holds #129 (M2.4f, merged before the bench run)
and #131 (F5, merged at 06:21). Neither touches a file of this pull
request. GitHub reads the pull request as mergeable.

## Verdict

**No finding in the code blocks the merge, at either head.** The window
mode, the pid reading and the seed rule do what they claim. Plants show
that their tests catch the behaviour, with the gaps listed below. One
thing must change before "ready to merge", and it is text only:

- **The description is stale at 27fe831 (R1).** It still gives the
  commit range `dbb1d11..861b537` and says "I have not changed the
  rule". Its table still gives sw's tpcb row NO VERDICT. At 27fe831 the
  same run renders that row as `BELOW 0.90 (seed inside window, lease
  at release (epoch 1))`. The description is the report, so it must
  say what the head does.

These remain open but do not block:

- **Codex's one finding at 27fe831** (P2, `bracket.py:162`) has no
  answer on its thread yet. I checked it against the code. It is right
  about the marker, and it is my D1. In every case the bench can reach,
  the verdict is the plain rule's; only the words "seed inside window,
  lease at release" are wrong. The fix is cheap, but it narrows the
  lead's rule, so it is **a decision for the lead** (below).
- **CI at 27fe831 is green:** all ten pull request runs, among them the
  daemon rig on three images and the stand-in on two, the replay rig,
  the cut rig and the cut daemon (see "CI and Codex"). Most runs at
  861b537 were cancelled by the push of 27fe831. The control rig and
  the node B rig were green there.
- **Low defects** (D1 to D5), **description items** R2 and R3, and
  **test gaps** (six surviving plants that are not equivalent). None
  of them changes a figure of run 44462902930 or a verdict that the
  bench can reach.

## For the lead

1. **The decision on window rows (question 3) is sound in principle.**
   A window that holds the whole seed, from the release to its commit,
   measures what the mode was built to measure. G4 (c) asked for "the
   lease held at both ends" to exclude windows of a daemon that was not
   uploading. A window that starts with no lease because MinIO was
   frozen, and ends with the seed committed under one lease, is not
   such a window. As built, the rule is right inside the release
   window (see (a) to (c) below).
2. **D1 / Codex's P2 needs a choice.** The rule as relayed accepts a
   start lease that is "held under the end's epoch". Inside the release
   window that cannot happen: MinIO is frozen until after the start's
   read, and the daemon takes the lease only from the store. So that
   branch admits only later windows, where the seed's tail commits and
   the marker is wrong. The options:
   - (i) accept only `none` at the start, a one-line change, after
     which a later window falls to the plain rule with the same verdict
     and no marker;
   - (ii) also record in the row that MinIO was let go in this window
     (Codex's suggestion), and require it;
   - (iii) keep the rule and fix only the marker's words.

   I recommend (i), with (ii) if the bench will keep rows that seed for
   longer than a window, for example `--zero-fill` with `=window`.
3. **Whether a seed row counts toward the median of design §9.** The
   module's docstring says that a row with no verdict "gives nothing to
   a median of verdicts". A seed row now has one, and it is a one-off
   cost rather than the steady state. The marker makes such rows
   recognisable. Whether they enter the median of the M1 throughput
   criterion or stand as a figure of their own is the lead's to say.
4. **R2 and R3 concern the notes:**
   - **R2:** sp's label in the description and in the notes commit
     5bca2b0, "the seed at the workload's start", is false. sp's seed
     committed 16 s after the import, before the workload.
   - **R3:** run 44462902930's daemon predates M2.4f (status version
     18). It did not lose a cut, so the M2.4f wait that follows a lost
     whole cut never arose, but the run is not of the daemon now on
     `zfs-topology`.

## The questions

### (1) `TB_SKZ_UPLOAD_FROM_LOAD=window`: right

- **Where MinIO is let go.** In `run_workload` (topology-bench.sh, from
  line 2795 at 27fe831):
  - the mark is set (2799);
  - `[ = window ] || skz_minio_resume` skips the release at the first
    warm-up (2800);
  - the warm-up runs, then `sync; sleep 2`, then the window's snapshot;
  - `snapshot "$OUT/$tag.before"` (2810) reads the status;
  - the sampler and the traces start;
  - `[ != window ] || skz_minio_resume` releases MinIO (2836), just
    before the "measuring" log and pgbench.

  All of it runs in sequence in the main shell, so the status read at
  the start comes before the release. In the run, "MinIO answers
  again" is at 05:38:07.0715 and "measuring" at 05:38:07.0777: the
  window starts within 6 ms of the release. At a later window
  `skz_minio_resume` returns at once (`SKZ_MINIO_PAUSED` is empty). The
  order is pinned by `test_copy_limit.sh`, and plants W1 to W4 are
  killed.
- **The watcher's clock.** `date +%s` in both shells, in whole seconds.
  The release time is written before the unpause (1360), so it is a
  lower bound on the release. A failed unpause removes it (1365). The
  watcher's interval is `max(prev, rel)` to `now`, where `prev` is the
  start of the last read that saw no commit and `now` is the end of the
  read that saw one (2016-2018). The commit lies in that interval. In
  the run the line reads "10 to 15 s after MinIO was let go at the
  first window's start", and it matches the log's times (release
  05:38:07, commit seen 05:38:21).
- **Races.** None. The watcher reads the release time only after it
  sees a commit. A commit needs MinIO, and the time is written before
  the unpause. A last row's stale time is removed at the build (2295),
  and plant W8 is killed.
- **Teardown.** `teardown_topology` unpauses a frozen MinIO (1404). It
  is called from `teardown_all`, which runs from `trap teardown_all
  EXIT` (1502). The script has no other trap. Bash runs an EXIT trap
  when a fatal signal ends it; I checked that here for TERM. So the
  ways out are the same as on #128. `TB_KEEP` on a failure and SIGKILL
  are the exceptions, and `minio.sh down` covers both.
- **Refusals** (1076-1081). A value other than 1, `window` or unset is
  refused, and so are `window` without `TB_SKZ_UPLOAD=1` and `window`
  with a wait over 0. All are tested in `test_skz_store.sh`, and W15
  and W16 are killed. `=1` is unchanged: the guard at 2800 still
  releases at the mark, and the release time is written only for
  `window` (W5 killed).
- **Small items:** D4 and D5 below, both nits.

### (2) `bracket.one_server`: right, with one text left (D2)

`pid_table.py` writes each of 30 combinations of readings at the two
ends as `snapshot` writes them:

- start readings: missing, failed (clock line only), gone (status read,
  no `pid`, "left by a daemon that is gone"), pid 7, pid 0;
- end readings: the same, plus pid 8.

It runs each through the row here-doc and `summarize`, and compares
the three consumers (the status column, the uploader table's row name,
the intent table's row name) with the verdict.

- **At both heads the three consumers agree with the verdict in all
  30 cases.** "status not read" appears exactly where the verdict says
  "was not read, or showed no serving daemon". "NO: the pid changed" /
  "TWO DAEMONS" appears exactly where both pids were read and differ.
  "yes" / plain appears exactly where they are the same.
- **At the base, 24 of the 30 read differently, and each of them
  misled:**
  - "NO: the pid changed" where an end was not read (missing-missing,
    7-missing, failed-missing, gone-missing, 0-missing);
  - "yes" and a plain row name where reads failed or the daemon was
    gone (missing-failed, missing-gone, failed-failed, failed-gone,
    gone-failed, gone-gone);
  - "TWO DAEMONS" where the start was missing, failed or gone and the
    end was read (missing-7, failed-8, gone-0 and the rest), and where
    an end failed or showed a dead daemon (7-failed, 7-gone).
- **No verdict changed** between the base and either head: only
  `not_uploading`'s wording (L2).
- **What still misleads (D2):** a daemon that died in the window (7-gone,
  gone-7) is printed as "status not read" and "(its status not read at
  an end of the window)" in all three tables. The status was read, and
  it said the daemon was gone. The verdict's "or showed no serving
  daemon" is right. The comment at topology-bench.sh:710-711 names "a
  window whose daemon died" as the first case these labels exist for.
- `pid 0` reads as a pid. `status.rs` prints `pid` only for a live
  device of ours, so it cannot happen.

### (3) The window row's verdict, and the lead's rule at 27fe831

At 861b537 sw's tpcb row got NO VERDICT ("at its start none at epoch 0,
at its end held at epoch 1"). That followed the letter of G4 (c). The
lead decided to judge such a row. As relayed, it is judged when:

- its status at the window's start shows `uploaded` 0;
- it shows `uploaded_whole` ≥ 1 at the end;
- the lease is held or unsure at the end with a numeric epoch, and at
  the start's epoch if it was held at the start too.

The verdict gets "(seed inside window, lease at release (epoch N))".
The diff implements exactly that: `seed_in_window` (bracket.py:148-173)
is called from `not_uploading` (129) after the status and pid checks,
and from `bracketed` (240) for the marker.

#### (a) Do "no upload at the start" and "a whole one at the end" show the commit fell inside the window?

**Yes, for one daemon instance, which the rule requires (both pids read
and `pid_same`).** How each case comes out, against `status.rs`:

- **Counters reset by a restart.** `uploaded` and `uploaded_whole`
  count from "this daemon started serving" and reset only at a start.
  A start inside the window changes the pid, and the row is not judged.
  If a reused pid let a restart through, the end's `uploaded_whole` ≥ 1
  would still be a whole commit by the new instance inside the window.
- **A whole cut uploaded by an earlier daemon** is counted as
  `upload_found_committed`, not as `uploaded` or `uploaded_whole`. So
  `uploaded` 0 at the start and `uploaded_whole` ≥ 1 at the end is a
  whole commit by this instance between the two reads.
- **Counted across epochs.** An instance takes the lease at most once:
  `lost` is final for an instance (upload_state 2), and `unsure` is
  held with the same epoch. Every commit of the instance is under one
  epoch.
- **The start's read before or after the release.** Before, always:
  the sequence in (1), pinned by the order test (W2 killed). The rule
  itself does not check the release time. It relies on the bench's
  order and on the `from_load` marker in the row. See D1 for what that
  leaves open.
- **An epoch that changed between the reads**, under a lease held at
  the start: refused (`start[1] != end[1]`, plants S7 and S8 killed).
  From `none` or `lost` at the start, the end's epoch is taken. `lost`
  at the start and held at the end cannot happen within one instance,
  but the rule would accept it (hardening, below).
- **`unsure` at the end.** It is held (the last renewal failed, and
  uploads go on under its epoch), and the plain rule's `LEASE_HELD`
  treats it the same way. Accepted (S9 killed).
- **The end's read is a few seconds after pgbench** (`sync; sleep 2`
  and the pressure look), so a commit in that tail counts as inside.
  The plain rule has the same tail; it does not matter.

What the rule shows, then: a whole commit by this daemon between the
window's two reads, with no upload before the first. What it does not
show by itself: that MinIO was let go at this window's start (D1).

#### (b) A lease held at the start under another epoch (a take-over in the window)

Refused, which is right. Within one instance it cannot happen. A
take-over by another daemon changes the pid, and that row is refused
earlier. Rendered: "it did not hold the lease for the whole window (at
its start held at epoch 1, at its end held at epoch 2); its seed, let
go at the window's start, was not shown to commit inside it under the
lease held at its end (uploaded 0 at its start, 1 whole at its end)".

#### (c) My own table test: `diff_seed.py`, 1,750,000 rows, 0 disagreements

`diff_seed.py NEW OLD` judges every row of a cartesian table with the
new head's `bracketed` and with 861b537's. The table varies:

- `from_load`: window, workload, the old JSON's True, missing, or no
  upload at all;
- seven pid combinations;
- the lease and its epoch at each end;
- `uploaded` at the start and at the end;
- `uploaded_whole` at the end.

Each row is judged between two model rows.

- **Every non-window row** (1,400,000) gets 861b537's verdict exactly.
  Plain and `=1` rows are unchanged.
- **Every window row** is judged with the marker exactly where an
  oracle of the relayed rule says so. Otherwise it gets 861b537's
  verdict, with the seed's clause added only when the reason is the
  lease.
- **A sample of the accepted rows** (1 in 11) was judged again in a
  bracket 26% wide and with no bracket at all. Each got NO VERDICT and
  no marker.
- **Accepted rows whose start is not `none`:**
  - start `lost` (240) and start lease missing (240): unreachable;
  - start held or unsure at the end's epoch (48 each): only a later
    window, D1;
  - end epoch 0 with a held lease (272): unreachable (`lease_epoch` is
    0 only "where not known").
- **Named edges** (the script prints them):
  - sw as the run had it is judged;
  - "the seed went on into a second window" (start held at epoch 1,
    `uploaded` 0) is judged with the marker (D1);
  - "uploaded_whole 1 with uploaded 0" and "uploaded not in the status"
    are judged, though neither can come from `status.rs`;
  - an epoch or a whole count given as a JSON int works.

**The run re-rendered at 27fe831.** Run 44462902930's rows through the
new head's `summarize`: only sw's tpcb verdict changes, from NO VERDICT
to `BELOW 0.90 (seed inside window, lease at release (epoch 1))`
(0.870), plus the summary's paragraph. sw's select row stays `BELOW
0.90` (0.839) and sp's tpcb row `at least 0.90` (0.902). No consumer
matches a verdict string exactly (grep), so the suffix breaks nothing.

## Defects (none blocks the merge)

### D1. The marker is given in a later window too (low to medium, text; Codex's P2)

`bracket.py:169-170` accepts a start lease "held at the end's epoch",
and `test_bracket.py:305` makes that a requirement ("A lease already
held at the start under the end's epoch is the same lease").
**Scenario:**

1. A topology with workloads tpcb then select runs with `=window`.
2. The seed outlasts tpcb's window. This is likely with a
   `--zero-fill` volume, whose idle seed took 227 s in this run, or
   with a seed lost once under M2.4f, which then waits a minute.
3. tpcb's window gets NO VERDICT ("not shown to commit inside it"),
   which is right.
4. select's window starts held at epoch 1 with `uploaded` 0 and ends
   with `uploaded_whole` 1. It gets "(seed inside window, lease at
   release (epoch 1))".

Its window held only the seed's tail, and MinIO was let go a window
earlier. A reader who takes marked rows as "what a seed costs" reads a
partial cost as the whole. The verdict itself is the one the plain rule
gives (held at both ends, uploads grew). Diff_seed found no reachable
row whose verdict differs. **Certainty: high** (named edge in
`diff_seed.py`, and Codex reached the same case). The options are under
"For the lead", item 2.

### D2. A daemon that died in the window is labelled "status not read" (low, text)

`bracket.py:101-102` returns 'status not read' for any end with no pid.
`topology-bench.sh:713-715` and `:971` print it as "status not read"
and "(its status not read at an end of the window)". Cases 7-gone and
gone-7 in `pid_table.py` show it. The status was read, and it said
`counters left_by_a_daemon_that_is_gone,_pid_N`. **Fix:** tell a
reading with fields but no pid apart from one with only `read_at_ns`,
for example "no serving daemon at an end". **Certainty: high.**

### D3. The paragraph and the docstrings state the rule loosely (low, text)

- `topology-bench.sh:931-934` says the lease is "taken at the release,
  whatever the lease at its start".
- The module docstring (`bracket.py:29-33`) says "at any lease at its
  start".
- `seed_in_window`'s docstring says "seed was let go at its window's
  start".

A start held under another epoch is refused, so "whatever" and "any"
are wrong. The rule does not check the release, so "taken at the
release" is an inference from the bench's order, and after D1 not even
that. **Certainty: high.**

### D4. A release tried again names the wrong window (nit)

If the first window's unpause fails and the second's succeeds, the
release time is written again at the second (1360, then 2836 on the
next `run_workload`). The watcher still says "after MinIO was let go
at the first window's start" (2018).

### D5. A commit seen before the release time drops the clause silently (nit)

`[ "$now" -ge "$rel" ]` (2017) can only fail if the freeze did not
hold. The line then says nothing of the release, rather than that the
seed committed before it. Plant W13 survives; no case has
`REL > now`.

**Hardening (optional), all cases that cannot happen today:** accept
only `none` at the start, which also covers D1, a start `lost`, and a
start lease missing; ask for a numeric epoch above 0 at the end; ask
that `uploaded` at the end be at least `uploaded_whole`.

## The description against the code and run 44462902930

Every figure the description cites matches the run's artifacts and
logs:

- the six tps values and the model rows;
- the ratios (0.895, 0.870, 0.902, 0.846, 0.839, 0.858) and the bracket
  widths (0.1% to 1.1%);
- sw's NO VERDICT reason at 861b537;
- sw's seed 10 to 15 s after the release and 43 to 48 s after the
  warm-up;
- blocks 969,410 against 571,214 and zero blocks 8,127,964 against 0;
- the upload thread at 5.6/1.7 s against 3.3/1.0 s;
- 27.8 against 22.9 KiB per transaction;
- zi's seed at 227 s (`uploaded 2, cuts lost 0`) and sp's at 16 s.

The run was one `sequence` job on one boot id, as the description says.

- **R1 (must be fixed before "ready to merge"):** at 27fe831 the
  description is stale:
  - the commit range is `dbb1d11..861b537`;
  - it says "I have not changed the rule";
  - the table and "Not done" still give sw's tpcb row NO VERDICT and
    call the window's verdict an open question;
  - it says nothing of `seed_in_window`, its tests or its plants.
- **R2 (low):** "`sp` is sparse with the seed when the workload begins
  (`WAIT_S=0`)". sp had no `TB_SKZ_UPLOAD_FROM_LOAD`, and its seed
  committed 16 s after the import, before the workload began. The
  description says so itself a few lines later. sp's windows hold no
  part of the seed, as with fl and sp on #128 (R3 there). The notes
  commit 5bca2b0 repeats the label.
- **R3 (low, for the notes):**
  - The run's daemon was built at 861b537, whose base `dbb1d11` is
    before #129 (M2.4f) merged. Its status reads `status_version 18`.
  - sw lost no cut (`upload_cuts_lost 0`), so the M2.4f wait after a
    lost whole cut would not have arisen. The figures likely hold, but
    a run on `zfs-topology` after the merge is the one of the uploader
    as built.
  - The seed's cost, about 3.5% of tps (0.870 against sp's 0.902),
    rests on one run per row, as the description says.
- **The CI the description cites** (skyzfs-ci 44462889643 and control
  rig 44462889645) is push runs on 861b537, both green. They are not
  runs of the pull request.

## Test gaps (surviving plants; none blocks)

| Plant | What it changes | Why it survives | Severity |
|---|---|---|---|
| W7 | release time written after the unpause | no test sees the order of the write and the unpause | low |
| W13 | `now >= rel` dropped | no case with `REL > now` (D5) | nit |
| O7 | status column prints `n/a` when the start has no pid | the embedded test's no-pid case is at the end only | low |
| S17 | a seed row judged in a bracket too wide | no seed case in a wide or missing bracket (`diff_seed.py` catches it: 74 rows) | low |
| S19 | the summary paragraph says nothing of the seed rule | text | nit |

Equivalent mutants:

- **W14** (`[ -n "$rel" ]` for the number test): the `-ge` test after
  it fails on a value that is not a number.
- **S10** (the pid check in `seed_in_window` dropped) and **S11**
  (the seed check moved before the status and pid checks):
  `not_uploading` checks the pids before `seed_in_window` is reached,
  and `seed_in_window` checks them itself. Either one alone is
  equivalent; both together would not be.

The author reports that six mutants each fail a test. I planted 19 on
the seed rule (S1 to S19): 15 killed, S10 and S11 equivalent, S17 and
S19 survived.

## Codex and CI

**Codex.**

- At 861b537: no finding posted. The pull request opened at 05:57,
  and the push of 27fe831 came at 06:04.
- At 27fe831: one P2, "Restrict seed-at-release classification to the
  release window" (`bracket.py:162`, review 5970146126, comment
  4624979778). It is valid and is D1. I checked it against the code:
  - verdicts reachable today are unchanged, and only the marker is
    wrong;
  - "the relaxed lease criterion outside the release window" applies
    only when a later window starts with `none` (a late lease), whose
    seed then lies wholly inside that window anyway.

  Not answered on its thread when I read it.

**CI at 861b537** (pull request runs from 05:57):

- control rig and node B rig: green;
- ci, coverage, e2e, kernel probe, daemon rig, replay rig, cut rig and
  cut daemon: cancelled at 06:04 by the push of 27fe831.

**CI at 27fe831** (pull request runs from 06:04; all ended by 06:52,
all green):

- ci 44469506544: changes, lint, s3-backends, chart-lint, test and
  unit-arm64;
- coverage 44469506392, e2e 44469506690, kernel probe 44469506641,
  control rig 44469506374 and node B rig 44469506626;
- daemon rig 44469506429: the daemon on ubuntu-24.04, 26.04 and
  26.04-arm, the stand-in on 24.04 and 26.04, and the table and its
  judge;
- replay rig 44469506418: both reference layouts, and through skyzfsd
  and "write into a hole" on 24.04 and 26.04;
- cut rig 44469506945: four images and the reference;
- cut daemon 44469506753: 26.04, 26.04-arm and 26.04-upload.

The push run skyzfs-ci 44469498825 skipped its jobs, because a pull
request is open. I read the jobs' conclusions, not the rigs' summary
tables; the lead reads those. The rigs run because
`tests/lib/test_skz_store.sh` is under `tests/lib/`, but the pull
request changes no file that they drive.

## Local gates

| Check | 861b537 | 27fe831 |
|---|---|---|
| shellcheck 0.11.0 on every tracked `*.sh` | clean | clean |
| `test_bracket.py` | 22 OK | 25 OK |
| `test_embedded_python.py` | 7 OK | 7 OK |
| `test_skz_time.py`, `test_cut_release.py`, `test_syscalls.py`, `test_perfcat.py` | OK (39, 13, 6, 6) | OK (39, 13, 6, 6) |
| `test_copy_limit.sh` | 107 ok | 107 ok |
| `tests/lib/test_skz_store.sh` (as root) | 109 ok | 109 ok |

The system's shellcheck 0.9.0 reports false SC2317 warnings on code
behind traps. 0.11.0 is clean.

## Plants at 27fe831

`plants132.py` applies each plant in turn to a worktree at 27fe831 and
restores it in a `finally`. A plant of `topology-bench.sh` runs
`test_copy_limit.sh`, `test_embedded_python.py` and `test_skz_store.sh`;
a plant of `bracket.py` runs `test_bracket.py` and
`test_embedded_python.py`. Each test has 600 s. 45 plants, all applied:
**37 killed, 3 equivalent (W14, S10, S11), 5 survived (W7, W13, O7, S17,
S19).** The tree was clean afterwards.

| Plant | Result |
|---|---|
| W1 `=window` let go at the first warm-up as well | KILLED (copy_limit) |
| W2 let go before the window's first status read | KILLED (copy_limit) |
| W3 never let go in `run_workload` | KILLED (copy_limit) |
| W4 let go after the window's pgbench | KILLED (copy_limit) |
| W5 release time written for `=1` too | KILLED (copy_limit) |
| W6 release time kept when the unpause fails | KILLED (copy_limit) |
| W7 release time written after the unpause | SURVIVED |
| W8 last row's release time not removed at the build | KILLED (copy_limit) |
| W9 watcher reads the release time without `watch` | KILLED (copy_limit) |
| W10 lower end not floored at 0 | KILLED (copy_limit) |
| W11 lower end always 0 | KILLED (copy_limit) |
| W12 upper end from the read before | KILLED (copy_limit) |
| W13 `now >= rel` dropped | SURVIVED |
| W14 a release time that is not a number taken | SURVIVED (equivalent) |
| W15 `=window` refused | KILLED (skz_store) |
| W16 `=window` lets a wait over 0 through | KILLED (skz_store) |
| W17 the record says 1 for `=window` | KILLED (copy_limit) |
| W18 the row reads window as the workload mode | KILLED (embedded) |
| W19 the log names the workload for `=window` | KILLED (copy_limit) |
| O1 `one_server` reads `pid_same` | KILLED (bracket, embedded) |
| O2 "status not read" only when both ends are | KILLED (bracket, embedded) |
| O3 "status not read" only at the end | KILLED (bracket) |
| O4 uploader table prints "status not read" as one daemon | KILLED (embedded) |
| O5 uploader table prints it as two daemons | KILLED (embedded) |
| O6 status column back to `pid_same` | KILLED (embedded) |
| O7 status column `n/a` for a start with no pid | SURVIVED |
| S1 every upload row gets the seed rule | KILLED (bracket) |
| S2 the start's `uploaded` not asked | KILLED (bracket) |
| S3 a whole upload at the end not asked | KILLED (bracket) |
| S4 whole uploads counted from 0 | KILLED (bracket) |
| S5 the end's lease not asked | KILLED (bracket) |
| S6 the end's epoch not asked to be a number | KILLED (bracket) |
| S7 a start held under another epoch taken | KILLED (bracket) |
| S8 the epochs compared at any start lease | KILLED (bracket) |
| S9 `unsure` at the end not taken | KILLED (bracket) |
| S10 the seed rule ignores the pids | SURVIVED (equivalent) |
| S11 the seed rule asked before the status and pid checks | SURVIVED (equivalent) |
| S12 the marker not printed | KILLED (bracket) |
| S13 the seed's reason not added | KILLED (bracket) |
| S14 the seed's reason for every row | KILLED (bracket) |
| S15 a seed row never judged | KILLED (bracket) |
| S16 the marker names the start's epoch | KILLED (bracket) |
| S17 a seed row judged in a bracket too wide | SURVIVED (`diff_seed.py` kills it) |
| S18 the start's whole uploads asked in place of its uploads | KILLED (bracket) |
| S19 the paragraph says nothing of the seed rule | SURVIVED |

## Appendix: the scripts (in this directory)

Run each with `python3 -I`, from a directory of its own:

- **`pid_table.py TREE`**: the 30 combinations of pid readings at the
  window's two ends, through TREE's row here-doc and `summarize`. For
  each it prints the three consumers and the verdict (question 2). Run
  at the base `zfs-topology`, at 861b537 and at 27fe831: the two heads
  give the same table.
- **`diff_seed.py NEW OLD`**: the seed rule at NEW (27fe831) against an
  oracle of the lead's rule, and every other row against OLD (861b537),
  over 1,750,000 rows, with named edges (question 3 (c)). It exits 1 on
  any disagreement. With S17 planted it reports 74.
- **`plants132.py TREE [FIRST [LAST]]`**: the 45 plants above. It needs
  root for `test_skz_store.sh`.

Their outputs, as they ran:

- `pid_base.out`: `zfs-topology` at 5bca2b0, whose bench files are
  `dbb1d11`'s;
- `pid_27fe831.out`: the same as at 861b537;
- `diff_seed.out`;
- `plants132.out`.
