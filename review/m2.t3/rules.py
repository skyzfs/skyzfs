#!/usr/bin/env python3
"""#99 at 2c67c42: which rule controls the daemon check could carry, by fixture.

    rules.py CHECKOUT

Each fixture of tests/fixtures/pools is written out as the daemon's request log (the test
file's `request_log`), read back with `records_of`, and the daemon's cuts (the reference's by
the design's rule) are compared with the reference by each other rule of cut_ref. `caught`
where the comparison fails, `same` where that rule gives the same cuts on this recording.
An added cut (the middle one twice) is tried too.
"""
import os
import sys

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import cut_daemon_check as cdc  # noqa: E402
import cut_ref  # noqa: E402
import test_cut_daemon_check as t  # noqa: E402

for name in sorted(os.listdir(os.path.join(ROOT, 'tests/fixtures/pools'))):
    path = os.path.join(ROOT, 'tests/fixtures/pools', name)
    try:
        records, read, dev = cut_ref.read_fixture(path)
    except Exception as e:  # noqa: BLE001
        print(f'{name}: not a fixture ({type(e).__name__})')
        continue
    cs, at = t.daemon_cuts_of(records, read, dev)
    text, marks = t.request_log(records, read, dev, at)
    recs, dcuts = cdc.records_of(cdc.parse_log(text)[0]['lines'], cdc.read_marks(marks))
    rread = cdc.read_of(recs, dev)
    ubs = cut_ref.scan(recs, rread, dev)
    out = []
    for rule in ('design', 'early', 'count-repeats', 'txg-alone', 'previous'):
        rcs, _ = cut_ref.cuts(recs, ubs, rule)
        ref = cdc.reference(rcs, cut_ref.opens(recs, rread, dev, rcs))
        ok, _ = cdc.compare(dcuts, ref)
        out.append(f"{rule} {'same' if ok else 'caught'} ({len(rcs)})")
    mid = len(dcuts) // 2
    extra = dcuts[:mid + 1] + dcuts[mid:]
    rcs, _ = cut_ref.cuts(recs, ubs)
    ok, _ = cdc.compare(extra, cdc.reference(rcs, cut_ref.opens(recs, rread, dev, rcs)))
    out.append(f"an added cut {'missed' if ok else 'caught'}")
    print(f'{name}: {len(dcuts)} cuts; ' + '; '.join(out))
