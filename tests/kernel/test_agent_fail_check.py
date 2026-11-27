#!/usr/bin/env python3
"""Tests of agent_fail_check.py (the agent rig's failure course, afail)
with no kernel: each pin on a case as the rig records it, and on that
case with one thing wrong.
Run: python3 tests/kernel/test_agent_fail_check.py
"""
import io
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import agent_fail_check as f  # noqa: E402

LOST_U = ('lost skzcut volume=00112233445566778899aabbccddeeff note=whole reason=image-failed held=unknown '
          'step=not-imported reopen=- state=- looked_ms=- verdict=failed')
LOST_W = ('lost skzcut volume=00112233445566778899aabbccddeeff note=whole reason=image-failed held=yes '
          'step=reopened reopen=ok state=SUSPENDED looked_ms=750 verdict=lost')
LOST_A = ('lost skzcut volume=00112233445566778899aabbccddeeff note=whole reason=image-failed held=yes '
          'step=suspended reopen=- state=SUSPENDED looked_ms=- verdict=lost')


def rnd(n, took, begun='-', late=0, ok='ok', under=0):
    return (f'round {n} {ok} took_ms={took} late_ms={late} skipped=0 begun={begun} under_way={under} '
            'synced=0 bound=0')


def run(out, errs=()):
    """[(line, arrival)] and [(line, arrival)] -> the run as read_run
    makes it from the rig's files."""
    return f.read_run(''.join(ln + '\n' for ln, _ in out),
                      ''.join(f'{k}\t{at}\n' for k, (_, at) in enumerate(out, 1)),
                      ''.join(ln + '\n' for ln, _ in errs),
                      ''.join(f'{k}\t{at}\n' for k, (_, at) in enumerate(errs, 1)))


SAID_2 = ('skyzfs-agent: round 2: the volume failed in this boot (note=whole reason=image-failed held=unknown), '
          'and the pool is not imported: nothing is suspended; run goes on')
SAID_3 = ('skyzfs-agent: round 3: the volume failed in this boot (a write or sync of the volume\'s image failed), '
          'and no client held its device: no pool is touched; a start of the daemon with --acknowledge-failure '
          'serves it afresh')
SAID_4 = ('skyzfs-agent: round 4: the daemon\'s attachment changed: run ends, to be started again on the device '
          'as it is now')


def case_u():
    return {
        'run': run([(rnd(1, 9000, 'skz-1'), 1010.0), ('snap/skz-1 abc', 1010.0), (LOST_U, 1016.2),
                    (rnd(2, 30), 1016.2), (rnd(3, 50), 1121.05), (rnd(4, 100, 'skz-2', under=1), 1181.1)],
                   [(SAID_2, 1016.2), (SAID_3, 1121.05), (SAID_4, 1181.1)]),
        'rc': '12', 'ended_at': 1181.3, 'started_at': 1000.0,
        'fail': (1015.0, 15_000_000_000), 'failed_rc': '3',
        'notes': {'failed': (1015.05, 60), 'refused': (1019.0, 60), 'served': (None, None)},
        'refused_rc': '4', 'refused_at': 1020.0, 'served_at': 1130.0,
    }


TRACE_W = [(4_000_000_000, 'ask', 'before', 'skz-3'), (5_000_000_000, 'asked', 'after', 'skz-3'),
           (15_000_000_000, 'assist', 'before', 'skz-3'), (17_500_000_000, 'assist', 'after', 'skz-3')]


def case_w():
    return {
        'run': run([(rnd(1, 17500, 'skz-3', under=1), 2018.2), (LOST_W, 2019.5), (rnd(2, 1300), 2019.5)]),
        'rc': '11', 'ended_at': 2019.6, 'started_at': 2000.0,
        'fail': (2017.0, 17_000_000_000), 'failed_rc': '3',
        'notes': {'failed': (2017.02, 60)},
        'trace': list(TRACE_W), 'state': 'SUSPENDED\n', 'state_again': 'SUSPENDED\n',
        'again': {'run': run([(LOST_A, 2030.4), (rnd(1, 40), 2030.4)]), 'rc': '11', 'ended_at': 2030.5,
                  'started_at': 2030.0},
    }


def verdicts(judge, c):
    j = f.Judge('afail', out=io.StringIO())
    judge(c, j)
    return {ln.split('\t')[0]: ln.split('\t')[1] for ln in j.lines}, j


class Unknown(unittest.TestCase):
    def test_as_recorded_every_pin_holds(self):
        v, j = verdicts(f.judge_unknown, case_u())
        bad = {k: x for k, x in v.items() if x not in ('ok', 'INFO')}
        self.assertEqual(bad, {}, j.lines)
        self.assertEqual(j.status(), 0)
        for k in ('pin_U_daemon_failed', 'pin_U_lost_line', 'pin_U_step_within', 'pin_U_refused', 'pin_U_unheld',
                  'pin_U_moved', 'pin_U_default_schedule', 'premise_U_round_1', 'premise_U_refused_in_time',
                  'premise_U_served_in_time'):
            self.assertEqual(v[k], 'ok', k)
        self.assertIn('note to lost line 1150 ms', next(ln for ln in j.lines if ln.startswith('obs_U_times')))

    def wrong(self, change, key, verdict='FAILED'):
        c = case_u()
        change(c)
        v, _ = verdicts(f.judge_unknown, c)
        self.assertEqual(v[key], verdict, v)

    def lines(self, c, k, text=None, at=None):
        ln = c['run']['lines'][k]
        if text is not None:
            c['run'] = run([(text if i == k else x['text'], x['at'] if at is None or i != k else at)
                            for i, x in enumerate(c['run']['lines'])],
                           c['run']['errs'])
        elif at is not None:
            ln['at'] = at

    def test_the_daemon_ended_otherwise(self):
        self.wrong(lambda c: c.update(failed_rc='137'), 'pin_U_daemon_failed')

    def test_no_note(self):
        self.wrong(lambda c: c['notes'].update(failed=(None, None)), 'pin_U_daemon_failed')

    def test_a_note_from_before_the_failure(self):
        self.wrong(lambda c: c['notes'].update(failed=(1010.0, 60)), 'pin_U_daemon_failed')

    def test_held_no_or_another_step(self):
        for old, new in (('held=unknown', 'held=yes'), ('step=not-imported', 'step=reopened'),
                         ('verdict=failed', 'verdict=lost'), ('reason=image-failed', 'reason=marked'),
                         ('lost skzcut', 'lost other'), ('note=whole', 'note=torn')):
            with self.subTest(new=new):
                self.wrong(lambda c, o=old, n=new: self.lines(c, 2, LOST_U.replace(o, n)), 'pin_U_lost_line')

    def test_no_lost_line(self):
        def drop(c):
            c['run'] = run([(x['text'], x['at']) for i, x in enumerate(c['run']['lines']) if i != 2],
                           c['run']['errs'])
        self.wrong(drop, 'pin_U_lost_line')
        self.wrong(drop, 'pin_U_step_within')

    def test_two_lost_lines(self):
        def two(c):
            ls = [(x['text'], x['at']) for x in c['run']['lines']]
            ls.insert(5, (LOST_U, 1180.0))
            c['run'] = run(ls, c['run']['errs'])
        self.wrong(two, 'pin_U_lost_line')

    def test_a_line_between_the_lost_line_and_round_2(self):
        def between(c):
            ls = [(x['text'], x['at']) for x in c['run']['lines']]
            ls.insert(3, ('snap/skz-9 abc', 1016.2))
            c['run'] = run(ls, c['run']['errs'])
        self.wrong(between, 'pin_U_lost_line')

    def test_the_run_did_not_go_on(self):
        def stop(c):
            c['run'] = run([(x['text'], x['at']) for x in c['run']['lines'][:4]], c['run']['errs'][:1])
        self.wrong(stop, 'pin_U_lost_line')

    def test_round_2_failed_or_began(self):
        self.wrong(lambda c: self.lines(c, 3, rnd(2, 30, ok='failed')), 'pin_U_lost_line')
        self.wrong(lambda c: self.lines(c, 3, rnd(2, 30, 'skz-9')), 'pin_U_lost_line')

    def test_round_2_not_said(self):
        self.wrong(lambda c: c['run'].update(errs=c['run']['errs'][1:]), 'pin_U_lost_line')

    def test_the_step_at_the_tick(self):
        def late(c):
            c['run']['lines'][2]['at'] = 1075.0
        self.wrong(late, 'pin_U_step_within')

    def test_the_step_just_within(self):
        c = case_u()
        c['run']['lines'][2]['at'] = 1015.05 + f.STEP_WITHIN
        self.assertEqual(verdicts(f.judge_unknown, c)[0]['pin_U_step_within'], 'ok')
        c['run']['lines'][2]['at'] = 1015.05 + f.STEP_WITHIN + 0.01
        self.assertEqual(verdicts(f.judge_unknown, c)[0]['pin_U_step_within'], 'FAILED')

    def test_the_start_served_or_no_note_after_it(self):
        self.wrong(lambda c: c.update(refused_rc='124'), 'pin_U_refused')
        self.wrong(lambda c: c['notes'].update(refused=(None, None)), 'pin_U_refused')

    def test_round_3_lost_or_not_said(self):
        def lost3(c):
            ls = [(x['text'], x['at']) for x in c['run']['lines']]
            ls.insert(4, (LOST_U, 1121.0))
            c['run'] = run(ls, c['run']['errs'])
        self.wrong(lost3, 'pin_U_unheld')
        self.wrong(lambda c: c['run'].update(errs=[c['run']['errs'][0], c['run']['errs'][2]]), 'pin_U_unheld')
        self.wrong(lambda c: self.lines(c, 4, rnd(3, 50, ok='failed')), 'pin_U_unheld')

    def test_not_12(self):
        self.wrong(lambda c: c.update(rc='0'), 'pin_U_moved')
        self.wrong(lambda c: c.update(ended_at=1190.0), 'pin_U_moved')
        self.wrong(lambda c: c['run'].update(errs=c['run']['errs'][:2]), 'pin_U_moved')
        self.wrong(lambda c: c['run'].update(errs=c['run']['errs'][:2] + [(SAID_4.replace('round 4', 'round 3'),
                                                                           1181.1)]), 'pin_U_moved')

    def test_a_line_after_round_4(self):
        def more(c):
            ls = [(x['text'], x['at']) for x in c['run']['lines']] + [(rnd(5, 10), 1241.0)]
            c['run'] = run(ls, c['run']['errs'])
        self.wrong(more, 'pin_U_moved')

    def test_the_ticks_moved_by_the_early_round(self):
        # Round 3 a tick after round 2 and not 120 s after round 1.
        self.wrong(lambda c: self.lines(c, 4, at=1076.25), 'pin_U_default_schedule')
        self.wrong(lambda c: self.lines(c, 5, at=1185.0), 'pin_U_default_schedule')

    def test_the_tick_with_late_ms(self):
        c = case_u()
        c['run'] = run([(x['text'] if i != 4 else rnd(3, 50, late=1500), x['at'] + (1.5 if i == 4 else 0))
                        for i, x in enumerate(c['run']['lines'])], c['run']['errs'])
        self.assertEqual(verdicts(f.judge_unknown, c)[0]['pin_U_default_schedule'], 'ok')

    def test_the_rig_late(self):
        self.wrong(lambda c: c.update(refused_at=1125.0), 'premise_U_refused_in_time', 'ERROR')
        self.wrong(lambda c: c.update(served_at=1185.0), 'premise_U_served_in_time', 'ERROR')
        self.wrong(lambda c: self.lines(c, 0, at=1015.5), 'premise_U_round_1', 'ERROR')
        c = case_u()
        c.update(served_at=1185.0)
        self.assertEqual(verdicts(f.judge_unknown, c)[1].status(), 2)


class Wait(unittest.TestCase):
    def test_as_recorded_every_pin_holds(self):
        v, j = verdicts(f.judge_wait, case_w())
        bad = {k: x for k, x in v.items() if x not in ('ok', 'INFO')}
        self.assertEqual(bad, {}, j.lines)
        for k in ('premise_W_waiting', 'pin_W_daemon_failed', 'pin_W_lost_line', 'pin_W_exit', 'pin_W_suspended',
                  'pin_W_again'):
            self.assertEqual(v[k], 'ok', k)
        flight = next(ln for ln in j.lines if ln.startswith('obs_W_in_flight'))
        self.assertIn('one in flight at it: yes, returned 500 ms after the failure', flight)
        times = next(ln for ln in j.lines if ln.startswith('obs_W_times'))
        self.assertIn('round 1\'s line to the lost line 1300 ms', times)

    def wrong(self, change, key, verdict='FAILED'):
        c = case_w()
        change(c)
        v, _ = verdicts(f.judge_wait, c)
        self.assertEqual(v[key], verdict, v)

    def test_zed_suspended_it_first(self):
        c = case_w()
        c['run'] = run([(rnd(1, 17500, 'skz-3', under=1), 2018.2), (LOST_A, 2018.3), (rnd(2, 20), 2018.3)])
        c['ended_at'] = 2018.4
        self.assertEqual(verdicts(f.judge_wait, c)[0]['pin_W_lost_line'], 'ok')

    def test_a_reopen_that_failed_is_said_and_allowed(self):
        c = case_w()
        c['run'] = run([(rnd(1, 17500, 'skz-3', under=1), 2018.2), (LOST_W.replace('reopen=ok', 'reopen=failed'),
                                                                     2019.5), (rnd(2, 1300), 2019.5)])
        self.assertEqual(verdicts(f.judge_wait, c)[0]['pin_W_lost_line'], 'ok')

    def test_the_lost_line_wrong(self):
        for old, new in (('held=yes', 'held=unknown'), ('verdict=lost', 'verdict=retry'),
                         ('state=SUSPENDED', 'state=ONLINE'), ('step=reopened', 'step=not-imported'),
                         ('looked_ms=750', 'looked_ms=10600'), ('looked_ms=750', 'looked_ms=-'),
                         ('reopen=ok', 'reopen=-')):
            with self.subTest(new=new):
                def change(c, o=old, n=new):
                    c['run'] = run([(rnd(1, 17500, 'skz-3', under=1), 2018.2), (LOST_W.replace(o, n), 2019.5),
                                    (rnd(2, 1300), 2019.5)])
                self.wrong(change, 'pin_W_lost_line')

    def test_suspended_with_a_reopen(self):
        def change(c):
            c['run'] = run([(rnd(1, 17500, 'skz-3', under=1), 2018.2), (LOST_A.replace('reopen=-', 'reopen=ok'),
                                                                         2019.5), (rnd(2, 1300), 2019.5)])
        self.wrong(change, 'pin_W_lost_line')

    def test_the_lost_round_began_a_publish(self):
        def change(c):
            c['run'] = run([(rnd(1, 17500, 'skz-3', under=1), 2018.2), (LOST_W, 2019.5),
                            (rnd(2, 1300, 'skz-4'), 2019.5)])
        self.wrong(change, 'pin_W_lost_line')

    def test_no_round_1_before_the_lost_line(self):
        def change(c):
            c['run'] = run([(LOST_W, 2019.5), (rnd(1, 1300), 2019.5)])
        self.wrong(change, 'pin_W_lost_line')

    def test_a_round_after_the_lost_one(self):
        def change(c):
            c['run'] = run([(rnd(1, 17500, 'skz-3', under=1), 2018.2), (LOST_W, 2019.5), (rnd(2, 1300), 2019.5),
                            (rnd(3, 10), 2079.0)])
        self.wrong(change, 'pin_W_lost_line')

    def test_not_11_or_late(self):
        self.wrong(lambda c: c.update(rc='0'), 'pin_W_exit')
        self.wrong(lambda c: c.update(rc='timeout'), 'pin_W_exit')
        self.wrong(lambda c: c.update(ended_at=2025.0), 'pin_W_exit')

    def test_not_suspended(self):
        self.wrong(lambda c: c.update(state='ONLINE\n'), 'pin_W_suspended')

    def test_again_wrong(self):
        def reopened(c):
            c['again']['run'] = run([(LOST_W, 2030.4), (rnd(1, 40), 2030.4)])
        self.wrong(reopened, 'pin_W_again')
        self.wrong(lambda c: c['again'].update(rc='1'), 'pin_W_again')

        def two(c):
            c['again']['run'] = run([(rnd(1, 40), 2030.4), (LOST_A, 2031.4), (rnd(2, 40), 2031.4)])
        self.wrong(two, 'pin_W_again')

    def test_the_premise(self):
        self.wrong(lambda c: c.update(trace=TRACE_W[:1]), 'premise_W_waiting', 'ERROR')
        self.wrong(lambda c: c.update(trace=[TRACE_W[0], (5_000_000_000, 'asked', 'before', 'skz-3')]),
                   'premise_W_waiting', 'ERROR')

        def early(c):
            c['run'] = run([(rnd(1, 1000, 'skz-3'), 2016.0), (LOST_W, 2019.5), (rnd(2, 1300), 2019.5)])
        self.wrong(early, 'premise_W_waiting', 'ERROR')

    def test_the_reopen_s_time(self):
        _, j = verdicts(f.judge_wait, case_w())
        self.assertIn('the reopen about 550 ms', next(ln for ln in j.lines if ln.startswith('obs_W_times')))
        c = case_w()
        c['run'] = run([(rnd(1, 17500, 'skz-3', under=1), 2018.2), (LOST_A, 2018.3), (rnd(2, 20), 2018.3)])
        _, j = verdicts(f.judge_wait, c)
        self.assertNotIn('the reopen', next(ln for ln in j.lines if ln.startswith('obs_W_times')))
        # Only a reopened step has a reopen to time, whatever its looked_ms.
        c['run'] = run([(rnd(1, 17500, 'skz-3', under=1), 2018.2),
                        (LOST_A.replace('looked_ms=-', 'looked_ms=50'), 2018.3), (rnd(2, 20), 2018.3)])
        _, j = verdicts(f.judge_wait, c)
        self.assertNotIn('the reopen', next(ln for ln in j.lines if ln.startswith('obs_W_times')))

    def test_no_assist_in_flight(self):
        c = case_w()
        c['trace'] = TRACE_W[:2]
        _, j = verdicts(f.judge_wait, c)
        flight = next(ln for ln in j.lines if ln.startswith('obs_W_in_flight'))
        self.assertIn('last point before the failure: asked after', flight)
        self.assertIn('one in flight at it: no', flight)

    def test_an_assist_that_never_returned(self):
        c = case_w()
        c['trace'] = TRACE_W[:3]
        _, j = verdicts(f.judge_wait, c)
        self.assertIn('never returned in the trace', next(ln for ln in j.lines if ln.startswith('obs_W_in_flight')))

    def test_daemon(self):
        self.wrong(lambda c: c.update(failed_rc='timeout'), 'pin_W_daemon_failed')


class Reading(unittest.TestCase):
    def test_a_line_of_no_kind(self):
        with self.assertRaises(f.RigError):
            run([('hello', 1.0)])

    def test_a_lost_line_with_a_field_more(self):
        with self.assertRaises(f.RigError):
            run([(LOST_W + ' more=1', 1.0)])

    def test_stamps(self):
        with self.assertRaises(f.RigError):
            f.read_stamps('2\t1.0\n', 'out.tsv')
        self.assertEqual(f.read_stamps('1\t1.5\n2\t2.5\n', 'x'), [1.5, 2.5])

    def test_a_line_not_yet_stamped(self):
        r = f.read_run(rnd(1, 10) + '\n' + rnd(2, 10) + '\n', '1\t5.0\n', '', '')
        self.assertEqual([ln['at'] for ln in r['lines']], [5.0, None])

    def test_notes_and_fail_at(self):
        self.assertEqual(f.read_notes('failed\t10.5\t60\nrefused\tnone\t-\n'),
                         {'failed': (10.5, 60), 'refused': (None, None)})
        with self.assertRaises(f.RigError):
            f.read_notes('failed\t10.5\n')
        self.assertEqual(f.read_fail_at('12.5\t99\n'), (12.5, 99))
        with self.assertRaises(f.RigError):
            f.read_fail_at('12.5\n')

    def test_zed(self):
        for text, want in (('want\ton\nruns\ton\npid\t5\n', 'ok'), ('want\toff\nruns\toff\npid\t-\n', 'ok'),
                           ('want\ton\nruns\toff\npid\t-\n', 'ERROR'), ('want\tmaybe\nruns\tmaybe\n', 'ERROR')):
            j = f.Judge('afail', out=io.StringIO())
            f.judge_zed(text, j)
            self.assertEqual(j.lines[0].split('\t')[1], want, text)


def write_case(d, cid, c):
    os.makedirs(os.path.join(d, 'cases', cid), exist_ok=True)

    def put(name, text):
        with open(os.path.join(d, 'cases', cid, name), 'w') as fh:
            fh.write(text)

    def put_run(label, r, rc, started, ended):
        put(f'{label}.out', ''.join(x['text'] + '\n' for x in r['lines']))
        put(f'{label}.out.tsv', ''.join(f'{k}\t{x["at"]}\n' for k, x in enumerate(r['lines'], 1)))
        put(f'{label}.err', ''.join(e + '\n' for e, _ in r['errs']))
        put(f'{label}.err.tsv', ''.join(f'{k}\t{at}\n' for k, (_, at) in enumerate(r['errs'], 1)))
        put(f'{label}.rc', rc + '\n')
        put(f'{label}.started-at', f'{started}\n')
        put(f'{label}.ended-at', f'{ended}\n')

    put_run('publish', c['run'], c['rc'], c['started_at'], c['ended_at'])
    put('fail-at', f"{c['fail'][0]:.6f}\t{c['fail'][1]}\n")
    put('failed.rc', c['failed_rc'] + '\n')
    put('note.tsv', ''.join(f'{k}\t{"none" if m is None else m}\t{"-" if s is None else s}\n'
                            for k, (m, s) in c['notes'].items()))
    if cid == 'U':
        put('refused.rc', c['refused_rc'] + '\n')
        put('refused-at', f"{c['refused_at']}\n")
        put('served-at', f"{c['served_at']}\n")
    else:
        put('trace.tsv', ''.join(f'{t[0]}\t{t[1]}\t{t[2]}\t{t[3]}\n' for t in c['trace']))
        put('pool-state.txt', c['state'])
        put('pool-state-again.txt', c['state_again'])
        a = c['again']
        put_run('again', a['run'], a['rc'], a['started_at'], a['ended_at'])


class Course(unittest.TestCase):
    def course(self, change=None):
        d = tempfile.mkdtemp()
        self.addCleanup(lambda: subprocess.run(['rm', '-rf', d], check=False))
        u, w = case_u(), case_w()
        write_case(d, 'U', u)
        write_case(d, 'W', w)
        with open(os.path.join(d, 'cases.tsv'), 'w') as fh:
            fh.write('U\tunknown\t-\t12\t-\nW\twait\t-\t11\t-\n')
        with open(os.path.join(d, 'zed.tsv'), 'w') as fh:
            fh.write('want\toff\nruns\toff\npid\t-\n')
        if change:
            change(d)
        return d

    def results(self, d, argv=None):
        out = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'agent_check.py'), 'afail', d,
                              os.path.join(d, 'requests.tsv'), '-', '1048576'] if argv is None else argv,
                             capture_output=True, text=True, check=False)
        with open(os.path.join(d, 'results.tsv')) as fh:
            return out.returncode, {ln.split('\t')[0]: ln.split('\t')[1] for ln in fh.read().splitlines()}

    def test_the_whole_course_through_agent_check(self):
        rc, v = self.results(self.course())
        self.assertEqual({k: x for k, x in v.items() if x not in ('ok', 'INFO')}, {})
        self.assertEqual(rc, 0)
        self.assertIn('pin_W_again', v)
        self.assertIn('pin_U_moved', v)

    def test_a_verdict_of_the_recorder(self):
        def verdict(d):
            with open(os.path.join(d, 'cases', 'W', 'verdicts.tsv'), 'w') as fh:
                fh.write('publish_ended\tthe agent\'s publish had not ended 90 s on\n')
        rc, v = self.results(self.course(verdict))
        self.assertEqual(v['pin_W_publish_ended'], 'FAILED')
        self.assertEqual(rc, 1)

    def test_the_rig_s_error_in_a_case(self):
        def err(d):
            with open(os.path.join(d, 'cases', 'U', 'rig-error.txt'), 'w') as fh:
                fh.write('round 1 left 1000 ms to its tick\n')
        rc, v = self.results(self.course(err))
        self.assertEqual(v['premise_U_rig'], 'ERROR')
        self.assertNotIn('pin_U_moved', v)
        self.assertEqual(v['pin_W_again'], 'ok')
        self.assertEqual(rc, 2)

    def test_a_case_not_reached(self):
        rc, v = self.results(self.course(lambda d: subprocess.run(['rm', '-rf', os.path.join(d, 'cases', 'W')],
                                                                  check=True)))
        self.assertEqual(v['case_W'], 'INFO')
        self.assertEqual(rc, 0)

    def test_a_missing_file(self):
        rc, v = self.results(self.course(lambda d: os.remove(os.path.join(d, 'cases', 'W', 'fail-at'))))
        self.assertEqual(v['premise_W_files'], 'ERROR')
        self.assertEqual(v['pin_U_moved'], 'ok')
        self.assertEqual(rc, 2)

    def test_the_course_s_error(self):
        def err(d):
            with open(os.path.join(d, 'rig-error.tsv'), 'w') as fh:
                fh.write('W\tthe writer did not end\n')
        rc, v = self.results(self.course(err))
        self.assertEqual(v['rig_error_in_the_course'], 'ERROR')
        self.assertEqual(rc, 2)

    def test_the_suspended_words_counted(self):
        def said(d):
            with open(os.path.join(d, 'zed.log'), 'w') as fh:
                fh.write('a\nzed: cannot open: pool I/O is currently suspended\nb\n')
            with open(os.path.join(d, 'cases', 'W', 'again.err'), 'a') as fh:
                fh.write('x: pool I/O is currently suspended\ny: pool I/O is currently suspended\n')
        d = self.course(said)
        rc, v = self.results(d)
        self.assertEqual(v['obs_suspended_said'], 'INFO')
        with open(os.path.join(d, 'results.tsv')) as fh:
            line = next(ln for ln in fh.read().splitlines() if ln.startswith('obs_suspended_said'))
        self.assertIn("zed's journal 1, U's run 0, W's run 0, W's second run 2, zpool status 0", line)
        self.assertEqual(rc, 0)

    def test_zed_not_as_asked(self):
        def zed(d):
            with open(os.path.join(d, 'zed.tsv'), 'w') as fh:
                fh.write('want\ton\nruns\toff\npid\t-\n')
        rc, v = self.results(self.course(zed))
        self.assertEqual(v['premise_zed'], 'ERROR')
        self.assertEqual(rc, 2)

    def test_a_verdict_beside_the_rig_s_error_is_1(self):
        def both(d):
            with open(os.path.join(d, 'zed.tsv'), 'w') as fh:
                fh.write('want\ton\nruns\toff\npid\t-\n')
            with open(os.path.join(d, 'cases', 'W', 'pool-state.txt'), 'w') as fh:
                fh.write('ONLINE\n')
        rc, v = self.results(self.course(both))
        self.assertEqual(v['pin_W_suspended'], 'FAILED')
        self.assertEqual(rc, 1)

    def test_another_course_name(self):
        d = self.course()
        out = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'agent_fail_check.py'), 'aplain', d],
                             capture_output=True, text=True, check=False)
        self.assertEqual(out.returncode, 2)


if __name__ == '__main__':
    unittest.main()
