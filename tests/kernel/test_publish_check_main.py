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
    # pretire: e1 and e2 published; e1 retired (refused with e2's commit
    # first), then asked again; e9 never published; e3 pends, its retire
    # is refused `asked`, and it is withdrawn. Node B lists e2 only.
    'pretire': {
        'ends': {'e1': 'done commit=' + commit_of('e1'), 'e2': 'done commit=' + commit_of('e2'),
                 'e1-other': f"refused other snap/T names commit {commit_of('e1')}, not the request's; it is "
                             'left as it is',
                 'e1-kept': look_of('e1', 8, 50), 'e1-progress': 'done commit=' + commit_of('e1'),
                 'e1-retire': 'retired', 'e1-look': 'absent', 'e1-progress-after': 'unknown',
                 'e2-progress': 'done commit=' + commit_of('e2'), 'e1-again': 'gone', 'e9': 'gone',
                 'e3': 'pending after 3 s', 'e3-retire': 'refused asked a publish of the name pends on this daemon',
                 'e3-withdraw': 'withdrawn'},
        'requests': {'e1': (7, 50, 50), 'e2': (8, 51, 51), 'e3': (9, 1000052, 52)},
        'rows': {'e2': (9, 51)},
        'log': [(PID, [(8, 50, True), (9, 51, True), ('publish', 'retire', 'e1 refused other'),
                       ('publish', 'retire', 'e1 retired'), ('publish', 'retire', 'e1 gone'),
                       ('publish', 'retire', 'e9 gone'), ('publish', 'retire', 'e3 refused asked'),
                       ('publish', 'withdraw', 'e3 withdrawn')])],
        'files': {'refs-after.txt': f'head\t{"f" * 64}\t9\nsnap/e2\t{commit_of("e2")}\t3\n'},
    },
    # pkillret: the daemon killed once k1's retire landed; the next
    # instance answers the same retire `gone`.
    'pkillret': {
        'ends': {'k1': 'done commit=' + commit_of('k1'), 'k1-retire': 'none (exit 2)', 'k1-again': 'gone',
                 'k1-look': 'absent'},
        'requests': {'k1': (7, 50, 50)},
        'rows': {},
        'log': [(PID, [(8, 50, True)]), (PID2, [('publish', 'retire', 'k1 gone')])],
        'files': {'kill.tsv': 'killed_rc\t137\n',
                  'serve.log': 'skyzfsd: test hook --kill-at-upload: killed after the transaction of retire 1 (k1) '
                               'landed, before the uploader knew it\n',
                  'k1-before.txt': f'ref\t{commit_of("k1")}\t3\n'
                                   f'snaprow\t{S}\t8\t50\t{commit_of("k1")}\t{ULID}\t1\t{"0" * 64}\n',
                  'k1-between.txt': 'ref\tnone\nsnaprow\tnone\n'},
    },
    # passist: a1 asked with G 5 above its snapshot's group on an idle
    # pool; the third assist wrote group 55, which cut 8 carried. a2,
    # asked far ahead, was assisted three times and never carried.
    'passist': {
        'ends': {'a1': 'done commit=' + commit_of('a1'), 'a2': 'pending after 70 s', 'a2-withdraw': 'withdrawn',
                 'a2-look': 'absent'},
        'requests': {'a1': (7, 55, 50), 'a2': (8, 1000056, 56)},
        'rows': {'a1': (8, 55)},
        'log': [(PID, [(7, 50, True), (8, 55, True), (9, 57, True), (10, 58, True), (11, 59, True)])],
        # The agent's wait A is 5 ns here, on the request log's clock.
        'files': {'assist.tsv': 'a1\t1011\t1012\t5000\t50\t51\t52\na1\t1013\t1014\t5001\t52\t53\t54\n'
                                'a1\t1015\t1016\t5002\t54\t55\t56\na2\t1025\t1026\t5003\t56\t57\t57\n'
                                'a2\t1027\t1028\t5004\t57\t58\t58\na2\t1029\t1052\t5005\t58\t58\t60\n',
                  'accepted.tsv': 'a1\taccepted\t1005\t50\t5e-09\na1\tend\t1017\t56\tdone\n'
                                  'a2\taccepted\t1020\t56\t5e-09\na2\tend\t1058\t60\tpending\n',
                  'a2-status-before.txt': 'uploaded 8\nupload_blocks 900\n',
                  'a2-status-after.txt': 'uploaded 11\nupload_blocks 960\n',
                  # Groups 51 to 58 and 60 wrote; 59 wrote nothing.
                  'a2-txgs.txt': 'txg birth state ndirty nread nwritten reads writes\n'
                                 + ''.join(f'{g} 1 C 0 0 {0 if g == 59 else 4096} 0 0\n' for g in range(50, 61)),
                  'agent.tsv': f'1020\t1022\tpublish --name a2 --attachment {A} --state {S} --after 8 '
                               f'--group 1000056 --manifest m --expiry-s 900\t0\taccepted\n'
                               '1053\t1060\tpublish-progress --name a2\t0\tpending after 70 s\n'},
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
            key = f'premise_{pc.KILLED[name]}_killed_by_the_hook'
            self.assertEqual((rc, lines[key][0]), (2, 'ERROR'), (name, fn))
        for fn, text in (('kill.tsv', 'killed_rc\t0\n'),
                         ('serve.log', 'skyzfsd: test hook --kill-at-upload: killed after the transaction of '
                                       'publish 1 (k1) landed, before the uploader knew it\n')):
            rc, lines = run_course('pkillret', write(fn, text))
            self.assertEqual((rc, lines['premise_k1_killed_by_the_hook'][0]), (2, 'ERROR'), fn)

    def test_a_missing_file_is_the_rig_s_error(self):
        for name, fn in (('prewind', 'rewind-row.txt'), ('prewind', 'rewind-top.txt'), ('prewindf', 'rewind-f.txt'),
                         ('prewindf', 'published.tsv'), ('prewindf', 'agent.tsv'), ('pkill', 'tw-status.txt'),
                         ('pchannel', 'socket-mode.txt')):
            rc, lines = run_course(name, lambda d, fn=fn: os.remove(os.path.join(d, fn)))
            self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'), (name, fn))



class Retire(unittest.TestCase):
    """pretire and pkillret (M2.7b-1)."""

    def test_right_courses_hold_each_pin(self):
        rc, lines = run_course('pretire')
        self.assertEqual(rc, 0)
        self.assertEqual(verdicts(lines, 'pin_e1_other_names_its_own', 'pin_e1_kept', 'pin_e1_not_listed',
                                  'pin_retire_lines', 'pin_e1_absent', 'pin_e9_absent', 'pin_e2_row',
                                  'pin_e2_nodeb', 'pin_progress_commits', 'pin_e3_absent',
                                  'pin_answer_e1-progress-after', 'pin_answer_e3-retire'), ['ok'] * 12)
        rc, lines = run_course('pkillret')
        self.assertEqual(rc, 0)
        self.assertEqual(verdicts(lines, 'premise_k1_killed_by_the_hook', 'pin_k1_retired_whole',
                                  'pin_retire_lines', 'pin_k1_absent', 'pin_k1_there_before_the_retire'),
                         ['ok'] * 5)
        self.assertNotIn('premise_k1_retired_before_the_kill', lines)

    def test_other_naming_another_commit_ends_1(self):
        for text in (f"refused other snap/T names commit {commit_of('e2')}, not the request's",
                     'refused other snap/T names another commit',
                     f"refused other snap/T names commit {commit_of('e1')}f, not the request's",
                     f"refused other {commit_of('e1')} and {commit_of('e2')}"):
            rc, lines = run_course('pretire', answer('e1-other', text))
            self.assertEqual((rc, lines['pin_e1_other_names_its_own'][0]), (1, 'FAILED'), text)

    def test_a_refusal_that_deleted_e1_ends_1(self):
        for text in ('absent', look_of('e2', 9, 51)):
            rc, lines = run_course('pretire', answer('e1-kept', text))
            self.assertEqual((rc, lines['pin_e1_kept'][0]), (1, 'FAILED'), text)

    def test_node_b_listing_e1_or_not_e2_ends_1(self):
        e1 = f'snap/e1\t{commit_of("e1")}\t3\n'
        e2 = f'snap/e2\t{commit_of("e2")}\t3\n'
        for text in (e1 + e2, '', f'snap/e2\t{commit_of("e1")}\t3\n'):
            rc, lines = run_course('pretire', write('refs-after.txt', text))
            self.assertEqual((rc, lines['pin_e1_not_listed'][0]), (1, 'FAILED'), text)

    def test_a_listing_that_does_not_read_is_the_rig_s_error(self):
        for text in ('snap/e2 nothing\n', f'snap/e2\t{commit_of("e2")}\t\u00b2\n'):
            rc, lines = run_course('pretire', write('refs-after.txt', text))
            self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'), text)

    def test_e1_still_at_node_b_ends_1(self):
        rc, lines = run_course('pretire', rows={'e1': (8, 50), 'e2': (9, 51)})
        self.assertEqual((rc, lines['pin_e1_absent'][0]), (1, 'FAILED'))
        rc, lines = run_course('pretire', rows={'e1': (8, 50), 'e2': (9, 51)}, row_only=('e1',))
        self.assertEqual((rc, lines['pin_e1_absent']), (1, ('FAILED', '[server] a row')))

    def test_e2_gone_with_e1_ends_1(self):
        # (rows={} would read as the course's own: another name's row stands in.)
        rc, lines = run_course('pretire', rows={'e4': (10, 52)})
        self.assertEqual(lines['pin_e2_row'][0], 'FAILED')
        self.assertEqual(rc, 1)

    def test_retire_lines_other_than_these_end_1(self):
        def log(*instances):
            return write('requests.tsv', log_text(list(instances)))
        swapped = [('publish', 'retire', 'e1 retired'), ('publish', 'retire', 'e1 refused other'),
                   ('publish', 'retire', 'e1 gone'), ('publish', 'retire', 'e9 gone'),
                   ('publish', 'retire', 'e3 refused asked')]
        right = [('publish', 'retire', 'e1 refused other'), ('publish', 'retire', 'e1 retired'),
                 ('publish', 'retire', 'e1 gone'), ('publish', 'retire', 'e9 gone'),
                 ('publish', 'retire', 'e3 refused asked')]
        for name, change in (('pretire', log((PID, [(8, 50, True), (9, 51, True)] + right), (PID2, []))),
                             ('pretire', log((PID, swapped))),
                             ('pretire', log((PID, swapped[1:]))),
                             ('pretire', log((PID, right[:-1]))),
                             ('pretire', log((PID, right[:-1] + [('publish', 'retire', 'e3 retired')]))),
                             ('pretire', log((PID, []))),
                             ('pkillret', log((PID, [('publish', 'retire', 'k1 retired')]),
                                              (PID2, [('publish', 'retire', 'k1 gone')]))),
                             ('pkillret', log((PID, []), (PID2, [('publish', 'retire', 'k1 retired')]))),
                             ('pkillret', log((PID2, [('publish', 'retire', 'k1 gone')])))):
            rc, lines = run_course(name, change)
            self.assertEqual((rc, lines['pin_retire_lines'][0]), (1, 'FAILED'), name)

    def test_retire_lines_that_name_commits(self):
        # M2.7b-2 (#179): `T answer commit=H`, and ` found=H2` after
        # `refused other`. The other publish lines are as before.
        e1, e2, k1 = commit_of('e1'), commit_of('e2'), commit_of('k1')

        def log(*instances):
            return write('requests.tsv', log_text(list(instances)))

        def pretire(lines):
            return log((PID, [(8, 50, True), (9, 51, True)] + [('publish', 'retire', ln) for ln in lines]
                        + [('publish', 'withdraw', 'e3 withdrawn')]))
        right = [f'e1 refused other commit={e2} found={e1}', f'e1 retired commit={e1}', f'e1 gone commit={e1}',
                 f'e9 gone commit={e1}', f'e3 refused asked commit={e1}']
        rc, lines = run_course('pretire', pretire(right))
        self.assertEqual((rc, lines['pin_retire_lines'][0]), (0, 'ok'))
        self.assertIn('each names the commit asked', lines['pin_retire_lines'][1])
        rc, lines = run_course('pkillret', log((PID, [(8, 50, True)]),
                                               (PID2, [('publish', 'retire', f'k1 gone commit={k1}')])))
        self.assertEqual((rc, lines['pin_retire_lines'][0]), (0, 'ok'))
        rc, lines = run_course('pretire')
        self.assertIn('name no commit', lines['pin_retire_lines'][1])
        for k, wrong in ((0, f'e1 refused other commit={e2}'),                    # found= missing
                         (0, f'e1 refused other commit={e1} found={e1}'),         # the commit asked was e2's
                         (0, f'e1 refused other commit={e2} found={e2}'),         # snap/e1 named e1's
                         (0, f'e1 refused other commit={e2} found={e1} found={e1}'),
                         (0, f'e1 refused other commit={e2} found={e1} x'),
                         (1, f'e1 retired commit={e1} found={e1}'),               # found= only after other
                         (1, f'e1 retired commit={e2}'),
                         (1, f'e1 retired commit={e1[:-1]}'),
                         (1, 'e1 retired'),                                       # one form in one log
                         (3, f'e9 gone commit={e2}'),
                         (4, f'e3 refused asked commit={e2}'),
                         (4, f'e3 refused asked found={e1} commit={e1}')):
            got = list(right)
            got[k] = wrong
            rc, lines = run_course('pretire', pretire(got))
            self.assertEqual((rc, lines['pin_retire_lines'][0]), (1, 'FAILED'), wrong)
        rc, lines = run_course('pkillret', log((PID, [(8, 50, True)]),
                                               (PID2, [('publish', 'retire', f'k1 gone commit={e1}')])))
        self.assertEqual((rc, lines['pin_retire_lines'][0]), (1, 'FAILED'))

    def test_k1_torn_or_there_after_the_kill_ends_1(self):
        # The hook printed its line once the transaction landed: whatever
        # is left of snap/k1 is the server's (#178's B1 and D2).
        row = f'snaprow\t{S}\t8\t50\t{commit_of("k1")}\t{ULID}\t1\t{"0" * 64}\n'
        for text in (f'ref\t{commit_of("k1")}\t3\nsnaprow\tnone\n', 'ref\tnone\n' + row,
                     f'ref\t{commit_of("k1")}\t3\n' + row):
            rc, lines = run_course('pkillret', write('k1-between.txt', text))
            self.assertEqual((rc, lines['pin_k1_retired_whole'][0]), (1, 'FAILED'), text)

    def test_k1_not_read_before_the_retire_ends_1(self):
        row = f'snaprow\t{S}\t8\t50\t{commit_of("k1")}\t{ULID}\t1\t{"0" * 64}\n'
        for text in ('ref\tnone\nsnaprow\tnone\n', f'ref\t{commit_of("k1")}\t3\nsnaprow\tnone\n',
                     f'ref\t{commit_of("e2")}\t3\n' + row, 'ref\tnone\n' + row):
            rc, lines = run_course('pkillret', write('k1-before.txt', text))
            self.assertEqual((rc, lines['pin_k1_there_before_the_retire'][0]), (1, 'FAILED'), text)
        rc, lines = run_course('pkillret', answer('k1', 'done commit=' + commit_of('e2')))
        self.assertEqual((rc, lines['pin_k1_there_before_the_retire'][0]), (1, 'FAILED'))

    def test_progress_on_another_commit_ends_1(self):
        for label, text in (('e1-progress', 'done commit=' + commit_of('e2')),
                            ('e2-progress', 'done commit=' + commit_of('e1')),
                            ('e2-progress', 'done commit=' + commit_of('e2') + ' x'),
                            ('e2-progress', 'unknown'),            # e1's retire let go of e2's outcome
                            ('e1-progress', 'done')):
            rc, lines = run_course('pretire', answer(label, text))
            self.assertEqual((rc, lines['pin_progress_commits'][0]), (1, 'FAILED'), (label, text))

    def test_a_retired_name_s_progress_done_ends_1(self):
        # #175's D4: done after the retire is a snapshot no longer listed.
        rc, lines = run_course('pretire', answer('e1-progress-after', 'done commit=' + commit_of('e1')))
        self.assertEqual((rc, lines['pin_answer_e1-progress-after'][0]), (1, 'FAILED'))

    def test_a_pending_name_retired_ends_1(self):
        for text in ('retired', 'gone', 'refused other x'):
            rc, lines = run_course('pretire', answer('e3-retire', text))
            self.assertEqual((rc, lines['pin_answer_e3-retire'][0]), (1, 'FAILED'), text)
        rc, lines = run_course('pretire', rows={'e2': (9, 51), 'e3': (10, 1000052)})
        self.assertEqual((rc, lines['pin_e3_absent'][0]), (1, 'FAILED'))

    def test_k1_at_node_b_after_the_take_over_ends_1(self):
        rc, lines = run_course('pkillret', rows={'k1': (8, 50)})
        self.assertEqual((rc, lines['pin_k1_absent'][0]), (1, 'FAILED'))

    def test_the_answers_outside_the_table_end_1(self):
        for name, label, text in (('pkillret', 'k1-again', 'retired'), ('pkillret', 'k1-retire', 'retired'),
                                  ('pretire', 'e1-again', 'retired'), ('pretire', 'e9', 'refused other x'),
                                  ('pretire', 'e1-retire', 'gone')):
            rc, lines = run_course(name, answer(label, text))
            self.assertEqual((rc, lines[f'pin_answer_{label}'][0]), (1, 'FAILED'), (name, label))


def assists(text):
    return write('assist.tsv', text)


def marks(text):
    return write('accepted.tsv', text)


def both(*changes):
    def change(d):
        for c in changes:
            c(d)
    return change


def ends(**answers):
    """The course's ends.tsv with ANSWERS in place of its own."""
    return write('ends.tsv', ''.join(f'{k.replace("_", "-")}\t{v}\n'
                                     for k, v in dict(COURSES['passist']['ends'],
                                                      **{k.replace('_', '-'): v for k, v in answers.items()}).items()))


class Assist(unittest.TestCase):
    """passist: the assist writes a group on a kernel (a verdict), its
    premises (the rig's), and the pace's line."""
    RIGHT = COURSES['passist']['files']['assist.tsv']
    MARKS = COURSES['passist']['files']['accepted.tsv']

    def test_the_right_course_names_the_assist_that_wrote_the_carrying_group(self):
        rc, lines = run_course('passist')
        self.assertEqual(rc, 0)
        self.assertEqual(verdicts(lines, 'premise_a1_needed_the_assist', 'premise_a1_assists_at_the_decided_pace',
                                  'premise_a2_assists_at_the_decided_pace', 'pin_assist_writes_a_group',
                                  'premise_a2_assisted', 'obs_a1_cut', 'obs_assist_pace'),
                         ['ok', 'ok', 'ok', 'ok', 'ok', 'INFO', 'INFO'])
        self.assertIn('assist 3 took it across G', lines['premise_a1_needed_the_assist'][1])
        self.assertIn('a group assist 3 wrote', lines['obs_a1_cut'][1])
        pace = lines['obs_assist_pace'][1]
        for part in ('3 assists', 'committed 3 groups that wrote', 'and 1 that wrote nothing', 'in 2 of them and the `zpool sync` after it in 1',
                     'counted 3 cuts', 'uploaded 3 cuts', 'and 60 blocks'):
            self.assertIn(part, pace)

    def test_an_assist_that_wrote_no_group_ends_1(self):
        rc, lines = run_course('passist', assists(self.RIGHT.replace('5004\t57\t58\t58', '5004\t58\t58\t58')))
        self.assertEqual((rc, lines['pin_assist_writes_a_group'][0]), (1, 'FAILED'))
        self.assertIn('assist 5 (a2, V 5004)', lines['pin_assist_writes_a_group'][1])

    def test_a_carrying_group_no_assist_wrote_is_said(self):
        rc, lines = run_course('passist', rows={'a1': (8, 57)})
        self.assertIn('a group no assist wrote', lines['obs_a1_cut'][1])

    def test_a_group_from_g_on_no_assist_wrote_ends_2(self):
        """#145's G1 (V1): a2's assists took the pool to G 55, and group
        56 wrote after the last of them by the end: a1 could have been
        carried by a group no assist wrote."""
        rc, lines = run_course('passist', both(
            assists(self.RIGHT.replace('a1\t1013\t1014\t5001\t52\t53\t54\n', 'a1\t1013\t1014\t5001\t52\t54\t55\n')
                    .replace('a1\t1015\t1016\t5002\t54\t55\t56\n', ''))))
        self.assertEqual((rc, lines['premise_a1_needed_the_assist'][0]), (2, 'ERROR'))
        self.assertIn('groups no assist wrote: 56', lines['premise_a1_needed_the_assist'][1])

    def test_a_group_below_g_no_assist_wrote_carries_nothing(self):
        """The hold's history, written by the txg timeout after the
        acceptance (passist's first run at 9bac3dd): group 51 for G 55."""
        rc, lines = run_course('passist', assists(self.RIGHT.replace('5000\t50\t51\t52', '5000\t51\t51\t52')))
        self.assertEqual((rc, lines['premise_a1_needed_the_assist'][0]), (0, 'ok'))

    def test_premises_that_fail_end_2(self):
        for why, change in (
                ('G was written before any assist', marks(self.MARKS.replace('accepted\t1005\t50', 'accepted\t1005\t55'))),
                ('no assist took the pool across G', assists(self.RIGHT.replace('\t54\t55\t56', '\t53\t53\t54'))),
                ('over 0 assists: no assist took', assists(''.join(ln + '\n' for ln in self.RIGHT.splitlines()
                                                    if not ln.startswith('a1')))),
                ('the assists never took the pool to G', both(
                    assists(self.RIGHT.replace('a1\t1015\t1016\t5002\t54\t55\t56\n', '')),
                    marks(self.MARKS.replace('a1\tend\t1017\t56\tdone', 'a1\tend\t1017\t54\tpending')),
                    ends(a1='pending after 300 s'))),
                ('a1 was accepted and has no end mark', marks(self.MARKS.replace('a1\tend\t1017\t56\tdone\n', ''))),
                ('and was not refused', marks(self.MARKS.replace('a1\taccepted\t1005\t50\t5e-09\n', ''))),
                ('a1 was never accepted', lambda d: os.remove(os.path.join(d, 'accepted.tsv'))),
                ("accepted.tsv line 5: not", marks(self.MARKS + 'a2\tend\t1061\t60\tpending\n')),
                ("accepted.tsv line 5: not ", marks(self.MARKS + 'a3\taccepted\t1061\n')),
                ('a2 was assisted 1 times', both(assists(''.join(ln + '\n' for ln in self.RIGHT.splitlines()[:4])),
                                                 marks(self.MARKS.replace('a2\tend\t1058', 'a2\tend\t1030')))),
                ('assist.tsv line 7', assists(self.RIGHT + 'a2\t1060\t1061\n')),
                ('lacks 1 of the groups 57 to 61', assists(self.RIGHT.replace('5005\t58\t58\t60', '5005\t58\t58\t61'))),
                ("a2's first assist", marks(self.MARKS.replace('a2\taccepted\t1020', 'a2\taccepted\t1022'))),
                # #145's next look, S2: the burst's pause from the end of
                # the assist before, at least the agent's (scaled: 0.5 ns).
                ("a1's assist 2 0.00 s after the end of the one before, in the burst", assists(self.RIGHT.replace(
                    'a1\t1013\t1014', 'a1\t1012\t1014'))),
                ("a2's assist 4", assists(self.RIGHT + 'a2\t1031\t1032\t5006\t60\t60\t60\n'))):
            with self.subTest(why=why):
                rc, lines = run_course('passist', change)
                self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'), why)
                self.assertIn(why, lines['rig_error'][1])

    def test_a_slow_gap_is_observed_and_judges_nothing(self):
        """The lead's answer to #145's next look, S1: a ceiling is no
        premise; a gap over 3 times its decided value is only said."""
        rc, lines = run_course('passist')
        self.assertEqual(lines['obs_assist_gap_over_3x'][0], 'INFO')
        self.assertIn('no gap', lines['obs_assist_gap_over_3x'][1])
        for change, said in ((marks(self.MARKS.replace('a2\tend\t1058', 'a2\tend\t1068')),
                              'a2 ended pending 0.00 s after its last assist'),
                             (marks(self.MARKS.replace('a2\taccepted\t1020', 'a2\taccepted\t1004')),
                              "a2's first assist 0.00 s after its acceptance"),
                             (assists(self.RIGHT.replace('a2\t1029\t1052', 'a2\t1045\t1052')),
                              "a2's assist 3 0.00 s after the end of the one before, in the burst")):
            with self.subTest(said=said):
                rc2, lines = run_course('passist', change)
                self.assertEqual((rc2, lines['obs_assist_gap_over_3x'][0]), (rc, 'INFO'))
                self.assertIn(said, lines['obs_assist_gap_over_3x'][1])
                self.assertNotIn(';', lines['obs_assist_gap_over_3x'][1])
                self.assertEqual(lines['premise_a2_assists_at_the_decided_pace'][0], 'ok')

    def test_an_end_the_daemon_made_early_is_its_verdict(self):
        """#145's F1: where the daemon ended a publish before the assists
        could make the case, the pins judge it (1), not a premise (2)."""
        no_a1 = ''.join(ln + '\n' for ln in self.RIGHT.splitlines() if not ln.startswith('a1'))
        for why, change, rows, pin in (
                # D3: a1 published on group 50, below G, before any assist.
                ('a1 done before any assist', both(
                    assists(no_a1), marks(self.MARKS.replace('a1\tend\t1017\t56\tdone', 'a1\tend\t1005\t50\tdone'))),
                 {'a1': (7, 50)}, 'pin_a1_row'),
                # D2: a1 published after its first assist, below G.
                ('a1 done after one assist', both(
                    assists(self.RIGHT.replace('a1\t1013\t1014\t5001\t52\t53\t54\n', '')
                            .replace('a1\t1015\t1016\t5002\t54\t55\t56\n', '')),
                    marks(self.MARKS.replace('a1\tend\t1017\t56\tdone', 'a1\tend\t1013\t52\tdone'))),
                 {'a1': (7, 50)}, 'pin_a1_row'),
                # D8: a1 refused at its publish, before it was accepted.
                ('a1 refused', both(assists(no_a1), marks(self.MARKS.replace('a1\taccepted\t1005\t50\t5e-09\n', '')
                                                         .replace('a1\tend\t1017\t56\tdone\n', '')),
                                    ends(a1='refused stale the attachment is not the volume\'s')),
                 {}, 'pin_answer_a1')):
            with self.subTest(why=why):
                rc, lines = run_course('passist', change, rows=rows)
                self.assertEqual(rc, 1, (why, lines.get('rig_error')))
                self.assertEqual(verdicts(lines, 'premise_a1_needed_the_assist', pin), ['ok', 'FAILED'], why)
        # D6 to D8 for a2: not `pending after`, so no pace and no premise
        # on its assists; pin_answer_a2 judges the end.
        for end in ('expired', 'refused unknown no such publish', 'done commit=' + commit_of('a2')):
            with self.subTest(end=end):
                rc, lines = run_course('passist', both(
                    assists(''.join(ln + '\n' for ln in self.RIGHT.splitlines()[:4])),
                    marks(self.MARKS.replace('a2\tend\t1058\t60\tpending', f'a2\tend\t1026\t57\t{end.split()[0]}')),
                    ends(a2=end)))
                self.assertEqual(rc, 1, (end, lines.get('rig_error')))
                self.assertEqual(verdicts(lines, 'pin_answer_a2', 'obs_assist_pace', 'premise_a2_assisted'),
                                 ['FAILED', 'NA', 'none'])

    def test_a_daemon_that_ends_both_before_any_assist_is_judged(self):
        """No assist at all, so no assist.tsv: a1 done below G and a2
        expired before their first assists are the daemon's (1)."""
        rc, lines = run_course('passist', both(
            lambda d: os.remove(os.path.join(d, 'assist.tsv')),
            marks('a1\taccepted\t1005\t50\t5e-09\na1\tend\t1003\t50\tdone\n'
                  'a2\taccepted\t1020\t56\t5e-09\na2\tend\t1022\t56\texpired\n'),
            ends(a2='expired')), rows={'a1': (7, 50)})
        self.assertEqual(rc, 1, lines.get('rig_error'))
        self.assertEqual(verdicts(lines, 'premise_a1_needed_the_assist', 'pin_a1_row', 'pin_answer_a2',
                                  'obs_assist_pace'), ['ok', 'FAILED', 'FAILED', 'NA'])

    def test_a_status_the_pace_cannot_read_is_said_and_is_no_error(self):
        """#145's F2: the line is INFO, and the pins have the verdict."""
        for fn, text, lack in (('a2-status-after.txt', 'uploaded x\nupload_blocks 960\n', 'uploaded (after)'),
                               ('a2-status-before.txt', 'upload_blocks 900\n', 'uploaded (before)'),
                               ('a2-status-after.txt', 'uploaded 11\n', 'upload_blocks (after)')):
            with self.subTest(fn=fn, lack=lack):
                rc, lines = run_course('passist', write(fn, text))
                self.assertEqual((rc, lines['obs_assist_pace'][0]), (0, 'INFO'))
                self.assertIn(f'no upload figures: the statuses around a2\'s publish lack {lack}',
                              lines['obs_assist_pace'][1])
        rc, lines = run_course('passist', lambda d: os.remove(os.path.join(d, 'a2-status-after.txt')))
        self.assertEqual(rc, 0)
        self.assertIn('lack upload_blocks (after), uploaded (after)', lines['obs_assist_pace'][1])

    def test_an_agent_log_the_pace_cannot_read_ends_2(self):
        rc, lines = run_course('passist', write('agent.tsv', '1020\t1022\tpublish --name a1 --after 8\t0\taccepted\n'))
        self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'))


class Watch(unittest.TestCase):
    def test_a_watch_log_with_no_line_is_the_rig_s_error(self):
        """#143's review, G3."""
        rc, lines = run_course('pplain', write('watch.log', ''))
        self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'))
        self.assertIn('no line', lines['rig_error'][1])


if __name__ == '__main__':
    unittest.main()
