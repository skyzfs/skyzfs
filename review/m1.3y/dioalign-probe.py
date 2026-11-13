import ctypes, os, sys
libc = ctypes.CDLL("libc.so.6", use_errno=True)
STATX_DIOALIGN = 0x2000
class Timestamp(ctypes.Structure):
    _fields_ = [("tv_sec", ctypes.c_int64), ("tv_nsec", ctypes.c_uint32), ("pad", ctypes.c_int32)]
class Statx(ctypes.Structure):
    _fields_ = [("stx_mask", ctypes.c_uint32), ("stx_blksize", ctypes.c_uint32),
                ("stx_attributes", ctypes.c_uint64), ("stx_nlink", ctypes.c_uint32),
                ("stx_uid", ctypes.c_uint32), ("stx_gid", ctypes.c_uint32),
                ("stx_mode", ctypes.c_uint16), ("pad1", ctypes.c_uint16),
                ("stx_ino", ctypes.c_uint64), ("stx_size", ctypes.c_uint64),
                ("stx_blocks", ctypes.c_uint64), ("stx_attributes_mask", ctypes.c_uint64),
                ("atime", Timestamp), ("btime", Timestamp), ("ctime", Timestamp), ("mtime", Timestamp),
                ("stx_rdev_major", ctypes.c_uint32), ("stx_rdev_minor", ctypes.c_uint32),
                ("stx_dev_major", ctypes.c_uint32), ("stx_dev_minor", ctypes.c_uint32),
                ("stx_mnt_id", ctypes.c_uint64),
                ("stx_dio_mem_align", ctypes.c_uint32), ("stx_dio_offset_align", ctypes.c_uint32),
                ("spare", ctypes.c_uint64 * 12)]
for path in sys.argv[1:]:
    p = os.path.join(path, "probe.bin")
    try:
        fd = os.open(p, os.O_RDWR | os.O_CREAT, 0o600)
        os.ftruncate(fd, 1 << 20); os.close(fd)
        try:
            dfd = os.open(p, os.O_RDWR | os.O_DIRECT)
            opened = "O_DIRECT open: accepted"
        except OSError as e:
            dfd, opened = None, f"O_DIRECT open: refused ({e.strerror})"
        st = Statx()
        fd = dfd if dfd is not None else os.open(p, os.O_RDWR)
        rc = libc.statx(fd, b"", 0x1000, STATX_DIOALIGN, ctypes.byref(st))
        print(f"{path}: {opened}; statx rc={rc}, DIOALIGN in mask={bool(st.stx_mask & STATX_DIOALIGN)}, "
              f"mem={st.stx_dio_mem_align}, offset={st.stx_dio_offset_align}")
        os.close(fd)
    finally:
        try: os.unlink(p)
        except OSError: pass
