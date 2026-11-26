"""Replay the PR's settle (ZCR_SETTLE_TXGS groups whose END is after the
probe's start, seen at the probe's once-a-second reads, polled every
0.1 s, then the 2 s margin) on records of an earlier run, and say whether
the window would open while the load's large first group still syncs.

usage: settle_sim.py N MARGIN BIG_MIB RECORD_DIR...
"""
import math
import sys


def main(argv):
    n, margin, big = int(argv[0]), float(argv[1]), float(argv[2])
    rows = []
    for d in argv[3:]:
        try:
            lines = open(d + "/txg.tsv").read().splitlines()
        except OSError:
            continue
        start = None
        groups = []
        ticks = []
        for line in lines:
            f = line.split("\t")
            if f[0] == "start":
                start = float(f[1])
            elif f[0] == "tick":
                ticks.append(float(f[1]))
            elif f[0] == "txg" and start is not None:
                end, s, written = float(f[2]), float(f[6]), int(f[8]) / 2**20
                if end > start:
                    groups.append((end, s, written))
        if start is None or len(groups) < n:
            continue
        groups.sort()
        # The probe's reads: its ticks (a read that found the kstat).
        reads = sorted(t for t in ticks)
        nth_end = groups[n - 1][0]
        seen = next((t for t in reads if t >= nth_end), None)
        if seen is None:
            continue
        t0 = math.ceil((seen - start) * 10) / 10 + start + margin
        # The first group of BIG MiB or more after the start: the burst.
        burst = next(((i + 1, g) for i, g in enumerate(groups) if g[2] >= big), None)
        if burst is None:
            rows.append((d, "no burst"))
            continue
        i, (end, s, w) = burst
        over = end - t0
        rows.append((d, "burst #%d %.0f MiB sync %.2f s ends %+.2f s from t0%s" % (
            i, w, s, over, "  << OPEN DURING ITS SYNC" if over > 0 else "")))
    for d, msg in rows:
        print(d, msg)


main(sys.argv[1:])
