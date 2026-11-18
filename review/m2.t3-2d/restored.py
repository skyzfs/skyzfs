#!/usr/bin/env python3
"""#115 at afd217a: Codex's remark on held_across_start. A block of a held
cut's map written with other content and then with the cut's content
again before the next start is counted (the function), though the next
instance reads the right bytes from the home with or without a copy.

    python3 -I restored.py CHECKOUT
"""
import os
import sys

sys.path.insert(0, os.path.join(sys.argv[1], 'tests/kernel'))
import cut_daemon_check as c  # noqa: E402

A, B = 'a' * 16, 'b' * 16


def wr(i, ns, block, h):
    return {'i': i, 'ns': ns, 'flags': [], 'pos': 0, 'offset': block * 4096, 'bytes': 4096, 'hashes': [h]}


recs = [{'i': 0, 'ns': 0, 'flags': [], 'op': 'flush'}, wr(1, 10, 1, A),      # cut 1 at prefix 1 holds block 1 = A
        wr(2, 20, 1, B), wr(3, 30, 1, A),                                    # A -> B -> A before the next start
        {'i': 4, 'ns': 40, 'flags': ['instance']}]
dcuts = [{'number': '1', 'prefix': 1}]
written = [{'number': 1, 'asked': 45}]
print('A -> B -> A before the next start:', c.held_across_start(recs, dcuts, written, {1: {1: A}}))
recs[3] = wr(3, 30, 2, A)
print('A -> B, left at B:                ', c.held_across_start(recs, dcuts, written, {1: {1: A}}))
