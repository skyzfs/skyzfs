#!/usr/bin/env python3
"""Unit tests for daemon_check.py: no kernel needed.

    python3 tests/kernel/test_daemon_check.py
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import daemon_check as dc  # noqa: E402

# The checks of a run by default, which are all of them since check f
# went with the log store (M1.3t).
ALL = {'a', 'b', 'c', 'd', 'e', 'g'}
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


RC = ('pin_refused_read_commit_taken_over', 'pin_refused_write_commit_taken_over',
      'pin_refused_flush_commit_taken_over', 'pin_refused_commit_at_a_stop_ends_8')


def as_expected(impl='reference'):
    r = {name: f'{(rest[3:] or [rest[0]])[0]}: seen' for name, (_, *rest) in dc.EXPECTED.items()}
    r.update({name: 'seen' for name in dc.OBSERVED})
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
        r['pin_volume_on_larger_blocks'] = 'PASS: seen'
        # The refused commits (M1.T35): the daemon's test hooks.
        for name in RC:
            r[name] = 'PASS: seen'
        # The commands the driver ended (M1.3aa): counted by the daemon only.
        for name in ('pin_commands_failed_zero_a', 'pin_commands_failed_zero_b', 'pin_commands_failed_zero_e',
                     'pin_c_commands_failed_zero'):
            r[name] = 'PASS: seen'
        # The stand-in's lines that the daemon's store cannot carry.
        for name in dc.NOT_ON_THE_DAEMON:
            r[name] = "NA: the daemon's store keeps no record of what it applied"
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
        name = 'obs_g_failed_sync'
        self.assertIn(name, dc.DAEMON_ONLY_OBSERVED)
        # the stand-in writes no line for it and none is asked for
        r = as_expected()
        del r[name]
        self.assertEqual(dc.judge(r, ALL)[1:], ([], []))
        # a line that says "not run" is the rig's error on either server
        r[name] = 'not run: x'
        self.assertEqual(len(dc.judge(r, ALL)[2]), 1)
        # with the daemon it is asked for
        r = as_expected('daemon')
        del r[name]
        self.assertEqual(len(dc.judge(r, ALL, impl='daemon')[2]), 1)
        r[name] = 'not run: x'
        self.assertEqual(len(dc.judge(r, ALL, impl='daemon')[2]), 1)

    def test_the_daemon_is_held_to_its_own_counts(self):
        r = as_expected('daemon')
        self.assertEqual(dc.judge(r, self.C, impl='daemon')[1:], ([], []))
        # the stand-in's counts, from a suite that was told it can reopen a failed volume
        r = as_expected('reference')
        r['pin_no_ublksrvd_entry_from_daemon'] = 'PASS: none'
        r['pin_detach_of_held_says_6'] = 'PASS: exit 6'
        r['pin_orderly_stop_under_load'] = 'PASS: exit 0'
        r['pin_serve_on_zfs_refused'] = 'PASS: refused'
        r['pin_volume_on_larger_blocks'] = 'PASS: refused and served'
        for name in ('pin_commands_failed_zero_a', 'pin_commands_failed_zero_b', 'pin_commands_failed_zero_e',
                     'pin_c_commands_failed_zero'):
            r[name] = 'PASS: commands_failed 0'
        for name in dc.NOT_ON_THE_DAEMON:
            r[name] = 'NA: no record'
        for name in G:
            r[name] = f'{dc.EXPECTED[name][1]}: seen'
        for name in RC:
            r[name] = 'PASS: exit 8, taken over'
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
        r['pin_clear_resumes_on_returned_device'] = 'ERROR: not run'
        self.assertEqual(len(dc.judge(r, ALL, impl='daemon')[2]), 1)
        r['pin_clear_resumes_on_returned_device'] = 'PASS: resumed'
        _, differs, errors = dc.judge(r, ALL, impl='daemon')
        self.assertEqual((differs, errors), ([], []))
        r['ctl_g_old_content_after_good_sync'] = 'WAITING: the stand-in'
        _, _, errors = dc.judge(r, ALL, impl='daemon')
        self.assertEqual(len(errors), 1)

    def test_an_observation_not_run_is_a_rig_error_however_it_says_so(self):
        name = next(iter(dc.OBSERVED))
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
        name = 'ctl_g_old_content_after_good_sync'
        r = as_expected()
        self.assertEqual(r[name], 'WAITING: seen')
        _, differs, errors = dc.judge(r, ALL)
        self.assertEqual((differs, errors), ([], []))
        # a pin the stand-in can carry may not be left waiting
        r['pin_kill_under_load'] = 'WAITING: not run'
        _, differs, errors = dc.judge(r, ALL)
        self.assertEqual((differs, len(errors)), ([], 1))
        # with the daemon, waiting is over: the control must have run and failed
        r = as_expected('daemon')
        r[name] = 'WAITING: the stand-in'
        _, differs, errors = dc.judge(r, ALL, impl='daemon')
        self.assertEqual((differs, len(errors)), ([], 1))
        r[name] = 'FAIL: the start served the write'
        self.assertEqual(dc.judge(r, ALL, impl='daemon')[1:], ([], []))
        # and a pin that waits for the daemon may not wait on the daemon
        r['pin_no_ublksrvd_entry_from_daemon'] = 'WAITING: the stand-in'
        self.assertEqual(len(dc.judge(r, ALL, impl='daemon')[2]), 1)
        r[name] = 'PASS: served'
        self.assertEqual(len(dc.judge(r, ALL, impl='daemon')[1]), 1)

    def test_an_observation_that_did_not_run_is_a_rig_error(self):
        for name in dc.OBSERVED:
            r = as_expected()
            r[name] = 'not run: the serve did not start'
            _, differs, errors = dc.judge(r, ALL)
            self.assertEqual((differs, len(errors)), ([], 1), name)

    def test_check_f_went_with_the_log_store(self):
        # Every course of check f began with a start refused for a short
        # log (M1.3t removed the log): the letter is no longer a check,
        # and no line of the tables belongs to it.
        self.assertNotIn('f', dc.CHECKS)
        self.assertEqual(len(dc.parse_checks('f')[1]), 1)
        self.assertEqual([n for n, (check, *_) in list(dc.EXPECTED.items()) + list(dc.OBSERVED.items())
                          if check not in dc.CHECKS], [])
        self.assertEqual(dc.parse_checks(dc.DEFAULT_CHECKS)[0], ALL)


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

    def test_every_such_pin_is_in_the_table_and_waits_on_the_stand_in(self):
        for name in dc.WAITS_FOR_THE_DAEMON:
            self.assertEqual(dc.EXPECTED[name][4], 'WAITING', name)


class NotOnTheDaemon(unittest.TestCase):
    """The stand-in's lines that the daemon's store cannot carry: NA on the daemon, and nowhere else."""

    def judge(self, r):
        return dc.judge(r, ALL, 'daemon')

    def test_every_name_not_taken_is_in_the_table_and_judged_on_the_stand_in(self):
        for name in dc.NOT_ON_THE_DAEMON:
            self.assertIn(name, dc.EXPECTED, name)
            # Measured on the reference server, not waiting there.
            self.assertEqual(dc.EXPECTED[name][2], dc.SERVER, name)
            self.assertEqual(dc.EXPECTED[name][4:], (), name)

    def test_a_run_with_na_where_the_table_says_so_passes(self):
        rows, differs, errors = self.judge(as_expected('daemon'))
        self.assertEqual((differs, errors), ([], []))
        na = {name for name, want, *_ in rows if want == 'NA'}
        self.assertEqual(na, set(dc.NOT_ON_THE_DAEMON))

    def test_na_on_the_stand_in_is_a_rig_error(self):
        for name in dc.NOT_ON_THE_DAEMON:
            r = as_expected()
            r[name] = 'NA: x'
            _, _, errors = dc.judge(r, ALL)
            self.assertTrue(any(name in e for e in errors), (name, errors))

    def test_na_for_a_pin_the_daemon_carries_is_a_rig_error(self):
        r = as_expected('daemon')
        r['pin_kill_under_load'] = 'NA: x'
        _, _, errors = self.judge(r)
        self.assertTrue(any('pin_kill_under_load:' in e for e in errors), errors)

    def test_a_line_not_taken_that_was_taken_differs(self):
        r = as_expected('daemon')
        r['ctl_delay_before_apply'] = 'FAIL: found nothing'
        _, differs, errors = self.judge(r)
        self.assertEqual(errors, [])
        self.assertTrue(any('ctl_delay_before_apply' in d for d in differs), differs)

    def test_the_log_stores_pins_are_gone(self):
        # M1.3t: they checked nothing on either server any more: NA on
        # the daemon, and on the stand-in WAITING, or PASS on what holds
        # nothing there (the stand-in's own `unstored 0`, and its
        # re-serves after a kill, now judged in pin_kill_under_load). A
        # line of one is the rig's error, a table and a rig that have
        # come apart.
        for name in ('pin_takeover_time_does_not_grow', 'pin_log_stays_below_high_mark',
                     'pin_kills_inside_steps_lose_nothing', 'pin_c_suite_with_low_marks',
                     'pin_write_zeroes_through_a_checkpoint', 'pin_unstored_zero_a', 'pin_unstored_zero_b',
                     'pin_unstored_zero_e', 'pin_serve_after_kill_accepts_log', 'ctl_recovered_log_too_short',
                     'ctl_recovered_log_too_short_no_log_device', 'ctl_f_starts_refused'):
            self.assertNotIn(name, dc.EXPECTED, name)
            r = as_expected('daemon')
            r[name] = 'NA: no log'
            self.assertTrue(any(name in e for e in self.judge(r)[2]), name)

    def test_a_store_line_is_context(self):
        # A rig from before M1.3t named the store; the judge no longer
        # reads one, and a run is not judged by it.
        r = as_expected('daemon')
        r['store'] = 'in-place'
        self.assertEqual(self.judge(r)[1:], ([], []))

    def test_the_suite_is_held_to_the_daemons_counts(self):
        r = as_expected('daemon')
        r['c_declared'] = 'PASS 2, WEAKER 12, NOT_EXERCISED 12'
        _, differs, _ = self.judge(r)
        self.assertTrue(any('C_DECLARED' in d for d in differs), differs)


class CheckG(unittest.TestCase):
    """Check g (M1.T23): the daemon's in-place store, a disk that fails for real."""

    def test_every_pin_of_check_g_is_in_g_pins(self):
        # The stand-in and a file system that was not set up write
        # check g's pins from G_PINS without running it; a pin left
        # out of it is "no result", a rig error (M1.T26 review).
        import re
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            listed = set(re.search(r'^G_PINS="([^"]*)"', f.read(), re.M).group(1).split())
        want = {name[len('pin_'):] for name, row in dc.EXPECTED.items() if row[0] == 'g' and name.startswith('pin_')}
        self.assertEqual(want, listed)

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
        main = text[text.index('\n    check_c\n'):]
        self.assertTrue(main.index('c_watch_back') < main.index(';;'), 'the watch is put back after both passes')
        for tag in ('c', 'cd'):
            self.assertIn(f'c_evidence_start {tag}\n', text)
            self.assertIn(f'c_evidence_end {tag} "$work" "$rc"', text)
        self.assertIn('c_watch_back', text[text.index('\ncleanup() {'):text.index('\ntrap cleanup EXIT')])

    def test_on_the_daemon_in_place_every_line_is_judged(self):
        r = as_expected('daemon')
        rows, differs, errors = dc.judge(r, ALL, impl='daemon')
        self.assertEqual((differs, errors), ([], []))
        self.assertTrue(all(want in ('PASS', 'FAIL') for name, want, *_ in rows if name in G), rows)

    def test_a_start_that_serves_the_cache_differs(self):
        r = as_expected('daemon')
        r['pin_start_serves_what_the_disk_holds'] = 'FAIL: the start served byte 0x22'
        _, differs, errors = dc.judge(r, ALL, impl='daemon')
        self.assertEqual(len(differs), 1)
        self.assertEqual(errors, [])

    def test_a_control_that_passes_differs(self):
        r = as_expected('daemon')
        r['ctl_g_old_content_after_good_sync'] = 'PASS: the start served 0x11'
        _, differs, _ = dc.judge(r, ALL, impl='daemon')
        self.assertEqual(len(differs), 1)

    def test_on_the_stand_in_it_waits(self):
        rows, differs, errors = dc.judge(as_expected(), ALL)
        self.assertEqual((differs, errors), ([], []))
        self.assertEqual({want for name, want, *_ in rows if name in G}, {'WAITING'})

    def test_its_observations_are_the_daemons(self):
        r = as_expected()
        for name in ('obs_g_failed_sync', 'obs_g_new_descriptor_after_failed_sync', 'obs_g_reads_while_failing'):
            del r[name]
        self.assertEqual(dc.judge(r, ALL)[1:], ([], []))
        r = as_expected('daemon')
        del r['obs_g_failed_sync']
        self.assertTrue(any('obs_g_failed_sync' in e for e in dc.judge(r, ALL, impl='daemon')[2]))


class CommandsFailed(unittest.TestCase):
    """M1.3aa: no command of the queues ended with a result other than OK or the driver's abort."""
    PINS = ('pin_commands_failed_zero_a', 'pin_commands_failed_zero_b', 'pin_commands_failed_zero_e',
            'pin_c_commands_failed_zero')

    def daemon(self):
        return as_expected('daemon')

    def test_a_command_the_driver_refused_differs(self):
        for name in self.PINS:
            r = self.daemon()
            r[name] = 'FAIL: status says commands_failed 1, the last with the result -22'
            _, differs, errors = dc.judge(r, ALL, impl='daemon')
            self.assertEqual((len(differs), errors), (1, []), name)

    def test_waiting_is_the_stand_ins_answer_only(self):
        for name in self.PINS:
            self.assertEqual(dc.EXPECTED[name][4], 'WAITING', name)
            self.assertNotIn(name, dc.NOT_ON_THE_DAEMON, name)
            r = self.daemon()
            r[name] = 'WAITING: the stand-in keeps no request log'
            _, differs, errors = dc.judge(r, ALL, impl='daemon')
            self.assertEqual(len(errors), 1, name)

    def test_a_build_without_the_count_is_a_rig_error(self):
        r = self.daemon()
        r['pin_c_commands_failed_zero'] = 'ERROR: this build of the daemon predates M1.3aa'
        _, differs, errors = dc.judge(r, ALL, impl='daemon')
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
        script = (f'eval "$(sed -n \'/^GJ_MAY_CHANGE=/p\' {rig!r})"; '
                  f'eval "$(sed -n \'/^gj_files_judged() {{$/,/^}}$/p\' {rig!r})"; gj_files_judged "$1" "$2"')
        return subprocess.run(['bash', '-c', script, '_', new, gone], capture_output=True, text=True, check=True).stdout

    def test_the_log_files_are_no_longer_allowed(self):
        # M1.3t removed the log store and with it the allowance for the
        # files its checkpoint rewrote at a start.
        self.assertEqual(self.judged('log.000000 log.super ', 'log.000000 log.super '),
                         'log.000000 rewritten; log.super rewritten; ')


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


class KillRefusal(unittest.TestCase):
    """A serve after a kill -9 that the server refuses (M1.T31): a FAIL of the kill test's pins, the rig's ERROR for a control."""

    BROKEN = ('skyzfsd: the kernel handed over tag 3 of device 7, which the queue does not have or whose '
              'request was still out, against the driver\'s rule')
    REFUSED = ('skyzfsd: the kernel refused serve\'s answer on tag 0 of device 7 (Invalid argument (os error 22)): '
               'nothing more is answered on this queue, the volume is not failed, and device 7 waits for the next '
               'server, which is handed again what was not answered')

    def verdict(self, serve_rc, kind, log_line='skyzfsd: the volume is marked as failed', when=''):
        import subprocess
        import tempfile
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with tempfile.TemporaryDirectory() as d:
            log = os.path.join(d, 'vol-b.serve.log')
            with open(log, 'w', encoding='utf-8') as f:
                f.write('ready\n' + log_line + '\n')
            # What kill_test sets when the serve after kill 3 did not come back.
            script = (f'eval "$(sed -n \'/^EXIT_FAILED=/p\' {rig!r})"; '
                      f'eval "$(sed -n \'/^kt_refusal() {{$/,/^}}$/p\' {rig!r})"; '
                      f'eval "$(sed -n \'/^kill_verdict() {{/,/^}}$/p\' {rig!r})"; '
                      'KT_ERR="kill 3: serve did not come back (rc 1, serve exit $1)"; '
                      'KT_REFUSED=$(kt_refusal 3 "$1" "$2" $4); KT_DONE=2; KT_INFLIGHT=2; KT_FAILED_WRITES=0; '
                      'KT_STATE=ONLINE; KT_VDEV="vdev errors 0 x"; KT_SCRUB="vdev errors 0 with 0 errors"; KT_FILES=ok; '
                      'kill_verdict $3; printf "%s\\n%s\\n" "$KT_VERDICT" "$KT_WHY"')
            out = subprocess.run(['bash', '-c', script, '_', serve_rc, log, kind, when],
                                 capture_output=True, text=True, check=True).stdout
        verdict, why = out.split('\n', 1)
        return verdict, why.strip()

    def test_a_planted_refusal_fails_the_pin_with_its_reason(self):
        for rc in ('3', '4', '5'):
            verdict, why = self.verdict(rc, 'pin')
            self.assertEqual(verdict, 'FAIL', rc)
            self.assertIn(f'kill 3: the next serve refused the volume with exit {rc}', why)
            self.assertIn('the volume is marked as failed', why)

    def test_a_planted_7_fails_the_pin_and_says_what_the_driver_did(self):
        # M1.3ac (#77): the daemon ends with 7 when the driver hands over
        # a tag whose request is still out (lead's ruling on M1.T34).
        verdict, why = self.verdict('7', 'pin', self.BROKEN)
        self.assertEqual(verdict, 'FAIL')
        self.assertIn('kill 3: the next serve ended with exit 7: the driver handed over a tag whose request was '
                      'still out', why)
        self.assertIn('the next serve should take it over', why)
        self.assertIn(self.BROKEN, why)

    def test_a_7_before_the_kill_fails_the_pin(self):
        # The server a kill was for had ended so by itself.
        verdict, why = self.verdict('7', 'pin', self.BROKEN, 'before')
        self.assertEqual(verdict, 'FAIL')
        self.assertIn('before kill 3: the server ended with exit 7: the driver handed over a tag', why)

    def test_a_7_in_a_control_stays_the_rigs_error(self):
        for when in ('', 'before'):
            verdict, _ = self.verdict('7', '', self.BROKEN, when)
            self.assertEqual(verdict, 'ERROR', when)

    def test_a_planted_8_fails_the_pin_and_says_what_the_driver_did(self):
        # M1.3ac part 3 (#83): the daemon ends with 8 when the driver
        # refused one of its commits; read as 7 is (M1.T35).
        verdict, why = self.verdict('8', 'pin', self.REFUSED)
        self.assertEqual(verdict, 'FAIL')
        self.assertIn('kill 3: the next serve ended with exit 8: the driver refused one of its answers (a commit)', why)
        self.assertIn('the next serve should be handed that request again', why)
        self.assertIn(self.REFUSED, why)

    def test_an_8_before_the_kill_fails_the_pin(self):
        verdict, why = self.verdict('8', 'pin', self.REFUSED, 'before')
        self.assertEqual(verdict, 'FAIL')
        self.assertIn('before kill 3: the server ended with exit 8: the driver refused one of its answers', why)

    def test_an_8_in_a_control_stays_the_rigs_error(self):
        for when in ('', 'before'):
            verdict, _ = self.verdict('8', '', self.REFUSED, when)
            self.assertEqual(verdict, 'ERROR', when)

    def loop(self, server):
        """The rig's own kill loop, from its server_pid to the look after
        serve_wait, for one kill of a made-up server started as `server`
        (a bash command line) -> (KT_ERR, KT_REFUSED, SERVE_RC)."""
        import subprocess
        import tempfile
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            text = f.read()
        a = text.index('        pid=$(server_pid "$impl" "$dir")\n')
        b = text.index('        # KT_WAIT_S: after the first kill', a)
        body = text[a:b]
        with tempfile.TemporaryDirectory() as d:
            for f, t in (('loop.sh', 'for n in 1; do\n' + body + 'done\n'), ('inflight', '0 1\n'), ('vol.serve.log', '')):
                with open(os.path.join(d, f), 'w', encoding='utf-8') as fh:
                    fh.write(t)
            script = (f'eval "$(sed -n \'/^EXIT_FAILED=/p\' {rig!r})"; '
                      # A one-line function is cut at its own line: a range from it
                      # would run on to the next lone } and eval the rig's lines.
                      f'for fn in now_ms server_pid serve_wait kt_refusal; do eval "$(sed -n "/^$fn() .*}}\\$/{{p;q}}; /^$fn() /,/^}}\\$/p" {rig!r})"; done; '
                      'vol_stat() { :; }; '
                      'D=$1; WORK=$D; name=kt; impl=x; dir=$D/vol; inflight=$D/inflight; BDEV=/dev/ublkb9; '
                      'KT_ERR=""; KT_REFUSED=""; KT_INFLIGHT=0; '
                      'bash -c "$2" >> "$dir.serve.log" & SERVE_PID=$!; SERVE_RC=""; sleep 0.5; '
                      '. "$D/loop.sh"; '
                      '[ -z "$SERVE_PID" ] || kill -9 "$SERVE_PID" 2>/dev/null; '
                      'printf "%s\\n%s\\n%s\\n" "$KT_ERR" "$KT_REFUSED" "$SERVE_RC"')
            out = subprocess.run(['bash', '-c', script, '_', d, server], capture_output=True, text=True,
                                 timeout=60, check=True).stdout
        return tuple(out.split('\n')[:3])

    def test_the_kill_loop_reads_a_7_from_a_server_that_ended_before_the_kill(self):
        # The shell reaps a server that ended by itself at once, so the
        # kill fails; the loop reads its status with wait (review of #82).
        err, refused, rc = self.loop('echo "skyzfsd: the kernel handed over tag 3"; exit 7')
        self.assertEqual(rc, '7')
        self.assertEqual(err, 'before kill 1: the server had ended by itself, with exit 7')
        self.assertIn('before kill 1: the server ended with exit 7: the driver handed over a tag', refused)
        self.assertIn('skyzfsd: the kernel handed over tag 3', refused)

    def test_the_kill_loop_reads_an_8_from_a_server_that_ended_before_the_kill(self):
        err, refused, rc = self.loop('echo "skyzfsd: the kernel refused serve\'s answer on tag 0"; exit 8')
        self.assertEqual(rc, '8')
        self.assertEqual(err, 'before kill 1: the server had ended by itself, with exit 8')
        self.assertIn('before kill 1: the server ended with exit 8: the driver refused one of its answers', refused)
        self.assertIn("skyzfsd: the kernel refused serve's answer on tag 0", refused)

    def test_the_kill_loop_names_another_status_as_the_rigs_error(self):
        err, refused, rc = self.loop('exit 1')
        self.assertEqual(refused, '')
        self.assertRegex(err, r'^kill 1: kill -9 \d+ failed \(the server had ended by itself, with exit 1\)$')

    def test_the_kill_loop_kills_a_live_server(self):
        err, refused, rc = self.loop('sleep 30')
        self.assertEqual((err, refused, rc), ('', '', '137'))

    def test_for_a_control_it_stays_the_rigs_error(self):
        # A control whose kills did not all happen has shown nothing,
        # and its expected result is FAIL: a refusal must not read so.
        verdict, why = self.verdict('4', '')
        self.assertEqual(verdict, 'ERROR')
        self.assertIn('serve did not come back', why)

    def test_a_serve_that_never_answered_is_the_rigs_error(self):
        # No exit status of the daemon's own: it ran out, or never began.
        for rc in ('', '124', '1', '2'):
            verdict, _ = self.verdict(rc, 'pin')
            self.assertEqual(verdict, 'ERROR', rc)

    def test_the_kill_test_pins_ask_for_it(self):
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            text = f.read()
        self.assertIn('KT_REFUSED=$(kt_refusal "$n" "${SERVE_RC:-}" "$dir.serve.log")', text)
        # kill_under_load, kill_under_load_applied_unacknowledged and
        # device_waits_for_a_server; the two controls stay plain.
        import re
        self.assertEqual(len(re.findall(r'^    kill_verdict pin\b', text, re.M)), 3)
        self.assertIn('SKZ_REF_REISSUE=0 kill_test "$REF" b0 5\n    kill_verdict; ', text)


class RefusedCommit(unittest.TestCase):
    """Check e's refused commits (M1.T35): PASS on the daemon, WAITING on the stand-in, a 0 at the stop differs."""

    def test_the_stand_in_waits(self):
        r = as_expected('reference')
        for name in RC:
            self.assertEqual(r[name].split(':')[0], 'WAITING', name)
        self.assertEqual(dc.judge(r, ALL)[1:], ([], []))

    def test_a_pass_on_the_stand_in_is_the_rigs_error_and_waiting_on_the_daemon_too(self):
        r = as_expected('reference')
        r['pin_refused_commit_at_a_stop_ends_8'] = 'PASS: seen'
        _, differs, errors = dc.judge(r, ALL)
        self.assertEqual(len(differs) + len(errors), 1)
        r = as_expected('daemon')
        r['pin_refused_write_commit_taken_over'] = 'WAITING: the stand-in has no test hooks'
        self.assertEqual(len(dc.judge(r, ALL, 'daemon')[2]), 1)

    def test_a_stop_that_ends_0_differs(self):
        # The negative control's build (before 343dd01).
        r = as_expected('daemon')
        r['pin_refused_commit_at_a_stop_ends_8'] = 'FAIL: SIGTERM while the write was held; serve ended with exit 0'
        _, differs, errors = dc.judge(r, ALL, 'daemon')
        self.assertEqual(errors, [])
        self.assertEqual(len(differs), 1)
        self.assertIn('pin_refused_commit_at_a_stop_ends_8: expected PASS, got FAIL', differs[0])

    def test_they_are_check_e(self):
        for name in RC:
            self.assertEqual(dc.EXPECTED[name][0], 'e', name)


class ZfsStuck(unittest.TestCase):
    """ZFS stuck on the machine (M1.T32): the courses after it are not run, and their lines read a named ERROR."""

    SINCE = 'd: the kept device answers again'
    LINE = (f'since "{SINCE}": zpool processes waiting in the kernel: 9 (8 for the namespace lock; 394984 in '
            'spa_config_enter); txg_sync threads\' states: R; courses not run: d, control')

    @staticmethod
    def without(r, check):
        """The run with no line of check `check` (pins, controls, observations)."""
        gone = {n for n, (c, *_) in dc.EXPECTED.items() if c == check} | {n for n, (c, _) in dc.OBSERVED.items() if c == check}
        return {k: v for k, v in r.items() if k not in gone}

    def test_a_missing_line_under_the_mark_reads_the_named_error(self):
        # Check g is of the daemon's store.
        r = self.without(as_expected('daemon'), 'g')
        r['zfs_stuck'] = self.LINE
        rows, differs, errors = dc.judge(r, ALL, 'daemon')
        g_rows = [row for row in rows if row[0] in G]
        self.assertEqual(len(g_rows), len(G))
        for name, _, got, detail, *_ in g_rows:
            self.assertEqual(got, 'ERROR', name)
            self.assertEqual(detail, f'not run: ZFS on this machine was stuck since "{self.SINCE}"')
        self.assertIn(f'obs_g_failed_sync: not run: ZFS on this machine was stuck since "{self.SINCE}"', errors)
        self.assertEqual(differs, [])
        self.assertEqual(len(errors), len(G) + sum(1 for c, _ in dc.OBSERVED.values() if c == 'g'))

    def test_without_the_mark_a_missing_line_is_still_missing(self):
        rows, _, errors = dc.judge(self.without(as_expected(), 'g'), ALL)
        self.assertIn('missing', {row[2] for row in rows if row[0] in G})
        self.assertIn(f'{G[0]}: no result', errors)

    def test_lines_written_before_keep_their_verdicts(self):
        r = self.without(as_expected(), 'g')
        r['zfs_stuck'] = self.LINE
        r['pin_clear_resumes_on_returned_device'] = 'FAIL: zpool clear rc 124; the clear ran out of its 120 s'
        r['pin_suspends_on_io_errors'] = 'PASS: pool SUSPENDED'
        rows, differs, _ = dc.judge(r, ALL)
        got = {row[0]: row[2] for row in rows}
        self.assertEqual(got['pin_clear_resumes_on_returned_device'], 'FAIL')
        self.assertEqual(got['pin_suspends_on_io_errors'], 'PASS')
        self.assertEqual(len(differs), 1)

    def test_the_mark_alone_changes_no_exit_status(self):
        m = Main()
        for change, want in ((None, 0), (('pin_discard', 'FAIL: 0'), 1)):
            r = as_expected()
            r['zfs_stuck'] = self.LINE
            if change:
                r[change[0]] = change[1]
            status, text = m.run_main([f'{k}\t{v}\n' for k, v in r.items()])
            self.assertEqual(status, want, change)
            self.assertIn('**ZFS on this machine got stuck**', text)
        r = self.without(as_expected(), 'g')
        r['zfs_stuck'] = self.LINE
        status, text = m.run_main([f'{k}\t{v}\n' for k, v in r.items()])
        self.assertEqual(status, 2)
        self.assertIn(f'| ERROR **!** | not run: ZFS on this machine was stuck since "{self.SINCE}" |', text)

    def test_check_c_not_run_says_why(self):
        r = {k: v for k, v in as_expected().items() if not k.startswith('case_') and k not in ('c_run', 'c_declared')}
        r = self.without(r, 'c')
        r['zfs_stuck'] = self.LINE
        _, _, errors = dc.judge(r, ALL)
        self.assertIn(f'c_run: not run: ZFS on this machine was stuck since "{self.SINCE}"', errors)
        self.assertIn(f'c_declared: not run: ZFS on this machine was stuck since "{self.SINCE}"', errors)

    def planted(self, state):
        """The rig's own course, record and end-of-run lines on a planted zpool command that ran out, with a
        zpool process left in `state` -> (results, the stacks file or None, whether the second course ran)."""
        import subprocess
        import tempfile
        here = os.path.dirname(os.path.abspath(__file__))
        rig = os.path.join(here, 'daemon-rig.sh')
        with tempfile.TemporaryDirectory() as d:
            for pid, comm, st in (('394984', 'zpool', state), ('388053', 'txg_sync', 'R')):
                os.makedirs(os.path.join(d, 'proc', pid))
                with open(os.path.join(d, 'proc', pid, 'comm'), 'w', encoding='utf-8') as f:
                    f.write(comm + '\n')
                with open(os.path.join(d, 'proc', pid, 'stat'), 'w', encoding='utf-8') as f:
                    f.write(f'{pid} ({comm}) {st} 1 1 1\n')
                with open(os.path.join(d, 'proc', pid, 'stack'), 'w', encoding='utf-8') as f:
                    f.write('[<0>] spa_config_enter+0x8c/0x168 [zfs]\n' if comm == 'zpool' else '[<0>] txg_sync_thread\n')
            script = (f'HERE={here!r}; OUT={d!r}; R=$OUT/results.tsv; : > "$R"; ZS_PROC=$OUT/proc; '
                      '. "$HERE/../lib/lim.sh"; . "$HERE/klog.sh"; . "$HERE/zfs-stuck.sh"; '
                      f'eval "$(sed -n \'/^rec() {{/,/^}}$/p; /^pin() /p; /^ctl() /p; /^step() /p\' {rig!r})"; '
                      f'eval "$(sed -n \'/^ZS_FILE=/,/^}}$/p\' {rig!r})"; '
                      # the planted command: a zpool clear that runs out of its limit
                      'first() { local rc; step "d: the kept device answers again"; lim 1 sleep 5; rc=$?; '
                      'pin clear_resumes_on_returned_device FAIL "zpool clear rc $rc; the clear ran out: $(zfs_stuck_say)"; }; '
                      'second() { step "d, control: a volume that does not fail"; : > "$OUT/second-ran"; '
                      'ctl volume_that_does_not_fail PASS seen; }; '
                      'course "d.1" && first; course "d, control" && second; course "d, scrub" && second; '
                      f'eval "$(sed -n \'/^if since=\\$(zfs_stuck_since); then$/,/^fi$/p\' {rig!r})"')
            subprocess.run(['bash', '-c', script], capture_output=True, text=True, check=True)
            results, _ = dc.read_results(os.path.join(d, 'results.tsv'))
            stacks = os.path.join(d, 'zfs-stuck-stacks.txt')
            stacks = open(stacks, encoding='utf-8').read() if os.path.exists(stacks) else None
            return results, stacks, os.path.exists(os.path.join(d, 'second-ran'))

    def test_a_planted_stuck_command_stops_the_courses_after_it(self):
        results, stacks, ran = self.planted('D')
        self.assertFalse(ran)
        self.assertTrue(results['pin_clear_resumes_on_returned_device'].startswith('FAIL: zpool clear rc 124;'))
        self.assertNotIn('ctl_volume_that_does_not_fail', results)
        self.assertTrue(results['zfs_stuck'].startswith(f'since "{self.SINCE}": zpool processes waiting in the kernel: 1 '))
        self.assertIn('courses not run: d, control / d, scrub;', results['zfs_stuck'])
        # The stacks: at the mark and at the end of the run, the holder's and txg_sync's.
        self.assertEqual(stacks.count('-- 394984 zpool D\n[<0>] spa_config_enter'), 2)
        self.assertEqual(stacks.count('-- 388053 txg_sync R\n'), 2)
        self.assertIn(f' {self.SINCE}\n', stacks)
        self.assertIn(' the end of the run\n', stacks)
        # And the judge: the line written keeps its FAIL, the control not run reads the named ERROR.
        r = self.without(self.without(as_expected(), 'd'), 'g')
        r.update(results)
        rows, _, errors = dc.judge(r, ALL)
        got = {row[0]: (row[2], row[3]) for row in rows}
        self.assertEqual(got['pin_clear_resumes_on_returned_device'][0], 'FAIL')
        self.assertEqual(got['ctl_volume_that_does_not_fail'],
                         ('ERROR', f'not run: ZFS on this machine was stuck since "{self.SINCE}"'))
        self.assertIn(f'ctl_volume_that_does_not_fail: not run: ZFS on this machine was stuck since "{self.SINCE}"', errors)

    def test_a_command_that_only_ran_out_is_not_a_stuck_zfs(self):
        # The zpool process sleeps and can be killed: nothing waits behind it.
        results, stacks, ran = self.planted('S')
        self.assertTrue(ran)
        self.assertEqual(results['ctl_volume_that_does_not_fail'], 'PASS: seen')
        self.assertNotIn('zfs_stuck', results)
        self.assertIsNone(stacks)

    def test_every_check_and_every_course_of_d_and_g_is_guarded(self):
        import re
        rig = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'daemon-rig.sh')
        with open(rig, encoding='utf-8') as f:
            text = f.read()
        dispatch = text[text.index('case $CHECKS in *" a "*)'):text.index('# ZFS stuck: where, what held it')]
        self.assertEqual(re.findall(r'(?<!&& )\bcheck_[a-z0-9_]+\b(?!\(\))', dispatch), [])
        self.assertEqual(len(re.findall(r'\bcourse (?:"[^"]+"|\S+) && check_', dispatch)), dispatch.count('&& check_'))
        self.assertGreaterEqual(dispatch.count('&& check_'), 8)
        for fn, courses in (('check_d', ['check_d_control', 'check_d_deleted', 'check_d_shorter', 'check_d_scrub',
                                         'check_d_not_held']),
                            ('check_g', ['check_g_start_after_failed_sync', 'check_g_failed_start',
                                         'check_g_sync_fails_while_serving', 'check_g_reason', 'check_g_data_journal'])):
            body = re.search(r'^' + fn + r'\(\) \{\n(.*?)^\}', text, re.S | re.M).group(1)
            for c in courses:
                self.assertRegex(body, r'course (\S+|"[^"]+") && ' + c + r'\n', c)
            self.assertEqual(re.findall(r'^    (check_[a-z0-9_]+)\s*$', body, re.M), [], fn)
        # The cleanup asks a stuck ZFS for no pools: each zpool command
        # there would wait out its limit and export nothing.
        cleanup = re.search(r'^cleanup\(\) \{\n(.*?)^\}', text, re.S | re.M).group(1)
        self.assertIn("    zfs_stuck_since > /dev/null || for p in $(lim 30 zpool list -H -o name", cleanup)


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



if __name__ == '__main__':
    unittest.main()
