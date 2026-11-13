#!/usr/bin/env python3
"""Power-cut replay of a direct write into a hole, through skyzfsd (M1.T27).

The in-place store writes the volume's blocks into its home image. A
block the image has never held is a hole (a sparse image) or an
unwritten extent (an image preallocated with fallocate). A write there,
direct or not, reaches the disk at once, but the file shows it only
once the file system has committed the change of its extent tree, and a
power cut before that commit loses it although the device took it
(design §4.2). Design §4.1 allows that: until a flush covers a write,
the block's durable content may be what it held before, zeros here. A
completed flush (the daemon's fdatasync) must make the write durable.
An image written through at create (`create --zero-fill`) has no hole
and no unwritten extent, so there a returned write should not be lost.

How the rig sees it (tests/kernel/hole-rig.sh): the volume's directory
is on a file system (ext4 or xfs) on dm-log-writes, as in the replay
rig's mode through skyzfsd, and skyzfsd serves the volume. The course
(`replay_hole.py course`) writes 4 KiB blocks the image has never held
straight to the ublk device with O_DIRECT, one request each, and after
each write returns it puts a mark `w-I` into the lower disk's log; it
flushes the device (fsync of the block device, which the daemon turns
into its sync) every few writes and marks `f-K` when that returned.
The last writes have no flush after them.

The check (`replay_hole.py check`) rebuilds the lower disk at a record
prefix, mounts it (the journal replays as after a power cut), starts
the daemon as after a reboot (replay_daemon.Daemon), reads the device,
and judges each of the course's blocks:

  - every block reads as zeros or as what its write wrote, whole per
    4 KiB block, and never anything else;
  - a block whose write had returned and was covered by a flush that
    had returned (a mark f-K after its w-I, both in the prefix) holds
    what its write wrote.

A prefix that breaks either is a verdict against the daemon (exit 1),
as is a prefix at which the file system does not mount, the daemon does
not start, or its device cannot be read.

The measurement: the prefixes at which a block whose write had
returned, and that no returned flush covered yet, read as zeros
("zeros after return"), per image kind, in two parts. dm-log-writes
logs a completed write only at the next flush of the lower disk, so a
returned write whose data is not in the prefix yet stood in the disk's
volatile cache: any image loses it at such a cut ("in the disk's
cache"). A block whose data is in the prefix, so on the disk, and that
still reads zeros is the file system's uncommitted extent change
("on the disk, not in the file"): the loss the image's allocation
decides (design §10), which a written-through image should not show.

Negative control (EXPECTED): `withheld` moves the lower disk's writes
that carry one flushed block's data to just after that flush's mark,
as a disk would that acknowledged the flush over data it did not have.
At the mark the block is due and is not there: the check must catch it.
"""

import argparse
import json
import mmap
import os
import random
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import logwrites  # noqa: E402
import replay_daemon as rd  # noqa: E402
from replay_check import run  # noqa: E402

BLOCK = 4096
# A block's content: this line, with the write's number, over and over.
HEADER = b'SKZ-HOLE-%06d\n'
KINDS = ('sparse', 'fallocate', 'zero-fill')
EXPECTED = {'withheld': 'caught'}


def content(i):
    """The 4 KiB a write of block i writes."""
    line = HEADER % i
    return (line * (BLOCK // len(line) + 1))[:BLOCK]


def what(data, i):
    """'zeros', 'written' (block i's write) or 'other'."""
    if data == bytes(len(data)):
        return 'zeros'
    if data == content(i):
        return 'written'
    return 'other'


def offsets(n, dev_bytes, seed=1):
    """n block offsets on a device of dev_bytes, each in its own 1 MiB, spread over the device, in a random order.

    Each block is in a different MiB of the image, so that no two share
    an extent of the file system or a block of a file system with large
    blocks; the order is shuffled so that the image is not filled front
    to back."""
    mibs = dev_bytes >> 20
    if n < 1 or mibs < 2 * n:
        raise ValueError(f'{n} blocks need at least {2 * n} MiB of device, not {mibs}')
    step = mibs // n
    out = [(k * step + step // 2) << 20 for k in range(n)]
    random.Random(seed).shuffle(out)
    return out


def course_plan(writes, flush_every, tail):
    """-> the course's steps: ('w', i) and ('f', k), in order.

    A flush after every flush_every writes; the last `tail` writes come
    after the last flush and have none."""
    if writes < 1 or flush_every < 1 or not 0 <= tail < writes:
        raise ValueError('writes >= 1, flush_every >= 1, 0 <= tail < writes')
    steps, k = [], 0
    flushed = writes - tail
    for i in range(writes):
        steps.append(('w', i))
        if i < flushed and ((i + 1) % flush_every == 0 or i + 1 == flushed):
            steps.append(('f', k))
            k += 1
    return steps


# ---------------------------------------------------------------- course
def mark(lw, text):
    rc_, out, _ = run(['dmsetup', 'message', lw, '0', 'mark', text], 60)
    if rc_:
        raise OSError(f'dmsetup message {lw} mark {text}: {out.strip()[-200:]}')


def course(a):
    """Run the course on the device; write the manifest. -> exit status."""
    dev_bytes = int(open(f'/sys/class/block/{os.path.basename(a.device)}/size').read()) * 512
    offs = offsets(a.writes, dev_bytes, a.seed)
    steps = course_plan(a.writes, a.flush_every, a.tail)
    aligned = mmap.mmap(-1, BLOCK)              # O_DIRECT wants an aligned buffer
    fd = os.open(a.device, os.O_RDWR | os.O_DIRECT)
    log = []
    try:
        mark(a.lw, 'hole-start')
        for kind, i in steps:
            if kind == 'w':
                aligned.seek(0)
                aligned.write(content(i))
                t0 = time.monotonic()
                n = os.pwritev(fd, [aligned], offs[i])
                if n != BLOCK:
                    raise OSError(f'write {i}: {n} bytes of {BLOCK}')
                mark(a.lw, f'w-{i}')
                log.append({'step': 'w', 'i': i, 'offset': offs[i], 's': round(time.monotonic() - t0, 4)})
            else:
                t0 = time.monotonic()
                os.fsync(fd)                    # a flush of the device: the daemon's sync
                mark(a.lw, f'f-{i}')
                log.append({'step': 'f', 'k': i, 's': round(time.monotonic() - t0, 4)})
            time.sleep(a.pause)
        mark(a.lw, 'hole-end')
    finally:
        os.close(fd)
        aligned.close()
    with open(a.manifest, 'w') as f:
        json.dump({'block': BLOCK, 'dev_bytes': dev_bytes, 'offsets': offs, 'steps': log}, f)
    return 0


# ----------------------------------------------------------------- judging
def marks_of(records):
    """-> (start prefix, end prefix, {i: prefix of w-i}, [(prefix of f-k, k)]) from the log's marks."""
    start = end = None
    w, f = {}, []
    for n, r in enumerate(records, 1):
        text = r.get('mark', '')
        if text == 'hole-start':
            start = n
        elif text == 'hole-end':
            end = n
        elif text.startswith('w-') and text[2:].isdigit():
            w[int(text[2:])] = n
        elif text.startswith('f-') and text[2:].isdigit():
            f.append((n, int(text[2:])))
    return start, end, w, f


def state_at(n, wmark, fmarks):
    """A block's state at prefix n: 'pending' (its write had not returned), 'returned' (returned, no flush
    after it had returned) or 'flushed'."""
    if wmark > n:
        return 'pending'
    if any(wmark < p <= n for p, _ in fmarks):
        return 'flushed'
    return 'returned'


def judge(n, reads, wmarks, fmarks, data_at=None):
    """Judge prefix n -> dict(violations=[...], zeros_after_return=[i...], zeros_on_disk=[i...], states=...).

    reads: {i: 'zeros' | 'written' | 'other'} for every block of the course.
    data_at: {i: the first prefix that holds the record carrying block i's
    data}; a returned block that reads zeros at a prefix that holds its
    data is also in zeros_on_disk."""
    data_at = data_at or {}
    out = {'n': n, 'violations': [], 'zeros_after_return': [], 'zeros_on_disk': [], 'states': {}}
    for i, got in sorted(reads.items()):
        st = state_at(n, wmarks[i], fmarks)
        out['states'][i] = (st, got)
        if got == 'other':
            out['violations'].append(f'block {i} ({st}) reads neither zeros nor its write')
        elif st == 'flushed' and got != 'written':
            out['violations'].append(f'block {i}: a flush after its write had returned, and it reads {got}')
        elif st == 'returned' and got == 'zeros':
            out['zeros_after_return'].append(i)
            if data_at.get(i, n + 1) <= n:
                out['zeros_on_disk'].append(i)
    return out


def data_prefixes(records, log_path, n_blocks):
    """-> {i: the first prefix whose last record carries block i's data}, for the blocks found in the log."""
    heads = {HEADER % i: i for i in range(n_blocks)}
    out = {}
    with open(log_path, 'rb') as log:
        for n, r in enumerate(records, 1):
            if 'pos' not in r or not r['bytes']:
                continue
            log.seek(r['pos'])
            data = log.read(r['bytes'])
            for h, i in heads.items():
                if i not in out and h in data:
                    out[i] = n
    return out


def plan(start, end, total, wmarks, fmarks, every, sample, seed=1):
    """The prefixes to check, in order.

    every: each prefix from the course's start mark to the end of the
    log. Otherwise every mark's prefix and the one before it, the end,
    and `sample` more at random among the rest."""
    lo, hi = start, total
    if every:
        return list(range(lo, hi + 1))
    must = {hi, lo, end or hi}
    for p in list(wmarks.values()) + [p for p, _ in fmarks]:
        must |= {p, p - 1}
    must = {p for p in must if lo <= p <= hi}
    rest = [p for p in range(lo, hi + 1) if p not in must]
    random.Random(seed).shuffle(rest)
    return sorted(must | set(rest[:max(0, sample)]))


def read_blocks(image, offs):
    out = {}
    with open(image, 'rb') as f:
        for i, o in enumerate(offs):
            f.seek(o)
            data = f.read(BLOCK)
            out[i] = what(data + bytes(BLOCK - len(data)), i)
    return out


def withheld(records, log_path, wmarks, fmarks):
    """The control's schedule -> dict or None.

    The first flush mark with a block before it, covered by no earlier
    flush, whose data is in the log between the mark before the block's
    w mark and the flush's mark (before its w mark when the daemon writes
    through, before the flush's when it buffers): the records that carry
    the block's content move to just after the flush mark."""
    prev = {}
    marks = sorted([(p, ('w', i)) for i, p in wmarks.items()] + [(p, ('f', k)) for p, k in fmarks])
    last = 0
    for p, m in marks:
        prev[m] = last
        last = p
    with open(log_path, 'rb') as log:
        for fp, k in fmarks:
            for i, wp in sorted(wmarks.items(), key=lambda kv: kv[1]):
                if not wp < fp:
                    continue
                if any(wp < q < fp for q, _ in fmarks):
                    continue                    # covered by an earlier flush
                head = HEADER % i
                victims = []
                for r in records[prev[('w', i)]:fp - 1]:
                    if 'pos' in r and r['bytes']:
                        log.seek(r['pos'])
                        if head in log.read(r['bytes']):
                            victims.append(r['i'])
                if not victims:
                    continue
                vs = set(victims)
                sched = [j for j in range(len(records)) if j not in vs]
                at = sched.index(fp - 1) + 1    # just after the flush's mark record
                sched[at:at] = victims
                return {'schedule': sched, 'point': at, 'mark_prefix': fp, 'block': i, 'flush': k, 'victims': victims,
                        'what': f"the {len(victims)} write(s) that carry block {i}'s data are moved after the mark "
                                f'of flush {k}: the flush returned over data the disk does not have'}
    return None


# ------------------------------------------------------------------ check
class Ns:
    pass


def check(a):
    t_all = time.monotonic()
    idx = logwrites.index(a.log, 0)
    records = idx['records']
    with open(a.manifest) as f:
        man = json.load(f)
    offs = man['offsets']
    start, end, wmarks, fmarks = marks_of(records)
    res = {'fs': a.fs, 'kind': a.kind, 'records': len(records), 'writes': len(offs), 'flushes': len(fmarks),
           'checked': 0, 'zero_prefixes': 0, 'zero_blocks': 0, 'disk_zero_prefixes': 0, 'disk_zero_blocks': 0,
           'violations': [], 'failures': [],
           'rig_error': None, 'control': None, 'budget': a.budget_name}
    if start is None or len(wmarks) != len(offs):
        res['rig_error'] = (f'the log has {"no" if start is None else "a"} start mark and {len(wmarks)} write marks '
                            f'for {len(offs)} writes')
        return res
    ns = Ns()
    for k in ('log', 'base', 'work', 'skyzfsd', 'volume', 'ublk', 'dd', 'discard'):
        setattr(ns, k, getattr(a, k))
    ns.serve_arg = a.serve_arg
    ns.dev_bytes = man['dev_bytes']
    ns.mount_opts = ['-o', 'nouuid'] if a.fs == 'xfs' else []
    daemon = rd.Daemon(ns, idx)
    image = os.path.join(a.work, 'hole.img')
    data_at = data_prefixes(records, a.log, len(offs))
    res['data_found'] = len(data_at)
    pts = plan(start, end, len(records), wmarks, fmarks, a.every_prefix, a.sample, a.seed)
    res['planned'] = len(pts)
    res['range'] = [start, len(records)]
    deadline = None if a.every_prefix else time.monotonic() + a.budget_s
    must = set(plan(start, end, len(records), wmarks, fmarks, False, 0))
    rows = []
    try:
        daemon.reset()
        for n in pts:
            if deadline and time.monotonic() > deadline and n not in must:
                continue
            failed, detail, info = daemon.make(n, image)
            row = {'n': n, 't': round(info.get('t_make', 0), 2)}
            if failed:
                row['failed'], row['detail'] = failed, detail
                if failed in rd.JUDGING:
                    res['failures'].append(f'prefix {n}: {failed}: {detail}')
                else:
                    res['rig_error'] = f'prefix {n}: {failed}: {detail}'
                    rows.append(row)
                    break
            else:
                j = judge(n, read_blocks(image, offs), wmarks, fmarks, data_at)
                row['zeros'] = j['zeros_after_return']
                row['zeros_on_disk'] = j['zeros_on_disk']
                if j['violations']:
                    row['violations'] = j['violations']
                    res['violations'] += [f'prefix {n}: {v}' for v in j['violations']]
                if j['zeros_after_return']:
                    res['zero_prefixes'] += 1
                    res['zero_blocks'] += len(j['zeros_after_return'])
                if j['zeros_on_disk']:
                    res['disk_zero_prefixes'] += 1
                    res['disk_zero_blocks'] += len(j['zeros_on_disk'])
                res['checked'] += 1
            rows.append(row)
            if info.get('abort'):
                res['rig_error'] = f'prefix {n}: {info["abort"]}'
                break
        if not res['rig_error'] and a.control:
            res['control'] = control(a, daemon, image, records, wmarks, fmarks, offs)
    except rd.RigError as e:
        res['rig_error'] = str(e)
    except Exception as e:                      # noqa: BLE001 - the rig's own fault, reported as such
        res['rig_error'] = f'{type(e).__name__}: {e}\n{traceback.format_exc()[-800:]}'
    res['seconds'] = round(time.monotonic() - t_all, 1)
    with open(os.path.join(a.out, f'hole-{a.fs}-{a.kind}-prefixes.json'), 'w') as f:
        json.dump(rows, f)
    return res


def control(a, daemon, image, records, wmarks, fmarks, offs):
    w = withheld(records, a.log, wmarks, fmarks)
    if not w:
        return {'result': 'not built', 'what': 'no flushed block whose data could be found in the log'}
    daemon.reset(w['schedule'])
    failed, detail, _ = daemon.make(w['point'], image)
    if failed:
        got = f'{failed}: {detail}'
        caught = failed in rd.JUDGING
    else:
        # The schedule's first w['point'] records are those of the log's
        # prefix up to the flush's mark, less the victims: the blocks'
        # states are those at that mark's prefix in the log.
        j = judge(w['mark_prefix'], read_blocks(image, offs), wmarks, fmarks)
        bad = [v for v in j['violations'] if v.startswith(f"block {w['block']}:")]
        caught = bool(bad)
        got = '; '.join(bad) or f"block {w['block']} read {j['states'][w['block']][1]}"
    return {'result': 'caught' if caught else 'not caught', 'what': w['what'], 'got': got,
            'block': w['block'], 'point': w['point']}


# ---------------------------------------------------------------- summary
def verdict(results):
    """-> (exit status, headline). 2: the rig failed; 1: a verdict against the daemon; 0: none."""
    results = [r for r in results if not r.get('skipped')]
    if not results:
        return 2, 'No course was checked. This is not a verdict on the daemon.'
    errors = [r for r in results if r.get('rig_error')]
    if errors:
        return 2, 'The rig failed. This is not a verdict on the daemon.'
    bad = [r for r in results if r['violations'] or r['failures']]
    ctl = [r for r in results if r.get('control') and r['control']['result'] != EXPECTED['withheld']]
    if ctl:
        return 2, 'A negative control was not caught: the check does not show what it should. Not a verdict.'
    if bad:
        return 1, 'A verdict against the daemon: a block read what it must not at a prefix, or a prefix did not start.'
    return 0, 'Every checked prefix as design §4.1 allows.'


def summary(results, kernel=''):
    code, head = verdict(results)
    out = ['## Power-cut replay: a direct write into a hole, through skyzfsd (M1.T27)', '',
           f'**{head}**' + (f' Kernel {kernel}.' if kernel else ''), '',
           '| file system | image | prefixes checked (of the course\'s) | zeros after return '
           '| of them on the disk, not in the file | violations | prefixes that did not start '
           '| control `withheld` | seconds |',
           '|---|---|---|---|---|---|---|---|---|']
    for r in results:
        if r.get('skipped'):
            out.append(f"| {r['fs']} | {r['kind']} | not run: {r['skipped']} | | | | | | |")
            continue
        rng = r.get('range')
        span = (rng[1] - rng[0] + 1) if rng else '?'
        c = r.get('control') or {}
        ctl = c.get('result', 'not asked') if c else 'not asked'
        out.append(f"| {r['fs']} | {r['kind']} | {r['checked']} of {span} | {r['zero_prefixes']} "
                   f"({r['zero_blocks']} block reads) | {r.get('disk_zero_prefixes', 0)} "
                   f"({r.get('disk_zero_blocks', 0)} block reads) | {len(r['violations'])} | {len(r['failures'])} "
                   f"| {ctl} | {r.get('seconds', '?')} |")
    out += ['', '"Zeros after return": prefixes at which a block whose write had returned, and that no '
                'returned flush covered yet, read as zeros; design §4.1 allows it. dm-log-writes logs a '
                'completed write only at the lower disk\'s next flush, so most of these are a write in the '
                'disk\'s volatile cache, which any image loses at such a cut. "On the disk, not in the file": '
                'those at which the block\'s data was already in the prefix, so on the disk, and the block '
                'still read zeros: the file system had not committed the change of its extent tree. That is '
                'the measurement for the image\'s allocation (design §10). A violation: a block that read '
                'neither zeros nor its write, or a block covered by a returned flush that did not read its '
                'write.', '']
    for r in (r for r in results if not r.get('skipped')):
        for k in ('rig_error',):
            if r.get(k):
                out.append(f"- {r['fs']} {r['kind']}: the rig's error: {r[k][:600]}")
        for v in (r.get('violations') or [])[:8]:
            out.append(f"- {r['fs']} {r['kind']}: {v}")
        for v in (r.get('failures') or [])[:8]:
            out.append(f"- {r['fs']} {r['kind']}: {v}")
        c = r.get('control')
        if c:
            out.append(f"- {r['fs']} {r['kind']}, control `withheld`: {c['result']} (expected {EXPECTED['withheld']}): "
                       f"{c['what']}; {c.get('got', '')}")
    return code, '\n'.join(out) + '\n'


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    sub = ap.add_subparsers(dest='cmd', required=True)
    c = sub.add_parser('course', help='write the course on the device (while recording)')
    c.add_argument('--device', required=True)
    c.add_argument('--lw', required=True, help='the dm-log-writes device that takes the marks')
    c.add_argument('--manifest', required=True)
    c.add_argument('--writes', type=int, default=16)
    c.add_argument('--flush-every', type=int, default=3)
    c.add_argument('--tail', type=int, default=2, help='writes after the last flush')
    c.add_argument('--pause', type=float, default=0.05, help='seconds after each step')
    c.add_argument('--seed', type=int, default=1)
    k = sub.add_parser('check', help='check one recorded course')
    for name in ('log', 'base', 'manifest', 'work', 'out', 'skyzfsd', 'volume', 'fs', 'kind'):
        k.add_argument('--' + name, required=True)
    k.add_argument('--ublk', default='ublk')
    k.add_argument('--dd', default='dd')
    k.add_argument('--serve-arg', action='append')
    k.add_argument('--discard', choices=('zero', 'skip'), default='zero')
    k.add_argument('--every-prefix', action='store_true')
    k.add_argument('--sample', type=int, default=40, help='prefixes at random besides the marks (not with --every-prefix)')
    k.add_argument('--budget-s', type=float, default=300, help='seconds for the sampled prefixes')
    k.add_argument('--budget-name', default='')
    k.add_argument('--control', action='store_true', help='build the negative control withheld')
    k.add_argument('--seed', type=int, default=1)
    s = sub.add_parser('summary', help='the summary of the courses checked')
    s.add_argument('results', nargs='+', help='the JSON of each check')
    s.add_argument('--kernel', default='')
    s.add_argument('--out', required=True)
    a = ap.parse_args()
    if a.cmd == 'course':
        return course(a)
    if a.cmd == 'check':
        res = check(a)
        with open(os.path.join(a.out, f'hole-result-{a.fs}-{a.kind}.json'), 'w') as f:
            json.dump(res, f, indent=1)
        print(json.dumps({k: res[k] for k in ('fs', 'kind', 'checked', 'zero_prefixes', 'rig_error')}))
        return 0
    results = []
    for p in a.results:
        with open(p) as f:
            results.append(json.load(f))
    code, text = summary(results, a.kernel)
    with open(a.out, 'w') as f:
        f.write(text)
    print(text)
    return code


if __name__ == '__main__':
    sys.exit(main())
