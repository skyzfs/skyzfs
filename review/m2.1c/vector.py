#!/usr/bin/env python3
"""Known answers for the label checksum, from the OpenZFS 2.4.1 sources alone.

A 1 KiB slot at 0x20400 (label 0's ring, slot 1): the uberblock magic and group 7, zeros
after, as a little-endian and as a big-endian host writes it (zio_checksum_compute with
ZIO_CHECKSUM_LABEL; abd_checksum_sha256's BE_64 of the digest's words).
"""
import hashlib
import struct

AT = 0x20400
SIZE = 1024
UB_MAGIC = 0x00bab10c
ZEC_MAGIC = 0x0210da7ab10c7a11

for name, e in (('little', '<'), ('big', '>')):
    slot = bytearray(SIZE)
    struct.pack_into(e + 'Q', slot, 0, UB_MAGIC)
    struct.pack_into(e + 'Q', slot, 16, 7)
    eck = SIZE - 40
    # the host writes the magic and the verifier natively, then hashes the whole slot
    struct.pack_into(e + 'Q', slot, eck, ZEC_MAGIC)
    struct.pack_into(e + '4Q', slot, eck + 8, AT, 0, 0, 0)
    digest = hashlib.sha256(bytes(slot)).digest()
    # zc_word[n] = BE_64(tmp.zc_word[n]): the digest's bytes read big endian, as a number
    words = struct.unpack('>4Q', digest)
    # ... written natively
    struct.pack_into(e + '4Q', slot, eck + 8, *words)
    print(name, 'words:', ', '.join(f'0x{w:016x}' for w in words))
    print(name, 'last 32 bytes:', bytes(slot[-32:]).hex())
    print(name, 'digest:', digest.hex())
