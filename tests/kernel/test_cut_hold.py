#!/usr/bin/env python3
"""Tests of the cut rig's holder (cut_hold.py) that need no kernel: its
parsing, and the holder against a fake daemon that answers the commands
about held cuts as bins/skyzfsd/src/cut_commands.rs says they are
answered (SIGUSR2, cut-command, cut-reply by rename, the token on the
reply's last line)."""
import hashlib
import io
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_hold as ch  # noqa: E402

# The fake: cuts {number: (group, opens, {block: byte})}, the oldest
# being uploaded and the next waiting; `write` puts each block of the
# map (4096 copies of its byte) into PATH and lists them in PATH.blocks;
# `release` lets the uploading cut go. A cut in REFUSE is refused at the
# end of its write, as one abandoned under it.
FAKE = r'''
import os, signal, sys, time, json
vrun = sys.argv[1]
cuts = {int(k): v for k, v in json.loads(sys.argv[2]).items()}
refuse = set(json.loads(sys.argv[3]))
asked = []
signal.signal(signal.SIGUSR2, lambda *a: asked.append(1))
open(os.path.join(vrun, 'ready'), 'w').close()

def reply(text):
    with open(os.path.join(vrun, 'cut-reply.tmp'), 'w') as f:
        f.write(text)
    os.rename(os.path.join(vrun, 'cut-reply.tmp'), os.path.join(vrun, 'cut-reply'))

while not os.path.exists(os.path.join(vrun, 'stop')):
    if not asked:
        time.sleep(0.005)
        continue
    asked.clear()
    token, cmd = open(os.path.join(vrun, 'cut-command')).read().split('\n')[0].split(' ', 1)
    order = sorted(cuts)
    phase = {n: ('uploading' if k == 0 else 'waiting') for k, n in enumerate(order)}
    w = cmd.split()
    if w[0] == 'held':
        reply(''.join(f"{n} {cuts[n][0]} {phase[n]} {cuts[n][1]} {len(cuts[n][2])}\n" for n in order)
              + f'{token} ok\n')
    elif w[0] == 'write':
        n, path = int(w[1]), w[2]
        if n not in cuts:
            reply(f'{token} refused: no cut {n} is held\n')
            continue
        blocks = sorted(int(b) for b in cuts[n][2])
        with open(path, 'r+b') as f:
            for b in blocks:
                f.seek(b * 4096)
                f.write(bytes([cuts[n][2][str(b)]]) * 4096)
        with open(path + '.blocks', 'w') as f:
            f.write(''.join(f'{b}\n' for b in blocks))
        if n in refuse:
            reply(f'{token} refused: cut {n} is no longer held after 0 of {len(blocks)} blocks\n')
            del cuts[n]
        else:
            reply(f'{token} ok {len(blocks)} blocks\n')
    elif w[0] == 'release':
        n = int(w[1])
        if order and order[0] == n:
            del cuts[n]
            reply(f'{token} ok\n')
        else:
            reply(f'{token} refused: cut {n} is not the one being uploaded\n')
'''


def h(byte):
    return hashlib.sha256(bytes([byte]) * 4096).digest()[:8].hex()


class Parsing(unittest.TestCase):
    def test_a_reply_is_its_tokens(self):
        self.assertEqual(ch.parse_reply('1 5 uploading itself 3\nt1 ok\n', 't1'),
                         (True, ['1 5 uploading itself 3'], 'ok'))
        self.assertIsNone(ch.parse_reply('t0 ok\n', 't1'))
        self.assertIsNone(ch.parse_reply('t1x ok\n', 't1'))
        self.assertIsNone(ch.parse_reply('', 't1'))
        self.assertEqual(ch.parse_reply('t1 refused: busy\n', 't1'), (False, [], 'refused: busy'))

    def test_held(self):
        self.assertEqual(ch.parse_held(['3 40 waiting rewind 7']),
                         [{'number': 3, 'group': 40, 'phase': 'waiting', 'opens': 'rewind', 'blocks': 7}])
        for bad in ('3 40 waiting rewind', '3 40 done itself 7', 'x 40 waiting itself 7', '3 40 waiting maybe 7'):
            with self.assertRaises(ch.HolderError):
                ch.parse_held([bad])

    def test_the_volumes_run_directory(self):
        d = tempfile.mkdtemp()
        try:
            st = os.stat(d)
            self.assertEqual(ch.run_dir_of(d, '/run/x'), f'/run/x/v-{st.st_dev:x}-{st.st_ino:x}')
        finally:
            shutil.rmtree(d)

    def test_hashes(self):
        d = tempfile.mkdtemp()
        try:
            img = os.path.join(d, 'img')
            with open(img, 'wb') as f:
                f.write(bytes(4096) + bytes([7]) * 4096 + bytes(4096))
            self.assertEqual(ch.hash_listed(img, [0, 1]), {0: ch.ZERO, 1: h(7)})
            self.assertEqual(ch.hash_all(img), {1: h(7)})
            with self.assertRaises(ch.HolderError):
                ch.hash_listed(img, [3])
        finally:
            shutil.rmtree(d)


class AgainstAFake(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.vrun = os.path.join(self.d, 'vrun')
        self.out = os.path.join(self.d, 'out')
        os.makedirs(self.vrun)
        os.makedirs(self.out)
        self.image = os.path.join(self.d, 'image')
        with open(self.image, 'wb') as f:
            f.truncate(16 * 4096)
        self.proc = None

    def tearDown(self):
        if self.proc is not None:
            open(os.path.join(self.vrun, 'stop'), 'w').close()
            self.proc.wait(timeout=10)
        shutil.rmtree(self.d)

    def fake(self, cuts, refuse=()):
        import json
        self.proc = subprocess.Popen([sys.executable, '-I', '-c', FAKE, self.vrun, json.dumps(cuts),
                                      json.dumps(list(refuse))])
        end = time.monotonic() + 10
        while not os.path.exists(os.path.join(self.vrun, 'ready')):
            self.assertLess(time.monotonic(), end, 'the fake did not start')
            time.sleep(0.01)
        return ch.Holder(ch.Daemon(self.proc.pid, self.vrun, io.StringIO()), self.image, self.out)

    def read(self, name):
        with open(os.path.join(self.out, name)) as f:
            return f.read().splitlines()

    def test_write_hash_release(self):
        holder = self.fake({'1': [5, 'itself', {'0': 1, '3': 2}], '2': [6, 'rewind', {'3': 9}]})
        self.assertTrue(holder.step())
        self.assertTrue(holder.step())
        self.assertFalse(holder.step())
        self.assertEqual(holder.held(), [])
        self.assertEqual([ln.split('\t')[:4] for ln in self.read('held.tsv')],
                         [['1', '5', 'itself', '2'], ['2', '6', 'rewind', '1']])
        self.assertEqual(self.read('held-blocks.tsv'), [f'1\t0\t{h(1)}', f'1\t3\t{h(2)}', f'2\t3\t{h(9)}'])
        self.assertEqual(ch.hash_all(self.image), {0: h(1), 3: h(9)})

    def test_a_write_refused_at_its_end_is_not_recorded(self):
        holder = self.fake({'1': [5, 'itself', {'0': 1}], '2': [6, 'itself', {'0': 4, '1': 3}]}, refuse=[1])
        while holder.step():
            pass
        self.assertEqual([ln.split('\t')[0] for ln in self.read('held.tsv')], ['2'])
        self.assertEqual(holder.written, [2])

    def test_a_daemon_gone(self):
        holder = self.fake({})
        open(os.path.join(self.vrun, 'stop'), 'w').close()
        self.proc.wait(timeout=10)
        self.proc = None
        with self.assertRaises(ch.HolderError):
            holder.held()

    def test_no_reply(self):
        d = ch.Daemon(os.getpid(), self.vrun, io.StringIO(), clock=iter(range(0, 10 ** 6, 100)).__next__,
                      sleep=lambda s: None)
        import signal
        old = signal.signal(signal.SIGUSR2, lambda *a: None)
        try:
            with self.assertRaises(ch.HolderError):
                d.send('held', limit=150)
        finally:
            signal.signal(signal.SIGUSR2, old)


if __name__ == '__main__':
    unittest.main()
