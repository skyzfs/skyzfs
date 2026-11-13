#!/usr/bin/env python3
"""Tests of syscalls.py: the daemon's system calls by thread."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import syscalls as sc  # noqa: E402

PERF = """
 Summary of events:

 skyzfsd-queue (4101), 2468 events, 40.1%

   syscall            calls  errors  total       min       avg       max       stddev
                                     (msec)    (msec)    (msec)    (msec)        (%)
   --------------- --------  ------ -------- --------- --------- ---------     ------
   io_uring_enter      1200      0   812.345     0.002     0.677    12.100      3.21%
   futex                 34      2     1.200     0.001     0.035     0.400     20.00%

 skyzfs-reader-0 (4102), 600 events, 10.0%

   syscall            calls    total       min       avg       max       stddev
                               (msec)    (msec)    (msec)    (msec)        (%)
   --------------- --------  -------- --------- --------- ---------     ------
   pread64              300    90.000     0.010     0.300     2.000      5.00%

 skyzfs-reader-1 (4103), 400 events, 8.0%

   syscall            calls  errors  total       min       avg       max       stddev
                                     (msec)    (msec)    (msec)    (msec)        (%)
   --------------- --------  ------ -------- --------- --------- ---------     ------
   pread64              200      0    50.000     0.010     0.250     2.000      5.00%
   futex                 10      0     0.500     0.010     0.050     0.100      5.00%
"""

STRACE = """4101  io_uring_enter(3, 1, 1, IORING_ENTER_GETEVENTS, NULL, 8) = 1 <0.000500>
4102  pread64(7, "..."..., 4096, 8192) = 4096 <0.000100>
4102  futex(0x7f, FUTEX_WAIT_PRIVATE, 0, NULL <unfinished ...>
4103  pwrite64(8, "..."..., 4096, 0) = 4096 <0.000040>
4102  <... futex resumed>) = 0 <0.002000>
4101  io_uring_enter(3, 0, 1, IORING_ENTER_GETEVENTS, NULL, 8) = 0 <0.000300>
4101  +++ exited with 0 +++
"""

# What `perf trace -S --duration 1` writes before its summary: the calls
# of 1 ms or more, one line each (one of them unfinished when the trace
# ended, which perf prints with no duration).
PERF_EVENTS = """     0.013 ( 1.204 ms): skyzfsd-queue/4101 pwrite64(fd: 7, buf: 0x7f00, count: 4096, pos: 8192) = 4096
     2.100 ( 3.000 ms): skyzfsd-queue/4101 pwrite64(fd: 7, buf: 0x7f00, count: 4096, pos: 0) = 4096
     5.000 ( 1.000 ms): skyzfs-reader-0/4102 pread64(fd: 7, buf: 0x7f00, count: 4096, pos: 0) = 4096
     6.000 ( 0.400 ms): skyzfs-reader-0/4102 futex(uaddr: 0x7f, op: WAIT|PRIVATE_FLAG) = 0
     7.000 (         ): skyzfsd-queue/4101 io_uring_enter(fd: 3, to_submit: 1) ...
"""

NAMES = "4101 skyzfsd-queue\n4102 skyzfs-reader-0\n4103 skyzfs-reader-1\n"


class Parse(unittest.TestCase):
    def test_perf_summary_with_and_without_the_errors_column(self):
        per, names = sc.perf(PERF)
        self.assertEqual(per[4101]['io_uring_enter'], [1200, 812.345])
        self.assertEqual(per[4101]['futex'], [34, 1.2])
        self.assertEqual(per[4102]['pread64'], [300, 90.0])
        self.assertEqual(names[4103], 'skyzfs-reader-1')
        t = sc.by_name(per, names)
        self.assertEqual(t['skyzfs-reader']['tids'], 2)
        self.assertEqual(t['skyzfs-reader']['calls']['pread64'], [500, 140.0])
        self.assertEqual(t['skyzfsd-queue']['tids'], 1)

    def test_strace_counts_a_split_call_once_at_its_end(self):
        per, slow = sc.strace(STRACE)
        self.assertEqual(slow, {})
        self.assertEqual(per[4101]['io_uring_enter'][0], 2)
        self.assertAlmostEqual(per[4101]['io_uring_enter'][1], 0.8)
        self.assertEqual(per[4102]['futex'][0], 1)
        self.assertAlmostEqual(per[4102]['futex'][1], 2.0)
        t = sc.by_name(per, sc.threads(NAMES))
        self.assertEqual(sorted(t), ['skyzfs-reader', 'skyzfsd-queue'])
        self.assertEqual(t['skyzfs-reader']['tids'], 2)
        self.assertEqual(sorted(t['skyzfs-reader']['calls']), ['futex', 'pread64', 'pwrite64'])

    def test_perf_slow_calls_from_the_event_lines(self):
        slow = sc.perf_slow(PERF_EVENTS + PERF, 1.0)
        self.assertEqual(slow, {4101: {'pwrite64': 2}, 4102: {'pread64': 1}})
        self.assertEqual(sc.perf_slow(PERF_EVENTS, 2.0), {4101: {'pwrite64': 1}})
        # The event lines do not disturb the summary's counts.
        per, _ = sc.perf(PERF_EVENTS + PERF)
        self.assertEqual(per[4101]['io_uring_enter'], [1200, 812.345])

    def test_slow_calls_by_thread_name(self):
        per, names = sc.perf(PERF)
        t = sc.by_name(per, names, sc.perf_slow(PERF_EVENTS, 1.0))
        self.assertEqual(t['skyzfsd-queue']['slow'], {'pwrite64': 2})
        self.assertEqual(t['skyzfs-reader']['slow'], {'pread64': 1})
        # Without a threshold there is no 'slow' at all.
        self.assertNotIn('slow', sc.by_name(per, names)['skyzfsd-queue'])

    def test_strace_slow_calls(self):
        per, slow = sc.strace(STRACE, 0.4)
        self.assertEqual(slow, {4101: {'io_uring_enter': 1}, 4102: {'futex': 1}})

    def test_a_thread_without_a_name_keeps_its_id(self):
        t = sc.by_name({999: {'read': [1, 0.1]}}, {})
        self.assertIn('tid 999', t)


if __name__ == '__main__':
    unittest.main()
