#!/usr/bin/env python3
"""The cut rig's watcher (cut_watch.py) with no object store: node B is a
stand-in script that prints what files in a directory say."""
import os
import shutil
import sys
import tempfile
import textwrap
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_watch as cw  # noqa: E402

STATE = 'ab' * 16
HEAD = 'cd' * 32

# node B's stand-in: `row` prints DIR/row.txt; `get` copies DIR/head.img
# to --out and, where DIR/move exists, puts its text in row.txt first
# (head moved under the read); DIR/get-rc is get's exit, and DIR/row-rc
# row's.
NODEB = textwrap.dedent('''\
    #!/usr/bin/env python3
    import os, shutil, sys
    d = os.environ['FAKE_NODEB']
    cmd, args = sys.argv[1], sys.argv[2:]
    def rc(name):
        p = os.path.join(d, name)
        if not os.path.exists(p):
            return 0
        with open(p) as f:
            return int(f.read())
    if cmd == 'row':
        if rc('row-rc'):
            sys.exit(rc('row-rc'))
        with open(os.path.join(d, 'row.txt')) as f:
            sys.stdout.write(f.read())
    elif cmd == 'get':
        if rc('get-rc'):
            sys.stderr.write('no such object\\n')
            sys.exit(rc('get-rc'))
        mv = os.path.join(d, 'move')
        if os.path.exists(mv):
            shutil.copy(mv, os.path.join(d, 'row.txt'))
            os.remove(mv)
        out = args[args.index('--out') + 1]
        shutil.copy(os.path.join(d, 'head.img'), out)
        print('read\\thead\\t' + 'cd' * 32 + '\\t0')
''')


def row_text(number, group, head=HEAD, state=STATE):
    return f'head\t{head}\t{number}\nrow\t{state}\t{number}\t{group}\nlease\trunner\t1\t99\n'


class Pure(unittest.TestCase):
    def test_hash_and_changed(self):
        d = tempfile.mkdtemp(prefix='cw-')
        try:
            p = os.path.join(d, 'img')
            with open(p, 'wb') as f:
                f.write(bytes(4096) + b'\1' * 4096 + bytes(4096) + b'\2' + bytes(100))
            h = cw.hash_image(p)
            self.assertEqual(sorted(h), [1, 3])
            self.assertEqual(h[3], cw.hashlib.sha256(b'\2' + bytes(4095)).digest()[:8].hex())
            self.assertEqual(cw.changed({1: 'a', 2: 'b'}, {1: 'a', 3: 'c'}), {3: 'c', 2: cw.ZERO})
        finally:
            shutil.rmtree(d)

    def test_rows(self):
        got = cw.parse_row(row_text(4, 17))
        self.assertEqual(cw.row_cut(got['row']), (STATE, 4, 17))
        self.assertIsNone(cw.row_cut('none'))
        self.assertIsNone(cw.row_cut('unreadable 0200'))
        with self.assertRaises(cw.RigError):
            cw.parse_row('head\tnone\n')


class Steps(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp(prefix='cw-')
        self.out = os.path.join(self.d, 'out')
        self.fake = os.path.join(self.d, 'fake')
        os.makedirs(self.out)
        os.makedirs(self.fake)
        self.nodeb = os.path.join(self.d, 'nodeb')
        with open(self.nodeb, 'w') as f:
            f.write(NODEB)
        os.chmod(self.nodeb, 0o755)
        os.environ['FAKE_NODEB'] = self.fake
        self.w = cw.Watcher(self.out, self.nodeb, 'http://x', 'b', 'v')

    def tearDown(self):
        shutil.rmtree(self.d)
        os.environ.pop('FAKE_NODEB', None)

    def put(self, name, text):
        with open(os.path.join(self.fake, name), 'w' if isinstance(text, str) else 'wb') as f:
            f.write(text)

    def read(self, name):
        p = os.path.join(self.out, name)
        if not os.path.exists(p):
            return ''
        with open(p) as f:
            return f.read()

    def test_each_cut_once_with_what_changed(self):
        self.put('row.txt', 'head\tnone\nrow\tnone\nlease\tnone\n')
        self.assertFalse(self.w.step())
        self.put('row.txt', row_text(1, 10))
        self.put('head.img', b'\1' * 4096 + b'\2' * 4096)
        self.assertTrue(self.w.step())
        self.assertFalse(self.w.step())         # the same cut
        self.put('row.txt', row_text(2, 11, head='ef' * 32))
        self.put('head.img', bytes(4096) + b'\3' * 4096)
        self.assertTrue(self.w.step())
        lines = [ln.split('\t') for ln in self.read('uploaded.tsv').splitlines()]
        self.assertEqual([(ln[0], ln[1], ln[2], ln[4]) for ln in lines],
                         [('1', '10', STATE, '2'), ('2', '11', STATE, '1')])
        blocks = [ln.split('\t') for ln in self.read('uploaded-blocks.tsv').splitlines()]
        self.assertEqual([(k, n, b) for k, n, b, _ in blocks],
                         [('0', '1', '0'), ('0', '1', '1'), ('1', '2', '0'), ('1', '2', '1')])
        self.assertEqual(blocks[2][3], cw.ZERO)
        self.assertEqual(len(self.read('rows.tsv').splitlines()), 6)

    def test_head_moved_under_the_read(self):
        self.put('row.txt', row_text(1, 10))
        self.put('head.img', b'\1' * 4096)
        self.put('move', row_text(2, 11, head='ef' * 32))
        self.assertFalse(self.w.step())
        self.assertEqual(self.read('uploaded.tsv'), '')
        self.assertIn('head moved under the read', self.read('watch.log'))
        self.assertTrue(self.w.step())          # the next look takes cut 2
        self.assertTrue(self.read('uploaded.tsv').startswith('2\t11\t'))

    def test_head_or_the_row_alone_moved(self):
        """The courses move them apart: move-head moves head and not the
        row, set-row the row and not head. Either alone drops the fetch
        (review of #119, W02a and W02b)."""
        for why, moved in (('head alone', row_text(1, 10, head='ef' * 32)), ('the row alone', row_text(2, 11))):
            with self.subTest(why):
                self.w.last = None
                self.put('row.txt', row_text(1, 10))
                self.put('head.img', b'\1' * 4096)
                self.put('move', moved)
                self.assertFalse(self.w.step())
                self.assertEqual(self.read('uploaded.tsv'), '')

    def test_what_ends_the_watcher(self):
        self.put('row.txt', row_text(1, 10))
        self.put('head.img', b'\1' * 4096)
        self.put('get-rc', '4')
        with self.assertRaises(cw.Verdict):
            self.w.step()
        self.put('get-rc', '0')
        self.put('row-rc', '2')
        self.assertFalse(self.w.step())
        self.assertFalse(self.w.step())
        with self.assertRaises(cw.RigError):
            self.w.step()

    def test_a_fetch_that_keeps_failing(self):
        """A get that fails each time is the rig's error at the third,
        though the row read before each works (review of #119)."""
        self.put('row.txt', row_text(1, 10))
        self.put('head.img', b'\1' * 4096)
        self.put('get-rc', '2')
        self.assertFalse(self.w.step())
        self.assertFalse(self.w.step())
        with self.assertRaises(cw.RigError):
            self.w.step()

    def test_the_last_look(self):
        """At STOP: a cut already fetched, or no cut named, ends it; a
        fetch that fails three times is the rig's error."""
        self.put('row.txt', 'head\tnone\nrow\tnone\nlease\tnone\n')
        self.w.finish()
        self.put('row.txt', row_text(1, 10))
        self.put('head.img', b'\1' * 4096)
        self.w.finish()
        self.assertTrue(self.read('uploaded.tsv').startswith('1\t10\t'))
        self.w.finish()                         # the same cut: nothing to fetch
        self.assertEqual(len(self.read('uploaded.tsv').splitlines()), 1)
        self.put('row.txt', row_text(2, 11, head='ef' * 32))
        self.put('get-rc', '2')
        with self.assertRaises(cw.RigError):
            self.w.finish()
        self.assertEqual(len(self.read('uploaded.tsv').splitlines()), 1)
        # Looks that never settle (head moved under each read): the
        # rig's error after three, not a quiet end.
        looks = []

        def moving():
            looks.append(1)
            self.w.settled = False
            return False
        self.w.step = moving
        with self.assertRaises(cw.RigError):
            self.w.finish()
        self.assertEqual(len(looks), cw.TRIES)


if __name__ == '__main__':
    unittest.main()
