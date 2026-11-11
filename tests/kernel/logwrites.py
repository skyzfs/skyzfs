#!/usr/bin/env python3
"""Read and replay a dm-log-writes log: the image provider of the replay rig.

dm-log-writes sits between a filesystem and its device and records every
write, flush, discard and mark, in the order they completed, on a second
device. This tool reads that log and reproduces the device as it was
after any number of records.

The replay rig (replay-check.py) talks to an image provider through two
commands, and this is the reference implementation of both:

    logwrites.py index   --log LOG --dev-bytes N --out INDEX.json
        Describe the log: one entry per record, in order (see index()).

    logwrites.py advance --log LOG --index INDEX.json --image IMG
                         --from A --to B [--schedule SCHEDULE.json]
                         [--discard zero|skip]
        Apply records A..B-1 to IMG, which already holds records 0..A-1.
        After a discard the content of the range is unspecified. `zero`
        (the default) makes it zeros, which is the harshest outcome for
        whoever still needs the range; `skip` leaves what was there.
        A schedule is a JSON list of record numbers that replaces the
        natural order 0, 1, 2, ...; A and B then count positions in it.
        The rig's negative controls are schedules: a record left out, or
        a record moved.

A provider for another source of write order (skyzfsd's own write log)
implements the same two commands with the same INDEX.json.

On-disk format (drivers/md/dm-log-writes.c; xfstests src/log-writes):
a superblock in the first block, then entries. An entry is one block
holding a header, followed by nr_sectors blocks of data for a write. A
mark carries its text inside the header block. A discard and a flush
carry no data. Blocks are `sectorsize` bytes, and `sector` counts them.
"""
import argparse
import ctypes
import json
import os
import struct
import sys

MAGIC = 0x6A736677736872
VERSION = 1
FLUSH, FUA, DISCARD, MARK, METADATA = 1, 2, 4, 8, 16
SUPER = struct.Struct('<QQQI')
ENTRY = struct.Struct('<QQQQ')

# A ZFS label is 256 KiB: two at the start of the device and two at the
# end. Its last 128 KiB is the uberblock ring.
LABEL = 256 << 10
UB_RING = 128 << 10
UB_MAGIC = (b'\x0c\xb1\xba\x00\x00\x00\x00\x00', b'\x00\x00\x00\x00\x00\xba\xb1\x0c')


def label_offsets(dev_bytes):
    end = dev_bytes // LABEL * LABEL
    return [0, LABEL, end - 2 * LABEL, end - LABEL]


def read_super(f):
    f.seek(0)
    raw = f.read(SUPER.size)
    if len(raw) < SUPER.size:
        raise ValueError('log is shorter than a superblock')
    magic, version, nr_entries, sectorsize = SUPER.unpack(raw)
    if magic != MAGIC:
        raise ValueError(f'not a dm-log-writes log: magic {magic:#x}')
    if version != VERSION:
        raise ValueError(f'log version {version}, this tool reads {VERSION}')
    if sectorsize < 512 or sectorsize & (sectorsize - 1):
        raise ValueError(f'bad sector size {sectorsize}')
    return nr_entries, sectorsize


def index(log_path, dev_bytes):
    """-> the INDEX: sector size, device size and one dict per record.

    A record has:
      i       its number
      flags   list of 'flush', 'fua', 'discard', 'mark', 'metadata'
      offset  byte offset on the device (writes and discards)
      bytes   length on the device (0 for a flush or a mark)
      pos     byte position of the write's data in the log
      mark    the mark's text
      label   True when the write touches a ZFS label
      ub      True when the write carries a ZFS uberblock
    """
    labels = label_offsets(dev_bytes) if dev_bytes else []
    records = []
    with open(log_path, 'rb') as f:
        nr_entries, ss = read_super(f)
        pos = ss
        for i in range(nr_entries):
            f.seek(pos)
            block = f.read(ss)
            if len(block) < ss:
                raise ValueError(f'log ends inside entry {i} of {nr_entries}')
            sector, nr_sectors, flags, data_len = ENTRY.unpack_from(block)
            if not flags and not nr_sectors:
                raise ValueError(f'entry {i} of {nr_entries} is empty: the log is damaged or was cut short')
            names = [n for bit, n in ((FLUSH, 'flush'), (FUA, 'fua'), (DISCARD, 'discard'),
                                      (MARK, 'mark'), (METADATA, 'metadata')) if flags & bit]
            rec = {'i': i, 'flags': names, 'offset': sector * ss, 'bytes': nr_sectors * ss}
            pos += ss
            if flags & MARK:
                rec['mark'] = block[ENTRY.size:ENTRY.size + data_len].split(b'\0')[0].decode('utf-8', 'replace')
                rec['bytes'] = 0
            elif flags & DISCARD:
                pass
            elif nr_sectors:
                rec['pos'] = pos
                start, end = rec['offset'], rec['offset'] + rec['bytes']
                for lo in labels:
                    if start < lo + LABEL and end > lo:
                        rec['label'] = True
                        # An uberblock starts a 4 KiB slot of the ring.
                        a = max(start, lo + LABEL - UB_RING)
                        a += -(a - lo) % 4096
                        for o in range(a, min(end, lo + LABEL), 4096):
                            f.seek(pos + (o - start))
                            if f.read(8) in UB_MAGIC:
                                rec['ub'] = True
                                break
                pos += rec['bytes']
            records.append(rec)
    return {'sectorsize': ss, 'dev_bytes': dev_bytes, 'records': records}


def zero_range(fd, offset, length):
    """Make a range of the image read as zeros, without filling a sparse file."""
    try:
        libc = ctypes.CDLL(None, use_errno=True)
        libc.fallocate.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_longlong, ctypes.c_longlong]
        if libc.fallocate(fd, 0x1 | 0x2, offset, length) == 0:      # KEEP_SIZE | PUNCH_HOLE
            return
    except (OSError, AttributeError):
        pass
    step = 1 << 20
    for o in range(offset, offset + length, step):
        os.pwrite(fd, bytes(min(step, offset + length - o)), o)


def advance(log_path, idx, image_path, a, b, schedule=None, discard='zero'):
    """Apply schedule[a:b] (records a..b-1 without a schedule) to the image.

    -> (writes applied, bytes written, discards seen). After a discard
    the content of the range is unspecified: `discard` is 'zero' to make
    it zeros, or 'skip' to leave what was there.
    """
    if discard not in ('zero', 'skip'):
        raise ValueError(f'discard must be zero or skip, not {discard}')
    records = idx['records']
    order = schedule if schedule is not None else range(len(records))
    if not 0 <= a <= b <= len(order):
        raise ValueError(f'range {a}..{b} is outside the {len(order)} records')
    writes = nbytes = discards = 0
    with open(log_path, 'rb') as log, open(image_path, 'r+b') as img:
        logfd, imgfd = log.fileno(), img.fileno()
        for k in range(a, b):
            rec = records[order[k]]
            if 'discard' in rec['flags']:
                discards += 1
                if discard == 'zero' and rec['bytes']:
                    zero_range(imgfd, rec['offset'], rec['bytes'])
                continue
            if 'pos' not in rec:
                continue
            data = os.pread(logfd, rec['bytes'], rec['pos'])
            if len(data) != rec['bytes']:
                raise ValueError(f"log ends inside the data of record {rec['i']}")
            os.pwrite(imgfd, data, rec['offset'])
            writes += 1
            nbytes += len(data)
    return writes, nbytes, discards


def write_log(path, sectorsize, entries):
    """Write a log in the kernel's format (for tests). entries: tuples
    ('write', offset, data) | ('flush',) | ('mark', text) | ('discard', offset, length)."""
    with open(path, 'wb') as f:
        f.write(SUPER.pack(MAGIC, VERSION, len(entries), sectorsize).ljust(sectorsize, b'\0'))
        for e in entries:
            if e[0] == 'write':
                assert e[1] % sectorsize == 0 and len(e[2]) % sectorsize == 0 and e[2]
                f.write(ENTRY.pack(e[1] // sectorsize, len(e[2]) // sectorsize, 0, 0).ljust(sectorsize, b'\0'))
                f.write(e[2])
            elif e[0] == 'flush':
                f.write(ENTRY.pack(0, 0, FLUSH, 0).ljust(sectorsize, b'\0'))
            elif e[0] == 'mark':
                text = e[1].encode()
                f.write((ENTRY.pack(0, 0, MARK, len(text)) + text).ljust(sectorsize, b'\0'))
            elif e[0] == 'discard':
                f.write(ENTRY.pack(e[1] // sectorsize, e[2] // sectorsize, DISCARD, 0).ljust(sectorsize, b'\0'))
            else:
                raise ValueError(e[0])


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    sub = ap.add_subparsers(dest='cmd', required=True)
    p = sub.add_parser('index')
    p.add_argument('--log', required=True)
    p.add_argument('--dev-bytes', type=int, default=0)
    p.add_argument('--out', required=True)
    p = sub.add_parser('advance')
    p.add_argument('--log', required=True)
    p.add_argument('--index', required=True)
    p.add_argument('--image', required=True)
    p.add_argument('--from', dest='a', type=int, required=True)
    p.add_argument('--to', dest='b', type=int, required=True)
    p.add_argument('--schedule')
    p.add_argument('--discard', choices=('zero', 'skip'), default='zero')
    args = ap.parse_args()
    try:
        if args.cmd == 'index':
            idx = index(args.log, args.dev_bytes)
            with open(args.out, 'w') as f:
                json.dump(idx, f)
            print(f"{len(idx['records'])} records, sector size {idx['sectorsize']}")
        else:
            with open(args.index) as f:
                idx = json.load(f)
            schedule = None
            if args.schedule:
                with open(args.schedule) as f:
                    schedule = json.load(f)
            w, n, d = advance(args.log, idx, args.image, args.a, args.b, schedule, args.discard)
            print(f"{w} writes, {n} bytes, {d} discards {'zeroed' if args.discard == 'zero' else 'skipped'}")
    except (ValueError, OSError) as e:
        sys.exit(f'logwrites: {e}')


if __name__ == '__main__':
    main()
