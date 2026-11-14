#!/usr/bin/env python3
"""Per session: cuts in the session, and those the kstat's window covers. coverage.py KERNEL_DIR ART_DIR"""
import os, re, sys
sys.path.insert(0, sys.argv[1])
import cut_ref, cut_check
art = sys.argv[2]
for name in sorted(os.listdir(art)):
    d = os.path.join(art, name)
    if not os.path.isdir(os.path.join(d, 'fixture')):
        continue
    records, read, dev = cut_ref.read_fixture(os.path.join(d, 'fixture'))
    cs, _ = cut_ref.cuts(records, cut_ref.scan(records, read, dev))
    out = []
    for k, (lo, hi) in sorted(cut_check.sessions(records).items()):
        with open(os.path.join(d, f'txgs-{k}.txt')) as f:
            want = cut_check.committed_txgs(f.read())
        ins = [c['txg'] for c in cs if lo < c['ub_record'] < hi]
        cov = [t for t in ins if want and min(want) <= t <= max(want)]
        below = [t for t in ins if want and t < min(want)]
        above = [t for t in ins if want and t > max(want)]
        out.append(f"s{k}: {len(ins)} cuts, {len(cov)} in the kstat's window {min(want) if want else '-'}..{max(want) if want else '-'}, below {below}, above {above}")
    outside = [c['n'] for c in cs if not any(lo < c['ub_record'] < hi for lo, hi in cut_check.sessions(records).values())]
    print(f"{name}: " + '; '.join(out) + (f'; cuts in no session: {outside}' if outside else ''))
