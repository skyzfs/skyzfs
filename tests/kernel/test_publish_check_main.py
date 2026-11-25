#!/usr/bin/env python3
"""The channel rig's check (publish_check.py), its `main` from end to end
with no kernel and no object store: node B is a stand-in that answers
`snap-row` and the stages as a right daemon's publish leaves them, and
each test changes one thing and reads the lines and the exit code (the
review of #139, N1). A right recording ends 0; a pin that fails ends 1,
and so does a control that misses (M3); a premise that does not hold, or
anything the rig cannot read, ends 2.
"""
import contextlib
import hashlib
import io
import os
import shutil
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import nodeb_check  # noqa: E402
import publish_check as pc  # noqa: E402

A = 'a' * 32
S = 'b' * 32
ULID = '01ARZ3NDEKTSV4RRFFQ69G5FAV'
PID = '100'
PID2 = '200'


def commit_of(t):
    return hashlib.sha256(t.encode()).hexdigest()


def manifest_of(t):
    return f'{hashlib.sha256(("file of " + t).encode()).hexdigest()}  {t}/file\n'.encode()


class FakeNodeB:
    """node B as a right daemon's publish leaves it. ROWS: T -> (number,
    group, commit) of the row and ref snap/T; T absent: neither. FAIL: the
    names whose import fails; MISS_FILES: node B does not see a digest
    that differs (the control ctl_files then misses)."""
    ROWS = {}
    FAIL = set()
    MISS_FILES = False
    ROW_ONLY = set()

    def __init__(self, rdir, logdev):
        self.dir = os.path.join(rdir, 'nodeb')
        os.makedirs(self.dir, exist_ok=True)

    def tool_run(self, args, timeout=600):
        assert args[0] == 'snap-row', args
        t = args[args.index('--snapshot') + 1]
        out = args[args.index('--out') + 1] if '--out' in args else None
        if t not in self.ROWS:
            return 0, 'ref\tnone\nsnaprow\tnone\n'
        n, g, c = self.ROWS[t]
        m = manifest_of(t)
        text = ('ref\tnone\n' if t in self.ROW_ONLY else f'ref\t{c}\t3\n') + \
            f'snaprow\t{S}\t{n}\t{g}\t{c}\t{ULID}\t{len(m)}\t{hashlib.sha256(m).hexdigest()}\n'
        if out:
            with open(out, 'wb') as f:
                f.write(m)
            text += f'manifest\t{len(m)}\t{hashlib.sha256(m).hexdigest()}\tmatches\n'
        return 0, text

    def verify(self, volume, ref, snap, want, damaged):
        seen = nodeb_check.Seen()
        right = nodeb_check.read_sums(manifest_of(snap).decode())
        for st in nodeb_check.STAGES:
            ok = not (st == 'import' and snap in self.FAIL) and \
                not (st == 'files' and want != right and not self.MISS_FILES)
            seen.put(st, ok, f'{st} of {ref}')
        return seen


def log_text(instances):
    """[(pid, [(number, group, itself) or (event, op, result)])] -> the request log."""
    out, ns = [], 1000
    for pid, lines in instances:
        out.append(f'{ns}\t{pid}\tstart\t0\t-\t-\t-\t64')
        for ln in lines:
            ns += 10
            if isinstance(ln[0], int):
                out.append(f'{ns}\t{pid}\tcut\t-\t-\t{"itself" if ln[2] else "rewind"}\t{ln[0]}\t{ln[1]}')
            else:
                out.append(f'{ns}\t{pid}\t{ln[0]}\t-\t{ln[1]}\t-\t-\t{ln[2]}')
        ns += 10
    return ''.join(ln + '\n' for ln in out)


def node_b_row(n, g):
    return f'head\t{"f" * 64}\t9\nrow\t{S}\t{n}\t{g}\nrow_commit\t{"f" * 64}\nlease\tskyzfsd\t1\t0\n'


# Each course as a right daemon leaves it: the answers, the requests
# (T: C, G, the snapshot's createtxg), node B's rows, the request log,
# and the course's own files.
COURSES = {
    'pplain': {
        'ends': {'t1': 'done commit=' + commit_of('t1'), 't1-again': 'accepted',
                 't1-again-end': 'done commit=' + commit_of('t1'), 't1-other': 'accepted',
                 't1-other-end': 'refused taken snap/t1 names another manifest',
                 't2': 'done commit=' + commit_of('t2'), 'att': 'refused attachment not this one',
                 'state': 'refused state not this one', 't4': 'done commit=' + commit_of('t4'),
                 't5': 'expired', 't7': 'pending after 12', 't7-other': 'refused asked other parameters',
                 't7-withdraw': 'withdrawn', 't7-again': 'accepted', 't7-end': 'done commit=' + commit_of('t7')},
        'requests': {'t1': (7, 50, 50), 't2': (8, 51, 51), 't4': (9, 72, 52), 't7': (12, 73, 73)},
        'rows': {'t1': (8, 50), 't2': (9, 51), 't4': (12, 72), 't7': (13, 73)},
        'log': [(PID, [(8, 50, True), (9, 51, True), (10, 52, True), (11, 60, True), (12, 72, True),
                       (13, 73, True)])],
        'files': {},
    },
    'prewind': {
        'ends': {'r1': 'done commit=' + commit_of('r1')},
        'requests': {'r1': (7, 50, 50)},
        'rows': {'r1': (10, 52)},
        'log': [(PID, [(8, 50, False), (9, 51, False), (10, 52, True)])],
        'files': {'rewind-status.txt': 'cuts_held 3\n', 'rewind-row.txt': node_b_row(9, 51)},
    },
    'pkill': {
        'ends': {'tw': 'pending after 7', 'tw-withdraw': 'withdrawn', 'tk': 'pending after 9',
                 'tw-after': 'unknown', 'tk-after': 'unknown', 'tk-again': 'accepted',
                 'tk-end': 'done commit=' + commit_of('tk'), 'tw-last': 'unknown'},
        'requests': {'tw': (7, 50, 20), 'tk': (9, 80, 50)},
        'rows': {'tk': (12, 80)},
        'log': [(PID, [(8, 49, True), ('publish', 'withdraw', 'tw withdrawn'), (9, 50, True)]),
                (PID2, [(12, 80, True)])],
        'files': {'tw-row.txt': node_b_row(9, 50),
                  'tw-status.txt': 'publish_withdrawn 1\npublish_pending 0\npublish_published 0\n'},
    },
    'pkillpub': {
        'ends': {'p1': 'unknown', 'p1-after': 'unknown', 'p1-again': 'accepted',
                 'p1-end': 'done commit=' + commit_of('p1')},
        'requests': {'p1': (7, 50, 50)},
        'rows': {'p1': (8, 50)},
        'log': [(PID, [(8, 50, True)]), (PID2, [])],
        'files': {'kill.tsv': 'killed_rc\t137\n',
                  'serve.log': 'skyzfsd: test hook --kill-at-upload: killed after the transaction of publish 1 (p1) '
                               'landed, before the uploader knew it\n'},
        'between': 'p1',
    },
    'pkillman': {
        'ends': {'m1': 'unknown', 'm1-after': 'unknown', 'm1-again': 'accepted',
                 'm1-end': 'done commit=' + commit_of('m1')},
        'requests': {'m1': (7, 50, 50)},
        'rows': {'m1': (8, 50)},
        'log': [(PID, [(8, 50, True)]), (PID2, [])],
        'files': {'kill.tsv': 'killed_rc\t137\n',
                  'serve.log': 'skyzfsd: test hook --kill-at-upload: killed after the framed manifest PUT 1 landed '
                               '(v/skyzfs/manifest/x), before its ref was made\n',
                  'm1-between.txt': 'ref\tnone\nsnaprow\tnone\n'},
    },
    'pchannel': {
        'ends': {'nu': 'refused no-upload', 'version': 'refused version not 1',
                 'malformed': 'refused malformed', 'name': 'refused name', 'peer': 'refused peer its own user',
                 'cuts': 'cuts 3'},
        'requests': {},
        'rows': {},
        'log': [(PID, []), (PID2, [(4, 9, True)])],
        'files': {'channel-status.txt': 'control_answered 4\ncontrol_peer 1\ncontrol_malformed 2\n',
                  'socket-mode.txt': '600\n'},
    },
}


def run_course(name, change=None, rows=None, fail=(), miss_files=False, row_only=(), patch=None):
    """A course's directory, changed by change(d) where given -> (exit
    code, {key: (verdict, detail)})."""
    c = COURSES[name]
    d = tempfile.mkdtemp(prefix='pc-main-')
    try:
        with open(os.path.join(d, 'ends.tsv'), 'w') as f:
            f.write(''.join(f'{k}\t{v}\n' for k, v in c['ends'].items()))
        with open(os.path.join(d, 'published.tsv'), 'w') as f:
            for t, (after, group, txg) in c['requests'].items():
                m = os.path.join(d, f'manifest-{t}')
                with open(m, 'wb') as g:
                    g.write(manifest_of(t))
                f.write(f'{t}\t{group}\t{A}\t{S}\t{after}\t{m}\t{txg}\n')
        with open(os.path.join(d, 'requests.tsv'), 'w') as f:
            f.write(log_text(c['log']))
        for fn, text in c['files'].items():
            with open(os.path.join(d, fn), 'w') as f:
                f.write(text)
        if 'between' in c:
            # What landed before the kill: the ref on the commit p1-end names.
            t = c['between']
            with open(os.path.join(d, f'{t}-between.txt'), 'w') as f:
                f.write(f'ref\t{commit_of(t)}\t3\nsnaprow\t{S}\t8\t50\t{commit_of(t)}\t{ULID}\t1\t{"0" * 64}\n')
        if change:
            change(d)
        fake = type('Fake', (FakeNodeB,), {
            'ROWS': {t: (n, g, commit_of(t)) for t, (n, g) in (rows or c['rows']).items()},
            'FAIL': set(fail), 'MISS_FILES': miss_files, 'ROW_ONLY': set(row_only)})
        argv = ['publish_check.py', name, d, os.path.join(d, 'requests.tsv'), '-', '1048576']
        out = io.StringIO()
        with mock.patch.object(nodeb_check, 'NodeB', fake), mock.patch.object(sys, 'argv', argv), \
                contextlib.ExitStack() as stack, contextlib.redirect_stdout(out):
            if patch:
                stack.enter_context(patch)
            try:
                pc.main()
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


def verdicts(lines, *keys):
    return [lines.get(k, ('none', ''))[0] for k in keys]


class Right(unittest.TestCase):
    def test_each_course_as_a_right_daemon_leaves_it_ends_0(self):
        for name in COURSES:
            rc, lines = run_course(name)
            self.assertEqual(rc, 0, (name, {k: v for k, v in lines.items() if v[0] not in ('ok', 'INFO', 'CAUGHT',
                                                                                          'CAUGHT_AT_FILES')}))
            self.assertNotIn('rig_error', lines)

    def test_pplain_s_controls_are_caught(self):
        _, lines = run_course('pplain')
        self.assertEqual(verdicts(lines, 'ctl_below', 'ctl_commit', 'ctl_files'),
                         ['CAUGHT', 'CAUGHT', 'CAUGHT_AT_FILES'])
        self.assertEqual(verdicts(lines, 'premise_t4_cuts_below_g', 'pin_t5_absent', 'pin_t3_absent',
                                  'pin_t1_nodeb', 'pin_asking_twice_is_once'), ['ok'] * 5)


class Controls(unittest.TestCase):
    """M3: a control that misses fails the run."""

    def test_a_missed_row_control_ends_1(self):
        real = pc.judge_row

        def no_group(got, req, commit):
            return real(got, dict(req, group=0), commit)
        rc, lines = run_course('pplain', patch=mock.patch.object(pc, 'judge_row', no_group))
        self.assertEqual(lines['ctl_below'][0], 'MISSED')
        self.assertEqual(lines['ctl_commit'][0], 'CAUGHT')
        self.assertEqual(rc, 1)

    def test_a_missed_files_control_ends_1(self):
        rc, lines = run_course('pplain', miss_files=True)
        self.assertEqual(lines['ctl_files'][0], 'MISSED')
        self.assertEqual(rc, 1)


class Pins(unittest.TestCase):
    def test_a_name_published_on_a_cut_below_g_ends_1(self):
        rc, lines = run_course('pplain', rows=dict(COURSES['pplain']['rows'], t4=(11, 60)))
        self.assertEqual(lines['pin_t4_row'][0], 'FAILED')
        self.assertEqual(rc, 1)

    def test_an_absent_name_with_a_row_and_no_ref_ends_1(self):
        rc, lines = run_course('pplain', rows=dict(COURSES['pplain']['rows'], t5=(14, 74)), row_only=('t5',))
        self.assertEqual(lines['pin_t5_absent'], ('FAILED', '[server] a row'))
        self.assertEqual(rc, 1)

    def test_node_b_failing_a_stage_ends_1(self):
        rc, lines = run_course('pplain', fail=('t2',))
        self.assertEqual(lines['pin_t2_nodeb'][0], 'FAILED')
        self.assertEqual(rc, 1)

    def test_an_answer_outside_the_table_ends_1(self):
        def change(d):
            with open(os.path.join(d, 'ends.tsv')) as f:
                text = f.read()
            with open(os.path.join(d, 'ends.tsv'), 'w') as f:
                f.write(text.replace('peer\trefused peer its own user', 'peer\t'))
        rc, lines = run_course('pchannel', change)
        self.assertEqual(lines['pin_answer_peer'][0], 'FAILED')
        self.assertEqual(rc, 1)

    def test_pchannel_s_counts_are_exact(self):
        for status, key in (('control_answered 5\ncontrol_peer 1\ncontrol_malformed 2\n', 'pin_status_control_answered'),
                            ('control_answered 4\ncontrol_peer 2\ncontrol_malformed 2\n', 'pin_status_control_peer'),
                            ('control_answered 4\ncontrol_peer 1\ncontrol_malformed 3\n',
                             'pin_status_control_malformed')):
            rc, lines = run_course('pchannel', write('channel-status.txt', status))
            self.assertEqual((rc, lines[key][0]), (1, 'FAILED'), status)

    def test_a_socket_the_daemon_made_wider_ends_1(self):
        rc, lines = run_course('pchannel', write('socket-mode.txt', '666\n'))
        self.assertEqual((rc, lines['pin_socket_mode'][0]), (1, 'FAILED'))

    def test_pkill_s_status_counting_tw_pending_ends_1(self):
        rc, lines = run_course('pkill', write('tw-status.txt', 'publish_withdrawn 0\npublish_pending 1\n'
                                                               'publish_published 0\n'))
        self.assertEqual((rc, lines['pin_tw_withdrawn_status'][0]), (1, 'FAILED'))

    def test_r1_made_by_the_rewind_cut_is_a_verdict_not_the_rig_s_error(self):
        rc, lines = run_course('prewind', rows={'r1': (9, 51)})
        self.assertEqual(lines['pin_r1_cut'][0], 'FAILED')
        self.assertEqual(lines['premise_rewind_cut_committed'][0], 'ok')
        self.assertEqual(rc, 1)

    def test_pkillpub_done_on_another_commit_than_landed_ends_1(self):
        def change(d):
            with open(os.path.join(d, 'p1-between.txt'), 'w') as f:
                f.write(f'ref\t{"9" * 64}\t3\nsnaprow\t{S}\t8\t50\t{"9" * 64}\t{ULID}\t1\t{"0" * 64}\n')
        rc, lines = run_course('pkillpub', change)
        self.assertEqual((rc, lines['pin_p1_found'][0]), (1, 'FAILED'))

    def test_pkillman_with_snap_m1_after_the_kill_ends_1(self):
        rc, lines = run_course('pkillman', write('m1-between.txt', f'ref\t{"9" * 64}\t3\nsnaprow\tnone\n'))
        self.assertEqual((rc, lines['pin_m1_not_before_the_transaction'][0]), (1, 'FAILED'))


class Premises(unittest.TestCase):
    """A course that did not make its case is the rig's error (exit 2)."""

    def test_t4_with_no_cut_below_g_ends_2(self):
        def change(d):
            with open(os.path.join(d, 'requests.tsv'), 'w') as f:
                f.write(log_text([(PID, [(8, 50, True), (9, 51, True), (12, 72, True), (13, 73, True)])]))
        rc, lines = run_course('pplain', change)
        self.assertEqual((rc, lines['premise_t4_cuts_below_g'][0]), (2, 'ERROR'))

    def test_prewind_whose_committed_cut_opens_as_itself_ends_2(self):
        def change(d):
            with open(os.path.join(d, 'requests.tsv'), 'w') as f:
                f.write(log_text([(PID, [(8, 50, False), (9, 51, True), (10, 52, True)])]))
        rc, lines = run_course('prewind', change)
        self.assertEqual((rc, lines['premise_rewind_cut_committed'][0]), (2, 'ERROR'))
        self.assertIn('opens as itself', lines['premise_rewind_cut_committed'][1])

    def test_prewind_with_no_cut_committed_after_the_ask_ends_2(self):
        rc, lines = run_course('prewind', write('rewind-row.txt', node_b_row(7, 50)))
        self.assertEqual((rc, lines['premise_rewind_cut_committed'][0]), (2, 'ERROR'))

    def test_prewind_with_r1_made_before_the_rewind_cut_ends_2(self):
        rc, lines = run_course('prewind', write('rewind-row.txt', node_b_row(10, 52)))
        self.assertEqual(rc, 2)

    def test_pkill_counting_only_the_next_instance_s_cuts_ends_2(self):
        """B3: cuts past tw's G that only the next instance counted are no premise."""
        def change(d):
            with open(os.path.join(d, 'requests.tsv'), 'w') as f:
                f.write(log_text([(PID, [(8, 49, True), ('publish', 'withdraw', 'tw withdrawn')]),
                                  (PID2, [(9, 50, True), (12, 80, True)])]))
        rc, lines = run_course('pkill', change)
        self.assertEqual((rc, lines['premise_tw_committed_past_g'][0]), (2, 'ERROR'))

    def test_pkill_whose_row_did_not_move_past_g_ends_2(self):
        rc, lines = run_course('pkill', write('tw-row.txt', node_b_row(8, 49)))
        self.assertEqual((rc, lines['premise_tw_committed_past_g'][0]), (2, 'ERROR'))

    def test_a_kill_that_was_not_the_hook_s_ends_2(self):
        for name, fn, text in (('pkillman', 'kill.tsv', 'killed_rc\t1\n'),
                               ('pkillman', 'serve.log', 'skyzfsd: killed by something else\n'),
                               ('pkillpub', 'serve.log', 'skyzfsd: test hook --kill-at-upload: killed after the '
                                                         'framed manifest PUT 1 landed (k), before its ref was made\n')):
            rc, lines = run_course(name, write(fn, text))
            key = f'premise_{pc.BETWEEN[name][0]}_killed_by_the_hook'
            self.assertEqual((rc, lines[key][0]), (2, 'ERROR'), (name, fn))

    def test_a_missing_file_is_the_rig_s_error(self):
        for name, fn in (('prewind', 'rewind-row.txt'), ('pkill', 'tw-status.txt'), ('pchannel', 'socket-mode.txt')):
            rc, lines = run_course(name, lambda d, fn=fn: os.remove(os.path.join(d, fn)))
            self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'), (name, fn))


if __name__ == '__main__':
    unittest.main()
