#!/usr/bin/env python3
"""#113 at ae36c2f, the next look: the recording `takeover` whose request
log a kill -9 left with a cut line (run 44183715455, ubuntu-26.04),
through the head's judge: as recorded, with the cut line taken out, and
with a line cut in the middle of an instance's log.

    python3 -I next_torn.py CHECKOUT ARTIFACT_DIR
"""
import os
import shutil
import subprocess
import sys
import tempfile

root, art = sys.argv[1], sys.argv[2]
JUDGE = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
src = os.path.join(art, 'takeover')


def judge(change, title):
    tmp = tempfile.mkdtemp(prefix='rv-torn-')
    try:
        d = os.path.join(tmp, 'takeover')
        shutil.copytree(src, d)
        os.remove(os.path.join(d, 'results.tsv'))
        p = os.path.join(d, 'requests.tsv')
        with open(p, 'rb') as f:
            lines = f.read().split(b'\n')
        lines = change(lines)
        with open(p, 'wb') as f:
            f.write(b'\n'.join(lines))
        r = subprocess.run([sys.executable, '-I', '-B', JUDGE, 'takeover', d, p, '-', str(512 << 20)],
                           capture_output=True, text=True, timeout=1500)
        print(f'{title}: exit {r.returncode}')
        with open(os.path.join(d, 'results.tsv')) as f:
            rows = [ln.rstrip('\n').split('\t') for ln in f]
        shown = [x for x in rows if x[1] not in ('PASS', 'INFO', 'CAUGHT', 'SAME') and x[0] != 'time']
        for x in shown:
            print(f'    {x[0]}\t{x[1]}\t{x[2][:200]}')
        if not shown:
            keys = {x[0]: x for x in rows}
            for k in ('cuts', 'cut_numbers', 'held_content', 'instance_2', 'instance_3', 'instance_4'):
                if k in keys:
                    print(f'    {k}\t{keys[k][1]}\t{keys[k][2][:160]}')
            print(f'    {len(rows)} lines, none FAIL or ERROR')
    finally:
        shutil.rmtree(tmp)


def bad(lines):
    return [k for k, ln in enumerate(lines[:-1])
            if not 8 <= len(ln.split(b'\t')) <= 10 or not ln.split(b'\t')[0].isdigit()]


def taken_out(lines):
    """The cut line gone, the start line that follows it in the same line kept."""
    k = bad(lines)[0]
    f = lines[k].split(b'\t')
    s = f.index(b'start')
    ns = len(lines[k + 1].split(b'\t')[0])
    return lines[:k] + [b'\t'.join([f[s - 2][-ns:]] + f[s - 1:])] + lines[k + 1:]


def on_its_own_line(lines):
    """As a start that first ends the line before it would leave it: the
    cut line on a line of its own, then the start line."""
    k = bad(lines)[0]
    f = lines[k].split(b'\t')
    s = f.index(b'start')
    ns = len(lines[k + 1].split(b'\t')[0])
    return (lines[:k] + [b'\t'.join(f[:s - 2] + [f[s - 2][:-ns]]), b'\t'.join([f[s - 2][-ns:]] + f[s - 1:])]
            + lines[k + 1:])


def in_the_middle(lines):
    """The log with its cut line taken out, and a label line of the second
    instance cut after half its bytes, the same instance's next line
    following in the same line."""
    lines = taken_out(lines)
    pid = None
    seen = 0
    for k, ln in enumerate(lines[:-1]):
        f = ln.split(b'\t')
        if f[2] == b'start':
            seen += 1
            pid = f[1]
        if seen == 2 and f[1] == pid and f[2] == b'label' and len(ln) > 4096 \
                and lines[k + 1].split(b'\t')[1] == pid:
            return lines[:k] + [ln[:len(ln) // 2] + lines[k + 1]] + lines[k + 2:]
    raise SystemExit('no long line of the second instance')


judge(lambda lines: lines, 'as recorded (the killed instance\'s last line cut, the next start line in the same line)')
judge(taken_out, 'the cut line taken out, the start line kept')
judge(on_its_own_line, 'the cut line on a line of its own, then the start line')
judge(in_the_middle, 'a line cut in the middle of the second instance\'s log, the cut last line taken out')
