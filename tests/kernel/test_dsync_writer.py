#!/usr/bin/env python3
"""Unit tests for dsync_writer.py: the window's stall, and a real writer
on a temporary file.

    python3 tests/kernel/test_dsync_writer.py
"""
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import dsync_writer  # noqa: E402


def log(*lines):
    return [l + "\n" for l in lines]


class Stall(unittest.TestCase):
    def test_a_write_across_the_whole_window(self):
        # Began before the take and ended after it: the take's stall.
        lines = log("start\t90.0", "slow\t99.5\t12.000000", "sec\t111\t1\t12.0")
        self.assertEqual(dsync_writer.stall(lines, 100.0, 110.0), "longest=12.000 writes=0 before=0.000")

    def test_the_longest_overlapping_write(self):
        lines = log("start\t90.0", "slow\t101.0\t0.500000", "slow\t104.0\t2.000000", "slow\t109.9\t0.300000")
        self.assertEqual(dsync_writer.stall(lines, 100.0, 110.0).split()[0], "longest=2.000")

    def test_writes_outside_the_window_do_not_count(self):
        # One ended before the window, one began after it.
        lines = log("start\t90.0", "slow\t95.0\t3.000000", "slow\t110.5\t9.000000", "sec\t97\t40\t0.002")
        self.assertEqual(dsync_writer.stall(lines, 100.0, 110.0), "longest=<0.01 writes=0 before=3.000")

    def test_a_write_ending_at_the_start_is_before(self):
        lines = log("start\t90.0", "slow\t99.0\t1.000000", "end\t111.0\t9")
        self.assertEqual(dsync_writer.stall(lines, 100.0, 110.0), "longest=<0.01 writes=0 before=1.000")

    def test_writes_counted_in_whole_seconds_of_the_window(self):
        lines = log("start\t90.0", "sec\t99\t50\t0.001", "sec\t100\t40\t0.001", "sec\t109\t30\t0.002", "sec\t110\t20\t0.001")
        self.assertEqual(dsync_writer.stall(lines, 100.0, 110.0), "longest=<0.01 writes=70 before=0.001")

    def test_no_start_is_no_writer(self):
        self.assertIsNone(dsync_writer.stall(log("slow\t101.0\t5.0"), 100.0, 110.0))

    def test_a_torn_line_is_skipped(self):
        lines = log("start\t90.0", "slow\t101.0", "slow\t102.0\t1.5", "end\t111.0\t9")
        self.assertEqual(dsync_writer.stall(lines, 100.0, 110.0).split()[0], "longest=1.500")

    def test_a_log_that_ends_in_the_window_is_not_covered(self):
        # The writer's last word is at 104.5: what came after is not known.
        lines = log("start\t90.0", "sec\t103\t10\t0.001", "slow\t104.0\t0.500000")
        self.assertEqual(dsync_writer.stall(lines, 100.0, 110.0),
                         "NOT COVERED: the log runs from 90.000 to 104.500, the window from 100.000 to 110.000; "
                         "longest=0.500 writes=10 before=0.000")

    def test_a_log_that_starts_in_the_window_is_not_covered(self):
        lines = log("start\t102.0", "end\t111.0\t9")
        self.assertTrue(dsync_writer.stall(lines, 100.0, 110.0).startswith(
            "NOT COVERED: the log runs from 102.000 to 111.000,"))

    def test_the_last_second_and_a_slow_write_cover(self):
        # A second counted to its end, a slow write to its end: both reach t1.
        self.assertFalse(dsync_writer.stall(log("start\t90.0", "sec\t109\t1\t0.001"), 100.0, 110.0)
                         .startswith("NOT"))
        self.assertFalse(dsync_writer.stall(log("start\t90.0", "slow\t108.0\t2.000000"), 100.0, 110.0)
                         .startswith("NOT"))

    def test_a_window_with_no_whole_second(self):
        lines = log("start\t90.0", "sec\t100\t40\t0.001", "end\t111.0\t9")
        self.assertEqual(dsync_writer.stall(lines, 100.2, 101.1), "longest=<0.01 writes=- before=0.000")
        self.assertEqual(dsync_writer.stall(lines, 100.0, 101.0).split()[1], "writes=40")


def open_flags(pid, path):
    """The flags PID opened PATH with, from /proc."""
    fds = "/proc/%d/fd" % pid
    for fd in os.listdir(fds):
        try:
            if os.readlink(os.path.join(fds, fd)) == path:
                with open("/proc/%d/fdinfo/%s" % (pid, fd)) as f:
                    for line in f:
                        if line.startswith("flags:"):
                            return int(line.split()[1], 8)
        except OSError:
            continue
    raise AssertionError("%s is not open in %d" % (path, pid))


class Writer(unittest.TestCase):
    def test_writes_until_told_and_logs_its_end(self):
        with tempfile.TemporaryDirectory() as d:
            data, out = os.path.join(d, "w.dat"), os.path.join(d, "w.tsv")
            p = subprocess.Popen([sys.executable, os.path.join(HERE, "dsync_writer.py"), "write", data, out, "0"])
            time.sleep(0.5)
            self.assertEqual(open_flags(p.pid, data) & os.O_DSYNC, os.O_DSYNC)
            t0 = time.time()
            time.sleep(1.2)
            t1 = time.time()
            time.sleep(0.3)
            p.send_signal(signal.SIGTERM)
            self.assertEqual(p.wait(timeout=30), 0)
            with open(out) as f:
                lines = f.readlines()
            kinds = [l.split("\t")[0] for l in lines]
            self.assertEqual(kinds[0], "start")
            self.assertEqual(kinds[-1], "end")
            # SLOW 0: every write is logged, so one overlaps the window.
            self.assertIn("slow", kinds)
            self.assertLessEqual(os.path.getsize(data), 64 * 1024 * 1024)
            self.assertEqual(os.path.getsize(data) % 4096, 0)
            line = dsync_writer.stall(lines, t0, t1, slow=0)
            self.assertTrue(line.startswith("longest="), line)
            self.assertNotIn("<", line)

    def test_fill_writes_new_data_over_its_span(self):
        with tempfile.TemporaryDirectory() as d:
            data, out = os.path.join(d, "f.dat"), os.path.join(d, "f.tsv")
            p = subprocess.Popen([sys.executable, os.path.join(HERE, "dsync_writer.py"), "fill", data, out,
                                  str(3 * 1024 * 1024 + 5)])
            time.sleep(1.0)
            flags = open_flags(p.pid, data)
            p.send_signal(signal.SIGTERM)
            self.assertEqual(p.wait(timeout=30), 0)
            # Buffered: the file is open without O_DSYNC.
            self.assertEqual(flags & os.O_DSYNC, 0)
            with open(out) as f:
                lines = f.readlines()
            self.assertEqual((lines[0].split("\t")[0], lines[-1].split("\t")[0]), ("start", "end"))
            # 1 MiB writes over the span's whole MiB, round and round.
            self.assertEqual(os.path.getsize(data), 3 * 1024 * 1024)
            self.assertGreater(int(lines[-1].split("\t")[2]), 3)

    def test_the_command_line(self):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "w.tsv")
            with open(out, "w") as f:
                f.write("start\t90.0\nslow\t101.0\t4.250000\nend\t111.0\t9\n")
            r = subprocess.run([sys.executable, os.path.join(HERE, "dsync_writer.py"), "stall", out, "100", "110"],
                               capture_output=True, text=True)
            self.assertEqual((r.returncode, r.stdout), (0, "longest=4.250 writes=0 before=0.000\n"))
            with open(out, "w") as f:
                f.write("")
            r = subprocess.run([sys.executable, os.path.join(HERE, "dsync_writer.py"), "stall", out, "100", "110"],
                               capture_output=True, text=True)
            self.assertEqual(r.returncode, 2)


if __name__ == "__main__":
    unittest.main()
