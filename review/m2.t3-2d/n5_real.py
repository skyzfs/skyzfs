#!/usr/bin/env python3
"""#115 at 813120a: what a real `takeover` recording has between its
marks noflush-begin and noflush-end, to hold the unit test's model
(test_a_group_synced_with_no_flush) against: the flushes received there,
the groups whose uberblocks were written there and whether the
reference cuts each, the starts there, and the first flush after
noflush-end with what it cuts.

    python3 -I n5_real.py CHECKOUT ARTIFACT_DIR...
"""
import os
import sys

root, arts = sys.argv[1], sys.argv[2:]
sys.path.insert(0, os.path.join(root, 'tests/kernel'))
import cut_daemon_check as c  # noqa: E402
import cut_ref  # noqa: E402

DEV = 512 << 20


def rd(d, name):
    with open(os.path.join(d, name)) as f:
        return f.read()


for art in arts:
    src = os.path.join(art, 'takeover')
    torn = []
    inst = c.parse_log(rd(src, 'requests.tsv'), torn)
    kinds = c.read_kinds(rd(src, 'instances.tsv'))
    acked = c.read_acknowledged(rd(src, 'instances.tsv'))
    inst, _ = c.unstored_at_kill(inst, acked)
    records, dcuts = c.records_of(c.log_lines(inst, kinds, acked), c.read_marks(rd(src, 'marks.tsv')))
    read = c.read_of(records, DEV)
    cs, taken = cut_ref.cuts(records, cut_ref.scan(records, read, DEV))
    cut = {x['txg']: x for x in cs}
    by_prefix = {x['prefix']: x for x in cs}
    print(os.path.basename(os.path.normpath(art)) + ':')
    lo = None
    for r in records:
        if 'mark' in r['flags'] and r.get('mark') == 'noflush-begin':
            lo = r['i']
        elif 'mark' in r['flags'] and r.get('mark') == 'noflush-end' and lo is not None:
            hi = r['i']
            flushes = [x['i'] for x in records[lo + 1:hi] if 'flush' in x['flags']]
            starts = [('took the device over' if x['taken_over'] else 'created the device')
                      for x in records[lo + 1:hi] if 'instance' in x['flags']]
            groups = sorted({u['txg'] for u in taken if lo < u['record'] < hi})
            nxt = next((x for x in records[hi + 1:] if 'flush' in x['flags']), None)
            said = 'none'
            if nxt is not None:
                k = by_prefix.get(nxt['i'])
                said = f"record {nxt['i']}, " + (f"which cuts group {k['txg']}" if k else 'which cuts nothing')
            print(f'  marks at records {lo} and {hi}: {len(flushes)} flushes received between them; '
                  f"starts between them: {starts or 'none'}; groups whose uberblocks were written there: "
                  + ', '.join(f"{g} ({'cut' if g in cut else 'not cut'})" for g in groups)
                  + f'; the first flush after noflush-end: {said}')
            lo = None
