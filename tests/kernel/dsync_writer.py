#!/usr/bin/env python3
"""A writer beside a take, and the longest stall it saw (Q10's scale).

    dsync_writer.py write PATH LOG [SLOW]
        4 KiB writes with O_DSYNC to PATH, over its first 64 MiB, until
        SIGTERM or SIGINT; the write under way when the signal comes is
        finished and logged. LOG gets tab-separated lines:
            start   T                   when the first write began
            slow    T  SECONDS          a write that began at T and took
                                        SECONDS, if at least SLOW (0.01)
            sec     S  COUNT  SECONDS   the writes that ended in the
                                        second S: how many, the longest
            end     T  WRITES           when it stopped, writes in all

    dsync_writer.py fill PATH LOG SPAN
        The same with 1 MiB buffered writes (no O_DSYNC) over the first
        SPAN bytes of PATH, a SLOW of 0.05: new data faster than a pool
        syncs it, so that its dirty data reaches the pool's limit and a
        write waits for a transaction group. SPAN should be more than
        zfs_dirty_data_max: a write over a block still dirty in the
        open group adds nothing. In stall's line, writes are MiB.

    dsync_writer.py stall LOG T0 T1
        One line about the window [T0, T1] (seconds since the epoch):
            longest=SECONDS writes=N before=SECONDS
        longest: the longest write that overlapped the window, "<SLOW"
        when no write of SLOW or more did; writes: the writes that ended
        in the window's whole seconds ("-" when it holds none); before:
        the longest write that ended before T0. A log that did not run
        from T0 to T1 (its start after T0, or its end, last second or
        last slow write before T1) is said first: "NOT COVERED: the log
        runs from A to B, the window from T0 to T1; " and then the line.
        Exit 2 when the log has no start.

Times are time.time(), the clock `date +%s.%N` reads.
"""
import math
import os
import signal
import sys
import time

BLOCK = 4096
SPAN = 64 * 1024 * 1024 // BLOCK
SLOW = 0.01
FILL_BLOCK = 1024 * 1024


def write(path, log, slow=SLOW, block=BLOCK, span=SPAN, flags=os.O_DSYNC):
    stop = []
    for s in (signal.SIGTERM, signal.SIGINT):
        signal.signal(s, lambda *_: stop.append(1))
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | flags, 0o644)
    buf = os.urandom(block)
    n = 0
    sec, count, longest = None, 0, 0.0
    with open(log, "w", buffering=1) as out:
        out.write("start\t%.6f\n" % time.time())
        while not stop:
            t0 = time.time()
            os.pwrite(fd, buf, (n % span) * block)
            t1 = time.time()
            n += 1
            lat = t1 - t0
            if lat >= slow:
                out.write("slow\t%.6f\t%.6f\n" % (t0, lat))
            s = int(t1)
            if s != sec:
                if sec is not None:
                    out.write("sec\t%d\t%d\t%.6f\n" % (sec, count, longest))
                sec, count, longest = s, 0, 0.0
            count += 1
            longest = max(longest, lat)
        if sec is not None:
            out.write("sec\t%d\t%d\t%.6f\n" % (sec, count, longest))
        out.write("end\t%.6f\t%d\n" % (time.time(), n))
    os.close(fd)


def stall(lines, t0, t1, slow=SLOW):
    """The line for the window [t0, t1], or None when the log has no start."""
    started = None
    last = None
    longest = None
    before = 0.0
    writes = 0
    for line in lines:
        f = line.rstrip("\n").split("\t")
        try:
            if f[0] == "start":
                started = float(f[1])
                last = max(last or started, started)
            elif f[0] == "slow":
                s, lat = float(f[1]), float(f[2])
                last = max(last or 0.0, s + lat)
                if s < t1 and s + lat > t0:
                    longest = max(longest or 0.0, lat)
                elif s + lat <= t0:
                    before = max(before, lat)
            elif f[0] == "sec":
                sec, count, lat = int(f[1]), int(f[2]), float(f[3])
                last = max(last or 0.0, sec + 1.0)
                if t0 <= sec and sec + 1 <= t1:
                    writes += count
                if sec + 1 <= t0:
                    before = max(before, lat)
            elif f[0] == "end":
                last = max(last or 0.0, float(f[1]))
        except (IndexError, ValueError):
            continue
    if started is None:
        return None
    shown = "%.3f" % longest if longest is not None else "<%g" % slow
    # Writes are counted in the window's whole seconds: none in a window
    # that holds no whole second.
    whole = math.ceil(t0) + 1 <= t1
    line = "longest=%s writes=%s before=%.3f" % (shown, writes if whole else "-", before)
    if started > t0 or last < t1:
        line = "NOT COVERED: the log runs from %.3f to %.3f, the window from %.3f to %.3f; %s" % (
            started, last, t0, t1, line)
    return line


def main(argv):
    if len(argv) >= 3 and argv[0] == "write":
        write(argv[1], argv[2], float(argv[3]) if len(argv) > 3 else SLOW)
        return 0
    if len(argv) == 4 and argv[0] == "fill":
        write(argv[1], argv[2], 0.05, FILL_BLOCK, max(1, int(argv[3]) // FILL_BLOCK), 0)
        return 0
    if len(argv) == 4 and argv[0] == "stall":
        with open(argv[1], encoding="utf-8", errors="replace") as f:
            line = stall(f, float(argv[2]), float(argv[3]))
        if line is None:
            print("no writer: the log has no start")
            return 2
        print(line)
        return 0
    print(__doc__, file=sys.stderr)
    return 64


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
