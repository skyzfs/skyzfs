#!/usr/bin/env python3
"""#113 at e88773b, the next look: a kill that cuts short the line of a
cut, or of its barrier. The logs are those of the stage 2b review's
kill_windows.py (review/m2.t3-2b: the fixture `life` split in two
instances at its 13th cut, the second instance a right daemon's by
design 5.2, which the store's own test showed there), with the first
instance's last line cut short and the second instance's start line
following it in the same line, as a kill leaves it:

  C whole  the first instance wrote the `barrier` line and the `cut`
           line 13 and was killed before the state stored the count; the
           second instance gets the flush again and cuts it as 13
           (kill_windows.py's C).
  C cut    the same, the `cut` line cut short within its last column.
  B before the first instance was killed after the flush's `recv` line
           and before its `barrier` line; the second instance gets the
           flush again: recv, barrier, cut 13. Nothing is cut short.
  B cut    the same, killed while it wrote the `barrier` line, which is
           cut short within its tag.

The fixture is not the recording `takeover`: the control ctl_mark-kept
DIFFERS on it with or without a line cut short (it is the exit 1 of the
two B logs, whose other lines all pass), and is no part of the question.

    next_torn_cut.py CHECKOUT
"""
import os
import shutil
import sys
import tempfile

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import cut_ref  # noqa: E402
import test_cut_daemon_check as t  # noqa: E402
import test_cut_daemon_main as m  # noqa: E402

K = 13


def split(text, k, how):
    lines = text.splitlines()
    ci = [i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'cut'][k - 1]
    assert [lines[ci - 2].split('\t')[2], lines[ci - 1].split('\t')[2]] == ['recv', 'barrier']
    first = lines[:ci + 1] if how.startswith('C') else lines[:ci]
    rest = lines[ci - 2:]
    ns = int(first[-1].split('\t')[0])

    def moved(ln, at=None):
        f = ln.split('\t')
        f[1] = t.PID2
        if at is not None:
            f[0] = str(at)
        return '\t'.join(f)
    # The times of the fixture have four digits; the second instance's
    # lines keep the number of digits, as the judge takes the start's.
    second = [f'{ns + 1}\t{t.PID2}\tstart\t0\t-\t-\t-\t64', f'{ns + 2}\t{t.PID2}\tcuts\t-\t-\t-\t-\ttaken_up']
    second += [moved(rest[0], ns + 3)] + [moved(ln) for ln in rest[1:]]
    if how == 'C cut':
        said = f'the first instance\'s last line {first[-1]!r} cut to {first[-1][:-1]!r}'
        first, second = first[:-1], [first[-1][:-1] + second[0]] + second[1:]
    elif how == 'B cut':
        f = first[-1].split('\t')
        assert f[2] == 'barrier' and f[3].isdigit()
        left = '\t'.join(f[:4])
        said = f'the first instance\'s last line {first[-1]!r} cut to {left!r}'
        first, second = first[:-1], [left + second[0]] + second[1:]
    elif how == 'B before':
        said = f'the first instance\'s log ends before its barrier line {first[-1]!r}, nothing cut short'
        first = first[:-1]
    else:
        said = f'the first instance\'s last line {first[-1]!r} whole'
    return '\n'.join(first + second) + '\n', said


def run(how):
    records, read, dev = t.fixture('life')
    cs, at = t.daemon_cuts_of(records, read, dev)
    text, marks = t.request_log(records, read, dev, at, hashes=True)
    text = text.replace(f'\t{t.PID}\tstart\t0\t-\t-\t-\t64\n',
                        f'\t{t.PID}\tstart\t0\t-\t-\t-\t64\n1015\t{t.PID}\tcuts\t-\t-\t-\t-\tbegun\n', 1)
    d = tempfile.mkdtemp(prefix='rv-torn-cut-')
    try:
        written, blocks, final = t.held_images(records, cs, cut_ref.opens(records, read, dev, cs))
        text, said = split(text, K, how)
        with open(os.path.join(d, 'instances.tsv'), 'w') as f:
            f.write(f'{t.PID2}\ttaken-over\n')
        starts = (t.START_STATUS, f'cuts {K - 1}\ncut_state taken_up\nintent_state taken_up\n')
        t.holder_files(d, written, blocks, final, starts=starts)
        status = m.held_status(records, read, dev, cs).replace('cut_state begun', 'cut_state taken_up')
        rc, lines = m.run_in(d, text, marks, records, read, dev, cs, status, 'life', None, 'takeover')
    finally:
        shutil.rmtree(d, ignore_errors=True)
    print(f'{how}: {said}: exit {rc}')
    for key in lines:
        if key in ('rig_error', 'obs_torn_at_kill', 'cuts', 'cut_numbers', 'instance_2') \
                or lines[key][0] in ('FAIL', 'ERROR') or 'DIFFERS' in lines[key][1]:
            print(f'   {key}: {lines[key][0]}: {lines[key][1][:260]}')


for how in ('C whole', 'C cut', 'B before', 'B cut'):
    run(how)
