#!/usr/bin/env python3
"""#115: held_across_start on a few records made by hand, for Codex's
remark (afd217a counted a block that any write had changed) and for the
rule of fe6970c (a block counts by its last known content before the next
start; a write that covers it in part, or a write-zeroes, last: not known).

    python3 -I last_known.py CHECKOUT

Cut 1 (at record 1) holds block 1; the next instance starts at the record
`instance`; the cut is written after it. "the home" is what the block
holds at the next start, by the store's order."""
import os
import sys

sys.path.insert(0, os.path.join(sys.argv[1], 'tests/kernel'))
import cut_daemon_check as c  # noqa: E402

A, B, Z = 'a' * 16, 'b' * 16, c.ZERO


def wr(block, h, n=1):
    return {'flags': [], 'pos': 0, 'offset': block * 4096, 'bytes': n * 4096, 'hashes': h}


def zero(block, ambiguous=()):
    # as records_of makes it (cut_daemon_check.py, the recv of a write_zeroes)
    return {'flags': ['discard'], 'op': 'write_zeroes', 'offset': block * 4096, 'bytes': 4096,
            'ambiguous': set(ambiguous), 'seq': 0}


def count(held, *after):
    recs = [{'flags': [], 'op': 'flush'}, wr(1, [held])] + list(after) + [{'flags': ['instance']}]
    for i, r in enumerate(recs):
        r['i'], r['ns'] = i, 10 * i
    got = c.held_across_start(recs, [{'number': '1', 'prefix': 1}], [{'number': 1, 'asked': 10 * len(recs) + 5}],
                              {1: {1: held}})
    return got[0][3]


CASES = [
    ('A, then B', 'B: a copy is needed', (A, wr(1, [B]))),
    ('A, then B, then A', 'A: no copy is needed', (A, wr(1, [B]), wr(1, [A]))),
    ('A, then B, then A, then B', 'B: a copy is needed', (A, wr(1, [B]), wr(1, [A]), wr(1, [B]))),
    ('A, then a write that covers the block in part', 'not known', (A, wr(1, [None]))),
    ('A, then B, then a write that covers it in part', 'not known (B in part at least, or A again)',
     (A, wr(1, [B]), wr(1, [None]))),
    ('A, then a write in part, then B whole', 'B: a copy is needed', (A, wr(1, [None]), wr(1, [B]))),
    ('A, then a write-zeroes', 'zeros: a copy is needed', (A, zero(1))),
    ('A, then B, then a write-zeroes', 'zeros: a copy is needed', (A, wr(1, [B]), zero(1))),
    ('zeros, then B, then a write-zeroes', 'zeros: no copy is needed', (Z, wr(1, [B]), zero(1))),
    ('A, then a write-zeroes, then B received after it', 'B: a copy is needed', (A, zero(1), wr(1, [B]))),
    ('zeros; B is received, then a write-zeroes over it, then B is applied (the store runs B first: '
     'records_of puts the write-zeroes at its receipt and calls the block ambiguous)',
     'zeros: no copy is needed', (Z, zero(1, ambiguous=[1]), wr(1, [B]))),
]
for what, home, (held, *after) in CASES:
    print(f'{count(held, *after)} counted   {what}   [the home: {home}]')
