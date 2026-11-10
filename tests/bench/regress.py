#!/usr/bin/env python3
"""Regression check for the nightly benchmark: within ONE compare.sh run,
compare each head variant against its base variant.

    tests/bench/regress.py RESULTS.json [BASE:HEAD ...] > regress.md

Each BASE:HEAD pair names two variants of the same run, e.g.
`base:head base-tuned:head-tuned`: the last green nightly's commit and
tonight's main, built and benchmarked side by side on the same runner,
interleaved. Comparing within one run is the point: runner VMs differ by
up to ~30% on the same binary, so numbers from two different runs can't
tell a regression from a slower VM.

With no pairs (a rebaseline run) only the run's own integrity is checked.
Prints a markdown report; exits 1 on any finding below, 0 otherwise.

Findings:
  * a run that failed outright (FAILED sample) or a matrix that stopped
    early (STOPPED, low disk): a broken or partial run never passes;
  * fewer than MIN_REPS reps, or a pair with no comparable cell: a check
    that compared nothing can't pass as "no regressions";
  * a regression: for one (latency point, workload, metric), the head
    median is worse than the base median by more than THRESHOLD
    (relative to the base median) AND the ranges separated (head's best
    rep worse than base's worst; with 5 reps a side that is the
    Mann-Whitney U = 0 case, p < 0.01 one-sided);
  * store-written (bytes left in the store) moving either way by more
    than THRESHOLD with separated ranges, or dropping to 0: fewer bytes
    is not an improvement when it can mean writes never reached the store.

Rows known to be unstable (NOISY) are reported but never judged.

env: THRESHOLD (fraction, default 0.15), MIN_REPS (default 3).
"""
import json
import os
import statistics
import sys
from collections import defaultdict

THRESHOLD = float(os.environ.get("THRESHOLD", "0.15"))
MIN_REPS = int(os.environ.get("MIN_REPS", "3"))

# metric -> higher is better
DIRECTION = {
    "iops": True,
    "mibs": True,
    "lat_p50_ms": False,
    "lat_p99_ms": False,
    "secs": False,
    "mib": False,
}
CHECKED = {
    "seqwrite-1m-qd1": ["mibs"],
    "randwrite-4k-qd1": ["iops"],
    "randwrite-4k-qd32": ["iops"],
    "cold-seqread-1m-qd1": ["mibs", "lat_p50_ms"],
    "cold-randread-4k-qd1": ["iops", "lat_p50_ms", "lat_p99_ms"],
    "cold-randread-4k-qd32": ["iops", "lat_p50_ms", "lat_p99_ms"],
    "thin-write-1m": ["mibs"],
    "publish-64m-unmount": ["secs"],
    "store-written": ["mib"],
}
# Bimodal on shared runners in every variant (~50 vs ~130 MiB/s, #116
# run 9): reported, never judged, until its setup is made stable.
NOISY = {("thin-write-1m", "mibs")}
# Judged in both directions: a drop can mean lost writes.
TWO_SIDED = {("store-written", "mib")}


def point_key(label):
    store, _, api = str(label).partition("+")
    return (float(store), float(api or 0))


def fmt(x):
    return f"{x:.0f}" if abs(x) >= 100 else f"{x:.2f}" if abs(x) < 10 else f"{x:.1f}"


def rng(xs):
    return f"{fmt(statistics.median(xs))} ({fmt(min(xs))}–{fmt(max(xs))})"


def main():
    samples = json.load(open(sys.argv[1]))
    pairs = [p.split(":", 1) for p in sys.argv[2:]]

    findings = []
    for s in samples:
        if s.get("job") == "FAILED":
            findings.append(f"run failed: `{s['variant']}` at {s['rtt']}, rep {s['rep']} (rc={s.get('rc')})")
        elif s.get("job") == "STOPPED":
            findings.append(f"incomplete matrix: stopped early for low disk before rep {s.get('rep')} at "
                            f"{s.get('rtt')} ({s.get('free_kb', 0) // 1024} MiB free)")
    reps = len({s["rep"] for s in samples if "rep" in s and s.get("job") not in ("FAILED", "STOPPED")})
    if reps < MIN_REPS:
        findings.append(f"only {reps} rep(s); judging needs at least {MIN_REPS}")

    cells = defaultdict(list)
    for s in samples:
        for m in CHECKED.get(s.get("job"), []):
            if m in s:
                cells[(s["rtt"], s["job"], m, s["variant"])].append(s[m])

    rows = []
    for base, head in pairs:
        compared = 0
        keys = sorted({(k[0], k[1], k[2]) for k in cells if k[3] in (base, head)},
                      key=lambda k: (point_key(k[0]),) + k[1:])
        for rtt, job, metric in keys:
            b, h = cells.get((rtt, job, metric, base), []), cells.get((rtt, job, metric, head), [])
            if len(b) < MIN_REPS or len(h) < MIN_REPS:
                continue
            compared += 1
            hib = DIRECTION[metric]
            bm, hm = statistics.median(b), statistics.median(h)
            label = f"`{job}` {metric} ({head} vs {base}, {rtt})"
            if bm == 0:
                rows.append((rtt, job, metric, f"{head} vs {base}", rng(b), rng(h), "–", "base median 0, not judged"))
                continue
            # Relative to the base median, signed so + is better.
            change = (hm - bm) / bm if hib else (bm - hm) / bm
            worse_sep = (max(h) < min(b)) if hib else (min(h) > max(b))
            better_sep = (min(h) > max(b)) if hib else (max(h) < min(b))
            if (job, metric) in NOISY:
                verdict = "noisy (not judged)"
            elif (job, metric) in TWO_SIDED and (hm == 0 or (abs(change) > THRESHOLD and (worse_sep or better_sep))):
                verdict = ":x: **changed**"
                raw = (hm - bm) / bm  # either direction matters here: show which way it moved
                findings.append(f"{label} changed: {rng(b)} → {rng(h)}, {raw * 100:+.0f}% of base"
                                + (" (nothing left in the store)" if hm == 0 else ""))
                rows.append((rtt, job, metric, f"{head} vs {base}", rng(b), rng(h), f"{raw * 100:+.0f}% of base", verdict))
                continue
            elif change < -THRESHOLD and worse_sep:
                verdict = ":x: **regression**"
                findings.append(f"{label}: {rng(b)} → {rng(h)}, {change * 100:+.0f}%")
            elif change > THRESHOLD and better_sep:
                verdict = ":white_check_mark: improved"
            else:
                verdict = ""
            rows.append((rtt, job, metric, f"{head} vs {base}", rng(b), rng(h), f"{change * 100:+.0f}%", verdict))
        if compared == 0:
            findings.append(f"`{head}` vs `{base}`: no cell had {MIN_REPS}+ reps on both sides, so nothing was compared")

    print("## Regression check (same runner, interleaved)\n")
    if not pairs:
        print("Rebaseline run: no comparison, integrity checks only.\n")
    if findings:
        print(f"**{len(findings)} finding(s)** (worse by >{THRESHOLD:.0%} with separated ranges, a changed "
              f"store footprint, a failed or incomplete run, or nothing compared):\n")
        for f in findings:
            print(f"- :x: {f}")
        print()
    else:
        print(f"No regressions (threshold {THRESHOLD:.0%} with separated min–max ranges).\n")
    if rows:
        print("<details><summary>All compared cells</summary>\n")
        print("| point | workload | metric | pair | base | head | Δ | |")
        print("|---|---|---|---|---|---|---|---|")
        for r in rows:
            print("| " + " | ".join(r) + " |")
        print("\n</details>\n")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
