#!/usr/bin/env python3
"""Unit tests for the replay rig's log reader and its planning. No kernel needed.

    python3 tests/kernel/test_replay.py
"""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import logwrites  # noqa: E402
import replay_check  # noqa: E402

SS = 4096
DEV = 8 << 20
UB = b'\x0c\xb1\xba\x00\x00\x00\x00\x00'.ljust(SS, b'\1')


def blk(byte, n=1):
    return bytes([byte]) * (SS * n)


def txg(k):
    """One transaction group as ZFS writes it: data, flush, uberblocks in all four labels, flush."""
    labels = logwrites.label_offsets(DEV)
    out = [('write', (1 << 20) + k * 8 * SS, blk(0x10 + k, 2)),
           ('write', (2 << 20) + k * 8 * SS, blk(0x40 + k)),
           ('write', (3 << 20) + k * 8 * SS, blk(0x70 + k, 3)),
           ('flush',)]
    out += [('write', lo + (128 << 10) + (k % 32) * SS, UB) for lo in labels]
    out += [('flush',), ('mark', f'sync-{k}-{k * 3}')]
    return out


class Log(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.log = os.path.join(self.dir.name, 'log')
        self.img = os.path.join(self.dir.name, 'img')
        self.entries = [e for k in range(1, 7) for e in txg(k)]
        logwrites.write_log(self.log, SS, self.entries)
        with open(self.img, 'wb') as f:
            f.truncate(DEV)
        self.idx = logwrites.index(self.log, DEV)

    def tearDown(self):
        self.dir.cleanup()

    def model(self, order):
        """What the device holds after the entries in `order`, by a model that shares no code with advance()."""
        dev = bytearray(DEV)
        for i in order:
            e = self.entries[i]
            if e[0] == 'write':
                dev[e[1]:e[1] + len(e[2])] = e[2]
        return bytes(dev)

    def image(self):
        with open(self.img, 'rb') as f:
            return f.read()

    def test_index_describes_every_record(self):
        recs = self.idx['records']
        self.assertEqual(len(recs), len(self.entries))
        self.assertEqual(self.idx['sectorsize'], SS)
        for r, e in zip(recs, self.entries):
            if e[0] == 'write':
                self.assertEqual((r['offset'], r['bytes'], r['flags']), (e[1], len(e[2]), []))
            elif e[0] == 'flush':
                self.assertEqual((r['flags'], r['bytes']), (['flush'], 0))
                self.assertNotIn('pos', r)
            else:
                self.assertEqual((r['flags'], r['mark']), (['mark'], e[1]))

    def test_uberblock_and_label_writes_are_recognised(self):
        recs = self.idx['records']
        for r, e in zip(recs, self.entries):
            is_ub = e[0] == 'write' and e[2] == UB
            self.assertEqual(bool(r.get('ub')), is_ub, r)
            self.assertEqual(bool(r.get('label')), is_ub, r)
        self.assertEqual(sum(1 for r in recs if r.get('ub')), 6 * 4)

    def test_a_data_write_that_looks_like_an_uberblock_is_not_one(self):
        logwrites.write_log(self.log, SS, [('write', 1 << 20, UB), ('flush',)])
        self.assertFalse(logwrites.index(self.log, DEV)['records'][0].get('ub'))

    def test_every_prefix_matches_the_model(self):
        for n in range(len(self.entries) + 1):
            logwrites.advance(self.log, self.idx, self.img, max(0, n - 1), n)
            self.assertEqual(self.image(), self.model(range(n)), f'prefix {n}')

    def test_advance_in_steps_equals_advance_at_once(self):
        logwrites.advance(self.log, self.idx, self.img, 0, 17)
        logwrites.advance(self.log, self.idx, self.img, 17, len(self.entries))
        self.assertEqual(self.image(), self.model(range(len(self.entries))))

    def test_schedule_replaces_the_order(self):
        sched = [i for i in range(len(self.entries)) if i != 1]
        logwrites.advance(self.log, self.idx, self.img, 0, len(sched), sched)
        self.assertEqual(self.image(), self.model(sched))
        self.assertNotEqual(self.image(), self.model(range(len(self.entries))))

    def test_discard_is_skipped_and_counted(self):
        logwrites.write_log(self.log, SS, [('write', 1 << 20, blk(7)), ('discard', 1 << 20, SS), ('flush',)])
        idx = logwrites.index(self.log, DEV)
        self.assertEqual(idx['records'][1]['flags'], ['discard'])
        self.assertEqual(logwrites.advance(self.log, idx, self.img, 0, 3), (1, SS, 1))
        self.assertEqual(self.image()[1 << 20:(1 << 20) + SS], blk(7))

    def test_damaged_logs_are_refused(self):
        with open(self.log, 'r+b') as f:
            f.truncate(os.path.getsize(self.log) - SS * 4)
        with self.assertRaises(ValueError):
            logwrites.index(self.log, DEV)
        with open(self.log, 'r+b') as f:
            f.write(b'\0' * 8)
        with self.assertRaises(ValueError):
            logwrites.index(self.log, DEV)

    def test_range_outside_the_log_is_refused(self):
        with self.assertRaises(ValueError):
            logwrites.advance(self.log, self.idx, self.img, 0, len(self.entries) + 1)

    # ------------------------------------------------------- planning
    def test_marks_and_files_due(self):
        marks = replay_check.parse_marks(self.idx['records'])
        self.assertEqual([(k, c) for _, k, c in marks], [(k, k * 3) for k in range(1, 7)])
        first = marks[0][0]
        self.assertEqual(replay_check.files_at(marks, first - 1), 0)
        self.assertEqual(replay_check.files_at(marks, first), 3)
        self.assertEqual(replay_check.files_at(marks, len(self.entries)), 18)

    def test_plan_takes_every_flush_boundary_first(self):
        recs = self.idx['records']
        flush = set(replay_check.flush_points(recs))
        kind, deep, avail = replay_check.plan(recs, 20, 5, window=2)
        self.assertEqual(avail['flush'], 12)
        self.assertTrue(flush <= set(kind))
        self.assertTrue({0, len(recs)} <= set(kind))
        self.assertLessEqual(len(kind), 20)
        self.assertTrue(deep <= flush)
        self.assertEqual(len(deep), 5)

    def test_plan_with_room_covers_everything(self):
        recs = self.idx['records']
        kind, _, avail = replay_check.plan(recs, 10000, 100, window=2)
        self.assertEqual(set(kind), set(range(len(recs) + 1)))
        counts = {k: sum(1 for v in kind.values() if v == k) for k in ('flush', 'ub_window', 'random', 'end')}
        self.assertEqual(counts['flush'], avail['flush'])
        self.assertEqual(counts['ub_window'], avail['ub_window'])
        self.assertEqual(counts['random'], avail['other'])

    def test_windows_that_merge_are_still_counted_as_windows(self):
        # Uberblock writes closer together than the window: one window
        # over the whole log, too large to take whole.
        recs = self.idx['records']
        kind, _, avail = replay_check.plan(recs, 10000, 100, window=50)
        counts = {k: sum(1 for v in kind.values() if v == k) for k in ('ub_window', 'random')}
        self.assertEqual(avail['other'], 0)
        self.assertEqual(counts, {'ub_window': avail['ub_window'], 'random': 0})
        kind, _, _ = replay_check.plan(recs, 30, 3, window=50)
        self.assertGreater(sum(1 for v in kind.values() if v == 'ub_window'), 0)
        self.assertEqual(sum(1 for v in kind.values() if v == 'random'), 0)

    def test_victim_facts(self):
        a, b = blk(0xA1), blk(0xB2)
        logwrites.write_log(self.log, SS, [
            ('write', 1 << 20, b),              # 0
            ('write', 2 << 20, a),              # 1  the victim
            ('write', 3 << 20, blk(1) + a),     # 2  the same block inside a larger write
            ('flush',),
            ('write', 2 << 20, b),              # 4  overwrites the victim's place
            ('write', 1 << 20, b),              # 5  changes nothing
            ('write', 4 << 20, blk(0)),         # 6  zeros over an empty device
        ])
        recs = logwrites.index(self.log, DEV)['records']
        f = replay_check.victim_facts(self.log, recs, 1, self.img)
        self.assertEqual(f['copies'], [{'record': 2, 'offset': (3 << 20) + SS}])
        self.assertEqual((f['earlier'], f['later'], f['zeros'], f['same_as_before']), (None, 4, False, False))
        f = replay_check.victim_facts(self.log, recs, 5, self.img)
        self.assertEqual((f['earlier'], f['later'], f['same_as_before']), (0, None, True))
        self.assertEqual([c['record'] for c in f['copies']], [0, 4])
        f = replay_check.victim_facts(self.log, recs, 6, self.img)
        self.assertEqual((f['zeros'], f['same_as_before'], f['copies']), (True, True, []))
        self.assertIn('record 2 at byte', replay_check.facts_text(replay_check.victim_facts(self.log, recs, 1)))

    def test_plan_is_repeatable(self):
        recs = self.idx['records']
        self.assertEqual(replay_check.plan(recs, 25, 3, seed=7, window=2), replay_check.plan(recs, 25, 3, seed=7, window=2))

    def test_uberblock_windows(self):
        recs = self.idx['records']
        clusters = replay_check.ub_clusters(recs, window=1)
        self.assertEqual(len(clusters), 6)
        for lo, hi in clusters:
            # four uberblock writes, one prefix either side
            self.assertEqual(hi - lo, 5)

    def test_controls_are_wrong_only_where_they_should_be(self):
        recs = self.idx['records']
        marks = replay_check.parse_marks(recs)
        ctl = {c['name']: c for c in replay_check.controls(recs, marks)}
        self.assertEqual(set(ctl), {'drop', 'reorder'})
        total = len(recs)

        drop = ctl['drop']
        self.assertEqual(sorted(drop['schedule']), [i for i in range(total) if i != drop['victim']])
        self.assertIn('pos', recs[drop['victim']])
        self.assertFalse(recs[drop['victim']].get('label'))
        self.assertTrue(all(drop['victim'] <= n <= len(drop['schedule']) for n in drop['points']))

        re = ctl['reorder']
        self.assertEqual(sorted(re['schedule']), list(range(total)))
        m = len(marks) // 2
        mark_rec = marks[m][0] - 1
        pos = re['schedule'].index(re['victim'])
        # held back to just before the mark, after the group's uberblocks
        self.assertEqual(re['schedule'][pos + 1], mark_rec)
        ubs = [i for i in range(marks[m - 1][0], mark_rec) if recs[i].get('ub')]
        self.assertTrue(all(re['schedule'].index(u) < pos for u in ubs))
        self.assertLess(re['victim'], ubs[0])
        # the image differs from the true one exactly while the victim is held back
        wrong = []
        for n in range(total + 1):
            if self.model(re['schedule'][:n]) != self.model(range(n)):
                wrong.append(n)
        self.assertEqual(wrong, list(range(re['victim'] + 1, pos + 1)))
        self.assertTrue(set(wrong) & set(re['points']))
        # and some checked prefix has the new uberblocks without the victim
        self.assertTrue(any(all(u in re['schedule'][:n] for u in ubs) and re['victim'] not in re['schedule'][:n]
                            for n in re['points']))

    def test_no_controls_without_enough_marks(self):
        logwrites.write_log(self.log, SS, txg(1) + txg(2))
        idx = logwrites.index(self.log, DEV)
        self.assertEqual(replay_check.controls(idx['records'], replay_check.parse_marks(idx['records'])), [])

    def test_controls_are_judged_against_the_table_in_both_directions(self):
        drop = {'name': 'drop', 'caught': True, 'failed_by': {'deep': 2}}
        reorder = {'name': 'reorder', 'caught': False, 'failed_by': {}}
        rows, gaps, ok = replay_check.judge_controls([drop, reorder])
        self.assertTrue(ok)
        self.assertEqual([g['name'] for g in gaps], ['reorder'])
        # a control that is no longer caught
        self.assertFalse(replay_check.judge_controls([dict(drop, caught=False, failed_by={}), reorder])[2])
        # drop caught by the light check, with or without the deep check: the rig did better, not worse
        for by in ({'deep': 1, 'verify': 3}, {'verify': 3}, {'import': 1}):
            rows, _, ok = replay_check.judge_controls([dict(drop, failed_by=by), reorder])
            self.assertTrue(ok, by)
            self.assertIn(sorted(by)[-1], rows[0]['observed'])
        # the gap closing is also a difference: the table has to be changed on purpose
        rows, gaps, ok = replay_check.judge_controls([drop, dict(reorder, caught=True, failed_by={'deep': 1})])
        self.assertFalse(ok)
        self.assertEqual(gaps, [])
        # a control that did not run, and one the table does not know
        self.assertFalse(replay_check.judge_controls([drop])[2])
        self.assertFalse(replay_check.judge_controls([drop, reorder, dict(drop, name='new')])[2])

    def test_run_survives_a_command_that_prints_and_then_hangs(self):
        rc, out, t = replay_check.run(['sh', '-c', 'echo before the hang; exec sleep 30'], timeout=1)
        self.assertEqual(rc, 124)
        self.assertIn('before the hang', out)
        self.assertIn('TIMEOUT after 1s', out)
        self.assertLess(t, 10)
        rc, out, _ = replay_check.run(['sleep', '30'], timeout=1)       # nothing captured
        self.assertEqual((rc, out), (124, '\nTIMEOUT after 1s'))

    def test_run_reports_other_trouble_as_a_result(self):
        self.assertEqual(replay_check.run(['/nonexistent/skz-command'])[0], 127)
        rc, out, _ = replay_check.run(['printf', '\\377\\376 ok'])       # output that is not UTF-8
        self.assertEqual(rc, 0)
        self.assertIn('ok', out)
        self.assertEqual(replay_check.run(['sh', '-c', 'echo no; exit 3'])[:2], (3, 'no\n'))

    def test_scrub_verdict(self):
        clean = ('  pool: p\n state: ONLINE\n  scan: scrub repaired 0B in 00:00:01 with 0 errors on Tue\nconfig:\n\n'
                 '\tNAME  STATE  READ WRITE CKSUM\n\tp  ONLINE  0 0 0\n\t  loop1  ONLINE  0 0 0\n\nerrors: No known data errors\n')
        self.assertIsNone(replay_check.scrub_verdict(clean))
        self.assertIsNotNone(replay_check.scrub_verdict(clean.replace('loop1  ONLINE  0 0 0', 'loop1  ONLINE  0 0 2')))
        self.assertIsNotNone(replay_check.scrub_verdict(clean.replace('repaired 0B', 'repaired 8K')))
        self.assertIsNotNone(replay_check.scrub_verdict(clean.replace('with 0 errors', 'with 1 errors')))
        self.assertIsNotNone(replay_check.scrub_verdict(clean.replace('No known data errors', '1 data errors, use -v')))
        self.assertIsNotNone(replay_check.scrub_verdict(''))


if __name__ == '__main__':
    unittest.main()
