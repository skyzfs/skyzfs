#!/usr/bin/env python3
"""#111 at 8210332: in a takeover recording, the uberblock writes between the
marks noflush-begin and noflush-end: which group, beside which instance of the
daemon, and whether the reference cuts it.

    python3 -I noflush_groups.py CHECKOUT RECORDING_DIR...
"""
import os
import sys

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import cut_daemon_check as c  # noqa: E402
import cut_ref  # noqa: E402

DEV = 512 << 20
for d in sys.argv[2:]:
    with open(os.path.join(d, 'requests.tsv')) as f:
        instances = c.parse_log(f.read())
    kinds = c.read_kinds(c.read_file(d, 'instances.tsv'))
    with open(os.path.join(d, 'marks.tsv')) as f:
        marks = c.read_marks(f.read())
    records, dcuts = c.records_of(c.log_lines(instances, kinds), marks)
    read = c.read_of(records, DEV)
    cs, taken = cut_ref.cuts(records, cut_ref.scan(records, read, DEV))
    cut = {x['txg']: x for x in cs}
    bounds = [r['i'] for r in records if 'instance' in r['flags']]
    print('/'.join(d.rstrip('/').split('/')[-2:]) + ':')
    inside = False
    for r in records:
        if 'instance' in r['flags']:
            k = bounds.index(r['i']) + 2
            print(f"  record {r['i']}: instance {k} ({'took the device over' if r['taken_over'] else 'created the device'})"
                  + (' [inside the marks]' if inside else ''))
        if 'mark' in r['flags'] and r.get('mark', '').startswith('noflush-'):
            inside = r['mark'] == 'noflush-begin'
            print(f"  record {r['i']}: mark {r['mark']}")
        if inside:
            for u in taken:
                if u['record'] == r['i'] and u['taken'] == 'marks':
                    x = cut.get(u['txg'])
                    print(f"  record {r['i']}: group {u['txg']} marks; "
                          + (f"cut {x['n']} at the flush of record {x['flush_record']}" if x else 'not cut'))
