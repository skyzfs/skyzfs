#!/usr/bin/env python3
"""#113 at 98024d5: the judge of that head run here on every recording of a
run's artifact (cut rig on the daemon); prints each recording's exit code
and whether its lines are the run's, and the lines of `failed` in full.

    python3 -I rejudge.py CHECKOUT ARTIFACT_DIR...
"""
import os
import shutil
import subprocess
import sys
import tempfile

root, arts = sys.argv[1], sys.argv[2:]
judge = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
for art in arts:
    names = sorted(n for n in os.listdir(art) if os.path.isdir(os.path.join(art, n)))
    for name in names:
        src = os.path.join(art, name)
        tmp = tempfile.mkdtemp(prefix='rv-rejudge-')
        try:
            d = os.path.join(tmp, name)
            shutil.copytree(src, d)
            with open(os.path.join(d, 'results.tsv')) as f:
                before = [ln.rstrip('\n').split('\t') for ln in f if not ln.startswith('time\t')]
            os.remove(os.path.join(d, 'results.tsv'))
            p = subprocess.run([sys.executable, '-I', '-B', judge, name, d, os.path.join(d, 'requests.tsv'), '-',
                                str(512 << 20)], capture_output=True, text=True, timeout=1500)
            with open(os.path.join(d, 'results.tsv')) as f:
                after = [ln.rstrip('\n').split('\t') for ln in f if not ln.startswith('time\t')]
            same = before == after
            print(f'{os.path.basename(art)} {name}: exit {p.returncode} here, {len(after)} lines, '
                  f"{'the same as the run' if same else 'NOT the run lines'}", flush=True)
            if not same:
                for b, a in zip(before, after):
                    if b != a:
                        print(f'    run:  {b}\n    here: {a}')
            if name == 'failed':
                for f in after:
                    print(f'    {f[0]}\t{f[1]}\t{f[2][:230]}')
        finally:
            shutil.rmtree(tmp)
