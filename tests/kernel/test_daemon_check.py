#!/usr/bin/env python3
"""Unit tests for daemon_check.py: no kernel needed.

    python3 tests/kernel/test_daemon_check.py
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import daemon_check as dc  # noqa: E402
import c_low_steps as cls  # noqa: E402

# The checks of a run by default. Check f is asked for by name, and a
# result of a check that was not asked for is a rig error: F below is
# what the tables hold for it.
ALL = {'a', 'b', 'c', 'd', 'e', 'g'}
F = [name for name, (check, *_) in list(dc.EXPECTED.items()) + list(dc.OBSERVED.items()) if check == 'f']
# Check g's pins and control: the daemon's in-place store (M1.T23).
G = [name for name, (check, *_) in dc.EXPECTED.items() if check == 'g']


def cases(impl='reference'):
    """As many cases of each kind as the suite declares for this server."""
    out = {}
    for part in dc.C_DECLARED[impl].split(', '):
        word, count = part.split()
        for i in range(int(count)):
            out[f'{word.lower()}_{i}'] = f'{word}: declared {word}: x'
    return out


def as_expected(impl='reference'):
    r = {name: f'{(rest[3:] or [rest[0]])[0]}: seen' for name, (_, *rest) in dc.EXPECTED.items() if name not in F}
    r.update({name: 'seen' for name in dc.OBSERVED if name not in F})
    r.update(impl='reference: x', setup='ok', dev_strays='none')
    r.update(c_run='ok: 26 cases', c_declared=dc.C_DECLARED[impl])
    r.update({f'case_{k}': v for k, v in cases(impl).items()})
    if impl == 'daemon':
        # A pin only the daemon can carry; the tests below set the
        # other one that waits on the stand-in by hand.
        r['pin_no_ublksrvd_entry_from_daemon'] = 'PASS: seen'
        r['pin_detach_of_held_says_6'] = 'PASS: seen'
        r['pin_orderly_stop_under_load'] = 'PASS: seen'
        r['pin_serve_on_zfs_refused'] = 'PASS: seen'
        r['pin_write_zeroes_through_a_checkpoint'] = 'PASS: seen'
        r['pin_volume_on_larger_blocks'] = 'PASS: seen'
        # The commands the driver ended (M1.3aa): counted by the daemon only.
        for name in ('pin_commands_failed_zero_a', 'pin_commands_failed_zero_b', 'pin_commands_failed_zero_e',
                     'pin_c_commands_failed_zero'):
            r[name] = 'PASS: seen'
        for name in ('pin_takeover_time_does_not_grow', 'pin_log_stays_below_high_mark',
                     'pin_kills_inside_steps_lose_nothing', 'pin_c_suite_with_low_marks'):
            r[name] = 'PASS: seen'
        r['ctl_recovered_log_too_short_no_log_device'] = 'FAIL: seen'
        for name in G:
            r[name] = f'{dc.EXPECTED[name][1]}: seen'
        # And those the daemon cannot carry yet.
        for name in dc.WAITS_FOR_THE_DAEMON:
            r[name] = 'WAITING: seen'
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

    def test_an_observation_only_the_daemon_makes(self):
        name = 'obs_c_low_steps'
        self.assertIn(name, dc.DAEMON_ONLY_OBSERVED)
        # the stand-in writes no line for it and none is asked for
        r = as_expected()
        del r[name]
        self.assertEqual(dc.judge(r, self.C)[1:], ([], []))
        # a line that says "not run" is the rig's error on either server
        r[name] = 'not run: x'
        self.assertEqual(len(dc.judge(r, self.C)[2]), 1)
        # with the daemon it is asked for
        r = as_expected('daemon')
        r['ctl_recovered_log_too_short'] = 'FAIL: refused'
        del r[name]
        self.assertEqual(len(dc.judge(r, self.C, impl='daemon')[2]), 1)
        r[name] = 'not run: x'
        self.assertEqual(len(dc.judge(r, self.C, impl='daemon')[2]), 1)

    def test_the_arm_without_a_log_device_is_judged_too(self):
        # An arm that did not happen, or whose start served, must not
        # hide behind its observation.
        name = 'ctl_recovered_log_too_short_no_log_device'

        def daemon_run(value):
            r = as_expected('daemon')
            r['ctl_recovered_log_too_short'] = 'FAIL: refused'
            r[name] = value
            return dc.judge(r, self.C, impl='daemon')[1:]

        self.assertEqual(daemon_run('FAIL: refused, served nothing'), ([], []))
        differs, errors = daemon_run('PASS: serve --recover-short 1 SERVED')
        self.assertEqual((len(differs), errors), (1, []))
        differs, errors = daemon_run('ERROR: zpool create failed')
        self.assertEqual((differs, len(errors)), ([], 1))
        r = as_expected('daemon')
        r['ctl_recovered_log_too_short'] = 'FAIL: refused'
        del r[name]
        self.assertEqual(len(dc.judge(r, self.C, impl='daemon')[2]), 1)
        # on the stand-in it waits, like the first arm's control
        self.assertEqual(as_expected()[name], 'WAITING: seen')
        self.assertEqual(dc.judge(as_expected(), self.C)[1:], ([], []))

    def test_the_daemon_is_held_to_its_own_counts(self):
        r = as_expected('daemon')
        r['ctl_recovered_log_too_short'] = 'FAIL: refused'
        self.assertEqual(dc.judge(r, self.C, impl='daemon')[1:], ([], []))
        # the stand-in's counts, from a suite that was told it can reopen a failed volume
        r = as_expected('reference')
        r['ctl_recovered_log_too_short'] = 'FAIL: refused'
        r['pin_no_ublksrvd_entry_from_daemon'] = 'PASS: none'
        r['pin_detach_of_held_says_6'] = 'PASS: exit 6'
        r['ctl_recovered_log_too_short_no_log_device'] = 'FAIL: refused'
        r['pin_orderly_stop_under_load'] = 'PASS: exit 0'
        r['pin_serve_on_zfs_refused'] = 'PASS: refused'
        r['pin_write_zeroes_through_a_checkpoint'] = 'PASS: zeros'
        r['pin_volume_on_larger_blocks'] = 'PASS: refused and served'
        for name in ('pin_commands_failed_zero_a', 'pin_commands_failed_zero_b', 'pin_commands_failed_zero_e',
                     'pin_c_commands_failed_zero'):
            r[name] = 'PASS: commands_failed 0'
        for name in ('pin_takeover_time_does_not_grow', 'pin_log_stays_below_high_mark',
                     'pin_kills_inside_steps_lose_nothing', 'pin_c_suite_with_low_marks'):
            r[name] = 'PASS: seen'
        for name in G:
            r[name] = f'{dc.EXPECTED[name][1]}: seen'
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
        # check f's lines have a test of their own below
        for name in [n for n in list(dc.EXPECTED) + list(dc.OBSERVED) if n not in F]:
            r = as_expected()
            del r[name]
            _, differs, errors = dc.judge(r, ALL)
            self.assertEqual(differs, [], name)
            # but for an observation only the daemon makes (its own test below)
            self.assertEqual(len(errors), 0 if name in dc.DAEMON_ONLY_OBSERVED else 1, name)

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

    def test_an_observation_not_run_is_a_rig_error_however_it_says_so(self):
        # The first observation of a default check (check f's need asking for).
        name = next(n for n, (check, _) in dc.OBSERVED.items() if check != 'f')
        for text in ('not run: no pool', 'Not run', 'the scenario was not run, the pool was gone'):
            r = as_expected()
            r[name] = text
            self.assertEqual(len(dc.judge(r, ALL)[2]), 1, text)
        r = as_expected()
        r[name] = 'zed not running; the pool stayed suspended'
        self.assertEqual(dc.judge(r, ALL)[1:], ([], []))

    def test_a_run_on_the_daemon_with_check_d_has_every_observation(self):
        # The observations of check d are made on the reference server
        # whatever the server under test: none is "not run" on the
        # daemon, and none is excused for saying so.
        r = as_expected('daemon')
        r['ctl_recovered_log_too_short'] = 'FAIL: refused'
        self.assertEqual(dc.judge(r, ALL, impl='daemon')[1:], ([], []))
        r['obs_returned_device_with_older_content'] = 'not run: needs the flat image of the reference server'
        _, differs, errors = dc.judge(r, ALL, impl='daemon')
        self.assertEqual((differs, len(errors)), ([], 1))

    def test_the_rig_skips_no_observation_because_of_the_server(self):
        # A line that writes "not run" under a condition on the server
        # would make every run on the daemon a rig error.
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            bad = [line.strip() for line in f if 'not run' in line.lower() and 'IMPL' in line]
        self.assertEqual(bad, [])

    def test_a_result_of_a_check_that_was_not_asked_for_is_a_rig_error(self):
        r = {k: v for k, v in as_expected().items() if dc.EXPECTED.get(k, dc.OBSERVED.get(k, ('',)))[0] in ('e', '')}
        self.assertEqual(dc.judge(r, {'e'})[1:], ([], []))
        r['pin_block_size'] = 'FAIL: 512'
        _, differs, errors = dc.judge(r, {'e'})
        self.assertEqual((differs, len(errors)), ([], 1))
        self.assertIn('pin_block_size', errors[0])
        r['pin_block_size'] = 'PASS: 4096'
        self.assertEqual(len(dc.judge(r, {'e'})[2]), 1)

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
            # with the check of the observation asked for, and the
            # judged line that check f then needs
            r = as_expected()
            r[name] = 'not run: the serve did not start'
            checks = ALL
            if name in F:
                checks = ALL | {'f'}
                r['ctl_f_starts_refused'] = 'WAITING: the stand-in'
            _, differs, errors = dc.judge(r, checks)
            self.assertEqual((differs, len(errors)), ([], 1), name)

    def test_check_f_is_asked_for_by_name_and_is_the_daemons(self):
        f = [name for name, (check, _) in dc.OBSERVED.items() if check == 'f']
        # seven short courses, and check b's two long courses since M1.T8
        self.assertEqual(len(f), 9)
        self.assertIn('obs_f_refused_start_long', f)
        self.assertIn('obs_f_refused_start_longnolog', f)
        # one judged line: the courses' control
        self.assertEqual([n for n, (check, *_) in dc.EXPECTED.items() if check == 'f'], ['ctl_f_starts_refused'])
        self.assertEqual(dc.parse_checks('f'), ({'f'}, []))
        # on the daemon each course's line must be there
        r = dict(impl='daemon: x', setup='ok', dev_strays='none', ctl_f_starts_refused='FAIL: refused', **{name: 'seen' for name in f})
        self.assertEqual(dc.judge(r, {'f'}, impl='daemon')[1:], ([], []))
        for name in f:
            q = dict(r)
            del q[name]
            self.assertEqual(dc.judge(q, {'f'}, impl='daemon')[2], [f'{name}: no observation'], name)
        # a run of the default checks does not ask for them, and a
        # line of check f in such a run is the rig's error
        self.assertEqual(dc.parse_checks(dc.DEFAULT_CHECKS)[0], ALL)
        q = as_expected()
        self.assertEqual([k for k in q if k in F], [])
        self.assertEqual(dc.judge(q, ALL)[1:], ([], []))
        q['ctl_f_starts_refused'] = 'WAITING: the stand-in'
        self.assertEqual(len(dc.judge(q, ALL)[2]), 1)
        # the stand-in has no log to come out short: no line is asked for
        q = dict(impl='reference: x', setup='ok', dev_strays='none', ctl_f_starts_refused='WAITING: the stand-in')
        self.assertEqual(dc.judge(q, {'f'})[1:], ([], []))

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
            self.assertIn(check, dc.CHECKS, name)
            self.assertIn(basis, (dc.KERNEL, dc.SERVER, dc.WAITING), name)
            self.assertEqual(want, 'FAIL' if name.startswith('ctl_') else 'PASS', name)
            self.assertTrue(claim, name)


class WaitsForTheDaemon(unittest.TestCase):
    def test_a_pin_the_daemon_cannot_carry_yet_is_waiting_there_and_nothing_else(self):
        r = as_expected('daemon')
        r['ctl_recovered_log_too_short'] = 'FAIL: seen'       # the control only the daemon runs
        r.update({name: 'FAIL: seen' if name.startswith('ctl_') else 'seen' for name in F})
        _, differs, errors = dc.judge(r, set(dc.CHECKS), 'daemon')
        self.assertEqual((differs, errors), ([], []))
        # PASS before its time is a difference: the table is changed on
        # purpose when the daemon can carry the pin.
        r['pin_failed_volume_while_a_request_is_held'] = 'PASS: early'
        _, differs, errors = dc.judge(r, set(dc.CHECKS), 'daemon')
        self.assertEqual(len(differs), 1)
        r['pin_failed_volume_while_a_request_is_held'] = 'FAIL: did not end'
        _, differs, errors = dc.judge(r, set(dc.CHECKS), 'daemon')
        self.assertEqual(len(differs), 1)

    def test_the_checkpoint_pins_are_judged_on_the_daemon(self):
        # Built with M1.T8: WAITING is no longer an answer there, and a
        # FAIL differs from the table.
        for name in ('pin_takeover_time_does_not_grow', 'pin_log_stays_below_high_mark',
                     'pin_kills_inside_steps_lose_nothing', 'pin_c_suite_with_low_marks'):
            self.assertNotIn(name, dc.WAITS_FOR_THE_DAEMON)
            r = as_expected('daemon')
            r['ctl_recovered_log_too_short'] = 'FAIL: seen'
            r.update({n: 'FAIL: seen' if n.startswith('ctl_') else 'seen' for n in F})
            r[name] = 'WAITING: not built'
            _, differs, errors = dc.judge(r, set(dc.CHECKS), 'daemon')
            self.assertEqual(len(errors), 1, name)
            r[name] = 'FAIL: seen'
            _, differs, errors = dc.judge(r, set(dc.CHECKS), 'daemon')
            self.assertEqual((len(differs), errors), (1, []), name)
            # On the stand-in they wait for good.
            self.assertEqual(dc.EXPECTED[name][4], 'WAITING', name)

    def test_every_such_pin_is_in_the_table_and_waits_on_the_stand_in(self):
        for name in dc.WAITS_FOR_THE_DAEMON:
            self.assertEqual(dc.EXPECTED[name][4], 'WAITING', name)


def in_place():
    """A run of the daemon with --store in-place, every line as the table has it."""
    r = as_expected('daemon')
    r.update(impl='daemon: x', store='in-place', c_declared=dc.C_DECLARED['daemon in-place'])
    for name in dc.NOT_FOR_IN_PLACE:
        if name not in F:
            r[name] = 'NA: the in-place store has no log'
    return r


class InPlace(unittest.TestCase):
    """The daemon with --store in-place: the pins that rest on the log are NA, every other as with the log store."""

    def judge(self, r):
        store, errors = dc.store_of(r, 'daemon')
        rows, differs, more = dc.judge(r, ALL, 'daemon', store)
        return rows, differs, errors + more

    def test_every_name_not_taken_is_in_the_table(self):
        for name in dc.NOT_FOR_IN_PLACE:
            self.assertIn(name, dc.EXPECTED, name)

    def test_a_run_with_na_where_the_table_says_so_passes(self):
        rows, differs, errors = self.judge(in_place())
        self.assertEqual((differs, errors), ([], []))
        na = {name for name, want, *_ in rows if want == 'NA'}
        self.assertEqual(na, {n for n in dc.NOT_FOR_IN_PLACE if dc.EXPECTED[n][0] in ALL})

    def test_na_on_the_log_store_is_a_rig_error(self):
        r = as_expected('daemon')
        r.update(impl='daemon: x', store='log', pin_log_stays_below_high_mark='NA: x')
        _, _, errors = self.judge(r)
        self.assertTrue(any('pin_log_stays_below_high_mark' in e for e in errors), errors)

    def test_na_for_a_pin_that_applies_to_in_place_is_a_rig_error(self):
        r = in_place()
        r['pin_kill_under_load'] = 'NA: x'
        _, _, errors = self.judge(r)
        self.assertTrue(any('pin_kill_under_load:' in e for e in errors), errors)

    def test_a_pin_not_taken_that_was_taken_differs(self):
        r = in_place()
        r['pin_takeover_time_does_not_grow'] = 'PASS: x'
        _, differs, errors = self.judge(r)
        self.assertEqual(errors, [])
        self.assertTrue(any('pin_takeover_time_does_not_grow' in d for d in differs), differs)

    def test_the_daemon_names_its_store(self):
        for store in (None, '', 'whatever'):
            r = in_place()
            r.pop('store')
            if store is not None:
                r['store'] = store
            self.assertTrue(dc.store_of(r, 'daemon')[1], store)
        # The stand-in has none to name.
        self.assertEqual(dc.store_of({'impl': 'reference: x'}, 'reference'), (None, []))

    def test_the_suite_is_held_to_the_in_place_counts(self):
        r = in_place()
        r['c_declared'] = 'PASS 2, WEAKER 12, NOT_EXERCISED 12'
        _, differs, _ = self.judge(r)
        self.assertTrue(any('C_DECLARED' in d for d in differs), differs)


class CheckG(unittest.TestCase):
    """Check g (M1.T23): the daemon's in-place store, a disk that fails for real."""

    def test_every_pin_of_check_g_is_in_g_pins(self):
        # The stand-in, the log store and a file system that was not set up
        # write check g's pins from G_PINS without running it; a pin left
        # out of it is "no result", a rig error (M1.T26 review).
        import re
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            listed = set(re.search(r'^G_PINS="([^"]*)"', f.read(), re.M).group(1).split())
        want = {name[len('pin_'):] for name, row in dc.EXPECTED.items() if row[0] == 'g' and name.startswith('pin_')}
        self.assertEqual(want, listed)

    def test_the_log_store_branch_writes_every_observation(self):
        # With the log store the rig writes check g's lines without running
        # it; an observation it leaves out fails the run (M1.T23, Codex).
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            text = f.read()
        start = text.index('elif ! in_place; then')
        branch = text[start:text.index('check_g\n', start)]
        for name, (check, _) in dc.OBSERVED.items():
            if check == 'g':
                self.assertIn(name[len('obs_'):], branch, name)

    def test_check_c_writes_its_evidence_on_every_path(self):
        # M1.T28: check c ends in one place, which writes obs_c_evidence
        # whether or not the pass with the delay ran; and the evidence is
        # started and ended around both passes of the tool.
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            text = f.read()
        body = text[text.index('\ncheck_c() {'):text.index('\ncheck_c_delay() {')]
        self.assertEqual(body.count('obs c_evidence '), 3, 'no trace possible, a pass that left too little, and all')
        # The two lines for a missing piece are the rig's error: the judge
        # reads "not run" in them.
        self.assertEqual(body.count('obs c_evidence "not run: '), 2)
        main = text[text.index('\n    check_c\n    check_c_low\n'):]
        self.assertTrue(main.index('c_watch_back') < main.index(';;'), 'the watch is put back after the third pass')
        for tag in ('c', 'cd'):
            self.assertIn(f'c_evidence_start {tag}\n', text)
            self.assertIn(f'c_evidence_end {tag} "$work" "$rc"', text)
        self.assertIn('c_watch_back', text[text.index('\ncleanup() {'):text.index('\ntrap cleanup EXIT')])

    def test_on_the_daemon_in_place_every_line_is_judged(self):
        r = in_place()
        rows, differs, errors = dc.judge(r, ALL, impl='daemon', store='in-place')
        self.assertEqual((differs, errors), ([], []))
        self.assertTrue(all(want in ('PASS', 'FAIL') for name, want, *_ in rows if name in G), rows)

    def test_a_start_that_serves_the_cache_differs(self):
        r = in_place()
        r['pin_start_serves_what_the_disk_holds'] = 'FAIL: the start served byte 0x22'
        _, differs, errors = dc.judge(r, ALL, impl='daemon', store='in-place')
        self.assertEqual(len(differs), 1)
        self.assertEqual(errors, [])

    def test_a_control_that_passes_differs(self):
        r = in_place()
        r['ctl_g_old_content_after_good_sync'] = 'PASS: the start served 0x11'
        _, differs, _ = dc.judge(r, ALL, impl='daemon', store='in-place')
        self.assertEqual(len(differs), 1)

    def test_with_the_log_store_it_is_na(self):
        r = as_expected('daemon')
        r.update(impl='daemon: x', store='log', ctl_recovered_log_too_short='FAIL: refused')
        r.update({name: 'NA: check g is of the in-place store' for name in G})
        _, differs, errors = dc.judge(r, ALL, impl='daemon', store='log')
        self.assertEqual((differs, errors), ([], []))

    def test_on_the_stand_in_it_waits(self):
        rows, differs, errors = dc.judge(as_expected(), ALL)
        self.assertEqual((differs, errors), ([], []))
        self.assertEqual({want for name, want, *_ in rows if name in G}, {'WAITING'})

    def test_its_observations_are_the_daemons(self):
        r = as_expected()
        for name in ('obs_g_failed_sync', 'obs_g_new_descriptor_after_failed_sync', 'obs_g_reads_while_failing'):
            del r[name]
        self.assertEqual(dc.judge(r, ALL)[1:], ([], []))
        r = in_place()
        del r['obs_g_failed_sync']
        self.assertTrue(any('obs_g_failed_sync' in e for e in dc.judge(r, ALL, impl='daemon', store='in-place')[2]))


class CommandsFailed(unittest.TestCase):
    """M1.3aa: no command of the queues ended with a result other than OK or the driver's abort."""
    PINS = ('pin_commands_failed_zero_a', 'pin_commands_failed_zero_b', 'pin_commands_failed_zero_e',
            'pin_c_commands_failed_zero')

    def daemon(self):
        return in_place()

    def test_a_command_the_driver_refused_differs(self):
        for name in self.PINS:
            r = self.daemon()
            r[name] = 'FAIL: status says commands_failed 1, the last with the result -22'
            _, differs, errors = dc.judge(r, ALL, impl='daemon', store='in-place')
            self.assertEqual((len(differs), errors), (1, []), name)

    def test_waiting_is_the_stand_ins_answer_only(self):
        for name in self.PINS:
            self.assertEqual(dc.EXPECTED[name][4], 'WAITING', name)
            self.assertNotIn(name, dc.NOT_FOR_IN_PLACE, name)
            r = self.daemon()
            r[name] = 'WAITING: the stand-in keeps no request log'
            _, differs, errors = dc.judge(r, ALL, impl='daemon', store='in-place')
            self.assertEqual(len(errors), 1, name)

    def test_a_build_without_the_count_is_a_rig_error(self):
        r = self.daemon()
        r['pin_c_commands_failed_zero'] = 'ERROR: this build of the daemon predates M1.3aa'
        _, differs, errors = dc.judge(r, ALL, impl='daemon', store='in-place')
        self.assertEqual((differs, len(errors)), ([], 1))

    def test_the_rig_writes_each_pin(self):
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            text = f.read()
        for check in ('a', 'b', 'e'):
            self.assertIn(f'pin_commands_failed {check} "$dir"', text, check)
        # Check b with no server at its end still writes the line.
        self.assertIn('pin_commands_failed b ""', text)
        body = text[text.index('\ncheck_c() {'):text.index('\ncheck_c_delay() {')]
        self.assertEqual(body.count('    pin_c_commands_failed\n'), 1)


class DataJournalFiles(unittest.TestCase):
    """g.4 (M1.T30): what a refused start may leave changed in the volume's directory."""

    def judged(self, new, gone):
        import subprocess
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        script = (f'eval "$(sed -n \'/^GJ_MAY_CHANGE=/,/^GJ_ALLOWED_UNTIL_M13T=/p\' {rig!r})"; '
                  f'eval "$(sed -n \'/^gj_files_judged() {{$/,/^}}$/p\' {rig!r})"; gj_files_judged "$1" "$2"')
        return subprocess.run(['bash', '-c', script, '_', new, gone], capture_output=True, text=True, check=True).stdout

    def test_the_log_files_are_allowed_until_m13t(self):
        self.assertEqual(self.judged('log.000000 log.super ', 'log.000000 log.super '), '')

    def test_the_status_file_may_change(self):
        self.assertEqual(self.judged('status ', 'status '), '')

    def test_the_image_and_the_identity_file_may_not(self):
        self.assertEqual(self.judged('home.img ', 'home.img '), 'home.img changed; ')
        self.assertEqual(self.judged('volume ', 'volume '), 'volume changed; ')

    def test_a_failure_marker_may_not_appear_or_go(self):
        self.assertIn('log.failed appeared', self.judged('log.failed ', ''))
        self.assertIn('volume.failed removed', self.judged('', 'volume.failed '))

    def test_any_other_file_is_named(self):
        self.assertEqual(self.judged('lock x.tmp ', 'lock '), 'lock rewritten; x.tmp rewritten; ')


class G1Premise(unittest.TestCase):
    """g.1 (M1.T30, review of #72): a daemon that holds a request after the disk failed is a FAIL, not a rig error."""

    def body(self):
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            text = f.read()
        return text[text.index('\ncheck_g_sync_fails_while_serving() {'):text.index('\ncase $CHECKS in *" a "*)')]

    def test_the_serve_is_waited_for_before_any_error(self):
        body = self.body()
        error = body.index('pin sync_failure_fails_volume ERROR')
        self.assertLess(body.index('if serve_wait 60; then'), error)
        self.assertEqual(body.count('pin sync_failure_fails_volume ERROR'), 1)

    def test_only_a_write_that_could_not_be_issued_is_an_error(self):
        body = self.body()
        branch = body[body.index('elif [ "$wrc" != 0 ] && grep -q \'failed to open\''):]
        condition = branch[:branch.index('; then')]
        # The device gone is the common case (the daemon failed the volume
        # and deleted it): the error needs every sign that nothing reached it.
        for sign in ('[ "$w1" = "$w0" ]', '[ "$f1" = "$f0" ]', '[ "${held:-}" = 0 ]',
                     "! grep -q 'the volume failed' \"$dir.serve.log\""):
            self.assertIn(sign, condition)
        self.assertTrue(branch[branch.index('; then') + 7:].lstrip().startswith('pin sync_failure_fails_volume ERROR'))
        # A write not answered, or a request held, is judged before it, and FAILs.
        held = body.index('[ "$wrc" = 124 ] || [[ ${held:-0}')
        self.assertLess(held, body.index('pin sync_failure_fails_volume ERROR'))
        self.assertIn('FAIL "serve was still running 60 s after the pool\'s wait ended, with a request held',
                      body[held:held + 400])


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

    def test_a_file_at_a_device_path_is_a_rig_error(self):
        # A regular file under a device's name in /dev, or no word on
        # it, makes the run a rig error whatever the pins say.
        good = as_expected()
        for change, want in ((None, 0), ('ERROR: at the end of the run: -rw-r--r-- 1 root root 4096 /dev/ublkb3', 2),
                             ('', 2), ('missing', 2)):
            r = dict(good)
            if change == 'missing':
                del r['dev_strays']
            elif change is not None:
                r['dev_strays'] = change
            status, text = self.run_main([f'{k}\t{v}\n' for k, v in r.items()])
            self.assertEqual(status, want, change)
            if want:
                self.assertIn('dev_strays', text)

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
        self.assertEqual(self.run_main(lines, '--checks', 'a b c d e g')[0], 0)
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



class CLowSteps(unittest.TestCase):
    """c_low_steps.py: each volume of check c's third pass in its class."""

    TRACE = [
        '0\t1\topen\t-\t64\t16',
        '1\t1\tsubmit\t1\twrite\t0\t4\th',
        '2\t1\tsubmit\t2\tread\t0\t4\t',
        '3\t2\topen\t-\t64\t16',
        '4\t2\tsubmit\t1\tread\t0\t4\t',
        '5\t3\topen\t-\t64\t16',
        '6\t3\tsubmit\t1\tread\t0\t4\t',
        '7\t3\tsubmit\t2\tdiscard\t0\t4\t',
        '8\t4\topen\t-\t64\t16',
        '9\t4\tsubmit\t1\twrite_zeroes\t0\t4\t',
        '10\t4\tsubmit\t2\tflush\t0\t0\t',
        '11\t5\topen\t-\t64\t16',
        '12\t5\tsubmit\t1\twrite\t0\t4\th',
        '13\t5\tsubmit\t2\twrite\t4\t4\th',
    ]
    HELPER = [
        'prefill 1: set: log_live 134213632, 4096 bytes below the high mark 134217728; a 4 KiB write adds 8192; checkpoint_steps 0',
        'steps 1 kill: checkpoint_steps 2 gate 1',
        'steps 1 destroy: checkpoint_steps 3 gate 0',
        'prefill 2: set: ...',
        'prefill 3: set: ...',
        'prefill 4: NOT SET: log_live 1 is not within 8192 bytes below the high mark 134217728',
        'prefill 5: set: ...',
        'steps 5 destroy: checkpoint_steps 0 gate 0',
    ]

    def test_classes(self):
        classes, steps, gate = cls.classify(self.TRACE, self.HELPER)
        self.assertEqual(classes, {'1': 'inside', '2': 'no_write', '3': 'last_write', '4': 'not_set', '5': 'no_step'})
        self.assertEqual(steps['1'], 5)
        self.assertEqual(gate['1'], 1)

    def test_too_many_missed_is_said(self):
        out = cls.report(*cls.classify(self.TRACE, self.HELPER))
        self.assertIn('inside=1', out[0])
        self.assertIn('verdict=TOO_MANY', out[0])
        self.assertIn('c_low volume 5 no_step', out)

    def test_one_miss_in_ten_is_ok(self):
        classes = {str(v): 'inside' for v in range(1, 10)}
        classes['10'] = 'no_step'
        self.assertIn('verdict=OK', cls.report(classes, {}, {})[0])
        classes['11'] = 'not_set'
        self.assertIn('verdict=TOO_MANY', cls.report(classes, {}, {})[0])

    # Volumes 49 and 52 of run 43206698581 (24.04, daemon), programs of
    # prop_failures_match_model: 49's log was failed before its first
    # call that adds to the log, 52's after its first writes.
    FAILED = [
        '220869\t49\topen\t-\t64\t16',
        '220988\t49\tfail\t-\t',
        '221205\t49\tsubmit\t1\twrite_zeroes\t16\t5\t',
        '221205\t49\tkill\t-\tsignal sent by the tool: None',
        '221215\t49\tsubmit\t2\twrite\t4\t3\t46366a8693a08b25,d7405ff88277f325,d998b4286b1b9b25',
        '221215\t49\tsubmit\t3\tread\t10\t1\t',
        '235767\t52\topen\t-\t64\t16',
        '235770\t52\tsubmit\t1\twrite_zeroes\t0\t4\t',
        '235771\t52\tsubmit\t2\twrite\t4\t4\th',
        '235822\t52\tfail\t-\t',
        '235830\t52\tsubmit\t3\twrite\t8\t4\th',
    ]
    FAILED_HELPER = [
        'prefill 49: set: log_live 134209536, 8192 bytes below the high mark 134217728; a 4 KiB write adds 8192; checkpoint_steps 0',
        'steps 49 destroy: checkpoint_steps 0 gate 0',
        'prefill 52: set: log_live 134209536, 8192 bytes below the high mark 134217728; a 4 KiB write adds 8192; checkpoint_steps 0',
        'steps 52 destroy: checkpoint_steps 1 gate 0',
    ]

    def test_a_log_failed_before_the_first_write_is_counted_apart(self):
        classes, _, _ = cls.classify(self.FAILED, self.FAILED_HELPER)
        self.assertEqual(classes, {'49': 'failed_first', '52': 'inside'})
        out = cls.report(classes, {}, {})
        self.assertIn('failed_first=1', out[0])
        self.assertIn('verdict=OK', out[0])
        self.assertIn('c_low volume 49 failed_first', out)

    def test_no_volume_that_could_is_too_many(self):
        self.assertIn('verdict=TOO_MANY', cls.report({'1': 'no_write'}, {}, {})[0])

if __name__ == '__main__':
    unittest.main()
