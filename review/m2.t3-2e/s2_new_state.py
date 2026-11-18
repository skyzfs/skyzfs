#!/usr/bin/env python3
"""#116 at 49c5f62, S2 (instance_lines): a start that takes up a state an
acknowledged start began, every cut of that state released before it.

    s2_new_state.py CHECKOUT

The log is the unit test's own (test_a_third_instance_takes_the_new_state_up):
the fixture `life`, a second instance that acknowledged a failure after
cut K and began a state, and a third that took the device over after the
new state's cut 2. The daemon numbers the new state's cuts from 1, and
the holder releases them by those numbers (`release 1`, `release 2`).
A right daemon whose holder released both before the third start holds
nothing at it: cuts_held 0.
"""
import os
import shutil
import sys
import tempfile

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import cut_daemon_check as cdc  # noqa: E402
import test_cut_daemon_check as t  # noqa: E402

records, read, dev = t.fixture('life')
cs, at = t.daemon_cuts_of(records, read, dev)
text, marks = t.request_log(records, read, dev, at, hashes=True)
text = text.replace(f'\t{t.PID}\tstart\t0\t-\t-\t-\t64\n',
                    f'\t{t.PID}\tstart\t0\t-\t-\t-\t64\n1015\t{t.PID}\tcuts\t-\t-\t-\t-\tbegun\n', 1)
K = len(cs) // 2
text, ns2 = t.split_begun(text, K)
lines = text.splitlines()
ci = [i for i, ln in enumerate(lines) if ln.split('\t')[1:3] == [t.PID2, 'cut']][1]
ns = int(lines[ci].split('\t')[0])
pid3 = '4444'
third = [f'{ns + 1}\t{pid3}\tstart\t0\t-\t-\t-\t64', f'{ns + 2}\t{pid3}\tcuts\t-\t-\t-\t-\ttaken_up']
third += ['\t'.join([f[0], pid3] + f[2:]) for f in (ln.split('\t') for ln in lines[ci + 1:])]
inst = cdc.parse_log('\n'.join(lines[:ci + 1] + third) + '\n')
recs, dcuts = cdc.records_of(cdc.log_lines(inst, {t.PID2: False, pid3: True}, {t.PID2}), cdc.read_marks(marks))
cdc.epochs(dcuts, recs)    # as main does before the instances' lines: the check's numbers go on across states
print(f'the cuts before the third start: the state\'s own numbers {[d.get("local", d["number"]) for d in dcuts[K:K + 2]]}, '
      f'the check\'s numbers {[d["number"] for d in dcuts[K:K + 2]]}')
good = 'cuts 0\ncut_state begun\ncuts_held 0\nintent_state taken_up\nintent_regions 128\nintent_named_at_start 3\n'


def judge(name, held, log):
    d = tempfile.mkdtemp(prefix='rv-s2-')
    try:
        third_status = f'cuts 2\ncut_state taken_up\ncuts_held {held}\nintent_state taken_up\n'
        for k, st in enumerate((t.START_STATUS, good, third_status)):
            with open(os.path.join(d, f'status-start-{k + 1}.txt'), 'w') as f:
                f.write(st)
        with open(os.path.join(d, 'holder.log'), 'w') as f:
            f.write(log)
        got = []
        rc = cdc.instance_lines(lambda k, v, x: got.append((k, v, x)), d, inst, dcuts, recs, [True, True, False])
        print(f'{name}: instance_lines {rc}; instance_3 {got[2][1]}: {got[2][2][:230]}')
    finally:
        shutil.rmtree(d)


first = ''.join(f'{1020 + 2 * n}\t> a{n} release {n}\n{1021 + 2 * n}\t< a{n} ok (0 lines)\n' for n in range(1, K + 1))
second = (f'{ns2 + 5}\t> b1 release 1\n{ns2 + 6}\t< b1 ok (0 lines)\n'
          f'{ns - 2}\t> b2 release 2\n{ns - 1}\t< b2 ok (0 lines)\n')
judge('A  the new state\'s cuts 1 and 2 released before the third start, cuts_held 0 (RIGHT)', 0, first + second)
judge('B  the new state\'s cut 2 not released, cuts_held 1 (RIGHT)', 1, first + second.split(f'{ns - 2}\t')[0])
judge('C  the new state\'s cut 2 not released, cuts_held 0 (WRONG)', 0, first + second.split(f'{ns - 2}\t')[0])
