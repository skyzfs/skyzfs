#!/usr/bin/env python3
"""Third look at #145 (M2.T5e) at 58fc660: the check of the worktree given
(publish_check.main) on a publish rig's own passist recording, node B
stood in for (its row of a1 as the run's results.tsv names it, a
stand-in manifest). The recordings: run 45616155197 (9bac3dd, the one
the earlier looks replayed) and run 45632551692 (58fc660, this head's
own); their groups are the same, and each case's times move with the
acceptance of its publish (see `at`).

Each daemon fault changes only what a faulty daemon would have changed
(its answers, its request log's lines, what it put in object storage)
and, where the agent's flow would have ended sooner or later because of
the answer, what the agent would then have done or not done. Each rig or
kernel fault changes only what a broken rig or kernel would have left.
The pace cases move the assists' times (assist.tsv) and the marks.

What each should end, at 58fc660 with the lead's answer to S1: a daemon
fault 1; the rig's or the kernel's 2; an assist too early (a floor) 2;
an assist too late or too slow 0, with obs_assist_gap_over_3x naming the
gap when it is over 3 times its value (the column "a gap over 3x").

Usage: python3 -I third_replay.py WORKTREE ARTIFACT_DIR
"""
import contextlib
import hashlib
import io
import os
import re
import shutil
import subprocess
import sys
import tempfile
from unittest import mock

WT, ART = sys.argv[1:3]
HEAD = subprocess.run(['git', '-C', WT, 'rev-parse', '--short=7', 'HEAD'], capture_output=True,
                      text=True).stdout.strip() or '?'
sys.path.insert(0, os.path.join(WT, 'tests/kernel'))
import nodeb_check  # noqa: E402
import publish_agent  # noqa: E402
import publish_check as pc  # noqa: E402

ULID = '01ARZ3NDEKTSV4RRFFQ69G5FAV'
C = 'passist'
S = 10**9


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
    """assist.tsv's rows through KEEP; with no row left there is no file,
    as publish_agent.py leaves none when it made no assist."""
    def change(d):
        p = os.path.join(d, 'assist.tsv')
        with open(p) as f:
            rows = keep([ln.split('\t') for ln in f.read().splitlines() if ln])
        if not rows:
            os.unlink(p)
            return
        with open(p, 'w') as f:
            f.write(''.join('\t'.join(map(str, r)) + '\n' for r in rows))
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


def retime(name, t0s, dur_s=None):
    """NAME's assists at the times T0S (s on the run's clock), each lasting
    DUR_S or as the first did, with the groups of the recorded rows (made
    up past them, two a row)."""
    def keep(rows):
        mine = [r for r in rows if r[0] == name]
        dur = int(mine[0][2]) - int(mine[0][1]) if dur_s is None else int(dur_s * S)
        out = [r for r in rows if r[0] != name]
        b = int(mine[0][4])
        for k, t in enumerate(t0s):
            r = mine[k] if k < len(mine) else None
            before, mid, after = (r[4], r[5], r[6]) if r else (b, b + 1, b + 2)
            b = int(after)
            out.append([name, int(t * S), int(t * S) + dur, 1795611009473 + k, before, mid, after])
        return out
    return assists(keep)


def marks(**kw):
    """accepted.tsv with the marks of KW changed: NAME_WHAT -> (ns, W, last)
    or None to drop the line."""
    def change(d):
        p = os.path.join(d, 'accepted.tsv')
        with open(p) as f:
            rows = [ln.split('\t') for ln in f.read().splitlines() if ln]
        out = []
        for r in rows:
            k = f'{r[0]}_{r[1]}'
            if k in kw:
                if kw[k] is None:
                    continue
                r = [r[0], r[1]] + [str(x) for x in kw[k]]
            out.append(r)
        with open(p, 'w') as f:
            f.write(''.join('\t'.join(r) + '\n' for r in out))
    return change


def agent_until(name, ns):
    """agent.tsv without NAME's progress requests sent after NS (the flow ended)."""
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


def drop_cuts(groups):
    return edit('requests.tsv', lambda ls: [ln for ln in ls if not (
        len(ln.split('\t')) == 8 and ln.split('\t')[2] == 'cut' and ln.split('\t')[7] in {str(g) for g in groups})])


def status_without(fn, key):
    return edit(fn, lambda ls: [ln for ln in ls if not ln.startswith(key + ' ')])



# The recording's own clock. The cases were written on the clock of the
# 9bac3dd recording (run 45616155197); on another recording of the same
# course (the groups are the same at 58fc660, run 45632551692) each time
# moves with the acceptance of its publish: a1's times with a1's, a2's
# with a2's.
REF_ACC = {'a1': 417763650654, 'a2': 448210515979}
SHIFT = {}


def at(t, s):
    """Seconds S on the 9bac3dd recording's clock, for publish T -> ns on
    this recording's."""
    return round(s * S) - SHIFT[t]


def sec(t, s):
    return at(t, s) / S


def run(rows, *changes):
    src = os.path.join(ART, C)
    with tempfile.TemporaryDirectory(prefix='third145-') as d:
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
        for change in changes:
            change(d)
        argv = ['publish_check.py', C, d, os.path.join(d, 'requests.tsv'), '-', '1048576']
        out = io.StringIO()
        patches = [mock.patch.object(nodeb_check, 'NodeB', fake_nodeb(rows)), mock.patch.object(sys, 'argv', argv)]
        with contextlib.ExitStack() as st:
            for p in patches:
                st.enter_context(p)
            st.enter_context(contextlib.redirect_stdout(out))
            try:
                pc.main()
                rc = None
            except SystemExit as e:
                rc = e.code
        with open(os.path.join(d, 'results.tsv')) as f:
            res = [ln.split('\t') for ln in f.read().splitlines()]
    return rc, res


RESULTS = []
KEYS = ('premise_a1_needed_the_assist', 'premise_a1_assists_at_the_decided_pace',
        'premise_a2_assists_at_the_decided_pace', 'obs_assist_gap_over_3x', 'premise_a2_assisted', 'obs_a1_cut',
        'obs_assist_pace')


def case(tag, title, want, rows, *changes, slow=None):
    """WANT: the exit wanted (None: not judged). SLOW: for the pace cases,
    whether obs_assist_gap_over_3x should name a gap (True) or say there
    is none (False)."""
    rc, res = run(rows, *changes)
    obs = next((r for r in res if r[0] == 'obs_assist_gap_over_3x'), None)
    said = None if obs is None else 'no gap' not in obs[2]
    right_exit = want is None or rc == want
    right = right_exit and (slow is None or said == slow)
    RESULTS.append((tag, rc, want, slow, said, right_exit, right))
    flag = 'not judged' if want is None else 'right' if right else 'NOT RIGHT'
    print(f'== {tag} {title}\n   exit {rc}, want {want}'
          + ('' if slow is None else f'; a gap over 3x named: {said}, want {slow}') + f' ({flag})')
    for r in res:
        if (r[1] not in ('ok', 'INFO', 'CAUGHT') or r[0] in KEYS) and r[0] not in ('time', 'obs_cuts'):
            print('   ' + '\t'.join(r)[:330])


def main():
    acc = pc.read_marks(read('accepted.tsv'))
    for t in ('a1', 'a2'):
        SHIFT[t] = REF_ACC[t] - acc[t]['accepted'][0]
    end1 = acc['a1']['end'][0]
    acc1, acc2 = acc['a1']['accepted'][0], acc['a2']['accepted'][0]
    ended1 = next(int(ln.split('\t')[0]) for ln in read('requests.tsv').splitlines()
                  if ln.split('\t')[2:3] == ['publish'] and ln.split('\t')[4] == 'ended'
                  and ln.split('\t')[7].startswith('a1 '))
    res = read('results.tsv')
    m = re.search(r'^pin_a1_row\tok\t.* cut (\d+) of group (\d+) ', res, re.M)
    pub = pc.read_published(read('published.tsv'))
    state = pub['a1']['state']
    commit = pc.done_commit(pc.read_ends(read('ends.tsv'))['a1'])
    good = {'a1': (int(m.group(1)), int(m.group(2)), commit, state)}
    h2, h3 = 'cd' * 32, 'ef' * 32
    pending = 'pending after 300 s'
    done = f'done commit={commit}'
    print(f'recording {os.path.basename(ART.rstrip("/"))}: a1 accepted at {acc1 / S:.3f} s, a2 at {acc2 / S:.3f} s; shifts from the 9bac3dd '
          f"recording's clock {SHIFT['a1'] / S:.3f} s and {SHIFT['a2'] / S:.3f} s; a1 carried by cut {good['a1'][0]} "
          f"of group {good['a1'][1]}")

    def look_snap(n, g):
        return answer('a2-look', f'snap commit={h3} state={state} number={n} group={g} manifest-len=1 '
                                 f'manifest-hash={"0" * 64}')

    def a2_done(n, g):
        return (answer('a2', f'done commit={h3}'), answer('a2-withdraw', f'done commit={h3}'), look_snap(n, g))

    def a2_at(*t0s, dur_s=None):
        return retime('a2', [sec('a2', t) for t in t0s], dur_s)

    case('P0', 'passist as run', 0, good, slow=False)

    # Daemon faults: each should end 1 (D13 and D13b, F2's statuses, 0).
    case('D1', 'a1 never carried although the assists wrote 22 and 23 (at or past G 20): pending after 300 s', 1,
         {}, answer('a1', pending), marks(a1_end=(at('a1', 718), 23, 'pending')),
         log_ended('a1', done, 'expired', after_ns=at('a1', 717.9)))
    case('D2', 'a1 published on cut 11 of group 19 (below G) right after assist 1', 1,
         {'a1': (11, 19, h2, state)}, answer('a1', f'done commit={h2}'), only('a1', 1),
         marks(a1_end=(at('a1', 428.906), 19, 'done')),
         log_ended('a1', done, f'done commit={h2}', after_ns=at('a1', 427.81)), agent_until('a1', at('a1', 428.904)))
    case('D3', 'a1 published on cut 9 of group 17 (the hold\'s history, below G) before any assist', 1,
         {'a1': (9, 17, h2, state)}, answer('a1', f'done commit={h2}'), only('a1', 0),
         marks(a1_end=(at('a1', 424.3), 17, 'done')),
         log_ended('a1', done, f'done commit={h2}', after_ns=at('a1', 423.3)), agent_until('a1', at('a1', 424.2)))
    case('D4', 'G ignored: a1 done on cut 8 (group 16), a2 on cut 17 (group 31), before any assist', 1,
         {'a1': (8, 16, h2, state), 'a2': (17, 31, h3, state)}, answer('a1', f'done commit={h2}'),
         *a2_done(17, 31), only('a1', 0), only('a2', 0),
         marks(a1_end=(at('a1', 418.767), 16, 'done'), a2_end=(at('a2', 449.212), 31, 'done')))
    case('D5', 'a2 published on cut 21 (group 37) after its second assist', 1,
         {'a1': good['a1'], 'a2': (21, 37, h3, state)}, *a2_done(21, 37), only('a2', 2),
         marks(a2_end=(at('a2', 460.46), 38, 'done')))
    case('D6', 'a2 published on cut 19 (group 33) after its first assist', 1,
         {'a1': good['a1'], 'a2': (19, 33, h3, state)}, *a2_done(19, 33), only('a2', 1),
         marks(a2_end=(at('a2', 459.34), 34, 'done')))
    case('D7', 'a2 expired by the daemon after 5 s (900 asked): no assist of a2', 1,
         good, answer('a2', 'expired'), answer('a2-withdraw', 'expired'), only('a2', 0),
         marks(a2_end=(at('a2', 453.3), 31, 'expired')))
    case('D8', 'a2 refused at its publish (no acceptance, so no marks)', 1,
         good, answer('a2', 'refused name x'), answer('a2-withdraw', 'unknown'), only('a2', 0),
         marks(a2_accepted=None, a2_end=None))
    case('D8a', 'a1 refused at its publish (no acceptance, so no marks)', 1,
         {}, answer('a1', 'refused stale x'), only('a1', 0), marks(a1_accepted=None, a1_end=None))
    case('D9', 'a2\'s withdrawal ignored (pending)', 1, good, answer('a2-withdraw', 'pending'))
    case('D10', 'a2-look answers a snap with nothing published', 1, good, look_snap(20, 34))
    case('D11', 'no cut counted for the assists\' groups (cuts of 18 to 67 dropped): a1 pending', 1,
         {}, answer('a1', pending), drop_cuts(range(18, 68)), marks(a1_end=(at('a1', 718), 23, 'pending')),
         log_ended('a1', done, 'expired', after_ns=at('a1', 717.9)))
    case('D12', 'the carrying cut opens only as a rewind', 1,
         good, edit('requests.tsv', lambda ls: [
             '\t'.join(f[:5] + ['rewind'] + f[6:]) if len(f) == 8 and f[2] == 'cut' and f[6] == '12' else '\t'.join(f)
             for f in (ln.split('\t') for ln in ls)]))
    case('D13', 'the status after a2 lacks upload_blocks (F2: an INFO line, no verdict)', 0,
         good, status_without('a2-status-after.txt', 'upload_blocks'))
    case('D13b', 'the status after a2 lacks uploaded and the one before is missing', 0,
         good, status_without('a2-status-after.txt', 'uploaded'), drop('a2-status-before.txt'))
    case('D14', 'a1 expired by the daemon after one assist (W 19)', 1,
         {}, answer('a1', 'expired'), only('a1', 1), marks(a1_end=(at('a1', 428.906), 19, 'expired')),
         log_ended('a1', done, 'expired', after_ns=at('a1', 427.9)), agent_until('a1', at('a1', 428.904)))
    case('D15', 'a1 refused by the daemon after both assists took the pool across G', 1,
         {}, answer('a1', 'refused rewound x'), marks(a1_end=(end1, 23, 'refused')),
         log_ended('a1', done, 'refused', after_ns=ended1))
    case('D16', 'a2 pending 20 s, then expired (its burst done)', 1,
         good, answer('a2', 'expired'), answer('a2-withdraw', 'expired'), only('a2', 3),
         marks(a2_end=(at('a2', 468.3), 42, 'expired')))
    case('D17', 'a2 published on cut 27 (group 51) after five assists', 1,
         {'a1': good['a1'], 'a2': (27, 51, h3, state)}, *a2_done(27, 51), only('a2', 5),
         marks(a2_end=(at('a2', 481.8), 52, 'done')))
    case('D18', 'a1 published on cut 11 of group 19 (below G), answered after both assists took the pool to 23', 1,
         {'a1': (11, 19, h2, state)}, answer('a1', f'done commit={h2}'),
         log_ended('a1', done, f'done commit={h2}'))
    case('D19', 'a1 published on cut 9 of group 17 (the hold\'s history, below G), answered after both assists', 1,
         {'a1': (9, 17, h2, state)}, answer('a1', f'done commit={h2}'),
         log_ended('a1', done, f'done commit={h2}'))
    # New at 58fc660: a daemon that answers each progress slowly, so the
    # agent's loop runs slow (every gap of a2 stretched): its pace is no
    # verdict, and its faults are still the daemon's. With a2 published
    # on cut 27 after five slow assists: 1, not 2.
    case('D20', 'a slow daemon (each progress 3 s: a2\'s gaps 4.1 s in the burst, 13 s spaced) that publishes a2', 1,
         {'a1': good['a1'], 'a2': (27, 51, h3, state)}, *a2_done(27, 51),
         a2_at(458.23, 462.35, 466.47, 479.5, 492.5), marks(a2_end=(at('a2', 496.0), 52, 'done')), slow=True)

    # Right but slow: should end 0.
    case('S1', 'a1 carried later: a third assist (23, 26, 27) 1.1 s after the second, then done', 0,
         good, assists(lambda rs: [r for r in rs if r[0] == 'a1'] + [
             ['a1', at('a1', 430.025), at('a1', 430.04), 1795610981265, 23, 26, 27]] + [r for r in rs if r[0] != 'a1']),
         marks(a1_end=(at('a1', 431.14), 27, 'done')))

    # The rig's or the kernel's: each should end 2.
    case('R1', 'the stand-in never assisted a1, which pends', 2,
         {}, answer('a1', pending), only('a1', 0), marks(a1_end=(at('a1', 718), 17, 'pending')),
         log_ended('a1', done, 'expired', after_ns=at('a1', 717.9)))
    case('R2', 'the pool not idle: at G when a1 was accepted', 2, good, marks(a1_accepted=(acc1, 20, 10.0)))
    case('R3', 'the assists wrote nothing on the kernel (after = before for a1), a1 pending', 2,
         {}, answer('a1', pending), assists(lambda rs: [r[:5] + [r[4], r[4]] if r[0] == 'a1' else r for r in rs]),
         marks(a1_end=(at('a1', 718), 17, 'pending')), log_ended('a1', done, 'expired', after_ns=at('a1', 717.9)))
    case('R4', 'no assist.tsv (the ends as recorded)', 2, good, drop('assist.tsv'))
    case('R5', 'no a2-txgs.txt', 2, good, drop('a2-txgs.txt'))
    case('R6', 'a2-txgs.txt short (groups up to 40 out of its history)', 2,
         good, edit('a2-txgs.txt', lambda ls: [ln for ln in ls if not (ln.split()[0].isdigit()
                                                                     and int(ln.split()[0]) <= 40)]))
    case('R7', 'a2 assisted once by the stand-in, a2 pending', 2, good, only('a2', 1))
    case('R8', 'an assist line torn', 2, good, append('assist.tsv', 'a2\t1\t2\n'))
    case('R9', 'no accepted.tsv', 2, good, drop('accepted.tsv'))
    case('R10', 'accepted.tsv torn', 2, good, append('accepted.tsv', 'a2\tend\t5\n'))
    case('R11', 'a1 accepted and no end mark', 2, good, marks(a1_end=None))

    # A1 carried by a group no assist wrote (G1): 2.
    case('V1', 'a1 carried by group 22 that no assist wrote (assist 2 read 23 before its set)', 2,
         good, assists(lambda rs: [r[:4] + ['23', '24', '25'] if r[0] == 'a1' and r[4] == '19' else r for r in rs]),
         marks(a1_end=(end1, 25, 'done')))
    case('V1b', 'a group at or above G written after the last assist, before the end (W 26 at the end)', 2,
         good, marks(a1_end=(end1, 26, 'done')))
    case('V4', 'a1 carried before any assist by group 20 that something else wrote', 2,
         {'a1': (10, 20, h2, state)}, answer('a1', f'done commit={h2}'), only('a1', 0),
         marks(a1_end=(at('a1', 424.3), 20, 'done')),
         log_ended('a1', done, f'done commit={h2}', after_ns=at('a1', 423.9)), agent_until('a1', at('a1', 424.2)))

    # Too fast, the floors (S1 as decided, S2): 2.
    case('V2', 'an assist storm: a2 assisted every second (35 rows)', 2,
         good, a2_at(*[458.23 + k for k in range(35)]))
    case('V3', 'a1 assisted from 1 s after the acceptance', 2,
         good, assists(lambda rs: [[r[0], str(int(r[1]) - 9 * S), str(int(r[2]) - 9 * S)] + r[3:]
                                   if r[0] == 'a1' else r for r in rs]))
    case('V9', 'the burst\'s pause 0.6 s after a set and sync of 0.5 s (1.1 s start to start)', 2,
         good, a2_at(458.23, 459.33, 460.43, 470.6, 480.7, 490.8, 500.9, 511.0, dur_s=0.5))
    case('V10', 'the burst\'s pause 0.88 s (a quick set and sync)', 2,
         good, a2_at(458.23, 459.13, 460.03, 470.2, 480.3, 490.4, 500.5, 510.6, dur_s=0.02))
    case('V11', 'a spaced gap of 9.4 s', 2,
         good, a2_at(458.23, 459.35, 460.47, 469.87, 480.0, 490.1, 500.2, 510.3))
    case('V12', 'a2\'s first assist 9.9 s after its acceptance', 2,
         good, a2_at(458.11, 459.23, 460.35, 470.5, 480.6, 490.7, 500.8, 510.9))
    case('V13', 'a third assist in the burst 0.95 s after the second\'s end (the first pause right)', 2,
         good, a2_at(458.23, 459.35, 460.315, 470.5, 480.6, 490.7, 500.8, 510.9, dur_s=0.015))

    # Too slow, the ceilings: no verdict since the lead's answer to S1;
    # obs_assist_gap_over_3x names it when a gap is over 3 times its value.
    case('V5', 'a2 assisted only in its burst (3 rows), then never again for 58 s; pending', 0,
         good, only('a2', 3), slow=True)
    case('V6', 'a2 spaced at 20 s after its burst (twice ASSIST_AFTER: not over 3x)', 0,
         good, a2_at(458.23, 459.35, 460.47, 480.6, 500.7), slow=False)
    case('V6b', 'a2 spaced at 31 s after its burst', 0,
         good, a2_at(458.23, 459.35, 460.47, 491.6), slow=True)
    case('V7', 'a2\'s first assist 30.02 s after its acceptance, not 10', 0,
         good, a2_at(478.23, 479.35, 480.47, 490.6, 500.7, 510.8), slow=True)
    case('V8', 'the stand-in run with A 5 s, at its own pace (the marks say 5.0; a2 pending 38 s)', 0,
         good, marks(a1_accepted=(acc1, 16, 5.0), a2_accepted=(acc2, 31, 5.0),
                     a2_end=(at('a2', 486.1), 67, 'pending')),
         retime('a1', [sec('a1', 422.8), sec('a1', 423.9)]),
         a2_at(453.3, 454.4, 455.5, 460.6, 465.7, 470.8, 475.9, 481.0), slow=False)
    case('V8b', 'the stand-in run with A 20 s at its own pace (a pause of 1.1 s in the burst, 20.1 s spaced)', 0,
         good, marks(a1_accepted=(acc1, 16, 20.0), a1_end=(at('a1', 440.0), 23, 'done'),
                     a2_accepted=(acc2, 31, 20.0), a2_end=(at('a2', 518.3), 67, 'pending')),
         retime('a1', [sec('a1', 437.8), sec('a1', 438.92)]),
         a2_at(468.23, 469.35, 470.47, 490.6, 510.7), slow=False)

    # The right pace on a slow runner: should end 0.
    case('F1', 'burst gaps of 3.5 s start to start (quick sets: a pause of 3.5 s, over 3 pauses)', 0,
         good, a2_at(458.23, 461.73, 465.23, 475.3, 485.4, 495.5, 505.6), slow=True)
    case('F2', 'burst gaps of 5.2 s (each set and sync 4.1 s): the pace of #144 and of the stand-in', 0,
         good, a2_at(458.23, 463.43, 468.63, 478.7, 488.8, 498.9, 509.0, dur_s=4.1), slow=False)
    case('F3', 'spaced gaps of 11.8 s (progress answered slowly)', 0,
         good, a2_at(458.23, 459.35, 460.47, 472.3, 484.1, 495.9, 507.7), slow=False)
    case('F4', 'a spaced gap of 9.6 s (the kstat read before the set 0.4 s slow at the assist before)', 0,
         good, a2_at(458.23, 459.35, 460.47, 470.87, 480.47, 490.6, 500.7, 510.8), slow=False)
    case('F5', 'the burst\'s pause 1.05 s after a set and sync of 12 s (a stalled sync; 13.05 s start to start)', 0,
         good, a2_at(458.23, 471.28, 484.33, 494.4, 504.5, dur_s=12.0), slow=False)
    case('F6', 'every gap three times slower (a runner under load): first 30.02 s, pauses 3.3 s, spaced 30.3 s', 0,
         good, marks(a2_end=(at('a2', 600.0), 67, 'pending')),
         a2_at(478.23, 481.55, 484.87, 515.2, 545.5), slow=True)

    judged = [r for r in RESULTS if r[2] is not None]
    bad = [r[0] for r in judged if not r[-1]]
    bad_exit = [r[0] for r in judged if not r[-2]]
    print(f'{HEAD} on {os.path.basename(ART.rstrip("/"))}: exit right in {len(judged) - len(bad_exit)} of '
          f'{len(judged)} cases (not: {", ".join(bad_exit) or "none"}); exit and obs_assist_gap_over_3x right in '
          f'{len(judged) - len(bad)} (not: {", ".join(bad) or "none"})')


main()
