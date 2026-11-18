#!/usr/bin/env python3
"""#116, S1: when was the uberblock ring slot of the cut held across the
take-over's first kill written again? From a `takeover` recording's
files, read as text.

    python3 -I ring.py TAKEOVER_DIR...

The cut held when the holder has left is the first cut counted after it
(the holder released every cut before it). Its uberblock is in slot
`group % 31` of each label's ring (4 KiB slots, one of the 32 kept for
MMP), sector 256 + 8 * slot of the first label. held_across_start's
floor needs a block of that cut changed before the kill; with the
writer's load the first is that slot, when a later group's uberblock is
written to it.
"""
import os
import sys

for d in sys.argv[1:]:
    with open(os.path.join(d, 'requests.tsv'), errors='replace') as f:
        lines = [ln.rstrip('\n').split('\t') for ln in f]
    with open(os.path.join(d, 'holder.log'), errors='replace') as f:
        left = next(int(ln.split('\t')[0]) for ln in f if ln.rstrip('\n').endswith('\tleft'))
    starts = [int(f[0]) for f in lines if len(f) > 2 and f[2] == 'start']
    cuts = [(int(f[0]), int(f[6]), int(f[7])) for f in lines if len(f) > 7 and f[2] == 'cut' and f[6].isdigit()]
    before = [c for c in cuts if c[0] < starts[1]]
    held = next(c for c in cuts if c[0] > left)
    sector = 256 + 8 * (held[2] % 31)
    writes = [int(f[0]) for f in lines if len(f) > 6 and f[2:7] == ['recv', f[3], 'write', str(sector), '8']]
    after = [t for t in writes if t > held[0]]
    under = [c for c in before if c[0] > left]
    stride = sorted({b[2] - a[2] for a, b in zip(under, under[1:])})
    again = next((c for c in cuts if c[0] > held[0] and c[2] % 31 == held[2] % 31), None)
    with open(os.path.join(d, 'status-start-2.txt'), errors='replace') as f:
        copied = next((ln.split()[1] for ln in f if ln.startswith('cut_copied_bytes ')), '?')
    print(f'{d}:')
    print(f'   the holder left at {left / 1e9:.3f} s; the cut held from then on is cut {held[1]} (group {held[2]}, '
          f'{held[0] / 1e9:.3f} s), slot {held[2] % 31}, sector {sector}')
    print(f'   the kill: the second start is at {starts[1] / 1e9:.3f} s, after {len(under)} cuts since the holder left, '
          f'the last of group {before[-1][2]} ({before[-1][2] - held[2]} groups after the held cut); the cuts under the '
          f'writer are {stride} groups apart')
    print(f'   the slot is written again at {after[0] / 1e9:.3f} s' if after else '   the slot is not written again',
          f'({"before" if after and after[0] < starts[1] else "after"} the second start)'
          + (f', by the uberblock of cut {again[1]} (group {again[2]}, {again[2] - held[2]} groups and '
             f'{again[1] - held[1]} cuts after the held cut)' if again else ''))
    print(f'   cut_copied_bytes at the second start: {copied}')
