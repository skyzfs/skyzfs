#!/usr/bin/env python3
"""#115 at afd217a and at fe6970c: the two judged floors on real recordings.

    python3 -I floors.py CHECKOUT ARTIFACT_DIR...

For each artifact's `takeover`:
  - what held_across_start counts, and how near a right daemon came to
    zero: the cut held across the first start, its group and the group
    the first instance reached, when the first block of its map changed
    in the home (ms after the cut's instant, ms before the next start,
    groups after the cut), and where the counted blocks lie;
  - of the blocks of its map that a write touched in that span, how many
    a write changed at any time (what afd217a counts), how many held the
    cut's content again at the next start (Codex's remark on #115), and
    how many were last touched by a write of no known content or by a
    write-zeroes (what fe6970c calls not known and does not count);
  - the same count by the method of the review of #111 (the blocks whose
    last content before the next start differs from the cut's);
  - T1: a daemon that took the cut up without the record of its copies
    (each counted block given its later content in the holder's hashes);
  - T2: a daemon that let go of what it held at the start (the cut held
    across the start is not written; the holder goes on from the next,
    whose map then has the lost cut's blocks too, as they were at its
    instant);
  - T3: no block of the cut changed before the start (the writes that
    changed them taken out of the count: the cut's instant moved to the
    record before the start, by shortening nothing in the log; done by
    giving held_across_start a map without the counted blocks).
For each artifact's `failed`: carried_across_failure, judged here.
The artifact is not changed: each scenario works on a copy."""
import os
import shutil
import subprocess
import sys
import tempfile

root, arts = sys.argv[1], sys.argv[2:]
sys.path.insert(0, os.path.join(root, 'tests/kernel'))
import cut_daemon_check as c  # noqa: E402

JUDGE = os.path.join(root, 'tests/kernel/cut_daemon_check.py')
SHOWN = ('held_content', 'held_across_start', 'carried_across_failure', 'cuts_held', 'held_reads', 'cuts',
         'cut_numbers', 'status', 'rig_error')


def rd(d, name):
    with open(os.path.join(d, name)) as f:
        return f.read()


def wr(d, name, text):
    with open(os.path.join(d, name), 'w') as f:
        f.write(text)


def judge(d, name='takeover'):
    if os.path.exists(os.path.join(d, 'results.tsv')):
        os.remove(os.path.join(d, 'results.tsv'))
    p = subprocess.run([sys.executable, '-I', '-B', JUDGE, name, d, os.path.join(d, 'requests.tsv'), '-',
                        str(512 << 20)], capture_output=True, text=True, timeout=1500)
    lines = {}
    for ln in rd(d, 'results.tsv').splitlines():
        f = ln.split('\t')
        lines[f[0]] = (f[1], f[2])
    return p.returncode, lines


def show(title, rc, lines):
    print(f'  {title}: exit {rc}')
    for k in SHOWN:
        if k in lines and (lines[k][0] not in ('PASS',) or k == 'held_content'):
            print(f'      {k}\t{lines[k][0]}\t{lines[k][1][:300]}')


def where(b, dev_blocks):
    off = b * c.BLOCK
    label = 256 << 10
    if off < 2 * label:
        return f'label {off // label}' + (' (uberblock ring)' if off % label >= 128 << 10 else '')
    if off >= dev_blocks * c.BLOCK - 2 * label:
        k = 2 + (off - (dev_blocks * c.BLOCK - 2 * label)) // label
        return f'label {k}' + (' (uberblock ring)' if (off - (dev_blocks * c.BLOCK - 2 * label)) % label
                               >= 128 << 10 else '')
    return 'the pool\'s space'


for art in arts:
    tag = os.path.basename(os.path.normpath(art))
    src = os.path.join(art, 'takeover')
    print(f'== {tag}')
    tmp = tempfile.mkdtemp(prefix='rv-floors-')
    try:
        # as the program's main has it
        torn = []
        inst = c.parse_log(rd(src, 'requests.tsv'), torn)
        kinds = c.read_kinds(rd(src, 'instances.tsv'))
        acked = c.read_acknowledged(rd(src, 'instances.tsv'))
        marks = c.read_marks(rd(src, 'marks.tsv'))
        inst, unstored = c.unstored_at_kill(inst, acked)
        records, dcuts = c.records_of(c.log_lines(inst, kinds, acked), marks)
        c.epochs(dcuts, records)
        print(f'  lines torn at a kill: {len(torn)}; flushes unstored at a kill: {len(unstored)}')
        starts = [r for r in records if 'instance' in r['flags']]
        by_number = {int(d['number']): d for d in dcuts if d['number'] != '-'}
        held = [ln.split('\t') for ln in rd(src, 'held.tsv').splitlines()]
        blocks = {}
        for ln in rd(src, 'held-blocks.tsv').splitlines():
            n, b, h = ln.split('\t')
            blocks.setdefault(int(n), {})[int(b)] = h
        written = [{'number': int(h[0]), 'asked': int(h[4])} for h in held]
        got = c.held_across_start(records, dcuts, written, blocks)
        print(f'  held_across_start, by the check: {got}')
        dev_blocks = (512 << 20) // c.BLOCK
        for n, counted, wrote, nb in got:
            if not nb:
                print(f'  cut {n}, counted by instance {counted}, written by instance {wrote}: no block counted')
                continue
            d = by_number[n]
            end = starts[counted - 1]
            cut_ns = d['ns']
            mine = blocks[n]
            first, last, hit = {}, {}, set()
            groups_at = {}
            cuts_before = [x for x in dcuts if x['prefix'] <= d['prefix']]
            cuts_between = [x for x in dcuts if d['prefix'] < x['prefix'] <= end['i']]
            unknown = set()
            for r in records[d['prefix'] + 1:end['i']]:
                zeroes = r.get('op') == 'write_zeroes'
                if c.cut_ref.is_write(r) or zeroes:
                    hs = None if zeroes else r.get('hashes')
                    b0 = r['offset'] // c.BLOCK
                    for k, b in enumerate(range(b0, (r['offset'] + r['bytes'] - 1) // c.BLOCK + 1)):
                        if b in mine:
                            if hs is None or hs[k] is None or hs[k] != mine[b]:
                                hit.add(b)
                                if b not in first:
                                    first[b] = r
                                    groups_at[b] = sum(1 for x in cuts_between if x['prefix'] <= r['i'])
                            last[b] = None if hs is None else hs[k]
                            (unknown.add if last[b] is None else unknown.discard)(b)
            restored = sorted(b for b in hit if last.get(b) == mine[b])
            by_last = sum(1 for b in hit if last[b] is not None and last[b] != mine[b])
            assert nb in (len(hit), by_last), (nb, len(hit), by_last)
            lie = {}
            for b in hit:
                lie[where(b, dev_blocks)] = lie.get(where(b, dev_blocks), 0) + 1
            print(f'  cut {n} (group {d["txg"]}), counted by instance {counted}, written by instance {wrote}: '
                  f'{len(mine)} blocks in its map, {nb} counted by the check; they lie in: '
                  f"{', '.join(f'{k}: {v}' for k, v in sorted(lie.items()))}")
            print(f'      the instance counted {len(cuts_between)} more cuts before the next start (to group '
                  f"{cuts_between[-1]['txg'] if cuts_between else d['txg']})")
            if first:
                fr = min(first.values(), key=lambda r: r['i'])
                fb = min(first, key=lambda b: first[b]['i'])
                print(f"      the first block of the map changed {(fr['ns'] - cut_ns) / 1e6:.0f} ms after the cut's "
                      f"instant and {(end['ns'] - fr['ns']) / 1e6:.0f} ms before the next start, "
                      f'{groups_at[fb]} cuts after it; by blocks, cuts after the cut at the first change: '
                      f'{sorted(groups_at.values())}')
            print(f'      blocks of the map a write changed in that span: {len(hit)}; of them, holding the cut\'s '
                  f'content again at the next start: {len(restored)}; last touched by a write of no known content '
                  f'or a write-zeroes: {len(unknown)}; by their last known content: {by_last}')
            # the #111 review's count: blocks whose content at the next start is not the cut's
            model, at = {}, 0
            while at < d['prefix']:
                c.apply(model, records[at])
                at += 1
            changed = {}
            for r in records[d['prefix']:end['i']]:
                if not c.cut_ref.is_write(r) or r.get('hashes') is None:
                    continue
                b0 = r['offset'] // c.BLOCK
                for j, h in enumerate(r['hashes']):
                    if h is not None and h != model.get(b0 + j, c.ZERO):
                        changed[b0 + j] = h
            lost = {b: h for b, h in changed.items() if b in mine and h != mine[b]}
            print(f"      by the review of #111's count (the content at the next start differs from the cut's): "
                  f'{len(lost)}')

            # T1: the cut taken up without the record of its copies
            t1 = os.path.join(tmp, f't1-{n}')
            shutil.copytree(src, t1)
            out = []
            for ln in rd(t1, 'held-blocks.tsv').splitlines():
                m, b, h = ln.split('\t')
                if int(m) == n and int(b) in lost:
                    h = lost[int(b)]
                out.append(f'{m}\t{b}\t{h}\n')
            wr(t1, 'held-blocks.tsv', ''.join(out))
            rc, lines = judge(t1)
            show(f'T1 cut {n} written without the copies instance {counted} made ({len(lost)} blocks as they were '
                 'at the next start)', rc, lines)

            # T2: what was held is let go at the start; the holder goes on from the
            # next cut it writes, whose map then covers the lost cut's too
            t2 = os.path.join(tmp, f't2-{n}')
            shutil.copytree(src, t2)
            nxt = next(int(h[0]) for h in held if int(h[0]) > n)
            dn = by_number[nxt]
            while at < dn['prefix']:
                c.apply(model, records[at])
                at += 1
            more = {b: model.get(b, c.ZERO) for b in mine if b not in blocks[nxt]}
            rows = sorted(list(blocks[nxt].items()) + list(more.items()))
            out = []
            for ln in rd(t2, 'held-blocks.tsv').splitlines():
                m = int(ln.split('\t')[0])
                if m == nxt:
                    out += [f'{nxt}\t{b}\t{h}' for b, h in rows]
                    rows = []
                elif m != n:
                    out.append(ln)
            wr(t2, 'held-blocks.tsv', ''.join(ln + '\n' for ln in out))
            wr(t2, 'held.tsv', ''.join('\t'.join(h[:3] + [str(len(blocks[nxt]) + len(more))] + h[4:]
                                                 if int(h[0]) == nxt else h) + '\n'
                                       for h in held if int(h[0]) != n))
            rc, lines = judge(t2)
            show(f'T2 cut {n} let go at the start and never written; cut {nxt}, the next written, has its {len(more)} '
                 'other blocks too (the status files as recorded)', rc, lines)

            # T3: the map without the blocks that changed before the start
            less = dict(blocks)
            less[n] = {b: h for b, h in mine.items() if b not in hit}
            print(f'  T3 held_across_start with no block of cut {n} changed before the start: '
                  f'{c.held_across_start(records, dcuts, written, less)} (the program takes blocks 0 as none: ERROR)')
        if not got:
            print('  no cut written across a start')
        fd = os.path.join(tmp, 'failed')
        shutil.copytree(os.path.join(art, 'failed'), fd)
        rc, lines = judge(fd, 'failed')
        print(f'  failed, judged here: exit {rc}')
        for k in ('carried_across_failure', 'held_content', 'instance_2'):
            print(f'      {k}\t{lines[k][0]}\t{lines[k][1][:300]}')
    finally:
        shutil.rmtree(tmp)
