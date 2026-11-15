#!/usr/bin/env python3
"""Tests of the cut rig's check on the daemon (M2.T3, first stage) that
need no kernel: a recorded pool's fixture (tests/fixtures/pools) written
out as the daemon's request log would hold it, read back, and held to
the reference."""
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_daemon_check as cdc  # noqa: E402
import cut_ref  # noqa: E402
from cut_check import RigError  # noqa: E402

FIXTURES = os.path.join(HERE, '..', 'fixtures', 'pools')
PID = '4242'


def request_log(records, read, dev, cuts_at=None, start=True, stray_before=()):
    """A fixture's records as the daemon's request log, and the marks.

    A write: recv, label (where it touches a label), applied. A flush:
    recv, barrier, and a cut line where cuts_at {record: (txg, itself)}
    has one. A flush with data flushes before its data; a write with FUA
    is followed by the daemon's fua_flush. -> (log text, marks text,
    {record index of the fixture: barrier's tag and op}).

    stray_before: writes of the fixture that a stray flush is received
    after and reaches its barrier between their apply and their commit,
    as run 43676360281's stray recording had it."""
    labels = cut_ref.label_offsets(dev)
    out, marks = [], []
    ns = 1000

    def line(event, tag, op='-', sector='-', sectors='-', result='-'):
        nonlocal ns
        ns += 10
        out.append(f'{ns}\t{PID}\t{event}\t{tag}\t{op}\t{sector}\t{sectors}\t{result}')

    if start:
        line('start', '0', result='64')
    cuts_at = cuts_at or {}

    def barrier(r, op):
        line('recv', '1', 'flush', '0', '0')
        line('barrier', '1', op)
        c = cuts_at.get((r['i'], op))
        if c:
            line('cut', '1', op, 'itself' if c[1] else 'rewind', '-', str(c[0]))

    for r in records:
        flags = set(r['flags'])
        if 'mark' in flags:
            # between the lines before it and those after it
            ns += 5
            marks.append(f"{ns}\t{r['mark']}")
            continue
        if 'flush' in flags:
            barrier(r, 'flush')
        if 'discard' in flags and r['bytes']:
            line('recv', '2', 'discard', str(r['offset'] // 512), str(r['bytes'] // 512))
        elif cut_ref.is_write(r):
            sec, n = str(r['offset'] // 512), str(r['bytes'] // 512)
            line('recv', '3', 'write', sec, n)
            stray = r['i'] in stray_before
            if stray:
                line('recv', '63', 'flush', '0', '0')
            if cut_ref.touches_label(r['offset'], r['bytes'], labels):
                line('label', '3', 'write', sec, n, read(r).hex())
            line('applied', '3', 'write')
            if stray:
                line('barrier', '63', 'flush')
            line('commit', '3', 'write', sec, n, str(r['bytes']))
            if 'fua' in flags:
                barrier(r, 'fua_flush')
    return '\n'.join(out) + '\n', '\n'.join(marks) + ('\n' if marks else '')


def fixture(name):
    records, read, dev = cut_ref.read_fixture(os.path.join(FIXTURES, name))
    return records, read, dev


def daemon_cuts_of(records, read, dev):
    """The reference's cuts on the fixture, keyed as request_log's cuts_at."""
    ubs = cut_ref.scan(records, read, dev)
    cs, _ = cut_ref.cuts(records, ubs)
    op = cut_ref.opens(records, read, dev, cs)
    by_i = {r['i']: r for r in records}
    at = {}
    for c in cs:
        r = by_i[c['flush_record']]
        # a cut whose barrier is the fua_flush after a write
        kind = 'fua_flush' if ('fua' in r['flags'] and 'flush' not in r['flags']) or c['prefix'] == r['i'] + 1 \
            else 'flush'
        at[(c['flush_record'], kind)] = (c['txg'], op[c['n']]['itself'])
    return cs, at


class RoundTrip(unittest.TestCase):
    """A fixture through the daemon's log and back: the reference on the
    log's records is the reference on the fixture's."""

    def run_on(self, name):
        records, read, dev = fixture(name)
        cs, at = daemon_cuts_of(records, read, dev)
        text, marks = request_log(records, read, dev, at)
        inst = cdc.parse_log(text)
        self.assertEqual(len(inst), 1)
        recs, dcuts = cdc.records_of(inst[0]['lines'], cdc.read_marks(marks))
        rread = cdc.read_of(recs, dev)
        ubs = cut_ref.scan(recs, rread, dev)
        rcs, _ = cut_ref.cuts(recs, ubs)
        ref = cdc.reference(rcs, cut_ref.opens(recs, rread, dev, rcs))
        self.assertEqual([(c['txg']) for c in rcs], [c['txg'] for c in cs])
        ok, detail = cdc.compare(dcuts, ref)
        self.assertTrue(ok, detail)
        return recs, rread, dev, dcuts, ref, ubs

    def test_life(self):
        recs, _, _, dcuts, ref, _ = self.run_on('life')
        self.assertGreater(len(ref), 10)
        # the marks come through, in order
        names = [r['mark'] for r in recs if 'mark' in r['flags']]
        self.assertIn('session-1-begin', names)
        self.assertLess(names.index('session-1-begin'), names.index('session-1-end'))

    def test_rewind_has_cuts_that_do_not_open_as_themselves(self):
        _, _, _, dcuts, ref, _ = self.run_on('rewind')
        self.assertTrue(any(not r['itself'] for r in ref))
        self.assertTrue(any(not d['itself'] for d in dcuts))

    def test_every_control_is_caught(self):
        recs, rread, dev, dcuts, ref, ubs = self.run_on('life')
        barriers = [r['i'] for r in recs if 'flush' in r['flags']]
        wrongs = cdc.mutants(dcuts, barriers)
        self.assertEqual(set(wrongs), set(cdc.LISTS))
        for ctl, wrong in wrongs.items():
            ok, detail = cdc.compare(wrong, ref)
            self.assertFalse(ok, ctl)
        ecs, _ = cut_ref.cuts(recs, ubs, 'early')
        eref = cdc.reference(ecs, cut_ref.opens(recs, rread, dev, ecs))
        ok, detail = cdc.compare(dcuts, eref)
        self.assertFalse(ok, 'early')

    def test_each_list_is_told_by_what_it_names(self):
        """last and extra by the count alone, late by the instant alone (the
        review of #99, S1)."""
        recs, _, _, dcuts, ref, _ = self.run_on('life')
        wrongs = cdc.mutants(dcuts, [r['i'] for r in recs if 'flush' in r['flags']])
        for ctl in ('last', 'extra'):
            self.assertIn('the daemon counted', cdc.compare(wrongs[ctl], ref)[1], ctl)
        mid = len(dcuts) // 2
        late = wrongs['late'][mid]
        self.assertEqual((late['txg'], late['itself']), (dcuts[mid]['txg'], dcuts[mid]['itself']))
        self.assertGreater(late['prefix'], dcuts[mid]['prefix'])
        self.assertEqual(cdc.mutants([], []), {c: None for c in cdc.LISTS})
        self.assertIsNone(cdc.mutants(dcuts, [r['i'] for r in recs if 'flush' in r['flags']
                                              and r['i'] <= dcuts[mid]['prefix']])['late'])

    def test_the_rules_on_the_fixtures(self):
        """Each rule control is caught where EXPECTED says it must be, on the
        recordings of M2.T1 read through the daemon's log."""
        for name in ('life', 'rewind', 'multihost'):
            recs, rread, dev, dcuts, ref, ubs = self.run_on(name)
            for rule in cdc.RULES:
                rcs, _ = cut_ref.cuts(recs, ubs, rule)
                rref = cdc.reference(rcs, cut_ref.opens(recs, rread, dev, rcs))
                same = rule != 'early' and cdc.compare(rref, ref)[0]
                verdict = 'same' if same else ('missed' if cdc.compare(dcuts, rref)[0] else 'caught')
                want = cdc.EXPECTED[rule][name]
                self.assertIn(verdict, want if isinstance(want, tuple) else (want,), (name, rule))

    def test_a_flush_received_before_the_uberblock_was_applied(self):
        """Design §5.2 cuts at the first flush *received* after the mark.
        A stray flush received while an uberblock write was in the store
        and whose barrier comes after the write's apply carries no cut:
        the next flush does (run 43676360281, stray, both images). The
        flush placed at its barrier alone, as the check had it, cuts at
        the stray flush."""
        records, read, dev = fixture('life')
        cs, at = daemon_cuts_of(records, read, dev)
        first = cs[0]['ub_record']
        text, marks = request_log(records, read, dev, at, stray_before={first})
        recs, dcuts = cdc.records_of(cdc.parse_log(text)[0]['lines'], cdc.read_marks(marks))
        rread = cdc.read_of(recs, dev)
        ubs = cut_ref.scan(recs, rread, dev)
        rcs, _ = cut_ref.cuts(recs, ubs)
        ref = cdc.reference(rcs, cut_ref.opens(recs, rread, dev, rcs))
        ok, detail = cdc.compare(dcuts, ref)
        self.assertTrue(ok, detail)
        self.assertEqual(len(ref), len(cs))
        stray = [r['i'] for r in recs if 'flush' in r['flags'] and r.get('tag') == '63']
        self.assertEqual(len(stray), 1)
        self.assertNotEqual(ref[0]['prefix'], stray[0])
        # The negative: each flush one point, at its barrier.
        flat = [dict(r, received=False) for r in recs if 'receipt' not in r['flags']]
        for k, r in enumerate(flat):
            r['i'] = k
        fubs = cut_ref.scan(flat, cdc.read_of(flat, dev), dev)
        fcs, _ = cut_ref.cuts(flat, fubs)
        self.assertEqual(fcs[0]['flush_record'], [r['i'] for r in flat if r.get('tag') == '63' and 'flush' in r['flags']][0])

    def test_a_copy_applied_while_a_flush_carries_its_state(self):
        """A stray flush received after the first copy of a group's
        uberblock was applied, with its barrier after the second: it cuts
        the group at its own barrier, and the copy it carries past marks
        nothing, so the group is cut once (the review of #99, S5, its
        test_receipt.py)."""
        records, read, dev = fixture('life')
        cs, _ = daemon_cuts_of(records, read, dev)
        first = cs[0]
        copies = sorted({u['record'] for u in cut_ref.scan(records, read, dev)
                         if (u['txg'], u['rootbp']) == (first['txg'], first['rootbp'])})
        self.assertGreaterEqual(len(copies), 2)
        text, marks = request_log(records, read, dev, {}, stray_before={copies[1]})
        recs, _ = cdc.records_of(cdc.parse_log(text)[0]['lines'], cdc.read_marks(marks))
        rread = cdc.read_of(recs, dev)
        rcs, _ = cut_ref.cuts(recs, cut_ref.scan(recs, rread, dev))
        stray = [r['i'] for r in recs if 'flush' in r['flags'] and r.get('tag') == '63']
        self.assertEqual(len(stray), 1)
        self.assertEqual(rcs[0]['flush_record'], stray[0], "the stray flush carried the first copy's mark")
        self.assertEqual([c['txg'] for c in rcs], [c['txg'] for c in cs], 'each group is cut once')

    def test_a_mark_made_since_the_receipt_outlives_the_barrier(self):
        """A flush received before every copy of a group's uberblock was
        applied, whose barrier comes after the last copy, carries no mark;
        the mark the copies made is the next flush's to cut (the review of
        #99, S5: cut_ref.cuts's `mark = kept`)."""
        records, read, dev = fixture('life')
        cs, _ = daemon_cuts_of(records, read, dev)
        first = cs[0]
        copies = sorted({u['record'] for u in cut_ref.scan(records, read, dev)
                         if (u['txg'], u['rootbp']) == (first['txg'], first['rootbp'])})
        self.assertTrue(all(c < first['flush_record'] for c in copies))
        # The fixture reads a record's bytes by its number: the two added
        # records take numbers between their neighbours'.
        out = []
        for r in records:
            if r['i'] == copies[0]:
                out.append({'i': r['i'] - 0.5, 'flags': ['receipt'], 'offset': 0, 'bytes': 0, 'tag': '63'})
            out.append(r)
            if r['i'] == copies[-1]:
                out.append({'i': r['i'] + 0.5, 'flags': ['flush'], 'offset': 0, 'bytes': 0, 'tag': '63',
                            'received': True})
        rcs, _ = cut_ref.cuts(out, cut_ref.scan(out, read, dev))
        stray = [r['i'] for r in out if r.get('tag') == '63' and 'flush' in r['flags']]
        self.assertEqual([c['txg'] for c in rcs], [c['txg'] for c in cs])
        self.assertNotIn(stray[0], [c['flush_record'] for c in rcs])
        self.assertGreater(rcs[0]['flush_record'], stray[0])

    def test_guards_of_the_receipt(self):
        with self.assertRaises(ValueError):
            cut_ref.cuts([{'i': 0, 'flags': ['receipt'], 'tag': '1', 'offset': 0, 'bytes': 0},
                          {'i': 1, 'flags': ['receipt'], 'tag': '1', 'offset': 0, 'bytes': 0}], [])
        text = f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n2\t{PID}\tbarrier\t1\tflush\t-\t-\t-\n'
        with self.assertRaises(RigError):
            cdc.records_of(cdc.parse_log(text)[0]['lines'], [])

    def test_a_cut_line_with_no_barrier_before_it(self):
        text = f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n2\t{PID}\tcut\t1\tflush\titself\t-\t9\n'
        with self.assertRaises(RigError):
            cdc.records_of(cdc.parse_log(text)[0]['lines'], [])


class Parse(unittest.TestCase):
    def test_instances(self):
        text = (f'1\t7\tstart\t0\t-\t-\t-\t64\n2\t7\trecv\t3\twrite\t0\t8\t-\n'
                f'3\t8\tstart\t0\t-\t-\t-\t64\n4\t7\tapplied\t3\twrite\t-\t-\t-\n')
        inst = cdc.parse_log(text)
        self.assertEqual([i['pid'] for i in inst], ['7', '8'])
        self.assertEqual([ln['event'] for ln in inst[0]['lines']], ['recv', 'applied'])

    def test_the_columns_of_an_applied_line_since_m2_2b_2(self):
        text = (f'1\t7\tstart\t0\t-\t-\t-\t64\n2\t7\tcuts\t-\t-\t-\t-\toff\n'
                f'3\t7\trecv\t3\twrite\t8\t8\t-\n4\t7\tapplied\t3\twrite\t-\t-\t-\t0123456789abcdef\n'
                f'5\t7\trecv\t4\twrite\t16\t8\t-\n6\t7\tapplied\t4\twrite\t-\t-\t-\t0123456789abcdef\tdata=4096\n')
        recs, _ = cdc.records_of(cdc.parse_log(text)[0]['lines'], [])
        self.assertEqual([r['offset'] for r in recs], [4096, 8192])
        for bad in (f'1\t7\tstart\t0\t-\t-\t-\t64\t9\n',
                    f'1\t7\tstart\t0\t-\t-\t-\t64\n2\t7\tapplied\t3\twrite\t-\t-\t-\ta\tb\tc\n'):
            with self.assertRaises(RigError):
                cdc.parse_log(bad)

    def test_bad_lines(self):
        for text in ('1\t7\tstart\n', f'x\t7\tstart\t0\t-\t-\t-\t64\n', '1\t9\trecv\t3\twrite\t0\t8\t-\n'):
            with self.assertRaises(RigError):
                cdc.parse_log(text)

    def test_an_applied_write_with_no_recv(self):
        text = f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n2\t{PID}\tapplied\t3\twrite\t-\t-\t-\n'
        with self.assertRaises(RigError):
            cdc.records_of(cdc.parse_log(text)[0]['lines'], [])

    def test_a_write_to_a_label_with_no_label_line(self):
        text = (f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n2\t{PID}\trecv\t3\twrite\t256\t8\t-\n'
                f'3\t{PID}\tapplied\t3\twrite\t-\t-\t-\n')
        recs, _ = cdc.records_of(cdc.parse_log(text)[0]['lines'], [])
        with self.assertRaises(RigError):
            cdc.read_of(recs, 512 << 20)(recs[0])

    def test_write_zeroes_at_its_recv_and_a_discard_changes_nothing(self):
        """The daemon keeps a discarded block's content (R6); a
        write-zeroes reads as zeros, placed by its recv line."""
        text = (f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n2\t{PID}\trecv\t3\twrite_zeroes\t8\t8\t-\n'
                f'3\t{PID}\trecv\t4\tdiscard\t16\t8\t-\n')
        recs, _ = cdc.records_of(cdc.parse_log(text)[0]['lines'], [])
        self.assertEqual([(r['flags'], r['offset']) for r in recs], [(['discard'], 4096)])


class Status(unittest.TestCase):
    REF = [{'txg': 5, 'itself': True, 'prefix': 3}, {'txg': 9, 'itself': False, 'prefix': 7}]

    def test_as_the_reference(self):
        st = cdc.read_status('version 12\ncuts 2\ncut_group 9\ncut_opens rewind\n')
        self.assertTrue(cdc.status_verdict(st, self.REF)[0])

    def test_each_value(self):
        for text in ('cuts 3\ncut_group 9\ncut_opens rewind\n', 'cuts 2\ncut_group 8\ncut_opens rewind\n',
                     'cuts 2\ncut_group 9\ncut_opens itself\n', 'cuts 2\n'):
            ok, detail = cdc.status_verdict(cdc.read_status(text), self.REF)
            self.assertFalse(ok, text)

    def test_no_cut(self):
        self.assertTrue(cdc.status_verdict(cdc.read_status('cuts 0\n'), [])[0])

    def test_a_status_written_before_the_last_cut(self):
        """Run 43675273810: the status read at once after the export was
        one write behind (cuts 22 of 26). The rig reads it a second later.
        The values decide; the time only says why they differ (the review
        of #99, S3: age_ms is a difference of /proc/uptime, which ticks
        every 10 ms)."""
        last = cdc.read_status('cuts 2\ncut_group 9\ncut_opens rewind\nage_ms 80\n')
        stale = cdc.read_status('cuts 1\ncut_group 5\ncut_opens itself\nage_ms 80\n')
        ms = 1_000_000
        times = [100 * ms, 300 * ms]
        latest = cdc.status_window(last, f'{400 * ms}\t{420 * ms}\n')
        self.assertEqual(latest, 350 * ms)
        self.assertTrue(cdc.status_verdict(last, self.REF, times, latest)[0])
        ok, detail = cdc.status_verdict(stale, self.REF, times, latest)
        self.assertFalse(ok)
        self.assertNotIn('before the daemon', detail)
        latest = cdc.status_window(stale, f'{350 * ms}\t{360 * ms}\n')
        ok, detail = cdc.status_verdict(stale, self.REF, times, latest)
        self.assertFalse(ok)
        self.assertIn('before the daemon\'s last cut line, after at most 1 of 2', detail)
        # A right status whose window, read early, ends before the last
        # cut line: the clock's tick, not the daemon.
        self.assertTrue(cdc.status_verdict(last, self.REF, times, 299 * ms)[0])

    def test_no_time_for_the_status(self):
        for st, text in (('age_ms 3\n', ''), ('age_ms 3\n', '5\tx\n'), ('cuts 1\n', '5\t6\n'),
                         ('age_ms 3\n', '6\t5\n')):
            with self.assertRaises(RigError):
                cdc.status_window(cdc.read_status(st), text)


class Numbers(unittest.TestCase):
    def cuts(self, *nums):
        return [{'number': n, 'line': k + 10} for k, n in enumerate(nums)]

    def test_numbers(self):
        self.assertEqual(cdc.numbers_verdict(self.cuts('1', '2', '3'))[0], 'PASS')
        self.assertEqual(cdc.numbers_verdict(self.cuts('-', '-'))[0], 'INFO')
        self.assertEqual(cdc.numbers_verdict([])[0], 'INFO')
        for wrong in (('1', '3'), ('2', '3'), ('1', '1'), ('1', '-')):
            self.assertEqual(cdc.numbers_verdict(self.cuts(*wrong))[0], 'FAIL', wrong)


class Expected(unittest.TestCase):
    def test_the_table(self):
        self.assertEqual(set(cdc.EXPECTED), set(cdc.CONTROLS))
        for ctl in cdc.LISTS + ('early',):
            self.assertEqual(cdc.EXPECTED[ctl], {r: 'caught' for r in cdc.RECS}, ctl)
        for ctl in ('txg-alone', 'count-repeats', 'previous'):
            self.assertEqual(set(cdc.EXPECTED[ctl]), set(cdc.RECS), ctl)
        # where each rule must be told for sure
        self.assertEqual(cdc.EXPECTED['txg-alone']['rewind'], 'caught')
        self.assertEqual(cdc.EXPECTED['count-repeats']['multihost'], 'caught')


class Unplaced(unittest.TestCase):
    """What the log cannot place makes a failed `cuts` the rig's error (the
    review of #99, S4, and Codex's two findings)."""
    def test_unplaced(self):
        dev = 512 << 20
        recs = [{'flags': ['flush'], 'op': 'flush', 'offset': 0, 'bytes': 0},
                {'flags': ['discard'], 'op': 'write_zeroes', 'offset': 1 << 20, 'bytes': 4096}]
        self.assertEqual(cdc.unplaced(recs, dev), [])
        recs += [{'flags': ['flush'], 'op': 'fua_flush', 'offset': 0, 'bytes': 0},
                 {'flags': ['discard'], 'op': 'write_zeroes', 'offset': 200 << 10, 'bytes': 4096}]
        self.assertEqual(cdc.unplaced(recs, dev), ['1 flushes after a write with FUA', '1 write-zeroes over a label'])


if __name__ == '__main__':
    unittest.main()
