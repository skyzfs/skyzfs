#!/usr/bin/env python3
"""#113 at 73869d1: where the start line's "no leading zero" still decides,
now that a candidate longer than the next line's time is skipped first.

A log whose second start stands one byte after a page of the file with a
0 before it (no kill leaves this), through the head's parse_log and
through a copy in which START_LINE takes leading zeros (plant P6).

    python3 -I next3_p6.py ROOT      (ROOT holds tests/kernel)
"""
import os
import shutil
import sys
import tempfile

root = sys.argv[1]
tmp = tempfile.mkdtemp(prefix='rv-p6-')
try:
    k = os.path.join(tmp, 'kernel')
    shutil.copytree(os.path.join(root, 'tests/kernel'), k)
    shutil.copytree(os.path.join(root, 'tests/lib'), os.path.join(tmp, 'lib'))
    with open(os.path.join(k, 'cut_daemon_check.py')) as f:
        src = f.read()
    a = "START_LINE = re.compile(r'([1-9]\\d*)\\t([1-9]\\d*)\\tstart"
    assert src.count(a) == 1
    with open(os.path.join(k, 'cdc_p6.py'), 'w') as f:
        f.write(src.replace(a, "START_LINE = re.compile(r'(\\d+)\\t(\\d+)\\tstart"))
    sys.path.insert(0, k)
    import cut_daemon_check as real  # noqa: E402
    import cdc_p6 as planted  # noqa: E402
    PAGE = 4096

    def mk(start, nxt):
        first = '999000000000\t9937\tstart\t0\t-\t-\t-\t64\n'
        whole = '999900000000\t9937\trecv\t25\twrite\t1047960\t8\t-\n'
        lost = '999950000000\t9937\tlabel\t25\twrite\t1047960\t8\t0cb1ba' + '0' * 100
        fill = (-(len(first) + len(whole) + len(lost)) + 1) % PAGE
        fill += PAGE if fill < 200 else 0
        line = '999000001000\t9937\trecv\t1\tread\t'
        head = first + line + '1' * (fill - len(line) - 5) + '\t8\t-\n' + whole
        assert (len(head) + len(lost) - 1) % PAGE == 0
        return head + lost + f'{start}\t10976\tstart\t1\t-\t-\t-\t64\n{nxt}\t10976\tcuts\t-\t-\t-\t-\ttaken_up\n'

    for title, start, nxt in (('the start and the next line both 12 digits', 999990000000, 999995000000),
                              ('the start 12 digits, the next line 13', 999990000000, 1000000000001)):
        for name, mod in (('the head', real), ('with P6', planted)):
            torn = []
            try:
                inst = mod.parse_log(mk(start, nxt), torn)
                r = f"taken, lost {torn[0]['bytes']} bytes, start at {inst[1]['ns']}" if torn else 'parsed, none lost'
            except mod.RigError as e:
                r = "the rig's error: " + str(e)[:70]
            print(f'{title}; {name}: {r}')
finally:
    shutil.rmtree(tmp)
