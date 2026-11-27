#!/usr/bin/env python3
"""R-rewind's timeline from an agent rig artifact (part r): argv[1] is the
artifact's arun directory. Prints, on the request log's clock (seconds),
the marks, every cut line and publish line from the freeze on, and the
first take of each name the run began (trace.tsv). Run with python3 -I
on a downloaded artifact; it only reads."""
import os
import sys

A = sys.argv[1]
C = os.path.join(A, 'cases', 'R-rewind')


def rows(path):
    with open(path) as f:
        return [ln.rstrip('\n').split('\t') for ln in f if ln.strip()]


marks = {m[1]: int(m[0]) for m in rows(os.path.join(A, 'marks.tsv'))}
t0 = marks.get('freeze-R-rewind', 0)
ev = []
for name, t in marks.items():
    if t >= t0:
        ev.append((t, f'mark {name}'))
for r in rows(os.path.join(A, 'requests.tsv')):
    if int(r[0]) < t0:
        continue
    if r[2] == 'cut':
        ev.append((int(r[0]), f'cut {r[6]} group {r[7]} opens {r[5]}'))
    elif r[2] == 'publish':
        ev.append((int(r[0]), f'publish {r[4]} {" ".join(x for x in r[5:] if x != "-")}'))
seen = set()
for r in rows(os.path.join(C, 'trace.tsv')):
    if r[1] in ('intent', 'take') and r[2] == 'before' and (r[1], r[3]) not in seen:
        seen.add((r[1], r[3]))
        ev.append((int(r[0]), f'agent {r[1]} {r[3]}'))
for t, what in sorted(ev):
    print(f'{t / 1e9:10.3f}  {what}')
for f in ('rewind-txg', 'rewind-top.txt', 'rewind-cuts.txt', 'publish.out', 'publish.rounds.tsv',
          'publish.term-at', 'publish.ended-at'):
    with open(os.path.join(C, f)) as fh:
        print(f'== {f}\n{fh.read().rstrip()}')
