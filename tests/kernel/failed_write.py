#!/usr/bin/env python3
"""The I/O of check g of the daemon rig (daemon-rig.sh): a write whose
sync fails under it, and what reads then return.

    failed_write.py fill   PATH OFFSET LEN BYTE   O_DIRECT write of LEN bytes of BYTE, then fsync
    failed_write.py write  PATH OFFSET LEN BYTE   buffered write, then fdatasync; then a buffered read back
    failed_write.py dirty  PATH OFFSET LEN BYTE   buffered write and nothing else: a page left dirty
    failed_write.py direct PATH OFFSET LEN        O_DIRECT read: what the disk (or the device) holds
    failed_write.py sync   PATH                   a new descriptor: fdatasync, then syncfs
    failed_write.py extent PATH OFFSET LEN        the 512-byte sectors of the file system's device under the range

Every command prints one line of words the rig records as they are,
and exits 0 when it could do what it was asked, whatever the kernel
answered; 2 when it could not (bad arguments, a range in several
extents). A read reports `byte 0xNN` when all LEN bytes are NN, else
`mixed`, or the error.
"""
import ctypes
import errno
import fcntl
import mmap
import os
import struct
import sys

BLOCK = 4096


def aligned(n):
    """A page-aligned buffer of n bytes (O_DIRECT wants an aligned one)."""
    return mmap.mmap(-1, n)


def say_error(e):
    return f"{errno.errorcode.get(e.errno, e.errno)}({e.errno})"


def what(buf):
    b = bytes(buf)
    return f"byte 0x{b[0]:02x}" if b and b.count(b[0]) == len(b) else 'mixed'


def fill(path, off, n, byte):
    buf = aligned(n)
    buf.write(bytes([byte]) * n)
    fd = os.open(path, os.O_WRONLY | os.O_DIRECT)
    try:
        if os.pwritev(fd, [buf], off) != n:
            return 'short write'
        os.fsync(fd)
    finally:
        os.close(fd)
    return 'filled'


def write(path, off, n, byte, sync=True):
    fd = os.open(path, os.O_RDWR)
    try:
        if os.pwrite(fd, bytes([byte]) * n, off) != n:
            return 'short write'
        if not sync:
            return 'written, not synced'
        try:
            os.fdatasync(fd)
            said = 'fdatasync ok'
        except OSError as e:
            said = f'fdatasync {say_error(e)}'
        # What the cache now holds, read through the same file.
        return f'{said}; cache reads {what(os.pread(fd, n, off))}'
    finally:
        os.close(fd)


def direct(path, off, n):
    buf = aligned(n)
    try:
        fd = os.open(path, os.O_RDONLY | os.O_DIRECT)
    except OSError as e:
        return f'open {say_error(e)}'
    try:
        got = os.preadv(fd, [buf], off)
        return what(buf[:got]) if got == n else f'short read {got}'
    except OSError as e:
        return f'read {say_error(e)}'
    finally:
        os.close(fd)


def sync(path):
    libc = ctypes.CDLL(None, use_errno=True)
    fd = os.open(path, os.O_RDWR)
    try:
        try:
            os.fdatasync(fd)
            a = 'ok'
        except OSError as e:
            a = say_error(e)
        r = libc.syncfs(fd)
        b = 'ok' if r == 0 else f"{errno.errorcode.get(ctypes.get_errno(), ctypes.get_errno())}({ctypes.get_errno()})"
    finally:
        os.close(fd)
    return f'fdatasync {a}; syncfs {b}'


FS_IOC_FIEMAP = 0xC020660B
FIEMAP_FLAG_SYNC = 1


def extent(path, off, n):
    """-> 'START COUNT' in 512-byte sectors, when one extent holds the range."""
    count = 8
    req = struct.pack('=QQIIII', off, n, FIEMAP_FLAG_SYNC, 0, count, 0) + b'\0' * (56 * count)
    buf = bytearray(req)
    fd = os.open(path, os.O_RDONLY)
    try:
        fcntl.ioctl(fd, FS_IOC_FIEMAP, buf)
    finally:
        os.close(fd)
    mapped = struct.unpack_from('=I', buf, 20)[0]
    for i in range(mapped):
        logical, physical, length = struct.unpack_from('=QQQ', buf, 32 + 56 * i)
        if logical <= off and off + n <= logical + length:
            return f'{(physical + off - logical) // 512} {n // 512}'
    return None


def main(argv):
    try:
        cmd, path = argv[1], argv[2]
        nums = [int(x, 0) for x in argv[3:]]
        if cmd in ('fill', 'write', 'dirty', 'direct', 'extent') and (nums[0] % BLOCK or nums[1] % BLOCK or not nums[1]):
            raise ValueError('offset and length are whole blocks')
        if cmd == 'fill':
            out = fill(path, nums[0], nums[1], nums[2])
        elif cmd == 'write':
            out = write(path, nums[0], nums[1], nums[2])
        elif cmd == 'dirty':
            out = write(path, nums[0], nums[1], nums[2], sync=False)
        elif cmd == 'direct':
            out = direct(path, nums[0], nums[1])
        elif cmd == 'sync':
            out = sync(path)
        elif cmd == 'extent':
            out = extent(path, nums[0], nums[1])
            if out is None:
                print('the range is not in one extent', file=sys.stderr)
                return 2
        else:
            raise ValueError(f'no command {cmd}')
    except (IndexError, ValueError) as e:
        print(f'{e}\n{__doc__}', file=sys.stderr)
        return 2
    except OSError as e:
        print(f'{cmd}: {say_error(e)}', file=sys.stderr)
        return 2
    print(out)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
