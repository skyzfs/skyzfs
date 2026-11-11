#!/usr/bin/env python3
"""Tests of the control rig's judge. No kernel: python3 tests/kernel/test_control_check.py"""
import os
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


if __name__ == '__main__':
    unittest.main()
