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

    def test_outside_the_kstat_groups_must_increase(self):
        # session 1: cuts at 4 (the create's first group, below the kstat),
        # 5, and 6 after the end mark (the export's); session 2: 9
        records = [rec(0, mark='session-1-begin'), rec(1), rec(2), rec(3), rec(4, mark='session-1-end'),
                   rec(5), rec(6, mark='session-2-begin'), rec(7), rec(8), rec(9, mark='session-2-end')]
        cs = [cut(1, 4, 1), cut(2, 5, 2), cut(3, 6, 5), cut(4, 9, 7)]
        for k, c in enumerate(cs):
            c['rootbp'] = (k,)
        covered, bad = cut_check.outside_kstat(cs, records, {1: [5], 2: [9]})
        self.assertEqual((covered, bad), (2, []))
        # the export's group counted twice (another cut of group 6) is caught
        dup = cs[:3] + [dict(cut(9, 6, 5), rootbp=(9,))] + cs[3:]
        dup[3]['ub_record'] = 5.5
        covered, bad = cut_check.outside_kstat(dup, records, {1: [5], 2: [9]})
        self.assertEqual(len(bad), 1)
        self.assertIn('(group 6) after cut 3 (group 6)', bad[0])
        # a new session may begin lower (a rewind import)
        cs2 = cs[:3] + [cut(4, 3, 7)]
        self.assertEqual(cut_check.outside_kstat(cs2, records, {1: [5], 2: [3]})[1], [])

    def test_no_kstat_is_the_rigs(self):
        v = cut_check.session_verdicts([], self.records, {1: [4]})
        self.assertIsNone(v[1][1])


class Loaded(unittest.TestCase):
    # Verbatim but for the thread's address, from rewindf's dbgmsg for
    # -FX in run 43647779663: the tryimport's load (its notes name the
    # pool too, not $import), then the import's: a load at 43 that
    # failed, and the rewind's at 34 (which failed too, further on).
    TEXT = """1794695322 ff36 spa_misc.c:431:spa_load_note(): spa_load(skzcut, config trusted): LOADING
1794695322 ff36 spa_misc.c:431:spa_load_note(): spa_load(skzcut, config untrusted): using uberblock with txg=43
1794695322 ff36 spa_misc.c:417:spa_load_failed(): spa_load(skzcut, config untrusted): FAILED: unable to open rootbp in dsl_pool_init [error=5]
1794695322 ff36 spa_misc.c:431:spa_load_note(): spa_load(skzcut, config untrusted): UNLOADING
1794695322 ff36 spa.c:7398:spa_import(): spa_import: importing skzcut, max_txg=-1 (RECOVERY MODE)
1794695322 ff36 spa_misc.c:431:spa_load_note(): spa_load(skzcut, config trusted): LOADING
1794695322 ff36 spa_misc.c:431:spa_load_note(): spa_load(skzcut, config untrusted): using uberblock with txg=43
1794695322 ff36 spa_misc.c:417:spa_load_failed(): spa_load(skzcut, config untrusted): FAILED: unable to open rootbp in dsl_pool_init [error=5]
1794695322 ff36 spa_misc.c:431:spa_load_note(): spa_load(skzcut, config untrusted): UNLOADING
1794695322 ff36 spa_misc.c:431:spa_load_note(): spa_load(skzcut, config untrusted): spa_load_retry: rewind, max txg: 42
1794695322 ff36 spa_misc.c:431:spa_load_note(): spa_load(skzcut, config untrusted): LOADING
1794695322 ff36 spa_misc.c:431:spa_load_note(): spa_load(skzcut, config untrusted): using uberblock with txg=34
"""

    def test_the_last_load_of_the_pool_counts(self):
        self.assertEqual(cut_check.loaded_txg(self.TEXT), 34)
        # cut before the rewind: the import's own load at 43 is the last
        self.assertEqual(cut_check.loaded_txg(self.TEXT.split('spa_load_retry')[0]), 43)
        self.assertIsNone(cut_check.loaded_txg('nothing here'))
        # another pool's load is not this one's
        self.assertIsNone(cut_check.loaded_txg(self.TEXT.replace('skzcut', 'other')))


class Root(unittest.TestCase):
    # zdb -uuuu after rewindf's export, verbatim, in run 43651302466:
    # 2.4.1 (ubuntu-26.04) prints `bp =` and `rootbp =`, 2.2.2
    # (ubuntu-24.04) `rootbp =` alone; and the uberblock's words as
    # cut_ref read them from the record (uberblocks.tsv).
    ZDB24 = """Uberblock:
\tmagic = 0000000000bab10c
\ttxg = 43
\ttimestamp = 1794700370 UTC = Sat Nov 14 23:52:50 2026
\tbp = DVA[0]=<0:6c64000:1000> DVA[1]=<0:215c000:1000> DVA[2]=<0:8041000:1000> [L0 DMU objset] fletcher4 uncompressed unencrypted LE contiguous unique triple size=1000L/1000P birth=43L/43P fill=68 cksum=00000002d6e930ab:00000ae9827480aa:0014fd6519cb3dd8:1af1e08127438f85
\tmmp_magic = 00000000a11cea11
\trootbp = DVA[0]=<0:6c64000:1000> DVA[1]=<0:215c000:1000> DVA[2]=<0:8041000:1000> [L0 DMU objset] fletcher4 uncompressed unencrypted LE contiguous unique triple size=1000L/1000P birth=43L/43P fill=68 cksum=00000002d6e930ab:00000ae9827480aa:0014fd6519cb3dd8:1af1e08127438f85
\tcheckpoint_txg = 0
"""
    WORDS24 = ('0000000000000008000000000003632000000000000000080000000000010ae000000000000000080000000000040208800b0702000700'
               '07000000000000000000000000000000000000000000000000000000000000002b000000000000004400000002d6e930ab00000ae98274'
               '80aa0014fd6519cb3dd81af1e08127438f85')
    ZDB22 = """Uberblock:
\tmagic = 0000000000bab10c
\ttxg = 43
\ttimestamp = 1794700449 UTC = Sat Nov 14 23:54:09 2026
\tmmp_magic = 00000000a11cea11
\trootbp = DVA[0]=<0:6c45000:1000> DVA[1]=<0:214d000:1000> DVA[2]=<0:8045000:1000> [L0 DMU objset] fletcher4 uncompressed unencrypted LE contiguous unique triple size=1000L/1000P birth=43L/43P fill=65 cksum=00000002e2f1669e:00000b190a37c925:00155b499534e141:1b6d8d3db839e4cb
\tcheckpoint_txg = 0
"""
    WORDS22 = ('0000000000000008000000000003622800000000000000080000000000010a6800000000000000080000000000040228800b0702000700'
               '07000000000000000000000000000000000000000000000000000000000000002b000000000000004100000002e2f1669e00000b190a37'
               'c92500155b499534e1411b6d8d3db839e4cb')

    @staticmethod
    def words(h):
        return tuple(int(h[k:k + 16], 16) for k in range(0, 256, 16))

    def test_zdb_and_the_record_name_the_same_root(self):
        for out, h in ((self.ZDB24, self.WORDS24), (self.ZDB22, self.WORDS22)):
            self.assertIsNotNone(cut_check.zdb_root(out))
            self.assertEqual(cut_check.zdb_root(out), cut_check.root_of(self.words(h)))

    def test_another_root_of_the_group_differs(self):
        self.assertNotEqual(cut_check.zdb_root(self.ZDB24), cut_check.root_of(self.words(self.WORDS22)))
        w = list(self.words(self.WORDS24))
        w[15] ^= 1
        self.assertNotEqual(cut_check.zdb_root(self.ZDB24), cut_check.root_of(tuple(w)))
        w = list(self.words(self.WORDS24))
        w[1] += 8
        self.assertNotEqual(cut_check.zdb_root(self.ZDB24), cut_check.root_of(tuple(w)))

    def test_no_root_line(self):
        self.assertIsNone(cut_check.zdb_root(self.ZDB22.replace('rootbp', 'other')))
        self.assertIsNone(cut_check.root_of(None))

    def test_judge_refuses_a_tie_with_another_root(self):
        """The pool's group and timestamp, but zdb's root is not the
        rings' best: the judge says so and checks nothing."""
        ck = cut_check.Check.__new__(cut_check.Check)
        ck.group = lambda txg=None: ((43, 1794700370, cut_check.zdb_root(self.ZDB24)), '')
        cut = {'txg': 43, 'timestamp': 1794700370, 'rootbp': self.words(self.WORDS22)}
        pred = dict(cut, itself=True)
        failed, detail, _, seen = ck.judge(cut, pred, kernel=False)
        self.assertEqual(failed, 'opens')
        self.assertIn('another root block pointer', detail)
        self.assertFalse(seen['whole'])
        # not itself: zdb -t at the cut's group must find the cut's root
        pred = {'txg': 43, 'timestamp': 1794700370, 'rootbp': self.words(self.WORDS24), 'itself': False}
        failed, detail, _, _ = ck.judge(cut, pred, kernel=False)
        self.assertEqual(failed, 'group')


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


class RuleSame(unittest.TestCase):
    def test_equal_lists_and_lists_but_for_stale_cuts(self):
        cs = [cut(1, 11, 10), cut(2, 12, 20), cut(3, 11, 30), cut(4, 13, 40)]
        self.assertIn('as the design rule', cut_check.rule_same(cs, list(cs), {}))
        dropped = [cs[0], cs[1], cs[3]]
        self.assertIsNone(cut_check.rule_same(cs, dropped, {}))
        self.assertIn('but for its 1 stale cuts', cut_check.rule_same(cs, dropped, {3: 1}))
        # a fresh cut dropped is not the same, stale cuts or not
        self.assertIsNone(cut_check.rule_same(cs, [cs[0], cs[3]], {3: 1}))
        # the same groups at other instants, or with other roots, are not the same list
        moved = [dict(c, prefix=c['prefix'] + 1) for c in cs]
        self.assertIsNone(cut_check.rule_same(cs, moved, {}))
        other = [dict(c, rootbp=(99,)) for c in cs]
        self.assertIsNone(cut_check.rule_same(cs, other, {}))


class Expected(unittest.TestCase):
    def test_a_set_takes_either(self):
        self.assertTrue(cut_check.as_expected('same', ('same', 'caught')))
        self.assertFalse(cut_check.as_expected('missed', ('same', 'caught')))
        self.assertTrue(cut_check.as_expected('caught', 'caught'))

    def test_stale_cuts_are_expected_of_multihost_alone(self):
        self.assertEqual(set(cut_check.EXPECTED_STALE), set(cut_check.RECS))
        self.assertEqual({r for r, v in cut_check.EXPECTED_STALE.items() if v != 'none'}, {'multihost'})

    def test_every_control_has_every_recording(self):
        with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'cut-record.sh')) as f:
            line = next(ln for ln in f if ln.startswith('ALL='))
        recs = set(line.split('"')[1].split())
        self.assertEqual(set(cut_check.RECS), recs)
        self.assertEqual(set(cut_check.EXPECTED_OPENS), recs)
        for ctl, table in cut_check.EXPECTED.items():
            self.assertEqual(set(table), recs, ctl)
            for v in table.values():
                # no control may be expected to be missed: that would be a
                # check that does not catch what it is there for
                self.assertTrue(set(v if isinstance(v, tuple) else (v,)) <= {'caught', 'same', 'not built'}, ctl)


if __name__ == '__main__':
    unittest.main()
