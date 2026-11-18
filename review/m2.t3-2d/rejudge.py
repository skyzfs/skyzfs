#!/usr/bin/env python3
"""#115: the judge of a checkout run here on every recording of a run's
artifact, on a copy; prints its exit, whether its lines are the run's
own (verdict and detail), and for takeover and failed the floors' lines
and obs_unstored_at_kill.

    python3 -I rejudge.py CHECKOUT ARTIFACT_DIR...
"""
import os
import shutil
import subprocess
import sys
import tempfile

root, arts = sys.argv[1], sys.argv[2:]
judge = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
SHOW = ('held_across_start', 'carried_across_failure', 'obs_unstored_at_kill', 'obs_noflush_groups')


def lines_of(path):
    with open(path) as f:
        return [ln.rstrip('\n').split('\t') for ln in f if not ln.startswith('time\t')]


for art in arts:
    tag = os.path.basename(os.path.normpath(art))
    names = sorted(n for n in os.listdir(art) if os.path.isfile(os.path.join(art, n, 'requests.tsv')))
    for name in names:
        src = os.path.join(art, name)
        tmp = tempfile.mkdtemp(prefix='rv-rejudge-')
        try:
            d = os.path.join(tmp, name)
            shutil.copytree(src, d)
            before = lines_of(os.path.join(d, 'results.tsv'))
            os.remove(os.path.join(d, 'results.tsv'))
            p = subprocess.run([sys.executable, '-I', '-B', judge, name, d, os.path.join(d, 'requests.tsv'), '-',
                                str(512 << 20)], capture_output=True, text=True, timeout=1500)
            after = lines_of(os.path.join(d, 'results.tsv')) if os.path.exists(os.path.join(d, 'results.tsv')) else []
            same = 'the same as the run' if after == before else (
                'verdicts as the run, details differ in: ' + ', '.join(a[0] for a, b in zip(after, before) if a != b)
                if [a[:2] for a in after] == [b[:2] for b in before] else 'NOT the run\'s lines')
            print(f'{tag} {name}: exit {p.returncode} here, {len(after)} lines, {same}')
            for f in after:
                if f[0] in SHOW:
                    print(f'    {f[0]}\t{f[1]}\t{f[2][:240]}')
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
