#!/usr/bin/env python3
"""Tests of the cut rig's check on the daemon (M2.T3, first stage) that
need no kernel: a recorded pool's fixture (tests/fixtures/pools) written
out as the daemon's request log would hold it, read back, and held to
the reference."""
import hashlib
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


def syn(i, b):
    """A write's stand-in hash of block b: the fixtures keep the bytes of
    label writes alone."""
    return hashlib.sha256(f'{i}:{b}'.encode()).digest()[:8].hex()


def syn_hashes(r):
    """An applied line's ninth column for fixture write r."""
    off, n = r['offset'], r['bytes']
    out = []
    for b in range(off // 4096, (off + n - 1) // 4096 + 1):
        out.append(syn(r['i'], b) if b * 4096 >= off and (b + 1) * 4096 <= off + n else '-')
    return ','.join(out)


def request_log(records, read, dev, cuts_at=None, start=True, stray_before=(), hashes=False):
    """A fixture's records as the daemon's request log, and the marks.

    A write: recv, label (where it touches a label), applied. A flush:
    recv, barrier, and a cut line where cuts_at {record: (txg, itself)}
    has one. A flush with data flushes before its data; a write with FUA
    is followed by the daemon's fua_flush. -> (log text, marks text,
    {record index of the fixture: barrier's tag and op}).

    stray_before: writes of the fixture that a stray flush is received
    after and reaches its barrier between their apply and their commit,
    as run 43676360281's stray recording had it. hashes: applied lines
    carry stand-in hashes (syn), and cut lines their numbers."""
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
    counted = 0

    def barrier(r, op):
        line('recv', '1', 'flush', '0', '0')
        line('barrier', '1', op)
        nonlocal counted
        c = cuts_at.get((r['i'], op))
        if c:
            counted += 1
            line('cut', '1', op, 'itself' if c[1] else 'rewind', str(counted) if hashes else '-', str(c[0]))

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
            if hashes:
                out.append(f'{ns + 1}\t{PID}\tapplied\t3\twrite\t-\t-\t-\t{syn_hashes(r)}')
                ns += 1
            else:
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


def held_images(records, cs, op):
    """The images a right daemon would leave, built from a fixture's own
    records apart from the check's model: a write sets its blocks, a
    discard keeps them (R6). -> (held.tsv's cuts, {number: map's block
    hashes}, the image after the drain)."""
    image, at_rec, prev = {}, 0, {}
    written, blocks = [], {}
    for c in cs:
        while at_rec < c['prefix']:
            r = records[at_rec]
            if cut_ref.is_write(r):
                for b in range(r['offset'] // 4096, (r['offset'] + r['bytes'] - 1) // 4096 + 1):
                    image[b] = syn(r['i'], b)
            at_rec += 1
        changed = {b: h for b, h in image.items() if prev.get(b) != h}
        written.append({'number': c['n'], 'group': c['txg'], 'blocks': len(changed),
                        'opens': 'itself' if op[c['n']]['itself'] else 'rewind'})
        blocks[c['n']] = changed
        prev = dict(image)
    return written, blocks, dict(image)


START_STATUS = 'cuts 0\ncut_state begun\nintent_state begun\nintent_regions 128\nintent_named_at_start 128\n'


COMMITTED = '1\t> t1 held\n2\t> t2 release 1\n3\t< t2 ok (0 lines)\n'   # a release answered before every start


def holder_files(d, written, blocks, final, log=COMMITTED, held_final='9 9 uploading itself 3\n',
                 status_held='cuts_held 1\n', starts=(START_STATUS,)):
    """The holder's files in recording directory d, and the status the rig
    read at each start (status-start-K.txt)."""
    def w(fn, text):
        with open(os.path.join(d, fn), 'w') as f:
            f.write(text)
    for k, st in enumerate(starts):
        w(f'status-start-{k + 1}.txt', st)
    w('holder.log', log)
    w('held.tsv', ''.join(f"{x['number']}\t{x['group']}\t{x['opens']}\t{x['blocks']}\t{x.get('asked', 1)}\t"
                          f"{x.get('asked', 1) + 1}\n" for x in written))
    # blocks: {number: map}, or a list of maps in written's order (numbers
    # that repeat across states, stage 2c)
    pairs = zip((x['number'] for x in written), blocks) if isinstance(blocks, list) else blocks.items()
    w('held-blocks.tsv', ''.join(f'{n}\t{b}\t{h}\n' for n, m in pairs for b, h in m.items()))
    w('held-image.tsv', ''.join(f'{b}\t{h}\n' for b, h in sorted(final.items())))
    w('held-final.txt', held_final)
    w('status-held.txt', status_held)


UNPLACED_WRITTEN = [{'number': 1, 'group': 5, 'opens': 'itself', 'blocks': 2},
                    {'number': 2, 'group': 6, 'opens': 'itself', 'blocks': 1}]


def unplaced_log(rewrite=False, rezero=False):
    """A write of block 1 (a) received, a write-zeroes over block 1
    received before it is applied (the store runs the write first, then
    the zeros; the model the other way), a write of block 2 (b), cut 1;
    then (rewrite) a write of block 1 (d) received after the
    write-zeroes, or (rezero) a write-zeroes of block 1 with nothing
    pending, a write of block 3 (c), cut 2 -> (records, cuts)."""
    a, b, c, dd = 'a' * 16, 'b' * 16, 'c' * 16, 'd' * 16
    text = (f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n'
            f'2\t{PID}\trecv\t3\twrite\t8\t8\t-\n3\t{PID}\trecv\t4\twrite_zeroes\t8\t8\t-\n'
            f'4\t{PID}\tapplied\t3\twrite\t-\t-\t-\t{a}\n5\t{PID}\tcommit\t4\twrite_zeroes\t8\t8\t0\n'
            f'6\t{PID}\trecv\t5\twrite\t16\t8\t-\n7\t{PID}\tapplied\t5\twrite\t-\t-\t-\t{b}\n'
            f'8\t{PID}\trecv\t1\tflush\t0\t0\t-\n9\t{PID}\tbarrier\t1\tflush\t-\t-\t-\n'
            f'10\t{PID}\tcut\t1\tflush\titself\t1\t5\n'
            + (f'11\t{PID}\trecv\t3\twrite\t8\t8\t-\n12\t{PID}\tapplied\t3\twrite\t-\t-\t-\t{dd}\n'
               if rewrite else '')
            + (f'11\t{PID}\trecv\t7\twrite_zeroes\t8\t8\t-\n12\t{PID}\tcommit\t7\twrite_zeroes\t8\t8\t0\n'
               if rezero else '')
            + f'13\t{PID}\trecv\t6\twrite\t24\t8\t-\n14\t{PID}\tapplied\t6\twrite\t-\t-\t-\t{c}\n'
            f'15\t{PID}\trecv\t1\tflush\t0\t0\t-\n16\t{PID}\tbarrier\t1\tflush\t-\t-\t-\n'
            f'17\t{PID}\tcut\t1\tflush\titself\t2\t6\n')
    return cdc.records_of(cdc.parse_log(text)[0]['lines'], [])


class HeldFixture(unittest.TestCase):
    """The images a right daemon would leave on the fixture life."""

    def setUp(self):
        records, read, dev = fixture('life')
        cs, at = daemon_cuts_of(records, read, dev)
        op = cut_ref.opens(records, read, dev, cs)
        text, marks = request_log(records, read, dev, at, hashes=True)
        self.recs, self.dcuts = cdc.records_of(cdc.parse_log(text)[0]['lines'], cdc.read_marks(marks))
        self.written, self.blocks, self.final = held_images(records, cs, op)

    def wrong_in_mid(self, k=1, value='0' * 16):
        """The maps with the first k blocks of the middle written cut's
        map held wrong -> (maps, the blocks made wrong)."""
        mid = self.written[len(self.written) // 2]['number']
        blocks = {n: dict(m) for n, m in self.blocks.items()}
        bad = sorted(blocks[mid])[:k]
        for b in bad:
            blocks[mid][b] = value
        return blocks, bad

    def judge(self, written=None, blocks=None, final=None):
        return cdc.held_content(self.recs, self.dcuts, written or self.written, blocks or self.blocks,
                                self.final if final is None else final)


class HeldContent(HeldFixture):
    """The second stage: the content of each held cut the holder wrote,
    against the reference's at the cut's instant."""

    def test_a_right_daemon(self):
        ok, detail, controls, _ = self.judge()
        self.assertTrue(ok, detail)
        self.assertIn(f'{len(self.written)} of {len(self.dcuts)} cuts written', detail)
        self.assertEqual({c: v for c, (v, _) in controls.items()}, cdc.CONTENT_EXPECTED)

    def test_some_cuts_written(self):
        """Cuts let go or replaced while waiting are never written: the next
        written cut's map holds their blocks."""
        keep = [w for k, w in enumerate(self.written) if k % 3 == 2 or k == len(self.written) - 1]
        blocks, acc = {}, {}
        for w in self.written:
            acc.update(self.blocks[w['number']])
            if w in keep:
                blocks[w['number']], acc = acc, {}
        ok, detail, _, _ = self.judge(keep, blocks)
        self.assertTrue(ok, detail)

    def test_a_block_held_wrong(self):
        mid = self.written[len(self.written) // 2]['number']
        blocks = {n: dict(m) for n, m in self.blocks.items()}
        b = sorted(blocks[mid])[0]
        blocks[mid][b] = '0' * 16
        ok, detail, _, _ = self.judge(blocks=blocks)
        self.assertIs(ok, False)
        self.assertIn(f'cut {mid}', detail)
        self.assertIn(f'block {b}: ', detail)
        self.assertIn(f"{'0' * 16} held", detail)

    def test_a_block_left_out_of_a_map(self):
        mid = self.written[len(self.written) // 2]['number']
        blocks = {n: dict(m) for n, m in self.blocks.items()}
        del blocks[mid][sorted(blocks[mid])[-1]]
        self.assertFalse(self.judge(blocks=blocks)[0])

    def test_the_held_group_differs_from_the_cut_line(self):
        written = [dict(w) for w in self.written]
        written[1]['group'] += 1
        ok, detail = self.judge(written)[:2]
        self.assertIs(ok, False)
        self.assertIn('its cut line says', detail)

    def test_the_held_answer_differs_from_the_cut_line(self):
        written = [dict(w) for w in self.written]
        written[1]['opens'] = 'rewind' if written[1]['opens'] == 'itself' else 'itself'
        ok, detail = self.judge(written)[:2]
        self.assertIs(ok, False)
        self.assertIn('its cut line says', detail)

    def test_a_block_written_and_in_no_map(self):
        """A block the reference wrote that no map held and the image does
        not hold (the review of #103, S2): zeros held, its write wanted."""
        once = {}
        for n, m in self.blocks.items():
            for b in m:
                once[b] = once.get(b, 0) + 1
        n, b = next((n, b) for n, m in self.blocks.items() for b in sorted(m) if once[b] == 1)
        blocks = {k: dict(m) for k, m in self.blocks.items()}
        del blocks[n][b]
        final = dict(self.final)
        del final[b]
        ok, detail = self.judge(blocks=blocks, final=final)[:2]
        self.assertIs(ok, False)
        self.assertIn(f'block {b}: ', detail)

    def test_a_block_never_written_held_in_a_map(self):
        """Content the daemon put, inside a map, in a block no write
        touched (the review of #103, S2)."""
        mid = self.written[len(self.written) // 2]['number']
        blocks = {k: dict(m) for k, m in self.blocks.items()}
        blocks[mid][10 ** 5] = 'f' * 16
        final = dict(self.final)
        final[10 ** 5] = 'f' * 16
        ok, detail = self.judge(blocks=blocks, final=final)[:2]
        self.assertIs(ok, False)
        self.assertIn(f'block {10 ** 5}: {cdc.ZERO} wanted', detail)

    def test_the_last_cut_not_written(self):
        final = {}
        for w in self.written[:-1]:
            final.update(self.blocks[w['number']])
        final = {b: h for b, h in final.items() if h != cdc.ZERO}
        ok, detail = self.judge(self.written[:-1], final=final)[:2]
        self.assertIs(ok, False)
        self.assertIn('the last cut written', detail)
        self.assertNotIn('after the drain', detail)

    def test_a_block_outside_every_map(self):
        final = dict(self.final)
        final[10 ** 5] = 'f' * 16
        ok, detail = self.judge(final=final)[:2]
        self.assertIs(ok, False)
        self.assertIn('differ from what the maps wrote', detail)

    def test_no_cut_written(self):
        with self.assertRaises(RigError):
            cdc.held_content(self.recs, self.dcuts, [], {})

    def test_a_discarded_block_reads_either(self):
        """A block discarded after a write: its old content or zeros (the
        lead, M2.T3); a block a write covers in part is not known."""
        a = 'a' * 16
        text = (f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n'
                f'2\t{PID}\trecv\t3\twrite\t16\t8\t-\n3\t{PID}\tapplied\t3\twrite\t-\t-\t-\t{a}\n'
                f'4\t{PID}\trecv\t4\twrite\t81\t2\t-\n5\t{PID}\tapplied\t4\twrite\t-\t-\t-\t-\n'
                f'6\t{PID}\trecv\t1\tflush\t0\t0\t-\n7\t{PID}\tbarrier\t1\tflush\t-\t-\t-\n'
                f'8\t{PID}\tcut\t1\tflush\titself\t1\t5\n'
                f'9\t{PID}\trecv\t2\tdiscard\t16\t8\t-\n'
                f'10\t{PID}\trecv\t1\tflush\t0\t0\t-\n11\t{PID}\tbarrier\t1\tflush\t-\t-\t-\n'
                f'12\t{PID}\tcut\t1\tflush\titself\t2\t6\n')
        recs, dcuts = cdc.records_of(cdc.parse_log(text)[0]['lines'], [])
        written = [{'number': 1, 'group': 5, 'opens': 'itself', 'blocks': 2},
                   {'number': 2, 'group': 6, 'opens': 'itself', 'blocks': 1}]
        for held, want in ((a, True), (cdc.ZERO, True), ('b' * 16, False)):
            blocks = {1: {2: a, 10: 'c' * 16}, 2: {2: held}}
            ok, detail, _, _ = cdc.held_content(recs, dcuts, written, blocks)
            self.assertEqual(ok, want, (held, detail))
        ok, detail, _, _ = cdc.held_content(recs, dcuts, written, {1: {2: a, 10: 'c' * 16}, 2: {2: a}})
        self.assertIn('1 of them discarded', detail)
        self.assertIn('not known to the reference', detail)

    def test_the_blocks_held_wrong_are_named(self):
        mid = self.written[len(self.written) // 2]['number']
        blocks = {n: dict(m) for n, m in self.blocks.items()}
        b = sorted(blocks[mid])[0]
        blocks[mid][b] = '0' * 16
        self.assertIn(b, self.judge(blocks=blocks)[3])
        self.assertEqual(self.judge()[3], set())

    def test_a_write_zeroes_with_an_overlapping_write_in_the_store(self):
        """Only the blocks a write-zeroes shares with a write received
        before it and not applied yet are not placed for certain."""
        text = (f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n'
                f'2\t{PID}\trecv\t3\twrite\t8\t16\t-\n'
                f'3\t{PID}\trecv\t4\twrite_zeroes\t16\t16\t-\n'
                f'4\t{PID}\tapplied\t3\twrite\t-\t-\t-\t{"a" * 16},{"b" * 16}\n'
                f'5\t{PID}\trecv\t5\twrite_zeroes\t64\t8\t-\n')
        recs, _ = cdc.records_of(cdc.parse_log(text)[0]['lines'], [])
        wz = [r for r in recs if r.get('op') == 'write_zeroes']
        self.assertEqual([r['ambiguous'] for r in wz], [{2}, set()])

    def test_the_unplaced_blocks_of_each_cut(self):
        """Codex on #103: ambiguity is the cuts' after the write-zeroes,
        ends where a write received after it sets the block again, and a
        definite wrong block beside it stays a verdict."""
        b, c = 'b' * 16, 'c' * 16
        recs, dcuts = unplaced_log()
        right = {1: {1: cdc.ZERO, 2: b}, 2: {3: c}}
        ok, detail, _, wrong = cdc.held_content(recs, dcuts, UNPLACED_WRITTEN, right, {2: b, 3: c})
        self.assertIsNone(ok, detail)
        self.assertEqual(wrong, {1})
        # another block wrong too: a verdict
        ok, detail, _, _ = cdc.held_content(recs, dcuts, UNPLACED_WRITTEN, {1: {1: cdc.ZERO, 2: c}, 2: {3: c}},
                                            {2: c, 3: c})
        self.assertIs(ok, False, detail)
        # block 1 set again by a write received after the write-zeroes:
        # cut 2 must hold it, and zeros there are a verdict
        recs, dcuts = unplaced_log(rewrite=True)
        ok, detail, _, _ = cdc.held_content(recs, dcuts, UNPLACED_WRITTEN, right, {2: b, 3: c})
        self.assertIs(ok, False, detail)
        self.assertIn('cut 2', detail)
        ok, detail, _, _ = cdc.held_content(recs, dcuts, UNPLACED_WRITTEN,
                                            {1: {1: cdc.ZERO, 2: b}, 2: {1: 'd' * 16, 3: c}},
                                            {1: 'd' * 16, 2: b, 3: c})
        self.assertIsNone(ok, detail)
        self.assertIn('cut 1', detail)
        # block 1 zeroed again by a write-zeroes received after the first,
        # with no write pending: cut 2 must hold zeros there (Codex on
        # #103, at a37b914)
        recs, dcuts = unplaced_log(rezero=True)
        ok, detail, _, _ = cdc.held_content(recs, dcuts, UNPLACED_WRITTEN,
                                            {1: {1: cdc.ZERO, 2: b}, 2: {1: 'a' * 16, 3: c}},
                                            {1: 'a' * 16, 2: b, 3: c})
        self.assertIs(ok, False, detail)
        self.assertIn('cut 2', detail)
        # zeros there: only cut 1's unplaced block is left
        ok, detail, _, wrong = cdc.held_content(recs, dcuts, UNPLACED_WRITTEN, {1: {1: cdc.ZERO, 2: b}, 2: {3: c}},
                                                {2: b, 3: c})
        self.assertIsNone(ok, detail)
        self.assertNotIn('cut 2', detail)
        self.assertEqual(wrong, {1})

    def test_write_zeroes_read_as_zeros(self):
        model = {}
        cdc.apply(model, {'flags': ['discard'], 'op': 'write_zeroes', 'offset': 4096, 'bytes': 8192 + 512})
        self.assertEqual(model, {1: cdc.ZERO, 2: cdc.ZERO, 3: None})

    def test_a_block_discarded_in_part_is_not_known(self):
        model = {1: 'a' * 16, 3: 'c' * 16}
        cdc.apply(model, {'flags': ['trim'], 'offset': 4096, 'bytes': 8192 + 512})
        self.assertEqual(model, {1: frozenset({'a' * 16, cdc.ZERO}), 2: frozenset({cdc.ZERO}), 3: None})

    def zeroes_behind(self, order):
        """A write of block 10 (a) applied; a write of block 30 (b)
        received; then ORDER's requests received, of `flush` (tag 3, the
        cut's), `flush2` (tag 5) and `zeroes` (block 10); then b applied,
        the barriers in the order received, and cut 1 at flush 3's. The
        review of #103's D1, from its reproducer -> (records, cuts)."""
        a, b = 'a' * 16, 'b' * 16
        out = [f'1\t{PID}\tstart\t0\t-\t-\t-\t64', f'2\t{PID}\tcuts\t-\t-\t-\t-\tbegun',
               f'3\t{PID}\trecv\t1\twrite\t80\t8\t-', f'4\t{PID}\tapplied\t1\twrite\t-\t-\t-\t{a}',
               f'5\t{PID}\trecv\t2\twrite\t240\t8\t-']
        ns = 6
        what = {'flush': ('3', 'flush', '18446744073709551615', '0'), 'flush2': ('5', 'flush', '0', '0'),
                'zeroes': ('4', 'write_zeroes', '80', '8'), 'discard': ('6', 'discard', '80', '8')}
        for x in order:
            tag, op, sec, n = what[x]
            out.append(f'{ns}\t{PID}\trecv\t{tag}\t{op}\t{sec}\t{n}\t-')
            ns += 1
        out.append(f'{ns}\t{PID}\tapplied\t2\twrite\t-\t-\t-\t{b}')
        for x in order:
            if x.startswith('flush'):
                tag = what[x][0]
                out.append(f'{ns + 1}\t{PID}\tbarrier\t{tag}\tflush\t-\t-\t-')
                if tag == '3':
                    out.append(f'{ns + 2}\t{PID}\tcut\t3\tflush\titself\t1\t5')
                ns += 2
        return cdc.records_of(cdc.parse_log('\n'.join(out) + '\n')[0]['lines'], [])

    @staticmethod
    def kinds(recs):
        return [r.get('op') if r.get('op') == 'write_zeroes' else 'trim' if 'trim' in r['flags']
                else f"barrier {r['tag']}" if 'flush' in r['flags'] else f"receipt {r['tag']}"
                if 'receipt' in r['flags'] else f"write {r['tag']}" for r in recs]

    def test_a_write_zeroes_received_while_a_flush_waits(self):
        """A write-zeroes received after a flush takes effect after its
        barrier (the store accepts in the order received): the cut holds
        the block as it was (the review of #103, D1)."""
        a, b = 'a' * 16, 'b' * 16
        written = [{'number': 1, 'group': 5, 'opens': 'itself', 'blocks': 2}]
        recs, dcuts = self.zeroes_behind(['flush', 'zeroes'])
        self.assertEqual(self.kinds(recs), ['write 1', 'receipt 3', 'write 2', 'barrier 3', 'write_zeroes'])
        ok, detail, _, wrong = cdc.held_content(recs, dcuts, written, {1: {10: a, 30: b}}, {10: a, 30: b})
        self.assertTrue(ok, detail)
        # The same write-zeroes received before the flush is in the cut.
        recs, dcuts = self.zeroes_behind(['zeroes', 'flush'])
        self.assertEqual(self.kinds(recs), ['write 1', 'write_zeroes', 'receipt 3', 'write 2', 'barrier 3'])
        ok, detail, _, wrong = cdc.held_content(recs, dcuts, written, {1: {10: a, 30: b}}, {10: a, 30: b})
        self.assertFalse(ok)
        self.assertEqual(wrong, {10})
        ok, detail, _, _ = cdc.held_content(recs, dcuts, written, {1: {10: cdc.ZERO, 30: b}}, {30: b})
        self.assertTrue(ok, detail)

    def test_behind_the_last_flush_received_before_it(self):
        recs, _ = self.zeroes_behind(['flush', 'zeroes', 'flush2'])
        self.assertEqual(self.kinds(recs), ['write 1', 'receipt 3', 'receipt 5', 'write 2', 'barrier 3',
                                            'write_zeroes', 'barrier 5'])
        recs, _ = self.zeroes_behind(['flush', 'flush2', 'zeroes', 'discard'])
        self.assertEqual(self.kinds(recs), ['write 1', 'receipt 3', 'receipt 5', 'write 2', 'barrier 3',
                                            'barrier 5', 'write_zeroes', 'trim'])

    def test_hashes_must_cover_the_write(self):
        text = (f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n2\t{PID}\trecv\t3\twrite\t16\t16\t-\n'
                f'3\t{PID}\tapplied\t3\twrite\t-\t-\t-\t{"a" * 16}\n')
        with self.assertRaises(RigError):
            cdc.records_of(cdc.parse_log(text)[0]['lines'], [])


class HeldLines(HeldFixture):
    """held_lines on a recording's directory written from the fixture: the
    holder's files, the status, what it was given."""

    def lines(self, name='life', status=None, final=None, log='', status_held=None, cuts_line='begun',
              recs=None, dcuts=None, written=None, blocks=None, image=None):
        import shutil
        import tempfile
        d = tempfile.mkdtemp(prefix='cdc-held-')
        try:
            holder_files(d, written or self.written, blocks or self.blocks, self.final if image is None else image,
                         log=log or '1\t> t1 held\n',
                         held_final=final if final is not None else '9 9 uploading itself 3\n',
                         status_held=status_held if status_held is not None else 'cuts_held 1\n')
            inst = {'lines': [{'event': 'cuts', 'result': cuts_line}]}
            got = []
            rc = cdc.held_lines(lambda k, v, t: got.append((k, v, t)), d, [inst], recs or self.recs,
                                dcuts or self.dcuts, status or {'cut_state': 'begun', 'cuts_abandoned': '0'},
                                512 << 20, name)
            return rc, {k: (v, t) for k, v, t in got}
        finally:
            shutil.rmtree(d)

    def test_a_right_daemon(self):
        rc, lines = self.lines()
        self.assertEqual(rc, (0, False), lines)
        for k in ('cut_state', 'cuts_held', 'held_content'):
            self.assertEqual(lines[k][0], 'PASS', (k, lines[k]))
        self.assertEqual(lines['cuts_abandoned'][0], 'INFO')

    def test_a_held_cut_the_daemon_could_not_read(self):
        """The holder's VERDICT (cut_hold.py: a refused read of a cut still
        being uploaded) is a FAIL line, not the rig's error, and nothing
        after it is judged (the holder ended there)."""
        rc, lines = self.lines(log='1\t> t1 held\n2\tVERDICT cut 9 is still being uploaded after its write was '
                               'refused (refused: read block 7 of cut 9: Input/output error (os error 5))\n')
        self.assertEqual(rc, (1, False))
        self.assertEqual(lines['held_reads'][0], 'FAIL')
        self.assertIn('read block 7 of cut 9', lines['held_reads'][1])
        self.assertNotIn('held_content', lines)
        self.assertEqual(self.lines()[1]['held_reads'][0], 'PASS')

    def test_the_state_not_begun(self):
        self.assertEqual(self.lines(cuts_line='taken_up')[1]['cut_state'][0], 'FAIL')
        rc, lines = self.lines(status={'cut_state': 'off', 'cuts_abandoned': '0'})
        self.assertEqual((rc[0], lines['cut_state'][0]), (1, 'FAIL'))

    def test_cuts_held_not_the_list(self):
        rc, lines = self.lines(status_held='cuts_held 2\n')
        self.assertEqual((rc[0], lines['cuts_held'][0]), (1, 'FAIL'))

    def test_bound_must_abandon(self):
        rc, lines = self.lines(name='bound')
        self.assertEqual((rc[0], lines['cuts_abandoned'][0]), (1, 'FAIL'))
        rc, lines = self.lines(name='bound', status={'cut_state': 'begun', 'cuts_abandoned': '2'})
        self.assertEqual((rc[0], lines['cuts_abandoned'][0]), (0, 'PASS'))

    def test_edges_must_send_write_zeroes_and_a_discard(self):
        with self.assertRaises(RigError):
            self.lines(name='edges')

    def test_the_holders_error(self):
        with self.assertRaises(RigError):
            self.lines(log='1\tERROR HolderError: no reply to `held` in 300 s\n')

    def test_a_refused_held_at_the_stop(self):
        with self.assertRaises(RigError):
            self.lines(final='refused: busy\n')

    def test_a_wrong_block_is_a_verdict(self):
        """The review of #103, S1: a failed held_content fails the check."""
        blocks, _ = self.wrong_in_mid()
        rc, lines = self.lines(blocks=blocks)
        self.assertEqual((rc, lines['held_content'][0]), ((1, False), 'FAIL'))

    def test_a_wrong_block_is_a_verdict_whatever_the_log_holds_after(self):
        """A write-zeroes that cannot be placed, after every cut, explains
        no wrong block of theirs (Codex on #103)."""
        blocks, bad = self.wrong_in_mid(7)
        recs = self.recs + [{'flags': ['discard'], 'op': 'write_zeroes', 'offset': bad[-1] * 4096, 'bytes': 4096,
                             'ambiguous': {bad[-1]}, 'seq': 10 ** 9, 'i': len(self.recs)}]
        rc, lines = self.lines(blocks=blocks, recs=recs)
        self.assertEqual((rc, lines['held_content'][0]), ((1, False), 'FAIL'))

    def test_a_wrong_block_under_an_unplaced_write_zeroes(self):
        """The rig's error, not a verdict, where every wrong block is
        under it (the store ran the write first, the model the
        write-zeroes)."""
        recs, dcuts = unplaced_log()
        rc, lines = self.lines(recs=recs, dcuts=dcuts, written=UNPLACED_WRITTEN,
                               blocks={1: {1: cdc.ZERO, 2: 'b' * 16}, 2: {3: 'c' * 16}},
                               image={2: 'b' * 16, 3: 'c' * 16})
        self.assertEqual((rc, lines['held_content'][0]), ((0, True), 'ERROR'))
        self.assertIn('blocks [1] (1 in all) were held wrong under a write-zeroes', lines['held_content'][1])

    def test_edges_must_send_both(self):
        """And hold a cut across its write-zeroes: a block of a cut after
        the first, zeroed after the cut's instant and before its write."""
        mid = self.written[len(self.written) // 2]
        b = sorted(self.blocks[mid['number']])[0]
        written = [dict(w, asked=10 ** 12) for w in self.written]
        zeroes = {'flags': ['discard'], 'op': 'write_zeroes', 'offset': b * 4096, 'bytes': 4096, 'ambiguous': set(),
                  'done_ns': 10 ** 12 - 1}
        trim = {'flags': ['trim'], 'offset': 10 ** 8, 'bytes': 4096}
        for extra in ([zeroes], [trim], [dict(zeroes, done_ns=10 ** 12), trim]):
            with self.assertRaises(RigError):
                self.lines(name='edges', recs=self.recs + extra, written=written)
        rc, lines = self.lines(name='edges', recs=self.recs + [zeroes, trim], written=written)
        self.assertEqual((rc, lines['edges'][0]), ((0, False), 'INFO'))
        self.assertIn('1 write-zeroes and 1 discards', lines['edges'][1])
        self.assertIn(f"cut {mid['number']} (1 blocks)", lines['edges'][1])

    def test_copied_blocks(self):
        """A block counts where a write after the cut's instant, applied
        before the holder asked for the cut's write, changed it; not where
        the write left what the cut holds (the review of #103's next look,
        R1)."""
        first, mid = self.written[0], self.written[len(self.written) // 2]
        held = self.blocks[mid['number']]

        def touched(r):
            return [b for b in range(r['offset'] // 4096, (r['offset'] + r['bytes'] - 1) // 4096 + 1)]

        # the first write after the cut's instant into a block of its map
        r = next(r for r in self.recs if 'ns' in r and r['i'] > self.dcuts[mid['number'] - 1]['prefix']
                 and any(b in held for b in touched(r)))
        hit = [b for b in touched(r) if b in held]
        self.assertTrue(hit)
        written = [dict(w, asked=r['ns'] + 1 if w is mid else 0) for w in self.written]
        got = dict((n, (c, z)) for n, c, z in cdc.copied(self.recs, self.dcuts, written, self.blocks))
        self.assertEqual(got[mid['number']], (len(hit), 0))
        self.assertEqual(got[first['number']], (0, 0))
        # asked for before that write: nothing
        early = [dict(w, asked=r['ns'] if w is mid else 0) for w in self.written]
        got = dict((n, (c, z)) for n, c, z in cdc.copied(self.recs, self.dcuts, early, self.blocks))
        self.assertEqual(got[mid['number']], (0, 0))
        # the cut holds what the write left in one of those blocks (its
        # last, so that its own hash is the one compared): that block
        # reads the same from the home
        blocks = {k: dict(m) for k, m in self.blocks.items()}
        b = hit[-1]
        self.assertGreater(touched(r).index(b), 0, 'a write of more than one block, the last of them in the map')
        blocks[mid['number']][b] = r['hashes'][touched(r).index(b)]
        got = dict((n, (c, z)) for n, c, z in cdc.copied(self.recs, self.dcuts, written, blocks))
        self.assertEqual(got[mid['number']], (len(hit) - 1, 0))
        # a log with no hashes: every block a write touched counts, as
        # far as is known
        bare = [{k: v for k, v in x.items() if k != 'hashes'} for x in self.recs]
        got = dict((n, (c, z)) for n, c, z in cdc.copied(bare, self.dcuts, written, blocks))
        self.assertEqual(got[mid['number']], (len(hit), 0))

    def test_copied_blocks_under_a_write_zeroes(self):
        """A block a write-zeroes answered before the write was asked for
        counts where the cut holds other than zeros."""
        a, c = 'a' * 16, 'c' * 16
        recs, dcuts = unplaced_log()
        written = [dict(w, asked=10 ** 6) for w in UNPLACED_WRITTEN]
        zeroes = {'flags': ['discard'], 'op': 'write_zeroes', 'offset': 0, 'bytes': 4 * 4096, 'ambiguous': set(),
                  'done_ns': 10 ** 6 - 1, 'seq': 99, 'i': len(recs)}
        got = cdc.copied(recs + [zeroes], dcuts, written, {1: {1: a, 2: cdc.ZERO}, 2: {3: c}})
        self.assertEqual([(n, x, z) for n, x, z in got], [(1, 1, 1), (2, 1, 1)])

    def test_a_control_missed_is_a_verdict(self):
        """Cuts 1 to 3 with no write after the first: the middle cut's
        image is the cut before's, and is what it was before its write,
        so neither control has anything to catch (the review of #103,
        S1)."""
        a = 'a' * 16
        text = (f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n'
                f'2\t{PID}\trecv\t3\twrite\t8\t8\t-\n3\t{PID}\tapplied\t3\twrite\t-\t-\t-\t{a}\n'
                + ''.join(f'{4 + 3 * k}\t{PID}\trecv\t1\tflush\t0\t0\t-\n'
                          f'{5 + 3 * k}\t{PID}\tbarrier\t1\tflush\t-\t-\t-\n'
                          f'{6 + 3 * k}\t{PID}\tcut\t1\tflush\titself\t{k + 1}\t{5 + k}\n' for k in range(3)))
        recs, dcuts = cdc.records_of(cdc.parse_log(text)[0]['lines'], [])
        written = [{'number': k + 1, 'group': 5 + k, 'opens': 'itself', 'blocks': 1} for k in range(3)]
        blocks = {k + 1: {1: a} for k in range(3)}
        ok, _, controls, _ = cdc.held_content(recs, dcuts, written, blocks, {1: a})
        self.assertTrue(ok)
        self.assertEqual({c: v for c, (v, _) in controls.items()}, {c: 'missed' for c in cdc.CONTENT_CONTROLS})
        rc, lines = self.lines(recs=recs, dcuts=dcuts, written=written, blocks=blocks, image={1: a})
        self.assertEqual(lines['held_content'][0], 'PASS')
        self.assertEqual(rc, (1, False))
        for c in cdc.CONTENT_CONTROLS:
            self.assertEqual(lines[f'ctl_{c}'][0], 'MISSED')
            self.assertIn('DIFFERS', lines[f'ctl_{c}'][1])


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
        self.assertEqual([(r['flags'], r['offset']) for r in recs], [(['discard'], 4096), (['trim'], 8192)])
        # A trim is for the held cuts' content alone: no write to cut_ref,
        # and nothing it zeroes.
        self.assertFalse(cut_ref.is_write(recs[1]))


    # A label line of eight sectors as the daemon writes it (some 8 KB),
    # cut by a kill -9 after 8012 bytes, the next instance's start line
    # following it with no newline: run 44183715455, takeover, request log
    # line 43866, with its times. There the line began 180 bytes into a
    # page of the file, so that the cut fell on the next page's start.
    LABEL = ('263932738825\t9937\tlabel\t25\twrite\t1047960\t8\t'
             + ('0cb1ba00000000008813000000000000' * 128) + ('0' * 4096))
    BEGIN = '263932000000\t9937\tstart\t0\t-\t-\t-\t64\n'
    RECV = '263932700000\t9937\trecv\t25\twrite\t1047960\t8\t-\n'
    START = '264075526204\t10976\tstart\t1\t-\t-\t-\t64'
    NEXT = '264075546749\t10976\tcuts\t-\t-\t-\t-\ttaken_up\n'

    @staticmethod
    def filler(n):
        """recv lines of the first instance, n bytes in all (n >= 40), between BEGIN and RECV."""
        out, i = [], 0
        while n:
            size = n if n <= 80 else min(44, n - 40)
            out.append(f'2639321{i:05d}\t9937\trecv\t{i}\tread\t' + '1' * (size - 34 - len(str(i))) + '\t8\t-\n')
            n, i = n - size, i + 1
        return ''.join(out)

    def torn(self, lost, start=None, after=None, off=0):
        """The log with the first instance's line `lost` cut short by a kill
        and the next instance's start line after it, where the cut falls
        on a page of the file (off bytes after it, where given) -> (text,
        the torn line's number)."""
        fill = (-(len(self.BEGIN) + len(self.RECV) + len(lost))) % cdc.PAGE
        head = self.BEGIN + self.filler(fill + (cdc.PAGE if fill < 40 else 0) + off) + self.RECV
        return (head + lost + (self.START if start is None else start) + '\n'
                + (self.NEXT if after is None else after)), head.count('\n') + 1

    def test_a_label_line_torn_by_a_kill(self):
        self.assertEqual(len(self.LABEL), 8235)    # 4096 bytes in hex after seven columns
        text, n = self.torn(self.LABEL[:8012])
        self.assertEqual((len(text.split('\n')[n - 1]) - len(self.START)) % cdc.PAGE, 8012 % cdc.PAGE)
        torn = []
        inst = cdc.parse_log(text, torn)
        self.assertEqual([i['pid'] for i in inst], ['9937', '10976'])
        self.assertEqual((inst[1]['ns'], inst[1]['line'], inst[1]['device']), (264075526204, n, '1'))
        self.assertEqual({ln['event'] for ln in inst[0]['lines']}, {'recv'})
        self.assertEqual(inst[0]['lines'][-1]['ns'], 263932700000)
        self.assertEqual([ln['event'] for ln in inst[1]['lines']], ['cuts'])
        self.assertEqual(torn, [{'line': n, 'pid': '9937', 'next': '10976', 'bytes': 8012,
                                 'head': '263932738825\t9937\tlabel\t25\twrite\t1047960\t8'}])
        # Where the caller takes no torn line, it is the rig's error.
        with self.assertRaisesRegex(RigError, f'line {n}: not eight to ten columns'):
            cdc.parse_log(text)

    def test_a_line_torn_anywhere_on_a_page(self):
        """What is left of the torn line may end in anything: a hex digit a
        to f, a tab, inside its pid, inside its time (the review of #113,
        T1 and T2)."""
        barrier = '263932738825\t9937\tbarrier\t26\tflush\t-\t-\t0'
        cases = {
            'a hex digit': (self.LABEL[:48], '263932738825\t9937\tlabel\t25\twrite\t1047960\t8\t0cb1b'),
            'the tab before the label': (self.LABEL[:43], '263932738825\t9937\tlabel\t25\twrite\t1047960\t8\t'),
            'a barrier line before its last column': (barrier[:-1], barrier[:-1]),
            'inside the pid': ('263932738825\t99', '263932738825\t99'),
            'after the time': ('263932738825\t', '263932738825\t'),
            'inside the time': ('2639', '2639'),
        }
        for why, (lost, head) in cases.items():
            with self.subTest(why):
                text, n = self.torn(lost)
                torn = []
                inst = cdc.parse_log(text, torn)
                self.assertEqual(torn, [{'line': n, 'pid': '9937', 'next': '10976', 'bytes': len(lost),
                                         'head': '\t'.join(head.split('\t')[:7])}])
                self.assertEqual((inst[1]['ns'], inst[1]['line']), (264075526204, n))
                self.assertEqual(inst[0]['lines'][-1]['ns'], 263932700000)
        # Off the page, a tear inside the time is a start line with a time
        # some digits too long, later than the line after it: the rig's error.
        for off in (1, -1):
            with self.subTest('inside the time, off the page', off=off):
                text, n = self.torn('2639', off=off)
                with self.assertRaisesRegex(RigError, f'line {n}: a start line later than the line after it'):
                    cdc.parse_log(text, [])
        text, n = self.torn('2639')
        with self.assertRaisesRegex(RigError, f'line {n}: a start line later than the line after it'):
            cdc.parse_log(text)

    def test_a_torn_line_only_before_a_take_over(self):
        torn = []
        text, n = self.torn(self.LABEL[:8012])
        cdc.parse_log(text, torn)
        self.assertEqual(cdc.torn_detail(torn, {'10976': True}),
                         f"1 lines of the request log cut short by a kill -9 and lost with it; line {n}: pid 9937's "
                         "`263932738825\t9937\tlabel\t25\twrite\t1047960\t8` line after 8012 bytes, then pid 10976's "
                         'start')
        self.assertTrue(cdc.torn_detail([], None).startswith('0 lines'))
        for kinds in ({'10976': False}, {}, None):
            with self.assertRaisesRegex(RigError, 'did not take the device over'):
                cdc.torn_detail(torn, kinds)

    def test_a_torn_line_not_followed_by_a_start(self):
        """Only the next instance's start may follow a torn line, on a page
        of the file and in time between the lines around it, and the torn
        line's daemon writes nothing after it."""
        cut = self.LABEL[:8012]
        bad = {
            'the same instance goes on': self.torn(cut, '264075526204\t9937\trecv\t26\twrite\t0\t8\t-'),
            'a start of a pid already started': self.torn(cut, self.START.replace('10976', '9937')),
            'the start before the last whole line': self.torn(cut, self.START.replace('264075526204',
                                                                                      '263932600000')),
            'the start after the line after it': self.torn(cut, self.START.replace('264075526204',
                                                                                   '264075546750')),
            'the torn line after the start': self.torn(cut.replace('263932738825', '264075526205', 1)),
            'the torn line before the last whole line': self.torn(cut.replace('263932738825', '263932600000', 1)),
            'the start at the end of the log': self.torn(cut, after=''),
            'the start not on a page': self.torn(cut, off=1),
            'the start a page late': self.torn(cut[:-1] + 'x', off=-1),
            'another pid torn': self.torn(cut.replace('\t9937\t', '\t9999\t', 1)),
            'a pid that is not the last instance\'s, cut': self.torn('263932738825\t98'),
            'a whole pid that begins the last instance\'s': self.torn('263932738825\t99\tlabel\t25\twrite\t1047960\t8\t'),
            'the torn line\'s daemon writes after it': self.torn(cut, after=self.NEXT
                                                                 + '264075546750\t9937\trecv\t26\twrite\t0\t8\t-\n'),
        }
        for why, (text, n) in bad.items():
            with self.subTest(why):
                with self.assertRaisesRegex(RigError, f'line {n}: |line {n + 2}: pid 9937 writes after'):
                    cdc.parse_log(text, [])
        # A tear that left eight columns and a newline parses as a line,
        # and its label is short of its sectors.
        text = self.BEGIN + self.RECV + cut + '\n' + self.START + '\n' + self.NEXT
        torn = []
        inst = cdc.parse_log(text, torn)
        self.assertEqual(torn, [])
        with self.assertRaisesRegex(RigError, 'label line'):
            cdc.records_of(inst[0]['lines'] + [{'ns': 263932738900, 'pid': '9937', 'event': 'applied', 'tag': '25',
                                               'op': 'write', 'sector': '-', 'sectors': '-', 'result': '-',
                                               'extra': [], 'line': 4}], [])


PID2 = '4343'


def split_log(text, k, lose_line=False):
    """The log of one instance as two: the daemon killed after its k-th cut's
    flush was received (the next instance is handed the flush again), or,
    with lose_line, after that flush's barrier and before its cut line was
    written. The next instance takes the state up and counts on."""
    lines = text.splitlines()
    ci = [i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'cut'][k - 1]
    assert [lines[ci - 2].split('\t')[2], lines[ci - 1].split('\t')[2]] == ['recv', 'barrier']
    first = lines[:ci] if lose_line else lines[:ci - 1]
    rest = lines[ci + 1:] if lose_line else lines[ci - 2:]
    ns = int(first[-1].split('\t')[0])

    def moved(ln, at=None):
        f = ln.split('\t')
        f[1] = PID2
        if at is not None:
            f[0] = str(at)
        return '\t'.join(f)
    second = [f'{ns + 1}\t{PID2}\tstart\t0\t-\t-\t-\t64', f'{ns + 2}\t{PID2}\tcuts\t-\t-\t-\t-\ttaken_up']
    second += [moved(rest[0], ns + 3)] + [moved(ln) for ln in rest[1:]]
    return '\n'.join(first + second) + '\n'


def split_begun(text, k):
    """The log of one instance as two, the second begun after the k-th
    cut's line as a start that acknowledged a failure does (stage 2c): no
    flush of the first under way, and the second's cut lines count from 1
    again. -> (text, the second's start ns)."""
    lines = text.splitlines()
    ci = [i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'cut'][k - 1]
    first, rest = lines[:ci + 1], lines[ci + 1:]
    ns = int(first[-1].split('\t')[0])

    def moved(ln, at=None):
        f = ln.split('\t')
        f[1] = PID2
        if at is not None:
            f[0] = str(at)
        if f[2] == 'cut':
            f[6] = str(int(f[6]) - k)
        return '\t'.join(f)
    second = [f'{ns + 1}\t{PID2}\tstart\t0\t-\t-\t-\t64', f'{ns + 2}\t{PID2}\tcuts\t-\t-\t-\t-\tbegun']
    second += [moved(rest[0], ns + 3)] + [moved(ln) for ln in rest[1:]]
    return '\n'.join(first + second) + '\n', ns + 1


class AcknowledgedFailure(unittest.TestCase):
    """A later start that acknowledged a failure begins a state (stage 2c):
    the fixture life's log split after a cut, the second instance begun."""

    def setUp(self):
        self.records, self.read, self.dev = fixture('life')
        self.cs, at = daemon_cuts_of(self.records, self.read, self.dev)
        text, self.marks = request_log(self.records, self.read, self.dev, at, hashes=True)
        # the first start's cuts line, as a daemon that holds cuts writes it
        start = f'\t{PID}\tstart\t0\t-\t-\t-\t64\n'
        self.text = text.replace(start, f'{start}1015\t{PID}\tcuts\t-\t-\t-\t-\tbegun\n', 1)
        self.k = len(self.cs) // 2

    def judge(self, text):
        inst = cdc.parse_log(text)
        recs, dcuts = cdc.records_of(cdc.log_lines(inst, {PID2: False}, {PID2}), cdc.read_marks(self.marks))
        starts = cdc.epochs(dcuts, recs)
        rread = cdc.read_of(recs, self.dev)
        rcs, _ = cut_ref.cuts(recs, cut_ref.scan(recs, rread, self.dev))
        return inst, recs, dcuts, starts, cdc.reference(rcs, cut_ref.opens(recs, rread, self.dev, rcs))

    def test_the_count_starts_again(self):
        text, _ = split_begun(self.text, self.k)
        inst, recs, dcuts, starts, ref = self.judge(text)
        self.assertEqual([r['begun'] for r in recs if 'instance' in r['flags']], [True])
        self.assertEqual(len(starts), 1)
        self.assertTrue(cdc.compare(dcuts, ref)[0])
        self.assertEqual([d['number'] for d in dcuts], [str(n + 1) for n in range(len(self.cs))])
        self.assertEqual([d['local'] for d in dcuts][self.k:self.k + 2], ['1', '2'])
        self.assertEqual(cdc.numbers_verdict(dcuts)[0], 'PASS')
        self.assertIn('state 2 counts from 1 again', cdc.numbers_verdict(dcuts)[1])
        # the status holds the new state's count
        last = ref[-1]
        st = {'cuts': str(len(self.cs) - self.k), 'cut_group': str(last['txg']),
              'cut_opens': 'itself' if last['itself'] else 'rewind'}
        self.assertTrue(cdc.status_verdict(st, ref, since=starts[0]['i'])[0])
        self.assertFalse(cdc.status_verdict(st, ref)[0])

    def test_a_count_that_goes_on_or_is_wrong(self):
        # pinned on #113: a new state counts from 1 again
        text, _ = split_begun(self.text, self.k)
        lines = text.splitlines()
        for i, ln in enumerate(lines):
            f = ln.split('\t')
            if f[1] == PID2 and f[2] == 'cut':
                f[6] = str(int(f[6]) + self.k)
                lines[i] = '\t'.join(f)
        dcuts = self.judge('\n'.join(lines) + '\n')[2]
        verdict, detail = cdc.numbers_verdict(dcuts)
        self.assertEqual(verdict, 'FAIL')
        self.assertIn(f"carries the number '{self.k + 1}', not 1", detail)
        lines = text.splitlines()
        for i, ln in enumerate(lines):
            f = ln.split('\t')
            if f[1] == PID2 and f[2] == 'cut':
                f[6] = str(int(f[6]) + 1)
                lines[i] = '\t'.join(f)
                break
        self.assertEqual(cdc.numbers_verdict(self.judge('\n'.join(lines) + '\n')[2])[0], 'FAIL')

    def test_held_cuts_keyed_by_state(self):
        text, ns = split_begun(self.text, self.k)
        _, _, dcuts, starts, _ = self.judge(text)
        written = [{'number': 1, 'group': 5, 'opens': 'itself', 'blocks': 2, 'asked': 1},
                   {'number': 1, 'group': 6, 'opens': 'itself', 'blocks': 1, 'asked': ns + 5}]
        h = '0123456789abcdef'
        blocks = f'1\t3\t{h}\n1\t4\t{h}\n1\t9\t{h}\n'
        out, b, stale = cdc.held_by_epoch(written, blocks, dcuts, starts)
        self.assertEqual(stale, [])
        self.assertEqual([w['number'] for w in out], [1, self.k + 1])
        self.assertEqual(b, {1: {3: h, 4: h}, self.k + 1: {9: h}})
        with self.assertRaisesRegex(RigError, 'does not hold the 1 blocks'):
            cdc.held_by_epoch(written, blocks.replace('1\t9', '2\t9'), dcuts, starts)
        with self.assertRaisesRegex(RigError, 'lines after the last cut'):
            cdc.held_by_epoch(written, blocks + f'1\t10\t{h}\n', dcuts, starts)
        written[1]['number'] = 99
        with self.assertRaisesRegex(RigError, 'cut 99 of state 2, which has no cut line'):
            cdc.held_by_epoch(written, blocks.replace('1\t9', '99\t9'), dcuts, starts)

    def test_the_second_instance_begins(self):
        import shutil
        import tempfile
        text, _ = split_begun(self.text, self.k)
        inst, recs, dcuts, _, _ = self.judge(text)
        begun = [True, True]
        good = 'cuts 0\ncut_state begun\nintent_state taken_up\nintent_regions 128\nintent_named_at_start 3\n'
        for second, want in ((good, 0), (good.replace('cut_state begun', 'cut_state taken_up'), 1),
                             (good.replace('intent_state taken_up', 'intent_state begun'), 1)):
            d = tempfile.mkdtemp(prefix='cdc-ack-')
            try:
                for k, st in enumerate((START_STATUS, second)):
                    with open(os.path.join(d, f'status-start-{k + 1}.txt'), 'w') as f:
                        f.write(st)
                with open(os.path.join(d, 'holder.log'), 'w') as f:
                    f.write(COMMITTED)
                got = []
                rc = cdc.instance_lines(lambda k, v, t: got.append((k, v, t)), d, inst, dcuts, recs, begun)
                self.assertEqual(rc, want, got)
                self.assertIn('acknowledging a failure', got[1][2])
            finally:
                shutil.rmtree(d)

    def test_kinds_with_the_acknowledgement(self):
        text = f'{PID}\tadded\n{PID2}\tadded\tacknowledged\n'
        self.assertEqual(cdc.read_kinds(text), {PID: False, PID2: False})
        self.assertEqual(cdc.read_acknowledged(text), {PID2})
        with self.assertRaises(RigError):
            cdc.read_kinds(f'{PID2}\tadded\tacked\n')


class FirstCommit(unittest.TestCase):
    def test_the_first_release_answered(self):
        self.assertEqual(cdc.first_commit(COMMITTED), 3)
        self.assertIsNone(cdc.first_commit('1\t> t1 held\n2\t< t1 ok (1 lines)\n3\t> t2 release 1\n'))
        self.assertIsNone(cdc.first_commit('1\t> t2 release 1\n2\t< t2 refused: not uploading\n'))
        self.assertEqual(cdc.first_commit('1\t> t2 release 1\n2\t< t2 refused: x\n3\t> t3 release 2\n'
                                          '4\tleft\n5\t< t3 ok (0 lines)\n'), 5)


class FailureLine(unittest.TestCase):
    def run_on(self, text):
        import shutil
        import tempfile
        d = tempfile.mkdtemp(prefix='cdc-fail-')
        try:
            with open(os.path.join(d, 'failure.tsv'), 'w') as f:
                f.write(text)
            got = []
            rc = cdc.failure_line(lambda k, v, t: got.append((k, v, t)), d)
            return rc, got
        finally:
            shutil.rmtree(d)

    def test_failure(self):
        rc, got = self.run_on('cuts_held\t2\nfailed_rc\t3\nrefused_rc\t4\n')
        self.assertEqual((rc, got[0][:2]), (0, ('failure', 'PASS')))
        rc, got = self.run_on('cuts_held\t2\nfailed_rc\t3\nrefused_rc\t0\n')
        self.assertEqual((rc, got[0][1]), (1, 'FAIL'))
        self.assertIn('ended with 0, wanted 4', got[0][2])
        rc, got = self.run_on('cuts_held\t2\nfailed_rc\t1\nrefused_rc\t4\n')
        self.assertEqual(rc, 1)
        for bad in ('cuts_held\t0\nfailed_rc\t3\nrefused_rc\t4\n', 'failed_rc\t3\nrefused_rc\t4\n', 'x\n'):
            with self.assertRaises(RigError):
                self.run_on(bad)


class TwoInstances(unittest.TestCase):
    """A request log of two instances of the daemon (stage 2b), from the
    fixture life's log split at a cut."""

    def setUp(self):
        self.records, self.read, self.dev = fixture('life')
        self.cs, at = daemon_cuts_of(self.records, self.read, self.dev)
        self.text, self.marks = request_log(self.records, self.read, self.dev, at, hashes=True)
        self.k = len(self.cs) // 2

    def judge(self, text, taken_over=True):
        inst = cdc.parse_log(text)
        lines = cdc.log_lines(inst, {PID2: taken_over})
        recs, dcuts = cdc.records_of(lines, cdc.read_marks(self.marks))
        rread = cdc.read_of(recs, self.dev)
        rcs, taken = cut_ref.cuts(recs, cut_ref.scan(recs, rread, self.dev))
        ref = cdc.reference(rcs, cut_ref.opens(recs, rread, self.dev, rcs))
        return inst, recs, dcuts, ref

    def test_a_take_over_between_a_flushs_receipt_and_its_barrier(self):
        inst, recs, dcuts, ref = self.judge(split_log(self.text, self.k))
        self.assertEqual(len(inst), 2)
        self.assertEqual([r['taken_over'] for r in recs if 'instance' in r['flags']], [True])
        ok, detail = cdc.compare(dcuts, ref)
        self.assertTrue(ok, detail)
        self.assertEqual(len(ref), len(self.cs))
        self.assertEqual(cdc.numbers_verdict(dcuts)[0], 'PASS')
        # The flush received before the kill is void: one receipt of its
        # tag carries the cut, the one in the second instance.
        b = next(r['i'] for r in recs if 'instance' in r['flags'])
        self.assertGreater(ref[self.k - 1]['prefix'], b)

    def test_the_rules_tell_a_kept_mark(self):
        inst, recs, dcuts, ref = self.judge(split_log(self.text, self.k))
        rread = cdc.read_of(recs, self.dev)
        ubs = cut_ref.scan(recs, rread, self.dev)
        dropped, _ = cut_ref.cuts(recs, ubs, 'mark-dropped')
        self.assertEqual(len(dropped), len(ref) - 1)
        kept, _ = cut_ref.cuts(recs, ubs, 'mark-kept')
        self.assertEqual(len(kept), len(ref))

    def test_a_created_device_drops_the_mark(self):
        """The same log, the second instance having created the device:
        the reference has no cut at the flush, and the daemon's line
        there is a verdict."""
        _, _, dcuts, ref = self.judge(split_log(self.text, self.k), taken_over=False)
        self.assertEqual(len(ref), len(self.cs) - 1)
        self.assertFalse(cdc.compare(dcuts, ref)[0])

    def test_a_cut_counted_and_its_line_lost_at_the_kill(self):
        _, recs, dcuts, ref = self.judge(split_log(self.text, self.k, lose_line=True))
        self.assertFalse(cdc.compare(dcuts, ref)[0])
        self.assertEqual(cdc.numbers_verdict(dcuts)[0], 'FAIL')
        self.assertIn(f'counting cut {self.k}', cdc.lost_at_kill(dcuts, ref, recs))
        # a daemon that skips a number with no cut lost is not excused
        _, recs, dcuts, ref = self.judge(split_log(self.text, self.k))
        self.assertIsNone(cdc.lost_at_kill(dcuts[:self.k - 1] + dcuts[self.k:], ref, recs))

    def test_what_the_first_instance_did_not_finish(self):
        """A write received and not applied, and a write-zeroes behind a
        flush with no barrier, when the instance ended: the kernel hands
        both to the next, which places them."""
        z = cdc.ZERO
        text = (f'1\t{PID}\tstart\t0\t-\t-\t-\t64\n2\t{PID}\tcuts\t-\t-\t-\t-\tbegun\n'
                f'3\t{PID}\trecv\t1\twrite\t8\t8\t-\n'
                f'4\t{PID}\trecv\t2\tflush\t0\t0\t-\n5\t{PID}\trecv\t3\twrite_zeroes\t16\t8\t-\n'
                f'6\t{PID2}\tstart\t0\t-\t-\t-\t64\n7\t{PID2}\tcuts\t-\t-\t-\t-\ttaken_up\n'
                f'8\t{PID2}\trecv\t1\twrite\t8\t8\t-\n9\t{PID2}\tapplied\t1\twrite\t-\t-\t-\t{z}\n'
                f'10\t{PID2}\trecv\t2\tflush\t0\t0\t-\n11\t{PID2}\trecv\t3\twrite_zeroes\t16\t8\t-\n'
                f'12\t{PID2}\tbarrier\t2\tflush\t-\t-\t-\n')
        recs, _ = cdc.records_of(cdc.log_lines(cdc.parse_log(text), {PID2: True}), [])
        kinds = [('instance' if 'instance' in r['flags'] else r['flags'][0] if r['flags'] else 'write') for r in recs]
        self.assertEqual(kinds, ['receipt', 'instance', 'write', 'receipt', 'flush', 'discard'])

    def test_the_log_must_say_which_took_over(self):
        text = split_log(self.text, self.k)
        with self.assertRaises(RigError):
            cdc.log_lines(cdc.parse_log(text), None)
        with self.assertRaises(RigError):
            cdc.log_lines(cdc.parse_log(text), {'1': True})
        with self.assertRaises(RigError):
            cdc.parse_log(text + f'99999999\t{PID2}\tstart\t0\t-\t-\t-\t64\n')
        with self.assertRaises(RigError):
            cdc.read_kinds('4343\ttaken over\n')
        self.assertEqual(cdc.read_kinds('4343\ttaken-over\n7\tadded\n'), {'4343': True, '7': False})

    def test_instance_lines(self):
        import shutil
        import tempfile
        inst, recs, dcuts, _ = self.judge(split_log(self.text, self.k))
        inst[0]['lines'].insert(0, {'event': 'cuts', 'result': 'begun'})
        second = (f'cuts {self.k - 1}\ncut_state taken_up\nintent_state taken_up\nintent_regions 128\n'
                  'intent_named_at_start 3\n')
        # With no release answered before the second start, the daemon
        # begins the intent map anew ("the intent map has no committed
        # cut"), and may: either is taken (the push run of 78b4dac).
        late = f'1\t> t1 release 1\n{inst[1]["ns"] + 1}\t< t1 ok (0 lines)\n'
        begun_map = second.replace('intent_state taken_up', 'intent_state begun')
        for starts, want, log in (((START_STATUS, second), 0, COMMITTED),
                                  ((START_STATUS, begun_map), 1, COMMITTED),
                                  ((START_STATUS, begun_map), 0, late),
                                  ((START_STATUS, second), 0, late),
                                  ((START_STATUS, second.replace(f'cuts {self.k - 1}', f'cuts {self.k - 2}')), 1,
                                   COMMITTED),
                                  ((START_STATUS.replace('cut_state begun', 'cut_state taken_up'), second), 1,
                                   COMMITTED)):
            d = tempfile.mkdtemp(prefix='cdc-inst-')
            try:
                for k, st in enumerate(starts):
                    with open(os.path.join(d, f'status-start-{k + 1}.txt'), 'w') as f:
                        f.write(st)
                with open(os.path.join(d, 'holder.log'), 'w') as f:
                    f.write(log)
                got = []
                rc = cdc.instance_lines(lambda k, v, t: got.append((k, v, t)), d, inst, dcuts, recs)
                self.assertEqual(rc, want, got)
                self.assertEqual([k for k, _, _ in got], ['instance_1', 'instance_2'])
                self.assertEqual('no cut committed before it' in got[1][2], log is late)
            finally:
                shutil.rmtree(d)


class NoFlush(unittest.TestCase):
    def test_groups_synced_with_no_flush_and_not_cut(self):
        recs = [{'i': 0, 'flags': ['mark'], 'mark': 'noflush-begin'}, {'i': 1, 'flags': []},
                {'i': 2, 'flags': []}, {'i': 3, 'flags': ['mark'], 'mark': 'noflush-end'}, {'i': 4, 'flags': []}]
        taken = [{'record': 1, 'txg': 7}, {'record': 2, 'txg': 8}, {'record': 4, 'txg': 9}]
        ks, left = cdc.noflush_kstats({1: [6, 7, 8, 9]}, recs, taken, [{'txg': 8}, {'txg': 9}])
        self.assertEqual((ks, left), ({1: [6, 8, 9]}, [7]))
        with self.assertRaises(RigError):
            cdc.noflush_kstats({}, recs[:2], taken, [])


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
