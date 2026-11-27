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
                't1-look': ('snap ',),                # the row node B reads (pin_t1_look)
                't1-other': ('accepted',),            # answered before any lookup in object storage
                't1-other-end': ('refused taken',),   # snap/t1 names another manifest
                't2': ('done ',),
                'att': ('refused attachment',),
                'state': ('refused state',),
                't4': ('done ',),
                't5': ('expired',),                   # G never reached in its limit
                't5-look': ('absent',),               # expired, so never made
                't3-look': ('absent',),               # never asked validly
                't7': ('pending after',),
                't7-other': ('refused asked',),
                't7-withdraw': ('withdrawn',),
                't7-again': ('accepted',),
                't7-end': ('done ',),
            },
            'prewind': {
                'r1': ('refused rewound',),       # rule (R): below the abandoned top every cut is a mark
                'r1-look': ('refused store', 'refused no-answer'),  # object storage down: never `absent`
                'r1-anew': ('accepted',),         # past the top, with a count read then
                'r1-end': ('done ',),
            },
            'prewindf': {
                'f1': ('pending after',),             # G far above: it waits
                'f1-end': ('refused rewound',),       # -FX's first cuts are marks, which end it
                'f1-look': ('absent',),               # ended, so never made
            },
            'prewindc': {                             # prewindf's control (#141's review, S1)
                'c1': ('pending after',),
                'c1-end': ('pending after',),         # a plain import marks no cut: it still waits
                'c1-withdraw': ('withdrawn',),
                'c1-look': ('absent',),               # withdrawn, so never made
            },
            'pkill': {
                'tw': ('pending after',),
                'tw-withdraw': ('withdrawn',),
                'tw-look': ('absent',),               # withdrawn, with a cut past its G committed
                'tk': ('pending after',),
                'tw-after': ('withdrawn', 'unknown'),  # the new instance may or may not know tw: never done
                'tk-after': ('unknown',),             # a pending publish lives in memory only
                'tk-again': ('refused rewound',),  # the count from before the take-over
                'tk-anew': ('accepted',),
                'tk-end': ('done ',),
                'tw-last': ('withdrawn', 'unknown'),
            },
            'pkillman': {
                'm1': ('none ', 'unknown'),           # killed under the agent: no answer, or the next one's
                'm1-after': ('unknown',),
                'm1-again': ('refused rewound',),
                'm1-anew': ('accepted',),
                'm1-end': ('done ',),
            },
            'pkillpub': {
                'p1': ('none ', 'unknown'),
                'p1-after': ('unknown',),
                'p1-again': ('refused rewound',),
                'p1-anew': ('accepted',),
                'p1-end': ('refused taken',),     # landed on a cut counted before the new count
                'p1-look': ('snap ',),            # on the commit that landed (pin_p1_found)
            },
            'pchannel': {
                'nu': ('refused no-upload',),
                'version': ('refused version',),
                'malformed': ('refused malformed',),
                'name': ('refused name',),
                'peer': ('refused peer',),
                'cuts': ('cuts ',),
            },
            'passist': {                              # the agent's assist on an idle pool (#144)
                'a1': ('done ',),                     # carried by the groups the assist writes
                'a2': ('pending after',),             # G far above: never carried
                'a2-withdraw': ('withdrawn',),
                'a2-look': ('absent',),               # withdrawn: never made
            },
            'pretire': {                              # retire (M2.7b-1)
                'e1': ('done ',),
                'e2': ('done ',),
                'e1-other': ('refused other',),       # asked with e2's commit
                'e1-kept': ('snap ',),                # the refusal deleted nothing
                'e1-progress': ('done ',),
                'e1-retire': ('retired',),
                'e1-look': ('absent',),
                'e1-progress-after': ('unknown',),    # its outcome let go (#175's D4)
                'e2-progress': ('done ',),
                'e1-again': ('gone',),                # landed, asked again: not an error
                'e9': ('gone',),                      # never published
                'e3': ('pending after',),
                'e3-retire': ('refused asked',),      # its publish pends
                'e3-withdraw': ('withdrawn',),
            },
            'pkillret': {
                'k1': ('done ',),
                'k1-retire': ('none ',),              # killed once the transaction landed: no answer
                'k1-again': ('gone',),                # the next instance, the same retire
                'k1-look': ('absent',),
            },
        })
        self.assertEqual(pc.PUBLISHED, {
            'pplain': {'t1': 't1', 't2': 't2', 't4': 't4', 't7': 't7-end'},
            'prewind': {'r1': 'r1-end'},
            'prewindf': {},
            'prewindc': {},
            'pkill': {'tk': 'tk-end'},
            'pkillman': {'m1': 'm1-end'},
            'pkillpub': {'p1': 'p1-look'},
            'pchannel': {},
            'passist': {'a1': 'a1'},
            'pretire': {'e2': 'e2'},
            'pkillret': {},
        })
        # t5 expired, t3 never asked validly, f1 ended by the rewind's
        # mark, c1 withdrawn, tw withdrawn before the kill, a2 withdrawn,
        # e1 and k1 retired, e9 never published.
        self.assertEqual(pc.ABSENT, {'pplain': ('t5', 't3'), 'prewindf': ('f1',), 'prewindc': ('c1',),
                                     'pkill': ('tw',), 'passist': ('a2',), 'pretire': ('e1', 'e9', 'e3'),
                                     'pkillret': ('k1',)})
        # A look at a name published must name its row.
        self.assertEqual(pc.LOOKS, {'pplain': {'t1': 't1-look'}, 'pkillpub': {'p1': 'p1-look'}})
        # manifest:1 kills before the transaction (no snap/m1 may show),
        # publish:1 after it (snap/p1 must).
        self.assertEqual(pc.BETWEEN, {'pkillman': ('m1', False), 'pkillpub': ('p1', True)})
        # Exact: the peer's request is never read, so not answered.
        self.assertEqual(pc.COUNTS, {'pchannel': {'control_answered': 4, 'control_peer': 1, 'control_malformed': 2}})
        self.assertEqual(pc.TW_STATUS, {'publish_withdrawn': 1, 'publish_pending': 0, 'publish_published': 0})
        self.assertEqual(pc.KILLED, {'pkillman': 'm1', 'pkillpub': 'p1', 'pkillret': 'k1'})
        self.assertEqual(set(pc.KILLED_BY), set(pc.KILLED))
        self.assertLessEqual(set(pc.BETWEEN), set(pc.KILLED))
        for course, (t, _) in pc.BETWEEN.items():
            self.assertEqual(pc.KILLED[course], t)
        # A retire's line is written after the uploader's answer: none
        # in the instance killed after the transaction landed.
        self.assertEqual(pc.RETIRES, {
            'pretire': [[('e1', 'refused other'), ('e1', 'retired'), ('e1', 'gone'), ('e9', 'gone'),
                         ('e3', 'refused asked')]],
            'pkillret': [[], [('k1', 'gone')]]})

    def test_killed_by_is_in_the_hook_s_own_words(self):
        root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        with open(os.path.join(root, 'bins', 'skyzfsd', 'src', 'kill.rs')) as f:
            src = f.read()
        self.assertIn('"after the framed manifest PUT {n} landed ({key}), before its ref was made"', src)
        self.assertIn('"after the transaction of publish {} ({}) landed, before the uploader knew it"', src)
        self.assertEqual(pc.KILLED_BY['pkillman'],
                         'after the framed manifest PUT {n} landed'.format(n=1))
        self.assertEqual(pc.KILLED_BY['pkillpub'], 'after the transaction of publish {} ({}) landed'.format(1, 'p1'))
        self.assertIn('"after the transaction of retire {} ({name}) landed, before the uploader knew it"', src)
        self.assertEqual(pc.KILLED_BY['pkillret'], 'after the transaction of retire {} ({name}) landed'.format(
            1, name='k1'))

    def test_each_published_name_ends_done_and_each_course_has_a_table(self):
        """Each published name ends `done`, but p1, which `look` reads
        after `refused taken` (rule (R), M2.6a)."""
        for course in pc.COURSES:
            self.assertIn(course, pc.EXPECTED)
            self.assertIn(course, pc.PUBLISHED)
            for t, label in pc.PUBLISHED[course].items():
                want = ('snap ',) if (course, t) == ('pkillpub', 'p1') else ('done ',)
                self.assertEqual(pc.EXPECTED[course][label], want, (course, t))
            for t, label in pc.LOOKS.get(course, {}).items():
                self.assertIn(t, pc.PUBLISHED[course])
                self.assertEqual(pc.EXPECTED[course][label], ('snap ',), (course, t))

    def test_a_snap_answer_names_its_commit_as_done_does(self):
        h = 'c' * 64
        self.assertEqual(pc.done_commit(f'done commit={h}'), h)
        self.assertEqual(pc.done_commit(f'snap commit={h} state={"b" * 32} number=8 group=50 manifest-len=1 '
                                        f'manifest-hash={"0" * 64}'), h)
        for other in (f'snap commit={h}', 'absent', f'unreadable commit={h} why=version', 'refused taken x'):
            self.assertIsNone(pc.done_commit(other), other)
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
    return {'event': event, 'sector': sector, 'sectors': sectors, 'result': result, 'line': k, 'op': op,
            'ns': 1000 * k}


class Look(unittest.TestCase):
    ROW = {'state': S, 'number': 9, 'group': 15, 'commit': H, 'snap': 'x', 'manifest_len': 414,
           'manifest_hash': 'd' * 64}
    LOOK = f'snap commit={H} state={S} number=9 group=15 manifest-len=414 manifest-hash={"d" * 64}'

    def test_a_look_that_names_the_row(self):
        self.assertEqual(pc.judge_look(self.LOOK, self.ROW), [])

    def test_each_field_that_differs_is_said(self):
        self.assertEqual(pc.judge_look(self.LOOK.replace('number=9', 'number=8'), self.ROW),
                         ['the look says number 8, the row 9'])
        self.assertEqual(pc.judge_look(self.LOOK.replace(' group=15', ''), self.ROW),
                         ['the look says group nothing, the row 15'])
        self.assertEqual(len(pc.judge_look(f'snap commit={H}', self.ROW)), 5)
        self.assertEqual(pc.judge_look(self.LOOK, dict(self.ROW, commit='e' * 64)),
                         [f'the look says commit {H}, the row {"e" * 64}'])
        self.assertEqual(pc.judge_look(self.LOOK, None), ['node B read no row to compare the look with'])


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
            ((S, 10, 52), 9, 'opens as itself'),
        ]
        for cut, named, why in cases:
            bad = pc.rewind_committed(self.CUTS, cut, R1, named)
            self.assertTrue(any(why in b for b in bad), (cut, named, bad))

    def test_r1_made_by_the_rewind_cut_itself_is_left_to_the_verdict(self):
        self.assertEqual(pc.rewind_committed(self.CUTS, (S, 9, 51), R1, 9), [])
        # An earlier cut that opens only as a rewind: the verdict too (D1).
        self.assertEqual(pc.rewind_committed(self.CUTS, (S, 9, 51), R1, 8), [])

    def test_r1_made_by_an_earlier_cut_that_opens_as_itself_is_no_case(self):
        cuts = [(8, 50, True, '1', 5), (9, 51, False, '1', 6)]
        self.assertEqual(pc.rewind_committed(cuts, (S, 9, 51), R1, 8),
                         ['r1 was made by cut 8, which opens as itself, before cut 9 was committed'])
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
        log[0]['lines'][1] = log_line('publish', '-', '-', 'tw withdrawn', 2, op='asked')
        self.assertEqual(pc.withdrawn_past(log, 'tw', 50), (None, []))
        log[0]['lines'][1] = log_line('publish', '-', '-', 'tw2 withdrawn', 2, op='withdraw')
        self.assertEqual(pc.withdrawn_past(log, 'tw', 50), (None, []))

    def test_the_withdrawal_line_whatever_it_answered(self):
        """B2 of #141's review: a daemon that answers the withdrawal
        `pending` fails pin_answer_tw-withdraw, and the premise holds."""
        log = self.withdraw_log()
        log[0]['lines'][1] = log_line('publish', '-', '-', 'tw pending', 2, op='withdraw')
        self.assertEqual(pc.withdrawn_past(log, 'tw', 50), ('21', [(10, 50), (12, 52)]))

    @staticmethod
    def rewound_log(refusal='r1 refused rewound', op='asked'):
        # The run at #140's 7d1152a (45551498087): the pool reached group
        # 43; r1 asked with C = 19 was refused after cuts 20 and 21.
        return [{'pid': '31', 'lines': [log_line('cut', 'itself', '17', '43', 1),
                                        log_line('cut', 'rewind', '18', '27', 2),
                                        log_line('cut', 'rewind', '19', '29', 3),
                                        log_line('cut', 'rewind', '20', '31', 4),
                                        log_line('cut', 'rewind', '21', '32', 5),
                                        log_line('publish', '30', '19', refusal, 6, op=op),
                                        log_line('cut', 'rewind', '22', '33', 7),
                                        log_line('cut', 'itself', '26', '48', 8)]}]

    def test_the_cuts_rule_r_refused_on(self):
        ask = pc.asked_line('r1', 19)
        self.assertEqual(pc.rewind_cuts(self.rewound_log(), 19, ask), (True, [(20, 31, 43, 4000), (21, 32, 43, 5000)]))
        # Cuts after the ask (22) are no ground, nor those at or before C.
        self.assertEqual(pc.rewind_cuts(self.rewound_log(), 21, ask), (True, []))

    def test_a_cut_above_the_groups_before_it_is_no_ground(self):
        log = self.rewound_log()
        log[0]['lines'][3:5] = [log_line('cut', 'itself', '20', '44', 4), log_line('cut', 'itself', '21', '45', 5)]
        self.assertEqual(pc.rewind_cuts(log, 19, pc.asked_line('r1', 19)), (True, []))
        # The equal group (the first cut of an -FX import) is a ground.
        log[0]['lines'][3:5] = [log_line('cut', 'itself', '20', '43', 4)]
        self.assertEqual(pc.rewind_cuts(log, 19, pc.asked_line('r1', 19)), (True, [(20, 43, 43, 4000)]))

    def test_the_ask_is_the_anchor_whatever_it_was_answered(self):
        """B2 of #141's review: a daemon without rule (R) at the ask, which
        answers r1 `accepted`, fails pin_answer_r1 (exit 1); the premise
        rests on the cut lines and the ask's place only."""
        for answer in ('r1 accepted', 'r1 refused rewound', 'r1 refused store'):
            self.assertEqual(pc.rewind_cuts(self.rewound_log(answer), 19, pc.asked_line('r1', 19)),
                             (True, [(20, 31, 43, 4000), (21, 32, 43, 5000)]), answer)
        for answer, op in (('r12 accepted', 'asked'), ('r1 refused rewound', 'ended'),
                           ('r1 withdrawn', 'withdraw')):
            self.assertEqual(pc.rewind_cuts(self.rewound_log(answer, op), 19, pc.asked_line('r1', 19))[0], False,
                             (answer, op))
        # Review of #141, N3: the line of an ask with another C (an ask
        # anew) is not the first ask's.
        self.assertEqual(pc.rewind_cuts(self.rewound_log(), 19, pc.asked_line('r1', 27)), (False, []))
        log = self.rewound_log()
        log[0]['lines'][5] = log_line('publish', '30', '27', 'r1 accepted', 6, op='asked')
        self.assertEqual(pc.rewind_cuts(log, 19, pc.asked_line('r1', 19))[0], False)

    def test_with_no_stop_every_ground_of_the_first_instance_that_has_one(self):
        """prewindf: the ground needs no end line."""
        log = self.rewound_log('f1 accepted')
        self.assertEqual(pc.rewind_cuts(log, 19), (False, [(20, 31, 43, 4000), (21, 32, 43, 5000), (22, 33, 43, 7000)]))
        self.assertEqual(pc.rewind_cuts([{'pid': '1', 'lines': []}] + log, 19)[1][0], (20, 31, 43, 4000))
        self.assertEqual(pc.rewind_cuts(log, 26), (False, []))

    def test_the_cuts_since_the_import_up_to_the_withdrawal(self):
        """prewindc's premise (#141's review, S1): cuts counted after C,
        on lines after the import's mark, before the withdrawal's line."""
        log = [{'pid': '31', 'lines': [log_line('cut', 'itself', '17', '43', 1),
                                       log_line('cut', 'itself', '18', '44', 2),
                                       log_line('cut', 'itself', '19', '45', 3),
                                       log_line('cut', 'itself', '20', '46', 4),
                                       log_line('publish', '-', '-', 'c1 withdrawn', 5, op='withdraw'),
                                       log_line('cut', 'itself', '21', '47', 6)]}]
        stop = pc.withdraw_line('c1')
        self.assertEqual(pc.cuts_since(log, 17, 2500, stop), [(19, 45), (20, 46)])
        # counted at or before C, though after the mark: not one
        self.assertEqual(pc.cuts_since(log, 19, 2500, stop), [(20, 46)])
        self.assertEqual(pc.cuts_since(log, 20, 0, stop), [])
        # whatever the withdrawal answered; another name's is not c1's
        log[0]['lines'][4] = log_line('publish', '-', '-', 'c1 unknown', 5, op='withdraw')
        self.assertEqual(pc.cuts_since(log, 17, 2500, stop), [(19, 45), (20, 46)])
        for result, op in (('c12 withdrawn', 'withdraw'), ('c1 withdrawn', 'ended')):
            log[0]['lines'][4] = log_line('publish', '-', '-', result, 5, op=op)
            self.assertEqual(pc.cuts_since(log, 17, 2500, stop), [], (result, op))

    def test_an_end_after_the_ground(self):
        log = self.rewound_log('f1 accepted')
        log[0]['lines'].append(log_line('publish', '-', '-', 'f1 refused rewound', 9, op='ended'))
        self.assertEqual(pc.judge_ended(log, 'f1', 4000), [])
        self.assertEqual(len(pc.judge_ended(log, 'f1', 9000)), 1)
        log[0]['lines'].insert(7, log_line('publish', '-', '-', 'f1 expired', 7, op='ended'))
        self.assertEqual(pc.judge_ended(log, 'f1', 4000), ["f1's first end is 'f1 expired' (line 7)"])
        self.assertEqual(pc.judge_ended(self.rewound_log('f1 accepted'), 'f1', 4000),
                         ['the request log never ends f1'])

    def test_the_ask_s_deadline_from_the_agent_s_record(self):
        text = ('100\t200\tcuts\t0\tcuts attachment=a\n'
                '5000000000\t5000000100\tpublish --name f1 --attachment a --after 3 --expiry-s 900\t0\taccepted\n'
                '7000000000\t7000000100\tpublish --name f1 --attachment a --after 4\t0\taccepted\n')
        self.assertEqual(pc.ask_deadline(text, 'f1'), 905_000_000_000)
        self.assertEqual(pc.ask_deadline(text.split('\n', 2)[2], 'f1'), 607_000_000_000)
        self.assertIsNone(pc.ask_deadline(text, 'f12'))
        with self.assertRaises(RigError):
            pc.ask_deadline('1\t2\tpublish --name f1 --expiry-s x\t0\t\n', 'f1')

    def test_exact_counts(self):
        st = {'publish_withdrawn': '1', 'publish_pending': '0', 'publish_published': '0'}
        self.assertEqual(pc.exact_counts(st, pc.TW_STATUS), [])
        self.assertEqual(pc.exact_counts(dict(st, publish_pending='1'), pc.TW_STATUS),
                         ['publish_pending is 1, not 0'])
        self.assertEqual(pc.exact_counts({}, {'control_peer': 1}), ['control_peer is missing, not 1'])
        self.assertEqual(pc.exact_counts({'control_answered': '5'}, {'control_answered': 4}),
                         ['control_answered is 5, not 4'])



KSTAT_HEAD = 'txg      birth            state ndirty       nread        nwritten     reads    writes\n'


class Assist(unittest.TestCase):
    """passist (#144's assist, M2.T5's stage 0): its file, and the pace's
    figures."""

    def test_assist_tsv_reads_and_a_line_that_does_not_is_the_rig_s_error(self):
        rows = pc.read_assists('a1\t100\t200\t5000\t60\t61\t61\na2\t300\t400\t5001\t61\t61\t62\n')
        self.assertEqual(rows, [{'name': 'a1', 't0': 100, 't1': 200, 'value': 5000, 'before': 60, 'mid': 61,
                                 'after': 61},
                                {'name': 'a2', 't0': 300, 't1': 400, 'value': 5001, 'before': 61, 'mid': 61,
                                 'after': 62}])
        self.assertEqual(pc.read_assists(''), [])
        for bad in ('a1\t100\t200\t5000\t60\t61\n', 'a1\t100\t200\t5000\t60\t61\tx\n',
                    '\t100\t200\t5000\t60\t61\t61\n', 'a1\t100\t200\t5000\t60\t61\t61\t9\n'):
            with self.assertRaises(pc.RigError, msg=bad):
                pc.read_assists(bad)

    def test_the_span_of_a_name_s_requests(self):
        text = ('1000000000\t1100000000\tpublish --name a2 --after 7\t0\taccepted\n'
                '1200000000\t1300000000\tpublish-progress --name a1\t0\tpending\n'
                '5000000000\t6000000000\tpublish-progress --name a2\t0\tpending\n'
                '7000000000\t7100000000\tcuts\t0\tcuts x\n')
        self.assertEqual(pc.requests_span(text, 'a2'), 5.0)
        self.assertIsNone(pc.requests_span(text, 'a3'))

    def test_the_cut_lines_times(self):
        log = [{'pid': '31', 'lines': [log_line('cut', 'itself', '17', '43', 1),
                                       log_line('publish', '-', '-', 'c1 withdrawn', 2, op='withdraw'),
                                       log_line('cut', 'rewind', '18', '44', 3)]}]
        self.assertEqual(pc.cut_times(log), [(1000, 17, 43), (3000, 18, 44)])

    def test_the_pace(self):
        rows = [{'t0': 10 * 10**9, 't1': 11 * 10**9, 'before': 60, 'mid': 61, 'after': 61},
                {'t0': 25 * 10**9, 't1': 26 * 10**9, 'before': 61, 'mid': 61, 'after': 62},
                {'t0': 39 * 10**9, 't1': 40 * 10**9, 'before': 62, 'mid': 63, 'after': 64}]
        cuts = [(9 * 10**9, 1, 60), (10 * 10**9, 2, 61), (30 * 10**9, 3, 62), (40 * 10**9, 4, 64),
                (41 * 10**9, 5, 65)]
        # Groups 61 to 64 that wrote, and two between them that wrote nothing.
        txgs = KSTAT_HEAD + ''.join(f'{g} 1 C 0 0 {w} 0 0\n' for g, w in
                                    ((59, 4096), (60, 4096), (61, 4096), (62, 0), (63, 8192), (64, 4096), (65, 0)))
        p = pc.assist_pace(rows, cuts, {'uploaded': '3', 'upload_blocks': '100'},
                           {'uploaded': '9', 'upload_blocks': '160'}, 60.0, txgs)
        self.assertEqual(p, {'lacks': [], 'assists': 3, 'seconds': 30.0, 'assists_per_min': 6.0,
                             'groups': 3, 'groups_per_min': 6.0, 'empty': 1, 'cuts': 3, 'cuts_per_min': 6.0,
                             'set_wrote': 2, 'sync_wrote': 2,
                             'flow_s': 60.0, 'uploaded': 6, 'uploads_per_min': 6.0,
                             'blocks': 60, 'blocks_per_min': 60.0})
        # A status without a count is said, with no upload figures (F2).
        for before, after, lacks in (({}, {'uploaded': '9', 'upload_blocks': '1'},
                                      ['upload_blocks (before)', 'uploaded (before)']),
                                     ({'uploaded': '3', 'upload_blocks': '1'}, {'uploaded': 'x', 'upload_blocks': '1'},
                                      ['uploaded (after)'])):
            p = pc.assist_pace(rows, cuts, before, after, 60.0, txgs)
            self.assertEqual((p['lacks'], p['uploaded'], p['uploads_per_min'], p['blocks'], p['blocks_per_min']),
                             (lacks, None, None, None, None))
            self.assertEqual(p['groups'], 3)
        # No span of requests is the rig's.
        with self.assertRaises(pc.RigError):
            pc.assist_pace(rows, cuts, {'uploaded': '3', 'upload_blocks': '1'},
                           {'uploaded': '4', 'upload_blocks': '1'}, None, txgs)

    def test_accepted_tsv_reads_and_a_line_that_does_not_is_the_rig_s_error(self):
        m = pc.read_marks('a1\taccepted\t100\t50\t10.0\na1\tend\t900\t56\tdone\na2\taccepted\t950\t56\t10\n')
        self.assertEqual(m, {'a1': {'accepted': (100, 50, 10.0), 'end': (900, 56, 'done')},
                             'a2': {'accepted': (950, 56, 10.0)}})
        for bad in ('a1\taccepted\t100\t50\n', 'a1\tstarted\t100\t50\t10\n', 'a1\taccepted\tx\t50\t10\n',
                    'a1\taccepted\t100\t50\tten\n', 'a1\taccepted\t100\t50\t0\n', 'a1\tend\t100\t50\t\n',
                    '\taccepted\t100\t50\t10\n', 'a1\tend\t1\t5\tdone\na1\tend\t2\t5\tdone\n'):
            with self.assertRaises(pc.RigError, msg=bad):
                pc.read_marks(bad)

    def a1(self, g, marks, rows, wrote=range(40, 70)):
        txgs = KSTAT_HEAD + ''.join(f'{x} 1 C 0 0 {4096 if x in wrote else 0} 0 0\n' for x in range(40, 70))
        return pc.a1_premise(g, marks, [dict(zip(('before', 'mid', 'after'), r)) for r in rows], txgs)

    def test_a1_s_premise_rests_on_the_marks_the_assists_and_the_kstat(self):
        acc = (0, 50, 10.0)
        ok, why = self.a1(55, {'accepted': acc, 'end': (9, 56, 'done')}, [(50, 51, 52), (52, 53, 54), (54, 55, 56)])
        self.assertTrue(ok, why)
        self.assertIn('assist 3 took it across G', why)
        # The pool below G at the end: a `done` is the daemon's alone (the
        # pins judge it), a `pending` means the assists did not get there.
        ok, why = self.a1(55, {'accepted': acc, 'end': (9, 52, 'done')}, [(50, 51, 52)])
        self.assertTrue(ok, why)
        ok, why = self.a1(55, {'accepted': acc, 'end': (9, 50, 'refused')}, [])
        self.assertTrue(ok, why)
        for g, marks, rows, wrote, said in (
                (55, {'accepted': acc, 'end': (9, 54, 'pending')}, [(50, 51, 52), (52, 53, 54)], range(40, 70),
                 'never took the pool to G'),
                (50, {'accepted': acc, 'end': (9, 52, 'done')}, [(50, 51, 52)], range(40, 70),
                 'G was written before any assist'),
                (55, {'accepted': acc, 'end': (9, 56, 'done')}, [], range(40, 70), 'no assist took the pool across G'),
                # Assists that wrote and a group from G on that none wrote.
                (55, {'accepted': acc, 'end': (9, 58, 'done')}, [(50, 51, 52), (52, 54, 55), (56, 57, 58)],
                 range(40, 70), 'groups no assist wrote: 56'),
                # The end read past the last assist: a group after it.
                (55, {'accepted': acc, 'end': (9, 57, 'done')}, [(50, 51, 52), (52, 54, 56)], range(40, 70),
                 'groups no assist wrote: 57'),
                (55, {'accepted': acc}, [(50, 51, 56)], range(40, 70), 'no end mark')):
            with self.subTest(said=said):
                ok, why = self.a1(g, marks, rows, wrote)
                self.assertFalse(ok)
                self.assertIn(said, why)
        # A group the kstat says wrote nothing is no stray, however it lies.
        ok, why = self.a1(55, {'accepted': acc, 'end': (9, 58, 'done')}, [(50, 51, 52), (52, 54, 55), (56, 57, 58)],
                          [x for x in range(40, 70) if x != 56])
        self.assertTrue(ok, why)
        # Below G a group no assist wrote carries nothing (the hold's
        # history, which the txg timeout writes after the acceptance: the
        # first run at 9bac3dd, group 17 for G 20, accepted at 16; here
        # 30 groups on).
        ok, why = self.a1(50, {'accepted': (0, 46, 10.0), 'end': (9, 53, 'done')}, [(47, 48, 49), (49, 52, 53)])
        self.assertTrue(ok, why)
        ok, why = self.a1(55, {'accepted': acc, 'end': (9, 56, 'done')}, [(50, 51, 52), (53, 54, 56)])
        self.assertTrue(ok, why)
        # A kstat that does not reach back over a1's groups is the rig's.
        with self.assertRaises(pc.RigError):
            self.a1(55, {'accepted': (0, 30, 10.0), 'end': (9, 56, 'done')}, [(30, 51, 56)])

    def test_a1_s_premise_at_its_boundaries(self):
        """#145's next look, T2 (plants N13, N16, N19)."""
        acc = (0, 50, 10.0)
        # The pool's top is the newest of the end mark and the assists'
        # reads: an end mark below G after an assist that reached it.
        ok, why = self.a1(55, {'accepted': acc, 'end': (9, 54, 'pending')}, [(50, 52, 54), (54, 55, 56)])
        self.assertTrue(ok, why)
        self.assertIn('assist 2 took it across G', why)
        # A top at G itself is G reached: a `pending` there is no error.
        ok, why = self.a1(55, {'accepted': acc, 'end': (9, 55, 'pending')}, [(50, 52, 54), (54, 55, 55)])
        self.assertTrue(ok, why)
        # An assist that starts at G did not take the pool across it.
        ok, why = self.a1(55, {'accepted': acc, 'end': (9, 57, 'done')}, [(50, 52, 54), (55, 56, 57)])
        self.assertFalse(ok, why)
        self.assertIn('no assist took the pool across G', why)

    def test_the_decided_pace(self):
        """Floors are the premise's verdict; a gap over 3 times its decided
        value is only observed (the lead's answer to #145's next look, S1),
        and the burst's pause runs from the end of the one before (S2)."""
        s = 10**9

        def rows(*t0, took=0.02):
            return [{'name': 'a2', 't0': int(t * s), 't1': int((t + took) * s)} for t in t0]
        ok = rows(10.0, 11.1, 12.2, 22.3, 32.4)
        self.assertEqual(pc.pace_faults(ok, 0, 10.0), ([], []))
        self.assertEqual(pc.pace_faults(ok, 0, 10.0, (int(47.0 * s), 60, 'pending')), ([], []))
        self.assertEqual(pc.pace_faults(rows(), 0, 10.0), ([], []))
        # The floors: before A; a pause under 1 s in the burst; a round
        # past it under 0.95 A; and for a shorter A, the same rules.
        for t0, said in (((9.9, 11.0), 'first assist 9.9 s'),
                         ((10.0, 10.9), 'assist 2 0.88 s after the end of the one before, in the burst'),
                         ((10.0, 11.1, 12.2, 13.3), 'assist 4 1.1 s'), ((10.0, 11.1, 12.2, 21.6), 'assist 4 9.4 s')):
            with self.subTest(t0=t0):
                bad, slow = pc.pace_faults(rows(*t0), 0, 10.0)
                self.assertEqual((len(bad), slow), (1, []), bad)
                self.assertIn(said, bad[0])
        self.assertEqual(pc.pace_faults(rows(4.0, 5.0, 6.0, 10.0, 14.0, took=0.5), 0, 4.0), ([], []))
        self.assertTrue(pc.pace_faults(rows(4.0, 5.0, 6.0, 7.0), 0, 4.0)[0])
        # The burst's pause from the end of the one before: a sync of
        # 4.5 s and a pause of 1 s keep it.
        self.assertEqual(pc.pace_faults(rows(10.0, 15.5, 21.0, took=4.5), 0, 10.0), ([], []))
        # Slow, observed and no fault: each gap over 3 times its value,
        # and a publish pending long after its last assist, or after its
        # acceptance with none.
        for t0, end, said in (((30.1, 31.2), None, 'first assist 30.10 s after its acceptance'),
                              ((10.0, 13.1), None, 'assist 2 3.08 s after the end of the one before, in the burst'),
                              ((10.0, 11.1, 12.2, 42.3), None, 'assist 4 30.10 s after the one before'),
                              ((10.0, 11.1, 12.2), (int(42.4 * s), 60, 'pending'), 'ended pending 30.18 s after its last'),
                              ((), (int(30.1 * s), 60, 'pending'), 'ended pending 30.10 s after its acceptance, with no')):
            with self.subTest(t0=t0, end=end):
                bad, slow = pc.pace_faults(rows(*t0), 0, 10.0, end)
                self.assertEqual((bad, len(slow)), ([], 1), slow)
                self.assertIn(said, slow[0])
        # Edges: 3 times exactly is not over; an end other than pending is
        # not read.
        self.assertEqual(pc.pace_faults(rows(30.0, 33.02, 36.04, 66.04), 0, 10.0,
                                        (int(96.06 * s), 60, 'pending')), ([], []))
        self.assertEqual(pc.pace_faults(rows(10.0), 0, 10.0, (int(99 * s), 60, 'done')), ([], []))
        # Just over 3 times prints as over it (#145's third look, N1).
        bad, slow = pc.pace_faults(rows(10.0, 11.1, 12.2, 42.22), 0, 10.0)
        self.assertEqual(bad, [])
        self.assertIn('assist 4 30.02 s after the one before', slow[0])

    def test_a_spaced_gap_between_0_95_a_and_a(self):
        """#145's third look, T3: the spaced floor is 0.95 A, not A (plant
        F9). A spaced gap of 9.6 s at A = 10 s, as replay F4 has, keeps the
        pace; one of 9.4 s does not."""
        s = 10**9

        def rows(*t0):
            return [{'name': 'a2', 't0': int(t * s), 't1': int((t + 0.02) * s)} for t in t0]
        self.assertEqual(pc.pace_faults(rows(10.0, 11.1, 12.2, 21.8), 0, 10.0), ([], []))
        self.assertTrue(pc.pace_faults(rows(10.0, 11.1, 12.2, 21.6), 0, 10.0)[0])

    def test_the_burst_s_pause_does_not_grow_past_one_second(self):
        """#145's third look, S5: neither agent's pause grows with A, so at
        A over ASSIST_AFTER the burst's floor stays 1 s (replay V8b: the
        stand-in's pace at A = 20 s, a pause of 1.1 s); a shorter A still
        scales it down, for the fixtures' clock."""
        s = 10**9

        def rows(*t0, took=0.02):
            return [{'name': 'a2', 't0': int(t * s), 't1': int((t + took) * s)} for t in t0]
        self.assertEqual(pc.pace_faults(rows(20.0, 21.12, 22.24, 42.3), 0, 20.0), ([], []))
        bad, _ = pc.pace_faults(rows(20.0, 20.9), 0, 20.0)
        self.assertIn('before the pause of 1 s', bad[0])
        bad, _ = pc.pace_faults(rows(5.0, 5.4), 0, 5.0)
        self.assertIn('before the pause of 0.5 s', bad[0])
        self.assertEqual(pc.pace_faults(rows(5.0, 5.6), 0, 5.0), ([], []))

    def test_no_assist_names_its_publish(self):
        """#145's third look, N1: a publish with no assist that ended
        pending is named."""
        s = 10**9
        _, slow = pc.pace_faults([], 0, 10.0, (int(31 * s), 60, 'pending'), 'a1')
        self.assertIn('a1 ended pending 31.00 s after its acceptance', slow[0])

    def test_a1_s_slow_gaps_are_kept_beside_a2_s(self):
        """#145's third look, T5 (plant F19: a2's slow gaps replaced
        a1's)."""
        s = 10**9
        rows = [{'name': 'a1', 't0': int(40 * s), 't1': int(40.1 * s)},
                {'name': 'a2', 't0': int(110 * s), 't1': int(110.1 * s)}]
        marks = {'a1': {'accepted': (0, 50, 10.0)}, 'a2': {'accepted': (int(100 * s), 60, 10.0)}}
        faults, slow = pc.assists_paced(rows, marks)
        self.assertEqual(faults, [('a1', []), ('a2', [])])
        self.assertEqual(len(slow), 1)
        self.assertIn("a1's first assist 40.00 s", slow[0])
        rows[1]['t0'], rows[1]['t1'] = int(150 * s), int(150.1 * s)
        faults, slow = pc.assists_paced(rows, marks)
        self.assertEqual([x[:9] for x in slow], ["a1's firs", "a2's firs"])
        # A publish never accepted is not judged.
        self.assertEqual(pc.assists_paced(rows, {'a2': marks['a2']})[0], [('a2', [])])

    def test_the_kstat_s_groups_that_wrote_and_those_that_did_not(self):
        txgs = KSTAT_HEAD + '41 1 C 0 0 4096 0 0\n42 1 C 0 0 0 0 0\n43 1 C 0 0 512 0 0\n44 1 S 0 0 0 0 0\n'
        self.assertEqual(pc.kstat_groups(txgs, 40, 43), (2, 1))
        self.assertEqual(pc.kstat_groups(txgs, 41, 42), (0, 1))
        # A history that does not reach back, or forward, over the range,
        # or a group still syncing, or an nwritten that is no number.
        for lo, hi, text in ((39, 43, txgs), (41, 44, txgs), (40, 43, txgs.replace('42 1 C 0 0 0', '42 1 C 0 0 x'))):
            with self.assertRaises(pc.RigError, msg=(lo, hi)):
                pc.kstat_groups(text, lo, hi)


if __name__ == '__main__':
    unittest.main()
