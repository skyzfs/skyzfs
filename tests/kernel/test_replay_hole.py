#!/usr/bin/env python3
"""Tests of replay_hole.py's planning and judging (M1.T27): no kernel needed.

Run: python3 tests/kernel/test_replay_hole.py
"""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import logwrites  # noqa: E402
import replay_hole as rh  # noqa: E402

SS = 512


class Content(unittest.TestCase):
    def test_a_block_is_its_write_zeros_or_other(self):
        self.assertEqual(len(rh.content(7)), rh.BLOCK)
        self.assertEqual(rh.what(rh.content(7), 7), 'written')
        self.assertEqual(rh.what(bytes(rh.BLOCK), 7), 'zeros')
        self.assertEqual(rh.what(rh.content(8), 7), 'other')
        # Torn at a sector: each sector zeros or the write's (design §4.1).
        half = rh.content(7)[:2048] + bytes(2048)
        self.assertEqual(rh.what(half, 7), 'torn')
        odd = bytes(512) + rh.content(7)[512:1024] + bytes(3072)
        self.assertEqual(rh.what(odd, 7), 'torn')
        # A sector of anything else is other, wherever it is.
        self.assertEqual(rh.what(half[:1024] + b'x' * 512 + half[1536:], 7), 'other')
        self.assertEqual(rh.what(rh.content(7)[:2047] + bytes(2049), 7), 'other')
        self.assertEqual(rh.what(rh.content(8)[:512] + bytes(3584), 7), 'other')

    def test_offsets_are_spread_distinct_and_in_their_own_mib(self):
        offs = rh.offsets(16, 256 << 20)
        self.assertEqual(len(set(offs)), 16)
        self.assertEqual(len({o >> 20 for o in offs}), 16)
        self.assertTrue(all(o % rh.BLOCK == 0 and o < 256 << 20 for o in offs))
        self.assertNotEqual(offs, sorted(offs))
        with self.assertRaises(ValueError):
            rh.offsets(16, 16 << 20)

    def test_the_course_flushes_every_few_writes_and_leaves_a_tail(self):
        steps = rh.course_plan(8, 3, 2)
        self.assertEqual(steps, [('w', 0), ('w', 1), ('w', 2), ('f', 0), ('w', 3), ('w', 4), ('w', 5), ('f', 1),
                                 ('w', 6), ('w', 7)])
        self.assertEqual(rh.course_plan(4, 3, 0)[-1], ('f', 1))
        with self.assertRaises(ValueError):
            rh.course_plan(4, 3, 4)


def records_of(marks, total=None):
    """Records with these marks at these prefixes (1-based), plain writes elsewhere."""
    total = total or max(marks) + 2
    recs = []
    for n in range(1, total + 1):
        if n in marks:
            recs.append({'i': n - 1, 'flags': ['mark'], 'mark': marks[n], 'bytes': 0, 'offset': 0})
        else:
            recs.append({'i': n - 1, 'flags': [], 'bytes': SS, 'offset': 0})
    return recs


class Judging(unittest.TestCase):
    def setUp(self):
        # start at 2; w-0 at 4, w-1 at 6, f-0 at 9; w-2 at 11 with no flush; end at 12
        self.recs = records_of({2: 'hole-start', 4: 'w-0', 6: 'w-1', 9: 'f-0', 11: 'w-2', 12: 'hole-end'}, 14)
        self.start, self.end, self.w, self.f = rh.marks_of(self.recs)

    def test_marks(self):
        self.assertEqual((self.start, self.end, self.w, self.f), (2, 12, {0: 4, 1: 6, 2: 11}, [(9, 0)]))

    def test_states(self):
        self.assertEqual(rh.state_at(3, 4, self.f), 'pending')
        self.assertEqual(rh.state_at(4, 4, self.f), 'returned')
        self.assertEqual(rh.state_at(8, 4, self.f), 'returned')
        self.assertEqual(rh.state_at(9, 4, self.f), 'flushed')
        self.assertEqual(rh.state_at(13, 11, self.f), 'returned')

    def test_zeros_after_return_is_counted_and_allowed(self):
        j = rh.judge(8, {0: 'zeros', 1: 'written', 2: 'zeros'}, self.w, self.f)
        self.assertEqual((j['violations'], j['zeros_after_return']), ([], [0]))

    def test_a_flushed_block_that_reads_zeros_is_a_violation(self):
        j = rh.judge(9, {0: 'zeros', 1: 'written', 2: 'zeros'}, self.w, self.f)
        self.assertEqual(len(j['violations']), 1)
        self.assertIn('block 0: a flush after its write had returned', j['violations'][0])
        self.assertEqual(j['zeros_after_return'], [])

    def test_other_content_is_a_violation_in_any_state(self):
        for n in (3, 8, 13):
            j = rh.judge(n, {0: 'other', 1: 'written', 2: 'zeros'}, self.w, self.f)
            self.assertEqual(len(j['violations']), 1, n)

    def test_zeros_with_the_data_on_the_disk_are_counted_apart(self):
        # Block 0 returned at 4; its data is in the log from prefix 7.
        j = rh.judge(6, {0: 'zeros', 1: 'zeros', 2: 'zeros'}, self.w, self.f, {0: 7})
        self.assertEqual((j['zeros_after_return'], j['zeros_on_disk']), ([0, 1], []))
        j = rh.judge(8, {0: 'zeros', 1: 'zeros', 2: 'zeros'}, self.w, self.f, {0: 7})
        self.assertEqual((j['zeros_after_return'], j['zeros_on_disk']), ([0, 1], [0]))

    def test_the_data_on_the_disk_at_the_prefix_itself_counts(self):
        j = rh.judge(7, {0: 'zeros', 1: 'zeros', 2: 'zeros'}, self.w, self.f, {0: 7})
        self.assertEqual(j['zeros_on_disk'], [0])

    def test_a_torn_block_is_allowed_until_a_flush_covers_it(self):
        for n in (3, 8):
            j = rh.judge(n, {0: 'torn', 1: 'written', 2: 'zeros'}, self.w, self.f)
            self.assertEqual((j['violations'], j['torn'], j['zeros_after_return']), ([], [0], []), n)
        j = rh.judge(9, {0: 'torn', 1: 'written', 2: 'zeros'}, self.w, self.f)
        self.assertEqual(len(j['violations']), 1)
        self.assertIn('block 0: a flush after its write had returned, and it reads torn', j['violations'][0])

    def test_a_pending_block_may_read_its_write(self):
        j = rh.judge(3, {0: 'written', 1: 'zeros', 2: 'zeros'}, self.w, self.f)
        self.assertEqual((j['violations'], j['zeros_after_return']), ([], []))

    def test_the_plan(self):
        self.assertEqual(rh.plan(self.start, self.end, len(self.recs), self.w, self.f, True, 0),
                         list(range(2, 15)))
        p = rh.plan(self.start, self.end, len(self.recs), self.w, self.f, False, 0)
        for n in (2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 14):
            self.assertIn(n, p)
        self.assertEqual(p, sorted(p))
        self.assertEqual(len(rh.plan(self.start, self.end, len(self.recs), self.w, self.f, False, 100)), 13)


class Withheld(unittest.TestCase):
    def test_the_control_moves_the_blocks_data_after_the_flush_mark(self):
        with tempfile.TemporaryDirectory() as d:
            log = os.path.join(d, 'log')
            other = b'x' * rh.BLOCK
            ents = [('mark', 'hole-start'),
                    ('write', 0, other),                    # file system metadata
                    ('write', 1 << 20, rh.content(0)),      # block 0's data
                    ('mark', 'w-0'),
                    ('write', 2 << 20, rh.content(1)),      # block 1's data
                    ('mark', 'w-1'),
                    ('write', 0, other), ('flush',),
                    ('mark', 'f-0'),
                    ('mark', 'hole-end')]
            logwrites.write_log(log, SS, ents)
            recs = logwrites.index(log, 0)['records']
            start, end, w, f = rh.marks_of(recs)
            c = rh.withheld(recs, log, w, f)
            self.assertIsNotNone(c)
            self.assertEqual((c['block'], c['flush'], c['victims']), (0, 0, [2]))
            self.assertEqual(c['mark_prefix'], 9)
            # The schedule holds every record once, and block 0's data comes
            # right after the mark of the flush.
            self.assertEqual(sorted(c['schedule']), list(range(len(recs))))
            self.assertEqual(c['schedule'][c['point'] - 1], 8)
            self.assertEqual(c['schedule'][c['point']], 2)

    def test_the_data_of_each_block_is_found_in_the_log(self):
        with tempfile.TemporaryDirectory() as d:
            log = os.path.join(d, 'log')
            logwrites.write_log(log, SS, [('mark', 'hole-start'), ('mark', 'w-0'), ('mark', 'w-1'),
                                          ('write', 2 << 20, rh.content(1)), ('write', 1 << 20, rh.content(0)),
                                          ('flush',), ('write', 1 << 20, rh.content(0))])
            recs = logwrites.index(log, 0)['records']
            self.assertEqual(rh.data_prefixes(recs, log, 3), {1: 4, 0: 5})

    def test_a_block_whose_data_is_not_in_the_log_is_the_rigs_error(self):
        self.assertIsNone(rh.data_missing({0: 3, 1: 5, 2: 9}, 3))
        self.assertIn('1 of 3 blocks', rh.data_missing({0: 3, 2: 9}, 3))

    def test_no_control_without_a_flush(self):
        with tempfile.TemporaryDirectory() as d:
            log = os.path.join(d, 'log')
            logwrites.write_log(log, SS, [('mark', 'hole-start'), ('write', 1 << 20, rh.content(0)),
                                          ('mark', 'w-0'), ('mark', 'hole-end')])
            recs = logwrites.index(log, 0)['records']
            _, _, w, f = rh.marks_of(recs)
            self.assertIsNone(rh.withheld(recs, log, w, f))


def result(**kw):
    r = {'fs': 'ext4', 'kind': 'sparse', 'checked': 10, 'zero_prefixes': 3, 'zero_blocks': 4,
         'disk_zero_prefixes': 2, 'disk_zero_blocks': 2, 'violations': [],
         'failures': [], 'rig_error': None, 'control': {'result': 'caught', 'what': 'w', 'got': 'g'},
         'range': [2, 20], 'seconds': 5}
    r.update(kw)
    return r


class Verdict(unittest.TestCase):
    def test_exits(self):
        self.assertEqual(rh.verdict([result()])[0], 0)
        self.assertEqual(rh.verdict([result(violations=['x'])])[0], 1)
        self.assertEqual(rh.verdict([result(failures=['prefix 3: start: exit 1'])])[0], 1)
        self.assertEqual(rh.verdict([result(rig_error='losetup')])[0], 2)
        self.assertEqual(rh.verdict([result(control={'result': 'not caught', 'what': 'w'})])[0], 2)
        self.assertEqual(rh.verdict([result(control={'result': 'not built', 'what': 'w'})])[0], 2)

    def test_on_a_written_through_image_the_third_count_is_pinned_at_zero(self):
        self.assertEqual(rh.ON_DISK, {'sparse': 'observed', 'fallocate': 'observed', 'zero-fill': 0})
        ok = result(kind='zero-fill', disk_zero_prefixes=0, disk_zero_blocks=0)
        self.assertEqual(rh.verdict([result(), ok])[0], 0)
        code, head = rh.verdict([result(), result(kind='zero-fill', disk_zero_prefixes=1, disk_zero_blocks=1)])
        self.assertEqual(code, 1)
        self.assertIn('written through at create', head)
        self.assertIn('ext4: 1 prefixes', head)
        # On the other kinds it is the measurement, whatever it is.
        self.assertEqual(rh.verdict([result(kind='fallocate', disk_zero_prefixes=40)])[0], 0)
        # A kind the table does not name is the rig's error.
        self.assertEqual(rh.verdict([result(kind='preallocated')])[0], 2)

    def test_a_daemon_that_failed_before_the_check_is_a_verdict(self):
        # As hole-rig.sh's course_json writes it: nothing checked, no control.
        failed = {'fs': 'ext4', 'kind': 'zero-fill', 'checked': 0, 'zero_prefixes': 0, 'zero_blocks': 0,
                  'violations': [], 'failures': ['skyzfsd serve did not get ready'], 'rig_error': None,
                  'control': None}
        code, head = rh.verdict([result(), failed])
        self.assertEqual(code, 1)
        self.assertIn('A verdict against the daemon', head)
        # A course that was checked still needs its control, failures or not.
        checked = dict(failed, checked=5, failures=['prefix 7: start: exit 1'])
        code, head = rh.verdict([result(), checked])
        self.assertEqual(code, 2)
        self.assertIn('without its negative control', head)

    def test_a_course_without_its_control_is_the_rigs_error(self):
        code, head = rh.verdict([result(), result(kind='fallocate', control=None)])
        self.assertEqual(code, 2)
        self.assertIn('without its negative control', head)

    def test_no_course_checked_is_the_rigs_error(self):
        self.assertEqual(rh.verdict([])[0], 2)
        self.assertEqual(rh.verdict([{'fs': 'xfs', 'kind': 'zero-fill', 'skipped': 'no --zero-fill'}])[0], 2)

    def test_the_summary_names_the_daemon_and_torn_reads(self):
        _, text = rh.summary([result(torn_blocks=2)], daemon='0123abc')
        self.assertIn('The daemon: 0123abc.', text)
        self.assertIn('ext4 sparse: 2 block reads torn at a sector', text)

    def test_the_summary_gives_the_three_counts_per_kind(self):
        code, text = rh.summary([result(), result(kind='zero-fill', zero_prefixes=0, zero_blocks=0,
                                                  disk_zero_prefixes=0, disk_zero_blocks=0),
                                 {'fs': 'xfs', 'kind': 'zero-fill', 'skipped': 'this daemon has no --zero-fill'}])
        self.assertEqual(code, 0)
        self.assertIn('| ext4 | sparse | 10 of 19 | 3 (4 block reads) | 2 (2 block reads) | 0 |', text)
        self.assertIn('| ext4 | zero-fill | 10 of 19 | 0 (0 block reads) | 0 (0 block reads; must be 0) | 0 |', text)
        self.assertIn('not run: this daemon has no --zero-fill', text)


if __name__ == '__main__':
    unittest.main()
