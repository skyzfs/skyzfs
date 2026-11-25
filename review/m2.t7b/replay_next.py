#!/usr/bin/env python3
"""Next look at #141 at b76ab97: the head's check (publish_check.main) on
the publish rig's own recordings of run 45569275289 (ubuntu-26.04, the
pull request's run at b76ab97), with node B stood in for as in
replay141.py. Each case changes only what a faulty daemon would have
changed (its answers, its request log's lines, what it put in object
storage), or, for the rig's-error cases, what a wrong course would have
left (the agent's record, the cut lines, the damage file).

Usage: python3 -I replay_next.py WORKTREE ARTIFACT_DIR [OLD_ARTIFACT_DIR]

OLD_ARTIFACT_DIR: run 45562017962's (e0c51f8) recordings, put through
the new check (prewindf there stops at the refused -F import).
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
OLD = sys.argv[3] if len(sys.argv) > 3 else None
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


def run(art, course, rows, *changes):
    src = os.path.join(art, course)
    d = tempfile.mkdtemp(prefix='replaynext-')
    try:
        for fn in os.listdir(src):
            p = os.path.join(src, fn)
            if os.path.isfile(p) and fn not in ('results.tsv', 'watch.img'):
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


def end_of(art, course, label):
    with open(os.path.join(art, course, 'ends.tsv')) as f:
        for ln in f.read().splitlines():
            k, _, v = ln.partition('\t')
            if k == label:
                return v
    raise SystemExit(f'no {label} in {course}/ends.tsv')


def state_of(art, course, t):
    with open(os.path.join(art, course, 'published.tsv')) as f:
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


def edit(fn, fun):
    """FN's lines through FUN (lines -> lines)."""
    def change(d):
        p = os.path.join(d, fn)
        with open(p, errors='replace') as f:
            lines = f.read().split('\n')
        tail = lines[-1] == ''
        if tail:
            lines.pop()
        lines = fun(lines)
        with open(p, 'w') as f:
            f.write('\n'.join(lines) + ('\n' if tail else ''))
    return change


def one(lines, pred):
    hit = [i for i, ln in enumerate(lines) if pred(ln.split('\t'))]
    assert len(hit) == 1, len(hit)
    return hit[0]


def log_result(op, old, new):
    """The request log's `publish OP` line whose result is OLD now ends in NEW."""
    def fun(lines):
        i = one(lines, lambda f: len(f) == 8 and f[2] == 'publish' and f[4] == op and f[7] == old)
        f = lines[i].split('\t')
        f[7] = new
        lines[i] = '\t'.join(f)
        return lines
    return edit('requests.tsv', fun)


def log_drop(pred):
    def fun(lines):
        i = one(lines, pred)
        return lines[:i] + lines[i + 1:]
    return edit('requests.tsv', fun)


def is_publish(op, result):
    return lambda f: len(f) == 8 and f[2] == 'publish' and f[4] == op and f[7] == result


def is_cut(number):
    return lambda f: len(f) == 8 and f[2] == 'cut' and f[6] == str(number)


def log_move_before(pred, number):
    """The line PRED moved to just before cut NUMBER, with a time 1 ns before it."""
    def fun(lines):
        i = one(lines, pred)
        moved = lines.pop(i).split('\t')
        j = one(lines, is_cut(number))
        moved[0] = str(int(lines[j].split('\t')[0]) - 1)
        lines.insert(j, '\t'.join(moved))
        return lines
    return edit('requests.tsv', fun)


def show(title, want, rc, res):
    flag = 'as it should' if rc == want else f'NOT {want}'
    print(f'== {title}: exit {rc} ({flag})')
    for r in res:
        if r[1] not in ('ok', 'INFO', 'CAUGHT', 'CAUGHT_AT_FILES'):
            print('   ' + '\t'.join(r)[:260])
    return rc == want


def main():
    ok = []
    # prewindf: f1 C 10, G 1000021; export cuts 13-16 (groups 23, 24, 25,
    # 34); -F refused, -FX imported; cut 17 of group 34 (equal to the
    # top) is the ground; `publish ended f1 refused rewound` after cut 19.
    pf = 'prewindf'
    ended = is_publish('ended', 'f1 refused rewound')
    ok.append(show('prewindf as run', 0, *run(ART, pf, {})))
    ok.append(show('prewindf, a daemon that never ends f1 (no ended line; f1-end pending): rule (R) not at the '
                   'equal group, or not at all', 1,
                   *run(ART, pf, {}, answer('f1-end', 'pending after 300 s'), log_drop(ended))))
    ok.append(show('prewindf, f1 ended expired', 1,
                   *run(ART, pf, {}, answer('f1-end', 'expired'), log_result('ended', 'f1 refused rewound',
                                                                             'f1 expired'))))
    ok.append(show('prewindf, f1 ended refused store', 1,
                   *run(ART, pf, {}, answer('f1-end', 'refused store x'),
                        log_result('ended', 'f1 refused rewound', 'f1 refused store'))))
    ok.append(show('prewindf, the daemon ends f1 (answer right) and writes no ended line', 1,
                   *run(ART, pf, {}, log_drop(ended))))
    ok.append(show('prewindf, f1 ended rewound before the rewind cut (at the export, before cut 16)', 1,
                   *run(ART, pf, {}, log_move_before(ended, 16))))
    ok.append(show('prewindf, f1 ended rewound just before cut 17 (the ground)', 1,
                   *run(ART, pf, {}, log_move_before(ended, 17))))
    c = 'ab' * 32
    ok.append(show('prewindf, f1 published on cut 18 (group 35) and done', 1,
                   *run(ART, pf, {'f1': (18, 35, c, state_of(ART, pf, 'f1'))}, answer('f1-end', 'done commit=' + c),
                        log_result('ended', 'f1 refused rewound', 'f1 done commit=' + c))))
    # The rig's error: a course that did not make the case.
    # f1 was sent at 336.023 s, cut 17 counted at 340.414 s: 4.39 s later.
    ok.append(show("prewindf, f1's expiry 5 s in agent.tsv (R41; the ground came 4.39 s after the send, "
                   "within it)", 0,
                   *run(ART, pf, {}, edit('agent.tsv', lambda ls: [ln.replace('--expiry-s 900', '--expiry-s 5')
                                                                    for ln in ls]))))
    ok.append(show("prewindf, f1's expiry 4 s in agent.tsv (f1 expired before the ground)", 2,
                   *run(ART, pf, {}, edit('agent.tsv', lambda ls: [ln.replace('--expiry-s 900', '--expiry-s 4')
                                                                    for ln in ls]))))
    ok.append(show("prewindf, no --expiry-s in agent.tsv (the default, 600 s)", 0,
                   *run(ART, pf, {}, edit('agent.tsv', lambda ls: [ln.replace(' --expiry-s 900', '')
                                                                    for ln in ls]))))
    ok.append(show('prewindf, no cut not above the groups before it (cut 17 dropped)', 2,
                   *run(ART, pf, {}, log_drop(is_cut(17)))))
    ok.append(show("prewindf, f1's C after the ground (published.tsv C = 17)", 2,
                   *run(ART, pf, {}, edit('published.tsv', lambda ls: ['\t'.join(f[:4] + ['17'] + f[5:])
                                                                        for f in (ln.split('\t') for ln in ls)]))))
    ok.append(show('prewindf, rewind-f.txt without an import line', 2,
                   *run(ART, pf, {}, edit('rewind-f.txt', lambda ls: [ln for ln in ls
                                                                       if not ln.startswith('imported with')]))))
    ok.append(show('prewindf, agent.tsv without the publish of f1', 2,
                   *run(ART, pf, {}, edit('agent.tsv', lambda ls: [ln for ln in ls if '\tpublish --name f1 ' not in ln]))))
    ok.append(show('prewindf, agent.tsv with an expiry that is not a number', 2,
                   *run(ART, pf, {}, edit('agent.tsv', lambda ls: [ln.replace('--expiry-s 900', '--expiry-s 9x0')
                                                                    for ln in ls]))))
    ok.append(show('prewindf, a cut line that does not read', 2,
                   *run(ART, pf, {}, edit('requests.tsv', lambda ls: [
                       '\t'.join(f[:7] + ['3x']) if len(f) == 8 and f[2] == 'cut' and f[6] == '17' else '\t'.join(f)
                       for f in (ln.split('\t') for ln in ls)]))))

    # prewind: r1 C 20, G 31; the -T import's cuts 19-22 (groups 27, 29,
    # 31, 32; rewind), top 43; `publish asked 31 20 r1 refused rewound`
    # after cut 22; r1 asked anew with C 27, made by cut 28 of group 46.
    pw = 'prewind'
    r1c, r1s = pc.done_commit(end_of(ART, pw, 'r1-end')), state_of(ART, pw, 'r1')
    good = {'r1': (28, 46, r1c, r1s)}
    asked = is_publish('asked', 'r1 refused rewound')
    ok.append(show('prewind as run', 0, *run(ART, pw, good)))
    ok.append(show('prewind, a daemon without rule (R) at the ask (r1 accepted, pending; its log line)', 1,
                   *run(ART, pw, good, answer('r1', 'pending after 300 s'),
                        log_result('asked', 'r1 refused rewound', 'r1 accepted'))))
    ok.append(show('prewind, the same daemon also published r1 on rewind-only cut 23 once the store came back', 1,
                   *run(ART, pw, {'r1': (23, 33, r1c, r1s)}, answer('r1', 'pending after 300 s'),
                        log_result('asked', 'r1 refused rewound', 'r1 accepted'))))
    ok.append(show('prewind, r1 refused with another code (refused asked)', 1,
                   *run(ART, pw, good, answer('r1', 'refused asked other parameters'),
                        log_result('asked', 'r1 refused rewound', 'r1 refused asked'))))
    for n, g in ((23, 33), (22, 32), (27, 43)):
        ok.append(show(f'prewind, r1 made by cut {n} of group {g}', 1, *run(ART, pw, {'r1': (n, g, r1c, r1s)})))
    ok.append(show('prewind, r1-look answered absent with the store down', 1,
                   *run(ART, pw, good, answer('r1-look', 'absent'))))
    # The premise then anchors on the next `asked` line of r1, the ask
    # anew's (C 27), after cuts 21 to 26: it holds (N3 of the next look).
    ok.append(show("prewind, the daemon's `asked` line of r1's first ask absent (its answer right)", 2,
                   *run(ART, pw, good, log_drop(asked))))
    ok.append(show('prewind, rewind-top.txt not a number', 2,
                   *run(ART, pw, good, edit('rewind-top.txt', lambda ls: ['4x3']))))
    ok.append(show('prewind, the ask before any ground (the asked line moved before cut 19)', 2,
                   *run(ART, pw, good, log_move_before(asked, 19))))

    # pkill: tw withdrawn by instance 14069, then cuts of group 43 or later.
    pk = 'pkill'
    tkc, tks = pc.done_commit(end_of(ART, pk, 'tk-end')), state_of(ART, pk, 'tk')
    rows = {'tk': (32, 76, tkc, tks)}
    ok.append(show('pkill as run', 0, *run(ART, pk, rows)))
    ok.append(show('pkill, withdraw answered pending (and logged so; no ended line)', 1,
                   *run(ART, pk, rows, answer('tw-withdraw', 'pending'),
                        log_result('withdraw', 'tw withdrawn', 'tw pending'),
                        log_drop(is_publish('ended', 'tw withdrawn')))))
    ok.append(show("pkill, the daemon's withdraw line absent", 2,
                   *run(ART, pk, rows, log_drop(is_publish('withdraw', 'tw withdrawn')))))

    # pplain: unchanged courses, the new check.
    pl = {t: (n, g, pc.done_commit(end_of(ART, 'pplain', lbl)), state_of(ART, 'pplain', t))
          for t, lbl, n, g in (('t1', 't1', 10, 18), ('t2', 't2', 12, 20), ('t4', 't4', 23, 43),
                               ('t7', 't7-end', 28, 50))}
    m = manifest_of('t1')

    def stand_in_look(text):
        f = dict(w.split('=', 1) for w in text.split()[1:])
        f['manifest-len'], f['manifest-hash'] = str(len(m)), hashlib.sha256(m).hexdigest()
        return 'snap ' + ' '.join(f'{k}={v}' for k, v in f.items())
    base = stand_in_look(end_of(ART, 'pplain', 't1-look'))
    ok.append(show("pplain as run (t1-look with the stand-in's manifest)", 0,
                   *run(ART, 'pplain', pl, answer('t1-look', base))))
    ok.append(show('pplain, t1-look with number=11', 1,
                   *run(ART, 'pplain', pl, answer('t1-look', base.replace('number=10', 'number=11')))))

    if OLD:
        ok.append(show("e0c51f8's prewindf recording (stopped at the refused -F) through the new check", 2,
                       *run(OLD, pf, {})))
        oc, os_ = pc.done_commit(end_of(OLD, pw, 'r1-end')), state_of(OLD, pw, 'r1')
        ok.append(show("e0c51f8's prewind recording through the new check (r1 on cut 28 of group 45)", 0,
                       *run(OLD, pw, {'r1': (28, 45, oc, os_)})))
    print(f'\n{sum(ok)} of {len(ok)} cases ended as they should')


if __name__ == '__main__':
    main()
