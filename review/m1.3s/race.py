# Mirror of the start's new sequence on a real file: read one block at 0
# through the cache, count, drop, count, drop, count. How often do pages
# stay after one drop and after two?
import ctypes, os, sys, time
libc = ctypes.CDLL(None, use_errno=True)
class Range(ctypes.Structure): _fields_ = [('off', ctypes.c_uint64), ('len', ctypes.c_uint64)]
class Stat(ctypes.Structure): _fields_ = [(n, ctypes.c_uint64) for n in ('cache', 'dirty', 'writeback', 'evicted', 'recently')]
def cached(fd):
    r, s = Range(0, 0), Stat()
    if libc.syscall(451, ctypes.c_uint(fd), ctypes.byref(r), ctypes.byref(s), ctypes.c_uint(0)) != 0:
        raise OSError(ctypes.get_errno(), 'cachestat')
    return s.cache
def drop(fd):
    os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)

d, trials = sys.argv[1], int(sys.argv[2])
BLOCK = 4096
def layout(name):
    p = os.path.join(d, name + '.img')
    fd = os.open(p, os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o600)
    os.ftruncate(fd, 64 << 20)
    if name == 'data':            # every block of the first 1 MiB written, in one go
        os.pwrite(fd, b'\x07' * (1 << 20), 0)
    elif name == 'hole-then-data':  # blocks 0 and 1 never written, the rest of 1 MiB written
        os.pwrite(fd, b'\x07' * ((1 << 20) - 2 * BLOCK), 2 * BLOCK)
    elif name == 'data-then-hole':  # block 0 written, the next ones not, then data
        os.pwrite(fd, b'\x07' * BLOCK, 0)
        os.pwrite(fd, b'\x07' * (1 << 20), 4 * BLOCK)
    elif name == 'pieces':        # each block written and synced apart, backwards, to scatter extents
        for b in reversed(range(64)):
            os.pwrite(fd, bytes([b + 1]) * BLOCK, b * BLOCK)
            os.fdatasync(fd)
    elif name == 'all-hole':
        pass
    os.fsync(fd)
    return p, fd

for name in ('data', 'hole-then-data', 'data-then-hole', 'pieces', 'all-hole'):
    p, fd = layout(name)
    seen = one = two = unready = 0
    worst = (0, 0, 0)
    for _ in range(trials):
        # start with nothing of the file in the cache
        for _ in range(50):
            drop(fd)
            if cached(fd) == 0: break
            time.sleep(0.001)
        else:
            unready += 1; continue
        os.pread(fd, BLOCK, 0)
        c0 = cached(fd)
        drop(fd); c1 = cached(fd)
        drop(fd); c2 = cached(fd)
        seen += c0 >= 1; one += c1 > 0; two += c2 > 0
        if (c2, c1, c0) > (worst[2], worst[1], worst[0]): worst = (c0, c1, c2)
    print('%-15s trials %d: count >= 1 after the read %d; pages left after one drop %d, after two %d; worst (read, drop, drop) %s; not ready %d'
          % (name, trials, seen, one, two, worst, unready))
    os.close(fd); os.unlink(p)
