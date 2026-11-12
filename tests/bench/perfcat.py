#!/usr/bin/env python3
"""Where the daemon's system CPU goes, by thread, from `perf record -g` (TB_PERF_RECORD).

    perfcat.py SCRIPT SECONDS > JSON

SCRIPT is `perf script -F comm,tid,ip,sym` of a recording of the
daemon's pid: per sample a line "comm tid", then one line per frame,
leaf first, "ip symbol". A sample is the kernel's when its leaf frame
is a kernel address. Each kernel sample is put in the first category
of CATEGORIES whose rule matches its call chain, so a copy made for
the page cache is counted as a copy, of the page cache, and ZFS's
handling of a finished request (which the kernel runs in the thread that
ends it, the daemon's) is counted as that and not as the wakes it makes.
On a runner VM perf samples by a timer, so a sample taken while
interrupts were off is charged where they are turned back on
(_raw_spin_unlock_irqrestore, finish_task_switch): the kind, from the
whole chain, is the better guide than the leaf symbol. The JSON gives
by thread name (a trailing -N is dropped, so the readers are one):

    {"seconds": 20, "threads": {"skyzfs-reader": {"samples": 1234,
     "kernel": 1000, "categories": {"copy, page cache": 400, ...},
     "symbols": {"copy_user_enhanced_fast_string": 380, ...}}}}

`samples` counts all of the thread's samples, `kernel` those in the
kernel; `symbols` keeps the twenty commonest kernel leaf symbols.
"""
import collections
import json
import re
import sys

KERNEL = 0xffff800000000000
COPY = re.compile(r'^(copy_user|rep_movs|_?copy_(to|from)_user|_copy_(to|from)_iter|copy_page_(to|from)_iter|'
                  r'copy_mc|__?memcpy|memcpy|memmove|__memmove|ublk_copy)')
PAGE_CACHE = re.compile(r'^(filemap_|iomap_|generic_perform_write|generic_file_|xfs_|ext4_|folio|__folio|'
                        r'page_cache|__filemap|writeback|__writeback|sync_file_range|do_writepages|'
                        r'file_write_and_wait|vfs_fsync|__x64_sys_fdatasync|__x64_sys_pread|__x64_sys_pwrite)')
UBLK = re.compile(r'^(ublk_|__ublk)')
WAKE = re.compile(r'^(futex_|__futex|do_futex|eventfd|try_to_wake_up|ttwu_|wake_up|__wake_up|schedule|__schedule|'
                  r'finish_task_switch|enqueue_task|dequeue_task|pick_next_task|select_task_rq|'
                  r'__x64_sys_futex|__x64_sys_eventfd)')
# ZFS's handling of a finished request, which the kernel runs in the
# thread that ends the request: the daemon's, when it commits a result.
ZFS_DONE = re.compile(r'^(zio_|taskq_|spa_taskq|vdev_disk_io_done|vdev_disk_bio_done)')
IO_URING = re.compile(r'^(io_uring|__io_|io_[a-z_]+$|__x64_sys_io_uring)')
# (name, rule over the chain's symbols, leaf first). The first that holds wins.
CATEGORIES = (
    ('copy, ublk', lambda c: COPY.match(c[0]) and any(UBLK.match(s) for s in c)),
    ('copy, page cache', lambda c: COPY.match(c[0]) and any(PAGE_CACHE.match(s) for s in c)),
    ('copy, other', lambda c: COPY.match(c[0])),
    ('ZFS completion', lambda c: any(ZFS_DONE.match(s) for s in c)),
    ('wake and switch', lambda c: any(WAKE.match(s) for s in c)),
    ('page cache', lambda c: any(PAGE_CACHE.match(s) for s in c)),
    ('ublk driver', lambda c: any(UBLK.match(s) for s in c)),
    ('io_uring', lambda c: any(IO_URING.match(s) for s in c)),
    ('other', lambda c: True),
)
# perf script pads the comm on the left and indents the frames.
HEADER = re.compile(r'^\s*(\S.*?)\s+(\d+)\s*$')
FRAME = re.compile(r'^\s+([0-9a-f]{8,16})\s+(\S+)')


def name_of(comm):
    """A thread's name without a trailing -N (skyzfs-reader-2 -> skyzfs-reader)."""
    return re.sub(r'-\d+$', '', comm)


def samples(text):
    """`perf script` -> [(comm, [(ip, symbol), ...])], leaf first."""
    out, cur = [], None
    for line in text.splitlines():
        if not line.strip():
            cur = None
            continue
        m = FRAME.match(line)
        if m and cur is not None:
            sym = m.group(2).split('+')[0]
            cur[1].append((int(m.group(1), 16), sym))
            continue
        m = HEADER.match(line)
        if m:
            cur = (m.group(1), [])
            out.append(cur)
    return out


def category(chain):
    for name, rule in CATEGORIES:
        if rule(chain):
            return name
    return 'other'


def summarize(text):
    threads = {}
    for comm, frames in samples(text):
        t = threads.setdefault(name_of(comm), {'samples': 0, 'kernel': 0, 'categories': collections.Counter(),
                                                 'symbols': collections.Counter()})
        t['samples'] += 1
        if not frames or frames[0][0] < KERNEL:
            continue
        chain = [s for _, s in frames]
        t['kernel'] += 1
        t['categories'][category(chain)] += 1
        t['symbols'][chain[0]] += 1
    for t in threads.values():
        t['categories'] = dict(t['categories'])
        t['symbols'] = dict(t['symbols'].most_common(20))
    return threads


def main():
    with open(sys.argv[1], errors='replace') as f:
        text = f.read()
    json.dump({'seconds': float(sys.argv[2]), 'threads': summarize(text)}, sys.stdout)


if __name__ == '__main__':
    main()
