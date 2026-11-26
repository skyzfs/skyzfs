#!/usr/bin/env python3
"""The agent rig's check (M2.T5 stage 1; M2.6's criterion on a real pool:
"after each loss, either snap/t is absent and a retry publishes it, or it
names a cut that holds the snapshot; a snapshot destroyed before the hold
is not published, and no hold is left behind after any loss").

Usage:
  agent_check.py NAME DIR LOG BASE DEV_BYTES [LOG_DEVICE_IMAGE]
  agent_check.py points TRACE [BASE_TRACE]

The first form is cut-record.sh's CUT_CHECK after the agent rig's courses
(aplain, adaemon, arare, arun; tests/kernel/agent-record.sh says what each
does), recorded through the daemon with its uploads (CUT_UPLOAD=1). LOG
is the daemon's request log; BASE is `-`. Environment: as
nodeb_check.py's. The volume in object storage is `cut-NAME`.

The second prints the points a trace reached (`--trace-points`), one
`EFFECT:SIDE:N` a line in the order reached, N its reach of that point
(from 1): the `--kill-at` of a case at each. With BASE_TRACE (a rare
scenario's trace against the plain one), the points from the first that
BASE_TRACE did not reach (by effect, side and reach) on, the steps they
share after it included (G1 of #158's review: a look, release or unlink
after a withdrawal is not the plain publish's), each at its first
NEW_REACHES reaches and its last: a look while the store is frozen is
reached again and again, and its last reach is the one after the thaw.

What each case left is in DIR/cases/ID (agent-record.sh), and its line
in DIR/cases.tsv. For each case:

- premise_ID_killed (rig): where the case kills the agent, its first run
  ended by SIGKILL (137) with the hook's line for that point on its
  standard error, and the trace's last point is that point at that
  reach. A point not reached is the rig's error (exit 2): the case did
  not test what it names. One exception: a later reach (N > 1) of a
  point that the run did not make, ending by itself with no hook line
  (how often an agent looks or syncs depends on time), is INFO
  (obs_ID_not_reached) and the case is judged as one with no kill.
  gone and both always kill (KILLED_ONLY): one of them judged with no
  kill is this premise's error.
- The scenario's own premises (rig): restart, the daemon killed with T
  not landed (node B's snap-row with the daemon down, between.txt);
  late, the agent stopped with T asked and not done (record-stopped.txt)
  and, once it went on, a `look` before `done` (it had not read the
  answer the lost daemon gave); dhook, the daemon killed by its hook
  (137) with the hook's line in its log (a missing kill line is the
  rig's error: the lead's ruling on #150); destroy and gone, the
  snapshot destroyed by the rig (destroy.log).
- pin_ID_criterion (kernel and server): T is published (node B has
  snap/T), or node B has no snap/T and the retry published (its `snap/`
  line, and node B has that name).
- pin_ID_outcome (server): T's outcome is the one EXPECTED for the
  scenario: `published`, or `abandoned WHY` in the agent's words, on its
  output or, for a run killed after it abandoned T, in its record; and
  `absent` for a case killed between the take and `taken` (MOVED) of a
  scenario that publishes T. Any other plain case with a kill, a case
  killed before T had a name, and one killed at intent:before:1 (T not
  yet recorded) are INFO (obs_ID_outcome): the criterion judges them.
- pin_ID_first_run (server): a case judged with no kill (none named,
  or a later reach not made) ended its first run as FIRST_RC says for
  its scenario: 0, 3 where T is abandoned (destroy, tooold); unmarked's
  own pin judges its 6.
- pin_ID_settle (server): the settle (the agent's next start) ended 3
  where its output gives a publish up (`abandoned T WHY`), else 0.
- pin_ID_clean (server): after the last run, the record has nothing
  under way and reads (the agent's dump, exit 0: a record that does not
  read is exit 6), no hold of the agent's tag is on any snapshot, the
  manifests directory is empty, and `skyzfs:assist` is set locally on
  the pool's root only.
- Case-specific: unmarked, the publish refused (exit 6) and released
  nothing; sweep, the publish that marks the directory ended 0
  (pin_ID_first_publish) and the stray hold released by the case's
  publish; both, the daemon ended by the rig's SIGKILL (137) and not
  before it (pin_ID_daemon_killed, both-kill.tsv). The
  carry hooks (the lead's ruling on #158): the request log's last ask of
  T that the daemon accepted (premise_ID_asked, the rig's), and the row's
  cut one that request accepts, counted after its C, of its G or later,
  opening as itself (pin_ID_carried_by_the_last_request): after the
  take-over the agent asks anew with a count read then, so the row may
  move later than the cut the hook killed at. Under carry-swap the row's
  cut is also no older than the killed cut, which had committed
  (pin_ID_no_older_than_the_killed_cut). The killed cut and the first
  cut the last request accepts are INFO (obs_ID_carried_by).

- pin_ID_stays_absent (kernel and server): where node B had no snap/T
  after the settle (snaprow.txt), it still has none when the check runs,
  after the uploads came to rest: a publish given up is never published
  by a later cut (G7). Where the retry published under T's own name (T
  not recorded, or its take destroyed, and the name free again), node B
  has the retry's publish under it and no other; T's outcome is then
  not `published`, and obs_ID_name_reused says so.
- pin_ID_moved (server): a case killed between the take and `taken`
  (MOVED): the settle published one name other than T, and one only,
  the publish going on after its take was destroyed, and POOL@T is not
  on node A after the last run (snapshots-after.txt) unless the retry
  took T's name again; T's outcome is `absent` where the scenario
  publishes T.
- pin_ID_name_not_given_up (server): where the retry published under
  T's own name, T was not given up first (`abandoned T` on a run's
  output or in the record): the engine never takes again a name it
  gave up.
- pin_ID_PIN (server): each verdict the rig wrote while it drove the
  case (DIR/cases/ID/verdicts.tsv: an agent that did not end, ask or
  withdraw in time, a row that did not land, a daemon that did not stop
  on SIGTERM or had ended by itself), for every case directory, listed
  in cases.tsv or not (D5), and DIR/cases/course, the daemon's stop at
  the course's end (pin_course_PIN, D10 of #159's review).
- premise_ID_rig (rig): the rig's own error in the case
  (DIR/cases/ID/rig-error.txt), or the check's own in judging it (a file
  of the case that does not read); the case is not judged further, and
  the next ones are (G6, D8).
- premise_ID_verdicts (rig): a line of the case's verdicts.tsv that is
  not PIN and WHAT; the other lines are still verdicts. The verdicts are
  read before anything else, so that no error of the rig hides one.

The rig's errors outside a case: rig_error_in_the_course (DIR/rig-error.tsv,
written where the recorder failed and ran this check on the cases done
before it) and premise_TRACE_points (DIR/points-error.tsv, a trace that
gave no points, or a rare scenario whose trace adds no point to the
plain trace's: no kill case of it ran), each an ERROR.

For each name published (DIR/published.tsv), node B: snap-row's ref and
row on one commit, the commit the agent printed (where it printed one),
the row's cut of the snapshot's createtxg or later and one that opens as
itself in the request log, its manifest object of the row's length and
hash and of the agent's format (pin_N_manifest: the snapshot and pool
named, the createtxg and the root's snapshot guid as node A read them);
then node B's stages (nodeb_check.NodeB.verify) on snap/N with the files
node A read through .zfs/snapshot/N.

Controls (once per course; each must be CAUGHT, or the run fails):
ctl_early, node B's stages on the first name's ref asked for a later
name's snapshot, which must be caught at `snapshot` (a publish one cut
early); ctl_files, a digest changed, caught at `files`; ctl_manifest, the
manifest judged against another guid.

obs_points (INFO): the effects the course's traces reached, and those of
the engine's (crates/skyzfs-agent/src/engine.rs) none reached.

pin_published_snapshots_exist (kernel and server): no name was published
whose snapshot node A does not have (DIR/published-gone.tsv: the
recorder found node B's snap/N, or the agent's `snap/N`, and no pool@N).
Such a name counts as published for its case's outcome.

results.tsv gets the lines (KEY, VERDICT, DETAIL), which cut_summary.py
prints. Exit 0: every line holds. Exit 1: a verdict, whatever else
happened: a FAILED line is never hidden behind an error of the rig.
Exit 2: the rig's own error with no line FAILED, which is also what any
exception ends in.
"""
import json
import os
import re
import shutil
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_daemon_check  # noqa: E402
import nodeb_check  # noqa: E402
import publish_check  # noqa: E402
from cut_check import RigError  # noqa: E402

COURSES = ('aplain', 'adaemon', 'arare', 'arun')
POOL = nodeb_check.POOL
HOLD_TAG = 'skyzfs-publish'
ENGINE = os.path.join(HERE, '..', '..', 'crates', 'skyzfs-agent', 'src', 'engine.rs')


def effects_of(text):
    """engine.rs's text -> the words of its effects, in Effect::ALL's order
    (`Effect::word`): the agent the rig runs is built from the same
    checkout, so its words are the check's (#153 changed them)."""
    at = text.find('pub fn word(self)')
    end = text.find('\n    }\n', at)
    words = tuple(re.findall(r'=> "([a-z][a-z-]*)"', text[at:end])) if at >= 0 and end >= 0 else ()
    if len(words) < 10 or len(set(words)) != len(words):
        raise RigError('no list of the engine\'s effects in its Effect::word')
    return words


def engine_effects():
    try:
        with open(ENGINE) as f:
            return effects_of(f.read())
    except OSError as e:
        raise RigError(f'the engine\'s effects: {e}') from e


EFFECTS = ()
SIDES = ('before', 'after')
KILL_LINE = re.compile(r'skyzfs-agent: test hook --kill-at: killed at ([a-z][a-z-]*) (before|after) \((\d+)\)')
# The daemon's hooks (bins/skyzfsd/src/kill.rs): what its kill line says.
DAEMON_KILLED = {
    'carry-put:1': re.compile(r'killed carry-put:1: after the first segment PUT of cut (\d+) \(group (\d+)\)'),
    'carry-swap:1': re.compile(r'killed carry-swap:1: after the commit of cut (\d+) \(group (\d+)\)'),
    'manifest:1': re.compile(r'killed after the framed manifest PUT 1 landed'),
    'publish:1': re.compile(r'killed after the transaction of publish 1 \('),
}
# The scenario -> the first run's status where no kill fired (run
# 45751597306's cases). gone and both always kill (KILLED_ONLY): one of
# them with no kill is the rig's error. unmarked has its own pin
# (pin_ID_refused).
KILLED_ONLY = ('gone', 'both', 'runkill')
FIRST_RC = {
    'plain': '0',
    'restart': '0',
    'assist': '0',
    'destroy': '3',
    'tooold': '3',
    'sweep': '0',
    'dhook': '0',
    'late': '0',
    'run': '0',
    'runtwice': '1',
    'runheld': '0',
    'runbad': '6',
}
# The scenario -> T's outcome where the scenario's premise holds and the
# case has no kill (expected: a kill leaves some to the criterion alone).
# A plain case with no kill must publish.
EXPECTED = {
    'plain': 'published',
    'restart': 'published',
    'assist': 'published',
    'destroy': 'abandoned destroyed',
    'tooold': 'abandoned too-old',
    'sweep': 'published',
    'gone': 'abandoned destroyed',
    'unmarked': 'absent',
    'dhook': 'published',
    'late': 'published',
    'both': 'published',
    'run': 'published',
    'runkill': 'published',
    'runtwice': 'published',
    'runheld': 'published',
    'runbad': 'published',
}
# `skyzfs-agent run` (arun): the rig's --every-s, and how long after
# the rig's SIGTERM (between two rounds) or its two signals the run may
# end, on the rig's clock: run's WAKE (0.25 s) and the rig's own looks
# (every 0.25 s) and writes. A round begins on its tick within
# TICK_SLACK s on the rig's clock, which reads each round line within
# 0.1 s of its arrival.
RUN_EVERY = 10
END_WITHIN = 2.0
TICK_SLACK = 1.0


def flat(text):
    return ' '.join(str(text).split())


def read_trace(text):
    """--trace-points' lines -> [(ns, effect, side, name)]."""
    out = []
    for ln in text.splitlines():
        f = ln.split('\t')
        if len(f) != 4 or not f[0].isdigit() or f[1] not in EFFECTS or f[2] not in SIDES:
            raise RigError(f'a trace line that is not ns, effect, side, name: {ln!r}')
        out.append((int(f[0]), f[1], f[2], f[3]))
    return out


def points(trace):
    """A trace -> ['EFFECT:SIDE:N'], N the reach of that point."""
    seen, out = {}, []
    for _, e, s, _ in trace:
        seen[(e, s)] = seen.get((e, s), 0) + 1
        out.append(f'{e}:{s}:{seen[(e, s)]}')
    return out


NEW_REACHES = 3


def new_points(trace, base):
    """The points of TRACE from the first BASE did not reach on, in
    TRACE's order, each at its first NEW_REACHES reaches and its last."""
    had = set(points(base))
    pts = points(trace)
    first = next((i for i, p in enumerate(pts) if p not in had), len(pts))
    last = {}
    for p in pts:
        e, s, n = p.split(':')
        last[(e, s)] = int(n)
    out = []
    for p in pts[first:]:
        e, s, n = p.split(':')
        if int(n) <= NEW_REACHES or int(n) == last[(e, s)]:
            out.append(p)
    return out


def parse_kill(kill):
    """'EFFECT:SIDE:N' -> (effect, side, n)."""
    f = kill.split(':')
    if len(f) != 3 or f[0] not in EFFECTS or f[1] not in SIDES or not f[2].isdigit() or int(f[2]) < 1:
        raise RigError(f'not a kill point: {kill!r}')
    return f[0], f[1], int(f[2])


def killed_at(kill, rc, err, trace):
    """Whether the run was killed where the case names -> [what is not so]."""
    e, s, n = parse_kill(kill)
    bad = []
    if rc != '137':
        bad.append(f'the run ended {rc}, not 137 (SIGKILL)')
    lines = KILL_LINE.findall(err)
    if lines != [(e, s, str(n))]:
        bad.append(f'the hook\'s lines say {lines or "nothing"}, not {e} {s} ({n})')
    if not trace:
        bad.append('the trace is empty')
    else:
        last = trace[-1]
        reach = sum(1 for _, e2, s2, _ in trace if (e2, s2) == (e, s))
        if (last[1], last[2]) != (e, s) or reach != n:
            bad.append(f'the trace ends at {last[1]} {last[2]} with {reach} reaches of {e} {s}, not reach {n}')
    return bad


def not_reached(kill, rc, err, trace):
    """Whether a kill at a later reach (N > 1) of a point was not reached:
    the run ended by itself (no SIGKILL, no hook line) with fewer than N
    reaches of it. How many looks or syncs a publish makes before an
    answer depends on time (run 45747041657: a destroy case's agent looked
    once, not twice), so such a case is judged as one with no kill and
    says so; a first reach not made is still the rig's error."""
    e, s, n = parse_kill(kill)
    reach = sum(1 for _, e2, s2, _ in trace if (e2, s2) == (e, s))
    return n > 1 and rc != '137' and not KILL_LINE.findall(err) and reach < n


ROUND_LINE = re.compile(r'round ([1-9][0-9]*) (ok|failed) took_ms=([0-9]+) late_ms=([0-9]+) skipped=([0-9]+) '
                        r'begun=(\S+) under_way=([0-9]+)')


def read_outputs(text):
    """The agent's standard output -> {'published': {t: commit}, 'abandoned': {t: why}, 'stopped': {t: why},
    'unsettled': [t], 'rounds': [round]}: `run`'s round lines are its
    fifth kind, each {'n', 'ok', 'took_ms', 'late_ms', 'skipped',
    'begun' (None for -), 'under_way'}, in order."""
    out = {'published': {}, 'abandoned': {}, 'stopped': {}, 'unsettled': [], 'rounds': []}
    for ln in text.splitlines():
        f = ln.split()
        r = ROUND_LINE.fullmatch(ln)
        if r:
            out['rounds'].append({'n': int(r[1]), 'ok': r[2] == 'ok', 'took_ms': int(r[3]), 'late_ms': int(r[4]),
                                  'skipped': int(r[5]), 'begun': None if r[6] == '-' else r[6],
                                  'under_way': int(r[7])})
        elif len(f) == 2 and f[0].startswith('snap/'):
            out['published'][f[0][len('snap/'):]] = f[1]
        elif len(f) == 3 and f[0] == 'abandoned':
            out['abandoned'][f[1]] = f[2]
        elif len(f) >= 3 and f[0] == 'stopped':
            out['stopped'][f[1]] = ' '.join(f[2:])
        elif len(f) == 2 and f[0] == 'unsettled':
            out['unsettled'].append(f[1])
        elif ln.strip():
            raise RigError(f'a line of the agent\'s output that is not one of its five: {ln!r}')
    return out


def record_abandoned(record):
    """A record's dump (`skyzfs-agent record`) -> {t: why} of its lines
    `abandoned T why=W at_ms=MS`."""
    out = {}
    for ln in record.splitlines():
        f = ln.split()
        if len(f) >= 3 and f[0] == 'abandoned' and f[2].startswith('why='):
            out[f[1]] = f[2][len('why='):]
    return out


def outcome(t, outs, published, record=''):
    """T's outcome: 'published' where node B has it (published), else the
    agent's last `abandoned T WHY` on its output, else the record's (a run
    killed after it abandoned T printed nothing), else 'absent'."""
    if t and t in published:
        return 'published'
    why = None
    for o in outs:
        if t in o['abandoned']:
            why = o['abandoned'][t]
    if t and not why:
        why = record_abandoned(record).get(t)
    return f'abandoned {why}' if why else 'absent'


# Kill points from the take to `taken` (the take exists, Phase::Taken is
# not recorded): the next start finds pool@T at Phase::Intent, cannot
# adopt it (what the pool was before the take is not kept across a
# restart), destroys it and publishes under a new name (engine.rs, step
# 5). T is then absent, and the settle publishes another name. Not
# destroy:after: that take is gone already and the record still holds T
# at Phase::Intent, so the next start takes T again under the same name
# (a_crash_around_the_destroy_takes_again in engine/tests.rs; D11 of
# #159's review).
MOVED = {('take', 'after'), ('failed', 'before'), ('failed', 'after'), ('destroy', 'before'),
         ('manifest', 'before'), ('manifest', 'after'), ('taken', 'before')}


def moved(kill):
    """Whether a kill at KILL ('EFFECT:SIDE:N') leaves a take that the
    next start destroys (MOVED)."""
    return bool(kill) and tuple(kill.split(':')[:2]) in MOVED


def expected(scen, kill, destroyed, t):
    """The outcome EXPECTED for a case, or None where the criterion
    alone judges it: a plain publish killed, a case killed before its
    publish had a name or before T was recorded (intent:before:1: a later
    publish may take the same name), a destroy that did not happen. A
    publish that must publish T, killed between the take and `taken`:
    'absent' (MOVED)."""
    if kill and (not t or kill == 'intent:before:1'):
        return None
    if moved(kill) and EXPECTED[scen] == 'published':
        return 'absent'
    if scen == 'plain':
        return None if kill else EXPECTED['plain']
    if scen in ('destroy', 'gone') and not destroyed:
        return None
    return EXPECTED[scen]


def clean(record, holds, manifests, assist, record_rc='0', record_err=''):
    """After the last run -> [what is left]. RECORD_RC and RECORD_ERR:
    the dump's status and standard error."""
    bad = []
    if record_rc.strip() != '0':
        bad.append(f'the record does not read: `skyzfs-agent record` ended {record_rc.strip() or "with no status"}: '
                   f'{flat(record_err)[-200:]}')
    under = [ln.split()[1] for ln in record.splitlines() if ln.startswith('publish ') and len(ln.split()) > 1]
    if under:
        bad.append(f'under way in the record: {", ".join(under)}')
    if not record.strip():
        bad.append('the record\'s dump is empty')
    held = [ln.split('\t')[0] for ln in holds.splitlines() if len(ln.split('\t')) >= 2 and ln.split('\t')[1] == HOLD_TAG]
    if held:
        bad.append(f'holds of {HOLD_TAG}: {", ".join(held)}')
    left = [ln for ln in manifests.splitlines() if ln.strip()]
    if left:
        bad.append(f'manifests left: {", ".join(left)}')
    for ln in assist.splitlines():
        f = ln.split('\t')
        if len(f) != 3:
            raise RigError(f'assist.txt: not name, value, source: {ln!r}')
        if f[2] == 'local' and f[0] != POOL:
            bad.append(f'skyzfs:assist set locally on {f[0]}')
    return bad


def judge_manifest_json(data, name, createtxg, guid):
    """The manifest object's bytes against node A's facts -> [what is wrong]."""
    try:
        m = json.loads(data)
    except (ValueError, UnicodeDecodeError) as e:
        return [f'the manifest is not JSON: {e}']
    if not isinstance(m, dict):
        return ['the manifest is not an object']
    bad = []
    for k, want in (('format', 'skyzfs-manifest'), ('version', 1), ('pool', POOL), ('snapshot', name),
                    ('createtxg', createtxg)):
        if m.get(k) != want:
            bad.append(f'{k} is {m.get(k)!r}, not {want!r}')
    members = m.get('members')
    if not isinstance(members, list) or not members or not isinstance(members[0], dict):
        bad.append('no members')
    else:
        root = members[0]
        if root.get('name') != POOL:
            bad.append(f"the first member is {root.get('name')!r}, not the pool's root")
        if root.get('snapshot_guid') != str(guid):
            bad.append(f"the root's snapshot_guid is {root.get('snapshot_guid')!r}, node A's {guid}")
    return bad


def daemon_kill(hook, kill_tsv, log):
    """dhook's premise -> (bad, the cut the hook named (number, group) or None)."""
    f = dict(ln.split('\t', 1) for ln in kill_tsv.splitlines() if '\t' in ln)
    bad = []
    if f.get('killed_rc') != '137':
        bad.append(f"the daemon ended {f.get('killed_rc', 'nothing')}, not 137 (its hook's SIGKILL)")
    m = DAEMON_KILLED[hook].search(log)
    if not m:
        bad.append(f'no kill line of {hook} in the killed instance\'s log')
    cut = (int(m.group(1)), int(m.group(2))) if m and m.groups() else None
    return bad, cut


def last_ask(instances, name):
    """The request log's last ask of NAME that the daemon accepted ->
    (ns, G, C), or None. skyzfsd writes G in the line's sector column and
    C in its sectors column (record.rs; publish_check.asked_line)."""
    last = None
    for inst in instances:
        for ln in inst['lines']:
            if (ln['event'] == 'publish' and ln['op'] == 'asked' and ln['result'] == f'{name} accepted'
                    and ln['sector'].isdigit() and ln['sectors'].isdigit()):
                last = (ln['ns'], int(ln['sector']), int(ln['sectors']))
    return last


def judge_carried(cuts, row, ask):
    """The lead's ruling on #158, (b): the row's cut is one the last
    accepted request accepts: counted after its C, of its G or later, and
    one that opens as itself in the request log -> [what is not so]. After
    a take-over the agent asks anew with a count read then (rule (R)), so
    the cut a hook killed at may be counted before the new C."""
    _, g, c = ask
    bad = []
    if row['number'] <= c:
        bad.append(f"the row's cut {row['number']} is not counted after C {c} of the last request accepted")
    if row['group'] < g:
        bad.append(f"the row's cut is of group {row['group']}, below G {g} of the last request accepted")
    return bad + publish_check.judge_cut(cuts, row)


def first_carrier(cuts, ask):
    """The first cut, by number, that the request ASK accepts and that
    opens as itself -> (number, group), or None: INFO, since a cut taken
    before the acceptance may carry it and one after may be abandoned."""
    _, g, c = ask
    ok = sorted((n, gr) for n, gr, itself, _, _ in cuts if n > c and gr >= g and itself)
    return ok[0] if ok else None


def late_premise(stopped, t, trace, continued):
    """d6's premise -> [what is not so]: T asked and not done when the agent
    was stopped, and a `look` of the agent's after it went on, before its
    `done`."""
    bad = []
    phase = next((w for ln in stopped.splitlines() if ln.startswith(f'publish {t} ')
                  for w in ln.split() if w.startswith('phase=')), None)
    if phase != 'phase=asked':
        bad.append(f'when stopped, the record had {t} at {phase or "nothing"}, not asked')
    after = [(e, s) for ns, e, s, _ in trace if ns >= continued]
    look = next((k for k, p in enumerate(after) if p[0] == 'look'), None)
    done = next((k for k, p in enumerate(after) if p == ('done', 'before')), None)
    if look is None or (done is not None and done < look):
        bad.append('after it went on, no look before done: it had the answer before the daemon was lost')
    return bad


def read_arrivals(text):
    """LABEL.rounds.tsv -> [seconds]: the rig's time at which each round
    line came, in order (the line's number, the time)."""
    out = []
    for k, ln in enumerate(text.splitlines(), 1):
        f = ln.split('\t')
        try:
            ok = len(f) == 2 and int(f[0]) == k
            at = float(f[1]) if ok else 0.0
        except ValueError:
            ok = False
        if not ok:
            raise RigError(f'rounds.tsv: line {k} is not {k}, seconds: {ln!r}')
        out.append(at)
    return out


def judge_rounds(rounds, arrivals, want, every=RUN_EVERY):
    """`run`'s round lines and the rig's arrival times of them -> [what is
    not so]: at least WANT rounds, numbered from 1, each ok, each
    beginning a publish of a name of its own; a round that ended before
    the next tick skips none, and the next begins within TICK_SLACK s
    of that tick; and on the rig's clock each round begins (its line's
    arrival less its took_ms) EVERY s times the ticks it took after the
    one before, give or take the rounds' own late_ms and TICK_SLACK."""
    bad = []
    if len(rounds) < want:
        bad.append(f'{len(rounds)} rounds, not {want} or more')
    nums = [r['n'] for r in rounds]
    if nums != list(range(1, len(rounds) + 1)):
        bad.append(f'the rounds are numbered {nums}, not from 1 on')
    failed = [r['n'] for r in rounds if not r['ok']]
    if failed:
        bad.append(f'round(s) {failed} failed')
    names = [r['begun'] for r in rounds]
    if None in names:
        bad.append(f'round(s) {[r["n"] for r in rounds if r["begun"] is None]} began no publish')
    given = [n for n in names if n is not None]
    if len(set(given)) != len(given):
        bad.append(f'a name begun twice: {given}')
    if len(arrivals) < len(rounds):
        bad.append(f'the rig saw {len(arrivals)} of the {len(rounds)} round lines come')
        return bad
    began = [a - r['took_ms'] / 1000 for a, r in zip(arrivals, rounds)]
    for a, b, ta, tb in zip(rounds, rounds[1:], began, began[1:]):
        if a['late_ms'] + a['took_ms'] < every * 1000:
            if a['skipped']:
                bad.append(f"round {a['n']} ended before its next tick and says it skipped {a['skipped']}")
            if b['late_ms'] > TICK_SLACK * 1000:
                bad.append(f"round {b['n']} began {b['late_ms']} ms after its tick, round {a['n']} having ended "
                           'before it')
        want_gap = every * (a['skipped'] + 1) + (b['late_ms'] - a['late_ms']) / 1000
        if abs((tb - ta) - want_gap) > TICK_SLACK:
            bad.append(f"round {b['n']} began {tb - ta:.2f} s after round {a['n']} on the rig's clock, not "
                       f'{want_gap:.2f} s')
    return bad


def ended_within(term, ended, limit=END_WITHIN):
    """LABEL.term-at and LABEL.ended-at -> [what is not so]: the run ended
    within LIMIT s of the rig's signal."""
    try:
        gap = float(ended) - float(term)
    except ValueError:
        raise RigError(f'term-at, ended-at: not times: {term!r}, {ended!r}') from None
    return [] if gap <= limit else [f'it ended {gap:.2f} s after the signal, not within {limit} s']


RUN_SCENARIOS = ('run', 'runkill', 'runtwice', 'runheld', 'runbad')


def judge_run(cid, cd, scen, outs, holds, premise, text):
    """The pins of a case of `skyzfs-agent run` (agent-record.sh's run
    scenarios); TEXT reads a file of the run's directory, '' where there
    is none. The first run's status is FIRST_RC's pin (runkill's is the
    kill's premise)."""
    def at(path):
        return os.path.join(cd, path)

    def ended(label, what):
        term, end = text(at(f'{label}.term-at')).strip(), text(at(f'{label}.ended-at')).strip()
        if term and end:
            holds(f'pin_{cid}_ended_on_{what}', ended_within(term, end), 'server',
                  f'it ended within {END_WITHIN} s of the {what.replace("_", " ")}')

    rounds = outs.get('publish', {}).get('rounds', [])
    if scen in ('run', 'runheld'):
        holds(f'pin_{cid}_rounds', judge_rounds(rounds, read_arrivals(text(at('publish.rounds.tsv'))), 3), 'server',
              f'{len(rounds)} rounds, every {RUN_EVERY} s on the rig\'s clock, each ok and beginning its own publish')
        ended('publish', 'sigterm')
    if scen == 'runheld':
        before = text(at('held.rounds')).strip()
        hrc, src = text(at('held.rc')).strip(), text(at('held-settle.rc')).strip()
        bad = ([] if hrc == '8' else [f'recover beside the run ended {hrc or "with no status"}, not 8: '
                                      f'{flat(text(at("held.err")))[-200:]}'])
        bad += ([] if src == '8' else [f'settle beside the run ended {src or "with no status"}, not 8: '
                                       f'{flat(text(at("held-settle.err")))[-200:]}'])
        bad += ([f'recover printed {flat(text(at("held.out")))[:200]!r}'] if text(at('held.out')).strip() else [])
        if not before.isdigit() or int(before) >= len(rounds):
            bad.append(f'no round line after the refusals ({before or "no count"} before them, {len(rounds)} in all)')
        holds(f'pin_{cid}_held', bad, 'server',
              f'recover and settle beside the run ended 8; the run went on to round {len(rounds)}')
    if scen == 'runkill':
        r2 = outs.get('run2', {}).get('rounds', [])
        rrc = text(at('run2.rc')).strip()
        holds(f'pin_{cid}_restarted',
              ([] if rrc == '0' else [f'the run started again ended {rrc or "with no status"}, not 0: '
                                      f'{flat(text(at("run2.err")))[-200:]}'])
              + judge_rounds(r2, read_arrivals(text(at('run2.rounds.tsv'))), 2), 'server',
              f'a run started again in the same directory: {len(r2)} rounds, ended {rrc} on SIGTERM')
        ended('run2', 'sigterm')
    if scen == 'runtwice':
        ended('publish', 'second_signal')
    if scen == 'runbad':
        kind = text(at('bad.kind')).strip()
        if kind not in ('corrupt', 'volume'):
            raise RigError(f'bad.kind: {kind!r}, not corrupt or volume')
        if kind == 'volume':
            orc = text(at('other.rc')).strip()
            premise(f'premise_{cid}_other_record', [] if orc == '1' else
                    [f'the publish of another volume ended {orc or "with no status"}, not 1 (no daemon there)'],
                    'a record of another volume, saved by the agent itself')
        before = text(at('bad.rounds')).strip()
        last = text(at('publish.out')).splitlines()[-1:] or ['']
        r = ROUND_LINE.fullmatch(last[0])
        bad = []
        if not before.isdigit():
            raise RigError(f'bad.rounds: {before!r}, not a count')
        n = int(before) + 1
        if not r or int(r[1]) != n or r[2] != 'failed':
            bad.append(f'the last line is {last[0]!r}, not round {n} failed')
        words = 'the record is corrupt' if kind == 'corrupt' else 'the record is of volume '
        if f'skyzfs-agent: round {n}: {words}' not in text(at('publish.err')):
            bad.append(f'standard error does not say "round {n}: {words}": {flat(text(at("publish.err")))[-200:]}')
        holds(f'pin_{cid}_record_judged', bad, 'server',
              f'the record {"made corrupt" if kind == "corrupt" else "of another volume"} met at round {n}\'s '
              'restart: the round\'s line, failed, was the last; then exit 6')


def main_points(argv):
    global EFFECTS
    try:
        EFFECTS = engine_effects()
        with open(argv[0]) as f:
            trace = read_trace(f.read())
        if len(argv) > 1:
            with open(argv[1]) as f:
                base = read_trace(f.read())
            out = new_points(trace, base)
        else:
            out = points(trace)
    except (RigError, OSError) as e:
        print(f'agent_check.py points: {e}', file=sys.stderr)
        sys.exit(2)
    # A trace with no line reached no point, with a base or not: the
    # scenario's cases would be dropped with no word.
    if not trace or (not out and len(argv) == 1):
        print('agent_check.py points: the trace reached no point', file=sys.stderr)
        sys.exit(2)
    print('\n'.join(out))


def main():
    if len(sys.argv) >= 2 and sys.argv[1] == 'points' and len(sys.argv) in (3, 4):
        main_points(sys.argv[2:])
        return
    if len(sys.argv) not in (6, 7):
        sys.exit(__doc__)
    name, rdir, log = sys.argv[1:4]
    t0 = time.monotonic()
    lines = []
    rc = 0
    any_failed = False

    def put(key, verdict, rests, detail):
        lines.append(f'{key}\t{verdict}\t{flat(f"[{rests}] {detail}")}')
        print(f'{name}: {key}: {verdict}: [{rests}] {flat(detail)}', flush=True)

    def holds(key, bad, rests, good):
        nonlocal rc, any_failed
        if bad:
            rc = max(rc, 1)
            any_failed = True
            put(key, 'FAILED', rests, '; '.join(bad))
        else:
            put(key, 'ok', rests, good)

    def premise(key, bad, good):
        """A case's premise: an ERROR line and exit 2, and the other cases
        still judged."""
        nonlocal rc
        if bad:
            rc = max(rc, 2)
            put(key, 'ERROR', 'rig', '; '.join(bad))
        else:
            put(key, 'ok', 'rig', good)
        return not bad

    def text_of(path, need=True):
        p = os.path.join(rdir, path)
        if not os.path.exists(p):
            if need:
                raise RigError(f'no {path}')
            return ''
        with open(p, errors='replace') as f:
            return f.read()

    global EFFECTS
    try:
        EFFECTS = engine_effects()
        if name not in COURSES:
            raise RigError(f'not an agent course: {name}')
        # The verdicts the recorder wrote while it drove the cases come
        # first: no error of the rig read after them hides one.
        cdir = os.path.join(rdir, 'cases')
        for cid in sorted(os.listdir(cdir)) if os.path.isdir(cdir) else []:
            for ln in text_of(os.path.join('cases', cid, 'verdicts.tsv'), need=False).splitlines():
                pin, _, what = ln.partition('\t')
                if not re.fullmatch(r'[a-z][a-z_]*', pin) or not what:
                    premise(f'premise_{cid}_verdicts', [f'cases/{cid}/verdicts.tsv: not pin, what: {ln!r}'], '')
                    continue
                holds(f'pin_{cid}_{pin}', [what], 'server', '')
        nb = nodeb_check.NodeB(rdir, None)
        vol = f'cut-{name}'
        torn = []
        with open(log, errors='replace') as f:
            instances = cut_daemon_check.parse_log(f.read(), torn)
        cuts = publish_check.cut_lines(instances)
        put('obs_cuts', 'INFO', 'server', f'{len(cuts)} cut lines in {len(instances)} instances'
            + (f', {len(torn)} line(s) cut short by a kill' if torn else ''))
        watch = text_of('watch.log').splitlines()
        if not watch:
            raise RigError('the watcher wrote no line to watch.log')
        err = [ln.split('\t', 1)[-1] for ln in watch if '\tERROR ' in ln]
        if err:
            raise RigError(f'the watcher: {err[0]}')
        bad = [ln.split('\tVERDICT ', 1)[1] for ln in watch if '\tVERDICT ' in ln]
        holds('pin_watcher_reads', bad, 'kernel and server',
              f'node B read the content of every cut the watcher found at head ({len(watch)} lines)')
        marks = dict((m, ns) for ns, m in cut_daemon_check.read_marks(text_of('marks.tsv')))
        pub = {}
        for ln in text_of('published.tsv').splitlines():
            f = ln.split('\t')
            if len(f) != 5 or not f[2].isdigit() or not f[3].isdigit():
                raise RigError(f'published.tsv: not case, name, createtxg, guid, commit: {ln!r}')
            pub[f[1]] = {'case': f[0], 'name': f[1], 'txg': int(f[2]), 'guid': int(f[3]), 'printed': f[4]}
        gone = {}
        for ln in text_of('published-gone.tsv').splitlines():
            f = ln.split('\t')
            if len(f) != 2 or not f[1]:
                raise RigError(f'published-gone.tsv: not case, name: {ln!r}')
            gone[f[1]] = f[0]
        holds('pin_published_snapshots_exist',
              [f'{n} (case {c}) is published and {POOL}@{n} is not on node A' for n, c in gone.items()],
              'kernel and server', 'every name published is a snapshot on node A')
        for ln in text_of('rig-error.tsv', need=False).splitlines():
            c, _, m = ln.partition('\t')
            rc = max(rc, 2)
            put('rig_error_in_the_course', 'ERROR', 'rig', f'in case {c}: {m}; the cases before it are judged')
        for ln in text_of('points-error.tsv', need=False).splitlines():
            c, _, m = ln.partition('\t')
            premise(f'premise_{c}_points', [f'no points from the trace of case {c}: {m}'], '')
        cases = [ln.split('\t') for ln in text_of('cases.tsv').splitlines() if ln]
        if not cases:
            raise RigError('no case in cases.tsv')
        reached = set()
        rows = {}
        for c in cases:
            if len(c) != 5:
                raise RigError(f'cases.tsv: not id, scenario, kill, status, name: {c!r}')
            cid = c[0]
            try:
                cid, scen, kill, first_rc, t = c
                kill = '' if kill == '-' else kill
                t = '' if t == '-' else t
                if scen not in EXPECTED:
                    raise RigError(f'case {cid}: no scenario {scen}')
                cd = os.path.join('cases', cid)
                cerr = text_of(os.path.join(cd, 'rig-error.txt'), need=False).strip()
                if cerr:
                    premise(f'premise_{cid}_rig', [f"the rig's own error in the case: {cerr}"], '')
                    put(f'case_{cid}', 'INFO', 'rig', f'{scen}{" killed at " + kill if kill else ""}: not judged')
                    continue
                trace = read_trace(text_of(os.path.join(cd, 'trace.tsv'), need=False))
                reached.update(e for _, e, _, _ in trace)
                outs = {}
                for run in ('publish', 'first', 'run2', 'settle-frozen', 'settle', 'retry'):
                    txt = text_of(os.path.join(cd, f'{run}.out'), need=False)
                    if os.path.exists(os.path.join(rdir, cd, f'{run}.out')):
                        outs[run] = read_outputs(txt)
                named = kill
                perr = text_of(os.path.join(cd, 'publish.err'), need=False)
                if kill and not_reached(kill, first_rc, perr, trace):
                    put(f'obs_{cid}_not_reached', 'INFO', 'rig',
                        f'the run ended {first_rc} before reach {parse_kill(kill)[2]} of {":".join(kill.split(":")[:2])}: '
                        'judged as a case with no kill')
                    kill = ''
                if kill:
                    premise(f'premise_{cid}_killed', killed_at(kill, first_rc, perr, trace),
                            f'killed at {kill}, as the trace and the hook\'s line say')
                elif scen in KILLED_ONLY:
                    premise(f'premise_{cid}_killed', [f'a {scen} case with no kill: the run ended {first_rc} '
                                                      'before its kill'], '')
                elif scen in FIRST_RC:
                    holds(f'pin_{cid}_first_run', [] if first_rc == FIRST_RC[scen]
                          else [f'the first run ended {first_rc}, not {FIRST_RC[scen]}: {flat(perr)[-200:]}'],
                          'server', f'the first run ended {first_rc}, as a {scen} case with no kill does')
                destroyed = os.path.exists(os.path.join(rdir, cd, 'destroy.log'))
                # A case with no kill must have done what it names (a case
                # killed early may not have, and is the kill's to judge).
                if scen == 'restart' and not kill and not os.path.exists(os.path.join(rdir, cd, 'between.txt')):
                    premise(f'premise_{cid}_pending_at_the_loss', ['the agent did not reach asked: the daemon was not '
                                                                   'restarted with a publish pending'], '')
                if scen == 'assist' and not kill:
                    premise(f'premise_{cid}_assisted', [] if any(e == 'assist' for _, e, _, _ in trace)
                            else ['the trace has no assist point'], 'the agent assisted while the store was frozen')
                if scen == 'restart' and os.path.exists(os.path.join(rdir, cd, 'between.txt')):
                    got = publish_check.read_snap_row(text_of(os.path.join(cd, 'between.txt')))
                    premise(f'premise_{cid}_pending_at_the_loss', ['snap/T had landed before the daemon was killed']
                            if got['ref'] is not None else [], f'no snap/{t} while the daemon was down: it was pending')
                if scen == 'late':
                    cont = marks.get(f'continued-{cid}')
                    premise(f'premise_{cid}_answer_lost',
                            late_premise(text_of(os.path.join(cd, 'record-stopped.txt')), t, trace, cont or 0)
                            + ([] if text_of(os.path.join(cd, 'record-stopped.rc')).strip() == '0'
                               else ['the record did not read when the agent was stopped'])
                            if cont else ['no mark continued-' + cid],
                            f'{t} asked and not done when the agent stopped; it looked after it went on')
                if scen == 'dhook':
                    hook = cid[2:] + ':1'
                    if hook not in DAEMON_KILLED:
                        raise RigError(f'case {cid}: no daemon hook {hook}')
                    kbad, kcut = daemon_kill(hook, text_of(os.path.join(cd, 'kill.tsv')),
                                             text_of(os.path.join(cd, 'serve-killed.log')))
                    premise(f'premise_{cid}_daemon_killed', kbad,
                            f'the daemon killed itself at {hook}' + (f', at cut {kcut[0]} of group {kcut[1]}' if kcut else ''))
                    rows[cid] = (hook, kcut)
                if scen in ('destroy', 'gone') and kill == '' and not destroyed:
                    premise(f'premise_{cid}_destroyed', ['the rig did not destroy the snapshot'], '')
                if scen == 'unmarked':
                    stray = text_of(os.path.join(cd, 'stray-holds.txt'))
                    holds(f'pin_{cid}_refused', ([f'the publish ended {first_rc}, not 6'] if first_rc != '6' else [])
                          + ([] if HOLD_TAG in stray else ['the stray hold was released'])
                          + ([] if text_of(os.path.join(cd, 'record-before.txt')).strip() == 'absent'
                             and text_of(os.path.join(cd, 'record-before.rc')).strip() == '0'
                             else ['the refused run wrote a record']),
                          'server', 'a fresh directory with a hold of the tag: exit 6, nothing released, no record written')
                if scen == 'sweep':
                    frc = text_of(os.path.join(cd, 'first.rc')).strip()
                    holds(f'pin_{cid}_first_publish', [] if frc == '0' else
                          [f'the publish that marks the directory ended {frc or "with no status"}: '
                           f'{flat(text_of(os.path.join(cd, "first.err"), need=False))[-200:]}'],
                          'server', 'the publish that marks the directory ended 0')
                if scen == 'both':
                    kt = dict(ln.split('\t', 1) for ln in text_of(os.path.join(cd, 'both-kill.tsv')).splitlines()
                              if '\t' in ln)
                    krc = kt.get('killed_rc', '')
                    holds(f'pin_{cid}_daemon_killed', [] if krc == '137' else
                          [f'the daemon ended {krc or "with no status"}, not by the rig\'s SIGKILL (137): it had '
                           'ended by itself'], 'server', 'the daemon was serving until the rig killed it (137)')
                if scen in RUN_SCENARIOS:
                    judge_run(cid, cd, scen, outs, holds, premise, lambda path: text_of(path, need=False))
                if scen == 'sweep' and not kill:
                    stray = text_of(os.path.join(cd, 'stray-holds.txt'))
                    holds(f'pin_{cid}_swept', [f'the stray hold is still there: {flat(stray)}'] if HOLD_TAG in stray else [],
                          'server', 'the publish released the hold that no publish named')
                if 'settle' in outs:
                    src = text_of(os.path.join(cd, 'settle.rc')).strip()
                    want_rc = '3' if outs['settle']['abandoned'] else '0'
                    holds(f'pin_{cid}_settle', [] if src == want_rc else
                          [f'the settle ended {src or "with no status"}, not {want_rc}: '
                           f'{flat(text_of(os.path.join(cd, "settle.err"), need=False))[-200:]}'],
                          'server', f'the settle ended {src}' + (', giving up ' + ', '.join(outs['settle']['abandoned'])
                                                                  if outs['settle']['abandoned'] else ''))
                retry = outs.get('retry', {}).get('published', {})
                # A retry that published under T's own name made a new
                # snapshot: T was not recorded, or its take was destroyed. It
                # is the retry's publish, not T's.
                reused = retry.get(t) if t else None
                if reused:
                    put(f'obs_{cid}_name_reused', 'INFO', 'server',
                        f'the retry published a new snapshot under {t}\'s name, {reused[:12]}')
                    # The engine never takes again a name it gave up
                    # (fresh_name skips the record's abandoned names): a
                    # retry under T's name is right only where T was not
                    # given up (G-a of #159's review).
                    why = next((o['abandoned'][t] for o in outs.values() if t in o['abandoned']), None) \
                        or record_abandoned(text_of(os.path.join(cd, 'record-after.txt'))).get(t)
                    holds(f'pin_{cid}_name_not_given_up', [f'the retry took {t}\'s name, which the agent had given up '
                                                           f'({why})'] if why else [],
                          'server', f'{t} was not given up before the retry took its name')
                got = outcome(t, list(outs.values()), (set(pub) | set(gone)) - ({t} if reused else set()),
                              text_of(os.path.join(cd, 'record-after.txt')))
                want = expected(scen, kill, destroyed, t)
                if want is None:
                    put(f'obs_{cid}_outcome', 'INFO', 'server', f'{t or "no name"}: {got}')
                else:
                    holds(f'pin_{cid}_outcome', [] if got == want else [f'{t or "no name"}: {got}, not {want}'],
                          'server', f'{t or "no name"}: {got}')
                if moved(kill) and t:
                    # One take, one publish (G-b), and the take the next
                    # start destroyed is gone from node A, unless the retry
                    # made a new one under its name (G-c).
                    new = sorted(n for n in outs.get('settle', {}).get('published', {}) if n != t)
                    left = f'{POOL}@{t}' in text_of(os.path.join(cd, 'snapshots-after.txt')).split() and not reused
                    holds(f'pin_{cid}_moved', ([] if len(new) == 1 else
                                               [f'the settle published {len(new)} names but {t}, not 1: '
                                                f'{", ".join(new) or flat(text_of(os.path.join(cd, "settle.out"), need=False))[-200:] or "nothing"}'])
                          + ([f'{POOL}@{t}, the take, is still on node A after the last run'] if left else []),
                          'server', f'{t}\'s take destroyed at the next start; the publish went on as {", ".join(new)}')
                if got == 'published':
                    ok = []
                elif retry and all(n in pub or n in gone for n in retry):
                    ok = []
                else:
                    ok = [f'snap/{t or "T"} absent and the retry did not publish '
                          f'(exit {text_of(os.path.join(cd, "retry.rc"), need=False).strip() or "none"})']
                holds(f'pin_{cid}_criterion', ok, 'kernel and server',
                      f'{t} published' if got == 'published' else f'snap/{t or "T"} absent; the retry published '
                      f'{", ".join(retry)}')
                srow = os.path.join(cd, 'snaprow.txt')
                if t and os.path.exists(os.path.join(rdir, srow)) \
                        and publish_check.read_snap_row(text_of(srow)).get('ref') is None:
                    rc_, text = nb.tool_run(['snap-row', '--volume', vol, '--snapshot', t])
                    if rc_:
                        raise RigError(f'skyzfs-nodeb snap-row {t}: exit {rc_}: {flat(text)[-300:]}')
                    later = publish_check.read_snap_row(text).get('ref')
                    if reused:
                        holds(f'pin_{cid}_stays_absent', [] if later is not None and later[0] == reused else
                              [f'node B has snap/{t} on {later[0][:12] if later else "nothing"}, not on the retry\'s '
                               f'{reused[:12]}'],
                              'kernel and server', f'snap/{t}, absent after the settle, is the retry\'s publish at the '
                              'check')
                    else:
                        holds(f'pin_{cid}_stays_absent', [] if later is None else
                              [f'node B had no snap/{t} after the settle and has it now, on {later[0][:12]}'],
                              'kernel and server', f'snap/{t}, absent after the settle, is still absent at the check')
                holds(f'pin_{cid}_clean',
                      clean(text_of(os.path.join(cd, 'record-after.txt')), text_of(os.path.join(cd, 'holds.txt')),
                            text_of(os.path.join(cd, 'manifests.txt')), text_of(os.path.join(cd, 'assist.txt')),
                            text_of(os.path.join(cd, 'record-after.rc')),
                            text_of(os.path.join(cd, 'record-after.err'), need=False)),
                      'server', 'nothing under way, no hold of the tag, no manifest left, the assist on the root only')
                for run, o in outs.items():
                    both = [n for n in o['abandoned'] if n in pub]
                    if both:
                        put(f'obs_{cid}_{run}_abandoned_published', 'INFO', 'server',
                            f'the agent gave up {", ".join(both)}, and node B has it published')
                how = f' killed at {kill}' if kill else (f' ({named} not reached)' if named else '')
                put(f'case_{cid}', 'INFO', 'rig',
                    f"{scen}{how}: first run {first_rc}, T {t or 'none'}, {got}; settle "
                    f"{text_of(os.path.join(cd, 'settle.rc'), need=False).strip() or '-'}, retry "
                    f"{text_of(os.path.join(cd, 'retry.rc'), need=False).strip() or '-'}")
            except RigError as e:
                # D8 of #159's review: the check's own error in judging a
                # case ends that case, and the next ones are judged.
                premise(f'premise_{cid}_rig', [f'the check could not judge the case: {e}'], '')
                put(f'case_{cid}', 'INFO', 'rig', 'not judged further')

        # Node B on each name published.
        names = list(pub)
        checked = {}
        for n in names:
            p = pub[n]
            args = ['snap-row', '--volume', vol, '--snapshot', n, '--out', os.path.join(nb.dir, f'manifest-{n}.got')]
            rc_, text = nb.tool_run(args)
            if rc_:
                raise RigError(f'skyzfs-nodeb snap-row {n}: exit {rc_}: {flat(text)[-300:]}')
            got = publish_check.read_snap_row(text)
            ref, row = got.get('ref'), got.get('row')
            bad = []
            if ref is None:
                bad.append('no ref')
            if not isinstance(row, dict):
                bad.append('no row' if row is None else 'a row that is not of version 1')
            if bad:
                holds(f'pin_{n}_row', bad, 'server', '')
                continue
            if row['commit'] != ref[0]:
                bad.append(f"the row names commit {row['commit'][:12]}, the ref {ref[0][:12]}")
            if p['printed'] != '-' and p['printed'] != ref[0]:
                bad.append(f"the agent printed {p['printed'][:12]}, the ref is on {ref[0][:12]}")
            if row['group'] < p['txg']:
                bad.append(f"the row's cut is of group {row['group']}, below the snapshot's createtxg {p['txg']}")
            holds(f'pin_{n}_row', bad + publish_check.judge_cut(cuts, row), 'server',
                  f"ref and row on {ref[0][:12]}: cut {row['number']} of group {row['group']} (createtxg {p['txg']}), "
                  'which opens as itself')
            m = got.get('manifest')
            mpath = os.path.join(nb.dir, f'manifest-{n}.got')
            data = None
            if os.path.exists(mpath):
                with open(mpath, 'rb') as f:
                    data = f.read()
            mbad = []
            if m is None or m == 'absent' or data is None:
                mbad.append('no manifest object')
            else:
                if not m[2]:
                    mbad.append("the manifest object is not of the row's length and hash")
                mbad += judge_manifest_json(data, n, p['txg'], p['guid'])
            holds(f'pin_{n}_manifest', mbad, 'server',
                  f'the agent\'s manifest of {POOL}@{n}: createtxg {p["txg"]}, the root\'s guid')
            sums = text_of(f'snap-{n}.sha256')
            want = nodeb_check.read_sums(sums)
            if not want:
                raise RigError(f'snap-{n}.sha256 lists no file')
            seen = nb.verify(vol, f'snap/{n}', n, want, damaged=False)
            failed = seen.failed()
            holds(f'pin_{n}_nodeb', [f'node B failed at {failed}: {seen.text()}'] if failed else [],
                  'kernel and server', seen.text())
            checked[n] = (row, want, data)
        for cid, (hook, kcut) in rows.items():
            t = next((c[4] for c in cases if c[0] == cid), '-')
            row = checked.get(t, (None,))[0]
            if kcut is None or hook not in ('carry-put:1', 'carry-swap:1'):
                continue
            ask = last_ask(instances, t)
            if not premise(f'premise_{cid}_asked', [] if ask else [f'no ask of {t} accepted in the request log'],
                           f'the last ask of {t} accepted: G {ask[1]}, C {ask[2]}' if ask else ''):
                continue
            first = first_carrier(cuts, ask)
            detail = (f"the hook killed at cut {kcut[0]} (group {kcut[1]}); the last request accepted (G {ask[1]}, "
                      f"C {ask[2]}) is first carried by "
                      + (f'cut {first[0]} (group {first[1]})' if first else 'no cut in the log') + f'; snap/{t} names '
                      + (f"cut {row['number']} (group {row['group']})" if row else 'no row'))
            put(f'obs_{cid}_carried_by', 'INFO', 'server', detail)
            if row is None:
                holds(f'pin_{cid}_carried_by_the_last_request', [f'no row of {t} checked at node B'], 'server', '')
                continue
            holds(f'pin_{cid}_carried_by_the_last_request', judge_carried(cuts, row, ask), 'server',
                  f"cut {row['number']} (group {row['group']}): counted after C {ask[2]}, of G {ask[1]} or later, "
                  'opens as itself')
            if hook == 'carry-swap:1':
                # The killed cut had committed: a row on an older one would
                # be a cut from before it (the lead's ruling on #158).
                older = (row['number'], row['group']) < kcut
                holds(f'pin_{cid}_no_older_than_the_killed_cut',
                      [f"snap/{t} names cut {row['number']} (group {row['group']}), older than the killed cut "
                       f"{kcut[0]} (group {kcut[1]})"] if older else [], 'server',
                      f"cut {row['number']} (group {row['group']}), not older than cut {kcut[0]} (group {kcut[1]})")
        # The controls, on the first name checked and a later one.
        order = [n for n in names if n in checked]
        if len(order) < 2:
            raise RigError(f'{len(order)} names checked at node B: the controls need two')
        a = order[0]
        b = next((n for n in order[1:] if pub[n]['txg'] > checked[a][0]['group']), None)
        if b is None:
            raise RigError(f'no name published after the cut that carried {a}: ctl_early has nothing to ask')
        seen = nb.verify(vol, f'snap/{a}', b, checked[b][1], damaged=False)
        verdict, ok, _ = nodeb_check.control_verdict(seen, ('snapshot',))
        put('ctl_early', verdict.upper().replace(' ', '_'), 'check',
            f'snap/{a} asked for {POOL}@{b}: {seen.text()}{"" if ok else "; DIFFERS from caught at snapshot"}')
        any_failed = any_failed or not ok
        rc = max(rc, 0 if ok else 1)
        wrong = dict(checked[a][1])
        k0 = sorted(wrong)[0]
        wrong[k0] = ('0' if wrong[k0][0] != '0' else '1') + wrong[k0][1:]
        seen = nb.verify(vol, f'snap/{a}', a, wrong, damaged=False)
        verdict, ok, _ = nodeb_check.control_verdict(seen, ('files',))
        put('ctl_files', verdict.upper().replace(' ', '_'), 'check',
            f'{k0} with another digest: {seen.text()}{"" if ok else "; DIFFERS from caught at files"}')
        any_failed = any_failed or not ok
        rc = max(rc, 0 if ok else 1)
        mbad = judge_manifest_json(checked[a][2], a, pub[a]['txg'], pub[a]['guid'] + 1)
        put('ctl_manifest', 'CAUGHT' if mbad else 'MISSED', 'check', '; '.join(mbad) or 'the pin held')
        any_failed = any_failed or not mbad
        rc = max(rc, 0 if mbad else 1)
        put('obs_points', 'INFO', 'rig', f"reached: {', '.join(e for e in EFFECTS if e in reached)}; none reached: "
            f"{', '.join(e for e in EFFECTS if e not in reached) or 'none'}")
    except RigError as e:
        put('rig_error', 'ERROR', 'rig', str(e))
        rc = 2
    except Exception as e:  # noqa: BLE001 -- anything else is the rig's too, not a verdict
        put('rig_error', 'ERROR', 'rig', f'{type(e).__name__}: {e}')
        rc = 2
    if any_failed:
        rc = 1
    put('time', 'INFO', 'rig', f'{time.monotonic() - t0:.0f} s')
    with open(os.path.join(rdir, 'results.tsv'), 'a') as f:
        f.write(''.join(ln + '\n' for ln in lines))
    img = os.path.join(rdir, 'nodeb', 'img')
    if os.path.isdir(img):
        shutil.rmtree(img)
    sys.exit(rc)


if __name__ == '__main__':
    main()
