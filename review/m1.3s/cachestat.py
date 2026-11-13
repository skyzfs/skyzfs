import ctypes, mmap, os, sys
libc = ctypes.CDLL(None, use_errno=True)
class Range(ctypes.Structure): _fields_ = [('off', ctypes.c_uint64), ('len', ctypes.c_uint64)]
class Stat(ctypes.Structure): _fields_ = [(n, ctypes.c_uint64) for n in ('cache', 'dirty', 'writeback', 'evicted', 'recently')]
def cached(fd):
    r, s = Range(0, 0), Stat()
    if libc.syscall(451, ctypes.c_uint(fd), ctypes.byref(r), ctypes.byref(s), ctypes.c_uint(0)) != 0:
        return 'errno %d' % ctypes.get_errno()
    return (s.cache, s.dirty, s.writeback)
def drop(fd):
    return os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
d = sys.argv[1]
p = os.path.join(d, 'cachestat-probe.img')
fd = os.open(p, os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o600)
os.pwrite(fd, b'\x07' * (1 << 20), 0)
print(d, 'after a write of 1 MiB:', cached(fd))
os.fdatasync(fd)
print('  after fdatasync:', cached(fd))
drop(fd)
print('  after the drop:', cached(fd))
os.pread(fd, 4096, 0)
print('  after a read of one page:', cached(fd))
# a second descriptor reads the whole file while we drop: pages a reader brought in
m = mmap.mmap(fd, 1 << 20, mmap.MAP_SHARED, mmap.PROT_READ)
x = m[0]; y = m[8192]
drop(fd)
print('  mapped and touched, after a drop:', cached(fd))
m.close()
drop(fd)
print('  unmapped, after a drop:', cached(fd))
os.close(fd); os.unlink(p)
