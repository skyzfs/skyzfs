#!/usr/bin/env python3
"""#111 at 8210332: what the check says of a daemon killed around a cut's
barrier, on the fixture `life`'s log split in two instances at its 13th cut.

    kill_windows.py CHECKOUT

Three logs. In each the first instance received the flush that carries
cut 13 and reached its barrier, and the second took the device over:

  A  (the pull request's own test, split_log(lose_line=True)) the first
     instance's log ends with the `barrier` line; the state had counted
     cut 13; the second instance's first cut line carries 14.
  B  the first instance's log ends with the `barrier` line; the state had
     not counted the cut (kill_between.out: the store tells the request
     log of the barrier and of the cut before it stores anything in the
     state); the kernel hands the flush to the second instance again,
     which cuts the mark the state kept: `recv`, `barrier`, `cut` 13.
  C  as B, but the first instance wrote the `cut` line 13 as well before
     it died, still before any store in the state: the log holds the
     line twice, once from each instance.

The check runs as cut-record.sh runs it (through test_cut_daemon_main's
run_in), as the recording takeover, with the holder's files of a right
daemon and the second start's status saying what the state had counted."""
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
    first = lines[:ci + 1] if how == 'C' else lines[:ci]
    rest = lines[ci - 2:]
    ns = int(first[-1].split('\t')[0])

    def moved(ln, at=None):
        f = ln.split('\t')
        f[1] = t.PID2
        if at is not None:
            f[0] = str(at)
        return '\t'.join(f)
    second = [f'{ns + 1}\t{t.PID2}\tstart\t0\t-\t-\t-\t64', f'{ns + 2}\t{t.PID2}\tcuts\t-\t-\t-\t-\ttaken_up']
    second += [moved(rest[0], ns + 3)] + [moved(ln) for ln in rest[1:]]
    return '\n'.join(first + second) + '\n'


def run(how):
    records, read, dev = t.fixture('life')
    cs, at = t.daemon_cuts_of(records, read, dev)
    text, marks = t.request_log(records, read, dev, at, hashes=True)
    text = text.replace(f'\t{t.PID}\tstart\t0\t-\t-\t-\t64\n',
                        f'\t{t.PID}\tstart\t0\t-\t-\t-\t64\n1015\t{t.PID}\tcuts\t-\t-\t-\t-\tbegun\n', 1)
    d = tempfile.mkdtemp(prefix='rv-kill-')
    try:
        written, blocks, final = t.held_images(records, cs, cut_ref.opens(records, read, dev, cs))
        if how == 'A':
            text, counted = t.split_log(text, K, lose_line=True), K
        else:
            text, counted = split(text, K, how), K - 1
        with open(os.path.join(d, 'instances.tsv'), 'w') as f:
            f.write(f'{t.PID2}\ttaken-over\n')
        starts = (t.START_STATUS, f'cuts {counted}\ncut_state taken_up\nintent_state taken_up\n')
        t.holder_files(d, written, blocks, final, starts=starts)
        status = m.held_status(records, read, dev, cs).replace('cut_state begun', 'cut_state taken_up')
        rc, lines = m.run_in(d, text, marks, records, read, dev, cs, status, 'life', None, 'takeover')
    finally:
        shutil.rmtree(d, ignore_errors=True)
    print(f'{how}: exit {rc}')
    for key in ('rig_error', 'cuts', 'cut_numbers', 'instance_2'):
        if key in lines:
            print(f'   {key}: {lines[key][0]}: {lines[key][1][:260]}')


for how in 'ABC':
    run(how)
