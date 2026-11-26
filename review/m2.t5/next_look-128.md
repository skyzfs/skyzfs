# Next look at #128 (M2.T5) at 73ebf56

Pull request: skyzfs/skyzfs#128 (`skyzfs-testing` into `zfs-topology`).
**Head reviewed: 73ebf56**, one commit on 0325c40, the head of my first
look (`review-128.md` here). The base is still 65dcfd6. The commit changes
test code only:

- `tests/bench/bracket.py`, with `test_bracket.py`;
- `tests/bench/topology-bench.sh`, with `test_copy_limit.sh` and
  `test_embedded_python.py`.

I read the diff before the author's answer (issue comment 6996029081),
then checked each claim in the answer against the code. Line numbers
below are at 73ebf56.

## Verdict

**Can merge. Nothing blocks.**

- **Codex's P2 is fixed,** both in the row generator and in
  `not_uploading`.
  - My comparison against the rule as now stated agrees on all
    1,600,000 combinations and on 31 of 33 named edges. The other two
    raise on input the parser never makes, as at 0325c40.
  - The check neither denies a row that uploaded nor judges a row that
    did not.
- **D1 to D6 are fixed.**
- **T1 is closed.** The freeze is a function, tested with MinIO stood in
  for, and M10 to M13 are now killed. So are W8 and W9.
- **Plants:** 76 at 73ebf56, of which 70 are killed. The 6 survivors are
  the 5 the answer leaves as harmless, needing docker, or text, and G9
  (L1).
- **Three new findings, none blocking:**
  - L1 (low, text): a status not read reads as a pid change in the
    summary's status table.
  - L2 (nit): the reason "status was not read" also covers a status that
    showed no serving daemon.
  - L3 (nit): D3's comment names `pg_test_fsync` without its condition.
- **R1 to R3** are corrected in the description, and in the notes at
  `zfs-topology` 26723a8.
- **CI is green on every run at 73ebf56.** The replay rig's "product
  layout" job, which failed at 0325c40 on the rig's own busy export,
  passed here, with all 104 flush boundaries checked. The rerun at
  0325c40 was cancelled by the push of 73ebf56 before that job ended.
- **Codex** finished its review of 73ebf56 with no finding, and the
  author answered its P2 on the thread.

## The status-not-read check (Codex's P2)

**What changed.**

- `not_uploading` (`bracket.py:98-102`) now asks first that the status
  was read at both ends, meaning it holds a `pid`. If not, the reason is
  "its daemon's status was not read at the window's start", "end" or
  "start and end".
- The pid reason now comes after that check, and so always names two
  pids read (`:103-105`).
- The row generator (`topology-bench.sh:2917-2945`) reads `.skzinfo`
  whether or not the status was read. An upload row whose `.after` has no
  `skz` line now keeps a `skyzfsd` part with its `upload`, an empty
  `after`, `pid_same` None, and "THE DAEMON'S STATUS WAS NOT READ at the
  window's end" in its serve arguments.

**My comparison** (`diff_not_uploading2.py`, appendix). It is the
first look's, with the oracle rewritten from the rule as the pull
request now states it:

1. no `pid` at either end: "status";
2. `pid_same` not `True`: "pid";
3. the lease not `held` or `unsure` at both ends, or the two epochs not
   the same number: "lease";
4. `uploaded` not a number at both ends: "unread";
5. `uploaded` did not grow: "none".

- **The table.** 1,600,000 combinations:
  - upload present or not;
  - the pid at each end 4242, 5151, empty or missing;
  - `pid_same` True, False, None, missing, or as the generator computes
    it (`sb.get('pid') == sa.get('pid')`);
  - the lease held, unsure, none, lost or missing at each end;
  - the epoch `1`, `2`, empty, `x` or missing at each end;
  - `uploaded` `3`, `27`, empty or missing at each end.
- **Result: 0 disagreements and 0 problems of order.**
  - Every seventh denied row was also judged inside a bracket 26% wide
    and outside any bracket. Its verdict starts with "NO VERDICT: the row
    says it uploads, but" each time, with the ratio and width kept when
    enclosed.
  - Every row the oracle judges reads "at least 0.90".
- **A control.** The same script on 0325c40 finds 741,152
  disagreements. Most are the reworded pid reason. Among them are 280
  rows judged with no pid at one end and `pid_same` True, and 72 judged
  with the epoch empty at both ends (D1). So the comparison sees what
  changed.
- **Named edges: 33**, the first look's 21 and 12 new ones. 31 come out
  as specified. The two others are the first look's type cases,
  `uploaded` given as an int or as `None`. They raise `AttributeError` as
  before, and remain unreachable, since the parser keeps every `skz` value
  as a string (`topology-bench.sh:2873`). The new edges:
  - the end read failed, leaving `read_at_ns` alone (the generator's
    `pid_same` False): "status";
  - both reads failed (`pid_same` True, from `None == None`): "status".
    At 0325c40 this one fell through to the lease;
  - the generator's new part (`after` empty, `pid_same` None): "status";
  - the start not read, the end read: "status";
  - a status read with no `pid` line at the end: "status" (see L2);
  - pid empty at the start: "status";
  - pid `0` at both ends: judged;
  - two daemons, both read: "pid";
  - an epoch `x` at both ends: "lease";
  - epoch `0` at both ends: judged;
  - the epoch as an int at both ends: judged (`str()` makes it digits);
  - a real row (held, epoch 1, 3 to 27): judged.

**Through the row generator** (`codex_p2b.py`, appendix). Each case runs
the row's here-document on a `.before` and an `.after` written as
`snapshot` writes them, with `.skzinfo` saying `upload` and
`upload_from_load 1`. Each row is judged between two model rows:

| case | 0325c40 | 73ebf56 |
|---|---|---|
| end: no `skz` line (Codex's case) | no `skyzfsd` part; "at least 0.90" | NO VERDICT: status not read at the window's end |
| end: `read_at_ns` alone (a failed read) | NO VERDICT: "not the same (pid 7 and not read)" | NO VERDICT: not read at the window's end |
| start: `read_at_ns` alone | NO VERDICT: "not the same (pid not read and 7)" | NO VERDICT: not read at the window's start |
| both: `read_at_ns` alone | NO VERDICT: lease "not read at epoch not read" | NO VERDICT: not read at the window's start and end |
| end read, no `pid` line (device not live) | NO VERDICT: "not the same (pid 7 and not read)" | NO VERDICT: not read at the window's end |
| both read in full, 3 to 27 (control) | at least 0.90 | at least 0.90 |

`summarize` on the first case exits 0. Its bracket table shows the
reason, and the build column the serve arguments' "THE DAEMON'S STATUS WAS
NOT READ at the window's end". Its status table is the subject of L1.

**Does it deny a row that uploaded?** No.

- The status prints `pid` only for this volume's device while it is
  live, from a status file of this boot
  (`crates/skyzfs-ublk/src/status.rs:1800-1803`).
- A daemon that served through its window is live at both reads. So a
  row that uploaded, read at both ends, has a pid at both ends and passes
  the check.
- A row with no pid at one end was denied at 0325c40 too, on the pid
  (`pid_same` False). The rows that the check newly denies are those with
  no pid at both ends. At 0325c40 they had `pid_same` True (`None ==
  None`) and could be judged. Such a status shows no serving daemon at
  either end, so none of them is a row that uploaded.

**Does it let a row through that did not upload?** No: the table above.

## D1 to D6: fixed

- **D1.** `bracket.py:106-108` adds `or not str(lease[0][1]).isdigit()`.
  With the equality before it, both epochs must be the same number.
  - Test: `test_an_epoch_that_is_not_a_number` (`test_bracket.py:247`),
    with the epoch missing, empty and `x`.
  - Plants E1 (the digit test removed) and E2 (asked only when the two
    epochs differ) are killed.
- **D2.** `topology-bench.sh:1980` is now `-gt`. A mark in the second of
  the read before reads "as the workload began".
  - Test: `test_copy_limit.sh:244` takes the mark at 79, which gives
    "under the workload, 1 to 31 s". `:251` adds 80 to the loop of "as
    the workload began".
  - Plant N1e (`-ge` back) is killed.
  - The comment at `:1949-1955` says why.
- **D3.** The option's comment (`:285-290`) now names what writes under
  the first cut before MinIO answers, and says pgbench's init ran on the
  origin. See L3 for one nit.
- **D4.** The freeze is a function, `skz_minio_freeze` (`:1321-1330`). It
  sets `SKZ_MINIO_PAUSED=1` before `docker pause`. Plant Z1 (the flag
  after the pause) is killed.
- **D5.** The watcher's comment is now above `skz_seed_watch_stop`
  (`:1344-1346`).
- **D6.** Both wordings are fixed:
  - the pid reason reads "was not shown to be the same" and is reached
    only with both pids read;
  - the summary's paragraph (`:911-919`) lists every reason.

## T1 and W8, W9: closed

- **The build's line** (`:2210`) is pinned verbatim, after the create
  and before the start (`test_copy_limit.sh:341-342`).
- **The function** runs in the test with `skz_minio` stood in for
  (`:345-358`). The test checks:
  - the flag is set before the pause;
  - the record's `upload_from_load 1` line and the log line;
  - a failed pause returns 1, keeps the flag for the teardown, and writes
    no record line.
- **The plants.**
  - M10 to M13 are killed.
  - So are the eight plants on the function (Z1 to Z8).
  - M11, M12, Z5 and Z6 are killed by the verbatim pin, not by running
    the build. For a one-line guard that is enough.
- **W8 and W9** are killed by the order check at `:362-365`.

The survivors the answer leaves (W10, M4, M16, M17, B13) survive again
(table below). I agree with the reasons given: harmless, no docker in
the tests, or text.

## New findings (none blocks the merge)

### L1. The summary's other tables read a status not read as a pid change (low, text)

- **Where:** `topology-bench.sh:956`, the status table's "one server
  throughout" column: `'yes' if k['pid_same'] else 'NO: the pid
  changed'`. Also `:705`, `one()`, which adds "(TWO DAEMONS in the
  window)" when `pid_same` is False.
- **What happens:**
  - The new part for an end not read has `pid_same` None, so the column
    says "NO: the pid changed". At 0325c40 that row had no part, and the
    column said "n/a".
  - A failed read at one end (`read_at_ns` alone) has `pid_same` False.
    It gives "NO: the pid changed", and "(TWO DAEMONS in the window)" in
    the uploader's table. This is as at 0325c40.
  - Failed reads at both ends give `pid_same` True (`None == None`,
    `:2952`), so the column says "yes".
- **Scenario:** `codex_p2b.py`, case "end: no `skz` line". The status
  table's row reads "NO: the pid changed".
- **Certainty:** certain. The verdict is right in every case; only these
  columns mislead a reader. Plant G9 (the column printing None as one
  server) survives, so the column is untested.
- **Fix:** set `pid_same` None unless both pids were read, and print
  "not read" for None in the column and in `one()`. It can wait for a
  later package.

### L2. "Status was not read" also covers a status that showed no serving daemon (nit)

- **Where:** `bracket.py:98-102`. The comment says "a status read shows
  the daemon's pid".
- **What:** `status.rs:1800-1803` prints `pid` only when the device is
  this volume's and live. A status read while the device was waiting for
  a server, dead or detached has no `pid`, and the row reads "status was
  not read".
- **Effect:** the verdict is right. Only the reason is inexact.
- **Fix, if wanted:** "was not read, or showed no serving daemon".

### L3. D3's comment names `pg_test_fsync` without its condition (nit)

`topology-bench.sh:285-290` lists `pg_test_fsync` among what writes
before MinIO answers. It runs only with `TB_FSYNC_TEST=1` (`:2685`).

## The description: R1 to R3 corrected

The description at 00:44 UTC says:

- **R1.** "122 to 154 MiB at the windows' ends".
- **R2.** "Of those 52 s, MinIO was frozen for 36: from 23:09:44, before
  the import at 23:10:06, to 23:10:42. The seed itself took 11 to 16 s
  from MinIO's answer."
- **R3.** "`fl`'s windows hold no part of the seed ... Its ratios (0.832,
  0.855) are figures from after the seed, like `sp`'s. They are not what
  a seed costs under load."

These match what I found in run 44412222348. The notes at
`zfs-topology` 26723a8 carry the same corrections, citing review's R1,
R2 and R3.

The answer's local figures match mine: `test_bracket.py` 21 OK,
`test_embedded_python.py` 7 OK.

## CI and Codex at 73ebf56

I read the runs at 01:36 UTC, when the last one had ended. The head was
still 73ebf56.

| run | state |
|---|---|
| push: skyzfs-ci 44430956147 | success |
| pull request: ci 44430962419, coverage 44430962436, e2e 44430962445 | success |
| kernel probe 44430962187, nodeb rig 44430962372, control rig 44430962437 | success |
| cut rig 44430962288 (the reference and four "record and check" jobs) | success |
| cut-daemon 44430962357 (three "record through skyzfsd and check" jobs) | success |
| **replay rig 44430962379** | **success, every job.** "Replay, product layout (local log device)", job 135249046134, checked 104 of 104 flush boundaries, with the scrub and every-copy checks clean and `zdb -e -bcc` exiting 0. Its three block controls were caught as `EXPECTED` says. "Replay, whole-pool layout" and the four daemon jobs passed. |
| daemon rig 44430962257 | success, five jobs. On the three daemon jobs (24.04, 26.04 and 26.04-arm), the WAITING line is `pin_failed_volume_while_a_request_is_held` ("not built"), and the NA lines are `ctl_delay_before_apply` and `pin_c_kill_with_applied_unacknowledged`. On every job, the FAIL lines are only the five negative controls (`ctl_*`). This is as at 0325c40. |

**The replay rig at 0325c40.** Its second attempt (run 44421456349,
attempt 2) passed every job but the product layout one. That job was
cancelled when 73ebf56 was pushed, so the rerun never ended. The run at
73ebf56 above is the answer: the same job passed. The busy export
remains a one-off of the rig.

**Codex.** Its summary comment says its code review of 73ebf56 completed
at 00:44 UTC with no finding. Its one thread, P2 on 0325c40, has the
author's answer (fixed in 73ebf56), which matches what I found.

## Local gates at 73ebf56

All run on a worktree of the head:

| gate | result |
|---|---|
| shellcheck 0.11.0, every tracked `*.sh` | clean |
| `test_bracket.py` | 21 OK |
| `test_embedded_python.py` | 7 OK |
| `test_skz_time.py`, `test_cut_release.py`, `test_syscalls.py`, `test_perfcat.py` | OK |
| `test_copy_limit.sh` | 96 ok, all passed |
| `tests/lib/test_skz_store.sh`, as root | 104 ok, all passed |

## Plants at 73ebf56

`plants128b.py` (appendix) is the first look's `plants128.py` with the
texts that moved adapted, plus plants on the new code. It applies each
plant alone to a worktree of the head and runs the tests that cover the
file, as before:

- for `topology-bench.sh`: `test_copy_limit.sh`,
  `test_embedded_python.py` and `test_skz_store.sh`;
- for `bracket.py`: `test_bracket.py` and `test_embedded_python.py`;
- for `minio.sh`: `test_skz_store.sh`.

Each test runs under a 600 s limit. The file is restored in a `finally`,
and both worktrees were clean at the end. B12's text moved (the
condition is now in brackets), so it did not apply. I ran it again in
its new form in a second worktree, with G8 and G9.

**The result: 76 plants, 70 killed, 6 survived.**

- The first look's 51: 46 killed, 5 survived (B13, M4, M16, M17, W10).
  M10 to M13, W8 and W9, which survived at 0325c40, are now killed.
- The new 25: 24 killed. G9 survived (L1).

| plant | 0325c40 | 73ebf56 |
|---|---|---|
| B1 to B11 (`not_uploading`, the verdict's order) | KILLED | KILLED (bracket; B8 also embedded) |
| B12 the lease refused only when not held at both ends | KILLED | KILLED (bracket), in its new text |
| B13 the summary paragraph's new sentence changed | SURVIVED | SURVIVED (text) |
| M1 to M3, M5 (the resume's place, the teardown's unpause, the flag on a failed unpause) | KILLED | KILLED (copy_limit) |
| M4 the teardown's unpause after the daemon's stop | SURVIVED | SURVIVED (harmless) |
| M6 to M8 (the refusals) | KILLED | KILLED (skz_store) |
| M9 MinIO never frozen (the pause line removed) | KILLED | KILLED (copy_limit) |
| **M10 the flag not set** | **SURVIVED** | **KILLED (copy_limit)** |
| **M11 the freeze on `$SKZ_UPLOAD`** | **SURVIVED** | **KILLED (copy_limit, the pin)** |
| **M12 the freeze's condition false** | **SURVIVED** | **KILLED (copy_limit, the pin)** |
| **M13 `upload_from_load 1` not written** | **SURVIVED** | **KILLED (copy_limit)** |
| M14, M15 (the resume) | KILLED | KILLED (copy_limit) |
| M16 `minio.sh pause` does something else | SURVIVED | SURVIVED (no docker in the tests) |
| M17 `minio.sh down` does not unpause | SURVIVED | SURVIVED (same) |
| N1a to N1d, N1f to N1i (the border between rounds) | KILLED | KILLED (copy_limit) |
| N1e `-ge` (D2) | the first look's N1e, `-gt`, KILLED | `-ge` back: KILLED (copy_limit) |
| W1 to W7 (`skz_seed_watch_stop`, the mark) | KILLED | KILLED (copy_limit) |
| **W8 `SKZ_SEED_INFO` not set at the build** | **SURVIVED** | **KILLED (copy_limit)** |
| **W9 `SKZ_SEED_T0` not set at the build** | **SURVIVED** | **KILLED (copy_limit)** |
| W10 the mark rewritten at every warm-up | SURVIVED | SURVIVED (harmless) |
| F1, F6 (the mark's place, the watcher's first line) | KILLED | KILLED (copy_limit) |
| S1 the status check removed | - | KILLED (bracket, embedded) |
| S2, S3 the status asked at one end only | - | KILLED (bracket; S3 also embedded) |
| S4 a read known by its `read_at_ns`, not its pid | - | KILLED (bracket, embedded) |
| S5 the reason names the first end only | - | KILLED (bracket) |
| S6 the status asked only when `pid_same` is False | - | KILLED (bracket, embedded) |
| E1 (D1) the epoch's digit test removed | - | KILLED (bracket) |
| E2 (D1) the digit test only when the epochs differ | - | KILLED (bracket) |
| G1 the generator's new part removed (Codex's P2 back) | - | KILLED (embedded) |
| G2 that part's `pid_same` True | - | KILLED (embedded) |
| G3 that part's "STATUS WAS NOT READ" dropped | - | KILLED (embedded) |
| G4 that part for a row that does not upload | - | KILLED (embedded) |
| G5 that part's end a copy of its start | - | KILLED (embedded) |
| G6 `from_load` dropped from `upload_of` | - | KILLED (embedded) |
| G7 the generator's `pid_same` always True | - | KILLED (embedded) |
| G8 that part's start dropped too | - | KILLED (embedded) |
| G9 the one-server column prints None as "yes" | - | SURVIVED (L1) |
| Z1 (D4) the flag after the pause | - | KILLED (copy_limit) |
| Z2 a failed pause taken as a freeze | - | KILLED (copy_limit) |
| Z3 a failed pause clears the flag | - | KILLED (copy_limit) |
| Z4 the record line before the pause | - | KILLED (copy_limit) |
| Z5 the build goes on when the freeze fails | - | KILLED (copy_limit, the pin) |
| Z6 the freeze after the daemon's start | - | KILLED (copy_limit, the pin) |
| Z7 the freeze does not log | - | KILLED (copy_limit) |
| Z8 the freeze unpauses | - | KILLED (copy_limit) |

## Appendix: the scripts

Each is run with `python3 -I`, from a directory of its own, with TREE a
worktree of the head. `diff_not_uploading2.py` and `codex_p2b.py` were
also run on a worktree of 0325c40, for the columns and the control
above. The three extra plants (B12 in its new text, G8, G9) are in the
same form as those in `plants128b.py`, which is the first look's
`plants128.py` with the changes said above.

### `diff_not_uploading2.py`

```python
"""diff_not_uploading2.py TREE: bracket.py's not_uploading and bracketed at TREE against an
oracle written from the rule as #128 now states it: an upload row has no verdict when its
daemon's status was not read at either end (no pid in it), when the daemon was not shown to
be the same (pid_same not True), when the lease was not held or unsure at both ends under one
epoch that is a number, when its uploads cannot be read, or when none committed in the window.
A cartesian table of readings at the two ends, then named edges. Prints the disagreements by
class and exits 1 when there is any."""
import itertools
import sys
from collections import Counter

sys.path.insert(0, sys.argv[1] + '/tests/bench')
import bracket as bk  # noqa: E402

MISSING = object()
DERIVED = object()  # pid_same as the row generator computes it
HELD = ('held', 'unsure')


def digits(v):
    return isinstance(v, str) and v.isdigit()


def oracle(k):
    """-> None (it uploaded, judged) or a class: status, pid, lease, unread, none."""
    if not (k or {}).get('upload'):
        return None
    b, a = k.get('before') or {}, k.get('after') or {}
    if not b.get('pid') or not a.get('pid'):
        return 'status'
    if k.get('pid_same') is not True:
        return 'pid'
    if b.get('lease') not in HELD or a.get('lease') not in HELD:
        return 'lease'
    eb, ea = b.get('lease_epoch'), a.get('lease_epoch')
    if not (digits(eb) and digits(ea) and eb == ea):
        return 'lease'
    ub, ua = b.get('uploaded'), a.get('uploaded')
    if not (digits(ub) and digits(ua)):
        return 'unread'
    if int(ua) <= int(ub):
        return 'none'
    return None


def klass(text):
    if text is None:
        return None
    for prefix, c in (("its daemon's status was not read at the window's ", 'status'),
                      ('its daemon was not shown to be the same', 'pid'),
                      ('it did not hold the lease', 'lease'),
                      ('its uploads cannot be read', 'unread'), ('no upload committed in its window', 'none')):
        if text.startswith(prefix):
            return c
    return 'OTHER:' + text


def status(lease, epoch, up, pid='4242'):
    s = {}
    for key, v in (('pid', pid), ('lease', lease), ('lease_epoch', epoch), ('uploaded', up)):
        if v is not MISSING:
            s[key] = v
    return s


def row(seq, topo, tps, **kw):
    r = {'_seq': seq, 'topology': topo, 'tps': tps, 'machine': 'm', 'origin_delay_ms': 0, 'workload': 'tpcb'}
    r.update(kw)
    return r


UP = {'endpoint': 'http://127.0.0.1:9000', 'bucket': 'b', 'volume': 'v'}
LEASES = ['held', 'unsure', 'none', 'lost', MISSING]
EPOCHS = ['1', '2', '', 'x', MISSING]
UPS = ['3', '27', '', MISSING]
PIDV = ['4242', '5151', '', MISSING]
PIDS = [True, False, None, MISSING, DERIVED]

dis = Counter()
examples = {}
n = 0
order_bad = []
for upload, pb, pa, ps, lb, la, eb, ea, ub, ua in itertools.product(
        [UP, MISSING], PIDV, PIDV, PIDS, LEASES, LEASES, EPOCHS, EPOCHS, UPS, UPS):
    k = {'before': status(lb, eb, ub, pb), 'after': status(la, ea, ua, pa)}
    if upload is not MISSING:
        k['upload'] = upload
    if ps is DERIVED:
        k['pid_same'] = k['before'].get('pid') == k['after'].get('pid')
    elif ps is not MISSING:
        k['pid_same'] = ps
    n += 1
    try:
        got = klass(bk.not_uploading({'skyzfsd': k}))
    except Exception as e:  # noqa: BLE001
        got = f'RAISED {type(e).__name__}'
    want = oracle(k)
    if got != want:
        key = (want, got)
        dis[key] += 1
        examples.setdefault(key, k)
    # The verdict, on a sample: the row's own reason first, enclosed or not; figures kept when enclosed.
    if n % 7 == 0 and want is not None and got == want:
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=k), row(3, 'zfs-slog', 1300)]
        b = bk.bracketed(rows[1], rows)  # also a bracket too wide (26%)
        if not (b['verdict'].startswith('NO VERDICT: the row says it uploads, but ') and b['ratio'] and b['width']):
            order_bad.append(('enclosed', k, b))
        rows2 = [row(2, 'zfs-skyzfsd', 950, skyzfsd=k), row(3, 'zfs-slog', 1300, origin_delay_ms=10)]
        b2 = bk.bracketed(rows2[0], rows2)
        if not (b2['verdict'].startswith('NO VERDICT: the row says it uploads, but ') and b2['ratio'] is None):
            order_bad.append(('not enclosed', k, b2))
    if want is None and got is None and upload is not MISSING:
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=k), row(3, 'zfs-slog', 1000)]
        if bk.bracketed(rows[1], rows)['verdict'] != 'at least 0.90':
            order_bad.append(('judged row not judged', k, None))

print(f'{n} scenarios; disagreements: {sum(dis.values())}')
for (want, got), c in sorted(dis.items(), key=lambda x: -x[1]):
    print(f'  oracle {want!s:7} code {got!s:7} x{c}  e.g. before={examples[(want, got)]["before"]} '
          f'after={examples[(want, got)]["after"]} pid_same={examples[(want, got)].get("pid_same", "MISSING")}')
print(f'verdict-order problems: {len(order_bad)}')
for o in order_bad[:5]:
    print('  ', o)

# Named edges: the 21 of the first look (classes as the rule now says), then new ones.
base = lambda **kw: {'upload': UP, 'pid_same': True, **kw}
EDGES = [
    ('epoch change 1 -> 2, held both', base(before=status('held', '1', '3'), after=status('held', '2', '27')), 'lease'),
    ('unsure at the start only', base(before=status('unsure', '1', '3'), after=status('held', '1', '27')), None),
    ('unsure at the end only', base(before=status('held', '1', '3'), after=status('unsure', '1', '27')), None),
    ('unsure at both ends', base(before=status('unsure', '1', '3'), after=status('unsure', '1', '27')), None),
    ('unsure, epoch changed', base(before=status('unsure', '1', '3'), after=status('unsure', '2', '27')), 'lease'),
    ('commit at the window end, not yet counted', base(before=status('held', '1', '3'), after=status('held', '1', '3')), 'none'),
    ('commit at the window end, counted', base(before=status('held', '1', '3'), after=status('held', '1', '4')), None),
    ('commit at the window start, counted before', base(before=status('held', '1', '4'), after=status('held', '1', '4')), 'none'),
    ('counter went down (a new daemon, same pid?)', base(before=status('held', '1', '5'), after=status('held', '1', '2')), 'none'),
    ('epoch missing at both ends (D1)', base(before=status('held', MISSING, '3'), after=status('held', MISSING, '27')), 'lease'),
    ('epoch empty at both ends', base(before=status('held', '', '3'), after=status('held', '', '27')), 'lease'),
    ('epoch missing at one end', base(before=status('held', '1', '3'), after=status('held', MISSING, '27')), 'lease'),
    ('uploaded missing at both ends', base(before=status('held', '1', MISSING), after=status('held', '1', MISSING)), 'unread'),
    ('no readings at all', base(before={}, after={}), 'status'),
    ('readings None', {'upload': UP, 'pid_same': True, 'before': None, 'after': None}, 'status'),
    ('pid_same missing', {'upload': UP, 'before': status('held', '1', '3'), 'after': status('held', '1', '27')}, 'pid'),
    ('pid_same truthy but not True (1)', base(pid_same=1, before=status('held', '1', '3'), after=status('held', '1', '27')), 'pid'),
    ('no skyzfsd part', None, None),
    ('upload {} (falsy)', {'upload': {}, 'pid_same': True, 'before': {}, 'after': {}}, None),
    ('uploaded as an int (JSON number)', base(before={'pid': '1', 'lease': 'held', 'lease_epoch': '1', 'uploaded': 3},
                                              after={'pid': '1', 'lease': 'held', 'lease_epoch': '1', 'uploaded': 27}), 'unread'),
    ('uploaded None', base(before=status('held', '1', None), after=status('held', '1', '27')), 'unread'),
    # new at 73ebf56
    ('the end read failed: its clock line alone (generator: pid_same False)',
     base(pid_same=False, before=status('held', '1', '3'), after={'read_at_ns': '2000'}), 'status'),
    ('both reads failed: clock lines alone (generator: pid_same None == None, True)',
     base(pid_same=True, before={'read_at_ns': '1000'}, after={'read_at_ns': '2000'}), 'status'),
    ('the end not read at all (generator\'s new part: after {}, pid_same None)',
     base(pid_same=None, before=status('held', '1', '3'), after={}), 'status'),
    ('the start not read, the end read', base(pid_same=False, before={'read_at_ns': '1000'}, after=status('held', '1', '27')), 'status'),
    ('a status read with no serving daemon at the end (no pid line: device not live)',
     base(pid_same=False, before=status('held', '1', '3'), after=status('held', '1', '27', pid=MISSING)), 'status'),
    ('pid empty at the start', base(before=status('held', '1', '3', pid=''), after=status('held', '1', '27')), 'status'),
    ('pid 0 at both ends', base(before=status('held', '1', '3', pid='0'), after=status('held', '1', '27', pid='0')), None),
    ('two daemons, both read', base(pid_same=False, before=status('held', '1', '3'), after=status('held', '1', '27', pid='5151')), 'pid'),
    ('epoch not a number at both ends', base(before=status('held', 'x', '3'), after=status('held', 'x', '27')), 'lease'),
    ('epoch 0 at both ends (a lease at epoch 0)', base(before=status('held', '0', '3'), after=status('held', '0', '27')), None),
    ('epoch as an int (JSON number) at both ends', base(before={'pid': '1', 'lease': 'held', 'lease_epoch': 1, 'uploaded': '3'},
                                                        after={'pid': '1', 'lease': 'held', 'lease_epoch': 1, 'uploaded': '27'}), None),
    ('a real row that uploaded (held, epoch 1, 3 -> 27)', base(before=status('held', '1', '3'), after=status('held', '1', '27')), None),
]
bad = 0
for name, k, want in EDGES:
    r = {} if k is None else {'skyzfsd': k}
    try:
        got = klass(bk.not_uploading(r))
    except Exception as e:  # noqa: BLE001
        got = f'RAISED {type(e).__name__}: {e}'
    flag = 'ok ' if got == want else 'DIFF'
    bad += got != want
    print(f'{flag} {name}: expected {want}, got {got}')
print(f'{len(EDGES)} edges, {bad} differ')
sys.exit(1 if dis or order_bad or bad else 0)
```

### `codex_p2b.py`

```python
"""codex_p2b.py TREE: rows of an upload row's window whose status reads failed, made by the row
here-document of TREE's topology-bench.sh and judged by TREE's bracket.py between two model rows,
then the summary of the first case. Cases: the .after with no skz line (Codex's P2), a failed
read at either or both ends (`skz read_at_ns` alone), a status read with no pid line (the device
not live), and a status read in full at both ends (the control: judged)."""
import json
import os
import re
import subprocess
import sys
import tempfile

tree = sys.argv[1]
HERE = os.path.join(tree, 'tests/bench')
sys.path.insert(0, HERE)
import bracket as bk  # noqa: E402

SCRIPT = os.path.join(HERE, 'topology-bench.sh')
HEREDOC = re.compile(r"<<\s*'PY'[^\n]*\n(.*?)\n\s*PY\n", re.S)
devs = 'dev pool 0 0 0 0 0 0 0 0\ndev origin 0 0 0 0 0 0 0 0\n'
st = lambda t, up, pid=True: (f'skz read_at_ns {t}\n' + ('skz pid 7\n' if pid else '')
                              + f'skz lease held\nskz lease_epoch 1\nskz uploaded {up}\n')
CASES = {
    'end: no skz line (Codex)': (devs + st(1000, 3), devs),
    'end: read_at_ns alone': (devs + st(1000, 3), devs + 'skz read_at_ns 2000\n'),
    'start: read_at_ns alone': (devs + 'skz read_at_ns 1000\n', devs + st(2000, 27)),
    'both: read_at_ns alone': (devs + 'skz read_at_ns 1000\n', devs + 'skz read_at_ns 2000\n'),
    'end: read, no pid line (device not live)': (devs + st(1000, 3), devs + st(2000, 27, pid=False)),
    'both read in full, 3 -> 27 (control)': (devs + st(1000, 3), devs + st(2000, 27)),
}
with open(SCRIPT) as f:
    doc = next(x for x in HEREDOC.findall(f.read()) if 'sys.argv[12]' in x)
first = None
for name, (before, after) in CASES.items():
    with tempfile.TemporaryDirectory() as d:
        open(os.path.join(d, 'row.py'), 'w').write(doc)
        base = os.path.join(d, 'r')
        for suf, text in (('.before', before), ('.after', after), ('.pgbench', 'tps = 950 (without initial connection time)\n')):
            open(base + suf, 'w').write(text)
        open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w').write(
            'serve_args \nrelease_every 0\nupload http://127.0.0.1:9000 b v\nupload_from_load 1\nbuild abc\n')
        p = subprocess.run(['python3', '-I', os.path.join(d, 'row.py'), base, 'zfs-skyzfsd', 'tpcb', '60', 'none', '0',
                            base + '.nolog', base + '.nofsync', 'x', '8', '0', HERE], capture_output=True, text=True, timeout=60)
        if p.returncode:
            print(f'{name}: row failed: {p.stderr[-300:]}')
            continue
        r = json.loads(p.stdout)
    r.update({'_seq': 2, 'machine': 'm', 'origin_delay_ms': 0})
    whole = json.loads(json.dumps(r))
    m1 = {'_seq': 1, 'topology': 'zfs-slog', 'tps': 1000, 'machine': 'm', 'origin_delay_ms': 0, 'workload': r['workload']}
    m2 = dict(m1, _seq=3)
    rows = [m1, r, m2]
    for o in rows:
        o.pop('duration_s', None); o.pop('clients', None); o.pop('mem_limit', None)
    k = r.get('skyzfsd')
    print(f"{name}: part {'present' if k else 'ABSENT'}"
          + (f", pid_same {k.get('pid_same')!r}" if k else '') + f"; verdict: {bk.bracketed(r, rows)['verdict']}")
    if first is None:
        first = whole
# The summary of the first case: its lines that name the row.
with tempfile.TemporaryDirectory() as d:
    out = os.path.join(d, '1')
    os.makedirs(out)
    json.dump(first, open(os.path.join(out, 'tpcb-zfs-skyzfsd.json'), 'w'))
    p = subprocess.run(['bash', SCRIPT, 'summarize', d], capture_output=True, text=True, timeout=120)
    print(f'summarize rc {p.returncode}' + (f': {p.stderr[-600:]}' if p.returncode else ''))
    for line in p.stdout.splitlines():
        if line.startswith('| tpcb | zfs-skyzfsd'):
            print('  ' + line[:400])
```

### `plants128b.py`

```python
"""plants128b.py TREE [FIRST [LAST]]: the plants of the next look at #128 (73ebf56), one at a time in TREE
(a worktree at the head). The plants of the first look, adapted where the text moved, then plants on
the new code. A plant of the bench script is followed by test_copy_limit.sh,
test_embedded_python.py and tests/lib/test_skz_store.sh; of bracket.py by test_bracket.py and
test_embedded_python.py; of minio.sh by test_skz_store.sh. KILLED: a test failed. SURVIVED:
none did. NOT APPLIED: the text was not found exactly once. The file is restored after each
plant (try/finally), every test runs under a time limit, and the tree is checked clean at the end."""
import subprocess
import sys

tree = sys.argv[1]
SH, BK, MS = 'tests/bench/topology-bench.sh', 'tests/bench/bracket.py', 'tests/bench/minio.sh'
T_COPY = ('copy_limit', ['bash', 'tests/bench/test_copy_limit.sh'])
T_EMB = ('embedded', ['python3', 'tests/bench/test_embedded_python.py'])
T_STORE = ('skz_store', ['bash', 'tests/lib/test_skz_store.sh'])
T_BRK = ('bracket', ['python3', 'tests/bench/test_bracket.py'])
TESTS = {SH: [T_COPY, T_EMB, T_STORE], BK: [T_BRK, T_EMB], MS: [T_STORE]}


def run(cmd):
    try:
        return subprocess.run(cmd, cwd=tree, capture_output=True, text=True, timeout=600).returncode
    except subprocess.TimeoutExpired:
        return 'TIMEOUT'


MARK = '    if [ -n "$SKZ_SEED_WATCH" ] && [ ! -e "$SKZ_SEED_BEGAN" ]; then date +%s > "$SKZ_SEED_BEGAN"; fi\n'
RESUME = '    skz_minio_resume\n'
WARM = ('    log "$tag: warm-up ${WARMUP}s"\n'
        '    as_pg "\'$PGBENCH\' -h \'$SOCK\' -T $WARMUP -c $CLIENTS -j $JOBS -b $builtin bench" >/dev/null 2>&1\n')
UNDER = 'if [[ $began =~ ^[0-9]+$ ]] && [ "$prev" -gt "$began" ]; then'
ASBEGAN = ('                elif [[ $began =~ ^[0-9]+$ ]] && [ "$now" -ge "$began" ]; then\n'
           '                    how=" from the import, as the workload began (within $(( now - prev )) s of its warm-up)"\n')
TEARDOWN_UNPAUSE = '    if [ -n "$SKZ_MINIO_PAUSED" ]; then skz_minio unpause || true; SKZ_MINIO_PAUSED=""; fi\n'
WATCH = ('        rm -f "$SKZ_SEED_BEGAN"\n'
         '        SKZ_SEED_T0=$(date +%s); SKZ_SEED_INFO="$OUT/$TOPO.skzinfo"\n'
         '        ( SKZ_UPLOAD_WAIT_S=3600; skz_first_upload watch ) &\n'
         '        SKZ_SEED_WATCH=$!\n')
P = [
    # (1) bracket.py: not_uploading and the verdict's order
    ('B1 an epoch change taken as held', BK, ' or lease[0][1] != lease[1][1]', ''),
    ('B2 unsure taken as not held', BK, "LEASE_HELD = ('held', 'unsure')", "LEASE_HELD = ('held',)"),
    ('B3 zero uploads taken as an upload', BK, 'if int(a) - int(b) <= 0:', 'if int(a) - int(b) < 0:'),
    ('B4 a pid change ignored', BK, "if k.get('pid_same') is not True:", 'if False:'),
    ('B5 pid_same unknown (None) taken as the same', BK, "if k.get('pid_same') is not True:",
     "if k.get('pid_same') is False:"),
    ('B6 unreadable counters taken as zero', BK, "a, b = sa.get('uploaded', ''), sb.get('uploaded', '')",
     "a, b = sa.get('uploaded') or '0', sb.get('uploaded') or '0'"),
    ('B7 the row\'s reason after the enclosure\'s', BK,
     "out['verdict'] = idle or f'NO VERDICT: no {model}", "out['verdict'] = f'NO VERDICT: no {model}"),
    ('B8 an enclosed row that did not upload judged', BK,
     "    if idle:\n        out['verdict'] = idle\n        return out\n", ''),
    ('B9 its ratio not kept when it did not upload', BK,
     "    before, after = enclose(row, rows, model)\n",
     "    if idle:\n        out['verdict'] = idle\n        return out\n    before, after = enclose(row, rows, model)\n"),
    ('B10 a row that does not upload asked', BK, "    if not k.get('upload'):\n        return None\n", ''),
    ('B11 the lease asked at the end only', BK, 'for s in (sb, sa)]', 'for s in (sa, sa)]'),
    ('B12 the lease refused only when not held at both ends', BK,
     'if any(state not in LEASE_HELD', 'if all(state not in LEASE_HELD'),
    ('B13 the summary paragraph says nothing of upload rows', SH,
     "the machine moved by more than is judged while it ran. A row that says it uploads (TB_SKZ_UPLOAD) keeps its '",
     "the machine moved by more than is judged while it ran. A row that says it uploads (TB_SKZ_UPLOAD) has its '"),
    # (2) TB_SKZ_UPLOAD_FROM_LOAD
    ('M1 MinIO let go before the mark', SH, MARK + RESUME, RESUME + MARK),
    ('M2 MinIO let go after the first warm-up', SH, RESUME + WARM, WARM + RESUME),
    ('M3 no unpause at the teardown', SH, TEARDOWN_UNPAUSE, ''),
    ('M4 the teardown\'s unpause after the daemon\'s stop', SH, TEARDOWN_UNPAUSE + '    # A trace',
     '    # A trace'),  # second half below
    ('M5 the flag cleared when the unpause fails', SH,
     '        log "WARNING: MinIO could not be let go as the workload began; the seed waits"\n',
     '        SKZ_MINIO_PAUSED=""; log "WARNING: MinIO could not be let go as the workload began; the seed waits"\n'),
    ('M6 a wait other than 0 let through', SH,
     '    [ "$SKZ_UPLOAD_WAIT_S" = 0 ] || { echo "TB_SKZ_UPLOAD_FROM_LOAD=1 needs TB_SKZ_UPLOAD_WAIT_S=0, not $SKZ_UPLOAD_WAIT_S" >&2; exit 64; }\n', ''),
    ('M7 the seed from the load without uploads let through', SH,
     '    [ "$SKZ_UPLOAD" = 1 ] || { echo "TB_SKZ_UPLOAD_FROM_LOAD=1 needs TB_SKZ_UPLOAD=1" >&2; exit 64; }\n', ''),
    ('M8 TB_SKZ_UPLOAD_FROM_LOAD=yes let through', SH,
     '    [ "$SKZ_UPLOAD_FROM_LOAD" = 1 ] || { echo "TB_SKZ_UPLOAD_FROM_LOAD is 1 or unset, not: $SKZ_UPLOAD_FROM_LOAD" >&2; exit 64; }\n', ''),
    ('M9 MinIO never frozen', SH,
     '    skz_minio pause || { echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; return 1; }\n', ''),
    ('M10 the freeze not recorded (MinIO never let go until the teardown)', SH,
     '    SKZ_MINIO_PAUSED=1\n    skz_minio pause', '    skz_minio pause'),
    ('M11 every upload row frozen (the freeze on TB_SKZ_UPLOAD)', SH,
     'if [ -n "$SKZ_UPLOAD_FROM_LOAD" ]; then skz_minio_freeze', 'if [ -n "$SKZ_UPLOAD" ]; then skz_minio_freeze'),
    ('M12 never frozen (the freeze\'s condition false)', SH,
     'if [ -n "$SKZ_UPLOAD_FROM_LOAD" ]; then skz_minio_freeze', 'if false; then skz_minio_freeze'),
    ('M13 the row\'s record does not say it was frozen', SH,
     '    echo "upload_from_load 1" >> "$OUT/$TOPO.skzinfo"\n', ''),
    ('M14 the resume logs but does not unpause', SH,
     '    if skz_minio unpause; then\n        SKZ_MINIO_PAUSED=""\n        log "skyzfsd: MinIO answers',
     '    if true; then\n        SKZ_MINIO_PAUSED=""\n        log "skyzfsd: MinIO answers'),
    ('M15 the resume unpauses a MinIO that was not frozen', SH,
     '    [ -n "$SKZ_MINIO_PAUSED" ] || return 0\n    if skz_minio unpause',
     '    if skz_minio unpause'),
    ('M16 minio.sh pause does something else (stop)', MS,
     '    pause | unpause) docker "$1" minio > /dev/null ;;', '    pause | unpause) docker stop minio > /dev/null ;;'),
    ('M17 minio.sh down does not unpause', MS, '    docker unpause minio > /dev/null 2>&1 || true\n', ''),
    # (3) N1: the border between the rounds
    ('N1a an unread round moves the lower end', SH, '        [ -z "$up" ] || prev=$rs\n', '        prev=$rs\n'),
    ('N1b the lower end at the read\'s end, not its start', SH, '        [ -z "$up" ] || prev=$rs\n',
     '        [ -z "$up" ] || prev=$now\n'),
    ('N1c under the workload when the read that saw it was after the mark (the old rule)', SH, UNDER,
     'if [[ $began =~ ^[0-9]+$ ]] && [ "$now" -gt "$began" ]; then'),
    ('N1d no "as the workload began": before', SH, ASBEGAN, ''),
    ("N1e (D2) under at the mark's own second again (-ge)", SH, UNDER,
     'if [[ $began =~ ^[0-9]+$ ]] && [ "$prev" -ge "$began" ]; then'),
    ('N1f within N counted from the mark', SH, '(within $(( now - prev )) s of its warm-up)',
     '(within $(( now - began )) s of its warm-up)'),
    ('N1g the lower end not set at the start', SH, '    t0=$(date +%s); prev=$t0\n', '    t0=$(date +%s); prev=0\n'),
    ('N1h the range\'s low end from the read that saw it', SH, '$(( prev - began )) to $(( now - began ))',
     '$(( now - began )) to $(( now - began ))'),
    ('N1i the read\'s start taken after the read', SH, '        rs=$(date +%s)\n        all=$(skz_status)\n',
     '        all=$(skz_status)\n        rs=$(date +%s)\n'),
    # (4) N2: skz_seed_watch_stop and the mark
    ('W1 the NONE line not written', SH,
     '        echo "first_upload NONE by the end of the rows ($(( $(date +%s) - SKZ_SEED_T0 )) s after the import)" >> "$SKZ_SEED_INFO"\n',
     '        :\n'),
    ('W2 NONE written for a watcher that has ended', SH, '    if kill -0 "$SKZ_SEED_WATCH" 2>/dev/null; then\n',
     '    if true; then\n'),
    ('W3 the watcher not stopped', SH, '    kill "$SKZ_SEED_WATCH" 2>/dev/null; SKZ_SEED_WATCH=""\n',
     '    SKZ_SEED_WATCH=""\n'),
    ('W4 the watcher\'s pid not cleared', SH, '    kill "$SKZ_SEED_WATCH" 2>/dev/null; SKZ_SEED_WATCH=""\n',
     '    kill "$SKZ_SEED_WATCH" 2>/dev/null\n'),
    ('W5 the last row\'s mark not removed (F5)', SH, WATCH, WATCH.replace('        rm -f "$SKZ_SEED_BEGAN"\n', '')),
    ('W6 the last row\'s mark removed after the watcher starts', SH, WATCH,
     WATCH.replace('        rm -f "$SKZ_SEED_BEGAN"\n', '') + '        rm -f "$SKZ_SEED_BEGAN"\n'),
    ('W7 the teardown does not call it', SH, '    skz_seed_watch_stop\n    if [ -n "$SKZ_MINIO_PAUSED" ]',
     '    if [ -n "$SKZ_MINIO_PAUSED" ]'),
    ('W8 the NONE line goes nowhere: no file set (F9)', SH,
     'SKZ_SEED_T0=$(date +%s); SKZ_SEED_INFO="$OUT/$TOPO.skzinfo"', 'SKZ_SEED_T0=$(date +%s)'),
    ('W9 the import\'s time not set: NONE counts from 0', SH,
     'SKZ_SEED_T0=$(date +%s); SKZ_SEED_INFO="$OUT/$TOPO.skzinfo"', 'SKZ_SEED_INFO="$OUT/$TOPO.skzinfo"'),
    ('W10 the mark rewritten at every warm-up (F4)', SH, MARK,
     '    if [ -n "$SKZ_SEED_WATCH" ]; then date +%s > "$SKZ_SEED_BEGAN"; fi\n'),
    ('F1 the mark written after the first warm-up', SH, MARK + RESUME + WARM, RESUME + WARM + MARK),
    ('F6 the watcher\'s first line not written', SH,
     'echo "first_upload not committed by the window\'s end (watched from the import)" >> "$OUT/$TOPO.skzinfo"', ':'),
    # (5) the new code at 73ebf56
    ('S1 the status-read check removed', BK, "    if unread:\n        return f\"its daemon's status", "    if False:\n        return f\"its daemon's status"),
    ('S2 the status asked at the end only', BK, "for end, s in (('start', sb), ('end', sa))", "for end, s in (('end', sa),)"),
    ('S3 the status asked at the start only', BK, "for end, s in (('start', sb), ('end', sa))", "for end, s in (('start', sb),)"),
    ('S4 a read known by its clock line, not its pid', BK, "if not s.get('pid')]", "if not s.get('read_at_ns')]"),
    ('S5 the reason names the first end only', BK, "{' and '.join(unread)}", "{unread[0]}"),
    ('S6 the status asked only when the pid differs', BK, "if not s.get('pid')]\n    if unread:", "if not s.get('pid')]\n    if unread and k.get('pid_same') is False:"),
    ('E1 (D1) an epoch that is not a number taken as one epoch', BK, "\n            or not str(lease[0][1]).isdigit())", ")"),
    ('E2 (D1) an epoch asked to be a number only when the two differ', BK, "lease[0][1] != lease[1][1]\n            or not str(lease[0][1]).isdigit())",
     "(lease[0][1] != lease[1][1]\n            and not str(lease[0][1]).isdigit()))"),
    ('G1 no upload part for a row whose end was not read (Codex P2 back)', SH, "if not skz[base + '.after'] and upload_of(skzinfo()):", "if False:"),
    ('G2 that part says the pid was the same', SH, "'after': {}, 'pid_same': None, 'info': info,", "'after': {}, 'pid_same': True, 'info': info,"),
    ('G3 that part does not say the status was not read', SH, """ + " (THE DAEMON'S STATUS WAS NOT READ at the window's end)"}""", "}"),
    ('G4 that part for every row whose end was not read', SH, "if not skz[base + '.after'] and upload_of(skzinfo()):", "if not skz[base + '.after']:"),
    ("G5 that part's end a copy of its start", SH, "'after': {}, 'pid_same': None, 'info': info,", "'after': skz[base + '.before'], 'pid_same': None, 'info': info,"),
    ('G6 the frozen MinIO not said in the upload', SH, "    if info.get('upload_from_load') == '1':\n        u['from_load'] = True\n", ""),
    ('G7 the generator takes every pid as the same', SH, "'pid_same': sb.get('pid') == sa.get('pid'), 'info': {}}", "'pid_same': True, 'info': {}}"),
    ('Z1 (D4) the flag set after the pause', SH,
     '    SKZ_MINIO_PAUSED=1\n    skz_minio pause || { echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; return 1; }\n',
     '    skz_minio pause || { echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; return 1; }\n    SKZ_MINIO_PAUSED=1\n'),
    ('Z2 a failed pause taken as a freeze', SH, 'echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; return 1; }',
     'echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; }'),
    ('Z3 a failed pause clears the flag', SH, '{ echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; return 1; }',
     '{ SKZ_MINIO_PAUSED=""; echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; return 1; }'),
    ('Z4 the record says frozen before the pause', SH,
     '    skz_minio pause || { echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; return 1; }\n    echo "upload_from_load 1" >> "$OUT/$TOPO.skzinfo"\n',
     '    echo "upload_from_load 1" >> "$OUT/$TOPO.skzinfo"\n    skz_minio pause || { echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; return 1; }\n'),
    ('Z5 the build goes on when the freeze fails', SH, 'then skz_minio_freeze || return 1; fi', 'then skz_minio_freeze || true; fi'),
    ('Z6 the build freezes after the daemon starts', SH,
     '            if [ -n "$SKZ_UPLOAD_FROM_LOAD" ]; then skz_minio_freeze || return 1; fi\n            SKZ_UP=1\n            skz_start\n',
     '            SKZ_UP=1\n            skz_start\n            if [ -n "$SKZ_UPLOAD_FROM_LOAD" ]; then skz_minio_freeze || return 1; fi\n'),
    ('Z7 the freeze does not log', SH, '    log "skyzfsd: MinIO frozen until the workload begins"\n', ''),
    ('Z8 the freeze unpauses instead of pausing', SH,
     '    skz_minio pause || { echo "MinIO could not be frozen', '    skz_minio unpause || { echo "MinIO could not be frozen'),
]
first = int(sys.argv[2]) if len(sys.argv) > 2 else 0
last = int(sys.argv[3]) if len(sys.argv) > 3 else len(P)
todo = P[first:last]
base = {k: run(c) for f in {p for _, p, _, _ in todo} for k, c in TESTS[f]}
assert all(v == 0 for v in base.values()), base
for name, path, old, new in todo:
    full = f'{tree}/{path}'
    text = open(full).read()
    if text.count(old) != 1:
        print(f'{name}: NOT APPLIED ({text.count(old)} matches)', flush=True)
        continue
    planted = text.replace(old, new)
    if name.startswith('M4 '):
        # The unpause moved to after the daemon's stop (and the ublk device's removal).
        anchor = '    for kv in $ZFS_PARAMS_SAVED; do\n'
        assert planted.count(anchor) == 1
        planted = planted.replace(anchor, TEARDOWN_UNPAUSE + anchor)
    try:
        open(full, 'w').write(planted)
        r = {k: run(c) for k, c in TESTS[path]}
    finally:
        open(full, 'w').write(text)
    died = [f'{k}({v})' if v == 'TIMEOUT' else k for k, v in r.items() if v != 0]
    print(f"{name}: {'KILLED by ' + ', '.join(died) if died else 'SURVIVED'}", flush=True)
st = subprocess.run(['git', 'status', '--porcelain'], cwd=tree, capture_output=True, text=True).stdout
print('tree clean' if not st.strip() else f'TREE NOT CLEAN:\n{st}', flush=True)
```
