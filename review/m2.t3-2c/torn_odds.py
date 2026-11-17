#!/usr/bin/env python3
"""#113 at e88773b, the next look: if a kill cut the request log at one of
its page boundaries (a write cut short ends at a multiple of 4096 of the
file: tear_at_kill.out, and the red run's own line), what would be left
of the line that crosses it, and would e88773b's torn_line take it?

For every multiple of 4096 inside the lines of the instances the
recording kills (the first two of `takeover`), the line that crosses it
and the byte before the boundary:

  taken      what is left has its first two columns and ends in a digit
  not taken  what is left has its first two columns and ends in no digit
             (a hex digit a-f, a tab, a letter, a comma, a minus): the
             judge's `len(digits) <= len(nxt)` leaves it the rig's error
  the time   what is left is digits of the first column only: with the
             start line it reads as a start line with another time
  the pid    what is left ends inside the second column: the rig's error

    python3 -I torn_odds.py ARTIFACT_DIR/takeover

Every boundary counts once: how likely a kill is to fall at one or another
is not known here.
"""
import os
import sys

data = open(os.path.join(sys.argv[1], 'requests.tsv'), 'rb').read()
# instances.tsv: the first is killed, and so is the second (the kills of courses a and b).
with open(os.path.join(sys.argv[1], 'instances.tsv'), 'rb') as f:
    pids = [ln.split(b'\t')[0] for ln in f.read().splitlines()]
# The log as a kill-free log: the recorded cut line made whole is not
# needed, the count is of boundaries inside whole lines.
off = 0
counts = {}
kinds = {}
for ln in data.split(b'\n'):
    f = ln.split(b'\t')
    end = off + len(ln) + 1
    if len(f) >= 3 and pids and f[1] in pids[:2] and len(f) <= 10:
        b = (off // 4096 + 1) * 4096
        while b < end:
            left = ln[:b - off]
            if b - off == 0 or b - off > len(ln):
                what = None                     # the boundary is at the line's start or after its last byte
            elif left.isdigit():
                what = 'the time'
            elif left.count(b'\t') < 2:
                what = 'the pid'
            elif left[-1:].isdigit():
                what = 'taken'
            else:
                what = 'not taken'
            if what:
                counts[what] = counts.get(what, 0) + 1
                ev = f[2].decode()
                kinds.setdefault(what, {})
                kinds[what][ev] = kinds[what].get(ev, 0) + 1
            b += 4096
    off = end
total = sum(counts.values())
print(f'{total} page boundaries inside lines of the two killed instances (pids {b", ".join(pids[:2]).decode()})')
for what in ('taken', 'not taken', 'the time', 'the pid'):
    n = counts.get(what, 0)
    by = ', '.join(f'{k} {v}' for k, v in sorted(kinds.get(what, {}).items(), key=lambda kv: -kv[1]))
    print(f'  {what}: {n} ({100 * n / total:.1f}%)  by the line\'s event: {by}')
