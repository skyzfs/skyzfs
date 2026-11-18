#!/usr/bin/env python3
"""#116 at 8e20e51, question (b): is main run in the test's process
(test_cut_daemon_main.recording(held=...)) the same program as main run
as cut-record.sh runs it?

    inproc.py CHECKOUT

1. Every recording the program's own tests build (the whole of
   test_cut_daemon_main.Main) is judged twice: by the subprocess, and in
   this process with held_lines "replaced" by itself (held=lambda real:
   real). The exit and every line but `time` are compared.
2. What a replacement that raises leaves: main takes any exception as the
   rig's error, so a broken replacement would end 2 as a set flag does.
   The test's case with the flag clear (exit 0) is what tells them apart.
3. What a replacement is called with, on each of main's two calls.
"""
import collections
import io
import os
import sys
import unittest

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import test_cut_daemon_main as m  # noqa: E402

orig = m.run_in
seen = []


def both(d, text, marks, records, read, dev, cs, status, name, tamper_kstat, as_name, held=None):
    if held is not None:
        return orig(d, text, marks, records, read, dev, cs, status, name, tamper_kstat, as_name, held)
    rc1, l1 = orig(d, text, marks, records, read, dev, cs, status, name, tamper_kstat, as_name)
    os.remove(os.path.join(d, 'results.tsv'))
    rc2, l2 = orig(d, text, marks, records, read, dev, cs, status, name, tamper_kstat, as_name, lambda real: real)
    a = {k: v for k, v in l1.items() if k != 'time'}
    b = {k: v for k, v in l2.items() if k != 'time'}
    seen.append((as_name or name, rc1, rc2, sorted(k for k in set(a) | set(b) if a.get(k) != b.get(k)),
                 sum(1 for v in a.values() if v[0] == 'ERROR')))
    return rc1, l1


m.run_in = both
suite = unittest.defaultTestLoader.loadTestsFromTestCase(m.Main)
res = unittest.TextTestRunner(stream=io.StringIO()).run(suite)
m.run_in = orig
print(f'1. the tests of Main with every recording judged both ways: {res.testsRun} tests, {len(res.failures)} failures, '
      f'{len(res.errors)} errors')
exits = collections.Counter((rc1, rc2) for _, rc1, rc2, _, _ in seen)
print(f'   {len(seen)} recordings; (exit of the subprocess, exit in this process): '
      + ', '.join(f'{k}: {n}' for k, n in sorted(exits.items())))
diff = [s for s in seen if s[1] != s[2] or s[3]]
print(f'   recordings whose exit or lines differ between the two: {len(diff)}')
for s in diff[:10]:
    print(f'      {s}')
print(f'   of them with an ERROR line: {sum(1 for s in seen if s[4])}; names: '
      + ', '.join(f'{k}: {n}' for k, n in sorted(collections.Counter(s[0] for s in seen).items())))

k = 13
none = lambda written, blocks, final: None    # noqa: E731
records, read, dev = m.t.fixture('life')
cs, _ = m.t.daemon_cuts_of(records, read, dev)
status = m.held_status(records, read, dev, cs).replace(f'cuts {len(cs)}\n', f'cuts {len(cs) - k}\n')
log = '1\t> t1 held\n2\t< t1 ok (0 lines)\n3\tVERDICT nothing to read\n'
cases = (('after the instances', {'split': (k, 'acknowledged'), 'status': status}),
         ("after the holder's verdict", {'holder_log': log, 'holder_left': ('holder.log',)}))
print('2. a replacement that raises:')
for why, kw in cases:
    rc, lines = m.recording(holder=none, as_name='failed', held=lambda real: lambda *a, **k2: 1 / 0, **kw)
    print(f'   {why}: exit {rc}; rig_error: {lines.get("rig_error")}')
print('3. what the replacement is called with, and what the real one returns there:')
for why, kw in cases:
    calls = []

    def held(real):
        def f(*a, **k2):
            got = real(*a, **k2)
            calls.append((len(a), sorted(k2), a[6] if len(a) > 6 else None, a[7] if len(a) > 7 else None, got))
            return got
        return f
    rc, lines = m.recording(holder=none, as_name='failed', held=held, **kw)
    print(f'   {why}: exit {rc}; {len(calls)} call(s): '
          + '; '.join(f'{n} positional, keywords {kws}, dev {dev_}, name {nm!r}, returned {got}' for n, kws, dev_, nm, got in calls))
    print('      lines not PASS or INFO or as expected: '
          + '; '.join(f'{key} {v[0]}' for key, v in lines.items()
                      if v[0] not in ('PASS', 'INFO', 'CAUGHT', 'SAME') or 'DIFFERS' in v[1]))
