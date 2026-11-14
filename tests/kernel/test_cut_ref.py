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


def ub(txg, root, order='<', mmp=False, slot=SLOT):
    """An uberblock slot as ZFS writes one: the struct, zeros, an embedded checksum at its end."""
    rootbp = [root * 1000 + k for k in range(16)]
    head = struct.pack(order + '5Q', cut_ref.UB_MAGIC, 5000, txg, 77, 1700000000 + txg)
    body = struct.pack(order + '16Q', *rootbp)
    tail = struct.pack(order + '6Q', 5000, cut_ref.MMP_MAGIC if mmp else 0, 0, 0, 0, 0)
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

    def run_log(self, entries, rule='design', preflush=()):
        logwrites.write_log(self.log, SS, entries)
        if preflush:
            # Set the flush flag on these entries' headers: a write with REQ_PREFLUSH.
            with open(self.log, 'r+b') as f:
                pos, k = SS, 0
                for k, e in enumerate(entries):
                    if k in preflush:
                        f.seek(pos + 16)
                        f.write(struct.pack('<Q', logwrites.FLUSH))
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


if __name__ == '__main__':
    unittest.main()
