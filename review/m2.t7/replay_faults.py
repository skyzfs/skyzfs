#!/usr/bin/env python3
"""The next look at #139 (c7cc03f): the two daemon faults that B2 and B3
were about, put through publish_check.main on the kernel run's own
recordings (run 45539186244), with node B stood in for.

A kernel cannot run here, so the daemon is not planted. Instead each
case takes the course's files as the run left them (the request log, the
answers, rewind-row.txt or tw-row.txt and the statuses) and changes only
what the faulty daemon would have changed in object storage, which node
B then reports:
- prewind, "opens as itself" removed from carried_by: the publish goes
  on the first committed cut past C and G, the rewind-only cut 22 of
  group 32 that rewind-row.txt names (the premise's cut);
- prewind, the same fault where an earlier rewind-only cut (21, group
  31) had been committed first;
- pkill, withdraw answers `withdrawn` and leaves tw queued: tw goes on
  cut 19 of group 43, the commit tw-row.txt names; the status as read,
  and as such a daemon would post it.
The manifests are replaced by the stand-in's (the replay is about the
rows, the cuts and the premises, not the bytes).

Usage: replay_faults.py WORKTREE ARTIFACT_DIR   (python3 -I)
"""
import contextlib
import hashlib
import io
import os
import shutil
import sys
import tempfile
from unittest import mock

WT, ART = sys.argv[1], sys.argv[2]
sys.path.insert(0, os.path.join(WT, 'tests/kernel'))
import nodeb_check  # noqa: E402
import publish_check as pc  # noqa: E402

ULID = '01ARZ3NDEKTSV4RRFFQ69G5FAV'


def manifest_of(t):
    return f'{hashlib.sha256(("file of " + t).encode()).hexdigest()}  {t}/file\n'.encode()


def fake_nodeb(rows):
    """node B finding ROWS: T -> (number, group, commit, state)."""
    class Fake:
        def __init__(self, rdir, logdev):
            self.dir = os.path.join(rdir, 'nodeb')
            os.makedirs(self.dir, exist_ok=True)

        def tool_run(self, args, timeout=600):
            t = args[args.index('--snapshot') + 1]
            out = args[args.index('--out') + 1] if '--out' in args else None
            if t not in rows:
                return 0, 'ref\tnone\nsnaprow\tnone\n'
            n, g, c, s = rows[t]
            m = manifest_of(t)
            text = f'ref\t{c}\t3\nsnaprow\t{s}\t{n}\t{g}\t{c}\t{ULID}\t{len(m)}\t{hashlib.sha256(m).hexdigest()}\n'
            if out:
                with open(out, 'wb') as f:
                    f.write(m)
                text += f'manifest\t{len(m)}\t{hashlib.sha256(m).hexdigest()}\tmatches\n'
            return 0, text

        def verify(self, volume, ref, snap, want, damaged):
            seen = nodeb_check.Seen()
            right = nodeb_check.read_sums(manifest_of(snap).decode())
            for st in nodeb_check.STAGES:
                seen.put(st, not (st == 'files' and want != right), f'{st} of {ref}')
            return seen
    return Fake


def run(course, rows, change=None):
    src = os.path.join(ART, course)
    d = tempfile.mkdtemp(prefix='replay-')
    try:
        for fn in os.listdir(src):
            p = os.path.join(src, fn)
            if os.path.isfile(p) and fn != 'results.tsv':
                shutil.copy(p, d)
        with open(os.path.join(d, 'published.tsv')) as f:
            pub = [ln.split('\t') for ln in f.read().splitlines() if ln]
        with open(os.path.join(d, 'published.tsv'), 'w') as f:
            for r in pub:
                m = os.path.join(d, f'stand-in-manifest-{r[0]}')
                with open(m, 'wb') as g:
                    g.write(manifest_of(r[0]))
                r[5] = m
                f.write('\t'.join(r) + '\n')
        if change:
            change(d)
        argv = ['publish_check.py', course, d, os.path.join(d, 'requests.tsv'), '-', '1048576']
        out = io.StringIO()
        with mock.patch.object(nodeb_check, 'NodeB', fake_nodeb(rows)), mock.patch.object(sys, 'argv', argv), \
                contextlib.redirect_stdout(out):
            try:
                pc.main()
                rc = None
            except SystemExit as e:
                rc = e.code
        with open(os.path.join(d, 'results.tsv')) as f:
            res = [ln.split('\t') for ln in f.read().splitlines()]
        return rc, res
    finally:
        shutil.rmtree(d)


def done_of(course, label):
    with open(os.path.join(ART, course, 'ends.tsv')) as f:
        for ln in f.read().splitlines():
            k, _, v = ln.partition('\t')
            if k == label:
                return pc.done_commit(v)
    raise SystemExit(f'no {label} in {course}/ends.tsv')


def state_of(course, t):
    with open(os.path.join(ART, course, 'published.tsv')) as f:
        for ln in f.read().splitlines():
            r = ln.split('\t')
            if r[0] == t:
                return r[3]
    raise SystemExit(f'no {t} in {course}/published.tsv')


def show(title, rc, res, keys):
    print(f'== {title}: exit {rc}')
    for r in res:
        if r[0] in keys or r[1] not in ('ok', 'INFO'):
            print('   ' + '\t'.join(r)[:260])


def main():
    r1c, r1s = done_of('prewind', 'r1'), state_of('prewind', 'r1')
    keys = ('pin_r1_row', 'pin_r1_cut', 'premise_rewind_cut_committed', 'rig_error')
    show('prewind as run (r1 on cut 27 of group 45)', *run('prewind', {'r1': (27, 45, r1c, r1s)}), keys)
    show('prewind, no "opens as itself": r1 on the committed rewind cut 22 of group 32',
         *run('prewind', {'r1': (22, 32, r1c, r1s)}), keys)
    show('prewind, the same fault on an earlier rewind cut, 21 of group 31',
         *run('prewind', {'r1': (21, 31, r1c, r1s)}), keys)
    tkc, tks = done_of('pkill', 'tk-end'), state_of('pkill', 'tk')
    keys = ('pin_tw_absent', 'premise_tw_committed_past_g', 'pin_tw_withdrawn_status', 'rig_error')
    show('pkill as run (tw absent)', *run('pkill', {'tk': (32, 76, tkc, tks)}), keys)
    tw = {'tk': (32, 76, tkc, tks), 'tw': (19, 43, 'c' * 64, tks)}
    show('pkill, withdraw leaves tw queued: tw on cut 19 of group 43, the status as read', *run('pkill', tw), keys)

    def queued_status(d):
        p = os.path.join(d, 'tw-status.txt')
        with open(p) as f:
            text = f.read()
        text = text.replace('publish_withdrawn 1\n', 'publish_withdrawn 0\n').replace(
            'publish_published 0\n', 'publish_published 1\n')
        with open(p, 'w') as f:
            f.write(text)
    show('pkill, the same, with the status such a daemon posts', *run('pkill', tw, queued_status), keys)


if __name__ == '__main__':
    main()
