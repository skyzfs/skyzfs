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


def recording(tamper_log=None, status=None, name='life', tamper_kstat=None, as_name=None, holder=None):
    """A recording's directory -> (exit code, {line: (verdict, detail)}).

    holder: where given, the daemon held cuts and the holder ran: its
    files as a right daemon leaves them, with holder(written, blocks,
    final) changing them in place."""
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
            t.holder_files(d, written, blocks, final)
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
