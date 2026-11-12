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

    def test_a_recorded_row_has_no_verdict(self):
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, perf_record={'threads': {}}),
                row(3, 'zfs-slog', 1000)]
        b = bk.bracketed(rows[1], rows)
        self.assertIsNone(b['ratio'])
        self.assertIn('NO VERDICT: the daemon was recorded by perf', b['verdict'])


if __name__ == '__main__':
    unittest.main()
