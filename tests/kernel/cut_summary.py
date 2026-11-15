#!/usr/bin/env python3
"""The cut rig's summary: OUT/recordings.tsv and each OUT/NAME/results.tsv as a table.

    cut_summary.py OUT IMAGE [TITLE]

TITLE is the heading's, "Cut rig (M2.T1)" by default; the node B rig
(M2.T2) gives its own.
"""
import json
import os
import sys


def main():
    out, image = sys.argv[1], sys.argv[2]
    title = sys.argv[3] if len(sys.argv) > 3 else 'Cut rig (M2.T1)'
    print(f'## {title}, {image}\n')
    zv = os.path.join(out, 'zfs-version.txt')
    if os.path.exists(zv):
        print(f"ZFS: {open(zv).read().strip().replace(chr(10), ', ')}; kernel {os.uname().release}\n")
    names = []
    rec = os.path.join(out, 'recordings.tsv')
    if os.path.exists(rec):
        for line in open(rec):
            f = line.rstrip('\n').split('\t')
            if f and f[0] not in names:
                names.append(f[0])
    print('| recording | line | verdict | detail |\n|---|---|---|---|')
    for n in names:
        res = os.path.join(out, n, 'results.tsv')
        if not os.path.exists(res):
            print(f'| {n} | | NOT CHECKED | |')
            continue
        chk = os.path.join(out, n, 'check.json')
        took = json.load(open(chk)).get('seconds') if os.path.exists(chk) else None
        for line in open(res):
            f = line.rstrip('\n').split('\t')
            if len(f) == 3:
                print(f"| {n} | {f[0]} | {f[1]} | {f[2].replace('|', '/')[:400]} |")
        if took is not None:
            print(f'| {n} | seconds | INFO | {took} |')


if __name__ == '__main__':
    main()
