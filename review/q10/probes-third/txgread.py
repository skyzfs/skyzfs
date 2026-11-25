# Review scratch (Q10, third look): an independent reading of a take's
# txg.tsv (data only). Args: TXG_TSV T0 T1 (the take's window, if known; else
# "-" to use start+margin and stop-margin), MARGIN.
import sys
def main(path, t0, t1, margin):
    rows = [l.rstrip("\n").split("\t") for l in open(path, encoding="utf-8", errors="replace")]
    start = next(float(r[1]) for r in rows if r[0] == "start")
    stop = next((float(r[1]) for r in rows if r[0] == "stop"), None)
    if t0 == "-":
        t0, t1 = start + margin, (stop or start) - margin
    t0, t1 = float(t0), float(t1)
    print("probe %.3f..%s window %.3f..%.3f (%.3f s)" % (start, "%.3f" % stop if stop else "-", t0, t1, t1 - t0))
    # clock check: a group logged after the first read must have ended
    # between the read before and the read that saw it
    ticks, bad, checked, initial = [], 0, 0, True
    last_tick = None
    pend = []
    for r in rows:
        if r[0] == "tick":
            t = float(r[1])
            for txg, end in pend:
                if last_tick is not None:
                    checked += 1
                    if not (last_tick - 0.05 <= end <= t + 0.05):
                        bad += 1
                        if bad <= 3: print("  clock? txg %s end %.3f not in (%.3f, %.3f]" % (txg, end, last_tick, t))
            pend = []
            last_tick = t
        elif r[0] == "txg" and last_tick is not None:
            pend.append((r[1], float(r[2])))
    print("clock check: %d groups between reads, %d outside" % (checked, bad))
    txgs = [(int(r[1]), float(r[2]), float(r[3]), float(r[4]), float(r[5]), float(r[6]), int(r[7]), int(r[8])) for r in rows if r[0] == "txg"]
    near = [t for t in txgs if t[1] - t[5] < t1 + margin and t[1] > t0 - margin]
    for t in sorted(near, key=lambda x: -x[5])[:4]:
        where = "in" if (t[1] - t[5] < t1 and t[1] > t0) else "margin"
        print("  txg %d end %+.3f o %.3f q %.3f w %.3f s %.3f ndirty %d MiB nwritten %d MiB (%s)" % (t[0], t[1] - t0, t[2], t[3], t[4], t[5], t[6] >> 20, t[7] >> 20, where))
    syncs = [(float(r[1]), float(r[2]), r[0]) for r in rows if r[0] in ("sync", "open")]
    for s in sorted(syncs, key=lambda x: -x[1])[:3]:
        print("  %s start %+.3f secs %.3f" % (s[2], s[0] - t0, s[1]))
    print("syncs in all", len(syncs), "groups in log", len(txgs))
args = sys.argv[1:]
main(args[0], args[1], args[2], float(args[3]) if len(args) > 3 else 2.0)
