#!/usr/bin/env python3
"""Plant one change at a time in cut_ref.py / cut_check.py and run both test files.

    plants.py ROOT        (ROOT holds tests/kernel and tests/fixtures/pools)
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
K = os.path.join(ROOT, 'tests', 'kernel')

PLANTS = [
    ('Z1 judge: root not compared (itself)', 'cut_check.py',
     "if g != (pred['txg'], pred['timestamp'], root_of(pred['rootbp'])):", "if g[:2] != (pred['txg'], pred['timestamp']):"),
    ('Z2 judge: root not compared (zdb -t)', 'cut_check.py',
     "g2 != (cut['txg'], cut['timestamp'], root_of(cut['rootbp']))", "g2[:2] != (cut['txg'], cut['timestamp'])"),
    ('Z3 root_of: checksum one word early', 'cut_check.py', "tuple(words[12:16])", "tuple(words[11:15])"),
    ('Z4 root_of: offset not shifted', 'cut_check.py', "(words[1] & ((1 << 63) - 1)) << 9", "(words[1] & ((1 << 63) - 1))"),
    ('Z5 root_of: no checksum', 'cut_check.py', "tuple(words[12:16])", "()"),
    ('Z6 zdb_root: no checksum', 'cut_check.py',
     "tuple(int(m.group(k), 16) for k in range(3, 7))", "()"),
    ('Z7 group: root not required', 'cut_check.py', "if rc or not t or not ts or not root:", "if rc or not t or not ts:"),
    ('Z8 zdb_root: DVA[1] instead', 'cut_check.py', r"DVA\[0\]=<(\d+)", r"DVA\[1\]=<(\d+)"),
]


def run_tests():
    out = []
    for t in ('test_cut_ref.py', 'test_cut_check.py'):
        p = subprocess.run([sys.executable, '-I', os.path.join(K, t)], cwd=ROOT, capture_output=True, text=True)
        fails = [ln for ln in p.stderr.splitlines() if ln.startswith(('FAIL:', 'ERROR:'))]
        out.append((t, p.returncode, fails))
    return out


def main():
    orig = {}
    for f in ('cut_ref.py', 'cut_check.py'):
        with open(os.path.join(K, f)) as fh:
            orig[f] = fh.read()
    base = run_tests()
    assert all(rc == 0 for _, rc, _ in base), base
    active = {}
    for name, f, old, new in PLANTS:
        text = orig[f]
        if name.startswith('F3b'):
            text = active['F3']
        n = text.count(old)
        if n != 1:
            print(f'{name}: NOT APPLIED (found {n} times)')
            continue
        text = text.replace(old, new)
        active[name.split()[0]] = text
        with open(os.path.join(K, f), 'w') as fh:
            fh.write(text)
        try:
            res = run_tests()
        finally:
            with open(os.path.join(K, f), 'w') as fh:
                fh.write(orig[f])
        killed = [(t, fl) for t, rc, fl in res if rc != 0]
        if killed:
            print(f'{name}: KILLED by ' + '; '.join(f"{t}: {', '.join(x.split(' ')[1] for x in fl[:4]) or 'exit'}"
                                                    for t, fl in killed))
        else:
            print(f'{name}: SURVIVED')


if __name__ == '__main__':
    main()
