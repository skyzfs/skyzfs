#!/usr/bin/env python3
"""The cut rig on the daemon's check (cut_daemon_check.py), its `main`
from end to end with no kernel: that a failed line fails the check, that
the rig's own error ends with 2. Each test writes a recording's directory
from the fixture `life` (the request log, the marks, a status, its times,
and each session's kstat as the reference's own cuts give it) and runs
the check as cut-record.sh does. Written by the review of #99 (S2) and
taken in with it.
"""
import contextlib
import io
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

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
              failure=FAILURE, tamper_starts=None, tamper_marks=None, held=None, upload=None, watch_log='1\tstopped\n'):
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
    rig's marks. held: where given, main runs in this process with
    held_lines replaced by held(the real held_lines), and the exit code is
    its SystemExit's. upload: where given, the daemon uploaded its cuts and
    the watcher ran (the third stage): its files as node B would leave
    them for a right daemon that uploaded every cut, with upload(written,
    blocks, status) -> (written, blocks, status[, {file: text}]) changing
    them and naming a course's own files (watcher_files); watch_log
    the watcher's log."""
    records, read, dev = t.fixture(name)
    cs, at = t.daemon_cuts_of(records, read, dev)
    text, marks = t.request_log(records, read, dev, at, hashes=holder is not None or upload is not None)
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
                starts += (f'cuts {split[0] - 1}\ncut_state taken_up\ncuts_held 1\nintent_state taken_up\n',)
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
        if upload is not None:
            written, blocks, _ = t.held_images(records, cs, cut_ref.opens(records, read, dev, cs))
            got = upload(written, blocks, (status or held_status(records, read, dev, cs))
                         + UPLOADED.format(n=len(written), r=len(cs) - len(written)))
            written, blocks, status = got[:3]
            watcher_files(d, written, blocks, watch_log)
            # A course's own files (cut-record.sh), where upload gives them.
            for fn, body in (got[3] if len(got) > 3 else {}).items():
                with open(os.path.join(d, fn), 'w') as f:
                    f.write(body)
        return run_in(d, text, marks, records, read, dev, cs, status, name, tamper_kstat, as_name, held)
    finally:
        shutil.rmtree(d, ignore_errors=True)


# The status's upload keys after a right daemon's drain (status
# version 15), with n cuts uploaded.
UPLOADED = ('upload_state idle\nupload_cut 0\nlease held\nlease_holder runner\nlease_epoch 1\nuploaded {n}\n'
            'uploaded_whole 1\nupload_mismatches 0\nupload_abandoned_at_mismatch 0\nupload_cuts_lost 0\n'
            'upload_given_up 0\nupload_committed_kept 0\nupload_found_committed 0\nupload_errors 0\n'
            'upload_checked matched\nupload_rule delta\ncuts_held 0\nupload_committed_gone 0\n'
            'cuts_replaced {r}\n')
# The look at the daemon's start: nothing counted yet.
STARTED = ''.join(f'{k} 0\n' for k in cdc.BALANCE) + 'upload_state unchecked\ncut_state begun\n'
STATE = 'ab' * 16


def watcher_files(d, written, blocks, log):
    """The watcher's files (cut_watch.py) in recording directory d: each
    cut of written fetched at head, its blocks those that changed (keyed
    by its number, or by its 'key' where it has one: another writer's
    fetch, which can carry a number of the daemon's), at its 'head'."""
    def w(fn, text):
        with open(os.path.join(d, fn), 'w') as f:
            f.write(text)
    w('watch.log', log)
    w('uploaded.tsv', ''.join(f"{x['number']}\t{x['group']}\t{x.get('state', STATE)}\t{x.get('head', 'cd' * 32)}\t"
                              f"{x['blocks']}\t"
                              f"{x.get('asked', 1)}\t{x.get('asked', 1) + 1}\n" for x in written))
    w('uploaded-blocks.tsv', ''.join(f'{k}\t{x["number"]}\t{b}\t{h}\n' for k, x in enumerate(written)
                                     for b, h in sorted(blocks[x.get('key', x['number'])].items())))
    w('drain.txt', 'seconds 3\nupload_state idle\ncuts_held 0\n')
    w('status-start-1.txt', STARTED)
    w('rows.tsv', f"5\t{'cd' * 32} 7\t{STATE} 1 4\trunner 1 99\n")


def held_status(records, read, dev, cs):
    op = cut_ref.opens(records, read, dev, cs)
    last = cs[-1]
    return (f"cuts {len(cs)}\ncut_group {last['txg']}\ncut_opens {'itself' if op[last['n']]['itself'] else 'rewind'}"
            f"\nage_ms 0\ncut_state begun\ncuts_abandoned 0\n")


def run_in(d, text, marks, records, read, dev, cs, status, name, tamper_kstat, as_name, held=None):
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
    argv = [cdc.__file__, as_name or name, d, log, '-', str(dev)]
    if held is None:
        rc = subprocess.run([sys.executable, '-I', '-B'] + argv, capture_output=True, text=True).returncode
    else:
        with mock.patch.object(cdc, 'held_lines', held(cdc.held_lines)), mock.patch.object(sys, 'argv', argv), \
                contextlib.redirect_stdout(io.StringIO()), self_exit() as code:
            cdc.main()
        rc = code[0]
    lines = {}
    with open(os.path.join(d, 'results.tsv')) as f:
        for ln in f:
            key, verdict, detail = ln.rstrip('\n').split('\t', 2)
            lines[key] = (verdict, detail)
    return rc, lines


@contextlib.contextmanager
def self_exit():
    """Catches main's SystemExit -> [its code]."""
    code = [None]
    try:
        yield code
    except SystemExit as e:
        code[0] = e.code
    if code[0] is None:
        raise AssertionError('main did not exit')


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

    def test_a_floor_not_met_ends_as_the_rigs_error(self):
        """main passes held_lines' ERROR flag on (either floor:
        held_across_start, carried_across_failure) and ends with 2 where
        nothing else failed, on both of its calls: after a holder's
        VERDICT and after the instances' lines (the review of #116,
        question 4, plants G3e and G3f). held_lines is replaced, so that
        the flag alone differs."""
        k = 13
        none = lambda written, blocks, final: None    # noqa: E731
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        status = held_status(records, read, dev, cs).replace(f'cuts {len(cs)}\n', f'cuts {len(cs) - k}\n')
        log = '1\t> t1 held\n2\t< t1 ok (0 lines)\n3\tVERDICT nothing to read\n'
        for why, kw in (('the instances', {'split': (k, 'acknowledged'), 'status': status}),
                        ("the holder's verdict", {'holder_log': log, 'holder_left': ('holder.log',)})):
            for errored, want in ((False, 0), (True, 2)):
                with self.subTest(why, errored=errored):
                    rc, lines = recording(holder=none, as_name='failed',
                                          held=lambda real: lambda *a, **kw: (0, errored), **kw)
                    self.assertEqual(rc, want, {n: v for n, v in lines.items() if v[0] in ('FAIL', 'ERROR')})

    def test_a_right_daemon_that_uploads(self):
        """The third stage (M2.4c): every cut fetched at head holds the
        reference's content, the last is the last counted, and the status
        after the drain is a clean upload's."""
        rc, lines = recording(upload=lambda w, b, st: (w, b, st))
        self.assertEqual(rc, 0, {k: v for k, v in lines.items() if v[0] in ('FAIL', 'ERROR')})
        for key in ('cuts', 'status', 'upload_reads', 'upload_content', 'upload_status'):
            self.assertEqual(lines[key][0], 'PASS', (key, lines[key]))
        for c in cdc.CONTENT_CONTROLS:
            self.assertEqual(lines[f'ctl_{c}'][0], 'CAUGHT', lines[f'ctl_{c}'])
        self.assertNotIn('held_content', lines)

    def test_node_b_saw_some_cuts(self):
        """The watcher fetches head twice a second and misses cuts that
        head held for less: those it fetched are judged, each with what
        changed since the one before it."""
        def sparse(written, blocks, st):
            kept, acc = [], {}
            for k, x in enumerate(written):
                acc.update(blocks[x['number']])
                if k % 3 == 2 or k == len(written) - 1:
                    blocks[x['number']] = dict(acc)
                    kept.append(dict(x, blocks=len(acc)))
                    acc = {}
            return kept, blocks, st
        rc, lines = recording(upload=sparse)
        self.assertEqual((rc, lines['upload_content'][0]), (0, 'PASS'), lines['upload_content'])

    def test_what_an_upload_gets_wrong(self):
        """Each wrong thing node B or the status could show is a verdict."""
        def wrong_block(w, b, st):
            n = w[len(w) // 2]['number']
            blk = sorted(b[n])[0]
            b[n][blk] = '0123456789abcdef'
            return w, b, st

        def wrong_group(w, b, st):
            w[2] = dict(w[2], group=w[2]['group'] + 1)
            return w, b, st

        def not_the_last(w, b, st):
            return w[:-1], b, st

        def two_states(w, b, st):
            w[-1] = dict(w[-1], state='ef' * 16)
            return w, b, st

        def status(old, new):
            return lambda w, b, st: (w, b, st.replace(old, new))
        for why, upload, key in (('a block of a cut', wrong_block, 'upload_content'),
                                 ("a row's group", wrong_group, 'upload_content'),
                                 ('the last cut not uploaded', not_the_last, 'upload_content'),
                                 ('two cut states', two_states, 'upload_content'),
                                 ('a mismatch', status('upload_mismatches 0', 'upload_mismatches 1'), 'upload_status'),
                                 ('a second whole cut', status('uploaded_whole 1', 'uploaded_whole 2'),
                                  'upload_status'),
                                 ('the lease lost', status('lease held', 'lease lost'), 'upload_status'),
                                 ('no holder named', status('lease_holder runner', 'lease_holder none'),
                                  'upload_status'),
                                 ('another epoch', status('lease_epoch 1', 'lease_epoch 2'), 'upload_status'),
                                 ('a cut lost', status('upload_cuts_lost 0', 'upload_cuts_lost 1'), 'upload_status'),
                                 ('not idle', status('upload_state idle', 'upload_state backoff'), 'upload_status'),
                                 ('a cut still held', status('cuts_held 0', 'cuts_held 1'), 'upload_status'),
                                 ('fewer uploaded than fetched', lambda w, b, st: (w, b, re.sub(
                                     r'^uploaded \d+$', 'uploaded 1', st, flags=re.M)), 'upload_status'),
                                 ('a cut that went nowhere', lambda w, b, st: (w, b, re.sub(
                                     r'^cuts (\d+)$', lambda m: f'cuts {int(m.group(1)) + 1}', st, flags=re.M)),
                                  'upload_balance'),
                                 ('a cut counted twice', status('cuts_replaced 0', 'cuts_replaced 1'),
                                  'upload_balance'),
                                 ('a daemon before cuts_replaced', status('cuts_replaced 0\n', ''),
                                  'upload_balance')):
            with self.subTest(why):
                rc, lines = recording(upload=upload)
                self.assertEqual((rc, lines[key][0]), (1, 'FAIL'), lines[key])

    def test_the_watchers_own_end(self):
        """A VERDICT in the watcher's log (node B could not read head's
        content) is one; its ERROR is the rig's; no cut fetched is a
        verdict."""
        rc, lines = recording(upload=lambda w, b, st: (w, b, st), watch_log='1\tVERDICT node B could not read head\n')
        self.assertEqual((rc, lines['upload_reads'][0]), (1, 'FAIL'))
        rc, lines = recording(upload=lambda w, b, st: (w, b, st), watch_log='1\tERROR node B row failed 3 times\n')
        self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'))
        rc, lines = recording(upload=lambda w, b, st: ([], {}, st))
        self.assertEqual((rc, lines['upload_content'][0]), (1, 'FAIL'))
        self.assertIn("node B found no cut of the daemon's at head", lines['upload_content'][1])

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



OTHER_HEAD = 'ee' * 32


def row_file(cut, head='cd' * 32, lease=('runner', 1)):
    """node B's `row` output naming cut (state, number, group) or none."""
    return (f'head\t{head}\t7\n' + (f'row\t{cut[0]}\t{cut[1]}\t{cut[2]}\n' if cut else 'row\tnone\n')
            + (f'lease\t{lease[0]}\t{lease[1]}\t99\n' if lease else 'lease\tnone\n'))


class Courses(unittest.TestCase):
    """The upload courses (cut-record.sh's umoved, urow, ukill, ulease,
    ulost): what another writer put at head is not the daemon's, and each
    course's own files."""

    def test_the_daemons_fetches(self):
        """The image is followed through every fetch; a cut of the
        daemon's is what changed since its cut fetched before, another
        writer's head or row is skipped, and a number at head under two
        of the daemon's commits is named."""
        got = [{'number': 1, 'group': 10, 'state': STATE, 'head': 'aa' * 32},
               {'number': 1, 'group': 10, 'state': STATE, 'head': OTHER_HEAD},
               {'number': 2, 'group': 11, 'state': STATE, 'head': 'bb' * 32},
               {'number': 2, 'group': 11, 'state': 'ff' * 16, 'head': 'bb' * 32},
               {'number': 2, 'group': 11, 'state': STATE, 'head': 'cc' * 32}]
        a, b, x = '1' * 16, '2' * 16, '3' * 16
        text = (f'0\t1\t0\t{a}\n0\t1\t1\t{b}\n1\t1\t0\t{x}\n1\t1\t5\t{x}\n'
                f'2\t2\t0\t{a}\n2\t2\t5\t{cdc.ZERO}\n4\t2\t1\t{x}\n')
        others = ({OTHER_HEAD}, {('ff' * 16, 2, 11)})
        written, blocks, skipped, twice, states = cdc.daemons_fetches(got, text, others)
        self.assertEqual([w['number'] for w in written], [1, 2])
        self.assertEqual(blocks, {1: {0: a, 1: b}, 2: {}})
        self.assertEqual((skipped, twice, states), (2, {2}, [STATE]))
        # A state begun later: its numbers are places in the whole log.
        later = got[:1] + [dict(got[2], state='99' * 16, number=1)]
        written, blocks, _, _, states = cdc.daemons_fetches(later, f'0\t1\t0\t{a}\n1\t1\t3\t{b}\n', (set(), set()),
                                                            (0, 26))
        self.assertEqual(([w['number'] for w in written], states), ([1, 27], [STATE, '99' * 16]))
        self.assertEqual(blocks[27], {3: b})
        written, _, _, _, states = cdc.daemons_fetches(later, f'0\t1\t0\t{a}\n', (set(), set()))
        self.assertEqual(([w['number'] for w in written], len(states)), ([1], 2))
        with self.assertRaises(cdc.RigError):
            cdc.daemons_fetches(got, '7\t1\t0\t' + a + '\n', others)
        with self.assertRaises(cdc.RigError):
            cdc.daemons_fetches(got, '1\t2\t0\t' + a + '\n', others)

    def files(self, files):
        files = dict({'rows.tsv': f"5\t{'cd' * 32} 7\t{STATE} 5 40\trunner 1 99\n"}, **files)
        d = tempfile.mkdtemp(prefix='cdc-course-')
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        for fn, body in files.items():
            with open(os.path.join(d, fn), 'w') as f:
                f.write(body)
        return d

    def judge(self, name, files, status):
        d = self.files(files)
        want, bad, notes = cdc.course_wants(name, d, status)
        wrong = [k for k, v in want.items()
                 if not (status.get(k) == v if isinstance(v, str) else v(status.get(k) or ''))]
        return wrong, bad, notes

    def test_the_lease_node_b_reads(self):
        """Every recording: the holder and the epoch the status names are
        those of the lease in the bucket, and a bucket with no lease is
        no holder (review of #119, G5)."""
        st = cdc.read_status(UPLOADED.format(n=5, r=0))
        self.assertEqual(self.judge('life', {}, st)[:2], ([], []))
        none = {'rows.tsv': f"5\t{'cd' * 32} 7\t{STATE} 5 40\tnone\n"}
        wrong, bad, _ = self.judge('life', none, dict(st, lease_holder='none'))
        self.assertEqual((wrong, len(bad)), ([], 1))
        self.assertIn('no lease in the bucket', bad[0])
        later = {'rows.tsv': f"5\t{'cd' * 32} 7\t{STATE} 5 40\trunner 7 99\n"}
        self.assertEqual(len(self.judge('life', later, st)[1]), 1)
        # The last look is the one read, not the first.
        two = {'rows.tsv': f"4\tnone\tnone\tnone\n5\t{'cd' * 32} 7\t{STATE} 5 40\trunner 1 99\n"}
        self.assertEqual(self.judge('life', two, st)[:2], ([], []))
        # Through the whole check: a recording that is no course says so on
        # upload_status.
        rows = f"5\t{'cd' * 32} 7\t{STATE} 5 40\tnone\n"
        rc, lines = recording(upload=lambda w, b, st: (w, b, st.replace('lease_holder runner', 'lease_holder none'),
                                                       {'rows.tsv': rows}))
        self.assertEqual((rc, lines['upload_status'][0]), (1, 'FAIL'), lines['upload_status'])
        self.assertIn('no lease in the bucket', lines['upload_status'][1])

    def test_each_courses_files(self):
        """Each course's files and status as a right daemon leaves them
        pass, and each thing it could get wrong is named."""
        st = cdc.read_status(UPLOADED.format(n=5, r=0))
        cut = (STATE, 5, 40)
        moved = {'moved-row.txt': row_file(cut), 'moved-status.txt': 'uploaded 5\n',
                 'end-row.txt': row_file(cut, head=OTHER_HEAD), 'others.tsv': f'head\t{OTHER_HEAD}\n'}
        good = dict(st, upload_mismatches='2', upload_abandoned_at_mismatch='3', upload_checked='head_moved')
        self.assertEqual(self.judge('umoved', moved, good)[:2], ([], []))
        self.assertEqual(self.judge('umoved', moved, dict(good, uploaded='6'))[0], ['uploaded'])
        self.assertEqual(self.judge('umoved', moved, dict(good, upload_checked='matched'))[0], ['upload_checked'])
        self.assertEqual(self.judge('umoved', moved, dict(good, upload_mismatches='0',
                                                          upload_abandoned_at_mismatch='0'))[0],
                         ['upload_mismatches', 'upload_abandoned_at_mismatch'])
        back = dict(moved, **{'end-row.txt': row_file((STATE, 6, 41))})
        self.assertEqual(len(self.judge('umoved', back, good)[1]), 2)

        rows = {'row1-row.txt': row_file(cut), 'end-row.txt': row_file((STATE, 9, 44)),
                'others.tsv': f"row\t{'5e' * 16}\t5\t40\nrow\t{STATE}\t12\t42\n",
                'row1-after.txt': 'upload_checked not_the_base\n', 'row2-after.txt': 'upload_checked matched\n'}
        good = dict(st, upload_mismatches='2', upload_abandoned_at_mismatch='2', uploaded_whole='3')
        wrong, bad, notes = self.judge('urow', rows, good)
        self.assertEqual((wrong, bad), ([], []))
        self.assertIn("row1: upload_checked 'not_the_base'", notes[0])
        self.assertEqual(self.judge('urow', rows, dict(good, uploaded_whole='2'))[0], ['uploaded_whole'])
        stuck = dict(rows, **{'end-row.txt': row_file((STATE, 12, 42))})
        self.assertEqual(len(self.judge('urow', stuck, good)[1]), 1)
        self.assertEqual(self.judge('urow', rows, dict(good, upload_mismatches='1'))[0], ['upload_mismatches'])
        self.assertEqual(self.judge('urow', rows, dict(good, upload_checked='not_the_base'))[0], ['upload_checked'])
        other = dict(rows, **{'end-row.txt': row_file(('ef' * 16, 9, 44))})
        self.assertEqual(len(self.judge('urow', other, good)[1]), 1)

        hook = ('skyzfsd: test hook --kill-at-upload: killed after the swap of commit 3 (group 30) landed, '
                'before its cut was let go\n')
        kill = {'kill.tsv': 'killed_rc\t137\n', 'serve.log': 'serving device 0 (added)\n' + hook}
        good = dict(st, upload_found_committed='1', uploaded_whole='0')
        self.assertEqual(self.judge('ukill', kill, good)[:2], ([], []))
        self.assertEqual(self.judge('ukill', kill, dict(good, upload_found_committed='0'))[0],
                         ['upload_found_committed'])
        self.assertEqual(self.judge('ukill', kill, dict(good, uploaded_whole='1', uploaded='0'))[0],
                         ['uploaded_whole', 'uploaded'])
        self.assertEqual(len(self.judge('ukill', dict(kill, **{'kill.tsv': 'killed_rc\t0\n'}), good)[1]), 1)
        self.assertEqual(len(self.judge('ukill', dict(kill, **{'serve.log': hook.replace('commit 3', 'commit 2')}),
                                        good)[1]), 1)

        lease_rows = ('3\tnone\tnone\tother-node 1 99\n4\tnone\tnone\tother-node 1 99\n'
                      f"9\t{'cd' * 32} 7\t{STATE} 5 40\trunner 2 99\n")
        lease = {'lease.tsv': 'refused_rc\t10\n', 'refused-wrote.txt': '', 'release-ns.txt': '5\n',
                 'status-start-1.txt': 'upload_state off\nlease none\n', 'without-status.txt': 'uploaded 0\n',
                 'rows.tsv': lease_rows}
        good = dict(st, lease_epoch='2')
        self.assertEqual(self.judge('ulease', lease, good)[:2], ([], []))
        # Node B read a head before the other let go: an upload without
        # the lease, whatever the status says; and a look before it.
        early = dict(lease, **{'rows.tsv': lease_rows.replace('4\tnone\tnone', f"4\t{'cd' * 32} 6\t{STATE} 1 9")})
        self.assertEqual(len(self.judge('ulease', early, good)[1]), 1)
        self.assertEqual(len(self.judge('ulease', dict(lease, **{'release-ns.txt': '2\n'}), good)[1]), 1)
        for fn, body in (('lease.tsv', 'refused_rc\t0\n'), ('status-start-1.txt', 'upload_state idle\nlease held\n'),
                         ('without-status.txt', 'uploaded 1\n')):
            self.assertTrue(self.judge('ulease', dict(lease, **{fn: body}), good)[1], fn)

        lost = {'lost-row.txt': row_file(cut), 'lost-status.txt': 'uploaded 5\n',
                'end-row.txt': row_file(cut, lease=('other-node', 2)),
                'other-lease.txt': 'lease\tacquired\tother-node\t2\t99\n',
                'rows.tsv': f"5\t{'cd' * 32} 7\t{STATE} 5 40\tother-node 2 99\n"}
        good = dict(st, upload_state='lease_lost', lease='lost', lease_holder='other-node', lease_epoch='2',
                    cuts_held='3')
        self.assertEqual(self.judge('ulost', lost, good)[:2], ([], []))
        self.assertEqual(self.judge('ulost', lost, dict(good, uploaded='6', lease='held'))[0], ['lease', 'uploaded'])
        self.assertEqual(self.judge('ulost', lost, dict(good, upload_state='idle', lease_holder='runner'))[0],
                         ['upload_state', 'lease_holder'])
        self.assertEqual(self.judge('ulost', lost, dict(good, lease_epoch='3'))[0], ['lease_epoch'])
        moved_on = dict(lost, **{'end-row.txt': row_file((STATE, 6, 41), lease=('other-node', 2))})
        self.assertEqual(len(self.judge('ulost', moved_on, good)[1]), 1)
        # Head moved alone at the end, the row as it was: the uploader
        # committed after the loss.
        head_on = dict(lost, **{'end-row.txt': row_file(cut, head='ee' * 32, lease=('other-node', 2))})
        self.assertEqual(len(self.judge('ulost', head_on, good)[1]), 1)
        taken_back = dict(lost, **{'end-row.txt': row_file(cut, lease=('runner', 3))})
        self.assertEqual(len(self.judge('ulost', taken_back, good)[1]), 1)

        put = {'kill.tsv': 'killed_rc\t137\n',
               'serve.log': 'skyzfsd: test hook --kill-at-upload: killed after segment PUT 5 landed (seg/x)\n'}
        self.assertEqual(self.judge('ukillput', put, st)[:2], ([], []))
        self.assertEqual(self.judge('ukillput', put, dict(st, uploaded_whole='0'))[:2], ([], []))
        self.assertEqual(len(self.judge('ukillput', dict(put, **{'serve.log': hook}), st)[1]), 1)

        down = {'nostore-status.txt': 'lease none\nupload_state backoff\nuploaded 0\nupload_why refused\n'}
        wrong, bad, notes = self.judge('unostore', down, st)
        self.assertEqual((wrong, bad), ([], []))
        self.assertIn("upload_why 'refused'", notes[0])
        self.assertEqual(len(self.judge('unostore', {'nostore-status.txt': 'lease held\nupload_state idle\n'
                                                     'uploaded 1\n'}, st)[1]), 3)

        anew = 'skyzfsd: intent map: begun anew with every region named\n'
        failed = {'failure.tsv': 'failed_rc\t3\n', 'status-start-2.txt': 'cut_state begun\n', 'serve-2.log': anew}
        self.assertEqual(self.judge('ufailed', failed, st)[:2], ([], []))
        self.assertEqual(len(self.judge('ufailed', {'failure.tsv': 'failed_rc\t0\n', 'serve-2.log': anew,
                                                    'status-start-2.txt': 'cut_state taken_up\n'}, st)[1]), 2)
        self.assertEqual(self.judge('ufailed', failed, dict(st, uploaded_whole='0'))[0], ['uploaded_whole'])
        # A start that took the intent map up: the new state's first cut is
        # a delta onto the map's base, as run 44255421061 had it.
        took = dict(failed, **{'serve-2.log': 'skyzfsd: intent map: taken up, naming 2 of 128 regions\n'})
        delta = dict(st, uploaded_whole='0', uploaded='2', upload_rule='delta', upload_checked='matched')
        wrong, bad, notes = self.judge('ufailed', took, delta)
        self.assertEqual((wrong, bad), ([], []))
        self.assertTrue(any('took the intent map up' in n for n in notes))
        self.assertEqual(self.judge('ufailed', took, dict(delta, uploaded_whole='1'))[0], ['uploaded_whole'])
        self.assertEqual(self.judge('ufailed', took, dict(delta, upload_rule='whole'))[0], ['upload_rule'])
        self.assertEqual(len(self.judge('ufailed', dict(failed, **{'serve-2.log': 'nothing\n'}), st)[1]), 1)

    def test_the_balance_over_one_daemon(self):
        """The balance is of changes from the start's look: a daemon that
        took a state up starts with cuts, abandoned and held cuts
        counted, and a cut held at its start that it found committed
        counts there."""
        start = dict.fromkeys(cdc.BALANCE, '0')
        start.update(cuts='7', cuts_abandoned='2', cuts_held='1')
        end = dict(start, cuts='12', uploaded='3', upload_found_committed='1', cuts_replaced='2', cuts_held='0')
        ok, detail = cdc.upload_balance(start, end)
        self.assertTrue(ok, detail)
        self.assertIn('cuts 5 = committed and let go 4 + replaced 2 + abandoned 0 + held -1', detail)
        for why, more in (('a cut gone', dict(cuts_replaced='1')), ('a kept commit', dict(upload_committed_kept='1')),
                          ('a gone commit', dict(upload_committed_gone='1')),
                          ('one more abandoned', dict(cuts_abandoned='3'))):
            with self.subTest(why):
                ok, detail = cdc.upload_balance(start, dict(end, **more))
                self.assertFalse(ok)
                self.assertIn(' != ', detail)
        self.assertEqual(cdc.upload_balance(start, {k: v for k, v in end.items() if k != 'cuts_replaced'}),
                         (False, 'the status has no cuts_replaced (a daemon before cuts_replaced?)'))

    def test_where_the_balance_starts(self):
        """The balance starts from DIR/balance-start.txt where the course
        took one, and that look must be at rest; a recording that started
        the daemon again and took none is the rig's error (review of #119,
        D3)."""
        def with_files(files):
            return lambda w, b, st: (w, b, st, files)
        rc, lines = recording(upload=with_files({}))
        self.assertEqual(lines['upload_balance'][0], 'PASS', lines['upload_balance'])
        self.assertIn('from status-start-1.txt', lines['upload_balance'][1])
        rest = STARTED.replace('upload_state unchecked', 'upload_state idle')
        rc, lines = recording(upload=with_files({'balance-start.txt': 'seconds 1\n' + rest}))
        self.assertIn('from balance-start.txt', lines['upload_balance'][1])
        busy = rest.replace('cuts 0', 'cuts 2').replace('cuts_held 0', 'cuts_held 1')
        rc, lines = recording(upload=with_files({'balance-start.txt': busy}))
        self.assertEqual((rc, lines['upload_balance'][0]), (2, 'ERROR'), lines['upload_balance'])
        self.assertIn('not at rest', lines['upload_balance'][1])
        rc, lines = recording(upload=with_files({'status-start-2.txt': STARTED}))
        self.assertEqual((rc, lines['rig_error'][0]), (2, 'ERROR'))
        self.assertIn('no balance-start.txt', lines['rig_error'][1])

    def test_the_blocks_written_before_a_failure(self):
        """ufailed's blocks written to the device outside the pool: node
        B's last image holds each, or the line says why not (review of
        #119, D4)."""
        d = tempfile.mkdtemp(prefix='cdc-raw-')
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        self.assertIsNone(cdc.raw_held(d))
        h = [cdc.hashlib.sha256(cdc.raw_block(k)).digest()[:8].hex() for k in range(3)]
        for fn, body in (('raw.tsv', 'raw\t100\t3\n'),
                         ('uploaded-blocks.tsv', f'0\t1\t100\t{h[0]}\n0\t1\t101\t{h[1]}\n1\t2\t101\t{cdc.ZERO}\n'
                                                 f'1\t2\t102\t{h[2]}\n')):
            with open(os.path.join(d, fn), 'w') as f:
                f.write(body)
        self.assertEqual(cdc.raw_held(d), (2, 3))       # block 101 was zeroed at the second fetch
        self.assertNotEqual(cdc.raw_block(0), cdc.raw_block(1))
        # Blocks the reference never had either: the course shows nothing.
        rc, lines = recording(upload=lambda w, b, st: (w, b, st, {'raw.tsv': 'raw\t120000\t4\n'}))
        self.assertEqual((rc, lines['upload_raw'][0]), (2, 'ERROR'), lines['upload_raw'])
        self.assertIn('0 of the 4 blocks', lines['upload_raw'][1])
        rc, lines = recording(upload=lambda w, b, st: (w, b, st))
        self.assertNotIn('upload_raw', lines)

    def test_a_course_that_stops_the_uploads(self):
        """umoved from end to end: the cuts before the other writer's head
        are judged, its head is skipped, and the last fetched is the last
        before it. A daemon that moved head back and uploaded once more
        fails the content and the course."""
        def stopped(more):
            def up(written, blocks, st):
                m = len(written) // 2
                kept = written[:m + 1]
                last = kept[-1]
                other = dict(last, head=OTHER_HEAD, key='other', blocks=last['blocks'] + 1)
                blocks['other'] = {0: '5a' * 8}
                fetched = kept + [other]
                if more:
                    nxt = dict(written[m + 1], head='dd' * 32)
                    # Since the other writer's head: its own blocks, and
                    # block 0 as the daemon's image has it.
                    blocks[nxt['number']] = dict(blocks[nxt['number']])
                    blocks[nxt['number']].setdefault(0, next(
                        (blocks[w['number']][0] for w in reversed(kept) if 0 in blocks[w['number']]), cdc.ZERO))
                    fetched.append(nxt)
                cut = (STATE, last['number'], last['group'])
                st = (st.replace('upload_mismatches 0', 'upload_mismatches 1')
                        .replace('upload_abandoned_at_mismatch 0', 'upload_abandoned_at_mismatch 2')
                        .replace('upload_checked matched', 'upload_checked head_moved'))
                st = re.sub(r'uploaded \d+', f'uploaded {m + 1}', st)
                # The cuts after the move went nowhere but to the abandoned.
                cuts = int(re.search(r'^cuts (\d+)$', st, flags=re.M).group(1))
                st = st.replace('cuts_abandoned 0', f'cuts_abandoned {cuts - m - 1}')
                files = {'moved-row.txt': row_file(cut), 'moved-status.txt': f'uploaded {m + 1}\n',
                         'end-row.txt': row_file(cut, head=OTHER_HEAD) if not more else
                         row_file((STATE, fetched[-1]['number'], fetched[-1]['group']), head='dd' * 32),
                         'others.tsv': f'head\t{OTHER_HEAD}\n'}
                return fetched, blocks, st, files
            return up
        rc, lines = recording(upload=stopped(False), as_name='umoved')
        self.assertEqual(rc, 0, {k: v for k, v in lines.items() if v[0] in ('FAIL', 'ERROR')})
        for key in ('upload_content', 'upload_status', 'upload_course'):
            self.assertEqual(lines[key][0], 'PASS', (key, lines[key]))
        self.assertIn("and 1 of another writer's", lines['upload_content'][1])
        rc, lines = recording(upload=stopped(True), as_name='umoved')
        self.assertEqual(rc, 1)
        self.assertEqual(lines['upload_content'][0], 'FAIL', lines['upload_content'])
        self.assertIn('the last cut written is', lines['upload_content'][1])
        self.assertEqual(lines['upload_course'][0], 'FAIL', lines['upload_course'])

    def test_a_cut_under_two_commits(self):
        """The daemon's cut at head again under another commit that no
        other writer made (the same number and group, the same blocks) is
        a verdict, though the image is right."""
        def again(written, blocks, st):
            last = written[-1]
            blocks['again'] = {}
            return written + [dict(last, head='dd' * 32, key='again')], blocks, st
        rc, lines = recording(upload=again)
        self.assertEqual((rc, lines['upload_content'][0]), (1, 'FAIL'), lines['upload_content'])
        self.assertIn('at head under two commits', lines['upload_content'][1])


if __name__ == '__main__':
    unittest.main()
