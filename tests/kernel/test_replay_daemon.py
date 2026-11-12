#!/usr/bin/env python3
"""Unit tests for the replay rig through skyzfsd (replay_daemon.py). No kernel needed.

    python3 tests/kernel/test_replay_daemon.py
"""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import logwrites  # noqa: E402
import replay_check  # noqa: E402
import replay_daemon as rd  # noqa: E402

SS = 4096


def blk(byte, n=1):
    return bytes([byte]) * (SS * n)


def lower_log(files=6):
    """A lower disk's log as the daemon's stack writes it: a few writes,
    then per file its writes, a flush and its fsync mark, and a sync mark
    after every second file."""
    out = [('write', 0, blk(0x01)), ('write', SS, blk(0x02)), ('flush',)]
    for n in range(files):
        out += [('write', (16 + 4 * n) * SS, blk(0x20 + n, 2)), ('write', (100 + n) * SS, blk(0x60 + n)),
                ('flush',), ('mark', f'fsync-{n}')]
        if n % 2 == 1:
            out += [('write', (200 + n) * SS, blk(0x90 + n)), ('flush',), ('mark', f'sync-{n // 2 + 1}-{n + 1}')]
    return out


class Pure(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.log = os.path.join(self.dir.name, 'log')
        logwrites.write_log(self.log, SS, lower_log())
        self.idx = logwrites.index(self.log, 0)
        self.records = self.idx['records']

    def tearDown(self):
        self.dir.cleanup()

    def test_txg_from_zdb(self):
        text = 'Uberblock:\n\tmagic = 0000000000bab10c\n\tversion = 5000\n\ttxg = 4711\n\tguid_sum = 1\n'
        self.assertEqual(rd.txg_of(text), 4711)
        self.assertIsNone(rd.txg_of('zdb: cannot open skzreplay\n'))
        # A line that only contains the word is not the field.
        self.assertIsNone(rd.txg_of('\tlast txg = 12 written\n'))

    def test_ready_line(self):
        self.assertEqual(rd.ready_device('ready /dev/ublkb3\n'), ('/dev/ublkb3', 3))
        self.assertIsNone(rd.ready_device('starting\n'))
        self.assertIsNone(rd.ready_device('ready /dev/sda\n'))

    def test_status_keys(self):
        text = 'state serving\nreplayed_bytes 1048576\nlog_live 4096\ncheckpoint_steps 3\nother 9\n'
        self.assertEqual(rd.status_values(text), {'replayed_bytes': 1048576, 'log_live': 4096, 'checkpoint_steps': 3})

    def test_plan_takes_the_base_every_mark_and_the_end(self):
        kind, rest, counts = rd.prefix_plan(self.records)
        total = len(self.records)
        first_flush = replay_check.flush_points(self.records)[0]
        self.assertEqual([n for n in sorted(kind) if kind[n] == 'base'], list(range(first_flush + 1)))
        for pos, _ in replay_check.fsync_marks(self.records):
            self.assertEqual(kind[pos], 'fsync')
        for pos, _, _ in replay_check.parse_marks(self.records):
            self.assertEqual(kind[pos], 'sync')
        self.assertIn(total, kind)
        self.assertTrue(rest)
        self.assertFalse(set(rest) & set(kind))
        self.assertTrue(set(rest) <= set(replay_check.flush_points(self.records)))
        self.assertEqual(counts['fsync'], 6)
        self.assertEqual(counts['sync'], 3)

    def test_the_base_is_sampled_evenly_when_long(self):
        log = os.path.join(self.dir.name, 'long')
        logwrites.write_log(log, SS, [('write', k * SS, blk(k % 250 + 1)) for k in range(40)] + lower_log()[3:])
        records = logwrites.index(log, 0)['records']
        kind, _, counts = rd.prefix_plan(records, base_max=8)
        base = sorted(n for n in kind if kind[n] == 'base')
        self.assertEqual(len(base), 8)
        self.assertEqual(base[0], 0)
        self.assertEqual(base[-1], replay_check.flush_points(records)[0])
        self.assertEqual(counts['base'], replay_check.flush_points(records)[0] + 1)

    def test_between_points_are_not_flush_boundaries(self):
        kind, _, _ = rd.prefix_plan(self.records)
        pts = rd.between_points(self.records, kind, 5, seed=3)
        self.assertEqual(pts, rd.between_points(self.records, kind, 5, seed=3))
        self.assertEqual(len(pts), 5)
        flush = set(replay_check.flush_points(self.records))
        self.assertFalse(set(pts) & (flush | set(kind)))
        self.assertEqual(rd.between_points(self.records, kind, 0), [])

    def test_withheld_moves_the_writes_of_one_fsync_past_its_mark(self):
        w = rd.withheld_schedule(self.records)
        self.assertIsNotNone(w)
        sched = w['schedule']
        self.assertEqual(sorted(sched), list(range(len(self.records))))
        srec = [self.records[i] for i in sched]
        # In the schedule's order the mark ends prefix w['mark'], and the
        # file it promises is the one whose writes were withheld.
        self.assertEqual(srec[w['mark'] - 1]['mark'], f"fsync-{w['file']}")
        self.assertIn(w['mark'], w['points'])
        fm = replay_check.fsync_marks(srec)
        self.assertEqual(replay_check.fsynced_at(fm, w['mark']), w['file'] + 1)
        # No sync mark between the two fsync marks: the sync promise is not what catches it.
        true_fm = replay_check.fsync_marks(self.records)
        k = [n for _, n in true_fm].index(w['file'] + 1)
        p0, p1 = true_fm[k - 1][0], true_fm[k][0]
        self.assertFalse([p for p, _, _ in replay_check.parse_marks(self.records) if p0 < p < p1])
        # The image at the mark lacks the file's blocks, and has them once the withheld writes follow.
        img = os.path.join(self.dir.name, 'img')
        with open(img, 'wb') as f:
            f.truncate(1 << 20)
        logwrites.advance(self.log, self.idx, img, 0, w['mark'], sched)
        n = w['file']
        with open(img, 'rb') as f:
            f.seek((16 + 4 * n) * SS)
            self.assertEqual(f.read(SS), bytes(SS))
        logwrites.advance(self.log, self.idx, img, w['mark'], w['mark'] + w['victims'], sched)
        with open(img, 'rb') as f:
            f.seek((16 + 4 * n) * SS)
            self.assertEqual(f.read(SS), blk(0x20 + n))

    def test_withheld_needs_two_fsync_marks_without_a_sync_between(self):
        log = os.path.join(self.dir.name, 'synced')
        logwrites.write_log(log, SS, [('write', 0, blk(1)), ('flush',), ('mark', 'fsync-0'), ('write', SS, blk(2)),
                                      ('flush',), ('mark', 'sync-1-1'), ('write', 2 * SS, blk(3)), ('flush',),
                                      ('mark', 'fsync-1')])
        self.assertIsNone(rd.withheld_schedule(logwrites.index(log, 0)['records']))

    def test_txg_points_are_the_first_and_last_of_each_group(self):
        pts = [{'n': n, 'txg': t} for n, t in ((1, 5), (4, 5), (9, 5), (12, 6), (15, None), (20, 6), (22, 7))]
        self.assertEqual(rd.txg_points(pts), {1, 9, 12, 20, 22})
        self.assertEqual(rd.txg_points([{'n': 3, 'txg': 8}]), {3})
        self.assertEqual(rd.txg_points([]), set())

    def test_a_refused_start_is_a_verdict_and_a_loop_device_is_not(self):
        res = [{'n': 1, 'ok': True},
               {'n': 2, 'ok': False, 'failed': 'start', 'detail': 'exit 1'},
               {'n': 3, 'ok': False, 'failed': 'rig: losetup', 'detail': 'busy'},
               {'n': 4, 'ok': False, 'failed': 'fsync: files', 'detail': 'f.3 missing'}]
        by, rig_errors, first = rd.tally(res)
        self.assertEqual(by, {'start': 1, 'fsync': 1})
        self.assertEqual([e['n'] for e in rig_errors], [3])
        self.assertEqual(first['n'], 2)

    def test_exit_codes(self):
        self.assertEqual(rd.verdict([], [], True, [], True, 0), (0, []))
        code, why = rd.verdict([{'n': 1}], [{'n': 2, 'failed': 'rig: losetup'}], True, [], True, 0)
        self.assertEqual(code, 1)
        self.assertEqual(len(why), 2)
        self.assertEqual(rd.verdict([], [], False, [], True, 0)[0], 1)
        self.assertEqual(rd.verdict([], [], True, [], True, 3)[0], 1)
        self.assertEqual(rd.verdict([], [{'n': 2, 'failed': 'rig: losetup'}], True, [], True, 0)[0], 2)
        self.assertEqual(rd.verdict([], [], True, ['a prefix was not checked'], True, 0)[0], 2)
        self.assertEqual(rd.verdict([], [], True, [], False, 0)[0], 2)
        # No final image to check is not by itself a verdict.
        self.assertEqual(rd.verdict([], [], True, [], True, None)[0], 0)

    def test_controls_against_the_table(self):
        def ctl(name, caught, by=None):
            return {'name': name, 'caught': caught, 'failed_by': by or {}, 'rig_errors': []}
        rows, _, ok = replay_check.judge_controls([ctl('recover-short', True, {'fsync': 2}), ctl('withheld', True, {'start': 1})],
                                                  expected=rd.EXPECTED, absent=rd.ABSENT)
        self.assertTrue(ok)
        self.assertEqual(sorted(r['name'] for r in rows if r.get('not_built')), ['data', 'drop', 'intent', 'reorder'])
        _, _, ok = replay_check.judge_controls([ctl('recover-short', False), ctl('withheld', True, {'fsync': 1})],
                                               expected=rd.EXPECTED, absent=rd.ABSENT)
        self.assertFalse(ok)
        # A control of the table that did not run fails, unless it is said why it was not built.
        _, _, ok = replay_check.judge_controls([ctl('recover-short', True, {'fsync': 1})], expected=rd.EXPECTED,
                                               absent=rd.ABSENT)
        self.assertFalse(ok)

    def test_the_provider_stages_are_verdicts(self):
        for s in ('volume mount', 'start', 'device read', 'stop', 'import', 'fsync: files', 'copies: differ'):
            self.assertIn(s, rd.JUDGING)
        self.assertNotIn('rig: losetup', rd.JUDGING)


class Pass(unittest.TestCase):
    """check() end to end, with the kernel's part faked: the image's txg is the number of sync marks before it."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        d = self.dir.name
        self.log = os.path.join(d, 'log')
        logwrites.write_log(self.log, SS, lower_log())
        self.base = os.path.join(d, 'base')
        with open(self.base, 'wb') as f:
            f.truncate(1 << 20)
        self.manifest = os.path.join(d, 'manifest')
        with open(self.manifest, 'w') as f:
            f.writelines(f'{n:064x} f.{n}\n' for n in range(6))
        self.saved = (rd.Checker.point, rd.Checker.deep, rd.run, rd.Daemon.make)

    def tearDown(self):
        rd.Checker.point, rd.Checker.deep, rd.run, rd.Daemon.make = self.saved
        self.dir.cleanup()

    def args(self, **kw):
        d = self.dir.name
        a = dict(log=self.log, base=self.base, manifest=self.manifest, pool='p', mountpoint='/p', work=os.path.join(d, 'w'),
                 out=os.path.join(d, 'out'), skyzfsd='skyzfsd', ublk='ublk', dd='dd', volume='vol', serve_arg=None,
                 dev_bytes=1 << 20, budget_s=1000, budget_name='test', between_budget_s=0, recover_short=1,
                 recover_points=3, steps='4', seed=1, replay_log='', require_witness=False, discard='zero')
        a.update(kw)
        os.makedirs(a['work'], exist_ok=True)
        os.makedirs(a['out'], exist_ok=True)
        return type('A', (), a)()

    def fake(self, fail_at=(), recover_caught=True):
        deep_at = []

        def point(ck, n, kind, hooks=(), marks=None, fmarks=None):
            ck.daemon.advance(n)
            txg = sum(1 for p, _, _ in (marks if marks is not None else ck.marks) if p <= n)
            r = {'n': n, 'kind': kind, 'files': 0, 'dir': 'x', 't_make': 1.0, 't_light': 0.5, 't_txg': 0.1, 'txg': txg,
                 'ok': True}
            if n in fail_at or (hooks and recover_caught):
                r.update(ok=False, failed='fsync: files', detail='f.1 checksum differs')
            return r

        def deep(ck, p, marks=None, fmarks=None):
            p['deep'], p['t_deep'], p['t_copies'] = True, 1.0, 1.0
            deep_at.append(p['n'])
            return p

        real = rd.run

        def run(cmd, timeout=300):
            return real(cmd, timeout) if cmd[0] == 'cp' else (0, '', 0.0)
        rd.Checker.point, rd.Checker.deep, rd.run = point, deep, run
        return deep_at

    def test_a_clean_pass(self):
        deep_at = self.fake()
        code = rd.check(self.args())
        with open(os.path.join(self.dir.name, 'out', 'replay-summary.md')) as f:
            text = f.read()
        self.assertIn('every checked prefix passed', text)
        # The withheld control is not caught in this fake (its points pass), so the table differs.
        self.assertEqual(code, 1)
        self.assertIn('withheld', text)
        records = logwrites.index(self.log, 0)['records']
        marks = replay_check.parse_marks(records)
        # Deep checks at the first and last checked prefix of each txg.
        self.assertTrue(deep_at)
        self.assertIn(0, deep_at)
        self.assertIn(len(records), deep_at)
        for pos, _, _ in marks:
            self.assertIn(pos, deep_at)              # the first prefix of the group a sync mark begins

    def test_a_failed_prefix_is_a_verdict(self):
        records = logwrites.index(self.log, 0)['records']
        fm = replay_check.fsync_marks(records)
        self.fake(fail_at={fm[2][0]})
        code = rd.check(self.args())
        self.assertEqual(code, 1)
        with open(os.path.join(self.dir.name, 'out', 'replay-summary.md')) as f:
            self.assertIn('prefixes FAILED', f.read())


if __name__ == '__main__':
    unittest.main()
