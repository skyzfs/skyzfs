import ctypes, mmap, os, sys
libc = ctypes.CDLL(None, use_errno=True)
class Range(ctypes.Structure): _fields_ = [('off', ctypes.c_uint64), ('len', ctypes.c_uint64)]
class Stat(ctypes.Structure): _fields_ = [(n, ctypes.c_uint64) for n in ('cache', 'dirty', 'writeback', 'evicted', 'recently')]
def cached(fd):
    r, s = Range(0, 0), Stat()
    if libc.syscall(451, ctypes.c_uint(fd), ctypes.byref(r), ctypes.byref(s), ctypes.c_uint(0)) != 0:
        return 'errno %d' % ctypes.get_errno()
    return s.cache
merged, upper = sys.argv[1], sys.argv[2]
fd = os.open(os.path.join(merged, 'img'), os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o600)
os.pwrite(fd, b'\x07' * (1 << 20), 0)
os.fdatasync(fd)
real = os.open(os.path.join(upper, 'img'), os.O_RDONLY)
print('written and synced through the overlay: counted through the overlay', cached(fd), '; through the file below', cached(real))
m = mmap.mmap(fd, 1 << 20, mmap.MAP_SHARED, mmap.PROT_READ)
x = m[0]; y = m[8192]; z = m[500000]
os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
print('mapped through the overlay, then dropped through the overlay: counted through the overlay', cached(fd), '; through the file below', cached(real))
m.close()
os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
print('unmapped and dropped: through the overlay', cached(fd), '; through the file below', cached(real))
os.close(fd); os.close(real); os.unlink(os.path.join(merged, 'img'))
