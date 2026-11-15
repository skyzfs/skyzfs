#!/usr/bin/env python3
"""The label checksum of every uberblock slot in the recorded pools, computed from the
OpenZFS 2.4.1 sources alone (zio_checksum_compute / zio_checksum_error_impl with
ZIO_CHECKSUM_LABEL, abd_checksum_sha256), with no code of the pull request.

    label_cksum.py FIXTURES_DIR

For each write that touches a ring, at each whole KiB where the uberblock magic is: which
slot sizes (1, 2, 4, 8 KiB), aligned in the ring and held whole by the write, verify.
"""
import gzip
import hashlib
import os
import struct
import sys
from collections import Counter

LABEL = 256 << 10
RING_OFF = 128 << 10
RING = 128 << 10
UB_MAGIC = 0x00bab10c
ZEC_MAGIC = 0x0210da7ab10c7a11


def rings(dev_bytes):
    psize = dev_bytes - dev_bytes % LABEL
    out = []
    for l in range(4):
        at = l * LABEL if l < 2 else psize - (4 - l) * LABEL
        out.append((l, at + RING_OFF))
    return out


def verifies(slot, offset):
    """zio_checksum_error_impl on a little-endian host."""
    size = len(slot)
    eck = size - 40
    magic_native = struct.unpack_from('<Q', slot, eck)[0]
    byteswap = magic_native == struct.unpack('<Q', struct.pack('>Q', ZEC_MAGIC))[0]
    # the verifier (offset, 0, 0, 0), as the host holds it, swapped if byteswap
    verifier = struct.pack('<4Q', offset, 0, 0, 0)
    if byteswap:
        verifier = struct.pack('>4Q', offset, 0, 0, 0)
    expected = struct.unpack_from('<4Q', slot, eck + 8)
    digest = hashlib.sha256(slot[:eck + 8] + verifier).digest()
    actual = struct.unpack('>4Q', digest)          # BE_64 of the digest's words
    if byteswap:
        expected = struct.unpack_from('>4Q', slot, eck + 8)
    return actual == expected, magic_native == ZEC_MAGIC, byteswap


def main(root):
    for pool in sorted(os.listdir(root)):
        path = os.path.join(root, pool, 'requests.tsv.gz')
        if not os.path.exists(path):
            continue
        dev_bytes = None
        sizes = Counter()
        magics = Counter()
        none = []
        group0 = 0
        multi = 0
        with gzip.open(path, 'rt') as f:
            for line in f:
                if line.startswith('#'):
                    if line.startswith('# dev_bytes'):
                        dev_bytes = int(line.split()[2])
                    continue
                i, op, offset, nbytes, data = line.rstrip('\n').split('\t')
                if not op.startswith('write') or data == '-':
                    continue
                offset = int(offset)
                data = bytes.fromhex(data)
                assert len(data) == int(nbytes)
                end = offset + len(data)
                for label, start in rings(dev_bytes):
                    lo, hi = max(offset, start), min(end, start + RING)
                    at = -(-lo // 1024) * 1024
                    while at + 208 <= hi:
                        b = data[at - offset:]
                        le = struct.unpack_from('<Q', b, 0)[0] == UB_MAGIC
                        be = struct.unpack_from('>Q', b, 0)[0] == UB_MAGIC
                        if le or be:
                            txg = struct.unpack_from('<Q' if le else '>Q', b, 16)[0]
                            if txg == 0:
                                group0 += 1
                            ok = []
                            for size in (1024, 2048, 4096, 8192):
                                if (at - start) % size == 0 and at + size <= hi:
                                    good, magic, swap = verifies(b[:size], at)
                                    if good:
                                        ok.append(size)
                                        magics['zec le' if magic else ('zec be' if swap else 'no zec magic')] += 1
                            if not ok:
                                none.append((int(i), label, at, txg, len(data)))
                            else:
                                sizes[ok[0]] += 1
                                if len(ok) > 1:
                                    multi += 1
                        at += 1024
        print(f'{pool}: verified by first size {dict(sizes)}, magics {dict(magics)}, '
              f'more than one size {multi}, group 0 {group0}, not verified {len(none)} {none[:5]}')


if __name__ == '__main__':
    main(sys.argv[1])
