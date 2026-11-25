#!/usr/bin/env python3
"""Tests of the agent stand-in of the channel rig (publish_agent.py): no kernel."""
import contextlib
import io
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
                     'done', f'done commit={H[:-1]}', f'done commit={H.upper()}', 'accepted now',
                     'pending  ', 'publish', f'done commit={H} commit={H}',
                     f'snap commit={H} state={S} number=9 group=51 manifest-len=911',
                     f'snap commit={H[:-1]} state={S} number=9 group=51 manifest-len=911 manifest-hash={H}',
                     f'snap commit={H} state={S} number=x group=51 manifest-len=911 manifest-hash={H}',
                     f'snap commit={H} state={S[:-1]} number=9 group=51 manifest-len=911 manifest-hash={H}',
                     'absent now', f'unreadable commit={H}'):
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
