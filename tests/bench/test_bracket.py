#!/usr/bin/env python3
"""Tests of bracket.py: a daemon row against the model rows around it."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bracket as bk  # noqa: E402

M = 'be53d812-3cf9-4196-bdd2-0d72aea53228 runnervm7aw5e'


def row(seq, topo, tps, machine=M, delay=0, workload='tpcb', **kw):
    r = {'_seq': seq, 'topology': topo, 'tps': tps, 'machine': machine,
         'origin_delay_ms': delay, 'workload': workload}
    r.update(kw)
    return r


# Run 43234635170 (24.04, slow syncs): its sequence's tps, as recorded.
RUN = [row(1, 'zfs-slog', 3732.371195), row(2, 'zfs-skyzfsd', 3438.17023),
       row(3, 'zfs-skyzfsd', 3433.045722), row(4, 'zfs-skyzfsd', 3443.621724),
       row(5, 'zfs-skyzfsd', 3194.584762), row(6, 'zfs-slog', 3677.098293)]


class Bracketed(unittest.TestCase):
    def test_recorded_run(self):
        # By hand: 3433 / mean(3732, 3677) = 0.927.
        b = bk.bracketed(RUN[2], RUN)
        self.assertIs(b['before'], RUN[0])
        self.assertIs(b['after'], RUN[5])
        self.assertAlmostEqual(b['ratio'], 3433.045722 / ((3732.371195 + 3677.098293) / 2))
        self.assertEqual(f"{b['ratio']:.3f}", '0.927')
        self.assertEqual(b['verdict'], 'at least 0.90')
        self.assertEqual(bk.bracketed(RUN[4], RUN)['verdict'], 'BELOW 0.90')  # 0.862

    def test_nearest_model_rows(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950), row(3, 'zfs-slog', 1200),
                row(4, 'zfs-skyzfsd', 1000), row(5, 'zfs-skyzfsd', 1000), row(6, 'zfs-slog', 800)]
        b = bk.bracketed(rows[1], rows)
        self.assertEqual((b['before']['_seq'], b['after']['_seq']), (1, 3))
        self.assertAlmostEqual(b['ratio'], 950 / 1100)
        for r in rows[3:5]:
            b = bk.bracketed(r, rows)
            self.assertEqual((b['before']['_seq'], b['after']['_seq']), (3, 6))
            self.assertAlmostEqual(b['ratio'], 1.0)

    def test_not_enclosed(self):
        rows = [row(1, 'zfs-skyzfsd', 900), row(2, 'zfs-slog', 1000), row(3, 'zfs-skyzfsd', 900)]
        for r, where in ((rows[0], 'before'), (rows[2], 'after')):
            b = bk.bracketed(r, rows)
            self.assertIsNone(b['ratio'])
            self.assertTrue(b['verdict'].startswith('NO VERDICT'), b['verdict'])
            self.assertIn(where, b['verdict'])

    def test_not_in_a_sequence(self):
        rows = [row(None, 'zfs-slog', 1000), row(None, 'zfs-skyzfsd', 950), row(None, 'zfs-slog', 1000)]
        b = bk.bracketed(rows[1], rows)
        self.assertIsNone(b['ratio'])
        self.assertIn('not in a sequence', b['verdict'])

    def test_other_machine_workload_or_snapshots_do_not_enclose(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950),
                row(3, 'zfs-slog', 1000, machine='other host'),
                row(3, 'zfs-slog', 1000, workload='select'),
                row(4, 'zfs-slog', 1000, snap={'mode': 'sync'}),
                row(5, 'zfs-slog', 0)]
        b = bk.bracketed(rows[1], rows)
        self.assertIsNone(b['after'])
        self.assertIsNone(b['ratio'])
        # A second job's sequence uses the same numbers on another machine.
        other = [row(1, 'zfs-slog', 2000, machine='x'), row(2, 'zfs-skyzfsd', 1900, machine='x'),
                 row(3, 'zfs-slog', 2000, machine='x')]
        b = bk.bracketed(other[1], RUN + other)
        self.assertAlmostEqual(b['ratio'], 0.95)

    def test_no_machine_or_no_tps(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, machine=None), row(3, 'zfs-slog', 1000)]
        self.assertIn('what machine', bk.bracketed(rows[1], rows)['verdict'])
        rows[1] = row(2, 'zfs-skyzfsd', None)
        self.assertEqual(bk.bracketed(rows[1], rows)['verdict'], 'no tps: the run did not measure')

    def test_bracket_too_wide(self):
        # Run 43250525526 (26.04): model rows 3371 and 2447 around row 5.
        rows = [row(4, 'zfs-slog', 3371.0), row(5, 'zfs-skyzfsd', 1547.0), row(7, 'zfs-slog', 2447.0)]
        b = bk.bracketed(rows[1], rows)
        self.assertAlmostEqual(b['ratio'], 1547 / ((3371 + 2447) / 2))
        self.assertAlmostEqual(b['width'], 924 / 2909)
        self.assertTrue(b['verdict'].startswith('NO VERDICT: bracket too wide'), b['verdict'])
        self.assertIn('31.8%', b['verdict'])
        # 10% of the mean is still judged; just over it is not.
        for before, judged in ((1050, True), (1051, False)):
            rows = [row(1, 'zfs-slog', before), row(2, 'zfs-skyzfsd', 1000), row(3, 'zfs-slog', 950)]
            b = bk.bracketed(rows[1], rows)
            self.assertEqual(b['verdict'].startswith('NO VERDICT'), not judged, (before, b))
        # The recorded run of 43234635170: 3732 and 3677, 1.5% apart.
        self.assertAlmostEqual(bk.bracketed(RUN[2], RUN)['width'], 55.273 / 3704.73, places=4)

    def test_model_row_with_other_settings(self):
        base = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950), row(3, 'zfs-slog', 1000)]
        for k, v, said in (('clients', 16, 'clients 16 against 8'), ('duration_s', 30, 'measured seconds 30 against 120'),
                           ('mem_limit', '2G', 'TB_MEM_LIMIT 2G against none')):
            rows = [dict(r, clients=8, duration_s=120, mem_limit='none') for r in base]
            rows[2][k] = v
            b = bk.bracketed(rows[1], rows)
            self.assertAlmostEqual(b['ratio'], 0.95)
            self.assertTrue(b['verdict'].startswith('NO VERDICT: a model row ran with other settings'), b['verdict'])
            self.assertIn('the model row after it: ' + said, b['verdict'])
        # The same settings, or a key a row does not carry, judge as before.
        rows = [dict(r, clients=8, duration_s=120, mem_limit='none') for r in base]
        self.assertEqual(bk.bracketed(rows[1], rows)['verdict'], 'at least 0.90')
        del rows[0]['mem_limit']
        self.assertEqual(bk.bracketed(rows[1], rows)['verdict'], 'at least 0.90')

    def test_exactly_the_criterion(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 900), row(3, 'zfs-slog', 1000)]
        self.assertEqual(bk.bracketed(rows[1], rows)['verdict'], 'at least 0.90')
        rows[1] = row(2, 'zfs-skyzfsd', 899.9)
        self.assertEqual(bk.bracketed(rows[1], rows)['verdict'], 'BELOW 0.90')

    def test_origin_delay(self):
        rows = [row(1, 'zfs-slog', 1000, delay=10), row(2, 'zfs-skyzfsd', 950), row(3, 'zfs-slog', 1000)]
        b = bk.bracketed(rows[1], rows)
        self.assertAlmostEqual(b['ratio'], 0.95)
        self.assertIn('origin delays of 10 and 0 ms', b['verdict'])
        rows[0]['origin_delay_ms'] = None
        self.assertIn('does not say what origin delay', bk.bracketed(rows[1], rows)['verdict'])

    def test_a_traced_row_has_no_verdict(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, syscalls={'tool': 'perf'}),
                row(3, 'zfs-slog', 1000)]
        b = bk.bracketed(rows[1], rows)
        self.assertIsNone(b['ratio'])
        self.assertIn('NO VERDICT: its system calls were traced', b['verdict'])

    def test_a_row_marked_by_its_setting_has_no_verdict(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, slowed_by=['TB_PERF_RECORD']),
                row(3, 'zfs-slog', 1000)]
        b = bk.bracketed(rows[1], rows)
        self.assertIsNone(b['ratio'])
        self.assertIn('NO VERDICT: its daemon was traced or recorded (TB_PERF_RECORD)', b['verdict'])

    def test_a_row_whose_releaser_ended_has_no_verdict(self):
        """TB_SKZ_RELEASE: a row whose releaser ended before the window
        did was not measured as its label says (M2.T4)."""
        rows = [row(1, 'zfs-slog', 1000),
                row(2, 'zfs-skyzfsd', 950, skyzfsd={'release': {'every': 1, 'ended': '# ERROR HolderError: no reply'}}),
                row(3, 'zfs-slog', 1000)]
        b = bk.bracketed(rows[1], rows)
        self.assertIsNone(b['ratio'])
        self.assertIn('NO VERDICT: its releaser of held cuts (TB_SKZ_RELEASE) ended', b['verdict'])
        self.assertIn('no reply', b['verdict'])
        rows[1]['skyzfsd']['release'] = {'every': 1, 'in_window': 23, 'running': True}
        self.assertEqual(bk.bracketed(rows[1], rows)['verdict'], 'at least 0.90')

    def test_a_recorded_row_has_no_verdict(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, perf_record={'threads': {}}),
                row(3, 'zfs-slog', 1000)]
        b = bk.bracketed(rows[1], rows)
        self.assertIsNone(b['ratio'])
        self.assertIn('NO VERDICT: the daemon was recorded by perf', b['verdict'])


def uploading(sb=None, sa=None, pid_same=True):
    """A row's skyzfsd part with uploads: by default one that uploaded
    over its window under one lease (the figures of run 44372469282)."""
    b = {'pid': '4242', 'lease': 'held', 'lease_epoch': '1', 'uploaded': '3'}
    a = {'pid': '4242', 'lease': 'held', 'lease_epoch': '1', 'uploaded': '27'}
    b.update(sb or {}); a.update(sa or {})
    return {'upload': {'endpoint': 'http://127.0.0.1:9000', 'bucket': 'tb-1-1', 'volume': 'tb-1'},
            'before': b, 'after': a, 'pid_same': pid_same}


class AnUploadRow(unittest.TestCase):
    """A row that says it uploads and did not over its window keeps its
    ratio, printed, and has no verdict (review's G4 on #123, the lead's
    decision (c))."""

    def judge(self, k, tps=950):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', tps, skyzfsd=k), row(3, 'zfs-slog', 1000)]
        return bk.bracketed(rows[1], rows)

    def assert_no_verdict(self, b, *words):
        self.assertTrue(b['verdict'].startswith('NO VERDICT: the row says it uploads, but '), b['verdict'])
        for w in words:
            self.assertIn(w, b['verdict'])
        # Its figures are still shown.
        self.assertAlmostEqual(b['ratio'], 0.95)
        self.assertAlmostEqual(b['width'], 0.0)

    def test_a_row_that_uploaded_under_one_lease_is_judged(self):
        self.assertEqual(self.judge(uploading())['verdict'], 'at least 0.90')
        self.assertEqual(self.judge(uploading(), tps=850)['verdict'], 'BELOW 0.90')
        # Unsure is held: the uploads go on under its epoch.
        self.assertEqual(self.judge(uploading(sa={'lease': 'unsure'}))['verdict'], 'at least 0.90')
        # A row without uploads is not asked.
        self.assertEqual(self.judge({'before': {}, 'after': {}, 'pid_same': True})['verdict'], 'at least 0.90')

    def test_no_upload_committed_in_the_window(self):
        b = self.judge(uploading(sa={'uploaded': '3'}))
        self.assert_no_verdict(b, 'no upload committed in its window', 'uploaded 3 at its start and 3 at its end')
        # Counters that cannot be read are no proof of an upload.
        for sb, sa in (({'uploaded': ''}, {}), ({}, {'uploaded': 'x'}), ({'uploaded': None}, {})):
            k = uploading(sb, sa)
            for s in (k['before'], k['after']):
                if s.get('uploaded') is None:
                    del s['uploaded']
            self.assert_no_verdict(self.judge(k), 'its uploads cannot be read')

    def test_no_lease_for_the_whole_window(self):
        for sb, sa, said in (({'lease': 'none'}, {}, 'at its start none at epoch 1'),
                             ({}, {'lease': 'lost'}, 'at its end lost at epoch 1'),
                             ({}, {'lease_epoch': '2'}, 'at its end held at epoch 2'),
                             ({'lease': 'unsure'}, {'lease_epoch': '2'}, 'at its start unsure at epoch 1')):
            b = self.judge(uploading(sb, sa))
            self.assert_no_verdict(b, 'it did not hold the lease for the whole window', said)
        k = uploading()
        del k['after']['lease']
        self.assert_no_verdict(self.judge(k), 'at its end not read at epoch 1')

    def test_two_daemons_in_the_window(self):
        b = self.judge(uploading(sa={'pid': '5151'}, pid_same=False))
        self.assert_no_verdict(b, "its daemon was not shown to be the same at the window's two ends (pid 4242 and 5151)")
        k = uploading()
        del k['pid_same']
        self.assert_no_verdict(self.judge(k), 'its daemon was not shown to be the same')

    def test_a_status_not_read_at_either_end(self):
        """Codex on #128: a row whose status was not read at an end has
        no proof of an upload, whatever its other figures say."""
        for sb, sa, said in (({}, None, 'end'), (None, {}, 'start'), (None, None, 'start and end')):
            k = uploading()
            if sb is None:
                k['before'] = {}
            if sa is None:
                k['after'] = {}
            self.assert_no_verdict(self.judge(k), f"its daemon's status was not read, or showed no serving daemon, at the window's {said}")
        # As the row generator writes such a row: no `after`, pid_same None.
        k = uploading()
        k['after'], k['pid_same'] = {}, None
        self.assert_no_verdict(self.judge(k), "or showed no serving daemon, at the window's end")
        # As a failed read leaves it (review on #128): the clock line alone.
        k = uploading()
        k['after'], k['pid_same'] = {'read_at_ns': '2000'}, False
        self.assert_no_verdict(self.judge(k), "its daemon's status was not read, or showed no serving daemon, at the window's end")

    def test_one_server_is_read_as_the_verdict_reads_it(self):
        """Review of #128, L1: the tables' "one server throughout" and
        "TWO DAEMONS" read the pids as not_uploading does."""
        self.assertEqual(bk.one_server(uploading()), 'yes')
        self.assertEqual(bk.one_server(uploading(sa={'pid': '5151'}, pid_same=False)), 'NO: the pid changed')
        # an end not read, or read with no pid: not a second daemon
        for before, after in (({}, {}), ({'pid': '4242'}, {}), ({}, {'pid': '4242'}),
                              ({'pid': '4242'}, {'read_at_ns': '2000'}), ({'pid': ''}, {'pid': ''})):
            k = uploading()
            k['before'], k['after'] = before, after
            self.assertEqual(bk.one_server(k), 'status not read', (before, after))
        self.assertEqual(bk.one_server({}), 'status not read')
        # Review of #132, D2: a status read that shows the daemon gone is
        # no serving daemon, not a status not read.
        gone = {'read_at_ns': '2000', 'counters': 'left_by_a_daemon_that_is_gone,_pid_4242'}
        for before, after in (({'pid': '4242'}, gone), (gone, {'pid': '4242'}), (gone, {}), ({}, gone)):
            k = uploading()
            k['before'], k['after'] = before, after
            self.assertEqual(bk.one_server(k), 'no serving daemon at an end', (before, after))

    def test_an_epoch_that_is_not_a_number(self):
        """Review's D1 on #128: an epoch unread at both ends is no one epoch."""
        for epoch in (None, '', 'x'):
            k = uploading()
            for s in (k['before'], k['after']):
                if epoch is None:
                    del s['lease_epoch']
                else:
                    s['lease_epoch'] = epoch
            self.assert_no_verdict(self.judge(k), 'it did not hold the lease for the whole window')

    def test_the_rows_own_reason_comes_before_a_missing_model_row(self):
        rows = [row(1, 'zfs-skyzfsd', 950, skyzfsd=uploading(sa={'uploaded': '3'})), row(2, 'zfs-slog', 1000)]
        b = bk.bracketed(rows[0], rows)
        self.assertIsNone(b['ratio'])
        self.assertIn('no upload committed in its window', b['verdict'])
        rows[0]['skyzfsd'] = uploading()
        self.assertIn('no zfs-slog row of this machine and workload before it', bk.bracketed(rows[0], rows)['verdict'])


def seeding(sb=None, sa=None, pid_same=True, from_load='window'):
    """A row whose seed was let go at its first window: the figures of
    run 44462902930's row sw (no lease at the window's start, the seed
    committed 10 to 15 s after the release under epoch 1)."""
    k = uploading({'lease': 'none', 'lease_epoch': '0', 'uploaded': '0'},
                  {'lease_epoch': '1', 'uploaded': '1', 'uploaded_whole': '1'}, pid_same)
    k['before'].update(sb or {}); k['after'].update(sa or {})
    if from_load:
        k['upload']['from_load'] = from_load
    k['released_here'] = True
    return k


class SeedRows(unittest.TestCase):
    """The lead's decision on #132: a row whose seed ran in its window is
    not one of design §9's median; bracketed says so."""

    def test_a_row_with_no_upload_at_its_start_is_a_seed_row(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=seeding()), row(3, 'zfs-slog', 1000)]
        self.assertEqual(bk.bracketed(rows[1], rows)['seed_row'], "no upload had committed at its window's start")
        # whatever its verdict, and whatever its mode
        rows[1]['skyzfsd'] = seeding(from_load=None)
        self.assertIsNotNone(bk.bracketed(rows[1], rows)['seed_row'])
        rows[1]['skyzfsd'] = uploading(sb={'uploaded': '0'})
        b = bk.bracketed(rows[1], rows)
        self.assertEqual((b['verdict'], b['seed_row'] is not None), ('at least 0.90', True))

    def test_steady_rows_are_not(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=uploading()), row(3, 'zfs-slog', 1000)]
        self.assertIsNone(bk.bracketed(rows[1], rows)['seed_row'])
        for k in ({'before': {'uploaded': '0'}, 'after': {}, 'pid_same': True}, None):
            rows[1]['skyzfsd'] = k
            if k is None:
                del rows[1]['skyzfsd']
            self.assertIsNone(bk.bracketed(rows[1], rows)['seed_row'])
        k = uploading()
        del k['before']['uploaded']
        rows[1]['skyzfsd'] = k
        self.assertIsNone(bk.bracketed(rows[1], rows)['seed_row'])


class ASeedInTheWindow(unittest.TestCase):
    """The lead's decision on #132: a row whose seed was let go at its
    first window measures the seed's cost there, so a lease taken in the
    window counts when the seed committed inside it."""

    judge = AnUploadRow.judge
    assert_no_verdict = AnUploadRow.assert_no_verdict

    def test_a_seed_that_committed_inside_its_window_is_judged(self):
        seed = 'seed inside window, lease at release (epoch 1)'
        self.assertEqual(self.judge(seeding())['verdict'], f'at least 0.90 ({seed})')
        self.assertEqual(self.judge(seeding(), tps=850)['verdict'], f'BELOW 0.90 ({seed})')
        self.assertEqual(self.judge(seeding(sa={'lease': 'unsure'}))['verdict'], f'at least 0.90 ({seed})')
        # A lease already held at the start was not taken at the release
        # (review of #132, D1; the lead's option (i)): the plain rule
        # judges it, with no marker.
        for lease in ('held', 'unsure'):
            self.assertEqual(self.judge(seeding(sb={'lease': lease, 'lease_epoch': '1'}))['verdict'], 'at least 0.90')
            self.assertIsNone(bk.seed_in_window(dict(skyzfsd=seeding(sb={'lease': lease, 'lease_epoch': '1'}))))
        for lease in ('lost', 'not read'):
            sb = {'lease': lease} if lease != 'not read' else {}
            k = seeding(sb=sb)
            if lease == 'not read':
                del k['before']['lease']
            self.assertIsNone(bk.seed_in_window(dict(skyzfsd=k)), lease)
        # A seed row in a bracket too wide, or with no model row after it,
        # has no verdict and no marker (review of #132, S17).
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=seeding()), row(3, 'zfs-slog', 1300)]
        b = bk.bracketed(rows[1], rows)
        self.assertTrue(b['verdict'].startswith('NO VERDICT: bracket too wide'), b['verdict'])
        self.assertNotIn('seed inside', b['verdict'])
        self.assertTrue(bk.bracketed(rows[1], rows[:2])['verdict'].startswith('NO VERDICT: no zfs-slog row'))
        # The plain rule still judges a window row that held one lease
        # throughout and uploaded after an earlier commit: no marker.
        k = seeding(sb={'lease': 'held', 'lease_epoch': '1', 'uploaded': '3'}, sa={'uploaded': '27'})
        self.assertEqual(self.judge(k)['verdict'], 'at least 0.90')

    def test_plants_that_must_have_no_verdict(self):
        said = 'it did not hold the lease for the whole window'
        why = "its seed, let go at the window's start, was not shown to commit inside it"
        for sb, sa, words in (
                ({}, {'uploaded_whole': '0'}, (said, why, '0 whole at its end')),       # no whole upload by the end
                ({}, {'uploaded_whole': 'x'}, (said, why)),                               # not read
                ({'uploaded': '1'}, {}, (said, why, 'uploaded 1 at its start')),        # committed before the window
                ({}, {'lease': 'lost'}, (said, why, 'at its end lost at epoch 1')),     # lease lost by the end
                ({}, {'lease': 'none'}, (said, why)),
                ({}, {'lease_epoch': 'x'}, (said, why)),
                ({'lease': 'held', 'lease_epoch': '1'}, {'lease_epoch': '2'}, (said, why))):  # lost and taken again
            self.assert_no_verdict(self.judge(seeding(sb, sa)), *words)
        k = seeding()
        del k['after']['uploaded_whole']
        self.assert_no_verdict(self.judge(k), said, 'not read whole at its end')
        # Two daemons, or an end not read, are said as before.
        self.assert_no_verdict(self.judge(seeding(sa={'pid': '5151'}, pid_same=False)),
                               'its daemon was not shown to be the same')
        k = seeding()
        k['after'] = {'read_at_ns': '2000'}
        self.assert_no_verdict(self.judge(k), "its daemon's status was not read")

    def test_a_later_row_than_the_release_keeps_the_plain_rule(self):
        """Codex on #132: a seed that outlived the window that let it go
        and committed in a later row's window is not measured from its
        release there."""
        for here in (False, None):
            k = seeding()
            k['released_here'] = here
            if here is None:
                del k['released_here']
            b = self.judge(k)
            self.assert_no_verdict(b, 'it did not hold the lease for the whole window',
                                   'its window did not let MinIO go')
            self.assertIsNone(bk.seed_in_window(dict(skyzfsd=k)))

    def test_a_row_not_let_go_at_its_window_keeps_the_plain_rule(self):
        for from_load in (None, 'workload'):
            b = self.judge(seeding(from_load=from_load))
            self.assert_no_verdict(b, 'it did not hold the lease for the whole window (at its start none at epoch 0, '
                                      'at its end held at epoch 1)')
            self.assertNotIn('seed', b['verdict'])


if __name__ == '__main__':
    unittest.main()
