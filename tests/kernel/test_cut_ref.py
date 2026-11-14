#!/usr/bin/env python3
"""Unit tests for the reference for cuts (cut_ref.py). No kernel needed.

    python3 tests/kernel/test_cut_ref.py
"""
import gzip
import os
import struct
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cut_ref  # noqa: E402
import logwrites  # noqa: E402

SS = 512
DEV = (8 << 20) + 100 * 1024     # not a multiple of 256 KiB: the back labels move down
SLOT = 4096


def ub(txg, root, order='<', mmp=False, slot=SLOT, mmp_delay=0, later=0):
    """An uberblock slot as ZFS writes one: the struct, zeros, an embedded checksum at its end."""
    rootbp = [root * 1000 + k for k in range(16)]
    head = struct.pack(order + '5Q', cut_ref.UB_MAGIC, 5000, txg, 77, 1700000000 + txg + later)
    body = struct.pack(order + '16Q', *rootbp)
    tail = struct.pack(order + '6Q', 5000, cut_ref.MMP_MAGIC if mmp else 0, mmp_delay, 0, 0, 0)
    raw = head + body + tail
    return raw.ljust(slot - 40, b'\0') + struct.pack('<Q', 0x210da7ab10c7a11) + b'\x11' * 32


def slot_at(label, txg, slot=SLOT):
    lo = cut_ref.label_offsets(DEV)[label]
    return lo + cut_ref.RING_START + (txg % ((128 << 10) // slot)) * slot


def group(txg, root, stray=False, preflush_ub=False):
    """One transaction group as ZFS writes it: data, flush, the uberblock in four labels, flush."""
    out = [('write', (1 << 20) + txg * 16 * SS, bytes([txg % 251 + 1]) * (4 * SS)), ('flush',)]
    for k in range(4):
        out.append(('write', slot_at(k, txg), ub(txg, root)))
        if stray and k == 0:
            out.append(('flush',))
    out.append(('flush',))
    return out


class Fixture(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.log = os.path.join(self.dir.name, 'log')

    def run_log(self, entries, rule='design', preflush=(), fua=()):
        logwrites.write_log(self.log, SS, entries)
        if preflush or fua:
            # Set flags on these entries' headers: a write with
            # REQ_PREFLUSH, or with REQ_FUA.
            with open(self.log, 'r+b') as f:
                pos = SS
                for k, e in enumerate(entries):
                    flags = (logwrites.FLUSH if k in preflush else 0) | (logwrites.FUA if k in fua else 0)
                    if flags:
                        f.seek(pos + 16)
                        f.write(struct.pack('<Q', flags))
                    pos += SS + (len(e[2]) if e[0] == 'write' else 0)
        self.idx = logwrites.index(self.log, DEV)
        idx, read = cut_ref.load_from(self.idx, self.log)
        self.read = read
        ubs = cut_ref.scan(idx['records'], read, DEV)
        return cut_ref.cuts(idx['records'], ubs, rule)


class Labels(unittest.TestCase):
    def test_four_labels_with_the_back_two_in_the_aligned_size(self):
        self.assertEqual(cut_ref.label_offsets(DEV), [0, 256 << 10, (8 << 20) - (512 << 10), (8 << 20) - (256 << 10)])

    def test_a_device_with_no_room_is_refused(self):
        with self.assertRaises(ValueError):
            cut_ref.label_offsets(3 << 18)

    def test_both_byte_orders_parse(self):
        for order, name in (('<', 'le'), ('>', 'be')):
            u = cut_ref.parse_uberblock(ub(42, 3, order, mmp=True))
            self.assertEqual((u['order'], u['txg'], u['mmp'], u['rootbp'][0]), (name, 42, True, 3000))

    def test_mmp_delay_says_multihost(self):
        # Every uberblock of 2.x carries the MMP magic; a delay that is
        # not 0 is what multihost=on writes (the check imports such a
        # pool from a writable copy, for the claim).
        self.assertEqual(cut_ref.parse_uberblock(ub(42, 3, mmp=True))['mmp_delay'], 0)
        for order in ('<', '>'):
            self.assertEqual(cut_ref.parse_uberblock(ub(42, 3, order, mmp=True, mmp_delay=123456))['mmp_delay'], 123456)
        self.assertEqual(cut_ref.parse_uberblock(ub(42, 3, mmp_delay=99))['mmp_delay'], 0)

    def test_no_magic_no_uberblock(self):
        self.assertIsNone(cut_ref.parse_uberblock(bytes(4096)))
        self.assertIsNone(cut_ref.parse_uberblock(ub(1, 1)[:100]))

    def test_slots_of_1k_are_found_and_the_nvlist_area_is_not_read(self):
        labels = cut_ref.label_offsets(DEV)
        lo = labels[2]
        data = bytearray(64 << 10)
        data[0:len(ub(5, 1, slot=1024))] = ub(5, 1, slot=1024)          # at lo + 16 KiB: the nvlist area
        found = cut_ref.uberblocks_in(lo + (16 << 10), bytes(data), labels)
        self.assertEqual(found, [])
        ring = bytearray(8 << 10)
        ring[3072:3072 + 1024] = ub(7, 2, slot=1024)
        found = cut_ref.uberblocks_in(lo + cut_ref.RING_START, bytes(ring), labels)
        self.assertEqual([(k, o - lo, u['txg']) for k, o, u in found], [(2, cut_ref.RING_START + 3072, 7)])


class Cuts(Fixture):
    def test_a_cut_per_group_at_the_flush_after_the_uberblocks(self):
        entries = group(10, 1) + group(11, 2) + group(12, 3)
        cs, taken = self.run_log(entries)
        self.assertEqual([(c['n'], c['txg']) for c in cs], [(1, 10), (2, 11), (3, 12)])
        # The first group: write, flush, four uberblocks (records 2..5), flush (record 6).
        self.assertEqual((cs[0]['ub_record'], cs[0]['flush_record'], cs[0]['prefix']), (2, 6, 6))
        self.assertEqual([u['taken'] for u in taken[:4]], ['marks', 'repeat', 'repeat', 'repeat'])

    def test_the_flush_before_the_uberblocks_makes_no_cut(self):
        cs, _ = self.run_log(group(10, 1))
        self.assertEqual(len(cs), 1)
        self.assertEqual(cs[0]['flush_record'], 6)

    def test_group_zero_marks_nothing(self):
        entries = [('write', slot_at(k, t), ub(0, 0)) for k in range(4) for t in range(3)] + [('flush',)]
        cs, taken = self.run_log(entries)
        self.assertEqual(cs, [])
        self.assertTrue(all(u['taken'] == 'group 0' for u in taken))

    def test_a_stray_flush_between_the_uberblock_writes_is_the_cut(self):
        cs, taken = self.run_log(group(10, 1, stray=True) + group(11, 2))
        self.assertEqual([c['txg'] for c in cs], [10, 11])
        self.assertEqual((cs[0]['ub_record'], cs[0]['flush_record']), (2, 3))
        self.assertEqual([u['taken'] for u in taken[:4]], ['marks', 'repeat', 'repeat', 'repeat'])

    def test_a_flush_with_data_flushes_before_its_data(self):
        # The uberblock write carries REQ_PREFLUSH: that flush is before
        # it, so the cut is at the flush after.
        entries = [('write', 1 << 20, b'\1' * SS), ('write', slot_at(0, 9), ub(9, 1)), ('flush',)]
        cs, _ = self.run_log(entries, preflush={1})
        self.assertEqual([(c['txg'], c['flush_record'], c['prefix']) for c in cs], [(9, 2, 2)])

    def test_a_fua_write_is_a_write_and_then_a_flush(self):
        # An uberblock write with FUA: the daemon's pump adds a flush
        # after it, so the cut is at that barrier, with the write in it.
        entries = [('write', 1 << 20, b'\1' * SS), ('flush',), ('write', slot_at(0, 9), ub(9, 1)),
                   ('write', slot_at(1, 9), ub(9, 1)), ('flush',)]
        cs, _ = self.run_log(entries, fua={2})
        self.assertEqual([(c['txg'], c['flush_record'], c['prefix']) for c in cs], [(9, 2, 3)])
        # A data write with FUA after the uberblock is the flush too.
        entries = [('write', slot_at(0, 9), ub(9, 1)), ('write', 1 << 20, b'\1' * SS), ('flush',)]
        cs, _ = self.run_log(entries, fua={1})
        self.assertEqual([(c['flush_record'], c['prefix']) for c in cs], [(1, 2)])
        cs, _ = self.run_log(entries, fua={1}, rule='early')
        self.assertEqual([c['prefix'] for c in cs], [0])

    def test_mmp_writes_of_the_same_state_are_repeats(self):
        entries = group(10, 1) + [('write', slot_at(0, 31), ub(10, 1, mmp=True)), ('flush',)] + group(11, 2)
        cs, taken = self.run_log(entries)
        self.assertEqual([c['txg'] for c in cs], [10, 11])
        self.assertEqual([u['taken'] for u in taken if u['mmp']], ['repeat'])

    def test_a_stale_multihost_write_before_the_flush_is_a_repeat_of_the_newest_cut(self):
        # N-1's state, written by the multihost thread after N's
        # uberblocks and before N's flush: no cut, and the mark stays N.
        g11 = group(11, 2)
        entries = group(10, 1) + g11[:-1] + [('write', slot_at(0, 31), ub(10, 1, mmp=True))] + g11[-1:]
        cs, _ = self.run_log(entries)
        self.assertEqual([c['txg'] for c in cs], [10, 11])
        self.assertEqual(cut_ref.stale(cs), {})

    def test_the_previous_rule_moved_the_mark_back_on_it(self):
        g11 = group(11, 2)
        entries = (group(10, 1) + g11[:-1] + [('write', slot_at(0, 31), ub(10, 1, mmp=True))] + g11[-1:]
                   + [('write', slot_at(0, 31), ub(11, 2, mmp=True)), ('flush',)])
        cs, _ = self.run_log(entries, rule='previous')
        self.assertEqual([c['txg'] for c in cs], [10, 10, 11])
        cs, _ = self.run_log(entries)
        self.assertEqual([c['txg'] for c in cs], [10, 11])

    def test_a_stale_write_after_the_newest_cut_makes_a_stale_cut(self):
        # What the rule lets through: N-1 applied after N's flush.
        entries = group(10, 1) + group(11, 2) + [('write', slot_at(0, 31), ub(10, 1, mmp=True)), ('flush',)] + group(12, 3)
        cs, _ = self.run_log(entries)
        self.assertEqual([c['txg'] for c in cs], [10, 11, 10, 12])
        self.assertEqual(cut_ref.stale(cs), {3: 1})

    def test_the_newest_cut_repeated_later_is_a_repeat(self):
        entries = group(10, 1) + [('write', slot_at(0, 31), ub(10, 1, mmp=True)), ('flush',)] * 3 + group(11, 2)
        cs, _ = self.run_log(entries)
        self.assertEqual([c['txg'] for c in cs], [10, 11])

    def test_after_a_rewind_a_used_group_with_a_new_root_is_a_cut(self):
        entries = group(10, 1) + group(11, 2) + group(11, 3) + group(12, 4)
        cs, _ = self.run_log(entries)
        self.assertEqual([(c['txg'], c['rootbp'][0]) for c in cs], [(10, 1000), (11, 2000), (11, 3000), (12, 4000)])

    def test_after_a_rewind_a_cut_below_the_abandoned_groups_does_not_open_as_itself(self):
        # 10, 11, 12 written; a rewind to 10; the new history writes
        # 11 and 12 (new roots), then 13. Each group's uberblocks go to
        # slot txg % 32 in each label.
        entries = group(10, 1) + group(11, 2) + group(12, 3) + group(11, 5) + group(12, 6) + group(13, 7)
        cs, _ = self.run_log(entries)
        op = cut_ref.opens(self.idx['records'], self.read, DEV, cs)
        self.assertEqual([(c['txg'], op[c['n']]['itself'], op[c['n']]['txg']) for c in cs],
                         [(10, True, 10), (11, True, 11), (12, True, 12), (11, False, 12), (12, True, 12), (13, True, 13)])
        # The new 11 overwrote the old 11's slots but not the old 12's:
        # the new 11 opens at the abandoned 12. The new 12 overwrites
        # it and opens as itself. With 14 abandoned, two new groups do
        # not open as themselves.
        entries = group(10, 1) + group(14, 9) + group(11, 5) + group(12, 6) + group(15, 7)
        cs, _ = self.run_log(entries)
        op = cut_ref.opens(self.idx['records'], self.read, DEV, cs)
        self.assertEqual([(c['txg'], op[c['n']]['itself'], op[c['n']]['txg']) for c in cs],
                         [(10, True, 10), (14, True, 14), (11, False, 14), (12, False, 14), (15, True, 15)])

    def test_a_multihost_copy_of_the_cut_with_a_later_timestamp_is_the_cut_itself(self):
        # MMP writes the best group's uberblock again, to another slot,
        # with a later timestamp: the same state, and the newest.
        entries = group(10, 1) + [('write', slot_at(0, 20), ub(10, 1, mmp=True, later=7)), ('flush',)]
        cs, _ = self.run_log(entries)
        op = cut_ref.opens(self.idx['records'], self.read, DEV, cs + [dict(cs[0], n=2, prefix=len(entries))])
        self.assertEqual([c['txg'] for c in cs], [10])
        self.assertTrue(op[1]['itself'])
        self.assertEqual((op[2]['itself'], op[2]['timestamp']), (True, 1700000000 + 10 + 7))

    def test_another_state_of_the_cuts_group_later_or_tied_is_not_the_cut(self):
        # Another root under the cut's group with a later timestamp is
        # the best, and it is not the cut's state.
        for later in (5, 0):
            g = group(10, 1)
            # after the group's data and its flush, before its own
            # uberblock writes: a mark the cut's own replaces
            entries = g[:2] + [('write', slot_at(0, 20), ub(10, 2, later=later))] + g[2:]
            cs, _ = self.run_log(entries)
            op = cut_ref.opens(self.idx['records'], self.read, DEV, cs)
            self.assertEqual([(c['txg'], c['rootbp'][0]) for c in cs], [(10, 1000)])
            # a tie (later 0) is not the cut's either: ZFS's choice then
            # follows the order it reads the slots in
            self.assertFalse(op[1]['itself'], later)
            self.assertEqual(op[1]['rootbp'][0], 2000, later)

    def test_the_best_is_by_group_then_timestamp(self):
        # A higher group with an older timestamp beats a lower group's
        # newer one: the group first.
        entries = group(10, 1) + [('write', slot_at(0, 20), ub(9, 4, later=100))] + [('flush',)]
        cs, _ = self.run_log(entries)
        op = cut_ref.opens(self.idx['records'], self.read, DEV, cs + [dict(cs[0], n=2, prefix=len(entries))])
        self.assertTrue(op[2]['itself'])
        self.assertEqual(op[2]['txg'], 10)

    def test_a_discard_or_an_overwrite_clears_a_slot(self):
        entries = group(10, 1) + group(14, 9) + [('discard', slot_at(k, 14), SLOT) for k in range(4)] + group(11, 5)
        cs, _ = self.run_log(entries)
        op = cut_ref.opens(self.idx['records'], self.read, DEV, cs)
        self.assertEqual([op[c['n']]['itself'] for c in cs], [True, True, True])
        entries = group(10, 1) + group(14, 9) + [('write', slot_at(k, 14), bytes(SLOT)) for k in range(4)] + group(11, 5)
        cs, _ = self.run_log(entries)
        op = cut_ref.opens(self.idx['records'], self.read, DEV, cs)
        self.assertEqual([op[c['n']]['itself'] for c in cs], [True, True, True])

    def test_the_rule_keyed_on_the_group_alone_misses_it(self):
        entries = group(10, 1) + group(11, 2) + group(11, 3) + group(12, 4)
        cs, _ = self.run_log(entries, rule='txg-alone')
        self.assertEqual([c['txg'] for c in cs], [10, 11, 12])

    def test_counting_repeats_counts_a_group_twice(self):
        entries = group(10, 1, stray=True) + group(11, 2)
        cs, _ = self.run_log(entries, rule='count-repeats')
        self.assertEqual([c['txg'] for c in cs], [10, 10, 11])

    def test_early_takes_the_flush_before_the_uberblock_write(self):
        cs, _ = self.run_log(group(10, 1) + group(11, 2), rule='early')
        self.assertEqual([(c['txg'], c['prefix']) for c in cs], [(10, 1), (11, 8)])

    def test_unknown_rule(self):
        with self.assertRaises(ValueError):
            cut_ref.cuts([], [], 'nope')


class WriteFixture(Fixture):
    def test_the_fixture_holds_label_data_and_the_cuts(self):
        entries = group(10, 1) + [('mark', 'sync-1'), ('discard', 2 << 20, 8 * SS)] + group(11, 2)
        self.run_log(entries)
        out = os.path.join(self.dir.name, 'fx')
        cs, _ = cut_ref.write_fixture(out, self.idx, self.read, about='a test\nof two lines')
        with gzip.open(os.path.join(out, 'requests.tsv.gz'), 'rt') as f:
            lines = f.read().splitlines()
        self.assertIn('# skyzfs cut fixture, version 1', lines)
        self.assertIn(f'# dev_bytes {DEV}', lines)
        self.assertIn('# about of two lines', lines)
        rows = [ln.split('\t') for ln in lines if not ln.startswith('#')]
        self.assertEqual(len(rows), len(entries))
        self.assertEqual(rows[0][4], '-')                        # a data write away from the labels
        self.assertEqual(rows[1][1:], ['flush', '0', '0', '-'])
        self.assertEqual(bytes.fromhex(rows[2][4]), ub(10, 1))   # an uberblock write, whole
        self.assertEqual(rows[7][1:], ['mark', '0', '0', 'sync-1'])
        self.assertEqual(rows[8][1:], ['discard', str(2 << 20), str(8 * SS), '-'])
        with open(os.path.join(out, 'cuts.tsv')) as f:
            cut_rows = [ln.split('\t') for ln in f.read().splitlines() if not ln.startswith('#')]
        self.assertEqual([r[:5] for r in cut_rows], [['1', '10', '6', '2', '6'], ['2', '11', '15', '11', '15']])
        self.assertEqual(cut_rows[0][5], cut_ref.rootbp_hex(cs[0]['rootbp']))
        with open(os.path.join(out, 'uberblocks.tsv')) as f:
            ub_rows = [ln.split('\t') for ln in f.read().splitlines() if not ln.startswith('#')]
        self.assertEqual(len(ub_rows), 8)
        self.assertEqual(ub_rows[1][:7], ['3', '1', str(slot_at(1, 10)), 'le', '10', '0', 'repeat'])


class Fixtures(unittest.TestCase):
    """The recorded pools in tests/fixtures/pools: the reference, read back from the fixture's own
    requests, gives the fixture's uberblocks and cuts, and the check's files agree with them."""
    ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'fixtures', 'pools')

    def dirs(self):
        out = sorted(d for d in os.listdir(self.ROOT) if os.path.isdir(os.path.join(self.ROOT, d)))
        self.assertTrue(out)
        return out

    def test_every_fixture_gives_its_own_cuts(self):
        for name in self.dirs():
            d = os.path.join(self.ROOT, name)
            with self.subTest(name):
                records, read, dev = cut_ref.read_fixture(d)
                cs, taken = cut_ref.cuts(records, cut_ref.scan(records, read, dev))
                want = cut_ref.read_tsv(os.path.join(d, 'cuts.tsv'))
                got = [[str(c['n']), str(c['txg']), str(c['prefix']), str(c['ub_record']), str(c['flush_record']),
                        cut_ref.rootbp_hex(c['rootbp'])] for c in cs]
                self.assertEqual(got, want)
                ubs = cut_ref.read_tsv(os.path.join(d, 'uberblocks.tsv'))
                self.assertEqual([[str(u['record']), str(u['label']), str(u['slot']), u['order'], str(u['txg']),
                                   str(int(u['mmp'])), u['taken'], cut_ref.rootbp_hex(u['rootbp'])] for u in taken], ubs)
                # and the rings' best at each cut, which M2.1b's test reads as
                # whether the cut opens as itself
                op = cut_ref.opens(records, read, dev, cs)
                opens = cut_ref.read_tsv(os.path.join(d, 'opens.tsv'))
                self.assertEqual([[str(c['n']), str(op[c['n']]['txg']), str(int(op[c['n']]['itself']))] for c in cs],
                                 [[o[0], o[4], o[5]] for o in opens])

    def test_a_fixture_of_another_version_is_refused(self):
        d = os.path.join(self.ROOT, self.dirs()[0])
        with tempfile.TemporaryDirectory() as t:
            with gzip.open(os.path.join(d, 'requests.tsv.gz'), 'rt') as f:
                text = f.read()
            self.assertIn(f'# skyzfs cut fixture, version {cut_ref.FIXTURE_VERSION}\n', text)
            for head in (f'# skyzfs cut fixture, version {cut_ref.FIXTURE_VERSION + 1}\n', ''):
                with gzip.open(os.path.join(t, 'requests.tsv.gz'), 'wt') as f:
                    f.write(text.replace(f'# skyzfs cut fixture, version {cut_ref.FIXTURE_VERSION}\n', head))
                with self.assertRaises(ValueError):
                    cut_ref.read_fixture(t)

    def test_opens_names_every_cut_and_the_check_passed_them(self):
        for name in self.dirs():
            d = os.path.join(self.ROOT, name)
            with self.subTest(name):
                cuts = cut_ref.read_tsv(os.path.join(d, 'cuts.tsv'))
                opens = cut_ref.read_tsv(os.path.join(d, 'opens.tsv'))
                self.assertEqual([o[:3] for o in opens], [c[:3] for c in cuts])
                # every cut whole at its own group; where it opens as itself zdb chose it
                for n, txg, _, zdb_txg, ring_txg, itself, whole in opens:
                    self.assertEqual(whole, '1', f'cut {n}')
                    self.assertEqual(zdb_txg, ring_txg, f'cut {n}')
                    if itself == '1':
                        self.assertEqual(zdb_txg, txg, f'cut {n}')
                check = dict((ln[0], ln[1]) for ln in cut_ref.read_tsv(os.path.join(d, 'check.tsv')))
                self.assertEqual(check.get('cuts'), 'PASS')
                self.assertNotIn('FAIL', check.values())


if __name__ == '__main__':
    unittest.main()
