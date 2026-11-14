#!/usr/bin/env python3
"""The reference for cuts (design §5.2, M2.T1): from a record of the
requests a pool's device was handed, in order, the uberblock writes, the
cuts and the volume at each cut's instant.

The rule, as the design states it:

  - An uberblock write is a write to an uberblock slot of one of the
    four labels that carries the uberblock magic, in either byte order,
    and a transaction group above zero. It counts from the moment it is
    applied. In a dm-log-writes record a write stands where it
    completed, which is after it was applied: its place in the record is
    the moment it counts from.
  - It is a repeat when its transaction group and its root block
    pointer both equal those of the uberblock that marks the cut to
    come, or those of the newest cut. A repeat makes no cut and changes
    no mark. (The second half is for multihost=on: a multihost write
    that still carries N-1 can be applied after N's uberblock writes.)
    A stale write that the rule lets through, after the newest cut,
    makes a cut whose state equals an older cut's and whose group errs
    low (`stale`); its volume is still a complete pool.
  - A cut is the barrier of the first flush the record holds after an
    uberblock write that marked. Its group N is that write's; its number
    is its place in the count of cuts, from 1.
  - The volume at the cut's instant is the device after every record
    before the flush (`prefix`, a count of records). A flush that
    carries data (a write with REQ_PREFLUSH) flushes before its data, so
    its own data is not in the cut.

This module parses uberblocks itself, from the on-disk format, and does
not use the daemon's parser (M2.1) or logwrites.py's 'ub' flag: the
two are held against each other through recorded pools.

On-disk format (OpenZFS, include/sys/vdev_impl.h, uberblock_impl.h,
vdev_label.c):
  - Four labels of 256 KiB (vdev_label_t): L0 at byte 0, L1 at 256 KiB,
    L2 and L3 in the last 512 KiB of the device's size rounded down to
    256 KiB (vdev_label_offset; the size is vdev_psize, aligned so in
    vdev_open).
  - In a label the uberblock ring is the last 128 KiB
    (VDEV_UBERBLOCK_RING), of slots of 2^shift bytes with
    shift = min(max(ashift, 10), 13) (VDEV_UBERBLOCK_SHIFT). Slot
    starts are therefore 1 KiB aligned, and the parser looks at each.
  - struct uberblock: ub_magic (0x00bab10c), ub_version, ub_txg,
    ub_guid_sum, ub_timestamp (five u64), ub_rootbp (a blkptr_t, 128
    bytes, sixteen u64), ub_software_version, ub_mmp_magic
    (0xa11cea11), ub_mmp_delay, ub_mmp_config, ub_checkpoint_txg. The
    byte order is the writer's; the magic says which.

Rule variants for the negative controls (`--rule`):
  design         the rule above
  txg-alone      a repeat is an uberblock whose group was seen before
                 (keyed on the group alone)
  count-repeats  every uberblock write with a group above zero marks
  previous       the rule before 2026-11-14: a repeat equals the
                 uberblock write just before it
  early          as `design`, but the instant is the last flush before
                 the uberblock write (one flush early)

Commands:
  cut_ref.py cuts --log LOG --index INDEX.json [--rule R] --out CUTS.json
  cut_ref.py fixture --log LOG --index INDEX.json --dir DIR [--about TEXT]
The index is the replay rig's (logwrites.py index).
"""
import argparse
import gzip
import json
import os
import struct
import sys

LABEL = 256 << 10
RING_START = 128 << 10          # the ring is the label's last 128 KiB
SLOT_ALIGN = 1 << 10            # the smallest uberblock slot
UB_MAGIC = 0x00bab10c
MMP_MAGIC = 0xa11cea11
UB_BYTES = 8 * 5 + 128 + 8 * 6  # through ub_raidz_reflow_info
FIXTURE_VERSION = 1


def label_offsets(dev_bytes):
    """The byte offsets of the four labels of a device of dev_bytes."""
    psize = dev_bytes // LABEL * LABEL
    if psize < 4 * LABEL:
        raise ValueError(f'a device of {dev_bytes} bytes has no room for four labels')
    return [0, LABEL, psize - 2 * LABEL, psize - LABEL]


def parse_uberblock(buf):
    """The bytes at a slot's start -> the uberblock's fields, or None when
    there is no uberblock magic there."""
    if len(buf) < UB_BYTES:
        return None
    for order in ('<', '>'):
        if struct.unpack_from(order + 'Q', buf, 0)[0] == UB_MAGIC:
            break
    else:
        return None
    w = struct.unpack_from(order + '5Q', buf, 0)
    rootbp = struct.unpack_from(order + '16Q', buf, 40)
    tail = struct.unpack_from(order + '5Q', buf, 168)
    return {'order': 'le' if order == '<' else 'be', 'version': w[1], 'txg': w[2], 'guid_sum': w[3],
            'timestamp': w[4], 'rootbp': rootbp, 'software_version': tail[0],
            'mmp': tail[1] == MMP_MAGIC, 'checkpoint_txg': tail[4]}


def uberblocks_in(offset, data, labels):
    """A write of data at offset -> [(label number, slot offset on the device, uberblock)]."""
    out = []
    end = offset + len(data)
    for k, lo in enumerate(labels):
        ring_lo, ring_hi = lo + RING_START, lo + LABEL
        a, b = max(offset, ring_lo), min(end, ring_hi)
        if a >= b:
            continue
        a += -(a - ring_lo) % SLOT_ALIGN
        for o in range(a, b, SLOT_ALIGN):
            ub = parse_uberblock(data[o - offset:o - offset + UB_BYTES])
            if ub is not None:
                out.append((k, o, ub))
    return out


def touches_label(offset, nbytes, labels):
    return any(offset < lo + LABEL and offset + nbytes > lo for lo in labels)


def is_write(rec):
    return 'pos' in rec and rec.get('bytes', 0) > 0 and not ({'discard', 'mark'} & set(rec['flags']))


def scan(records, read, dev_bytes):
    """Every uberblock in every write -> [dict(record, label, slot, ...fields)], in record order."""
    labels = label_offsets(dev_bytes)
    ubs = []
    for r in records:
        if not is_write(r) or not touches_label(r['offset'], r['bytes'], labels):
            continue
        for k, slot, ub in uberblocks_in(r['offset'], read(r), labels):
            ubs.append(dict(ub, record=r['i'], label=k, slot=slot))
    return ubs


def cuts(records, ubs, rule='design'):
    """Records and their uberblocks (scan) -> (cuts, the uberblocks with how each was taken).

    A cut: n (its number in the count), txg, rootbp, ub_record (the
    write that marked it), flush_record and prefix (the records before
    the instant)."""
    if rule not in ('design', 'txg-alone', 'count-repeats', 'early', 'previous'):
        raise ValueError(f'no rule {rule}')
    by_record = {}
    for u in ubs:
        by_record.setdefault(u['record'], []).append(u)
    out, taken = [], []
    prev = None                 # (txg, rootbp) of the uberblock write before, for `previous`
    newest = None               # (txg, rootbp) of the newest cut
    seen = set()                # groups seen, for txg-alone
    mark = None
    last_flush = None           # the prefix of the last flush's barrier, for `early`
    for r in records:
        flags = set(r['flags'])
        if 'flush' in flags:
            # A flush that carries data flushes before its data.
            if mark is not None:
                prefix = r['i'] if rule != 'early' else mark['early']
                out.append({'n': len(out) + 1, 'txg': mark['txg'], 'rootbp': mark['rootbp'], 'timestamp': mark['timestamp'],
                            'ub_record': mark['record'], 'flush_record': r['i'], 'prefix': prefix})
                newest = (mark['txg'], mark['rootbp'])
                mark = None
            last_flush = r['i']
        for u in by_record.get(r['i'], ()):
            u = dict(u)
            key = (u['txg'], u['rootbp'])
            if u['txg'] == 0:
                u['taken'] = 'group 0'
            elif rule == 'count-repeats':
                u['taken'] = 'marks'
            elif rule == 'txg-alone' and u['txg'] in seen:
                u['taken'] = 'repeat'
            elif rule == 'previous' and key == prev:
                u['taken'] = 'repeat'
            elif rule not in ('txg-alone', 'previous') and (
                    key == newest or (mark is not None and key == (mark['txg'], mark['rootbp']))):
                u['taken'] = 'repeat'
            else:
                u['taken'] = 'marks'
            if u['txg']:
                prev = key
                seen.add(u['txg'])
            if u['taken'] == 'marks':
                mark = {'txg': u['txg'], 'rootbp': u['rootbp'], 'record': u['record'], 'timestamp': u['timestamp'],
                        'early': last_flush if last_flush is not None else 0}
            taken.append(u)
    return out, taken


def stale(cs):
    """The cuts whose pool state (group and root block pointer) equals that
    of a cut before the one just before them -> {n: the older cut's n}.
    Under the design's rule these are multihost writes of an older state
    applied after a newer cut: the volume is a complete pool at a later
    group, and the cut's group errs low."""
    out, first = {}, {}
    for k, c in enumerate(cs):
        key = (c['txg'], c['rootbp'])
        if key in first and first[key] < k - 1:
            out[c['n']] = cs[first[key]]['n']
        first.setdefault(key, k)
    return out


def load(index_path, log_path):
    with open(index_path) as f:
        return load_from(json.load(f), log_path)


def load_from(idx, log_path):
    """An index and its log -> (the index, a reader of a write record's data)."""
    fd = os.open(log_path, os.O_RDONLY)

    def read(rec):
        data = os.pread(fd, rec['bytes'], rec['pos'])
        if len(data) != rec['bytes']:
            raise ValueError(f"the log ends inside the data of record {rec['i']}")
        return data
    return idx, read


def rootbp_hex(rootbp):
    return ''.join(f'{w:016x}' for w in rootbp)


def cut_json(c):
    return dict(c, rootbp=rootbp_hex(c['rootbp']))


def op_name(rec):
    flags = [f for f in rec['flags'] if f != 'metadata']
    if 'mark' in flags:
        return 'mark'
    if 'discard' in flags:
        return 'discard'
    if is_write(rec):
        return '+'.join(['write'] + [f for f in flags if f in ('flush', 'fua')])
    return '+'.join(f for f in flags if f in ('flush', 'fua')) or 'none'


def write_fixture(dirname, idx, read, about=''):
    """A record and its reference cuts as files a test with no kernel reads (format: README.md beside them)."""
    records, dev = idx['records'], idx['dev_bytes']
    labels = label_offsets(dev)
    ubs = scan(records, read, dev)
    cs, taken = cuts(records, ubs)
    os.makedirs(dirname, exist_ok=True)
    head = [f'# skyzfs cut fixture, version {FIXTURE_VERSION}', f'# dev_bytes {dev}',
            f"# sectorsize {idx['sectorsize']}", f'# records {len(records)}']
    head += [f'# about {line}' for line in about.splitlines() if line.strip()]
    with gzip.open(os.path.join(dirname, 'requests.tsv.gz'), 'wt', compresslevel=9) as f:
        f.write('\n'.join(head) + '\n')
        f.write('# i\top\toffset\tbytes\tdata\n')
        for r in records:
            op = op_name(r)
            if op == 'mark':
                data = r.get('mark', '').replace('\t', ' ').replace('\n', ' ') or '-'
                f.write(f"{r['i']}\tmark\t0\t0\t{data}\n")
                continue
            data = '-'
            if is_write(r) and touches_label(r['offset'], r['bytes'], labels):
                data = read(r).hex()
            f.write(f"{r['i']}\t{op}\t{r.get('offset', 0)}\t{r.get('bytes', 0)}\t{data}\n")
    with open(os.path.join(dirname, 'uberblocks.tsv'), 'w') as f:
        f.write('\n'.join(head) + '\n')
        f.write('# record\tlabel\tslot\torder\ttxg\tmmp\ttaken\trootbp\n')
        for u in taken:
            f.write(f"{u['record']}\t{u['label']}\t{u['slot']}\t{u['order']}\t{u['txg']}\t{int(u['mmp'])}\t"
                    f"{u['taken']}\t{rootbp_hex(u['rootbp'])}\n")
    with open(os.path.join(dirname, 'cuts.tsv'), 'w') as f:
        f.write('\n'.join(head) + '\n')
        f.write('# n\ttxg\tprefix\tub_record\tflush_record\trootbp\n')
        for c in cs:
            f.write(f"{c['n']}\t{c['txg']}\t{c['prefix']}\t{c['ub_record']}\t{c['flush_record']}\t{rootbp_hex(c['rootbp'])}\n")
    return cs, taken


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    sub = ap.add_subparsers(dest='cmd', required=True)
    p = sub.add_parser('cuts')
    p.add_argument('--log', required=True)
    p.add_argument('--index', required=True)
    p.add_argument('--rule', default='design')
    p.add_argument('--out', required=True)
    p = sub.add_parser('fixture')
    p.add_argument('--log', required=True)
    p.add_argument('--index', required=True)
    p.add_argument('--dir', required=True)
    p.add_argument('--about', default='')
    a = ap.parse_args()
    try:
        idx, read = load(a.index, a.log)
        if a.cmd == 'cuts':
            ubs = scan(idx['records'], read, idx['dev_bytes'])
            cs, taken = cuts(idx['records'], ubs, a.rule)
            with open(a.out, 'w') as f:
                json.dump({'rule': a.rule, 'cuts': [cut_json(c) for c in cs],
                           'uberblocks': [dict(u, rootbp=rootbp_hex(u['rootbp'])) for u in taken]}, f)
            print(f"{len(cs)} cuts from {sum(1 for u in taken if u['taken'] == 'marks')} marking uberblock writes "
                  f"of {len(taken)} ({a.rule})")
        else:
            cs, taken = write_fixture(a.dir, idx, read, a.about)
            print(f'{len(cs)} cuts, {len(taken)} uberblock writes, into {a.dir}')
    except (OSError, ValueError) as e:
        sys.exit(f'cut_ref.py: {e}')


if __name__ == '__main__':
    main()
