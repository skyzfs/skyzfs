#!/usr/bin/env python3
"""The cut rig on the daemon's check (cut_daemon_check.py), its `main`
from end to end with no kernel: that a failed line fails the check, that
the rig's own error ends with 2. Each test writes a recording's directory
from the fixture `life` (the request log, the marks, a status, its times,
and each session's kstat as the reference's own cuts give it) and runs
the check as cut-record.sh does. Written by the review of #99 (S2) and
taken in with it.
"""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_check  # noqa: E402
import cut_daemon_check as cdc  # noqa: E402
import cut_ref  # noqa: E402
import test_cut_daemon_check as t  # noqa: E402


def recording(tamper_log=None, status=None, name='life', tamper_kstat=None, as_name=None, holder=None,
              split=None, tamper_split=None):
    """A recording's directory -> (exit code, {line: (verdict, detail)}).

    holder: where given, the daemon held cuts and the holder ran: its
    files as a right daemon leaves them, with holder(written, blocks,
    final) changing them in place. split: (k, kind) splits the log in two
    instances at the k-th cut (t.split_log), the second having
    `taken-over` or `added` the device, or `acknowledged` a failure after
    the k-th cut's line (t.split_begun: a new state, which counts from 1
    again; the holder's numbers then repeat, and failure.tsv is the
    rig's as the recording `failed` leaves it)."""
    records, read, dev = t.fixture(name)
    cs, at = t.daemon_cuts_of(records, read, dev)
    text, marks = t.request_log(records, read, dev, at, hashes=holder is not None)
    if tamper_log:
        text = tamper_log(text)
    d = tempfile.mkdtemp(prefix='cdc-')
    try:
        if holder is not None:
            text = text.replace(f'\t{t.PID}\tstart\t0\t-\t-\t-\t64\n',
                                f'\t{t.PID}\tstart\t0\t-\t-\t-\t64\n1015\t{t.PID}\tcuts\t-\t-\t-\t-\tbegun\n', 1)
            written, blocks, final = t.held_images(records, cs, cut_ref.opens(records, read, dev, cs))
            holder(written, blocks, final)
            starts = (t.START_STATUS,)
            if split and split[1] == 'acknowledged':
                k = split[0]
                text, ns = t.split_begun(text, k)
                if tamper_split:
                    text = tamper_split(text)
                with open(os.path.join(d, 'instances.tsv'), 'w') as f:
                    f.write(f'{t.PID2}\tadded\tacknowledged\n')
                with open(os.path.join(d, 'failure.tsv'), 'w') as f:
                    f.write('cuts_held\t1\nfailed_rc\t3\nrefused_rc\t4\n')
                starts += ('cuts 0\ncut_state begun\nintent_state taken_up\n',)
                blocks = [blocks[w['number']] for w in written]
                written = [dict(w, number=w['number'] - k, asked=ns + 1) if w['number'] > k else w for w in written]
            elif split:
                text = t.split_log(text, split[0])
                with open(os.path.join(d, 'instances.tsv'), 'w') as f:
                    f.write(f'{t.PID2}\t{split[1]}\n')
                starts += (f'cuts {split[0] - 1}\ncut_state taken_up\nintent_state taken_up\n',)
            t.holder_files(d, written, blocks, final, starts=starts)
            status = status or held_status(records, read, dev, cs)
        return run_in(d, text, marks, records, read, dev, cs, status, name, tamper_kstat, as_name)
    finally:
        shutil.rmtree(d, ignore_errors=True)


def held_status(records, read, dev, cs):
    op = cut_ref.opens(records, read, dev, cs)
    last = cs[-1]
    return (f"cuts {len(cs)}\ncut_group {last['txg']}\ncut_opens {'itself' if op[last['n']]['itself'] else 'rewind'}"
            f"\nage_ms 0\ncut_state begun\ncuts_abandoned 0\n")


def run_in(d, text, marks, records, read, dev, cs, status, name, tamper_kstat, as_name):
    log = os.path.join(d, 'requests.tsv')
    with open(log, 'w') as f:
        f.write(text)
    with open(os.path.join(d, 'marks.tsv'), 'w') as f:
        f.write(marks)
    op = cut_ref.opens(records, read, dev, cs)
    last = cs[-1]
    with open(os.path.join(d, 'status.txt'), 'w') as f:
        f.write(status or f"cuts {len(cs)}\ncut_group {last['txg']}\n"
                f"cut_opens {'itself' if op[last['n']]['itself'] else 'rewind'}\nage_ms 0\n")
    end = max(int(ln.split('\t')[0]) for ln in text.splitlines()) + 1000
    with open(os.path.join(d, 'status-time.tsv'), 'w') as f:
        f.write(f'{end}\t{end + 1}\n')
    # Each session's kstat: the groups of the reference's cuts inside it.
    skip = cut_ref.stale(cs)
    for k, (lo, hi) in cut_check.sessions(records).items():
        groups = [c['txg'] for c in cs if lo < c['ub_record'] < hi and c['n'] not in skip]
        if tamper_kstat:
            groups = tamper_kstat(k, groups)
        with open(os.path.join(d, f'txgs-{k}.txt'), 'w') as f:
            f.write(''.join(f'{g} 0 C 0 0 1\n' for g in groups))
    p = subprocess.run([sys.executable, '-I', '-B', cdc.__file__, as_name or name, d, log, '-', str(dev)],
                       capture_output=True, text=True)
    lines = {}
    with open(os.path.join(d, 'results.tsv')) as f:
        for ln in f:
            key, verdict, detail = ln.rstrip('\n').split('\t', 2)
            lines[key] = (verdict, detail)
    return p.returncode, lines


def without_cut(k):
    """The log without its k-th cut line (from the end where k is negative)."""
    def tamper(text):
        lines = text.splitlines()
        cuts = [i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'cut']
        del lines[cuts[k]]
        return '\n'.join(lines) + '\n'
    return tamper


def cut_at_the_next_barrier(k):
    """The log with its k-th cut line after the next barrier that has no cut, as that barrier's."""
    def tamper(text):
        lines = text.splitlines()
        cuts = [i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'cut']
        cut = lines.pop(cuts[k]).split('\t')
        for j in range(cuts[k], len(lines)):
            f = lines[j].split('\t')
            nxt = lines[j + 1].split('\t')[2] if j + 1 < len(lines) else ''
            if f[2] == 'barrier' and nxt != 'cut':
                cut[0], cut[3], cut[4] = str(int(f[0]) + 1), f[3], f[4]
                lines.insert(j + 1, '\t'.join(cut))
                return '\n'.join(lines) + '\n'
        raise AssertionError('no later barrier without a cut')
    return tamper


def numbered(numbers):
    """The log with its cut lines numbered by numbers(k), k from 1."""
    def tamper(text):
        out, k = [], 0
        for ln in text.splitlines():
            f = ln.split('\t')
            if f[2] == 'cut':
                k += 1
                f[6] = str(numbers(k))
            out.append('\t'.join(f))
        return '\n'.join(out) + '\n'
    return tamper


class Main(unittest.TestCase):
    def test_a_right_daemon(self):
        rc, lines = recording()
        self.assertEqual(rc, 0, lines)
        for key in ('cuts', 'status', 'sessions', 'import_syncs_before_return'):
            self.assertEqual(lines[key][0], 'PASS', (key, lines[key]))
        for ctl in cdc.CONTROLS:
            want = cdc.EXPECTED[ctl]['life']
            self.assertIn(lines[f'ctl_{ctl}'][0].lower(), want if isinstance(want, tuple) else (want,),
                          (ctl, lines[f'ctl_{ctl}']))

    def test_a_cut_the_daemon_did_not_count(self):
        for k in (3, -1):
            rc, lines = recording(without_cut(k))
            self.assertEqual(lines['cuts'][0], 'FAIL', (k, lines['cuts']))
            self.assertEqual(rc, 1, k)

    def test_a_status_one_cut_behind(self):
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        rc, lines = recording(status=f"cuts {len(cs) - 1}\ncut_group {cs[-2]['txg']}\ncut_opens itself\nage_ms 0\n")
        self.assertEqual(lines['status'][0], 'FAIL', lines['status'])
        self.assertEqual(rc, 1)

    def test_a_cut_counted_at_the_flush_after_its_own(self):
        rc, lines = recording(cut_at_the_next_barrier(3))
        self.assertEqual(lines['cuts'][0], 'FAIL', lines['cuts'])
        self.assertEqual(rc, 1)

    def test_a_session_whose_kstat_has_a_group_the_daemon_did_not_cut(self):
        rc, lines = recording(tamper_kstat=lambda k, groups: groups + [groups[-1] + 1] if k == 1 else groups)
        self.assertEqual(lines['sessions'][0], 'FAIL', lines['sessions'])
        self.assertEqual(rc, 1)

    def test_cut_numbers(self):
        rc, lines = recording(numbered(lambda k: k))
        self.assertEqual((rc, lines['cut_numbers'][0]), (0, 'PASS'), lines['cut_numbers'])
        rc, lines = recording(numbered(lambda k: k + 1))
        self.assertEqual((rc, lines['cut_numbers'][0]), (1, 'FAIL'), lines['cut_numbers'])

    def test_a_failed_cuts_with_what_the_check_cannot_place(self):
        """A flush after a write with FUA has no recv line: where `cuts`
        fails in a log that holds one, the check cannot say whose fault it
        is (the review of #99, S4, and Codex's)."""
        def tamper(text):
            text = without_cut(3)(text)
            lines = text.splitlines()
            k = next(i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'commit')
            f = lines[k].split('\t')
            lines.insert(k + 1, '\t'.join([str(int(f[0]) + 1), f[1], 'barrier', f[3], 'fua_flush', '-', '-', '-']))
            return '\n'.join(lines) + '\n'
        rc, lines = recording(tamper)
        self.assertEqual(lines['cuts'][0], 'ERROR', lines['cuts'])
        self.assertIn('1 flushes after a write with FUA', lines['cuts'][1])
        self.assertEqual(rc, 2)

    def test_a_control_that_differs_from_its_table(self):
        """life judged as rewind: the rule txg-alone gives life's own cuts
        (same), where the table says rewind's must be caught. A control
        that differs from its table, in either direction, fails the check
        (the review of #99, its next look)."""
        rc, lines = recording(as_name='rewind')
        self.assertEqual(lines['cuts'][0], 'PASS', lines['cuts'])
        self.assertEqual(lines['ctl_txg-alone'][0], 'SAME', lines['ctl_txg-alone'])
        self.assertIn('DIFFERS', lines['ctl_txg-alone'][1])
        self.assertEqual(rc, 1)

    def test_the_second_stage(self):
        """The holder's files make main judge the held cuts; a block held
        wrong fails the check (the review of #103, S1)."""
        rc, lines = recording(holder=lambda written, blocks, final: None)
        self.assertEqual(rc, 0, lines)
        for key in ('cut_state', 'cuts_held', 'held_content'):
            self.assertEqual(lines[key][0], 'PASS', (key, lines[key]))
        self.assertEqual(lines['cuts_abandoned'][0], 'INFO')

        def wrong(written, blocks, final):
            m = blocks[written[len(written) // 2]['number']]
            m[sorted(m)[0]] = '0' * 16
        rc, lines = recording(holder=wrong)
        self.assertEqual((rc, lines['held_content'][0]), (1, 'FAIL'), lines['held_content'])

    def test_the_second_stage_knows_the_recording(self):
        """bound must abandon a cut: life's directory judged as bound fails
        that line, where life itself has it as information."""
        rc, lines = recording(holder=lambda written, blocks, final: None, as_name='bound')
        self.assertEqual(lines['cuts_abandoned'][0], 'FAIL', lines['cuts_abandoned'])
        self.assertEqual(rc, 1)

    def test_an_acknowledged_failure(self):
        """The recording `failed` (stage 2c): a later start that
        acknowledged a failure begins a state whose count starts at 1
        again; the held cuts of both states are judged, keyed by state,
        and the status holds the new state's count. A daemon that took
        the state up there fails cut_state and instance_2."""
        k = 13
        none = lambda written, blocks, final: None    # noqa: E731
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        status = held_status(records, read, dev, cs).replace(f'cuts {len(cs)}\n', f'cuts {len(cs) - k}\n')
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=status, as_name='failed')
        for key in ('cuts', 'cut_numbers', 'status', 'cut_state', 'held_content', 'instance_1', 'instance_2',
                    'failure', 'held_reads'):
            self.assertEqual(lines[key][0], 'PASS', (key, lines[key]))
        self.assertIn('state 2 counts from 1 again', lines['cut_numbers'][1])
        self.assertIn('acknowledging a failure', lines['instance_2'][1])
        self.assertEqual(rc, 0, {k: v for k, v in lines.items() if 'DIFFERS' in v[1]})
        # the total, as a daemon that counted on would say it
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=held_status(records, read, dev, cs),
                              as_name='failed')
        self.assertEqual((rc, lines['status'][0]), (1, 'FAIL'))
        # the second start took the state up
        took = lambda text: text.replace(f'\t{t.PID2}\tcuts\t-\t-\t-\t-\tbegun', f'\t{t.PID2}\tcuts\t-\t-\t-\t-\ttaken_up')  # noqa: E731,E501
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=status, as_name='failed',
                              tamper_split=took)
        self.assertEqual((rc, lines['cut_state'][0], lines['instance_2'][0]), (1, 'FAIL', 'FAIL'))

    def test_two_instances(self):
        """A take-over between a flush's receipt and its barrier: the
        reference cuts at the flush the next instance is handed again,
        the numbers count on, the held cuts are written across it, and
        the rule mark-dropped is caught; the rule mark-kept is not, the
        device not having been created, so as takeover the check fails
        on that control's row alone. The same log with the device
        created is a verdict: the mark is gone, and the daemon's cut
        there is one the reference has not."""
        k = 13
        none = lambda written, blocks, final: None    # noqa: E731
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        status = held_status(records, read, dev, cs).replace('cut_state begun', 'cut_state taken_up')
        rc, lines = recording(holder=none, split=(k, 'taken-over'), status=status, as_name='takeover')
        for key in ('cuts', 'cut_numbers', 'status', 'cut_state', 'held_content', 'instance_1', 'instance_2'):
            self.assertEqual(lines[key][0], 'PASS', (key, lines[key]))
        self.assertIn('took the device over', lines['instance_2'][1])
        self.assertEqual(lines['ctl_mark-dropped'][0], 'CAUGHT', lines['ctl_mark-dropped'])
        self.assertNotIn('DIFFERS', lines['ctl_mark-dropped'][1])
        self.assertEqual(lines['ctl_mark-kept'][0], 'SAME', lines['ctl_mark-kept'])
        self.assertIn('DIFFERS', lines['ctl_mark-kept'][1])
        self.assertEqual(rc, 1)
        # as life, whose table says same for both
        rc, lines = recording(holder=none, split=(k, 'taken-over'), status=status)
        self.assertIn('DIFFERS', lines['ctl_mark-dropped'][1])
        self.assertEqual(rc, 1)
        rc, lines = recording(holder=none, split=(k, 'added'), status=status, as_name='takeover')
        self.assertEqual(lines['cuts'][0], 'FAIL', lines['cuts'])
        self.assertIn('created the device', lines['instance_2'][1])
        self.assertEqual(rc, 1)
        # and the first status's state must be begun
        rc, lines = recording(holder=none, split=(k, 'taken-over'), as_name='takeover')
        self.assertEqual(lines['cut_state'][0], 'FAIL', lines['cut_state'])

    def test_a_recording_with_no_row(self):
        rc, lines = recording(as_name='nosuch')
        self.assertEqual(rc, 2, lines)
        self.assertIn('no row of EXPECTED', lines['rig_error'][1])

    def test_a_second_instance_is_the_rigs_error(self):
        rc, lines = recording(lambda text: text + f'999999999\t7\tstart\t0\t-\t-\t-\t64\n')
        self.assertEqual(rc, 2, lines)
        self.assertIn('rig_error', lines)


if __name__ == '__main__':
    unittest.main()
