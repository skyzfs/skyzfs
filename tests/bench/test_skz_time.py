#!/usr/bin/env python3
"""Tests of skz_time.py: the differences of two status readings and their tables."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import skz_time as st  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
STATUS_RS = os.path.join(HERE, '..', '..', 'crates', 'skyzfs-ublk', 'src', 'status.rs')


def status_keys():
    """The time_* keys as the daemon's source lists them."""
    keys, inside = [], False
    with open(STATUS_RS) as f:
        lines = f.readlines()
    for line in lines:
        if line.strip().startswith('"time_'):
            keys.append(line.strip().strip(',').strip('"'))
            inside = True
        elif inside and line.strip().startswith('];'):
            break
    return keys


def reading(scale, pid='100'):
    """A reading in which every sum is scale * (its index + 1), and the identities hold.

    As of a daemon of status version 6: a read waits only for what the
    reader did, so a read's wait by the worker's other activities is 0.
    """
    r = {'pid': pid, 'state': 'serving'}
    for i, key in enumerate(status_keys()):
        if 'longest_wait_cause' in key and not key.endswith('_ns'):
            r[key] = 'step_apply'
        else:
            r[key] = str(scale * (i + 1))
    for a in st.ACTIVITIES:
        if a not in st.READER_ACTIVITIES:
            r[f'time_read_wait_by_{a}_ns'] = '0'
    r['time_worker_read_ns'] = '0'
    for kind in st.KINDS:
        acts = st.READER_ACTIVITIES if kind == 'read' else st.ACTIVITIES
        r[f'time_{kind}_wait_ns'] = str(sum(int(r[f'time_{kind}_wait_by_{a}_ns']) for a in acts))
    return r


def fifth(r):
    """The same reading as a daemon of status version 5 would give it: no key of version 6."""
    old = {k: v for k, v in r.items() if k not in VERSION_6_KEYS}
    return old


VERSION_6_KEYS = {'time_worker_step_barrier_ns'} | {f'time_{k}_wait_by_step_barrier_ns' for k in st.KINDS} | {
    f'time_barrier_{b}_{f}' for b in st.BARRIERS for f in ('count', 'ns', 'max_ns')} | {
    f'time_reader_{a}_ns' for a in st.READER_ACTIVITIES} | {
    f'time_read_unflushed_{f}' for f in ('count', 'extra_bytes', 'mismatched')}


class Delta(unittest.TestCase):
    def test_the_module_knows_the_daemons_keys(self):
        keys = status_keys()
        self.assertEqual(len(keys), 171)
        # what version 6 added, all of it known to the module
        self.assertEqual(len(VERSION_6_KEYS), 171 - 153)
        for key in VERSION_6_KEYS:
            self.assertIn(key, keys)
        for a in st.READER_ACTIVITIES:
            self.assertIn(a, st.ACTIVITIES)
        for kind in st.KINDS:
            for field in ('count', 'wait_ns', 'wait_max_ns', 'call_ns', 'call_max_ns', 'longest_wait_cause', 'longest_wait_cause_ns'):
                self.assertIn(f'time_{kind}_{field}', keys)
            for a in st.ACTIVITIES:
                self.assertIn(f'time_{kind}_wait_by_{a}_ns', keys)
        for a in st.ACTIVITIES:
            self.assertIn(f'time_worker_{a}_ns', keys)
        for p in st.STEP_PARTS:
            self.assertIn(f'time_step_{p}_ns', keys)
        # every key is either a difference or an end value, and the end
        # values are the longest ones and their causes
        ends = [k for k in keys if st.is_end_value(k)]
        self.assertEqual(len(ends), 5 * 2 + 7 + 1 + 4 + 2 + 5 * 2 + len(st.BARRIERS))

    def test_every_key_is_kept(self):
        b, a = reading(1), reading(3)
        t = st.delta(b, a, 1000, 5000)
        self.assertTrue(t['ok'], t['why'])
        self.assertEqual(len(t['delta']) + len(t['end']), 171)
        self.assertEqual(len(t['before']), 171)
        self.assertEqual(len(t['after']), 171)
        self.assertEqual(t['window_ns'], 4000)
        keys = status_keys()
        i = keys.index('time_read_call_ns')
        self.assertEqual(t['delta']['time_read_call_ns'], 2 * (i + 1))
        # a longest one is the end reading's, not a difference
        j = keys.index('time_read_wait_max_ns')
        self.assertEqual(t['end']['time_read_wait_max_ns'], 3 * (j + 1))
        self.assertNotIn('time_read_wait_max_ns', t['delta'])
        self.assertEqual(t['end']['time_read_longest_wait_cause'], 'step_apply')
        self.assertIsInstance(t['end']['time_read_longest_wait_cause_ns'], int)

    def test_a_reading_without_the_keys_says_so_and_does_not_raise(self):
        old = {'pid': '100', 'state': 'serving', 'reads': '5'}
        for b, a, word in ((old, reading(1), 'before the window'), (reading(1), old, 'after it'), ({}, {}, 'before the window and after it')):
            t = st.delta(b, a)
            self.assertFalse(t['ok'])
            self.assertIn(word, t['why'])
            self.assertIn('version 5', t['why'])
            self.assertEqual(t['delta'], {})

    def test_another_daemon_or_a_sum_that_went_down_is_doubtful(self):
        t = st.delta(reading(1, pid='100'), reading(3, pid='101'))
        self.assertFalse(t['ok'])
        self.assertIn('pid changed', t['why'])
        t = st.delta(reading(3), reading(1))
        self.assertFalse(t['ok'])
        self.assertIn('went down', t['why'])
        a = reading(3)
        a['time_read_count'] = 'x'
        t = st.delta(reading(1), a)
        self.assertFalse(t['ok'])
        self.assertIn('time_read_count is not a number', t['why'])
        a = reading(3)
        del a['time_gate_ns']
        t = st.delta(reading(1), a)
        self.assertFalse(t['ok'])
        self.assertIn('in one reading only', t['why'])


class Tables(unittest.TestCase):
    def d(self, **over):
        d = {k: 0 for k in status_keys() if not st.is_end_value(k)}
        d.update(over)
        return d

    def test_a_reads_life(self):
        d = self.d(time_read_count=1000, time_read_wait_ns=50_000_000, time_read_call_ns=120_000_000,
                   time_pump_submit_count=4000, time_pump_submit_ns=8_000_000,
                   time_pump_commit_read_count=1000, time_pump_commit_read_ns=30_000_000, time_pump_taken_read_ns=20_000_000)
        stages, n = st.life(d, 'read')
        self.assertEqual(n, 1000)
        self.assertEqual([round(v, 3) for _, v in stages], [2.0, 50.0, 120.0, 20.0, 10.0])
        # a write's life takes the pump's times of everything but reads
        d.update(time_write_count=500, time_write_wait_ns=5_000_000, time_write_call_ns=10_000_000,
                 time_pump_commit_other_count=600, time_pump_commit_other_ns=6_000_000, time_pump_taken_other_ns=3_000_000)
        stages, n = st.life(d, 'write')
        self.assertEqual([round(v, 3) for _, v in stages], [2.0, 10.0, 20.0, 5.0, 5.0])
        # nothing of a kind in the window: no means, and no division
        stages, n = st.life(self.d(), 'flush')
        self.assertEqual([v for _, v in stages], [None] * 5)

    def test_the_wait_by_cause_is_sorted_and_sums(self):
        d = self.d(time_write_wait_ns=100, time_write_wait_by_idle_ns=10, time_write_wait_by_step_apply_ns=60,
                   time_write_wait_by_flush_ns=30)
        rows, total, wait = st.wait_by(d, 'write')
        self.assertEqual([a for a, v in rows[:3]], ['step_apply', 'flush', 'idle'])
        self.assertEqual((total, wait), (100, 100))
        self.assertEqual(len(rows), len(st.ACTIVITIES))

    def test_a_reads_wait_is_the_readers_and_an_older_daemons_the_workers(self):
        d = self.d(time_read_wait_ns=100, time_read_wait_by_idle_ns=10, time_read_wait_by_read_ns=70,
                   time_read_wait_by_other_ns=20)
        rows, total, wait = st.wait_by(d, 'read')
        self.assertEqual([a for a, _ in rows], ['read', 'other', 'idle'])
        self.assertEqual((total, wait), (100, 100))
        # a daemon of version 5: a read waited for the worker's fourteen
        old = {k: v for k, v in d.items() if k not in VERSION_6_KEYS}
        old.update(time_read_wait_by_step_apply_ns=40, time_read_wait_ns=140)
        rows, total, wait = st.wait_by(old, 'read')
        self.assertEqual(len(rows), len(st.ACTIVITIES) - 1)
        self.assertEqual((rows[0], total, wait), (('read', 70), 140, 140))

    def test_the_groups_hold_every_activity_once(self):
        acts = [a for _, group in st.GROUPS for a in group]
        self.assertEqual(sorted(acts), sorted(st.ACTIVITIES))
        d = self.d(time_write_wait_by_idle_ns=10, time_write_wait_by_step_apply_ns=60, time_write_wait_by_own_sync_ns=5,
                   time_write_wait_by_step_barrier_ns=4, time_write_wait_by_flush_ns=30,
                   time_write_wait_by_discard_ns=2, time_write_wait_by_read_ns=1)
        self.assertEqual([v for _, v in st.wait_groups(d, 'write')], [10, 65, 4, 30, 2, 1, 0])
        # a read of a version 6 daemon: the reader's three causes only
        d = self.d(time_read_wait_by_idle_ns=10, time_read_wait_by_read_ns=7, time_read_wait_by_other_ns=3,
                   time_read_wait_by_step_apply_ns=99)
        self.assertEqual([v for _, v in st.wait_groups(d, 'read')], [10, 0, 0, 0, 0, 7, 3])

    def test_the_identities(self):
        b, a = reading(1), reading(3)
        keys = status_keys()
        worker = sum(2 * int(b[f'time_worker_{x}_ns']) for x in st.ACTIVITIES)
        reader = sum(2 * (keys.index(f'time_reader_{x}_ns') + 1) for x in st.READER_ACTIVITIES)
        t = st.delta(b, a, 0, worker + 7)
        ids = st.identities(t)
        self.assertEqual(len(ids), 7)
        for what, left, right, diff in ids[:5]:
            self.assertEqual(diff, 0, what)
        self.assertEqual(ids[5][3], -7)
        self.assertIn("the reader's", ids[6][0])
        self.assertEqual(ids[6][1:], (reader, worker + 7, reader - worker - 7))
        # without the readings' times the worker's and the reader's lines are not formed
        self.assertEqual(len(st.identities(st.delta(b, a))), 5)
        # a reading in which a kind's causes do not add up shows it
        a['time_read_wait_by_idle_ns'] = str(int(a['time_read_wait_by_idle_ns']) + 5)
        self.assertEqual(st.identities(st.delta(b, a))[0][3], 5)

    def test_a_version_5_reading_gives_its_identities(self):
        b, a = fifth(reading(1)), fifth(reading(3))
        # an older daemon's read waited for the worker's fourteen
        for r in (b, a):
            for kind in st.KINDS:
                r[f'time_{kind}_wait_ns'] = str(sum(int(r.get(f'time_{kind}_wait_by_{x}_ns', '0')) for x in st.ACTIVITIES))
        worker = sum(int(a[f'time_worker_{x}_ns']) - int(b[f'time_worker_{x}_ns']) for x in st.ACTIVITIES[:14])
        t = st.delta(b, a, 0, worker)
        self.assertTrue(t['ok'], t['why'])
        ids = st.identities(t)
        self.assertEqual(len(ids), 6)
        for what, left, right, diff in ids:
            self.assertEqual(diff, 0, what)

    def test_render(self):
        b, a = reading(1), reading(3)
        good = {'label': 'zfs-skyzfsd', 'workload': 'tpcb', 'tps': 4321.0, 'time': st.delta(b, a, 0, 10 ** 9), 'steps': 4,
                'relation_read_us': 345.6, 'relation_reads': 777, 'duration_s': 120, 'ages': ('12', '340'), 'model_relation_read_us': 151.5}
        none = {'label': 'old daemon', 'workload': 'tpcb', 'tps': 1.0, 'time': st.delta({}, {}), 'steps': None,
                'relation_read_us': None}
        bare = {'label': 'no reading', 'workload': 'tpcb', 'tps': 1.0, 'time': None, 'steps': None, 'relation_read_us': None}
        doubtful = dict(good, label='restarted', time=st.delta(reading(1, pid='1'), reading(3, pid='2')))
        text = '\n'.join(st.render([good, none, bare, doubtful]))
        self.assertIn('old daemon: NO TIMES', text)
        self.assertIn('no reading: NO TIMES: the row has no reading', text)
        self.assertIn('restarted: THE READING IS DOUBTFUL', text)
        for head in ("A read's life", "A write's life", "A flush's life", "A read's wait", "The worker's time",
                     'Checkpoint steps', "The pump's store", 'The identities', 'The longest of each'):
            self.assertIn(head, text)
        self.assertIn("the readings' ages: 12 ms and 340 ms", text)
        self.assertIn("the daemon's share of it", text)
        self.assertIn('| 151.5 |', text)
        self.assertIn('steps per second | reads per step', text)
        self.assertIn('| 4 | 4.00 |', text)      # 4 steps in a window of 1 s
        self.assertIn(' 0.3 |', text)     # 777 relation reads of 345.6 µs: 0.3 s
        for head in ('The window of each reading', 'The same wait in groups', 'Flushes beside reads', '`busy` is all but idle',
                     "The step's barriers in the window", 'The reader (status version 6',
                     'the walk, of the six parts', "all reads' wait, ms", "The checkpoint rule's own sync"):
            self.assertIn(head, text)
        # every table's rows have as many cells as its header
        lines = text.split('\n')
        for i, line in enumerate(lines):
            if line.startswith('|---'):
                cells = lines[i - 1].count('|')
                self.assertTrue(lines[i - 1].endswith('|'), lines[i - 1])
                self.assertNotIn('', [c.strip() for c in lines[i - 1].split('|')[1:-1]], lines[i - 1])
                self.assertEqual(line.count('|'), cells, lines[i - 1])
                j = i + 1
                while j < len(lines) and lines[j].startswith('|'):
                    self.assertEqual(lines[j].count('|'), cells, lines[j])
                    j += 1
        # the worker's shares add up to 100
        w = lines[lines.index(next(x for x in lines if x.startswith("The worker's time"))) + 4]
        shares = [float(c.strip().rstrip('%')) for c in w.split('|')[3:-1]]
        # busy first, then the fifteen: they add up to 100, and busy is all but idle
        self.assertAlmostEqual(sum(shares[1:]), 100.0, delta=0.8)
        self.assertAlmostEqual(shares[0] + shares[1], 100.0, delta=0.11)
        self.assertEqual(st.render([]), [])
        # a window with no request of any kind renders, with n/a
        zero = dict(good, label='idle', time=st.delta(b, b, 0, 5))
        self.assertIn('n/a', '\n'.join(st.render([zero])))

    def test_a_version_5_reading_renders_with_n_a_where_it_has_nothing(self):
        good = {'label': 'fifth', 'workload': 'tpcb', 'tps': 1.0, 'steps': 4, 'relation_read_us': None,
                'time': st.delta(fifth(reading(1)), fifth(reading(3)), 0, 10 ** 9)}
        text = '\n'.join(st.render([good]))
        self.assertNotIn('NO TIMES', text)
        self.assertEqual(cells(text, "The step's barriers in the window", 'fifth')['home: count'], 'n/a')
        self.assertEqual(cells(text, 'The reader (status version 6', 'fifth')['checked reads'], 'n/a')
        self.assertEqual(cells(text, "The worker's time by activity", 'fifth')['step_barrier'], 'n/a')


def cells(text, title, label):
    """The row of `label` in the table under the paragraph that starts with `title`: {header: cell}."""
    lines = text.split('\n')
    i = next(n for n, x in enumerate(lines) if x.startswith(title))
    j = next(n for n in range(i, len(lines)) if lines[n].startswith('|---'))
    heads = [c.strip() for c in lines[j - 1].split('|')[1:-1]]
    for row in lines[j + 1:]:
        if not row.startswith('|'):
            break
        got = [c.strip() for c in row.split('|')[1:-1]]
        if got[1] == label:
            return dict(zip(heads, got))
    raise AssertionError(f'no row {label!r} under {title!r}')


class Columns(unittest.TestCase):
    """Each printed figure against a reading made by hand: the unit, the divisor and what is summed."""

    def setUp(self):
        d = {k: 0 for k in status_keys() if not st.is_end_value(k)}
        d.update(
            time_read_count=2000, time_read_wait_ns=400_000_000, time_read_call_ns=60_000_000,
            time_pump_submit_count=4000, time_pump_submit_ns=20_000_000,
            time_pump_commit_read_count=2000, time_pump_commit_read_ns=44_000_000, time_pump_taken_read_ns=40_000_000,
            time_flush_count=50, time_flush_call_ns=3_500_000_000, time_flush_wait_ns=1_000_000,
            # a step: 2 + 0 + 4 + 36 + 2 + 0 ms, in 10 steps; 4 own syncs of 300 ms
            time_step_walk_ns=20_000_000, time_step_save_ns=0, time_step_apply_ns=40_000_000,
            time_step_home_sync_ns=360_000_000, time_step_log_start_ns=20_000_000, time_step_end_ns=0,
            time_step_own_sync_ns=1_200_000_000,
            time_gate_count=3, time_gate_ns=7_500_000,
            time_pump_store_count=100, time_pump_store_ns=1_300_000, time_pump_store_completions=140,
            # 10 batches: the home barrier 3 ms in all, the reuse barrier 0.5 ms
            time_barrier_home_count=10, time_barrier_home_ns=3_000_000,
            time_barrier_reuse_count=10, time_barrier_reuse_ns=500_000,
            # the reader: 1 s idle, 3 s reading, 1 s other; 400 checked reads, 6 blocks beyond each
            time_reader_idle_ns=1_000_000_000, time_reader_read_ns=3_000_000_000, time_reader_other_ns=1_000_000_000,
            time_read_unflushed_count=400, time_read_unflushed_extra_bytes=400 * 24576, time_read_unflushed_mismatched=0)
        end = {k: 0 for k in status_keys() if st.is_end_value(k)}
        end.update(time_step_home_sync_max_ns=511_140_000, time_step_own_sync_max_ns=878_370_000, time_step_walk_max_ns=9_000_000,
                   time_gate_max_ns=2_500_000, time_read_longest_wait_cause='step_home_sync',
                   time_barrier_home_max_ns=1_250_000)
        t = {'ok': True, 'why': '', 'delta': d, 'end': end, 'before': {}, 'after': {}, 'window_ns': 5 * 10 ** 9}
        self.row = {'label': 'row', 'workload': 'tpcb', 'tps': 4000.0, 'time': t, 'steps': 10, 'syncs': 4,
                    'relation_read_us': 400.0, 'relation_reads': 5000, 'duration_s': 4, 'ages': ('10', '20'),
                    'model_relation_read_us': 180.0}
        self.text = '\n'.join(st.render([self.row]))

    def test_the_step_parts_are_ms_per_step(self):
        c = cells(self.text, 'Checkpoint steps in the window', 'row')
        self.assertEqual([c[p] for p in st.STEP_PARTS], ['2.00', '0.00', '4.00', '36.00', '2.00', '0.00'])
        self.assertEqual((c['steps'], c['steps per second'], c['reads per step']), ('10', '2.00', '200'))

    def test_all_parts_are_the_six_and_the_own_sync_is_not_one(self):
        c = cells(self.text, 'Checkpoint steps in the window', 'row')
        self.assertEqual(c['the six parts'], '44.00')
        self.assertNotIn('own sync', c)

    def test_the_walks_share_is_of_the_six_parts(self):
        c = cells(self.text, 'Checkpoint steps in the window', 'row')
        self.assertEqual(c['the walk, of the six parts'], '4.5%')      # 20 of 440 ms, not of 1640

    def test_the_gate_is_a_count_and_ms(self):
        c = cells(self.text, 'Checkpoint steps in the window', 'row')
        self.assertEqual((c['gate count'], c['gate, ms in all'], c["all reads' wait, ms"]), ('3', '7.5', '400.0'))

    def test_the_own_sync_is_per_sync_with_its_count(self):
        c = cells(self.text, "The checkpoint rule's own sync", 'row')
        self.assertEqual((c['own syncs in the window'], c['ms per own sync'], c['ms in all']), ('4', '300.0', '1200.0'))
        self.assertEqual((c['flushes'], c['ms per flush call']), ('50', '70.0'))
        # without the count of syncs there is no mean, and no division by the steps in its place
        row = dict(self.row, syncs=None)
        c = cells('\n'.join(st.render([row])), "The checkpoint rule's own sync", 'row')
        self.assertEqual((c['own syncs in the window'], c['ms per own sync']), ('n/a', 'n/a'))

    def test_the_mean_flush_call_is_over_the_flushes(self):
        c = cells(self.text, 'Flushes beside reads', 'row')
        self.assertEqual((c['flushes'], c['flushes per read'], c['mean flush call, µs']), ('50', '0.025', '70000.0'))

    def test_the_daemons_reads_in_total_are_seconds(self):
        c = cells(self.text, "A read's life", 'row')
        # per read: 5 + 200 + 30 + 20 + 2 = 257 µs; 2000 reads: 0.514 s; postgres: 5000 x 400 µs = 2 s
        self.assertEqual(c['sum'], '257.0')
        self.assertEqual(c['all reads in the daemon, s'], '0.5')
        self.assertEqual((c['postgres: relation reads'], c['postgres: µs per relation read'], c['postgres: s in relation reads']),
                         ('5000', '400.0', '2.0'))
        self.assertEqual(c["the daemon's share of it"], '25.7%')
        self.assertEqual(c["the model row's on this machine"], '180.0')

    def test_the_longest_of_each_part_is_the_end_reading_and_says_so(self):
        self.assertIn('SINCE THE DAEMON STARTED SERVING', self.text)
        c = cells(self.text, 'The longest single one of each part', 'row')
        self.assertEqual((c['home_sync'], c['own sync'], c['walk'], c['gate']), ('511.14', '878.37', '9.00', '2.50'))

    def test_the_barriers_are_counts_ms_in_all_and_us_each(self):
        c = cells(self.text, "The step's barriers in the window", 'row')
        self.assertEqual((c['home: count'], c['home: ms in all'], c['home: µs each'], c['home: longest, ms']),
                         ('10', '3.0', '300.0', '1.25'))
        self.assertEqual((c['reuse: count'], c['reuse: ms in all'], c['reuse: µs each'], c['reuse: longest, ms']),
                         ('10', '0.5', '50.0', '0.00'))

    def test_the_reader_and_the_checked_reads(self):
        c = cells(self.text, 'The reader (status version 6', 'row')
        self.assertEqual((c['busy'], c['idle'], c['read'], c['other']), ('80.0%', '20.0%', '60.0%', '20.0%'))
        self.assertEqual((c['checked reads'], c['of all reads'], c['mismatched']), ('400', '20.0%', '0'))
        self.assertEqual(c['bytes read beyond those asked for, per checked read'], '24576')

    def test_the_pumps_store(self):
        c = cells(self.text, "The pump's store", 'row')
        self.assertEqual((c['stores'], c['ms each'], c['completions behind one store']), ('100', '0.013', '1.40'))

    def test_a_doubtful_row_is_marked_in_every_table(self):
        t = dict(self.row['time'], ok=False, why='the pid changed (1 then 2): the sums are not of one daemon')
        text = '\n'.join(st.render([self.row, dict(self.row, label='other', time=t)]))
        self.assertIn('- tpcb, other: THE READING IS DOUBTFUL: the pid changed', text)
        lines = text.split('\n')
        tables = [i for i, x in enumerate(lines) if x.startswith('|---')]
        self.assertGreaterEqual(len(tables), 16)
        for i in tables:
            rows = []
            j = i + 1
            while j < len(lines) and lines[j].startswith('|'):
                rows.append([c.strip() for c in lines[j].split('|')[1:-1]][1])
                j += 1
            self.assertIn('other (DOUBTFUL READING)', rows, lines[i - 1])
            self.assertIn('row', rows, lines[i - 1])
            self.assertNotIn('other', rows, lines[i - 1])


if __name__ == '__main__':
    unittest.main()
