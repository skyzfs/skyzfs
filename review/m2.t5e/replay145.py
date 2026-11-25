#!/usr/bin/env python3
"""Review of #145 at cb91ad1: the head's check (publish_check.main) on the
publish rig's own passist recording of run 45600486106 (ubuntu-26.04, the
pull request's run, passist's first kernel run), node B stood in for (its
row of a1 as the run's results.tsv names it, a stand-in manifest).

Each daemon fault changes only what a faulty daemon would have changed
(its answers, its request log's lines, what it put in object storage)
and, where the agent's flow would have ended sooner because of the
answer, what the agent would then not have done (assists that did not
happen, requests that were not sent). Each rig or kernel fault changes
only what a broken rig or kernel would have left.

Usage: python3 -I replay145.py WORKTREE ARTIFACT_DIR
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

WT, ART = sys.argv[1:3]
sys.path.insert(0, os.path.join(WT, 'tests/kernel'))
import nodeb_check  # noqa: E402
import publish_check as pc  # noqa: E402

ULID = '01ARZ3NDEKTSV4RRFFQ69G5FAV'
C = 'passist'


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


def read(fn):
    with open(os.path.join(ART, C, fn), errors='replace') as f:
        return f.read()


def answer(label, text):
    def change(d):
        p = os.path.join(d, 'ends.tsv')
        with open(p) as f:
            rows = [ln.split('\t', 1) for ln in f.read().splitlines() if ln]
        assert label in [k for k, _ in rows], label
        with open(p, 'w') as f:
            f.write(''.join(f'{k}\t{text if k == label else v}\n' for k, v in rows))
    return change


def edit(fn, fun):
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


def assists(keep):
    """assist.tsv's rows through KEEP (list of [name, t0, t1, v, before, mid, after] -> same); with
    no row left there is no file, as publish_agent.py leaves none when it made no assist."""
    def change(d):
        p = os.path.join(d, 'assist.tsv')
        with open(p) as f:
            rows = keep([ln.split('\t') for ln in f.read().splitlines() if ln])
        if not rows:
            os.unlink(p)
            return
        with open(p, 'w') as f:
            f.write(''.join('\t'.join(r) + '\n' for r in rows))
    return change


def only(name, n):
    """Keep NAME's first N assists (the flow ended after them), the others' all."""
    def keep(rows):
        out, k = [], 0
        for r in rows:
            if r[0] == name:
                k += 1
                if k > n:
                    continue
            out.append(r)
        return out
    return assists(keep)


def agent_until(name, ns):
    """agent.tsv without NAME's requests sent after NS (the flow ended)."""
    def fun(lines):
        out = []
        for ln in lines:
            f = ln.split('\t')
            if len(f) >= 3 and f[2].split()[1:3] == ['--name', name] and f[2].startswith('publish-progress') \
                    and int(f[0]) > ns:
                continue
            out.append(ln)
        return out
    return edit('agent.tsv', fun)


def log_ended(name, old, new, after_ns=None):
    """The request log's `ended NAME OLD` line made `ended NAME NEW`, moved
    to AFTER_NS when given."""
    def fun(lines):
        out = []
        for ln in lines:
            f = ln.split('\t')
            if len(f) == 8 and f[2] == 'publish' and f[4] == 'ended' and f[7] == f'{name} {old}':
                f[7] = f'{name} {new}'
                if after_ns is not None:
                    f[0] = str(after_ns)
                out.append(('\t'.join(f)))
                continue
            out.append(ln)
        out.sort(key=lambda ln: int(ln.split('\t')[0]) if ln.split('\t')[0].isdigit() else 0)
        return out
    return edit('requests.tsv', fun)


def log_add(ns, op, result):
    def fun(lines):
        f = lines[0].split('\t')
        lines.append('\t'.join([str(ns), f[1], 'publish', '-', op, '-', '-', result]))
        lines.sort(key=lambda ln: int(ln.split('\t')[0]) if ln.split('\t')[0].isdigit() else 0)
        return lines
    return edit('requests.tsv', fun)


def acc(**kw):
    """accepted.tsv (read only by the fix sketch, premise_sketch.diff; the
    head ignores it): NAME -> (newest group that wrote at the acceptance,
    at the end), as the run's kstat and log place them unless changed."""
    def change(d):
        now = {'a1': (16, 23), 'a2': (31, 67)}
        now.update(kw)
        with open(os.path.join(d, 'accepted.tsv'), 'w') as f:
            f.write(''.join(f'{k}\t{a}\t{b}\n' for k, (a, b) in now.items()))
    return change


def drop_cuts(groups):
    return edit('requests.tsv', lambda ls: [ln for ln in ls if not (
        len(ln.split('\t')) == 8 and ln.split('\t')[2] == 'cut' and ln.split('\t')[7] in {str(g) for g in groups})])


def status_without(fn, key):
    return edit(fn, lambda ls: [ln for ln in ls if not ln.startswith(key + ' ')])


def run(rows, *changes):
    src = os.path.join(ART, C)
    with tempfile.TemporaryDirectory(prefix='replay145-') as d:
        for fn in os.listdir(src):
            p = os.path.join(src, fn)
            if os.path.isfile(p) and fn not in ('results.tsv',):
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
        acc()(d)
        for change in changes:
            change(d)
        argv = ['publish_check.py', C, d, os.path.join(d, 'requests.tsv'), '-', '1048576']
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


RESULTS = []


def show(tag, title, want, rc, res, keys=()):
    flag = 'not judged here' if want is None else 'as it should' if rc == want else f'NOT {want}'
    RESULTS.append((tag, rc, want))
    print(f'== {tag} {title}: exit {rc} ({flag})')
    for r in res:
        if r[1] not in ('ok', 'INFO', 'CAUGHT') or r[0] in keys:
            print('   ' + '\t'.join(r)[:400])
    return rc == want


def main():
    res = read('results.tsv')
    m = re.search(r'^pin_a1_row\tok\t.* cut (\d+) of group (\d+) ', res, re.M)
    pub = pc.read_published(read('published.tsv'))
    a1 = pub['a1']
    state = a1['state']
    commit = pc.done_commit(pc.read_ends(read('ends.tsv'))['a1'])
    good = {'a1': (int(m.group(1)), int(m.group(2)), commit, state)}
    h2 = 'cd' * 32
    keys = ('premise_a1_needed_the_assist', 'premise_a2_assisted', 'obs_a1_cut', 'obs_assist_pace',
            'pin_assist_writes_a_group')
    pending = 'pending after 300 s'

    # The run: a1 C 6, G 20 (createtxg 15). Written groups: 15 (snapshot),
    # 16 (hold), 17 (the hold's history, ~5 s later). a1's assist 1 wrote
    # 18 (set) and 19 (the CLI's history), its sync committed 20 and 21
    # empty; assist 2 wrote 22 and 23; cut 12 of group 22 carried a1 at
    # 641.8 s. a2: C 15, G 1000030; 8 assists from 671.05 s; groups 33 to
    # 67; withdrawn at 731.2 s.
    show('P0', 'passist as run', 0, *run(good), keys=keys)

    # Daemon faults: each should end 1.
    show('D1', 'a1 never carried although the assists wrote groups 22 and 23 at or above G (pending after 300 s)',
         1, *run({}, answer('a1', pending), acc(a1=(16, 23)),
                 log_ended('a1', f'done commit={commit}', 'expired', after_ns=930522960598)), keys=keys)
    show('D2', 'a1 published on cut 11 of group 19 (below G 20) right after assist 1: the agent stopped there', 1,
         *run({'a1': (11, 19, h2, state)}, answer('a1', f'done commit={h2}'), only('a1', 1), acc(a1=(16, 19)),
              log_ended('a1', f'done commit={commit}', f'done commit={h2}', after_ns=640590000000),
              agent_until('a1', 640600000000)), keys=keys)
    show('D3', 'a1 published on cut 9 of group 17 (the hold\'s history, below G) before any assist', 1,
         *run({'a1': (9, 17, h2, state)}, answer('a1', f'done commit={h2}'), only('a1', 0), acc(a1=(16, 17)),
              log_ended('a1', f'done commit={commit}', f'done commit={h2}', after_ns=635900000000),
              agent_until('a1', 636000000000)), keys=keys)
    show('D4', 'a daemon that ignores G: a1 done on cut 8 (group 16) and a2 done on cut 17 (group 31), '
         'before any assist; a2\'s withdrawal answers done, its look a snap', 1,
         *run({'a1': (8, 16, h2, state), 'a2': (17, 31, 'ef' * 32, state)},
              answer('a1', f'done commit={h2}'), answer('a2', 'done commit=' + 'ef' * 32),
              answer('a2-withdraw', 'done commit=' + 'ef' * 32),
              answer('a2-look', f'snap commit={"ef" * 32} state={state} number=17 group=31 manifest-len=1 '
                                f'manifest-hash={"0" * 64}'),
              only('a1', 0), only('a2', 0), acc(a1=(16, 16), a2=(31, 31))), keys=keys)
    show('D5', 'a2 published on cut 21 (group 37) after its second assist: G far above ignored', 1,
         *run({'a1': good['a1'], 'a2': (21, 37, 'ef' * 32, state)}, answer('a2', 'done commit=' + 'ef' * 32),
              answer('a2-withdraw', 'done commit=' + 'ef' * 32),
              answer('a2-look', f'snap commit={"ef" * 32} state={state} number=21 group=37 manifest-len=1 '
                                f'manifest-hash={"0" * 64}'),
              only('a2', 2), acc(a2=(31, 38))), keys=keys)
    show('D6', 'a2 published on cut 19 (group 33) after its first assist', 1,
         *run({'a1': good['a1'], 'a2': (19, 33, 'ef' * 32, state)}, answer('a2', 'done commit=' + 'ef' * 32),
              answer('a2-withdraw', 'done commit=' + 'ef' * 32),
              answer('a2-look', f'snap commit={"ef" * 32} state={state} number=19 group=33 manifest-len=1 '
                                f'manifest-hash={"0" * 64}'),
              only('a2', 1), acc(a2=(31, 34))), keys=keys)
    show('D7', 'a2 expired by the daemon after 5 s (expiry 900 asked): no assist of a2', 1,
         *run(good, answer('a2', 'expired'), answer('a2-withdraw', 'expired'), only('a2', 0), acc(a2=(31, 31))),
         keys=keys)
    show('D8', 'a2 refused at once (refused name): no assist of a2', 1,
         *run(good, answer('a2', 'refused name x'), answer('a2-withdraw', 'unknown'), only('a2', 0),
              acc(a2=(31, 31))), keys=keys)
    show('D9', 'the withdrawal of a2 ignored (pending)', 1, *run(good, answer('a2-withdraw', 'pending')), keys=keys)
    show('D10', 'a2-look answers a snap with nothing published', 1,
         *run(good, answer('a2-look', f'snap commit={h2} state={state} number=20 group=34 manifest-len=1 '
                                      f'manifest-hash={"0" * 64}')), keys=keys)
    show('D11', 'the daemon counts no cut for the assists\' groups (cuts of 18 to 67 dropped); a1 pending', 1,
         *run({}, answer('a1', pending), drop_cuts(range(18, 68)),
              log_ended('a1', f'done commit={commit}', 'expired', after_ns=930522960598)), keys=keys)
    show('D12', 'a1 carried by cut 12 written as opening only as a rewind (pin_a1_cut)', 1,
         *run(good, edit('requests.tsv', lambda ls: [
             '\t'.join(f[:5] + ['rewind'] + f[6:]) if len(f) == 8 and f[2] == 'cut' and f[6] == '12' else '\t'.join(f)
             for f in (ln.split('\t') for ln in ls)])), keys=keys)
    show('D13', 'the status after a2 lacks upload_blocks (a status the daemon wrote short)', 1,
         *run(good, status_without('a2-status-after.txt', 'upload_blocks')), keys=keys)

    # The rig's or the kernel's: each should end 2.
    show('R1', 'the stand-in never assisted a1, which pends', 2,
         *run({}, answer('a1', pending), only('a1', 0), acc(a1=(16, 17)),
              log_ended('a1', f'done commit={commit}', 'expired', after_ns=930522960598)), keys=keys)
    show('R2', 'a1 at G before any assist (the pool not idle: the first assist found group 21)', 2,
         *run(good, assists(lambda rs: [r if r[0] != 'a1' else r[:4] + (['21', '22', '22'] if r[4] == '17' else
                                                                       ['22', '26', '27']) for r in rs]),
              acc(a1=(16, 27))), keys=keys)
    show('R3', 'the assists wrote nothing on the kernel (after = before for a1), a1 pending', 2,
         *run({}, answer('a1', pending), assists(lambda rs: [r[:5] + [r[4], r[4]] if r[0] == 'a1' else r for r in rs]),
              acc(a1=(16, 17)),
              log_ended('a1', f'done commit={commit}', 'expired', after_ns=930522960598)), keys=keys)
    show('R4', 'no assist.tsv', 2, *run(good, drop('assist.tsv')), keys=keys)
    show('R5', 'no a2-txgs.txt', 2, *run(good, drop('a2-txgs.txt')), keys=keys)
    show('R6', 'a2-txgs.txt short (groups up to 40 out of its history)', 2,
         *run(good, edit('a2-txgs.txt', lambda ls: [ln for ln in ls if not (ln.split()[0].isdigit()
                                                                             and int(ln.split()[0]) <= 40)])),
         keys=keys)
    show('R7', 'a2 assisted once by the stand-in, a2 pending', 2, *run(good, only('a2', 1)), keys=keys)
    show('R8', 'an assist line torn', 2, *run(good, append('assist.tsv', 'a2\t1\t2\n')), keys=keys)

    # Vacuous passes: should not end 0.
    show('V1', 'a1 carried by group 22 that no assist wrote (assist 2 read 23 before its set): '
         'something else wrote it', 2,
         *run(good, assists(lambda rs: [r[:4] + ['23', '24', '25'] if r[0] == 'a1' and r[4] == '19' else r
                                        for r in rs]), acc(a1=(16, 25))), keys=keys)
    show('V4', 'a1 carried before any assist by group 20 that something else wrote (the pool not idle)', 2,
         *run({'a1': (10, 20, h2, state)}, answer('a1', f'done commit={h2}'), only('a1', 0), acc(a1=(16, 20)),
              log_ended('a1', f'done commit={commit}', f'done commit={h2}', after_ns=636000000000),
              agent_until('a1', 636100000000)), keys=keys)
    show('V2', 'an assist storm: a2 assisted every second (35 rows, not the pace decided)', None,
         *run(good, assists(lambda rs: [r for r in rs if r[0] == 'a1'] + [
             ['a2', str(671050368444 + k * 10**9), str(671060000000 + k * 10**9), str(1795605205051 + k),
              str(32 + k), str(33 + k), str(33 + k)] for k in range(0, 35)])), keys=keys)
    show('V3', 'a1 assisted from 1 s after the acceptance (before ASSIST_AFTER, and before twice it)', None,
         *run(good, assists(lambda rs: [[r[0], str(int(r[1]) - 9 * 10**9), str(int(r[2]) - 9 * 10**9)] + r[3:]
                                        if r[0] == 'a1' else r for r in rs])), keys=keys)

    judged = [(t, rc, want) for t, rc, want in RESULTS if want is not None]
    bad = [t for t, rc, want in judged if rc != want]
    print(f'\n{len(judged) - len(bad)} of {len(judged)} judged cases as they should; not: {", ".join(bad) or "none"}')


main()
