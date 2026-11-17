#!/usr/bin/env python3
"""#113 at ab34565: recordings with a holder's VERDICT, run through a
head's judge. Made from the recording `failed` of a run's artifact (a
right daemon's), on copies.

  early   what `failed` and `takeover` leave when holder_wrote_one ends
          them at a verdict before the first cut: the first instance's
          part of the request log, its line of instances.tsv, the first
          session's marks and groups, a status read after the export
          (the artifact's status-failure.txt, with a time made for it),
          holder.log with the verdict alone, and none of held.tsv,
          held-blocks.tsv, held-image.tsv, held-final.txt,
          status-held.txt, failure.tsv.
  whole   the whole recording with a verdict as the holder's last line.
  wrong   the whole recording with a verdict, and its second instance
          said to have taken the state up (a wrong start as well).
  both    early, with an ERROR line of the holder before the verdict.

    python3 -I next2_verdict.py ROOT ARTIFACT_DIR...

ROOT holds tests/kernel/cut_daemon_check.py.
"""
import os
import shutil
import subprocess
import sys
import tempfile

root, arts = sys.argv[1], sys.argv[2:]
JUDGE = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
VERDICT = ('VERDICT cut 1 is still being uploaded after its write was refused (refused: read block 7 of cut 1: '
           'Input/output error (os error 5))')
HOLDER = ('held.tsv', 'held-blocks.tsv', 'held-image.tsv', 'held-final.txt', 'status-held.txt', 'failure.tsv',
          'status-failure.txt')


def rd(d, name):
    with open(os.path.join(d, name)) as f:
        return f.read()


def wr(d, name, text):
    with open(os.path.join(d, name), 'w') as f:
        f.write(text)


def judge(d, name):
    if os.path.exists(os.path.join(d, 'results.tsv')):
        os.remove(os.path.join(d, 'results.tsv'))
    p = subprocess.run([sys.executable, '-I', '-B', JUDGE, name, d, os.path.join(d, 'requests.tsv'), '-',
                        str(512 << 20)], capture_output=True, text=True, timeout=1500)
    lines = [ln.split('\t') for ln in rd(d, 'results.tsv').splitlines()]
    return p.returncode, lines


def show(title, rc, lines):
    keys = [f[0] for f in lines]
    print(f'  {title}: exit {rc}')
    print(f"      lines: {' '.join(k for k in keys if not k.startswith(('obs_', 'ctl_')))}")
    for f in lines:
        if f[0].startswith('ctl_'):
            if 'DIFFERS' in f[2]:
                print(f'      {f[0]}\t{f[1]}\t{f[2][:260]}')
        elif f[1] not in ('PASS', 'INFO') or f[0] == 'held_reads':
            print(f'      {f[0]}\t{f[1]}\t{f[2][:260]}')


def early(src, d, holder):
    """The copy d cut down to what the recording leaves at an early end."""
    first = rd(src, 'instances.tsv').splitlines()[0]
    pid = first.split('\t')[0]
    log = [ln for ln in rd(src, 'requests.tsv').splitlines() if ln.split('\t')[1] == pid]
    # The artifact's first instance was failed by SIGUSR1 after the status
    # read; an early end stops it with SIGTERM. Its lines from the
    # failure on are left out.
    fail = next((i for i, ln in enumerate(log) if ln.split('\t')[2] == 'ended'), len(log))
    log = log[:fail]
    wr(d, 'requests.tsv', '\n'.join(log) + '\n')
    wr(d, 'instances.tsv', first + '\n')
    wr(d, 'marks.tsv', ''.join(ln + '\n' for ln in rd(src, 'marks.tsv').splitlines() if '\tsession-1-' in ln))
    wr(d, 'status.txt', rd(src, 'status-failure.txt'))
    # The status was read a second after the export and before the
    # failure: the time of the log's last line before it, plus a second.
    at = int(log[-1].split('\t')[0]) + 1_000_000_000
    wr(d, 'status-time.tsv', f'{at}\t{at + 15_000_000}\n')
    for fn in HOLDER + ('txgs-2.txt', 'status-start-2.txt'):
        if os.path.exists(os.path.join(d, fn)):
            os.remove(os.path.join(d, fn))
    for fn in os.listdir(d):
        if fn.startswith(('serve-2', 'serve-refused', 'snap')):
            os.remove(os.path.join(d, fn))
    wr(d, 'holder.log', holder)


for art in arts:
    src = os.path.join(art, 'failed')
    label = os.path.basename(os.path.normpath(art))
    print(f'{label}:')
    t0 = int(rd(src, 'holder.log').split('\t')[0])
    with tempfile.TemporaryDirectory() as tmp:
        def copy(n):
            d = os.path.join(tmp, n)
            shutil.copytree(src, d)
            return d
        for name in ('failed', 'takeover'):
            d = copy(f'early-{name}')
            early(src, d, f'{t0}\t{VERDICT}\n')
            show(f'early, judged as {name}', *judge(d, name))
        d = copy('early-none')
        early(src, d, f'{t0}\t> t1x1 held\n')
        show('early with no verdict in the log (what the script does not leave), judged as failed', *judge(d, 'failed'))
        d = copy('both')
        early(src, d, f'{t0}\tERROR HolderError: x\n{t0 + 1}\t{VERDICT}\n')
        show('both, judged as failed', *judge(d, 'failed'))
        d = copy('whole')
        last = int(rd(d, 'holder.log').splitlines()[-1].split('\t')[0])
        wr(d, 'holder.log', rd(d, 'holder.log') + f'{last + 1}\t{VERDICT}\n')
        show('whole, judged as failed', *judge(d, 'failed'))
        d = copy('wrong')
        wr(d, 'holder.log', rd(d, 'holder.log') + f'{last + 1}\t{VERDICT}\n')
        second = rd(d, 'instances.tsv').splitlines()[1].split('\t')[0]
        out = []
        for ln in rd(d, 'requests.tsv').splitlines():
            f = ln.split('\t')
            if f[1] == second and f[2] == 'cuts' and f[-1] == 'begun':
                f[-1] = 'taken_up'
            out.append('\t'.join(f))
        wr(d, 'requests.tsv', '\n'.join(out) + '\n')
        show('wrong, judged as failed', *judge(d, 'failed'))
