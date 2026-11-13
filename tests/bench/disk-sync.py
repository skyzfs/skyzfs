"""How fast the benchmark's work disk takes a sync after much was written.

    python3 -I tests/bench/disk-sync.py DIR [MIB [IMAGE]]

Also says which block device DIR is on, its queue's stable_writes, and
its queue's write_cache (write_back when the disk declares a volatile
cache, so that the kernel sends it flushes; write_through when it does
not, and then a flush is completed without reaching the disk), and
the file system DIR is on: its type, the mount's options and the super
block's (`fs`, `mount`, `super`, from /proc/self/mountinfo; a mount with nobarrier or a file system that
sends no flushes would explain syncs that cost nothing).
Writes MIB (default 64) MiB to a new file under DIR through the page cache
and times the fdatasync that follows; then the same amount with O_DIRECT,
1 MiB a write, and its fdatasync. Prints one line of `key=value` fields,
with the machine (boot id and host name, as the benchmark's rows carry it)
so that a summary can set the figures beside the rows of that machine,
and the machine's architecture and kernel (`arch`, `kernel`, from
uname) and the runner image IMAGE names (`image`; the workflow passes
the runner's ImageOS and ImageVersion as an argument, since sudo-rs does
not carry the job's environment).

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


def queue_of(where):
    """The block device under DIR, its queue's stable_writes (1 when a
    page under writeback must not change, so that a write into it waits
    until the writeback has ended: M1.3m's open question) and its
    queue's write_cache."""
    try:
        st = os.stat(where).st_dev
        dev = os.path.realpath(f'/sys/dev/block/{os.major(st)}:{os.minor(st)}')
    except OSError:
        return 'unknown', 'unknown', 'unknown'
    for d in (dev, os.path.dirname(dev)):  # a partition's queue is its disk's
        try:
            stable = open(os.path.join(d, 'queue', 'stable_writes')).read().strip()
        except OSError:
            continue
        try:
            cache = open(os.path.join(d, 'queue', 'write_cache')).read().strip().replace(' ', '_')
        except OSError:
            cache = 'not said'
        return os.path.basename(dev), stable, cache
    return os.path.basename(dev), 'not said', 'not said'


def mount_of(where, mountinfo='/proc/self/mountinfo'):
    """The mount DIR is on: (type, the mount's options, the super block's)."""
    try:
        st = os.stat(where).st_dev
        lines = open(mountinfo).read().splitlines()
    except OSError:
        return 'unknown', 'unknown', 'unknown'
    best = None
    for line in lines:
        f = line.split()
        if ' - ' not in line or len(f) < 6:
            continue
        pre, post = line.split(' - ', 1)
        pf, qf = pre.split(), post.split()
        if len(qf) < 3:
            continue
        try:
            major, minor = (int(x) for x in pf[2].split(':'))
        except ValueError:
            continue
        if os.makedev(major, minor) != st:
            continue
        point = pf[4]
        # The deepest mount point that holds DIR (bind mounts share a device).
        real = os.path.realpath(where)
        if (real == point or real.startswith(point.rstrip('/') + '/')) and (best is None or len(point) > len(best[0])):
            best = (point, qf[0], pf[5], qf[2])
    if best is None:
        return 'unknown', 'unknown', 'unknown'
    return best[1:]


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
    image = sys.argv[3] if len(sys.argv) > 3 else ''
    u = os.uname()
    fields = {'machine': machine().replace(' ', '_'), 'mib': total,
              'arch': u.machine, 'kernel': u.release, 'image': image.replace(' ', '_') or 'not_said'}
    fields['dev'], fields['stable_writes'], fields['write_cache'] = queue_of(where)
    fields['fs'], fields['mount'], fields['super'] = mount_of(where)
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
