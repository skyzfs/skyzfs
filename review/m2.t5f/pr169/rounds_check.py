"""The rounds of a recorder run at #169, from the records themselves.

For each round record (scale-LBL-sweep-KIND-rI and -idle-rI) it prints
the window's length (the record's file window: t1 - t0), the reads'
runs, the groups that ended in the window (txg.tsv) and the groups per
second, the probe's txgs= and sync_longest=, the writer's longest=, the
settle's line, and where the load's first group of BIG MiB or more ended
against t0 (negative: before the window opened). Then, per scale and
kind, the medians of each side.

For every record with a settle (file settled), it counts the windows
that opened before the first large group had committed.

usage: rounds_check.py BIG_MIB ZFS_CLI_DIR
"""
import os
import re
import statistics
import sys


def field(path, key):
    try:
        text = open(path).read()
    except OSError:
        return None
    m = re.search(r"(?:^| )%s=([^ \n]*)" % re.escape(key), text)
    return m.group(1) if m else None


def groups_of(d):
    start, groups = None, []
    try:
        lines = open(os.path.join(d, "txg.tsv")).read().splitlines()
    except OSError:
        return None, []
    for line in lines:
        f = line.split("\t")
        if f[0] == "start":
            start = float(f[1])
        elif f[0] == "txg" and start is not None and float(f[2]) > start:
            groups.append((float(f[2]), float(f[6]), int(f[8]) / 2**20))
    groups.sort()
    return start, groups


def window_of(d):
    try:
        a, b = open(os.path.join(d, "window")).read().split()
        return float(a), float(b)
    except (OSError, ValueError):
        return None


def num(x):
    try:
        return float(str(x).lstrip("<>"))
    except (TypeError, ValueError):
        return None


def med(xs):
    xs = [x for x in xs if x is not None]
    return "%.3f" % statistics.median(xs) if xs else "?"


def main(argv):
    big, top = float(argv[0]), argv[1]
    names = sorted(os.listdir(top))
    pat = re.compile(r"^scale-(.+)-sweep-(list|holds)-(idle-)?r(\d+)$")
    table = {}
    print("record\twindow_s\truns\tgroups_in_window\tgroups_per_s\ttxgs\tsync_longest\twriter_longest\tbig_end_minus_t0\tsettled")
    for n in names:
        m = pat.match(n)
        if not m:
            continue
        d = os.path.join(top, n)
        w = window_of(d)
        start, groups = groups_of(d)
        if w is None:
            continue
        t0, t1 = w
        inw = [g for g in groups if t0 <= g[0] <= t1]
        burst = next((g for g in groups if g[2] >= big), None)
        over = "%+.2f" % (burst[0] - t0) if burst else "none"
        settled = open(os.path.join(d, "settled")).read().strip() if os.path.exists(os.path.join(d, "settled")) else "-"
        runs = field(os.path.join(d, "stdout"), "runs") if not m.group(3) else "-"
        txgs = field(os.path.join(d, "txg"), "txgs")
        sl = field(os.path.join(d, "txg"), "sync_longest")
        wl = field(os.path.join(d, "stall"), "longest")
        print("%s\t%.3f\t%s\t%d\t%.3f\t%s\t%s\t%s\t%s\t%s" % (
            n, t1 - t0, runs, len(inw), len(inw) / (t1 - t0), txgs, sl, wl, over, settled))
        side = "idle" if m.group(3) else "read"
        key = (m.group(1), m.group(2))
        t = table.setdefault(key, {"read": [], "idle": []})
        t[side].append((t1 - t0, num(txgs), num(sl), num(wl), len(inw) / (t1 - t0)))
    print()
    print("scale\tkind\tside\trounds\tmedian_window_s\tmedian_txgs\tmedian_groups_per_s\tmedian_sync_longest\tmedian_writer_longest")
    for (lbl, kind), t in sorted(table.items()):
        for side in ("read", "idle"):
            rows = t[side]
            print("%s\t%s\t%s\t%d\t%s\t%s\t%s\t%s\t%s" % (
                lbl, kind, side, len(rows), med([r[0] for r in rows]), med([r[1] for r in rows]),
                med([r[4] for r in rows]), med([r[2] for r in rows]), med([r[3] for r in rows])))
    print()
    opened_during, total, unsettled = [], 0, []
    for n in names:
        d = os.path.join(top, n)
        if not os.path.exists(os.path.join(d, "settled")):
            continue
        s = open(os.path.join(d, "settled")).read().strip()
        if not s.startswith("settled="):
            unsettled.append((n, s))
        w = window_of(d)
        start, groups = groups_of(d)
        burst = next((g for g in groups if g[2] >= big), None)
        if w is None or burst is None:
            continue
        total += 1
        if burst[0] > w[0]:
            opened_during.append("%s (%+.2f s, %.0f MiB, sync %.2f s)" % (n, burst[0] - w[0], burst[2], burst[1]))
    print("records with a settle and a group of %.0f MiB or more: %d; the window opened before it committed: %d" % (
        big, total, len(opened_during)))
    for x in opened_during:
        print("  ", x)
    print("settle lines that are not settled=: %d" % len(unsettled))
    for n, s in unsettled:
        print("  ", n, s)


main(sys.argv[1:])
