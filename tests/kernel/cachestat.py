#!/usr/bin/env python3
"""What the page cache holds of a file, by the cachestat system call.

    python3 -I tests/kernel/cachestat.py [--drop] FILE

Prints one line: `cache N dirty N writeback N evicted N recently_evicted
N`, in pages of the whole file (Linux 6.5 and later; the call is 451
on x86_64 and on arm64 alike). Exit 1 with the reason when the call
fails. An observation for the daemon rig's check g.4 (M1.T30): whether
writes made with O_DIRECT to a file leave its data in the cache, where
statx would say the file system serves it through the cache.

With --drop, first asks the kernel to evict the file's clean pages
(posix_fadvise DONTNEED), then counts: so that a read of the rig's own,
such as a digest of the file, is not counted as the server's.
"""
import ctypes
import os
import platform
import sys

SYSCALLS = {'x86_64': 451, 'aarch64': 451}
FIELDS = ('cache', 'dirty', 'writeback', 'evicted', 'recently_evicted')


class Range(ctypes.Structure):
    _fields_ = [('off', ctypes.c_uint64), ('len', ctypes.c_uint64)]


class Stat(ctypes.Structure):
    _fields_ = [(name, ctypes.c_uint64) for name in FIELDS]


def cachestat(path):
    """The five counts of the whole file, or OSError."""
    nr = SYSCALLS.get(platform.machine())
    if nr is None:
        raise OSError(f'no cachestat number known for {platform.machine()}')
    libc = ctypes.CDLL(None, use_errno=True)
    libc.syscall.restype = ctypes.c_long
    fd = os.open(path, os.O_RDONLY)
    try:
        rng, st = Range(0, 0), Stat()  # a length of 0: to the end of the file
        if libc.syscall(ctypes.c_long(nr), ctypes.c_int(fd), ctypes.byref(rng), ctypes.byref(st),
                        ctypes.c_uint(0)) != 0:
            e = ctypes.get_errno()
            raise OSError(e, os.strerror(e))
        return {name: getattr(st, name) for name in FIELDS}
    finally:
        os.close(fd)


def drop(path):
    """Evict the file's clean pages from the cache."""
    fd = os.open(path, os.O_RDONLY)
    try:
        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
    finally:
        os.close(fd)


def main():
    args = sys.argv[1:]
    dropping = args[:1] == ['--drop']
    if dropping:
        args = args[1:]
    if len(args) != 1:
        sys.exit(__doc__.strip().splitlines()[2].strip())
    try:
        if dropping:
            drop(args[0])
        c = cachestat(args[0])
    except OSError as e:
        print(f'cachestat failed: {e}')
        sys.exit(1)
    print(' '.join(f'{k} {v}' for k, v in c.items()))


if __name__ == '__main__':
    main()
