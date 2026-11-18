#!/usr/bin/env python3
"""#113 at 3fe4133: parse_log's rule for a line torn by a kill, probed
with small logs in which the place of the tear in the file is chosen.

A log here is: the first instance's start line, whole `recv` lines of it
as filler, its last whole line, what is left of its torn line, the next
instance's start line with no newline before it, and that instance's
next line. The filler's length puts the end of what is left on a
multiple of 4096 of the file, or OFF bytes after it.

    python3 -I next2_torn_rule.py ROOT

ROOT holds tests/kernel/cut_daemon_check.py.
"""
import os
import sys

root = sys.argv[1]
sys.path.insert(0, os.path.join(root, 'tests/kernel'))
import cut_daemon_check as cdc  # noqa: E402

PAGE = 4096
P1, P2 = '9937', '10976'
T_BEGIN, T_LAST, T_TORN, T_START, T_NEXT = 263932000000, 263932700000, 263932738825, 264075526204, 264075546749
LABEL = f'{T_TORN}\t{P1}\tlabel\t25\twrite\t1047960\t8\t' + '0cb1ba00' * 2100


def filler(n, pid=P1, t0=263932100000):
    """Whole recv lines of n bytes in all (n >= 40)."""
    out, i = [], 0
    while n:
        size = n if n <= 80 else min(44, n - 40)
        head = f'{t0 + i}\t{pid}\trecv\t{i % 64}\tread\t'
        out.append(head + '1' * (size - len(head) - 5) + '\t8\t-\n')
        n, i = n - size, i + 1
    return ''.join(out)


def log(lost, off=0, start=None, begin=T_BEGIN, last=T_LAST, nxt=T_NEXT, more=''):
    first = f'{begin}\t{P1}\tstart\t0\t-\t-\t-\t64\n'
    whole = f'{last}\t{P1}\trecv\t25\twrite\t1047960\t8\t-\n'
    start = f'{T_START}\t{P2}\tstart\t1\t-\t-\t-\t64' if start is None else start
    fill = (-(len(first) + len(whole) + len(lost))) % PAGE
    fill += PAGE if fill < 40 else 0
    fill += off
    if fill < 40:
        fill += PAGE
    head = first + filler(fill, t0=begin + 1000) + whole
    assert (len(head) + len(lost) - off) % PAGE == 0
    return head + lost + start + '\n' + f'{nxt}\t{P2}\tcuts\t-\t-\t-\t-\ttaken_up\n' + more, head.count('\n') + 1


def run(text, with_torn=True):
    torn = [] if with_torn else None
    try:
        inst = cdc.parse_log(text, torn)
    except cdc.RigError as e:
        return f"the rig's error: {str(e)[:150]}"
    except Exception as e:  # noqa: BLE001
        return f'{type(e).__name__}: {str(e)[:150]}'
    starts = ', '.join(f"pid {i['pid']} at {i['ns']}" for i in inst)
    lost = '; '.join(f"lost {c['bytes']} bytes of pid {c['pid']}'s line" for c in (torn or []))
    return f"parsed: {starts}; {lost or 'no line lost'}"


def show(title, text, n=None, **kw):
    print(f'  {title}: {run(text, **kw)}')


print('1. what is left of the torn line ends on a page of the file')
ENDS = {
    'one digit of the time': LABEL[:1],
    'four digits of the time': LABEL[:4],
    'the time but its last digit': LABEL[:11],
    'the whole time, no tab': LABEL[:12],
    'the time and its tab': LABEL[:13],
    'one digit of the pid': LABEL[:14],
    'the whole pid, no tab': LABEL[:17],
    'the pid and its tab': LABEL[:18],
    'inside the event\'s name': LABEL[:21],
    'a tab before the label': LABEL[:43],
    'a hex digit a to f': LABEL[:48],
    'a hex digit 0 to 9': LABEL[:44],
    '3916 bytes (a page less than the recorded tear)': LABEL[:3916],
    '8012 bytes (the recorded tear)': LABEL[:8012],
    '16204 bytes (four pages in the line)': LABEL[:16204],
    'a whole barrier line but its newline': f'{T_TORN}\t{P1}\tbarrier\t26\tflush\t-\t-\t0',
    'a whole cut line but its newline': f'{T_TORN}\t{P1}\tcut\t-\t-\t731\t-\t243',
    'a cut line before its number': f'{T_TORN}\t{P1}\tcut\t-\t-\t731\t-\t',
}
for why, lost in ENDS.items():
    text, n = log(lost)
    show(why, text)

print('2. the same, one byte off the page (a kill does not leave this)')
for why in ('four digits of the time', 'the time and its tab', 'one digit of the pid', 'a hex digit a to f',
            '8012 bytes (the recorded tear)'):
    for off in (1, -1):
        text, n = log(ENDS[why], off=off)
        show(f'{why}, {off:+d}', text)

print('3. no line is torn: a whole start line that a page of the file crosses')
for d in (1, 2, 6, 11, 12, 13, 15, 20):
    start = f'{T_START}\t{P2}\tstart\t1\t-\t-\t-\t64'
    # The start line begins d bytes before a page: "lost" is empty and
    # the filler ends d bytes before the page.
    first = f'{T_BEGIN}\t{P1}\tstart\t0\t-\t-\t-\t64\n'
    whole = f'{T_LAST}\t{P1}\trecv\t25\twrite\t1047960\t8\t-\n'
    fill = (-(len(first) + len(whole)) - d) % PAGE
    fill += PAGE if fill < 40 else 0
    text = first + filler(fill, t0=T_BEGIN + 1000) + whole + start + '\n' + f'{T_NEXT}\t{P2}\tcuts\t-\t-\t-\t-\ttaken_up\n'
    show(f'the page falls {d} bytes into the start line', text)
print('   the same with 100 s and more between the last line and the start (the rig leaves seconds)')
for last, st, d in ((95_000_000_000, 196_000_000_000, 1), (95_000_000_000, 196_000_000_000, 2),
                    (950_000_000_000, 1_960_000_000_000, 1), (263_000_000_000, 364_000_000_000, 1),
                    (263_000_000_000, 293_000_000_000, 1)):
    start = f'{st}\t{P2}\tstart\t1\t-\t-\t-\t64'
    first = f'{last - 5_000_000_000}\t{P1}\tstart\t0\t-\t-\t-\t64\n'
    whole = f'{last}\t{P1}\trecv\t25\twrite\t1047960\t8\t-\n'
    fill = (-(len(first) + len(whole)) - d) % PAGE
    fill += PAGE if fill < 40 else 0
    text = (first + filler(fill, t0=last - 4_000_000_000) + whole + start + '\n'
            + f'{st + 20000}\t{P2}\tcuts\t-\t-\t-\t-\ttaken_up\n')
    show(f'last line at {last}, start at {st}, the page {d} bytes into the start line', text)

print('4. two kills in one log, each with a torn line')
a, n = log(LABEL[:8012])
p3 = '11111'
b_lost = f'{T_NEXT + 5000}\t{P2}\tsent\t-\t-\t-'
fill = (-(len(a) + len(b_lost))) % PAGE
fill += PAGE if fill < 40 else 0
two = (a + filler(fill, pid=P2, t0=T_NEXT + 1000) + b_lost + f'{T_NEXT + 9000}\t{p3}\tstart\t1\t-\t-\t-\t64\n'
       + f'{T_NEXT + 9500}\t{p3}\tcuts\t-\t-\t-\t-\ttaken_up\n')
show('a label line torn at the first, a sent line at the second', two)

print('5. with no list for torn lines (a recording that kills no daemon is judged the same way: main always gives one)')
text, n = log(LABEL[:8012])
show('the recorded tear', text, with_torn=False)
text, n = log(LABEL[:4])
show('four digits of the time', text, with_torn=False)

print('7. what is left ends in a long run of decimal digits (the label\'s bytes in hex)')
head = f'{T_TORN}\t{P1}\tlabel\t25\twrite\t1047960\t8\t0cb1ba'
for why, digit, n in (('zero bytes, as the recorded line has them', '0', 9000), ('bytes 0x11, 4000 digits', '1', 4000),
                      ('bytes 0x11, 9000 digits', '1', 9000), ('bytes 0x90, 9000 digits', '90', 4500)):
    text, _ = log(head + digit * n)
    show(why, text)
print('6. bytes that are not ASCII before the tear (the offset is counted in bytes, the line is cut in characters)')
text, n = log(LABEL[:8012])
show('a two-byte character in an earlier line (the tear a byte off the page as the file has it)',
     text.replace('\tread\t1', '\tread\té', 1))
lost = LABEL[:40] + 'é' + LABEL[42:8012]
text, n = log(lost)
show('a two-byte character in what is left of the torn line, as many characters', text)
