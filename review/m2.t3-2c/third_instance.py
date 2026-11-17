#!/usr/bin/env python3
"""#113 at 98024d5: a third instance after the one that acknowledged the
failure, as a right daemon would leave it (a hand-made log: the recording
`failed` of a run's artifact, its second instance cut in two at a quiet
cut line, the third taking the device and the new state over). No
recording has this today; it shows what `instance_3` would say.

    python3 -I third_instance.py CHECKOUT ARTIFACT_DIR...
"""
import os
import shutil
import subprocess
import sys
import tempfile

root, arts = sys.argv[1], sys.argv[2:]
JUDGE = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
PID3 = '999999'

for art in arts:
    src = os.path.join(art, 'failed')
    tmp = tempfile.mkdtemp(prefix='rv-third-')
    try:
        d = os.path.join(tmp, 'failed')
        shutil.copytree(src, d)
        os.remove(os.path.join(d, 'results.tsv'))
        with open(os.path.join(d, 'instances.tsv')) as f:
            pid2 = f.read().splitlines()[1].split('\t')[0]
        with open(os.path.join(d, 'requests.tsv')) as f:
            lines = f.read().splitlines()
        # A cut line of the second instance with nothing in flight at it.
        open_, at, local = set(), None, None
        for i, ln in enumerate(lines):
            f = ln.split('\t')
            if f[1] != pid2:
                continue
            if f[2] == 'recv' and f[4] in ('write', 'flush', 'write_zeroes'):
                open_.add((f[4], f[3]))
            elif f[2] == 'applied':
                open_.discard(('write', f[3]))
            elif f[2] == 'barrier':
                open_.discard(('flush', f[3]))
            elif f[2] == 'commit' and f[4] == 'write_zeroes':
                open_.discard((f[4], f[3]))
            elif f[2] == 'cut' and not open_ and 2 <= int(f[6]) <= 5 and at is None:
                at, local = i, int(f[6])
        if at is None:
            print(f'{os.path.basename(art)}: no quiet cut line of the second instance')
            continue
        ns = int(lines[at].split('\t')[0])
        rest = []
        for ln in lines[at + 1:]:
            f = ln.split('\t')
            f[1] = PID3
            rest.append('\t'.join(f))
        new = lines[:at + 1] + [f'{ns + 1}\t{PID3}\tstart\t0\t-\t-\t-\t64',
                                f'{ns + 2}\t{PID3}\tcuts\t-\t-\t-\t-\ttaken_up'] + rest
        with open(os.path.join(d, 'requests.tsv'), 'w') as f:
            f.write('\n'.join(new) + '\n')
        with open(os.path.join(d, 'instances.tsv'), 'a') as f:
            f.write(f'{PID3}\ttaken-over\n')
        with open(os.path.join(d, 'status-start-2.txt')) as f:
            st = f.read()
        with open(os.path.join(d, 'status-start-3.txt'), 'w') as f:
            f.write(st.replace('cuts 0\n', f'cuts {local}\n').replace('cut_state begun\n', 'cut_state taken_up\n'))
        with open(os.path.join(d, 'status.txt')) as f:
            st = f.read()
        with open(os.path.join(d, 'status.txt'), 'w') as f:
            f.write(st.replace('cut_state begun\n', 'cut_state taken_up\n'))
        p = subprocess.run([sys.executable, '-I', '-B', JUDGE, 'failed', d, os.path.join(d, 'requests.tsv'), '-',
                            str(512 << 20)], capture_output=True, text=True, timeout=1500)
        print(f'{os.path.basename(art)}: the third instance starts after cut {local} of the new state, takes it up '
              f'and says cuts {local}: exit {p.returncode}')
        with open(os.path.join(d, 'results.tsv')) as f:
            for ln in f:
                k, v, t = ln.rstrip('\n').split('\t')
                if v not in ('PASS', 'INFO', 'CAUGHT', 'SAME') or k.startswith('instance_') or k in ('cut_numbers', 'status'):
                    print(f'    {k}\t{v}\t{t[:300]}')
    finally:
        shutil.rmtree(tmp)
