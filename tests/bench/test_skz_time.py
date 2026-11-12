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
    """A reading in which every sum is scale * (its index + 1), and the identities hold."""
    r = {'pid': pid, 'state': 'serving'}
    for i, key in enumerate(status_keys()):
        if 'longest_wait_cause' in key and not key.endswith('_ns'):
            r[key] = 'step_apply'
        else:
            r[key] = str(scale * (i + 1))
    for kind in st.KINDS:
        r[f'time_{kind}_wait_ns'] = str(sum(int(r[f'time_{kind}_wait_by_{a}_ns']) for a in st.ACTIVITIES))
    return r


class Delta(unittest.TestCase):
    def test_the_module_knows_the_daemons_keys(self):
        keys = status_keys()
        self.assertEqual(len(keys), 153)
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
        self.assertEqual(len(ends), 5 * 2 + 7 + 1 + 4 + 2 + 5 * 2)

    def test_every_key_is_kept(self):
        b, a = reading(1), reading(3)
        t = st.delta(b, a, 1000, 5000)
        self.assertTrue(t['ok'], t['why'])
        self.assertEqual(len(t['delta']) + len(t['end']), 153)
        self.assertEqual(len(t['before']), 153)
        self.assertEqual(len(t['after']), 153)
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
        d = self.d(time_read_wait_ns=100, time_read_wait_by_idle_ns=10, time_read_wait_by_step_apply_ns=60,
                   time_read_wait_by_flush_ns=30)
        rows, total, wait = st.wait_by(d, 'read')
        self.assertEqual([a for a, v in rows[:3]], ['step_apply', 'flush', 'idle'])
        self.assertEqual((total, wait), (100, 100))
        self.assertEqual(len(rows), len(st.ACTIVITIES))

    def test_the_groups_hold_every_activity_once(self):
        acts = [a for _, group in st.GROUPS for a in group]
        self.assertEqual(sorted(acts), sorted(st.ACTIVITIES))
        d = self.d(time_read_wait_by_idle_ns=10, time_read_wait_by_step_apply_ns=60, time_read_wait_by_own_sync_ns=5,
                   time_read_wait_by_flush_ns=30, time_read_wait_by_discard_ns=2, time_read_wait_by_read_ns=1)
        self.assertEqual([v for _, v in st.wait_groups(d, 'read')], [10, 65, 30, 2, 1, 0])

    def test_the_identities(self):
        b, a = reading(1), reading(3)
        worker = sum(2 * (status_keys().index(f'time_worker_{x}_ns') + 1) for x in st.ACTIVITIES)
        t = st.delta(b, a, 0, worker + 7)
        ids = st.identities(t)
        self.assertEqual(len(ids), 6)
        for what, left, right, diff in ids[:5]:
            self.assertEqual(diff, 0, what)
        self.assertEqual(ids[5][3], -7)
        # without the readings' times the worker's line is not formed
        self.assertEqual(len(st.identities(st.delta(b, a))), 5)
        # a reading in which a kind's causes do not add up shows it
        a['time_read_wait_by_idle_ns'] = str(int(a['time_read_wait_by_idle_ns']) + 5)
        self.assertEqual(st.identities(st.delta(b, a))[0][3], 5)

    def test_render(self):
        b, a = reading(1), reading(3)
        good = {'label': 'zfs-skyzfsd', 'workload': 'tpcb', 'tps': 4321.0, 'time': st.delta(b, a, 0, 10 ** 9), 'steps': 4,
                'relation_read_us': 345.6, 'relation_reads': 777, 'duration_s': 120, 'ages': ('12', '340')}
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
        self.assertIn(' 0.3 |', text)     # 777 relation reads of 345.6 µs: 0.3 s
        for head in ('The window of each reading', 'The same wait in groups', 'Flushes beside reads', '`busy` is all but idle',
                     'the walk, of the six parts', "all reads' wait, ms"):
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
        # busy first, then the fourteen: they add up to 100, and busy is all but idle
        self.assertAlmostEqual(sum(shares[1:]), 100.0, delta=0.8)
        self.assertAlmostEqual(shares[0] + shares[1], 100.0, delta=0.11)
        self.assertEqual(st.render([]), [])
        # a window with no request of any kind renders, with n/a
        zero = dict(good, label='idle', time=st.delta(b, b, 0, 5))
        self.assertIn('n/a', '\n'.join(st.render([zero])))


if __name__ == '__main__':
    unittest.main()
