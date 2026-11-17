#!/usr/bin/env python3
"""#113, the next look (at ae36c2f, then at e88773b): the recording
`takeover` whose request log a kill -9 left with a line cut short (run
44183715455, ubuntu-26.04), through the head's judge: as recorded, with
the cut line taken out, with a line cut in the middle of an instance's
log, and (for e88773b's rule) with the same cut line ending otherwise,
with other lines cut, and with a line cut before an instance that did not
take the device over.

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
        print(f'{title}: exit {r.returncode}', flush=True)
        with open(os.path.join(d, 'results.tsv')) as f:
            rows = [ln.rstrip('\n').split('\t') for ln in f]
        for x in rows:
            if x[0] == 'obs_torn_at_kill':
                print(f'    {x[0]}\t{x[1]}\t{x[2][:220]}')
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


def parts(lines):
    """-> the index of the line cut short, what is left of it, the start line after it."""
    k = bad(lines)[0]
    f = lines[k].split(b'\t')
    s = f.index(b'start')
    ns = len(lines[k + 1].split(b'\t')[0])
    start = b'\t'.join([f[s - 2][-ns:]] + f[s - 1:])
    return k, lines[k][:len(lines[k]) - len(start)], start


def left(what):
    """The line cut short leaves `what(its bytes as recorded)` instead."""
    def change(lines):
        k, frag, start = parts(lines)
        return lines[:k] + [what(frag) + start] + lines[k + 1:]
    return change


def before_the_fourth(lines):
    """The cut line taken out; the third instance's last line (a `sent`
    line that ends in a digit) without its newline, the fourth instance's
    start line (which created the device) following in the same line."""
    lines = taken_out(lines)
    k = [j for j, ln in enumerate(lines) if ln.split(b'\t')[2:3] == [b'start']][3]
    return lines[:k - 1] + [lines[k - 1] + lines[k]] + lines[k + 1:]


def before_the_third(lines):
    """As recorded, and the second instance's last line (a `sent` line
    that ends in a digit; that instance was killed as well) without its
    newline, the third instance's start line following it."""
    k = [j for j, ln in enumerate(lines) if ln.split(b'\t')[2:3] == [b'start']][-2]
    return lines[:k - 1] + [lines[k - 1] + lines[k]] + lines[k + 1:]


def last_line_gone(lines):
    """The cut line taken out and the first instance's last whole line
    with it: a kill one line earlier."""
    k = bad(lines)[0]
    lines = taken_out(lines)
    return lines[:k - 1] + lines[k:]


judge(lambda lines: lines, 'as recorded (the killed instance\'s last line cut, the next start line in the same line)')
judge(taken_out, 'the cut line taken out, the start line kept')
judge(on_its_own_line, 'the cut line on a line of its own, then the start line')
judge(in_the_middle, 'a line cut in the middle of the second instance\'s log, the cut last line taken out')
judge(last_line_gone, 'the cut line and the whole line before it taken out (a kill one line earlier)')
judge(left(lambda frag: frag[:-1] + b'a'), 'what is left of the cut line ends in the hex digit a, not 0')
judge(left(lambda frag: frag[:-1] + b'f'), 'what is left of the cut line ends in the hex digit f')
judge(left(lambda frag: b'\t'.join(frag.split(b'\t')[:7]) + b'\t'), 'what is left ends in a tab (the seven columns before the bytes)')
judge(left(lambda frag: frag[:len(frag) - 4096]), 'the line cut one page (4096 bytes) earlier')
judge(left(lambda frag: frag[:4]), 'only four digits of the line\'s time are left')
judge(left(lambda frag: frag[:15]), 'the time and two digits of the pid are left')
judge(left(lambda frag: frag.split(b'\t')[0] + b'\t' + frag.split(b'\t')[1] + b'\tcut\t50\tflush\titself\t229\t7'),
      'a cut line is cut short instead, within its last column')
judge(left(lambda frag: frag.split(b'\t')[0] + b'\t' + frag.split(b'\t')[1] + b'\tbarrier\t50\tflush\t-\t-\t'),
      'a barrier line is cut short instead, before its last column')
judge(before_the_third, 'as recorded, and the second killed instance\'s last line whole but for its newline')
judge(before_the_fourth, 'a line whole but for its newline before the start of the instance that created the device')
