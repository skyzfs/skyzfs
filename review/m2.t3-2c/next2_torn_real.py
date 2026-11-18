#!/usr/bin/env python3
"""#113 at 3fe4133: the recorded `takeover` (run 44183715455, ubuntu-26.04)
with its first kill moved back to earlier pages of the file, through a
head's judge.

The recording's first instance was killed while it wrote, and its last
line stops at byte 13389824 of the log, a multiple of 4096. For each of
the pages before that one, the instance's log is ended at that page: what
is left of the line the page falls in, and the next instance's start
after it with no newline, as a kill at that moment would have left the
file. Beside each, the control: the log ended at the end of the line
before (a kill between two writes, no torn line). The rule is right
where the two give the same exit and the same lines, but for
obs_torn_at_kill.

The first page found for each shape of what is left (where in the line
the page falls) is judged, and the recorded tear itself.

    python3 -I next2_torn_real.py ROOT ARTIFACT_DIR [PAGES_BACK]
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile

root, art = sys.argv[1], sys.argv[2]
BACK = int(sys.argv[3]) if len(sys.argv) > 3 else 400
JUDGE = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
src = os.path.join(art, 'takeover')
PAGE = 4096
START = re.compile(rb'\t([1-9]\d*)\tstart\t\d+\t-\t-\t-\t\d+\n(\d+)\t')

with open(os.path.join(src, 'requests.tsv'), 'rb') as f:
    data = f.read()


def shape(line, p):
    """Where byte p of the line falls."""
    cols = line[:p].split(b'\t')
    last = line[p - 1:p]
    if len(cols) == 1:
        return 'inside the time' if p < line.index(b'\t') else 'after the time, before its tab'
    if len(cols) == 2:
        if last == b'\t':
            return 'after the time\'s tab'
        return 'inside the pid' if p < line.index(b'\t', line.index(b'\t') + 1) else 'after the pid, before its tab'
    if last == b'\t':
        return f'after the tab of column {len(cols) - 1}'
    if len(cols) >= 8 and last in b'abcdef':
        return 'a hex digit a to f in the last column'
    if len(cols) >= 8 and p == len(line) - 1:
        return 'the whole line but its newline'
    if len(cols) >= 8:
        return 'a digit 0 to 9 in the last column'
    return f'inside column {len(cols)}'


def judge(text):
    tmp = tempfile.mkdtemp(prefix='rv-torn-')
    try:
        d = os.path.join(tmp, 'takeover')
        shutil.copytree(src, d)
        os.remove(os.path.join(d, 'results.tsv'))
        p = os.path.join(d, 'requests.tsv')
        with open(p, 'wb') as f:
            f.write(text)
        r = subprocess.run([sys.executable, '-I', '-B', JUDGE, 'takeover', d, p, '-', str(512 << 20)],
                           capture_output=True, text=True, timeout=1500)
        with open(os.path.join(d, 'results.tsv')) as f:
            rows = [ln.rstrip('\n').split('\t') for ln in f]
        return r.returncode, rows
    finally:
        shutil.rmtree(tmp)


def lines_of(rows):
    return [(x[0], x[1], x[2]) for x in rows if x[0] not in ('time', 'obs_torn_at_kill')]


def bad(rows):
    return [x for x in rows if x[1] not in ('PASS', 'INFO', 'CAUGHT', 'SAME') and x[0] != 'time']


# The recorded tear: the one start line with more than a time and a tab
# before its pid in its line. What is left of the torn line ends in a long
# run of the digit 0 (the label's bytes), so the start's time is taken as
# long as the time of the line after it.
m = [x for x in START.finditer(data) if b'\t' in data[data.rfind(b'\n', 0, x.start()) + 1:x.start()]]
assert len(m) == 1, len(m)
tear = m[0].start() - len(m[0].group(2))
assert tear % PAGE == 0, tear
rest = data[tear:]
print(f'the recorded tear: byte {tear} = {tear // PAGE} * 4096 + {tear % PAGE}')
rc0, rows0 = judge(data)
torn0 = [x for x in rows0 if x[0] == 'obs_torn_at_kill']
print(f'as recorded: exit {rc0}; {torn0[0][2][:150] if torn0 else "no obs_torn_at_kill"}')
for x in bad(rows0):
    print(f'    {x[0]}\t{x[1]}\t{x[2][:160]}')

seen = {}
for j in range(1, BACK + 1):
    b = tear - j * PAGE
    s = data.rfind(b'\n', 0, b) + 1          # the line the page falls in begins here
    if s == b:
        what = 'no torn line: the page is where a line begins'
    else:
        e = data.index(b'\n', s)
        what = shape(data[s:e + 1], b - s)
    if what not in seen:
        seen[what] = (j, b, s)
print(f'{len(seen)} shapes in the {BACK} pages before it')
same = differ = 0
for what, (j, b, s) in sorted(seen.items(), key=lambda kv: kv[1][0]):
    dropped = data[s:tear].count(b'\n')
    rc_t, rows_t = judge(data[:b] + rest)
    rc_c, rows_c = judge(data[:s] + rest)
    torn = [x for x in rows_t if x[0] == 'obs_torn_at_kill']
    eq = rc_t == rc_c and lines_of(rows_t) == lines_of(rows_c)
    same, differ = same + eq, differ + (not eq)
    print(f'{j} pages back, {b - s} bytes left of a line of {data.index(bytes([10]), s) - s + 1} ({what}); '
          f'{dropped} whole lines fewer than recorded')
    print(f'    torn: exit {rc_t}; control: exit {rc_c}; {"the same lines" if eq else "DIFFERENT"}')
    print(f'    {torn[0][2][:170] if torn else "no obs_torn_at_kill line"}')
    for x in bad(rows_t)[:4]:
        print(f'    torn     {x[0]}\t{x[1]}\t{x[2][:150]}')
    if not eq:
        for x in bad(rows_c)[:4]:
            print(f'    control  {x[0]}\t{x[1]}\t{x[2][:150]}')
print(f'{same} shapes judged as their control, {differ} differently')
