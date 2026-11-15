#!/usr/bin/env python3
"""#99 at 2c67c42: a copy of an uberblock applied while a flush carries its state.

    python3 test_receipt.py CHECKOUT

`cut_ref.cuts` takes an uberblock that repeats a carried mark as a repeat, as the daemon does
(cuts.rs, `marks`). With that clause removed, every test of the pull request passes. Here a
stray flush is received after the first copy of a group's uberblock was applied and reaches
its barrier after the second: it cuts the group at its own barrier, and the group is cut once.
"""
import os
import sys
import unittest

ROOT = sys.argv.pop(1)
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import cut_daemon_check as cdc  # noqa: E402
import cut_ref  # noqa: E402
import test_cut_daemon_check as t  # noqa: E402


class Receipt(unittest.TestCase):
    def test_a_copy_applied_while_a_flush_carries_its_state(self):
        records, read, dev = t.fixture('life')
        cs, _ = t.daemon_cuts_of(records, read, dev)
        first = cs[0]
        copies = sorted({u['record'] for u in cut_ref.scan(records, read, dev)
                         if (u['txg'], u['rootbp']) == (first['txg'], first['rootbp'])})
        self.assertGreaterEqual(len(copies), 2)
        text, marks = t.request_log(records, read, dev, {}, stray_before={copies[1]})
        recs, _ = cdc.records_of(cdc.parse_log(text)[0]['lines'], cdc.read_marks(marks))
        rread = cdc.read_of(recs, dev)
        rcs, _ = cut_ref.cuts(recs, cut_ref.scan(recs, rread, dev))
        stray = [r['i'] for r in recs if 'flush' in r['flags'] and r.get('tag') == '63']
        self.assertEqual(len(stray), 1)
        self.assertEqual(rcs[0]['flush_record'], stray[0], 'the stray flush carried the first copy\'s mark')
        self.assertEqual([c['txg'] for c in rcs], [c['txg'] for c in cs], 'each group is cut once')


if __name__ == '__main__':
    unittest.main()
