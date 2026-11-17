#!/usr/bin/env python3
"""#111 at 8210332: in a takeover recording, which written cuts were counted by
one instance of the daemon and written by the holder beside a later one.

    python3 -I held_across.py RECORDING_DIR...

From requests.tsv (the `start` and `cut` lines, with their pids and times) and
held.tsv (when each write was asked for)."""
import os
import sys

for d in sys.argv[1:]:
    starts, cuts = [], {}
    kills = {}
    with open(os.path.join(d, 'requests.tsv')) as f:
        last = {}
        for ln in f:
            c = ln.split('\t', 8)
            if len(c) < 8:
                continue
            last[c[1]] = int(c[0])
            if c[2] == 'start':
                starts.append((int(c[0]), c[1]))
            elif c[2] == 'cut':
                cuts[int(c[6])] = (c[1], int(c[0]), int(c[7]))
    order = [p for _, p in starts]
    kinds = dict(ln.split() for ln in open(os.path.join(d, 'instances.tsv')))
    print(f'{d}: instances ' + ', '.join(f'{k + 1}={p} ({kinds[p]}; lines {t}..{last[p]})' for k, (t, p) in enumerate(starts)))
    for ln in open(os.path.join(d, 'held.tsv')):
        n, group, _, blocks, t0, t1 = ln.split('\t')
        n, t0 = int(n), int(t0)
        counted = order.index(cuts[n][0]) + 1
        asked = max(k + 1 for k, (t, _) in enumerate(starts) if t <= t0)
        note = '' if counted == asked else f'   <-- held across {asked - counted} start(s), {(t0 - cuts[n][1]) / 1e9:.1f} s after its cut line'
        print(f'  cut {n} (group {group}, {blocks} blocks): counted by instance {counted}, write asked beside instance {asked}{note}')
