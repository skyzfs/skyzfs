#!/usr/bin/env python3
"""Tests of the benchmark's releaser of held cuts (cut_release.py) that
need no kernel: when a cut is due, and the releaser against the cut
rig's fake daemon (tests/kernel/test_cut_hold.py)."""
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, '..', 'kernel'))
import cut_hold as ch  # noqa: E402
import cut_release as cr  # noqa: E402
import test_cut_hold as tch  # noqa: E402


def cut(n, phase):
    return {'number': n, 'group': n + 4, 'phase': phase, 'opens': 'itself', 'blocks': 1}


class Due(unittest.TestCase):
    def test_every_cut(self):
        self.assertEqual(cr.due([cut(3, 'uploading')], 1), 3)
        self.assertEqual(cr.due([cut(3, 'uploading'), cut(5, 'waiting')], 1), 3)
        self.assertIsNone(cr.due([], 1))
        self.assertIsNone(cr.due([cut(3, 'flushing')], 1))

    def test_every_fourth(self):
        self.assertIsNone(cr.due([cut(3, 'uploading'), cut(5, 'waiting')], 4))
        self.assertEqual(cr.due([cut(3, 'uploading'), cut(6, 'waiting')], 4), 3)
        self.assertEqual(cr.due([cut(3, 'uploading'), cut(6, 'flushing')], 4), 3)
        self.assertIsNone(cr.due([cut(3, 'uploading')], 2))


class AgainstAFake(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        # the volume's directory and the run directory, and the volume's
        # run directory under it as the daemon names it
        self.vol, self.run = os.path.join(self.d, 'vol'), os.path.join(self.d, 'run')
        os.makedirs(self.vol)
        self.vrun = ch.run_dir_of(self.vol, self.run)
        os.makedirs(self.vrun)
        self.proc = None

    def tearDown(self):
        if self.proc is not None:
            open(os.path.join(self.vrun, 'stop'), 'w').close()
            self.proc.wait(timeout=10)
        shutil.rmtree(self.d)

    def fake(self, cuts, stuck=()):
        self.proc = subprocess.Popen([sys.executable, '-I', '-c', tch.FAKE, self.vrun, json.dumps(cuts),
                                      '[]', '[]', json.dumps(list(stuck))])
        end = time.monotonic() + 10
        while not os.path.exists(os.path.join(self.vrun, 'ready')):
            self.assertLess(time.monotonic(), end, 'the fake did not start')
            time.sleep(0.01)
        return ch.Daemon(self.proc.pid, self.vrun, io.StringIO())

    def test_releases_each_cut_in_turn(self):
        out = io.StringIO()
        r = cr.Releaser(self.fake({'1': [5, 'itself', {}], '2': [6, 'itself', {}]}), 1, out)
        self.assertTrue(r.step())
        self.assertTrue(r.step())
        self.assertFalse(r.step())
        self.assertEqual([ln.split('\t')[1:] for ln in out.getvalue().splitlines()], [['1', '2'], ['2', '2']])
        self.assertEqual((r.released, r.polls), (2, 3))

    def test_holds_until_enough_cuts_are_taken(self):
        out = io.StringIO()
        r = cr.Releaser(self.fake({'1': [5, 'itself', {}], '2': [6, 'itself', {}]}), 2, out)
        self.assertTrue(r.step())     # 2 - 1 >= 1
        self.assertFalse(r.step())    # cut 2 alone
        self.assertEqual(r.released, 1)

    def test_a_refused_release(self):
        r = cr.Releaser(self.fake({'1': [5, 'itself', {}]}, stuck=[1]), 1, io.StringIO())
        with self.assertRaisesRegex(ch.HolderError, 'release 1 refused'):
            r.step()

    def test_main_to_the_stop(self):
        self.fake({'1': [5, 'itself', {}], '2': [6, 'itself', {}]})
        out, stop = os.path.join(self.d, 'rel.tsv'), os.path.join(self.d, 'stop-releaser')
        p = subprocess.Popen([sys.executable, '-I', os.path.join(HERE, 'cut_release.py'), out, str(self.proc.pid),
                              self.vol, self.run, '1', stop])
        end = time.monotonic() + 10
        while time.monotonic() < end:
            if os.path.exists(out) and len(open(out).read().splitlines()) >= 2:
                break
            time.sleep(0.05)
        open(stop, 'w').close()
        self.assertEqual(p.wait(timeout=10), 0)
        with open(out) as f:
            lines = f.read().splitlines()
        self.assertEqual([ln.split('\t')[1] for ln in lines[:-1]], ['1', '2'])
        self.assertTrue(lines[-1].startswith('# released 2, polls '), lines[-1])

    def test_main_with_no_daemon_answering(self):
        """A release build has no commands: no reply, the releaser's error."""
        out, stop = os.path.join(self.d, 'rel.tsv'), os.path.join(self.d, 'stop-releaser')
        sleeper = subprocess.Popen([sys.executable, '-I', '-c',
                                    'import signal, time; signal.signal(signal.SIGUSR2, lambda *a: None); time.sleep(30)'])
        try:
            time.sleep(0.3)
            env = dict(os.environ, CUT_RELEASE_REPLY_S='1')
            p = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'cut_release.py'), out, str(sleeper.pid),
                                self.vol, self.run, '1', stop], env=env, capture_output=True, text=True, timeout=30)
        finally:
            sleeper.kill()
            sleeper.wait()
        self.assertEqual(p.returncode, 2)
        with open(out) as f:
            self.assertTrue(f.read().splitlines()[-1].startswith('# ERROR HolderError: no reply'))

    def test_every_must_be_a_count(self):
        p = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'cut_release.py'), 'o', '1', '.', '.', '0', 's'],
                           capture_output=True, text=True)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('EVERY', p.stderr)


if __name__ == '__main__':
    unittest.main()
