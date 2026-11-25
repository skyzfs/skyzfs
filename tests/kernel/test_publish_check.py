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
    def test_the_tables_are_exactly_these(self):
        """A change to what a course allows is a change to this copy too,
        on purpose (B4 of #139's review). Each entry says why."""
        self.assertEqual(pc.EXPECTED, {
            'pplain': {
                't1': ('done ',),
                't1-again': ('accepted',),            # the same request twice is one publish
                't1-again-end': ('done ',),
                't1-other': ('accepted',),            # answered before any lookup in object storage
                't1-other-end': ('refused taken',),   # snap/t1 names another manifest
                't2': ('done ',),
                'att': ('refused attachment',),
                'state': ('refused state',),
                't4': ('done ',),
                't5': ('expired',),                   # G never reached in its limit
                't7': ('pending after',),
                't7-other': ('refused asked',),
                't7-withdraw': ('withdrawn',),
                't7-again': ('accepted',),
                't7-end': ('done ',),
            },
            'prewind': {'r1': ('done ',)},
            'pkill': {
                'tw': ('pending after',),
                'tw-withdraw': ('withdrawn',),
                'tk': ('pending after',),
                'tw-after': ('withdrawn', 'unknown'),  # the new instance may or may not know tw: never done
                'tk-after': ('unknown',),             # a pending publish lives in memory only
                'tk-again': ('accepted',),
                'tk-end': ('done ',),
                'tw-last': ('withdrawn', 'unknown'),
            },
            'pkillman': {
                'm1': ('none ', 'unknown'),           # killed under the agent: no answer, or the next one's
                'm1-after': ('unknown',),
                'm1-again': ('accepted',),
                'm1-end': ('done ',),
            },
            'pkillpub': {
                'p1': ('none ', 'unknown'),
                'p1-after': ('unknown',),
                'p1-again': ('accepted',),
                'p1-end': ('done ',),                 # and on the commit that landed (pin_p1_found)
            },
            'pchannel': {
                'nu': ('refused no-upload',),
                'version': ('refused version',),
                'malformed': ('refused malformed',),
                'name': ('refused name',),
                'peer': ('refused peer',),
                'cuts': ('cuts ',),
            },
        })
        self.assertEqual(pc.PUBLISHED, {
            'pplain': {'t1': 't1', 't2': 't2', 't4': 't4', 't7': 't7-end'},
            'prewind': {'r1': 'r1'},
            'pkill': {'tk': 'tk-end'},
            'pkillman': {'m1': 'm1-end'},
            'pkillpub': {'p1': 'p1-end'},
            'pchannel': {},
        })
        # t5 expired, t3 never asked validly, tw withdrawn before the kill.
        self.assertEqual(pc.ABSENT, {'pplain': ('t5', 't3'), 'pkill': ('tw',)})
        # manifest:1 kills before the transaction (no snap/m1 may show),
        # publish:1 after it (snap/p1 must).
        self.assertEqual(pc.BETWEEN, {'pkillman': ('m1', False), 'pkillpub': ('p1', True)})
        # Exact: the peer's request is never read, so not answered.
        self.assertEqual(pc.COUNTS, {'pchannel': {'control_answered': 4, 'control_peer': 1, 'control_malformed': 2}})
        self.assertEqual(pc.TW_STATUS, {'publish_withdrawn': 1, 'publish_pending': 0, 'publish_published': 0})
        self.assertEqual(set(pc.KILLED_BY), set(pc.BETWEEN))

    def test_killed_by_is_in_the_hook_s_own_words(self):
        root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        with open(os.path.join(root, 'bins', 'skyzfsd', 'src', 'kill.rs')) as f:
            src = f.read()
        self.assertIn('"after the framed manifest PUT {n} landed ({key}), before its ref was made"', src)
        self.assertIn('"after the transaction of publish {} ({}) landed, before the uploader knew it"', src)
        self.assertEqual(pc.KILLED_BY['pkillman'],
                         'after the framed manifest PUT {n} landed'.format(n=1))
        self.assertEqual(pc.KILLED_BY['pkillpub'], 'after the transaction of publish {} ({}) landed'.format(1, 'p1'))

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

    def test_ctl_commits_commit_differs_in_the_short_id_the_line_shows(self):
        for c in (H, '0' + H[1:], 'a' * 63 + 'b'):
            o = pc.other_commit(c)
            self.assertEqual(len(o), len(c))
            self.assertNotEqual(o[:12], c[:12])
        o = pc.other_commit(H)
        self.assertIn(f'the ref is on {H[:12]}, done named {o[:12]}', pc.judge_row(snap_row(), REQ, o))

    def test_the_manifest(self):
        got = snap_row()
        got['fetched'] = b'x' * 12
        self.assertEqual(pc.judge_manifest(got, b'x' * 12), [])
        self.assertEqual(len(pc.judge_manifest(got, b'y' * 12)), 1)
        self.assertEqual(len(pc.judge_manifest(snap_row(manifest='differs'), b'')), 1)
        self.assertEqual(pc.judge_manifest(snap_row(manifest=None), b''), ['node B did not fetch the manifest'])
        self.assertEqual(pc.judge_manifest({'manifest': None}, b''), ['no manifest object where the row says'])


def log_line(event, sector, sectors, result, k, op='-'):
    return {'event': event, 'sector': sector, 'sectors': sectors, 'result': result, 'line': k, 'op': op}


class Cuts(unittest.TestCase):
    def test_cut_lines_and_the_row_s_cut(self):
        inst = [{'pid': '11', 'lines': [log_line('applied', '0', '8', '0', 1), log_line('cut', 'itself', '8', '50', 2),
                                        log_line('cut', 'rewind', '9', '51', 3)]},
                {'pid': '12', 'lines': [log_line('cut', 'itself', '10', '60', 4)]}]
        cuts = pc.cut_lines(inst)
        self.assertEqual(cuts, [(8, 50, True, '11', 2), (9, 51, False, '11', 3), (10, 60, True, '12', 4)])
        self.assertEqual(pc.judge_cut(cuts, {'number': 8, 'group': 50}), [])
        self.assertIn('only with a rewind', pc.judge_cut(cuts, {'number': 9, 'group': 51})[0])
        self.assertIn('is of group 50', pc.judge_cut(cuts, {'number': 8, 'group': 52})[0])
        self.assertIn('no cut line', pc.judge_cut(cuts, {'number': 11, 'group': 60})[0])
        self.assertEqual(pc.counted_between(cuts, 7, 50, 60), [(8, 50, True, '11', 2), (9, 51, False, '11', 3)])
        self.assertEqual(pc.counted_between(cuts, 8, 50, 60), [(9, 51, False, '11', 3)])
        with self.assertRaises(RigError):
            pc.cut_lines([{'pid': '1', 'lines': [log_line('cut', 'maybe', '8', '50', 1)]}])


R1 = dict(REQ, name='r1', after=7, group=50)


class Premises(unittest.TestCase):
    """The premises that make prewind and pkill test what they are for."""
    CUTS = [(8, 50, False, '1', 5), (9, 51, False, '1', 6), (10, 52, True, '1', 7), (11, 49, False, '1', 8)]

    def test_a_committed_rewind_cut_past_c_and_g_then_a_later_one_made_r1(self):
        self.assertEqual(pc.rewind_committed(self.CUTS, (S, 9, 51), R1, 10), [])

    def test_each_way_the_rewind_premise_fails(self):
        cases = [
            (None, 10, "named no cut"),
            (('e' * 32, 9, 51), 10, 'not the one asked'),
            ((S, 7, 50), 10, 'not counted after C = 7'),
            ((S, 11, 49), 12, 'below G = 50'),
            ((S, 10, 52), 11, 'opens as itself'),
            ((S, 12, 53), 13, 'no cut line of number 12'),
            ((S, 9, 51), 8, 'before cut 9 was committed'),
        ]
        for cut, named, why in cases:
            bad = pc.rewind_committed(self.CUTS, cut, R1, named)
            self.assertTrue(any(why in b for b in bad), (cut, named, bad))

    def test_r1_made_by_the_rewind_cut_itself_is_left_to_the_verdict(self):
        self.assertEqual(pc.rewind_committed(self.CUTS, (S, 9, 51), R1, 9), [])
        self.assertIn('opens only with a rewind', pc.judge_cut(self.CUTS, {'number': 9, 'group': 51})[0])

    def test_a_cut_number_seen_twice_counts_as_itself_if_either_line_says_so(self):
        cuts = self.CUTS + [(9, 51, True, '2', 9)]
        self.assertTrue(any('opens as itself' in b for b in pc.rewind_committed(cuts, (S, 9, 51), R1, 10)))

    def withdraw_log(self):
        return [
            {'pid': '21', 'lines': [log_line('cut', 'itself', '8', '50', 1),
                                    log_line('publish', '-', '-', 'tw withdrawn', 2, op='withdraw'),
                                    log_line('cut', 'itself', '9', '49', 3),
                                    log_line('cut', 'itself', '10', '50', 4),
                                    log_line('cut', 'rewind', '11', '51', 5),
                                    log_line('cut', 'itself', '12', '52', 6)]},
            {'pid': '22', 'lines': [log_line('cut', 'itself', '13', '60', 7)]},
        ]

    def test_cuts_after_the_withdrawal_line_on_its_instance_only(self):
        pid, past = pc.withdrawn_past(self.withdraw_log(), 'tw', 50)
        self.assertEqual(pid, '21')
        # Not cut 8 (before the line), 9 (below G), 11 (a rewind) or 13
        # (another instance).
        self.assertEqual(past, [(10, 50), (12, 52)])

    def test_no_withdrawal_line_no_instance(self):
        log = self.withdraw_log()
        log[0]['lines'][1] = log_line('publish', '-', '-', 'tw pending', 2, op='withdraw')
        self.assertEqual(pc.withdrawn_past(log, 'tw', 50), (None, []))
        log[0]['lines'][1] = log_line('publish', '-', '-', 'tw withdrawn', 2, op='asked')
        self.assertEqual(pc.withdrawn_past(log, 'tw', 50), (None, []))

    def test_exact_counts(self):
        st = {'publish_withdrawn': '1', 'publish_pending': '0', 'publish_published': '0'}
        self.assertEqual(pc.exact_counts(st, pc.TW_STATUS), [])
        self.assertEqual(pc.exact_counts(dict(st, publish_pending='1'), pc.TW_STATUS),
                         ['publish_pending is 1, not 0'])
        self.assertEqual(pc.exact_counts({}, {'control_peer': 1}), ['control_peer is missing, not 1'])
        self.assertEqual(pc.exact_counts({'control_answered': '5'}, {'control_answered': 4}),
                         ['control_answered is 5, not 4'])


if __name__ == '__main__':
    unittest.main()
