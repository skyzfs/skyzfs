#!/usr/bin/env python3
"""#111 at 8210332: the judge run here on the recordings of a run's artifact.

    python3 -I rejudge.py CHECKOUT ARTIFACT_DIR...

Each recording's directory is copied (the judge appends to results.tsv),
and the judge is run as cut-record.sh runs it. Prints the exit code beside
the one recordings.tsv has, and whether every line's verdict is the run's."""
import os
import shutil
import subprocess
import sys
import tempfile

root, arts = sys.argv[1], sys.argv[2:]
judge = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
for art in arts:
    said = {}
    for ln in open(os.path.join(art, 'recordings.tsv')):
        f = ln.rstrip('\n').split('\t')
        if len(f) == 3 and f[1] == 'checked':
            said[f[0]] = f[2]
    for name in sorted(said):
        tmp = tempfile.mkdtemp(prefix='rv-rejudge-')
        try:
            d = os.path.join(tmp, name)
            shutil.copytree(os.path.join(art, name), d)
            with open(os.path.join(d, 'results.tsv')) as f:
                before = [ln.split('\t')[:2] for ln in f if not ln.startswith('time\t')]
            os.remove(os.path.join(d, 'results.tsv'))
            p = subprocess.run([sys.executable, '-I', '-B', judge, name, d, os.path.join(d, 'requests.tsv'), '-',
                                str(512 << 20)], capture_output=True, text=True, timeout=900)
            with open(os.path.join(d, 'results.tsv')) as f:
                after = [ln.split('\t')[:2] for ln in f if not ln.startswith('time\t')]
            print(f"{os.path.basename(art)} {name}: exit {p.returncode} here, the run's {said[name]}; "
                  f"{len(after)} lines, {'the same verdicts' if after == before else 'DIFFERENT verdicts'}")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
