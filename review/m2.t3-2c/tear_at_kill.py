#!/usr/bin/env python3
"""#113 at e88773b, the next look: what a kill -9 leaves of a file that a
process appends lines to, each line one write(2) on a descriptor opened
with O_APPEND, as skyzfsd's request log is written (record.rs).

A child writes lines of LONG bytes and of SHORT bytes in turn, from
THREADS threads, each line `<thread> <count> <filler>\n`; the parent
kills it with SIGKILL after a few milliseconds and reads the file: is the
last line whole, where does a line that is not whole end (the file's
offset modulo 4096), and is a line that is not whole anywhere but last?

    python3 tear_at_kill.py DIR ROUNDS [THREADS]

No kernel feature of the rigs is used: this is the local kernel and the
file system DIR is on, which the output names.
"""
import os
import random
import re
import signal
import subprocess
import sys
import time

LONG, SHORT = 8235, 40
THREADS = int(sys.argv[3]) if len(sys.argv) > 3 else 3

CHILD = r'''
import os, sys, threading
fd = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
def run(t):
    n = 0
    while True:
        for size in (%d, %d):
            head = b'%%d %%d ' %% (t, n)
            os.write(fd, head + b'x' * (size - len(head) - 1) + b'\n')
            n += 1
for t in range(%d):
    threading.Thread(target=run, args=(t,), daemon=True).start()
threading.Event().wait()
''' % (LONG, SHORT, THREADS)


def main():
    d, rounds = sys.argv[1], int(sys.argv[2])
    path = os.path.join(d, 'tear-at-kill.log')
    with open('/proc/mounts') as f:
        mounts = [ln.split() for ln in f]
    best = max((m for m in mounts if os.path.abspath(d).startswith(m[1])), key=lambda m: len(m[1]))
    print(f'kernel {os.uname().release}, page size {os.sysconf("SC_PAGE_SIZE")}, the file is on {best[2]}')
    whole = torn_last = torn_elsewhere = 0
    ends = {}
    kinds = {'long': 0, 'short': 0}
    after = {}
    detail = []
    rnd = random.Random(113)
    for _ in range(rounds):
        if os.path.exists(path):
            os.unlink(path)
        p = subprocess.Popen([sys.executable, '-c', CHILD, path])
        t0 = time.monotonic()
        while (not os.path.exists(path) or os.path.getsize(path) < 4 * LONG) and time.monotonic() - t0 < 20:
            time.sleep(0.001)
        time.sleep(rnd.uniform(0.0, 0.004))
        p.send_signal(signal.SIGKILL)
        p.wait(timeout=20)
        with open(path, 'rb') as f:
            data = f.read()
        lines = data.split(b'\n')
        last = lines.pop()
        bad = [k for k, ln in enumerate(lines) if len(ln) + 1 not in (LONG, SHORT)]
        if bad:
            torn_elsewhere += 1
            # A line that is not whole and not last: the next line's
            # head follows what is left of it; where does that begin?
            off = sum(len(ln) + 1 for ln in lines[:bad[0]])
            m = re.search(rb'x(\d+ \d+ x)', lines[bad[0]])
            key = (off + m.start(1)) % 4096 if m else 'not found'
            after[key] = after.get(key, 0) + 1
            rest = lines[bad[0]][m.start(1):] if m else b''
            detail.append(f'    line {bad[0] + 1} of {len(lines)} whole or fused lines (the last of the file '
                          f'{"cut short" if last else "whole"}): thread {lines[bad[0]][:1].decode()}\'s line of '
                          f'{m.start(1) if m else "?"} bytes, then thread {rest[:1].decode()}\'s line of {len(rest) + 1} '
                          f'bytes; {len(lines) - 1 - bad[0]} lines after it, of threads '
                          f'{sorted({ln[:1].decode() for ln in lines[bad[0] + 1:]})}; {len(bad)} such lines in the file')
        if last:
            torn_last += 1
            ends[len(data) % 4096] = ends.get(len(data) % 4096, 0) + 1
            kinds['long' if len(last) > SHORT or last.count(b'x') > SHORT else 'short'] += 1
        elif not bad:
            whole += 1
    os.unlink(path)
    print(f'{rounds} kills: the file ended with a whole line and had no other broken line {whole} times;')
    print(f'  the last line was cut short {torn_last} times; where it ended, as the file\'s offset modulo 4096: '
          + (', '.join(f'{k}: {v} times' for k, v in sorted(ends.items())) or 'never'))
    print(f'  a line cut short stood before other lines of the same process {torn_elsewhere} times'
          + (('; where the cut line ended, modulo 4096: ' + ', '.join(f'{k}: {v}' for k, v in sorted(after.items(), key=str)))
             if after else ''))
    for ln in detail:
        print(ln)


if __name__ == '__main__':
    main()
