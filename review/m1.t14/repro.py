"""Reproducers for the review of PR 39 (replay_daemon.py exit codes). Run: python3 repro.py <tests/kernel dir>"""
import errno, os, subprocess, sys, unittest
sys.path.insert(0, sys.argv.pop(1))
import logwrites, replay_check
import replay_daemon as rd
import test_replay_daemon as t


class Repro(t.Pass):
    def run_main(self, a):
        """main()'s own handling, on args already parsed."""
        try:
            return rd.check(a)
        except rd.RigError:
            return 2

    def test_1_end_image_for_zdb_fails_and_the_run_passes(self):
        # The main pass is clean; the second making of the end image (for zdb -bcc) fails at the daemon's start.
        self.fake()
        clean = rd.Checker.point
        seen = []

        def point(ck, n, kind, hooks=(), marks=None, fmarks=None):
            r = clean(ck, n, kind, hooks, marks, fmarks)
            # In this log the last record is a sync mark, so the main pass checks the
            # end as kind `sync`; the only point of kind `end` is the one made for zdb -bcc.
            if kind == 'end':
                seen.append(n)
                r.update(ok=False, failed='start', detail='exit 1 before it was ready')
            return r
        rd.Checker.point = point
        code = rd.check(self.args())
        with open(os.path.join(self.dir.name, 'out', 'replay-summary.md')) as f:
            text = f.read()
        print('\n1: exit', code, '|', [ln for ln in text.splitlines() if 'bcc' in ln or ln.startswith('Exit')])
        self.assertEqual(len(seen), 1)
        self.assertNotEqual(code, 0, 'a start that failed, and no zdb -bcc, and the run passes')

    def test_2_a_rig_error_after_a_failure_hides_the_failure(self):
        records = logwrites.index(self.log, 0)['records']
        fm = replay_check.fsync_marks(records)
        self.fake(fail_at={fm[1][0]})
        failing = rd.Checker.point

        def point(ck, n, kind, hooks=(), marks=None, fmarks=None):
            if n == fm[4][0] and kind != 'calibration':
                raise rd.RigError(f'prefix {n}: cleaning up after the start failed: umount: target is busy')
            return failing(ck, n, kind, hooks, marks, fmarks)
        rd.Checker.point = point
        code = self.run_main(self.args())
        print('\n2: exit', code, '(a prefix failed fsync: files before the rig failed)')
        self.assertEqual(code, 1, 'a verdict already reached is reported as no verdict')

    def test_3_a_hooked_start_that_never_starts_counts_as_caught(self):
        # Every hooked start ends before it serves (say the option is refused): no start line, stage `start`.
        self.fake(recover_caught=False)
        clean = rd.Checker.point

        def point(ck, n, kind, hooks=(), marks=None, fmarks=None):
            r = clean(ck, n, kind, hooks, marks, fmarks)
            if hooks:
                r.pop('start')
                r.update(ok=False, failed='start', detail="exit 2 before it was ready: error: unexpected argument '--recover-short'")
            return r
        rd.Checker.point = point
        code = rd.check(self.args(recover_short=8))
        with open(os.path.join(self.dir.name, 'out', 'replay-results.json')) as f:
            import json
            rows = [r for r in json.load(f)['control_rows'] if r['name'] == 'recover-short']
        print('\n3: exit', code, '|', rows)
        self.assertNotEqual(code, 0, 'the control is "caught" by a start that refused the option')


if __name__ == '__main__':
    unittest.main(verbosity=1)
