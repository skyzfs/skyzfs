#!/usr/bin/env python3
"""Overlapping writes in flight together must apply whole (design §4.1).

    overlap-tear.py DEVICE SECONDS [BLOCKS [WRITERS]]

Each round, WRITERS threads write BLOCKS blocks of 4096 bytes at the
same offset at the same moment, with direct I/O, each its own content;
when all have returned, the range is read. Whatever the order of the
writes, every block of the range must then come from one and the same
writer. A round in which the blocks come from more than one is torn.

Prints one line: rounds R torn T first "..." (the writers of the first
torn round's blocks, run-length), and exits 0. Exit 2: could not run.
"""
import mmap
import os
import sys
import threading
import time

BLOCK = 4096


def main():
    dev, seconds = sys.argv[1], float(sys.argv[2])
    blocks = int(sys.argv[3]) if len(sys.argv) > 3 else 16
    writers = int(sys.argv[4]) if len(sys.argv) > 4 else 2
    size = blocks * BLOCK
    fd = os.open(dev, os.O_RDWR | os.O_DIRECT)
    bufs = [mmap.mmap(-1, size) for _ in range(writers)]
    back = mmap.mmap(-1, size)
    rounds = torn = 0
    first = ''
    errors = []
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        # Every block of a writer's buffer says which writer and round.
        for w, buf in enumerate(bufs):
            buf.seek(0)
            buf.write((b'%02d%010d' % (w, rounds)).ljust(64, b'.') * (size // 64))
        offset = (rounds % 64) * size
        gate = threading.Barrier(writers)

        def write(buf):
            try:
                gate.wait()
                if os.pwritev(fd, [buf], offset) != size:
                    errors.append('short write')
            except OSError as e:
                errors.append(str(e))

        threads = [threading.Thread(target=write, args=(b,)) for b in bufs]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        if errors:
            print(f'rounds {rounds} torn {torn} error "{errors[0]}"')
            return 2
        if os.preadv(fd, [back], offset) != size:
            print(f'rounds {rounds} torn {torn} error "short read"')
            return 2
        owners = [bytes(back[i * BLOCK:i * BLOCK + 2]) for i in range(blocks)]
        want = b'%010d' % rounds
        if any(bytes(back[i * BLOCK + 2:i * BLOCK + 12]) != want for i in range(blocks)):
            print(f'rounds {rounds} torn {torn} error "a block of round {rounds} is from another round"')
            return 2
        rounds += 1
        if len(set(owners)) > 1:
            torn += 1
            if not first:
                runs = []
                for o in owners:
                    if runs and runs[-1][0] == o:
                        runs[-1][1] += 1
                    else:
                        runs.append([o, 1])
                first = ' '.join(f'{o.decode()}x{n}' for o, n in runs)
    print(f'rounds {rounds} torn {torn} first "{first}"')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, IndexError) as e:
        print(f'rounds 0 torn 0 error "{e}"')
        sys.exit(2)
