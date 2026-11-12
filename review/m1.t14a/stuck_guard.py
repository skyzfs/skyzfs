"""The stuck control's guard, on the pull request's own Pass harness. Run:
    python3 review/m1.t14a/stuck_guard.py <tests/kernel dir of the head>

Two starts of the stuck control that never reach the time limit:
  1. the daemon is stopped and the read fails at once with an I/O error;
  2. the daemon is stopped and the read fails at the limit, but the reader is
     not reaped (the mechanism the control is there to show did not work).
Prints the exit code and the control's row for each.
"""
import json, os, sys, unittest
sys.path.insert(0, sys.argv.pop(1))
import replay_daemon as rd
import test_replay_daemon as t


class Guard(t.Pass):
    def stuck_with(self, **fields):
        self.fake()
        clean = rd.Checker.point

        def point(ck, n, kind, hooks=(), marks=None, fmarks=None):
            r = clean(ck, n, kind, hooks, marks, fmarks)
            if rd.STOP_AFTER_READY in hooks:
                r.update(fields)
            return r
        rd.Checker.point = point
        code = rd.check(self.args())
        with open(os.path.join(self.dir.name, 'out', 'replay-results.json')) as f:
            res = json.load(f)
        rows = [r for r in res.get('control_rows', []) if r['name'] == 'stuck']
        return code, rows, res.get('stopped')

    def test_1_a_read_that_fails_at_once(self):
        code, rows, _ = self.stuck_with(detail='dd: error reading /dev/ublkb7: Input/output error; the daemon said: ')
        print('\n1: exit', code, '|', [(r['observed'], r['match']) for r in rows])

    def test_2_a_reader_that_is_not_reaped(self):
        code, rows, stopped = self.stuck_with(
            detail='TIMEOUT after 20s; the reader not reaped 60 s after the server was killed',
            abort='dd of /dev/ublkb7 did not end 60 s after the daemon was killed and its device deleted')
        print('\n2: exit', code, '| stopped:', stopped, '|', [(r['observed'], r['match']) for r in rows])


if __name__ == '__main__':
    unittest.main(argv=['x'], verbosity=0)
