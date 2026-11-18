#!/usr/bin/env python3
"""#115 at 813120a: is test_a_group_synced_with_no_flush a fair model of
a span with zfs_nocacheflush?

    n5_model.py CHECKOUT

The fixture `life` around its 11th cut (group 21), by the request log's
times:

    18170  cut 10 (group 18)
    20280  a flush, before group 21's uberblock writes (it cuts nothing)
    20300..20450  group 21's four uberblock writes
    20460  a flush: cut 11 (group 21)
    21420  a flush, before group 22's uberblock writes
    21440..21590  group 22's four uberblock writes
    21600  a flush: cut 12 (group 22)

The recording sets zfs_nocacheflush between its marks (cut-record.sh:
`noflush` writes the mark and then sets it, `flushing` clears it and
then writes the mark), so between the marks ZFS sends no flush and after
noflush-end it sends every one.

Each variant goes through the program as the unit tests' helper runs it
(test_cut_daemon_main.recording), with the kstats of the whole fixture
(ZFS's own account, which has every group):

  V0   the test's own log and marks: the flushes at 20460 and 21420
       taken out; noflush-begin at 18175, noflush-end at 20485. The
       flush at 20280 stays though it lies between the marks, and the
       one at 21420 is gone though it lies after noflush-end.
  V0n  V0 without the marks (the test's second half).
  V1   V0 with noflush-end after the last flush taken out (21435).
  V2   every flush between the marks gone and none after them: the
       flushes at 20280, 20460 and 21420 taken out, the marks at 18175
       and 21435. What zfs_nocacheflush gives, if flushing came back
       between ZFS's flush before group 22's uberblocks and those
       writes.
  V3a  the recording's course (b) with V0's marks: the flushes between
       18175 and 20485 gone (20280 and 20460), the one at 21420 there.
       The daemon's log has no cut at it (the test's "first form").
  V3b  V3a, and the daemon cuts group 21 at the flush of 21420, as the
       mark that waited tells it to.
  V4   two groups between the marks, as in the recording's course (c):
       groups 18 and 21 synced with no flush (four flushes gone), the
       marks from after cut 9 to 20485; the daemon cuts group 21 at the
       flush of 21420 and never group 18.

For each: the exit, and the lines sessions, cuts and obs_noflush_groups,
and whatever else is FAIL or ERROR."""
import os
import sys

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import cut_ref  # noqa: E402
import test_cut_daemon_check as t  # noqa: E402
import test_cut_daemon_main as m  # noqa: E402

records, read, dev = t.fixture('life')
cs, at = t.daemon_cuts_of(records, read, dev)
text, _ = t.request_log(records, read, dev, at)
L = text.splitlines()
CUTS = [i for i, ln in enumerate(L) if ln.split('\t')[2] == 'cut']


def ns(i):
    return int(L[i].split('\t')[0])


def flush_before(i):
    """The barrier line of the last flush before line i."""
    return max(j for j in range(i) if L[j].split('\t')[2:5] == ['barrier', '1', 'flush'])


def flush_of_cut(k):
    """The lines of the k-th cut's flush (0-based): recv, barrier, cut."""
    ci = CUTS[k]
    assert [L[i].split('\t')[2] for i in (ci - 2, ci - 1, ci)] == ['recv', 'barrier', 'cut']
    return [ci - 2, ci - 1, ci]


def before_uberblocks(k):
    """The lines of the flush before the k-th cut's group's uberblock
    writes: recv, barrier (it carries no cut)."""
    b = flush_before(CUTS[k] - 2)
    assert b > CUTS[k - 1] and L[b + 1].split('\t')[2] == 'recv' and L[b + 2].split('\t')[2] == 'label'
    return [b - 1, b]


def run(name, gone, marks, cut_at=None, said=''):
    """gone: line numbers taken out. marks: [(ns, name)] added. cut_at:
    (barrier's line, group): a cut line added after that barrier."""
    gone = set(gone)

    def tamper(text):
        out = []
        for i, ln in enumerate(text.splitlines()):
            if i in gone:
                continue
            out.append(ln)
            if cut_at and i == cut_at[0]:
                f = ln.split('\t')
                out.append('\t'.join([str(int(f[0]) + 5), f[1], 'cut', f[3], f[4], cut_at[2], '-', str(cut_at[1])]))
        return ''.join(ln + '\n' for ln in out)

    def span(text):
        rows = [tuple(ln.split('\t')) for ln in text.splitlines()] + [(str(n), mk) for n, mk in marks]
        return ''.join(f'{n}\t{mk}\n' for n, mk in sorted(rows, key=lambda r: int(r[0])))
    n_cuts = len(CUTS) - sum(1 for i in gone if i in CUTS) + (1 if cut_at else 0)
    last = cs[-1]
    op = cut_ref.opens(records, read, dev, cs)
    status = (f"cuts {n_cuts}\ncut_group {last['txg']}\n"
              f"cut_opens {'itself' if op[last['n']]['itself'] else 'rewind'}\nage_ms 0\n")
    rc, out = m.recording(tamper, status=status, tamper_marks=span if marks else None)
    print(f'{name}: exit {rc}   [{said}]')
    for key, (verdict, detail) in out.items():
        if key in ('sessions', 'cuts', 'obs_noflush_groups') or verdict in ('FAIL', 'ERROR'):
            print(f'    {key}\t{verdict}\t{detail[:260]}')


g21, g22 = L[CUTS[10]].split('\t')[7], L[CUTS[11]].split('\t')[7]
assert (g21, g22) == ('21', '22'), (g21, g22)
cut21 = flush_of_cut(10)
pre21 = before_uberblocks(10)
pre22 = before_uberblocks(11)
lo, hi = ns(CUTS[9]) + 5, ns(CUTS[10]) + 5
print(f'the flush before group 21\'s uberblocks at {ns(pre21[0])}, cut 11 at {ns(cut21[2])}, the flush before group '
      f'22\'s at {ns(pre22[0])}, cut 12 at {ns(CUTS[11])}; the test\'s marks at {lo} and {hi}')
run('V0', cut21 + pre22, [(lo, 'noflush-begin'), (hi, 'noflush-end')],
    said='the test: one flush kept between the marks, one gone after noflush-end')
run('V0n', cut21 + pre22, [], said='the test without the marks')
after = ns(pre22[1]) + 5
run('V1', cut21 + pre22, [(lo, 'noflush-begin'), (after, 'noflush-end')],
    said=f'noflush-end at {after}, after the last flush gone')
run('V2', pre21 + cut21 + pre22, [(lo, 'noflush-begin'), (after, 'noflush-end')],
    said='no flush between the marks, every flush after them')
run('V3a', pre21 + cut21, [(lo, 'noflush-begin'), (hi, 'noflush-end')],
    said='course (b): the flush after noflush-end is there, and the daemon cuts nothing at it')
run('V3b', pre21 + cut21, [(lo, 'noflush-begin'), (hi, 'noflush-end')], cut_at=(pre22[1], 21, 'itself'),
    said='course (b): the daemon cuts group 21 at the first flush after noflush-end')
g18 = L[CUTS[9]].split('\t')[7]
lo2 = ns(CUTS[8]) + 5
run('V4', before_uberblocks(9) + flush_of_cut(9) + pre21 + cut21, [(lo2, 'noflush-begin'), (hi, 'noflush-end')],
    cut_at=(pre22[1], 21, 'itself'),
    said=f'two groups between the marks ({g18} and 21): {g18} is never cut, 21 at the first flush after')
