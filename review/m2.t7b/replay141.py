#!/usr/bin/env python3
"""Review of #141 at e0c51f8: the head's check (publish_check.main) on
the publish rig's own recordings of run 45562017962 (ubuntu-26.04),
with node B stood in for, as the next look at #139 did
(review/m2.t7/replay_faults.py): each case changes only what a faulty
daemon would have changed (its answers, its request log's lines, what
it put in object storage). The manifests are the stand-in's.

Usage: python3 -I replay141.py WORKTREE ARTIFACT_DIR
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


def run(course, rows, *changes):
    src = os.path.join(ART, course)
    d = tempfile.mkdtemp(prefix='replay141-')
    try:
        for fn in os.listdir(src):
            p = os.path.join(src, fn)
            if os.path.isfile(p) and fn != 'results.tsv':
                shutil.copy(p, d)
        if os.path.exists(os.path.join(d, 'published.tsv')):
            with open(os.path.join(d, 'published.tsv')) as f:
                pub = [ln.split('\t') for ln in f.read().splitlines() if ln]
            with open(os.path.join(d, 'published.tsv'), 'w') as f:
                for r in pub:
                    m = os.path.join(d, f'stand-in-manifest-{r[0]}')
                    with open(m, 'wb') as g:
                        g.write(manifest_of(r[0]))
                    r[5] = m
                    f.write('\t'.join(r) + '\n')
        for change in changes:
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


def end_of(course, label):
    with open(os.path.join(ART, course, 'ends.tsv')) as f:
        for ln in f.read().splitlines():
            k, _, v = ln.partition('\t')
            if k == label:
                return v
    raise SystemExit(f'no {label} in {course}/ends.tsv')


def state_of(course, t):
    with open(os.path.join(ART, course, 'published.tsv')) as f:
        for ln in f.read().splitlines():
            r = ln.split('\t')
            if r[0] == t:
                return r[3]
    raise SystemExit(f'no {t} in {course}/published.tsv')


def answer(label, text):
    def change(d):
        p = os.path.join(d, 'ends.tsv')
        with open(p) as f:
            rows = [ln.split('\t', 1) for ln in f.read().splitlines() if ln]
        with open(p, 'w') as f:
            f.write(''.join(f'{k}\t{text if k == label else v}\n' for k, v in rows))
    return change


def log_line(old, new):
    """The request log's line ending in OLD (a publish result) now ends in NEW."""
    def change(d):
        p = os.path.join(d, 'requests.tsv')
        with open(p, errors='replace') as f:
            lines = f.read().split('\n')
        hit = [i for i, ln in enumerate(lines) if ln.endswith('\t' + old)]
        assert len(hit) == 1, (old, len(hit))
        lines[hit[0]] = lines[hit[0]][:-len(old)] + new
        with open(p, 'w') as f:
            f.write('\n'.join(lines))
    return change


def show(title, rc, res):
    print(f'== {title}: exit {rc}')
    for r in res:
        if r[1] not in ('ok', 'INFO', 'CAUGHT', 'CAUGHT_AT_FILES'):
            print('   ' + '\t'.join(r)[:240])


def main():
    r1c, r1s = pc.done_commit(end_of('prewind', 'r1-end')), state_of('prewind', 'r1')
    show('prewind as run (r1 on cut 28 of group 45)', *run('prewind', {'r1': (28, 45, r1c, r1s)}))
    # A daemon without rule (R) at the ask (M2.5): r1 accepted with the
    # store down and left pending past the flow's limit; its log line
    # is `r1 accepted`.
    show('prewind, no rule (R) at the ask (answer and log line as such a daemon writes them)',
         *run('prewind', {'r1': (28, 45, r1c, r1s)}, answer('r1', 'pending after 300 s'),
              log_line('r1 refused rewound', 'r1 accepted')))
    show('prewind, the same, the log line left as the right daemon wrote it (the PR\'s test)',
         *run('prewind', {'r1': (28, 45, r1c, r1s)}, answer('r1', 'pending after 300 s')))
    # #139's D1: r1 made by a rewind-only cut, the row's cut 23 and an
    # earlier one (22), and by the top's cut 27 (opens as itself).
    for n, g in ((23, 32), (22, 31), (27, 42)):
        show(f'prewind, r1 on cut {n} of group {g}', *run('prewind', {'r1': (n, g, r1c, r1s)}))
    show('prewind, r1-look answered absent with the store down',
         *run('prewind', {'r1': (28, 45, r1c, r1s)}, answer('r1-look', 'absent')))

    pl = {t: (n, g, pc.done_commit(end_of('pplain', lbl)), state_of('pplain', t))
          for t, lbl, n, g in (('t1', 't1', 8, 16), ('t2', 't2', 11, 19), ('t4', 't4', 22, 42),
                               ('t7', 't7-end', 28, 50))}
    look = end_of('pplain', 't1-look')
    # The stand-in's manifest differs from the run's: the look is
    # rewritten to the stand-in's length and hash so that only the
    # field planted differs.
    m = manifest_of('t1')

    def stand_in_look(text):
        f = dict(w.split('=', 1) for w in text.split()[1:])
        f['manifest-len'], f['manifest-hash'] = str(len(m)), hashlib.sha256(m).hexdigest()
        return 'snap ' + ' '.join(f'{k}={v}' for k, v in f.items())
    base = stand_in_look(look)
    show('pplain as run (t1-look the stand-in\'s manifest)', *run('pplain', pl, answer('t1-look', base)))
    for old, new in (('number=8', 'number=9'), ('group=16', 'group=17'), (f'state={pl["t1"][3]}', 'state=' + 'f' * 32)):
        show(f'pplain, t1-look with {new}', *run('pplain', pl, answer('t1-look', base.replace(old, new))))
    show('pplain, t5-look finds a row', *run('pplain', pl, answer('t1-look', base), answer('t5-look', base)))


if __name__ == '__main__':
    main()
