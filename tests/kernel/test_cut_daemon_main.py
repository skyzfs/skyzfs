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


PID3 = '4444'
FAILURE = 'cuts_held\t1\nfailed_rc\t3\nrefused_rc\t4\n'


def recording(tamper_log=None, status=None, name='life', tamper_kstat=None, as_name=None, holder=None,
              split=None, tamper_split=None, tamper_written=None, holder_log=None, holder_left=None,
              failure=FAILURE, tamper_starts=None, tamper_marks=None):
    """A recording's directory -> (exit code, {line: (verdict, detail)}).

    holder: where given, the daemon held cuts and the holder ran: its
    files as a right daemon leaves them, with holder(written, blocks,
    final) changing them in place. split: (k, kind) splits the log in two
    instances at the k-th cut (t.split_log), the second having
    `taken-over` or `added` the device, or `acknowledged` a failure after
    the k-th cut's line (t.split_begun: a new state, which counts from 1
    again; the holder's numbers then repeat, and failure.tsv is the
    rig's as the recording `failed` leaves it, or FAILURE; a third
    element k2 begins a third state after the k2-th cut, pid PID3);
    tamper_split(text)
    changes the split log. tamper_written(written,
    ns) changes the holder's writes after that renumbering, or, after
    another split, with ns the second instance's start. holder_log:
    the holder's log in place of a right one's; holder_left: where given,
    only these of the holder's files are left. tamper_starts(starts)
    changes the statuses read at each start; tamper_marks(marks) the
    rig's marks."""
    records, read, dev = t.fixture(name)
    cs, at = t.daemon_cuts_of(records, read, dev)
    text, marks = t.request_log(records, read, dev, at, hashes=holder is not None)
    if tamper_log:
        text = tamper_log(text)
    if tamper_marks:
        marks = tamper_marks(marks)
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
                k2 = split[2] if len(split) > 2 else None
                if k2:
                    text, ns2 = t.split_begun(text, k2, pid=PID3, minus=k2 - k)
                if tamper_split:
                    text = tamper_split(text)
                with open(os.path.join(d, 'instances.tsv'), 'w') as f:
                    f.write(f'{t.PID2}\tadded\tacknowledged\n' + (f'{PID3}\tadded\tacknowledged\n' if k2 else ''))
                with open(os.path.join(d, 'failure.tsv'), 'w') as f:
                    f.write(failure)
                starts += ('cuts 0\ncut_state begun\ncuts_held 0\nintent_state taken_up\n',) * (2 if k2 else 1)
                blocks = [blocks[w['number']] for w in written]
                written = [dict(w, number=w['number'] - k2, asked=ns2 + 1) if k2 and w['number'] > k2 else
                           dict(w, number=w['number'] - k, asked=ns + 1) if w['number'] > k else w for w in written]
                if tamper_written:
                    written = tamper_written(written, ns)
            elif split:
                text = t.split_log(text, split[0], shape=split[2] if len(split) > 2 else None)
                if tamper_split:
                    text = tamper_split(text)
                if tamper_written:
                    ns = int(next(ln for ln in text.splitlines() if ln.split('\t')[1:3] == [t.PID2, 'start'])
                             .split('\t')[0])
                    written = tamper_written(written, ns)
                with open(os.path.join(d, 'instances.tsv'), 'w') as f:
                    f.write(f'{t.PID2}\t{split[1]}\n')
                starts += (f'cuts {split[0] - 1}\ncut_state taken_up\nintent_state taken_up\n',)
            if tamper_starts:
                starts = tamper_starts(starts)
            t.holder_files(d, written, blocks, final, starts=starts,
                           **({'log': holder_log} if holder_log is not None else {}))
            if holder_left is not None:
                for fn in ('holder.log', 'held.tsv', 'held-blocks.tsv', 'held-image.tsv', 'held-final.txt',
                           'status-held.txt'):
                    if fn not in holder_left:
                        os.remove(os.path.join(d, fn))
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


def tearer(seen, keep=20, back=0):
    """-> tear(text): the first instance's last line cut short by its kill
    after `keep` bytes, on a page of the file, and the next instance's
    start line after it with no newline (cut_daemon_check.torn_line).
    `send` lines of the first instance, which the check reads past, are
    put `back` lines before it to bring the cut onto a page (in the
    daemon's log nothing stands between a barrier and its cut line). seen
    gets the lost bytes and the torn line's number."""
    def tear(text):
        lines = text.split('\n')
        j = next(i for i, ln in enumerate(lines) if ln.split('\t')[1:3] == [t.PID2, 'start'])
        ns = lines[j - 1].split('\t')[0]
        at = sum(len(ln) + 1 for ln in lines[:j - 1])
        n = (-(at + keep)) % cdc.PAGE
        n += cdc.PAGE if n < 40 else 0
        fill = []
        while n:
            size = n if n <= 80 else min(44, n - 40)
            fill.append(f'{ns}\t{t.PID}\tsend\t-\t-\t-\t-\t' + '1' * (size - len(ns) - len(t.PID) - 16))
            n -= size
        seen['lost'] = lines[j - 1][:keep]
        seen['line'] = j + len(fill)
        lines[j - 1:j + 1] = [seen['lost'] + lines[j]]
        lines[j - 1 - back:j - 1 - back] = fill
        return '\n'.join(lines)
    return tear


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

    def test_a_failed_instance_line_is_a_verdict(self):
        """A start's status that only its instance line judges (cuts_held
        at an acknowledged start, G3 of #113) ends main with 1 (the plant I7
        of #111: main dropped what instance_lines returns)."""
        k = 13
        none = lambda written, blocks, final: None    # noqa: E731
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        status = held_status(records, read, dev, cs).replace(f'cuts {len(cs)}\n', f'cuts {len(cs) - k}\n')
        held = lambda starts: starts[:1] + tuple(st.replace('cuts_held 0', 'cuts_held 1') for st in starts[1:])  # noqa: E731,E501
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=status, as_name='failed',
                              tamper_starts=held)
        self.assertEqual(lines['instance_2'][0], 'FAIL', lines['instance_2'])
        self.assertEqual([key for key, (v, _) in lines.items() if v in ('FAIL', 'ERROR')], ['instance_2'])
        self.assertEqual(rc, 1)

    def test_an_acknowledged_start_cuts_nothing_again(self):
        """main hands unstored_at_kill the starts that acknowledged a
        failure (the review of #115, G4: plant U14): such a start counts
        from 1, and its first cut, here made to carry the number and group
        of the cut line that ends the instance before, is no cut again."""
        def same_group(text):
            lines = text.splitlines()
            cuts = [i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'cut']
            last = lines[cuts[0]].split('\t')
            self.assertEqual(lines[cuts[0] - 1].split('\t')[2], 'barrier')
            j = next(i for i in cuts if lines[i].split('\t')[1] == t.PID2)
            first = lines[j].split('\t')
            self.assertEqual(first[6], last[6])
            lines[j] = '\t'.join(first[:7] + last[7:])
            return '\n'.join(lines) + '\n'
        rc, lines = recording(holder=lambda written, blocks, final: None, split=(1, 'acknowledged'), as_name='failed',
                              tamper_split=same_group)
        self.assertTrue(lines['obs_unstored_at_kill'][1].startswith('0 flushes'), lines['obs_unstored_at_kill'])

    def test_a_group_synced_with_no_flush(self):
        """A group the pool committed with no flush (zfs_nocacheflush,
        between the marks noflush-begin and noflush-end) is cut by neither
        the daemon nor the reference: main leaves it out of the sessions'
        committed groups and says so (the plant N5 of #111). Without the
        marks, `sessions` holds it to a cut and fails."""
        records, read, dev = t.fixture('life')
        cs, at = t.daemon_cuts_of(records, read, dev)
        text, _ = t.request_log(records, read, dev, at)
        lines = text.splitlines()
        cuts = [i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'cut']
        ci = cuts[10]
        self.assertEqual([lines[i].split('\t')[2:5] for i in (ci - 2, ci - 1)],
                         [['recv', '1', 'flush'], ['barrier', '1', 'flush']])
        group = lines[ci].split('\t')[7]
        # The span: from after the cut before it to the flush that comes
        # before the next group's uberblock writes. Under zfs_nocacheflush
        # the pool sends no flush in it (none of the twelve spans of the
        # real recordings has one; the review of #115, N1), so every flush
        # between the marks goes: the group's own, before and after its
        # uberblock writes, and the next group's first.
        nxt = next(i for i in range(ci + 1, cuts[11]) if lines[i].split('\t')[2:5] == ['barrier', '1', 'flush'])
        lo, hi = int(lines[cuts[9]].split('\t')[0]) + 5, int(lines[nxt].split('\t')[0]) + 5
        gone = {i for i, ln in enumerate(lines) if lo < int(ln.split('\t')[0]) < hi
                and ln.split('\t')[2:5] in (['recv', '1', 'flush'], ['barrier', '1', 'flush'], ['cut', '1', 'flush'])}
        self.assertEqual(len(gone), 7)
        self.assertLess(nxt, cuts[11] - 2)

        def no_flush(text):
            return ''.join(ln + '\n' for i, ln in enumerate(text.splitlines()) if i not in gone)

        def span(marks):
            return ''.join(f'{n}\t{m}\n' for n, m in sorted(
                [tuple(ln.split('\t')) for ln in marks.splitlines()] + [(str(lo), 'noflush-begin'),
                                                                         (str(hi), 'noflush-end')],
                key=lambda nm: int(nm[0])))
        last = cs[-1]
        op = cut_ref.opens(records, read, dev, cs)
        status = (f"cuts {len(cs) - 1}\ncut_group {last['txg']}\n"
                  f"cut_opens {'itself' if op[last['n']]['itself'] else 'rewind'}\nage_ms 0\n")
        rc, out = recording(no_flush, status=status, tamper_marks=span)
        self.assertEqual(out['sessions'][0], 'PASS', out['sessions'])
        self.assertEqual(out['obs_noflush_groups'][0], 'INFO')
        self.assertIn(f'groups [{group}] were synced with no flush', out['obs_noflush_groups'][1])
        self.assertEqual(rc, 0, {k: v for k, v in out.items() if v[0] in ('FAIL', 'ERROR')})
        rc, out = recording(no_flush, status=status)
        self.assertEqual(out['sessions'][0], 'FAIL', out['sessions'])
        self.assertNotIn('obs_noflush_groups', out)

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
        # the new state's first written cut, against the old state's image
        # (the review of #113, G2)
        self.assertEqual(lines['carried_across_failure'][0], 'INFO', lines['carried_across_failure'])
        self.assertIn(f'cut {k + 1}, the first written of the new state: ', lines['carried_across_failure'][1])
        # the total, as a daemon that counted on would say it
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=held_status(records, read, dev, cs),
                              as_name='failed')
        self.assertEqual((rc, lines['status'][0]), (1, 'FAIL'))
        # the second start took the state up
        took = lambda text: text.replace(f'\t{t.PID2}\tcuts\t-\t-\t-\t-\tbegun', f'\t{t.PID2}\tcuts\t-\t-\t-\t-\ttaken_up')  # noqa: E731,E501
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=status, as_name='failed',
                              tamper_split=took)
        self.assertEqual((rc, lines['cut_state'][0], lines['instance_2'][0]), (1, 'FAIL', 'FAIL'))
        # ... and still held the old state's last cut, which its holder
        # wrote after the start: a verdict, not the rig's error (the
        # review of #113, D2), with instance_2 judged before it
        k2 = len(cs) // 2 + 1    # a number state 2 does not reach
        late = lambda written, ns: [dict(w, asked=ns + 1) if w['number'] == k2 else w  # noqa: E731
                                    for w in written]
        st2 = held_status(records, read, dev, cs).replace(f'cuts {len(cs)}\n', f'cuts {len(cs) - k2}\n')
        rc, lines = recording(holder=none, split=(k2, 'acknowledged'), status=st2, as_name='failed',
                              tamper_split=took, tamper_written=late)
        self.assertEqual((rc, lines['cut_state'][0], lines['instance_2'][0], lines['held_content'][0]),
                         (1, 'FAIL', 'FAIL', 'FAIL'), lines.get('rig_error'))
        self.assertIn(f'cut {k2} in state 2', lines['held_content'][1])
        # the same write with no defect in the start's lines is still one
        rc, lines = recording(holder=none, split=(k2, 'acknowledged'), status=st2, as_name='failed',
                              tamper_written=late)
        self.assertEqual((rc, lines['instance_2'][0], lines['held_content'][0]), (1, 'PASS', 'FAIL'),
                         lines.get('rig_error'))

    def test_the_failure_and_later_states(self):
        """failed: a failure line that fails is a verdict (the plant F7 of
        the review of #113); with two starts that acknowledged a failure
        the status holds the last state's count (Q2); where a new state
        numbers its cuts wrong, cut_numbers says so and the status is
        still held to that state's cuts (Q3); and the instances' lines
        come before the held cuts', so a rig's error in the holder leaves
        them (X5)."""
        k = 9
        none = lambda written, blocks, final: None    # noqa: E731
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        st = held_status(records, read, dev, cs)

        def since(n):
            return st.replace(f'cuts {len(cs)}\n', f'cuts {len(cs) - n}\n')
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=since(k), as_name='failed',
                              failure='cuts_held\t1\nfailed_rc\t0\nrefused_rc\t4\n')
        self.assertEqual((rc, lines['failure']), (1, ('FAIL', 'serve ended with 0 after SIGUSR1, wanted 3')))
        k2 = 18
        rc, lines = recording(holder=none, split=(k, 'acknowledged', k2), status=since(k2), as_name='failed')
        self.assertEqual(rc, 0, {n: v for n, v in lines.items() if v[0] not in ('PASS', 'INFO', 'CAUGHT', 'SAME')})
        self.assertIn('state 2 counts from 1 again; state 3 counts from 1 again', lines['cut_numbers'][1])
        rc, lines = recording(holder=none, split=(k, 'acknowledged', k2), status=since(k), as_name='failed')
        self.assertEqual((rc, lines['status']), (1, ('FAIL', f"cuts '{len(cs) - k}', not '{len(cs) - k2}'")))

        def from_2(text):
            out = []
            for ln in text.split('\n'):
                f = ln.split('\t')
                if len(f) > 6 and f[1:3] == [t.PID2, 'cut']:
                    f[6] = str(int(f[6]) + 1)
                out.append('\t'.join(f))
            return '\n'.join(out)
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=since(k), as_name='failed',
                              tamper_split=from_2)
        self.assertEqual((rc, lines['cut_numbers'][0], lines['status'][0]), (1, 'FAIL', 'PASS'))
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=since(k), as_name='failed',
                              holder_log='1\t> t1 held\n2\tERROR HolderError: no reply\n')
        self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'))
        for key in ('instance_1', 'instance_2', 'failure'):
            self.assertEqual(lines[key][0], 'PASS', (key, lines))

    def test_carried_across_failure_must_show_something(self):
        """carried_across_failure is the rig's ERROR where no cut of the
        new state was written, or where its first holds nothing the old
        state's image does not (the review of #113, G2)."""
        k = 13
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        status = held_status(records, read, dev, cs).replace(f'cuts {len(cs)}\n', f'cuts {len(cs) - k}\n')

        def same(written, blocks, final):
            old = {}
            for n in range(1, k + 1):
                old.update(blocks[n])
            blocks[k + 1] = dict(list(old.items())[:5])
            written[k]['blocks'] = 5
        rc, lines = recording(holder=same, split=(k, 'acknowledged'), status=status, as_name='failed')
        self.assertEqual(lines['carried_across_failure'],
                         ('ERROR', f'cut {k + 1}, the first written of the new state: 0 of its 5 blocks differ from '
                          'the image the holders of the state before left'))
        none = lambda written, blocks, final: None    # noqa: E731
        rc, lines = recording(holder=none, split=(k, 'acknowledged'), status=status, as_name='failed',
                              tamper_written=lambda written, ns: [w for w in written if w.get('asked', 1) <= ns])
        self.assertEqual(lines['carried_across_failure'], ('ERROR', 'no cut of the new state was written'))

        # a recording with no acknowledged start is not asked
        rc, lines = recording(holder=none)
        self.assertNotIn('carried_across_failure', lines)

    def test_what_cannot_be_judged(self):
        """The rig's errors of a recording that cannot be judged (the plants
        I10 and I11 of the review of #111): two instances with no holder,
        as no recording that holds no cuts starts the daemon again; and a
        held stage on a log with no hashes, as a daemon from before
        M2.2b-2 writes it. The same log with no holder is judged."""
        none = lambda written, blocks, final: None    # noqa: E731
        rc, lines = recording(holder=none, split=(13, 'taken-over'), holder_left=())
        self.assertEqual((rc, lines['rig_error']),
                         (2, ('ERROR', '2 instances of the daemon and no holder: only a recording that holds cuts '
                              'starts the daemon again')))

        def bare(text):
            return '\n'.join('\t'.join(ln.split('\t')[:8]) if '\tapplied\t' in ln else ln
                             for ln in text.split('\n'))
        rc, lines = recording(holder=none, tamper_log=bare)
        self.assertEqual(rc, 2)
        self.assertIn('writes applied with no hashes in the request log', lines['rig_error'][1])
        rc, lines = recording(tamper_log=bare)
        self.assertEqual((rc, lines['cuts'][0]), (0, 'PASS'))

    def test_a_verdict_before_the_first_cut(self):
        """The daemon refused to read the first cut the holder asked for
        (Codex on #113): the holder ended with its VERDICT and wrote no
        held.tsv, and the recording ended there with no failure.tsv. A
        verdict, not the rig's error."""
        none = lambda written, blocks, final: None    # noqa: E731
        log = ('1\t> t1 held\n2\t< t1 ok (2 lines)\n3\t> t2 write 1 /w/held.img\n'
               '4\t< t2 refused: read block 7 of cut 1: Input/output error (os error 5)\n'
               '5\tVERDICT cut 1 is still being uploaded after its write was refused '
               '(refused: read block 7 of cut 1: Input/output error (os error 5))\n')
        for name in ('failed', 'takeover'):
            with self.subTest(name):
                rc, lines = recording(holder=none, as_name=name, holder_log=log, holder_left=('holder.log',))
                self.assertNotIn('rig_error', lines)
                self.assertEqual((rc, lines['held_reads'][0]), (1, 'FAIL'))
                self.assertIn('read block 7 of cut 1', lines['held_reads'][1])

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

    def test_a_cut_held_across_the_take_over(self):
        """A cut the first instance counted and the second wrote, with
        blocks of its map changed in the home before the second started:
        held_across_start names it (the review of #111, G1); a take-over
        whose holder wrote every cut before the start is the rig's
        ERROR, the recording being there to show copies surviving it."""
        k = 13
        none = lambda written, blocks, final: None    # noqa: E731
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        status = held_status(records, read, dev, cs).replace('cut_state begun', 'cut_state taken_up')
        later = lambda written, ns: [dict(w, asked=ns + w['number']) for w in written]    # noqa: E731
        rc, lines = recording(holder=none, split=(k, 'taken-over'), status=status, as_name='takeover',
                              tamper_written=later)
        self.assertEqual(lines['held_content'][0], 'PASS', lines['held_content'])
        self.assertEqual(lines['held_across_start'][0], 'INFO', lines['held_across_start'])
        self.assertIn('cut 1, counted by instance 1 and written by instance 2: ', lines['held_across_start'][1])
        self.assertEqual(rc, 1)    # ctl_mark-kept, as test_two_instances
        rc, lines = recording(holder=none, split=(k, 'taken-over'), status=status, as_name='takeover')
        self.assertEqual(lines['held_across_start'][0], 'ERROR', lines['held_across_start'])
        # only the take-over is asked for it
        rc, lines = recording(holder=none, split=(k, 'taken-over'), status=status, tamper_written=later)
        self.assertNotIn('held_across_start', lines)

    def test_a_kill_before_the_state_stored_the_cut(self):
        """The take-over after a kill between the barrier and the state
        storing the cut (windows B and C of the review of #111, D1): the
        right daemon passes, and obs_unstored_at_kill names the line."""
        k = 13
        none = lambda written, blocks, final: None    # noqa: E731
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        status = held_status(records, read, dev, cs).replace('cut_state begun', 'cut_state taken_up')
        for shape, said in (('B', 'barrier with no cut line'), ('C', f'cut {k} (group')):
            with self.subTest(shape):
                rc, lines = recording(holder=none, split=(k, 'taken-over', shape), status=status,
                                      as_name='takeover')
                for key in ('cuts', 'cut_numbers', 'status', 'cut_state', 'held_content', 'instance_1',
                            'instance_2'):
                    self.assertEqual(lines[key][0], 'PASS', (key, lines[key]))
                self.assertIn('1 flushes killed after their barrier and before their answer', lines['obs_unstored_at_kill'][1])
                self.assertIn(said, lines['obs_unstored_at_kill'][1])
                self.assertEqual(rc, 1)    # ctl_mark-kept, as test_two_instances
        rc, lines = recording(holder=none, split=(k, 'taken-over'), status=status, as_name='takeover')
        self.assertIn('0 flushes', lines['obs_unstored_at_kill'][1])
        # The cut counted past with no line (shape A): the rig's error, not a kill.
        rc, lines = recording(holder=none, split=(k, 'taken-over', 'A'), status=status, as_name='takeover')
        self.assertEqual(rc, 2, lines)
        self.assertIn(f'no line of cut {k}', lines['rig_error'][1])
        self.assertIn('no kill leaves this', lines['rig_error'][1])

    def test_a_cut_line_torn_at_the_take_over(self):
        """A kill that tears the cut line (window B by another way): the
        line is lost, and the flush is one with no barrier."""
        k = 13
        none = lambda written, blocks, final: None    # noqa: E731
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        status = held_status(records, read, dev, cs).replace('cut_state begun', 'cut_state taken_up')
        seen = {}
        rc, lines = recording(holder=none, split=(k, 'taken-over', 'C'), status=status, as_name='takeover',
                              tamper_split=tearer(seen, back=1))
        self.assertIn(f"line {seen['line']}: pid {t.PID}'s `{cdc.flat(seen['lost'])}` line after 20 bytes",
                      lines['obs_torn_at_kill'][1])
        self.assertIn('\tcut\t', seen['lost'])
        self.assertIn('barrier with no cut line', lines['obs_unstored_at_kill'][1])
        for key in ('cuts', 'cut_numbers', 'status', 'instance_1', 'instance_2'):
            self.assertEqual(lines[key][0], 'PASS', (key, lines[key]))
        self.assertEqual(rc, 1)    # ctl_mark-kept

    def test_a_line_torn_at_the_take_over(self):
        """The first instance's last line cut short by its kill, on a page
        of the file, and the next instance's start line after it with no
        newline: the line is lost with the kill, and the check judges the
        rest as it judges a kill a moment earlier, where the next instance
        took the device over (the review of #113, L12 to L14)."""
        k = 13
        none = lambda written, blocks, final: None    # noqa: E731
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        status = held_status(records, read, dev, cs).replace('cut_state begun', 'cut_state taken_up')
        seen = {}
        tear = tearer(seen)
        rc, lines = recording(holder=none, split=(k, 'taken-over'), status=status, as_name='takeover',
                              tamper_split=tear)
        self.assertEqual(lines['obs_torn_at_kill'][0], 'INFO')
        self.assertIn(f"1 lines of the request log cut short by a kill -9 and lost with it; line {seen['line']}: "
                      f"pid {t.PID}'s", lines['obs_torn_at_kill'][1])
        self.assertIn(f"line after {len(seen['lost'])} bytes, then pid {t.PID2}'s start",
                      lines['obs_torn_at_kill'][1])
        for key in ('cuts', 'cut_numbers', 'status', 'cut_state', 'held_content', 'instance_1', 'instance_2'):
            self.assertEqual(lines[key][0], 'PASS', (key, lines[key]))
        self.assertEqual(rc, 1)    # ctl_mark-kept, as test_two_instances
        rc, lines = recording(holder=none, split=(k, 'taken-over'), status=status, as_name='takeover')
        self.assertIn('0 lines', lines['obs_torn_at_kill'][1])
        # Before an instance that created the device, the rig's error.
        rc, lines = recording(holder=none, split=(k, 'added'), status=status, as_name='takeover',
                              tamper_split=tear)
        self.assertEqual(rc, 2, lines)
        self.assertIn('did not take the device over', lines['rig_error'][1])

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
