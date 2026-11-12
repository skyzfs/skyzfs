#!/usr/bin/env python3
"""Overlapping writes in flight together must apply whole (design §4.1).

    overlap-tear.py DEVICE SECONDS [BLOCKS [WRITERS]]

Each round, WRITERS threads write BLOCKS blocks of 4096 bytes at the
same offset at the same moment, with direct I/O, each its own content;
when all have returned, the range is read. Whatever the order of the
writes, the range must then be exactly one writer's buffer. A round in
which it is not is torn.

Each writer notes when its call began and ended. A round counts as
"together" when every call began before any ended: only such a round
had the writes in flight together, and only those say anything. Two
writes that never overlap in time give "0 torn" on any server.

The times are taken around the call, outside the request's time at
the device, so "together" is an upper bound. "Well together" is the
stricter count: the calls overlapped for at least a quarter of the
shorter one's length, which a few microseconds of slack at either end
cannot produce.

Prints one line: rounds R torn T together G well W first "..." (whose sector
each 512-byte sector of the first torn round was, run-length; "??" is
neither writer's), and exits 0. Exit 2: could not run.
"""
import mmap
import os
import sys
import threading
import time

BLOCK = 4096
SECTOR = 512


def main():
    dev, seconds = sys.argv[1], float(sys.argv[2])
    blocks = int(sys.argv[3]) if len(sys.argv) > 3 else 16
    writers = int(sys.argv[4]) if len(sys.argv) > 4 else 2
    size = blocks * BLOCK
    fd = os.open(dev, os.O_RDWR | os.O_DIRECT)
    bufs = [mmap.mmap(-1, size) for _ in range(writers)]
    back = mmap.mmap(-1, size)
    rounds = torn = together = well = 0
    first = ''
    errors = []
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        # Every 64 bytes of a writer's buffer say which writer and round.
        for w, buf in enumerate(bufs):
            buf.seek(0)
            buf.write((b'%02d%010d' % (w, rounds)).ljust(64, b'.') * (size // 64))
        offset = (rounds % 64) * size
        gate = threading.Barrier(writers)
        spans = [None] * writers

        def write(w):
            try:
                gate.wait()
                began = time.monotonic_ns()
                n = os.pwritev(fd, [bufs[w]], offset)
                spans[w] = (began, time.monotonic_ns())
                if n != size:
                    errors.append('short write')
            except OSError as e:
                errors.append(str(e))

        threads = [threading.Thread(target=write, args=(w,)) for w in range(writers)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        if errors or None in spans:
            print(f'rounds {rounds} torn {torn} together {together} well {well} error "{errors[0] if errors else "a writer did not return"}"')
            return 2
        if os.preadv(fd, [back], offset) != size:
            print(f'rounds {rounds} torn {torn} together {together} well {well} error "short read"')
            return 2
        rounds += 1
        overlap = min(e for _, e in spans) - max(b for b, _ in spans)
        if overlap > 0:
            together += 1
            if overlap * 4 >= min(e - b for b, e in spans):
                well += 1
        got = bytes(back)
        wrote = [bytes(buf) for buf in bufs]
        if got not in wrote:
            torn += 1
            if not first:
                first = describe(got, wrote)
    print(f'rounds {rounds} torn {torn} together {together} well {well} first "{first}"')
    return 0


def describe(got, wrote):
    """Whose each sector of the range is, run-length: "00x64 01x64"."""
    runs = []
    for i in range(0, len(got), SECTOR):
        owner = '??'
        for w, buf in enumerate(wrote):
            if got[i:i + SECTOR] == buf[i:i + SECTOR]:
                owner = '%02d' % w
                break
        if runs and runs[-1][0] == owner:
            runs[-1][1] += 1
        else:
            runs.append([owner, 1])
    return ' '.join(f'{o}x{n}' for o, n in runs)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, IndexError) as e:
        print(f'rounds 0 torn 0 together 0 well 0 error "{e}"')
        sys.exit(2)
