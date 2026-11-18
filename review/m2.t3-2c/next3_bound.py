#!/usr/bin/env python3
"""#113 at 73869d1: the bound on the digits of a torn line's times
(torn_line skips a candidate whose start time, or whose own time without
leading zeros, has more digits than the next line's time, and takes no
torn line where the next line's time has more than 20).

Small logs as next2_torn_rule.py makes them, with the times chosen, and
one log made from the recorded `takeover` (run 44183715455) in which a
page falls on a digit other than 0 two pages before the tear.

    python3 -I next3_bound.py ROOT [ARTIFACT_DIR]
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile

root = sys.argv[1]
art = sys.argv[2] if len(sys.argv) > 2 else None
sys.path.insert(0, os.path.join(root, 'tests/kernel'))
import cut_daemon_check as cdc  # noqa: E402

PAGE = 4096
P1, P2, P3 = '9937', '10976', '11500'


def filler(n, t0, pid=P1):
    out, i = [], 0
    while n:
        size = n if n <= 160 else min(80, n - 80)
        head = f'{t0 + i}\t{pid}\trecv\t{i % 64}\tread\t'
        out.append(head + '1' * (size - len(head) - 5) + '\t8\t-\n')
        n, i = n - size, i + 1
    return ''.join(out)


def log(lost, begin, last, start, nxt, off=0, more=''):
    """begin, last, start: times (int); nxt: the next line's first column as text."""
    first = f'{begin}\t{P1}\tstart\t0\t-\t-\t-\t64\n'
    whole = f'{last}\t{P1}\trecv\t25\twrite\t1047960\t8\t-\n'
    st = f'{start}\t{P2}\tstart\t1\t-\t-\t-\t64'
    fill = (-(len(first) + len(whole) + len(lost))) % PAGE
    fill += PAGE if fill < 80 else 0
    fill += off
    head = first + filler(fill, begin + 1000) + whole
    assert (len(head) + len(lost) - off) % PAGE == 0
    return head + lost + st + '\n' + f'{nxt}\t{P2}\tcuts\t-\t-\t-\t-\ttaken_up\n' + more


def run(text):
    torn = []
    try:
        inst = cdc.parse_log(text, torn)
    except cdc.RigError as e:
        return f"the rig's error: {str(e)[:140]}"
    except Exception as e:  # noqa: BLE001
        return f'{type(e).__name__}: {str(e)[:60]}'
    starts = ', '.join(f"pid {i['pid']} at {i['ns']}" for i in inst)
    lost = '; '.join(f"lost {c['bytes']} bytes of pid {c['pid']}'s line" for c in torn)
    return f"taken: {starts}; {lost}" if torn else f'parsed: {starts}; no line lost'


def shapes(t_torn):
    label = f'{t_torn}\t{P1}\tlabel\t25\twrite\t1047960\t8\t' + '0cb1ba00' * 2100
    n = len(str(t_torn))
    return {
        'one digit of the time': label[:1],
        'four digits of the time': label[:4],
        'the time but its last digit': label[:n - 1],
        'the whole time, no tab': label[:n],
        'the time and its tab': label[:n + 1],
        'one digit of the pid': label[:n + 2],
        'the whole pid, no tab': label[:n + 5],
        'the pid and its tab': label[:n + 6],
        'inside the event\'s name': label[:n + 9],
        'a tab before the label': label[:n + 31],
        'a hex digit a to f': label[:n + 36],
        'a hex digit 0 to 9': label[:n + 32],
        '3916 bytes': label[:3916],
        '8012 bytes': label[:8012],
        '16204 bytes': label[:16204],
        'a whole barrier line but its newline': f'{t_torn}\t{P1}\tbarrier\t26\tflush\t-\t-\t0',
        'a whole cut line but its newline': f'{t_torn}\t{P1}\tcut\t-\t-\t731\t-\t243',
        'a cut line before its number': f'{t_torn}\t{P1}\tcut\t-\t-\t731\t-\t',
    }


def all_shapes(title, begin, last, t_torn, start, nxt):
    taken = other = 0
    for why, lost in shapes(t_torn).items():
        r = run(log(lost, begin, last, start, str(nxt)))
        if r.startswith('taken') and f'lost {len(lost)} bytes' in r:
            taken += 1
        else:
            other += 1
            print(f'    {why}: {r}')
    print(f'  {title}: {taken} of {taken + other} shapes taken')


print('1. the eighteen shapes on a page, with the times a recording can have')
all_shapes('times of 12 digits (the recorded ones, 264 s)', 263932000000, 263932700000, 263932738825, 264075526204, 264075546749)
all_shapes('times of 13 digits (after 1000 s)', 1263932000000, 1263932700000, 1263932738825, 1264075526204, 1264075546749)
all_shapes('the torn line at 999.95 s, the start at 1000.1 s (12 digits, then 13)',
           999000000000, 999900000000, 999950000000, 1000100000000, 1000200000000)
all_shapes('the start at 999.99 s, the next line at 1000.0 s (the start 12 digits, the next line 13)',
           999000000000, 999900000000, 999950000000, 999990000000, 1000000000001)
all_shapes('times of 9 digits (the first second after boot)', 500000000, 600000000, 700000000, 800000000, 900000000)
all_shapes('times of 20 digits', 18446744073709000000, 18446744073709100000, 18446744073709200000,
           18446744073709300000, 18446744073709400000)

B, L, S, N = 263932000000, 263932700000, 264075526204, 264075546749
print('2. what no kill leaves: the times the bound is about')
CASES = {
    'left: 13 digits and no tab, where the next line\'s time has 12': ('2639327388251', S, str(N)),
    'left: 12 digits and no tab (a whole time)': ('263932738825', S, str(N)),
    'left: 30 digits and no tab': ('2' * 30, S, str(N)),
    'left: 0 and eleven digits, no tab (a leading zero)': ('026393273882', S, str(N)),
    'left: twenty zeros and a 1, no tab': ('0' * 20 + '1', S, str(N)),
    'left: a time with a leading zero, the pid and more': (f'0263932738825\t{P1}\tbarrier\t26\tflush\t-\t-\t0', S, str(N)),
    'left: a time of 9000 digits, the pid and more': ('2' * 9000 + f'\t{P1}\tbarrier\t26\tflush\t-\t-\t0', S, str(N)),
    'the next line\'s time has 20 digits': ('2639', S, '18446744073709551615'),
    'the next line\'s time has 21 digits': ('2639', S, '184467440737095516150'),
    'the next line\'s time has 5000 digits': ('2639', S, '1' * 5000),
    'the next line\'s time has a leading zero and is later': ('2639', S, '0' + str(N)),
    'the next line\'s time is shorter than the start\'s': ('2639', S, str(N)[:11]),
    'the start has 13 digits, the next line 12': ('2639', 2640755262040, str(N)),
}
for why, (lost, start, nxt) in CASES.items():
    print(f'  {why}: {run(log(lost, B, L, start, nxt))}')

print('3. a second kill in the line after the start (its time cut: the start is not on a page, so not a tear a kill leaves)')
for digits in (4, 9, 12):
    lost2 = str(N)[:digits]
    text = log('2639', B, L, S, str(N))
    # the line after the start is: lost2 + the third instance's start line, then a line of the third
    text = text.rsplit('\n', 2)[0] + '\n' + lost2 + f'264500000000\t{P3}\tstart\t1\t-\t-\t-\t64\n' \
        + f'264500100000\t{P3}\tcuts\t-\t-\t-\t-\ttaken_up\n'
    print(f'  {digits} digits of the second instance\'s first line, then the third\'s start: {run(text)}')

print('4. T3: a line of digits over three pages (what is left ends in N digits that begin on a page with a digit 1 to 9)')
for n, d in ((4096, '1'), (8192, '1'), (8192, '9'), (12288, '7'), (262144, '1')):
    lost = f'263932738825\t{P1}\tlabel\t25\twrite\t1047960\t8\t0cb1ba' + d * n
    print(f'  {n} digits {d}: {run(log(lost, B, L, S, str(N)))}')
lost = f'263932738825\t{P1}\tlabel\t25\twrite\t1047960\t8\t0cb1ba' + '1' + '0' * 8191
print(f'  a 1 on the page and 8191 zeros: {run(log(lost, B, L, S, str(N)))}')

if art:
    print('5. T3 on the recorded takeover: the kill moved into a long label line, a 1 put on a page two pages before the tear')
    JUDGE = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
    src = os.path.join(art, 'takeover')
    START = re.compile(rb'\t([1-9]\d*)\tstart\t\d+\t-\t-\t-\t\d+\n(\d+)\t')
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
            rows = []
            if os.path.exists(os.path.join(d, 'results.tsv')):
                with open(os.path.join(d, 'results.tsv')) as f:
                    rows = [ln.rstrip('\n').split('\t') for ln in f]
            return r.returncode, rows, (r.stdout + r.stderr).strip().splitlines()
        finally:
            shutil.rmtree(tmp)

    def lines_of(rows):
        return [(x[0], x[1], x[2]) for x in rows if x[0] not in ('time', 'obs_torn_at_kill')]

    done = 0
    for q in range(tear - PAGE, 3 * PAGE, -PAGE):
        s = data.rfind(b'\n', 0, q) + 1
        p = q - 2 * PAGE
        if p <= s + 60 or not re.fullmatch(rb'0+', data[p:q]):
            continue
        made = data[:p] + b'1' + data[p + 1:q] + rest
        rc_t, rows_t, out_t = judge(made)
        rc_c, rows_c, out_c = judge(data[:s] + rest)
        torn = [x for x in rows_t if x[0] == 'obs_torn_at_kill']
        same = rc_t == rc_c and lines_of(rows_t) == lines_of(rows_c)
        print(f'  the kill {(tear - q) // PAGE} pages before the recorded one, {q - s} bytes into a line of '
              f'{data.index(bytes([10]), s) - s + 1}; the control is the log ended at the end of the line before')
        print(f'    torn: exit {rc_t}; control: exit {rc_c}; {"the same lines" if same else "DIFFERENT"}')
        print(f'    {torn[0][2][:150] if torn else (out_t[-1][:170] if out_t else "")}')
        done += 1
        if done == 2:
            break
    if not done:
        print('  no such place found')
