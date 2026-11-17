#!/usr/bin/env python3
"""#111 at 8210332: how wide the window of D1 is in the takeover recordings.

    python3 -I window_width.py RECORDING_DIR...

In the first instance's last three seconds (course (a)'s writer, before the
kill -9): the cuts counted, and for each the time from its `barrier` line to
its `cut` line, both made on the queue's thread (the time a line carries is
taken when the line is made). A kill inside that time leaves the barrier
line with no cut line (kill_windows.out, B). The time from the cut line to
the state's count (C) is not in the log, and comes on top."""
import os
import sys

for d in sys.argv[1:]:
    first, lines = None, []
    with open(os.path.join(d, 'requests.tsv')) as f:
        for ln in f:
            c = ln.split('\t', 8)
            if c[2] == 'start':
                if first is None:
                    first = c[1]
                continue
            if c[1] == first and c[2] in ('barrier', 'cut'):
                lines.append((int(c[0]), c[2], c[3]))
    with open(os.path.join(d, 'requests.tsv')) as f:
        end = max(int(ln.split('\t', 2)[0]) for ln in f if ln.split('\t', 2)[1] == first)
    widths, prev = [], None
    for ns, ev, tag in lines:
        if ev == 'cut' and prev and prev[1] == 'barrier' and prev[2] == tag and ns >= end - 3_000_000_000:
            widths.append(ns - prev[0])
        prev = (ns, ev, tag)
    widths.sort()
    total = sum(widths)
    print(f'{d}: {len(widths)} cuts in the 3 s before the kill; barrier line to cut line: median '
          f'{widths[len(widths) // 2] / 1000:.1f} us, max {widths[-1] / 1000:.1f} us, {total / 1000:.0f} us in all: '
          f'{total / 3e9 * 100:.3f}% of the 3 s, one kill in {3e9 / total:.0f}')
