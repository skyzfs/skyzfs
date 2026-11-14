#!/usr/bin/env python3
"""What a start's look leaves behind (pull request 69): a file whose
first two blocks are a hole and whose blocks 2 and 3 hold data, as the
test `every_start_serves_where_the_image_begins_with_a_hole` makes it.
Each round does what two starts in a row do with nothing between them:

    read block 0 through the cache      (the end of one start: its look)
    drop the cache, count; drop, count  (the next start's two drops)

and counts the rounds in which pages are left after the two drops. The
count is mincore's, over the first 64 blocks.

    readahead.py FILE ROUNDS [PAUSE_US] [alone]

PAUSE_US waits between the read and the drops (what the log store's
open used to put there). With `alone` the read is made as d173019
makes the look: POSIX_FADV_RANDOM before it, POSIX_FADV_NORMAL after.
"""
import ctypes, mmap, os, sys, time

path, rounds = sys.argv[1], int(sys.argv[2])
pause = int(sys.argv[3]) / 1e6 if len(sys.argv) > 3 else 0.0
alone = len(sys.argv) > 4 and sys.argv[4] == 'alone'
B = 4096
N = 64
libc = ctypes.CDLL(None, use_errno=True)
fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o600)
os.ftruncate(fd, 16 << 20)
os.pwrite(fd, b'\x22' * (2 * B), 2 * B)
os.fsync(fd)
m = mmap.mmap(fd, N * B, prot=mmap.PROT_READ)
addr = ctypes.addressof(ctypes.c_char.from_buffer_copy(b'')) if False else None
buf = (ctypes.c_ubyte * N)()
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
base = libc.mmap(None, N * B, mmap.PROT_READ, mmap.MAP_SHARED, fd, 0)
libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]


def cached():
    if libc.mincore(base, N * B, buf) != 0:
        raise OSError(ctypes.get_errno())
    return sum(b & 1 for b in buf)


def drop():
    os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)


left_first = left_second = 0
worst = 0
for _ in range(rounds):
    drop(); drop()
    if alone:
        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_RANDOM)
    os.pread(fd, B, 0)
    if alone:
        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_NORMAL)
    if pause:
        time.sleep(pause)
    drop(); a = cached()
    b = a
    if a:
        drop(); b = cached()
    left_first += a > 0
    left_second += b > 0
    worst = max(worst, b)
    time.sleep(0.002)
print(f'{rounds} rounds, pause {pause * 1e6:.0f} us{", the read alone" if alone else ""}: pages left after the first drop in {left_first}, '
      f'after the second in {left_second} (most left: {worst})')
