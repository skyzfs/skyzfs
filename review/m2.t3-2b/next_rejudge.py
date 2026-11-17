#!/usr/bin/env python3
"""#111 at 1f71211: the judge of that head run here on the takeover recording
of a run's artifact; prints its exit code and the instance lines, and the
first commit the holder's log shows beside each start.

    python3 -I next_rejudge.py CHECKOUT ARTIFACT_DIR...
"""
import os
import shutil
import subprocess
import sys
import tempfile

root, arts = sys.argv[1], sys.argv[2:]
sys.path.insert(0, os.path.join(root, 'tests/kernel'))
import cut_daemon_check as c  # noqa: E402

judge = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
for art in arts:
    src = os.path.join(art, 'takeover')
    with open(os.path.join(src, 'holder.log')) as f:
        first = c.first_commit(f.read())
    with open(os.path.join(src, 'requests.tsv')) as f:
        starts = [inst['ns'] for inst in c.parse_log(f.read())]
    tmp = tempfile.mkdtemp(prefix='rv-rejudge-')
    try:
        d = os.path.join(tmp, 'takeover')
        shutil.copytree(src, d)
        with open(os.path.join(d, 'results.tsv')) as f:
            before = {ln.split('\t')[0]: ln.split('\t')[1] for ln in f if not ln.startswith('time\t')}
        os.remove(os.path.join(d, 'results.tsv'))
        p = subprocess.run([sys.executable, '-I', '-B', judge, 'takeover', d, os.path.join(d, 'requests.tsv'), '-',
                            str(512 << 20)], capture_output=True, text=True, timeout=1500)
        with open(os.path.join(d, 'results.tsv')) as f:
            after = [ln.rstrip('\n').split('\t') for ln in f if not ln.startswith('time\t')]
        print(f'{os.path.basename(art)}: exit {p.returncode} here; the first release answered '
              + ('never' if first is None else ', '.join(
                  f'{(first - s) / 1e9:+.2f} s from start {k + 1}' for k, s in enumerate(starts))))
        for f in after:
            if f[0].startswith('instance_'):
                print(f'  {f[0]}: {f[1]} (the run: {before.get(f[0])}): {f[2][:150]}')
        diff = [(f[0], before.get(f[0]), f[1]) for f in after if before.get(f[0]) != f[1]]
        print(f'  lines whose verdict differs from the run\'s: {diff or "none"}')
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
