#!/usr/bin/env python3
"""#111 at 1f71211: what the judge of that head says of a takeover recording
whose second start's status is changed by hand (a copy; the artifact is not
touched).

    python3 -I next_lax.py CHECKOUT ARTIFACT_DIR CASE...

CASE is KEY=VALUE for status-start-2.txt, for example intent_state=taken_up.
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile

root, art, cases = sys.argv[1], sys.argv[2], sys.argv[3:]
judge = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
src = os.path.join(art, 'takeover')
asked = [int(ln.split('\t')[0]) for ln in open(os.path.join(src, 'holder.log'))
         if re.match(r'\d+\t> \S+ release ', ln)]
starts = [int(ln.split('\t')[0]) for ln in open(os.path.join(src, 'requests.tsv')) if ln.split('\t')[2:3] == ['start']]
print(f'{os.path.basename(art)}: the first release was asked {(asked[0] - starts[1]) / 1e9:+.2f} s from start 2')
for case in cases:
    key, value = case.split('=')
    tmp = tempfile.mkdtemp(prefix='rv-lax-')
    try:
        d = os.path.join(tmp, 'takeover')
        shutil.copytree(src, d)
        os.remove(os.path.join(d, 'results.tsv'))
        p = os.path.join(d, 'status-start-2.txt')
        text = open(p).read()
        new, n = re.subn(rf'(?m)^{key} .*$', f'{key} {value}', text)
        assert n == 1, key
        open(p, 'w').write(new)
        r = subprocess.run([sys.executable, '-I', '-B', judge, 'takeover', d, os.path.join(d, 'requests.tsv'), '-',
                            str(512 << 20)], capture_output=True, text=True, timeout=1500)
        line = [ln for ln in open(os.path.join(d, 'results.tsv')) if ln.startswith('instance_2\t')]
        print(f'  the second start says {key} {value}: exit {r.returncode}; ' + (line[0].rstrip()[:230] if line else 'no line'))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
