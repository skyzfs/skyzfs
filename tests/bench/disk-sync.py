"""How fast the benchmark's work disk takes a sync after much was written.

    python3 -I tests/bench/disk-sync.py DIR [MIB]

Writes MIB (default 64) MiB to a new file under DIR through the page cache
and times the fdatasync that follows; then the same amount with O_DIRECT,
1 MiB a write, and its fdatasync. Prints one line of `key=value` fields,
with the machine (boot id and host name, as the benchmark's rows carry it)
so that a summary can set the figures beside the rows of that machine.

The daemon's files sit on this disk. Its syncs wait for what the page cache
holds of the file since the last sync (M1.T10, M1.3j: a home sync of 8 MiB
took 36 to 46 ms on some runners and 3.6 to 4.2 ms on others), and this
says at the start of a job which kind of machine it is. An observation:
nothing judges it.
"""
import mmap
import os
import socket
import sys
import time

MIB = 1 << 20


def machine():
    try:
        boot = open('/proc/sys/kernel/random/boot_id').read().strip()
    except OSError:
        boot = 'unknown'
    return f'{boot} {socket.gethostname()}'


def timed_write(path, total, direct):
    flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC | (os.O_DIRECT if direct else 0)
    fd = os.open(path, flags, 0o600)
    try:
        buf = mmap.mmap(-1, MIB)  # page-aligned, as O_DIRECT needs
        buf.write(os.urandom(MIB))
        t0 = time.monotonic_ns()
        for _ in range(total):
            if os.write(fd, buf) != MIB:
                raise OSError('short write')
        t1 = time.monotonic_ns()
        os.fdatasync(fd)
        t2 = time.monotonic_ns()
        return (t1 - t0) / 1e6, (t2 - t1) / 1e6
    finally:
        os.close(fd)
        os.unlink(path)


def main():
    where = sys.argv[1]
    total = int(sys.argv[2]) if len(sys.argv) > 2 else 64
    os.makedirs(where, exist_ok=True)
    path = os.path.join(where, f'disk-sync.{os.getpid()}')
    fields = {'machine': machine().replace(' ', '_'), 'mib': total}
    for name, direct in (('buffered', False), ('direct', True)):
        try:
            write_ms, sync_ms = timed_write(path, total, direct)
            fields[f'{name}_write_ms'] = f'{write_ms:.1f}'
            fields[f'{name}_sync_ms'] = f'{sync_ms:.1f}'
            fields[f'{name}_sync_mib_s'] = f'{total / (sync_ms / 1e3):.0f}' if name == 'buffered' and sync_ms > 0 else ''
            fields[f'{name}_all_mib_s'] = f'{total / ((write_ms + sync_ms) / 1e3):.0f}'
        except OSError as e:
            fields[f'{name}_error'] = str(e).replace(' ', '_')
    print('disk-sync ' + ' '.join(f'{k}={v}' for k, v in fields.items() if v != ''))


if __name__ == '__main__':
    main()
