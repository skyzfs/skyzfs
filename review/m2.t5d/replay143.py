#!/usr/bin/env python3
"""Review of #143 at d3b75af: the head's check (publish_check.main) on the
publish rig's own recordings of run 45588605464 (ubuntu-26.04, the pull
request's run, prewindc's first kernel run), node B stood in for (its
rows as the run's results.tsv names them, a stand-in manifest). Each
fault changes only what a faulty daemon would have changed (its
answers, its request log's lines, what it put in object storage); each
wrong or malformed recording only what a wrong course or a broken rig
would have left. The watcher's lines planted are ones cut_watch.py
itself wrote (a stand-in node B whose `get` ends 4, or whose `row`
fails three times), passed as WATCH_VERDICT and WATCH_ERROR.

Usage: python3 -I replay143.py WORKTREE ARTIFACT_DIR WATCH_VERDICT WATCH_ERROR [OLD_ARTIFACT_DIR]

OLD_ARTIFACT_DIR: run 45569275289's (b76ab97) recordings, through the new
check: prewind (N3 on another real log) and pplain.
"""
import contextlib
import hashlib
import io
import os
import re
import shutil
import sys
import tempfile
from unittest import mock

WT, ART, WV, WE = sys.argv[1:5]
OLD = sys.argv[5] if len(sys.argv) > 5 else None
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


def read(art, course, fn):
    with open(os.path.join(art, course, fn), errors='replace') as f:
        return f.read()


def end_of(art, course, label):
    for ln in read(art, course, 'ends.tsv').splitlines():
        k, _, v = ln.partition('\t')
        if k == label:
            return v
    raise SystemExit(f'no {label} in {course}/ends.tsv')


def rows_of(art, course):
    """The rows node B read in the run, from its results.tsv, for each name
    the course published: T -> (number, group, commit, state)."""
    out = {}
    res = read(art, course, 'results.tsv')
    pub = pc.read_published(read(art, course, 'published.tsv')) if pc.PUBLISHED[course] else {}
    for t, label in pc.PUBLISHED[course].items():
        m = re.search(rf'^pin_{t}_row\tok\t.* cut (\d+) of group (\d+) ', res, re.M)
        if m is None:
            raise SystemExit(f'no row of {t} in {course}/results.tsv')
        out[t] = (int(m.group(1)), int(m.group(2)), pc.done_commit(end_of(art, course, label)), pub[t]['state'])
    return out


def stand_in_look(text, t):
    """A look's `snap` answer with the stand-in manifest's length and hash."""
    m = manifest_of(t)
    f = dict(w.split('=', 1) for w in text.split()[1:])
    f['manifest-len'], f['manifest-hash'] = str(len(m)), hashlib.sha256(m).hexdigest()
    return 'snap ' + ' '.join(f'{k}={v}' for k, v in f.items())


def answer(label, text):
    def change(d):
        p = os.path.join(d, 'ends.tsv')
        with open(p) as f:
            rows = [ln.split('\t', 1) for ln in f.read().splitlines() if ln]
        assert label in [k for k, _ in rows], label
        with open(p, 'w') as f:
            f.write(''.join(f'{k}\t{text if k == label else v}\n' for k, v in rows))
    return change


def looks_stood_in(course):
    """The course's looks at published names, with the stand-in manifest."""
    def change(d):
        for t, label in pc.LOOKS.get(course, {}).items():
            with open(os.path.join(d, 'ends.tsv')) as f:
                v = dict(ln.split('\t', 1) for ln in f.read().splitlines() if ln)[label]
            answer(label, stand_in_look(v, t))(d)
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


def drop(fn):
    def change(d):
        os.unlink(os.path.join(d, fn))
    return change


def append(fn, text):
    def change(d):
        with open(os.path.join(d, fn), 'a') as f:
            f.write(text)
    return change


def one(lines, pred):
    hit = [i for i, ln in enumerate(lines) if pred(ln.split('\t'))]
    assert len(hit) == 1, len(hit)
    return hit[0]


def is_publish(op, result):
    return lambda f: len(f) == 8 and f[2] == 'publish' and f[4] == op and f[7] == result


def is_cut(number):
    return lambda f: len(f) == 8 and f[2] == 'cut' and f[6] == str(number)


def log_result(op, old, new):
    def fun(lines):
        i = one(lines, is_publish(op, old))
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


def log_drop_all(pred):
    return edit('requests.tsv', lambda lines: [ln for ln in lines if not pred(ln.split('\t'))])


def log_after_cut(number, line):
    """A publish line (op, result) written 1 ns after cut NUMBER, as the
    instance's own."""
    op, result = line

    def fun(lines):
        j = one(lines, is_cut(number))
        f = lines[j].split('\t')
        lines.insert(j + 1, '\t'.join([str(int(f[0]) + 1), f[1], 'publish', '-', op, '-', '-', result]))
        return lines
    return edit('requests.tsv', fun)


def cut_as(number, group=None, opens=None):
    def fun(lines):
        j = one(lines, is_cut(number))
        f = lines[j].split('\t')
        if group is not None:
            f[7] = str(group)
        if opens is not None:
            f[5] = opens
        lines[j] = '\t'.join(f)
        return lines
    return edit('requests.tsv', fun)


def published_field(t, k, v):
    def fun(lines):
        out = []
        for ln in lines:
            f = ln.split('\t')
            if f[0] == t:
                f[k] = str(v)
            out.append('\t'.join(f))
        return out
    return edit('published.tsv', fun)


def run(art, course, rows, *changes):
    src = os.path.join(art, course)
    with tempfile.TemporaryDirectory(prefix='replay143-') as d:
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


def show(title, want, rc, res, key=None):
    flag = 'as it should' if rc == want else f'NOT {want}'
    print(f'== {title}: exit {rc} ({flag})')
    for r in res:
        if r[1] not in ('ok', 'INFO', 'CAUGHT', 'CAUGHT_AT_FILES') or (key and r[0] == key):
            print('   ' + '\t'.join(r)[:300])
    return rc == want


def main():
    ok = []
    with open(WV) as f:
        verdict_line = f.read()
    with open(WE) as f:
        error_lines = f.read()
    rewound = 'refused rewound a rewind may have been counted after the count asked from'
    prem = 'premise_c1_waited_across_a_plain_import'

    # prewindc: c1 C 10, G 1000021, asked at 528.296 s; the export's cuts
    # 13 to 16 (groups 23, 24, 25, 34); session-2-begin 531.394 s; the
    # import's cuts 17 (36) and 18 (38); session-2-ready 531.504 s; cuts
    # 19 (40) to 74 (205) during the minute of syncs; `withdraw c1
    # withdrawn` and `ended c1 withdrawn` at 591.197 s.
    c = 'prewindc'
    wd, endw = is_publish('withdraw', 'c1 withdrawn'), is_publish('ended', 'c1 withdrawn')
    ok.append(show('prewindc as run', 0, *run(ART, c, {}), key=prem))

    def ended_at(number, how, withdraw_answer):
        """A daemon that ended c1 HOW after cut NUMBER: its end and
        withdrawal answers, its log's lines."""
        return (log_after_cut(number, ('ended', f'c1 {how}')), log_drop(endw),
                log_result('withdraw', 'c1 withdrawn', f'c1 {how}'),
                answer('c1-withdraw', withdraw_answer))
    # The S1 fault: every waiting publish ended `refused rewound` at an
    # import, here at its first cut (17, group 36, above the top 34).
    ok.append(show('prewindc, S1: c1 ended refused rewound at the import (after cut 17)', 1,
                   *run(ART, c, {}, answer('c1-end', rewound), *ended_at(17, 'refused rewound', rewound)), key=prem))
    ok.append(show('prewindc, S1, and the faulty daemon writes the import\'s cuts 17 and 18 as opening only as a rewind',
                   1, *run(ART, c, {}, answer('c1-end', rewound), *ended_at(17, 'refused rewound', rewound),
                           cut_as(17, opens='rewind'), cut_as(18, opens='rewind')), key=prem))
    ok.append(show('prewindc, S1 at the import\'s return (after cut 19), and no `ended` line', 1,
                   *run(ART, c, {}, answer('c1-end', rewound), log_drop(endw),
                        log_result('withdraw', 'c1 withdrawn', 'c1 refused rewound'),
                        answer('c1-withdraw', rewound)), key=prem))
    ok.append(show('prewindc, c1 ended at the export (after cut 16) as expired', 1,
                   *run(ART, c, {}, answer('c1-end', 'expired'), *ended_at(16, 'expired', 'expired')), key=prem))
    ok.append(show('prewindc, c1 ended refused store after cut 25', 1,
                   *run(ART, c, {}, answer('c1-end', 'refused store x'),
                        *ended_at(25, 'refused store', 'refused store x')), key=prem))
    state = pc.read_published(read(ART, c, 'published.tsv'))['c1']['state']
    h = 'cd' * 32
    look = f'snap commit={h} state={state} number=20 group=43 manifest-len=1 manifest-hash={"0" * 64}'
    ok.append(show('prewindc, c1 published on cut 20 (G not honoured): done, its look a snap, node B finds it', 1,
                   *run(ART, c, {'c1': (20, 43, h, state)}, answer('c1-end', 'done commit=' + h),
                        answer('c1-look', look), *ended_at(20, 'done commit=' + h, 'done commit=' + h)), key=prem))
    ok.append(show('prewindc, the withdrawal ignored (pending; no ended line)', 1,
                   *run(ART, c, {}, answer('c1-withdraw', 'pending'), log_drop(endw),
                        log_result('withdraw', 'c1 withdrawn', 'c1 pending')), key=prem))
    ok.append(show('prewindc, the look answers a snap with nothing published', 1,
                   *run(ART, c, {}, answer('c1-look', look)), key=prem))
    ok.append(show("prewindc, the watcher's VERDICT (cut_watch.py's own line)", 1,
                   *run(ART, c, {}, append('watch.log', verdict_line)), key=prem))
    # The rig's error: the course did not make its case, or its files do not read.
    for g in (34, 30):
        ok.append(show(f'prewindc, a rewind: the import\'s first cut 17 of group {g} (the top 34), answers right', 2,
                       *run(ART, c, {}, cut_as(17, g)), key=prem))
    ok.append(show('prewindc, a rewind and a right daemon that ended c1 at it', 2,
                   *run(ART, c, {}, cut_as(17, 34), answer('c1-end', rewound),
                        *ended_at(17, 'refused rewound', rewound)), key=prem))
    ok.append(show('prewindc, no cut once the import returned (cuts 19 to 74 dropped)', 2,
                   *run(ART, c, {}, log_drop_all(lambda f: len(f) == 8 and f[2] == 'cut'
                                                  and f[6].isdigit() and 19 <= int(f[6]) <= 74)), key=prem))
    ok.append(show("prewindc, no line of c1's withdrawal", 2, *run(ART, c, {}, log_drop(wd)), key=prem))
    ok.append(show('prewindc, no mark session-2-ready', 2,
                   *run(ART, c, {}, edit('marks.tsv', lambda ls: [ln for ln in ls if not ln.endswith('session-2-ready')])),
                   key=prem))
    ok.append(show('prewindc, published.tsv without c1', 2, *run(ART, c, {}, edit('published.tsv', lambda ls: [])),
                   key=prem))
    ok.append(show('prewindc, no watch.log', 2, *run(ART, c, {}, drop('watch.log')), key=prem))
    ok.append(show("prewindc, the watcher's ERROR (cut_watch.py's own lines)", 2,
                   *run(ART, c, {}, append('watch.log', error_lines)), key=prem))
    ok.append(show('prewindc, a cut line that does not read', 2,
                   *run(ART, c, {}, edit('requests.tsv', lambda ls: [
                       '\t'.join(f[:7] + ['4x']) if len(f) == 8 and f[2] == 'cut' and f[6] == '30' else '\t'.join(f)
                       for f in (ln.split('\t') for ln in ls)])), key=prem))
    # Vacuous passes: the course did not wait across the import.
    ok.append(show('prewindc, c1 asked after the import (its C 19, the first cut after the return; the asked '
                   'line moved there)', 2,
                   *run(ART, c, {}, published_field('c1', 4, 19), log_drop(is_publish('asked', 'c1 accepted')),
                        log_after_cut(19, ('asked', 'c1 accepted'))), key=prem))
    ok.append(show('prewindc, c1 asked with --expiry-s 30, then (rightly) expired before its end was read', 2,
                   *run(ART, c, {}, edit('agent.tsv', lambda ls: [ln.replace('--expiry-s 900', '--expiry-s 30')
                                                                  for ln in ls]),
                        answer('c1-end', 'expired'), *ended_at(30, 'expired', 'expired')), key=prem))

    # prewindf: the new look, and the course as before.
    f = 'prewindf'
    ok.append(show('prewindf as run', 0, *run(ART, f, {})))
    ok.append(show('prewindf, f1-look a snap', 1, *run(ART, f, {}, answer('f1-look', look.replace('c1', 'f1')))))
    ok.append(show('prewindf, a daemon that never ends f1 (no ended line; f1-end pending)', 1,
                   *run(ART, f, {}, answer('f1-end', 'pending after 300 s'),
                        log_drop(is_publish('ended', 'f1 refused rewound')))))
    ok.append(show('prewindf, no cut not above the groups before it (cut 17 dropped)', 2,
                   *run(ART, f, {}, log_drop(is_cut(17)))))

    # prewind: N3 on the run's own log.
    p = 'prewind'
    rows = rows_of(ART, p)
    first = is_publish('asked', 'r1 refused rewound')
    ok.append(show('prewind as run', 0, *run(ART, p, rows)))
    ok.append(show("prewind, the daemon's `asked` line of r1's first ask absent (N3: was exit 0 at b76ab97)", 2,
                   *run(ART, p, rows, log_drop(first)), key='premise_r1_asked_after_a_rewind_cut'))
    ok.append(show("prewind, the first ask's line with C written in the G column (31 and 20 swapped)", 2,
                   *run(ART, p, rows, edit('requests.tsv', lambda ls: [
                       ln.replace('\tasked\t31\t20\tr1 refused rewound', '\tasked\t20\t31\tr1 refused rewound')
                       for ln in ls])), key='premise_r1_asked_after_a_rewind_cut'))
    ok.append(show('prewind, a daemon without rule (R) at the ask (r1 accepted; its log line)', 1,
                   *run(ART, p, rows, answer('r1', 'pending after 300 s'),
                        log_result('asked', 'r1 refused rewound', 'r1 accepted')),
                   key='premise_r1_asked_after_a_rewind_cut'))

    # pkill: the new look.
    k = 'pkill'
    rows = rows_of(ART, k)
    ok.append(show('pkill as run', 0, *run(ART, k, rows)))
    ok.append(show('pkill, tw-look a snap', 1, *run(ART, k, rows, answer('tw-look', look.replace('c1', 'tw')))))

    # The watcher's verdict in every course (cut_watch.py's own lines).
    for course in pc.COURSES:
        rows = rows_of(ART, course)
        base = [looks_stood_in(course)]
        ok.append(show(f'{course} as run', 0, *run(ART, course, rows, *base)))
        ok.append(show(f"{course}, the watcher's VERDICT", 1,
                       *run(ART, course, rows, *base, append('watch.log', verdict_line))))
        ok.append(show(f"{course}, the watcher's ERROR", 2,
                       *run(ART, course, rows, *base, append('watch.log', error_lines))))

    if OLD:
        rows = rows_of(OLD, 'prewind')
        ok.append(show("b76ab97's prewind recording through the new check", 0, *run(OLD, 'prewind', rows)))
        ok.append(show("b76ab97's prewind, its first ask's line absent (N3)", 2,
                       *run(OLD, 'prewind', rows, log_drop(is_publish('asked', 'r1 refused rewound')))))
        ok.append(show("b76ab97's prewindf recording (no f1-look then) through the new check", 2,
                       *run(OLD, 'prewindf', {})))
    print(f'\n{sum(ok)} of {len(ok)} cases ended as they should')


if __name__ == '__main__':
    main()
