#!/usr/bin/env python3
"""What a direct write leaves for fdatasync, by what lies under it.

Three files on the same ext4: holes, a fallocate'd (unwritten) extent,
and extents already written. Each gets 4 KiB direct writes, and the
fdatasync after each one is timed. A write over a written extent needs
no metadata change; a write into a hole or an unwritten extent does
(allocation, or the conversion to written), and that change is only in
the kernel until a journal commit.

Run: python3 -I dio.py DIR
"""
import mmap
import os
import statistics
import sys
import time

SIZE = 64 << 20
BLOCK = 4096
WRITES = 24

d = sys.argv[1]
os.makedirs(d, exist_ok=True)
buf = mmap.mmap(-1, BLOCK)
buf.write(b'\xa5' * BLOCK)


def made(name, how):
    path = os.path.join(d, name)
    if os.path.exists(path):
        os.unlink(path)
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    how(fd)
    os.fsync(fd)
    os.close(fd)
    return path


def holes(fd):
    os.ftruncate(fd, SIZE)


def unwritten(fd):
    os.posix_fallocate(fd, 0, SIZE)


def written(fd):
    os.ftruncate(fd, 0)
    page = b'\0' * (1 << 20)
    for _ in range(SIZE >> 20):
        os.write(fd, page)


def run(name, how):
    path = made(name, how)
    fd = os.open(path, os.O_RDWR | os.O_DIRECT)
    write_ms, sync_ms = [], []
    for i in range(WRITES):
        offset = (i * 4096 + i * (1 << 20)) % (SIZE - BLOCK) & ~(BLOCK - 1)
        t0 = time.perf_counter()
        os.pwrite(fd, buf, offset)
        t1 = time.perf_counter()
        os.fdatasync(fd)
        t2 = time.perf_counter()
        write_ms.append((t1 - t0) * 1e3)
        sync_ms.append((t2 - t1) * 1e3)
    os.close(fd)
    med = statistics.median
    print(f'{name:>10}: write {med(write_ms):7.3f} ms, fdatasync after it '
          f'{med(sync_ms):7.3f} ms (max {max(sync_ms):7.3f})')


for name, how in (('written', written), ('unwritten', unwritten), ('holes', holes)):
    run(name, how)
    run(name + '2', how)
