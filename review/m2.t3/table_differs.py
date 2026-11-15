#!/usr/bin/env python3
"""#99 at 6e0ecd8: a control whose result differs from its table must fail the check.

    table_differs.py CHECKOUT

No test of the pull request has a control that differs from the table (plants N8 and N9 of
plants_next2.py pass). This runs `main` on the fixture `life` under the name `rewind`, whose row
expects the rule txg-alone to be caught; on `life` that rule gives the design's cuts.
"""
import os
import sys
import tempfile

ROOT = sys.argv.pop(1)
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import test_cut_daemon_check as t  # noqa: E402
import test_cut_daemon_main as m  # noqa: E402

records, read, dev = t.fixture('life')
cs, at = t.daemon_cuts_of(records, read, dev)
text, marks = t.request_log(records, read, dev, at)
d = tempfile.mkdtemp(prefix='cdc-')
rc, lines = m.run_in(d, text, marks, records, read, dev, cs, None, 'rewind', None)
print('exit', rc)
for key in ('cuts', 'ctl_txg-alone', 'ctl_count-repeats', 'ctl_early'):
    print(key, lines.get(key))
