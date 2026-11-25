#!/usr/bin/env python3
"""Whether a take held the pool's transaction groups (Q10's scale).

    txg_probe.py probe KSTAT POOL LOG [EVERY]
        Until SIGTERM or SIGINT: once a second, read KSTAT (the pool's
        txgs kstat, /proc/spl/kstat/zfs/POOL/txgs) and log each group
        the first time it reads committed; every EVERY seconds (2), when
        no sync is running, start `zpool sync POOL` and log its time
        when it ends (a zpool that cannot be started is an error line,
        and no more syncs are tried). A sync still running at the end is logged as open
        and left: it may be in the kernel's wait. LOG gets tab-separated
        lines:
            start  T  OFFSET            T: time.time(); OFFSET: T minus
                                        CLOCK_MONOTONIC_RAW, the kstat's
                                        clock (gethrtime)
            txg    TXG  END  O  Q  W  S  NDIRTY  NWRITTEN
                                        a group committed: END, when its
                                        sync ended (seconds since the
                                        epoch); O, Q, W, S its open,
                                        quiesce, wait and sync times (s)
            tick   T  TXG  STATE        the newest group at each read
            sync   T0  SECONDS  RC      a `zpool sync` that ended
            open   T0  SECONDS          one that had not, at the end
            stop   T
            error  T  TEXT              the kstat could not be read

    txg_probe.py report LOG T0 T1
        One line about the window [T0, T1] (seconds since the epoch):
            txgs=N longest_sync=S longest_gap=G syncs=M sync_longest=X
        txgs: the groups whose sync ended in the window; longest_sync:
        the longest sync of a group whose sync overlapped the window;
        longest_gap: the longest time between two groups' ends that
        overlapped the window (">G" when one side is the probe's start
        or stop, with no group's end there); syncs: the `zpool sync`
        runs that overlapped the window; sync_longest: the longest of
        them (">X" when it was still open). "-" when there is none.
        Exit 2 when the log has no start.
"""
import os
import signal
import subprocess
import sys
import time


def offset():
    return time.time() - time.clock_gettime_ns(time.CLOCK_MONOTONIC_RAW) / 1e9


def rows(text):
    """The kstat's rows: (txg, birth, state, ndirty, nwritten, o, q, w, s)."""
    out = []
    for line in text.splitlines():
        f = line.split()
        if len(f) < 12 or not f[0].isdigit():
            continue
        try:
            out.append((int(f[0]), int(f[1]), f[2], int(f[3]), int(f[5]),
                        int(f[8]), int(f[9]), int(f[10]), int(f[11])))
        except ValueError:
            continue
    return out


def probe(kstat, pool, log, every=2.0):
    stop = []
    for s in (signal.SIGTERM, signal.SIGINT):
        signal.signal(s, lambda *_: stop.append(1))
    seen = set()
    sync = None
    last_sync = 0.0
    with open(log, "w", buffering=1) as out:
        off = offset()
        out.write("start\t%.6f\t%.6f\n" % (time.time(), off))

        def read():
            try:
                with open(kstat) as f:
                    text = f.read()
            except OSError as e:
                out.write("error\t%.6f\t%s\n" % (time.time(), e))
                return
            rs = rows(text)
            for txg, birth, state, ndirty, nwritten, o, q, w, s in rs:
                if state == "C" and txg not in seen:
                    seen.add(txg)
                    end = (birth + o + q + w + s) / 1e9 + off
                    out.write("txg\t%d\t%.6f\t%.6f\t%.6f\t%.6f\t%.6f\t%d\t%d\n"
                              % (txg, end, o / 1e9, q / 1e9, w / 1e9, s / 1e9, ndirty, nwritten))
            if rs:
                newest = max(rs)
                out.write("tick\t%.6f\t%d\t%s\n" % (time.time(), newest[0], newest[2]))

        def reap():
            nonlocal sync
            if sync is not None and sync[0].poll() is not None:
                out.write("sync\t%.6f\t%.6f\t%d\n" % (sync[1], time.time() - sync[1], sync[0].returncode))
                sync = None

        next_read = time.time()
        while not stop:
            now = time.time()
            if now >= next_read:
                read()
                next_read += 1.0
            reap()
            if sync is None and every and now - last_sync >= every:
                last_sync = now
                try:
                    sync = (subprocess.Popen(["zpool", "sync", pool], stdout=subprocess.DEVNULL,
                                             stderr=subprocess.DEVNULL), now)
                except OSError as e:
                    # Said once; no more syncs.
                    out.write("error\t%.6f\tzpool sync: %s\n" % (now, e))
                    every = 0
            time.sleep(0.05)
        read()
        reap()
        if sync is not None:
            out.write("open\t%.6f\t%.6f\n" % (sync[1], time.time() - sync[1]))
        out.write("stop\t%.6f\n" % time.time())


def report(lines, t0, t1):
    """The line for the window [t0, t1], or None when the log has no start."""
    start = stop = None
    ends = []
    syncs = []  # (t0, seconds, open)
    txgs = 0
    longest_sync = None
    last_tick = None
    for line in lines:
        f = line.rstrip("\n").split("\t")
        try:
            if f[0] == "start":
                start = float(f[1])
            elif f[0] == "stop":
                stop = float(f[1])
            elif f[0] == "tick":
                last_tick = float(f[1])
            elif f[0] == "txg":
                end, s = float(f[2]), float(f[6])
                ends.append(end)
                if t0 <= end <= t1:
                    txgs += 1
                if end - s < t1 and end > t0:
                    longest_sync = max(longest_sync or 0.0, s)
            elif f[0] in ("sync", "open"):
                syncs.append((float(f[1]), float(f[2]), f[0] == "open"))
        except (IndexError, ValueError):
            continue
    if start is None:
        return None
    if stop is None:
        stop = last_tick if last_tick is not None else start
    # Anchors where no group's end is known: the probe's start, unless
    # the kstat's history held a group that ended before it, and its stop.
    points = [(e, False) for e in sorted(ends)]
    if not points or points[0][0] > start:
        points.insert(0, (start, True))
    points.append((max(stop, points[-1][0]), True))
    gap, gap_open = None, False
    for (a, ao), (b, bo) in zip(points, points[1:]):
        if a < t1 and b > t0 and (gap is None or b - a > gap):
            gap, gap_open = b - a, ao or bo
    n, longest, longest_open = 0, None, False
    for s0, secs, is_open in syncs:
        if s0 < t1 and s0 + secs > t0:
            n += 1
            if longest is None or secs > longest:
                longest, longest_open = secs, is_open

    def shown(v, is_open=False):
        if v is None:
            return "-"
        return (">" if is_open else "") + "%.3f" % v

    return "txgs=%d longest_sync=%s longest_gap=%s syncs=%d sync_longest=%s" % (
        txgs, shown(longest_sync), shown(gap, gap_open), n, shown(longest, longest_open))


def main(argv):
    if len(argv) in (4, 5) and argv[0] == "probe":
        probe(argv[1], argv[2], argv[3], float(argv[4]) if len(argv) > 4 else 2.0)
        return 0
    if len(argv) == 4 and argv[0] == "report":
        with open(argv[1], encoding="utf-8", errors="replace") as f:
            line = report(f, float(argv[2]), float(argv[3]))
        if line is None:
            print("no probe: the log has no start")
            return 2
        print(line)
        return 0
    print(__doc__, file=sys.stderr)
    return 64


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
