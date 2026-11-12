#!/usr/bin/env python3
"""Tests of the control rig's judge. No kernel: python3 tests/kernel/test_control_check.py"""
import os
import re
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import control_check as cc  # noqa: E402


def as_expected():
    r = {name: f'{want}: seen' for name, (want, *_) in cc.EXPECTED.items()}
    r.update({name: 'seen' for name in cc.OBSERVED})
    r.update(setup='ok', kernel='7.0', commit='abc')
    return r


class Judge(unittest.TestCase):
    def test_a_run_as_the_table_has_it_passes(self):
        self.assertEqual(cc.judge(as_expected())[1:], ([], []))

    def test_a_pin_that_fails_differs(self):
        r = as_expected()
        r['pin_add_on_live_stranger_is_taken'] = 'FAIL: refused: errno 22'
        _, differs, errors = cc.judge(r)
        self.assertEqual((len(differs), errors), (1, []))

    def test_a_control_that_passes_differs(self):
        r = as_expected()
        r['ctl_stranger_deleted_between_looks'] = 'PASS: the stranger was as before'
        _, differs, errors = cc.judge(r)
        self.assertEqual((len(differs), errors), (1, []))

    def test_a_line_of_the_crate_that_fails_differs(self):
        r = as_expected()
        r['control_add_taken_delete'] = 'FAIL: the second add was added'
        self.assertEqual(len(cc.judge(r)[1]), 1)

    def test_a_missing_line_is_a_rig_error(self):
        for name in ('control_can_reissue', 'pin_delete_returns_while_held', 'obs_delete_ms'):
            r = as_expected()
            del r[name]
            _, differs, errors = cc.judge(r)
            self.assertEqual((differs, len(errors)), ([], 1), name)

    def test_a_line_the_table_does_not_know_is_a_rig_error(self):
        for name in ('control_a_new_property', 'pin_something', 'ctl_something'):
            r = as_expected()
            r[name] = 'PASS'
            self.assertEqual(len(cc.judge(r)[2]), 1, name)

    def test_a_new_detail_line_of_the_crate_is_shown_and_not_an_error(self):
        r = as_expected()
        r['control_stop_addresses_detail'] = '12 addresses told, 0 overlaps'
        rows, differs, errors = cc.judge(r)
        self.assertEqual((differs, errors), ([], []))
        self.assertIn('control_stop_addresses_detail', cc.summary(r, rows, differs, errors))

    def test_a_detail_line_written_many_times_is_counted_and_not_an_error(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, 'results.tsv')
            with open(path, 'w', encoding='utf-8') as f:
                for k, v in as_expected().items():
                    f.write(f'{k}\t{v}\n')
                for i in range(3):
                    f.write(f'control_stop_under_writers_stop\tstop {i}: 4 addresses told, 0 watched frees\n')
            r, twice = cc.read_results(path)
            self.assertEqual(twice, [])
            self.assertTrue(r['control_stop_under_writers_stop'].startswith('(3 lines; the last:) stop 2'))
            self.assertEqual(cc.judge(r)[1:], ([], []))

    def test_a_detail_name_does_not_hide_a_verdict(self):
        for value in ('PASS: all well', 'FAIL: no'):
            r = as_expected()
            r['control_stop_addresses_detail'] = value
            self.assertEqual(len(cc.judge(r)[2]), 1, value)

    def test_an_error_or_another_word_is_a_rig_error_not_a_verdict(self):
        for value in ('ERROR: no driver', 'WAITING: later', 'ok'):
            r = as_expected()
            r['pin_killed_server_device_waits'] = value
            _, differs, errors = cc.judge(r)
            self.assertEqual((differs, len(errors)), ([], 1), value)

    def test_an_observation_not_run_is_a_rig_error(self):
        r = as_expected()
        r['obs_delete_ms'] = 'Not run: no device'
        self.assertEqual(len(cc.judge(r)[2]), 1)

    def test_no_setup_line_is_a_rig_error(self):
        r = as_expected()
        del r['setup']
        self.assertEqual(len(cc.judge(r)[2]), 1)


class Table(unittest.TestCase):
    def test_every_stranger_state_has_its_three_pins(self):
        for state in cc.STATES:
            for name in (f'pin_add_on_{state}_stranger_is_taken', f'pin_{state}_stranger_untouched',
                         f'pin_info_of_{state}_stranger'):
                self.assertIn(name, cc.EXPECTED)

    def test_only_controls_are_expected_to_fail(self):
        for name, (want, who, row, claim) in cc.EXPECTED.items():
            self.assertEqual(want, 'FAIL' if name.startswith('ctl_') else 'PASS', name)
            self.assertTrue(row and claim, name)
            self.assertIn(who, (cc.CRATE, cc.RIG), name)


class TheCratesTests(unittest.TestCase):
    """The table against the source of the crate's kernel tests.

    A kernel test added by another pull request printed a verdict the
    table did not know, and both pull requests were green alone: the
    rig then refused the line on the integration branch. This test has
    no kernel in it and runs in the control rig workflow's job without
    a kernel, which a pull request that touches the crate starts: one
    that adds a kernel test fails here, by itself, until the table
    knows its line.
    """
    SOURCE = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..',
                          'crates', 'skyzfs-ublk-sys', 'tests', 'kernel.rs')

    def test_every_verdict_of_the_kernel_tests_is_in_the_table(self):
        with open(self.SOURCE, encoding='utf-8') as f:
            source = f.read()
        said = {'control_' + n for n in re.findall(r'property\(\s*"([a-z0-9_]+)"', source)}
        self.assertTrue(said)
        table = {n for n, (_, who, *_rest) in cc.EXPECTED.items() if who == cc.CRATE}
        self.assertEqual(said - table, set(), 'verdicts of the kernel tests that the table does not know')
        self.assertEqual(table - said, set(), 'lines the table expects of the kernel tests that none prints')


class Main(unittest.TestCase):
    def run_on(self, results):
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, 'results.tsv'), 'w', encoding='utf-8') as f:
                for k, v in results:
                    f.write(f'{k}\t{v}\n')
            out, sys.stdout = sys.stdout, open(os.devnull, 'w', encoding='utf-8')
            try:
                return cc.main(['control_check.py', d])
            finally:
                sys.stdout.close()
                sys.stdout = out

    def test_exit_statuses(self):
        r = as_expected()
        self.assertEqual(self.run_on(r.items()), 0)
        r['pin_delete_fails_requests'] = 'FAIL: the read returned data'
        self.assertEqual(self.run_on(r.items()), 1)
        r['pin_delete_fails_requests'] = 'ERROR: no holder'
        self.assertEqual(self.run_on(r.items()), 2)

    def test_no_results_file_is_a_rig_error_and_not_a_difference(self):
        import contextlib
        import io
        import tempfile
        with tempfile.TemporaryDirectory() as d, contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(cc.main(['control_check.py', os.path.join(d, 'none')]), 2)
        self.assertIn('not a verdict', out.getvalue())

    def test_a_line_written_twice_with_two_results_is_a_rig_error(self):
        r = list(as_expected().items()) + [('control_can_reissue', 'FAIL: no')]
        self.assertEqual(self.run_on(r), 2)


class KernelTestsThatDidNotEnd(unittest.TestCase):
    def test_a_hung_kernel_test_is_a_verdict_and_not_a_rig_error(self):
        r = as_expected()
        crate = [n for n in cc.EXPECTED if n.startswith('control_')]
        self.assertTrue(crate)
        # the binary was killed at the limit before two tests printed their lines
        r['pin_kernel_tests_end'] = 'FAIL: the test binary had not ended after 600 s'
        for name in crate[-2:]:
            del r[name]
        _, differs, errors = cc.judge(r)
        self.assertEqual(errors, [])
        self.assertEqual(len(differs), 3)      # the pin and the two lines
        # with the binary ended well, a missing line is the rig's error as before
        r = as_expected()
        del r[crate[-1]]
        _, differs, errors = cc.judge(r)
        self.assertEqual((differs, len(errors)), ([], 1))
        # and a missing line of the rig's own stays the rig's error either way
        r = as_expected()
        r['pin_kernel_tests_end'] = 'FAIL: ended with 101'
        own = next(n for n in cc.EXPECTED if n.startswith('pin_') and n != 'pin_kernel_tests_end')
        del r[own]
        _, differs, errors = cc.judge(r)
        self.assertEqual((len(differs), len(errors)), (1, 1))

    def test_an_observation_of_a_hung_kernel_test_is_a_verdict_too(self):
        # the crate's tests print these themselves; the rig only copies them
        self.assertEqual(sorted(cc.CRATE_OBSERVED), sorted(n for n in cc.OBSERVED if n.startswith('control_')))
        for name in cc.CRATE_OBSERVED:
            # the binary was cut before the test that prints this line
            r = as_expected()
            r['pin_kernel_tests_end'] = 'FAIL: the test binary had not ended after 600 s'
            del r[name]
            _, differs, errors = cc.judge(r)
            self.assertEqual(errors, [], name)
            self.assertEqual(len(differs), 2, name)        # the pin and the line
            self.assertEqual(Main().run_on(list(r.items())), 1, name)
            # the tests ended well and the line is missing: the rig lost it
            r = as_expected()
            del r[name]
            _, differs, errors = cc.judge(r)
            self.assertEqual((differs, len(errors)), ([], 1), name)
            self.assertEqual(Main().run_on(list(r.items())), 2, name)
        # an observation of the rig's own is the rig's error either way
        r = as_expected()
        r['pin_kernel_tests_end'] = 'FAIL: ended with 101'
        own = next(n for n in cc.OBSERVED if n.startswith('obs_'))
        del r[own]
        _, differs, errors = cc.judge(r)
        self.assertEqual((len(differs), len(errors)), (1, 1))
        self.assertEqual(Main().run_on(list(r.items())), 2)


if __name__ == '__main__':
    unittest.main()
