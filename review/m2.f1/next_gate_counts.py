#!/usr/bin/env python3
"""Counts of a cargo test output, by test binary, and the counts of the
binaries of a second output summed in the first. Usage:
gate_counts.py PLAIN [OTHER...]"""
import re
import sys


def binaries(path):
    agg, cur = {}, None
    for line in open(path).read().splitlines():
        m = re.search(r'Running (.+?) \((?:\S*/)?target/debug/(?:build/([^/]+)/|deps/([A-Za-z0-9_]+)-)', line)
        if m:
            cur = (m.group(2) or m.group(3)) + ':' + m.group(1)
            continue
        m = re.search(r'Doc-tests (\S+)', line)
        if m:
            cur = 'doc:' + m.group(1)
            continue
        m = re.match(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored', line)
        if m and cur:
            a = agg.setdefault(cur, [0, 0, 0])
            for k in range(3):
                a[k] += int(m.group(k + 1))
            cur = None
    return agg


def total(agg, keys=None):
    r = [0, 0, 0]
    for k, v in agg.items():
        if keys is None or k in keys:
            for i in range(3):
                r[i] += v[i]
    return r


plain = binaries(sys.argv[1])
print(f'{sys.argv[1].split("/")[-1]}: {len(plain)} binaries, passed/failed/ignored {total(plain)}')
for other in sys.argv[2:]:
    o = binaries(other)
    same = total(plain, set(o))
    new = total(o)
    missing = sorted(set(o) - set(plain))
    print(f'{other.split("/")[-1]}: {len(o)} binaries, {new}; the same binaries in the plain run {same}; '
          f'difference {[a - b for a, b in zip(new, same)]}; the plain total with it {[p - s + n for p, s, n in zip(total(plain), same, new)]}'
          + (f'; not in the plain run: {missing}' if missing else ''))
