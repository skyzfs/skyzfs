#!/usr/bin/env python3
"""Does one direct write to a ublk device reach the server as one request?

The #58 review's account of a check c failure (run 43430825700): the
suite's tool writes with one pwrite on the device opened with O_DIRECT.
If the kernel cannot drop the device's cached pages in the range, it
writes through the page cache instead. A second direct write that
overlaps the first then writes out the overlapping dirty pages before
its own request, and the first call reaches the server as two requests,
cut where the second ends.

This probe repeats the shape of that failure on one device. Two threads
write together, with O_DIRECT: A covers blocks 16372 to 16379, and B
covers 16377 to 16383. Then the probe reads blocks 16372 to 16383 back
with O_DIRECT. Each round counts the write requests the device
completed, from /sys/block/NAME/stat, against the two calls. It runs
in three modes:

  plain    nothing else touches the device
  discard  blkdiscard --force of blocks 16368 to 16371 before each pair,
           as the suite's case did (a writable open and close of the
           device, which a udev watch answers with a probe)
  reader   a process reads the device's last 96 KiB through the page
           cache, over and over, while the pairs run

Each block's content names the round, the writer and the block, so the
read-back says which call won each shared block. A block that holds
neither writer's data is torn ("X"). Neither A winning nor B winning is a
fault: the two calls are concurrent. What the probe counts is requests.

    dio_split_probe.py DEVICE MODE ROUNDS OUT.json

Exit 0 when it ran, whatever it found, and 2 for an error of its own.
"""
import json
import mmap
import os
import struct
import subprocess
import sys
import threading
import time

BLOCK = 4096
A = (16372, 8)
B = (16377, 7)
WINDOW = (16368, 16)


def fill(buf, rnd, who, first, count):
    for i in range(count):
        tag = struct.pack('<IIQ', rnd, who, first + i)
        buf[i * BLOCK:(i + 1) * BLOCK] = tag * (BLOCK // len(tag))


def owner(block_bytes, rnd, block):
    for who in (1, 2):
        tag = struct.pack('<IIQ', rnd, who, block)
        if block_bytes == tag * (BLOCK // len(tag)):
            return 'AB'[who - 1]
    return 'X'


def write_stats(name):
    with open(f'/sys/block/{name}/stat') as f:
        v = [int(x) for x in f.read().split()]
    # writes completed, writes merged, sectors written
    return v[4], v[5], v[6]


def main():
    if len(sys.argv) != 5:
        print(__doc__, file=sys.stderr)
        return 2
    dev, mode, rounds, out = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
    if mode not in ('plain', 'discard', 'reader'):
        print(f'mode is plain, discard or reader: {mode}', file=sys.stderr)
        return 2
    name = os.path.basename(os.path.realpath(dev))
    fd = os.open(dev, os.O_RDWR | os.O_DIRECT)
    size = os.lseek(fd, 0, os.SEEK_END)
    if size != 16384 * BLOCK:
        print(f'{dev}: {size} bytes, the probe wants a 64 MiB device', file=sys.stderr)
        return 2
    bufs = {1: mmap.mmap(-1, A[1] * BLOCK), 2: mmap.mmap(-1, B[1] * BLOCK)}
    back = mmap.mmap(-1, WINDOW[1] * BLOCK)
    reader = None
    if mode == 'reader':
        reader = subprocess.Popen(
            ['bash', '-c', 'while :; do dd if="$1" of=/dev/null bs=4096 skip=16360 count=24 status=none; done',
             'reader', dev])
    res = {'device': dev, 'mode': mode, 'rounds': rounds, 'calls': 0, 'requests': 0, 'merges': 0,
           'split_rounds': 0, 'extra_sectors_rounds': 0, 'overlap': {}, 'torn_rounds': 0,
           'first_splits': [], 'errors': []}
    try:
        for rnd in range(1, rounds + 1):
            if mode == 'discard':
                r = subprocess.run(['blkdiscard', '--force', '--offset', str(WINDOW[0] * BLOCK),
                                    '--length', str(4 * BLOCK), dev], capture_output=True, text=True)
                if r.returncode != 0:
                    res['errors'].append(f'round {rnd}: blkdiscard exit {r.returncode}: {r.stderr.strip()}')
                    break
            fill(bufs[1], rnd, 1, A[0], A[1])
            fill(bufs[2], rnd, 2, B[0], B[1])
            gate = threading.Barrier(2)
            errs = []

            def go(who, first):
                try:
                    gate.wait()
                    n = os.pwrite(fd, bufs[who], first * BLOCK)
                    if n != len(bufs[who]):
                        errs.append(f'{"AB"[who - 1]} wrote {n} bytes')
                except OSError as e:
                    errs.append(f'{"AB"[who - 1]}: {e}')
            w0 = write_stats(name)
            ts = [threading.Thread(target=go, args=(1, A[0])), threading.Thread(target=go, args=(2, B[0]))]
            for t in ts:
                t.start()
            for t in ts:
                t.join()
            # Writeback of a buffered fallback may still be going: the
            # direct path waits for it, but give the counters a moment.
            time.sleep(0.002)
            w1 = write_stats(name)
            if errs:
                res['errors'].append(f'round {rnd}: ' + '; '.join(errs))
                break
            reqs, merges, sectors = (w1[i] - w0[i] for i in range(3))
            res['calls'] += 2
            res['requests'] += reqs
            res['merges'] += merges
            if reqs + merges > 2:
                res['split_rounds'] += 1
                if len(res['first_splits']) < 10:
                    res['first_splits'].append({'round': rnd, 'requests': reqs, 'merges': merges, 'sectors': sectors})
            if sectors != (A[1] + B[1]) * BLOCK // 512:
                res['extra_sectors_rounds'] += 1
            os.preadv(fd, [back], WINDOW[0] * BLOCK)
            got = ''.join(owner(back[(b - WINDOW[0]) * BLOCK:(b - WINDOW[0] + 1) * BLOCK], rnd, b)
                          for b in range(A[0], B[0] + B[1]))
            # blocks 16372..16376 must be A's, 16380..16383 B's; the three
            # shared ones either.
            key = got[5:8]
            res['overlap'][key] = res['overlap'].get(key, 0) + 1
            if 'X' in got or got[:5] != 'AAAAA' or got[8:] != 'BBBB':
                res['torn_rounds'] += 1
    finally:
        if reader:
            reader.kill()
            reader.wait()
        os.close(fd)
    with open(out, 'w') as f:
        json.dump(res, f, indent=1)
    print(json.dumps({k: res[k] for k in ('mode', 'rounds', 'calls', 'requests', 'merges', 'split_rounds',
                                          'extra_sectors_rounds', 'overlap', 'torn_rounds', 'errors')}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
