"""For each record: the O_DSYNC writer's stall in the load's first large
group's sync, and its longest stall that began after that group
committed (the load's steady state), from writer.tsv and txg.tsv.

usage: steady.py BIG_MIB RECORD_DIR...
"""
import sys


def main(argv):
    big = float(argv[0])
    for d in argv[1:]:
        try:
            t = open(d + "/txg.tsv").read().splitlines()
            w = open(d + "/writer.tsv").read().splitlines()
        except OSError:
            continue
        start = None
        groups = []
        for line in t:
            f = line.split("\t")
            if f[0] == "start":
                start = float(f[1])
            elif f[0] == "txg" and start is not None and float(f[2]) > start:
                groups.append((float(f[2]), float(f[6]), int(f[8]) / 2**20))
        groups.sort()
        burst = next((g for g in groups if g[2] >= big), None)
        if start is None or burst is None:
            continue
        slows = []
        end_log = None
        for line in w:
            f = line.split("\t")
            if f[0] == "slow":
                slows.append((float(f[1]), float(f[2])))
            elif f[0] == "end":
                end_log = float(f[1])
        first = max((s for t0, s in slows if t0 < burst[0] <= t0 + s + 0.05), default=0.0)
        steady = [(t0, s) for t0, s in slows if t0 >= burst[0]]
        st = max((s for _, s in steady), default=None)
        span = (end_log or 0) - burst[0]
        print("%s\tfirst=%.2f\tsteady_longest=%s\tsteady_span=%.1f" % (
            d, first, "%.2f" % st if st is not None else "-", span))


main(sys.argv[1:])
