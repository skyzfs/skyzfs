#!/usr/bin/env python3
"""#115: each plant's result on this pull request against its result in the
reviews of #111 and #113 (the last output there that names it).

    python3 plants_compare.py

A name is a plant's first word; the lists of the two reviews reuse some
names for other plants (2b's K3 is not 2c's), so each is compared within
its own review's outputs."""
import glob
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))


def read(paths):
    out = {}
    for p in paths:
        with open(p) as f:
            for ln in f:
                m = re.match(r'(?:2[bc] )?(\S+) .*?: (KILLED|SURVIVED|NOT APPLIED)', ln)
                if m:
                    out[m.group(1)] = (m.group(2), os.path.basename(p))
    return out


def here(*names):
    return [os.path.join(HERE, n) for n in names]


def there(d, *names):
    return [os.path.join(HERE, '..', d, n) for n in names]


def again(prefix):
    out = {}
    with open(os.path.join(HERE, 'plants-again-fe6970c.out')) as f:
        for ln in f:
            m = re.match(prefix + r' (\S+) (\(as #115 has it\) )?.*?: (KILLED|SURVIVED|NOT APPLIED)', ln)
            if m and not m.group(2):
                out[m.group(1)] = (m.group(3), 'plants-again-fe6970c.out')
    return out


for title, before, now, re_planted in (
        ('the review of #111 (m2.t3-2b)',
         read(there('m2.t3-2b', 'plants.out', 'next_plants.out')),
         read(here('plants-2b.out', 'plants-2b-next.out')), again('2b')),
        ('the review of #113 (m2.t3-2c)',
         read(there('m2.t3-2c', 'plants.out', 'next_plants-ae36c2f.out', 'next_plants-e88773b.out',
                    'next2_plants-ab34565.out', 'next2_plants-3fe4133.out', 'next3_plants-73869d1.out')),
         read(here('plants-2c.out', 'plants-2c-next.out', 'plants-2c-rest.out')), again('2c'))):
    for k, v in re_planted.items():
        if now.get(k, ('NOT APPLIED',))[0] == 'NOT APPLIED':
            now[k] = v
    print(f'== {title}: {len(now)} plants run on this pull request')
    tally = {}
    for k, (st, _) in now.items():
        was = before.get(k, ('not run',))[0]
        tally[(was, st)] = tally.get((was, st), []) + [k]
    for (was, st), ks in sorted(tally.items()):
        print(f'  {was} there, {st} here: {len(ks)}' + (f": {' '.join(ks)}" if (was, st) != ('KILLED', 'KILLED') else ''))
