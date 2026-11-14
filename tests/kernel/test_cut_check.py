#!/usr/bin/env python3
"""Unit tests for the cut rig's check (cut_check.py): its pure parts. No kernel needed.

    python3 tests/kernel/test_cut_check.py
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cut_check  # noqa: E402

KSTAT = """18 0 0x01 4 448 1234 5678
txg      birth            state ndirty       nread        nwritten     reads    writes   otime        qtime        wtime        stime
3        500              C     0            0            0            0        0        100          10           10           10
4        1000             C     0            0            4096         0        1        100          10           10           10
5        2000             C     0            0            8192         0        2        100          10           10           10
6        3000             S     0            0            4096         0        1        100          10           0            0
"""


def rec(i, flags=(), offset=0, nbytes=0, mark=None):
    r = {'i': i, 'flags': list(flags), 'offset': offset, 'bytes': nbytes}
    if mark is not None:
        r['mark'] = mark
        r['flags'] = ['mark']
    elif nbytes and 'discard' not in flags:
        r['pos'] = 1
    return r


def cut(n, txg, ub_record, prefix=None):
    return {'n': n, 'txg': txg, 'ub_record': ub_record, 'prefix': prefix if prefix is not None else ub_record + 1,
            'rootbp': (n,)}


class Kstat(unittest.TestCase):
    def test_committed_groups_that_wrote_only(self):
        self.assertEqual(cut_check.committed_txgs(KSTAT), [4, 5])


class Sessions(unittest.TestCase):
    def setUp(self):
        self.records = [rec(0, mark='session-1-begin'), rec(1), rec(2), rec(3), rec(4, mark='session-1-end'),
                        rec(5), rec(6, mark='session-2-begin'), rec(7), rec(8), rec(9, mark='session-2-end')]

    def test_marks_make_sessions(self):
        self.assertEqual(cut_check.sessions(self.records), {1: (0, 4), 2: (6, 9)})

    def test_cuts_as_committed_pass(self):
        cs = [cut(1, 4, 1), cut(2, 5, 2), cut(3, 6, 3), cut(4, 6, 5), cut(5, 9, 7)]
        v = cut_check.session_verdicts(cs, self.records, {1: [4, 5], 2: [9]})
        self.assertEqual([ok for _, ok, _ in v], [True, True])

    def test_a_group_counted_twice_fails(self):
        cs = [cut(1, 4, 1), cut(2, 4, 2), cut(3, 5, 3), cut(4, 9, 7)]
        v = cut_check.session_verdicts(cs, self.records, {1: [4, 5], 2: [9]})
        self.assertEqual([ok for _, ok, _ in v], [False, True])
        self.assertIn('first difference at 1', v[0][2])

    def test_a_group_missed_fails(self):
        cs = [cut(1, 4, 1), cut(2, 9, 7)]
        v = cut_check.session_verdicts(cs, self.records, {1: [4, 5], 2: [9]})
        self.assertFalse(v[0][1])

    def test_a_stale_cut_is_left_out(self):
        cs = [cut(1, 4, 1), cut(2, 5, 2), cut(3, 9, 7), cut(4, 10, 8)]
        for c, root in zip(cs, (1, 2, 3, 1)):
            c['rootbp'] = (root,)
        cs[3]['txg'] = 9
        cs[2]['rootbp'] = (5,)
        cs[3]['rootbp'] = (5,)
        cs.insert(3, cut(9, 10, 7))
        cs[3]['rootbp'] = (6,)
        # cuts: 4/r1, 5/r2, 9/r5, 10/r6, 9/r5 (stale: the state of the third)
        for k, c in enumerate(cs):
            c['n'] = k + 1
        v = cut_check.session_verdicts(cs, self.records, {1: [4, 5], 2: [9, 10]})
        self.assertEqual([ok for _, ok, _ in v], [True, True])

    def test_a_cut_before_the_command_returned(self):
        records = [rec(0, mark='session-1-begin'), rec(1), rec(2), rec(3, mark='session-1-ready'), rec(4),
                   rec(5, mark='session-1-end'), rec(6, mark='session-2-begin'), rec(7, mark='session-2-ready')]
        c = cut(1, 4, 1)
        c['flush_record'] = 2
        late = cut(2, 5, 4)
        late['flush_record'] = 4
        v = cut_check.synced_before_ready([c, late], records)
        self.assertEqual([(k, n) for k, n, _ in v], [(1, 1), (2, 0)])
        c['flush_record'] = 3.5
        self.assertEqual(cut_check.synced_before_ready([c], records)[0][1], 0)

    def test_no_kstat_is_the_rigs(self):
        v = cut_check.session_verdicts([], self.records, {1: [4]})
        self.assertIsNone(v[1][1])


class Picks(unittest.TestCase):
    BPS = [{'dvas': [(0, 0x1000, 0x1000), (0, 0x9000, 0x1000)]}]
    OFF = cut_check.replay_check.DVA_OFFSET

    def test_overlap_of_any_copy(self):
        self.assertTrue(cut_check.overlaps(rec(0, (), self.OFF + 0x9800, 512), self.BPS))
        self.assertFalse(cut_check.overlaps(rec(0, (), self.OFF + 0x2000, 0x7000), self.BPS))
        self.assertTrue(cut_check.overlaps(rec(0, (), self.OFF + 0x1fff, 1), self.BPS))

    def test_withheld_is_the_last_write_before_the_uberblock(self):
        records = [rec(0, (), self.OFF + 0x1000, 4096), rec(1, (), self.OFF + 0x9000, 4096), rec(2, ('flush',)),
                   rec(3, (), 0, 4096), rec(4, ('flush',))]
        self.assertEqual(cut_check.pick_withheld(records, cut(1, 5, 3, 4), self.BPS), 1)
        self.assertIsNone(cut_check.pick_withheld(records[2:], cut(1, 5, 1), self.BPS))

    def test_later_is_the_first_write_or_discard_after_the_instant(self):
        records = [rec(0, (), self.OFF + 0x1000, 4096), rec(1, ('flush',)), rec(2, (), self.OFF + 0x5000, 4096),
                   rec(3, ('discard',), self.OFF + 0x9000, 4096), rec(4, (), self.OFF + 0x1000, 4096)]
        self.assertEqual(cut_check.pick_later(records, cut(1, 5, 0, 2), self.BPS), 3)
        self.assertIsNone(cut_check.pick_later(records[:3], cut(1, 5, 0, 2), self.BPS))


class Later(unittest.TestCase):
    def test_the_middle_first_then_earlier(self):
        cs = [cut(n, n, n) for n in range(1, 33)]
        self.assertEqual([c['n'] for c in cut_check.later_candidates(cs, cs[16])], [17, 9, 5, 3, 1])
        self.assertEqual([c['n'] for c in cut_check.later_candidates(cs[:2], cs[1])], [2, 1])


class Dropped(unittest.TestCase):
    """A rule that drops a cut whose state the volume then holds (rewindf: -FX wrote group 43 again)."""

    def setUp(self):
        self.cs = [cut(1, 42, 10), cut(2, 43, 20), cut(3, 43, 30), cut(4, 46, 40)]
        self.cs[2]['rootbp'] = (9,)
        self.op = {c['n']: {'itself': True} for c in self.cs}

    def test_a_dropped_state_is_caught(self):
        rcs = [self.cs[0], self.cs[1], self.cs[3]]
        d = cut_check.dropped_states(self.cs, rcs, self.op)
        self.assertEqual(len(d), 1)
        self.assertIn("cut 3's instant", d[0])
        self.assertIn('another root block pointer', d[0])

    def test_the_same_list_drops_nothing(self):
        self.assertEqual(cut_check.dropped_states(self.cs, list(self.cs), self.op), [])

    def test_a_cut_that_does_not_open_as_itself_is_not_held_against_the_rule(self):
        self.op[3]['itself'] = False
        self.assertEqual(cut_check.dropped_states(self.cs, [self.cs[0], self.cs[1], self.cs[3]], self.op), [])

    def test_a_rule_with_no_cut_yet(self):
        d = cut_check.dropped_states(self.cs[:1], [], self.op)
        self.assertEqual(len(d), 1)
        self.assertIn('none', d[0])


class Expected(unittest.TestCase):
    def test_a_set_takes_either(self):
        self.assertTrue(cut_check.as_expected('same', ('same', 'caught')))
        self.assertFalse(cut_check.as_expected('missed', ('same', 'caught')))
        self.assertTrue(cut_check.as_expected('caught', 'caught'))

    def test_every_control_has_every_recording(self):
        with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'cut-record.sh')) as f:
            line = next(ln for ln in f if ln.startswith('ALL='))
        recs = set(line.split('"')[1].split())
        self.assertEqual(set(cut_check.RECS), recs)
        self.assertEqual(set(cut_check.EXPECTED_OPENS), recs)
        for ctl, table in cut_check.EXPECTED.items():
            self.assertEqual(set(table), recs, ctl)
            for v in table.values():
                self.assertTrue(set(v if isinstance(v, tuple) else (v,)) <= {'caught', 'same', 'not built', 'missed'}, ctl)


if __name__ == '__main__':
    unittest.main()
