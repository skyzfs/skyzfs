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
    """[(pid, [(number, group, itself) or (event, op, result[, G, C])])] -> the request log.

    A publish's `asked` line carries G and C in its sector and sectors
    columns, as skyzfsd writes it (record.rs)."""
    out, ns = [], 1000
    for pid, lines in instances:
        out.append(f'{ns}\t{pid}\tstart\t0\t-\t-\t-\t64')
        for ln in lines:
            ns += 10
            if isinstance(ln[0], int):
                out.append(f'{ns}\t{pid}\tcut\t-\t-\t{"itself" if ln[2] else "rewind"}\t{ln[0]}\t{ln[1]}')
            else:
                g, c = ln[3:5] if len(ln) == 5 else ('-', '-')
                out.append(f'{ns}\t{pid}\t{ln[0]}\t-\t{ln[1]}\t{g}\t{c}\t{ln[2]}')
        ns += 10
    return ''.join(ln + '\n' for ln in out)


def look_of(t, n, g):
    """`look`'s `snap` answer as node B's row of T reads (FakeNodeB)."""
    m = manifest_of(t)
    return (f'snap commit={commit_of(t)} state={S} number={n} group={g} manifest-len={len(m)} '
            f'manifest-hash={hashlib.sha256(m).hexdigest()}')


def answer(label, text):
    """A change: LABEL's answer in ends.tsv replaced by TEXT."""
    def change(d):
        p = os.path.join(d, 'ends.tsv')
        with open(p) as f:
            lines = [ln for ln in f.read().splitlines() if ln.split('\t')[0] != label]
        with open(p, 'w') as f:
            f.write(''.join(ln + '\n' for ln in lines + [f'{label}\t{text}']))
    return change


def node_b_row(n, g):
    return f'head\t{"f" * 64}\t9\nrow\t{S}\t{n}\t{g}\nrow_commit\t{"f" * 64}\nlease\tskyzfsd\t1\t0\n'


# Each course as a right daemon leaves it: the answers, the requests
# (T: C, G, the snapshot's createtxg; a list where T was asked anew,
# the last the one judged), node B's rows, the request log,
# and the course's own files.
COURSES = {
    'pplain': {
        'ends': {'t1': 'done commit=' + commit_of('t1'), 't1-again': 'accepted',
                 't1-again-end': 'done commit=' + commit_of('t1'), 't1-look': look_of('t1', 8, 50),
                 't1-other': 'accepted',
                 't1-other-end': 'refused taken snap/t1 names another manifest',
                 't2': 'done commit=' + commit_of('t2'), 'att': 'refused attachment not this one',
                 'state': 'refused state not this one', 't4': 'done commit=' + commit_of('t4'),
                 't5': 'expired', 't5-look': 'absent', 't3-look': 'absent', 't7': 'pending after 12', 't7-other': 'refused asked other parameters',
                 't7-withdraw': 'withdrawn', 't7-again': 'accepted', 't7-end': 'done commit=' + commit_of('t7')},
        'requests': {'t1': (7, 50, 50), 't2': (8, 51, 51), 't4': (9, 72, 52), 't7': (12, 73, 73)},
        'rows': {'t1': (8, 50), 't2': (9, 51), 't4': (12, 72), 't7': (13, 73)},
        'log': [(PID, [(8, 50, True), (9, 51, True), (10, 52, True), (11, 60, True), (12, 72, True),
                       (13, 73, True)])],
        'files': {},
    },
    'prewind': {
        'ends': {'r1': 'refused rewound a rewind may have been counted after the count asked from',
                 'r1-look': 'refused store object storage answered with an error',
                 'r1-anew': 'accepted', 'r1-end': 'done commit=' + commit_of('r1')},
        # The first ask, refused, then the one asked anew past the top.
        'requests': {'r1': [(7, 50, 50), (10, 50, 50)]},
        'rows': {'r1': (11, 53)},
        # Before the rewind the pool reached group 52 (cut 6, the top);
        # cuts 8 and 9, after the first C and not above it, are the
        # refusal's ground.
        'log': [(PID, [(6, 52, True), (7, 45, False), (8, 50, False), (9, 51, False),
                       ('publish', 'asked', 'r1 refused rewound', 50, 7), (10, 52, True), (11, 53, True)])],
        'files': {'rewind-status.txt': 'cuts_held 3\n', 'rewind-row.txt': node_b_row(9, 51),
                  'rewind-top.txt': '52\n'},
    },
    'prewindf': {
        'ends': {'f1': 'pending after 3 s',
                 'f1-end': 'refused rewound a rewind may have been counted after the count asked from',
                 'f1-look': 'absent'},
        'requests': {'f1': (7, 1000050, 50)},
        'rows': {},
        # The export synced up to group 55 (cut 9); -F went back past it,
        # and cut 10, of group 55 again, is the mark that ended f1.
        'log': [(PID, [(8, 52, True), (9, 55, True), (10, 55, False), ('publish', 'ended', 'f1 refused rewound'),
                       (11, 56, True)])],
        'files': {'rewind-f.txt': 'zeroed 3 copies of the root block of group 55: DVA[0]=<0:1000:1000>\n'
                                  '== zpool import -d (the listing)\nimported with -F\n',
                  # f1 asked before the log's first cut (its clock is the log's), 900 s to live.
                  'agent.tsv': f'990\t995\tpublish --name f1 --attachment {A} --state {S} --after 7 '
                               f'--group 1000050 --manifest m --expiry-s 900\t0\taccepted\n'},
    },
    # prewindf's control: a plain import (session 2 ready at ns 1035, the
    # log's clock); cuts 10 and 11 after it go on above group 55, and c1
    # still waits.
    'prewindc': {
        'ends': {'c1': 'pending after 3 s', 'c1-end': 'pending after 60 s', 'c1-withdraw': 'withdrawn',
                 'c1-look': 'absent'},
        'requests': {'c1': (7, 1000050, 50)},
        'rows': {},
        'log': [(PID, [(8, 52, True), (9, 55, True), (10, 56, True), (11, 57, True),
                       ('publish', 'withdraw', 'c1 withdrawn'), (12, 58, True)])],
        'files': {'marks.tsv': '1005\tsession-1-begin\n1006\tsession-1-ready\n1025\tsession-1-end\n'
                               '1030\tsession-2-begin\n1035\tsession-2-ready\n'},
    },
    'pkill': {
        'ends': {'tw': 'pending after 7', 'tw-withdraw': 'withdrawn', 'tw-look': 'absent', 'tk': 'pending after 9',
                 'tw-after': 'unknown', 'tk-after': 'unknown', 'tk-again': 'refused rewound',
                 'tk-anew': 'accepted', 'tk-end': 'done commit=' + commit_of('tk'), 'tw-last': 'unknown'},
        'requests': {'tw': (7, 50, 20), 'tk': [(9, 80, 50), (11, 80, 50)]},
        'rows': {'tk': (12, 80)},
        'log': [(PID, [(8, 49, True), ('publish', 'withdraw', 'tw withdrawn'), (9, 50, True)]),
                (PID2, [(12, 80, True)])],
        'files': {'tw-row.txt': node_b_row(9, 50),
                  'tw-status.txt': 'publish_withdrawn 1\npublish_pending 0\npublish_published 0\n'},
    },
    'pkillpub': {
        'ends': {'p1': 'unknown', 'p1-after': 'unknown', 'p1-again': 'refused rewound', 'p1-anew': 'accepted',
                 'p1-end': 'refused taken',
                 # As node B's row of p1 reads (FakeNodeB): cut 8 of group 50 and its manifest.
                 'p1-look': look_of('p1', 8, 50)},
        'requests': {'p1': (7, 50, 50)},
        'rows': {'p1': (8, 50)},
        'log': [(PID, [(8, 50, True)]), (PID2, [])],
        'files': {'kill.tsv': 'killed_rc\t137\n',
                  'serve.log': 'skyzfsd: test hook --kill-at-upload: killed after the transaction of publish 1 (p1) '
                               'landed, before the uploader knew it\n'},
        'between': 'p1',
    },
    'pkillman': {
        'ends': {'m1': 'unknown', 'm1-after': 'unknown', 'm1-again': 'refused rewound', 'm1-anew': 'accepted',
                 'm1-end': 'done commit=' + commit_of('m1')},
        'requests': {'m1': [(7, 50, 50), (9, 50, 50)]},
        'rows': {'m1': (10, 50)},
        'log': [(PID, [(8, 50, True)]), (PID2, [(10, 50, True)])],
        'files': {'kill.tsv': 'killed_rc\t137\n',
                  'serve.log': 'skyzfsd: test hook --kill-at-upload: killed after the framed manifest PUT 1 landed '
                               '(v/skyzfs/manifest/x), before its ref was made\n',
                  'm1-between.txt': 'ref\tnone\nsnaprow\tnone\n'},
    },
    'pchannel': {
        'ends': {'nu': 'refused no-upload', 'version': 'refused version not 2',
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
            for t, reqs in c['requests'].items():
                m = os.path.join(d, f'manifest-{t}')
                with open(m, 'wb') as g:
                    g.write(manifest_of(t))
                for after, group, txg in (reqs if isinstance(reqs, list) else [reqs]):
                    f.write(f'{t}\t{group}\t{A}\t{S}\t{after}\t{m}\t{txg}\n')
        with open(os.path.join(d, 'requests.tsv'), 'w') as f:
            f.write(log_text(c['log']))
        for fn, text in c['files'].items():
            with open(os.path.join(d, fn), 'w') as f:
                f.write(text)
        with open(os.path.join(d, 'watch.log'), 'w') as f:
            f.write('1000\tfetched cut 1 (group 4, state x): 107 blocks, 107 changed\n2000\tstopped\n')
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

    def test_a_request_from_before_a_rewind_not_refused_rewound_ends_1(self):
        """A daemon without rule (R) (M2.6a): the request asked with the
        count from before the take-over or below the abandoned top is
        answered as M2.5 answered it."""
        for course, label, answer in (('prewind', 'r1', 'done commit=' + commit_of('r1')),
                                      ('pkill', 'tk-again', 'accepted'),
                                      ('pkillman', 'm1-again', 'accepted'),
                                      ('pkillpub', 'p1-again', 'accepted')):
            def change(d, label=label, answer=answer):
                with open(os.path.join(d, 'ends.tsv')) as f:
                    rows = [ln.split('\t', 1) for ln in f.read().splitlines()]
                with open(os.path.join(d, 'ends.tsv'), 'w') as f:
                    f.write(''.join(f'{k}\t{answer if k == label else v}\n' for k, v in rows))
            rc, lines = run_course(course, change)
            self.assertEqual((rc, lines[f'pin_answer_{label}'][0]), (1, 'FAILED'), course)

    def test_a_daemon_without_rule_r_at_the_ask_ends_1_with_its_own_log(self):
        """B2 of #141's review: the request log as such a daemon writes it
        (r1 `accepted` at its first ask, and made by a cut below the top),
        not the right daemon's with only the answer changed."""
        def change(d):
            with open(os.path.join(d, 'requests.tsv'), 'w') as f:
                f.write(log_text([(PID, [(6, 52, True), (7, 45, False), (8, 50, False), (9, 51, False),
                                         ('publish', 'asked', 'r1 accepted', 50, 7), (10, 52, True),
                                         ('publish', 'ended', 'r1 done commit=' + commit_of('r1'))])]))
            answer('r1', 'done commit=' + commit_of('r1'))(d)
        rc, lines = run_course('prewind', change, rows={'r1': (10, 52)})
        self.assertEqual(rc, 1)
        self.assertEqual(verdicts(lines, 'pin_answer_r1', 'premise_r1_asked_after_a_rewind_cut'), ['FAILED', 'ok'])
        self.assertNotIn('rig_error', lines)

    def test_prewind_s_premise_is_of_the_first_ask(self):
        """The rewind cut (9, group 51) was counted after the first ask's
        C (7) and before the second's (10): the premise holds of the first."""
        rc, lines = run_course('prewind')
        self.assertEqual((rc, lines['premise_rewind_cut_committed'][0]), (0, 'ok'))
        self.assertIn('counted after C = 7', lines['premise_rewind_cut_committed'][1])
        self.assertIn('asked after 10', lines['pin_r1_row'][1])

    def test_r1_made_at_or_below_the_top_ends_1(self):
        rc, lines = run_course('prewind', write('rewind-top.txt', '53\n'))
        self.assertEqual((rc, lines['pin_r1_above_top'][0]), (1, 'FAILED'))
        self.assertEqual(verdicts(lines, 'pin_r1_row', 'pin_r1_cut', 'premise_r1_asked_after_a_rewind_cut',
                                  'premise_rewind_cut_committed'), ['ok'] * 4)

    def test_pkillpub_done_on_another_commit_than_landed_ends_1(self):
        def change(d):
            with open(os.path.join(d, 'p1-between.txt'), 'w') as f:
                f.write(f'ref\t{"9" * 64}\t3\nsnaprow\t{S}\t8\t50\t{"9" * 64}\t{ULID}\t1\t{"0" * 64}\n')
        rc, lines = run_course('pkillpub', change)
        self.assertEqual((rc, lines['pin_p1_found'][0]), (1, 'FAILED'))

    def test_r1_made_by_an_earlier_rewind_only_cut_is_a_verdict_not_the_rig_s_error(self):
        """#139's second look, D1: the fault lands on a rewind-only cut
        counted before the one node B's row named."""
        rc, lines = run_course('prewind', rows={'r1': (8, 50)})
        self.assertEqual(lines['pin_r1_cut'][0], 'FAILED')
        self.assertEqual(lines['premise_rewind_cut_committed'][0], 'ok')
        self.assertEqual(rc, 1)

    def test_t1_asked_again_done_on_another_commit_ends_1(self):
        def change(d):
            with open(os.path.join(d, 'ends.tsv')) as f:
                text = f.read()
            with open(os.path.join(d, 'ends.tsv'), 'w') as f:
                f.write(text.replace('t1-again-end\tdone commit=' + commit_of('t1'),
                                     't1-again-end\tdone commit=' + commit_of('t1x')))
        rc, lines = run_course('pplain', change)
        self.assertEqual(lines['pin_answer_t1-again-end'][0], 'ok')
        self.assertEqual((rc, lines['pin_asking_twice_is_once'][0]), (1, 'FAILED'))

    def test_a_look_that_does_not_name_the_row_ends_1(self):
        """#140's review, N6: a look's fields against node B's row, for
        p1 (its look stands in for done) and t1 (a look beside done)."""
        for name, t in (('pkillpub', 'p1'), ('pplain', 't1')):
            look = COURSES[name]['ends'][f'{t}-look']
            for old, new in (('number=8', 'number=9'), ('group=50', 'group=51'),
                             ('manifest-len=', 'manifest-len=1'), (f'state={S}', f'state={A}'),
                             ('manifest-hash=', 'manifest-hash=0'), ('commit=', 'commit=0')):
                def change(d, old=old, new=new, look=look):
                    with open(os.path.join(d, 'ends.tsv')) as f:
                        text = f.read()
                    with open(os.path.join(d, 'ends.tsv'), 'w') as f:
                        f.write(text.replace(look, look.replace(old, new, 1)))
                rc, lines = run_course(name, change)
                self.assertEqual((rc, lines[f'pin_{t}_look'][0]), (1, 'FAILED'), (name, new))

    def test_a_look_that_finds_what_is_not_there_ends_1(self):
        """`absent` for names never made; never `absent` with the store down."""
        for name, label, text in (('pplain', 't5-look', look_of('t5', 14, 74)), ('pplain', 't3-look', 'unknown'),
                                    ('prewind', 'r1-look', 'absent')):
            rc, lines = run_course(name, answer(label, text))
            self.assertEqual((rc, lines[f'pin_answer_{label}'][0]), (1, 'FAILED'), label)

    def test_prewindf_s_publish_not_ended_or_published_ends_1(self):
        """B2 of #141's review: with the log as such a daemon writes it."""
        def daemon(lines, end):
            def change(d):
                write('requests.tsv', log_text([(PID, lines)]))(d)
                answer('f1-end', end)(d)
            return change
        ground = [(8, 52, True), (9, 55, True), (10, 55, False)]
        for why, change, pins in (
                ('never ended', daemon(ground + [(11, 56, True)], 'pending after 300 s'),
                 ['FAILED', 'FAILED']),
                ('ended expired', daemon(ground + [('publish', 'ended', 'f1 expired')], 'expired'),
                 ['FAILED', 'FAILED']),
                ('ended rewound before the rewind cut', daemon([(8, 52, True), ('publish', 'ended', 'f1 refused rewound'),
                                                                (9, 55, True), (10, 55, False)], 'refused rewound x'),
                 ['ok', 'FAILED'])):
            rc, lines = run_course('prewindf', change)
            self.assertEqual(rc, 1, why)
            self.assertEqual(verdicts(lines, 'pin_answer_f1-end', 'pin_f1_ended_after_the_rewind_cut'), pins, why)
            self.assertEqual(lines['premise_f1_waited_across_a_rewind_cut'][0], 'ok', why)
        rc, lines = run_course('prewindf', rows={'f1': (11, 56)})
        self.assertEqual((rc, lines['pin_f1_absent'][0]), (1, 'FAILED'))

    def test_prewindf_with_no_ground_is_the_rig_s_error(self):
        def log(lines):
            return write('requests.tsv', log_text([(PID, lines)]))
        agent = COURSES['prewindf']['files']['agent.tsv']
        for why, change in (
                ('no cut not above the groups before it', log([(8, 52, True), (9, 55, True), (10, 56, True),
                                                               ('publish', 'ended', 'f1 refused rewound')])),
                ('the mark at or before C', log([(7, 55, True), (6, 50, False),
                                                 ('publish', 'ended', 'f1 refused rewound')])),
                # Plant R41 of the review: f1 expired before the rewind cut.
                ('the rewind cut after f1 expired', write('agent.tsv', agent.replace('--expiry-s 900', '--expiry-s 0')))):
            rc, lines = run_course('prewindf', change)
            self.assertEqual((rc, lines['premise_f1_waited_across_a_rewind_cut'][0]), (2, 'ERROR'), why)
        rc, lines = run_course('prewindf', write('agent.tsv', agent.replace('--name f1', '--name f2')))
        self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'))
        rc, lines = run_course('prewindf', write('rewind-f.txt', 'zeroed 3 copies of x\n== zpool import -FX\n'
                                                                 'imported with -FX\n'))
        self.assertEqual((rc, lines['premise_f1_import_f'][0]), (0, 'ok'))
        for text in ('zeroed 3 copies of the root block of group 55: x\n',
                     'zeroed 3 copies of x\ncannot import: imported with -F was not\n',
                     'zeroed 0 copies of the root block of group 55: \nimported with -F\n', ''):
            rc, lines = run_course('prewindf', write('rewind-f.txt', text))
            self.assertEqual((rc, lines['premise_f1_import_f'][0]), (2, 'ERROR'), text)

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
                f.write(log_text([(PID, [(6, 52, True), (7, 45, False), (8, 50, False),
                                         ('publish', 'asked', 'r1 refused rewound', 50, 7), (9, 51, True), (10, 52, True),
                                         (11, 53, True)])]))
        rc, lines = run_course('prewind', change)
        self.assertEqual((rc, lines['premise_rewind_cut_committed'][0]), (2, 'ERROR'))
        self.assertIn('opens as itself', lines['premise_rewind_cut_committed'][1])

    def test_prewind_with_no_cut_committed_after_the_ask_ends_2(self):
        rc, lines = run_course('prewind', write('rewind-row.txt', node_b_row(7, 50)))
        self.assertEqual((rc, lines['premise_rewind_cut_committed'][0]), (2, 'ERROR'))

    def test_prewind_whose_row_names_a_cut_that_opens_as_itself_ends_2(self):
        rc, lines = run_course('prewind', write('rewind-row.txt', node_b_row(10, 52)))
        self.assertEqual((rc, lines['premise_rewind_cut_committed'][0]), (2, 'ERROR'))
        self.assertIn('opens as itself: no daemon would pass it over', lines['premise_rewind_cut_committed'][1])

    def test_prewind_with_r1_made_before_the_rewind_cut_by_one_that_opens_as_itself_ends_2(self):
        """r1 was published before the rewind-only cut was committed: the
        course never offered it (#139's second look, P9)."""
        def change(d):
            with open(os.path.join(d, 'requests.tsv'), 'w') as f:
                f.write(log_text([(PID, [(6, 52, True), (7, 45, False), (8, 50, True),
                                         ('publish', 'asked', 'r1 refused rewound', 50, 7), (9, 51, False), (10, 52, True),
                                         (11, 53, True)])]))
        rc, lines = run_course('prewind', change, rows={'r1': (8, 50)})
        self.assertEqual(lines['pin_r1_cut'][0], 'ok')
        self.assertEqual((rc, lines['premise_rewind_cut_committed'][0]), (2, 'ERROR'))
        self.assertIn('r1 was made by cut 8, which opens as itself', lines['premise_rewind_cut_committed'][1])

    def test_prewind_without_cuts_held_at_the_rewind_ends_2(self):
        rc, lines = run_course('prewind', write('rewind-status.txt', 'cuts_held 0\n'))
        self.assertEqual((rc, lines['premise_held_at_rewind'][0]), (2, 'ERROR'))

    def prewind_log(self, lines):
        def change(d):
            with open(os.path.join(d, 'requests.tsv'), 'w') as f:
                f.write(log_text([(PID, lines)]))
        return change

    def test_prewind_refused_with_no_cut_that_may_follow_the_rewind_ends_2(self):
        key = 'premise_r1_asked_after_a_rewind_cut'
        # The refusal before any cut after C; and cuts after C above the groups before them.
        for lines in ([(6, 52, True), (7, 45, False), ('publish', 'asked', 'r1 refused rewound', 50, 7), (8, 50, False),
                       (9, 51, False), (10, 52, True), (11, 53, True)],
                      [(6, 52, True), (7, 45, False), (8, 60, False), (9, 61, False),
                       ('publish', 'asked', 'r1 refused rewound', 50, 7), (10, 62, True), (11, 63, True)]):
            rc, got = run_course('prewind', self.prewind_log(lines), rows={'r1': (11, 63)})
            self.assertEqual((rc, got[key][0]), (2, 'ERROR'), lines)
            self.assertIn('none', got[key][1])

    def prewindc_log(self, lines):
        def change(d):
            with open(os.path.join(d, 'requests.tsv'), 'w') as f:
                f.write(log_text([(PID, lines)]))
        return change

    def test_prewindc_ends_1_for_a_daemon_that_ends_c1_at_any_import(self):
        """#141's review, S1: with no cut marked, c1 ended `refused
        rewound` is a verdict against the daemon, the premise holding."""
        log = self.prewindc_log([(8, 52, True), (9, 55, True), (10, 56, True),
                                 ('publish', 'ended', 'c1 refused rewound'), (11, 57, True),
                                 ('publish', 'withdraw', 'c1 unknown'), (12, 58, True)])

        def change(d):
            log(d)
            answer('c1-end', 'refused rewound a rewind may have been counted after the count asked from')(d)
            answer('c1-withdraw', 'unknown')(d)
        rc, lines = run_course('prewindc', change)
        self.assertEqual(rc, 1)
        self.assertEqual(verdicts(lines, 'pin_answer_c1-end', 'pin_answer_c1-withdraw',
                                  'premise_c1_waited_across_a_plain_import'), ['FAILED', 'FAILED', 'ok'])
        self.assertNotIn('rig_error', lines)

    def test_prewindc_whose_import_was_a_rewind_or_made_no_cut_ends_2(self):
        key = 'premise_c1_waited_across_a_plain_import'
        for lines, why in (
                # a cut after C not above the groups before it: the import was a rewind
                ([(8, 52, True), (9, 55, True), (10, 55, False), (11, 57, True),
                  ('publish', 'withdraw', 'c1 withdrawn')], 'cuts not above the groups before them: 10 of group 55'),
                # no cut once the import returned (ns 1035) and before the withdrawal
                ([(8, 52, True), (9, 55, True), (10, 56, True), ('publish', 'withdraw', 'c1 withdrawn'),
                  (11, 57, True)], 'before c1\'s withdrawal: none'),
                # no line of c1's withdrawal
                ([(8, 52, True), (9, 55, True), (10, 56, True), (11, 57, True)], 'no line of c1\'s withdrawal'),
                # another name's withdrawal is not c1's
                ([(8, 52, True), (9, 55, True), (10, 56, True), (11, 57, True),
                  ('publish', 'withdraw', 'c12 withdrawn')], 'no line of c1\'s withdrawal')):
            rc, got = run_course('prewindc', self.prewindc_log(lines))
            self.assertEqual((rc, got[key][0]), (2, 'ERROR'), lines)
            self.assertIn(why, got[key][1])
        rc, got = run_course('prewindc', write('marks.tsv', '1005\tsession-1-begin\n'))
        self.assertEqual(rc, 2)
        self.assertIn('no mark session-2-ready', got['rig_error'][1])

    def test_a_look_that_finds_an_ended_or_withdrawn_name_ends_1(self):
        for course, label, t in (('prewindf', 'f1-look', 'f1'), ('prewindc', 'c1-look', 'c1'),
                                 ('pkill', 'tw-look', 'tw')):
            rc, lines = run_course(course, answer(label, look_of(t, 9, 55)))
            self.assertEqual((rc, lines[f'pin_answer_{label}'][0]), (1, 'FAILED'), course)

    def test_the_watcher_s_verdict_ends_1(self):
        """#139's review: the watcher's VERDICT line (node B could not
        read head's content) is judged, in every course."""
        for course in pc.COURSES:
            rc, lines = run_course(course, write('watch.log', '3000\tVERDICT node B could not read cut 4 at head\n',
                                                 'a'))
            self.assertEqual((rc, lines['pin_watcher_reads'][0]), (1, 'FAILED'), course)
            self.assertIn('node B could not read cut 4 at head', lines['pin_watcher_reads'][1])
            self.assertNotIn('rig_error', lines)
        rc, lines = run_course('pplain')
        self.assertEqual((rc, lines['pin_watcher_reads'][0]), (0, 'ok'))

    def test_the_watcher_s_error_or_no_watch_log_ends_2(self):
        rc, lines = run_course('pplain', write('watch.log', '3000\tERROR node B: exit 3\n', 'a'))
        self.assertEqual(rc, 2)
        self.assertIn('the watcher: ERROR node B: exit 3', lines['rig_error'][1])

        def gone(d):
            os.unlink(os.path.join(d, 'watch.log'))
        rc, lines = run_course('pplain', gone)
        self.assertEqual(rc, 2)
        self.assertIn('no watch.log', lines['rig_error'][1])

    def test_prewind_whose_first_ask_has_no_line_ends_2(self):
        """Review of #141, N3: the line of r1 asked anew (C 10) does not
        stand in for the first ask's (C 7), so cuts before it are no
        ground of the first."""
        rc, lines = run_course('prewind', self.prewind_log([(6, 52, True), (7, 45, False), (8, 50, False),
                                                            (9, 51, False), (10, 52, True),
                                                            ('publish', 'asked', 'r1 accepted', 50, 10),
                                                            (11, 53, True)]))
        self.assertEqual((rc, lines['premise_r1_asked_after_a_rewind_cut'][0]), (2, 'ERROR'))
        self.assertIn("no line of r1's ask", lines['premise_r1_asked_after_a_rewind_cut'][1])

    def test_prewind_with_no_line_of_r1_s_ask_ends_2(self):
        rc, lines = run_course('prewind', self.prewind_log([(6, 52, True), (7, 45, False), (8, 50, False),
                                                            (9, 51, False), (10, 52, True), (11, 53, True)]))
        self.assertEqual((rc, lines['premise_r1_asked_after_a_rewind_cut'][0]), (2, 'ERROR'))
        self.assertIn("no line of r1's ask", lines['premise_r1_asked_after_a_rewind_cut'][1])

    def test_prewind_with_r1_never_asked_anew_ends_2(self):
        def change(d):
            with open(os.path.join(d, 'published.tsv')) as f:
                first = f.readline()
            with open(os.path.join(d, 'published.tsv'), 'w') as f:
                f.write(first)
        rc, lines = run_course('prewind', change)
        self.assertEqual(rc, 2)
        self.assertIn('not asked anew', lines['rig_error'][1])

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
        for name, fn in (('prewind', 'rewind-row.txt'), ('prewind', 'rewind-top.txt'), ('prewindf', 'rewind-f.txt'),
                         ('prewindf', 'published.tsv'), ('prewindf', 'agent.tsv'), ('pkill', 'tw-status.txt'),
                         ('pchannel', 'socket-mode.txt')):
            rc, lines = run_course(name, lambda d, fn=fn: os.remove(os.path.join(d, fn)))
            self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'), (name, fn))


if __name__ == '__main__':
    unittest.main()
