#!/usr/bin/env python3
"""The agent rig's check of its retention course (aretain, part rt;
M2.7b-2, #179): `skyzfs-agent run` keeps the newest --keep names
published and retires the older ones, and a name given up is destroyed,
dropped or forgotten once it is older than --abandoned-keep-s
(tests/kernel/agent-record.sh, rec_aretain, says what each case does and
what it leaves).

Usage (agent_check.py hands an aretain recording on to it):
  agent_retain_check.py NAME DIR LOG

LOG is the daemon's request log. Each line of results.tsv is KEY,
VERDICT, [what it rests on] and the detail, as agent_check.py's. Exit 1
is a verdict against the agent or the daemon, 2 the rig's own error, 0
neither.

The agent's lines: on standard output `snap/<t> <commit>`, `retired
<t>`, `forgotten <t> (<why>)`, `abandoned <t> <why>` and the round
lines; on standard error `skyzfs-agent: report <t> not-retired: tried
again at the next round: <why>`. A line belongs to the round whose line
comes next after it (a round's line ends the round). A round retires
before it settles (program::schedule: the begin, retention, then the
settle), so a name published in round r counts for retention from round
r + 1 on: with --keep N, N + 1 names published are kept between two
rounds. The record's dump
(`skyzfs-agent record`): `published <t> commit=H group=G members=N
at_ms=M retire=-|ref|members|other`, `abandoned <t> why=W at_ms=M
left=nothing|unknown|members:N` and `stuck <t> members=N at_ms=M`.

In every case:
- pin_ID_record_reads (server): the record's dump after the case read.
- pin_ID_retired_gone (kernel and server): each name a run said
  `retired` has no @t left on the pool (but the rig's own, in K) and no
  ref and no row at node B, and is in no list of the record.
- pin_ID_kept (server): each name published and not retired is in the
  record's names published, unmarked, with the commit the run printed,
  its ref at node B on that commit, and pool@t on the pool.
- pin_ID_status (server): the daemon's counts of retires over the case
  (status-after less status-before: publish_retired,
  publish_retire_gone, publish_retire_other) are the request log's
  `retire` lines of the case's names answered retired, gone and refused
  other; the case's every retire line names the commit the run printed
  for its name. premise_ID_status where the status has no such counts
  (a daemon from before M2.7b-2) or the daemon was served again in the
  case.
- pin_ID_holds (server): no hold of the agent's tag is left.

K (--keep 2; two snapshots under t1 that are not the agent's):
- premise_K_names: four names or more published.
- pin_K_retired: the `retired` lines are, once each and in the order
  published, every name published before the last round but the newest
  two of those; the record keeps the rest and nothing else (no stuck, no
  given up).
- pin_K_keep: no name is said retired in a round before the one after
  the round in which the second name after it was published (the newest
  --keep are kept); pin_K_retired_in_round: and none later, unless a
  not-retired report named it (the round's steps are within STEPS_MAX).
- pin_K_others_kept (kernel and server): POOL/late@t1 (a dataset made
  after t1 was taken) and POOL/c2@t1 (destroyed and taken again by the
  rig) are still there, with their GUIDs: an @t of a GUID the record
  does not hold is not the agent's to destroy. premise_K_others: the
  rig made them between rounds 1 and 2.
- pin_K_request_log: one `retire` line a name retired, answered
  retired, in the order published, and none for a name kept.
- ctl_K_keep_3: pin_K_keep's judge with --keep 3 must catch the case's
  own lines; ctl_K_left: pin_ID_retired_gone's with POOL/c1@t1 added to
  the pool must catch it.

S (--keep 1; POOL/c1@t1 held by the rig from round 1 to round 4: t1 is
retired in round 3, when t2 is published, and t2 in round 4):
- premise_S_held: the hold was placed before round 2's line, and let go
  after round 4's or later.
- pin_S_stuck: after round 4 the record has t1 among the names stuck
  and not among those published; node B has no ref and no row of t1
  (its ref was retired before the destroy failed).
- pin_S_children_first (kernel and server): then POOL@t1 and POOL/c1@t1
  are there and POOL/c1/g@t1 is not: the children are destroyed before
  their parents, and the first failure stops the name's destroys.
- pin_S_said: a not-retired report of t1 names `zfs destroy
  POOL/c1@t1`, and no report of t1 repeats the one before it (said once
  for each reason).
- pin_S_others_go_on: t2 is said retired before the hold is let go: a
  name stuck counts for no room.
- pin_S_released: t1 is said retired once, in one of the two rounds
  after the hold was let go, and not before.
- pin_S_request_log: one `retire` line of t1, answered retired: a name
  stuck is not asked again.

X-retire, X-retain, X-destroy (--keep 1, the run killed at
retire:after:1, retain:before:1 or destroy-member:after:1, in round 2's
retire of t1; then the next run, run2):
- premise_ID_killed: the run's stderr has the hook's kill line for the
  case's point.
- pin_ID_said: `retired t1` is said by the killed run and by run2 as
  X_WANT says: a kill after `retired` was told and before the name left
  the record tells it again (the module's docs); one before tells it
  once.
- pin_ID_request_log: t1's retire lines are as X_WANT says: the retire
  asked again after a kill before its answer was saved is answered gone.
- pin_X-retire_nothing_destroyed, pin_X-destroy_first_member (kernel
  and server): after the kill, every member's @t1 is there (X-retire),
  or only the deepest, POOL/c1/g@t1, is gone (X-destroy).

B (--keep 60, ten rounds; then run2 --keep 1):
- premise_B_names: ten names or more published by the first run.
- pin_B_kept_60: the first run said nothing retired.
- pin_B_retired: every name published before the last round but the
  newest of those is said retired once, in the order published.
- pin_B_steps: no round of run2 says more than STEPS_MAX names retired,
  and its first says min(STEPS_MAX, the first run's names past --keep)
  where no not-retired report came (premise_B_backlog otherwise).
- obs_B_rounds: run2's retired count and took_ms by round.

A-old (store frozen; --age-max-s 8 --abandoned-keep-s 5):
- premise_A-old_given_up: the first `abandoned` line is `abandoned t1
  too-old`, t1 the name round 1 began (the give-up comes in the round
  after the one whose settle outlived the age: round 1's or round 2's).
- pin_A-old_left: the record had t1 given up as too-old, left with its
  members (left=members:N, N >= 1).
- pin_A-old_retired: `retired t1` once, in a round after the one that
  gave it up, and no `forgotten t1`; the record keeps no t1.

A-nothing, A-members (`publish` killed at intent:after:1 or
taken:after:1, `recover --give-up`, then `run --abandoned-keep-s 5`):
- premise_ID_killed as X's.
- pin_ID_left: the record had t given up by a person, left=nothing
  (A-nothing: no take) or left=members:N, N >= 1 (A-members).
- pin_A-nothing_dropped: the run names t on no line, and the record
  keeps no t: a give-up that left nothing leaves silently.
- pin_A-members_retired: the run says `retired t` once, and no
  `forgotten t`; the record keeps no t (and pin_ID_retired_gone).

And the course's: pin_watcher_reads (agent_fail_check.judge_watcher),
rig_error_in_the_course for DIR/rig-error.tsv, pin_ID_PIN for each
verdict the recorder wrote, premise_ID_rig for a case's rig-error.txt
(that case not judged).
"""
import os
import re
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import agent_fail_check  # noqa: E402
import cut_daemon_check  # noqa: E402
import publish_check  # noqa: E402
from cut_check import RigError  # noqa: E402

COURSES = ('aretain',)
POOL = 'skzcut'
HOLD_TAG = 'skyzfs-publish'
STEPS_MAX = 8
# The children the course makes under the pool's root before its cases.
CHILDREN = ('c1', 'c1/g', 'c2')
# The cases in the order the recorder runs them, and their kill points.
CASES = ('K', 'S', 'X-retire', 'X-retain', 'X-destroy', 'B', 'A-old', 'A-nothing', 'A-members')
KILLS = {'X-retire': 'retire:after:1', 'X-retain': 'retain:before:1', 'X-destroy': 'destroy-member:after:1',
         'A-nothing': 'intent:after:1', 'A-members': 'taken:after:1'}
# X: t1's retire lines in the request log, in order, and how often the
# killed run and run2 say `retired t1`.
X_WANT = {'X-retire': (['retired', 'gone'], 0, 1), 'X-retain': (['retired'], 1, 1),
          'X-destroy': (['retired'], 0, 1)}
KILL_LINE = re.compile(r'skyzfs-agent: test hook --kill-at: killed at ([a-z][a-z-]*) (before|after) \((\d+)\)')
ROUND = re.compile(r'round ([1-9][0-9]*) (ok|failed) took_ms=([0-9]+) ')
NOT_RETIRED = re.compile(r'skyzfs-agent: report (\S+) not-retired: tried again at the next round: (.*)')
STATUS_KEYS = ('publish_retired', 'publish_retire_gone', 'publish_retire_other')
ANSWER_KEY = {'retired': 'publish_retired', 'gone': 'publish_retire_gone', 'refused other': 'publish_retire_other'}


def flat(text):
    return ' '.join(str(text).split())


def read_out(text):
    """A run's standard output -> [{'kind', 'name', 'round', ...}] in
    order. 'round' is the round the line belongs to: 1 + the round lines
    before it (a round line belongs to its own round)."""
    lines, k = [], 1
    for ln in text.splitlines():
        f = ln.split(' ')
        m = ROUND.match(ln)
        if m:
            begun = [x[len('begun='):] for x in f if x.startswith('begun=')]
            lines.append({'kind': 'round', 'name': None, 'round': k, 'n': int(m.group(1)),
                          'took_ms': int(m.group(3)), 'begun': begun[0] if begun else None})
            k += 1
        elif f[0].startswith('snap/') and len(f) == 2:
            lines.append({'kind': 'snap', 'name': f[0][len('snap/'):], 'round': k, 'commit': f[1]})
        elif f[0] in ('retired', 'unsettled') and len(f) == 2:
            lines.append({'kind': f[0], 'name': f[1], 'round': k})
        elif f[0] in ('forgotten', 'abandoned', 'stopped') and len(f) >= 3:
            lines.append({'kind': f[0], 'name': f[1], 'round': k, 'why': ' '.join(f[2:])})
        else:
            lines.append({'kind': 'other', 'name': None, 'round': k, 'line': ln})
    return lines


def named(lines, kind, name=None):
    return [ln for ln in lines if ln['kind'] == kind and (name is None or ln['name'] == name)]


def published_names(lines):
    return [ln['name'] for ln in named(lines, 'snap')]


def last_round(lines):
    return max([ln['round'] for ln in named(lines, 'round')], default=0)


def due_retired(lines, keep):
    """-> the names the last round's retention had past --keep: those
    published before the last round, but the newest KEEP of them (a
    round retires before it settles)."""
    last = last_round(lines)
    seen = [ln['name'] for ln in named(lines, 'snap') if ln['round'] < last]
    return seen[:max(0, len(seen) - keep)]


def read_dump(text):
    """The record's dump -> {'published': {t: fields}, 'abandoned': ...,
    'stuck': ..., 'order': [t published, in order]}."""
    d = {'published': {}, 'abandoned': {}, 'stuck': {}, 'order': []}
    for ln in text.splitlines():
        f = ln.split(' ')
        if f[0] not in ('published', 'abandoned', 'stuck'):
            continue
        kv = dict(w.split('=', 1) for w in f[2:] if '=' in w)
        if len(f) < 2 or len(kv) != len(f) - 2 or f[1] in d[f[0]]:
            raise RigError(f'the record\'s dump: a line the rig cannot read: {ln!r}')
        d[f[0]][f[1]] = kv
        if f[0] == 'published':
            d['order'].append(f[1])
    return d


def read_snaps(text):
    """`name guid` lines (tab-separated) -> {snapshot: guid}."""
    snaps = {}
    for ln in text.splitlines():
        f = ln.split('\t')
        if len(f) != 2 or '@' not in f[0] or not f[1].isdigit():
            raise RigError(f'the pool\'s snapshots: not name, guid: {ln!r}')
        snaps[f[0]] = f[1]
    return snaps


def at(snaps, t):
    """The snapshots named @t, sorted."""
    return sorted(s for s in snaps if s.split('@', 1)[1] == t)


def read_status(text):
    st = {}
    for ln in text.splitlines():
        f = ln.split()
        if len(f) == 2:
            st[f[0]] = f[1]
    return st


def retire_lines_of(instances, names):
    """The request log's retire lines of `names`, in order -> [(T, the
    answer, fields)]."""
    return [r for inst in publish_check.retire_lines(instances) for r in inst if r[0] in names]


def judge_gone(names, snaps, rows, dump, allowed=()):
    """-> what is left of names said retired: an @t on the pool (but the
    snapshots `allowed`, as (snapshot, guid)), a ref or a row at node B,
    or the name in the record."""
    bad = []
    ok = set(allowed)
    for t in names:
        left = [s for s in at(snaps, t) if (s, snaps[s]) not in ok]
        if left:
            bad.append(f'{t} is retired and {", ".join(left)} is on the pool')
        got = rows.get(t)
        if got is None:
            bad.append(f'no row-{t}.txt from node B')
        elif got['ref'] is not None or got['row'] is not None:
            bad.append(f'{t} is retired and node B has its ' + ('ref' if got['ref'] is not None else 'row'))
        for kind in ('published', 'abandoned', 'stuck'):
            if t in dump[kind]:
                bad.append(f'{t} is retired and the record has it among the names {kind}')
    return bad


def judge_kept(names, printed, snaps, rows, dump):
    """-> what differs for names published and kept."""
    bad = []
    for t in names:
        p = dump['published'].get(t)
        if p is None:
            bad.append(f'{t} is kept and not among the record\'s names published')
        else:
            if p.get('commit') != printed[t]:
                bad.append(f"the record has {t} on commit {p.get('commit', '-')[:12]}, the run printed "
                           f'{printed[t][:12]}')
            if p.get('retire') != '-':
                bad.append(f"{t} is kept and marked retire={p.get('retire')}")
        got = rows.get(t)
        if got is None:
            bad.append(f'no row-{t}.txt from node B')
        elif got['ref'] is None or got['ref'][0] != printed[t]:
            bad.append(f'{t} is kept and node B has ' + ('no ref' if got['ref'] is None
                                                         else f"its ref on {got['ref'][0][:12]}"))
        if f'{POOL}@{t}' not in snaps:
            bad.append(f'{t} is kept and {POOL}@{t} is not on the pool')
    return bad


def judge_keep(lines, keep):
    """-> each name said retired in a round before the one after the
    round in which the keep-th name after it was published (lines: the
    runs' lines in order, rounds counted on across runs)."""
    names = published_names(lines)
    when = {ln['name']: ln['round'] for ln in named(lines, 'snap')}
    bad = []
    for ln in named(lines, 'retired'):
        t = ln['name']
        if t not in when:
            continue
        i = names.index(t)
        if i + keep >= len(names):
            bad.append(f'{t} said retired with fewer than {keep} names published after it')
        elif ln['round'] <= when[names[i + keep]]:
            bad.append(f"{t} said retired in round {ln['round']}, not after {names[i + keep]} was published "
                       f'(round {when[names[i + keep]]})')
    return bad


def judge_in_round(lines, keep, reported):
    """-> each name said retired later than the round after the one in
    which the keep-th name after it was published, where no not-retired
    report named it."""
    names = published_names(lines)
    when = {ln['name']: ln['round'] for ln in named(lines, 'snap')}
    bad = []
    for ln in named(lines, 'retired'):
        t = ln['name']
        if t not in when or t in reported or names.index(t) + keep >= len(names):
            continue
        due = when[names[names.index(t) + keep]] + 1
        if ln['round'] > due:
            bad.append(f"{t} said retired in round {ln['round']}, not in round {due}")
    return bad


def judge_retired_once(lines, want):
    """-> what differs between the `retired` lines and WANT, the names
    in order, each once."""
    seen = [ln['name'] for ln in named(lines, 'retired')]
    return [] if seen == list(want) else [f"retired {' '.join(seen) or 'none'}, not {' '.join(want) or 'none'}"]


def judge_retire_log(lines, want, printed):
    """The case's retire lines against WANT [(T, answer)] in order, each
    naming the commit printed for T."""
    bad = []
    seen = [(t, a) for t, a, _ in lines]
    if seen != list(want):
        bad.append('retire lines ' + (', '.join(f'{t} {a}' for t, a in seen) or 'none') + '; want '
                   + (', '.join(f'{t} {a}' for t, a in want) or 'none'))
    for t, a, fields in lines:
        if fields.get('commit') != printed.get(t):
            bad.append(f"the retire of {t} ({a}) names commit {fields.get('commit', '-')[:12]}, the run printed "
                       f"{printed.get(t, '-')[:12]}")
    return bad


def reports(err):
    """not-retired reports -> [(t, why)] in order."""
    return [(m.group(1), m.group(2)) for m in (NOT_RETIRED.match(ln) for ln in err.splitlines()) if m]


def killed(err, kill):
    effect, side, n = kill.split(':')
    return any(m.groups() == (effect, side, n) for m in (KILL_LINE.search(ln) for ln in err.splitlines()) if m)


class Case:
    """What a case left (DIR/cases/ID), read."""

    def __init__(self, rdir, cid):
        self.rdir, self.cid = rdir, cid
        self.dir = os.path.join(rdir, 'cases', cid)

    def text(self, name, need=True):
        p = os.path.join(self.dir, name)
        if not os.path.exists(p):
            if need:
                raise RigError(f'case {self.cid}: no {name}')
            return ''
        with open(p, errors='replace') as f:
            return f.read()

    def exists(self, name):
        return os.path.exists(os.path.join(self.dir, name))


def read_case(c, instances):
    """-> the case's facts (agent-record.sh, rcase and rt_after)."""
    runs = [lb for lb in ('publish', 'run2', 'run') if c.exists(f'{lb}.out')]
    k = {'runs': runs, 'out': {}, 'err': {}, 'rc': {}}
    for lb in runs:
        k['out'][lb] = read_out(c.text(f'{lb}.out'))
        k['err'][lb] = c.text(f'{lb}.err', need=False)
        k['rc'][lb] = c.text(f'{lb}.rc', need=False).strip() or '-'
    # Rounds counted on across the runs, for a judge of the whole case.
    k['lines'], base = [], 0
    for lb in runs:
        for ln in k['out'][lb]:
            k['lines'].append(dict(ln, round=ln['round'] + base, run=lb))
        base += len(named(k['out'][lb], 'round'))
    k['printed'] = {ln['name']: ln['commit'] for ln in named(k['lines'], 'snap')}
    k['record_rc'] = c.text('record-after.rc').strip()
    k['dump'] = read_dump(c.text('record-after.txt')) if k['record_rc'] == '0' else read_dump('')
    k['snaps'] = read_snaps(c.text('snaps-after.txt'))
    k['names'] = [ln for ln in c.text('printed.txt').splitlines() if ln]
    k['rows'] = {t: publish_check.read_snap_row(c.text(f'row-{t}.txt')) for t in k['names']
                 if c.exists(f'row-{t}.txt')}
    k['before'] = read_status(c.text('status-before.txt'))
    k['after'] = read_status(c.text('status-after.txt'))
    k['retires'] = retire_lines_of(instances, set(k['names']))
    k['holds'] = [ln.split('\t') for ln in c.text('holds.txt', need=False).splitlines() if ln]
    k['recovered'] = c.text('recovered.txt', need=False)
    k['reports'] = [r for lb in runs for r in reports(k['err'][lb])]
    return k


def judge_common(cid, k, j, allowed=()):
    j.holds(f'pin_{cid}_record_reads', [] if k['record_rc'] == '0' else
            [f"skyzfs-agent record ended {k['record_rc']}"], 'server', 'the record\'s dump read')
    retired = [ln['name'] for ln in named(k['lines'], 'retired')]
    j.holds(f'pin_{cid}_retired_gone', judge_gone(sorted(set(retired)), k['snaps'], k['rows'], k['dump'], allowed),
            'kernel and server', f'{len(set(retired))} names retired: no @t of theirs, no ref, no row, not in the record')
    kept = [t for t in k['printed'] if t not in retired]
    j.holds(f'pin_{cid}_kept', judge_kept(kept, k['printed'], k['snaps'], k['rows'], k['dump']), 'server',
            f'{len(kept)} names kept: in the record, their refs on the commits printed, pool@t there')
    left = [h[0] for h in k['holds'] if len(h) >= 2 and h[1] == HOLD_TAG]
    j.holds(f'pin_{cid}_holds', [f'a hold of {HOLD_TAG} on {s}' for s in left], 'server', 'no hold of the agent\'s left')
    before, after = k['before'], k['after']
    if not j.premise(f'premise_{cid}_status',
                     [f'the status has no {key}' for key in STATUS_KEYS if key not in before or key not in after]
                     + (['the daemon was served again in the case'] if 'the daemon served again' in k['recovered']
                        else []), 'the status counts retires, and one daemon served the case'):
        return
    counted = {key: int(after[key]) - int(before[key]) for key in STATUS_KEYS}
    logged = dict.fromkeys(STATUS_KEYS, 0)
    for _, a, _ in k['retires']:
        if a in ANSWER_KEY:
            logged[ANSWER_KEY[a]] += 1
    bad = [f'{key} counted {counted[key]} over the case, the request log has {logged[key]}'
           for key in STATUS_KEYS if counted[key] != logged[key]]
    bad += [f"the retire of {t} ({a}) names commit {fields.get('commit', '-')[:12]}, the run printed "
            f"{k['printed'].get(t, '-')[:12]}" for t, a, fields in k['retires']
            if t in k['printed'] and fields.get('commit') != k['printed'][t]]
    j.holds(f'pin_{cid}_status', bad, 'server', ', '.join(f'{key} {counted[key]}' for key in STATUS_KEYS)
            + f"; {len(k['retires'])} retire lines on the commits printed")


def judge_k(c, k, j):
    names = published_names(k['lines'])
    if not j.premise('premise_K_names', [] if len(names) >= 4 else [f'{len(names)} names published, not 4 or more'],
                     f'{len(names)} names published'):
        return
    others = [ln.split('\t') for ln in c.text('others.tsv').splitlines() if ln]
    obad = [] if c.text('others.rounds').strip() == '1' else \
        [f"made after {c.text('others.rounds').strip()} round lines, not 1"]
    if len(others) != 2 or any(len(o) != 4 for o in others):
        obad.append(f'others.tsv: not two lines of what, snapshot, guid before, guid after: {others!r}')
    elif others[1][2] == others[1][3]:
        obad.append(f'{others[1][1]} has the GUID it had before the rig destroyed it')
    if not j.premise('premise_K_others', obad, 'two snapshots under t1 not the agent\'s, made between rounds 1 and 2'):
        return
    allowed = [(o[1], o[3]) for o in others]
    judge_common('K', k, j, allowed)
    want = due_retired(k['lines'], 2)
    bad = judge_retired_once(k['lines'], want)
    d = k['dump']
    if sorted(d['published']) != sorted(set(names) - set(want)) or d['stuck'] or d['abandoned']:
        bad.append(f"the record keeps published {' '.join(d['order']) or 'none'}, stuck "
                   f"{' '.join(d['stuck']) or 'none'}, given up {' '.join(d['abandoned']) or 'none'}")
    j.holds('pin_K_retired', bad, 'server', f'{len(want)} names retired in order; {len(names) - len(want)} kept')
    j.holds('pin_K_keep', judge_keep(k['lines'], 2), 'server',
            'no name retired before the round after 2 newer were published')
    reported = {t for t, _ in k['reports']}
    j.holds('pin_K_retired_in_round', judge_in_round(k['lines'], 2, reported), 'server',
            'each retired in the round after the one that published the second name after it'
            + (f" (reported: {' '.join(sorted(reported))})" if reported else ''))
    j.holds('pin_K_others_kept', [f'{s} with GUID {g} is not on the pool' + (
        f' (GUID {k["snaps"][s]} there)' if s in k['snaps'] else '') for _, s, _, g in others
        if k['snaps'].get(s) != g], 'kernel and server', ', '.join(o[1] for o in others) + ' left as they were')
    j.holds('pin_K_request_log', judge_retire_log(k['retires'], [(t, 'retired') for t in want], k['printed']),
            'server', f'{len(want)} retire lines, answered retired, in order')
    caught = judge_keep(k['lines'], 3)
    j.control('ctl_K_keep_3', caught, '--keep 3 judged on the case\'s lines')
    added = dict(k['snaps'])
    added[f'{POOL}/c1@{names[0]}'] = '1'
    j.control('ctl_K_left', judge_gone(names[:1], added, k['rows'], k['dump'], allowed),
              f'{POOL}/c1@{names[0]} added to the pool')


def judge_s(c, k, j):
    names = published_names(k['lines'])
    held, released = c.text('held.rounds').strip(), c.text('released.rounds').strip()
    sbad = [] if held == '1' else [f'the hold placed after {held} round lines, not 1']
    if not released.isdigit() or int(released) < 4:
        sbad.append(f'the hold let go after {released or "no"} round lines, not 4 or more')
    if len(names) < 2:
        sbad.append(f'{len(names)} names published, not 2 or more')
    if not j.premise('premise_S_held', sbad, f'held after round 1, let go after round {released}'):
        return
    judge_common('S', k, j)
    t1, t2, rel = names[0], names[1], int(released)
    sd = read_dump(c.text('stuck-dump.txt'))
    bad = []
    if t1 not in sd['stuck'] or t1 in sd['published']:
        bad.append(f"after round 4 the record has stuck {' '.join(sd['stuck']) or 'none'}, published "
                   f"{' '.join(sd['order']) or 'none'}")
    row = publish_check.read_snap_row(c.text('stuck-row.txt'))
    if row['ref'] is not None or row['row'] is not None:
        bad.append(f'node B has the ref or the row of {t1} while it is stuck')
    j.holds('pin_S_stuck', bad, 'server', f"{t1} stuck (members={sd['stuck'].get(t1, {}).get('members', '-')}), "
            'its ref and row retired')
    ss = read_snaps(c.text('stuck-snaps.txt'))
    want = {f'{POOL}@{t1}': True, f'{POOL}/c1@{t1}': True, f'{POOL}/c1/g@{t1}': False}
    j.holds('pin_S_children_first', [f"{s} is {'not ' if there else ''}on the pool while {t1} is stuck"
                                     for s, there in want.items() if (s in ss) != there], 'kernel and server',
            f"left: {', '.join(at(ss, t1))}")
    said = [why for t, why in reports(k['err']['publish']) if t == t1]
    bad = [] if said and f'zfs destroy {POOL}/c1@{t1}' in said[0] else \
        [f'the first not-retired report of {t1}: {said[0] if said else "none"}']
    bad += [f'a report of {t1} said twice running: {a}' for a, b in zip(said, said[1:]) if a == b]
    j.holds('pin_S_said', bad, 'server', f'{len(said)} not-retired reports of {t1}: {said[0] if said else ""}')
    r2 = named(k['lines'], 'retired', t2)
    j.holds('pin_S_others_go_on', [] if r2 and r2[0]['round'] <= rel else
            [f"{t2} said retired in round {r2[0]['round'] if r2 else 'none'}, not by round {rel}"], 'server',
            f"{t2} retired in round {r2[0]['round'] if r2 else '-'} while {t1} was stuck")
    r1 = named(k['lines'], 'retired', t1)
    j.holds('pin_S_released', [] if len(r1) == 1 and rel < r1[0]['round'] <= rel + 2 else
            [f"{t1} said retired in rounds {', '.join(str(ln['round']) for ln in r1) or 'none'}; the hold was let go "
             f'after round {rel}'], 'server', f"{t1} retired in round {r1[0]['round'] if r1 else '-'}, "
            f'the hold let go after round {rel}')
    j.holds('pin_S_request_log', judge_retire_log([r for r in k['retires'] if r[0] == t1], [(t1, 'retired')],
                                                  k['printed']), 'server', f'one retire of {t1}, answered retired')


def judge_x(c, k, j):
    cid = c.cid
    err = k['err'].get('publish', '')
    if not j.premise(f'premise_{cid}_killed', [] if killed(err, KILLS[cid]) else
                     [f"no kill line for {KILLS[cid]} in publish.err: {flat(err)[-200:]}"], f'killed at {KILLS[cid]}'):
        return
    names = published_names(k['out'].get('publish', []))
    if not j.premise(f'premise_{cid}_names', [] if names else ['the killed run published no name'],
                     f'{names[0] if names else ""} published first'):
        return
    judge_common(cid, k, j)
    t1 = names[0]
    log, n1, n2 = X_WANT[cid]
    s1 = len(named(k['out'].get('publish', []), 'retired', t1))
    s2 = len(named(k['out'].get('run2', []), 'retired', t1))
    j.holds(f'pin_{cid}_said', [] if (s1, s2) == (n1, n2) else
            [f'`retired {t1}` said {s1} times by the killed run and {s2} by run2, not {n1} and {n2}'], 'server',
            f'`retired {t1}` said {s1} and {s2} times')
    j.holds(f'pin_{cid}_request_log', judge_retire_log([r for r in k['retires'] if r[0] == t1],
                                                       [(t1, a) for a in log], k['printed']), 'server',
            f"{t1}'s retire lines: {', '.join(log)}")
    if cid in ('X-retire', 'X-destroy'):
        ks = read_snaps(c.text('killed-snaps.txt'))
        gone = {'X-retire': (), 'X-destroy': (f'{POOL}/c1/g@{t1}',)}[cid]
        want = [f'{POOL}@{t1}'] + [f'{POOL}/{ch}@{t1}' for ch in CHILDREN]
        bad = [f"{s} is {'' if s in ks else 'not '}on the pool after the kill" for s in want
               if (s in ks) == (s in gone)]
        key = {'X-retire': 'pin_X-retire_nothing_destroyed', 'X-destroy': 'pin_X-destroy_first_member'}[cid]
        j.holds(key, bad, 'kernel and server', f"after the kill: {', '.join(at(ks, t1))}")


def judge_b(c, k, j):
    n1 = published_names(k['out'].get('publish', []))
    if not j.premise('premise_B_names', [] if len(n1) >= 10 else [f'{len(n1)} names published by the first run, not 10'],
                     f'{len(n1)} names published by the first run'):
        return
    judge_common('B', k, j)
    names = published_names(k['lines'])
    j.holds('pin_B_kept_60', [f"the first run said {ln['name']} retired" for ln in
                              named(k['out']['publish'], 'retired')], 'server', 'nothing retired at --keep 60')
    want = due_retired(k['lines'], 1)
    j.holds('pin_B_retired', judge_retired_once(k['lines'], want), 'server',
            f'{len(want)} names retired in order, {len(names) - len(want)} kept')
    run2 = k['out'].get('run2', [])
    rounds = [ln['round'] for ln in named(run2, 'round')]
    per = [(r, len([ln for ln in named(run2, 'retired') if ln['round'] == r])) for r in rounds]
    due = min(STEPS_MAX, len(n1) - 1)
    bad = [f'round {r} of run2 said {n} retired' for r, n in per if n > STEPS_MAX]
    j.info('obs_B_rounds', 'server', '; '.join(
        f"round {ln['round']}: {dict(per).get(ln['round'], 0)} retired, took_ms={ln['took_ms']}"
        for ln in named(run2, 'round')))
    if j.premise('premise_B_backlog', [f'{len(k["reports"])} not-retired reports in the case'] if k['reports'] else [],
                 'no not-retired report'):
        got = per[0][1] if per else 0
        if got != due:
            bad.append(f'round 1 of run2 said {got} retired, not {due}')
    j.holds('pin_B_steps', bad, 'server', f'at most {STEPS_MAX} a round; round 1 of run2 said {due}')


def judge_a_old(c, k, j):
    out = k['out'].get('publish', [])
    ab = named(out, 'abandoned')
    r1 = [ln.get('begun') for ln in named(out, 'round') if ln['round'] == 1]
    if not j.premise('premise_A-old_given_up', [] if ab and r1 and ab[0]['name'] == r1[0] and ab[0]['why'] == 'too-old'
                     else [f"the first abandoned line is not `abandoned {r1[0] if r1 else '?'} too-old` (round 1's "
                           f"begun): {[(a['name'], a['why'], a['round']) for a in ab]}"],
                     f"{ab[0]['name'] if ab else ''} given up as too old in round {ab[0]['round'] if ab else '-'}"):
        return
    judge_common('A-old', k, j)
    t1 = ab[0]['name']
    g = read_dump(c.text('given-up.txt'))['abandoned'].get(t1, {})
    left = g.get('left', '')
    j.holds('pin_A-old_left', [] if g.get('why') == 'too-old' and re.fullmatch(r'members:[1-9][0-9]*', left) else
            [f'the record had {t1} given up why={g.get("why", "-")} left={left or "-"}'], 'server',
            f'{t1} given up why=too-old left={left}')
    r = named(k['lines'], 'retired', t1)
    bad = [] if len(r) == 1 and r[0]['round'] > ab[0]['round'] else \
        [f"`retired {t1}` in rounds {', '.join(str(ln['round']) for ln in r) or 'none'}"]
    bad += [f'forgotten {t1} ({ln["why"]})' for ln in named(k['lines'], 'forgotten', t1)]
    j.holds('pin_A-old_retired', bad, 'server', f"{t1} retired in round {r[0]['round'] if r else '-'}")


def judge_a_person(c, k, j):
    cid = c.cid
    if not j.premise(f'premise_{cid}_killed', [] if killed(k['err'].get('publish', ''), KILLS[cid]) else
                     [f'no kill line for {KILLS[cid]} in publish.err'], f'killed at {KILLS[cid]}'):
        return
    t = c.text('names.txt').strip()
    if not j.premise(f'premise_{cid}_name', [] if t and t != '-' else ['no name before the kill'], t):
        return
    judge_common(cid, k, j)
    g = read_dump(c.text('given-up.txt'))['abandoned'].get(t, {})
    want = r'nothing' if cid == 'A-nothing' else r'members:[1-9][0-9]*'
    j.holds(f'pin_{cid}_left', [] if g.get('why') == 'person' and re.fullmatch(want, g.get('left', '')) else
            [f"the record had {t} given up why={g.get('why', '-')} left={g.get('left', '-')}"], 'server',
            f"{t} given up why=person left={g.get('left')}")
    run = k['out'].get('run', [])
    d = k['dump']
    inrec = [kind for kind in ('published', 'abandoned', 'stuck') if t in d[kind]]
    if cid == 'A-nothing':
        bad = [f"the run said: {ln['kind']} {t}" for ln in run if ln['name'] == t]
        bad += [f'the record keeps {t} among the names {kind}' for kind in inrec]
        bad += [f'{s} is on the pool' for s in at(k['snaps'], t)]
        j.holds('pin_A-nothing_dropped', bad, 'server', f'{t} dropped with no line')
    else:
        r = named(run, 'retired', t)
        bad = [] if len(r) == 1 else [f'`retired {t}` said {len(r)} times']
        bad += [f'forgotten {t} ({ln["why"]})' for ln in named(run, 'forgotten', t)]
        bad += [f'the record keeps {t} among the names {kind}' for kind in inrec]
        j.holds('pin_A-members_retired', bad, 'server', f"{t} retired in round {r[0]['round'] if r else '-'}")


JUDGES = {'K': judge_k, 'S': judge_s, 'X-retire': judge_x, 'X-retain': judge_x, 'X-destroy': judge_x,
          'B': judge_b, 'A-old': judge_a_old, 'A-nothing': judge_a_person, 'A-members': judge_a_person}


class Judge(agent_fail_check.Judge):
    def control(self, key, caught, what):
        """A negative control: what the judge said of a wrong input, which
        must be something."""
        if caught:
            self.put(key, 'CAUGHT', 'check', f'{what}: ' + '; '.join(caught))
        else:
            self.failed = True
            self.put(key, 'MISSED', 'check', f'{what}: the judge found nothing')


def judge_course(rdir, log, j):
    def text_of(path, need=True):
        p = os.path.join(rdir, path)
        if not os.path.exists(p):
            if need:
                raise RigError(f'no {path}')
            return ''
        with open(p, errors='replace') as f:
            return f.read()

    cdir = os.path.join(rdir, 'cases')
    for cid in sorted(os.listdir(cdir)) if os.path.isdir(cdir) else []:
        for ln in text_of(os.path.join('cases', cid, 'verdicts.tsv'), need=False).splitlines():
            pin, _, what = ln.partition('\t')
            if not re.fullmatch(r'[a-z][a-z_]*', pin) or not what:
                j.premise(f'premise_{cid}_verdicts', [f'cases/{cid}/verdicts.tsv: not pin, what: {ln!r}'], '')
                continue
            j.holds(f'pin_{cid}_{pin}', [what], 'server', '')
    for ln in text_of('rig-error.tsv', need=False).splitlines():
        c, _, m = ln.partition('\t')
        j.rc = max(j.rc, 2)
        j.put('rig_error_in_the_course', 'ERROR', 'rig', f'in case {c}: {m}; the cases before it are judged')
    with open(log, errors='replace') as f:
        instances = cut_daemon_check.parse_log(f.read())
    done = {ln.split('\t')[0] for ln in text_of('cases.tsv').splitlines() if ln}
    for cid in CASES:
        c = Case(rdir, cid)
        cerr = c.text('rig-error.txt', need=False).strip()
        if cerr:
            j.premise(f'premise_{cid}_rig', [f"the rig's own error in the case: {cerr}"], '')
            continue
        if not os.path.isdir(c.dir):
            j.put(f'case_{cid}', 'INFO', 'rig', 'not reached')
            continue
        if cid not in done:
            j.put(f'case_{cid}', 'INFO', 'rig', 'not ended (no line in cases.tsv): judged from what it left')
        try:
            k = read_case(c, instances)
            JUDGES[cid](c, k, j)
        except RigError as e:
            j.premise(f'premise_{cid}_files', [str(e)], '')
    agent_fail_check.judge_watcher(text_of('watch.log', need=False), j)


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) != 3:
        sys.exit(__doc__)
    name, rdir, log = argv
    t0 = time.monotonic()
    j = Judge(name)
    try:
        if name not in COURSES:
            raise RigError(f'not the retention course: {name}')
        judge_course(rdir, log, j)
    except RigError as e:
        j.rc = 2
        j.put('rig_error', 'ERROR', 'rig', str(e))
    except Exception as e:  # noqa: BLE001 -- anything else is the rig's too, not a verdict
        j.rc = 2
        j.put('rig_error', 'ERROR', 'rig', f'{type(e).__name__}: {e}')
    j.put('time', 'INFO', 'rig', f'{time.monotonic() - t0:.0f} s')
    with open(os.path.join(rdir, 'results.tsv'), 'a') as f:
        f.write(''.join(ln + '\n' for ln in j.lines))
    return j.status()


if __name__ == '__main__':
    sys.exit(main())
