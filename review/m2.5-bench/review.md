# Review of #123 at 405b7ed: TB_SKZ_UPLOAD on the benchmark, the uploader's and the intent map's tables

Pull request: skyzfs/skyzfs#123 (`skyzfs-testing` into `zfs-topology`).
**Head reviewed: 405b7ed.** The head moved since the request named
24e3df5: f91bb6e (the wait for the first upload, MinIO anew after each
upload row) and 405b7ed (the wait after the import) are on top. Base:
`origin/zfs-topology` at 4dafc03, merge base a3f3580. Reviewed
`git diff origin/zfs-topology...405b7ed`: five files, +359 −2
(`topology-bench.sh`, `minio.sh`, `topology-bench.yml`, and the two
bench tests). The code was read before the description. CI at 405b7ed:
ci, coverage, e2e and skyzfs-ci green. Bench runs read: 44358962767 (at
405b7ed, green), 44325181328 (at 24e3df5) and 44335231766 (at f91bb6e,
cancelled at the job's limit).

A previous reviewer's uncommitted notes for this pull request were
looked at and not relied on: every result below was made again here, at
405b7ed, with the scripts in this directory.

## Verdict

**Not yet.** Three findings block the merge (F1 to F3), each a small
change: a refused upload row ends the whole sequence (F1, which Codex
also reported), the abandoned column is described as abandons at a
mismatch while it counts the bound's (F2), and the refusals have no
test (F3). The delta arithmetic is right: I recomputed every figure of
both tables in all eight windows of run 44358962767 from the raw status
lines and they match, and the cuts balance in every window. The
refusals do what they say (28 cases, below; nothing legitimate is
refused). The wait for the first upload is, in my view, fair to the
model rows for the question the round asks (G1 says what it does not
measure). `minio.sh`'s `rm` is safe as called (G8). Of the new columns,
lost, first upload and named at end are pinned by the tests; the
abandoned, mismatches and errors columns and the intent map's end values
are not (G3).

## Blocking

### F1. A refused upload row ends the sequence, without its line in the summary (high certainty)

`.github/workflows/topology-bench.yml:444`:
`echo "$used" | sudo tee "$TB_OUT/$i/minio.txt" > /dev/null`.
Every refusal in `topology-bench.sh` exits 64 before
`mkdir -p "$OUT" "$WORK"` (`topology-bench.sh:1125`), so for an upload
row that is refused `$TB_OUT/$i` does not exist, `tee` fails, and the
step runs under `bash -e` (the job log of 44358962767:
`shell: /usr/bin/bash -e {0}`): the step ends there. The rows after it
do not run, the summary gets no "Sequence N failed with 64 (its
settings were refused before anything ran ...)" line, and MinIO is not
started anew. Before this pull request a refused row let the sequence
go on (the loop's own comment: "A row that fails does not end the
job").

Scenario, reproduced (`loop_sim.sh`: the workflow's own "Run benchmark"
step, read from the YAML, under `bash -e`, with sudo, the bench script
and minio.sh made up): sequence
`m1|zfs-slog; up1|zfs-skyzfsd TB_SKZ_UPLOAD=1 (refused); m2|zfs-slog; up2|zfs-skyzfsd TB_SKZ_UPLOAD=1`:
step exit 1 right after row 2, the log's last line
`tee: .../out/2/minio.txt: No such file or directory`, an empty
summary, rows 3 and 4 never ran. Controls: the same sequence with no
refusal runs all four rows and writes both MinIO lines; a refused row
without uploads gets its summary line and the rows after it run.

Every new refusal reaches this: uploads with the releaser, uploads in an
entry with no daemon, `TB_SKZ_UPLOAD_WAIT_S=abc`, and also the older
ones in an upload entry (a `--store` the build does not take, a missing
tool). Codex reported the same at f91bb6e (review thread 4616420855);
it has no answer yet and 405b7ed still has the line.

Fix: `sudo mkdir -p "$TB_OUT/$i"` before the `tee`, or write the file
only when the directory exists (the summary line can stay). With the
`mkdir` added to a copy of the workflow, the same simulation runs all
four rows, writes "Sequence 2 (up1) failed with 64 ..." and both MinIO
lines, and ends with 1 as a sequence with a failed row does.
`loop_sim.out` has the three runs.

### F2. The uploader's table calls the column "abandoned at a mismatch"; it counts every abandoned cut, most of them at the bound (high certainty)

`topology-bench.sh:684` (the table's text) lists "abandoned at a
mismatch", and the column is `d('cuts_abandoned')`
(`topology-bench.sh:700`). `cuts_abandoned` is the store's count: "the
cuts abandoned at the bound or let go at a copy that could not be
made" (`crates/skyzfs-ublk/src/status.rs:116`), and a mismatch's
abandons are added to it "each counted as at the bound"
(`crates/skyzfs-local/src/inplace/state.rs`, `abandon_all`), as is a
cut whose read failed (`abandon_uploading`). The uploader's own count of
the cuts abandoned at a mismatch is a different key,
`upload_abandoned_at_mismatch`.

Scenario, from a real run: in 44325181328 (24e3df5) this column read 10
or 11 in every tpcb window and 2 in every select window, with
`upload_mismatches` 0 and `upload_abandoned_at_mismatch` 0 over the same
windows (recomputed from the raw lines, `recompute.py`). They were
abandons at the cut bound, the finding the lead wrote into the notes on
2026-11-18. Under this text a reader takes them for mismatches. The next
round the description announces (`TB_SKZ_UPLOAD_WAIT_S=0`, the seed
under load after M2.4e) is the one most likely to show bound abandons
again.

Fix: say "abandoned (at the bound, at a copy or read that failed, or at
a mismatch)", or show `upload_abandoned_at_mismatch` beside it. While
there: the balance sentence (`:688`) leaves out
`upload_committed_gone` (status.rs's balance subtracts it), and the
table shows the cuts held at the end but not at the start, so "the
change in held" it names cannot be read from the table (in 44358962767
uploaded is 25 and cuts 24 in six windows; the difference is a held cut
at the start).

### F3. No test exercises any of the refusals (high certainty)

None of the five new checks (`TB_SKZ_UPLOAD` is 1 or unset, `:1017`;
`TB_SKZ_UPLOAD_WAIT_S` a number, `:1019`; the four settings, `:1092`;
not with the releaser, `:1094`; not without the daemon, `:1103`) is run
by a test of the pull request. Planted one at a time (R1 to R7: each
check disabled, one setting left out of the list, the no-daemon refusal
exiting 0), every plant survives `test_copy_limit.sh`,
`test_embedded_python.py` and `tests/lib/test_skz_store.sh`; each is
killed by `refusals.sh` (table below). With the no-daemon refusal gone
(R5) a zfs-slog row in an upload entry goes on to build its machine (the
probe's made-up tools stop it at the space check); on a runner it would
be measured as a model row under an upload entry's label. `tests/lib/test_skz_store.sh`
already runs the releaser's refusals this way (`bench_release`, made-up
tools, `no-such-topology` after the daemon), in CI, so this is about
fifteen lines there. CONTRIBUTING asks for the failing test first, and
the releaser's refusals got theirs.

## Not blocking: defects and gaps

### G1. The wait for the first upload: fair, with what it leaves out said (judgment, asked by testing)

`skz_first_upload` (`:1846`) runs in `build_zfs_direct` after the import,
`zfs_set_props` and `lim 120 zpool sync` (`:2115-2118`), before
`add_slog`, postgres's start, the warm-up and the window. Nothing it does
reaches into the window: the whole cut is read with direct I/O
(`serve --help`: the image is read and written by O_DIRECT), so it warms
no page cache the window reads; ZFS is idle and its ARC untouched; the
map's regions are cleared by the commit, so the window's map syncs are
those of the steady state (named at the end: at most 19 of 8192). The
model rows wait for nothing, so the time between two model rows grows by
about three minutes per upload row; the bracket's rule (both model rows
within 10% of their mean) still guards against drift, and the brackets
of 44358962767 are 0.3% to 3.6% wide.

So the rows answer "what do uploads and the header sync cost once a
volume is seeded", which is the round's question. They do not include:
the seed itself (it is in `first upload`, 178 to 179 s in every row);
a seed under load (the case M2.4e changes); and an object store that is
not on the runner. The last is worth a sentence in the description and
the notes beside the ratios: MinIO writes its data to the same work disk
and runs on the same CPUs as postgres and the daemon (about 5.1 GiB per
upload row in 44358962767), which a node uploading to remote storage
does not carry, so the upload rows are pessimistic in that respect.

### G2. The intent map's table shows rows whose daemon keeps no map (high certainty; low impact)

`topology-bench.sh:708-709` says, and the description repeats, that a
row whose daemon keeps no map is left out; the filter is
`intent_syncs` being a number. A daemon of status version 14 or later
prints every intent key whether or not it keeps a map:
`intent = (version >= 14).then(...)` in `status.rs`'s decode, and the
print loop prints them all, with `intent_state off` and zeros for a
store with no held cuts. So a plain daemon row (no `--hold-cuts`) is in
the table, as "off | 0 | n/a | 0.00 | 0 | n/a | 0.00 | 0 | 0 / 0"
(`scenarios.py`, row "nomap"); "off" there reads like the header sync
being off. The test's "none" case (`test_embedded_python.py:105`,
`skz cuts 1` alone) is a daemon before version 14, not one without a
map. Filter on `intent_state` other than `off` (plant I6 does it, and
the pull request's tests still pass), and make the test's case a v16
status with `intent_state off`.

### G3. The tables' tests do not pin half the uploader's columns, nor the intent map's end values

In `test_a_row_that_uploads` `cuts_abandoned`, `upload_mismatches` and
`upload_errors` are 0 at both ends, and in `test_the_intent_maps_table`
`intent_named`, `intent_regions`, both longest syncs and
`intent_header_sync` are the same at both ends. So a plant that shows
`cuts_abandoned`, the mismatches or the errors at the window's end
instead of their change (T1 to T3), or swaps abandoned and mismatches
(T4), survives; as does reading "named at end", the longest map sync or
the header-sync flag at the window's start (I1 to I3). The abandoned
column is the one F2 is about and the one the first run's finding rests
on. Different values at the two ends would kill them. Three more
survive: "ms each" dividing by a count of 0 (I8: no window of the tests
has 0 syncs of either kind, while every `off` row of 44358962767 has 0
header syncs, so this plant passes the tests and fails the summary of a
real run: `summarize` over 44358962767's row files with I8 planted ends
with exit 1 on a ZeroDivisionError, and without it reproduces both
tables); the keys not exported to the daemon (U1); and the row's
`upload` line not written to `.skzinfo` (U4), which removes the
uploader's table altogether. A run would show U1 and U4 at once; I8 is
the one worth a case. The last two survivors are minor: the wait's
limit off by one second (W6), and a negative change printed as 0 (T10,
which only a window spanning two daemons shows, G6). The new columns themselves are pinned: lost (T6),
first upload (T8) and named at end against regions (I7) are killed, as
are the wait's place and conditions (W1 to W5).

### G4. A row that says it uploads and uploaded nothing still gets a verdict (a question for the lead)

`bracket.py:95` withholds the verdict from a row whose releaser ended
("not the row its label would say"), and nothing does so for uploads. In
44335231766 the four upload rows' first upload was `NONE in 900 s` and
the uploader committed nothing in any window, and each row got
"BELOW 0.90" (0.508 to 0.632) under serve arguments that say "(uploads
to tb-...)". With `scenarios.py`: rows whose uploader had no lease the
whole window, or whose daemon died or changed in the window, get "at
least 0.90" at 950 against model rows of 1000. A row whose seed never
landed does measure a daemon trying to upload, so a verdict may be what
the lead wants for that one; a row with no lease measures a daemon
without uploads. The lead should say which states give no verdict.

### G5. The wait against the job's limit, and on a dead daemon

The default `TB_SKZ_UPLOAD_WAIT_S=900` applies per upload row, and the
job's limit is 120 minutes (`topology-bench.yml:167`). In 44335231766
four rows each waited the full 900 s and the job was cancelled at its
limit (step "Run benchmark" 15:28:30 to 17:24:35). At 405b7ed the seed
lands in about three minutes, but if it does not land again (the case
the next round looks at), 44358962767's sequence of four upload rows
would take about 48 minutes more than its 64, close to the limit, and a
fifth upload row would pass it and lose the job. Say it at the input,
or let the wait share a budget across rows. Also, the wait does not end when the
status cannot be read (a daemon that died): it waits its full time.

### G6. The tables do not say when the window spans two daemons

The row records `pid_same` (`:2772`) and the daemon's status table shows
it ("one server throughout"); the two new tables do not. A window whose
daemon died shows the dead daemon's figures, and one whose daemon
changed shows negative changes (`scenarios.py`: uploaded −37, map syncs
−800, −0.12 ms each). Nothing in the benchmark restarts the daemon
inside a window, so this needs a crash to show; a mark in the row (or
n/a when `pid_same` is false) would make it impossible to misread.

### G7. Combinations the refusals let through

- `TB_TOPOLOGIES="zfs-slog zfs-skyzfsd" TB_SKZ_UPLOAD=1` in one entry
  is accepted (`refusals.sh` F1): the zfs-slog row is measured without
  uploads under the entry's label, the case the comment at `:1101`
  says is refused. The check could ask that zfs-skyzfsd be the only
  topology of an upload entry.
- `TB_SKZ_UPLOAD=1 TB_SKYZFSD_COPY_REPEAT=N` is accepted (F5): each
  repetition creates a volume in object storage that then uploads during
  the next copies, and the copy table's line does not say uploads were
  on. Refuse it, or say it in that table.

Nothing legitimate is refused: no upload with the settings present,
the releaser alone, the header-sync pair's arguments, a hooks build with
uploads, and a wait of 0 all get past the checks.

### G8. MinIO per row (judged, as asked)

- `minio.sh down` (`:52-55`): `docker rm -f` then
  `sudo rm -rf -- "${1:?}"`, reached through `down "${2:?dir}"`. An unset
  or empty directory is refused before anything runs (tried with
  made-up sudo and docker: exit 1, no call); `--` keeps a name that
  begins with a dash from being an option; `rm` itself refuses `/`. The
  workflow passes only `$TB_MINIO_DIR`, which only the MinIO step sets,
  to `$RUNNER_TEMP/minio-data`, and checks it is not empty first; a
  sequence entry's assignments reach the bench script's environment, not
  the workflow's shell, so no entry can point it elsewhere. Safe as
  called. A guard on the name (the basename `minio-data`, or a
  `.minio.sys` inside) would keep a later caller from deleting another
  directory as root.
- The restart runs before the check for a hung row
  (`skyzfsd-hang`), so the evidence step sees MinIO new and empty rather
  than as the hang left it; the usage line is recorded first. Minor.
- Usage: objects counted as `xl.meta` under the bucket and unfinished
  uploads as the upload directories under `.minio.sys/multipart`, which
  matches MinIO's single-drive layout; 44358962767 read 1587 to 1591
  objects, 5075 to 5118 MiB and 0 unfinished after each upload row.
- The image is `pgsty/minio:RELEASE.2026-08-04T00-00-00Z`, the tag every
  other workflow here pins (no digest anywhere), published on all
  interfaces with MinIO's default keys, as in the other workflows; a
  hosted runner is not reachable from outside, so this is consistent
  and harmless (`-p 127.0.0.1:9000:9000` would say so in the code).

### G9. The workflow otherwise

- Credentials: the keys are MinIO's defaults; they reach the job's log
  through the existing `cat "$RUNNER_TEMP/tb.env"` and the env listing,
  and nothing else (summary and `.skzinfo` carry the endpoint, bucket
  and volume only). Not a leak; if real keys ever come from secrets,
  GitHub masks them.
- shellcheck 0.11.0: every tracked `*.sh`, as CI runs it, clean; the
  MinIO step and "Run benchmark" step bodies, extracted, clean.
- `bash tests/lib/apt-fallback.sh` runs after checkout in the bench
  job; the MinIO step installs nothing (docker and curl are on the
  image).
- Triggers: `on:` is unchanged, so a push to `zfs-topology` or a
  `skyzfs-*` branch still does not start the benchmark; the push
  `paths` do not name `minio.sh`, which a run without a sequence never
  uses. The bench tests run in `ci.yml` on every pull request
  (`test_copy_limit.sh`, `test_embedded_python.py`, and
  `test_skz_store.sh`, where F3's tests belong).

## The tables' deltas, recomputed by hand (run 44358962767)

From the raw `skz` lines of each upload row's `.before` and `.after`
(`recompute.py`, Python run with `-I` on the downloaded artifact):
every number in both tables is the change between the two reads, or the
end value where the table says so, in all eight windows (rows 2, 3, 5, 6;
tpcb and select). For example row 2 tpcb: cuts 31 to 55 (24), uploaded
25, whole 0, blocks 591564, replaced, abandoned, mismatches, errors and
lost 0, held 2 to 1, uploading cut 55 of 55, reading; map syncs 697 over
391.18 ms (0.56 ms each), longest 38.01 ms, waits 825, named 18 of 8192.
Row 3 tpcb (header sync on): header syncs 25 over 2.43 ms (0.10 each),
longest 0.77 ms. The balance `cuts = uploaded + found − gone − kept +
replaced + abandoned + Δheld` holds in every window of both runs
(44358962767: 24 = 25 + 0 + 0 − 1 or 24 + 0 + 0 + 0). One daemon
throughout every window (the pid the same at both reads). Lost,
replaced and abandoned are 0 everywhere in this run, so it shows nothing
of F2 or G3; 44325181328 does.

On the brief's list: the counters are taken as changes (cuts, uploaded,
whole, blocks, replaced, abandoned, mismatches, errors, lost, map and
header syncs and their time, waits) and the gauges at the end (held,
uploading, newest, state, lease, named, regions, header sync, the two
longest); within one daemon it does not matter which keys a start takes
up and which begin at 0, since the benchmark starts no daemon inside a
window (G6 for when one dies); "ms each" gives n/a on no syncs (no
division by zero); a status without a key gives n/a (version 15: the
header's columns n/a; before 15 no upload row can exist, since the
create needs the place); a negative change is printed as it is (G6).

## Refusals at 405b7ed (`refusals.sh`, as root, made-up tools)

| case | exit | last line |
|---|---|---|
| A1 upload, no daemon (zfs-slog) | 64 | TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: zfs-slog |
| A2 upload, no daemon, no settings | 64 | TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: zfs-slog |
| A3 upload, thin topology (nocache) | 64 | TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: nocache |
| A4 upload, daemon named as zfs-skyzfsdx | 64 | unknown topology: zfs-skyzfsdx |
| B0 upload + daemon, the four: past the checks | 64 | unknown topology: no-such-topology |
| B1 upload + daemon, without TB_S3_ENDPOINT | 64 | TB_SKZ_UPLOAD=1 needs TB_S3_ENDPOINT (the workflow sets it when the sequence names TB_SKZ_UPLOAD=1) |
| B2 upload + daemon, TB_S3_ENDPOINT empty | 64 | TB_SKZ_UPLOAD=1 needs TB_S3_ENDPOINT (the workflow sets it when the sequence names TB_SKZ_UPLOAD=1) |
| B1 upload + daemon, without TB_S3_BUCKET | 64 | TB_SKZ_UPLOAD=1 needs TB_S3_BUCKET (the workflow sets it when the sequence names TB_SKZ_UPLOAD=1) |
| B2 upload + daemon, TB_S3_BUCKET empty | 64 | TB_SKZ_UPLOAD=1 needs TB_S3_BUCKET (the workflow sets it when the sequence names TB_SKZ_UPLOAD=1) |
| B1 upload + daemon, without TB_S3_KEY_ID | 64 | TB_SKZ_UPLOAD=1 needs TB_S3_KEY_ID (the workflow sets it when the sequence names TB_SKZ_UPLOAD=1) |
| B2 upload + daemon, TB_S3_KEY_ID empty | 64 | TB_SKZ_UPLOAD=1 needs TB_S3_KEY_ID (the workflow sets it when the sequence names TB_SKZ_UPLOAD=1) |
| B1 upload + daemon, without TB_S3_SECRET | 64 | TB_SKZ_UPLOAD=1 needs TB_S3_SECRET (the workflow sets it when the sequence names TB_SKZ_UPLOAD=1) |
| B2 upload + daemon, TB_S3_SECRET empty | 64 | TB_SKZ_UPLOAD=1 needs TB_S3_SECRET (the workflow sets it when the sequence names TB_SKZ_UPLOAD=1) |
| C1 upload + releaser (hold-cuts, hooks) | 64 | TB_SKZ_UPLOAD=1 and TB_SKZ_RELEASE: a daemon that uploads lets its cuts go itself |
| C2 upload + releaser, no hold-cuts | 64 | TB_SKZ_RELEASE needs --hold-cuts in TB_SKYZFSD_ARGS (''): without it the daemon holds no cut |
| D1 TB_SKZ_UPLOAD=0 | 64 | TB_SKZ_UPLOAD is 1 or unset, not: 0 |
| D2 TB_SKZ_UPLOAD=yes | 64 | TB_SKZ_UPLOAD is 1 or unset, not: yes |
| D3 TB_SKZ_UPLOAD=0, no daemon | 64 | TB_SKZ_UPLOAD is 1 or unset, not: 0 |
| E1 wait abc | 64 | TB_SKZ_UPLOAD_WAIT_S is a number of seconds, not: abc |
| E2 wait -5 | 64 | TB_SKZ_UPLOAD_WAIT_S is a number of seconds, not: -5 |
| E3 wait 0 (no wait): past the checks | 64 | unknown topology: no-such-topology |
| E4 wait 30 without upload: past the checks | 64 | unknown topology: no-such-topology |
| F1 upload, zfs-slog and zfs-skyzfsd in one row | 64 | unknown topology: no-such-topology |
| F2 no upload, the settings given | 64 | unknown topology: no-such-topology |
| F3 releaser alone (as before) | 64 | unknown topology: no-such-topology |
| F4 upload + header sync args | 64 | unknown topology: no-such-topology |
| F5 upload + copy repeat | 64 | unknown topology: no-such-topology |
| F6 upload + hooks build, no releaser | 64 | unknown topology: no-such-topology |

served: nothing (no made-up tool and no daemon was run by any case)

## Plants (`plants.py`: one at a time on a copy of the tree, restored after each)

KILLED when a test of the pull request (`test_copy_limit.sh`,
`test_embedded_python.py`, `tests/lib/test_skz_store.sh`) fails or
`refusals.sh`'s output changes. The second column says which.

| plant | result | killed by |
|---|---|---|
| R1 TB_SKZ_UPLOAD value not checked | KILLED | refusals.sh (D1) |
| R2 the four settings not checked | KILLED | refusals.sh (B1) |
| R3 TB_S3_SECRET left out of the check | KILLED | refusals.sh (B1) |
| R4 upload with the releaser let through | KILLED | refusals.sh (C1) |
| R5 upload without a daemon let through | KILLED | refusals.sh (A1) |
| R6 upload without a daemon exits 0 | KILLED | refusals.sh (A1) |
| R7 TB_SKZ_UPLOAD_WAIT_S not checked | KILLED | refusals.sh (E1) |
| U1 the keys not handed to the daemon | SURVIVED | - |
| U2 the create names no place | KILLED | copy_limit |
| U3 one volume name for every create | KILLED | copy_limit |
| U4 no upload line in .skzinfo | SURVIVED | - |
| W1 the wait never runs | KILLED | copy_limit |
| W2 no sync before the wait | KILLED | copy_limit |
| W3 the wait on every daemon row | KILLED | copy_limit |
| W4 the wait ends at uploaded 2 | KILLED | copy_limit |
| W5 a run-out wait fails the row | KILLED | copy_limit |
| W6 the wait off by one second (gt) | SURVIVED | - |
| T1 cuts_abandoned shown at the end, not over the window | SURVIVED | - |
| T2 mismatches shown at the end | SURVIVED | - |
| T3 errors shown at the end | SURVIVED | - |
| T4 abandoned and mismatches swapped | SURVIVED | - |
| T5 the abandoned column reads abandoned_at_mismatch | KILLED | embedded |
| T6 lost shown at the end | KILLED | embedded |
| T7 held at the start, not the end | KILLED | embedded |
| T8 first upload column empty | KILLED | embedded |
| T9 replaced shown at the end | KILLED | embedded |
| T10 a negative change shown as 0 | SURVIVED | - |
| I1 named at end from the start | SURVIVED | - |
| I2 longest map sync from the start | SURVIVED | - |
| I3 header sync said from the start | SURVIVED | - |
| I4 waits at the end | KILLED | embedded |
| I5 header ms each from the map ns | KILLED | embedded |
| I6 rows with no map left out (filter on intent_state; the fix of G2, not a defect) | SURVIVED | - (as it should: the tests still pass with the fix) |
| I7 named at end from regions | KILLED | embedded |
| I8 "ms each" divides by a count of 0 | SURVIVED | - |

## Scripts and outputs in this directory

- `refusals.sh ROOT`: every refusal case, as `test_skz_store.sh` runs
  the releaser's; `refusals.out`.
- `plants.py COPY PROBE`: the plants; `plants.out`.
- `loop_sim.sh ROOT SEQUENCE`: the workflow's "Run benchmark" step
  under `bash -e` with made-up sudo, bench script and minio.sh (F1).
- `recompute.py SEQDIR`: the tables' figures from the raw lines.
- `scenarios.py ROOT`: crafted v16 statuses through the row's own
  here-document, `summarize` and `bracket.py` (G2, G4, G6);
  `scenarios.out`.
