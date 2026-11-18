#!/usr/bin/env python3
"""#119 at 32d8d67: could run 44255421061's ufailed tell a daemon whose
first cut after the acknowledged failure left out the intent map's
regions?

    ufailed_map.py CHECKOUT RECORDING_DIR

Such a daemon's first cut of the new state carries only what was written
since its start. What is then wrong at head: the blocks written between
the last cut uploaded before the failure and the failure, unless the new
state wrote them again before the cut node B next fetched. This counts
them, from the request log, for each cut of the new state that node B
fetched.
"""
import os
import sys

sys.path.insert(0, os.path.join(sys.argv[1], 'tests', 'kernel'))
import cut_daemon_check as cdc  # noqa: E402

rdir = sys.argv[2]


def read(name):
    with open(os.path.join(rdir, name)) as f:
        return f.read()


instances = cdc.parse_log(read('requests.tsv'), [])
kinds = cdc.read_kinds(read('instances.tsv'))
acked = cdc.read_acknowledged(read('instances.tsv'))
instances, _ = cdc.unstored_at_kill(instances, acked)
records, dcuts = cdc.records_of(cdc.log_lines(instances, kinds, acked), cdc.read_marks(read('marks.tsv')))
cdc.epochs(dcuts, records)
start2 = next(r['i'] for r in records if 'instance' in r['flags'] and r.get('begun'))
got = cdc.read_uploaded(read('uploaded.tsv'))
n_epochs = 1 + max(d.get('epoch', 0) for d in dcuts)
offsets = [sum(1 for d in dcuts if d.get('epoch', 0) < e) for e in range(n_epochs)]
mine, _, _, _, states = cdc.daemons_fetches(got, read('uploaded-blocks.tsv'), (set(), set()), offsets)
by_number = {int(d['number']): d for d in dcuts}
old = [g for g in mine if by_number[g['number']]['epoch'] == 0]
new = [g for g in mine if by_number[g['number']]['epoch'] == 1]
base = by_number[old[-1]['number']]
print(f"the last cut node B fetched of the first state: {old[-1]['number']} (group {base['txg']}); "
      f"the new state's first cut line is place {offsets[1] + 1}; node B fetched of the new state: "
      f"{[g['number'] for g in new]}")


def model(upto, skip=None):
    m = {}
    for r in records[:upto]:
        if skip and skip[0] <= r['i'] < skip[1]:
            continue
        cdc.apply(m, r)
    return m


between = [r for r in records[base['prefix']:start2] if cdc.cut_ref.is_write(r) or 'discard' in r['flags']]
touched = set()
for r in between:
    touched |= set(range(r['offset'] // cdc.BLOCK, (r['offset'] + r['bytes'] - 1) // cdc.BLOCK + 1))
print(f"between that cut's instant and the failure: {len(between)} writes, {len(touched)} blocks "
      f"({sorted(touched)[:8]})")
for g in new:
    d = by_number[g['number']]
    right = model(d['prefix'])
    wrong = model(d['prefix'], skip=(base['prefix'], start2))   # the writes between never reached head
    bad = [b for b in set(right) | set(wrong) if right.get(b, cdc.ZERO) != wrong.get(b, cdc.ZERO)]
    print(f"cut {g['number']} (group {d['txg']}): a head without those writes differs from the reference in "
          f"{len(bad)} blocks {sorted(bad)[:8]}")
