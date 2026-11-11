#!/usr/bin/env python3
"""What can zdb tell about a pool image at a prefix of the write log?

An investigation tool for the replay rig's deep check, not a test: it
judges nothing and always exits 0 unless it cannot run at all. It takes
a recording made by `RIG_RECORD_ONLY=1 replay-rig.sh` and answers, with
the full zdb output kept beside the answers:

  - does `zdb -e -bcc` run on the image at an early, a middle and the
    last prefix? With the log device's image beside it, where the pool
    has one? With -AAA?
  - on the images of the negative controls, at a prefix where the
    transaction group's uberblocks are in and the victim is not: what
    does zdb say?
  - which block pointers cover the bytes each control withholds: which
    object, which level, how many copies?
  - do all copies of every block that has several hold the same bytes?
    (A check that needs only zdb's list of block pointers and the
    image.)

    zdb_explore.py --work WORK --pool POOL --out OUT [--slog WORK/slog.img]
"""
import argparse
import gzip
import json
import os
import re
import shutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import logwrites  # noqa: E402
import replay_check  # noqa: E402

LABEL = 0x400000        # a DVA's offset counts from the end of the two front labels
DVA = re.compile(r'DVA\[(\d)\]=<(\d+):([0-9a-f]+):([0-9a-f]+)>')
SIZE = re.compile(r'size=([0-9a-f]+)L/([0-9a-f]+)P')


def block_pointers(text):
    """Lines of `zdb -bbbbb` that describe a block pointer -> list of dicts."""
    out = []
    for line in text.splitlines():
        dvas = DVA.findall(line)
        size = SIZE.search(line)
        if not dvas or not size:
            continue
        out.append({'line': line.strip()[:400], 'gang': ' gang ' in line,
                    'dvas': [(int(v), int(off, 16), int(asize, 16)) for _, v, off, asize in dvas],
                    'psize': int(size.group(2), 16)})
    return out


def covering(bps, lo, hi):
    """Block pointers with a copy that overlaps device bytes [lo, hi)."""
    hits = []
    for bp in bps:
        for k, (_, off, asize) in enumerate(bp['dvas']):
            a = off + LABEL
            if a < hi and lo < a + asize:
                hits.append({'copy': k, 'copies': len(bp['dvas']), 'at': a, 'asize': asize, 'line': bp['line']})
    return hits


def copies_that_differ(image, bps):
    """Block pointers with several copies whose copies do not all hold the same bytes."""
    bad, checked = [], 0
    with open(image, 'rb') as f:
        for bp in bps:
            if len(bp['dvas']) < 2 or bp['gang']:
                continue
            checked += 1
            datas = []
            for _, off, _ in bp['dvas']:
                f.seek(off + LABEL)
                datas.append(f.read(bp['psize']))
            if any(d != datas[0] for d in datas[1:]):
                bad.append({'line': bp['line'], 'differs': [k for k, d in enumerate(datas) if d != datas[0]]})
    return checked, bad


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--work', required=True, help='WORK of a recording: base.img, log.img')
    ap.add_argument('--pool', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--slog', default='', help="image of the pool's log device as it was at the end, if it has one")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    base, log = os.path.join(args.work, 'base.img'), os.path.join(args.work, 'log.img')
    imgdir = os.path.join(args.work, 'explore')
    os.makedirs(imgdir, exist_ok=True)
    image = os.path.join(imgdir, 'replay.img')
    idx = logwrites.index(log, os.path.getsize(base))
    records = idx['records']
    total = len(records)
    marks = replay_check.parse_marks(records)
    flush = replay_check.flush_points(records)
    ctl = {c['name']: c for c in replay_check.controls(records, marks)}
    mid = marks[len(marks) // 2][0]          # the prefix that ends with the middle mark
    res = {'records': total, 'marks': len(marks), 'mid': mid, 'zdb': [], 'controls': {}}
    state = {'at': 0, 'sched': None}

    def image_at(n, schedule=None):
        if schedule is not state['sched'] or n < state['at'] or not os.path.exists(image):
            replay_check.run(['cp', '--sparse=always', base, image])
            state['at'], state['sched'] = 0, schedule
        logwrites.advance(log, idx, image, state['at'], n, schedule)
        state['at'] = n

    def zdb(tag, opts, timeout=300):
        rc, out, t = replay_check.run(['zdb', '-e', '-p', imgdir] + opts + [args.pool], timeout)
        with gzip.open(os.path.join(args.out, f'zdb-{tag}.txt.gz'), 'wt') as f:
            f.write(out)
        said = [ln.strip()[:300] for ln in out.splitlines()
                if re.search(r'ASSERT|VERIFY|PANIC|panic|rror|cannot|leak|checksum|failed', ln)]
        row = {'tag': tag, 'opts': ' '.join(opts), 'rc': rc, 's': round(t, 2), 'lines': out.count('\n'),
               'said': said[:12], 'said_count': len(said)}
        res['zdb'].append(row)
        print(f"zdb {tag}: rc {rc} in {t:.1f}s, {row['lines']} lines; " + ' | '.join(said[:3]), flush=True)
        return rc, out

    # 1. Does zdb run at all, and where?
    for name, n in (('early', flush[min(2, len(flush) - 1)]), ('mid', mid), ('end', total)):
        image_at(n)
        zdb(f'{name}-{n}-bcc', ['-bcc'])
        if name != 'early':
            zdb(f'{name}-{n}-AAA-bcc', ['-AAA', '-bcc'])
    if args.slog:
        # The log device's image exists only as it was at the end.
        image_at(total)
        shutil.copyfile(args.slog, os.path.join(imgdir, 'slog.img'))
        zdb(f'end-{total}-with-log-device-bcc', ['-bcc'])
        os.unlink(os.path.join(imgdir, 'slog.img'))

    # 2. Every block pointer at the middle mark, on the true image.
    image_at(mid)
    rc, dump = zdb(f'mid-{mid}-bbbbb-cc', ['-bbbbb', '-cc'])
    bps = block_pointers(dump)
    checked, bad = copies_that_differ(image, bps)
    res['true_mid'] = {'rc': rc, 'block_pointers': len(bps), 'with_copies': checked, 'copies_differ': bad[:20],
                       'copies_differ_count': len(bad),
                       'by_copies': {str(k): sum(1 for b in bps if len(b['dvas']) == k) for k in (1, 2, 3)}}
    print(f"true image at {mid}: {len(bps)} block pointers, {checked} with copies, {len(bad)} whose copies differ", flush=True)

    # 3. The controls: what the victim is, and what zdb and the copy check say without it.
    for name, c in ctl.items():
        v = records[c['victim']]
        hits = covering(bps, v['offset'], v['offset'] + v['bytes'])
        # A prefix of the wrong replay that has the group's uberblocks and not the victim.
        n = c['schedule'].index(c['victim']) if c['victim'] in c['schedule'] else mid
        n = min(n, mid) if name == 'reorder' else mid
        image_at(n, c['schedule'])
        rc1, _ = zdb(f'{name}-{n}-bcc', ['-bcc'])
        rc2, cdump = zdb(f'{name}-{n}-bbbbb-cc', ['-bbbbb', '-cc'])
        cbps = block_pointers(cdump)
        cchecked, cbad = copies_that_differ(image, cbps)
        res['controls'][name] = {'victim': c['victim'], 'offset': v['offset'], 'bytes': v['bytes'], 'prefix': n,
                                 'covered_by': hits[:12], 'covered_by_count': len(hits),
                                 'zdb_bcc_rc': rc1, 'zdb_dump_rc': rc2, 'block_pointers': len(cbps),
                                 'with_copies': cchecked, 'copies_differ': cbad[:20], 'copies_differ_count': len(cbad)}
        print(f"control {name} at {n}: zdb -bcc rc {rc1}; {len(cbps)} block pointers, {len(cbad)} whose copies differ; "
              f"victim covered by {len(hits)} block pointers on the true image", flush=True)
        for h in hits[:6]:
            print(f"   copy {h['copy']} of {h['copies']}: {h['line'][:240]}", flush=True)
    if os.path.exists(image):
        os.unlink(image)

    with open(os.path.join(args.out, 'explore.json'), 'w') as f:
        json.dump(res, f, indent=1)
    md = [f"### zdb on the images of `{os.path.basename(args.work.rstrip('/'))}`\n",
          f"{total} records, {len(marks)} marks; the middle mark ends prefix {mid}.\n",
          '| what | options | rc | seconds | zdb said |', '|---|---|---|---|---|']
    for z in res['zdb']:
        md.append(f"| {z['tag']} | `{z['opts']}` | {z['rc']} | {z['s']} | {(' / '.join(z['said'][:2]) or '–').replace('|', '/')[:300]} |")
    t = res['true_mid']
    md.append(f"\nTrue image at the middle mark: {t['block_pointers']} block pointers ({t['by_copies']} by number of copies), "
              f"{t['copies_differ_count']} of {t['with_copies']} with several copies hold different bytes in them.\n")
    for name, c in res['controls'].items():
        md.append(f"Control `{name}` (record {c['victim']}, {c['bytes']} bytes at {c['offset']}), image at prefix {c['prefix']}: "
                  f"`zdb -bcc` rc {c['zdb_bcc_rc']}; {c['copies_differ_count']} of {c['with_copies']} blocks with several copies "
                  f"differ between copies. On the true image the withheld bytes are covered by {c['covered_by_count']} block pointer(s):")
        for h in c['covered_by'][:6]:
            md.append(f"- copy {h['copy']} of {h['copies']}: `{h['line'][:260]}`")
        md.append('')
    with open(os.path.join(args.out, 'explore.md'), 'w') as f:
        f.write('\n'.join(md) + '\n')
    print('\n'.join(md))


if __name__ == '__main__':
    main()
