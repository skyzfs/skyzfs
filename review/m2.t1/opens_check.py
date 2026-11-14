#!/usr/bin/env python3
"""From each fixture: recompute cut_ref.opens and compare with opens.tsv; look for ties
and for same-group slots with different timestamps.

    opens_check.py KERNEL_DIR FIXTURES_DIR
"""
import os
import sys

sys.path.insert(0, sys.argv[1])
import cut_ref  # noqa: E402

root = sys.argv[2]
for name in sorted(os.listdir(root)):
    d = os.path.join(root, name)
    if not os.path.isdir(d):
        continue
    records, read, dev = cut_ref.read_fixture(d)
    cs, _ = cut_ref.cuts(records, cut_ref.scan(records, read, dev))
    op = cut_ref.opens(records, read, dev, cs)
    want = {int(r[0]): r for r in cut_ref.read_tsv(os.path.join(d, 'opens.tsv'))}
    diff = [n for n in op if (str(op[n]['txg']), str(int(op[n]['itself']))) != (want[n][4], want[n][5])]
    # ties and same-group timestamps, at each cut's instant, by re-walking the rings
    labels = cut_ref.label_offsets(dev)
    rings = [(lo + cut_ref.RING_START, lo + cut_ref.LABEL) for lo in labels]
    slots = {}
    ties, multi_ts, txg_only_differs = 0, 0, 0
    by_prefix = {}
    for c in cs:
        by_prefix.setdefault(c['prefix'], []).append(c)

    def look(prefix):
        global ties, multi_ts, txg_only_differs
        for c in by_prefix.get(prefix, ()):
            if not slots:
                continue
            top = max((u['txg'], u['timestamp']) for u in slots.values())
            states = {tuple(u['rootbp']) for u in slots.values() if (u['txg'], u['timestamp']) == top}
            if len(states) > 1:
                ties += 1
            same_txg = {u['timestamp'] for u in slots.values() if u['txg'] == top[0]}
            if len(same_txg) > 1:
                multi_ts += 1
            # would choosing by group alone pick a different state?
            by_txg = [u for u in slots.values() if u['txg'] == top[0]]
            if {tuple(u['rootbp']) for u in by_txg} != states:
                txg_only_differs += 1
    for r in records:
        look(r['i'])
        a, b = r.get('offset', 0), r.get('offset', 0) + r.get('bytes', 0)
        ring = [(max(a, lo), min(b, hi), lo) for lo, hi in rings if a < hi and b > lo]
        if not ring:
            continue
        if 'discard' in r['flags']:
            for x, y, lo in ring:
                for o in range(x + (-(x - lo) % cut_ref.SLOT_ALIGN), y, cut_ref.SLOT_ALIGN):
                    slots.pop(o, None)
        elif cut_ref.is_write(r):
            data = read(r)
            for x, y, lo in ring:
                for o in range(x + (-(x - lo) % cut_ref.SLOT_ALIGN), y, cut_ref.SLOT_ALIGN):
                    u = cut_ref.parse_uberblock(data[o - a:o - a + cut_ref.UB_BYTES])
                    if u is not None and u['txg'] > 0:
                        slots[o] = u
                    else:
                        slots.pop(o, None)
    look(len(records))
    print(f'{name}: {len(cs)} cuts; opens recomputed differs from opens.tsv at {diff or "none"}; '
          f'cuts with a tie of another state {ties}; with several timestamps in the best group {multi_ts}; '
          f'where the group alone would admit another state {txg_only_differs}')
