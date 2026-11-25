#!/usr/bin/env python3
"""Unit tests for txg_probe.py: the kstat's rows, the window's line, and
a real probe on a kstat file with a fake zpool.

    python3 tests/kernel/test_txg_probe.py
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
import txg_probe  # noqa: E402

HEADER = ("txg      birth            state ndirty       nread        nwritten     reads    writes   "
          "otime        qtime        wtime        stime\n")


def log(*lines):
    return [l + "\n" for l in lines]


def txg(n, end, s, ndirty=0, nwritten=0):
    return "txg\t%d\t%.6f\t1.0\t0.0\t0.0\t%.6f\t%d\t%d" % (n, end, s, ndirty, nwritten)


def fields(line):
    return dict(f.split("=", 1) for f in line.split())


MIB = 1 << 20


class Rows(unittest.TestCase):
    def test_the_columns(self):
        text = HEADER + "12  5000  C  4096  0  8192  0  2  100  200  300  400\n"
        self.assertEqual(txg_probe.rows(text), [(12, 5000, "C", 4096, 8192, 100, 200, 300, 400)])

    def test_open_groups_and_noise(self):
        text = HEADER + "13  6000  O  0  0  0  0  0  0  0  0  0\nbad line\n14 x C 1 2 3 4 5 6 7 8 9\n"
        self.assertEqual([r[0] for r in txg_probe.rows(text)], [13])


class Report(unittest.TestCase):
    def test_groups_in_the_window(self):
        lines = log("start\t90.0\t0", txg(1, 95.0, 0.1), txg(2, 101.0, 0.2), txg(3, 105.0, 3.0, MIB, 2 * MIB),
                    txg(4, 112.0, 0.5), "tick\t115.0\t5\tO", "stop\t115.0")
        self.assertEqual(txg_probe.report(lines, 100.0, 110.0),
                         "txgs=2 longest_sync=3.000 longest_group=3:1/2 longest_gap=7.000 syncs=0 sync_longest=- "
                         "sync_failed=0 before_sync=0.100 before_sync_longest=- errors=0")

    def test_a_sync_across_the_window(self):
        # Group 2's sync began before the window and ended after it.
        lines = log("start\t90.0\t0", txg(1, 95.0, 0.1), txg(2, 111.0, 14.0, 300 * MIB, 1400 * MIB),
                    "tick\t115.0\t3\tO", "stop\t115.0")
        f = fields(txg_probe.report(lines, 100.0, 110.0))
        self.assertEqual((f["txgs"], f["longest_sync"], f["longest_group"], f["longest_gap"]),
                         ("0", "14.000", "2:300/1400", "16.000"))

    def test_no_commit_since_the_window_began(self):
        # The gap runs to the probe's stop: at least that.
        lines = log("start\t90.0\t0", txg(1, 99.0, 0.1), "tick\t115.0\t2\tS", "stop\t115.0")
        self.assertIn("longest_gap=>16.000", txg_probe.report(lines, 100.0, 110.0))

    def test_no_commit_known_before_the_window(self):
        lines = log("start\t98.0\t0", txg(1, 104.0, 0.1), txg(2, 106.0, 0.1), "tick\t115.0\t3\tO", "stop\t115.0")
        self.assertIn("longest_gap=>9.000", txg_probe.report(lines, 100.0, 110.0))

    def test_history_before_the_start_is_not_cut(self):
        # The kstat's history held a group that ended before the probe
        # began: the gap from it is known, not a bound; its sync is no
        # part of the baseline, which is the margin the probe watched.
        lines = log("start\t98.0\t0", txg(1, 97.0, 5.0), txg(2, 99.0, 0.3), txg(3, 108.0, 0.1), txg(4, 109.0, 0.1),
                    "tick\t115.0\t5\tO", "stop\t115.0")
        f = fields(txg_probe.report(lines, 100.0, 110.0))
        self.assertEqual((f["longest_gap"], f["before_sync"]), ("9.000", "0.300"))

    def test_syncs(self):
        lines = log("start\t90.0\t0", txg(1, 99.0, 0.1), txg(2, 111.0, 0.1), "sync\t95.0\t1.0\t0",
                    "sync\t101.0\t0.5\t0", "sync\t104.0\t4.5\t0", "tick\t115.0\t3\tO", "stop\t115.0")
        f = fields(txg_probe.report(lines, 100.0, 110.0))
        self.assertEqual((f["syncs"], f["sync_longest"], f["sync_failed"], f["before_sync_longest"]),
                         ("2", "4.500", "0", "1.000"))

    def test_a_sync_spanning_the_window_counts(self):
        lines = log("start\t90.0\t0", "sync\t99.0\t12.0\t0", "tick\t115.0\t3\tO", "stop\t115.0")
        f = fields(txg_probe.report(lines, 100.0, 110.0))
        self.assertEqual((f["syncs"], f["sync_longest"], f["before_sync_longest"]), ("1", "12.000", "-"))

    def test_an_open_sync(self):
        lines = log("start\t90.0\t0", "sync\t101.0\t0.5\t0", "open\t103.0\t12.0", "tick\t115.0\t3\tO",
                    "stop\t115.0")
        f = fields(txg_probe.report(lines, 100.0, 110.0))
        self.assertEqual((f["syncs"], f["sync_longest"]), ("2", ">12.000"))

    def test_a_failed_sync_is_no_time(self):
        # A `zpool sync` that exits 1 at once is not a fast sync.
        lines = log("start\t90.0\t0", "sync\t94.0\t0.05\t1", "sync\t101.0\t0.05\t1", "sync\t104.0\t3.0\t0",
                    "sync\t107.0\t0.05\t1", "tick\t115.0\t3\tO", "stop\t115.0")
        f = fields(txg_probe.report(lines, 100.0, 110.0))
        self.assertEqual((f["syncs"], f["sync_failed"], f["sync_longest"], f["before_sync_longest"]),
                         ("3", "2", "3.000", "-"))

    def test_errors_are_counted(self):
        lines = log("start\t90.0\t0", "error\t91.0\tzpool sync: no such file", "error\t104.0\tbad",
                    "tick\t115.0\t3\tO", "stop\t115.0")
        self.assertTrue(txg_probe.report(lines, 100.0, 110.0).endswith(" errors=2"))

    def test_no_stop_uses_the_last_tick(self):
        lines = log("start\t90.0\t0", txg(1, 99.0, 0.1), "tick\t112.0\t2\tW")
        line = txg_probe.report(lines, 100.0, 110.0)
        self.assertIn("longest_gap=>13.000", line)
        self.assertFalse(line.startswith("NOT"))

    def test_a_kstat_never_read_is_not_covered(self):
        # Missing, or its header alone (zfs_txg_history 0): no tick at all.
        lines = log("start\t90.0\t0", "error\t91.0\tNo such file", "stop\t115.0")
        self.assertEqual(txg_probe.report(lines, 100.0, 110.0),
                         "NOT COVERED: the probe ran from 90.000 to 115.000, its last read of the kstat none, "
                         "the window from 100.000 to 110.000; txgs=0 longest_sync=- longest_group=- "
                         "longest_gap=>25.000 syncs=0 sync_longest=- sync_failed=0 before_sync=- "
                         "before_sync_longest=- errors=1")

    def test_a_probe_that_died_in_the_window_is_not_covered(self):
        lines = log("start\t90.0\t0", txg(1, 99.0, 0.1), "tick\t104.0\t2\tO")
        self.assertTrue(txg_probe.report(lines, 100.0, 110.0).startswith(
            "NOT COVERED: the probe ran from 90.000 to 104.000, its last read of the kstat at 104.000,"))

    def test_a_probe_that_began_late_is_not_covered(self):
        lines = log("start\t101.5\t0", "tick\t115.0\t2\tO", "stop\t115.0")
        self.assertTrue(txg_probe.report(lines, 100.0, 110.0).startswith("NOT COVERED: the probe ran from 101.500"))

    def test_no_start_is_no_probe(self):
        self.assertIsNone(txg_probe.report(log(txg(1, 99.0, 0.1)), 100.0, 110.0))


class Probe(unittest.TestCase):
    def test_a_probe_on_a_file(self):
        with tempfile.TemporaryDirectory() as d:
            kstat, out, bindir = os.path.join(d, "txgs"), os.path.join(d, "p.tsv"), os.path.join(d, "bin")
            os.mkdir(bindir)
            with open(os.path.join(bindir, "zpool"), "w") as f:
                f.write("#!/bin/sh\n[ \"$1 $2\" = 'sync skzt' ] || exit 3\nsleep 0.2\n")
            os.chmod(os.path.join(bindir, "zpool"), 0o755)
            now = time.clock_gettime_ns(time.CLOCK_MONOTONIC_RAW)
            with open(kstat, "w") as f:
                f.write(HEADER)
                # Committed: its end is 1 s ago (5 s of times from a birth 6 s ago).
                f.write("7  %d  C  4096  0  8192  0  2  %d  0  0  %d\n" % (now - 6 * 10**9, 4 * 10**9, 10**9))
                f.write("8  %d  O  0  0  0  0  0  0  0  0  0\n" % (now - 10**9))
            env = dict(os.environ, PATH=bindir + os.pathsep + os.environ["PATH"])
            p = subprocess.Popen([sys.executable, os.path.join(HERE, "txg_probe.py"), "probe", kstat, "skzt", out, "0.5"],
                                 env=env)
            time.sleep(2.5)
            p.send_signal(signal.SIGTERM)
            self.assertEqual(p.wait(timeout=30), 0)
            with open(out) as f:
                lines = f.readlines()
            kinds = [l.split("\t")[0] for l in lines]
            self.assertEqual((kinds[0], kinds[-1]), ("start", "stop"))
            # Group 7 once, though read at every tick; group 8 never.
            groups = [l.split("\t") for l in lines if l.startswith("txg\t")]
            self.assertEqual([g[1] for g in groups], ["7"])
            self.assertAlmostEqual(float(groups[0][2]), time.time() - 1 - 2.5, delta=1.0)
            self.assertEqual(groups[0][6], "1.000000")
            self.assertIn("tick", kinds)
            self.assertTrue(all(l.split("\t")[2] == "8" for l in lines if l.startswith("tick\t")))
            syncs = [l.split("\t") for l in lines if l.startswith("sync\t")]
            self.assertGreaterEqual(len(syncs), 2)
            self.assertTrue(all(s[3] == "0\n" for s in syncs))
            self.assertTrue(all(float(s[2]) >= 0.2 for s in syncs))

    def test_a_kstat_that_cannot_be_read(self):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "p.tsv")
            env = dict(os.environ, PATH=d)
            p = subprocess.Popen([sys.executable, os.path.join(HERE, "txg_probe.py"), "probe",
                                  os.path.join(d, "none"), "skzt", out, "0.2"], env=env)
            time.sleep(1.0)
            p.send_signal(signal.SIGTERM)
            self.assertEqual(p.wait(timeout=30), 0)
            with open(out) as f:
                errors = [l.split("\t")[2] for l in f if l.startswith("error\t")]
            # The kstat at each read; zpool, not on PATH here, once.
            self.assertTrue(any("No such file" in e for e in errors if not e.startswith("zpool")), errors)
            self.assertEqual(len([e for e in errors if e.startswith("zpool sync:")]), 1, errors)

    def test_the_command_line(self):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "p.tsv")
            with open(out, "w") as f:
                f.write("start\t90.0\t0\n" + txg(1, 104.0, 2.0, 3 * MIB, 5 * MIB) + "\ntick\t115.0\t2\tO\nstop\t115.0\n")
            r = subprocess.run([sys.executable, os.path.join(HERE, "txg_probe.py"), "report", out, "100", "110"],
                               capture_output=True, text=True)
            self.assertEqual((r.returncode, r.stdout),
                             (0, "txgs=1 longest_sync=2.000 longest_group=1:3/5 longest_gap=>14.000 syncs=0 "
                                 "sync_longest=- sync_failed=0 before_sync=- before_sync_longest=- errors=0\n"))
            with open(out, "w") as f:
                f.write("")
            r = subprocess.run([sys.executable, os.path.join(HERE, "txg_probe.py"), "report", out, "100", "110"],
                               capture_output=True, text=True)
            self.assertEqual(r.returncode, 2)


if __name__ == "__main__":
    unittest.main()
