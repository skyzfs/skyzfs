#!/usr/bin/env python3
"""The next look at #116: every plant list's results on 8e20e51 against
its results on 49c5f62 (the .out files here): which plants changed.

    compare2.py
"""
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
SETS = {
    '2b': (['plants-2b-49c5f62.out'], ['plants-2b-8e20e51.out']),
    '2b-next': (['plants-2b-next-49c5f62.out'], ['plants-2b-next-8e20e51.out']),
    '2c': (['plants-2c-a-49c5f62.out', 'plants-2c-b-49c5f62.out'], ['plants-2c-a-8e20e51.out', 'plants-2c-b-8e20e51.out']),
    'again': (['plants-again-49c5f62.out'], ['plants-again-8e20e51.out']),
    'head2': (['plants-head2-49c5f62.out'], ['plants-head2-8e20e51.out']),
    'look': (['plants-look-49c5f62.out'], ['plants-look-8e20e51.out']),
    'the 69 of the review at 49c5f62': (['plants-new-a-49c5f62.out', 'plants-new-b-49c5f62.out', 'plants-new-c-49c5f62.out'],
                                        ['plants-new-8e20e51.out']),
}


def read(names):
    out = {}
    for n in names:
        p = os.path.join(HERE, n)
        if not os.path.exists(p):
            continue
        with open(p) as f:
            for ln in f:
                m = re.match(r'(.*?): (KILLED|SURVIVED|NOT APPLIED)', ln)
                if m:
                    out[m.group(1)] = m.group(2)
    return out


total = {'KILLED': 0, 'SURVIVED': 0, 'NOT APPLIED': 0}
for name, (old, new) in SETS.items():
    a, b = read(old), read(new)
    count = {w: sum(1 for v in b.values() if v == w) for w in total}
    for w in total:
        total[w] += count[w]
    print(f'{name}: {len(b)} plants on 8e20e51: {count["KILLED"]} killed, {count["SURVIVED"]} survived, '
          f'{count["NOT APPLIED"]} not applied ({len(a)} results on 49c5f62)')
    for k in a:
        if k not in b:
            print(f'   no result on 8e20e51: {k}')
        elif a[k] != b[k]:
            print(f'   {a[k]} -> {b[k]}: {k}')
    for k in b:
        if k not in a:
            print(f'   new: {b[k]}: {k}')
    for k in b:
        if a.get(k) == b[k] == 'SURVIVED':
            print(f'   survived before and now: {k}')
print(f'in all: {sum(total.values())} plants: {total["KILLED"]} killed, {total["SURVIVED"]} survived, '
      f'{total["NOT APPLIED"]} not applied')
