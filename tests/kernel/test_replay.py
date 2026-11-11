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

    def test_discard_is_zeros_or_nothing(self):
        ents = [('write', 1 << 20, blk(7, 4)), ('discard', (1 << 20) + SS, 2 * SS), ('flush',),
                ('write', (1 << 20) + SS, blk(9)), ('discard', 5 << 20, 1 << 20)]
        logwrites.write_log(self.log, SS, ents)
        idx = logwrites.index(self.log, DEV)
        self.assertEqual((idx['records'][1]['flags'], idx['records'][1]['offset'], idx['records'][1]['bytes']),
                         (['discard'], (1 << 20) + SS, 2 * SS))
        at = slice(1 << 20, (1 << 20) + 4 * SS)
        # as nothing: what was there stays
        self.assertEqual(logwrites.advance(self.log, idx, self.img, 0, 3, discard='skip'), (1, 4 * SS, 1))
        self.assertEqual(self.image()[at], blk(7, 4))
        # as zeros, the default: exactly the range, and a later write lands on top
        with open(self.img, 'r+b') as f:
            f.truncate(0)
            f.truncate(DEV)
        self.assertEqual(logwrites.advance(self.log, idx, self.img, 0, 3), (1, 4 * SS, 1))
        self.assertEqual(self.image()[at], blk(7) + blk(0, 2) + blk(7))
        logwrites.advance(self.log, idx, self.img, 3, 5)
        self.assertEqual(self.image()[at], blk(7) + blk(9) + blk(0) + blk(7))
        self.assertEqual(os.path.getsize(self.img), DEV)
        with self.assertRaises(ValueError):
            logwrites.advance(self.log, idx, self.img, 0, 1, discard='other')

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

    def test_fsync_marks(self):
        logwrites.write_log(self.log, SS, [('write', 1 << 20, blk(1)), ('flush',), ('mark', 'fsync-0'), ('mark', 'sync-1-0'),
                                           ('write', 2 << 20, blk(2)), ('flush',), ('mark', 'fsync-1'), ('mark', 'fsync-2'),
                                           ('mark', 'sync-2-3'), ('mark', 'fsync-x'), ('mark', 'other')])
        recs = logwrites.index(self.log, DEV)['records']
        fm = replay_check.fsync_marks(recs)
        self.assertEqual(fm, [(3, 1), (7, 2), (8, 3)])
        self.assertEqual([replay_check.fsynced_at(fm, n) for n in (0, 2, 3, 6, 7, 8, 11)], [0, 0, 1, 1, 2, 3, 3])
        # the sync marks are read as before, whatever stands between them
        self.assertEqual(replay_check.parse_marks(recs), [(4, 1, 0), (9, 2, 3)])

    def test_a_wrong_replay_is_asked_for_a_file_from_its_own_mark(self):
        ents = [('write', 1 << 20, blk(1)), ('write', 2 << 20, blk(2)), ('flush',), ('mark', 'fsync-0'),
                ('write', 3 << 20, blk(3)), ('flush',), ('mark', 'sync-1-1'), ('mark', 'fsync-1')]
        logwrites.write_log(self.log, SS, ents)
        recs = logwrites.index(self.log, DEV)['records']
        files, due = replay_check.promised(recs, True)
        self.assertEqual([due(n) for n in range(9)], [0, 0, 0, 0, 1, 1, 1, 1, 2])
        self.assertEqual([files(n) for n in range(9)], [0, 0, 0, 0, 0, 0, 0, 1, 1])
        # with record 1 dropped, every mark sits one place earlier, and the promises move with it
        sched = [0, 2, 3, 4, 5, 6, 7]
        files, due = replay_check.promised([recs[i] for i in sched], True)
        self.assertEqual([due(n) for n in range(8)], [0, 0, 0, 1, 1, 1, 1, 2])
        self.assertEqual([files(n) for n in range(8)], [0, 0, 0, 0, 0, 0, 1, 1])
        # without the fsync check nothing is asked of the writable import
        self.assertEqual(replay_check.promised(recs, False)[1](8), 0)

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
        self.assertFalse(f['intent_log'])
        zil = bytearray(blk(3))
        zil[144:152] = (0x210da7ab10c7a11).to_bytes(8, 'little')
        logwrites.write_log(self.log, SS, [('write', 1 << 20, bytes(zil)), ('flush',)])
        f = replay_check.victim_facts(self.log, logwrites.index(self.log, DEV)['records'], 0)
        self.assertTrue(f['intent_log'])
        self.assertIn('intent-log block', replay_check.facts_text(f))

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

    def test_controls_are_judged_against_the_table(self):
        drop = {'name': 'drop', 'caught': True, 'failed_by': {'deep': 2}}
        reorder = {'name': 'reorder', 'caught': True, 'failed_by': {'copies': 1}}
        data = {'name': 'data', 'caught': True, 'failed_by': {'verify': 30, 'copies': 2}}
        rows, gaps, ok = replay_check.judge_controls([drop, reorder, data], absent=('intent',))
        self.assertTrue(ok, rows)
        self.assertEqual(gaps, [])
        self.assertEqual(rows[2]['observed'], 'caught by the every-copy check')

        def judged(**changed):
            ctl = {c['name']: dict(c) for c in (drop, reorder, data)}
            for name, c in changed.items():
                ctl[name].update(c)
            return replay_check.judge_controls(list(ctl.values()), absent=('intent',))[2]
        # a control that is no longer caught
        for name in ('drop', 'reorder', 'data'):
            self.assertFalse(judged(**{name: {'caught': False, 'failed_by': {}}}), name)
        # drop may be caught by any check
        for by in ({'deep': 1, 'verify': 3}, {'verify': 3}, {'import': 1}, {'copies': 4}):
            self.assertTrue(judged(drop={'failed_by': by}), by)
        # reorder must be caught by the every-copy check, whatever else catches it
        self.assertTrue(judged(reorder={'failed_by': {'copies': 1, 'deep': 1}}))
        self.assertFalse(judged(reorder={'failed_by': {'deep': 1}}))
        # data must be caught by the files' checksums: that is what it is for
        self.assertFalse(judged(data={'failed_by': {'copies': 2}}))
        # a control that did not run, and one the table does not know
        self.assertFalse(replay_check.judge_controls([drop, reorder])[2])
        self.assertFalse(replay_check.judge_controls([drop, reorder, data, dict(drop, name='new')])[2])
        # a known gap is strict in both directions, and is stated while it holds
        table = {'reorder': {'block': {'by': None, 'gap': 'not seen yet'}}}
        rows, gaps, ok = replay_check.judge_controls([{'name': 'reorder', 'caught': False, 'failed_by': {}}], table)
        self.assertEqual((ok, gaps), (True, [{'name': 'reorder', 'gap': 'not seen yet'}]))
        rows, gaps, ok = replay_check.judge_controls([reorder], table)
        self.assertEqual((ok, gaps), (False, []))

    def test_the_table_is_keyed_by_what_the_rule_picked(self):
        reorder = {'name': 'reorder', 'caught': True, 'failed_by': {'copies': 1}}
        data = {'name': 'data', 'caught': True, 'failed_by': {'verify': 3}}

        def judged(**drop):
            c = dict({'name': 'drop', 'caught': True, 'failed_by': {'deep': 1}}, **drop)
            rows, _, ok = replay_check.judge_controls([c, reorder, data], absent=('intent',))
            return ok, rows[1]
        # when drop happens to pick an intent-log block it is not judged, either way, and the reason is given
        for c in ({'failed_by': {'fsync': 2}}, {'caught': False, 'failed_by': {}}):
            ok, row = judged(**dict(c, **{'class': 'intent-log block'}))
            self.assertTrue(ok)
            self.assertIn('the `intent` control', row['note'])
        self.assertFalse(judged(**{'class': 'intent-log block', 'rig_errors': [{'n': 1, 'failed': 'export'}]})[0])
        # the intent control is the one that is judged: by the fsync check, and it must run where it can be built
        intent = {'name': 'intent', 'class': 'intent-log block', 'caught': True, 'failed_by': {'fsync': 1}}
        base = [{'name': 'drop', 'caught': True, 'failed_by': {'deep': 1}}, reorder, data]
        self.assertTrue(replay_check.judge_controls(base + [intent])[2])
        self.assertFalse(replay_check.judge_controls(base + [dict(intent, failed_by={'deep': 1})])[2])
        self.assertFalse(replay_check.judge_controls(base + [dict(intent, caught=False, failed_by={})])[2])
        self.assertFalse(replay_check.judge_controls(base)[2])                          # not run where it should
        self.assertTrue(replay_check.judge_controls(base, absent=('intent',))[2])       # the layout has no such block
        # a superseded write harms nothing: expected not to be caught, strictly, and the reason is given
        ok, row = judged(**{'class': 'superseded', 'caught': False, 'failed_by': {}})
        self.assertTrue(ok)
        self.assertIn('written again', row['note'])
        self.assertFalse(judged(**{'class': 'superseded', 'failed_by': {'deep': 1}})[0])
        # a class the table does not name for that control fails the job
        ok, row = judged(**{'class': 'something new'})
        self.assertFalse(ok)
        self.assertIn('does not name', row['expected'])
        c = dict(data, **{'class': 'intent-log block'})
        self.assertFalse(replay_check.judge_controls([dict(reorder), c, {'name': 'drop', 'caught': True, 'failed_by': {'deep': 1}}],
                                                     absent=('intent',))[2])

    def test_the_intent_control_takes_a_block_the_fsync_depends_on(self):
        def zil(k):
            b = bytearray(blk(0x50 + k))
            b[144:152] = (0x210da7ab10c7a11).to_bytes(8, 'little')
            return bytes(b)
        ents = []
        for k in range(1, 7):
            # one intent-log write whose fsync mark comes only after the group's uberblocks, then one that fits
            ents += [('write', (5 << 20) + k * 16 * SS, zil(k)), ('flush',)]
            ents += [e for e in txg(k) if e[0] != 'mark']
            ents += [('mark', f'fsync-{2 * k - 2}'), ('write', (6 << 20) + k * 16 * SS, zil(k)), ('flush',),
                     ('mark', f'fsync-{2 * k - 1}'), ('mark', f'sync-{k}-{2 * k - 2}')]
        logwrites.write_log(self.log, SS, ents)
        recs = logwrites.index(self.log, DEV)['records']
        marks = replay_check.parse_marks(recs)
        m = len(marks) // 2
        lo, hi = marks[m - 1][0], marks[m][0] - 1
        found = replay_check.intent_log_writes(self.log, recs, lo, hi)
        self.assertEqual(len(found), 2)
        ctl = {c['name']: c for c in replay_check.controls(recs, marks, None,
                                                            lambda a, b: replay_check.intent_log_writes(self.log, recs, a, b))}
        c = ctl['intent']
        self.assertEqual(c['victim'], found[1])           # not the first: uberblocks stand between it and its mark
        mark = next(i for i in range(c['victim'] + 1, len(recs)) if recs[i].get('mark', '').startswith('fsync-'))
        self.assertFalse(any(recs[i].get('ub') for i in range(c['victim'], mark)))
        self.assertEqual(sorted(c['schedule']), [i for i in range(len(recs)) if i != c['victim']])
        # the prefix that ends with that mark, in the wrong replay's own numbering, is among the points
        self.assertIn(c['schedule'].index(mark) + 1, c['points'])
        self.assertEqual(replay_check.victim_class(recs, c['victim'], replay_check.victim_facts(self.log, recs, c['victim'])),
                         'intent-log block')
        # no intent-log writes, no such control
        self.assertNotIn('intent', {x['name'] for x in replay_check.controls(recs, marks, None, lambda a, b: [])})

    def test_victim_class(self):
        ents = [('write', 1 << 20, blk(1)),              # 0  rewritten whole before the next uberblock
                ('write', 2 << 20, blk(2, 2)),           # 1  rewritten only in part
                ('write', 3 << 20, blk(3)),              # 2  rewritten, but after the uberblock
                ('write', 4 << 20, blk(4)),              # 3  never written again
                ('write', 1 << 20, blk(5, 2)),           # 4
                ('write', 2 << 20, blk(6)),              # 5
                ('flush',), ('write', 128 << 10, UB), ('flush',),
                ('write', 3 << 20, blk(7))]
        zil = bytearray(blk(8))
        zil[144:152] = (0x210da7ab10c7a11).to_bytes(8, 'little')
        ents.append(('write', 5 << 20, bytes(zil)))      # 10
        logwrites.write_log(self.log, SS, ents)
        recs = logwrites.index(self.log, DEV)['records']
        got = [replay_check.victim_class(recs, i, replay_check.victim_facts(self.log, recs, i)) for i in (0, 1, 2, 3, 10)]
        self.assertEqual(got, ['superseded', 'block', 'block', 'block', 'intent-log block'])
        self.assertEqual(replay_check.victim_class(recs, 0, None), 'block')

    def test_group_points(self):
        recs = self.idx['records']
        groups = replay_check.group_points(recs)
        ubs = [i for i, r in enumerate(recs) if r.get('ub')]
        self.assertEqual(len(groups), 6)
        # first: right after a group's first uberblock write; last: just before the next group's
        self.assertEqual([f for f, _ in groups], [u + 1 for u in ubs[0::4]])
        self.assertEqual([la for _, la in groups], ubs[4::4] + [len(recs)])
        for f, la in groups:
            self.assertTrue(recs[f - 1].get('ub'))
            self.assertLess(f, la)
        # a flush or a mark between two uberblock writes does not start a new group
        logwrites.write_log(self.log, SS, [('write', 1 << 20, blk(1)), ('flush',), ('write', 128 << 10, UB), ('flush',),
                                           ('write', (128 << 10) + SS, UB), ('mark', 'sync-1-0'), ('write', 1 << 20, blk(2))])
        recs = logwrites.index(self.log, DEV)['records']
        self.assertEqual(replay_check.group_points(recs), [(3, 7)])
        self.assertEqual(replay_check.group_points(recs[:2]), [])

    def test_coverage_needs_every_group(self):
        recs = self.idx['records']
        groups = replay_check.group_points(recs)
        flush = [{'n': n, 't_deep': 0.1} for n in replay_check.flush_points(recs)]
        firsts = [{'n': f, 't_copies': 1.0} for f, _ in groups]
        lasts = [{'n': la, 't_copies': 1.0} for _, la in groups]
        cov = replay_check.coverage(recs, flush + firsts + lasts, groups=groups)
        self.assertEqual((cov['errors'], cov['warnings'], cov['groups'], cov['group_firsts'], cov['group_lasts']), ([], [], 6, 6, 6))
        # last points may go without, with a warning
        cov = replay_check.coverage(recs, flush + firsts + lasts[:2], groups=groups)
        self.assertEqual((cov['errors'], len(cov['warnings']), cov['group_lasts']), ([], 1, 2))
        # a first point may not; nor may it be checked without the every-copy check
        self.assertEqual(len(replay_check.coverage(recs, flush + firsts[1:] + lasts, groups=groups)['errors']), 1)
        cov = replay_check.coverage(recs, flush + [{'n': firsts[0]['n']}] + firsts[1:] + lasts, groups=groups)
        self.assertEqual(len(cov['errors']), 1)
        # a log in which no uberblock write is recognised cannot pass
        self.assertEqual(len(replay_check.coverage(recs, flush, groups=[])['errors']), 1)
        # every fsync mark must have had the fsync check
        marks = [{'n': 5, 'fsynced': 1, 't_deep': 0.1}, {'n': 9, 't_deep': 0.1}]
        cov = replay_check.coverage(recs, flush + marks, fsync_points={5})
        self.assertEqual((cov['errors'], cov['fsync_checked']), ([], 1))
        cov = replay_check.coverage(recs, flush + marks, fsync_points={5, 9, 11})
        self.assertEqual((len(cov['errors']), cov['fsync_checked'], cov['fsync_marks']), (1, 1, 3))

    def test_the_data_control_takes_a_files_block(self):
        # six groups; in group k, file k-1 is written: once to the intent log, then to its place
        ents = []
        for k in range(1, 7):
            name = replay_check.file_pattern(k - 1)
            ents += [('write', (5 << 20) + k * 16 * SS, name + blk(k)), ('flush',),
                     ('write', (1 << 20) + k * 16 * SS, blk(0x30 + k) + name + blk(k, 2))]
            ents += [e for e in txg(k) if e[0] != 'mark'] + [('mark', f'sync-{k}-{k}')]
        logwrites.write_log(self.log, SS, ents)
        recs = logwrites.index(self.log, DEV)['records']
        marks = replay_check.parse_marks(recs)
        self.assertEqual(len(replay_check.file_pattern(3)), 4096)
        self.assertEqual(replay_check.writes_carrying(self.log, recs, replay_check.file_pattern(0)), [0, 2])

        def find(n):
            return replay_check.writes_carrying(self.log, recs, replay_check.file_pattern(n))
        ctl = {c['name']: c for c in replay_check.controls(recs, marks, find)}
        self.assertEqual(set(ctl), {'drop', 'reorder', 'data'})
        m = len(marks) // 2
        wanted = marks[m][2] - 1
        d = ctl['data']
        # the victim is the last write before the middle mark that carries that file's first block
        self.assertEqual(d['victim'], find(wanted)[-1])
        self.assertLess(d['victim'], marks[m][0])
        self.assertEqual(sorted(d['schedule']), [i for i in range(len(recs)) if i != d['victim']])
        # and it is checked from the mark on, where the file is due
        self.assertTrue(any(n >= marks[m][0] for n in d['points']))
        self.assertTrue(all(d['victim'] <= n <= len(d['schedule']) for n in d['points']))
        # without a way to find a file's block there is no such control
        self.assertEqual({c['name'] for c in replay_check.controls(recs, marks)}, {'drop', 'reorder'})

    def fsync_log(self):
        """Six groups; in group k, file k-1 is written: first as an intent-log block, with its fsync mark, then in place."""
        import struct
        zil = b'\0' * 144 + struct.pack('<Q', replay_check.ZEC_MAGIC)
        ents = []
        for k in range(1, 7):
            name = replay_check.file_pattern(k - 1)
            ents += [('write', (5 << 20) + k * 16 * SS, (zil + name)[:SS].ljust(SS, b'\2') + name), ('flush',),
                     ('mark', f'fsync-{k - 1}'),
                     ('write', (1 << 20) + k * 16 * SS, blk(0x30 + k) + name + blk(k, 2))]
            ents += [e for e in txg(k) if e[0] != 'mark'] + [('mark', f'sync-{k}-{k}')]
        logwrites.write_log(self.log, SS, ents)

    def test_a_named_provider_gets_the_same_controls(self):
        # The index as the rig gets it from a provider command, against the one the default path makes.
        import json
        import subprocess
        self.fsync_log()
        out = os.path.join(self.dir.name, 'index.json')
        subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'logwrites.py'),
                        'index', '--log', self.log, '--dev-bytes', str(DEV), '--out', out], check=True)
        with open(out) as f:
            recs = json.load(f)['records']
        self.assertEqual(recs, logwrites.index(self.log, DEV)['records'])
        marks = replay_check.parse_marks(recs)
        for fsync_check, names in ((True, {'drop', 'reorder', 'data', 'intent'}), (False, {'drop', 'reorder', 'data'})):
            find, intent, absent = replay_check.control_sources(self.log, recs, fsync_check)
            ctl = replay_check.controls(recs, marks, find, intent)
            self.assertEqual({c['name'] for c in ctl}, names)
            self.assertEqual(set(absent), {'drop', 'reorder', 'data', 'intent'} - names)
            # every control of the table is either built or named as absent, with a reason
            self.assertEqual(names | set(absent), set(replay_check.EXPECTED))
            self.assertTrue(all(absent.values()))

    def test_a_provider_without_data_positions_says_which_controls_are_missing(self):
        self.fsync_log()
        recs = logwrites.index(self.log, DEV)['records']
        for r in recs:
            r.pop('pos', None)
        marks = replay_check.parse_marks(recs)
        find, intent, absent = replay_check.control_sources(self.log, recs, True)
        ctl = replay_check.controls(recs, marks, find, intent)
        self.assertEqual({c['name'] for c in ctl}, {'drop', 'reorder'})
        self.assertEqual(set(absent), {'data', 'intent'})
        self.assertIn('pos', absent['data'])
        # judged: the two that ran are of unknown kind and not judged; the two missing are named, not failed
        seen = [{'name': n, 'class': 'unknown', 'caught': False, 'failed_by': [], 'rig_errors': []} for n in ('drop', 'reorder')]
        rows, _, ok = replay_check.judge_controls(seen, absent=absent)
        self.assertTrue(ok)
        by = {r['name']: r for r in rows}
        self.assertEqual(by['data']['observed'], 'not built')
        self.assertIn('pos', by['intent']['not_built'])
        self.assertEqual(by['drop']['expected'], 'either: not judged')
        # a control that is neither built nor named as absent still fails the job
        rows, _, ok = replay_check.judge_controls(seen, absent={'intent': 'x'})
        self.assertFalse(ok)
        self.assertEqual({r['name']: r['observed'] for r in rows}['data'], 'not run')
        # and an unknown kind with a rig error is still an error
        seen[0]['rig_errors'] = [{'n': 3, 'failed': 'import'}]
        self.assertFalse(replay_check.judge_controls(seen, absent=absent)[2])

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

    # ------------------------------------------- what may count as a result
    def test_only_stages_that_judge_the_image_catch_a_control(self):
        def pt(n, failed=None):
            return {'n': n, 'ok': failed is None, **({'failed': failed, 'detail': 'd'} if failed else {})}
        by, rig, first = replay_check.tally_control(
            [pt(1), pt(2, 'verify'), pt(3, 'deep: scrub'), pt(4, 'deep: import (writable)'), pt(5, 'import'), pt(6, 'mount')])
        self.assertEqual((by, rig, first['n']), ({'verify': 1, 'deep': 2, 'import': 1, 'mount': 1}, [], 2))
        # the rig failing is not the image being judged
        for failed in ('losetup', 'export', 'deep: losetup', 'deep: dm-snapshot', 'deep: status',
                       replay_check.stage('import', 124), replay_check.stage('mount', 127),
                       'deep: ' + replay_check.stage('scrub', 124), 'deep: ' + replay_check.stage('import (writable)', 127)):
            by, rig, first = replay_check.tally_control([pt(1), pt(2, failed)])
            self.assertEqual((by, first, len(rig)), ({}, None, 1), failed)
            # and a control with a rig error is neither "caught" nor "as expected", whatever the table says
            for name in ('drop', 'reorder'):
                c = {'name': name, 'caught': False, 'failed_by': {}, 'rig_errors': rig}
                rows = replay_check.judge_controls([c], {name: replay_check.EXPECTED[name]})
                self.assertFalse(rows[2], (name, failed))
                self.assertIn('the rig failed', rows[0][0]['observed'])
            c = {'name': 'drop', 'caught': True, 'failed_by': {'deep': 1}, 'rig_errors': rig}
            self.assertFalse(replay_check.judge_controls([c], {'drop': replay_check.EXPECTED['drop']})[2])
        self.assertEqual(replay_check.stage('import', 1), 'import')
        self.assertNotIn(replay_check.stage('import', 124), replay_check.JUDGING)

    def test_coverage_has_a_floor(self):
        recs = self.idx['records']
        flush = replay_check.flush_points(recs)
        self.assertEqual(len(flush), 12)
        full = [{'n': n, 't_deep': 0.1} for n in flush] + [{'n': 0}, {'n': len(recs)}]
        cov = replay_check.coverage(recs, full)
        self.assertEqual((cov['errors'], cov['warnings'], cov['deep'], cov['deep_floor']), ([], [], 12, 6))
        # a flush boundary that was not checked fails the job
        cov = replay_check.coverage(recs, full[1:])
        self.assertEqual(cov['flush_unchecked'], [flush[0]])
        self.assertEqual(len(cov['errors']), 1)
        # the deep check at half of them: allowed, with a warning
        half = [{'n': n, **({'t_deep': 0.1} if i % 2 == 0 else {})} for i, n in enumerate(flush)]
        cov = replay_check.coverage(recs, half)
        self.assertEqual((cov['errors'], len(cov['warnings']), cov['deep']), ([], 1, 6))
        # below half: a failure
        fewer = [{'n': n, **({'t_deep': 0.1} if i % 3 == 0 else {})} for i, n in enumerate(flush)]
        cov = replay_check.coverage(recs, fewer)
        self.assertEqual((len(cov['errors']), cov['deep']), (1, 4))
        # a plan squeezed by a slow check drops flush boundaries, and that is caught here
        kind, deep, _ = replay_check.plan(recs, 6, 1, window=2)
        cov = replay_check.coverage(recs, [{'n': n, **({'t_deep': 1} if n in deep else {})} for n in kind])
        self.assertEqual(len(cov['errors']), 2)

    def test_an_absent_witness_does_not_pass_when_required(self):
        self.assertEqual(replay_check.witness_verdict([], True), (False, 'witness not run although it is required'))
        self.assertEqual(replay_check.witness_verdict([], False), (True, 'witness not run'))
        self.assertTrue(replay_check.witness_verdict([{'same': True}, {'same': True}], True)[0])
        self.assertFalse(replay_check.witness_verdict([{'same': True}, {'same': False}], False)[0])

    def test_marks_must_fit_the_manifest(self):
        marks = replay_check.parse_marks(self.idx['records'])          # promise up to 18 files
        self.assertIsNone(replay_check.manifest_trouble(marks, 18))
        self.assertIsNone(replay_check.manifest_trouble(marks, 40))
        self.assertIn('promises 18 files but the manifest lists 17', replay_check.manifest_trouble(marks, 17))
        self.assertIn('no sync marks', replay_check.manifest_trouble([], 5))
        self.assertIn('promises no files', replay_check.manifest_trouble([(3, 1, 0), (9, 2, 0)], 5))

    def test_a_pool_left_imported_is_a_rig_error_not_a_verdict(self):
        # A machine without a kernel: commands are answered by a model in
        # which the export after the deep check's scrub always fails.
        import argparse
        work = os.path.join(self.dir.name, 'work')
        os.makedirs(work)
        manifest = os.path.join(self.dir.name, 'manifest')
        open(manifest, 'w').close()
        rig = replay_check.Rig(argparse.Namespace(provider='', work=work, manifest=manifest, pool='p', mountpoint='/p'))
        with open(rig.image, 'wb') as f:
            f.truncate(DEV)
        state = {'imported': False, 'fail': {'export'}, 'calls': []}
        clean = ' scan: scrub repaired 0B in 00:00:01 with 0 errors on Tue\n\tp ONLINE 0 0 0\nerrors: No known data errors\n'

        def fake(cmd, timeout=300):
            state['calls'].append(cmd[:2])
            if cmd[:2] == ['zpool', 'list']:
                if 'list' in state['fail']:
                    return state['list_rc'], state.get('list_out', ''), 0.0
                return (0, 'p\n', 0.0) if state['imported'] else (1, "cannot open 'p': no such pool\n", 0.0)
            if cmd[:2] == ['zpool', 'import']:
                if state['imported']:
                    return 1, "cannot import 'p': a pool with that name already exists", 0.0
                state['imported'] = True
                return 0, '', 0.0
            if cmd[:2] == ['zpool', 'export']:
                if 'export' in state['fail']:
                    return 1, 'pool is busy', 0.0
                state['imported'] = False
                return 0, '', 0.0
            if cmd[:2] == ['zpool', 'status']:
                return 0, clean, 0.0
            if cmd[0] == 'losetup' and '--find' in cmd:
                return 0, '/dev/loop9\n', 0.0
            if cmd[:2] == ['dmsetup', 'remove'] and 'dmremove' in state['fail']:
                return 1, 'device busy', 0.0
            if cmd[:2] == ['losetup', '-d'] and 'detach' in state['fail']:
                return 1, 'busy', 0.0
            return 0, '', 0.0

        real, real_sleep = replay_check.run, replay_check.time.sleep
        replay_check.run, replay_check.time.sleep = fake, lambda s: None
        try:
            # the deep check itself passes, the export does not: a rig error for that point
            failed, _, _ = rig.deep()
            self.assertEqual(failed, 'export')
            self.assertNotIn('deep: ' + failed, replay_check.JUDGING)
            # the pool is still there; no later check may read that as a verdict, and none tries to import
            for _ in range(3):
                n = len(state['calls'])
                r = rig.point(0, 0, True)
                self.assertFalse(r['ok'])
                self.assertNotIn(r['failed'], replay_check.JUDGING)
                self.assertIn('still imported', r['failed'])
                self.assertNotIn(['zpool', 'import'], state['calls'][n:])
            self.assertIn('still imported', rig.deep()[0])
            # in a control pass that is a rig error, never "caught"
            by, rig_errors, first = replay_check.tally_control([rig.point(0, 0, True) for _ in range(2)])
            self.assertEqual((by, first, len(rig_errors)), ({}, None, 2))
            # other cleanup that fails is reported too, and never as a judging stage
            state.update(imported=False, fail={'dmremove'})
            self.assertEqual(rig.deep()[0], 'dm-snapshot remove')
            state.update(imported=False, fail={'detach'})
            self.assertEqual(rig.deep()[0], 'losetup -d')
            self.assertEqual(rig.light(0)[0], 'losetup -d')
            for st in ('export', 'dm-snapshot remove', 'losetup -d'):
                self.assertFalse({st, 'deep: ' + st} & replay_check.JUDGING)
            # only "no such pool" means there is none: any other answer from zpool list is a rig error
            for rc, out in ((1, 'cannot open: I/O error'), (2, 'usage'), (124, 'TIMEOUT'), (127, 'zpool: not found'),
                            (1, '')):
                state.update(imported=False, fail={'list'}, list_rc=rc, list_out=out)
                n = len(state['calls'])
                for failed in (rig.light(0)[0], rig.deep()[0]):
                    self.assertTrue(failed.startswith('rig: '), (rc, out, failed))
                self.assertNotIn(['zpool', 'import'], state['calls'][n:])
            # the fsync promise: after the scrub, a file whose fsync returned must be in the mounted pool
            state.update(imported=False, fail=set())
            rig.manifest = [['0' * 64, 'f.0']]
            r = rig.point(0, 0, True, False, 1)
            self.assertEqual((r['ok'], r['failed']), (False, 'fsync: files'))
            self.assertIn(r['failed'], replay_check.JUDGING)
            self.assertIn('f.0', r['detail'])
            self.assertFalse(state['imported'])
            # at an fsync mark the same promise is judged without the scrub
            n = len(state['calls'])
            r = rig.point(0, 0, 'fsync', False, 1)
            self.assertEqual((r['failed'], 't_fsync' in r, 't_deep' in r), ('fsync: files', True, False))
            self.assertNotIn(['zpool', 'scrub'], state['calls'][n:])
            self.assertIn(['zpool', 'import'], state['calls'][n:])
            n = len(state['calls'])
            rig.point(0, 0, True, False, 1)
            self.assertIn(['zpool', 'scrub'], state['calls'][n:])
            rig.manifest = []
            # and with nothing failing, both checks pass
            state.update(imported=False, fail=set())
            self.assertTrue(rig.point(0, 0, True)['ok'])
            self.assertFalse(state['imported'])
        finally:
            replay_check.run, replay_check.time.sleep = real, real_sleep

    # ------------------------------------------------ the every-copy check
    A, B = 0x100000, 0x300000        # DVA offsets; on the device they sit 4 MiB further on
    BLOCKS = (                       # (object, kind, checksum name, copies' offsets, bytes of the block on disk, content)
        ('objset 0 object 0 level -1', 'L0 DMU objset', 'fletcher4', (A, B, B + 0x1000), 0x1000, 0xAA),
        ('objset 54 object 28 level 0', 'L0 ZFS plain file', 'fletcher4', (A + 0x2000,), 0x2000, 0xCC),
        ('objset 0 object 5 level 0', 'L0 SPA space map', 'fletcher4', (A + 0x4000, B + 0x4000), 0x600, 0xBB),
        ('objset 54 object 1 level 0', 'L0 ZIL intent log', 'zilog2', (A + 0x8000,), 0x1000, 0xDD),
    )

    @staticmethod
    def content(byte, size):
        return bytes((byte + i * 7) & 0xff for i in range(size))

    def zdb_text(self, extra='', bp_count=5, ganged=0, blocks=None):
        out = ['Traversing all blocks to verify nothing leaked ...', '']
        for obj, kind, ckname, offs, psize, byte in (blocks or self.BLOCKS):
            ck = ':'.join(f'{x:016x}' for x in replay_check.fletcher4(self.content(byte, psize)))
            dvas = ' '.join(f'DVA[{k}]=<0:{o:x}:{(psize + 0xfff) & ~0xfff:x}>' for k, o in enumerate(offs))
            out.append(f'{obj} offset 0x0 {dvas} [{kind}] {ckname} uncompressed unencrypted LE contiguous unique '
                       f"{('single', 'double', 'triple')[len(offs) - 1]} size={psize:x}L/{psize:x}P birth=9L/9P fill=1 cksum={ck}")
        out += ['objset 0 object 34 level 0 offset 0x0 EMBEDDED [L0 DSL props] et=0 lz4 size=200L/4dP birth=4L',
                'objset 0 object 0 level 0 offset 0x20000 HOLE [L0 DMU dnode] size=4000L birth=56L']
        return ('\n'.join(out) + '\n' + extra
                + f'\n\tNo leaks (block sum matches space maps exactly)\n\n\tbp count:   {bp_count}\n\tganged count:    {ganged}\n')

    def good_image(self):
        with open(self.img, 'r+b') as f:
            for _, _, _, offs, psize, byte in self.BLOCKS:
                for o in offs:
                    f.seek(replay_check.DVA_OFFSET + o)
                    f.write(self.content(byte, psize))

    def test_fletcher4(self):
        def plain(data):
            a = b = c = d = 0
            for i in range(0, len(data), 4):
                a = (a + int.from_bytes(data[i:i + 4], 'little')) & replay_check.M64
                b = (b + a) & replay_check.M64
                c = (c + b) & replay_check.M64
                d = (d + c) & replay_check.M64
            return a, b, c, d
        self.assertEqual(replay_check.fletcher4(b'\x01\0\0\0\x02\0\0\0'), (3, 4, 5, 6))
        for size in (4, 512, 4096, 131072):
            data = bytes((i * 131 + size) & 0xff for i in range(size))
            self.assertEqual(replay_check.fletcher4(data), plain(data), size)
        ones = b'\xff' * 131072               # sums that wrap around 64 bits
        self.assertEqual(replay_check.fletcher4(ones), plain(ones))
        # with and without numpy, the same sums
        saved, replay_check.np = replay_check.np, None
        try:
            self.assertEqual(replay_check.fletcher4(ones), plain(ones))
        finally:
            replay_check.np = saved

    def test_every_copy_passes_a_good_image(self):
        self.good_image()
        b = replay_check.zdb_blocks(self.zdb_text())
        self.assertEqual((len(b['bps']), b['embedded'], b['holes'], b['bp_count'], b['ganged_count'], b['unread']),
                         (4, 1, 1, 5, 0, []))
        self.assertEqual([(len(x['dvas']), x['type'], x['level'], x['checksum']) for x in b['bps']],
                         [(3, 'DMU objset', 0, 'fletcher4'), (1, 'ZFS plain file', 0, 'fletcher4'),
                          (2, 'SPA space map', 0, 'fletcher4'), (1, 'ZIL intent log', 0, 'zilog2')])
        failed, detail, counts = replay_check.every_copy_verdict(0, self.zdb_text(), self.img)
        self.assertIsNone(failed, detail)
        self.assertEqual((counts['blocks'], counts['copies'], counts['with_copies'], counts['intent_log']), (3, 6, 2, 1))

    def test_every_copy_sees_each_bad_copy_by_itself(self):
        off = replay_check.DVA_OFFSET
        # each copy of a block with three, the single copy of a file's block, the last byte of a short block
        for at in (self.A, self.B, self.B + 0x1000, self.A + 0x2000 + 0x1fff, self.B + 0x4000 + 0x5ff):
            self.good_image()
            with open(self.img, 'r+b') as f:
                f.seek(off + at)
                f.write(b'\x01')
            failed, detail, _ = replay_check.every_copy_verdict(0, self.zdb_text(), self.img)
            self.assertEqual(failed, 'copies: differ', hex(at))
            self.assertIn(failed, replay_check.JUDGING)
            self.assertIn('1 of 6 copies', detail)
        # a copy that is stale in the same way in all three places is still caught: each is checked against the checksum
        self.good_image()
        with open(self.img, 'r+b') as f:
            for at in (self.A, self.B, self.B + 0x1000):
                f.seek(off + at)
                f.write(b'\x01')
        self.assertIn('3 of 6 copies', replay_check.every_copy_verdict(0, self.zdb_text(), self.img)[1])
        # bytes after the end of a short block are not the block; an intent-log block is not judged
        self.good_image()
        with open(self.img, 'r+b') as f:
            for at in (self.B + 0x4000 + 0x600, self.A + 0x8000):
                f.seek(off + at)
                f.write(b'\x01')
        self.assertIsNone(replay_check.every_copy_verdict(0, self.zdb_text(), self.img)[0])
        # an intent-log block on the log device (vdev 1) is not judged either, and is not a rig error
        on_log = ('objset 54 object 0 level -2 offset 0x1 DVA[0]=<1:0:1000> [L0 ZIL intent log] zilog2 uncompressed unencrypted '
                  'LE contiguous unique single size=1000L/1000P birth=28L/28P fill=0 cksum=1:2:3:4\n')
        failed, detail, counts = replay_check.every_copy_verdict(0, self.zdb_text(on_log, 6), self.img)
        self.assertIsNone(failed, detail)
        self.assertEqual(counts['intent_log'], 2)

    def test_every_copy_takes_zdbs_exit_code_as_a_verdict(self):
        self.good_image()
        text = self.zdb_text('zdb_blkptr_cb: Got error 52 reading <54, 28, 0, 0>  -- skipping\n')
        failed, detail, _ = replay_check.every_copy_verdict(3, text, self.img)
        self.assertEqual(failed, 'copies: zdb')
        self.assertIn('error 52', detail)

    def test_every_copy_never_passes_what_it_could_not_check(self):
        self.good_image()
        line = 'objset 9 object 9 level 0 offset 0x0 {} [L0 ZFS plain file] {} uncompressed unencrypted {} {} unique single size=1000L/1000P birth=9L/9P fill=1 cksum=1:2:3:4\n'
        cases = {
            'timed out': (124, self.zdb_text()),
            'not started': (127, ''),
            'died': (-6, "libzpool: PANIC: Pool 'p' has encountered an uncorrectable I/O failure"),
            'list too short': (0, self.zdb_text(bp_count=6)),
            'list too long': (0, self.zdb_text(bp_count=4)),
            'no counts': (0, self.zdb_text().split('\tbp count')[0]),
            'a line it cannot read': (0, self.zdb_text('objset 0 object 7 level 0 offset 0x0 something new\n')),
            'no checksum on the line': (0, self.zdb_text(line.format('DVA[0]=<0:500000:1000>', 'fletcher4', 'LE', 'contiguous').split(' cksum')[0] + '\n', 6)),
            'copies out of order': (0, self.zdb_text(line.format('DVA[1]=<0:500000:1000>', 'fletcher4', 'LE', 'contiguous'), 6)),
            'ganged': (0, self.zdb_text(line.format('DVA[0]=<0:500000:1000>', 'fletcher4', 'LE', 'gang'), 6, ganged=1)),
            'ganged by count only': (0, self.zdb_text(ganged=2)),
            'another device': (0, self.zdb_text(line.format('DVA[0]=<1:500000:1000>', 'fletcher4', 'LE', 'contiguous'), 6)),
            'another checksum': (0, self.zdb_text(line.format('DVA[0]=<0:500000:1000>', 'sha256', 'LE', 'contiguous'), 6)),
            'another byte order': (0, self.zdb_text(line.format('DVA[0]=<0:500000:1000>', 'fletcher4', 'BE', 'contiguous'), 6)),
            'empty output': (0, ''),
        }
        for name, (rc, text) in cases.items():
            failed, _, _ = replay_check.every_copy_verdict(rc, text, self.img)
            self.assertIsNotNone(failed, name)
            self.assertTrue(failed.startswith('rig: '), (name, failed))
            self.assertNotIn(failed, replay_check.JUDGING, name)

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
