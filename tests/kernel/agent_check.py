#!/usr/bin/env python3
"""The agent rig's check (M2.T5 stage 1; M2.6's criterion on a real pool:
"after each loss, either snap/t is absent and a retry publishes it, or it
names a cut that holds the snapshot; a snapshot destroyed before the hold
is not published, and no hold is left behind after any loss").

Usage:
  agent_check.py NAME DIR LOG BASE DEV_BYTES [LOG_DEVICE_IMAGE]
  agent_check.py points TRACE [BASE_TRACE]

The first form is cut-record.sh's CUT_CHECK after the agent rig's courses
(aplain, adaemon, arare; tests/kernel/agent-record.sh says what each
does), recorded through the daemon with its uploads (CUT_UPLOAD=1). LOG
is the daemon's request log; BASE is `-`. Environment: as
nodeb_check.py's. The volume in object storage is `cut-NAME`.

The second prints the points a trace reached (`--trace-points`), one
`EFFECT:SIDE:N` a line in the order reached, N its reach of that point
(from 1): the `--kill-at` of a case at each. With BASE_TRACE, only those
BASE_TRACE did not reach (by effect, side and reach).

What each case left is in DIR/cases/ID (agent-record.sh), and its line
in DIR/cases.tsv. For each case:

- premise_ID_killed (rig): where the case kills the agent, its first run
  ended by SIGKILL (137) with the hook's line for that point on its
  standard error, and the trace's last point is that point at that
  reach. A point not reached is the rig's error (exit 2): the case did
  not test what it names.
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
  scenario: `published`, or `abandoned WHY` in the agent's words. A plain
  case with a kill is INFO (obs_ID_outcome): the criterion judges it, and
  the outcome at each point is the first run's to show.
- pin_ID_clean (server): after the last run, the record has nothing
  under way (the agent's dump), no hold of the agent's tag is on any
  snapshot, the manifests directory is empty, and `skyzfs:assist` is set
  locally on the pool's root only.
- Case-specific: unmarked, the publish refused (exit 6) and released
  nothing; sweep, the stray hold released by the traced publish;
  carry-swap, the row of T names the cut the hook killed at (a finding
  against the daemon: the lead's ruling on #150), carry-put the same as
  INFO (the named cut had not committed and may be abandoned).

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

results.tsv gets the lines (KEY, VERDICT, DETAIL), which cut_summary.py
prints. Exit 0: every line holds. Exit 1: a verdict. Exit 2: the rig's
own error, which is also what any exception ends in.
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

COURSES = ('aplain', 'adaemon', 'arare')
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
# The scenario -> T's outcome where the scenario's premise holds; None:
# the criterion alone (INFO). A plain case with no kill must publish.
EXPECTED = {
    'plain': None,
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
}


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


def new_points(trace, base):
    """The points of TRACE that BASE did not reach, in TRACE's order."""
    had = set(points(base))
    return [p for p in points(trace) if p not in had]


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


def read_outputs(text):
    """The agent's standard output -> {'published': {t: commit}, 'abandoned': {t: why}, 'stopped': {t: why},
    'unsettled': [t]}."""
    out = {'published': {}, 'abandoned': {}, 'stopped': {}, 'unsettled': []}
    for ln in text.splitlines():
        f = ln.split()
        if len(f) == 2 and f[0].startswith('snap/'):
            out['published'][f[0][len('snap/'):]] = f[1]
        elif len(f) == 3 and f[0] == 'abandoned':
            out['abandoned'][f[1]] = f[2]
        elif len(f) >= 3 and f[0] == 'stopped':
            out['stopped'][f[1]] = ' '.join(f[2:])
        elif len(f) == 2 and f[0] == 'unsettled':
            out['unsettled'].append(f[1])
        elif ln.strip():
            raise RigError(f'a line of the agent\'s output that is not one of its four: {ln!r}')
    return out


def outcome(t, outs, published):
    """T's outcome: 'published' where node B has it (published), else the
    agent's last `abandoned T WHY`, else 'absent'."""
    if t and t in published:
        return 'published'
    why = None
    for o in outs:
        if t in o['abandoned']:
            why = o['abandoned'][t]
    return f'abandoned {why}' if why else 'absent'


def expected(scen, kill, destroyed):
    """The outcome EXPECTED for a case, or None where the criterion
    alone judges it."""
    if scen == 'plain':
        return None if kill else 'published'
    if scen in ('destroy', 'gone') and not destroyed:
        return None
    return EXPECTED[scen]


def clean(record, holds, manifests, assist):
    """After the last run -> [what is left]."""
    bad = []
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
    if not out and len(argv) == 1:
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

    def put(key, verdict, rests, detail):
        lines.append(f'{key}\t{verdict}\t{flat(f"[{rests}] {detail}")}')
        print(f'{name}: {key}: {verdict}: [{rests}] {flat(detail)}', flush=True)

    def holds(key, bad, rests, good):
        nonlocal rc
        if bad:
            rc = max(rc, 1)
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
        cases = [ln.split('\t') for ln in text_of('cases.tsv').splitlines() if ln]
        if not cases:
            raise RigError('no case in cases.tsv')
        reached = set()
        rows = {}
        for c in cases:
            if len(c) != 5:
                raise RigError(f'cases.tsv: not id, scenario, kill, status, name: {c!r}')
            cid, scen, kill, first_rc, t = c
            kill = '' if kill == '-' else kill
            t = '' if t == '-' else t
            if scen not in EXPECTED:
                raise RigError(f'case {cid}: no scenario {scen}')
            cd = os.path.join('cases', cid)
            trace = read_trace(text_of(os.path.join(cd, 'trace.tsv'), need=False))
            reached.update(e for _, e, _, _ in trace)
            outs = {}
            for run in ('publish', 'first', 'settle-frozen', 'settle', 'retry'):
                txt = text_of(os.path.join(cd, f'{run}.out'), need=False)
                if os.path.exists(os.path.join(rdir, cd, f'{run}.out')):
                    outs[run] = read_outputs(txt)
            if kill:
                premise(f'premise_{cid}_killed',
                        killed_at(kill, first_rc, text_of(os.path.join(cd, 'publish.err'), need=False), trace),
                        f'killed at {kill}, as the trace and the hook\'s line say')
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
                         else ['the refused run wrote a record']),
                      'server', 'a fresh directory with a hold of the tag: exit 6, nothing released, no record written')
            if scen == 'sweep' and not kill:
                stray = text_of(os.path.join(cd, 'stray-holds.txt'))
                holds(f'pin_{cid}_swept', [f'the stray hold is still there: {flat(stray)}'] if HOLD_TAG in stray else [],
                      'server', 'the publish released the hold that no publish named')
            got = outcome(t, list(outs.values()), pub)
            want = expected(scen, kill, destroyed)
            if want is None:
                put(f'obs_{cid}_outcome', 'INFO', 'server', f'{t or "no name"}: {got}')
            else:
                holds(f'pin_{cid}_outcome', [] if got == want else [f'{t or "no name"}: {got}, not {want}'],
                      'server', f'{t or "no name"}: {got}')
            retry = outs.get('retry', {}).get('published', {})
            if got == 'published':
                ok = []
            elif retry and all(n in pub for n in retry):
                ok = []
            else:
                ok = [f'snap/{t or "T"} absent and the retry did not publish '
                      f'(exit {text_of(os.path.join(cd, "retry.rc"), need=False).strip() or "none"})']
            holds(f'pin_{cid}_criterion', ok, 'kernel and server',
                  f'{t} published' if got == 'published' else f'snap/{t or "T"} absent; the retry published '
                  f'{", ".join(retry)}')
            holds(f'pin_{cid}_clean',
                  clean(text_of(os.path.join(cd, 'record-after.txt')), text_of(os.path.join(cd, 'holds.txt')),
                        text_of(os.path.join(cd, 'manifests.txt')), text_of(os.path.join(cd, 'assist.txt'))),
                  'server', 'nothing under way, no hold of the tag, no manifest left, the assist on the root only')
            for run, o in outs.items():
                both = [n for n in o['abandoned'] if n in pub]
                if both:
                    put(f'obs_{cid}_{run}_abandoned_published', 'INFO', 'server',
                        f'the agent gave up {", ".join(both)}, and node B has it published')
            put(f'case_{cid}', 'INFO', 'rig',
                f"{scen}{' killed at ' + kill if kill else ''}: first run {first_rc}, T {t or 'none'}, {got}; settle "
                f"{text_of(os.path.join(cd, 'settle.rc'), need=False).strip() or '-'}, retry "
                f"{text_of(os.path.join(cd, 'retry.rc'), need=False).strip() or '-'}")
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
            same = row is not None and (row['number'], row['group']) == kcut
            detail = (f"the hook killed at cut {kcut[0]} (group {kcut[1]}); snap/{t} names "
                      + (f"cut {row['number']} (group {row['group']})" if row else 'no row'))
            if hook == 'carry-swap:1':
                holds(f'pin_{cid}_carried_by_the_killed_cut', [] if same else [detail], 'server', detail)
            else:
                put(f'obs_{cid}_carried_by', 'INFO', 'server', detail + ('' if same else ': a later cut (design §5.2 '
                                                                       'allows it: the named cut had not committed)'))
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
        rc = max(rc, 0 if ok else 1)
        wrong = dict(checked[a][1])
        k0 = sorted(wrong)[0]
        wrong[k0] = ('0' if wrong[k0][0] != '0' else '1') + wrong[k0][1:]
        seen = nb.verify(vol, f'snap/{a}', a, wrong, damaged=False)
        verdict, ok, _ = nodeb_check.control_verdict(seen, ('files',))
        put('ctl_files', verdict.upper().replace(' ', '_'), 'check',
            f'{k0} with another digest: {seen.text()}{"" if ok else "; DIFFERS from caught at files"}')
        rc = max(rc, 0 if ok else 1)
        mbad = judge_manifest_json(checked[a][2], a, pub[a]['txg'], pub[a]['guid'] + 1)
        put('ctl_manifest', 'CAUGHT' if mbad else 'MISSED', 'check', '; '.join(mbad) or 'the pin held')
        rc = max(rc, 0 if mbad else 1)
        put('obs_points', 'INFO', 'rig', f"reached: {', '.join(e for e in EFFECTS if e in reached)}; none reached: "
            f"{', '.join(e for e in EFFECTS if e not in reached) or 'none'}")
    except RigError as e:
        put('rig_error', 'ERROR', 'rig', str(e))
        rc = 2
    except Exception as e:  # noqa: BLE001 -- anything else is the rig's too, not a verdict
        put('rig_error', 'ERROR', 'rig', f'{type(e).__name__}: {e}')
        rc = 2
    put('time', 'INFO', 'rig', f'{time.monotonic() - t0:.0f} s')
    with open(os.path.join(rdir, 'results.tsv'), 'a') as f:
        f.write(''.join(ln + '\n' for ln in lines))
    img = os.path.join(rdir, 'nodeb', 'img')
    if os.path.isdir(img):
        shutil.rmtree(img)
    sys.exit(rc)


if __name__ == '__main__':
    main()
