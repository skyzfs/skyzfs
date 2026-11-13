# Two remedies for the look's read-ahead, on the layout that fails
# (blocks 0 and 1 never written, data from block 2):
#  v1: read with read-ahead off (POSIX_FADV_RANDOM), count, then drop, count, drop, count;
#  v2: drop, count (twice at most), and only then read and count.
import ctypes, os, sys, time
libc = ctypes.CDLL(None, use_errno=True)
class Range(ctypes.Structure): _fields_ = [('off', ctypes.c_uint64), ('len', ctypes.c_uint64)]
class Stat(ctypes.Structure): _fields_ = [(n, ctypes.c_uint64) for n in ('cache', 'dirty', 'writeback', 'evicted', 'recently')]
def cached(fd):
    r, s = Range(0, 0), Stat()
    if libc.syscall(451, ctypes.c_uint(fd), ctypes.byref(r), ctypes.byref(s), ctypes.c_uint(0)) != 0:
        raise OSError(ctypes.get_errno(), 'cachestat')
    return s.cache
def drop(fd): os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
d, trials = sys.argv[1], int(sys.argv[2])
BLOCK = 4096
p = os.path.join(d, 'hole-then-data.img')
fd = os.open(p, os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o600)
os.ftruncate(fd, 64 << 20)
os.pwrite(fd, b'\x07' * ((1 << 20) - 2 * BLOCK), 2 * BLOCK)
os.fsync(fd)
def empty():
    for _ in range(200):
        drop(fd)
        if cached(fd) == 0: return
        time.sleep(0.001)
    raise SystemExit('could not empty the cache')
for name in ('as it is', 'v1', 'v2'):
    blind = refused = 0
    for _ in range(trials):
        empty()
        # the state a start meets: something of the file in the cache (what the daemon before left)
        os.pread(fd, 64 * BLOCK, 0); time.sleep(0.0005); empty()
        if name == 'as it is':
            os.pread(fd, BLOCK, 0); blind += cached(fd) == 0
            drop(fd); c = cached(fd)
            if c: drop(fd); c = cached(fd)
            refused += c > 0
        elif name == 'v1':
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_RANDOM)
            os.pread(fd, BLOCK, 0); blind += cached(fd) == 0
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_NORMAL)
            drop(fd); c = cached(fd)
            if c: drop(fd); c = cached(fd)
            refused += c > 0
        else:
            drop(fd); c = cached(fd)
            if c: drop(fd); c = cached(fd)
            refused += c > 0
            os.pread(fd, BLOCK, 0); blind += cached(fd) == 0
    print('%-9s %d trials: refused for pages left %d; the count saw nothing after the read %d' % (name, trials, refused, blind))
os.close(fd); os.unlink(p)
