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

    dsync_writer.py stall LOG T0 T1
        One line about the window [T0, T1] (seconds since the epoch):
            longest=SECONDS writes=N before=SECONDS
        longest: the longest write that overlapped the window, "<SLOW"
        when no write of SLOW or more did; writes: the writes that ended
        in the window's whole seconds; before: the longest write that
        ended before T0. Exit 2 when the log has no start.

Times are time.time(), the clock `date +%s.%N` reads.
"""
import os
import signal
import sys
import time

BLOCK = 4096
SPAN = 64 * 1024 * 1024 // BLOCK
SLOW = 0.01


def write(path, log, slow=SLOW):
    stop = []
    for s in (signal.SIGTERM, signal.SIGINT):
        signal.signal(s, lambda *_: stop.append(1))
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_DSYNC, 0o644)
    buf = os.urandom(BLOCK)
    n = 0
    sec, count, longest = None, 0, 0.0
    with open(log, "w", buffering=1) as out:
        out.write("start\t%.6f\n" % time.time())
        while not stop:
            t0 = time.time()
            os.pwrite(fd, buf, (n % SPAN) * BLOCK)
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
    started = False
    longest = None
    before = 0.0
    writes = 0
    for line in lines:
        f = line.rstrip("\n").split("\t")
        try:
            if f[0] == "start":
                started = True
            elif f[0] == "slow":
                s, lat = float(f[1]), float(f[2])
                if s < t1 and s + lat > t0:
                    longest = max(longest or 0.0, lat)
                elif s + lat <= t0:
                    before = max(before, lat)
            elif f[0] == "sec":
                sec, count, lat = int(f[1]), int(f[2]), float(f[3])
                if t0 <= sec and sec + 1 <= t1:
                    writes += count
                if sec + 1 <= t0:
                    before = max(before, lat)
        except (IndexError, ValueError):
            continue
    if not started:
        return None
    shown = "%.3f" % longest if longest is not None else "<%g" % slow
    return "longest=%s writes=%d before=%.3f" % (shown, writes, before)


def main(argv):
    if len(argv) >= 3 and argv[0] == "write":
        write(argv[1], argv[2], float(argv[3]) if len(argv) > 3 else SLOW)
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
