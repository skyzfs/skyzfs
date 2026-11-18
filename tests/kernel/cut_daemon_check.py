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
  1 to N in order, as one instance counts, and from 1 again in each
  state a start that acknowledges a failure begins; INFO on a log
  without them;
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
- `held_reads`: the daemon read each cut it held when the holder asked
  for its write. A write it refused with `read block B of cut N` while
  the cut was still being uploaded is a verdict against it (the
  holder's log says VERDICT), and the lines below are not written; a
  refusal for the rig's image is the rig's error;
- `cut_state`: the request log's `cuts` line and the status both say
  `begun`, as a new volume's first start with --hold-cuts does, and
  `taken_up` for each start after it (stage 2b);
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
  (DIR/held-image.tsv). A failed `held_content` is the rig's error
  (ERROR) only where every wrong block is under a write-zeroes received
  while an overlapping write was in the store (the store ran the write
  first; the log does not say when the write-zeroes ran), in a cut after
  it, with no write or write-zeroes received after the write-zeroes
  setting the block again by the cut's instant, and nothing else is
  wrong.
- controls: the middle written cut's image against the reference at the
  cut before it (`content_early`), and the image before its write
  against the reference at its instant (`content_stale`): both caught.
- `obs_copied_blocks`: how many blocks of each written cut's map a
  write applied, or a write-zeroes answered, after the cut's instant
  and before the holder asked for its write had left with content other
  than the cut's
  (held.tsv's times, the request log's clock): those the daemon wrote
  from copies, so that held_content is shown to have read some. The
  first written cut maps the whole volume and holds nearly all of them.
- `cuts_abandoned`: at least one in `bound`; `edges`: at least one
  write-zeroes and one discard, and a cut written after the first that
  had a block a write-zeroes changed before its write (the rig's
  error otherwise: the recording did not hold a cut across it).

Stage 2b, a daemon started again on the volume (the recording
`takeover`): the request log holds one instance after another, and
DIR/instances.tsv says for each pid whether it took the kernel's device
over or created it (from serve's line "serving device N (taken over)"
or "(added)"). They are read as one record, with a record `instance`
where each later one starts (cut_ref.cuts says what it does to the
mark: kept from the cut state at a take-over, dropped on a created
device; a flush received and not at its barrier carries nothing on, as
the kernel hands it to the next instance again, and what waited behind
it is placed when that one runs it). Then:
- `cuts`, `cut_numbers`, `status` and `held_content` across the
  instances: the numbers count on, and a cut held across a start is
  written by the holder the rig starts beside the next instance (onto
  the same image, the same files);
- `instance_K`: the K-th start's `cuts` line, and its status read once
  it was ready (DIR/status-start-K.txt): `begun` for the first, then
  `taken_up` for the cut state and the intent map, and `cuts` no lower
  than the cuts counted before it; `cuts_held` at least 1 at a start
  that takes the state up where the newest cut counted before it was
  not released before it (the holder's log); the intent map's regions
  named at the start, as information;
- the controls `mark-dropped` (no mark kept at a take-over) and
  `mark-kept` (the mark kept on a created device), caught in takeover,
  which makes a mark wait across each (zfs_nocacheflush, between the
  marks `noflush-begin` and `noflush-end`; the groups synced there that
  the reference cuts not are left out of `sessions`, obs_noflush_groups).
A cut counted and killed before its line was written is the rig's
error (the next instance's number says so); a log with more than one
instance and no holder is the rig's error too.

Stage 2c, a failure (the recording `failed`): the volume fails with cuts
held and no client holding its device; DIR/instances.tsv marks the start
after it `acknowledged`. That start trusts none of the cut state: its
`instance` record begins a new state (cut_ref: no mark, no newest cut),
`instance_K` wants `begun` for the cut state and `taken_up` for the
intent map, and its count of cuts may start from 1 again (the daemon's
does). The check then numbers the cut lines by their place in the whole
log (`epochs`); `cut_numbers` says which way each state counted, the
status's `cuts` is held to the new state's count where it started again,
and the holder's numbers are keyed by the state whose instance had
started when the write was asked for (`held_by_epoch`). Then:
- `failure`: serve ended with 3 at the failure, a start without
  --acknowledge-failure was refused with 4, and one with it served
  (DIR/failure.tsv, the rig's). The cuts held at the failure are lost
  with the instance (design §5.2 as of zfs-topology d26841d: uploading
  them is M2.5's); the first state's held cuts written before it, and
  the new state's after it, are judged by `held_content` on one image,
  so a new state's first cut that lacks a block changed since the last
  cut written is caught there.

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
        'bound', 'edges', 'takeover', 'failed',   # these four only where the daemon holds cuts (cut-record.sh)
        'umoved', 'urow', 'ukill', 'ulease', 'ulost',     # these where it uploads them
        'unostore', 'ukillput', 'ufailed')                # and these by name only
LISTS = ('drop', 'last', 'extra', 'group', 'opens', 'late')
RULES = ('early', 'txg-alone', 'count-repeats', 'previous', 'mark-dropped', 'mark-kept')
CONTROLS = LISTS + RULES
# By recording. The rules' columns are the runs on 192427b and 2c67c42's
# first run (43675946002, 43676360281 on both images): txg-alone tells
# the rewinds whose new history writes a group the old one used (and
# rewindsync and multihost in dm-log-writes' runs, cut_check); count-
# repeats tells a flush between copies of an uberblock (multihost
# always; stray in two of three, its flushes being the rig's, every
# 2 ms); previous tells a stale multihost write (one of three).
# takeover's columns for the three rules are not known before its first
# run, and say so. mark-dropped and mark-kept tell a recording with more
# than one instance of the daemon (stage 2b), and takeover is made to
# hold a mark across a take-over and one across a created device.
# failed (stage 2c) is new: its columns for the three rules are not known
# before its first run either; its later instance trusts no state, so
# neither mark rule changes it. The upload courses (stage 3) are new
# too: none of the three rules' columns is known before their first run,
# nor the mark rules' for those with a second instance (ukill and
# ukillput take the device over with a writer on the pool; ulease's
# second instance is added after a clean end, ufailed's acknowledges a
# failure).
TWO_STARTS = ('ukill', 'ulease', 'ukillput', 'ufailed')     # the upload courses that serve twice
UNKNOWN = {r: ('same', 'caught') for r in ('umoved', 'urow', 'ukill', 'ulease', 'ulost', 'unostore', 'ukillput',
                                          'ufailed')}
EXPECTED = dict({c: {r: 'caught' for r in RECS} for c in LISTS + ('early',)}, **{
    'txg-alone': dict({r: 'same' for r in RECS}, rewind='caught', rewindf='caught',
                      rewindsync=('same', 'caught'), multihost=('same', 'caught'), takeover=('same', 'caught'),
                      failed=('same', 'caught'), **UNKNOWN),
    'count-repeats': dict({r: 'same' for r in RECS}, multihost='caught', stray=('same', 'caught'),
                          takeover=('same', 'caught'), failed=('same', 'caught'), **UNKNOWN),
    'previous': dict({r: 'same' for r in RECS}, multihost=('same', 'caught'), takeover=('same', 'caught'),
                     failed=('same', 'caught'), **UNKNOWN),
    'mark-dropped': dict({r: 'same' for r in RECS}, takeover='caught',
                         **{r: ('same', 'caught') for r in TWO_STARTS}),
    'mark-kept': dict({r: 'same' for r in RECS}, takeover='caught', **{r: ('same', 'caught') for r in TWO_STARTS}),
})
CLOCK_NS = 10_000_000       # the tick of /proc/uptime, which age_ms is a difference of
SECTOR = 512
BLOCK = 4096
ZERO = hashlib.sha256(bytes(BLOCK)).digest()[:8].hex()
CONTENT_CONTROLS = ('content_early', 'content_stale')
CONTENT_EXPECTED = {c: 'caught' for c in CONTENT_CONTROLS}


def flat(text):
    return ' '.join(str(text).split())


# The next instance's start line, after a torn line (torn_line).
# The daemon writes its numbers with no leading zero.
START_LINE = re.compile(r'([1-9]\d*)\t([1-9]\d*)\tstart\t(\d+)\t-\t-\t-\t(\d+)')
# A kill -9 stops a write(2) of the page cache between two pages: what was
# written of a line ends at a multiple of this in the file. The hosted
# images have 4 KiB pages; a larger page is a multiple of it.
PAGE = 4096
# What is left of a torn line: its time (no leading zero, as the daemon
# writes it), then perhaps its pid, whole or not, then perhaps more of its
# columns.
TORN_HEAD = re.compile(r'([1-9]\d*)(?:\t(\d*)(\t.*)?)?', re.S)


def torn_line(ln, k, lines, offset, instances, by_pid):
    """Line ln (index k of lines, at byte offset of the file): where it is the last instance's line
    cut short by a kill and the next instance's start line follows it with
    no newline between them, -> (the start line, what was lost); else None.

    A line is one write(2) of the daemon (record.rs), but a kill -9 can stop
    a write of a long line (a label line is some 8 KB) between two pages of
    the file: the rest of that line never reaches the file, and the next
    instance appends its start line to it. So the start line must begin at
    a multiple of PAGE in the file and end the line, be of a pid not yet
    started, and lie in time between the last instance's last whole line
    and the line after it; what is left before it must be the beginning of
    a line of the last instance (its time, or a time and that pid, whole or
    cut, or more), and where its time is whole, lie between the two as
    well. A tear inside the time column leaves digits that, with the start
    line, parse as a start line with a longer time: the page bound tells
    the two apart.

    The rule holds while the daemon writes its request log from one thread,
    as the test build does: the torn line is then its last. With more
    writers a torn line can be followed by whole lines of the same daemon
    (the review of #113 saw it with three writer threads), which parse_log
    takes as the rig's error."""
    if not instances or k + 1 >= len(lines):
        return None
    last = instances[-1]
    nxt = lines[k + 1].split('\t', 1)[0]
    # A time no longer than the next line's: a boundary pages before the
    # tear can see thousands of digits, which int() refuses (the review of
    # #113, T3). A next line whose time has more than 20 digits is no
    # daemon's, and parse_log refuses it: none is taken here, where int()
    # would refuse it first.
    if not nxt.isdigit() or len(nxt) > 20:
        return None
    before = last['lines'][-1]['ns'] if last['lines'] else last['ns']
    for p in range(-offset % PAGE or PAGE, len(ln), PAGE):
        lost, rest = ln[:p], ln[p:]
        m = START_LINE.fullmatch(rest)
        h = TORN_HEAD.fullmatch(lost)
        if not m or m.group(2) in by_pid or not h:
            continue
        if h.group(3) is not None and h.group(2) != last['pid']:
            continue
        if h.group(2) is not None and not last['pid'].startswith(h.group(2)):
            continue
        if len(m.group(1)) > len(nxt) or len(h.group(1)) > len(nxt):
            continue
        ns = int(m.group(1))
        if not before <= ns <= int(nxt):
            continue
        if h.group(2) is not None and not before <= int(h.group(1)) <= ns:
            continue
        return rest, lost
    return None


def torn_detail(torn, kinds):
    """The lines parse_log found cut short, and instances.tsv's kinds ->
    the line obs_torn_at_kill's detail. A line cut short is lost at a kill
    only: the instance after it took the device over from a daemon the rig
    killed; anywhere else it is the rig's error."""
    for c in torn:
        if not (kinds or {}).get(c['next']):
            raise RigError(f"request log line {c['line']}: pid {c['pid']}'s line is cut short and pid "
                           f"{c['next']}'s start follows it, but pid {c['next']} did not take the device over "
                           'from a killed daemon (instances.tsv)')
    return (f'{len(torn)} lines of the request log cut short by a kill -9 and lost with it'
            + ''.join(f"; line {c['line']}: pid {c['pid']}'s `{c['head']}` line after {c['bytes']} bytes, then "
                      f"pid {c['next']}'s start" for c in torn))


def parse_log(text, torn=None):
    """The request log -> [instance], each {'pid', 'device', 'lines': [dict]} in the file's order.

    torn: where given, a list to which each line cut short by a kill is
    added ({'line', 'pid', 'next', 'bytes', 'head'}: torn_line); the line
    is lost at the kill, as the lines the daemon had not written. main
    holds that the next instance took the device over (instances.tsv).
    Where not given, or anywhere else, a line that does not parse is the
    rig's error."""
    instances, by_pid, cut_short = [], {}, set()
    lines = text.split('\n')
    if lines and lines[-1] == '':
        lines.pop()
    offset = 0
    for k, ln in enumerate(lines):
        at, offset = offset, offset + len(ln.encode()) + 1
        f = ln.split('\t')
        if torn is not None:
            cut = torn_line(ln, k, lines, at, instances, by_pid)
            if cut is not None:
                start, lost = cut
                torn.append({'line': k + 1, 'pid': instances[-1]['pid'], 'next': start.split('\t')[1],
                             'bytes': len(lost), 'head': '\t'.join(lost.split('\t')[:7])})
                cut_short.add(instances[-1]['pid'])
                f = start.split('\t')
        # Eight columns; since M2.2b-2 an `applied` line has a ninth (the
        # blocks' hashes) and, with --request-log-content whole, a tenth.
        if not 8 <= len(f) <= 10 or not f[0].isdigit():
            raise RigError(f'request log line {k + 1}: not eight to ten columns: {ln[:120]!r}')
        # The daemon writes a u64 of nanoseconds: 20 digits at most.
        if len(f[0]) > 20:
            raise RigError(f'request log line {k + 1}: a time of {len(f[0])} digits: {ln[:120]!r}')
        if len(f) > 8 and f[2] != 'applied':
            raise RigError(f'request log line {k + 1}: a {f[2]} line with more than eight columns: {ln[:120]!r}')
        rec = {'ns': int(f[0]), 'pid': f[1], 'event': f[2], 'tag': f[3], 'op': f[4], 'sector': f[5],
               'sectors': f[6], 'result': f[7], 'extra': f[8:], 'line': k + 1}
        if rec['pid'] in cut_short:
            raise RigError(f'request log line {k + 1}: pid {rec["pid"]} writes after its line cut short by a '
                           'kill (torn_line): the rule holds while the daemon writes its log from one thread')
        if rec['event'] == 'start':
            nxt = lines[k + 1].split('\t', 1)[0] if k + 1 < len(lines) else ''
            if nxt.isdigit() and len(nxt) <= 20 and rec['ns'] > int(nxt):
                raise RigError(f'request log line {k + 1}: a start line later than the line after it, as of a '
                               f'line cut short in its time: {ln[:120]!r}')
            if rec['pid'] in by_pid:
                raise RigError(f'request log line {k + 1}: a second start line of pid {rec["pid"]}')
            inst = {'pid': rec['pid'], 'device': rec['tag'], 'lines': [], 'ns': rec['ns'], 'line': k + 1}
            instances.append(inst)
            by_pid[rec['pid']] = inst
            continue
        inst = by_pid.get(rec['pid'])
        if inst is None:
            raise RigError(f'request log line {k + 1}: pid {rec["pid"]} has no start line')
        inst['lines'].append(rec)
    return instances


def read_kinds(text):
    """instances.tsv (the rig's: `PID taken-over|added [acknowledged]` for
    each serve, from its line "serving device N (...)", the third column
    where the serve acknowledged a failure) -> {pid: whether it took the
    device over}."""
    out = {}
    for ln in text.splitlines():
        f = ln.split('\t')
        if len(f) not in (2, 3) or not f[0].isdigit() or f[1] not in ('taken-over', 'added') \
                or f[2:] not in ([], ['acknowledged']):
            raise RigError(f'instances.tsv: not a pid, taken-over or added, and maybe acknowledged: {ln!r}')
        out[f[0]] = f[1] == 'taken-over'
    return out


def read_acknowledged(text):
    """instances.tsv -> the pids of the serves that acknowledged a failure."""
    read_kinds(text)
    return {ln.split('\t')[0] for ln in text.splitlines() if ln.endswith('\tacknowledged')}


def log_lines(instances, kinds=None, acked=frozenset()):
    """The instances' lines in the file's order, each instance after the
    first behind a line `instance` that says whether it took the device
    over (kinds, read_kinds) and whether it begins a state, trusting none
    (acked, read_acknowledged: it acknowledged a failure). Instances follow one another in the file:
    a serve begins once the one before has ended (killed or stopped)."""
    if len(instances) > 1 and kinds is None:
        raise RigError(f'{len(instances)} instances of the daemon in the request log and no instances.tsv '
                       'to say which took the device over')
    out = []
    for k, inst in enumerate(instances):
        if k:
            if inst['pid'] not in kinds:
                raise RigError(f"instances.tsv does not say whether pid {inst['pid']} took the device over")
            prev = instances[k - 1]['lines']
            if prev and prev[-1]['line'] > inst['line']:
                raise RigError(f"request log line {inst['line']}: pid {inst['pid']} started before pid "
                               f"{instances[k - 1]['pid']}'s last line (line {prev[-1]['line']})")
            out.append({'event': 'instance', 'ns': inst['ns'], 'pid': inst['pid'], 'line': inst['line'],
                        'taken_over': kinds[inst['pid']], 'begun': inst['pid'] in acked})
        out.extend(inst['lines'])
    return out


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
        if ev == 'instance':
            # The daemon before this one ended (log_lines). What it had
            # received and not finished, the kernel hands to this one
            # again: a flush with no barrier carries nothing on, and what
            # waited behind it did not run; a write with no `applied`
            # line is placed when this one applies it.
            received.clear()
            pending.clear()
            data.clear()
            zeroing.clear()
            last_barrier = None
            add({'flags': ['instance'], 'offset': 0, 'bytes': 0, 'taken_over': ln['taken_over'], 'pid': ln['pid'],
                 'ns': ln['ns'], 'begun': ln.get('begun', False)})
            continue
        if ev == 'recv':
            if not (ln['sector'].isdigit() and ln['sectors'].isdigit()):
                raise RigError(f"request log line {ln['line']}: a recv without its sectors")
            off, n = int(ln['sector']) * SECTOR, int(ln['sectors']) * SECTOR
            if ln['op'] == 'write':
                pending[ln['tag']] = (off, n, ln['line'])
                data.pop(ln['tag'], None)
            elif ln['op'] == 'write_zeroes':
                # Placed at its receipt, or behind a flush (below). A write
                # received before it that overlaps it and is not applied
                # yet is applied first by the store (both in the order
                # received) and after it here: their shared blocks are not
                # placed for certain.
                lo, hi = off // BLOCK, (off + n - 1) // BLOCK
                amb = {b for (o, m, _) in pending.values() for b in range(max(lo, o // BLOCK),
                                                                          min(hi, (o + m - 1) // BLOCK) + 1)}
                # seq: its recv line, to tell the writes received after it
                z = {'flags': ['discard'], 'offset': off, 'bytes': n, 'op': ln['op'], 'ambiguous': amb,
                     'seq': ln['line']}
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
            off, n, seq = pending.pop(ln['tag'])
            r = {'flags': [], 'offset': off, 'bytes': n, 'pos': None, 'tag': ln['tag'], 'ns': ln['ns'], 'seq': seq}
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


def epochs(dcuts, records):
    """Each cut line's state (stage 2c): a later instance that begins a
    state (it acknowledged a failure) begins an epoch -> the instance
    records that begin one. Sets each line's `epoch` (0 for the first
    state), `local` (the number the line carries) and `number`, its place
    in the whole log (1, 2, ...) where the lines carry numbers, which the
    reference counts the same way (cut_ref) and the held cuts are keyed
    by (held_by_epoch). With one state the lines keep their own numbers."""
    starts = [r for r in records if 'instance' in r['flags'] and r.get('begun')]
    for k, d in enumerate(dcuts):
        d['epoch'] = sum(1 for r in starts if r['i'] < d['prefix'])
        d['local'] = d['number']
        if starts and d['number'] != '-':
            d['number'] = str(k + 1)
    return starts


def numbers_verdict(dcuts):
    """The `cut` lines' numbers (M2.2b-2's sectors column; `-` in a log
    from before it) -> (verdict, detail). One instance that held no cuts
    before counts from 1: the k-th line carries k. An instance that took
    the state up counts on (record.rs), so across the instances of one
    volume the k-th line carries k too. A state begun by a later instance
    (epochs) counts from 1 again: a cut is named by its state, its number
    and its group, so a number means something only within its state
    (the lead's decision on #113). A state that counts on fails."""
    nums = [d.get('local', d['number']) for d in dcuts]
    if all(n == '-' for n in nums):
        return 'INFO', 'the cut lines carry no numbers (a daemon from before M2.2b-2)'
    said, k0, epoch = [], 0, 0
    for k, n in enumerate(nums):
        e = dcuts[k].get('epoch', 0)
        if e != epoch:
            epoch = e
            if n != '1':
                return 'FAIL', (f"state {e + 1}'s first cut line (request log line {dcuts[k]['line']}) "
                                f"carries the number {n!r}, not 1: a new state counts from 1 again")
            k0 = k
            said.append(f'state {e + 1} counts from 1 again')
        if n != str(k - k0 + 1):
            return 'FAIL', f"cut line {k + 1} (request log line {dcuts[k]['line']}) carries the number {n!r}"
    return 'PASS', f'the {len(nums)} cut lines are numbered 1 to {len(nums)}' + (
        f" ({'; '.join(said)})" if said else '')


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
    blocks, of them under a write-zeroes)]: the blocks of its map whose
    content in the home a write applied, or a write-zeroes answered,
    after the cut's instant and before its write was asked for had made
    differ from what the cut holds (a block rewritten with the same
    content reads the same from either). The review of #103 (S4): a
    daemon that makes no copy passes held_content where there are none."""
    by_number = {int(d['number']): d for d in dcuts if d['number'] != '-'}
    out = []
    for w in written:
        n = w['number']
        held = blocks.get(n, {})
        d = by_number.get(n)
        if d is None:
            continue
        hit, zeroed = set(), set()
        for r in records[d['prefix'] + 1:]:
            if r.get('op') == 'write_zeroes':
                if r.get('done_ns', w['asked']) < w['asked']:
                    zeroed |= {b for b in full_blocks(r['offset'], r['bytes'])[0]
                               if b in held and held[b] != ZERO}
            elif cut_ref.is_write(r) and 'ns' in r and r['ns'] < w['asked']:
                hs = r.get('hashes')
                first = r['offset'] // BLOCK
                for k, b in enumerate(range(first, (r['offset'] + r['bytes'] - 1) // BLOCK + 1)):
                    # a block covered in part, or a log with no hashes: changed, as far as is known
                    if b in held and (hs is None or hs[k] is None or hs[k] != held[b]):
                        hit.add(b)
        out.append((n, len(hit | zeroed), len(zeroed)))
    return out


def held_across_start(records, dcuts, written, blocks):
    """The cuts written by a later instance's holder than the instance that
    counted them -> [(number, counted by, written by, blocks)], instances
    counted from 1 and blocks those of the cut's map whose content in the
    home, as the last write before the next instance started left it,
    differs from the cut's: the next instance read them from copies an
    earlier one made (the review of #111, G1). Only what is known counts:
    a block a write covered in part, or a write-zeroes touched, last in
    that span is not known, and a block written back to the cut's content
    is not counted (Codex on #115). Nor is a block a write-zeroes left
    unplaced (records_of's ambiguous set) and that a write received before
    it was applied to after it in the log: the store ran that write first,
    and the block may hold the zeros (the review of #115, R2)."""
    starts = [r for r in records if 'instance' in r['flags']]
    by_number = {int(d['number']): d for d in dcuts if d['number'] != '-'}
    out = []
    for w in written:
        d = by_number.get(w['number'])
        if d is None:
            continue
        counted = sum(1 for r in starts if r['i'] < d['prefix'])
        wrote = sum(1 for r in starts if r['ns'] < w['asked'])
        if wrote <= counted:
            continue
        end = starts[counted]['i']
        held = blocks.get(w['number'], {})
        home = {}               # block -> its content as last written, None where not known
        unplaced = {}           # block -> the recv line of the write-zeroes that left it unplaced
        for r in records[d['prefix'] + 1:end]:
            zeroes = r.get('op') == 'write_zeroes'
            if not (cut_ref.is_write(r) or zeroes):
                continue
            hs = None if zeroes else r.get('hashes')
            seq = r.get('seq')
            first = r['offset'] // BLOCK
            for k, b in enumerate(range(first, (r['offset'] + r['bytes'] - 1) // BLOCK + 1)):
                # a write received before the write-zeroes ran before it
                before = not zeroes and seq is not None and b in unplaced and seq < unplaced[b]
                if b in held:
                    home[b] = None if hs is None or before else hs[k]
            if zeroes:
                for b in r.get('ambiguous', ()):
                    unplaced[b] = seq
        hit = sum(1 for b, h in home.items() if h is not None and h != held[b])
        out.append((w['number'], counted + 1, wrote + 1, hit))
    return out


def across_failure(dcuts, written, blocks):
    """The first cut written of a state that an acknowledged start began,
    against the image the holders of the state before left -> (its number,
    its blocks, those that differ from that image), or None where no such
    cut was written. The start trusts none of the old state, so this cut
    holds whole every region the intent map names: what differs is what a
    cut that lacked them would get wrong (the review of #113, G2)."""
    by_number = {int(d['number']): d for d in dcuts if d['number'] != '-'}
    image = {}
    for w in written:
        d = by_number.get(w['number'])
        if d is None:
            continue
        mine = blocks.get(w['number'], {})
        if d.get('epoch', 0) == 0:
            image.update(mine)
            continue
        return w['number'], len(mine), sum(1 for b, h in mine.items() if image.get(b, ZERO) != h)
    return None


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


def held_by_epoch(written, text, dcuts, starts):
    """Where a later state began (epochs) its count may start again, so
    the holder's numbers repeat: each written cut's state is the one whose
    instance had started when its write was asked for, and its blocks are
    the next `blocks` lines of held-blocks.tsv, which the holder writes
    just before each line of held.tsv -> (written, blocks, stale): written
    and blocks keyed by the lines' place in the whole log, as `number`
    after epochs; stale, the writes of a cut that has no line in its state
    and a line of that number in a state before (number, state): a cut of
    a state the daemon should have trusted none of, still held after a
    start that acknowledged a failure (the review of #113, D2), which is a
    verdict and not the rig's error. Their blocks are not kept."""
    rows = text.splitlines()
    at, blocks, out, stale = 0, {}, [], []
    place = {(d['epoch'], d['local']): int(d['number']) for d in dcuts if d['number'] != '-'}
    for w in written:
        part = read_hashes('\n'.join(rows[at:at + w['blocks']]), 'held-blocks.tsv')
        at += w['blocks']
        if len(rows[at - w['blocks']:at]) != w['blocks'] or set(part) - {w['number']}:
            raise RigError(f"held-blocks.tsv does not hold the {w['blocks']} blocks of cut {w['number']} where "
                           'held.tsv puts them')
        e = sum(1 for r in starts if r['ns'] < w['asked'])
        n = place.get((e, str(w['number'])))
        if n is None and any((b, str(w['number'])) in place for b in range(e)):
            stale.append((w['number'], e + 1))
            continue
        if n is None:
            raise RigError(f"the holder wrote cut {w['number']} of state {e + 1}, which has no cut line in the "
                           'request log')
        out.append(dict(w, number=n, local=w['number'], epoch=e))
        blocks[n] = part.get(w['number'], {})
    if at != len(rows):
        raise RigError(f'held-blocks.tsv has {len(rows) - at} lines after the last cut held.tsv names')
    return out, blocks, stale


def held_content(records, dcuts, written, blocks, final=None, last=None):
    """The cuts the holder wrote, each against the reference's content at
    its instant -> (ok, detail, controls {name: (verdict, detail)}, every
    block held wrong). ok is None, not False, where the only thing wrong
    is blocks the check cannot place (the rig's error): blocks under a
    write-zeroes received while an overlapping write was in the store,
    in a cut whose instant is after it, and that no write received after
    the write-zeroes had set again by then.

    written: read_held; blocks: {number: {block: hash}} of each cut's map
    as the rig's image held it after the write; final: {block: hash} of
    the image's blocks that are not zeros after the drain; last: the
    number the last cut written must have, where not the last counted (an
    upload course whose daemon stops uploading on purpose)."""
    by_number = {}
    for k, d in enumerate(dcuts):
        if d['number'] != '-':
            by_number[int(d['number'])] = k
    if not written:
        raise RigError('the holder wrote no cut')
    image, model, at = {}, {}, 0
    bad, notes, total = [], [], [0, 0, 0]
    wrong_blocks, unplaced, definite = set(), set(), False
    live = {}       # block -> the recv line of the write-zeroes that left it unplaced
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
            definite = True
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
            r = records[at]
            apply(model, r)
            if r.get('op') == 'write_zeroes':
                # A write-zeroes received after another places the blocks
                # it covers whole again (it waits for the other in the
                # store), unless a write it overlaps was pending at its own
                # receipt: then they are its own to leave unplaced.
                for b in full_blocks(r['offset'], r['bytes'])[0]:
                    if b in live and r['seq'] > live[b]:
                        del live[b]
                for b in r['ambiguous']:
                    live[b] = r['seq']
            elif cut_ref.is_write(r) and 'seq' in r:
                # A write received after the write-zeroes ran after it in
                # the store too (it waits for an overlap): the block is
                # placed again.
                for b in range(r['offset'] // BLOCK, (r['offset'] + r['bytes'] - 1) // BLOCK + 1):
                    if b in live and r['seq'] > live[b]:
                        del live[b]
            at += 1
        compared, unknown, either, wrong = held_compare(model, image, limit=10 ** 9)
        here = {x[0] for x in wrong}
        wrong_blocks |= here
        unplaced |= here & set(live)
        definite = definite or bool(here - set(live))
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
    if last is None:
        last = int(dcuts[-1]['number']) if dcuts and dcuts[-1]['number'] != '-' else len(dcuts)
    if written[-1]['number'] != last:
        bad.append(f"the last cut written is {written[-1]['number']}, the last counted {last}")
        definite = True
    if final is not None:
        stray = [b for b in sorted(set(final) | {b for b, h in image.items() if h != ZERO})
                 if final.get(b, ZERO) != image.get(b, ZERO)]
        if stray:
            bad.append(f'{len(stray)} blocks of the image after the drain differ from what the maps wrote, '
                       f'for example {stray[:5]}')
            definite = True
    if unplaced:
        notes.append(f'blocks {sorted(unplaced)[:5]} ({len(unplaced)} in all) were held wrong under a write-zeroes '
                     'received while an overlapping write was in the store, which the check cannot place'
                     + ('' if definite else '; nothing else was wrong'))
    notes.append(f'{len(written)} of {last} cuts written and released; {total[0]} blocks compared over them, '
                 f'{total[2]} of them discarded (either content), {total[1]} not known to the reference (a write '
                 f'that covers part of a block)')
    controls = {}
    for c in CONTENT_CONTROLS:
        wrong = ctl.get(c)
        controls[c] = ('caught', f'cut {mid}: {len(wrong)} blocks differ') if wrong \
            else ('missed', f'cut {mid}: as the reference')
    ok = True if not bad else (False if definite else None)
    return ok, '; '.join(bad + notes), controls, wrong_blocks


def reapplied(inst, later):
    """Whether the mark the write whose `applied` line ends instance inst
    may set was not stored: instance later applies the write again (the
    same range, the same hashes), and no flush that later received before
    that cuts.

    A flush takes the mark out of the state at its receipt and cuts at
    its barrier, after the writes on the threads (inplace.rs, cuts.rs). A
    flush received before the second `applied` line had the stored mark,
    if there was one, whether its `cut` line comes before or after that
    line (the review of #116, D1: the write handed over first, the flush
    received while it was on the threads); a flush received after it has
    the mark of the second application. So the first line is taken out
    when the write is applied again and no cut line of an earlier flush
    follows, before a flush received after it cuts."""
    last = inst['lines'][-1]
    recv = next((ln for ln in reversed(inst['lines']) if ln['event'] == 'recv' and ln['tag'] == last['tag']), None)
    if recv is None or recv['op'] != 'write':
        return False
    rng, early, again = {}, set(), False
    for ln in later['lines']:
        if ln['event'] == 'cut':
            if not again or ln['tag'] in early:
                return False
            return True
        if ln['event'] == 'recv':
            rng[ln['tag']] = (ln['op'], ln['sector'], ln['sectors'])
            if ln['op'] == 'flush' and not again:
                early.add(ln['tag'])
            else:
                early.discard(ln['tag'])
        elif not again and ln['event'] == 'applied' \
                and rng.get(ln['tag']) == (recv['op'], recv['sector'], recv['sectors']) \
                and ln['extra'][:1] == last['extra'][:1]:
            again = True
    return again


def unstored_at_kill(instances, acked=frozenset()):
    """The instances, with what a kill left of a cut that the state never
    stored taken out -> (instances, [note]).

    At a flush's barrier the store writes the `barrier` line, counts in
    the rule and writes the `cut` line, then stores the count, the newest
    cut and the cleared mark in its state, and only then answers the
    flush with its `commit` line (inplace.rs, cuts.rs; the review of
    #111, D1). The log is written from one thread, so a barrier with no
    commit after it ends its instance. Where it does:
    - with no cut line (window B, or the cut line torn by the kill), the
      cut, if the rule took one, was not stored: the flush is one with
      no barrier, which the kernel hands to the next instance again.
      Where the next instance takes the same state up and its first cut
      counts on past one more (the shape no kill leaves,
      cut_line_missing), the barrier is kept for that error;
    - with cut line K (window C), the cut was stored or not: the next
      cut line of the same state says which, in whichever later instance
      it comes (the next may be killed before it cuts, the review of
      #115, R1). Where it is K again, the first line was no cut and both
      lines are taken out, at whatever group: a device created at the
      next start drops the mark and cuts K at a later group (R1's Cc),
      and the reference judges where the cut falls. Anything else keeps
      them, and a cut K+1 there means the state had stored K.
    The next cut line of the same state is the first in a later instance;
    there is none where a start that acknowledged a failure comes first,
    as it begins a state of its own.
    A write is answered with its `commit` line, after the store applied
    it (`applied`) and stored the mark an uberblock write sets
    (inplace.rs; the review of #115, F2). An instance that ends with an
    `applied` line was killed before the answer, and the mark may not
    have been stored: where the next instance takes the state up and
    applies the same write again (the same range and hashes: the kernel
    handed it over), and no flush it received before that cuts, the
    first `applied` line is taken out (window M), and the write is
    placed where it was applied again (reapplied). Where such a flush
    cuts, before or after the second `applied` line, it had the mark
    from the state, and the line is kept.
    A note: {'line', 'pid', 'shape', 'number', 'group'}."""
    out, notes, last = [], [], 0
    for k, inst in enumerate(instances):
        lines = inst['lines']
        if k and inst['pid'] in acked:
            last = 0
        if k + 1 < len(instances):
            tail = lines[-2:]
            again = None
            for later in instances[k + 1:]:
                if later['pid'] in acked:
                    break
                again = next((ln for ln in later['lines'] if ln['event'] == 'cut'), None)
                if again is not None:
                    break
            counted = [ln['sectors'] for ln in lines if ln['event'] == 'cut']
            n = int(counted[-1]) if counted and counted[-1].isdigit() else last
            skips = again is not None and again['sectors'] == str(n + 2)
            if lines and lines[-1]['event'] == 'barrier' and not skips:
                notes.append({'line': lines[-1]['line'], 'pid': inst['pid'], 'shape': 'B', 'number': None,
                              'group': None})
                lines = lines[:-1]
            elif lines and lines[-1]['event'] == 'applied' and k + 1 < len(instances) \
                    and instances[k + 1]['pid'] not in acked and reapplied(dict(inst, lines=lines), instances[k + 1]):
                notes.append({'line': lines[-1]['line'], 'pid': inst['pid'], 'shape': 'M', 'number': None,
                              'group': None})
                lines = lines[:-1]
            elif len(tail) == 2 and [ln['event'] for ln in tail] == ['barrier', 'cut'] \
                    and tail[0]['tag'] == tail[1]['tag']:
                if again is not None and again['sectors'] == tail[1]['sectors']:
                    notes.append({'line': tail[1]['line'], 'pid': inst['pid'], 'shape': 'C',
                                  'number': tail[1]['sectors'], 'group': tail[1]['result']})
                    lines = lines[:-2]
        counted = [ln['sectors'] for ln in lines if ln['event'] == 'cut']
        last = int(counted[-1]) if counted and counted[-1].isdigit() else last
        out.append(dict(inst, lines=lines))
    return out, notes


def unstored_detail(notes):
    """unstored_at_kill's notes -> the line obs_unstored_at_kill's detail."""
    # Most flushes carry no cut, and every flush writes its barrier and
    # then syncs: a kill on a barrier line says a cut was not stored only
    # where the flush carried one (the review of #115, F1).
    flushes = [n for n in notes if n['shape'] != 'M']
    writes = len(notes) - len(flushes)
    return (f'{len(flushes)} flushes killed after their barrier and before their answer; a cut such a flush carried '
            'was not stored'
            + (f'; {writes} writes killed after their applied line and before their answer, applied again by the '
               'next instance before it cut: a mark such a write set was not stored' if writes else '')
            + ''.join(f"; line {n['line']}: pid {n['pid']}'s barrier with no cut line, taken as a flush with no "
                      'barrier (whether or not it carried a cut)'
                      if n['shape'] == 'B' else
                      f"; line {n['line']}: pid {n['pid']}'s applied line with no answer, taken as a write not "
                      'applied (the next instance applied it again)'
                      if n['shape'] == 'M' else
                      f"; line {n['line']}: pid {n['pid']}'s cut {n['number']} (group {n['group']}), cut again "
                      'by the next instance: no cut' for n in notes))


def cut_line_missing(dcuts, ref, records):
    """A cut the next instance counts past and the log has no line for ->
    a message, or None. The daemon writes a cut's line before it stores
    the cut in its state (unstored_at_kill), so no kill leaves this: a
    line of the log was lost, or the daemon counts a cut before it says
    so. Either is the rig's error until it is known which."""
    bounds = [r['i'] for r in records if 'instance' in r['flags']]
    for b in bounds:
        before = sum(1 for d in dcuts if d['prefix'] < b)
        rbefore = [c for c in ref if c['prefix'] < b]
        after = [d for d in dcuts if d['prefix'] > b]
        last = max((r['i'] for r in records[:b] if 'flush' in r['flags']), default=None)
        if len(rbefore) == before + 1 and rbefore[-1]['prefix'] == last and after \
                and after[0].get('local', after[0]['number']) == str(before + 2):
            return (f'the next instance counts on from {before + 2}, and the log has no line of cut {before + 1} '
                    f'(group {rbefore[-1]["txg"]}): the daemon writes a cut\'s line before it stores the cut, so '
                    'no kill leaves this; a line was lost, or the daemon counted the cut before it said so')
    return None


def first_commit(log):
    """holder.log -> the time of the first release the daemon answered `ok`
    (a commit, as an upload's will be), or None."""
    asked = set()
    for ln in log.splitlines():
        f = ln.split('\t')
        if len(f) != 2 or not f[0].isdigit():
            continue
        w = f[1].split()
        if len(w) >= 3 and w[0] == '>' and w[2] == 'release':
            asked.add(w[1])
        elif len(w) >= 3 and w[0] == '<' and w[1] in asked and w[2] == 'ok':
            return int(f[0])
    return None


def releases(log):
    """holder.log -> [(the time the daemon answered `ok`, the cut's number)]
    for each release answered."""
    asked, out = {}, []
    for ln in log.splitlines():
        f = ln.split('\t')
        if len(f) != 2 or not f[0].isdigit():
            continue
        w = f[1].split()
        if len(w) >= 4 and w[0] == '>' and w[2] == 'release' and w[3].isdigit():
            asked[w[1]] = int(w[3])
        elif len(w) >= 3 and w[0] == '<' and w[1] in asked and w[2] == 'ok':
            out.append((int(f[0]), asked.pop(w[1])))
    return out


def instance_lines(put, rdir, instances, dcuts, records, begun=None):
    """Each instance's start with --hold-cuts (stage 2b): its `cuts` line,
    and its status read once it was ready (DIR/status-start-K.txt) ->
    1 when a line is a verdict against the daemon, else 0.

    The first start of a new volume begins a cut state and an intent
    map; each later one takes both up, whether it took the device over
    or created it, and its count goes on from the cuts counted before it
    (the status was read after the start, so it may have counted more):
    the cuts of its state, since the start that began it.
    A later start that acknowledges a failure (begun, stage 2c) trusts
    none of the cut state and begins one, and takes the intent map up
    (`skyzfsd help`: after a failure that is acknowledged the next cut
    holds only the regions the map names); its count is not judged here
    (numbers_verdict). An intent map with no committed cut is begun anew
    at any start (the daemon: "the intent map has no committed cut"), so a
    later start wants it taken up only where the holder's log shows a
    release answered before that start (the request log's clock), and
    takes either otherwise. A start that takes the state up holds the
    cuts the state held: at least one where the newest cut of the state
    counted before it was not released before it (the review of #115,
    S2; the daemon keeps its newest cut until it is released). Numbers
    and releases are the state's own: since the start that began it. A
    newest cut the daemon abandoned at its bound would rightly leave none
    held; no recording that starts the daemon again reaches the bound."""
    rc = 0
    bounds = [r for r in records if 'instance' in r['flags']]
    begun = begun or [True] + [False] * (len(instances) - 1)
    hlog = read_file(rdir, 'holder.log')
    committed = first_commit(hlog)
    released = releases(hlog)
    for k, inst in enumerate(instances):
        said = [ln['result'] for ln in inst['lines'] if ln['event'] == 'cuts']
        st = read_status(read_file(rdir, f'status-start-{k + 1}.txt'))
        want = 'begun' if begun[k] else 'taken_up'
        want_intent = 'begun' if k == 0 else 'taken_up'
        if k and (committed is None or committed > inst['ns']):
            want_intent = ('begun', 'taken_up')
        # The cuts of this state before it: since the start that began the
        # state (the first, or the last that acknowledged a failure).
        j = max(i for i in range(k + 1) if begun[i])
        lo = bounds[j - 1]['i'] if j else -1
        before = 0 if begun[k] else sum(1 for d in dcuts if lo < d['prefix'] < bounds[k - 1]['i'])
        bad = []
        if said != [want]:
            bad.append(f"its cuts line says {said or 'nothing'}")
        for key, w in (('cut_state', want), ('intent_state', want_intent)):
            if st.get(key) not in (w if isinstance(w, tuple) else (w,)):
                bad.append(f'{key} {st.get(key)!r}')
        n = st.get('cuts', '')
        if not n.isdigit() or int(n) < before:
            bad.append(f'cuts {n!r} at the start, with {before} counted before it')
        if k and begun[k] and st.get('cuts_held') != '0':
            # It trusts none of the cut state: nothing of the old one is
            # held (the review of #113, G3).
            bad.append(f"cuts_held {st.get('cuts_held')!r} at a start that acknowledged a failure, wanted '0'")
        if k and not begun[k]:
            # The state's own numbers, which the holder's are (epochs keeps
            # them as `local`), and the releases answered since the state
            # began (Codex and D2 of the review of #116).
            counted = [int(d.get('local', d['number'])) for d in dcuts if lo < d['prefix'] < bounds[k - 1]['i']
                       and d.get('local', d['number']).isdigit()]
            since = instances[j]['ns'] if j else -1
            gone = [n for t, n in released if since < t < inst['ns']]
            held = st.get('cuts_held', '')
            if counted and max(counted) > max(gone, default=0) and not (held.isdigit() and int(held) >= 1):
                bad.append(f'cuts_held {held!r} at a start that took the state up, wanted at least 1: cut '
                           f'{max(counted)} was counted before it and not released')
        how = 'the first' if k == 0 else ('took the device over' if bounds[k - 1]['taken_over'] else 'created the device')
        if k and begun[k]:
            how += ', acknowledging a failure'
        if isinstance(want_intent, tuple):
            how += ', no cut committed before it'
        want = (f"cut_state {want}, intent_state "
                f"{' or '.join(want_intent) if isinstance(want_intent, tuple) else want_intent}")
        put(f'instance_{k + 1}', 'FAIL' if bad else 'PASS',
            (f"pid {inst['pid']} ({how}): " + '; '.join(bad) + f'; wanted {want}') if bad else
            f"pid {inst['pid']} ({how}): {want} by its cuts line and the status's cut_state and intent_state; "
            f"cuts {n} at the start, {before} counted before it, cuts_held {st.get('cuts_held')!r}; the intent map named "
            f"{st.get('intent_named_at_start')!r} of {st.get('intent_regions')!r} regions at the start")
        rc = 1 if bad else rc
    return rc


def failure_line(put, rdir):
    """The recording `failed` (stage 2c): DIR/failure.tsv, the rig's `key
    value` lines -> 1 when the line is a verdict against the daemon. The
    volume failed (SIGUSR1, the test build) with cuts held and no client
    holding the device: serve ends with 3; a start without
    --acknowledge-failure is refused with 4, and one with it serves (the
    check's other lines judge it). A held cut is lost with the instance
    that saw the failure (design §5.2, as of zfs-topology d26841d)."""
    got = {}
    for ln in read_file(rdir, 'failure.tsv').splitlines():
        f = ln.split('\t')
        if len(f) != 2:
            raise RigError(f'failure.tsv: not a key and a value: {ln!r}')
        got[f[0]] = f[1]
    for key in ('cuts_held', 'failed_rc', 'refused_rc'):
        if key not in got:
            raise RigError(f'failure.tsv has no {key}: the recording did not get as far')
    if not got['cuts_held'].isdigit() or int(got['cuts_held']) < 1:
        raise RigError(f"failure.tsv: {got['cuts_held']!r} cuts held before the failure; the recording holds one "
                       'across it')
    bad = []
    if got['failed_rc'] != '3':
        bad.append(f"serve ended with {got['failed_rc']} after SIGUSR1, wanted 3")
    if got['refused_rc'] != '4':
        bad.append(f"a start without --acknowledge-failure ended with {got['refused_rc']}, wanted 4")
    put('failure', 'FAIL' if bad else 'PASS', '; '.join(bad) if bad else
        f"with {got['cuts_held']} cuts held the volume failed and serve ended with 3; a start without "
        '--acknowledge-failure was refused with 4, and one with it served')
    return 1 if bad else 0


def noflush_kstats(kstats, records, taken, cs):
    """The groups ZFS synced with no flush (zfs_nocacheflush, between the
    marks `noflush-begin` and `noflush-end`) that the reference cuts not
    -> kstats without them, and the groups left out. Such a group is
    committed with no flush after its uberblock, and only the last mark
    before a flush is cut (design §5.2): the reference, which `cuts`
    holds the daemon to, says which."""
    spans, lo = [], None
    for r in records:
        if 'mark' in r['flags'] and r.get('mark') == 'noflush-begin':
            lo = r['i']
        elif 'mark' in r['flags'] and r.get('mark') == 'noflush-end' and lo is not None:
            spans.append((lo, r['i']))
            lo = None
    if lo is not None:
        raise RigError('a noflush-begin mark with no noflush-end after it')
    cut = {c['txg'] for c in cs}
    left = {u['txg'] for u in taken if any(a < u['record'] < b for a, b in spans)} - cut
    return {k: [g for g in v if g not in left] for k, v in kstats.items()}, sorted(left)


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


def status_verdict(status, ref, times=None, latest=None, since=None):
    """status.txt's values and the reference's cuts -> (ok, detail).

    The values are held to the last cut. Where they differ, times (of the
    daemon's cut lines) and latest (status_window) say whether the status
    was written before the last cut line. since: the record where the
    last state began, if one began after the first (epochs): the daemon
    counts from 1 again in it (numbers_verdict), so the count is the
    reference's cuts after it."""
    want = {'cuts': str(len(ref) if since is None else sum(1 for c in ref if c['prefix'] > since))}
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


def held_lines(put, rdir, instances, records, dcuts, status, dev, name='', begun=None):
    """The second stage's lines, where the holder ran -> (1 when a line is a
    verdict against the daemon, else 0; whether a line is the rig's ERROR).
    begun: for each instance, whether it begins a state (the first, and
    one that acknowledged a failure)."""
    begun = begun or [True] + [False] * (len(instances) - 1)
    rc, errored = 0, False
    log = read_file(rdir, 'holder.log')
    err = [ln for ln in log.splitlines() if '\tERROR ' in ln]
    if err:
        raise RigError(f"the holder: {err[0].split(chr(9), 1)[1]}")
    # The daemon refused to read a block of a cut it held (cut_hold.py's
    # VERDICT): its own failure, and the holder ended there, so nothing
    # after it was recorded.
    bad = [ln.split('\tVERDICT ', 1)[1] for ln in log.splitlines() if '\tVERDICT ' in ln]
    put('held_reads', 'FAIL' if bad else 'PASS', '; '.join(bad) if bad else
        'the daemon read every block of each cut the holder asked it to write')
    if bad:
        return 1, False
    # The content of held cuts is held to the applied lines' hashes: a
    # log without them (a daemon from before M2.2b-2) cannot be judged.
    bare = [r for r in records if cut_ref.is_write(r) and r.get('hashes') is None]
    if bare:
        raise RigError(f'{len(bare)} writes applied with no hashes in the request log (a daemon from before M2.2b-2): '
                       'the content of held cuts cannot be judged')
    # The state: a new volume's first start begins one, by the log's
    # `cuts` line and by the status; each later start takes it up.
    said = [ln['result'] for inst in instances for ln in inst['lines'] if ln['event'] == 'cuts']
    want = ['begun' if b else 'taken_up' for b in begun]
    st = status.get('cut_state')
    ok = said == want and st == want[-1]
    put('cut_state', 'PASS' if ok else 'FAIL', f"the request log's cuts lines say {said or 'nothing'}, the last "
        f"status {st!r}; a new volume's first start with --hold-cuts begins a state, and each start after it "
        f"takes it up unless it acknowledged a failure, which begins one ({len(instances)} instances, wanted "
        f"{want})")
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
    starts = [r for r in records if 'instance' in r['flags'] and r.get('begun')]
    if starts:
        written, blocks, stale = held_by_epoch(written, read_file(rdir, 'held-blocks.tsv'), dcuts, starts)
        if stale:
            # The content after such a write cannot be held to the
            # reference: the holder's image has a cut of the old state.
            put('held_content', 'FAIL', 'the holder wrote ' + ', '.join(
                f'cut {n} in state {e}' for n, e in stale) + ', which has no cut line in that state and has '
                'one in the state before: a cut of the state before the start that acknowledged the failure was '
                'still held after it, though that start trusts none of the cut state')
            return 1, errored
    else:
        blocks = read_hashes(read_file(rdir, 'held-blocks.tsv'), 'held-blocks.tsv')
    final_image = read_hashes(read_file(rdir, 'held-image.tsv'), 'held-image.tsv', keyed=False)
    ok, detail, controls, wrong = held_content(records, dcuts, written, blocks, final_image)
    if ok is None:
        put('held_content', 'ERROR', detail)
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
    if name == 'takeover':
        across = [x for x in held_across_start(records, dcuts, written, blocks) if x[3]]
        put('held_across_start', 'INFO' if across else 'ERROR', '; '.join(
            f'cut {n}, counted by instance {c} and written by instance {w}: {b} blocks from copies instance {c} '
            'made' for n, c, w, b in across) if across else
            'no cut was written by a later instance than the one that counted it with a block from an earlier '
            "instance's copy: the recording is there to show copies surviving a start")
        errored = errored or not across
    if any(begun[1:]):
        got = across_failure(dcuts, written, blocks)
        ok = got is not None and got[2] > 0
        put('carried_across_failure', 'INFO' if ok else 'ERROR',
            f'cut {got[0]}, the first written of the new state: {got[2]} of its {got[1]} blocks differ from the '
            'image the holders of the state before left' if got else
            'no cut of the new state was written')
        errored = errored or not ok
    for c in CONTENT_CONTROLS:
        verdict, d = controls[c]
        good = verdict == CONTENT_EXPECTED[c]
        put(f'ctl_{c}', verdict.upper(), f"{d} (expected: {CONTENT_EXPECTED[c]}{'' if good else '; DIFFERS'})")
        rc = rc if good else 1
    return rc, errored


def read_uploaded(text):
    """uploaded.tsv (cut_watch.py) -> [{'number', 'group', 'state', 'head',
    'blocks', 'asked'}], asked the monotonic time node B's read began."""
    out = []
    for k, ln in enumerate(text.splitlines()):
        f = ln.split('\t')
        if len(f) != 7 or not all(x.isdigit() for x in (f[0], f[1], f[4], f[5], f[6])) \
                or not re.fullmatch(r'[0-9a-f]{32}', f[2]) or not re.fullmatch(r'[0-9a-f]{64}', f[3]):
            raise RigError(f'uploaded.tsv line {k + 1}: not `number group state head blocks ns ns`: {ln!r}')
        out.append({'number': int(f[0]), 'group': int(f[1]), 'state': f[2], 'head': f[3], 'blocks': int(f[4]),
                    'asked': int(f[5])})
    return out


# The status's counters of the uploads (status version 15) that a
# recording with no other writer, one instance and a bound it does not
# reach leaves at 0 (M2.4c-2's table): a cut lost, a commit given up, a
# mismatch, a committed cut kept or found committed already.
UPLOAD_ZERO = ('upload_mismatches', 'upload_abandoned_at_mismatch', 'upload_cuts_lost', 'upload_given_up',
               'upload_committed_kept', 'upload_found_committed')
# The courses (cut-record.sh): where another writer stops the uploads,
# the file whose row names the last cut the daemon uploaded before it.
UPLOAD_COURSES = ('umoved', 'urow', 'ukill', 'ulease', 'ulost', 'unostore', 'ukillput', 'ufailed')
STOPPED_AT = {'umoved': 'moved-row.txt', 'ulost': 'lost-row.txt'}
OTHER = 'other-node'


def read_others(text):
    """others.tsv -> (heads, rows) another writer put: head's commits in
    hex, and rows as (state, number, group)."""
    heads, rows = set(), set()
    for k, ln in enumerate(text.splitlines()):
        f = ln.split('\t')
        if len(f) == 2 and f[0] == 'head' and re.fullmatch(r'[0-9a-f]{64}', f[1]):
            heads.add(f[1])
        elif len(f) == 4 and f[0] == 'row' and f[2].isdigit() and f[3].isdigit():
            rows.add((f[1], int(f[2]), int(f[3])))
        else:
            raise RigError(f'others.tsv line {k + 1}: {ln!r}')
    return heads, rows


def read_row(text, what):
    """node B's `row` output -> {'head': hex or None, 'cut': (state,
    number, group) or None, 'lease': (holder, epoch) or None}."""
    got = {}
    for ln in text.splitlines():
        f = ln.split('\t')
        if f[0] == 'head':
            got['head'] = f[1] if len(f) == 3 else None
        elif f[0] == 'row':
            got['cut'] = (f[1], int(f[2]), int(f[3])) if len(f) == 4 and f[2].isdigit() and f[3].isdigit() \
                else None
        elif f[0] == 'lease':
            got['lease'] = (f[1], int(f[2])) if len(f) == 4 and f[2].isdigit() else None
    if set(got) != {'head', 'cut', 'lease'}:
        raise RigError(f'{what}: not what node B row prints: {text!r}')
    return got


def daemons_fetches(got, text, others, offsets=(0,)):
    """The cuts node B fetched that the daemon uploaded -> (written,
    blocks, skipped, twice, states): written as held_content takes it,
    blocks {number: what changed since the daemon's cut fetched before
    it}, skipped the fetches of what another writer put (its head, or its
    row at head), twice the numbers the daemon's fetches carried more than
    once (a cut at head under two commits), states the cut states their
    rows named, in the order first fetched.

    A state begun later counts from 1 again (epochs): the k-th state the
    rows name is the k-th epoch, and its numbers become places in the
    whole log by offsets[k], the cut lines before it. A fetch of a state
    past the last offset is left out (the caller fails it by states).

    text: uploaded-blocks.tsv, `fetch number block hash`; each fetch's
    blocks are what changed since the fetch before, whoever's it was, so
    the image is followed through every fetch."""
    heads, rows = others
    per = {}
    for k, ln in enumerate(text.splitlines()):
        f = ln.split('\t')
        if len(f) != 4 or not all(x.isdigit() for x in f[:3]) or not re.fullmatch(r'[0-9a-f]{16}', f[3]):
            raise RigError(f'uploaded-blocks.tsv line {k + 1}: {ln!r}')
        if int(f[0]) >= len(got) or int(f[1]) != got[int(f[0])]['number']:
            raise RigError(f'uploaded-blocks.tsv line {k + 1}: fetch {f[0]} is not cut {f[1]} of uploaded.tsv')
        per.setdefault(int(f[0]), {})[int(f[2])] = f[3]
    image, at_last = {}, {}
    written, blocks, skipped, twice, states = [], {}, 0, set(), []
    for k, g in enumerate(got):
        image.update(per.get(k, {}))
        if g['head'] in heads or (g['state'], g['number'], g['group']) in rows:
            skipped += 1
            continue
        if g['state'] not in states:
            states.append(g['state'])
        e = states.index(g['state'])
        if e >= len(offsets):
            continue
        g = dict(g, number=g['number'] + offsets[e])
        if g['number'] in blocks:
            twice.add(g['number'])
            continue
        blocks[g['number']] = {b: h for b, h in image.items() if at_last.get(b, ZERO) != h}
        at_last = dict(image)
        written.append(g)
    return written, blocks, skipped, twice, states


def at_least(n):
    return lambda v: v.isdigit() and int(v) >= n


def course_wants(name, rdir, status):
    """What the status after the drain must say in recording name, and
    the course's own findings -> ({key: str or predicate}, [problems],
    [notes])."""
    # The holder the row names (M2.4c-2's table), as node B last read it.
    rows = [ln.split('\t') for ln in read_file(rdir, 'rows.tsv').splitlines()]
    if not rows or len(rows[-1]) != 4:
        raise RigError(f"rows.tsv: no line `ns head row lease` at its end: {rows[-1:] or 'nothing'}")
    lease = rows[-1][3].split()
    holder = lease[0]
    want = {'upload_state': 'idle', 'cuts_held': '0', 'lease': 'held', 'lease_holder': holder,
            'uploaded_whole': '1', 'lease_epoch': '1'}
    want.update({k: '0' for k in UPLOAD_ZERO})
    bad, notes = [], []
    # Node B's own read of the lease: a holder, and the status's epoch.
    if holder == 'none' or len(lease) < 2:
        bad.append(f"node B read no lease in the bucket at its last look ({rows[-1][3]!r})")
    elif status.get('lease_epoch') != lease[1]:
        bad.append(f"the status says lease_epoch {status.get('lease_epoch')!r}, the bucket's lease {lease[1]}")

    def row(fn):
        return read_row(read_file(rdir, fn), fn)

    def stat(fn):
        return read_status(read_file(rdir, fn))

    if name == 'umoved':
        before, end = row('moved-row.txt'), row('end-row.txt')
        heads, _ = read_others(read_file(rdir, 'others.tsv'))
        n = stat('moved-status.txt').get('uploaded', '?')
        want.update(upload_mismatches=at_least(1), upload_abandoned_at_mismatch=at_least(1),
                    upload_checked='head_moved', uploaded=n)
        if end['head'] not in heads:
            bad.append(f"head is {end['head']} at the end, not the other writer's {sorted(heads)}: the uploader "
                       "moved it")
        if end['cut'] != before['cut']:
            bad.append(f"the row is {end['cut']} at the end, {before['cut']} before the other writer")
    elif name == 'urow':
        first = row('row1-row.txt')
        _, rows = read_others(read_file(rdir, 'others.tsv'))
        end = row('end-row.txt')
        want.update(upload_mismatches='2', upload_abandoned_at_mismatch=at_least(0), uploaded_whole='3',
                    upload_checked='matched')
        if end['cut'] is None or end['cut'] in rows or end['cut'][0] != first['cut'][0]:
            bad.append(f"the row at the end is {end['cut']}, not a cut of the daemon's state {first['cut'][0]}")
        for fn in ('row1-after.txt', 'row2-after.txt'):
            notes.append(f"{fn.split('-')[0]}: upload_checked {stat(fn).get('upload_checked')!r} once the "
                         "mismatch was counted")
    elif name in ('ukill', 'ukillput'):
        rc = dict(ln.split('\t', 1) for ln in read_file(rdir, 'kill.tsv').splitlines()).get('killed_rc')
        if rc != '137':
            bad.append(f'the first instance ended with {rc}, not 137 (SIGKILL)')
        hook = [ln for ln in read_file(rdir, 'serve.log').splitlines() if 'test hook --kill-at-upload' in ln]
        point = 'the swap of commit 3 ' if name == 'ukill' else 'segment PUT 5 landed'
        if not hook or point not in hook[0]:
            bad.append(f"the first instance's log says {hook[:1] or 'nothing'} of the kill, not {point.strip()}")
        if name == 'ukill':
            # Rule (1): the row is the cut it holds; let go, nothing uploaded.
            want.update(upload_found_committed='1', uploaded_whole='0', uploaded=at_least(1))
        else:
            # The cut being uploaded goes up again: whole where it was the
            # first, which the kill's point does not say.
            want.update(uploaded_whole=at_least(0), uploaded=at_least(1))
            notes.append(f"uploaded_whole {status.get('uploaded_whole')!r} after the kill")
    elif name == 'unostore':
        down = stat('nostore-status.txt')
        for k, v in (('lease', ('none',)), ('upload_state', ('no_lease', 'backoff')), ('uploaded', ('0',))):
            if down.get(k) not in v:
                bad.append(f"with object storage down, {k} {down.get(k)!r}, not one of {v}")
        notes.append(f"with object storage down: upload {down.get('upload')!r}, upload_why {down.get('upload_why')!r}, "
                     f"upload_wait_ms {down.get('upload_wait_ms')!r}; upload_errors {status.get('upload_errors')!r} "
                     "at the end")
    elif name == 'ufailed':
        rc = dict(ln.split('\t', 1) for ln in read_file(rdir, 'failure.tsv').splitlines()).get('failed_rc')
        if rc != '3':
            bad.append(f'the failed instance ended with {rc}, not 3')
        st2 = stat('status-start-2.txt')
        if st2.get('cut_state') != 'begun':
            bad.append(f"the start that acknowledged the failure says cut_state {st2.get('cut_state')!r}, not begun")
        # The status counts the second instance's. Its new state's first
        # cut holds the whole volume (rule 3) only where the start began
        # the intent map anew; one that took the map up counts only the
        # map's regions as changed (M2.3), and that cut goes up as a delta
        # onto the base the map names (rule 2). The start's log says which.
        log2 = read_file(rdir, 'serve-2.log')
        if 'intent map: taken up' in log2:
            want.update(uploaded_whole='0', uploaded=at_least(1), upload_rule='delta', upload_checked='matched')
            notes.append('the start that acknowledged the failure took the intent map up: the new state\'s first '
                         'cut went up as a delta onto the map\'s base')
        elif 'intent map: begun anew' not in log2:
            bad.append('serve-2.log says neither that the intent map was taken up nor that it was begun anew')
    elif name == 'ulease':
        rc = dict(ln.split('\t', 1) for ln in read_file(rdir, 'lease.tsv').splitlines()).get('refused_rc')
        if rc != '10':
            bad.append(f'the serve while another held the lease ended with {rc}, not 10')
        wrote = [ln for ln in read_file(rdir, 'refused-wrote.txt').splitlines() if ln.strip()]
        notes.append(f"the refused start wrote {len(wrote)} paths ({', '.join(os.path.basename(x) for x in wrote[:4])})")
        without = stat('status-start-1.txt')
        for k, v in (('upload_state', ('off', 'no_lease')), ('lease', ('none', 'lost'))):
            if without.get(k) not in v:
                bad.append(f"served with --serve-without-upload, {k} {without.get(k)!r}, not one of {v}")
        n = stat('without-status.txt').get('uploaded')
        if n != '0':
            bad.append(f'served with --serve-without-upload, uploaded {n!r} cuts')
        want.update(lease_epoch=at_least(1))
        notes.append(f"lease_epoch {status.get('lease_epoch')!r} after the other holder let go")
        # Node B's side: before the release, head stayed where it was at
        # node B's first look (none, or the create's own commit, which
        # names no cut) and the row named no cut.
        until = int(read_file(rdir, 'release-ns.txt').split()[0])
        before = [r for r in rows if len(r) == 4 and int(r[0]) < until]
        if not before:
            bad.append('node B read nothing before the other holder let the lease go')
        moved = [r for r in before if r[1] != before[0][1] or r[2] != 'none']
        if moved:
            bad.append(f"node B read head {moved[0][1]} and the row {moved[0][2]} before the other holder let the "
                       f"lease go, head {before[0][1]} at its first look: the daemon uploaded without the lease")
    elif name == 'ulost':
        before, end = row('lost-row.txt'), row('end-row.txt')
        took = read_file(rdir, 'other-lease.txt').split()
        n = stat('lost-status.txt').get('uploaded', '?')
        want.update(upload_state='lease_lost', lease='lost', lease_holder=OTHER, cuts_held=at_least(0),
                    lease_epoch=took[3] if len(took) > 3 and took[:2] == ['lease', 'acquired'] else '?', uploaded=n)
        if (end['head'], end['cut']) != (before['head'], before['cut']):
            bad.append(f"head and the row are {end['head']} {end['cut']} at the end, {before['head']} "
                       f"{before['cut']} when the lease was lost")
        if end['lease'] is None or end['lease'][0] != OTHER:
            bad.append(f"the lease at the end is {end['lease']}, not {OTHER}'s")
    return want, bad, notes


def raw_block(k):
    """Block k of those ufailed writes to the device itself before the
    failure (cut-record.sh raw_write)."""
    return (f'ufailed raw block {k}\n'.encode() * 256)[:BLOCK]


def raw_held(rdir):
    """ufailed's blocks written outside the pool before the failure, in
    the last image node B fetched -> (how many of them it holds, how
    many were written), or None where the course wrote none (raw.tsv)."""
    if not os.path.exists(os.path.join(rdir, 'raw.tsv')):
        return None
    f = read_file(rdir, 'raw.tsv').split()
    if len(f) != 3 or f[0] != 'raw' or not (f[1].isdigit() and f[2].isdigit()):
        raise RigError(f'raw.tsv: {f}')
    at, n = int(f[1]), int(f[2])
    image = {}
    for ln in read_file(rdir, 'uploaded-blocks.tsv').splitlines():
        x = ln.split('\t')
        if len(x) == 4:
            image[int(x[2])] = x[3]
    held = sum(1 for k in range(n) if image.get(at + k) == hashlib.sha256(raw_block(k)).digest()[:8].hex())
    return held, n


def at_rest(st):
    """A status look the balance can start from: the uploader idle with
    nothing held, or nothing counted yet."""
    return (st.get('upload_state') == 'idle' and st.get('cuts_held') == '0') or \
        all(st.get(k) == '0' for k in BALANCE)


BALANCE = ('cuts', 'uploaded', 'upload_found_committed', 'upload_committed_gone', 'upload_committed_kept',
           'cuts_replaced', 'cuts_abandoned', 'cuts_held')


def upload_balance(start, end):
    """The cuts the store counted against where they went, over one
    daemon's life (status.rs, `cuts_replaced`): between the look at its
    start and the one after the drain, the change in `cuts` equals the
    change in the cuts committed and let go (`uploaded` +
    `upload_found_committed` - `upload_committed_gone` -
    `upload_committed_kept`), plus those replaced, abandoned and held.
    -> (True or False, detail)."""
    missing = sorted({k for k in BALANCE for st in (start, end) if not (st.get(k) or '').isdigit()})
    if missing:
        return False, f"the status has no {', '.join(missing)} (a daemon before cuts_replaced?)"
    d = {k: int(end[k]) - int(start[k]) for k in BALANCE}
    committed = d['uploaded'] + d['upload_found_committed'] - d['upload_committed_gone'] - d['upload_committed_kept']
    rest = committed + d['cuts_replaced'] + d['cuts_abandoned'] + d['cuts_held']
    detail = (f"cuts {d['cuts']} = committed and let go {committed} + replaced {d['cuts_replaced']} + abandoned "
              f"{d['cuts_abandoned']} + held {d['cuts_held']:+d}"
              + ('' if rest == d['cuts'] else f": the right side is {rest}")
              + f"; at the start upload_state {start.get('upload_state')!r}, after the drain "
              f"{end.get('upload_state')!r}")
    if rest != d['cuts']:
        detail = detail.replace(' = ', ' != ', 1)
    return rest == d['cuts'], detail


def upload_lines(put, rdir, records, dcuts, status, name='life'):
    """The daemon's uploads (M2.T3 on M2.4c), where the watcher ran beside
    the recording -> (1 when a line is a verdict against the daemon, else
    0; whether a line is the rig's ERROR).

    Each cut node B fetched at head (cut_watch.py) that the daemon
    uploaded, and not another writer (DIR/others.tsv), is held to the
    reference's content at its instant, as a holder's written cuts are
    (held_content: the row's number and group against the cut line's,
    every block, the last cut fetched the last counted, or in a course
    that stops the uploads the last before it). The status after the
    drain says what the course wants (course_wants): in a recording with
    no other writer, that the daemon uploads, holds the lease it took at
    epoch 1 under the holder node B's row names, holds nothing and is idle, uploaded the volume whole once,
    and counted none of the things no other writer and no bound make
    (UPLOAD_ZERO). `upload_course` judges a course's own files."""
    rc, errored = 0, False
    log = read_file(rdir, 'watch.log')
    err = [ln for ln in log.splitlines() if '\tERROR ' in ln]
    if err:
        raise RigError(f"the watcher: {err[0].split(chr(9), 1)[1]}")
    bad = [ln.split('\tVERDICT ', 1)[1] for ln in log.splitlines() if '\tVERDICT ' in ln]
    put('upload_reads', 'FAIL' if bad else 'PASS', '; '.join(bad) if bad else
        "node B read the content of every cut it found at head")
    if bad:
        return 1, False
    bare = [r for r in records if cut_ref.is_write(r) and r.get('hashes') is None]
    if bare:
        raise RigError(f'{len(bare)} writes applied with no hashes in the request log: the content of uploaded '
                       'cuts cannot be judged')
    got = read_uploaded(read_file(rdir, 'uploaded.tsv')) if os.path.exists(os.path.join(rdir, 'uploaded.tsv')) \
        else []
    others = read_others(read_file(rdir, 'others.tsv')) \
        if os.path.exists(os.path.join(rdir, 'others.tsv')) else (set(), set())
    # Where a later instance began a state (ufailed), its cuts' numbers
    # are their places in the whole log, as the cut lines' are (epochs).
    n_epochs = 1 + max((d.get('epoch', 0) for d in dcuts), default=0)
    offsets = [sum(1 for d in dcuts if d.get('epoch', 0) < e) for e in range(n_epochs)]
    mine, blocks, skipped, twice, states = daemons_fetches(
        got, read_file(rdir, 'uploaded-blocks.tsv') if got else '', others, offsets)
    if not mine:
        put('upload_content', 'FAIL', f"node B found no cut of the daemon's at head ({skipped} of another "
            f"writer's); the daemon counted {len(dcuts)}")
        return 1, False
    by_number = {int(d['number']): d for d in dcuts if d['number'].isdigit()}
    written = []
    for g in mine:
        d = by_number.get(g['number'])
        # The row says no more than the number and the group: the cut
        # line says how it opens, and held_content holds the group.
        written.append({'number': g['number'], 'group': g['group'], 'blocks': g['blocks'], 'asked': g['asked'],
                        'opens': ('itself' if d['itself'] else 'rewind') if d else 'itself'})
    last = None
    if name in STOPPED_AT:
        cut = read_row(read_file(rdir, STOPPED_AT[name]), STOPPED_AT[name])['cut']
        last = cut[1] if cut else None
    ok, detail, controls, _ = held_content(records, dcuts, written, blocks, last=last)
    if len(states) > n_epochs:
        ok, detail = False, (f"the rows name {len(states)} cut states ({', '.join(states)}), the request log "
                             f"{n_epochs}; {detail}")
    if twice:
        ok, detail = False, f"cuts {sorted(twice)} were at head under two commits; {detail}"
    detail = detail.replace(' cuts written and released;', ' cuts fetched at head;')
    ok_content = bool(ok)
    if ok is None:
        put('upload_content', 'ERROR', detail)
        errored = True
    else:
        put('upload_content', 'PASS' if ok else 'FAIL',
            f"node B fetched cuts {', '.join(str(g['number']) for g in mine)} at head"
            + (f' and {skipped} of another writer\'s' if skipped else '') + f": {detail}")
        rc = rc if ok else 1
    for c in CONTENT_CONTROLS:
        verdict, d = controls[c]
        good = verdict == CONTENT_EXPECTED[c]
        put(f'ctl_{c}', verdict.upper(), f"{d} (expected: {CONTENT_EXPECTED[c]}{'' if good else '; DIFFERS'})")
        rc = rc if good else 1
    drain = read_file(rdir, 'drain.txt').splitlines()
    secs = drain[0].split()[-1] if drain and drain[0].startswith('seconds ') else '?'
    want, cbad, notes = course_wants(name, rdir, status)
    wrong = [f"{k} {status.get(k)!r}, not {v if isinstance(v, str) else 'as the course wants'}"
             for k, v in want.items()
             if not (status.get(k) == v if isinstance(v, str) else v(status.get(k) or ''))]
    n = status.get('uploaded', '')
    if name not in UPLOAD_COURSES and (not n.isdigit() or int(n) < len(mine)):
        wrong.append(f'uploaded {n!r}, fewer than the {len(mine)} cuts node B fetched')
    if name not in UPLOAD_COURSES:
        wrong += cbad           # no course line: node B's read of the lease goes here
    put('upload_status', 'FAIL' if wrong else 'PASS',
        ('; '.join(wrong) if wrong else ', '.join(f'{k} {status.get(k)}' for k in want))
        + f"; uploaded {n}, upload_checked {status.get('upload_checked')!r}, upload_rule "
        f"{status.get('upload_rule')!r}, upload_errors {status.get('upload_errors')!r}, lease_holder "
        f"{status.get('lease_holder')!r}; the drain took {secs} s")
    rc = rc if not wrong else 1
    # The balance over the last instance, from a look at rest after its
    # start (DIR/balance-start.txt, which a course that starts the daemon
    # again takes) to the drain.
    k = max(int(m.group(1)) for m in (re.fullmatch(r'status-start-(\d+)\.txt', f) for f in os.listdir(rdir)) if m)
    fn = 'balance-start.txt' if os.path.exists(os.path.join(rdir, 'balance-start.txt')) else f'status-start-{k}.txt'
    if k > 1 and fn != 'balance-start.txt':
        raise RigError(f'serve {k} and no balance-start.txt: the balance has no look at rest to start from')
    start = read_status(read_file(rdir, fn))
    if not at_rest(start):
        put('upload_balance', 'ERROR', f"serve {k}: the look it starts from ({fn}) is not at rest: upload_state "
            f"{start.get('upload_state')!r}, cuts_held {start.get('cuts_held')!r}")
        errored = True
    else:
        ok, detail = upload_balance(start, status)
        put('upload_balance', 'PASS' if ok else 'FAIL', f'serve {k}, from {fn}: {detail}')
        rc = rc if ok else 1
    # ufailed: the blocks written to the device after the old state's
    # last cut and before the failure are in the new state's cuts.
    raw = raw_held(rdir)
    if raw is not None:
        held, n = raw
        if held == n:
            put('upload_raw', 'PASS', f"node B's last cut holds the {n} blocks written to the device after the old "
                "state's last cut and before the failure")
        elif ok_content:
            put('upload_raw', 'ERROR', f"node B's last cut holds {held} of the {n} blocks written before the "
                "failure, and so does the reference: the pool wrote over them, and the course shows nothing")
            errored = True
        else:
            put('upload_raw', 'FAIL', f"node B's last cut holds {held} of the {n} blocks written before the "
                "failure; the content line says where it differs from the reference")
            rc = 1
    if name in UPLOAD_COURSES:
        put('upload_course', 'FAIL' if cbad else 'PASS', '; '.join(cbad + notes) if cbad or notes else name)
        rc = rc if not cbad else 1
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
            torn = []
            instances = parse_log(f.read(), torn)
        if not instances:
            raise RigError('no instance of the daemon in the request log')
        kinds, acked = None, set()
        if os.path.exists(os.path.join(rdir, 'instances.tsv')):
            kinds = read_kinds(read_file(rdir, 'instances.tsv'))
            acked = read_acknowledged(read_file(rdir, 'instances.tsv'))
        put('obs_torn_at_kill', 'INFO', torn_detail(torn, kinds))
        marks_path = os.path.join(rdir, 'marks.tsv')
        with open(marks_path) as f:
            marks = read_marks(f.read())
        instances, unstored = unstored_at_kill(instances, acked)
        put('obs_unstored_at_kill', 'INFO', unstored_detail(unstored))
        records, dcuts = records_of(log_lines(instances, kinds, acked), marks)
        starts = epochs(dcuts, records)
        begun = [True] + [r['begun'] for r in records if 'instance' in r['flags']]
        read = read_of(records, dev)
        ubs = cut_ref.scan(records, read, dev)
        cs, taken = cut_ref.cuts(records, ubs)
        op = cut_ref.opens(records, read, dev, cs)
        ref = reference(cs, op)
        put('records', 'INFO', f"{len(records)} records from the request log ({sum(1 for r in records if 'data' in r)} "
            f"with their bytes), {sum(1 for u in taken if u['taken'] == 'marks')} uberblock writes that mark, of "
            f"{len(taken)}; {sum(1 for r in records if 'mark' in r['flags'])} marks")
        ok, detail = compare(dcuts, ref)
        if not ok:
            lost = cut_line_missing(dcuts, ref, records)
            if lost:
                raise RigError(lost)
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
        since = starts[-1]['i'] if starts else None
        ok, detail = status_verdict(status, ref, [c['ns'] for c in dcuts], latest, since)
        put('status', 'PASS' if ok else 'FAIL', detail)
        if not ok:
            rc = 1
        if os.path.exists(os.path.join(rdir, 'holder.log')):
            # A holder that ended with a verdict (the daemon refused to read
            # a cut it held) ended the recording's holding there, perhaps
            # before its first cut and before what a whole recording
            # leaves: held_reads alone is judged of the held cuts.
            if '\tVERDICT ' in read_file(rdir, 'holder.log'):
                v, e = held_lines(put, rdir, instances, records, dcuts, status, dev, name, begun)
                rc, errored = max(rc, v), errored or e
            else:
                # The instances' lines first: they name a start that took
                # up a state it should have begun, whatever the held cuts
                # show.
                rc = max(rc, instance_lines(put, rdir, instances, dcuts, records, begun))
                if name == 'failed':
                    rc = max(rc, failure_line(put, rdir))
                v, e = held_lines(put, rdir, instances, records, dcuts, status, dev, name, begun)
                rc, errored = max(rc, v), errored or e
        elif os.path.exists(os.path.join(rdir, 'watch.log')):
            if len(instances) > 1 and name not in TWO_STARTS:
                raise RigError(f'{len(instances)} instances of the daemon in a recording that uploads: only '
                               f"{', '.join(TWO_STARTS)} start it again")
            v, e = upload_lines(put, rdir, records, dcuts, status, name)
            rc, errored = max(rc, v), errored or e
        elif len(instances) > 1:
            raise RigError(f'{len(instances)} instances of the daemon and no holder: only a recording that holds '
                           'cuts starts the daemon again')
        # What ZFS says it synced, as the cut rig holds the reference to it.
        kstats = {}
        for fn in os.listdir(rdir):
            m = re.fullmatch(r'txgs-(\d+)\.txt', fn)
            if m:
                with open(os.path.join(rdir, fn)) as f:
                    kstats[int(m.group(1))] = cut_check.committed_txgs(f.read())
        kstats, left = noflush_kstats(kstats, records, taken, cs)
        if left:
            put('obs_noflush_groups', 'INFO', f'groups {left} were synced with no flush and are not cut, by the '
                'reference: left out of the sessions\' committed groups')
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
