#!/usr/bin/env python3
"""Tests of the channel rig's check (publish_check.py): no kernel, no object store."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import publish_check as pc  # noqa: E402
from cut_check import RigError  # noqa: E402

A = 'a' * 32
S = 'b' * 32
H = 'c' * 64
REQ = {'name': 't1', 'group': 50, 'attachment': A, 'state': S, 'after': 7, 'manifest': '/m', 'txg': 50}


def snap_row(commit=H, state=S, number=8, group=50, ref=H, manifest='matches'):
    lines = [f'ref\t{ref}\t3' if ref else 'ref\tnone',
             f'snaprow\t{state}\t{number}\t{group}\t{commit}\t01ARZ3NDEKTSV4RRFFQ69G5FAV\t12\t{"d" * 64}']
    if manifest:
        lines.append(f'manifest\t12\t{"d" * 64}\t{manifest}')
    return pc.read_snap_row('\n'.join(lines) + '\n')


class Tables(unittest.TestCase):
    def test_each_published_name_ends_done_and_each_course_has_a_table(self):
        for course in pc.COURSES:
            self.assertIn(course, pc.EXPECTED)
            self.assertIn(course, pc.PUBLISHED)
            for t, label in pc.PUBLISHED[course].items():
                self.assertEqual(pc.EXPECTED[course][label], ('done ',), (course, t))
        for course, (t, _) in pc.BETWEEN.items():
            self.assertIn(t, pc.PUBLISHED[course])

    def test_no_name_is_both_published_and_absent(self):
        for course, names in pc.ABSENT.items():
            self.assertFalse(set(names) & set(pc.PUBLISHED[course]))


class Ends(unittest.TestCase):
    def test_read(self):
        self.assertEqual(pc.read_ends('t1\tdone commit=x\nnu\trefused no-upload this daemon\n'),
                         {'t1': 'done commit=x', 'nu': 'refused no-upload this daemon'})
        for bad in ('t1\n', '\tdone\n', 't1\ta\nt1\tb\n'):
            with self.assertRaises(RigError, msg=bad):
                pc.read_ends(bad)

    def test_judged_by_the_table_and_a_missing_label_is_the_rigs_error(self):
        ends = {k: v[0] + 'x' for k, v in pc.EXPECTED['pchannel'].items()}
        self.assertTrue(all(ok for _, ok, _, _ in pc.judge_ends('pchannel', ends)))
        ends['peer'] = 'cuts attachment=...'
        self.assertEqual([k for k, ok, _, _ in pc.judge_ends('pchannel', ends) if not ok], ['peer'])
        del ends['peer']
        with self.assertRaises(RigError):
            pc.judge_ends('pchannel', ends)
        ends['peer'], ends['more'] = 'refused peer', 'x'
        with self.assertRaises(RigError):
            pc.judge_ends('pchannel', ends)

    def test_withdrawn_before_a_kill_may_not_end_done(self):
        ends = {k: v[0] for k, v in pc.EXPECTED['pkill'].items()}
        ends['tw-after'] = f'done commit={H}'
        self.assertEqual([k for k, ok, _, _ in pc.judge_ends('pkill', ends) if not ok], ['tw-after'])

    def test_done_commit(self):
        self.assertEqual(pc.done_commit(f'done commit={H}'), H)
        for a in ('done', 'withdrawn', f'done commit={H} x', 'pending after 3 s'):
            self.assertIsNone(pc.done_commit(a))


class Published(unittest.TestCase):
    def test_the_last_request_of_a_name_counts(self):
        got = pc.read_published(f't1\t50\t{A}\t{S}\t7\t/m\t50\nt1\t70\t{A}\t{S}\t9\t/m2\t50\n')
        self.assertEqual((got['t1']['group'], got['t1']['after'], got['t1']['txg']), (70, 9, 50))
        for bad in (f't1\t50\t{A}\t{S}\t7\t/m\n', f't1\tx\t{A}\t{S}\t7\t/m\t50\n', f't1\t50\t{A[1:]}\t{S}\t7\t/m\t50\n'):
            with self.assertRaises(RigError, msg=bad):
                pc.read_published(bad)


class Rows(unittest.TestCase):
    def test_read(self):
        got = snap_row()
        self.assertEqual(got['ref'], (H, '3'))
        self.assertEqual((got['row']['number'], got['row']['group'], got['row']['commit']), (8, 50, H))
        self.assertEqual(got['manifest'], (12, 'd' * 64, True))
        got = pc.read_snap_row('ref\tnone\nsnaprow\tnone\n')
        self.assertEqual((got['ref'], got['row']), (None, None))
        self.assertEqual(pc.read_snap_row('ref\tnone\nsnaprow\tunreadable\t0200\n')['row'], 'unreadable')
        self.assertIsNone(pc.read_snap_row(f'ref\t{H}\t1\nsnaprow\tnone\nmanifest\tnone\n')['manifest'])
        for bad in ('snaprow\tnone\n', 'ref\tnone\n', 'ref\tnone\nsnaprow\tx\t1\n', f'ref\t{H}\nsnaprow\tnone\n',
                    'ref\tnone\nsnaprow\tnone\nmanifest\t1\tx\tmaybe\n'):
            with self.assertRaises(RigError, msg=bad):
                pc.read_snap_row(bad)

    def test_a_row_that_holds(self):
        self.assertEqual(pc.judge_row(snap_row(), REQ, H), [])
        self.assertEqual(pc.judge_row(snap_row(group=900, number=40), REQ, H), [])

    def test_a_cut_below_g_is_never_what_snap_t_names(self):
        bad = pc.judge_row(snap_row(group=49), REQ, H)
        self.assertEqual(len(bad), 1)
        self.assertIn('below G = 50', bad[0])

    def test_each_way_it_can_be_wrong(self):
        o = 'e' * 64
        cases = {
            'no ref': snap_row(ref=None),
            'the ref is on': snap_row(ref=o, commit=o),
            'the row names commit': snap_row(commit=o),
            'cut state': snap_row(state='f' * 32),
            'not counted after C': snap_row(number=7),
        }
        for what, got in cases.items():
            bad = pc.judge_row(got, REQ, H)
            self.assertTrue(any(what in b for b in bad), (what, bad))
        self.assertEqual(pc.judge_row({'ref': (H, 1), 'row': None}, REQ, H), ['no row skyzfs/snap/T'])
        self.assertEqual(pc.judge_row({'ref': (H, 1), 'row': 'unreadable'}, REQ, H),
                         ['a row that is not of version 1'])

    def test_the_manifest(self):
        got = snap_row()
        got['fetched'] = b'x' * 12
        self.assertEqual(pc.judge_manifest(got, b'x' * 12), [])
        self.assertEqual(len(pc.judge_manifest(got, b'y' * 12)), 1)
        self.assertEqual(len(pc.judge_manifest(snap_row(manifest='differs'), b'')), 1)
        self.assertEqual(pc.judge_manifest(snap_row(manifest=None), b''), ['node B did not fetch the manifest'])
        self.assertEqual(pc.judge_manifest({'manifest': None}, b''), ['no manifest object where the row says'])


def log_line(event, sector, sectors, result, k):
    return {'event': event, 'sector': sector, 'sectors': sectors, 'result': result, 'line': k}


class Cuts(unittest.TestCase):
    def test_cut_lines_and_the_row_s_cut(self):
        inst = [{'lines': [log_line('applied', '0', '8', '0', 1), log_line('cut', 'itself', '8', '50', 2),
                           log_line('cut', 'rewind', '9', '51', 3)]},
                {'lines': [log_line('cut', 'itself', '10', '60', 4)]}]
        cuts = pc.cut_lines(inst)
        self.assertEqual(cuts, [(8, 50, True), (9, 51, False), (10, 60, True)])
        self.assertEqual(pc.judge_cut(cuts, {'number': 8, 'group': 50}), [])
        self.assertIn('only with a rewind', pc.judge_cut(cuts, {'number': 9, 'group': 51})[0])
        self.assertIn('is of group 50', pc.judge_cut(cuts, {'number': 8, 'group': 52})[0])
        self.assertIn('no cut line', pc.judge_cut(cuts, {'number': 11, 'group': 60})[0])
        self.assertEqual(pc.counted_between(cuts, 7, 50, 60), [(8, 50, True), (9, 51, False)])
        self.assertEqual(pc.counted_between(cuts, 8, 50, 60), [(9, 51, False)])
        with self.assertRaises(RigError):
            pc.cut_lines([{'lines': [log_line('cut', 'maybe', '8', '50', 1)]}])


if __name__ == '__main__':
    unittest.main()
