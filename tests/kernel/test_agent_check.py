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
                'record-before.txt': 'absent\n', 'stray-holds.txt': STRAY,
                'retry.out': f'snap/a3 {commit_of("a3")}\n', 'retry.rc': '0\n'}),
            ('gone', 'gone', 'holding:before:1', '137', 'g1', {
                'trace.tsv': trace('g1', *upto('holding', 'before')),
                'publish.err': 'skyzfs-agent: test hook --kill-at: killed at holding before (1)\n',
                'destroy.log': '', 'settle.out': 'abandoned g1 destroyed\n',
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
        out = args[args.index('--out') + 1]
        if t not in self.ROWS:
            return 0, 'ref\tnone\nsnaprow\tnone\nmanifest\tnone\n'
        n, g, c = self.ROWS[t]
        m = manifest_of(t, *self.PUB[t])
        with open(out, 'wb') as f:
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


def log_text(cuts):
    """[(number, group, itself)] -> a request log of one instance."""
    out, ns = ['1000\t100\tstart\t0\t-\t-\t-\t64'], 1000
    for n, g, itself in cuts:
        ns += 10
        out.append(f'{ns}\t100\tcut\t-\t-\t{"itself" if itself else "rewind"}\t{n}\t{g}')
    return ''.join(ln + '\n' for ln in out)


def run_course(name='arare', change=None, fail=(), miss_files=False, miss_snap=False, course=None):
    c = course or right()
    d = tempfile.mkdtemp()
    try:
        files = {'watch.log': '1000\tread cut 5\n', 'marks.tsv': '',
                 'requests.tsv': log_text([(n, g, True) for n, g in sorted(c['rows'].values())]),
                 'published.tsv': ''.join(f'{a}\t{n}\t{t}\t{g}\t{p}\n' for a, n, t, g, p in c['pub']),
                 'cases.tsv': ''.join(f'{i}\t{s}\t{k}\t{r}\t{t}\n' for i, s, k, r, t, _ in c['cases'])}
        for _, n, _, _, _ in c['pub']:
            files[f'snap-{n}.sha256'] = sums_of(n)
        for cid, _, _, _, _, own in c['cases']:
            base = {'record-after.txt': RECORD, 'holds.txt': '', 'manifests.txt': '', 'assist.txt': ASSIST,
                    'settle.rc': '0\n'}
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
        self.assertEqual(ac.new_points(t, base), ['ask:before:2', 'sweep:before:1'])

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

    def test_expected(self):
        self.assertEqual(ac.expected('plain', '', False), 'published')
        self.assertIsNone(ac.expected('plain', 'ask:after:1', False))
        self.assertIsNone(ac.expected('destroy', 'ask:after:1', False))
        self.assertEqual(ac.expected('destroy', '', True), 'abandoned destroyed')
        self.assertEqual(ac.expected('tooold', '', False), 'abandoned too-old')
        self.assertEqual(ac.expected('unmarked', '', False), 'absent')

    def test_clean(self):
        self.assertEqual(ac.clean(RECORD, '', '', ASSIST), [])
        self.assertEqual(len(ac.clean(RECORD + 'publish t phase=held count=1\n', '', '', ASSIST)), 1)
        self.assertEqual(len(ac.clean(RECORD, STRAY, '', ASSIST)), 1)
        self.assertEqual(len(ac.clean(RECORD, f'{POOL}@x\tother\tThu\n', '', ASSIST)), 0)
        self.assertEqual(len(ac.clean(RECORD, '', 't\n', ASSIST)), 1)
        self.assertEqual(len(ac.clean(RECORD, '', '', ASSIST + f'{POOL}/fs\toff\tlocal\n')), 1)
        self.assertEqual(len(ac.clean('', '', '', ASSIST)), 1)
        self.assertEqual(ac.clean(RECORD + 'abandoned t why=destroyed at_ms=1\n', '', '', ASSIST), [])

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
                  'premise_gone_killed', 'premise_d-carry-swap_daemon_killed', 'pin_d-carry-swap_carried_by_the_killed_cut',
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
        c = right()
        c['pub'].append(('gone', 'g1', 75, 506, '-'))
        c['rows']['g1'] = (10, 95)
        rc, lines = run_course(course=c)
        self.assertEqual(rc, 1)
        self.assertEqual(lines['pin_gone_outcome'][0], 'FAILED')

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

    def test_carry_swap_on_another_cut_ends_1_and_carry_put_is_info(self):
        c = right()
        c['rows']['a5'] = (8, 85)
        c['rows']['a4'] = (7, 81)
        c['rows']['a3'] = (6, 71)
        c['rows']['a2'] = (5, 61)
        c['rows']['a1'] = (4, 52)
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_d-carry-swap_carried_by_the_killed_cut'][0]), (1, 'FAILED'))
        put = with_case('d-carry-swap', **{
            'serve-killed.log': 'skyzfsd: test hook --kill-at-upload: killed carry-put:1: after the first segment '
                                'PUT of cut 8 (group 84), chosen to carry publish 1, landed (k)\n'})
        put['cases'] = [('d-carry-put' if i == 'd-carry-swap' else i, s, k, r, t, own)
                        for i, s, k, r, t, own in put['cases']]
        put['pub'] = [('d-carry-put' if p[0] == 'd-carry-swap' else p[0],) + p[1:] for p in put['pub']]
        rc, lines = run_course(course=put)
        self.assertEqual(rc, 0, not_ok(lines))
        self.assertEqual(lines['obs_d-carry-put_carried_by'][0], 'INFO')
        self.assertIn('a later cut', lines['obs_d-carry-put_carried_by'][1])

    def test_a_daemon_not_killed_by_its_hook_is_the_rig_s_error(self):
        rc, lines = run_course(change=case_file('d-carry-swap', 'kill.tsv', 'killed_rc\tnone\nserve\t2\n'))
        self.assertEqual((rc, lines['premise_d-carry-swap_daemon_killed'][0]), (2, 'ERROR'))
        rc, lines = run_course(change=case_file('d-carry-swap', 'serve-killed.log', ''))
        self.assertEqual((rc, lines['premise_d-carry-swap_daemon_killed'][0]), (2, 'ERROR'))

    def test_a_stray_hold_left_by_the_sweep_ends_1(self):
        c = right()
        c['cases'].append(('sweep', 'sweep', '-', '0', 'a6', {
            'trace.tsv': trace('a6', ('sweep', 'before'), ('sweep', 'after'), *WHOLE),
            'first.out': f'snap/a7 {commit_of("a7")}\n', 'publish.out': f'snap/a6 {commit_of("a6")}\n',
            'stray-holds.txt': ''}))
        c['pub'] += [('sweep', 'a6', 90, 507, commit_of('a6')), ('sweep', 'a7', 88, 508, commit_of('a7'))]
        c['rows'].update({'a6': (11, 91), 'a7': (10, 89)})
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['pin_sweep_swept'][0]), (0, 'ok'), not_ok(lines))
        rc, lines = run_course(course=c, change=case_file('sweep', 'stray-holds.txt',
                                                          f'{POOL}@sweep-sweep\tskyzfs-publish\tThu\n'))
        self.assertEqual((rc, lines['pin_sweep_swept'][0]), (1, 'FAILED'))

    def test_late_s_premise(self):
        c = right()
        c['cases'].append(('d6', 'late', '-', '0', 'a6', {
            'trace.tsv': trace('a6', *WHOLE, ns=1000),
            'record-stopped.txt': RECORD + 'publish a6 phase=asked count=2\n',
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
        c['cases'].append(('restart', 'restart', '-', '1', '-', {'retry.out': f'snap/a6 {commit_of("a6")}\n'}))
        c['pub'].append(('restart', 'a6', 90, 507, '-'))
        c['rows']['a6'] = (11, 91)
        rc, lines = run_course(course=c)
        self.assertEqual((rc, lines['premise_restart_pending_at_the_loss'][0]), (2, 'ERROR'))
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
                       case_file('trace', 'publish.out', 'what\n')):
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


if __name__ == '__main__':
    unittest.main()
