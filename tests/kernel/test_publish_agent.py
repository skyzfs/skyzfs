#!/usr/bin/env python3
"""Tests of the agent stand-in of the channel rig (publish_agent.py): no kernel."""
import os
import socket
import subprocess
import sys
import tempfile
import threading
import unittest

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

    def test_what_is_not_an_answer_is_the_rigs_error(self):
        for line in ('', 'refused ', 'cuts', f'cuts attachment={A} state={S} count=7 group=41',
                     f'cuts attachment={A} state={S} count=7 group=41 opens=itself extra=1',
                     f'cuts attachment={A[:-1]} state={S} count=7 group=41 opens=itself',
                     f'cuts attachment={A} state={S} count=x group=41 opens=itself',
                     f'cuts attachment={A} state={S} count=7 group=none opens=itself',
                     f'cuts attachment={A} state={S} count=7 group=41 opens=maybe',
                     'done', f'done commit={H[:-1]}', f'done commit={H.upper()}', 'accepted now',
                     'pending  ', 'publish', f'done commit={H} commit={H}'):
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
                        (0, 'withdrawn\n'))
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
        self.assertEqual(self.fake.calls[4][1], 'withdraw')
        with open(self.log) as f:
            rows = [ln.rstrip('\n').split('\t') for ln in f]
        self.assertEqual(len(rows), 5)
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

    def run_flow(self, answers, sync=False, limit=300, expiry=None, hold=True):
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
                      shell=sh, files=lambda root: f'{"0" * 64}  f1\n'.encode(), now=lambda: next(clock))
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
                                 'zpool sync p', 'zfs release skyzfs-publish p@t1'])
        with open(os.path.join(self.d.name, 'manifest-t1'), 'rb') as f:
            self.assertEqual(f.read(), f'{"0" * 64}  f1\n'.encode())
        with open(os.path.join(self.d.name, 'published.tsv')) as f:
            self.assertEqual(f.read().split('\t')[:5], ['t1', '55', A, S, '7'])

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
                c.sendall(b'skyzfs-control 1\nrefused version not 1\n')
                c.close()
            th = threading.Thread(target=serve)
            th.start()
            log = os.path.join(d, 'agent.tsv')
            self.assertEqual(pa.raw(d, log, 'version', b'skyzfs-control 9\ncuts\n'), 'refused version not 1')
            th.join(10)
            srv.close()
            self.assertEqual(got, [b'skyzfs-control 9\ncuts\n'])
            with open(log) as f:
                self.assertEqual(f.read().split('\t')[2:], ['raw version', '1', 'refused version not 1\n'])

    def test_not_one_socket_is_the_rigs_error(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(pa.RigError):
                pa.socket_path(d)


if __name__ == '__main__':
    unittest.main()
