#!/usr/bin/env python3
"""#113 at 3fe4133, T3: how often a tear in the recorded `takeover` log
(run 44183715455, ubuntu-26.04) would meet the conversion limit in
torn_line.

torn_line turns the digits at the beginning of "the rest" into a number at
every page inside a line. Where a page falls on a decimal digit other than
0 and everything from there to the tear is decimal digits, the rest is a
"start line" whose time has those digits and the real start's: with two
pages or more between the two that is over 8192 digits, and Python's int()
refuses more than 4300.

For every page of the first instance's part of the log at which a kill
could have torn a line, this says whether an earlier page of the same line
is such a place, and judges the first one found beside its control (the
log ended at the end of the line before).

    python3 -I next2_torn_digits.py ROOT ARTIFACT_DIR
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile

root, art = sys.argv[1], sys.argv[2]
JUDGE = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
src = os.path.join(art, 'takeover')
PAGE = 4096
START = re.compile(rb'\t([1-9]\d*)\tstart\t\d+\t-\t-\t-\t\d+\n(\d+)\t')
DIGITS = re.compile(rb'\d*')

with open(os.path.join(src, 'requests.tsv'), 'rb') as f:
    data = f.read()
m = [x for x in START.finditer(data) if b'\t' in data[data.rfind(b'\n', 0, x.start()) + 1:x.start()]]
assert len(m) == 1, len(m)
tear = m[0].start() - len(m[0].group(2))
assert tear % PAGE == 0, tear
rest = data[tear:]


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
        return r.returncode, (r.stdout + r.stderr).strip().splitlines()
    finally:
        shutil.rmtree(tmp)


inside = hit = 0
first = None
long_lines = set()
for q in range(PAGE, tear + 1, PAGE):
    s = data.rfind(b'\n', 0, q) + 1
    if s == q:
        continue                      # the page is where a line begins: nothing torn
    inside += 1
    for p in range(q - 2 * PAGE, s, -PAGE):
        if data[p:p + 1] in b'123456789' and DIGITS.match(data, p, q).end() == q:
            hit += 1
            long_lines.add(s)
            if first is None:
                first = (q, p, s)
            break
print(f'the first instance wrote {tear} bytes, {tear // PAGE} pages; a kill at {inside} of them tears a line')
print(f'at {hit} of those, in {len(long_lines)} lines, an earlier page of the line is a digit 1 to 9 with only digits '
      f'up to the tear, two pages or more away')
if first:
    q, p, s = first
    e = data.index(b'\n', s)
    print(f'the first: a line of {e - s + 1} bytes that begins {data[s:s + 60]!r}; torn after {q - s} bytes; '
          f'the page {(q - p) // PAGE} pages before the tear falls on {data[p:p + 12]!r}...')
    rc_t, out_t = judge(data[:q] + rest)
    rc_c, out_c = judge(data[:s] + rest)
    print(f'    torn there: exit {rc_t}: {out_t[-1][:200] if out_t else ""}')
    print(f'    control (the log ended at the end of the line before): exit {rc_c}: {out_c[-1][:200] if out_c else ""}')
