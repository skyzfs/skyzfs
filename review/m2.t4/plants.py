#!/usr/bin/env python3
"""#108 at 6cedb4d (M2.T4): plants in the benchmark's releaser (tests/bench/cut_release.py),
one at a time, against tests/bench/test_cut_release.py. The file is restored.

    plants.py CHECKOUT
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
J = 'tests/bench/cut_release.py'
PLANTS = [
    ('P1 a cut is due one cut later than asked',
     "    return up[0]['number'] if newest - up[0]['number'] >= every - 1 else None",
     "    return up[0]['number'] if newest - up[0]['number'] >= every else None"),
    ('P2 a cut is due whatever EVERY says',
     "    return up[0]['number'] if newest - up[0]['number'] >= every - 1 else None",
     "    return up[0]['number']"),
    ('P3 the newest cut is taken among those being uploaded only',
     "    newest = max(c['number'] for c in held)\n    return up",
     "    newest = max(c['number'] for c in up)\n    return up"),
    ('P4 a cut whose flush is under way is released too',
     "    up = [c for c in held if c['phase'] == 'uploading']",
     "    up = [c for c in held if c['phase'] in ('uploading', 'flushing')]"),
    ('P5 a refused release is counted as a release',
     "        if not ok:\n            raise ch.HolderError(f'release {n} refused: {last}')\n",
     ""),
    ('P6 a refused held is taken as nothing held',
     "        if not ok:\n            raise ch.HolderError(f'held refused: {last}')\n        return ch.parse_held(lines)",
     "        return ch.parse_held(lines if ok else [])"),
    ('P7 an error ends the releaser with exit 0',
     "            rc = 2\n", "            rc = 0\n"),
    ('P8 an error leaves no line in the output',
     "            f.write(f'# ERROR {type(e).__name__}: {e} (released {r.released}, polls {r.polls})\\n')\n", ""),
    ('P9 the end leaves no line in the output',
     "            f.write(f'# released {r.released}, polls {r.polls}\\n')\n", "            pass\n"),
    ('P10 a release is not written to the output',
     "        self.out.write(f\"{time.monotonic_ns()}\\t{n}\\t{max(c['number'] for c in held)}\\n\")\n", ""),
    ('P11 the stop file is not looked at',
     "            while not os.path.exists(stop):", "            while True:"),
    ('P12 the reply limit is not taken from CUT_RELEASE_REPLY_S',
     "REPLY_S = float(os.environ.get('CUT_RELEASE_REPLY_S', '30'))", "REPLY_S = 30.0"),
]
for name, old, new in PLANTS:
    full = os.path.join(ROOT, J)
    orig = open(full).read()
    if orig.count(old) != 1:
        print(f'{name}: NOT APPLIED (found {orig.count(old)} times)')
        continue
    try:
        with open(full, 'w') as f:
            f.write(orig.replace(old, new))
        try:
            p = subprocess.run([sys.executable, 'test_cut_release.py'], cwd=os.path.join(ROOT, 'tests/bench'),
                               capture_output=True, text=True, timeout=120)
            failed = [ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL:', 'ERROR:'))]
            if not failed and p.returncode != 0:
                failed = ['(the run failed)']
        except subprocess.TimeoutExpired:
            subprocess.run(['pkill', '-f', 'cut_release.py'], check=False)
            failed = ['(did not end in 120 s)']
        print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(sorted(set(failed))[:5])}' if failed else 'SURVIVED'), flush=True)
    finally:
        with open(full, 'w') as f:
            f.write(orig)
print('finished')
