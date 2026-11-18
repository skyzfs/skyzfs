#!/usr/bin/env python3
"""hold_copies.py REQUESTS CUT_NS RELEASE_NS: of the 4 KiB blocks first written
after the cut's instant and before its release, how many lie in a 64 KiB
chunk that a write committed before the instant (since the state began):
those a daemon that keeps the image's holes must still copy under the cut."""
import sys
req, cut_ns, rel_ns = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
before_chunks, after_blocks = set(), set()
for ln in open(req):
    f = ln.rstrip('\n').split('\t')
    if len(f) < 8 or f[2] != 'commit' or f[4] != 'write':
        continue
    t, off, n = int(f[0]), int(f[5]), int(f[6])  # sectors of 512 bytes
    blocks = range(off // 8, (off + n + 7) // 8)
    if t < cut_ns:
        before_chunks.update(b // 16 for b in blocks)
    elif t < rel_ns:
        after_blocks.update(blocks)
copied = [b for b in after_blocks if b // 16 in before_chunks]
print(f"chunks written before the cut: {len(before_chunks)}; blocks first written in the hold: {len(after_blocks)} "
      f"({len(after_blocks) * 4096 >> 20} MiB); of them in a chunk written before the cut: {len(copied)} "
      f"({len(copied) * 4096} bytes)")
