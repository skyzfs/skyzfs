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


def request_log(records, read, dev, cuts_at=None, start=True):
    """A fixture's records as the daemon's request log, and the marks.

    A write: recv, label (where it touches a label), applied. A flush:
    recv, barrier, and a cut line where cuts_at {record: (txg, itself)}
    has one. A flush with data flushes before its data; a write with FUA
    is followed by the daemon's fua_flush. -> (log text, marks text,
    {record index of the fixture: barrier's tag and op})."""
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
            if cut_ref.touches_label(r['offset'], r['bytes'], labels):
                line('label', '3', 'write', sec, n, read(r).hex())
            line('applied', '3', 'write')
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
        for ctl, wrong in cdc.mutants(dcuts).items():
            ok, detail = cdc.compare(wrong, ref)
            self.assertFalse(ok, ctl)
        ecs, _ = cut_ref.cuts(recs, ubs, 'early')
        eref = cdc.reference(ecs, cut_ref.opens(recs, rread, dev, ecs))
        ok, detail = cdc.compare(dcuts, eref)
        self.assertFalse(ok, 'early')

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

    def test_discards_and_write_zeroes_at_their_recv(self):
        text = (f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n2\t{PID}\trecv\t3\twrite_zeroes\t8\t8\t-\n'
                f'3\t{PID}\trecv\t4\tdiscard\t16\t8\t-\n')
        recs, _ = cdc.records_of(cdc.parse_log(text)[0]['lines'], [])
        self.assertEqual([(r['flags'], r['offset']) for r in recs], [(['discard'], 4096), (['discard'], 8192)])


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


class Expected(unittest.TestCase):
    def test_every_control_is_caught(self):
        self.assertEqual(cdc.EXPECTED, {c: 'caught' for c in ('drop', 'group', 'opens', 'early')})


if __name__ == '__main__':
    unittest.main()
