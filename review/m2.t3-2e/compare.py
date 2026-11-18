#!/usr/bin/env python3
"""The older lists' results on #116's head against their results on the
heads before it (review/m2.t3-2d's .out files): which plants changed.

    compare.py
"""
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
OLD = os.path.join(HERE, '..', 'm2.t3-2d')
SETS = {
    '2b': (['plants-2b.out', 'plants-2b-813120a.out'], ['plants-2b-49c5f62.out']),
    '2b-next': (['plants-2b-next.out', 'plants-2b-next-813120a.out'], ['plants-2b-next-49c5f62.out']),
    '2c': (['plants-2c.out', 'plants-2c-next.out', 'plants-2c-rest.out', 'plants-2c-survivors-813120a.out'],
           ['plants-2c-a-49c5f62.out', 'plants-2c-b-49c5f62.out']),
    'again': (['plants-again-fe6970c.out', 'plants-again-813120a.out'], ['plants-again-49c5f62.out']),
    'head2': (['plants-head2-a.out', 'plants-head2-b.out', 'plants-head2-813120a.out'], ['plants-head2-49c5f62.out']),
    'look': (['plants-look-813120a.out'], ['plants-look-49c5f62.out']),
}


def read(d, names):
    out = {}
    for n in names:
        p = os.path.join(d, n)
        if not os.path.exists(p):
            continue
        with open(p) as f:
            for ln in f:
                m = re.match(r'(.*?): (KILLED|SURVIVED|NOT APPLIED)', ln)
                if m:
                    out[m.group(1)] = m.group(2)
    return out


for name, (old, new) in SETS.items():
    a, b = read(OLD, old), read(HERE, new)
    count = {w: sum(1 for v in b.values() if v == w) for w in ('KILLED', 'SURVIVED', 'NOT APPLIED')}
    print(f'{name}: {len(b)} plants on 49c5f62: {count["KILLED"]} killed, {count["SURVIVED"]} survived, '
          f'{count["NOT APPLIED"]} not applied ({len(a)} results before)')
    for k in b:
        if k in a and a[k] != b[k]:
            print(f'   {a[k]} -> {b[k]}: {k}')
        elif k not in a:
            print(f'   (no result before) {b[k]}: {k}')
    for k in b:
        if b[k] == 'SURVIVED' and a.get(k) == 'SURVIVED':
            print(f'   survived before and now: {k}')
    for k in a:
        if k not in b:
            print(f'   NOT RUN on 49c5f62: {k} (was {a[k]})')
