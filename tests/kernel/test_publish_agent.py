#!/usr/bin/env python3
"""Tests of the agent stand-in of the channel rig (publish_agent.py): no kernel."""
import contextlib
import io
import itertools
import os
import socket
import subprocess
import sys
import tempfile
import threading
import types
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import publish_agent as pa  # noqa: E402

A = 'a' * 32
S = 'b' * 32
H = 'c' * 64


class Answers(unittest.TestCase):
    def test_each_answer_of_the_protocol(self):
        self.assertEqual(pa.parse_answer(f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                         ('cuts', {'attachment': A, 'state': S, 'count': '7', 'group': '41', 'opens': 'itself'},
                          None, None))
        self.assertEqual(pa.parse_answer(f'cuts attachment={A} state=none count=0 group=none opens=none')[1]['state'],
                         'none')
        self.assertEqual(pa.parse_answer(f'cuts attachment={A} state={S} count=3 group=12 opens=rewind')[1]['opens'],
                         'rewind')
        # Version 3 (M2.7b): the store's rewind mark, which version 2 lacks.
        self.assertEqual(pa.parse_answer(f'cuts attachment={A} state={S} count=7 group=41 opens=itself rewound=5\n'),
                         ('cuts', {'attachment': A, 'state': S, 'count': '7', 'group': '41', 'opens': 'itself',
                                   'rewound': '5'}, None, None))
        self.assertEqual(pa.parse_answer(f'cuts attachment={A} state=none count=0 group=none opens=none rewound=0')[1]
                         ['rewound'], '0')
        for verb in ('accepted', 'pending', 'expired', 'withdrawn', 'unknown'):
            self.assertEqual(pa.parse_answer(verb), (verb, {}, None, None))
        self.assertEqual(pa.parse_answer(f'done commit={H}'), ('done', {'commit': H}, None, None))
        self.assertEqual(pa.parse_answer('refused stopped the uploader steps no more'),
                         ('refused', {}, 'stopped', 'the uploader steps no more'))
        self.assertEqual(pa.parse_answer('refused peer'), ('refused', {}, 'peer', ''))
        snap = f'snap commit={H} state={S} number=9 group=51 manifest-len=911 manifest-hash={H}'
        self.assertEqual(pa.parse_answer(snap), ('snap', {'commit': H, 'state': S, 'number': '9', 'group': '51',
                                                          'manifest-len': '911', 'manifest-hash': H}, None, None))
        self.assertEqual(pa.parse_answer('absent'), ('absent', {}, None, None))
        self.assertEqual(pa.parse_answer(f'unreadable commit={H} why=version'),
                         ('unreadable', {'commit': H, 'why': 'version'}, None, None))

    def test_what_is_not_an_answer_is_the_rigs_error(self):
        for line in ('', 'refused ', 'cuts', f'cuts attachment={A} state={S} count=7 group=41',
                     f'cuts attachment={A} state={S} count=7 group=41 opens=itself extra=1',
                     f'cuts attachment={A[:-1]} state={S} count=7 group=41 opens=itself',
                     f'cuts attachment={A} state={S} count=x group=41 opens=itself',
                     f'cuts attachment={A} state={S} count=7 group=none opens=itself',
                     f'cuts attachment={A} state={S} count=7 group=41 opens=maybe',
                     f'cuts attachment={A} state={S} count=7 group=41 opens=itself rewound=x',
                     f'cuts attachment={A} state={S} count=7 group=41 opens=itself rewound=',
                     f'cuts attachment={A} state={S} count=7 group=41 opens=itself rewound=5 extra=1',
                     f'cuts attachment={A} state={S} count=7 group=41 rewound=5',
                     f'cuts attachment={A} state={S} count=7 group=41 opens=itself rewound=5 rewound=5',
                     f'accepted rewound=5',
                     'done', f'done commit={H[:-1]}', f'done commit={H.upper()}', 'accepted now',
                     'pending  ', 'publish', f'done commit={H} commit={H}',
                     f'snap commit={H} state={S} number=9 group=51 manifest-len=911',
                     f'snap commit={H[:-1]} state={S} number=9 group=51 manifest-len=911 manifest-hash={H}',
                     f'snap commit={H} state={S} number=x group=51 manifest-len=911 manifest-hash={H}',
                     f'snap commit={H} state={S[:-1]} number=9 group=51 manifest-len=911 manifest-hash={H}',
                     'absent now', f'unreadable commit={H}',
                     # #140's review, N7: A3 and A5.
                     f'unreadable commit={H[:-1]} why=version', f'unreadable commit={H.upper()} why=no-row',
                     f'snap commit={H} state={S} number=9 group=51 manifest-len=x manifest-hash={H}',
                     f'snap commit={H} state={S} number=9 group=51 manifest-len=-1 manifest-hash={H}'):
            with self.assertRaises(pa.RigError, msg=line):
                pa.parse_answer(line)


class Fake:
    """skyzfsd's commands, stood in for: each call answers the next line."""

    def __init__(self, *answers):
        self.answers, self.calls = list(answers), []

    def __call__(self, cmd, capture_output, text, timeout):
        self.calls.append(cmd)
        rc, out = self.answers.pop(0)
        return subprocess.CompletedProcess(cmd, rc, out, 'err')


class AnAgent(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.TemporaryDirectory()
        self.log = os.path.join(self.d.name, 'agent.tsv')
        self.t = iter(range(100, 10000, 10))

    def tearDown(self):
        self.d.cleanup()

    def agent(self, *answers):
        self.fake = Fake(*answers)
        return pa.Agent('skyzfsd', 'VOL', 'RUN', self.log, clock=lambda: next(self.t), runner=self.fake)

    def test_a_request_is_sent_and_recorded(self):
        ag = self.agent((0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                        (0, 'accepted\n'), (1, 'refused name not a name\n'), (0, f'done commit={H}\n'),
                        (0, 'withdrawn\n'), (0, 'absent\n'))
        self.assertEqual(ag.cuts()[1]['count'], '7')
        self.assertEqual(ag.publish('t1', A, S, 7, 41, '/m', expiry=30)[0], 'accepted')
        self.assertEqual(ag.progress('x y')[2], 'name')
        self.assertEqual(ag.progress('t1')[1]['commit'], H)
        self.assertEqual(ag.withdraw('t1')[0], 'withdrawn')
        self.assertEqual(self.fake.calls[0], ['skyzfsd', 'cuts', '--dir', 'VOL', '--run-dir', 'RUN'])
        self.assertEqual(self.fake.calls[1], ['skyzfsd', 'publish', '--dir', 'VOL', '--run-dir', 'RUN', '--name', 't1',
                                              '--attachment', A, '--state', S, '--after', '7', '--group', '41',
                                              '--manifest', '/m', '--expiry-s', '30'])
        self.assertEqual(self.fake.calls[2][1], 'publish-progress')
        self.assertEqual(ag.look('t1')[0], 'absent')
        self.assertEqual(self.fake.calls[4][1], 'withdraw')
        self.assertEqual(self.fake.calls[5], ['skyzfsd', 'look', '--dir', 'VOL', '--run-dir', 'RUN', '--name', 't1'])
        with open(self.log) as f:
            rows = [ln.rstrip('\n').split('\t') for ln in f]
        self.assertEqual(len(rows), 6)
        self.assertEqual(rows[0][:2], ['100', '110'])
        self.assertEqual(rows[2][2:], ['publish-progress --name x y', '1', 'refused name not a name'])
        self.assertEqual(rows[3][4], f'done commit={H}')

    def test_ask_look_through_main(self):
        """#140's review, N7 (A4): `ask look T` is skyzfsd's look, and
        prints its answer."""
        snap = f'snap commit={H} state={S} number=9 group=15 manifest-len=414 manifest-hash={H}'
        for answer, rc_want, printed in ((0, 0, snap), (1, 1, 'refused store the store did not answer')):
            fake = Fake((answer, printed + '\n'))
            real = pa.Agent
            out = io.StringIO()
            with mock.patch.object(pa, 'Agent', lambda *a: real(*a, runner=fake)), contextlib.redirect_stdout(out):
                rc = pa.main(['publish_agent.py', 'ask', 'skyzfsd', 'VOL', 'RUN', self.log, 'look', 'p1'])
            self.assertEqual((rc, out.getvalue()), (rc_want, printed + '\n'))
            self.assertEqual(fake.calls, [['skyzfsd', 'look', '--dir', 'VOL', '--run-dir', 'RUN', '--name', 'p1']])

    def test_no_answer_or_an_exit_that_disagrees_is_the_rigs_error(self):
        for rc, out in ((1, ''), (2, 'accepted\n'), (0, 'refused peer\n'), (1, 'accepted\n'), (0, '')):
            ag = self.agent((rc, out))
            with self.assertRaises(pa.RigError, msg=(rc, out)):
                ag.cuts()
        with open(self.log) as f:
            self.assertEqual(len(f.readlines()), 5)   # each recorded all the same


class AFlow(unittest.TestCase):
    """The agent's order (design §5.2): cuts, snapshot, hold, publish,
    progress (syncing when asked), release; whatever happens, released."""

    def setUp(self):
        self.d = tempfile.TemporaryDirectory()
        self.log = os.path.join(self.d.name, 'agent.tsv')

    def tearDown(self):
        self.d.cleanup()

    def run_flow(self, answers, sync=False, limit=300, expiry=None, hold=True, group_plus=0, twice=False):
        fake = Fake(*answers)
        ag = pa.Agent('skyzfsd', 'VOL', 'RUN', self.log, clock=lambda: 1, runner=fake)
        shell = []

        def sh(cmd):
            shell.append(' '.join(cmd))
            if cmd[:2] == ['zfs', 'get'] and 'createtxg' in cmd:
                return '55\n'
            if cmd[:2] == ['zfs', 'get']:
                return '/p\n'
            return ''
        clock = iter(range(0, 100000, 100))
        end = pa.flow(ag, 'p', 't1', sync=sync, limit=limit, expiry=expiry, hold=hold, sleep=lambda s: None,
                      shell=sh, files=lambda root: f'{"0" * 64}  f1\n'.encode(), now=lambda: next(clock),
                      group_plus=group_plus, twice=twice, nudge=lambda path: shell.append(f'nudge {path}'))
        return end, fake.calls, shell

    def test_the_order_and_the_request(self):
        end, calls, shell = self.run_flow([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                                           (0, 'accepted\n'), (0, 'pending\n'), (0, f'done commit={H}\n')], sync=True)
        self.assertEqual(end, f'done commit={H}')
        self.assertEqual([c[1] for c in calls], ['cuts', 'publish', 'publish-progress', 'publish-progress'])
        pub = calls[1]
        self.assertEqual(pub[pub.index('--after') + 1], '7')
        self.assertEqual(pub[pub.index('--group') + 1], '55')
        self.assertEqual(pub[pub.index('--attachment') + 1], A)
        self.assertEqual(shell, ['zfs snapshot p@t1', 'zfs hold skyzfs-publish p@t1',
                                 'zfs get -Hp -o value createtxg p@t1', 'zfs get -H -o value mountpoint p',
                                 'nudge /p/publish-nudge', 'zpool sync p', 'zfs release skyzfs-publish p@t1'])
        with open(os.path.join(self.d.name, 'manifest-t1'), 'rb') as f:
            self.assertEqual(f.read(), f'{"0" * 64}  f1\n'.encode())
        with open(os.path.join(self.d.name, 'published.tsv')) as f:
            self.assertEqual(f.read().rstrip('\n').split('\t'), ['t1', '55', A, S, '7',
                                                                 os.path.join(self.d.name, 'manifest-t1'), '55'])

    def test_a_group_above_the_snapshots_and_asking_twice(self):
        end, calls, _ = self.run_flow([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                                       (0, 'accepted\n'), (0, 'accepted\n'), (0, f'done commit={H}\n')],
                                      group_plus=100, twice=True)
        self.assertEqual(end, f'done commit={H}')
        self.assertEqual([c[1] for c in calls], ['cuts', 'publish', 'publish', 'publish-progress'])
        self.assertEqual(calls[1], calls[2])
        self.assertEqual(calls[1][calls[1].index('--group') + 1], '155')
        with open(os.path.join(self.d.name, 'published.tsv')) as f:
            row = f.read().rstrip('\n').split('\t')
        self.assertEqual((row[1], row[6]), ('155', '55'))

    def test_no_sync_unless_asked_and_the_limit(self):
        end, calls, shell = self.run_flow([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                                           (0, 'accepted\n')] + [(0, 'pending\n')] * 5, limit=250)
        self.assertEqual(end, 'pending after 250 s')
        self.assertNotIn('zpool sync p', shell)
        self.assertEqual(shell[-1], 'zfs release skyzfs-publish p@t1')

    def test_a_refusal_ends_it_and_the_hold_is_released(self):
        end, calls, shell = self.run_flow([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                                           (1, 'refused stopped ask the next daemon\n')])
        self.assertEqual(end, 'refused stopped ask the next daemon')
        self.assertEqual(shell[-1], 'zfs release skyzfs-publish p@t1')
        end, calls, shell = self.run_flow([(1, 'refused no-upload not uploading\n')])
        self.assertEqual((end, shell), ('refused no-upload not uploading', []))

    def test_each_end_is_printed_as_answered(self):
        for last in ('expired', 'withdrawn', 'unknown'):
            end, _, _ = self.run_flow([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                                       (0, 'accepted\n'), (0, f'{last}\n')], expiry=5)
            self.assertEqual(end, last)

    def test_without_a_hold(self):
        _, _, shell = self.run_flow([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                                     (0, 'accepted\n'), (0, f'done commit={H}\n')], hold=False)
        self.assertFalse([c for c in shell if 'hold' in c or 'release' in c])

    def assisted(self, rounds, after, walls, synced, step=4):
        fake = Fake(*([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'), (0, 'accepted\n')]
                      + [(0, 'pending\n')] * rounds + [(0, f'done commit={H}\n')]))
        ag = pa.Agent('skyzfsd', 'VOL', 'RUN', self.log, clock=lambda: 1, runner=fake)
        shell = []

        def sh(cmd):
            shell.append(' '.join(cmd))
            if cmd[:2] == ['zfs', 'get'] and 'createtxg' in cmd:
                return '55\n'
            if cmd[:2] == ['zfs', 'get']:
                return '/p\n'
            return ''
        clock = iter(range(0, 100000, step))
        wall = iter(walls)
        mono = iter(range(1000, 100000, 10))
        end = pa.flow(ag, 'p', 't1', sleep=lambda s: None, shell=sh, files=lambda root: b'x  f1\n',
                      now=lambda: next(clock), assist_after=after, synced=lambda pool: next(synced),
                      wall=lambda: next(wall), mono=lambda: next(mono))
        with open(os.path.join(self.d.name, 'assist.tsv')) as f:
            rows = [ln.rstrip('\n').split('\t') for ln in f]
        return end, shell, rows

    def marks(self):
        with open(os.path.join(self.d.name, 'accepted.tsv')) as f:
            return [ln.rstrip('\n').split('\t') for ln in f]

    def test_the_assist_sets_a_new_value_then_syncs_and_writes_nothing(self):
        """#144's `Zfs::assist`: after ASSIST_AFTER, a `zfs set` with a
        value not set before (one above the last when the wall clock
        steps back), then `zpool sync`; no nudge."""
        # Rounds at now() 4, 8, 12, 16 (t0 = 0): no assist before 10 s.
        # One read at the acceptance; four an assist: before, after the
        # set, then until two agree; one at the end.
        synced = iter([60,   60, 61, 61, 61,   61, 62, 62, 62,   62])
        end, shell, rows = self.assisted(4, 10, [5_000_000_000, 4_000_000_000], synced)
        self.assertEqual(end, f'done commit={H}')
        mid = shell[4:-1]
        self.assertEqual(mid, ['zfs set skyzfs:assist=5000 p', 'zpool sync p',
                               'zfs set skyzfs:assist=5001 p', 'zpool sync p'])
        self.assertFalse([c for c in shell if 'nudge' in c])
        self.assertEqual(rows, [['t1', '1010', '1020', '5000', '60', '61', '61'],
                                ['t1', '1030', '1040', '5001', '61', '62', '62']])

    def test_the_pool_at_the_acceptance_and_at_the_end_whatever_the_daemon_answers(self):
        """#145's F1: the premises read the kstat at both ends of the
        wait, not how long the daemon answered `pending`."""
        synced = iter([58,   60, 61, 61, 61,   63])
        end, _, _ = self.assisted(3, 10, [5_000_000_000], synced)
        self.assertEqual(end, f'done commit={H}')
        self.assertEqual(self.marks(), [['t1', 'accepted', '1000', '58', '10'],
                                        ['t1', 'end', '1030', '63', 'done']])

    def test_an_end_by_the_limit_or_a_refusal_is_marked_too(self):
        for answers, word in (([(0, 'pending\n')] * 5, 'pending'), ([(1, 'refused unknown no\n')], 'refused')):
            with self.subTest(word=word):
                marks = os.path.join(self.d.name, 'accepted.tsv')
                if os.path.exists(marks):
                    os.unlink(marks)
                fake = Fake(*([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                               (0, 'accepted\n')] + answers))
                ag = pa.Agent('skyzfsd', 'VOL', 'RUN', self.log, clock=lambda: 1, runner=fake)
                clock = iter(range(0, 1000, 4))
                end = pa.flow(ag, 'p', 't1', sleep=lambda s: None, limit=6,
                              shell=lambda c: '55\n' if 'createtxg' in c else '/p\n' if c[:2] == ['zfs', 'get'] else '',
                              files=lambda root: b'x  f1\n', now=lambda: next(clock), assist_after=100,
                              synced=lambda pool: 70, mono=iter(range(5, 1000)).__next__)
                self.assertTrue(end.startswith(word), end)
                self.assertEqual([m[1] for m in self.marks()], ['accepted', 'end'])
                self.assertEqual(self.marks()[-1][3:], ['70', word])

    def test_no_mark_without_the_assist(self):
        self.run_flow([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                       (0, 'accepted\n'), (0, f'done commit={H}\n')])
        self.assertFalse(os.path.exists(os.path.join(self.d.name, 'accepted.tsv')))

    def test_assist_after_is_ten_seconds_as_on_144(self):
        """The course passes `--assist` alone, so the default is what
        must equal #144's `plan::ASSIST_AFTER` (#145's G3, plant A19)."""
        self.assertEqual((pa.ASSIST_AFTER, pa.ASSIST_BURST), (10, 3))

    def test_the_group_after_the_sync_is_read_until_it_stays_put(self):
        synced = iter([59,   60, 60, 61, 62, 62,   62])
        _, _, rows = self.assisted(3, 10, [7_000_000_000], synced)
        self.assertEqual(rows, [['t1', '1010', '1020', '7000', '60', '60', '62']])

    def test_three_assists_a_second_apart_then_one_every_assist_after(self):
        """The pace the lead decided on #144: from ASSIST_AFTER after the
        acceptance, three a round apart, then one every ASSIST_AFTER."""
        # A round a second (t0 = 0): rounds at 1 to 35 s.
        _, _, rows = self.assisted(35, 10, iter(range(5_000_000_000, 10**12, 10**9)), itertools.repeat(60), step=1)
        self.assertEqual([r[3] for r in rows], ['5000', '6000', '7000', '8000', '9000'])
        # With ASSIST_AFTER 4: at 4, 5 and 6 s, then 10, 14, ... 34 s.
        os.unlink(os.path.join(self.d.name, 'assist.tsv'))
        _, _, rows = self.assisted(35, 4, iter(range(5_000_000_000, 10**12, 10**9)), itertools.repeat(60), step=1)
        self.assertEqual(len(rows), 3 + (35 - 6) // 4)

    def test_no_assist_unless_asked(self):
        end, calls, shell = self.run_flow([(0, f'cuts attachment={A} state={S} count=7 group=41 opens=itself\n'),
                                           (0, 'accepted\n')] + [(0, 'pending\n')] * 5, limit=250)
        self.assertFalse([c for c in shell if 'skyzfs:assist' in c])
        self.assertFalse(os.path.exists(os.path.join(self.d.name, 'assist.tsv')))

    def test_main_passes_the_assist_and_its_wait(self):
        got = {}
        with mock.patch.object(pa, 'flow', lambda *a, **k: got.update(k) or f'done commit={H}'), \
                mock.patch.object(pa, 'Agent', lambda *a: types.SimpleNamespace(log=self.log)), \
                contextlib.redirect_stdout(io.StringIO()):
            pa.main(['publish_agent.py', 'flow', 'skyzfsd', 'VOL', 'RUN', self.log, 'p', 't1', '--assist'])
            self.assertEqual(got['assist_after'], pa.ASSIST_AFTER)
            pa.main(['publish_agent.py', 'flow', 'skyzfsd', 'VOL', 'RUN', self.log, 'p', 't1', '--assist',
                     '--assist-after-s', '3'])
            self.assertEqual(got['assist_after'], 3)
            pa.main(['publish_agent.py', 'flow', 'skyzfsd', 'VOL', 'RUN', self.log, 'p', 't1',
                     '--assist-after-s', '3'])
            self.assertIsNone(got['assist_after'])


class WrittenGroup(unittest.TestCase):
    HEAD = 'txg      birth            state ndirty       nread        nwritten     reads    writes\n'

    def test_the_newest_committed_group_that_wrote(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, 'p'))
            with open(os.path.join(d, 'p', 'txgs'), 'w') as f:
                f.write(self.HEAD +
                        '41       100              C     4096         0            8192         0        2\n'
                        '43       300              C     4096         0            12288        0        3\n'
                        '42       200              C     0            0            4096         0        1\n'
                        '44       400              C     0            0            0            0        0\n'
                        '45       500              S     0            0            0            0        0\n'
                        '46       600              O     0            0            0            0        0\n')
            # 44 committed and wrote nothing: no uberblock, no cut.
            self.assertEqual(pa.written_txg('p', root=d), 43)

    def test_no_group_that_wrote_or_no_kstat_is_the_rig_s_error(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, 'p'))
            with open(os.path.join(d, 'p', 'txgs'), 'w') as f:
                f.write(self.HEAD + '44       400              C     0            0            0            0        0\n'
                        '45       500              O     0            0            0            0        0\n')
            with self.assertRaises(pa.RigError):
                pa.written_txg('p', root=d)
            with self.assertRaises(pa.RigError):
                pa.written_txg('q', root=d)


class Raw(unittest.TestCase):
    def test_the_bytes_go_as_given_and_the_answer_comes_back(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, 'vol-x'))
            path = os.path.join(d, 'vol-x', 'control')
            srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            srv.bind(path)
            srv.listen(1)
            got = []

            def serve():
                c, _ = srv.accept()
                data = b''
                while True:
                    b = c.recv(4096)
                    if not b:
                        break
                    data += b
                got.append(data)
                c.sendall(b'skyzfs-control 2\nrefused version not 2\n')
                c.close()
            th = threading.Thread(target=serve)
            th.start()
            log = os.path.join(d, 'agent.tsv')
            self.assertEqual(pa.raw(d, log, 'version', b'skyzfs-control 9\ncuts\n'), 'refused version not 2')
            th.join(10)
            srv.close()
            self.assertEqual(got, [b'skyzfs-control 9\ncuts\n'])
            with open(log) as f:
                self.assertEqual(f.read().split('\t')[2:], ['raw version', '1', 'refused version not 2\n'])

    def daemon(self, d, speaks, answers):
        """A daemon of version SPEAKS (2 or 3) on D's socket that reads
        each request whole and answers ANSWERS[k] (bytes after its own
        greeting) to the kth, one connection each: what each was sent
        is returned in a list, filled as they come."""
        os.makedirs(os.path.join(d, 'vol-x'), exist_ok=True)
        path = os.path.join(d, 'vol-x', 'control')
        srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        srv.bind(path)
        srv.listen(4)
        srv.settimeout(10)
        got = []

        def serve():
            try:
                for a in answers:
                    c, _ = srv.accept()
                    data = b''
                    while True:
                        b = c.recv(4096)
                        if not b:
                            break
                        data += b
                    got.append(data)
                    c.sendall(f'skyzfs-control {speaks}\n'.encode() + a)
                    c.close()
            finally:
                srv.close()
        th = threading.Thread(target=serve, daemon=True)
        th.start()
        return th, got

    def test_either_greeting_is_read(self):
        for speaks in (2, 3):
            with tempfile.TemporaryDirectory() as d:
                th, got = self.daemon(d, speaks, [f'cuts attachment={A} state=none count=0 group=none opens=none\n'
                                                  .encode()])
                log = os.path.join(d, 'agent.tsv')
                out = os.path.join(d, 'greeting')
                self.assertEqual(pa.raw(d, log, 'cuts', f'skyzfs-control {speaks}\ncuts\n'.encode(),
                                        greeting_out=out),
                                 f'cuts attachment={A} state=none count=0 group=none opens=none')
                th.join(10)
                self.assertEqual(got, [f'skyzfs-control {speaks}\ncuts\n'.encode()])
                with open(out) as f:
                    self.assertEqual(f.read(), f'skyzfs-control {speaks}\n')
        self.assertEqual(pa.answer_of(b'skyzfs-control 4\ncuts\n'), 'no answer line: skyzfs-control 4\\ncuts\\n')

    def test_an_offer_to_a_daemon_of_version_3_is_one_request(self):
        with tempfile.TemporaryDirectory() as d:
            th, got = self.daemon(d, 3, [b'refused malformed no key extra\n'])
            log = os.path.join(d, 'agent.tsv')
            self.assertEqual(pa.raw(d, log, 'malformed', b'skyzfs-control ?\ncuts extra=1\n'),
                             'refused malformed no key extra')
            th.join(10)
            self.assertEqual(got, [b'skyzfs-control 3\ncuts extra=1\n'])
            with open(log) as f:
                self.assertEqual(len(f.read().splitlines()), 1)

    def test_an_offer_to_a_daemon_of_version_2_falls_back_on_a_new_connection(self):
        with tempfile.TemporaryDirectory() as d:
            th, got = self.daemon(d, 2, [b'refused version not 2\n', b'refused malformed no key extra\n'])
            log = os.path.join(d, 'agent.tsv')
            out = os.path.join(d, 'greeting')
            self.assertEqual(pa.raw(d, log, 'malformed', b'skyzfs-control ?\ncuts extra=1\n', greeting_out=out),
                             'refused malformed no key extra')
            th.join(10)
            self.assertEqual(got, [b'skyzfs-control 3\ncuts extra=1\n', b'skyzfs-control 2\ncuts extra=1\n'])
            with open(log) as f:
                self.assertEqual([ln.split('\t')[4] for ln in f.read().splitlines()],
                                 ['refused version not 2', 'refused malformed no key extra'])
            with open(out) as f:
                self.assertEqual(f.read(), 'skyzfs-control 2\n')

    def test_an_offer_refused_otherwise_is_not_sent_again(self):
        """Only `version` with the greeting of version 2 is a fall back:
        another refusal, or `version` from a daemon of version 3, is the
        answer."""
        for speaks, answer in ((2, b'refused peer this daemon answers its own user\n'),
                               (3, b'refused version not 3\n')):
            with tempfile.TemporaryDirectory() as d:
                th, got = self.daemon(d, speaks, [answer, b'refused malformed x\n'])
                log = os.path.join(d, 'agent.tsv')
                self.assertEqual(pa.raw(d, log, 'peer', b'skyzfs-control ?\ncuts\n'), answer.decode().strip())
                self.assertEqual(got, [b'skyzfs-control 3\ncuts\n'])
                pa.raw(d, log, 'again', b'skyzfs-control 3\ncuts\n')
                th.join(10)

    REQUEST = b'skyzfs-control 2\ncuts\n'

    def serve_unread(self, d, answer):
        """A daemon that answers and closes without reading the request,
        as skyzfsd refuses a peer of another user. The request is only
        peeked at, so that it is there and unread when the daemon closes:
        on Linux the client's read after the answer then fails with
        ECONNRESET, not EOF."""
        os.makedirs(os.path.join(d, 'vol-x'))
        path = os.path.join(d, 'vol-x', 'control')
        srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        srv.bind(path)
        srv.listen(1)
        srv.settimeout(10)

        def serve():
            try:
                c, _ = srv.accept()
                c.settimeout(10)
                c.recv(len(self.REQUEST), socket.MSG_PEEK | socket.MSG_WAITALL)
                if answer:
                    c.sendall(answer)
                c.close()
            finally:
                srv.close()
        th = threading.Thread(target=serve, daemon=True)
        th.start()
        return th

    def test_an_answer_before_a_reset_is_the_answer(self):
        with tempfile.TemporaryDirectory() as d:
            th = self.serve_unread(d, b'skyzfs-control 2\nrefused peer this daemon answers its own user\n')
            log = os.path.join(d, 'agent.tsv')
            answer = pa.raw(d, log, 'peer', self.REQUEST)
            th.join(10)
            self.assertEqual(answer, 'refused peer this daemon answers its own user')
            with open(log) as f:
                self.assertEqual(f.read().split('\t')[2:],
                                 ['raw peer', '1', 'refused peer this daemon answers its own user\n'])

    def test_a_reset_with_nothing_written_is_the_answer_nothing(self):
        """A daemon that closes with the request unread and nothing
        written answers '', a verdict as it is when it read the request
        (#139's review, N5), and not the rig's error."""
        with tempfile.TemporaryDirectory() as d:
            th = self.serve_unread(d, b'')
            log = os.path.join(d, 'agent.tsv')
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                self.assertEqual(pa.raw(d, log, 'peer', self.REQUEST), '')
            th.join(10)
            self.assertIn('raw peer: the daemon closed with nothing written', err.getvalue())
            with open(log) as f:
                self.assertEqual(f.read().split('\t')[2:], ['raw peer', '0', '\n'])

    def test_a_socket_that_is_not_there_is_the_rigs_error(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, 'vol-x'))
            open(os.path.join(d, 'vol-x', 'control'), 'w').close()  # a file, not a socket
            log = os.path.join(d, 'agent.tsv')
            with self.assertRaises(pa.RigError):
                pa.raw(d, log, 'peer', self.REQUEST)
            with open(log) as f:
                self.assertEqual(f.read().split('\t')[2:4], ['raw peer', '2'])

    def serve_first(self, d, answer, closed):
        """A daemon that answers and closes before the client has written
        a byte: CLOSED is set once it has closed."""
        os.makedirs(os.path.join(d, 'vol-x'))
        path = os.path.join(d, 'vol-x', 'control')
        srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        srv.bind(path)
        srv.listen(1)
        srv.settimeout(10)

        def serve():
            try:
                c, _ = srv.accept()
                if answer:
                    c.sendall(answer)
                c.close()
            finally:
                srv.close()
                closed.set()
        th = threading.Thread(target=serve, daemon=True)
        th.start()
        return th

    def late_writer(self, closed):
        """publish_agent's `socket` module with a socket whose writes wait
        for CLOSED: the other order on the wire, where the client's write
        fails with EPIPE (#139's review, X1). The test's server keeps the
        real module."""
        mod = types.ModuleType('socket')
        mod.__dict__.update(socket.__dict__)

        class Late(socket.socket):
            def sendall(self, data, *args):
                closed.wait(10)
                return super().sendall(data, *args)
        mod.socket = Late
        return mock.patch.object(pa, 'socket', mod)

    def test_an_answer_before_the_client_writes_is_the_answer(self):
        with tempfile.TemporaryDirectory() as d:
            closed = threading.Event()
            th = self.serve_first(d, b'skyzfs-control 2\nrefused peer this daemon answers its own user\n', closed)
            log = os.path.join(d, 'agent.tsv')
            with self.late_writer(closed):
                answer = pa.raw(d, log, 'peer', self.REQUEST)
            th.join(10)
            self.assertTrue(closed.is_set())
            self.assertEqual(answer, 'refused peer this daemon answers its own user')

    def test_a_close_before_the_client_writes_with_nothing_is_the_answer_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            closed = threading.Event()
            th = self.serve_first(d, b'', closed)
            log = os.path.join(d, 'agent.tsv')
            err = io.StringIO()
            with self.late_writer(closed), contextlib.redirect_stderr(err):
                self.assertEqual(pa.raw(d, log, 'peer', self.REQUEST), '')
            th.join(10)
            self.assertIn('the daemon closed with nothing written', err.getvalue())

    def test_only_a_whole_line_is_an_answer(self):
        """As skyzfsd's own `ask` takes it (#139's review, N5)."""
        cases = [
            (b'', ''),
            (b'skyzfs-control 2\nrefused peer x\n', 'refused peer x'),
            (b'skyzfs-control 2\nrefused peer x\nmore\n', 'refused peer x'),
            # No greeting, or another version's: skyzfsd's own ask refuses
            # it (#139's second look, D2).
            (b'cuts 1\n', 'no answer line: cuts 1\\n'),
            (b'refused peer x\nmore\n', 'no answer line: refused peer x\\nmore\\n'),
            (b'skyzfs-control 1\nrefused peer x\n', 'no answer line: skyzfs-control 1\\nrefused peer x\\n'),
            (b'skyzfs-control 2\nrefused peer this daem', 'no answer line: skyzfs-control 2\\nrefused peer this daem'),
            (b'refused peer this daem', 'no answer line: refused peer this daem'),
            (b'skyzfs-control 2\n', 'no answer line: skyzfs-control 2\\n'),
            (b'skyzfs-control 2\na\tb', 'no answer line: skyzfs-control 2\\na\\tb'),
        ]
        for got, want in cases:
            self.assertEqual(pa.answer_of(got), want, got)

    def test_a_daemon_that_never_answers_is_a_verdict(self):
        """Connected, nothing written and no close within the limit: the
        answer `no answer within ... s`, not the rig's error (#139's
        second look, D3)."""
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, 'vol-x'))
            path = os.path.join(d, 'vol-x', 'control')
            srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            srv.bind(path)
            srv.listen(1)
            held = []
            done = threading.Event()

            def serve():
                c, _ = srv.accept()
                held.append(c)
                done.wait(10)
                c.close()
            th = threading.Thread(target=serve, daemon=True)
            th.start()
            log = os.path.join(d, 'agent.tsv')
            err = io.StringIO()
            try:
                with contextlib.redirect_stderr(err):
                    answer = pa.raw(d, log, 'peer', self.REQUEST, limit=0.3)
            finally:
                done.set()
                th.join(10)
                srv.close()
            self.assertEqual(answer, 'no answer within 0.3 s')
            self.assertIn('did not close within 0.3 s (0 bytes written)', err.getvalue())

    def test_a_socket_with_no_listener_is_the_rigs_error(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, 'vol-x'))
            path = os.path.join(d, 'vol-x', 'control')
            srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            srv.bind(path)
            srv.close()  # the file stays, with nobody listening: ECONNREFUSED
            with self.assertRaises(pa.RigError):
                pa.raw(d, os.path.join(d, 'agent.tsv'), 'peer', self.REQUEST)

    def test_a_cut_short_answer_is_served_as_no_answer_line(self):
        with tempfile.TemporaryDirectory() as d:
            th = self.serve_unread(d, b'skyzfs-control 2\nrefused peer this daem')
            log = os.path.join(d, 'agent.tsv')
            self.assertEqual(pa.raw(d, log, 'peer', self.REQUEST),
                             'no answer line: skyzfs-control 2\\nrefused peer this daem')
            th.join(10)

    def test_not_one_socket_is_the_rigs_error(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(pa.RigError):
                pa.socket_path(d)


if __name__ == '__main__':
    unittest.main()
