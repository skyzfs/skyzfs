#!/usr/bin/env python3
"""The agent rig's check (agent_check.py): its parts, and its `main` from
end to end with no kernel and no object store. Node B is a stand-in that
answers `snap-row` and the stages as a right publish leaves them; each
test changes one thing and reads the lines and the exit code. A right
course ends 0; a pin that fails ends 1, and so does a control that
misses; a premise that does not hold, or anything the rig cannot read,
ends 2.
"""
import contextlib
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import agent_check as ac  # noqa: E402
import nodeb_check  # noqa: E402
from cut_check import RigError  # noqa: E402

# The words of the checkout's agent, as main reads them. The traces below
# use words that #153 kept.
ac.EFFECTS = ac.engine_effects()

S = 'b' * 32
ULID = '01ARZ3NDEKTSV4RRFFQ69G5FAV'
POOL = 'skzcut'


def commit_of(t):
    return hashlib.sha256(t.encode()).hexdigest()


def sums_of(t):
    return f'{hashlib.sha256(("file of " + t).encode()).hexdigest()}  w0\n'


def manifest_of(t, txg, guid):
    return json.dumps({'createtxg': txg, 'format': 'skyzfs-manifest', 'members': [
        {'guid': '11', 'local': {}, 'name': POOL, 'snapshot_guid': str(guid), 'type': 'filesystem'}],
        'pool': POOL, 'pool_guid': '7', 'snapshot': t, 'version': 1}, sort_keys=True).encode()


def trace(name, *points, ns=1000):
    """[(effect, side)] -> --trace-points' text, each point named NAME."""
    out = []
    for e, s in points:
        ns += 10
        out.append(f'{ns}\t{e}\t{s}\t{"" if e == "enrol" else name}')
    return ''.join(ln + '\n' for ln in out)


WHOLE = [('enrol', 'before'), ('enrol', 'after'), ('intent', 'before'), ('intent', 'after'),
         ('holding', 'before'), ('holding', 'after'),
         ('ask', 'before'), ('ask', 'after'), ('asked', 'before'), ('asked', 'after'), ('look', 'before'),
         ('look', 'after'), ('release', 'before'), ('release', 'after'), ('done', 'before'), ('done', 'after')]
RECORD = f'volume {"a" * 64}\nenrolled {POOL} pool_guid=7 leaf_guid=8\n'
ASSIST = f'{POOL}\ton\tlocal\n{POOL}/fs\ton\tinherited from {POOL}\n'
STRAY = f'{POOL}@stray-u\tskyzfs-publish\tThu Nov 26 10:00 2026\n'


def upto(*last):
    """WHOLE up to and with the point LAST."""
    return WHOLE[:WHOLE.index(last) + 1]


# A right course: the cases (id, scenario, kill, first run's status, T,
# {file: text}), the names published (case, name, createtxg, guid, the
# commit printed or '-'), node B's rows (name: (number, group)).
def right():
    return {
        'cases': [
            ('trace', 'plain', '-', '0', 'a1', {
                'trace.tsv': trace('a1', *WHOLE), 'publish.out': f'snap/a1 {commit_of("a1")}\n'}),
            ('A-asked-after-1', 'plain', 'asked:after:1', '137', 'a2', {
                'trace.tsv': trace('a2', *upto('asked', 'after')),
                'publish.err': 'skyzfs-agent: test hook --kill-at: killed at asked after (1)\n',
                'settle.out': f'snap/a2 {commit_of("a2")}\n'}),
            ('unmarked', 'unmarked', '-', '6', '-', {
                'record-before.txt': 'absent\n', 'record-before.rc': '0\n', 'stray-holds.txt': STRAY,
                'retry.out': f'snap/a3 {commit_of("a3")}\n', 'retry.rc': '0\n'}),
            ('gone', 'gone', 'holding:before:1', '137', 'g1', {
                'trace.tsv': trace('g1', *upto('holding', 'before')),
                'publish.err': 'skyzfs-agent: test hook --kill-at: killed at holding before (1)\n',
                'destroy.log': '', 'settle.out': 'abandoned g1 destroyed\n', 'settle.rc': '3\n',
                'snaprow.txt': 'ref\tnone\nsnaprow\tnone\nmanifest\tnone\n',
                'retry.out': f'snap/a4 {commit_of("a4")}\n', 'retry.rc': '0\n'}),
            ('d-carry-swap', 'dhook', '-', '0', 'a5', {
                'trace.tsv': trace('a5', *WHOLE), 'publish.out': f'snap/a5 {commit_of("a5")}\n',
                'kill.tsv': 'killed_rc\t137\nserve\t2\n',
                'serve-killed.log': 'skyzfsd: test hook --kill-at-upload: killed carry-swap:1: after the commit of '
                                    'cut 9 (group 90), chosen to carry publish 1, landed, before the cut was let go\n'}),
        ],
        'pub': [('trace', 'a1', 50, 501, commit_of('a1')), ('A-asked-after-1', 'a2', 60, 502, '-'),
                ('unmarked', 'a3', 70, 503, '-'), ('gone', 'a4', 80, 504, '-'),
                ('d-carry-swap', 'a5', 85, 505, commit_of('a5'))],
        'rows': {'a1': (5, 52), 'a2': (6, 61), 'a3': (7, 71), 'a4': (8, 81), 'a5': (9, 90)},
        # The daemon's accepted asks: (name, G, C).
        'asks': [('a5', 85, 8)],
    }


class FakeNodeB:
    """Node B as a right publish leaves it. ROWS: T -> (number, group,
    commit), PUB: T -> (createtxg, guid). FAIL: the names whose import
    fails; MISS_FILES: node B does not see a digest that differs."""
    ROWS = {}
    PUB = {}
    FAIL = set()
    MISS_FILES = False
    MISS_SNAP = False

    def __init__(self, rdir, logdev):
        self.dir = os.path.join(rdir, 'nodeb')
        os.makedirs(self.dir, exist_ok=True)

    def tool_run(self, args, timeout=600):
        assert args[0] == 'snap-row', args
        t = args[args.index('--snapshot') + 1]
        if t not in self.ROWS:
            return 0, 'ref\tnone\nsnaprow\tnone\nmanifest\tnone\n'
        n, g, c = self.ROWS[t]
        m = manifest_of(t, *self.PUB.get(t, (1, 1)))
        if '--out' in args:
            with open(args[args.index('--out') + 1], 'wb') as f:
                f.write(m)
        h = hashlib.sha256(m).hexdigest()
        return 0, (f'ref\t{c}\t3\nsnaprow\t{S}\t{n}\t{g}\t{c}\t{ULID}\t{len(m)}\t{h}\n'
                   f'manifest\t{len(m)}\t{h}\tmatches\n')

    def verify(self, volume, ref, snap, want, damaged):
        seen = nodeb_check.Seen()
        right_sums = nodeb_check.read_sums(sums_of(snap))
        for st in nodeb_check.STAGES:
            ok = not (st == 'import' and snap in self.FAIL) and \
                not (st == 'snapshot' and ref != f'snap/{snap}' and not self.MISS_SNAP) and \
                not (st == 'files' and want != right_sums and not self.MISS_FILES)
            seen.put(st, ok, f'{st} of {ref}')
        return seen


def log_text(cuts, asks=()):
    """[(number, group, itself)], [(name, G, C)] -> a request log of one
    instance: the asks, then the cuts."""
    out, ns = ['1000\t100\tstart\t0\t-\t-\t-\t64'], 1000
    for name, g, c in asks:
        ns += 10
        out.append(f'{ns}\t100\tpublish\t-\tasked\t{g}\t{c}\t{name} accepted')
    for n, g, itself in cuts:
        ns += 10
        out.append(f'{ns}\t100\tcut\t-\t-\t{"itself" if itself else "rewind"}\t{n}\t{g}')
    return ''.join(ln + '\n' for ln in out)


def run_course(name='arare', change=None, fail=(), miss_files=False, miss_snap=False, course=None):
    c = course or right()
    d = tempfile.mkdtemp()
    try:
        files = {'watch.log': '1000\tread cut 5\n', 'marks.tsv': '', 'published-gone.tsv': c.get('gone', ''),
                 'requests.tsv': log_text([(n, g, True) for n, g in sorted(c['rows'].values())], c.get('asks', ())),
                 'published.tsv': ''.join(f'{a}\t{n}\t{t}\t{g}\t{p}\n' for a, n, t, g, p in c['pub']),
                 'cases.tsv': ''.join(f'{i}\t{s}\t{k}\t{r}\t{t}\n' for i, s, k, r, t, _ in c['cases'])}
        for _, n, _, _, _ in c['pub']:
            files[f'snap-{n}.sha256'] = sums_of(n)
        for cid, _, _, _, _, own in c['cases']:
            base = {'record-after.txt': RECORD, 'record-after.rc': '0\n', 'holds.txt': '', 'manifests.txt': '',
                    'assist.txt': ASSIST, 'settle.rc': '0\n'}
            base.update(own)
            for fn, text in base.items():
                files[os.path.join('cases', cid, fn)] = text
        for fn, text in files.items():
            os.makedirs(os.path.dirname(os.path.join(d, fn)), exist_ok=True)
            with open(os.path.join(d, fn), 'w') as f:
                f.write(text)
        if change:
            change(d)
        fake = type('Fake', (FakeNodeB,), {
            'ROWS': {t: (n, g, commit_of(t)) for t, (n, g) in c['rows'].items()},
            'PUB': {n: (t, g) for _, n, t, g, _ in c['pub']},
            'FAIL': set(fail), 'MISS_FILES': miss_files, 'MISS_SNAP': miss_snap})
        argv = ['agent_check.py', name, d, os.path.join(d, 'requests.tsv'), '-', '1048576']
        out = io.StringIO()
        with mock.patch.object(nodeb_check, 'NodeB', fake), mock.patch.object(sys, 'argv', argv), \
                contextlib.redirect_stdout(out):
            try:
                ac.main()
                rc = None
            except SystemExit as e:
                rc = e.code
        with open(os.path.join(d, 'results.tsv')) as f:
            lines = {}
            for ln in f.read().splitlines():
                k, v, detail = ln.split('\t')
                lines[k] = (v, detail)
        return rc, lines
    finally:
        shutil.rmtree(d)


def write(fn, text, mode='w'):
    def change(d):
        with open(os.path.join(d, fn), mode) as f:
            f.write(text)
    return change


def case_file(cid, fn, text):
    return write(os.path.join('cases', cid, fn), text)


def remove(fn):
    return lambda d: os.unlink(os.path.join(d, fn))


def with_case(cid, **files):
    """The course with case CID's files changed."""
    c = right()
    c['cases'] = [(i, s, k, r, t, dict(own, **files) if i == cid else own) for i, s, k, r, t, own in c['cases']]
    return c


def not_ok(lines):
    return {k: v for k, v in lines.items() if v[0] not in ('ok', 'INFO', 'CAUGHT', 'CAUGHT_AT_SNAPSHOT',
                                                         'CAUGHT_AT_FILES')}


class Points(unittest.TestCase):
    def test_points_count_each_reach(self):
        t = ac.read_trace(trace('x', ('ask', 'before'), ('ask', 'after'), ('ask', 'before'), ('done', 'before')))
        self.assertEqual(ac.points(t), ['ask:before:1', 'ask:after:1', 'ask:before:2', 'done:before:1'])

    def test_new_points_leave_out_what_the_base_reached(self):
        base = ac.read_trace(trace('x', ('ask', 'before'), ('done', 'before')))
        t = ac.read_trace(trace('y', ('ask', 'before'), ('ask', 'before'), ('sweep', 'before'), ('done', 'before')))
        # The shared step after the scenario's own (done) is kept (G1).
        self.assertEqual(ac.new_points(t, base), ['ask:before:2', 'sweep:before:1', 'done:before:1'])
        self.assertEqual(ac.new_points(base, base), [])

    def test_new_points_keep_the_shared_steps_after_the_scenario_s_own(self):
        # G1 of #158's review: destroy's trace shares look, release and
        # unlink with the plain one, after its withdrawal.
        base = ac.read_trace(trace('x', ('ask', 'before'), ('look', 'before'), ('release', 'before'),
                                   ('unlink', 'before')))
        t = ac.read_trace(trace('y', ('ask', 'before'), ('look', 'before'), ('withdraw', 'before'),
                                ('look', 'before'), ('release', 'before'), ('unlink', 'before')))
        self.assertEqual(ac.new_points(t, base), ['withdraw:before:1', 'look:before:2', 'release:before:1',
                                                  'unlink:before:1'])

    def test_new_points_keep_three_reaches_and_the_last(self):
        base = ac.read_trace(trace('x', ('ask', 'before')))
        t = ac.read_trace(trace('y', ('ask', 'before'), *[('look', 'before')] * 7, ('release', 'before')))
        self.assertEqual(ac.new_points(t, base), ['look:before:1', 'look:before:2', 'look:before:3',
                                                  'look:before:7', 'release:before:1'])

    def test_a_trace_line_that_does_not_read_is_the_rig_s_error(self):
        for bad in ('1\task\tbefore', 'x\task\tbefore\tt', '1\tasks\tbefore\tt', '1\task\tduring\tt'):
            with self.assertRaises(RigError, msg=bad):
                ac.read_trace(bad + '\n')

    def test_the_effects_are_read_from_the_engine(self):
        text = ('impl Effect {\n    pub fn word(self) -> &\'static str {\n        match self {\n'
                + ''.join(f'            Effect::E{k} => "{w}",\n' for k, w in enumerate(
                    ('enrol', 'intent', 'take', 'failed', 'destroy', 'manifest', 'taken', 'holding', 'hold',
                     'held', 'not-whole')))
                + '        }\n    }\n}\n')
        self.assertEqual(ac.effects_of(text)[:3], ('enrol', 'intent', 'take'))
        self.assertEqual(ac.effects_of(text)[-1], 'not-whole')
        for bad in ('', text.replace('"held"', '"enrol"'), text[:text.index('"hold"')]):
            with self.assertRaises(RigError):
                ac.effects_of(bad)
        self.assertIn('enrol', ac.engine_effects())
        self.assertIn('sweep', ac.engine_effects())

    def test_the_points_command(self):
        d = tempfile.mkdtemp()
        try:
            a, b = os.path.join(d, 'a.tsv'), os.path.join(d, 'b.tsv')
            with open(a, 'w') as f:
                f.write(trace('x', ('ask', 'before'), ('sweep', 'after')))
            with open(b, 'w') as f:
                f.write(trace('y', ('ask', 'before')))
            run = [sys.executable, '-I', os.path.join(HERE, 'agent_check.py'), 'points']
            p = subprocess.run(run + [a], capture_output=True, text=True)
            self.assertEqual((p.returncode, p.stdout), (0, 'ask:before:1\nsweep:after:1\n'))
            p = subprocess.run(run + [a, b], capture_output=True, text=True)
            self.assertEqual((p.returncode, p.stdout), (0, 'sweep:after:1\n'))
            with open(b, 'w') as f:
                f.write('')
            p = subprocess.run(run + [b], capture_output=True, text=True)
            self.assertEqual(p.returncode, 2)
            with open(b, 'w') as f:
                f.write('1\tnot\tbefore\tt\n')
            p = subprocess.run(run + [b], capture_output=True, text=True)
            self.assertEqual(p.returncode, 2)
        finally:
            shutil.rmtree(d)


class Parts(unittest.TestCase):
    def test_killed_at(self):
        t = ac.read_trace(trace('x', ('ask', 'after'), ('asked', 'after'), ('ask', 'after')))
        line = 'skyzfs-agent: test hook --kill-at: killed at ask after (2)\n'
        self.assertEqual(ac.killed_at('ask:after:2', '137', line, t), [])
        self.assertTrue(ac.killed_at('ask:after:2', '0', line, t))
        self.assertTrue(ac.killed_at('ask:after:2', '137', '', t))
        self.assertTrue(ac.killed_at('ask:after:1', '137', line.replace('(2)', '(1)'), t))
        self.assertTrue(ac.killed_at('asked:after:1', '137', line.replace('ask after (2)', 'asked after (1)'), t))
        self.assertTrue(ac.killed_at('ask:after:2', '137', line, []))
        with self.assertRaises(RigError):
            ac.parse_kill('ask:after:0')

    def test_read_outputs(self):
        o = ac.read_outputs(f'snap/t1 {"c" * 64}\nabandoned t2 destroyed\nstopped t3 by a person\nunsettled t4\n')
        self.assertEqual(o, {'published': {'t1': 'c' * 64}, 'abandoned': {'t2': 'destroyed'},
                             'stopped': {'t3': 'by a person'}, 'unsettled': ['t4']})
        with self.assertRaises(RigError):
            ac.read_outputs('something else\n')

    def test_outcome(self):
        outs = [ac.read_outputs('abandoned t destroyed\n')]
        self.assertEqual(ac.outcome('t', outs, {'t': {}}), 'published')
        self.assertEqual(ac.outcome('t', outs, {}), 'abandoned destroyed')
        self.assertEqual(ac.outcome('u', outs, {}), 'absent')
        self.assertEqual(ac.outcome('', outs, {}), 'absent')
        # A run killed after it abandoned T printed nothing: the record says.
        rec = RECORD + 'abandoned u why=too-old at_ms=17\n'
        self.assertEqual(ac.outcome('u', [], {}, rec), 'abandoned too-old')
        self.assertEqual(ac.outcome('t', outs, {}, rec), 'abandoned destroyed')
        self.assertEqual(ac.outcome('', [], {}, rec), 'absent')
        self.assertEqual(ac.record_abandoned(rec + 'publish v phase=held\n'), {'u': 'too-old'})

    def test_not_reached(self):
        t = ac.read_trace(trace('x', ('look', 'before'), ('look', 'after'), ('abandon', 'before')))
        self.assertTrue(ac.not_reached('look:before:2', '3', '', t))
        # The first reach, a SIGKILL, a hook line, or a reach made: not so.
        self.assertFalse(ac.not_reached('look:before:1', '3', '', []))
        self.assertFalse(ac.not_reached('look:before:2', '137', '', t))
        line = 'skyzfs-agent: test hook --kill-at: killed at look before (2)\n'
        self.assertFalse(ac.not_reached('look:before:2', '3', line, t))
        t2 = ac.read_trace(trace('x', ('look', 'before'), ('look', 'before')))
        self.assertFalse(ac.not_reached('look:before:2', '3', '', t2))

    def test_expected(self):
        self.assertEqual(ac.expected('plain', '', False, 't'), 'published')
        self.assertIsNone(ac.expected('plain', 'ask:after:1', False, 't'))
        self.assertIsNone(ac.expected('destroy', 'ask:after:1', False, 't'))
        self.assertEqual(ac.expected('destroy', '', True, 't'), 'abandoned destroyed')
        self.assertEqual(ac.expected('tooold', '', False, 't'), 'abandoned too-old')
        self.assertEqual(ac.expected('unmarked', '', False, ''), 'absent')
        # Killed before the publish had a name: the criterion alone.
        self.assertIsNone(ac.expected('sweep', 'sweep:before:1', False, ''))
        self.assertEqual(ac.expected('sweep', 'done:before:1', False, 't'), 'published')

    def test_clean(self):
        self.assertEqual(ac.clean(RECORD, '', '', ASSIST), [])
        self.assertEqual(len(ac.clean(RECORD + 'publish t phase=held count=1\n', '', '', ASSIST)), 1)
        self.assertEqual(len(ac.clean(RECORD, STRAY, '', ASSIST)), 1)
        self.assertEqual(len(ac.clean(RECORD, f'{POOL}@x\tother\tThu\n', '', ASSIST)), 0)
        self.assertEqual(len(ac.clean(RECORD, '', 't\n', ASSIST)), 1)
        self.assertEqual(len(ac.clean(RECORD, '', '', ASSIST + f'{POOL}/fs\toff\tlocal\n')), 1)
        self.assertEqual(len(ac.clean('', '', '', ASSIST)), 1)
        self.assertEqual(ac.clean(RECORD + 'abandoned t why=destroyed at_ms=1\n', '', '', ASSIST), [])
        # A record that does not read: its dump's status, whatever it printed.
        self.assertEqual(ac.clean(RECORD, '', '', ASSIST, '0\n', ''), [])
        bad = ac.clean(RECORD, '', '', ASSIST, '6\n', 'skyzfs-agent: d: the record is torn\n')
        self.assertEqual(len(bad), 1)
        self.assertIn('ended 6', bad[0])
        self.assertEqual(len(ac.clean(RECORD, '', '', ASSIST, '', '')), 1)

    def test_judge_manifest_json(self):
        m = manifest_of('t', 50, 501)
        self.assertEqual(ac.judge_manifest_json(m, 't', 50, 501), [])
        self.assertTrue(ac.judge_manifest_json(m, 'u', 50, 501))
        self.assertTrue(ac.judge_manifest_json(m, 't', 51, 501))
        self.assertTrue(ac.judge_manifest_json(m, 't', 50, 502))
        self.assertTrue(ac.judge_manifest_json(b'not json', 't', 50, 501))
        d = json.loads(m)
        d['members'][0]['name'] = POOL + '/fs'
        self.assertTrue(ac.judge_manifest_json(json.dumps(d).encode(), 't', 50, 501))

    def test_daemon_kill(self):
        log = ('skyzfsd: test hook --kill-at-upload: killed carry-put:1: after the first segment PUT of cut 4 '
               '(group 40), chosen to carry publish 1, landed (k)\n')
        self.assertEqual(ac.daemon_kill('carry-put:1', 'killed_rc\t137\n', log), ([], (4, 40)))
        bad, _ = ac.daemon_kill('carry-put:1', 'killed_rc\tnone\n', log)
        self.assertTrue(bad)
        bad, cut = ac.daemon_kill('carry-swap:1', 'killed_rc\t137\n', log)
        self.assertTrue(bad)
        self.assertIsNone(cut)
        # The variant whose commit call failed names the cut as well.
        swap = ('skyzfsd: test hook --kill-at-upload: killed carry-swap:1: after the commit of cut 4 (group 40), '
                'chosen to carry publish 1, landed (its call failed; seen as the cut was let go), before the cut '
                'was let go\n')
        self.assertEqual(ac.daemon_kill('carry-swap:1', 'killed_rc\t137\n', swap), ([], (4, 40)))
        man = 'skyzfsd: test hook --kill-at-upload: killed after the framed manifest PUT 1 landed (k), before its ref\n'
        self.assertEqual(ac.daemon_kill('manifest:1', 'killed_rc\t137\n', man), ([], None))
        # The line that says what is chosen is not the kill.
        chosen = 'skyzfsd: test hook --kill-at-upload: carry-put:1: cut 4 (group 40) chosen to carry publish 1\n'
        self.assertTrue(ac.daemon_kill('carry-put:1', 'killed_rc\t137\n', chosen)[0])

    def test_late_premise(self):
        stopped = RECORD + 'publish t phase=asked count=3 group=40\n'
        t = ac.read_trace(trace('t', ('asked', 'after'), ('look', 'before'), ('done', 'before'), ns=2000))
        self.assertEqual(ac.late_premise(stopped, 't', t, 2000), [])
        self.assertTrue(ac.late_premise(stopped.replace('phase=asked', 'phase=done'), 't', t, 2000))
        t2 = ac.read_trace(trace('t', ('done', 'before'), ('look', 'before'), ns=2000))
        self.assertTrue(ac.late_premise(stopped, 't', t2, 2000))
        # A look before the agent was continued does not count.
        self.assertTrue(ac.late_premise(stopped, 't', t, 2025))


class Main(unittest.TestCase):
    def test_a_right_course_ends_0(self):
        rc, lines = run_course()
        self.assertEqual(rc, 0, not_ok(lines))
        self.assertNotIn('rig_error', lines)
        for k in ('pin_trace_criterion', 'pin_trace_outcome', 'pin_unmarked_refused', 'pin_gone_outcome',
                  'premise_gone_killed', 'premise_d-carry-swap_daemon_killed', 'pin_d-carry-swap_carried_by_the_last_request',
                  'pin_d-carry-swap_no_older_than_the_killed_cut',
                  'pin_a1_row', 'pin_a1_manifest', 'pin_a1_nodeb', 'pin_a5_nodeb'):
            self.assertEqual(lines[k][0], 'ok', (k, lines.get(k)))
        self.assertEqual(lines['obs_A-asked-after-1_outcome'][0], 'INFO')
        self.assertEqual(lines['ctl_early'][0], 'CAUGHT_AT_SNAPSHOT')
        self.assertEqual(lines['ctl_files'][0], 'CAUGHT_AT_FILES')
        self.assertEqual(lines['ctl_manifest'][0], 'CAUGHT')
        self.assertIn('manifest', lines['obs_points'][1].split('none reached: ')[1].split(', '))

    def test_a_course_not_of_the_agent_is_the_rig_s_error(self):
        rc, lines = run_course('pplain')
        self.assertEqual(rc, 2)
        self.assertIn('rig_error', lines)

    def test_a_kill_not_reached_is_the_rig_s_error(self):
        rc, lines = run_course(change=case_file('A-asked-after-1', 'publish.err', ''))
        self.assertEqual(rc, 2)
        self.assertEqual(lines['premise_A-asked-after-1_killed'][0], 'ERROR')
        # The other cases are still judged.
        self.assertEqual(lines['pin_a1_nodeb'][0], 'ok')

    def test_a_later_reach_not_made_is_judged_as_no_kill(self):
        # Run 45747041657: the agent looked once, abandoned and ended 3.
        c = right()
        c['cases'].append(('K-destroy-look-before-2', 'destroy', 'look:before:2', '3', 'k1', {
            'trace.tsv': trace('k1', *upto('asked', 'after'), ('withdraw', 'before'), ('withdraw', 'after'),
                               ('look', 'before'), ('look', 'after'), ('abandon', 'before'), ('abandon', 'after')),
            'destroy.log': '', 'publish.out': 'abandoned k1 destroyed\n',
            'retry.out': f'snap/a6 {commit_of("a6")}\n', 'retry.rc': '0\n'}))
        c['pub'].append(('K-destroy-look-before-2', 'a6', 95, 506, commit_of('a6')))
        c['rows']['a6'] = (10, 96)
        rc, lines = run_course(course=c)
        self.assertEqual(rc, 0, not_ok(lines))
        self.assertEqual(lines['obs_K-destroy-look-before-2_not_reached'][0], 'INFO')
        self.assertNotIn('premise_K-destroy-look-before-2_killed', lines)
        self.assertEqual(lines['pin_K-destroy-look-before-2_outcome'][0], 'ok')
        self.assertIn('look:before:2 not reached', lines['case_K-destroy-look-before-2'][1])
        # Another point's later reach: the same.
        c['cases'][-1] = c['cases'][-1][:2] + ('withdraw:before:2',) + c['cases'][-1][3:]
        rc, lines = run_course(course=c)
        self.assertEqual(lines['obs_K-destroy-look-before-2_not_reached'][0], 'INFO')
        # A first reach not made is still the rig's error.
        c['cases'][-1] = c['cases'][-1][:2] + ('look:after:1',) + c['cases'][-1][3:]
        c['cases'][-1][5]['trace.tsv'] = trace('k1', *upto('asked', 'after'))
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_K-destroy-look-before-2_killed'][0]), (2, 'ERROR'))

    def test_an_abandon_in_the_record_alone_is_the_outcome(self):
        # Run 45747041657: killed after abandon, the settle had nothing to print.
        c = right()
        rec = RECORD + 'abandoned k2 why=too-old at_ms=5\n'
        c['cases'].append(('K-tooold-abandon-after-1', 'tooold', 'abandon:after:1', '137', 'k2', {
            'trace.tsv': trace('k2', *upto('asked', 'after'), ('withdraw', 'before'), ('withdraw', 'after'),
                               ('abandon', 'before'), ('abandon', 'after')),
            'publish.err': 'skyzfs-agent: test hook --kill-at: killed at abandon after (1)\n',
            'record-after.txt': rec, 'retry.out': f'snap/a6 {commit_of("a6")}\n', 'retry.rc': '0\n'}))
        c['pub'].append(('K-tooold-abandon-after-1', 'a6', 95, 506, commit_of('a6')))
        c['rows']['a6'] = (10, 96)
        rc, lines = run_course(course=c)
        self.assertEqual(rc, 0, not_ok(lines))
        self.assertEqual(lines['pin_K-tooold-abandon-after-1_outcome'], ('ok', '[server] k2: abandoned too-old'))

    def test_a_trace_that_goes_on_past_the_kill_is_the_rig_s_error(self):
        c = with_case('A-asked-after-1', **{'trace.tsv': trace('a2', *WHOLE)})
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_A-asked-after-1_killed'][0]), (2, 'ERROR'))

    def test_a_hold_left_ends_1(self):
        rc, lines = run_course(change=case_file('trace', 'holds.txt', f'{POOL}@a1\tskyzfs-publish\tThu\n'))
        self.assertEqual((rc, lines['pin_trace_clean'][0]), (1, 'FAILED'))

    def test_a_publish_left_under_way_ends_1(self):
        rc, lines = run_course(change=case_file('gone', 'record-after.txt', RECORD + 'publish g1 phase=held\n'))
        self.assertEqual((rc, lines['pin_gone_clean'][0]), (1, 'FAILED'))

    def test_a_manifest_left_ends_1(self):
        rc, lines = run_course(change=case_file('trace', 'manifests.txt', 'a1\n'))
        self.assertEqual((rc, lines['pin_trace_clean'][0]), (1, 'FAILED'))

    def test_a_snapshot_destroyed_and_published_ends_1(self):
        # As the recorder leaves it (D1 of #158's review): node B has
        # snap/g1, node A has no g1, so published.tsv has no line of it and
        # published-gone.tsv has one.
        c = right()
        c['gone'] = 'gone\tg1\n'
        c['cases'] = [(i, s, k, r, t, dict(own, **{'settle.out': '', 'settle.rc': '0\n'}) if i == 'gone' else own)
                      for i, s, k, r, t, own in c['cases']]
        rc, lines = run_course(course=c)
        self.assertEqual(rc, 1)
        self.assertEqual(lines['pin_published_snapshots_exist'][0], 'FAILED')
        self.assertIn('g1 (case gone)', lines['pin_published_snapshots_exist'][1])
        self.assertEqual(lines['pin_gone_outcome'], ('FAILED', '[server] g1: published, not abandoned destroyed'))
        self.assertNotIn('rig_error', lines)
        rc, lines = run_course()
        self.assertEqual(lines['pin_published_snapshots_exist'][0], 'ok')

    def test_a_first_run_that_ended_otherwise_ends_1(self):
        # D2 of #158's review: a panic (101) where a plain publish with no
        # kill ends 0, and a later reach not made where the run ended 1.
        c = right()
        c['cases'][0] = c['cases'][0][:3] + ('101',) + c['cases'][0][4:]
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_trace_first_run'][0]), (1, 'FAILED'))
        self.assertEqual(lines['pin_trace_criterion'][0], 'ok')
        rc, lines = run_course()
        self.assertEqual(lines['pin_trace_first_run'][0], 'ok')
        self.assertNotIn('pin_A-asked-after-1_first_run', lines)
        self.assertNotIn('pin_unmarked_first_run', lines)
        c = right()
        c['cases'].append(('K-restart-look-before-2', 'restart', 'look:before:2', '1', 'k1', {
            'trace.tsv': trace('k1', *upto('look', 'before')), 'between.txt': 'ref\tnone\nsnaprow\tnone\n',
            'settle.out': f'snap/k1 {commit_of("k1")}\n'}))
        c['pub'].append(('K-restart-look-before-2', 'k1', 95, 506, commit_of('k1')))
        c['rows']['k1'] = (10, 96)
        rc, lines = run_course(course=c)
        self.assertEqual(lines['obs_K-restart-look-before-2_not_reached'][0], 'INFO')
        self.assertEqual((rc, lines['pin_K-restart-look-before-2_first_run'][0]), (1, 'FAILED'))
        self.assertEqual(lines['pin_K-restart-look-before-2_outcome'][0], 'ok')
        # Abandoned with no kill: 3, as FIRST_RC says.
        self.assertEqual(ac.FIRST_RC['destroy'], '3')

    def test_the_settle_s_status(self):
        # G5 of #158's review: 0, or 3 where its output gives T up.
        rc, lines = run_course()
        self.assertEqual(lines['pin_gone_settle'], ('ok', '[server] the settle ended 3, giving up g1'))
        self.assertEqual(lines['pin_A-asked-after-1_settle'][0], 'ok')
        for cid, code in (('A-asked-after-1', '101'), ('A-asked-after-1', '3'), ('gone', '0'), ('gone', '')):
            rc, lines = run_course(change=case_file(cid, 'settle.rc', code))
            self.assertEqual((rc, lines[f'pin_{cid}_settle'][0]), (1, 'FAILED'), (cid, code))

    def test_a_record_that_does_not_read_ends_1(self):
        # D3 of #158's review: the dump ended 6 and printed nothing under way.
        rc, lines = run_course(change=lambda d: (case_file('trace', 'record-after.rc', '6\n')(d),
                                                 case_file('trace', 'record-after.err', 'torn\n')(d)))
        self.assertEqual((rc, lines['pin_trace_clean'][0]), (1, 'FAILED'))
        self.assertIn('torn', lines['pin_trace_clean'][1])
        rc, lines = run_course(change=case_file('unmarked', 'record-before.rc', '1\n'))
        self.assertEqual((rc, lines['pin_unmarked_refused'][0]), (1, 'FAILED'))

    def test_a_verdict_the_rig_wrote_fails_its_case(self):
        # D5 of #158's review: what the recorder saw the agent or the
        # daemon do wrong is FAILED (exit 1), not the rig's error.
        rc, lines = run_course(change=case_file('A-asked-after-1', 'verdicts.tsv',
                                                'landed\tsnap/a2 had not landed 120 s after the thaw\n'))
        self.assertEqual(rc, 1)
        self.assertEqual(lines['pin_A-asked-after-1_landed'][0], 'FAILED')
        self.assertIn('had not landed', lines['pin_A-asked-after-1_landed'][1])
        self.assertEqual(lines['pin_gone_criterion'][0], 'ok')

    def test_a_verdict_in_a_case_cases_tsv_does_not_list(self):
        # The case under way when the rig failed has no line yet.
        def change(d):
            os.makedirs(os.path.join(d, 'cases', 'K-x'))
            write(os.path.join('cases', 'K-x', 'verdicts.tsv'), 'daemon_stopped\tserve 3 ended with 1\n')(d)
        rc, lines = run_course(change=change)
        self.assertEqual((rc, lines['pin_K-x_daemon_stopped'][0]), (1, 'FAILED'))

    def test_a_verdict_line_that_does_not_read_is_the_rig_s_error(self):
        for bad in ('no tab\n', 'Bad-Pin\twhat\n', 'pin\t\n'):
            rc, lines = run_course(change=case_file('trace', 'verdicts.tsv', bad))
            self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'), bad)

    def test_a_timed_out_first_run_fails(self):
        def change(d):
            with open(os.path.join(d, 'cases.tsv')) as f:
                text = f.read()
            write('cases.tsv', text.replace('trace\tplain\t-\t0', 'trace\tplain\t-\ttimeout'))(d)
            case_file('trace', 'verdicts.tsv', "publish_ended\tthe agent's publish had not ended 700 s on\n")(d)
        rc, lines = run_course(change=change)
        self.assertEqual(rc, 1)
        self.assertEqual(lines['pin_trace_first_run'][0], 'FAILED')
        self.assertEqual(lines['pin_trace_publish_ended'][0], 'FAILED')

    def test_the_rig_s_error_in_one_case_leaves_the_others_judged(self):
        # G6 of #158's review.
        rc, lines = run_course(change=case_file('A-asked-after-1', 'rig-error.txt',
                                                'releasing and destroying skzcut@a2 failed: busy\n'))
        self.assertEqual(rc, 2)
        self.assertEqual(lines['premise_A-asked-after-1_rig'][0], 'ERROR')
        self.assertNotIn('pin_A-asked-after-1_criterion', lines)
        for k in ('pin_gone_criterion', 'pin_unmarked_refused', 'pin_trace_clean'):
            self.assertEqual(lines[k][0], 'ok', k)
        # A verdict in another case still ends 1.
        rc, lines = run_course(change=lambda d: (
            case_file('A-asked-after-1', 'rig-error.txt', 'busy\n')(d),
            case_file('trace', 'manifests.txt', 'a1\n')(d)))
        self.assertEqual((rc, lines['pin_trace_clean'][0]), (1, 'FAILED'))

    def test_the_rig_s_error_in_the_course_and_in_a_trace(self):
        rc, lines = run_course(change=write('rig-error.tsv', 'K-destroy-x\tzfs holds failed: no pool\n'))
        self.assertEqual(rc, 2)
        self.assertEqual(lines['rig_error_in_the_course'][0], 'ERROR')
        self.assertIn('K-destroy-x', lines['rig_error_in_the_course'][1])
        self.assertEqual(lines['pin_gone_criterion'][0], 'ok')
        rc, lines = run_course(change=write('points-error.tsv', 'destroy\tthe trace reached no point\n'))
        self.assertEqual((rc, lines['premise_destroy_points'][0]), (2, 'ERROR'))

    def test_a_name_absent_after_the_settle_stays_absent(self):
        # G7 of #158's review: node B read again at the check.
        rc, lines = run_course()
        self.assertEqual((rc, lines['pin_gone_stays_absent'][0]), (0, 'ok'))
        c = right()
        c['rows']['g1'] = (10, 95)
        rc, lines = run_course(course=c)
        self.assertEqual(rc, 1)
        self.assertEqual(lines['pin_gone_stays_absent'][0], 'FAILED')
        self.assertIn('snap/g1', lines['pin_gone_stays_absent'][1])

    def test_a_verdict_is_not_hidden_by_the_rig_s_error(self):
        # G3 of #158's review: a premise that does not hold and a pin that
        # fails end 1, both lines kept.
        rc, lines = run_course(change=lambda d: (case_file('A-asked-after-1', 'publish.err', '')(d),
                                                 case_file('trace', 'manifests.txt', 'a1\n')(d)))
        self.assertEqual(rc, 1)
        self.assertEqual(lines['premise_A-asked-after-1_killed'][0], 'ERROR')
        self.assertEqual(lines['pin_trace_clean'][0], 'FAILED')
        # And a later error of the rig (too few names for the controls).
        c = right()
        c['cases'] = c['cases'][:1]
        c['pub'] = c['pub'][:1]
        c['rows'] = {'a1': c['rows']['a1']}
        rc, lines = run_course(course=c, change=case_file('trace', 'manifests.txt', 'a1\n'))
        self.assertEqual((rc, lines['pin_trace_clean'][0]), (1, 'FAILED'))
        self.assertIn('rig_error', lines)

    def test_both_s_daemon_killed(self):
        # G2 of #158's review.
        c = right()
        c['cases'].append(('b-ask-after-1', 'both', 'ask:after:1', '137', 'a6', {
            'trace.tsv': trace('a6', *upto('ask', 'after')),
            'publish.err': 'skyzfs-agent: test hook --kill-at: killed at ask after (1)\n',
            'both-kill.tsv': 'killed_rc\t137\n', 'settle.out': f'snap/a6 {commit_of("a6")}\n'}))
        c['pub'].append(('b-ask-after-1', 'a6', 90, 507, '-'))
        c['rows']['a6'] = (11, 91)
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_b-ask-after-1_daemon_killed'][0]), (0, 'ok'), not_ok(lines))
        for text in ('killed_rc\t0\n', 'killed_rc\t7\n', ''):
            rc, lines = run_course(course=c, change=case_file('b-ask-after-1', 'both-kill.tsv', text))
            self.assertEqual((rc, lines['pin_b-ask-after-1_daemon_killed'][0]), (1, 'FAILED'), text)

    def test_t_absent_and_no_retry_published_ends_1(self):
        c = with_case('unmarked', **{'retry.out': '', 'retry.rc': '1\n'})
        c['pub'] = [p for p in c['pub'] if p[1] != 'a3']
        del c['rows']['a3']
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_unmarked_criterion'][0]), (1, 'FAILED'))

    def test_a_publish_not_refused_unmarked_ends_1(self):
        c = right()
        c['cases'] = [(i, s, k, '0' if i == 'unmarked' else r, t, own) for i, s, k, r, t, own in c['cases']]
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_unmarked_refused'][0]), (1, 'FAILED'))

    def test_a_refused_publish_that_wrote_a_record_ends_1(self):
        rc, lines = run_course(change=case_file('unmarked', 'record-before.txt', RECORD))
        self.assertEqual((rc, lines['pin_unmarked_refused'][0]), (1, 'FAILED'))

    def test_a_published_name_below_its_createtxg_ends_1(self):
        c = right()
        c['rows']['a1'] = (4, 45)
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_a1_row'][0]), (1, 'FAILED'))

    def test_a_row_on_a_cut_that_opens_only_with_a_rewind_ends_1(self):
        def change(d):
            p = os.path.join(d, 'requests.tsv')
            with open(p) as f:
                text = f.read()
            with open(p, 'w') as f:
                f.write(text.replace('itself\t5\t52', 'rewind\t5\t52'))
        rc, lines = run_course(change=change)
        self.assertEqual((rc, lines['pin_a1_row'][0]), (1, 'FAILED'))

    def test_a_printed_commit_other_than_the_ref_s_ends_1(self):
        c = right()
        c['pub'][0] = ('trace', 'a1', 50, 501, 'f' * 64)
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_a1_row'][0]), (1, 'FAILED'))

    def test_a_manifest_that_differs_from_node_a_ends_1(self):
        def change(d):
            p = os.path.join(d, 'published.tsv')
            with open(p) as f:
                text = f.read()
            with open(p, 'w') as f:
                f.write(text.replace('\ta1\t50\t501\t', '\ta1\t50\t5011\t'))
        rc, lines = run_course(change=change)
        self.assertEqual((rc, lines['pin_a1_manifest'][0]), (1, 'FAILED'))

    def test_node_b_failing_a_stage_ends_1(self):
        rc, lines = run_course(fail=('a2',))
        self.assertEqual((rc, lines['pin_a2_nodeb'][0]), (1, 'FAILED'))

    def test_a_missed_control_ends_1(self):
        rc, lines = run_course(miss_files=True)
        self.assertEqual((rc, lines['ctl_files'][0]), (1, 'MISSED'))
        rc, lines = run_course(miss_snap=True)
        self.assertEqual(rc, 1)
        self.assertNotEqual(lines['ctl_early'][0], 'CAUGHT_AT_SNAPSHOT')

    def test_the_carry_hooks_row_against_the_last_request(self):
        # Run 45749017239: killed at cut 9, asked anew with C 10 after the
        # take-over, the row on cut 11: right (the lead's ruling on #158).
        c = right()
        c['rows']['a5'] = (11, 92)
        c['asks'] = [('a5', 85, 8), ('a5', 85, 10)]
        rc, lines = run_course(course=c)
        self.assertEqual(rc, 0, not_ok(lines))
        self.assertEqual(lines['pin_d-carry-swap_carried_by_the_last_request'][0], 'ok')
        self.assertEqual(lines['pin_d-carry-swap_no_older_than_the_killed_cut'][0], 'ok')
        self.assertIn('the hook killed at cut 9 (group 90)', lines['obs_d-carry-swap_carried_by'][1])
        self.assertIn('first carried by cut 11 (group 92)', lines['obs_d-carry-swap_carried_by'][1])
        # The row on a cut the last request does not accept: counted at C.
        c['asks'] = [('a5', 85, 8), ('a5', 85, 11)]
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_d-carry-swap_carried_by_the_last_request'][0]), (1, 'FAILED'))
        # Below G.
        c['asks'] = [('a5', 93, 10)]
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_d-carry-swap_carried_by_the_last_request'][0]), (1, 'FAILED'))
        # A row older than the killed cut, though the request accepts it.
        c = right()
        c['rows']['a5'] = (8, 85)
        c['rows']['a4'] = (7, 81)
        c['rows']['a3'] = (6, 71)
        c['rows']['a2'] = (5, 61)
        c['rows']['a1'] = (4, 52)
        c['asks'] = [('a5', 85, 7)]
        rc, lines = run_course(course=c)
        self.assertEqual(lines['pin_d-carry-swap_carried_by_the_last_request'][0], 'ok')
        self.assertEqual((rc, lines['pin_d-carry-swap_no_older_than_the_killed_cut'][0]), (1, 'FAILED'))
        # carry-put: an older cut is no finding, the last request still is.
        put = with_case('d-carry-swap', **{
            'serve-killed.log': 'skyzfsd: test hook --kill-at-upload: killed carry-put:1: after the first segment '
                                'PUT of cut 9 (group 90), chosen to carry publish 1, landed (k)\n'})
        put['cases'] = [('d-carry-put' if i == 'd-carry-swap' else i, s, k, r, t, own)
                        for i, s, k, r, t, own in put['cases']]
        put['pub'] = [('d-carry-put' if p[0] == 'd-carry-swap' else p[0],) + p[1:] for p in put['pub']]
        put['rows'] = c['rows']
        put['asks'] = [('a5', 85, 7)]
        rc, lines = run_course(course=put)
        self.assertEqual(rc, 0, not_ok(lines))
        self.assertNotIn('pin_d-carry-put_no_older_than_the_killed_cut', lines)
        self.assertEqual(lines['pin_d-carry-put_carried_by_the_last_request'][0], 'ok')
        put['asks'] = [('a5', 85, 8)]
        rc, lines = run_course(course=put)
        self.assertEqual((rc, lines['pin_d-carry-put_carried_by_the_last_request'][0]), (1, 'FAILED'))

    def test_a_carry_hook_with_no_ask_accepted_is_the_rig_s_error(self):
        c = right()
        c['asks'] = [('a4', 85, 8)]
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_d-carry-swap_asked'][0]), (2, 'ERROR'))

    def test_a_daemon_not_killed_by_its_hook_is_the_rig_s_error(self):
        rc, lines = run_course(change=case_file('d-carry-swap', 'kill.tsv', 'killed_rc\tnone\nserve\t2\n'))
        self.assertEqual((rc, lines['premise_d-carry-swap_daemon_killed'][0]), (2, 'ERROR'))
        rc, lines = run_course(change=case_file('d-carry-swap', 'serve-killed.log', ''))
        self.assertEqual((rc, lines['premise_d-carry-swap_daemon_killed'][0]), (2, 'ERROR'))

    def test_a_stray_hold_left_by_the_sweep_ends_1(self):
        c = right()
        c['cases'].append(('sweep', 'sweep', '-', '0', 'a6', {
            'trace.tsv': trace('a6', ('sweep', 'before'), ('sweep', 'after'), *WHOLE),
            'first.out': f'snap/a7 {commit_of("a7")}\n', 'first.rc': '0\n',
            'publish.out': f'snap/a6 {commit_of("a6")}\n', 'stray-holds.txt': ''}))
        c['pub'] += [('sweep', 'a6', 90, 507, commit_of('a6')), ('sweep', 'a7', 88, 508, commit_of('a7'))]
        c['rows'].update({'a6': (11, 91), 'a7': (10, 89)})
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_sweep_swept'][0]), (0, 'ok'), not_ok(lines))
        rc, lines = run_course(course=c, change=case_file('sweep', 'stray-holds.txt',
                                                          f'{POOL}@sweep-sweep\tskyzfs-publish\tThu\n'))
        self.assertEqual((rc, lines['pin_sweep_swept'][0]), (1, 'FAILED'))
        # D4 of #158's review: the publish that marks the directory failed.
        rc, lines = run_course(course=c, change=case_file('sweep', 'first.rc', '1\n'))
        self.assertEqual((rc, lines['pin_sweep_first_publish'][0]), (1, 'FAILED'))
        # Its status missing is the rig's error, not a 0.
        rc, lines = run_course(course=c, change=remove(os.path.join('cases', 'sweep', 'first.rc')))
        self.assertEqual(rc, 2)
        self.assertIn('rig_error', lines)

    def test_late_s_premise(self):
        c = right()
        c['cases'].append(('d6', 'late', '-', '0', 'a6', {
            'trace.tsv': trace('a6', *WHOLE, ns=1000),
            'record-stopped.txt': RECORD + 'publish a6 phase=asked count=2\n', 'record-stopped.rc': '0\n',
            'publish.out': f'snap/a6 {commit_of("a6")}\n'}))
        c['pub'].append(('d6', 'a6', 90, 507, commit_of('a6')))
        c['rows']['a6'] = (11, 91)
        # Continued at 1105: the look (1110) comes after it, before done (1150).
        rc, lines = run_course(course=c, change=write('marks.tsv', '1105\tcontinued-d6\n'))
        self.assertEqual((rc, lines['premise_d6_answer_lost'][0]), (0, 'ok'), not_ok(lines))
        rc, lines = run_course(course=c, change=write('marks.tsv', '1125\tcontinued-d6\n'))
        self.assertEqual((rc, lines['premise_d6_answer_lost'][0]), (2, 'ERROR'))
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_d6_answer_lost'][0]), (2, 'ERROR'))
        # The record that did not read when the agent was stopped.
        rc, lines = run_course(course=c, change=lambda d: (write('marks.tsv', '1105\tcontinued-d6\n')(d),
                                                           case_file('d6', 'record-stopped.rc', '6\n')(d)))
        self.assertEqual((rc, lines['premise_d6_answer_lost'][0]), (2, 'ERROR'))

    def test_restart_s_premise(self):
        c = right()
        c['cases'].append(('d1', 'restart', '-', '0', 'a6', {
            'trace.tsv': trace('a6', *WHOLE), 'between.txt': 'ref\tnone\nsnaprow\tnone\n',
            'publish.out': f'snap/a6 {commit_of("a6")}\n'}))
        c['pub'].append(('d1', 'a6', 90, 507, commit_of('a6')))
        c['rows']['a6'] = (11, 91)
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_d1_pending_at_the_loss'][0]), (0, 'ok'), not_ok(lines))
        landed = f'ref\t{commit_of("a6")}\t3\nsnaprow\t{S}\t11\t91\t{commit_of("a6")}\t{ULID}\t1\t{"0" * 64}\n'
        rc, lines = run_course(course=c, change=case_file('d1', 'between.txt', landed))
        self.assertEqual((rc, lines['premise_d1_pending_at_the_loss'][0]), (2, 'ERROR'))

    def test_a_restart_or_assist_that_did_not_happen_is_the_rig_s_error(self):
        c = right()
        c['cases'].append(('restart', 'restart', '-', '0', '-', {'retry.out': f'snap/a6 {commit_of("a6")}\n'}))
        c['pub'].append(('restart', 'a6', 90, 507, '-'))
        c['rows']['a6'] = (11, 91)
        rc, lines = run_course(course=c)
        # With no T its outcome fails too, and a FAILED line is never
        # hidden behind the premise's error (G3 of #158's review).
        self.assertEqual((rc, lines['premise_restart_pending_at_the_loss'][0]), (1, 'ERROR'))
        self.assertEqual(lines['pin_restart_outcome'][0], 'FAILED')
        c = right()
        c['cases'].append(('assist', 'assist', '-', '0', 'a6', {
            'trace.tsv': trace('a6', *WHOLE), 'publish.out': f'snap/a6 {commit_of("a6")}\n'}))
        c['pub'].append(('assist', 'a6', 90, 507, commit_of('a6')))
        c['rows']['a6'] = (11, 91)
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_assist_assisted'][0]), (2, 'ERROR'))
        c['cases'][-1][5]['trace.tsv'] = trace('a6', *(WHOLE[:12] + [('assist', 'before'), ('assist', 'after')]
                                                         + WHOLE[12:]))
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_assist_assisted'][0]), (0, 'ok'), not_ok(lines))

    def test_the_watcher_s_verdict_ends_1(self):
        rc, lines = run_course(change=write('watch.log', '1000\tVERDICT cut 5 differs\n', 'a'))
        self.assertEqual((rc, lines['pin_watcher_reads'][0]), (1, 'FAILED'))

    def test_files_that_do_not_read_are_the_rig_s_error(self):
        for change in (write('cases.tsv', 'x\tplain\n', 'a'), write('cases.tsv', 'x\tnone\t-\t0\t-\n', 'a'),
                       write('published.tsv', 'x\ty\n', 'a'), remove('watch.log'),
                       case_file('trace', 'publish.out', 'what\n'),
                       # The gone list missing, a line of one field or none
                       # named, and a record's dump with no status.
                       remove('published-gone.tsv'), write('published-gone.tsv', 'gone\n'),
                       write('published-gone.tsv', 'gone\t\n'),
                       remove(os.path.join('cases', 'trace', 'record-after.rc'))):
            rc, lines = run_course(change=change)
            self.assertEqual(rc, 2)
            self.assertIn('rig_error', lines)

    def test_one_name_published_is_too_few_for_the_controls(self):
        c = right()
        c['cases'] = c['cases'][:1]
        c['pub'] = c['pub'][:1]
        c['rows'] = {'a1': c['rows']['a1']}
        rc, lines = run_course(course=c)
        self.assertEqual(rc, 2)
        self.assertIn('the controls need two', lines['rig_error'][1])


class Survivors(unittest.TestCase):
    """G9 of #158's review: the plants that no test killed."""

    def test_killed_at_reads_the_hook_s_line_for_the_point(self):
        # P02: the trace ends where the case names, the hook's line names
        # another point, or names two.
        t = ac.read_trace(trace('x', ('ask', 'after'), ('asked', 'after'), ('ask', 'after')))
        right_line = 'skyzfs-agent: test hook --kill-at: killed at ask after (2)\n'
        other = 'skyzfs-agent: test hook --kill-at: killed at asked after (1)\n'
        self.assertEqual(ac.killed_at('ask:after:2', '137', right_line, t), [])
        self.assertTrue(ac.killed_at('ask:after:2', '137', other, t))
        self.assertTrue(ac.killed_at('ask:after:2', '137', right_line + other, t))

    def test_expected_of_each_scenario_that_publishes(self):
        # P17, P18, P20, with and without a kill.
        for scen in ('restart', 'assist', 'sweep', 'dhook', 'late', 'both'):
            self.assertEqual(ac.expected(scen, '', False, 't'), 'published', scen)
        self.assertEqual(ac.expected('both', 'ask:after:1', False, 't'), 'published')
        self.assertEqual(ac.expected('late', 'asked:after:1', False, 't'), 'published')

    def test_first_rc_names_each_scenario_judged_with_no_kill(self):
        # N20, N23: gone and both always kill; unmarked has its own pin.
        self.assertEqual(set(ac.FIRST_RC), set(ac.EXPECTED) - set(ac.KILLED_ONLY) - {'unmarked'})
        self.assertEqual({k for k, v in ac.FIRST_RC.items() if v != '0'}, {'destroy', 'tooold'})

    def test_a_tooold_case_with_no_kill_ends_its_first_run_3(self):
        # N18.
        def course(rc):
            c = right()
            c['cases'].append(('K-tooold', 'tooold', '-', rc, 'o1', {
                'trace.tsv': trace('o1', ('enrol', 'before'), ('intent', 'after'), ('abandon', 'before')),
                'publish.out': 'abandoned o1 too-old\n'}))
            return c
        rc, lines = run_course(course=course('3'))
        self.assertEqual(lines['pin_K-tooold_first_run'][0], 'ok', not_ok(lines))
        self.assertEqual(lines['pin_K-tooold_outcome'][0], 'ok', not_ok(lines))
        rc, lines = run_course(course=course('0'))
        self.assertEqual((rc, lines['pin_K-tooold_first_run'][0]), (1, 'FAILED'))

    def test_a_gone_or_both_case_with_no_kill_is_the_rig_s_error(self):
        # N20, N23: such a case has nothing to judge its first run by.
        c = right()
        c['cases'] = [(i, s, '-' if i == 'gone' else k, '0' if i == 'gone' else r, t, own)
                      for i, s, k, r, t, own in c['cases']]
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_gone_killed'][0]), (2, 'ERROR'))
        self.assertNotIn('pin_gone_first_run', lines)
        c = right()
        c['cases'].append(('b-x', 'both', '-', '0', 'a6', {
            'trace.tsv': trace('a6', *WHOLE), 'publish.out': f'snap/a6 {commit_of("a6")}\n',
            'both-kill.tsv': 'killed_rc\t137\n'}))
        c['pub'].append(('b-x', 'a6', 90, 506, commit_of('a6')))
        c['rows']['a6'] = (10, 91)
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_b-x_killed'][0]), (2, 'ERROR'))
        self.assertNotIn('pin_b-x_first_run', lines)

    def test_a_missed_control_beside_the_rig_s_error_ends_1(self):
        # N62-N64, P58, Q17: a control missed is a verdict, whatever else
        # the course had.
        err = case_file('d-carry-swap', 'kill.tsv', 'killed_rc\tnone\nserve\t2\n')
        rc, lines = run_course(change=err)
        self.assertEqual((rc, lines['premise_d-carry-swap_daemon_killed'][0]), (2, 'ERROR'))
        rc, lines = run_course(change=err, miss_files=True)
        self.assertEqual((rc, lines['ctl_files'][0]), (1, 'MISSED'))
        rc, lines = run_course(change=err, miss_snap=True)
        self.assertEqual(rc, 1)
        self.assertNotEqual(lines['ctl_early'][0], 'CAUGHT_AT_SNAPSHOT')
        with mock.patch.object(ac, 'judge_manifest_json', return_value=[]):
            rc, lines = run_course(change=err)
        self.assertEqual((rc, lines['ctl_manifest'][0]), (1, 'MISSED'))
        with mock.patch.object(ac, 'judge_manifest_json', return_value=[]):
            rc, lines = run_course()
        self.assertEqual((rc, lines['ctl_manifest'][0]), (1, 'MISSED'))

    def test_the_carry_helpers(self):
        # R03: a row on a cut that opens only with a rewind; R11: the first
        # carrier opens as itself; R06: an ask refused, or of another name.
        cuts = [(9, 90, False, 1, 'l9'), (10, 91, True, 1, 'l10'), (11, 92, True, 1, 'l11')]
        ask = (5, 85, 8)
        self.assertEqual(ac.judge_carried(cuts, {'number': 10, 'group': 91}, ask), [])
        self.assertTrue(ac.judge_carried(cuts, {'number': 9, 'group': 90}, ask))
        self.assertEqual(ac.first_carrier(cuts, ask), (10, 91))
        self.assertIsNone(ac.first_carrier(cuts[:1], ask))

        def asked(result, ns='5'):
            return {'event': 'publish', 'op': 'asked', 'result': result, 'sector': '85', 'sectors': '8', 'ns': ns}
        self.assertEqual(ac.last_ask([{'lines': [asked('a5 accepted')]}], 'a5'), ('5', 85, 8))
        for result in ('a5 refused', 'a5x accepted', 'a55 accepted'):
            self.assertIsNone(ac.last_ask([{'lines': [asked(result)]}], 'a5'), result)
        self.assertEqual(ac.last_ask([{'lines': [asked('a5 accepted'), asked('a5 refused', '6')]}], 'a5'),
                         ('5', 85, 8))

    def test_a_carry_hook_whose_name_has_no_row_ends_1(self):
        # R09.
        c = right()
        del c['rows']['a5']
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_d-carry-swap_carried_by_the_last_request'][0]), (1, 'FAILED'))


if __name__ == '__main__':
    unittest.main()
