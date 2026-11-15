#!/usr/bin/env python3
"""The cut rig on the daemon (M2.T3, design §5.2): the daemon's count of
cuts, their groups and whether each opens as itself, against the
reference (the first stage); and where the daemon held cuts, the content
of each cut the rig's holder wrote out (the second stage).

Usage: cut_daemon_check.py NAME DIR REQUEST_LOG - DEV_BYTES [LOG_DEVICE_IMAGE]

Called by tests/kernel/cut-record.sh with CUT_SERVER=daemon, after each
recording: the pool's device was a volume that skyzfsd served with its
request log (bins/skyzfsd/src/record.rs). The fourth argument stands
where the dm-log-writes check takes its base image, and is not used.

The request log is read into the records of tests/kernel/cut_ref.py in
the daemon's own order, the order design §5.2 counts in:

- a write is placed at its `applied` line, when the store had applied it
  whole, with its bytes from its `label` line where it touches a label
  (the log writes those out whole);
- a flush is two records: its receipt at its `recv` line, which takes
  the mark (design §5.2: the first flush *received* after an applied
  uberblock write), and its barrier at its `barrier` line, the cut's
  instant. The daemon's added flush after a write with FUA
  (`fua_flush`) has no `recv` line, as it is received once the write
  completed, and is one record at its barrier;
- a write-zeroes, which has no line for when it ran, by its `recv`
  line: the store accepts requests in the order received, so one
  received after a flush takes effect after that flush's barrier. It
  is placed at its receipt, or, when a flush received before it has
  not reached its barrier yet, right after that barrier (the last such
  flush's). It is a range that reads as zeros after it (cut_ref's
  `discard`: no uberblock left in the slots it covers);
- a discard placed as a write-zeroes is, as a `trim` that cut_ref does
  not read: the store keeps a discarded block's content (rule R6), so
  the slots it covers still hold what they held;
- the recording's marks (DIR/marks.tsv, CLOCK_MONOTONIC as the log's)
  by their time.

The reference's cuts on those records (cut_ref.cuts, with
cut_ref.opens for whether each opens as itself) are then held to:

- `cuts`: the daemon's `cut` lines, in order: the same number, each at
  the same instant (the barrier its line follows), with the same group
  and the same answer to whether it opens as itself (`itself` or
  `rewind`);
- `cut_numbers`: the number each `cut` line carries (since M2.2b-2):
  1 to N in order, as one instance counts; INFO on a log without them;
- `status`: `skyzfsd status` read before the detach (DIR/status.txt):
  `cuts`, `cut_group` and `cut_opens` of the last cut. The daemon
  rewrites its status file at most every 100 ms, when a counter has
  changed, so the rig reads it a second after the last export and
  writes the monotonic times before and after the read
  (DIR/status-time.tsv). The values are held to the last cut; where
  they differ, the read's time less the status's `age_ms` (a difference
  of /proc/uptime, which ticks every 10 ms) says whether the status was
  written before the daemon's last `cut` line, which after a second,
  ten of the daemon's intervals, is the daemon's fault too;
- `sessions` and `import_syncs_before_return`: what ZFS says it synced,
  as the cut rig judges the reference (cut_check.py).

Controls (`ctl_*`, `EXPECTED`), wrong lists that must make `cuts` fail,
so that the comparison is shown to see each thing it compares:
- `drop`: the middle cut left out; `last`: the last cut left out;
  `extra`: one cut more after the last (the count alone tells these);
- `group`: one group moved by one; `opens`: one answer turned;
  `late`: one cut at the next barrier, with its group and answer (the
  instant alone tells it);
- `early`: the reference by the rule `early` (each cut at the barrier
  before its own);
- `txg-alone`, `count-repeats`, `previous`: the reference by the rules
  that key a repeat on the group alone, count every uberblock write,
  and compare only with the write before. Where a recording has
  nothing that tells a rule from the design's (no group written twice,
  no flush between copies of an uberblock, no stale multihost write)
  the rule gives the design's cuts, and the control says `same`; the
  table says where each must be caught.

A `cuts` line that fails where the log holds what the check cannot
place is the rig's error (ERROR), not a verdict: the daemon's flush
after a write with FUA, received when the write completed with no line
of its own, and a write-zeroes over a label, applied when it ran with
no line of its own. ZFS issues neither; no recording has had one.

The second stage, where DIR/holder.log says the holder ran
(cut_hold.py; the daemon served with --hold-cuts):
- `cut_state`: the request log's `cuts` line and the status both say
  `begun`, as a new volume's first start with --hold-cuts does;
- `cuts_held`: the status's count of held cuts is the number of lines
  `held` listed, both read after the export (DIR/held-final.txt,
  DIR/status-held.txt);
- `held_content`: each cut the holder wrote (DIR/held.tsv) holds, block
  by block, the reference's content at the cut's instant. The reference
  plays the request log's records up to the cut's barrier: a write sets
  each 4 KiB block it touches to the hash its `applied` line gives (a
  block it covers in part is not known, and counted); a write-zeroes
  sets zeros; a discard leaves either the block's content or zeros (the
  store keeps it, R6; the lead accepts both). Blocks never written are
  zeros. The holder hashed each block of the cut's map as its image held
  it after the write (DIR/held-blocks.tsv); the image accumulates, as an
  upload onto the image before it does. Also: each written cut's group
  and answer are its cut line's, the last cut counted was written, and
  the image after the drain holds nothing the maps did not write
  (DIR/held-image.tsv). A failed `held_content` whose wrong blocks
  include one under a write-zeroes received while an overlapping write
  was in the store is the rig's error: the store ran the write first,
  the log does not say when the write-zeroes ran.
- controls: the middle written cut's image against the reference at the
  cut before it (`content_early`), and the image before its write
  against the reference at its instant (`content_stale`): both caught.
- `obs_copied_blocks`: how many blocks of each written cut's map had
  changed in the home (a write applied, a write-zeroes answered) after
  the cut's instant and before the holder asked for its write
  (held.tsv's times, the request log's clock): those the daemon wrote
  from copies, so that held_content is shown to have read some. The
  first written cut maps the whole volume and holds nearly all of them.
- `cuts_abandoned`: at least one in `bound`; `edges`: at least one
  write-zeroes and one discard, and a cut written after the first that
  had a block a write-zeroes changed before its write (the rig's
  error otherwise: the recording did not hold a cut across it).

Not judged here: a daemon that is started again on the volume (the
count is per instance, and the rings it starts from are the volume's),
and a held cut written after the volume failed. A request log with more
than one instance is the rig's error.

Exit 0: everything as the reference; 1: a verdict; 2: the rig's error.
"""
import hashlib
import os
import re
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_check  # noqa: E402
import cut_ref  # noqa: E402
from cut_check import RigError  # noqa: E402

RECS = ('life', 'slog', 'rewind', 'rewindsync', 'rewindf', 'multihost', 'config', 'stray', 'autotrim', 'full',
        'bound', 'edges')   # the last two only where the daemon holds cuts (cut-record.sh)
LISTS = ('drop', 'last', 'extra', 'group', 'opens', 'late')
RULES = ('early', 'txg-alone', 'count-repeats', 'previous')
CONTROLS = LISTS + RULES
# By recording. The rules' columns are the runs on 192427b and 2c67c42's
# first run (43675946002, 43676360281 on both images): txg-alone tells
# the rewinds whose new history writes a group the old one used (and
# rewindsync and multihost in dm-log-writes' runs, cut_check); count-
# repeats tells a flush between copies of an uberblock (multihost
# always; stray in two of three, its flushes being the rig's, every
# 2 ms); previous tells a stale multihost write (one of three).
EXPECTED = dict({c: {r: 'caught' for r in RECS} for c in LISTS + ('early',)}, **{
    'txg-alone': dict({r: 'same' for r in RECS}, rewind='caught', rewindf='caught',
                      rewindsync=('same', 'caught'), multihost=('same', 'caught')),
    'count-repeats': dict({r: 'same' for r in RECS}, multihost='caught', stray=('same', 'caught')),
    'previous': dict({r: 'same' for r in RECS}, multihost=('same', 'caught')),
})
CLOCK_NS = 10_000_000       # the tick of /proc/uptime, which age_ms is a difference of
SECTOR = 512
BLOCK = 4096
ZERO = hashlib.sha256(bytes(BLOCK)).digest()[:8].hex()
CONTENT_CONTROLS = ('content_early', 'content_stale')
CONTENT_EXPECTED = {c: 'caught' for c in CONTENT_CONTROLS}


def flat(text):
    return ' '.join(str(text).split())


def parse_log(text):
    """The request log -> [instance], each {'pid', 'device', 'lines': [dict]} in the file's order."""
    instances, by_pid = [], {}
    for k, ln in enumerate(text.splitlines()):
        f = ln.split('\t')
        # Eight columns; since M2.2b-2 an `applied` line has a ninth (the
        # blocks' hashes) and, with --request-log-content whole, a tenth.
        if not 8 <= len(f) <= 10 or not f[0].isdigit():
            raise RigError(f'request log line {k + 1}: not eight to ten columns: {ln[:120]!r}')
        if len(f) > 8 and f[2] != 'applied':
            raise RigError(f'request log line {k + 1}: a {f[2]} line with more than eight columns: {ln[:120]!r}')
        rec = {'ns': int(f[0]), 'pid': f[1], 'event': f[2], 'tag': f[3], 'op': f[4], 'sector': f[5],
               'sectors': f[6], 'result': f[7], 'extra': f[8:], 'line': k + 1}
        if rec['event'] == 'start':
            inst = {'pid': rec['pid'], 'device': rec['tag'], 'lines': []}
            instances.append(inst)
            by_pid[rec['pid']] = inst
            continue
        inst = by_pid.get(rec['pid'])
        if inst is None:
            raise RigError(f'request log line {k + 1}: pid {rec["pid"]} has no start line')
        inst['lines'].append(rec)
    return instances


def read_marks(text):
    """marks.tsv -> [(ns, name)]."""
    out = []
    for ln in text.splitlines():
        f = ln.split('\t')
        if len(f) != 2 or not f[0].isdigit():
            raise RigError(f'marks.tsv: not a time and a name: {ln!r}')
        out.append((int(f[0]), f[1]))
    return out


def records_of(lines, marks):
    """One instance's lines and the marks -> (records, the daemon's cuts).

    The records are cut_ref's, in the daemon's order. A daemon cut:
    {'txg', 'itself', 'prefix', 'line'}, prefix the record of the
    barrier its line follows: the records before it are those before
    the cut's instant, as a reference cut's prefix."""
    records, dcuts, pending, data = [], [], {}, {}
    marks = sorted(marks)
    mk = 0

    def add(r):
        r['i'] = len(records)
        records.append(r)
        return r

    def marks_until(ns):
        nonlocal mk
        while mk < len(marks) and marks[mk][0] <= ns:
            add({'flags': ['mark'], 'mark': marks[mk][1], 'offset': 0, 'bytes': 0})
            mk += 1

    last_barrier = None
    # Flushes received whose barrier has not come, in the order received,
    # each with the requests that have no line of their own for when they
    # took effect (a write-zeroes, a discard) received after it.
    received = {}
    zeroing = {}                # write-zeroes not answered yet, by tag

    def behind(r):
        """A request placed by its receipt: the store accepts requests in
        the order received (`InPlace::accept`), and one received after a
        flush takes effect after that flush's barrier, whether the flush
        waits (a request received while one waits is put behind it) or
        ran at once. It is placed right after the barrier of the last
        flush received before it, or at its receipt when none waits."""
        if received:
            received[next(reversed(received))].append(r)
        else:
            add(r)
    for ln in lines:
        marks_until(ln['ns'])
        ev = ln['event']
        if ev == 'recv':
            if not (ln['sector'].isdigit() and ln['sectors'].isdigit()):
                raise RigError(f"request log line {ln['line']}: a recv without its sectors")
            off, n = int(ln['sector']) * SECTOR, int(ln['sectors']) * SECTOR
            if ln['op'] == 'write':
                pending[ln['tag']] = (off, n)
                data.pop(ln['tag'], None)
            elif ln['op'] == 'write_zeroes':
                # Placed at its receipt, or behind a flush (below). A write
                # received before it that overlaps it and is not applied
                # yet is applied first by the store (both in the order
                # received) and after it here: their shared blocks are not
                # placed for certain.
                lo, hi = off // BLOCK, (off + n - 1) // BLOCK
                amb = {b for (o, m) in pending.values() for b in range(max(lo, o // BLOCK),
                                                                       min(hi, (o + m - 1) // BLOCK) + 1)}
                z = {'flags': ['discard'], 'offset': off, 'bytes': n, 'op': ln['op'], 'ambiguous': amb}
                zeroing[ln['tag']] = z
                behind(z)
            elif ln['op'] == 'flush':
                # The receipt takes the mark; the barrier counts the cut.
                add({'flags': ['receipt'], 'offset': 0, 'bytes': 0, 'tag': ln['tag']})
                received[ln['tag']] = []
            elif ln['op'] == 'discard':
                # A discard changes no content in the store (R6), and
                # nothing of cut_ref's reads a `trim`: only the content of
                # held cuts does, which accepts either for it.
                behind({'flags': ['trim'], 'offset': off, 'bytes': n})
        elif ev == 'commit' and ln['op'] == 'write_zeroes' and ln['tag'] in zeroing:
            # When it was answered: it had taken effect by then.
            zeroing.pop(ln['tag'])['done_ns'] = ln['ns']
        elif ev == 'label':
            try:
                data[ln['tag']] = bytes.fromhex(ln['result'])
            except ValueError:
                raise RigError(f"request log line {ln['line']}: a label line whose bytes are not hex") from None
        elif ev == 'applied':
            if ln['tag'] not in pending:
                raise RigError(f"request log line {ln['line']}: write {ln['tag']} applied with no recv before it")
            off, n = pending.pop(ln['tag'])
            r = {'flags': [], 'offset': off, 'bytes': n, 'pos': None, 'tag': ln['tag'], 'ns': ln['ns']}
            if ln['extra']:
                # Since M2.2b: each 4 KiB block the write touches, from its
                # first, as the write leaves it; `-` where it covers part.
                hs = ln['extra'][0].split(',')
                if len(hs) != len(range(off // BLOCK, (off + n - 1) // BLOCK + 1)) \
                        or any(h != '-' and not re.fullmatch(r'[0-9a-f]{16}', h) for h in hs):
                    raise RigError(f"request log line {ln['line']}: write {ln['tag']}'s hashes are not one for each "
                                   f"block it touches ({len(hs)} for {n} bytes at {off})")
                r['hashes'] = [None if h == '-' else h for h in hs]
            b = data.pop(ln['tag'], None)
            if b is not None:
                if len(b) != n:
                    raise RigError(f"request log line {ln['line']}: write {ln['tag']} of {n} bytes, label line of {len(b)}")
                r['data'] = b
            add(r)
        elif ev == 'barrier':
            r = {'flags': ['flush'], 'offset': 0, 'bytes': 0, 'tag': ln['tag'], 'op': ln['op']}
            if ln['op'] == 'flush':
                if ln['tag'] not in received:
                    raise RigError(f"request log line {ln['line']}: the barrier of flush {ln['tag']} with no recv before it")
                after = received.pop(ln['tag'])
                r['received'] = True
            else:
                after = []
            # The daemon's own flush after a write with FUA has no recv
            # line: it is received after the write completed, and is
            # placed here at its barrier, as dm-log-writes places a flush.
            last_barrier = add(r)
            for x in after:
                add(x)
        elif ev == 'cut':
            if not ln['result'].isdigit() or ln['sector'] not in ('itself', 'rewind'):
                raise RigError(f"request log line {ln['line']}: a cut line that is not a group and itself or rewind")
            if last_barrier is None or last_barrier['tag'] != ln['tag'] or last_barrier['op'] != ln['op']:
                raise RigError(f"request log line {ln['line']}: a cut that does not follow its own barrier")
            dcuts.append({'txg': int(ln['result']), 'itself': ln['sector'] == 'itself',
                          'prefix': last_barrier['i'], 'line': ln['line'], 'ns': ln['ns'],
                          'number': ln['sectors']})
    marks_until(float('inf'))
    for waiting in received.values():
        for x in waiting:       # a flush whose barrier the log does not have
            add(x)
    return records, dcuts


def read_of(records, dev_bytes):
    """cut_ref's read(r): the bytes of a write that touches a label, which the log holds."""
    labels = cut_ref.label_offsets(dev_bytes)

    def read(r):
        if 'data' not in r:
            if cut_ref.touches_label(r['offset'], r['bytes'], labels):
                raise RigError(f"write at {r['offset']} touches a label and the log has no label line for it")
            return b'\0' * r['bytes']
        return r['data']
    return read


def compare(dcuts, ref):
    """The daemon's cuts and the reference's [{'txg', 'itself', 'prefix'}] -> (ok, detail).
    The same instant (prefix), group and answer to itself, in order."""
    for k, (d, r) in enumerate(zip(dcuts, ref)):
        key_d = (d['prefix'], d['txg'], d['itself'])
        key_r = (r['prefix'], r['txg'], r['itself'])
        if key_d != key_r:
            return False, (f"cut {k + 1}: the daemon's is group {d['txg']} "
                           f"({'itself' if d['itself'] else 'rewind'}) at record {d['prefix']}, the reference's "
                           f"group {r['txg']} ({'itself' if r['itself'] else 'rewind'}) at record {r['prefix']}")
    if len(dcuts) != len(ref):
        return False, f'the daemon counted {len(dcuts)} cuts, the reference {len(ref)}'
    return True, f'{len(ref)} cuts, each at the same barrier with the same group and the same answer to itself'


def numbers_verdict(dcuts):
    """The `cut` lines' numbers (M2.2b-2's sectors column; `-` in a log
    from before it) -> (verdict, detail). One instance that held no cuts
    before counts from 1: the k-th line carries k."""
    nums = [d['number'] for d in dcuts]
    if all(n == '-' for n in nums):
        return 'INFO', 'the cut lines carry no numbers (a daemon from before M2.2b-2)'
    for k, n in enumerate(nums):
        if n != str(k + 1):
            return 'FAIL', f"cut line {k + 1} (request log line {dcuts[k]['line']}) carries the number {n!r}"
    return 'PASS', f'the {len(nums)} cut lines are numbered 1 to {len(nums)}'


def full_blocks(off, n):
    """The blocks a range covers whole, and those it covers in part."""
    first, last = off // BLOCK, (off + n - 1) // BLOCK
    whole = [b for b in range(first, last + 1) if b * BLOCK >= off and (b + 1) * BLOCK <= off + n]
    part = [b for b in range(first, last + 1) if b not in set(whole)]
    return whole, part


def apply(model, r):
    """Record r's change to the content model {block: hash, a frozenset of
    hashes any of which is right, or None for not known}."""
    flags = r['flags']
    if 'trim' in flags:
        whole, part = full_blocks(r['offset'], r['bytes'])
        for b in whole:
            cur = model.get(b, ZERO)
            model[b] = None if cur is None else (cur | {ZERO} if isinstance(cur, frozenset) else frozenset({cur, ZERO}))
        for b in part:
            model[b] = None
    elif 'discard' in flags:            # a write-zeroes
        whole, part = full_blocks(r['offset'], r['bytes'])
        for b in whole:
            model[b] = ZERO
        for b in part:
            model[b] = None
    elif cut_ref.is_write(r):
        first = r['offset'] // BLOCK
        hs = r.get('hashes')
        for k, b in enumerate(range(first, (r['offset'] + r['bytes'] - 1) // BLOCK + 1)):
            model[b] = hs[k] if hs is not None else None


def same(expected, got):
    """-> True, False, or None when the model does not know the block."""
    if expected is None or (isinstance(expected, frozenset) and None in expected):
        return None
    if isinstance(expected, frozenset):
        return got in expected
    return got == expected


def held_compare(model, image, limit=5):
    """The model and an image {block: hash} (zeros elsewhere) -> (compared,
    not known, either, [wrong blocks, at most limit])."""
    compared = unknown = either = 0
    bad = []
    for b in set(model) | set(image):
        e = model.get(b, ZERO)
        v = same(e, image.get(b, ZERO))
        if v is None:
            unknown += 1
            continue
        compared += 1
        either += isinstance(e, frozenset)
        if not v:
            bad.append(b)
    bad.sort()
    wrong = []
    for b in bad:
        if len(wrong) < limit:
            e = model.get(b, ZERO)
            wrong.append((b, e if not isinstance(e, frozenset) else '|'.join(sorted(e)), image.get(b, ZERO)))
        else:
            wrong.append(None)
    return compared, unknown, either, wrong


def read_held(text):
    """held.tsv -> [{'number', 'group', 'opens', 'blocks', 'asked'}], asked
    the monotonic time the write was asked for."""
    out = []
    for k, ln in enumerate(text.splitlines()):
        f = ln.split('\t')
        if len(f) != 6 or not all(x.isdigit() for x in (f[0], f[1], f[3], f[4], f[5])) \
                or f[2] not in ('itself', 'rewind'):
            raise RigError(f'held.tsv line {k + 1}: not `number group opens blocks ns ns`: {ln!r}')
        out.append({'number': int(f[0]), 'group': int(f[1]), 'opens': f[2], 'blocks': int(f[3]), 'asked': int(f[4])})
    return out


def copied(records, dcuts, written, blocks):
    """The blocks each written cut's write read from copies -> [(number,
    blocks, of them under a write-zeroes)]: the blocks of its map that a
    write applied, or a write-zeroes answered, after the cut's instant
    and before its write was asked for had changed in the home. The
    review of #103 (S4): a daemon that makes no copy passes held_content
    where there are none."""
    by_number = {int(d['number']): d for d in dcuts if d['number'] != '-'}
    out = []
    for w in written:
        n = w['number']
        m = set(blocks.get(n, ()))
        d = by_number.get(n)
        if d is None:
            continue
        hit, zeroed = set(), set()
        for r in records[d['prefix'] + 1:]:
            if r.get('op') == 'write_zeroes':
                if r.get('done_ns', w['asked']) < w['asked']:
                    zeroed |= m & set(full_blocks(r['offset'], r['bytes'])[0])
            elif cut_ref.is_write(r) and 'ns' in r and r['ns'] < w['asked']:
                hit |= m & set(range(r['offset'] // BLOCK, (r['offset'] + r['bytes'] - 1) // BLOCK + 1))
        out.append((n, len(hit | zeroed), len(zeroed)))
    return out


def read_hashes(text, what, keyed=True):
    """`number block hash` (keyed) or `block hash` lines -> {number: {block: hash}} or {block: hash}."""
    out = {}
    for k, ln in enumerate(text.splitlines()):
        f = ln.split('\t')
        if len(f) != (3 if keyed else 2) or not all(x.isdigit() for x in f[:-1]) \
                or not re.fullmatch(r'[0-9a-f]{16}', f[-1]):
            raise RigError(f'{what} line {k + 1}: {ln!r}')
        if keyed:
            out.setdefault(int(f[0]), {})[int(f[1])] = f[2]
        else:
            out[int(f[0])] = f[1]
    return out


def held_content(records, dcuts, written, blocks, final=None):
    """The cuts the holder wrote, each against the reference's content at
    its instant -> (ok, detail, controls {name: (verdict, detail)}, every
    block held wrong).

    written: read_held; blocks: {number: {block: hash}} of each cut's map
    as the rig's image held it after the write; final: {block: hash} of
    the image's blocks that are not zeros after the drain."""
    by_number = {}
    for k, d in enumerate(dcuts):
        if d['number'] != '-':
            by_number[int(d['number'])] = k
    if not written:
        raise RigError('the holder wrote no cut')
    image, model, at = {}, {}, 0
    bad, notes, total = [], [], [0, 0, 0]
    wrong_blocks = set()
    mid = written[len(written) // 2]['number']
    ctl = {}
    prev_image = None
    for w in written:
        n = w['number']
        if n not in by_number:
            raise RigError(f'the holder wrote cut {n}, which has no cut line in the request log')
        d = dcuts[by_number[n]]
        if (w['group'], w['opens'] == 'itself') != (d['txg'], d['itself']):
            bad.append(f"cut {n}: held as group {w['group']} ({w['opens']}), its cut line says group {d['txg']} "
                       f"({'itself' if d['itself'] else 'rewind'})")
        if n == mid:
            prev_image = dict(image)
            # content_early: the reference at the barrier of the cut before
            k = by_number[n]
            early_at = dcuts[k - 1]['prefix'] if k > 0 else 0
            early = {}
            for r in records[:early_at]:
                apply(early, r)
        if n not in blocks:
            raise RigError(f'cut {n} was written and no block of its map was hashed')
        image.update(blocks[n])
        while at < d['prefix']:
            apply(model, records[at])
            at += 1
        compared, unknown, either, wrong = held_compare(model, image, limit=10 ** 9)
        wrong_blocks |= {x[0] for x in wrong}
        wrong = wrong[:5] + [None] * (len(wrong) - 5) if len(wrong) > 5 else wrong
        total[0] += compared
        total[1] += unknown
        total[2] += either
        if wrong:
            shown = [x for x in wrong if x is not None]
            bad.append(f"cut {n} (group {d['txg']}): {len(wrong)} blocks differ from the reference at its instant, "
                       f"for example " + ', '.join(f'block {b}: {e} wanted, {g} held' for b, e, g in shown))
        if n == mid:
            ctl['content_early'] = held_compare(early, image)[3]
            ctl['content_stale'] = held_compare(model, prev_image)[3]
    last = len(dcuts)
    if written[-1]['number'] != last:
        bad.append(f"the last cut written is {written[-1]['number']}, the last counted {last}")
    if final is not None:
        stray = [b for b in sorted(set(final) | {b for b, h in image.items() if h != ZERO})
                 if final.get(b, ZERO) != image.get(b, ZERO)]
        if stray:
            bad.append(f'{len(stray)} blocks of the image after the drain differ from what the maps wrote, '
                       f'for example {stray[:5]}')
    notes.append(f'{len(written)} of {last} cuts written and released; {total[0]} blocks compared over them, '
                 f'{total[2]} of them discarded (either content), {total[1]} not known to the reference (a write '
                 f'that covers part of a block)')
    controls = {}
    for c in CONTENT_CONTROLS:
        wrong = ctl.get(c)
        controls[c] = ('caught', f'cut {mid}: {len(wrong)} blocks differ') if wrong \
            else ('missed', f'cut {mid}: as the reference')
    return not bad, '; '.join(bad + notes), controls, wrong_blocks


def reference(cs, op):
    return [{'txg': c['txg'], 'itself': op[c['n']]['itself'], 'prefix': c['prefix']} for c in cs]


def mutants(dcuts, barriers):
    """The controls' wrong lists of the daemon's cuts -> {name: list or None
    when it cannot be built}. barriers: the records of every barrier, in
    order."""
    if not dcuts:
        return {c: None for c in LISTS}
    mid = len(dcuts) // 2

    def after(prefix):
        return next((b for b in barriers if b > prefix), None)
    out = {
        'drop': dcuts[:mid] + dcuts[mid + 1:],
        'last': dcuts[:-1],
        'extra': dcuts + [dict(dcuts[-1], prefix=after(dcuts[-1]['prefix']) or dcuts[-1]['prefix'] + 1)],
        'group': [dict(c, txg=c['txg'] + 1) if k == mid else c for k, c in enumerate(dcuts)],
        'opens': [dict(c, itself=not c['itself']) if k == mid else c for k, c in enumerate(dcuts)],
    }
    nxt = after(dcuts[mid]['prefix'])
    out['late'] = None if nxt is None else [dict(c, prefix=nxt) if k == mid else c for k, c in enumerate(dcuts)]
    return out


def unplaced(records, dev_bytes):
    """What the log does not place where the store applied it -> [str]."""
    labels = cut_ref.label_offsets(dev_bytes)
    out = []
    n = sum(1 for r in records if r.get('op') == 'fua_flush')
    if n:
        out.append(f'{n} flushes after a write with FUA')
    n = sum(1 for r in records if r.get('op') == 'write_zeroes'
            and cut_ref.touches_label(r['offset'], r['bytes'], labels))
    if n:
        out.append(f'{n} write-zeroes over a label')
    return out


def read_status(text):
    out = {}
    for ln in text.splitlines():
        k, _, v = ln.partition(' ')
        if k:
            out[k] = v.strip()
    return out


def status_window(status, text):
    """status-time.tsv (`BEFORE_NS AFTER_NS` of the read) and the status's
    age_ms -> the latest monotonic time at which the status can have
    been written."""
    f = text.split()
    age = status.get('age_ms', '')
    if len(f) != 2 or not all(x.isdigit() for x in f) or not age.isdigit():
        raise RigError(f'no time for the status: status-time.tsv {text.strip()!r}, age_ms {age!r}')
    before, after = int(f[0]), int(f[1])
    if after < before:
        raise RigError(f'status-time.tsv: the read ended before it began ({before} {after})')
    return after - int(age) * 1_000_000 + CLOCK_NS


def status_verdict(status, ref, times=None, latest=None):
    """status.txt's values and the reference's cuts -> (ok, detail).

    The values are held to the last cut. Where they differ, times (of the
    daemon's cut lines) and latest (status_window) say whether the status
    was written before the last cut line."""
    want = {'cuts': str(len(ref))}
    if ref:
        want['cut_group'] = str(ref[-1]['txg'])
        want['cut_opens'] = 'itself' if ref[-1]['itself'] else 'rewind'
    bad = [f'{k} {status.get(k)!r}, not {v!r}' for k, v in want.items() if status.get(k) != v]
    if not bad:
        return True, ', '.join(f'{k} {v}' for k, v in want.items())
    if latest is not None and times and latest < times[-1]:
        k = sum(1 for t in times if t <= latest)
        bad.append(f'the status was written before the daemon\'s last cut line, after at most {k} of '
                   f'{len(times)}, at least {(times[-1] - latest) / 1e6:.0f} ms before it, and read a second '
                   'after the export')
    return False, '; '.join(bad)


def read_file(rdir, name):
    try:
        with open(os.path.join(rdir, name)) as f:
            return f.read()
    except FileNotFoundError:
        raise RigError(f'no {name}: the holder did not get as far') from None


def held_lines(put, rdir, inst, records, dcuts, status, dev, name=''):
    """The second stage's lines, where the holder ran -> (1 when a line is a
    verdict against the daemon, else 0; whether a line is the rig's ERROR)."""
    rc, errored = 0, False
    log = read_file(rdir, 'holder.log')
    err = [ln for ln in log.splitlines() if '\tERROR ' in ln]
    if err:
        raise RigError(f"the holder: {err[0].split(chr(9), 1)[1]}")
    # The state: a new volume's first start begins one, by the log's
    # `cuts` line and by the status.
    said = [ln['result'] for ln in inst['lines'] if ln['event'] == 'cuts']
    st = status.get('cut_state')
    ok = said == ['begun'] and st == 'begun'
    put('cut_state', 'PASS' if ok else 'FAIL', f"the request log's cuts line says {said or 'nothing'}, the status "
        f"{st!r}; a new volume's first start with --hold-cuts begins a state")
    rc = rc if ok else 1
    # cuts_held against the list, both read after the export
    final = read_file(rdir, 'held-final.txt').splitlines()
    sh = read_status(read_file(rdir, 'status-held.txt'))
    if any(ln.startswith('refused') for ln in final):
        raise RigError(f'held at the stop was refused: {final}')
    ok = sh.get('cuts_held') == str(len(final))
    put('cuts_held', 'PASS' if ok else 'FAIL', f"the status says {sh.get('cuts_held')!r} held, `held` listed "
        f"{len(final)} ({'; '.join(final) or 'none'}), a second and a half apart after the export")
    rc = rc if ok else 1
    written = read_held(read_file(rdir, 'held.tsv'))
    blocks = read_hashes(read_file(rdir, 'held-blocks.tsv'), 'held-blocks.tsv')
    final_image = read_hashes(read_file(rdir, 'held-image.tsv'), 'held-image.tsv', keyed=False)
    ok, detail, controls, wrong = held_content(records, dcuts, written, blocks, final_image)
    amb = set().union(*(r.get('ambiguous', set()) for r in records))
    if not ok and wrong & amb:
        put('held_content', 'ERROR', f'{detail}; blocks {sorted(wrong & amb)[:5]} were under a write-zeroes '
            'received while an overlapping write was in the store, which the check cannot place')
        errored = True
    else:
        put('held_content', 'PASS' if ok else 'FAIL', detail)
        rc = rc if ok else 1
    # The paths the ten recordings do not take (the lead, M2.T3): bound
    # must abandon a cut, edges must send write-zeroes and a discard.
    ab = status.get('cuts_abandoned')
    if name == 'bound':
        ok = ab is not None and ab.isdigit() and int(ab) >= 1
        put('cuts_abandoned', 'PASS' if ok else 'FAIL', f'{ab!r} abandoned at the bound, with the copies '
            f"{status.get('cut_copied_bytes')!r} bytes of {status.get('cut_bound_bytes')!r}; the holder kept the "
            'first cut while the pool wrote 320 MiB of new data, so at least one must be')
        rc = rc if ok else 1
    else:
        put('cuts_abandoned', 'INFO', f'{ab!r}')
    cp = copied(records, dcuts, written, blocks)
    first, later = cp[:1], cp[1:]
    put('obs_copied_blocks', 'INFO', f'{sum(c for _, c, _ in first)} blocks of the first written cut and '
        f'{sum(c for _, c, _ in later)} of the {len(later)} written after it (in cuts '
        f"{', '.join(str(n) for n, c, _ in later if c) or 'none'}; {sum(z for _, _, z in cp)} under a write-zeroes) "
        'had changed in the home between the cut\'s instant and its write: the daemon wrote them from copies')
    if name == 'edges':
        nz = sum(1 for r in records if r.get('op') == 'write_zeroes')
        nt = sum(1 for r in records if 'trim' in r['flags'])
        if not nz or not nt:
            raise RigError(f'edges sent {nz} write-zeroes and {nt} discards; it is there to send both')
        held_over = [(n, z) for n, _, z in later if z]
        if not held_over:
            raise RigError('edges holds a cut across its write-zeroes, and no cut written after the first had a '
                           'block a write-zeroes changed before its write')
        put('edges', 'INFO', f'{nz} write-zeroes and {nt} discards in the log, none of them from ZFS; held across '
            f"the write-zeroes: {', '.join(f'cut {n} ({z} blocks)' for n, z in held_over)}")
    for c in CONTENT_CONTROLS:
        verdict, d = controls[c]
        good = verdict == CONTENT_EXPECTED[c]
        put(f'ctl_{c}', verdict.upper(), f"{d} (expected: {CONTENT_EXPECTED[c]}{'' if good else '; DIFFERS'})")
        rc = rc if good else 1
    return rc, errored


def main():
    if len(sys.argv) not in (6, 7):
        sys.exit(__doc__)
    name, rdir, log = sys.argv[1:4]
    dev = int(sys.argv[5])
    t0 = time.monotonic()
    lines = []

    def put(key, verdict, detail):
        lines.append(f'{key}\t{verdict}\t{flat(detail)}')
        print(f'{name}: {key}: {verdict}: {flat(detail)}', flush=True)

    rc, errored = 0, False
    try:
        if name not in RECS:
            raise RigError(f'no row of EXPECTED for the recording {name!r}')
        with open(log) as f:
            instances = parse_log(f.read())
        if len(instances) != 1:
            raise RigError(f'{len(instances)} instances of the daemon in the request log; this stage reads one')
        marks_path = os.path.join(rdir, 'marks.tsv')
        with open(marks_path) as f:
            marks = read_marks(f.read())
        records, dcuts = records_of(instances[0]['lines'], marks)
        read = read_of(records, dev)
        ubs = cut_ref.scan(records, read, dev)
        cs, taken = cut_ref.cuts(records, ubs)
        op = cut_ref.opens(records, read, dev, cs)
        ref = reference(cs, op)
        put('records', 'INFO', f"{len(records)} records from the request log ({sum(1 for r in records if 'data' in r)} "
            f"with their bytes), {sum(1 for u in taken if u['taken'] == 'marks')} uberblock writes that mark, of "
            f"{len(taken)}; {sum(1 for r in records if 'mark' in r['flags'])} marks")
        ok, detail = compare(dcuts, ref)
        cannot = [] if ok else unplaced(records, dev)
        if cannot:
            put('cuts', 'ERROR', f"{detail}; the log holds what the check cannot place ({', '.join(cannot)})")
            errored = True
        else:
            put('cuts', 'PASS' if ok else 'FAIL', detail)
        if not ok and not cannot:
            rc = 1
        verdict, detail = numbers_verdict(dcuts)
        put('cut_numbers', verdict, detail)
        if verdict == 'FAIL':
            rc = 1
        n_self = sum(1 for r in ref if r['itself'])
        put('obs_opens_as_itself', 'INFO', f'{n_self} of {len(ref)} cuts open as themselves, by the reference')
        with open(os.path.join(rdir, 'status.txt')) as f:
            status = read_status(f.read())
        with open(os.path.join(rdir, 'status-time.tsv')) as f:
            latest = status_window(status, f.read())
        ok, detail = status_verdict(status, ref, [c['ns'] for c in dcuts], latest)
        put('status', 'PASS' if ok else 'FAIL', detail)
        if not ok:
            rc = 1
        if os.path.exists(os.path.join(rdir, 'holder.log')):
            v, e = held_lines(put, rdir, instances[0], records, dcuts, status, dev, name)
            rc, errored = max(rc, v), errored or e
        # What ZFS says it synced, as the cut rig holds the reference to it.
        kstats = {}
        for fn in os.listdir(rdir):
            m = re.fullmatch(r'txgs-(\d+)\.txt', fn)
            if m:
                with open(os.path.join(rdir, fn)) as f:
                    kstats[int(m.group(1))] = cut_check.committed_txgs(f.read())
        sv = cut_check.session_verdicts(cs, records, kstats)
        if any(v is None for _, v, _ in sv):
            raise RigError('; '.join(d for _, v, d in sv if v is None))
        bad = [d for _, v, d in sv if not v]
        covered, order = cut_check.outside_kstat(cs, records, kstats)
        bad += order[:5]
        put('sessions', 'PASS' if not bad else 'FAIL', '; '.join(bad) if bad else
            f'{len(sv)} sessions as committed; {covered} cuts within the kstats\' windows, the others in strictly '
            'increasing groups')
        if bad:
            rc = 1
        sb = cut_check.synced_before_ready(cs, records)
        if any(n is None for _, n, _ in sb):
            raise RigError('; '.join(d for _, n, d in sb if n is None))
        none = [d for _, n, d in sb if n == 0]
        put('import_syncs_before_return', 'PASS' if not none else 'FAIL',
            '; '.join(d for _, _, d in sb) if not none else '; '.join(none))
        if none:
            rc = 1
        # The controls: each wrong list must fail the comparison.
        got = {}
        barriers = [r['i'] for r in records if 'flush' in r['flags']]
        for ctl, wrong in mutants(dcuts, barriers).items():
            if wrong is None:
                got[ctl] = ('not built', 'no cut, or no barrier after the middle one')
            else:
                ok, detail = compare(wrong, ref)
                got[ctl] = ('missed' if ok else 'caught', detail)
        for rule in RULES:
            rcs, _ = cut_ref.cuts(records, ubs, rule)
            rref = reference(rcs, cut_ref.opens(records, read, dev, rcs))
            if rule != 'early' and compare(rref, ref)[0]:
                got[rule] = ('same', f'the rule {rule} gives the design\'s {len(ref)} cuts here')
                continue
            ok, detail = compare(dcuts, rref)
            got[rule] = ('missed' if ok else 'caught', f'the reference by the rule {rule}: {detail}')
        for ctl in CONTROLS:
            verdict, detail = got[ctl]
            want = EXPECTED[ctl][name]
            good = verdict in want if isinstance(want, tuple) else verdict == want
            want = ' or '.join(want) if isinstance(want, tuple) else want
            put(f'ctl_{ctl}', verdict.upper().replace(' ', '_'),
                f"{detail} (expected: {want}{'' if good else '; DIFFERS'})")
            if not good:
                rc = 1
    except RigError as e:
        put('rig_error', 'ERROR', str(e))
        rc = 2
    except Exception as e:  # noqa: BLE001 -- anything else is the rig's too, not a verdict
        put('rig_error', 'ERROR', f'{type(e).__name__}: {e}')
        rc = 2
    if errored and rc == 0:
        rc = 2
    put('time', 'INFO', f'{time.monotonic() - t0:.0f} s')
    with open(os.path.join(rdir, 'results.tsv'), 'a') as f:
        f.write(''.join(ln + '\n' for ln in lines))
    sys.exit(rc)


if __name__ == '__main__':
    main()
