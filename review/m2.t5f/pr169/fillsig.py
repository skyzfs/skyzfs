"""Whether the end of the writer of new data's first slow write (fill.tsv)
marks the commit of the load's first large group, record by record.

usage: fillsig.py BIG_MIB MIN_SLOW RECORD_DIR...
"""
import sys


def main(argv):
    big, min_slow = float(argv[0]), float(argv[1])
    for d in argv[2:]:
        try:
            t = open(d + "/txg.tsv").read().splitlines()
            fl = open(d + "/fill.tsv").read().splitlines()
        except OSError:
            continue
        start = None
        groups = []
        for line in t:
            f = line.split("\t")
            if f[0] == "start":
                start = float(f[1])
            elif f[0] == "txg" and start is not None and float(f[2]) > start:
                groups.append((float(f[2]), int(f[8]) / 2**20))
        groups.sort()
        burst = next((g for g in groups if g[1] >= big), None)
        slow = next(((float(f.split("\t")[1]), float(f.split("\t")[2])) for f in fl
                     if f.startswith("slow\t") and float(f.split("\t")[2]) >= min_slow), None)
        if start is None or burst is None or slow is None:
            print("%s\t-" % d)
            continue
        print("%s\tfill_first_slow_end - burst_end = %+.3f s" % (d, slow[0] + slow[1] - burst[0]))


main(sys.argv[1:])
