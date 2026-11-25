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
from test_cut_check import assert_pick_wiring  # noqa: E402

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
        # A later group's uberblock before the instant (a rewind's
        # abandoned history) is not after it.
        ubs = [{'record': 3, 'txg': 20}, {'record': 14, 'txg': 10}]
        self.assertEqual(nodeb_check.later_uberblock(ubs, c), 14)
        self.assertIsNone(nodeb_check.later_uberblock(ubs[:1], c))

    def test_live_block(self):
        bps = [{'type': 'DMU dnode', 'level': 0, 'dvas': [(0, 0x1000, 0x1000)]},
               {'type': 'ZFS plain file', 'level': 1, 'dvas': [(0, 0x2000, 0x1000)]},
               {'type': 'ZFS plain file', 'level': 0, 'dvas': [(0, 0x3000, 0x2000), (0, 0x9000, 0x2000)]}]
        self.assertEqual(nodeb_check.live_block(bps), (0x3000 + 0x400000, 0x2000))
        self.assertIsNone(nodeb_check.live_block(bps[:2]))


class Compare(unittest.TestCase):
    A, B = '0' * 64, '1' * 64

    def test_the_same_files(self):
        ok, detail = nodeb_check.compare({'f1': self.A, 'f2': self.B}, {'f2': self.B, 'f1': self.A})
        self.assertTrue(ok, detail)

    def test_each_difference(self):
        want = {'f1': self.A, 'f2': self.B}
        for got, part in (({'f1': self.A}, '1 missing'), (dict(want, f3=self.A), '1 extra'),
                          ({'f1': self.A, 'f2': self.A}, '1 differ')):
            ok, detail = nodeb_check.compare(want, got)
            self.assertFalse(ok)
            self.assertIn(part, detail)


class Verdicts(unittest.TestCase):
    def seen(self, *stages):
        s = nodeb_check.Seen()
        for st, ok in stages:
            s.put(st, ok, f'{st}\nsaid\tthis')
        return s

    def test_the_first_stage_that_failed(self):
        s = self.seen(('refs', True), ('content', True), ('import', True), ('snapshot', False), ('copies', True))
        self.assertEqual(s.failed(), 'snapshot')
        self.assertEqual(nodeb_check.control_verdict(s, ('snapshot',)), ('caught at snapshot', True, []))
        self.assertEqual(nodeb_check.control_verdict(s, ('files',)), ('caught at snapshot', False, []))
        self.assertNotIn('\n', s.text())
        self.assertNotIn('\t', s.text())
        self.assertTrue(s.ok('copies'))
        self.assertFalse(s.ok('files'))

    def test_order_is_the_stages_not_the_calls(self):
        s = self.seen(('copies', False), ('import', False))
        self.assertEqual(s.failed(), 'import')

    def test_missed(self):
        s = self.seen(('refs', True), ('import', True), ('copies', True))
        self.assertEqual(nodeb_check.control_verdict(s, ('import',)), ('missed', False, []))

    def test_early_must_be_otherwise_clean(self):
        """Caught at the snapshot, with the copies failed: not what early
        is for (review of #97, N3)."""
        clean = nodeb_check.CLEAN['early']
        want = nodeb_check.EXPECTED['early']
        s = self.seen(('refs', True), ('content', True), ('import', True), ('snapshot', False), ('copies', True))
        self.assertEqual(nodeb_check.control_verdict(s, want, clean), ('caught at snapshot', True, []))
        s = self.seen(('refs', True), ('content', True), ('import', True), ('snapshot', False), ('copies', False))
        self.assertEqual(nodeb_check.control_verdict(s, want, clean), ('caught at snapshot', False, ['copies']))
        # a stage that never ran is not clean either
        s = self.seen(('refs', True), ('content', True), ('import', True), ('snapshot', False))
        self.assertEqual(nodeb_check.control_verdict(s, want, clean), ('caught at snapshot', False, ['copies']))

    def test_flat(self):
        self.assertEqual(nodeb_check.flat("cannot import 'p':\n\tI/O error\n"), "cannot import 'p': I/O error")


class VerifyPaths(unittest.TestCase):
    """verify's every-copy branch, with run, the tool and the import
    stubbed: which errors of the rig are catches (review of #97, N1, N2)."""

    def node(self, import_ok, zdb_failed):
        nb = nodeb_check.NodeB.__new__(nodeb_check.NodeB)
        nb.img = nb.image = '/nonexistent/nodeb-test'
        nb.tool_run = lambda args, timeout=600: (0, 'snap/t\tc0\t0\nhead\tc1\t1\n' if args[0] == 'refs'
                                                 else 'read\tsnap/t\tc0\t512\n')

        def files(snap, want, seen):
            seen.put('import', import_ok, 'stub')
            if import_ok:
                seen.put('snapshot', True, 'stub')
                seen.put('mount', True, 'stub')
                seen.put('files', True, 'stub')
        nb.snapshot_files = files
        self._saved = (nodeb_check.run, nodeb_check.replay_check.every_copy_verdict)
        nodeb_check.run = lambda cmd, timeout=None: (0, '', '')
        nodeb_check.replay_check.every_copy_verdict = lambda rc, text, image: (
            (zdb_failed, 'said', {}) if zdb_failed else (None, '', {'blocks': 1, 'copies': 2}))
        return nb

    def tearDown(self):
        if hasattr(self, '_saved'):
            nodeb_check.run, nodeb_check.replay_check.every_copy_verdict = self._saved

    def test_an_abort_on_a_control_is_a_catch(self):
        seen = self.node(True, nodeb_check.ZDB_ABORT).verify('v', 'snap/t', 's', {}, damaged=True)
        self.assertEqual(seen.failed(), 'copies')

    def test_a_timeout_on_a_control_is_the_rigs(self):
        for failed in ('rig: zdb timed out', 'rig: the list is not complete'):
            with self.assertRaises(RigError):
                self.node(True, failed).verify('v', 'snap/t', 's', {}, damaged=True)
            self.tearDown()

    def test_an_abort_on_the_published_cut_is_the_rigs(self):
        with self.assertRaises(RigError):
            self.node(True, nodeb_check.ZDB_ABORT).verify('v', 'snap/t', 's', {}, damaged=False)

    def test_a_published_cut_that_failed_keeps_its_verdict(self):
        """It failed its import, and zdb then aborts: the cut's FAIL, at
        the import, not the rig's error (N2)."""
        seen = self.node(False, nodeb_check.ZDB_ABORT).verify('v', 'snap/t', 's', {}, damaged=False)
        self.assertEqual(seen.failed(), 'import')
        self.assertIn('not judged', seen.text())

    def test_a_clean_pass(self):
        seen = self.node(True, None).verify('v', 'snap/t', 's', {}, damaged=False)
        self.assertIsNone(seen.failed())
        self.assertEqual(list(seen.stages), ['refs', 'content', 'import', 'snapshot', 'mount', 'files', 'copies'])


class SnapshotPaths(unittest.TestCase):
    """snapshot_files with run stubbed: a zfs list or a mount that does
    not come to an end is the rig's error, not a catch (review of #97,
    the look at 0f2f8c7)."""

    def calls(self, answers):
        nb = nodeb_check.NodeB.__new__(nodeb_check.NodeB)
        nb.image, nb.alt, nb.mnt, nb.logdev = '/nonexistent/img', '/nonexistent/alt', '/nonexistent/mnt', None
        saved = nodeb_check.run

        def run(cmd, timeout=None):
            key = cmd[0] if cmd[0] not in ('zpool', 'zfs') else f'{cmd[0]} {cmd[1]}'
            return answers.get(key, (0, '', ''))
        nodeb_check.run = run
        self.addCleanup(setattr, nodeb_check, 'run', saved)
        answers.setdefault('zpool list', (1, 'no such pool', ''))
        answers.setdefault('losetup', (0, '/dev/loop9\n', ''))
        seen = nodeb_check.Seen()
        return nb, seen

    def test_a_list_that_times_out_is_the_rigs(self):
        nb, seen = self.calls({'zfs list': (124, '', '')})
        with self.assertRaises(RigError):
            nb.snapshot_files('s24', {}, seen)

    def test_a_list_that_fails_is_the_rigs(self):
        nb, seen = self.calls({'zfs list': (1, 'cannot open', '')})
        with self.assertRaises(RigError):
            nb.snapshot_files('s24', {}, seen)

    def test_an_empty_list_is_a_missing_snapshot(self):
        nb, seen = self.calls({'zfs list': (0, '', '')})
        nb.snapshot_files('s24', {}, seen)
        self.assertEqual(seen.failed(), 'snapshot')

    def test_a_mount_that_times_out_is_the_rigs(self):
        nb, seen = self.calls({'zfs list': (0, 'skzcut@s24\n', ''), 'mount': (124, '', '')})
        with self.assertRaises(RigError):
            nb.snapshot_files('s24', {}, seen)

    def test_a_mount_that_fails_is_a_stage(self):
        nb, seen = self.calls({'zfs list': (0, 'skzcut@s24\n', ''), 'mount': (32, 'I/O error', '')})
        nb.snapshot_files('s24', {}, seen)
        self.assertEqual(seen.failed(), 'mount')


class Expected(unittest.TestCase):
    def test_every_control_has_its_stages(self):
        self.assertEqual(set(nodeb_check.EXPECTED), set(nodeb_check.CONTROLS))
        for ctl, stages in nodeb_check.EXPECTED.items():
            self.assertTrue(stages and set(stages) <= set(nodeb_check.STAGES), ctl)
            # A store or a ref that fails is never a control's catch.
            self.assertFalse({'refs', 'content'} & set(stages), ctl)
        # Every row, exactly (review of #97, N5).
        damage = ('import', 'mount', 'files', 'copies')
        self.assertEqual(nodeb_check.EXPECTED, {'digest': ('files',), 'early': ('snapshot',), 'lacks': damage,
                                                'later': damage, 'hole': damage})
        self.assertEqual(nodeb_check.CLEAN, {'early': ('import', 'copies')})
        self.assertEqual(nodeb_check.ZDB_ABORT, 'rig: zdb died with signal 6')


class PickWiring(unittest.TestCase):
    def test_lacks_picks_through_withheld_pick_and_names_what_it_passed_over(self):
        """Review of #139, W6 and W7: node B's lacks takes the cut rig's
        pick, and its line names the writes passed over."""
        assert_pick_wiring(self, nodeb_check.main, 'lacks', 'cut')


if __name__ == '__main__':
    unittest.main()
