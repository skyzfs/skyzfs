#!/usr/bin/env python3
"""#103 at d905bd4: do the recordings show a held cut read from its copies?

    copies_shown.py CHECKOUT ARTIFACT_DIR [RECORDING ...]

`held_content` holds each written cut's blocks to the reference at the cut's instant. A daemon
that made no copy before overwriting a held cut's block would differ only in a block that was
overwritten with other content between the cut's instant and the moment the daemon read it for
the write. This counts those blocks from the run's own files: for each cut the holder wrote
(held.tsv, with the times its write began and ended), the blocks of its map that a later write
in the request log changed

- before the write command was sent: the daemon read them after the overwrite, so what it wrote
  came from a copy (or the check would have failed);
- while the write ran: from a copy or from the image, by the order of the two.

The request log's times, the marks' and the holder's are all the monotonic clock's nanoseconds.
Uses the pull request's own parser (CHECKOUT/tests/kernel/cut_daemon_check.py).
"""
import os
import sys

ROOT, ART = sys.argv[1], sys.argv[2]  # ROOT may also be a copy of the tree that holds tests/kernel
sys.path.insert(0, os.path.join(ROOT, 'tests', 'kernel'))
import cut_daemon_check as c  # noqa: E402

RECS = sys.argv[3:] or sorted(d for d in os.listdir(ART) if os.path.isdir(os.path.join(ART, d)))


def one(name):
    rdir = os.path.join(ART, name)
    with open(os.path.join(rdir, 'requests.tsv')) as f:
        inst = c.parse_log(f.read())[0]
    with open(os.path.join(rdir, 'marks.tsv')) as f:
        marks = c.read_marks(f.read())
    records, dcuts = c.records_of(inst['lines'], marks)
    # The time of each write record: its applied line's, in order.
    applied = [ln['ns'] for ln in inst['lines'] if ln['event'] == 'applied']
    writes = [r for r in records if r['flags'] == [] and 'tag' in r]
    assert len(applied) == len(writes), (len(applied), len(writes))
    for r, ns in zip(writes, applied):
        r['ns'] = ns
    by_number = {int(d['number']): k for k, d in enumerate(dcuts) if d['number'] != '-'}
    held = []
    with open(os.path.join(rdir, 'held.tsv')) as f:
        for ln in f:
            n, group, opens, blocks, t0, t1 = ln.split('\t')
            held.append((int(n), int(t0), int(t1)))
    blocks = c.read_hashes(open(os.path.join(rdir, 'held-blocks.tsv')).read(), 'held-blocks.tsv')
    model, at = {}, 0
    before = during = 0
    lines = []
    for n, t0, t1 in held:
        d = dcuts[by_number[n]]
        while at < d['prefix']:
            c.apply(model, records[at])
            at += 1
        in_map = blocks[n]
        b4, dur = set(), set()
        for r in records[d['prefix']:]:
            if 'ns' not in r:
                continue
            if r['ns'] >= t1:
                break
            hs = r.get('hashes') or []
            first = r['offset'] // c.BLOCK
            for k, h in enumerate(hs):
                b = first + k
                if b in in_map and h is not None and h != model.get(b, c.ZERO):
                    (b4 if r['ns'] < t0 else dur).add(b)
        dur -= b4
        wrong = [b for b in b4 | dur if c.same(model.get(b, c.ZERO), in_map[b]) is False]
        before += len(b4)
        during += len(dur)
        lines.append(f'cut {n}: {len(in_map)} blocks in its map; changed after its instant and before the write '
                     f'was sent {len(b4)}, while it ran {len(dur)}; of those, not as at the instant: {len(wrong)}; '
                     f'the write was sent {(t0 - d["ns"]) / 1e6:.0f} ms after the instant and took {(t1 - t0) / 1e6:.0f} ms')
    print(f'{name}: {len(held)} cuts written; blocks changed after the instant and before the write was sent: '
          f'{before}; while it ran: {during}')
    for ln in lines:
        print('   ', ln)


for name in RECS:
    one(name)
