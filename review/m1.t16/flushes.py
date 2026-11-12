#!/usr/bin/env python3
"""Per row of a topology-bench artifact: the flushes the work disk and the
log device counted, per transaction, and the time per flush.

    python3 -I review/m1.t16/flushes.py <artifact directory>

The artifact directory is the unpacked `topology-bench-sequence` of one job:
it holds one directory per row (1, 2, ...), each with one `*.json`.
Reads only; the artifact is data.
"""
import glob
import json
import os
import sys


def per(dev, txns):
    n = dev.get("flushes", 0)
    ms = dev.get("flush_ms", 0)
    return n, (n / txns if txns else 0.0), (ms / n if n else 0.0)


def main(root):
    rows = sorted(
        (d for d in os.listdir(root) if d.isdigit()), key=int
    )
    print("row label                      tps   workdisk n /txn  ms    slog n /txn  ms")
    for r in rows:
        for path in sorted(glob.glob(os.path.join(root, r, "*.json"))):
            with open(path) as f:
                j = json.load(f)
            if "tps" not in j or "devices" not in j:
                continue
            txns = j["tps"] * j["duration_s"]
            devs = j["devices"]
            wn, wp, wm = per(devs.get("workdisk", {}), txns)
            slog = devs.get("slog") or devs.get("skyzfslog") or {}
            sn, sp, sm = per(slog, txns)
            print(
                f"{r:>3} {j.get('label', '')[:26]:<26} {j['tps']:6.0f} "
                f"{wn:9d} {wp:4.2f} {wm:5.2f} {sn:9d} {sp:4.2f} {sm:5.2f}"
            )


if __name__ == "__main__":
    main(sys.argv[1])
