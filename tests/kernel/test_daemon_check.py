#!/usr/bin/env python3
"""Unit tests for daemon_check.py: no kernel needed.

    python3 tests/kernel/test_daemon_check.py
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import daemon_check as dc  # noqa: E402

ALL = {'a', 'b', 'c', 'd', 'e'}


def cases(impl='reference'):
    """As many cases of each kind as the suite declares for this server."""
    out = {}
    for part in dc.C_DECLARED[impl].split(', '):
        word, count = part.split()
        for i in range(int(count)):
            out[f'{word.lower()}_{i}'] = f'{word}: declared {word}: x'
    return out


def as_expected(impl='reference'):
    r = {name: f'{(rest[3:] or [rest[0]])[0]}: seen' for name, (_, *rest) in dc.EXPECTED.items()}
    r.update({name: 'seen' for name in dc.OBSERVED})
    r.update(impl='reference: x', setup='ok')
    r.update(c_run='ok: 26 cases', c_declared=dc.C_DECLARED[impl])
    r.update({f'case_{k}': v for k, v in cases(impl).items()})
    if impl == 'daemon':
        # A pin only the daemon can carry; the tests below set the
        # other one that waits on the stand-in by hand.
        r['pin_no_ublksrvd_entry_from_daemon'] = 'PASS: seen'
    return r


def with_cases(**changed):
    """A run of check c in which every case ended as the suite declares."""
    r = as_expected()
    r.update({f'case_{k}': v for k, v in changed.items()})
    return r


class Cases(unittest.TestCase):
    C = ALL

    def test_cases_that_end_as_declared_pass(self):
        _, differs, errors = dc.judge(with_cases(), self.C)
        self.assertEqual((differs, errors), ([], []))

    def test_a_failed_case_differs(self):
        _, differs, errors = dc.judge(with_cases(weaker_0='FAIL: declared WEAKER: #3 failed'), self.C)
        self.assertEqual(len(differs), 1)
        self.assertEqual(errors, [])

    def test_a_case_that_did_more_or_less_than_declared_differs(self):
        for got in ('PASS', 'NOT_EXERCISED'):
            _, differs, errors = dc.judge(with_cases(weaker_0=f'{got}: declared WEAKER: '), self.C)
            self.assertEqual(len(differs), 1, got)
            self.assertEqual(errors, [])

    def test_what_the_harness_said_it_cannot_do_is_a_rig_error(self):
        _, differs, errors = dc.judge(with_cases(weaker_0='UNSUPPORTED: declared WEAKER: a power cut'), self.C)
        self.assertEqual(differs, [])
        self.assertEqual(len(errors), 1)

    def test_a_suite_that_declares_otherwise_differs(self):
        # the suite now declares one more case passed in full, and it does
        r = with_cases(weaker_0='PASS: declared PASS: x')
        r['c_declared'] = 'PASS 2, WEAKER 14, NOT_EXERCISED 10'
        _, differs, errors = dc.judge(r, self.C)
        self.assertEqual(len(differs), 1)
        self.assertEqual(errors, [])

    def test_a_suite_that_did_not_run_is_a_rig_error(self):
        r = with_cases()
        r['c_run'] = 'ERROR: skyzfs-blkcheck exit 124 after 3 cases'
        _, differs, errors = dc.judge(r, self.C)
        self.assertEqual(differs, [])
        self.assertEqual(len(errors), 1)
        r = {k: v for k, v in as_expected().items() if not k.startswith(('c_', 'case_'))}
        _, _, errors = dc.judge(r, self.C)
        self.assertTrue(errors)

    def test_the_daemon_is_held_to_its_own_counts(self):
        r = as_expected('daemon')
        r['ctl_recovered_log_too_short'] = 'FAIL: refused'
        self.assertEqual(dc.judge(r, self.C, impl='daemon')[1:], ([], []))
        # the stand-in's counts, from a suite that was told it can reopen a failed volume
        r = as_expected('reference')
        r['ctl_recovered_log_too_short'] = 'FAIL: refused'
        _, differs, errors = dc.judge(r, self.C, impl='daemon')
        self.assertEqual(len(differs), 1)
        self.assertEqual(errors, [])

    def test_fewer_cases_than_the_suite_declared_is_a_rig_error(self):
        r = with_cases()
        del r['case_weaker_3']
        _, differs, errors = dc.judge(r, self.C)
        self.assertEqual(differs, [])
        self.assertEqual(len(errors), 1)
        self.assertIn('25 cases', errors[0])

    def test_check_c_is_asked_for_only_when_it_ran(self):
        r = {k: v for k, v in as_expected().items() if not k.startswith(('c_', 'case_', 'pin_c_'))}
        _, differs, errors = dc.judge(r, ALL - {'c'})
        self.assertEqual((differs, errors), ([], []))


class Judge(unittest.TestCase):
    def test_a_run_equal_to_the_table_passes(self):
        _, differs, errors = dc.judge(as_expected(), ALL)
        self.assertEqual((differs, errors), ([], []))

    def test_a_failed_pin_differs(self):
        r = as_expected()
        r['pin_kill_under_load'] = 'FAIL: 2 write errors'
        _, differs, errors = dc.judge(r, ALL)
        self.assertEqual(len(differs), 1)
        self.assertIn('pin_kill_under_load', differs[0])
        self.assertEqual(errors, [])

    def test_a_control_that_passes_differs(self):
        # A wrong server that the check did not catch: the check cannot fail.
        for name in ('ctl_kill_without_reissue', 'ctl_volume_that_does_not_fail', 'ctl_hold_too_short'):
            r = as_expected()
            r[name] = 'PASS: nothing went wrong'
            _, differs, errors = dc.judge(r, ALL)
            self.assertEqual(len(differs), 1, name)
            self.assertEqual(errors, [])

    def test_a_rig_error_is_not_a_verdict(self):
        r = as_expected()
        r['pin_kill_under_load'] = 'ERROR: zpool create failed'
        r['ctl_kill_without_reissue'] = 'ERROR: zpool create failed'
        _, differs, errors = dc.judge(r, ALL)
        self.assertEqual(differs, [])
        self.assertEqual(len(errors), 2)

    def test_a_missing_line_is_a_rig_error(self):
        for name in list(dc.EXPECTED) + list(dc.OBSERVED):
            r = as_expected()
            del r[name]
            _, differs, errors = dc.judge(r, ALL)
            self.assertEqual(differs, [], name)
            self.assertEqual(len(errors), 1, name)

    def test_an_unknown_word_is_a_rig_error(self):
        r = as_expected()
        r['pin_discard'] = 'MAYBE: ?'
        _, differs, errors = dc.judge(r, ALL)
        self.assertEqual((differs, len(errors)), ([], 1))

    def test_only_the_checks_that_ran_are_asked_for(self):
        r = {k: v for k, v in as_expected().items() if dc.EXPECTED.get(k, dc.OBSERVED.get(k, ('',)))[0] in ('a', '')}
        _, differs, errors = dc.judge(r, {'a'})
        self.assertEqual((differs, errors), ([], []))
        _, _, errors = dc.judge(r, {'a', 'b'})
        self.assertTrue(errors)

    def test_with_the_daemon_the_kept_device_pin_is_still_asked_for(self):
        r = as_expected('daemon')
        r['ctl_recovered_log_too_short'] = 'FAIL: refused'
        r['pin_clear_resumes_on_returned_device'] = 'ERROR: not run'
        self.assertEqual(len(dc.judge(r, ALL, impl='daemon')[2]), 1)
        r['pin_clear_resumes_on_returned_device'] = 'PASS: resumed'
        _, differs, errors = dc.judge(r, ALL, impl='daemon')
        self.assertEqual((differs, errors), ([], []))
        r['ctl_recovered_log_too_short'] = 'WAITING: no log'
        _, _, errors = dc.judge(r, ALL, impl='daemon')
        self.assertEqual(len(errors), 1)

    def test_waiting_is_accepted_only_where_the_table_says_so(self):
        r = as_expected()
        self.assertEqual(r['ctl_recovered_log_too_short'], 'WAITING: seen')
        _, differs, errors = dc.judge(r, ALL)
        self.assertEqual((differs, errors), ([], []))
        # a pin the stand-in can carry may not be left waiting
        r['pin_kill_under_load'] = 'WAITING: not run'
        _, differs, errors = dc.judge(r, ALL)
        self.assertEqual((differs, len(errors)), ([], 1))
        # with the daemon, waiting is over: the control must have run and failed
        r = as_expected('daemon')
        _, differs, errors = dc.judge(r, ALL, impl='daemon')
        self.assertEqual((differs, len(errors)), ([], 1))
        r['ctl_recovered_log_too_short'] = 'FAIL: refused with status 4, pool suspended'
        self.assertEqual(dc.judge(r, ALL, impl='daemon')[1:], ([], []))
        # and a pin that waits for the daemon may not wait on the daemon
        r['pin_no_ublksrvd_entry_from_daemon'] = 'WAITING: the stand-in'
        self.assertEqual(len(dc.judge(r, ALL, impl='daemon')[2]), 1)
        r['ctl_recovered_log_too_short'] = 'PASS: served'
        self.assertEqual(len(dc.judge(r, ALL, impl='daemon')[1]), 1)

    def test_an_observation_that_did_not_run_is_a_rig_error(self):
        for name in dc.OBSERVED:
            r = as_expected()
            r[name] = 'not run: the serve did not start'
            _, differs, errors = dc.judge(r, ALL)
            self.assertEqual((differs, len(errors)), ([], 1), name)

    def test_a_pin_the_table_does_not_know_is_a_rig_error(self):
        for name, value in (('pin_new_thing', 'PASS: fine'), ('ctl_new_thing', 'FAIL: caught'), ('pin_new_thing', 'FAIL: x')):
            r = as_expected()
            r[name] = value
            _, differs, errors = dc.judge(r, ALL)
            self.assertEqual((differs, len(errors)), ([], 1), name)
        # a line that is neither a pin nor a control is context, as before
        r = as_expected()
        r['b_rounds'] = 'kill 1: ...'
        self.assertEqual(dc.judge(r, ALL)[1:], ([], []))

    def test_a_run_with_nothing_to_judge_is_a_rig_error(self):
        _, differs, errors = dc.judge({'impl': 'reference: x', 'setup': 'ok'}, set())
        self.assertEqual(differs, [])
        self.assertEqual(len(errors), 1)

    def test_every_pin_says_what_it_rests_on(self):
        for name, (check, want, basis, claim, *_) in dc.EXPECTED.items():
            self.assertIn(check, ALL, name)
            self.assertIn(basis, (dc.KERNEL, dc.SERVER, dc.WAITING), name)
            self.assertEqual(want, 'FAIL' if name.startswith('ctl_') else 'PASS', name)
            self.assertTrue(claim, name)


class Main(unittest.TestCase):
    def test_exit_statuses(self):
        import contextlib
        import io
        import tempfile
        for change, want in ((None, 0), (('pin_discard', 'FAIL: 0'), 1), (('pin_discard', 'ERROR: x'), 2), (('setup', 'FAILED'), 2)):
            r = as_expected()
            if change:
                r[change[0]] = change[1]
            with tempfile.TemporaryDirectory() as d:
                with open(os.path.join(d, 'results.tsv'), 'w', encoding='utf-8') as f:
                    f.writelines(f'{k}\t{v}\n' for k, v in r.items())
                with contextlib.redirect_stdout(io.StringIO()) as out:
                    self.assertEqual(dc.main([d]), want, change)
                self.assertIn('pin_discard', out.getvalue())
        with tempfile.TemporaryDirectory() as d, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(dc.main([d]), 2)

    def run_main(self, lines, *args):
        import contextlib
        import io
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, 'results.tsv'), 'w', encoding='utf-8') as f:
                f.writelines(lines)
            with contextlib.redirect_stdout(io.StringIO()) as out:
                return dc.main([d, *args]), out.getvalue()

    def test_checks_that_name_nothing_are_refused(self):
        lines = [f'{k}\t{v}\n' for k, v in as_expected().items()]
        self.assertEqual(self.run_main(lines)[0], 0)
        self.assertEqual(self.run_main(lines, '--checks', 'a b c d e')[0], 0)
        for asked in (' ', '', 'abcde', 'a b x', 'a,b'):
            status, text = self.run_main(lines, '--checks', asked)
            self.assertEqual(status, 2, repr(asked))
            self.assertIn('checks:', text)
        # the rig's own view of the same input: it ran nothing and wrote only context
        status, _ = self.run_main(['impl\treference: x\n', 'setup\tok\n'], '--checks', ' ')
        self.assertEqual(status, 2)

    def test_a_pin_written_twice_with_two_results_is_a_rig_error(self):
        lines = [f'{k}\t{v}\n' for k, v in as_expected().items()]
        status, text = self.run_main(['pin_discard\tFAIL: 0\n'] + lines)
        self.assertEqual(status, 2)
        self.assertIn('written twice', text)
        # the same result twice is the same result
        self.assertEqual(self.run_main(lines + ['pin_discard\tPASS: again\n'])[0], 0)


if __name__ == '__main__':
    unittest.main()
