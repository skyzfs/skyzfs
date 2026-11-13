#!/usr/bin/env python3
"""Defects planted in tests/kernel/replay_hole.py and hole-rig.sh (pull
request #63), one at a time, and which of test_replay_hole.py's tests
fail. Each file is put back after its plant.

    plants63.py WORKTREE
"""
import os
import re
import subprocess
import sys

W = sys.argv[1]
HOLE = 'tests/kernel/replay_hole.py'
RIG = 'tests/kernel/hole-rig.sh'
PLANTS = [
    ('a flushed block that reads zeros is no violation', HOLE,
     "        elif st == 'flushed' and got != 'written':", "        elif False:"),
    ('content that is neither zeros nor the write is no violation', HOLE,
     "        if got == 'other':", "        if False:"),
    ('a flush marked before the write covers it', HOLE,
     "    if any(wmark < p <= n for p, _ in fmarks):", "    if any(p <= n for p, _ in fmarks):"),
    ('a flush marked beyond the prefix covers the write', HOLE,
     "    if any(wmark < p <= n for p, _ in fmarks):", "    if any(wmark < p for p, _ in fmarks):"),
    ('"on the disk" counted whether or not the data is in the prefix', HOLE,
     "            if data_at.get(i, n + 1) <= n:", "            if True:"),
    ('"on the disk" one prefix late (data_at < n)', HOLE,
     "            if data_at.get(i, n + 1) <= n:", "            if data_at.get(i, n + 1) < n:"),
    ('the data found one prefix late', HOLE,
     "                if i not in out and h in data:\n                    out[i] = n",
     "                if i not in out and h in data:\n                    out[i] = n + 1"),
    ('the control moves the data to before the flush mark, not after', HOLE,
     "                at = sched.index(fp - 1) + 1    # just after the flush's mark record",
     "                at = sched.index(fp - 1)"),
    ('a control that is not caught does not fail the run', HOLE,
     "    if ctl:\n        return 2,", "    if False:\n        return 2,"),
    ('no course checked gives exit 0', HOLE,
     "    if not results:\n        return 2,", "    if not results:\n        return 0,"),
    ('the prefix before each mark is not always checked', HOLE,
     "        must |= {p, p - 1}", "        must |= {p}"),
    ('the rig does not ask for the control', RIG,
     "        --control >&2 ||", "        >&2 ||"),
    ('the rig error of a course does not fail the run', HOLE,
     "    if errors:\n        return 2,", "    if False:\n        return 2,"),
    # Added for the second look (head 18311fa).
    ('a torn block is allowed after a flush covers it', HOLE,
     "        elif st == 'flushed' and got != 'written':", "        elif st == 'flushed' and got not in ('written', 'torn'):"),
    ('a sector of anything counts as torn', HOLE,
     "    if len(data) == len(want) and all(data[s:s + SECTOR] in (want[s:s + SECTOR], zero)",
     "    if len(data) == len(want) and any(data[s:s + SECTOR] in (want[s:s + SECTOR], zero)"),
    ('a torn block is counted as zeros after return', HOLE,
     "        elif got == 'torn':\n            out['torn'].append(i)", "        elif got == 'torn':\n            out['zeros_after_return'].append(i)"),
    ('data missing from the log is not an error', HOLE,
     "    if not missing:\n        return None", "    if True:\n        return None"),
    ('the check goes on when data is missing from the log', HOLE,
     "    if res['rig_error']:\n        return res\n    pts = plan(", "    pts = plan("),
    ('the third count is not pinned on a written-through image', HOLE,
     "    if bad or pinned:", "    if bad:"),
    ('the third count is pinned on every kind', HOLE,
     "    pinned = [r for r in results if ON_DISK[r['kind']] != 'observed'\n", "    pinned = [r for r in results if True\n"),
    ('a kind not in the table is no error', HOLE,
     "    if unknown:\n        return 2,", "    if False:\n        return 2,"),
    ('a course without its control gives no error', HOLE,
     "    if [r for r in results if not r.get('control')]:", "    if False:"),
    ('the summary does not name the daemon', HOLE,
     "(f' The daemon: {daemon}.' if daemon else '')", "''"),
    ('the rig does not pass on what the daemon was built from', RIG,
     ' --daemon "${RIG_DAEMON_FROM:-}"', ''),
]


def tests():
    p = subprocess.run([sys.executable, os.path.join(W, 'tests/kernel/test_replay_hole.py')], cwd=W,
                       capture_output=True, text=True, timeout=300)
    out = p.stdout + p.stderr
    failed = sorted(set(re.findall(r'^(?:FAIL|ERROR): (\S+)', out, re.M)))
    ran = re.search(r'^Ran (\d+) tests', out, re.M)
    return (', '.join(failed) or 'nothing fails') + f" ({ran.group(1) if ran else '?'} tests)"


print('unchanged:', tests(), flush=True)
for name, path, old, new in PLANTS:
    full = os.path.join(W, path)
    orig = open(full, encoding='utf-8').read()
    if orig.count(old) != 1:
        print(f'{name}: THE PLANT DOES NOT APPLY ({orig.count(old)} places)', flush=True)
        continue
    try:
        open(full, 'w', encoding='utf-8').write(orig.replace(old, new))
        print(f'{name}: {tests()}', flush=True)
    finally:
        open(full, 'w', encoding='utf-8').write(orig)
