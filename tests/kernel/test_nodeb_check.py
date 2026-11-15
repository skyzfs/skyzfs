#!/usr/bin/env python3
"""Tests of node B's logic that need no kernel (M2.T2): which snapshot and
which cuts it takes, the files it reads, the blocks its controls take."""
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_ref  # noqa: E402
import nodeb_check  # noqa: E402
from cut_check import RigError  # noqa: E402

FIXTURES = os.path.join(HERE, '..', 'fixtures', 'pools')


def mark(i, name):
    return {'i': i, 'flags': ['mark'], 'mark': name, 'offset': 0, 'bytes': 0}


def cut(n, txg, ub_record):
    return {'n': n, 'txg': txg, 'ub_record': ub_record, 'prefix': ub_record + 2}


class Files(unittest.TestCase):
    def test_snaps(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(nodeb_check.read_snaps(d), [])
            with open(os.path.join(d, 'snaps.tsv'), 'w') as f:
                f.write('s6\t12\t1\ns12\t15\t2\n')
            self.assertEqual(nodeb_check.read_snaps(d), [('s6', 12, 1), ('s12', 15, 2)])
            with open(os.path.join(d, 'snaps.tsv'), 'a') as f:
                f.write('s18\t\t2\n')
            with self.assertRaises(RigError):
                nodeb_check.read_snaps(d)

    def test_sums(self):
        a, b = '0' * 64, 'ab' * 32
        self.assertEqual(nodeb_check.read_sums(f'{a}  f1\n{b} *f 2\n'), {'f1': a, 'f 2': b})
        with self.assertRaises(RigError):
            nodeb_check.read_sums('not a sum line')


class Choose(unittest.TestCase):
    # Two sessions: records 0 and 50 are their begin marks.
    RECORDS = [mark(0, 'session-1-begin')] + [{'i': i, 'flags': []} for i in range(1, 50)] \
        + [mark(50, 'session-2-begin')] + [{'i': i, 'flags': []} for i in range(51, 100)]
    CUTS = [cut(1, 4, 5), cut(2, 6, 10), cut(3, 9, 20), cut(4, 12, 30),
            cut(5, 13, 55), cut(6, 16, 60), cut(7, 19, 70)]

    def spans(self):
        return nodeb_check.spans_of(self.RECORDS)

    def test_spans(self):
        self.assertEqual(self.spans(), {1: (0, 50), 2: (50, None)})

    def test_the_cut_at_or_after_the_group_and_the_one_before(self):
        ok = lambda c: True  # noqa: E731
        (snap, at, early) = nodeb_check.choose([('s', 8, 1)], self.CUTS, ok, self.spans())
        self.assertEqual((snap, at['n'], early['n']), (('s', 8), 3, 2))
        # its own group publishes it
        (_, at, early) = nodeb_check.choose([('s', 9, 1)], self.CUTS, ok, self.spans())
        self.assertEqual((at['n'], early['n']), (3, 2))

    def test_within_its_session(self):
        ok = lambda c: True  # noqa: E731
        # Group 13 in session 2: cut 5 (group 13) is the session's first
        # cut, so nothing in the session comes a group early.
        self.assertIsNone(nodeb_check.choose([('s', 13, 2)], self.CUTS, ok, self.spans()))
        (_, at, early) = nodeb_check.choose([('s', 15, 2)], self.CUTS, ok, self.spans())
        self.assertEqual((at['n'], early['n']), (6, 5))
        # A snapshot after the session's last cut, or before its first.
        self.assertIsNone(nodeb_check.choose([('s', 13, 1)], self.CUTS, ok, self.spans()))
        self.assertIsNone(nodeb_check.choose([('s', 3, 1)], self.CUTS, ok, self.spans()))
        self.assertIsNone(nodeb_check.choose([('s', 8, 3)], self.CUTS, ok, self.spans()))

    def test_only_cuts_that_may_be_used(self):
        self.assertIsNone(nodeb_check.choose([('s', 8, 1)], self.CUTS, lambda c: c['n'] != 3, self.spans()))
        self.assertIsNone(nodeb_check.choose([('s', 8, 1)], self.CUTS, lambda c: c['n'] != 2, self.spans()))

    def test_the_middle_of_those_that_qualify(self):
        ok = lambda c: True  # noqa: E731
        snaps = [('a', 5, 1), ('b', 8, 1), ('c', 11, 1), ('d', 13, 2)]
        (snap, _, _) = nodeb_check.choose(snaps, self.CUTS, ok, self.spans())
        self.assertEqual(snap, ('b', 8))

    def test_on_a_recorded_pool(self):
        """life, from run 43647779663: each session's snapshots get a cut."""
        records, read, dev = cut_ref.read_fixture(os.path.join(FIXTURES, 'life'))
        cs, _ = cut_ref.cuts(records, cut_ref.scan(records, read, dev))
        spans = nodeb_check.spans_of(records)
        self.assertEqual(sorted(spans), [1, 2])
        for k, (lo, hi) in spans.items():
            span = [c for c in cs if lo < c['ub_record'] and (hi is None or c['ub_record'] < hi)]
            self.assertGreater(len(span), 2)
            group = span[2]['txg']
            (_, at, early) = nodeb_check.choose([('s', group, k)], cs, lambda c: True, spans)
            self.assertEqual((at['n'], early['n']), (span[2]['n'], span[1]['n']))


class Blocks(unittest.TestCase):
    def test_later_uberblock(self):
        ubs = [{'record': 5, 'txg': 9}, {'record': 12, 'txg': 9}, {'record': 14, 'txg': 10}]
        c = {'txg': 9, 'prefix': 11}
        self.assertEqual(nodeb_check.later_uberblock(ubs, c), 14)
        self.assertIsNone(nodeb_check.later_uberblock(ubs, {'txg': 10, 'prefix': 11}))

    def test_live_block(self):
        bps = [{'type': 'DMU dnode', 'level': 0, 'dvas': [(0, 0x1000, 0x1000)]},
               {'type': 'ZFS plain file', 'level': 1, 'dvas': [(0, 0x2000, 0x1000)]},
               {'type': 'ZFS plain file', 'level': 0, 'dvas': [(0, 0x3000, 0x2000), (0, 0x9000, 0x2000)]}]
        self.assertEqual(nodeb_check.live_block(bps), (0x3000 + 0x400000, 0x2000))
        self.assertIsNone(nodeb_check.live_block(bps[:2]))


class Expected(unittest.TestCase):
    def test_every_control_must_be_caught(self):
        self.assertEqual(set(nodeb_check.EXPECTED), set(nodeb_check.CONTROLS))
        self.assertEqual(set(nodeb_check.EXPECTED.values()), {'caught'})


if __name__ == '__main__':
    unittest.main()
