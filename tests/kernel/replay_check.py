#!/usr/bin/env python3
"""Power-cut replay check: does the pool import at every prefix of the write log?

The recording half of the rig (replay-rig.sh) ran a workload on a pool
whose data vdev sat on dm-log-writes, marked the log at each completed
`zpool sync`, and noted which files existed at each mark. This half
rebuilds the device as it was after N records, for many N, and checks
each one:

  light check, every sampled prefix
      a read-only loop device over the image; `zpool import -o
      readonly=on -N -m -f` must succeed; the dataset must mount; every
      file recorded at the last mark at or before the prefix must have
      its checksum.
  deep check, at flush boundaries
      a throwaway dm-snapshot over the image; a writable import and
      `zpool scrub -w`, which reads every block the pool can reach;
      no error may be counted and nothing repaired.

Which prefixes: every flush boundary, every prefix within WINDOW records
of an uberblock write, and random others, as many as the time budget
allows after a short calibration. One image is advanced record by
record; the checks are serial, because one kernel cannot import the
same pool twice.

Negative controls. A rig that cannot catch a wrong replay proves
nothing, so two wrong replays are run through the same checks:
  drop      one data write from the middle of a transaction group is
            never applied
  reorder   the last data write before the flush that precedes a
            group's uberblocks is held back until that group is
            complete, so the uberblocks land without it
The victims are picked by a fixed rule (see controls()), not chosen to
be caught. What each is expected to do today is the table EXPECTED; the
job fails when a result differs from it in either direction. Today the
rig shows that the pool imports and that the promised files are intact
at every prefix. It does not yet show that every block the newest
uberblock reaches is on the device in every copy: `reorder`, whose
block is one of three copies of pool metadata, is not caught.

The image comes from a provider (default logwrites.py over the
dm-log-writes log; see its header for the two commands). The same checks
will run on images from skyzfsd's own log replay.

Needs root, ZFS, losetup, dmsetup with dm-snapshot.
"""
import argparse
import hashlib
import json
import math
import os
import random
import re
import shlex
import shutil
import statistics
import struct
import subprocess
import sys
import time

try:
    import numpy as np
except ImportError:          # the checksum then runs in plain Python, slowly
    np = None

WINDOW = 50
HERE = os.path.dirname(os.path.abspath(__file__))


# ----------------------------------------------------------- pure logic
def parse_marks(records):
    """-> [(prefix at which the mark is in the log, sync number, files complete)], in order.

    A mark reads sync-K-C: the K-th sync, C files complete before it began."""
    marks = []
    for i, r in enumerate(records):
        text = r.get('mark', '')
        f = text.split('-')
        if len(f) == 3 and f[0] == 'sync' and f[1].isdigit() and f[2].isdigit():
            marks.append((i + 1, int(f[1]), int(f[2])))
    return marks


def files_at(marks, n):
    """Files that must verify at prefix n: those of the last mark at or before it."""
    c = 0
    for pos, _, count in marks:
        if pos > n:
            break
        c = count
    return c


def flush_points(records):
    return [i + 1 for i, r in enumerate(records) if 'flush' in r['flags']]


def ub_clusters(records, window=WINDOW):
    """Uberblock writes, grouped where their windows touch -> [(first prefix, last prefix)]."""
    total = len(records)
    out = []
    for i, r in enumerate(records):
        if not r.get('ub'):
            continue
        lo, hi = max(0, i + 1 - window), min(total, i + 1 + window)
        if out and lo <= out[-1][1] + 1:
            out[-1] = (out[-1][0], hi)
        else:
            out.append((lo, hi))
    return out


def group_points(records):
    """Where the every-copy check belongs -> [(first, last)], one pair per transaction group.

    The blocks the newest uberblock reaches change only when a new
    uberblock lands. `first` is the prefix right after a group's first
    uberblock write: the earliest moment its tree is the live one, so
    everything it reaches must already be there. `last` is the prefix
    just before the next group's first uberblock write: the latest
    moment the same tree is still the one a power cut falls back on, so
    nothing written since may have damaged it.
    """
    firsts, after_ub = [], False
    for i, r in enumerate(records):
        if 'pos' not in r:
            continue                    # a flush, a mark or a discard does not end a run of uberblock writes
        if r.get('ub') and not after_ub:
            firsts.append(i)
        after_ub = bool(r.get('ub'))
    return [(f + 1, firsts[k + 1] if k + 1 < len(firsts) else len(records)) for k, f in enumerate(firsts)]


def file_pattern(n):
    """The 4 KiB that start file f.<n> of the rig's workload (see replay-rig.sh)."""
    return (b'SKZ-FILE-%06d\n' % n) * 256


def writes_carrying(log_path, records, pattern):
    """Numbers of the write records whose data contains `pattern`."""
    hits = []
    with open(log_path, 'rb') as f:
        for r in records:
            if 'pos' not in r or r['bytes'] < len(pattern):
                continue
            f.seek(r['pos'])
            if pattern in f.read(r['bytes']):
                hits.append(r['i'])
    return hits


def spread(items, k):
    """k of the items, evenly spaced, keeping the first and the last."""
    items = list(items)
    if k >= len(items):
        return items
    if k <= 0:
        return []
    if k == 1:
        return [items[len(items) // 2]]
    return [items[round(j * (len(items) - 1) / (k - 1))] for j in range(k)]


def plan(records, budget_points, deep_points, seed=1, window=WINDOW):
    """Choose the prefixes to check -> (kind by prefix, deep set, what was available).

    Priority: the two ends and every flush boundary; then whole windows
    around uberblock writes, for at most 60% of what is left; then random
    prefixes. A prefix that is several things counts as the first, and a
    random pick that falls inside a window counts as the window's: when
    uberblock writes are closer together than the window, the windows
    merge and may be too large to take whole.
    """
    total = len(records)
    flush = flush_points(records)
    clusters = ub_clusters(records, window)
    in_window = set()
    for lo, hi in clusters:
        in_window.update(range(lo, hi + 1))
    # The two ends are their own kind, whatever else they are.
    avail = {'records': total, 'prefixes': total + 1, 'flush': len(set(flush) - {0, total}),
             'ub_writes': sum(1 for r in records if r.get('ub')),
             'ub_window': len(in_window - set(flush) - {0, total}),
             'other': total + 1 - len(in_window | set(flush) | {0, total})}
    kind = {0: 'end', total: 'end'}
    for n in spread(flush, max(0, budget_points - len(kind))):
        kind.setdefault(n, 'flush')
    left = budget_points - len(kind)
    quota = int(left * 0.6)
    order = spread(range(len(clusters)), len(clusters))
    # Whole windows, evenly spread over the log, while they fit.
    picked, used = [], 0
    for step in range(1, len(clusters) + 1):
        cand = spread(order, step)
        size = sum(len([n for n in range(clusters[c][0], clusters[c][1] + 1) if n not in kind]) for c in cand)
        if size > quota:
            break
        picked, used = cand, size
    for c in picked:
        for n in range(clusters[c][0], clusters[c][1] + 1):
            kind.setdefault(n, 'ub_window')
    left = budget_points - len(kind)
    rest = [n for n in range(total + 1) if n not in kind]
    rng = random.Random(seed)
    for n in rng.sample(rest, min(max(left, 0), len(rest))):
        kind[n] = 'ub_window' if n in in_window else 'random'
    deep = set(spread([n for n in sorted(kind) if kind[n] == 'flush'], deep_points))
    return kind, deep, avail


def data_writes(records, lo, hi):
    """Record numbers in [lo, hi) that are writes outside the labels."""
    return [i for i in range(lo, hi) if 'pos' in records[i] and not records[i].get('label')]


def controls(records, marks, find=None):
    """The wrong replays -> list of dicts with a schedule and the prefixes to check.

    `find(file number)` gives the write records that carry the first
    block of that file; without it the `data` control is not built.

    Fixed rule, on the transaction group of the middle mark:
      A      the last flush before that group's first uberblock write
      drop   the median data write between the previous mark and A is
             left out for good. Checked from there to the mark after
             the middle one, so that the files of the group are due.
      reorder  the last data write before A is moved to just before the
             middle mark, after the group's uberblocks and flushes.
             Checked at every prefix from A to the mark.
      data   the last write before the middle mark that carries the
             first block of the last file that mark promises is left
             out for good. A file's block has one copy, and the file is
             due from the mark on: this is the control that says what
             the light check catches by itself. Checked as `drop` is.
    """
    if len(marks) < 4:
        return []
    m = len(marks) // 2
    lo, hi = marks[m - 1][0], marks[m][0] - 1          # record numbers of the group; hi is the mark
    ubs = [i for i in range(lo, hi) if records[i].get('ub')]
    if not ubs:
        return []
    flushes = [i for i in range(lo, ubs[0]) if 'flush' in records[i]['flags']]
    if not flushes:
        return []
    a = flushes[-1]
    writes = data_writes(records, lo, a)
    if not writes:
        return []
    end = marks[m + 1][0] if m + 1 < len(marks) else len(records)
    natural = list(range(len(records)))
    out = []

    victim = writes[len(writes) // 2]
    sched = [i for i in natural if i != victim]
    pts = set(range(a, hi + 1))                         # around the group's own uberblocks
    pts.update(n for n in range(victim, end) if 'flush' in records[sched[n - 1]]['flags'])
    pts.update(spread(range(victim, end), 40))
    pts.add(end - 1)
    out.append({'name': 'drop', 'victim': victim, 'schedule': sched, 'points': sorted(p for p in pts if p >= victim),
                'what': f'record {victim} (a write of {records[victim]["bytes"]} bytes at byte {records[victim]["offset"]}) is never applied'})

    victim = writes[-1]
    sched = [i for i in natural if i != victim]
    sched.insert(hi - 1, victim)                        # just before the mark
    pts = sorted(range(a, hi + 1))
    out.append({'name': 'reorder', 'victim': victim, 'schedule': sched, 'points': pts,
                'what': f'record {victim} (a write of {records[victim]["bytes"]} bytes at byte {records[victim]["offset"]}, '
                        f'the last data write before flush {a}) is applied only after the uberblocks of its transaction group'})

    wanted = marks[m][2] - 1                            # the last file the middle mark promises
    hits = [i for i in find(wanted) if i < hi] if find and wanted >= 0 else []
    if hits:
        victim = hits[-1]
        sched = [i for i in natural if i != victim]
        pts = set(range(max(victim, hi - 20), hi + 2))  # around the mark from which the file is due
        pts.update(n for n in range(victim, end) if 'flush' in records[sched[n - 1]]['flags'])
        pts.update(spread(range(victim, end), 40))
        pts.add(end - 1)
        out.append({'name': 'data', 'victim': victim, 'schedule': sched, 'points': sorted(p for p in pts if p >= victim),
                    'what': f'record {victim} (a write of {records[victim]["bytes"]} bytes at byte {records[victim]["offset"]}, '
                            f'the last one before the middle mark that carries the first block of file f.{wanted}) is never applied'})
    return out


# What each control is expected to do today. The job fails when a result
# differs from this table. Do not edit a control to fit the table.
#   by: 'any'     caught, by whichever check
#       a check   caught, and by that check among those that caught it:
#                 'copies' (the every-copy check), 'verify' (the files'
#                 checksums in the light check), 'deep' (the scrub)
#       None      not caught: a known gap, stated in every run's summary
#                 under 'gap'. Strict in both directions: once a check
#                 catches it, the job fails until this table is changed
#                 on purpose.
# History: `reorder` was None until the every-copy check existed. Its
# victim is one copy of several of pool metadata; the import reads
# another copy and the scrub runs after a writable import, so neither
# sees it.
EXPECTED = {
    'drop': {'by': 'any'},
    'reorder': {'by': 'copies'},
    'data': {'by': 'verify'},
}
CHECK_NAMES = {'deep': 'the scrub', 'copies': 'the every-copy check', 'verify': "the files' checksums",
               'import': 'the import', 'mount': 'the mount'}

# A failed check says something about the image only when the stage that
# failed is one of these. Anything else (a loop device, the snapshot, an
# export, a command that timed out or could not start) is the rig
# failing: never a caught control, and never a pass.
JUDGING = {'import', 'mount', 'verify', 'deep: import (writable)', 'deep: scrub', 'copies: zdb', 'copies: differ'}
RIG_RC = (124, 127)             # run(): timed out, could not start

# The deep check must run at no less than this share of the flush
# boundaries. Half, evenly spread: the one fault the deep check has
# caught so far (the drop control) showed at 2 of the 6 to 8 flush
# boundaries after it, so a thinner sample could step over such a fault.
# In practice the budget covers every boundary, and a run in which any
# boundary goes without says so in a warning.
DEEP_FLOOR = 0.5


# ------------------------------------------------- the every-copy check
# Is every block that the newest uberblock reaches on the device, in
# every copy? Nothing is imported and nothing is written:
#   1. `zdb -e -bbbbb` walks everything reachable from the newest
#      uberblock, checks that nothing is leaked, and prints every block
#      pointer: where each copy is, its size, and its checksum.
#   2. The rig reads every copy of every block from the image and
#      computes its checksum itself. zdb cannot do this part: when it
#      verifies checksums it is content with one good copy (it reads as
#      a scrub does, and the read succeeds while any copy is good), so
#      its exit code does not see one bad copy among several.
# The list must be provably complete: pointers with addresses plus
# embedded ones must equal zdb's own count. A ganged block, a checksum
# the rig cannot compute, a line it cannot read, or a zdb that dies or
# times out stops the check as a rig error; none of them is ever a pass
# or a verdict.
# Not covered: the uberblock's own four label copies (the import
# chooses among them), and intent-log blocks, whose checksum is inside
# the block and whose chains may end early by design.
DVA_OFFSET = 0x400000           # a DVA's offset counts from the end of the two front labels
DVA_RE = re.compile(r'DVA\[(\d)\]=<(\d+):([0-9a-f]+):([0-9a-f]+)>')
SIZE_RE = re.compile(r'size=([0-9a-f]+)L/([0-9a-f]+)P')
CKSUM_RE = re.compile(r'cksum=([0-9a-f]+):([0-9a-f]+):([0-9a-f]+):([0-9a-f]+)')
KIND_RE = re.compile(r'\[L(-?\d+) ([^\]]+)\] (\S+) ')
M64 = (1 << 64) - 1


def fletcher4(data):
    """ZFS's fletcher4 over little-endian 32-bit words -> four 64-bit sums."""
    n = len(data) // 4
    if np is not None and n < (1 << 20):
        w = np.frombuffer(data, dtype='<u4', count=n).astype(np.uint64)
        k = np.arange(n, 0, -1, dtype=np.uint64)             # how many more times a word is added to the next sum
        with np.errstate(over='ignore'):
            return (int(w.sum(dtype=np.uint64)), int((w * k).sum(dtype=np.uint64)),
                    int((w * (k * (k + np.uint64(1)) // np.uint64(2))).sum(dtype=np.uint64)),
                    int((w * (k * (k + np.uint64(1)) * (k + np.uint64(2)) // np.uint64(6))).sum(dtype=np.uint64)))
    a = b = c = d = 0
    for (x,) in struct.iter_unpack('<I', data[:n * 4]):
        a += x
        b += a
        c += b
        d += c
    return a & M64, b & M64, c & M64, d & M64


def zdb_blocks(text):
    """Read `zdb -bbbbb` -> block pointers and what is needed to prove the list complete."""
    out = {'bps': [], 'embedded': 0, 'holes': 0, 'ganged': 0, 'unread': [], 'bp_count': None, 'ganged_count': None}
    for line in text.splitlines():
        m = re.match(r'\s*(bp count|ganged count):\s+(\d+)', line)
        if m:
            out['bp_count' if m.group(1) == 'bp count' else 'ganged_count'] = int(m.group(2))
            continue
        if not line.startswith('objset '):
            continue
        if ' EMBEDDED ' in line:
            out['embedded'] += 1
            continue
        if ' HOLE ' in line:
            out['holes'] += 1
            continue
        dvas, size, ck, kind = DVA_RE.findall(line), SIZE_RE.search(line), CKSUM_RE.search(line), KIND_RE.search(line)
        if not dvas or not size or not ck or not kind or [int(k) for k, _, _, _ in dvas] != list(range(len(dvas))):
            out['unread'].append(line.strip()[:300])
            continue
        if ' gang ' in line:
            out['ganged'] += 1
        out['bps'].append({'line': line.strip()[:260], 'type': kind.group(2), 'level': int(kind.group(1)),
                           'checksum': kind.group(3), 'little_endian': ' LE ' in line,
                           'dvas': [(int(v), int(off, 16), int(asize, 16)) for _, v, off, asize in dvas],
                           'psize': int(size.group(2), 16), 'cksum': tuple(int(x, 16) for x in ck.groups())})
    return out


def bad_copies(image, bps):
    """Check every copy of every block -> (counts, copies whose bytes do not pass the block's checksum)."""
    counts = {'blocks': 0, 'copies': 0, 'intent_log': 0, 'with_copies': 0}
    bad = []
    with open(image, 'rb') as f:
        for bp in bps:
            if bp['type'] == 'ZIL intent log':
                counts['intent_log'] += 1
                continue
            counts['blocks'] += 1
            counts['with_copies'] += len(bp['dvas']) > 1
            for k, (_, off, _) in enumerate(bp['dvas']):
                f.seek(off + DVA_OFFSET)
                data = f.read(bp['psize'])
                counts['copies'] += 1
                if len(data) != bp['psize'] or fletcher4(data) != bp['cksum']:
                    bad.append({'copy': k, 'of': len(bp['dvas']), 'at': off + DVA_OFFSET, 'line': bp['line']})
    return counts, bad


def every_copy_verdict(rc, text, image):
    """zdb's exit code and listing, and the image -> (stage that failed or None, detail, counts)."""
    counts = {}
    if rc in RIG_RC:
        return stage('zdb', rc), text.strip()[-300:], counts
    if rc < 0:
        said = [ln.strip() for ln in text.splitlines() if re.search(r'PANIC|ASSERT|VERIFY', ln)][:2]
        return f'rig: zdb died with signal {-rc}', ' | '.join(said)[:400], counts
    if rc:
        said = [ln.strip()[:200] for ln in text.splitlines() if re.search(r'rror|leak|cannot|failed', ln)][:3]
        return 'copies: zdb', f'zdb exit code {rc}: ' + ' | '.join(said), counts
    b = zdb_blocks(text)
    counts.update(listed=len(b['bps']), embedded=b['embedded'], bp_count=b['bp_count'])
    if b['unread']:
        return 'rig: zdb printed a block pointer the rig cannot read', b['unread'][0], counts
    if b['bp_count'] is None or b['ganged_count'] is None:
        return 'rig: zdb printed no block counts', '', counts
    if len(b['bps']) + b['embedded'] != b['bp_count']:
        return ('rig: the list of block pointers is not complete',
                f"{len(b['bps'])} with addresses + {b['embedded']} embedded, but zdb counted {b['bp_count']}", counts)
    if b['ganged'] or b['ganged_count']:
        return 'rig: ganged blocks, whose copies this check cannot read', f"{b['ganged_count']} ganged", counts
    for bp in b['bps']:
        if any(v != 0 for v, _, _ in bp['dvas']):
            return 'rig: a block on a device other than the image', bp['line'], counts
        if bp['type'] != 'ZIL intent log' and (bp['checksum'] != 'fletcher4' or not bp['little_endian']):
            return 'rig: a block whose checksum the rig cannot compute', bp['line'], counts
    c, bad = bad_copies(image, b['bps'])
    counts.update(c)
    if bad:
        x = bad[0]
        return ('copies: differ', f"{len(bad)} of {c['copies']} copies do not pass their block's checksum; the first is "
                f"copy {x['copy']} of {x['of']} at byte {x['at']}: {x['line']}", counts)
    return None, '', counts


def stage(name, rc):
    """The stage to report for a failed command: its own name, unless the command never judged anything."""
    if rc == 124:
        return f'rig: {name} timed out'
    if rc == 127:
        return f'rig: {name} could not start'
    return name


def tally_control(results):
    """Results of a control's points -> (failures by judging stage, rig errors, first judging failure)."""
    by, rig_errors, first = {}, [], None
    for r in results:
        if r['ok']:
            continue
        if r['failed'] in JUDGING:
            key = r['failed'].split(': ')[0] if ': ' in r['failed'] else r['failed']
            by[key] = by.get(key, 0) + 1
            if first is None:
                first = {'n': r['n'], 'failed': r['failed'], 'detail': r['detail']}
        else:
            rig_errors.append({'n': r['n'], 'failed': r['failed'], 'detail': r['detail']})
    return by, rig_errors, first


def coverage(records, points, floor=DEEP_FLOOR, groups=None):
    """What the pass over `points` left out -> dict with 'errors' (fail the job) and 'warnings'.

    Every flush boundary must have been checked, and the scrub must
    have run at `floor` of them. With `groups` (see group_points), the
    every-copy check must have run at the first point of every
    transaction group; how many last points had it is reported.
    """
    flush = set(flush_points(records))
    checked = {p['n'] for p in points}
    deep = {p['n'] for p in points if 't_deep' in p}
    missed = sorted(flush - checked)
    need = math.ceil(len(flush) * floor)
    cov = {'flush': len(flush), 'flush_unchecked': missed, 'deep': len(flush & deep), 'deep_floor': need,
           'errors': [], 'warnings': []}
    if missed:
        cov['errors'].append(f'{len(missed)} of {len(flush)} flush boundaries were not checked (first: prefix {missed[0]})')
    if len(flush & deep) < need:
        cov['errors'].append(f'the deep check ran at {len(flush & deep)} of {len(flush)} flush boundaries, '
                             f'below the floor of {need}')
    elif len(flush & deep) < len(flush):
        cov['warnings'].append(f'the deep check ran at {len(flush & deep)} of {len(flush)} flush boundaries; '
                               f'{len(flush) - len(flush & deep)} went without it')
    if groups is not None:
        copies = {p['n'] for p in points if 't_copies' in p}
        firsts, lasts = {f for f, _ in groups}, {la for _, la in groups}
        cov.update(groups=len(groups), group_firsts=len(firsts & copies), group_lasts=len(lasts & copies),
                   group_lasts_total=len(lasts))
        missed = sorted(firsts - copies)
        if not groups:
            cov['errors'].append('the log shows no transaction group: no uberblock write was recognised')
        if missed:
            cov['errors'].append(f'the every-copy check did not run at the first point of {len(missed)} of {len(firsts)} '
                                 f'transaction groups (first: prefix {missed[0]})')
        if len(lasts & copies) < len(lasts):
            cov['warnings'].append(f'the every-copy check ran at the last point of {len(lasts & copies)} of {len(lasts)} '
                                   f'transaction groups; the rest went without it for time')
    return cov


def witness_verdict(witness, required):
    """-> (ok, text). No comparison at all passes only when the witness was not required."""
    if not witness:
        return (not required), 'witness not run' + (' although it is required' if required else '')
    if all(w['same'] for w in witness):
        return True, 'same image at every compared prefix'
    return False, 'DIFFERENT image'


def manifest_trouble(marks, n_files):
    """Why the marks and the manifest cannot be checked against each other, or None."""
    if not marks:
        return 'the log has no sync marks: nothing to check against'
    worst = max(c for _, _, c in marks)
    if worst > n_files:
        return f'a mark promises {worst} files but the manifest lists {n_files}'
    if marks[-1][2] == 0:
        return 'the last mark promises no files: the checks would verify nothing'
    return None


def judge_controls(ctl, expected=None):
    """Compare the controls' results with the table -> (rows, gaps, all as expected)."""
    expected = EXPECTED if expected is None else expected
    seen = {c['name']: c for c in ctl}
    rows, gaps, ok = [], [], True
    for name in sorted(set(expected) | set(seen)):
        want, c = expected.get(name), seen.get(name)
        if c is None:
            got = 'not run'
        elif c.get('rig_errors'):
            e = c['rig_errors'][0]
            got = f"the rig failed at {len(c['rig_errors'])} prefix(es), first at {e['n']}: {e['failed']}"
        elif not c['caught']:
            got = 'not caught'
        else:
            got = 'caught by ' + ', '.join(CHECK_NAMES.get(k, k) for k in sorted(c['failed_by']))
        clean = bool(c and c['caught'] and not c.get('rig_errors'))
        if want is None:
            exp, match = 'not in the table', False
        elif want['by'] is None:
            exp, match = 'not caught', got == 'not caught'
        elif want['by'] == 'any':
            exp, match = 'caught', clean
        else:
            exp, match = 'caught, and by ' + CHECK_NAMES[want['by']], clean and want['by'] in c['failed_by']
        ok = ok and match
        rows.append({'name': name, 'expected': exp, 'observed': got, 'match': match})
        if want and want.get('gap') and match:
            gaps.append({'name': name, 'gap': want['gap']})
    return rows, gaps, ok


def blocks_at(bps, lo, hi):
    """Which listed blocks have a copy in device bytes [lo, hi) -> short descriptions."""
    out = []
    for bp in bps:
        for k, (_, off, asize) in enumerate(bp['dvas']):
            if off + DVA_OFFSET < hi and lo < off + DVA_OFFSET + asize:
                obj = re.match(r'objset (\d+) object (\d+) level (-?\d+)', bp['line'])
                out.append({'copy': k, 'of': len(bp['dvas']), 'type': bp['type'], 'level': bp['level'],
                            'objset': int(obj.group(1)) if obj else None, 'object': int(obj.group(2)) if obj else None})
    return out


def victim_facts(log_path, records, victim, base_path=None):
    """What can be said from the log alone about the block a control withholds.

    A control that is not caught needs explaining, and these are the
    cheap explanations: the write changed nothing, the same block went
    to another place as well (ZFS keeps two or three copies of its
    metadata), or it was overwritten soon after.
    """
    v = records[victim]
    lo, hi = v['offset'], v['offset'] + v['bytes']
    facts = {'copies': [], 'earlier': None, 'later': None, 'same_as_before': None}
    with open(log_path, 'rb') as f:
        f.seek(v['pos'])
        data = f.read(v['bytes'])
        facts['zeros'] = not any(data)
        for r in records:
            if 'pos' not in r:
                continue
            overlaps = r['offset'] < hi and lo < r['offset'] + r['bytes']
            if r['i'] != victim and overlaps:
                if r['i'] < victim:
                    facts['earlier'] = r['i']
                elif facts['later'] is None:
                    facts['later'] = r['i']
            if r['bytes'] < len(data):
                continue
            f.seek(r['pos'])
            payload = f.read(r['bytes'])
            at = payload.find(data)
            while at >= 0:
                if not (r['i'] == victim and at == 0) and len(facts['copies']) < 16:
                    facts['copies'].append({'record': r['i'], 'offset': r['offset'] + at})
                at = payload.find(data, at + len(data))
        before = None
        e = facts['earlier']
        if e is not None and records[e]['offset'] <= lo and hi <= records[e]['offset'] + records[e]['bytes']:
            f.seek(records[e]['pos'] + lo - records[e]['offset'])
            before = f.read(len(data))
        elif e is None and base_path:
            with open(base_path, 'rb') as b:
                b.seek(lo)
                before = b.read(len(data))
        if before is not None:
            facts['same_as_before'] = before == data
    return facts


def facts_text(facts):
    c = facts['copies']
    parts = ['all zeros' if facts['zeros'] else None,
             'the device already held the same bytes there' if facts['same_as_before'] else None,
             (f"the same bytes are also written at {len(c)} other place(s): "
              + ', '.join(f"record {x['record']} at byte {x['offset']}" for x in c[:4])) if c else 'no other write in the log carries the same bytes',
             f"that place is next written by record {facts['later']}" if facts['later'] is not None else 'that place is not written again',
             f"and was last written by record {facts['earlier']}" if facts['earlier'] is not None else 'and was not written earlier in the log']
    text = '; '.join(x for x in parts if x)
    b = facts.get('blocks')
    if b:
        text += '. By zdb, on the true image, it holds ' + '; '.join(
            f"copy {x['copy'] + 1} of {x['of']} of a level-{x['level']} `{x['type']}` block (objset {x['objset']}, object {x['object']})"
            for x in b[:6])
    elif b is not None:
        text += '. zdb lists no live block there on the true image at the end of the checked range'
    return text


# ------------------------------------------------------------- running
def run(cmd, timeout=300):
    """Run a command -> (exit code, output, seconds). Never raises for a command that hangs or cannot start."""
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors='replace',
                           timeout=timeout)
        return p.returncode, p.stdout, time.monotonic() - t0
    except subprocess.TimeoutExpired as e:
        # What was captured before a timeout is bytes, whatever text= says.
        out = e.stdout or ''
        if isinstance(out, bytes):
            out = out.decode(errors='replace')
        return 124, out + f'\nTIMEOUT after {timeout}s', time.monotonic() - t0
    except OSError as e:
        return 127, f'{cmd[0]}: {e}', time.monotonic() - t0


class Rig:
    def __init__(self, args):
        self.a = args
        self.provider = shlex.split(args.provider) if args.provider else [sys.executable, os.path.join(HERE, 'logwrites.py')]
        self.index_path = os.path.join(args.work, 'index.json')
        self.imgdir = os.path.join(args.work, 'img')
        self.image = os.path.join(self.imgdir, 'replay.img')
        self.alt = os.path.join(args.work, 'alt')
        with open(args.manifest) as f:
            self.manifest = [line.split()[:2] for line in f if line.strip()]
        self.at = 0
        self.sched_path = None
        os.makedirs(self.imgdir, exist_ok=True)
        os.makedirs(self.alt, exist_ok=True)
        # zdb cannot open a pool without its log device. Where the log
        # device is not under test, its image from the time of the base
        # image stands beside every prefix: the pool opens, and its
        # intent-log chains simply end there.
        self.logdev = getattr(args, 'log_device_image', '')
        if self.logdev:
            rc, out, _ = run(['cp', '--sparse=always', self.logdev, os.path.join(self.imgdir, 'log-device.img')])
            if rc:
                sys.exit(f'copying the log device image failed: {out}')

    # image provider
    def make_index(self):
        rc, out, _ = run(self.provider + ['index', '--log', self.a.log, '--dev-bytes', str(os.path.getsize(self.a.base)),
                                          '--out', self.index_path])
        if rc:
            sys.exit(f'provider index failed: {out}')
        with open(self.index_path) as f:
            return json.load(f)

    def reset(self, schedule=None):
        """A fresh image at prefix 0: a copy of the base."""
        if os.path.exists(self.image):
            os.unlink(self.image)
        rc, out, _ = run(['cp', '--sparse=always', self.a.base, self.image])
        if rc:
            sys.exit(f'copying the base image failed: {out}')
        self.at = 0
        self.sched_path = None
        if schedule is not None:
            self.sched_path = os.path.join(self.a.work, 'schedule.json')
            with open(self.sched_path, 'w') as f:
                json.dump(schedule, f)

    def advance(self, n):
        if n < self.at:
            raise ValueError('the image only moves forward')
        if n == self.at:
            return 0.0
        cmd = self.provider + ['advance', '--log', self.a.log, '--index', self.index_path, '--image', self.image,
                               '--from', str(self.at), '--to', str(n), '--discard', getattr(self.a, 'discard', 'zero')]
        if self.sched_path:
            cmd += ['--schedule', self.sched_path]
        rc, out, t = run(cmd)
        if rc:
            sys.exit(f'provider advance {self.at}..{n} failed: {out}')
        self.at = n
        return t

    # checks
    def export(self):
        rc, out, _ = run(['zpool', 'export', self.a.pool], 120)
        if rc:
            rc, out, _ = run(['zpool', 'export', '-f', self.a.pool], 120)
        return rc, out

    def leftover(self):
        """Why an import now could not be judged, or None.

        A pool of this name that is still imported, after an export that
        failed, makes the next import fail whatever the image holds. That
        must read as the rig failing, never as a verdict on the image.
        Only "no such pool" with exit code 1 means there is none.
        """
        rc, out, _ = run(['zpool', 'list', '-H', '-o', 'name', self.a.pool], 60)
        if rc == 0:
            return f'rig: a pool named {self.a.pool} is still imported'
        if rc == 1 and 'no such pool' in out:
            return None
        if rc in RIG_RC:
            return stage('zpool list', rc)
        return f'rig: zpool list failed with code {rc}'

    # Each check below is layered: a step that sets something up, the
    # steps inside it, and its cleanup. Cleanup that fails replaces the
    # result of what ran inside, because the pool, snapshot or loop
    # device it left behind makes everything after it untrustworthy.
    def light(self, files):
        """Read-only import, mount, verify -> (stage that failed or None, detail, seconds by stage)."""
        t = {}
        left = self.leftover()
        if left:
            return left, '', t
        rc, out, _ = run(['losetup', '-r', '--find', '--show', self.image])
        if rc:
            return 'losetup', out.strip(), t
        loop = out.strip()
        try:
            res = self.light_on(loop, files, t)
        except BaseException:
            run(['losetup', '-d', loop])
            raise
        rc, out, _ = run(['losetup', '-d', loop])
        if rc:
            res = ('losetup -d', out.strip()[-400:])
        return res[0], res[1], t

    def light_on(self, loop, files, t):
        rc, out, t['import'] = run(['zpool', 'import', '-d', loop, '-o', 'readonly=on', '-o', 'cachefile=none',
                                    '-N', '-m', '-f', '-R', self.alt, self.a.pool], 180)
        if rc:
            return stage('import', rc), out.strip()[-400:]
        try:
            res = self.light_imported(files, t)
        except BaseException:
            self.export()
            raise
        rc, out = self.export()
        if rc:
            res = ('export', out.strip()[-400:])
        return res

    def light_imported(self, files, t):
        rc, out, t['mount'] = run(['zfs', 'mount', self.a.pool], 60)
        if rc:
            return stage('mount', rc), out.strip()[-400:]
        t0 = time.monotonic()
        root = self.alt + self.a.mountpoint
        for want, name in self.manifest[:files]:
            try:
                h = hashlib.sha256()
                with open(os.path.join(root, name), 'rb') as f:
                    for chunk in iter(lambda: f.read(1 << 20), b''):
                        h.update(chunk)
            except OSError as e:
                return 'verify', f'{name}: {e}'
            if h.hexdigest() != want:
                return 'verify', f'{name}: checksum differs'
        t['verify'] = time.monotonic() - t0
        return None, ''

    SNAP = 'skz-replay-snap'

    def deep(self):
        """Writable import of a throwaway snapshot, and a scrub -> (failure or None, detail, seconds)."""
        t0 = time.monotonic()
        left = self.leftover()
        if left:
            return left, '', time.monotonic() - t0
        cow = os.path.join(self.a.work, 'cow.img')
        with open(cow, 'wb') as f:
            f.truncate(2 << 30)
        loops = []
        try:
            res = self.deep_loops(cow, loops)
        finally:
            failed = [(lo, run(['losetup', '-d', lo])) for lo in loops]
            if os.path.exists(cow):
                os.unlink(cow)
        for lo, (rc, out, _) in failed:
            if rc:
                res = ('losetup -d', out.strip()[-400:])
        return res[0], res[1], time.monotonic() - t0

    def deep_loops(self, cow, loops):
        for cmd in (['losetup', '-r', '--find', '--show', self.image], ['losetup', '--find', '--show', cow]):
            rc, out, _ = run(cmd)
            if rc:
                return 'losetup', out.strip()
            loops.append(out.strip())
        sectors = os.path.getsize(self.image) // 512
        rc, out, _ = run(['dmsetup', 'create', self.SNAP, '--table', f'0 {sectors} snapshot {loops[0]} {loops[1]} N 8'])
        if rc:
            return 'dm-snapshot', out.strip()
        try:
            res = self.deep_snapshot()
        except BaseException:
            run(['dmsetup', 'remove', self.SNAP])
            raise
        for _ in range(20):
            rc, out, _ = run(['dmsetup', 'remove', self.SNAP])
            if rc == 0:
                break
            time.sleep(0.25)
        if rc:
            res = ('dm-snapshot remove', out.strip()[-400:])
        return res

    def deep_snapshot(self):
        rc, out, _ = run(['zpool', 'import', '-d', f'/dev/mapper/{self.SNAP}', '-o', 'cachefile=none', '-N', '-m', '-f',
                          '-R', self.alt, self.a.pool], 180)
        if rc:
            return stage('import (writable)', rc), out.strip()[-400:]
        try:
            res = self.deep_imported()
        except BaseException:
            self.export()
            raise
        rc, out = self.export()
        if rc:
            res = ('export', out.strip()[-400:])
        return res

    def deep_imported(self):
        rc, out, _ = run(['zpool', 'scrub', '-w', self.a.pool], 600)
        if rc:
            return stage('scrub', rc), out.strip()[-400:]
        rc, out, _ = run(['zpool', 'status', '-p', self.a.pool], 60)
        if rc:
            return 'status', out.strip()[-400:]
        bad = scrub_verdict(out)
        if bad:
            return 'scrub', bad
        return None, ''

    def copies(self):
        """The every-copy check on the image as it is -> (failure or None, detail, seconds, counts)."""
        t0 = time.monotonic()
        rc, out, _ = run(['zdb', '-e', '-p', self.imgdir, '-bbbbb', self.a.pool], 300)
        failed, detail, counts = every_copy_verdict(rc, out, self.image)
        return failed, detail, time.monotonic() - t0, counts

    def point(self, n, files, deep, copies=False):
        """All the checks due at prefix n. A check runs only if those before it passed."""
        res = {'n': n, 'files': files, 't_advance': self.advance(n)}
        t0 = time.monotonic()
        failed, detail, t = self.light(files)
        res['t_light'] = time.monotonic() - t0
        res['t'] = t
        if failed is None and deep:
            failed, detail, res['t_deep'] = self.deep()
            if failed:
                failed = 'deep: ' + failed
        if failed is None and copies:
            failed, detail, res['t_copies'], res['copies'] = self.copies()
        res['ok'] = failed is None
        if failed:
            res['failed'], res['detail'] = failed, detail
        return res


def scrub_verdict(status):
    """None when `zpool status -p` shows a scrub that found nothing, else what it found."""
    errors = 0
    scan = verdict = ''
    for line in status.splitlines():
        f = line.split()
        if len(f) >= 5 and all(x.isdigit() for x in f[2:5]):
            errors += sum(int(x) for x in f[2:5])
        if line.strip().startswith('scan:'):
            scan = line.strip()
        if line.startswith('errors:'):
            verdict = line.strip()
    if errors or 'with 0 errors' not in scan or 'repaired 0B' not in scan or 'No known data errors' not in verdict:
        return f'{errors} errors counted on the vdevs; {scan}; {verdict}'
    return None


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else float('nan')


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--log', required=True, help='the write log the provider reads')
    ap.add_argument('--base', required=True, help='image of the device before the first record')
    ap.add_argument('--manifest', required=True, help='"sha256 name" per file, in the order the files were completed')
    ap.add_argument('--pool', required=True)
    ap.add_argument('--mountpoint', required=True, help='mountpoint property of the pool root dataset')
    ap.add_argument('--work', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--provider', default='', help='image provider command (default: logwrites.py)')
    ap.add_argument('--budget-s', type=float, default=840, help='seconds for the main pass')
    ap.add_argument('--deep-budget-s', type=float, default=240, help='seconds of deep checks inside the main pass')
    ap.add_argument('--seed', type=int, default=1)
    ap.add_argument('--replay-log', default='', help='path of xfstests replay-log, as a witness for the provider')
    ap.add_argument('--require-witness', action='store_true', help='fail when the witness comparison did not run')
    ap.add_argument('--copies-budget-s', type=float, default=300, help='seconds of every-copy checks in the main pass')
    ap.add_argument('--log-device-image', default='',
                    help="image of the pool's log device as it was when the base image was taken, where the pool has one")
    ap.add_argument('--discard', choices=('zero', 'skip'), default='zero',
                    help='what a discarded range holds afterwards: zeros (the harsher), or what was there')
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    rig = Rig(args)
    idx = rig.make_index()
    records = idx['records']
    total = len(records)
    marks = parse_marks(records)
    res = {'records': total, 'marks': len(marks), 'files': len(rig.manifest), 'sectorsize': idx['sectorsize'],
           'write_bytes': sum(r['bytes'] for r in records if 'pos' in r),
           'discards': sum(1 for r in records if 'discard' in r['flags']),
           'discard_bytes': sum(r['bytes'] for r in records if 'discard' in r['flags']), 'discard_mode': args.discard}
    groups = group_points(records)
    print(f"log: {total} records, {len(marks)} marks, {res['write_bytes'] >> 20} MiB written, {len(rig.manifest)} files", flush=True)
    trouble = manifest_trouble(marks, len(rig.manifest))
    if trouble:
        sys.exit(trouble)

    # Calibration: a few prefixes spread over the log, two with the deep check.
    rig.reset()
    cal = []
    flush = flush_points(records)
    cal_copies = {g[0] for g in spread(groups, 2)}
    cal_pts = sorted(set(spread(range(total + 1), 8)) | set(spread(flush, 2)) | cal_copies)
    for n in cal_pts:
        r = rig.point(n, files_at(marks, n), n in set(spread(flush, 2)), n in cal_copies)
        r['kind'] = 'calibration'
        cal.append(r)
        print(f"calibration {n}: {'ok' if r['ok'] else 'FAILED ' + r['failed']} light {r['t_light']:.2f}s"
              + (f" deep {r['t_deep']:.2f}s" if 't_deep' in r else '')
              + (f" every-copy {r['t_copies']:.2f}s" if 't_copies' in r else ''), flush=True)
    t_light = statistics.mean(r['t_light'] + r['t_advance'] for r in cal)
    deeps = [r['t_deep'] for r in cal if 't_deep' in r]
    t_deep = statistics.mean(deeps) if deeps else 5.0
    budget_points = max(4, int(args.budget_s * 0.9 / t_light))
    deep_points = max(1, int(args.deep_budget_s / t_deep))
    kind, deep, avail = plan(records, budget_points, deep_points, args.seed)
    # The every-copy check: the first point of every transaction group
    # and the two ends, whatever it costs; then as many last points as
    # the budget leaves, spread over the run.
    tc = [r['t_copies'] for r in cal if 't_copies' in r]
    t_copies = statistics.mean(tc) if tc else 2.0
    firsts = {f for f, _ in groups} | {0, total}
    room = int(args.copies_budget_s / t_copies) - len(firsts)
    lasts = sorted({la for _, la in groups} - firsts)
    copies_at = firsts | set(spread(lasts, max(0, room)))
    for n in copies_at:
        kind.setdefault(n, 'group')
    avail['groups'] = len(groups)
    res['calibration'] = {'points': len(cal), 't_light_mean': t_light, 't_deep_mean': t_deep, 't_copies_mean': t_copies,
                          'budget_points': budget_points, 'deep_points': deep_points, 'copies_points': len(copies_at)}
    res['available'] = avail
    print(f'plan: {len(kind)} prefixes of {total + 1} ({budget_points} fit the budget at {t_light:.2f}s each), '
          f'{len(deep)} deep checks at {t_deep:.2f}s each, {len(copies_at)} every-copy checks at {t_copies:.2f}s each '
          f'for {len(groups)} transaction groups', flush=True)

    # Main pass, in order, on one image.
    rig.reset()
    points, failures = list(cal), [r for r in cal if not r['ok']]
    start = time.monotonic()
    skipped = 0
    for j, n in enumerate(sorted(kind)):
        over = time.monotonic() - start - args.deep_budget_s
        # Out of time: random prefixes go first, flush boundaries never.
        if kind[n] == 'random' and over > args.budget_s or kind[n] == 'ub_window' and over > args.budget_s * 1.25:
            skipped += 1
            continue
        r = rig.point(n, files_at(marks, n), n in deep, n in copies_at)
        r['kind'] = kind[n]
        points.append(r)
        if not r['ok']:
            failures.append(r)
            print(f"FAILED prefix {n} ({kind[n]}): {r['failed']}: {r['detail']}", flush=True)
            if len(failures) >= 25:
                print('25 failures: stopping the main pass', flush=True)
                break
        if j % 200 == 0:
            print(f'  {j} of {len(kind)} prefixes, {time.monotonic() - start:.0f}s', flush=True)
    res['main_pass_s'] = time.monotonic() - start
    res['coverage'] = cov = coverage(records, points, groups=groups)
    for e in cov['errors']:
        print(f'::error title=replay rig, coverage::{e}', flush=True)
    for w in cov['warnings']:
        print(f'::warning title=replay rig, coverage::{w}', flush=True)
    res['skipped_for_time'] = skipped

    # The provider against xfstests replay-log. First the image the main
    # pass ended on, which was reached through thousands of small steps;
    # then a fresh image at a prefix that lies between two writes.
    witness = []
    other = os.path.join(args.work, 'witness.img')

    def replay_log_image(n):
        run(['cp', '--sparse=always', args.base, other])
        cmd = [args.replay_log, '--log', args.log, '--replay', other, '--limit', str(n)]
        # replay-log makes a discarded range zeros on a file; --no-discard leaves it.
        rc, out, _ = run(cmd + (['--no-discard'] if args.discard == 'skip' else [])) if n else (0, '', 0)
        same = rc == 0 and run(['cmp', '-s', rig.image, other])[0] == 0
        return {'n': n, 'rc': rc, 'same': same, 'tail': out.strip()[-200:]}

    if args.replay_log:
        if rig.at == total:
            witness.append(dict(replay_log_image(total), what="the main pass's final image"))
        between = [n for n in range(1, total) if 'pos' in records[n - 1] and 'pos' in records[n]]
        for n, what in ((min(between, key=lambda n: abs(n - total // 2)) if between else total // 2, 'between two writes'),
                        (total, 'the end')):
            rig.reset()
            rig.advance(n)
            witness.append(dict(replay_log_image(n), what=what))
        for w in witness:
            print(f"replay-log at prefix {w['n']} ({w['what']}): {'same image' if w['same'] else 'DIFFERENT'} (rc {w['rc']})",
                  flush=True)
        if os.path.exists(other):
            os.unlink(other)

    # zdb's own checksum pass at the end, as a second opinion on the
    # rig's checksums: both must find the true image clean.
    rig.reset()
    rig.advance(total)
    rc, out, t = run(['zdb', '-e', '-p', rig.imgdir, '-bcc', args.pool], 300)
    said = [ln.strip() for ln in out.splitlines() if re.search(r'ASSERT|VERIFY|PANIC|rror|cannot|leak', ln)][:3]
    res['zdb_cc'] = {'n': total, 'rc': rc, 's': t, 'said': ' | '.join(said)[:500]}
    print(f'zdb -e -bcc at the end: rc {rc} in {t:.1f}s', flush=True)
    if rc:
        print(f"::error title=replay rig::zdb -bcc on the final image: exit code {rc}: {res['zdb_cc']['said']}", flush=True)
    res['witness'] = witness
    witness_ok, res['witness_verdict'] = witness_verdict(witness, args.require_witness)
    if not witness_ok:
        print(f"::error title=replay rig::provider against replay-log: {res['witness_verdict']}", flush=True)

    # Negative controls: wrong replays, judged against EXPECTED.
    ctl = []
    def find(n):
        return [] if args.provider else writes_carrying(args.log, records, file_pattern(n))

    for c in controls(records, marks, find):
        rig.reset(c['schedule'])
        # The transaction-group points of the wrong replay itself, inside the range the control checks.
        lo, hi = c['points'][0], c['points'][-1]
        gp = group_points([records[i] for i in c['schedule']])
        ctl_copies = {n for pair in gp for n in pair if lo <= n <= hi}
        pts = sorted(set(c['points']) | ctl_copies)
        deep_at = {n for n in pts if n and 'flush' in records[c['schedule'][n - 1]]['flags']}
        results = [rig.point(n, files_at(marks, n), n in deep_at, n in ctl_copies) for n in pts]
        checked = len(results)
        by, rig_errors, first = tally_control(results)
        facts = victim_facts(args.log, records, c['victim'], args.base) if not args.provider else None
        # Which blocks the victim is, by the pool's own account: the
        # true image at the end of the control's range, listed by zdb.
        if facts is not None:
            rig.reset()
            rig.advance(hi)
            rc, out, _ = run(['zdb', '-e', '-p', rig.imgdir, '-bbbbb', args.pool], 300)
            v = records[c['victim']]
            facts['blocks'] = blocks_at(zdb_blocks(out)['bps'], v['offset'], v['offset'] + v['bytes']) if rc == 0 else None
        ctl.append({'name': c['name'], 'what': c['what'], 'victim': c['victim'], 'points': checked, 'facts': facts,
                    'deep_points': len(deep_at), 'copies_points': len(ctl_copies), 'caught': first is not None,
                    'first': first, 'failed_by': by,
                    'rig_errors': rig_errors})
        print(f"control {c['name']}: {'caught' if first else 'NOT CAUGHT'} over {checked} prefixes"
              + (f", first at {first['n']} by {first['failed']}" if first else '')
              + (f"; THE RIG FAILED at {len(rig_errors)} prefixes, first: {rig_errors[0]['failed']}" if rig_errors else ''),
              flush=True)
    res['controls'] = ctl
    res['control_rows'], res['gaps'], controls_ok = judge_controls(ctl)
    for g in res['gaps']:
        print(f"::warning title=replay rig, known gap::control {g['name']} is not caught: {g['gap']}", flush=True)
    for r in res['control_rows']:
        if not r['match']:
            print(f"::error title=replay rig::control {r['name']}: expected {r['expected']}, observed {r['observed']}", flush=True)

    # The index goes with the results: a finding needs the records around it.
    run(['cp', rig.index_path, os.path.join(args.out, 'replay-index.json')])
    res['points'] = points
    res['failures'] = failures
    with open(os.path.join(args.out, 'replay-results.json'), 'w') as f:
        json.dump(res, f, indent=1)
    with open(os.path.join(args.out, 'replay-summary.md'), 'w') as f:
        f.write(summary(res))
    print(summary(res))
    ok = not failures and controls_ok and witness_ok and not cov['errors'] and res['zdb_cc']['rc'] == 0
    sys.exit(0 if ok else 1)


def summary(res):
    pts = [p for p in res['points'] if p.get('kind') != 'calibration']
    out = ['## Power-cut replay\n']
    av = res['available']
    out.append(f"Write log: {res['records']} records, {res['write_bytes'] >> 20} MiB written, {res['marks']} sync marks, "
               f"{res['files']} files, {av['flush']} flushes, {av['ub_writes']} uberblock writes.\n")
    verdict = 'every checked prefix imported and verified' if not res['failures'] else f"**{len(res['failures'])} prefixes FAILED**"
    out.append(f"Result: {verdict}. Main pass {res['main_pass_s']:.0f} s"
               + (f", {res['skipped_for_time']} planned prefixes skipped for time" if res['skipped_for_time'] else '') + '.\n')
    out.append('| kind of prefix | in the log | checked | failed | light check s, mean / p95 |')
    out.append('|---|---|---|---|---|')
    for k, label, n_av in (('flush', 'flush boundary', av['flush']),
                           ('ub_window', f'within {WINDOW} records of an uberblock write', av['ub_window']),
                           ('random', 'any other, at random', av['other']),
                           ('group', 'taken only for the every-copy check', '–'), ('end', 'first and last', 2)):
        sel = [p for p in pts if p['kind'] == k]
        ts = [p['t_light'] + p['t_advance'] for p in sel]
        out.append(f"| {label} | {n_av} | {len(sel)} | {sum(1 for p in sel if not p['ok'])} | "
                   + (f"{statistics.mean(ts):.2f} / {pct(ts, 0.95):.2f} |" if ts else '– |'))
    ts = [p['t_light'] + p['t_advance'] for p in pts]
    deep = [p['t_deep'] for p in pts if 't_deep' in p]
    if ts:
        per = statistics.mean(ts)
        parts = {k: statistics.mean([p['t'].get(k, 0) for p in pts]) for k in ('import', 'mount', 'verify')}
        out.append(f"\nLight check: {per:.2f} s per prefix on average (import {parts['import']:.2f}, mount {parts['mount']:.2f}, "
                   f"verify {parts['verify']:.2f}, the rest is the loop device, the export and advancing the image). "
                   f"At that rate a 30-minute job covers about {int(1800 / per)} prefixes with no deep check.")
    if deep:
        out.append(f"\nScrub (writable import of a snapshot, then a scrub): {len(deep)} flush boundaries, "
                   f"{statistics.mean(deep):.2f} s each on average, {max(deep):.2f} s at most, "
                   f"{sum(1 for p in pts if 't_deep' in p and not p['ok'])} failed.")
    cp = [p for p in pts if 't_copies' in p]
    if cp:
        tc = [p['t_copies'] for p in cp]
        last = cp[-1].get('copies', {})
        cov = res.get('coverage', {})
        out.append(f"\nEvery-copy check (zdb lists every block the newest uberblock reaches; the rig checksums every copy "
                   f"on the image; nothing is imported or written): {len(cp)} prefixes, {statistics.mean(tc):.2f} s each on "
                   f"average, {max(tc):.2f} s at most, {sum(1 for p in cp if not p['ok'])} failed. "
                   f"Transaction groups: {cov.get('groups', '?')}; checked right after the first uberblock write of "
                   f"{cov.get('group_firsts', '?')} (all are required) and just before the next group's of "
                   f"{cov.get('group_lasts', '?')} of {cov.get('group_lasts_total', '?')}. At the last one checked: "
                   f"{last.get('blocks', '?')} blocks in {last.get('copies', '?')} copies, {last.get('with_copies', '?')} "
                   f"blocks with more than one copy, {last.get('intent_log', '?')} intent-log blocks not judged.")
    z = res.get('zdb_cc')
    if z:
        out.append(f"\n`zdb -e -bcc` on the final image, as a second opinion on the rig's checksums: exit code {z['rc']} in "
                   f"{z['s']:.1f} s" + (f" — **{z['said']}**" if z['rc'] else '') + '.')
    if res.get('discards') is not None:
        out.append(f"\nDiscards in the log: {res['discards']}, covering {res.get('discard_bytes', 0) >> 10} KiB; a discarded "
                   f"range was made {'zeros' if res.get('discard_mode') == 'zero' else 'nothing new (left as it was)'} on the image.")
    cov = res.get('coverage')
    if cov:
        out.append(f"\nCoverage: {cov['flush'] - len(cov['flush_unchecked'])} of {cov['flush']} flush boundaries checked "
                   f"(all are required), the deep check at {cov['deep']} of them (at least {cov['deep_floor']} are required).")
        for e in cov['errors']:
            out.append(f'\n> [!CAUTION]\n> Coverage: {e}.')
        for w in cov['warnings']:
            out.append(f'\n> [!WARNING]\n> Coverage: {w}.')
    if not res.get('witness'):
        out.append(f"\nProvider against xfstests `replay-log`: **{res.get('witness_verdict', 'witness not run')}**.")
    if res.get('witness'):
        out.append('\nProvider against xfstests `replay-log`: '
                   + '; '.join(f"prefix {w['n']} ({w.get('what', '')}): {'same image' if w['same'] else 'DIFFERENT'}"
                               for w in res['witness']) + '.')
    out.append('\n### Negative controls\n')
    if not res['controls']:
        out.append('**None could be built from this log.**')
    else:
        out.append('| control | what was done | prefixes checked (scrub, every-copy) | caught | first failure |')
        out.append('|---|---|---|---|---|')
        for c in res['controls']:
            first = f"prefix {c['first']['n']}: {c['first']['failed']}: {c['first']['detail'][:160]}" if c['first'] else '–'
            by = ', '.join(f'{k} {v}' for k, v in c['failed_by'].items())
            if c.get('rig_errors'):
                first += f"; **the rig failed** at {len(c['rig_errors'])} prefixes, first {c['rig_errors'][0]['failed']}"
            out.append(f"| {c['name']} | {c['what']} | {c['points']} ({c['deep_points']}, {c.get('copies_points', 0)}) | "
                       f"{'yes, ' + by if c['caught'] else 'no'} | {first.replace('|', '/')} |")
        out.append('\nAgainst what each control is expected to do today (`EXPECTED` in `replay_check.py`; '
                   'a difference fails the job):\n')
        out.append('| control | expected | observed | |')
        out.append('|---|---|---|---|')
        for r in res.get('control_rows', []):
            out.append(f"| {r['name']} | {r['expected']} | {r['observed']} | {'as expected' if r['match'] else '**DIFFERS**'} |")
        for g in res.get('gaps', []):
            out.append(f"\n> [!WARNING]\n> Known gap (`{g['name']}` is not caught): {g['gap']}")
        for c in res['controls']:
            if c.get('facts'):
                out.append(f"\nThe block withheld by `{c['name']}`: {facts_text(c['facts'])}.")
    if res['failures']:
        out.append('\n### Failures\n')
        for p in res['failures'][:25]:
            out.append(f"- prefix {p['n']} ({p.get('kind')}): {p['failed']}: {p['detail'][:300]}")
    return '\n'.join(out) + '\n'


if __name__ == '__main__':
    main()
