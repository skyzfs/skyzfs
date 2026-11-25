#!/usr/bin/env python3
"""Next look at #145 (M2.T5e) at 9bac3dd and b3bb916: the check of the
worktree given (publish_check.main) on the publish rig's own passist
recording of run 45616155197 (ubuntu-26.04, the pull request's run at
9bac3dd, the first with accepted.tsv), node B stood in for (its row of a1
as the run's results.tsv names it, a stand-in manifest). The column
"head" is the worktree's commit, as it is.

Each daemon fault changes only what a faulty daemon would have changed
(its answers, its request log's lines, what it put in object storage)
and, where the agent's flow would have ended sooner or later because of
the answer, what the agent would then have done or not done (its
assists, its marks in accepted.tsv as the kstat would have had them,
its requests). Each rig or kernel fault changes only what a broken rig
or kernel would have left.

Every case runs against the head as it is, and with the sketch of B1's
fix put in by monkeypatching: `a1_premise` counts as a stray only a
group that wrote from a1's first assist on (B1_SKETCH=first, the
default) or only one at or above G (B1_SKETCH=ge_g, which is what
b3bb916 made of the head); the V and F cases also with the sketch of S1
and S2 (the burst's gaps from the end of the assist before, ceilings on
the pace, A the decided one).

The cases D18, D19 and I1 were added for b3bb916: a daemon that publishes
a1 below G after the assists took the pool past G (the hole asked about:
the stray check no longer sees groups below G), and a group below G
written between a1's assists by something else (where the two forms of
the fix differ; not judged).

Usage: python3 -I next_replay.py WORKTREE ARTIFACT_DIR
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


# --------------------------------------------------------------- sketches
def a1_premise_sketch(g, marks_, rows, txgs):
    """B1's fix: the head's a1_premise, with a stray only a group that
    wrote from the first assist on (SKETCH 'first') or at or above G
    (SKETCH 'ge_g') and that no assist wrote."""
    if 'end' not in marks_:
        return False, 'a1 was accepted and has no end mark'
    w_acc, w_end, verb = marks_['accepted'][1], marks_['end'][1], marks_['end'][2]
    top = max([w_end] + [r['after'] for r in rows])
    said = (f'a1 asked with G {g}: the newest group that wrote was {w_acc} when it was accepted and {top} by its '
            f'end ({verb}), over {len(rows)} assists')
    if w_acc >= g:
        return False, said + ': G was written before any assist'
    if top < g:
        if verb == 'pending':
            return False, said + ': the assists never took the pool to G, so nothing could carry a1'
        return True, said + ': below G at the end, so the end was the daemon\'s alone (the pins judge it)'
    crossed = [k + 1 for k, r in enumerate(rows) if r['before'] < g <= r['after']]
    theirs = {x for r in rows for x in range(r['before'] + 1, r['after'] + 1)}
    if not crossed:
        return False, said + ': no assist took the pool across G'
    if SKETCH == 'ge_g':
        wrote = pc.written_groups(txgs, w_acc, top)
        stray = sorted(x for x in wrote - theirs if x >= g)
    else:
        wrote = pc.written_groups(txgs, rows[0]['before'], top)
        stray = sorted(wrote - theirs)
    if stray:
        return False, said + f": groups no assist wrote ({SKETCH}): {', '.join(map(str, stray[:10]))}"
    before = sorted(pc.written_groups(txgs, w_acc, top) - theirs - set(stray))
    return True, said + f': assist {crossed[0]} took it across G, and every group that wrote ({SKETCH}) was ' \
        f"an assist's" + (f" (not judged: {', '.join(map(str, before[:10]))})" if before else '')


# B1's sketch: 'first' (the smaller form: groups that wrote from the
# first assist's `before` on must be the assists') or 'ge_g' (groups at
# or above G that wrote must be the assists').
SKETCH = os.environ.get('B1_SKETCH', 'first')


pc_pace_faults = pc.pace_faults
ENDS = {}


def pace_faults_ceiling(rows, accepted_ns, after):
    """S1's and S2's sketch: the head's rules, with the burst's gaps taken
    from the end of the assist before (t1, so that a slow set and sync
    does not read as a spaced pace), and with ceilings: the first assist
    within A + 3 s of the acceptance, the spaced gaps within 1.5 A, and,
    for a publish that ended `pending`, its last assist within 1.5 A of
    the end mark (ENDS, read from accepted.tsv by run()); and A the
    decided ASSIST_AFTER."""
    bad = []
    if after != publish_agent.ASSIST_AFTER:
        bad.append(f'the wait A {after} s, not ASSIST_AFTER {publish_agent.ASSIST_AFTER} s')
    if rows and rows[0]['t0'] - accepted_ns < after * 1e9:
        bad.append(f"{rows[0]['name']}'s first assist {(rows[0]['t0'] - accepted_ns) / 1e9:.1f} s after its "
                   f'acceptance, before {after} s')
    for k in range(1, len(rows)):
        gap = (rows[k]['t0'] - rows[k - 1]['t0']) / 1e9
        pause = (rows[k]['t0'] - rows[k - 1]['t1']) / 1e9
        if k < publish_agent.ASSIST_BURST and pause >= after / 2:
            bad.append(f"{rows[k]['name']}'s assist {k + 1} {pause:.1f} s after the end of the one before, "
                       'in the burst')
        if k >= publish_agent.ASSIST_BURST and gap < after * 0.95:
            bad.append(f"{rows[k]['name']}'s assist {k + 1} {gap:.1f} s after the one before, past the burst")
    end = ENDS.get(rows[0]['name']) if rows else None
    if end and end[2] == 'pending' and end[0] - rows[-1]['t0'] > 1.5 * after * 1e9:
        bad.append(f"{rows[0]['name']}'s last assist {(end[0] - rows[-1]['t0']) / 1e9:.1f} s before its "
                   f'end, past 1.5 A')
    if rows and rows[0]['t0'] - accepted_ns > (after + 3) * 1e9:
        bad.append(f"{rows[0]['name']}'s first assist {(rows[0]['t0'] - accepted_ns) / 1e9:.1f} s after its "
                   f'acceptance, past {after} s + 3 s')
    for k in range(publish_agent.ASSIST_BURST, len(rows)):
        gap = (rows[k]['t0'] - rows[k - 1]['t0']) / 1e9
        if gap > 1.5 * after:
            bad.append(f"{rows[k]['name']}'s assist {k + 1} {gap:.1f} s after the one before, past 1.5 A")
    return bad


def run(rows, *changes, sketch=False, ceiling=False):
    src = os.path.join(ART, C)
    with tempfile.TemporaryDirectory(prefix='next145-') as d:
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
        ENDS.clear()
        if os.path.exists(os.path.join(d, 'accepted.tsv')):
            with open(os.path.join(d, 'accepted.tsv')) as f:
                for ln in f.read().splitlines():
                    x = ln.split('\t')
                    if len(x) == 5 and x[1] == 'end' and x[2].isdigit():
                        ENDS[x[0]] = (int(x[2]), x[3], x[4])
        argv = ['publish_check.py', C, d, os.path.join(d, 'requests.tsv'), '-', '1048576']
        out = io.StringIO()
        patches = [mock.patch.object(nodeb_check, 'NodeB', fake_nodeb(rows)), mock.patch.object(sys, 'argv', argv)]
        if sketch:
            patches.append(mock.patch.object(pc, 'a1_premise', a1_premise_sketch))
        if ceiling:
            patches.append(mock.patch.object(pc, 'pace_faults', pace_faults_ceiling))
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
        'premise_a2_assists_at_the_decided_pace', 'premise_a2_assisted', 'obs_a1_cut', 'obs_assist_pace')


def case(tag, title, want, rows, *changes, ceiling=False):
    head = run(rows, *changes)
    b1 = run(rows, *changes, sketch=True)
    fixed = run(rows, *changes, sketch=True, ceiling=True) if ceiling else b1
    RESULTS.append((tag, head[0], b1[0], fixed[0], want))

    def flag(rc):
        return 'not judged' if want is None else 'right' if rc == want else f'NOT {want}'
    print(f'== {tag} {title}\n   head ({HEAD}): exit {head[0]} ({flag(head[0])}); with B1\'s sketch: exit {b1[0]} '
          f'({flag(b1[0])})' + (f'; and the ceiling: exit {fixed[0]} ({flag(fixed[0])})' if ceiling else ''))
    for label, (_, res) in (('head', head), ('B1', b1)) + ((('ceil', fixed),) if ceiling else ()):
        for r in res:
            if (r[1] not in ('ok', 'INFO', 'CAUGHT') or r[0] in KEYS) and r[0] not in ('time', 'obs_cuts'):
                print(f'   {label:6} ' + '\t'.join(r)[:330])


def main():
    res = read('results.tsv')
    m = re.search(r'^pin_a1_row\tok\t.* cut (\d+) of group (\d+) ', res, re.M)
    pub = pc.read_published(read('published.tsv'))
    state = pub['a1']['state']
    commit = pc.done_commit(pc.read_ends(read('ends.tsv'))['a1'])
    good = {'a1': (int(m.group(1)), int(m.group(2)), commit, state)}
    h2, h3 = 'cd' * 32, 'ef' * 32
    pending = 'pending after 300 s'
    done = f'done commit={commit}'

    def look_snap(n, g):
        return answer('a2-look', f'snap commit={h3} state={state} number={n} group={g} manifest-len=1 '
                                 f'manifest-hash={"0" * 64}')

    def a2_done(n, g):
        return (answer('a2', f'done commit={h3}'), answer('a2-withdraw', f'done commit={h3}'), look_snap(n, g))

    # The run at 9bac3dd (45616155197): a1 C 6, G 20 (createtxg 15),
    # accepted at 417.764 s with W 16. Groups 15 (snapshot), 16 (hold);
    # 17 the hold's history, at the transaction-group timeout (cut 9 at
    # 423.27 s), before a1's first assist at 427.79 s; assist 1 wrote 18
    # and 19, assist 2 wrote 22 and 23; cut 12 of group 22 carried a1;
    # end read at 430.02 s with W 23. a2: C 15, G 1000030, accepted at
    # 448.21 s with W 31 (32 is its hold's history); 8 assists from
    # 458.23 s; end `pending after 70 s` at 518.33 s with W 67.
    case('P0', 'passist as run', 0, good)

    # Daemon faults: each should end 1.
    case('D1', 'a1 never carried although the assists wrote 22 and 23 (at or past G 20): pending after 300 s', 1,
         {}, answer('a1', pending), marks(a1_end=(718000000000, 23, 'pending')),
         log_ended('a1', done, 'expired', after_ns=717900000000))
    case('D2', 'a1 published on cut 11 of group 19 (below G) right after assist 1', 1,
         {'a1': (11, 19, h2, state)}, answer('a1', f'done commit={h2}'), only('a1', 1),
         marks(a1_end=(428906000000, 19, 'done')),
         log_ended('a1', done, f'done commit={h2}', after_ns=427810000000), agent_until('a1', 428904000000))
    case('D3', 'a1 published on cut 9 of group 17 (the hold\'s history, below G) before any assist', 1,
         {'a1': (9, 17, h2, state)}, answer('a1', f'done commit={h2}'), only('a1', 0),
         marks(a1_end=(424300000000, 17, 'done')),
         log_ended('a1', done, f'done commit={h2}', after_ns=423300000000), agent_until('a1', 424200000000))
    case('D4', 'G ignored: a1 done on cut 8 (group 16), a2 on cut 17 (group 31), before any assist', 1,
         {'a1': (8, 16, h2, state), 'a2': (17, 31, h3, state)}, answer('a1', f'done commit={h2}'),
         *a2_done(17, 31), only('a1', 0), only('a2', 0),
         marks(a1_end=(418767000000, 16, 'done'), a2_end=(449212000000, 31, 'done')))
    case('D5', 'a2 published on cut 21 (group 37) after its second assist', 1,
         {'a1': good['a1'], 'a2': (21, 37, h3, state)}, *a2_done(21, 37), only('a2', 2),
         marks(a2_end=(460460000000, 38, 'done')))
    case('D6', 'a2 published on cut 19 (group 33) after its first assist', 1,
         {'a1': good['a1'], 'a2': (19, 33, h3, state)}, *a2_done(19, 33), only('a2', 1),
         marks(a2_end=(459340000000, 34, 'done')))
    case('D7', 'a2 expired by the daemon after 5 s (900 asked): no assist of a2', 1,
         good, answer('a2', 'expired'), answer('a2-withdraw', 'expired'), only('a2', 0),
         marks(a2_end=(453300000000, 31, 'expired')))
    case('D8', 'a2 refused at its publish (no acceptance, so no marks)', 1,
         good, answer('a2', 'refused name x'), answer('a2-withdraw', 'unknown'), only('a2', 0),
         marks(a2_accepted=None, a2_end=None))
    case('D8a', 'a1 refused at its publish (no acceptance, so no marks)', 1,
         {}, answer('a1', 'refused stale x'), only('a1', 0), marks(a1_accepted=None, a1_end=None))
    case('D9', 'a2\'s withdrawal ignored (pending)', 1, good, answer('a2-withdraw', 'pending'))
    case('D10', 'a2-look answers a snap with nothing published', 1, good, look_snap(20, 34))
    case('D11', 'no cut counted for the assists\' groups (cuts of 18 to 67 dropped): a1 pending', 1,
         {}, answer('a1', pending), drop_cuts(range(18, 68)), marks(a1_end=(718000000000, 23, 'pending')),
         log_ended('a1', done, 'expired', after_ns=717900000000))
    case('D12', 'the carrying cut opens only as a rewind', 1,
         good, edit('requests.tsv', lambda ls: [
             '\t'.join(f[:5] + ['rewind'] + f[6:]) if len(f) == 8 and f[2] == 'cut' and f[6] == '12' else '\t'.join(f)
             for f in (ln.split('\t') for ln in ls)]))
    case('D13', 'the status after a2 lacks upload_blocks (F2: an INFO line, no verdict)', 0,
         good, status_without('a2-status-after.txt', 'upload_blocks'))
    case('D13b', 'the status after a2 lacks uploaded and the one before is missing', 0,
         good, status_without('a2-status-after.txt', 'uploaded'), drop('a2-status-before.txt'))
    case('D14', 'a1 expired by the daemon after one assist (W 19)', 1,
         {}, answer('a1', 'expired'), only('a1', 1), marks(a1_end=(428906000000, 19, 'expired')),
         log_ended('a1', done, 'expired', after_ns=427900000000), agent_until('a1', 428904000000))
    case('D15', 'a1 refused by the daemon after both assists took the pool across G', 1,
         {}, answer('a1', 'refused rewound x'), marks(a1_end=(430022870380, 23, 'refused')),
         log_ended('a1', done, 'refused', after_ns=429082241813))
    case('D16', 'a2 pending 20 s, then expired (its burst done)', 1,
         good, answer('a2', 'expired'), answer('a2-withdraw', 'expired'), only('a2', 3),
         marks(a2_end=(468300000000, 42, 'expired')))
    case('D17', 'a2 published on cut 27 (group 51) after five assists', 1,
         {'a1': good['a1'], 'a2': (27, 51, h3, state)}, *a2_done(27, 51), only('a2', 5),
         marks(a2_end=(481800000000, 52, 'done')))
    # For b3bb916 (strays only at or above G): a daemon that publishes a1
    # below G once the assists have taken the pool past G. The stray check
    # no longer looks below G; the pins on the row must still see it.
    case('D18', 'a1 published on cut 11 of group 19 (below G), answered after both assists took the pool to 23', 1,
         {'a1': (11, 19, h2, state)}, answer('a1', f'done commit={h2}'),
         log_ended('a1', done, f'done commit={h2}'))
    case('D19', 'a1 published on cut 9 of group 17 (the hold\'s history, below G), answered after both assists', 1,
         {'a1': (9, 17, h2, state)}, answer('a1', f'done commit={h2}'),
         log_ended('a1', done, f'done commit={h2}'))

    # Right but slow: a daemon that took longer than the run's.
    case('S1', 'a1 carried later: a third assist (23, 26, 27) 1.1 s after the second, then done', 0,
         good, assists(lambda rs: [r for r in rs if r[0] == 'a1'] + [
             ['a1', 430025000000, 430040000000, 1795610981265, 23, 26, 27]] + [r for r in rs if r[0] != 'a1']),
         marks(a1_end=(431140000000, 27, 'done')))

    # The rig's or the kernel's: each should end 2.
    case('R1', 'the stand-in never assisted a1, which pends', 2,
         {}, answer('a1', pending), only('a1', 0), marks(a1_end=(718000000000, 17, 'pending')),
         log_ended('a1', done, 'expired', after_ns=717900000000))
    case('R2', 'the pool not idle: at G when a1 was accepted', 2, good, marks(a1_accepted=(417763650654, 20, 10.0)))
    case('R3', 'the assists wrote nothing on the kernel (after = before for a1), a1 pending', 2,
         {}, answer('a1', pending), assists(lambda rs: [r[:5] + [r[4], r[4]] if r[0] == 'a1' else r for r in rs]),
         marks(a1_end=(718000000000, 17, 'pending')), log_ended('a1', done, 'expired', after_ns=717900000000))
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

    # Vacuous passes: should not end 0.
    case('V1', 'a1 carried by group 22 that no assist wrote (assist 2 read 23 before its set)', 2,
         good, assists(lambda rs: [r[:4] + ['23', '24', '25'] if r[0] == 'a1' and r[4] == '19' else r for r in rs]),
         marks(a1_end=(430022870380, 25, 'done')))
    case('V1b', 'a group at or above G written after the last assist, before the end (W 26 at the end)', 2,
         good, marks(a1_end=(430022870380, 26, 'done')))
    case('V2', 'an assist storm: a2 assisted every second (35 rows)', 2,
         good, retime('a2', [458.23 + k for k in range(35)]))
    case('V3', 'a1 assisted from 1 s after the acceptance', 2,
         good, assists(lambda rs: [[r[0], str(int(r[1]) - 9 * S), str(int(r[2]) - 9 * S)] + r[3:]
                                   if r[0] == 'a1' else r for r in rs]))
    case('V4', 'a1 carried before any assist by group 20 that something else wrote', 2,
         {'a1': (10, 20, h2, state)}, answer('a1', f'done commit={h2}'), only('a1', 0),
         marks(a1_end=(424300000000, 20, 'done')),
         log_ended('a1', done, f'done commit={h2}', after_ns=423900000000), agent_until('a1', 424200000000))
    case('V5', 'a2 assisted only in its burst (3 rows), then never again for 58 s; pending', 2,
         good, only('a2', 3), ceiling=True)
    case('V6', 'a2 spaced at 20 s after its burst (twice ASSIST_AFTER)', 2,
         good, retime('a2', [458.23, 459.35, 460.47, 480.6, 500.7]), ceiling=True)
    case('V7', 'a2\'s first assist 30 s after its acceptance, not 10', 2,
         good, retime('a2', [478.23, 479.35, 480.47, 490.6, 500.7, 510.8]), ceiling=True)
    case('V8', 'the stand-in run with A 5 s, at its own pace (the marks say 5.0; a2 pending 38 s)', 2,
         good, marks(a1_accepted=(417763650654, 16, 5.0), a2_accepted=(448210515979, 31, 5.0),
                     a2_end=(486100000000, 67, 'pending')),
         retime('a1', [422.8, 423.9]),
         retime('a2', [453.3, 454.4, 455.5, 460.6, 465.7, 470.8, 475.9, 481.0]),
         ceiling=True)

    # The right pace on a slow runner: should end 0.
    case('F1', 'burst gaps of 3.5 s (each set and sync 2.4 s)', 0,
         good, retime('a2', [458.23, 461.73, 465.23, 475.3, 485.4, 495.5, 505.6]), ceiling=True)
    case('F2', 'burst gaps of 5.2 s (each set and sync 4.1 s): the pace of #144 and of the stand-in', 0,
         good, retime('a2', [458.23, 463.43, 468.63, 478.7, 488.8, 498.9, 509.0], dur_s=4.1), ceiling=True)
    case('F3', 'spaced gaps of 11.8 s (progress answered slowly)', 0,
         good, retime('a2', [458.23, 459.35, 460.47, 472.3, 484.1, 495.9, 507.7]), ceiling=True)

    judged = [r for r in RESULTS if r[-1] is not None]
    for k, label in ((1, 'head'), (2, "with B1's sketch"), (3, 'with B1\'s sketch and the ceiling')):
        bad = [r[0] for r in judged if r[k] != r[-1]]
        print(f'{label}: {len(judged) - len(bad)} of {len(judged)} cases right; not: {", ".join(bad) or "none"}')


main()
